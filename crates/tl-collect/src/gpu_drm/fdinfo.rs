//! DRM fdinfo: per-client engine busy time and memory (PERF-064, PERF-065, PERF-074, PERF-075,
//! ADR-016).
//!
//! Each fd open on `/dev/dri/*` has a `/proc/<pid>/fdinfo/<fd>` file with `drm-*` keys (kernel
//! `Documentation/gpu/drm-usage-stats.rst`). Only processes tasklens can read are covered; a
//! denied read sets `partial`, which the UI shows as "Own processes only". No path is logged.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use smallvec::SmallVec;
use tl_model::time::{TimeNs, now_boot_ns};

use crate::CollectError;
use crate::sysroot::{SysRoot, read_file};

/// fdinfo files are small; this bounds a read of a bad or huge file.
const FDINFO_LIMIT: usize = 16 * 1024;

const DRI_PREFIX: &str = "/dev/dri/";

/// Reads one fdinfo file into the buffer. A test hook replaces it to simulate EACCES.
type ReadFdinfo<'a> = dyn FnMut(&Path, &mut Vec<u8>) -> Result<(), CollectError> + 'a;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrmClient {
    pub pid: u32,
    /// `drm-driver`.
    pub driver: Arc<str>,
    /// `drm-pdev`, for example `0000:03:00.0`. Empty when the line is missing.
    pub pdev: Arc<str>,
    /// `drm-client-id`.
    pub client_id: u64,
    /// `drm-engine-<name>: <n> ns`.
    pub engine_ns: SmallVec<[(Arc<str>, u64); 8]>,
    /// `drm-cycles-<name>`.
    pub cycles: SmallVec<[(Arc<str>, u64); 8]>,
    /// `drm-total-cycles-<name>`.
    pub total_cycles: SmallVec<[(Arc<str>, u64); 8]>,
    /// `drm-resident-<region>`, in bytes.
    pub resident_bytes: SmallVec<[(Arc<str>, u64); 4]>,
    /// `drm-total-<region>`, in bytes.
    pub total_bytes: SmallVec<[(Arc<str>, u64); 4]>,
}

/// Parses one fdinfo file. None when there is no drm-driver line or no drm-client-id.
pub fn parse_fdinfo(pid: u32, bytes: &[u8]) -> Option<DrmClient> {
    parse_with(pid, bytes, &mut Interner::default())
}

#[derive(Clone, Debug, Default)]
pub struct FdinfoScan {
    pub clients: Vec<DrmClient>,
    /// True when some `/proc/<pid>/fd` or fdinfo read was denied (EACCES or EPERM).
    pub partial: bool,
    /// CLOCK_BOOTTIME read before the walk.
    pub t_ns: TimeNs,
}

/// Walks <proc>/<pid>/fd, keeps fds whose link target starts with "/dev/dri/", reads their fdinfo.
/// Clients are de-duplicated by (pdev, client_id); the first one wins.
pub fn scan(root: &SysRoot, out: &mut FdinfoScan) {
    let t_ns = now_boot_ns();
    scan_with(root, out, t_ns, &mut |path, buf| {
        read_file(path, buf, FDINFO_LIMIT).map(|_| ())
    });
}

/// `scan` with the clock value and the fdinfo reader passed in, so tests can simulate EACCES.
fn scan_with(root: &SysRoot, out: &mut FdinfoScan, t_ns: TimeNs, read_fdinfo: &mut ReadFdinfo<'_>) {
    out.clients.clear();
    out.partial = false;
    out.t_ns = t_ns;
    let Ok(pids) = fs::read_dir(&root.proc) else {
        return;
    };
    let mut interner = Interner::default();
    let mut buf = Vec::with_capacity(4096);
    let mut path = PathBuf::new();
    for entry in pids.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(parse_pid) else {
            continue;
        };
        path.clone_from(&root.proc);
        path.push(&name);
        path.push("fd");
        let fds = match fs::read_dir(&path) {
            Ok(fds) => fds,
            Err(e) => {
                note_error(&CollectError::from(e), &mut out.partial);
                continue;
            }
        };
        for fd in fds.flatten() {
            let fd_name = fd.file_name();
            let target = match fs::read_link(fd.path()) {
                Ok(target) => target,
                Err(e) => {
                    note_error(&CollectError::from(e), &mut out.partial);
                    continue;
                }
            };
            if !target
                .as_os_str()
                .as_encoded_bytes()
                .starts_with(DRI_PREFIX.as_bytes())
            {
                continue;
            }
            path.pop();
            path.push("fdinfo");
            path.push(&fd_name);
            let read = read_fdinfo(&path, &mut buf);
            path.pop();
            path.pop();
            path.push("fd");
            if let Err(e) = read {
                note_error(&e, &mut out.partial);
                continue;
            }
            let Some(client) = parse_with(pid, &buf, &mut interner) else {
                continue;
            };
            let seen = out
                .clients
                .iter()
                .any(|c| c.client_id == client.client_id && c.pdev == client.pdev);
            if !seen {
                out.clients.push(client);
            }
        }
    }
}

