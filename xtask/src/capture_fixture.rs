//! `cargo xtask capture-fixture <name> [--force]`: copies the files tasklens reads from this
//! machine into `crates/tl-collect/tests/fixtures/<name>/` as a fake root, with personal data
//! scrubbed (ADR-013, ADR-015, R-162).

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::Context as _;

/// Options for [`capture`].
#[derive(Debug)]
pub struct CaptureOpts {
    /// How many processes to copy, lowest pids first.
    pub max_pids: usize,
    /// Replace an existing output directory.
    pub force: bool,
}

impl Default for CaptureOpts {
    fn default() -> Self {
        Self {
            max_pids: 50,
            force: false,
        }
    }
}

/// What [`capture`] copied and what it could not read.
#[derive(Debug)]
pub struct CaptureReport {
    /// Relative paths written to the fixture.
    pub copied: Vec<PathBuf>,
    /// Relative paths that exist but could not be read, with the reason.
    pub skipped: Vec<(PathBuf, String)>,
}

/// One allowlisted path, relative to the source root.
pub(crate) enum Wanted {
    /// A file whose scrubbed content is copied.
    File(PathBuf),
    /// A symlink recreated with its target text.
    Link(PathBuf),
    /// A directory recreated empty, with a `.keep` file.
    Marker(PathBuf),
}

/// Final path components that are never copied (ADR-013, ADR-015).
const FORBIDDEN_NAMES: &[&str] = &[
    "environ",
    "maps",
    "smaps",
    "smaps_rollup",
    "mem",
    "fd",
    "fdinfo",
    "cwd",
];

/// Basenames whose content is always replaced by `REDACTED\n`.
const SERIAL_NAMES: &[&str] = &[
    "serial",
    "wwid",
    "wwn",
    "eui",
    "nguid",
    "uuid",
    "product_serial",
    "board_serial",
    "chassis_serial",
    "product_uuid",
];

const PROC_FILES: &[&str] = &[
    "proc/stat",
    "proc/meminfo",
    "proc/vmstat",
    "proc/diskstats",
    "proc/net/dev",
    "proc/loadavg",
    "proc/sys/fs/file-nr",
    "proc/swaps",
    "proc/cpuinfo",
    "proc/sys/kernel/random/boot_id",
    "proc/sys/kernel/osrelease",
];

const PID_FILES: &[&str] = &["stat", "statm", "status", "io", "comm", "cgroup"];

const HWMON_FILES: &[&str] = &[
    "name",
    "tempN_input",
    "tempN_label",
    "fanN_input",
    "fanN_label",
    "fanN_max",
    "pwmN",
    "pwmN_enable",
];

/// Most symlinks followed while resolving one path, like the kernel's limit.
const MAX_LINK_HOPS: usize = 40;

/// CLI entry: `capture-fixture <name> [--force]`.
#[allow(clippy::print_stdout)]
pub fn run(args: &[String]) -> anyhow::Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask directory has no parent")?;
    let fixtures = workspace.join("crates/tl-collect/tests/fixtures");
    let report = run_with(args, &fixtures, Path::new("/"))?;
    println!(
        "copied {} files, skipped {}",
        report.copied.len(),
        report.skipped.len()
    );
    for (path, reason) in &report.skipped {
        println!("{}: {reason}", path.display());
    }
    Ok(())
}

/// [`run`] with the fixtures directory and source root given, so tests can point them at temp
/// directories.
fn run_with(args: &[String], fixtures: &Path, src_root: &Path) -> anyhow::Result<CaptureReport> {
    let mut name = None;
    let mut force = false;
    for arg in args {
        match arg.as_str() {
            "--force" => force = true,
            other if name.is_none() && !other.starts_with('-') => name = Some(other),
            _ => anyhow::bail!("usage: cargo xtask capture-fixture <name> [--force]"),
        }
    }
    let name = name.context("usage: cargo xtask capture-fixture <name> [--force]")?;
    let name_ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !name_ok {
        anyhow::bail!("fixture name must be lowercase letters, digits and dashes");
    }
    let opts = CaptureOpts {
        force,
        ..CaptureOpts::default()
    };
    capture(src_root, &fixtures.join(name), &opts)
}