/// Denied reads make the scan partial. Gone pids and other errors are skipped silently.
fn note_error(e: &CollectError, partial: &mut bool) {
    if matches!(e, CollectError::PermissionDenied) {
        *partial = true;
    }
}

fn parse_pid(name: &str) -> Option<u32> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    name.parse().ok()
}

/// Busy percent per engine name for one pdev between two scans: sum over clients of
/// delta(engine_ns) / delta(t_ns) * 100, clamped to 0..=100. Clients present in only one scan add nothing.
pub fn engine_busy(
    prev: &FdinfoScan,
    cur: &FdinfoScan,
    pdev: &str,
    out: &mut SmallVec<[(Arc<str>, f32); 8]>,
) {
    out.clear();
    let dt = cur.t_ns.saturating_sub(prev.t_ns);
    if dt == 0 {
        return;
    }
    for (c, p) in matched_clients(prev, cur, pdev) {
        for (name, now) in &c.engine_ns {
            let delta = lookup(&p.engine_ns, name).map_or(0, |before| counter_delta(before, *now));
            add_share(out, name, delta as f64 / dt as f64);
        }
    }
    clamp_percent(out);
}

/// Same for xe cycles: delta(cycles) / delta(total_cycles) * 100 per engine, summed over clients, clamped.
pub fn engine_busy_cycles(
    prev: &FdinfoScan,
    cur: &FdinfoScan,
    pdev: &str,
    out: &mut SmallVec<[(Arc<str>, f32); 8]>,
) {
    out.clear();
    for (c, p) in matched_clients(prev, cur, pdev) {
        for (name, now) in &c.cycles {
            let delta = lookup(&p.cycles, name).map_or(0, |before| counter_delta(before, *now));
            let total = match (lookup(&c.total_cycles, name), lookup(&p.total_cycles, name)) {
                (Some(t_now), Some(t_before)) => counter_delta(t_before, t_now),
                _ => 0,
            };
            let share = if total == 0 {
                0.0
            } else {
                delta as f64 / total as f64
            };
            add_share(out, name, share);
        }
    }
    clamp_percent(out);
}

/// Pairs of (current, previous) clients on `pdev` with the same client id.
fn matched_clients<'a>(
    prev: &'a FdinfoScan,
    cur: &'a FdinfoScan,
    pdev: &'a str,
) -> impl Iterator<Item = (&'a DrmClient, &'a DrmClient)> {
    cur.clients
        .iter()
        .filter(move |c| &*c.pdev == pdev)
        .filter_map(move |c| {
            let p = prev
                .clients
                .iter()
                .find(|p| p.client_id == c.client_id && &*p.pdev == pdev)?;
            Some((c, p))
        })
}

/// A counter that went backwards adds nothing (ADR-008).
fn counter_delta(before: u64, now: u64) -> u64 {
    now.saturating_sub(before)
}

fn lookup(list: &[(Arc<str>, u64)], name: &str) -> Option<u64> {
    list.iter().find(|(n, _)| &**n == name).map(|&(_, v)| v)
}

/// Adds `share` (0.0 to 1.0 for one client) as percent to the entry for `name`.
fn add_share(out: &mut SmallVec<[(Arc<str>, f32); 8]>, name: &Arc<str>, share: f64) {
    let pct = (share * 100.0) as f32;
    match out.iter_mut().find(|(n, _)| n == name) {
        Some((_, v)) => *v += pct,
        None => out.push((Arc::clone(name), pct)),
    }
}

fn clamp_percent(out: &mut SmallVec<[(Arc<str>, f32); 8]>) {
    for (_, v) in out.iter_mut() {
        *v = if v.is_finite() {
            v.clamp(0.0, 100.0)
        } else {
            0.0
        };
    }
}