/// Testable core: copies from `src_root` (normally "/") into `out_dir`.
pub fn capture(
    src_root: &Path,
    out_dir: &Path,
    opts: &CaptureOpts,
) -> anyhow::Result<CaptureReport> {
    if fs::symlink_metadata(out_dir).is_ok() {
        let name = out_dir
            .file_name()
            .unwrap_or(out_dir.as_os_str())
            .to_string_lossy();
        if !opts.force {
            anyhow::bail!("fixture {name} exists; pass --force to replace it");
        }
        fs::remove_dir_all(out_dir).with_context(|| format!("removing fixture {name}"))?;
    }
    fs::create_dir_all(out_dir).context("creating the fixture directory")?;

    let mut wanted: Vec<Wanted> = PROC_FILES
        .iter()
        .map(|p| Wanted::File(PathBuf::from(p)))
        .collect();
    wanted.extend(sys_wanted(src_root));
    let mut report = capture_entries(src_root, out_dir, &wanted)?;

    let ctx = Ctx::new(src_root, out_dir);
    ctx.copy_proc_self(&mut report)?;
    ctx.copy_pids(opts.max_pids, &mut report)?;

    write_capture_txt(&ctx, &report)?;
    Ok(report)
}

/// Copies the given allowlisted entries. [`capture`] calls it with the full allowlist; tests
/// call it with a few entries.
pub(crate) fn capture_entries(
    src_root: &Path,
    out_dir: &Path,
    wanted: &[Wanted],
) -> anyhow::Result<CaptureReport> {
    let ctx = Ctx::new(src_root, out_dir);
    let mut report = CaptureReport {
        copied: Vec::new(),
        skipped: Vec::new(),
    };
    for entry in wanted {
        ctx.copy_entry(entry, &mut report)?;
    }
    Ok(report)
}

/// Source root, output root and the scrub rules read from the source.
struct Ctx<'a> {
    src: &'a Path,
    out: &'a Path,
    users: Vec<(String, String)>,
    hostname: Option<String>,
}

impl<'a> Ctx<'a> {
    fn new(src: &'a Path, out: &'a Path) -> Self {
        let users = fs::read_to_string(src.join("etc/passwd"))
            .map(|p| user_map(&p))
            .unwrap_or_default();
        let hostname = fs::read_to_string(src.join("proc/sys/kernel/hostname"))
            .ok()
            .map(|h| h.trim().to_owned())
            .filter(|h| h.len() >= 3);
        Self {
            src,
            out,
            users,
            hostname,
        }
    }

    fn scrub(&self, bytes: &[u8]) -> Vec<u8> {
        scrub_bytes(bytes, &self.users, self.hostname.as_deref())
    }