/// One `Arc<str>` per distinct name within a scan.
#[derive(Default)]
struct Interner {
    names: Vec<Arc<str>>,
}

impl Interner {
    fn get(&mut self, name: &str) -> Arc<str> {
        if let Some(found) = self.names.iter().find(|n| &***n == name) {
            return Arc::clone(found);
        }
        let new: Arc<str> = Arc::from(name);
        self.names.push(Arc::clone(&new));
        new
    }
}

fn parse_with(pid: u32, bytes: &[u8], interner: &mut Interner) -> Option<DrmClient> {
    let mut client = DrmClient {
        pid,
        ..DrmClient::default()
    };
    let mut has_driver = false;
    let mut has_client_id = false;
    for line in bytes.split(|&b| b == b'\n') {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let Some(key) = key.strip_prefix("drm-") else {
            continue;
        };
        let value = value.trim();
        match key {
            "driver" if !has_driver && !value.is_empty() => {
                client.driver = interner.get(value);
                has_driver = true;
            }
            "pdev" => client.pdev = interner.get(value),
            "client-id" if !has_client_id => {
                if let Ok(id) = value.parse() {
                    client.client_id = id;
                    has_client_id = true;
                }
            }
            _ => parse_counter(key, value, &mut client, interner),
        }
    }
    (has_driver && has_client_id).then_some(client)
}

/// Engine, cycle and memory keys. Unknown keys and bad values are skipped.
fn parse_counter(key: &str, value: &str, client: &mut DrmClient, interner: &mut Interner) {
    // `total-cycles-` must be checked before `total-`.
    if let Some(name) = key.strip_prefix("total-cycles-") {
        push_value(&mut client.total_cycles, name, value.parse().ok(), interner);
    } else if let Some(name) = key.strip_prefix("cycles-") {
        push_value(&mut client.cycles, name, value.parse().ok(), interner);
    } else if let Some(name) = key.strip_prefix("engine-") {
        let ns = value
            .strip_suffix("ns")
            .and_then(|n| n.trim_end().parse().ok());
        push_value(&mut client.engine_ns, name, ns, interner);
    } else if let Some(name) = key.strip_prefix("resident-") {
        push_value(
            &mut client.resident_bytes,
            name,
            parse_bytes(value),
            interner,
        );
    } else if let Some(name) = key.strip_prefix("total-") {
        push_value(&mut client.total_bytes, name, parse_bytes(value), interner);
    }
}

/// Keeps the first value for a name.
fn push_value<A: smallvec::Array<Item = (Arc<str>, u64)>>(
    list: &mut SmallVec<A>,
    name: &str,
    value: Option<u64>,
    interner: &mut Interner,
) {
    let Some(value) = value else {
        return;
    };
    if name.is_empty() || list.iter().any(|(n, _)| &**n == name) {
        return;
    }
    list.push((interner.get(name), value));
}

/// `<n> KiB`, `<n> MiB` or `<n>` (bytes).
fn parse_bytes(value: &str) -> Option<u64> {
    let (num, mult) = if let Some(n) = value.strip_suffix("KiB") {
        (n, 1024)
    } else if let Some(n) = value.strip_suffix("MiB") {
        (n, 1024 * 1024)
    } else {
        (value, 1)
    };
    num.trim_end().parse::<u64>().ok()?.checked_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> SysRoot {
        SysRoot::at(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drm-basic"))
    }

    fn real_read(path: &Path, buf: &mut Vec<u8>) -> Result<(), CollectError> {
        read_file(path, buf, FDINFO_LIMIT).map(|_| ())
    }

    fn get<T: Copy>(list: &[(Arc<str>, T)], name: &str) -> Option<T> {
        list.iter().find(|(n, _)| &**n == name).map(|&(_, v)| v)
    }

    fn client(id: u64, pdev: &str) -> DrmClient {
        DrmClient {
            pid: 1,
            driver: Arc::from("amdgpu"),
            pdev: Arc::from(pdev),
            client_id: id,
            ..DrmClient::default()
        }
    }

    const AMD: &str = "pos:\t0\n\
        drm-driver:\tamdgpu\n\
        drm-pdev:\t0000:03:00.0\n\
        drm-client-id:\t12\n\
        drm-engine-gfx:\t5000000 ns\n\
        drm-engine-compute:\t1000000 ns\n\
        drm-resident-vram:\t2048 KiB\n";

    #[test]
    fn parse_amd_client() {
        let c = parse_fdinfo(42, AMD.as_bytes()).expect("drm client");
        assert_eq!(c.pid, 42);
        assert_eq!(&*c.driver, "amdgpu");
        assert_eq!(&*c.pdev, "0000:03:00.0");
        assert_eq!(c.client_id, 12);
        assert_eq!(get(&c.engine_ns, "gfx"), Some(5_000_000));
        assert_eq!(get(&c.engine_ns, "compute"), Some(1_000_000));
        assert_eq!(get(&c.resident_bytes, "vram"), Some(2_097_152));
        assert!(c.cycles.is_empty() && c.total_cycles.is_empty() && c.total_bytes.is_empty());

        let units = "drm-driver: i915\ndrm-client-id: 1\n\
            drm-total-local0: 3 MiB\ndrm-resident-system0: 4096\n\
            drm-engine-capacity-vcs: 2\ndrm-engine-render: 7 ns\n";
        let c = parse_fdinfo(1, units.as_bytes()).expect("drm client");
        assert_eq!(get(&c.total_bytes, "local0"), Some(3 * 1024 * 1024));
        assert_eq!(get(&c.resident_bytes, "system0"), Some(4096));
        // The capacity line has no ns suffix, so it is not an engine time.
        assert_eq!(c.engine_ns.len(), 1);
        assert_eq!(get(&c.engine_ns, "render"), Some(7));
    }

    #[test]
    fn no_driver_none() {
        let text = AMD.replace("drm-driver:\tamdgpu\n", "");
        assert_eq!(parse_fdinfo(1, text.as_bytes()), None);
        let text = AMD.replace("drm-client-id:\t12\n", "");
        assert_eq!(parse_fdinfo(1, text.as_bytes()), None);
        assert_eq!(parse_fdinfo(1, b"pos:\t0\nflags:\t02\n"), None);
    }

    #[test]
    fn busy_from_ns_delta() {
        let pdev = "0000:03:00.0";
        let mut a = client(12, pdev);
        a.engine_ns.push((Arc::from("compute"), 1_000_000));
        a.engine_ns.push((Arc::from("gfx"), 900));
        let mut b = a.clone();
        b.engine_ns[0].1 += 250_000_000;
        // gfx went backwards: adds nothing.
        b.engine_ns[1].1 = 100;
        // Client 13 is only in the current scan: adds nothing.
        let mut only_cur = client(13, pdev);
        only_cur.engine_ns.push((Arc::from("compute"), 900_000_000));
        let prev = FdinfoScan {
            clients: vec![a],
            partial: false,
            t_ns: 10_000_000_000,
        };
        let cur = FdinfoScan {
            clients: vec![b, only_cur],
            partial: false,
            t_ns: 11_000_000_000,
        };
        let mut out = SmallVec::new();
        engine_busy(&prev, &cur, pdev, &mut out);
        assert_eq!(get(&out, "compute"), Some(25.0));
        assert_eq!(get(&out, "gfx"), Some(0.0));

        // Another pdev gives nothing.
        engine_busy(&prev, &cur, "0000:04:00.0", &mut out);
        assert!(out.is_empty());
        // Same time stamps give nothing.
        engine_busy(&cur, &cur, pdev, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn busy_clamped_to_100() {
        let pdev = "p";
        let mut a = client(1, pdev);
        a.engine_ns.push((Arc::from("gfx"), 0));
        let mut b = a.clone();
        b.engine_ns[0].1 = 3_000_000_000;
        let prev = FdinfoScan {
            clients: vec![a],
            partial: false,
            t_ns: 0,
        };
        let cur = FdinfoScan {
            clients: vec![b],
            partial: false,
            t_ns: 1_000_000_000,
        };
        let mut out = SmallVec::new();
        engine_busy(&prev, &cur, pdev, &mut out);
        assert_eq!(get(&out, "gfx"), Some(100.0));
    }

    #[test]
    fn dedupe_client_id() {
        let mut scan = FdinfoScan::default();
        scan_with(&fixture(), &mut scan, 5, &mut real_read);
        assert!(!scan.partial);
        assert_eq!(scan.t_ns, 5);
        // pid 1000 has fds 3 and 4 on client 12 and fd 5 on /dev/null; pid 1001 has one xe fd.
        assert_eq!(scan.clients.len(), 2, "{:?}", scan.clients);
        let amd = scan
            .clients
            .iter()
            .find(|c| c.client_id == 12)
            .expect("client 12");
        assert_eq!(amd.pid, 1000);
        assert_eq!(&*amd.pdev, "0000:03:00.0");
        assert_eq!(get(&amd.resident_bytes, "vram"), Some(2_097_152));
        assert_eq!(get(&amd.resident_bytes, "gtt"), Some(512));
        assert_eq!(get(&amd.total_bytes, "vram"), Some(4 * 1024 * 1024));
        // The fd read first wins; both fds of one process share the same client, so either
        // file's values belong to client 12. Fixture fd 3 has gfx 5000000, fd 4 has 9999.
        let gfx = get(&amd.engine_ns, "gfx");
        assert!(gfx == Some(5_000_000) || gfx == Some(9999), "{gfx:?}");
        let xe = scan
            .clients
            .iter()
            .find(|c| c.client_id == 7)
            .expect("client 7");
        assert_eq!(&*xe.driver, "xe");
        assert_eq!(get(&xe.cycles, "rcs"), Some(600));
        assert_eq!(get(&xe.total_cycles, "rcs"), Some(1000));
        assert_eq!(get(&xe.total_bytes, "system"), Some(8 * 1024 * 1024));

        // A second scan reuses the client vector.
        let cap = scan.clients.capacity();
        scan_with(&fixture(), &mut scan, 6, &mut real_read);
        assert_eq!(scan.clients.len(), 2);
        assert_eq!(scan.clients.capacity(), cap);
    }

    #[test]
    fn names_interned_per_scan() {
        let mut interner = Interner::default();
        let a = parse_with(1, AMD.as_bytes(), &mut interner).expect("client");
        let b = parse_with(2, AMD.as_bytes(), &mut interner).expect("client");
        assert!(Arc::ptr_eq(&a.driver, &b.driver));
        assert!(Arc::ptr_eq(&a.engine_ns[0].0, &b.engine_ns[0].0));
        // "vram" in resident and total is one Arc too.
        let c = parse_with(
            3,
            b"drm-driver: x\ndrm-client-id: 1\ndrm-total-vram: 1\n",
            &mut interner,
        )
        .expect("client");
        assert!(Arc::ptr_eq(&a.resident_bytes[0].0, &c.total_bytes[0].0));
    }

    #[test]
    fn partial_on_eacces() {
        let mut scan = FdinfoScan::default();
        scan_with(&fixture(), &mut scan, 1, &mut |path, buf| {
            if path.components().any(|c| c.as_os_str() == "1001") {
                Err(CollectError::PermissionDenied)
            } else {
                real_read(path, buf)
            }
        });
        assert!(scan.partial);
        assert_eq!(scan.clients.len(), 1);
        assert_eq!(scan.clients[0].client_id, 12);

        // A gone process is skipped without making the scan partial.
        scan_with(&fixture(), &mut scan, 2, &mut |path, buf| {
            if path.components().any(|c| c.as_os_str() == "1001") {
                Err(CollectError::NotFound)
            } else {
                real_read(path, buf)
            }
        });
        assert!(!scan.partial);
        assert_eq!(scan.clients.len(), 1);

        // No proc directory at all: empty and not partial.
        let missing = SysRoot::at(Path::new("/nonexistent/tasklens-fixture"));
        scan_with(&missing, &mut scan, 3, &mut real_read);
        assert!(scan.clients.is_empty() && !scan.partial);
    }

    #[test]
    fn xe_cycles_busy() {
        let pdev = "0000:00:02.0";
        let text = |cycles: u64, total: u64| {
            format!(
                "drm-driver: xe\ndrm-client-id: 7\ndrm-pdev: {pdev}\n\
                 drm-cycles-rcs: {cycles}\ndrm-total-cycles-rcs: {total}\n"
            )
        };
        let a = parse_fdinfo(1, text(600, 1000).as_bytes()).expect("client");
        let b = parse_fdinfo(1, text(900, 2000).as_bytes()).expect("client");
        let prev = FdinfoScan {
            clients: vec![a],
            partial: false,
            t_ns: 0,
        };
        let cur = FdinfoScan {
            clients: vec![b],
            partial: false,
            t_ns: 1_000_000_000,
        };
        let mut out = SmallVec::new();
        engine_busy_cycles(&prev, &cur, pdev, &mut out);
        assert_eq!(get(&out, "rcs"), Some(30.0));

        // Total cycles that did not move give 0, never a division by zero.
        engine_busy_cycles(&prev, &prev, pdev, &mut out);
        assert_eq!(get(&out, "rcs"), Some(0.0));
    }
}