    fn copy_entry(&self, entry: &Wanted, report: &mut CaptureReport) -> anyhow::Result<()> {
        let (rel, keep_last_link) = match entry {
            Wanted::File(p) | Wanted::Marker(p) => (p, false),
            Wanted::Link(p) => (p, true),
        };
        if is_forbidden(rel) {
            return Ok(());
        }
        let resolved = match self.resolve(rel, keep_last_link) {
            Ok(Some(path)) => path,
            Ok(None) => return Ok(()),
            Err(Resolve::Skip(reason)) => {
                report.skipped.push((rel.clone(), reason));
                return Ok(());
            }
            Err(Resolve::Fatal(e)) => return Err(e),
        };
        match entry {
            Wanted::Link(_) => {
                // `resolve` recreated the link when the final component was one.
                if self.out.join(&resolved).is_symlink() {
                    report.copied.push(rel.clone());
                }
            }
            Wanted::Marker(_) => {
                if self.src.join(&resolved).is_dir() {
                    let dir = self.out.join(&resolved);
                    fs::create_dir_all(&dir)
                        .with_context(|| format!("creating {}", resolved.display()))?;
                    fs::write(dir.join(".keep"), b"")
                        .with_context(|| format!("writing {}/.keep", resolved.display()))?;
                    report.copied.push(rel.clone());
                }
            }
            Wanted::File(_) => match fs::read(self.src.join(&resolved)) {
                Ok(bytes) => {
                    let content = if basename_is(rel, is_serial_file) {
                        b"REDACTED\n".to_vec()
                    } else if is_mac_address(rel) {
                        b"00:00:00:00:00:00\n".to_vec()
                    } else {
                        self.scrub(&bytes)
                    };
                    self.write_file(&resolved, &content)?;
                    report.copied.push(rel.clone());
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => report.skipped.push((rel.clone(), reason(&e))),
            },
        }
        Ok(())
    }

    /// Walks `rel` from the source root like the kernel resolves a path. Every symlink met on
    /// the way is recreated in the fixture with its original target text. Returns the canonical
    /// relative path, or the path of the final link when `keep_last_link` is set and the final
    /// component is a symlink. `Ok(None)` means the path does not exist.
    fn resolve(&self, rel: &Path, keep_last_link: bool) -> Result<Option<PathBuf>, Resolve> {
        let mut pending: VecDeque<OsString> = normal_components(rel).collect();
        let mut cur = PathBuf::new();
        let mut hops = 0;
        while let Some(comp) = pending.pop_front() {
            if comp == ".." {
                if !cur.pop() {
                    return Err(Resolve::Skip("symlink leaves the source root".to_owned()));
                }
                continue;
            }
            let cand = cur.join(&comp);
            let meta = match fs::symlink_metadata(self.src.join(&cand)) {
                Ok(meta) => meta,
                Err(e) if is_missing(&e) => return Ok(None),
                Err(e) => return Err(Resolve::Skip(reason(&e))),
            };
            // `proc/self` stays a real directory in the fixture, so `proc/net -> self/net`
            // resolves to `proc/self/net` there too.
            if !meta.file_type().is_symlink() || cand == Path::new("proc/self") {
                cur = cand;
                continue;
            }
            let target =
                fs::read_link(self.src.join(&cand)).map_err(|e| Resolve::Skip(reason(&e)))?;
            self.make_link(&cand, &target).map_err(Resolve::Fatal)?;
            if pending.is_empty() && keep_last_link {
                return Ok(Some(cand));
            }
            hops += 1;
            if hops > MAX_LINK_HOPS {
                return Err(Resolve::Skip(
                    "too many levels of symbolic links".to_owned(),
                ));
            }
            if target.is_absolute() {
                cur = PathBuf::new();
            }
            for (i, c) in target_components(&target).into_iter().enumerate() {
                pending.insert(i, c);
            }
        }
        Ok(Some(cur))
    }

    /// Recreates the symlink `rel` in the fixture, unless it is already there.
    fn make_link(&self, rel: &Path, target: &Path) -> anyhow::Result<()> {
        if is_forbidden(rel) {
            anyhow::bail!("refusing to write forbidden path {}", rel.display());
        }
        let path = self.out.join(rel);
        if fs::symlink_metadata(&path).is_ok() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating parent of {}", rel.display()))?;
        }
        std::os::unix::fs::symlink(target, &path)
            .with_context(|| format!("creating symlink {}", rel.display()))
    }

    /// Writes `content` at `rel` in the fixture, after the forbidden-path check.
    fn write_file(&self, rel: &Path, content: &[u8]) -> anyhow::Result<()> {
        if is_forbidden(rel) {
            anyhow::bail!("refusing to write forbidden path {}", rel.display());
        }
        let path = self.out.join(rel);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            anyhow::bail!("refusing to write through symlink {}", rel.display());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating parent of {}", rel.display()))?;
        }
        fs::write(&path, content).with_context(|| format!("writing {}", rel.display()))
    }

    /// Copies `proc/self/mountinfo` into a real `proc/self` directory and recreates the
    /// `proc/self/ns/net` symlink with its target text.
    fn copy_proc_self(&self, report: &mut CaptureReport) -> anyhow::Result<()> {
        let mountinfo = Path::new("proc/self/mountinfo");
        match fs::read(self.src.join(mountinfo)) {
            Ok(bytes) => {
                self.write_file(mountinfo, &self.scrub(&bytes))?;
                report.copied.push(mountinfo.to_path_buf());
            }
            Err(e) if is_missing(&e) => {}
            Err(e) => report.skipped.push((mountinfo.to_path_buf(), reason(&e))),
        }
        let net = Path::new("proc/self/ns/net");
        match fs::read_link(self.src.join(net)) {
            Ok(target) => {
                self.make_link(net, &target)?;
                report.copied.push(net.to_path_buf());
            }
            Err(e) if is_missing(&e) || e.kind() == io::ErrorKind::InvalidInput => {}
            Err(e) => report.skipped.push((net.to_path_buf(), reason(&e))),
        }
        Ok(())
    }

    /// Copies the per-process files of the `max_pids` lowest pids whose `stat` can be read.
    fn copy_pids(&self, max_pids: usize, report: &mut CaptureReport) -> anyhow::Result<()> {
        let mut pids: Vec<u64> = match fs::read_dir(self.src.join("proc")) {
            Ok(dir) => dir
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().to_str()?.parse().ok())
                .collect(),
            Err(e) if is_missing(&e) => return Ok(()),
            Err(e) => return Err(e).context("listing proc"),
        };
        pids.sort_unstable();
        let mut taken = 0;
        for pid in pids {
            if taken >= max_pids {
                break;
            }
            let dir = PathBuf::from(format!("proc/{pid}"));
            if fs::read(self.src.join(dir.join("stat"))).is_err() {
                continue;
            }
            taken += 1;
            for name in PID_FILES {
                self.copy_entry(&Wanted::File(dir.join(name)), report)?;
            }
            let cmdline = dir.join("cmdline");
            match fs::read(self.src.join(dir.join("comm"))) {
                Ok(comm) => {
                    let comm = comm.strip_suffix(b"\n").unwrap_or(&comm);
                    self.write_file(&cmdline, &scrubbed_cmdline(&self.scrub(comm)))?;
                    report.copied.push(cmdline);
                }
                Err(e) if is_missing(&e) => {}
                Err(e) => report.skipped.push((cmdline, reason(&e))),
            }
        }
        Ok(())
    }
}

/// Why [`Ctx::resolve`] stopped.
enum Resolve {
    /// The path exists but cannot be read; it goes to `skipped`.
    Skip(String),
    /// Writing to the fixture failed.
    Fatal(anyhow::Error),
}

/// Writes `CAPTURE.txt`: kernel release, tool name, then the copied and skipped paths. No date,
/// hostname or user name.
fn write_capture_txt(ctx: &Ctx<'_>, report: &CaptureReport) -> anyhow::Result<()> {
    let kernel = fs::read_to_string(ctx.src.join("proc/sys/kernel/osrelease"))
        .map(|k| scrub_text(k.trim(), &ctx.users, ctx.hostname.as_deref()))
        .unwrap_or_else(|_| "unknown".to_owned());
    let mut text = format!("kernel {kernel}\ncaptured-by cargo xtask capture-fixture\ncopied:\n");
    for path in &report.copied {
        text.push_str(&format!("{}\n", path.display()));
    }
    text.push_str("skipped:\n");
    for (path, why) in &report.skipped {
        text.push_str(&format!("{}: {why}\n", path.display()));
    }
    fs::write(ctx.out.join("CAPTURE.txt"), text).context("writing CAPTURE.txt")
}

/// The `/sys` allowlist, expanded against `src_root`.
fn sys_wanted(src_root: &Path) -> Vec<Wanted> {
    const CPU: &str = "sys/devices/system/cpu";
    let mut files: Vec<String> = Vec::new();
    let mut links: Vec<String> = Vec::new();
    let mut markers: Vec<String> = Vec::new();
    let with = |base: &str, leaves: &[&str]| -> Vec<String> {
        leaves.iter().map(|l| format!("{base}/{l}")).collect()
    };

    files.extend(with(CPU, &["online", "possible", "present"]));
    files.extend(with(
        &format!("{CPU}/cpuN"),
        &["online", "cpu_capacity", "acpi_cppc/nominal_freq"],
    ));
    files.extend(with(
        &format!("{CPU}/cpuN/topology"),
        &[
            "core_id",
            "physical_package_id",
            "die_id",
            "thread_siblings_list",
            "core_siblings_list",
        ],
    ));
    files.extend(with(
        &format!("{CPU}/cpuN/cache/indexN"),
        &["level", "type", "size", "shared_cpu_list"],
    ));
    files.extend(with(
        &format!("{CPU}/cpufreq/policyN"),
        &[
            "related_cpus",
            "affected_cpus",
            "cpuinfo_avg_freq",
            "scaling_cur_freq",
            "base_frequency",
            "bios_limit",
            "cpuinfo_max_freq",
            "cpuinfo_min_freq",
            "scaling_driver",
            "scaling_governor",
            "energy_performance_preference",
            "amd_pstate_prefcore_ranking",
        ],
    ));
    files.extend([
        "sys/devices/cpu_core/cpus".to_owned(),
        "sys/devices/cpu_atom/cpus".to_owned(),
        "sys/firmware/acpi/platform_profile".to_owned(),
    ]);

    files.extend(with("sys/class/hwmon/hwmonN", HWMON_FILES));
    links.push("sys/class/hwmon/hwmonN/device".to_owned());

    files.extend(with(
        "sys/block/*",
        &[
            "dev",
            "size",
            "removable",
            "ro",
            "stat",
            "queue/rotational",
            "device/model",
            "device/type",
            "dm/name",
        ],
    ));
    files.extend(with(
        "sys/block/*/*",
        &["dev", "size", "partition", "start"],
    ));
    links.extend([
        "sys/block/*/holders/*".to_owned(),
        "sys/block/*/slaves/*".to_owned(),
    ]);
    files.extend(with(
        "sys/block/zram*",
        &["disksize", "mm_stat", "comp_algorithm"],
    ));
    files.extend(with("sys/block/*/device/hwmonN", HWMON_FILES));
    files.extend(with("sys/block/*/device/hwmon/hwmonN", HWMON_FILES));

    files.extend(with(
        "sys/class/net/*",
        &[
            "ifindex",
            "type",
            "operstate",
            "carrier",
            "speed",
            "address",
            "tun_flags",
        ],
    ));
    links.extend(with(
        "sys/class/net/*",
        &["phy80211", "device", "device/driver"],
    ));
    markers.extend(with("sys/class/net/*", &["wireless", "bridge"]));

    files.extend(with(
        "sys/class/powercap/intel-rapl:*",
        &["name", "energy_uj", "max_energy_range_uj"],
    ));
    files.extend(with(
        "sys/module/zswap/parameters",
        &["enabled", "compressor", "max_pool_percent"],
    ));

    let mut wanted = Vec::new();
    for p in &files {
        wanted.extend(expand(src_root, p).into_iter().map(Wanted::File));
    }
    for p in &links {
        wanted.extend(expand(src_root, p).into_iter().map(Wanted::Link));
    }
    for p in &markers {
        wanted.extend(expand(src_root, p).into_iter().map(Wanted::Marker));
    }
    wanted
}

/// Expands a pattern whose components may hold `*` (any text) or `N` (a decimal number) into
/// the matching relative paths under `src_root`, sorted by name. Literal components are joined
/// without checking that they exist.
fn expand(src_root: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::new()];
    for comp in pattern.split('/') {
        if !comp.contains(['*', 'N']) {
            for p in &mut paths {
                p.push(comp);
            }
            continue;
        }
        let mut next = Vec::new();
        for p in &paths {
            let Ok(dir) = fs::read_dir(src_root.join(p)) else {
                continue;
            };
            let mut names: Vec<OsString> = dir
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .filter(|n| n.to_str().is_some_and(|n| component_matches(comp, n)))
                .collect();
            names.sort();
            next.extend(names.into_iter().map(|n| p.join(n)));
        }
        paths = next;
    }
    paths
}

/// Matches one name against a pattern component holding one `*` or one `N`.
fn component_matches(pattern: &str, name: &str) -> bool {
    if let Some((prefix, suffix)) = pattern.split_once('*') {
        return name.len() >= prefix.len() + suffix.len()
            && name.starts_with(prefix)
            && name.ends_with(suffix);
    }
    if let Some((prefix, suffix)) = pattern.split_once('N') {
        let Some(middle) = name
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
        else {
            return false;
        };
        return !middle.is_empty() && middle.bytes().all(|b| b.is_ascii_digit());
    }
    pattern == name
}

/// Maps every regular user (uid >= 1000, not 65534) to `user1`, `user2`, ... in uid order.
pub(crate) fn user_map(passwd: &str) -> Vec<(String, String)> {
    let mut users: Vec<(u32, &str)> = passwd
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let uid: u32 = fields.nth(1)?.parse().ok()?;
            (!name.is_empty() && uid >= 1000 && uid != 65534).then_some((uid, name))
        })
        .collect();
    users.sort_by_key(|&(uid, _)| uid);
    users
        .into_iter()
        .enumerate()
        .map(|(i, (_, name))| (name.to_owned(), format!("user{}", i + 1)))
        .collect()
}

/// Replaces whole-word user names and every occurrence of the hostname.
pub(crate) fn scrub_text(text: &str, users: &[(String, String)], hostname: Option<&str>) -> String {
    String::from_utf8_lossy(&scrub_bytes(text.as_bytes(), users, hostname)).into_owned()
}

/// [`scrub_text`] on raw bytes, so files with invalid UTF-8 keep their other bytes.
fn scrub_bytes(text: &[u8], users: &[(String, String)], hostname: Option<&str>) -> Vec<u8> {
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    'scan: while i < text.len() {
        if i == 0 || !is_word(text[i - 1]) {
            for (name, alias) in users {
                let name = name.as_bytes();
                let end = i + name.len();
                if text[i..].starts_with(name) && (end == text.len() || !is_word(text[end])) {
                    out.extend_from_slice(alias.as_bytes());
                    i = end;
                    continue 'scan;
                }
            }
        }
        out.push(text[i]);
        i += 1;
    }
    match hostname {
        Some(host) if host.len() >= 3 => replace_all(&out, host.as_bytes(), b"REDACTED"),
        _ => out,
    }
}

fn replace_all(text: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

/// The replacement for a process's `cmdline`: `<comm>\0--arg\0`.
pub(crate) fn scrubbed_cmdline(comm: &[u8]) -> Vec<u8> {
    let mut out = comm.to_vec();
    out.extend_from_slice(b"\0--arg\0");
    out
}

/// Whether a file with this basename holds a serial number or unique ID.
pub(crate) fn is_serial_file(basename: &str) -> bool {
    SERIAL_NAMES.contains(&basename)
}

fn basename_is(rel: &Path, pred: fn(&str) -> bool) -> bool {
    rel.file_name().and_then(OsStr::to_str).is_some_and(pred)
}

/// `sys/class/net/<iface>/address`.
fn is_mac_address(rel: &Path) -> bool {
    let comps: Vec<&OsStr> = rel.iter().collect();
    comps.len() == 5 && comps[..3] == ["sys", "class", "net"] && comps[4] == "address"
}

/// A path that must never be written: a forbidden final component, or any `fd` component.
fn is_forbidden(rel: &Path) -> bool {
    let last = rel.file_name().and_then(OsStr::to_str);
    last.is_some_and(|n| FORBIDDEN_NAMES.contains(&n)) || rel.iter().any(|c| c == "fd")
}

fn normal_components(path: &Path) -> impl Iterator<Item = OsString> + '_ {
    path.components().filter_map(|c| match c {
        Component::Normal(n) => Some(n.to_os_string()),
        _ => None,
    })
}

/// A symlink target's components, keeping `..` so resolution can apply them.
fn target_components(target: &Path) -> Vec<OsString> {
    target
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_os_string()),
            Component::ParentDir => Some(OsString::from("..")),
            _ => None,
        })
        .collect()
}

fn is_missing(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn reason(e: &io::Error) -> String {
    if e.kind() == io::ErrorKind::PermissionDenied {
        "permission denied".to_owned()
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("tasklens-capture-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn put(root: &Path, rel: &str, content: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// The fake source tree from the acceptance criteria.
    fn fake_src() -> TempDir {
        let src = TempDir::new();
        let r = &src.0;
        put(r, "proc/1/stat", b"1 (bash) S 0\n");
        put(r, "proc/1/statm", b"1 2 3 4 5 6 7\n");
        put(r, "proc/1/status", b"Name:\tbash\n");
        put(r, "proc/1/comm", b"bash\n");
        put(r, "proc/1/cmdline", b"bash\0-c\0secret\0");
        put(r, "proc/1/environ", b"HOME=/home/alice\0");
        put(r, "proc/1/maps", b"00400000-00452000 r-xp\n");
        put(r, "proc/stat", b"cpu 1 2 3 4\n");
        put(
            r,
            "etc/passwd",
            b"root:x:0:0:root:/root:/bin/bash\nbob:x:1001:1001::/home/bob:/bin/sh\n\
              alice:x:1000:1000::/home/alice:/bin/sh\nnobody:x:65534:65534::/:/bin/false\n",
        );
        put(r, "proc/sys/kernel/hostname", b"myhost\n");
        src
    }

    fn capture_fresh(src: &Path, opts: &CaptureOpts) -> (TempDir, PathBuf, CaptureReport) {
        let out = TempDir::new();
        let dir = out.0.join("fixture");
        let report = capture(src, &dir, opts).unwrap();
        (out, dir, report)
    }

    #[test]
    fn never_copies_environ_or_maps() {
        let src = fake_src();
        let (_out, dir, _) = capture_fresh(&src.0, &CaptureOpts::default());
        assert!(dir.join("proc/1/stat").is_file());
        assert!(dir.join("proc/1/cmdline").is_file());
        assert!(!dir.join("proc/1/environ").exists());
        assert!(!dir.join("proc/1/maps").exists());
        assert!(!dir.join("proc/sys/kernel/hostname").exists());
        assert!(is_forbidden(Path::new("proc/1/fd/3")));
        assert!(is_forbidden(Path::new("proc/1/smaps_rollup")));
    }

    #[test]
    fn cmdline_is_scrubbed() {
        let src = fake_src();
        let (_out, dir, _) = capture_fresh(&src.0, &CaptureOpts::default());
        assert_eq!(
            fs::read(dir.join("proc/1/cmdline")).unwrap(),
            b"bash\0--arg\0"
        );
        assert_eq!(scrubbed_cmdline(b"bash"), b"bash\0--arg\0");
    }

    #[test]
    fn users_and_hostname_scrubbed() {
        let users = user_map(&fs::read_to_string(fake_src().0.join("etc/passwd")).unwrap());
        assert_eq!(
            users,
            vec![
                ("alice".to_owned(), "user1".to_owned()),
                ("bob".to_owned(), "user2".to_owned())
            ]
        );
        assert_eq!(
            scrub_text("/home/alice/x bob alicex myhost", &users, Some("myhost")),
            "/home/user1/x user2 alicex REDACTED"
        );

        let src = fake_src();
        put(&src.0, "proc/stat", b"/home/alice/x bob alicex myhost");
        let (_out, dir, _) = capture_fresh(&src.0, &CaptureOpts::default());
        assert_eq!(
            fs::read_to_string(dir.join("proc/stat")).unwrap(),
            "/home/user1/x user2 alicex REDACTED"
        );
        let capture_txt = fs::read_to_string(dir.join("CAPTURE.txt")).unwrap();
        assert!(!capture_txt.contains("myhost"));
    }

    #[test]
    fn serial_files_redacted() {
        let src = fake_src();
        put(&src.0, "sys/block/sda/device/serial", b"WD-ABC123\n");
        put(&src.0, "sys/class/net/eth0/address", b"aa:bb:cc:dd:ee:ff\n");
        let out = TempDir::new();
        let wanted = [
            Wanted::File("sys/block/sda/device/serial".into()),
            Wanted::File("sys/class/net/eth0/address".into()),
        ];
        let report = capture_entries(&src.0, &out.0, &wanted).unwrap();
        assert_eq!(report.copied.len(), 2);
        assert_eq!(
            fs::read(out.0.join("sys/block/sda/device/serial")).unwrap(),
            b"REDACTED\n"
        );
        assert_eq!(
            fs::read(out.0.join("sys/class/net/eth0/address")).unwrap(),
            b"00:00:00:00:00:00\n"
        );
        assert!(is_serial_file("wwid"));
        assert!(!is_serial_file("model"));
    }

    #[test]
    fn pid_limit_is_lowest_50() {
        let src = TempDir::new();
        for pid in 1..=60 {
            put(&src.0, &format!("proc/{pid}/stat"), b"x\n");
        }
        let opts = CaptureOpts {
            max_pids: 50,
            force: false,
        };
        let (_out, dir, _) = capture_fresh(&src.0, &opts);
        for pid in 1..=50 {
            assert!(dir.join(format!("proc/{pid}/stat")).is_file(), "pid {pid}");
        }
        for pid in 51..=60 {
            assert!(!dir.join(format!("proc/{pid}")).exists(), "pid {pid}");
        }
    }

    #[test]
    fn symlinks_recreated() {
        let src = fake_src();
        put(&src.0, "sys/devices/x/block/sda/size", b"1000\n");
        fs::create_dir_all(src.0.join("sys/block")).unwrap();
        symlink("../devices/x/block/sda", src.0.join("sys/block/sda")).unwrap();
        put(&src.0, "proc/7/net/dev", b"eth0: 1 2\n");
        put(&src.0, "proc/7/mountinfo", b"1 2 0:1 / /home/alice rw\n");
        symlink("7", src.0.join("proc/self")).unwrap();
        symlink("self/net", src.0.join("proc/net")).unwrap();
        let (_out, dir, _) = capture_fresh(&src.0, &CaptureOpts::default());
        assert!(!dir.join("proc/self").is_symlink());
        assert_eq!(
            fs::read_link(dir.join("proc/net")).unwrap(),
            Path::new("self/net")
        );
        assert_eq!(fs::read(dir.join("proc/net/dev")).unwrap(), b"eth0: 1 2\n");
        assert_eq!(
            fs::read(dir.join("proc/self/mountinfo")).unwrap(),
            b"1 2 0:1 / /home/user1 rw\n"
        );
        assert_eq!(
            fs::read_link(dir.join("sys/block/sda")).unwrap(),
            Path::new("../devices/x/block/sda")
        );
        assert!(dir.join("sys/devices/x/block/sda").is_dir());
        assert_eq!(
            fs::read(dir.join("sys/devices/x/block/sda/size")).unwrap(),
            b"1000\n"
        );
        assert_eq!(fs::read(dir.join("sys/block/sda/size")).unwrap(), b"1000\n");
    }

    #[test]
    fn unreadable_file_is_skipped() {
        let src = fake_src();
        let io_path = src.0.join("proc/1/io");
        fs::write(&io_path, b"rchar: 1\n").unwrap();
        fs::set_permissions(&io_path, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(&io_path).is_ok() {
            // Running as root: the file stays readable, so there is nothing to test.
            return;
        }
        let (_out, dir, report) = capture_fresh(&src.0, &CaptureOpts::default());
        assert!(!dir.join("proc/1/io").exists());
        assert!(
            report
                .skipped
                .contains(&(PathBuf::from("proc/1/io"), "permission denied".to_owned()))
        );
        let capture_txt = fs::read_to_string(dir.join("CAPTURE.txt")).unwrap();
        let skipped = capture_txt.split("skipped:\n").nth(1).unwrap();
        assert!(skipped.contains("proc/1/io"));
    }

    #[test]
    fn bad_name_rejected() {
        let err = run(&["Owner_Desktop".to_owned()]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "fixture name must be lowercase letters, digits and dashes"
        );
    }

    #[test]
    fn existing_fixture_needs_force() {
        let src = fake_src();
        let fixtures = TempDir::new();
        put(&fixtures.0, "owner-desktop/keep.txt", b"old\n");
        let err = run_with(&["owner-desktop".to_owned()], &fixtures.0, &src.0).unwrap_err();
        assert_eq!(
            err.to_string(),
            "fixture owner-desktop exists; pass --force to replace it"
        );
        assert_eq!(
            fs::read(fixtures.0.join("owner-desktop/keep.txt")).unwrap(),
            b"old\n"
        );

        let args = ["owner-desktop".to_owned(), "--force".to_owned()];
        run_with(&args, &fixtures.0, &src.0).unwrap();
        assert!(!fixtures.0.join("owner-desktop/keep.txt").exists());
        assert!(fixtures.0.join("owner-desktop/proc/1/stat").is_file());
    }
}
