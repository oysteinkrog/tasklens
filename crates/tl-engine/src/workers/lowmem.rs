//! Stripped process sweep for `EngineMode::LowMemory` (ADR-004, PROC-098): `stat`, `status`
//! and cgroup `memory.current` every 2 s, plus the memory pressure state.
//!
//! One sweep reads `stat`, `status` and `cgroup` of every pid relative to the `/proc` dirfd, the
//! `memory.current` of each app unit through the `/sys/fs/cgroup` dirfd (R-137), `/proc/meminfo`
//! and `/proc/pressure/memory`. No `smaps`, fd count, exe, command line, icon or desktop file is
//! read. Records hold names and cgroup paths: this module logs only counts (ADR-015).

use std::fmt::Write as _;
use std::mem::MaybeUninit;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustc_hash::FxHashMap;
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::{AtFlags, Mode, OFlags, RawDir};
use smallvec::smallvec;
use tl_collect::SysRoot;
use tl_collect::apps::cgroup_app::parse_app_unit;
use tl_collect::apps::classify::{
    ClassifyInput, ProtectInputs, classify, flags_for, protected_set, wayland_compositor_pid,
};
use tl_collect::apps::naming::search_lower;
use tl_collect::parse::meminfo::{MemInfo, parse_meminfo};
use tl_collect::proc::cgroup::{CgroupPath, app_slice_unit, parse_cgroup_file};
use tl_collect::proc::meta::{PasswdUserNames, UserNames};
use tl_collect::proc::pid_stat::{PF_KTHREAD, PidStat, parse_pid_stat};
use tl_collect::proc::status::{PidStatus, STATUS_READ_MAX, parse_status};
use tl_collect::sanitize::{sanitize_comm, sanitize_text};
use tl_collect::sysroot::{SMALL_FILE_LIMIT, read_at, read_file};
use tl_model::actions::CgroupRef;
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{
    GroupKind, LowMemGroup, LowMemoryInfo, ProcFlags, ProcState, ProcessMeta, ProcessRecord,
};
use tl_model::settings::EngineSettings;
use tl_model::time::{BootId, TimeNs, now_boot_ns};
use tl_model::validity::{FieldValidity, ProcessField, Validity};

use crate::source::Source;
use crate::update::{ProcessSweep, SourceUpdate};

/// One sweep every 2 s, whatever the demand (PROC-098).
pub const LOWMEM_PERIOD: Duration = Duration::from_secs(2);
/// Critical when the `full` stall share over the interval is above this (R-062).
pub const LOWMEM_PSI_FULL_PCT: f32 = 10.0;
/// Critical when `MemAvailable` is below this share of `MemTotal` (R-062).
pub const LOWMEM_MEMAVAIL_MIN_PCT: u64 = 3;

/// getdents buffer for the `<proc>` directory listing.
const DIRENT_BUF: usize = 32 * 1024;
/// Largest `/proc/meminfo` kept. The real file is about 1.5 KB.
const MEMINFO_LIMIT: usize = 4 * SMALL_FILE_LIMIT;
/// Longest app name kept, in bytes.
const APP_NAME_MAX: usize = 256;

/// One app unit of this sweep.
struct Unit {
    rel: Arc<str>,
    group: GroupKey,
    name: Arc<str>,
    process_count: u32,
}

/// Buffers reused across sweeps.
#[derive(Default)]
struct Scratch {
    buf: Vec<u8>,
    path: String,
    text: String,
    search: String,
    stat: PidStat,
    status: PidStatus,
    meminfo: MemInfo,
    records: Vec<ProcessRecord>,
    /// The cgroup of each record, at the same index, for the protected set.
    cgroups: Vec<Option<CgroupPath>>,
    units: Vec<Unit>,
    unit_index: FxHashMap<Arc<str>, usize>,
}

/// The `Processes` source in `EngineMode::LowMemory` (E32-006).
pub struct LowMemSource {
    root: SysRoot,
    /// Read at `new`, or at the first sweep that can read it. Without it no `ProcessKey` can
    /// match the full sweep (U-50), so the sweep reports Failed.
    boot_id: Option<BootId>,
    /// `<sys>/fs/cgroup`, opened at `new` or at the first sweep that can open it (R-137).
    cgroup_fd: Option<OwnedFd>,
    self_uid: u32,
    protect: ProtectInputs,
    users: Box<dyn UserNames + Send>,
    /// `full` `total` of `/proc/pressure/memory` in µs and the time it was read.
    psi_prev: Option<(u64, TimeNs)>,
    clock: fn() -> TimeNs,
    /// The meta of each key from the last sweep, so an unchanged process keeps the same `Arc`.
    metas: FxHashMap<ProcessKey, Arc<ProcessMeta>>,
    next_metas: FxHashMap<ProcessKey, Arc<ProcessMeta>>,
    dirents: Vec<MaybeUninit<u8>>,
    scratch: Scratch,
}

impl LowMemSource {
    /// The user names come from `<root>/etc/passwd`, where `<root>` is the parent of
    /// `root.proc`. Never fails: a boot id or cgroup root that cannot be read now is tried
    /// again at each sweep.
    pub fn new(root: SysRoot) -> Self {
        let base = root.proc.parent().unwrap_or(Path::new("/"));
        let users = PasswdUserNames::load(&base.join("etc/passwd"));
        let protect = ProtectInputs {
            self_pids: smallvec![
                rustix::process::getpid()
                    .as_raw_nonzero()
                    .get()
                    .unsigned_abs()
            ],
            compositor_pid: host_compositor_pid(),
            session_bus_pid: None,
        };
        Self {
            boot_id: read_boot_id(&root),
            cgroup_fd: open_cgroup_root(&root),
            root,
            self_uid: rustix::process::getuid().as_raw(),
            protect,
            users: Box::new(users),
            psi_prev: None,
            clock: now_boot_ns,
            metas: FxHashMap::default(),
            next_metas: FxHashMap::default(),
            dirents: vec![MaybeUninit::uninit(); DIRENT_BUF],
            scratch: Scratch::default(),
        }
    }

    /// Test hook: the sweep runs as `uid`.
    #[doc(hidden)]
    pub fn with_self_uid(mut self, uid: u32) -> Self {
        self.self_uid = uid;
        self
    }

    /// Test hook: `clock` returns CLOCK_BOOTTIME ns in place of the real clock.
    #[doc(hidden)]
    pub fn with_clock(mut self, clock: fn() -> TimeNs) -> Self {
        self.clock = clock;
        self
    }

    /// Reads every process into `scratch.records` and `scratch.cgroups`. `Err` when `<proc>`
    /// cannot be listed.
    fn sweep_procs(&mut self, boot_id: BootId) -> Result<(), rustix::io::Errno> {
        let proc_fd = rustix::fs::open(
            &self.root.proc,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let mut dir = RawDir::new(proc_fd.as_fd(), &mut self.dirents[..]);
        while let Some(entry) = dir.next() {
            let Ok(entry) = entry else { break };
            let Some(pid) = parse_pid(entry.file_name().to_bytes()) else {
                continue;
            };
            sweep_one(
                proc_fd.as_fd(),
                pid,
                boot_id,
                self.self_uid,
                &mut *self.users,
                &self.metas,
                &mut self.next_metas,
                &mut self.scratch,
            );
        }
        Ok(())
    }

    /// The protected set from this sweep's records, then every record's flags.
    fn set_flags(&mut self) {
        let s = &mut self.scratch;
        let set = {
            let rows: Vec<(u32, u32, &str, Option<&CgroupPath>, u32)> = s
                .records
                .iter()
                .zip(&s.cgroups)
                .map(|(r, cgroup)| (r.key.pid, r.ppid, &*r.meta.comm, cgroup.as_ref(), r.uid))
                .collect();
            protected_set(&rows, self.self_uid, &self.protect)
        };
        for r in &mut s.records {
            let kernel_thread = r.flags.contains(ProcFlags::KERNEL_THREAD);
            r.flags = flags_for(
                r.key.pid,
                kernel_thread,
                r.uid,
                self.self_uid,
                &set,
                &self.protect,
            );
        }
    }

    /// One `LowMemGroup` per app unit whose cgroup directory still exists: the directory is
    /// opened from the cgroup root, `fstat`ed for its inode, its `memory.current` read, and
    /// closed (R-137).
    fn app_groups(&mut self) -> Vec<LowMemGroup> {
        if self.cgroup_fd.is_none() {
            self.cgroup_fd = open_cgroup_root(&self.root);
        }
        let Some(root) = self.cgroup_fd.as_ref() else {
            return Vec::new();
        };
        let s = &mut self.scratch;
        let mut groups = Vec::with_capacity(s.units.len());
        for unit in &s.units {
            let Ok(dir) = rustix::fs::openat(
                root,
                &*unit.rel,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            ) else {
                continue;
            };
            let Ok(st) = rustix::fs::fstat(&dir) else {
                continue;
            };
            let Some(memory_current) = read_at(dir.as_fd(), "memory.current", &mut s.buf, 64)
                .ok()
                .and_then(|_| std::str::from_utf8(s.buf.trim_ascii()).ok()?.parse().ok())
            else {
                continue;
            };
            groups.push(LowMemGroup {
                group: unit.group,
                name: unit.name.clone(),
                process_count: unit.process_count,
                uid: self.self_uid,
                cgroup: CgroupRef {
                    rel_path: unit.rel.clone(),
                    cgroup_id: st.st_ino,
                },
                memory_current,
            });
        }
        groups
    }

    /// `MemTotal`, `MemAvailable`, the `full` stall share since the last sweep and the R-062
    /// rule. A missing pressure file gives `None` and the rule uses `MemAvailable` only.
    fn pressure(&mut self, now: TimeNs) -> (u64, u64, Option<f32>, bool) {
        let s = &mut self.scratch;
        let (mem_total, mem_available) =
            match read_file(&self.root.proc_path("meminfo"), &mut s.buf, MEMINFO_LIMIT)
                .and_then(|_| parse_meminfo(&s.buf, &mut s.meminfo))
            {
                Ok(()) => (s.meminfo.mem_total, s.meminfo.mem_available),
                Err(_) => (0, 0),
            };

        let total = read_file(
            &self.root.proc_path("pressure/memory"),
            &mut s.buf,
            SMALL_FILE_LIMIT,
        )
        .ok()
        .and_then(|_| psi_full_total(&s.buf));
        let psi_full_pct = match (self.psi_prev, total) {
            (Some((was, then)), Some(total)) if now > then && total >= was => {
                let elapsed_us = (now - then) as f64 / 1e3;
                Some((100.0 * (total - was) as f64 / elapsed_us) as f32)
            }
            _ => None,
        };
        self.psi_prev = total.map(|t| (t, now));

        let low_avail = mem_total > 0
            && u128::from(mem_available) * 100
                < u128::from(mem_total) * u128::from(LOWMEM_MEMAVAIL_MIN_PCT);
        let critical = low_avail || psi_full_pct.is_some_and(|p| p > LOWMEM_PSI_FULL_PCT);
        (mem_total, mem_available, psi_full_pct, critical)
    }
}

impl Source for LowMemSource {
    fn id(&self) -> SourceId {
        SourceId::Processes
    }

    /// Always `Some(LOWMEM_PERIOD)`: the rescue window is always visible and ignores the hidden
    /// policy and `processes.interval_ms`.
    fn period(&self, _: &Demand, _: &EngineSettings) -> Option<Duration> {
        Some(LOWMEM_PERIOD)
    }

    fn sample(mut self) -> (Self, SourceUpdate) {
        if self.boot_id.is_none() {
            self.boot_id = read_boot_id(&self.root);
        }
        let Some(boot_id) = self.boot_id else {
            return (self, failed("cannot read boot id"));
        };
        let observed_start = (self.clock)();
        let s = &mut self.scratch;
        s.records.clear();
        s.cgroups.clear();
        s.units.clear();
        s.unit_index.clear();
        self.next_metas.clear();

        if self.sweep_procs(boot_id).is_err() {
            return (self, failed("cannot read /proc"));
        }
        std::mem::swap(&mut self.metas, &mut self.next_metas);
        self.set_flags();
        let groups = self.app_groups();
        let observed_end = (self.clock)();
        let (mem_total, mem_available, psi_full_pct, critical) = self.pressure(observed_end);

        let mut records = std::mem::take(&mut self.scratch.records);
        records.sort_unstable_by_key(|r| r.key);
        tracing::debug!(
            count = records.len(),
            groups = groups.len(),
            "low-memory sweep"
        );
        let system_threads = records
            .iter()
            .fold(0u32, |sum, r| sum.saturating_add(r.threads));
        let sweep = ProcessSweep {
            observed_start,
            observed_end,
            records,
            extras: Vec::new(),
            extras_p2: Vec::new(),
            app_stats: Vec::new(),
            unit_stats: Vec::new(),
            system_threads,
            file_nr: None,
            online_cpus: 0,
            low_memory: Some(Arc::new(LowMemoryInfo {
                groups: Arc::from(groups),
                mem_total,
                mem_available,
                psi_full_pct,
                critical,
            })),
        };
        (self, SourceUpdate::Processes(sweep))
    }

    fn reset_baselines(&mut self) {
        self.psi_prev = None;
    }
}

/// Reads one process into `s.records` and `s.cgroups`. A pid whose `stat` cannot be read is
/// skipped: it has no key.
#[allow(clippy::too_many_arguments, reason = "split borrows of LowMemSource")]
fn sweep_one(
    proc_fd: BorrowedFd<'_>,
    pid: u32,
    boot_id: BootId,
    self_uid: u32,
    users: &mut (dyn UserNames + Send),
    metas: &FxHashMap<ProcessKey, Arc<ProcessMeta>>,
    next_metas: &mut FxHashMap<ProcessKey, Arc<ProcessMeta>>,
    s: &mut Scratch,
) {
    let stat_read = read_at(
        proc_fd,
        pid_path(&mut s.path, pid, "stat"),
        &mut s.buf,
        SMALL_FILE_LIMIT,
    )
    .and_then(|_| parse_pid_stat(&s.buf, &mut s.stat));
    if stat_read.is_err() {
        tracing::trace!(pid, "stat not read, process skipped");
        return;
    }
    // The same key the full sweep builds (U-50).
    let key = ProcessKey {
        start_ticks: s.stat.start_ticks,
        boot_id,
        pid,
    };
    let kernel_thread = s.stat.flags & PF_KTHREAD != 0;

    // Every field this mode does not read stays 0 with Unsupported, never a measured zero.
    let mut validity = FieldValidity::all(Validity::Unsupported);
    let status_read = read_at(
        proc_fd,
        pid_path(&mut s.path, pid, "status"),
        &mut s.buf,
        STATUS_READ_MAX,
    )
    .and_then(|_| parse_status(&s.buf, &mut s.status));
    let (uid, threads, rss_anon, swap) = match status_read {
        Ok(()) => {
            let present = |v: Option<u64>| {
                if v.is_some() {
                    Validity::Valid
                } else {
                    Validity::Unsupported
                }
            };
            validity.set(ProcessField::RssAnon, present(s.status.rss_anon));
            validity.set(ProcessField::Swap, present(s.status.vm_swap));
            (
                s.status.uid[0],
                s.status.threads,
                s.status.rss_anon.unwrap_or(0),
                s.status.vm_swap.unwrap_or(0),
            )
        }
        Err(e) => {
            validity.set(ProcessField::RssAnon, e.validity());
            validity.set(ProcessField::Swap, e.validity());
            // The owner of `/proc/<pid>` is the process's uid, so OTHER_USER stays right.
            let Ok(st) =
                rustix::fs::statat(proc_fd, pid_path(&mut s.path, pid, ""), AtFlags::empty())
            else {
                return;
            };
            (st.st_uid, s.stat.num_threads, 0, 0)
        }
    };
    validity.set(ProcessField::Threads, Validity::Valid);

    let (cgroup, cgroup_denied) = match read_at(
        proc_fd,
        pid_path(&mut s.path, pid, "cgroup"),
        &mut s.buf,
        SMALL_FILE_LIMIT,
    ) {
        Ok(_) => (parse_cgroup_file(&s.buf), false),
        Err(e) => (None, e.validity() == Validity::Restricted),
    };
    let unit = cgroup.as_ref().and_then(|c| {
        let name = app_slice_unit(c, self_uid)?;
        let id = parse_app_unit(name)?;
        // `user.slice/user-<uid>.slice/user@<uid>.service/app.slice/<unit>`: five components.
        let end = c.rel.split('/').take(5).map(|p| p.len() + 1).sum::<usize>() - 1;
        Some((Arc::<str>::from(&c.rel[..end]), id.app_id))
    });
    let (_, kind) = classify(
        &ClassifyInput {
            pid,
            ruid: uid,
            kernel_thread,
            cgroup: cgroup.as_ref(),
            cgroup_denied,
            app_id: unit.as_ref().map(|(_, id)| id),
            owns_window: false,
        },
        self_uid,
    );
    let group = kind.key();
    if let (GroupKind::App { .. }, Some((rel, app_id))) = (&kind, unit) {
        let i = *s.unit_index.entry(rel.clone()).or_insert_with(|| {
            sanitize_text(app_id.as_bytes(), APP_NAME_MAX, &mut s.text);
            s.units.push(Unit {
                rel,
                group,
                name: Arc::from(s.text.as_str()),
                process_count: 0,
            });
            s.units.len() - 1
        });
        s.units[i].process_count += 1;
    }

    // Meta: sanitized comm as the name, no exe, command line, icon or description (R-152 OI-8).
    sanitize_comm(&s.stat.comm_raw, &mut s.text);
    let user = users.name(uid);
    let rel = cgroup.as_ref().map(|c| &c.rel);
    let app_id = match &kind {
        GroupKind::App { app_id } => Some(app_id),
        _ => None,
    };
    let meta = match metas.get(&key) {
        Some(m)
            if *m.comm == *s.text
                && Arc::ptr_eq(&m.user, &user)
                && m.cgroup.as_ref() == rel
                && m.app_id.as_ref() == app_id =>
        {
            m.clone()
        }
        _ => {
            let comm: Arc<str> = Arc::from(s.text.as_str());
            search_lower(
                &[&*comm, &*user, app_id.map_or("", |a| &**a)],
                &mut s.search,
            );
            Arc::new(ProcessMeta {
                name: comm.clone(),
                comm,
                exe: None,
                cmdline: None,
                user,
                cgroup: rel.cloned(),
                app_id: app_id.cloned(),
                icon: None,
                description: None,
                search_lower: Arc::from(s.search.as_str()),
            })
        }
    };
    next_metas.insert(key, meta.clone());

    let mut flags = ProcFlags::empty();
    flags.set(ProcFlags::KERNEL_THREAD, kernel_thread);
    s.records.push(ProcessRecord {
        key,
        ppid: s.stat.ppid,
        uid,
        meta,
        group,
        state: ProcState::from_stat_char(s.stat.state),
        flags,
        nice: s.stat.nice,
        threads,
        cpu_pct: 0.0,
        rss_anon_bytes: rss_anon,
        rss_bytes: 0,
        pss_bytes: 0,
        uss_bytes: 0,
        swap_bytes: swap,
        io_read_bps: 0.0,
        io_write_bps: 0.0,
        net_rx_bps: 0.0,
        net_tx_bps: 0.0,
        gpu_util_pct: 0.0,
        gpu_mem_bytes: 0,
        validity,
    });
    s.cgroups.push(cgroup);
}

/// The `Health` update of a sweep that could not run.
fn failed(error: &str) -> SourceUpdate {
    SourceUpdate::Health {
        id: SourceId::Processes,
        state: SourceState::Failed,
        last_error: Some(error.into()),
        t_ns: now_boot_ns(),
    }
}

/// Sets `path` to `"<pid>/<file>"`, or `"<pid>"` when `file` is empty.
fn pid_path<'p>(path: &'p mut String, pid: u32, file: &str) -> &'p str {
    path.clear();
    let _ = write!(path, "{pid}");
    if !file.is_empty() {
        path.push('/');
        path.push_str(file);
    }
    path
}

/// A directory name made of ASCII digits only that fits a `u32`.
fn parse_pid(name: &[u8]) -> Option<u32> {
    if name.is_empty() || !name.iter().all(u8::is_ascii_digit) {
        return None;
    }
    name.iter().try_fold(0u32, |n, &d| {
        n.checked_mul(10)?.checked_add(u32::from(d - b'0'))
    })
}

/// The `total=` value of the `full` line of `/proc/pressure/memory`, in µs.
fn psi_full_total(buf: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(buf).ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("full "))?;
    line.split_ascii_whitespace()
        .find_map(|kv| kv.strip_prefix("total="))?
        .parse()
        .ok()
}

/// `<proc>/sys/kernel/random/boot_id`, as the full sweep reads it.
fn read_boot_id(root: &SysRoot) -> Option<BootId> {
    let text = std::fs::read_to_string(root.proc_path("sys/kernel/random/boot_id")).ok()?;
    BootId::from_uuid_text(&text)
}

/// `<sys>/fs/cgroup` as an `O_DIRECTORY` fd.
fn open_cgroup_root(root: &SysRoot) -> Option<OwnedFd> {
    rustix::fs::open(
        root.sys_path("fs/cgroup"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()
}

/// The Wayland compositor of this session, for the protected set, as the full sweep finds it.
fn host_compositor_pid() -> Option<u32> {
    let runtime = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    if !runtime.is_absolute() {
        return None;
    }
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_default();
    wayland_compositor_pid(&runtime, &display)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::path::PathBuf;

    use tl_collect::apps::AppPaths;
    use tl_collect::proc::sweep::{ProcessSampler, SweepCtx, SweepInputs, SweepOutput};

    use super::*;

    const BOOT_ID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0\n";
    /// The fields this mode never reads.
    const UNREAD_FIELDS: [ProcessField; 10] = [
        ProcessField::Cpu,
        ProcessField::Rss,
        ProcessField::Pss,
        ProcessField::Uss,
        ProcessField::IoRead,
        ProcessField::IoWrite,
        ProcessField::NetRx,
        ProcessField::NetTx,
        ProcessField::GpuUtil,
        ProcessField::GpuMem,
    ];
    const KONSOLE: &str =
        "user.slice/user-1000.slice/user@1000.service/app.slice/app-org.kde.konsole-1234.scope";

    thread_local! {
        static NOW: Cell<TimeNs> = const { Cell::new(1_000_000_000) };
    }

    fn test_clock() -> TimeNs {
        NOW.with(Cell::get)
    }

    /// A fake `/`, `/proc` and `/sys` under `target/tmp`, removed on drop.
    struct Fake {
        dir: tempfile::TempDir,
    }

    impl Fake {
        fn new() -> Self {
            let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
            fs::create_dir_all(&base).expect("create target/tmp");
            let dir = tempfile::Builder::new()
                .prefix("tl-engine-lowmem-")
                .tempdir_in(base)
                .expect("tempdir");
            let fake = Self { dir };
            fake.write("proc/sys/kernel/random/boot_id", BOOT_ID.as_bytes());
            fake.write(
                "etc/passwd",
                b"root:x:0:0::/root:/bin/sh\nuser1:x:1000:1000::/home/user1:/bin/sh\n",
            );
            fake.meminfo(16 << 20, 8 << 20);
            fs::create_dir_all(fake.path("sys/fs/cgroup")).expect("cgroup root");
            fake
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }

        fn root(&self) -> SysRoot {
            SysRoot::at(self.dir.path())
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let path = self.path(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
            fs::write(path, bytes).expect("write");
        }

        /// `/proc/meminfo` with every key `parse_meminfo` requires. Values in kB.
        fn meminfo(&self, total_kb: u64, avail_kb: u64) {
            let mut text = String::new();
            for (k, v) in [
                ("MemTotal", total_kb),
                ("MemFree", avail_kb),
                ("MemAvailable", avail_kb),
                ("Buffers", 0),
                ("Cached", 0),
                ("SwapCached", 0),
                ("Dirty", 0),
                ("Writeback", 0),
                ("Shmem", 0),
                ("SReclaimable", 0),
                ("SUnreclaim", 0),
                ("Committed_AS", 0),
                ("CommitLimit", 0),
                ("SwapTotal", 0),
                ("SwapFree", 0),
            ] {
                let _ = writeln!(text, "{k}: {v} kB");
            }
            self.write("proc/meminfo", text.as_bytes());
        }

        fn psi(&self, full_total_us: u64) {
            let text = format!(
                "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n\
                 full avg10=0.00 avg60=0.00 avg300=0.00 total={full_total_us}\n"
            );
            self.write("proc/pressure/memory", text.as_bytes());
        }

        /// One process: `stat`, `status` and `cgroup`.
        fn process(&self, pid: u32, comm: &[u8], uid: u32, cgroup: &str, start_ticks: u64) {
            // Fields 3 to 52; token i is field i + 3.
            let mut fields = vec!["0".to_owned(); 50];
            fields[0] = "S".into();
            fields[1] = "1".into();
            fields[17] = "3".into();
            fields[19] = start_ticks.to_string();
            let mut stat = format!("{pid} (").into_bytes();
            stat.extend_from_slice(comm);
            stat.extend_from_slice(format!(") {}\n", fields.join(" ")).as_bytes());
            self.write(&format!("proc/{pid}/stat"), &stat);
            let status = format!(
                "Name:\tx\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nGid:\t{uid}\t{uid}\t{uid}\t{uid}\n\
                 Threads:\t3\nRssAnon:\t    2048 kB\nVmSwap:\t       4 kB\n"
            );
            self.write(&format!("proc/{pid}/status"), status.as_bytes());
            self.write(
                &format!("proc/{pid}/cgroup"),
                format!("0::/{cgroup}\n").as_bytes(),
            );
        }

        fn source(&self) -> LowMemSource {
            LowMemSource::new(self.root())
                .with_self_uid(1000)
                .with_clock(test_clock)
        }
    }

    /// Three processes: konsole in an app scope, a background shell and a root daemon.
    fn three() -> Fake {
        let fake = Fake::new();
        fake.process(4101, b"konsole", 1000, KONSOLE, 500);
        fake.process(
            4102,
            b"bash",
            1000,
            "user.slice/user-1000.slice/session-2.scope",
            600,
        );
        fake.process(4103, b"rootd", 0, "system.slice/rootd.service", 700);
        fake.write(
            &format!("sys/fs/cgroup/{KONSOLE}/memory.current"),
            b"123456789\n",
        );
        fake
    }

    fn sweep(update: &SourceUpdate) -> &ProcessSweep {
        match update {
            SourceUpdate::Processes(sweep) => sweep,
            other => panic!("not a process sweep: {other:?}"),
        }
    }

    fn record(sweep: &ProcessSweep, pid: u32) -> &ProcessRecord {
        sweep
            .records
            .iter()
            .find(|r| r.key.pid == pid)
            .expect("record for pid")
    }

    #[test]
    fn sweep_fake_root_records_and_groups() {
        let fake = three();
        let (_, update) = fake.source().sample();
        let sweep = sweep(&update);
        assert_eq!(sweep.records.len(), 3);
        assert!(sweep.records.is_sorted_by_key(|r| r.key));
        assert!(sweep.extras.is_empty() && sweep.extras_p2.is_empty());
        assert!(sweep.app_stats.is_empty());

        let info = sweep.low_memory.as_ref().expect("low memory info");
        assert_eq!(info.groups.len(), 1);
        let g = &info.groups[0];
        assert_eq!(g.memory_current, 123_456_789);
        assert_eq!(&*g.name, "org.kde.konsole");
        assert_eq!(g.process_count, 1);
        assert_eq!(g.uid, 1000);
        assert_eq!(&*g.cgroup.rel_path, KONSOLE);
        let ino = fs::metadata(fake.path(&format!("sys/fs/cgroup/{KONSOLE}")))
            .map(|m| std::os::unix::fs::MetadataExt::ino(&m))
            .expect("scope dir");
        assert_eq!(g.cgroup.cgroup_id, ino);
        let app = GroupKind::App {
            app_id: Arc::from("org.kde.konsole"),
        }
        .key();
        assert_eq!(g.group, app);

        let konsole = record(sweep, 4101);
        assert_eq!(konsole.group, app);
        assert_eq!(konsole.meta.app_id.as_deref(), Some("org.kde.konsole"));
        assert_eq!(konsole.meta.cgroup.as_deref(), Some(KONSOLE));
        assert_eq!(&*konsole.meta.user, "user1");
        assert!(konsole.meta.exe.is_none() && konsole.meta.cmdline.is_none());
        assert!(konsole.meta.icon.is_none() && konsole.meta.description.is_none());
        assert_eq!(konsole.rss_anon_bytes, 2048 * 1024);
        assert_eq!(konsole.swap_bytes, 4 * 1024);
        assert_eq!(konsole.threads, 3);
        assert_eq!(record(sweep, 4102).group, GroupKind::Background.key());
        assert_eq!(record(sweep, 4103).group, GroupKind::System.key());

        assert_eq!(info.mem_total, 16 << 30);
        assert_eq!(info.mem_available, 8 << 30);
        assert!(!info.critical);
    }

    #[test]
    fn key_matches_full_sweep() {
        let fake = three();
        let (_, update) = fake.source().sample();
        let low: Vec<ProcessKey> = sweep(&update).records.iter().map(|r| r.key).collect();

        let empty = fake.path("share");
        fs::create_dir_all(&empty).expect("share dir");
        let paths = AppPaths {
            home: fake.path("home"),
            xdg_data_home: Some(empty.clone().into_os_string()),
            xdg_data_dirs: Some(empty.into_os_string()),
            xdg_config_home: None,
            icon_cache: fake.path("cache/icons.toml"),
        };
        let users = PasswdUserNames::from_bytes(b"user1:x:1000:1000::/home/user1:/bin/sh\n");
        let mut sampler =
            ProcessSampler::new(fake.root(), &paths, Box::new(users)).expect("open sampler");
        let ctx = SweepCtx {
            boot_id: BootId::from_uuid_text(BOOT_ID).expect("boot id"),
            self_uid: 1000,
            ticks_per_sec: 100,
            online_cpus: 1,
            want_fd_count: false,
            want_status_fields: false,
            want_io: false,
        };
        let protect = ProtectInputs::default();
        let mut out = SweepOutput::default();
        sampler.sweep(
            &ctx,
            &SweepInputs {
                protect: &protect,
                detail_keys: &[],
                want_pss: false,
                window_pids: None,
                wm_class: None,
            },
            &mut out,
        );
        let full: Vec<ProcessKey> = out.records.iter().map(|r| r.key).collect();
        assert_eq!(low.len(), 3);
        assert_eq!(low, full);
    }

    #[test]
    fn other_user_flag() {
        let fake = three();
        let (_, update) = fake.source().sample();
        let sweep = sweep(&update);
        let root = record(sweep, 4103);
        assert_eq!(root.uid, 0);
        assert!(root.flags.contains(ProcFlags::OTHER_USER));
        assert!(!record(sweep, 4101).flags.contains(ProcFlags::OTHER_USER));
        // pid 1 is always protected; none of these is.
        assert!(!root.flags.contains(ProcFlags::PROTECTED));
    }

    #[test]
    fn sanitized_names() {
        let fake = Fake::new();
        let comm = "evil\x1b[31m\u{202e}name\x07";
        fake.process(
            4200,
            comm.as_bytes(),
            1000,
            "user.slice/user-1000.slice/session-2.scope",
            5,
        );
        let (_, update) = fake.source().sample();
        let r = record(sweep(&update), 4200);
        let mut expected = String::new();
        sanitize_comm(comm.as_bytes(), &mut expected);
        assert_eq!(&*r.meta.name, expected);
        assert_eq!(&*r.meta.comm, expected);
        assert!(
            !r.meta.name.chars().any(char::is_control),
            "{:?}",
            r.meta.name
        );
        assert!(!r.meta.name.contains('\u{202e}'));
    }

    #[test]
    fn vanished_pid_skipped() {
        let fake = three();
        // Listed, but its files are gone.
        fs::create_dir_all(fake.path("proc/4999")).expect("pid dir");
        let (_, update) = fake.source().sample();
        let sweep = sweep(&update);
        assert_eq!(sweep.records.len(), 3);
        assert!(sweep.records.iter().all(|r| r.key.pid != 4999));
    }

    #[test]
    fn psi_missing_falls_back_to_memavailable() {
        let fake = three();
        // 2% of MemTotal available: critical without any pressure file.
        fake.meminfo(100 << 10, 2 << 10);
        let src = fake.source();
        NOW.with(|n| n.set(1_000_000_000));
        let (src, first) = src.sample();
        NOW.with(|n| n.set(3_000_000_000));
        let (_, second) = src.sample();
        for update in [&first, &second] {
            let info = sweep(update).low_memory.clone().expect("low memory info");
            assert_eq!(info.psi_full_pct, None);
            assert!(info.critical);
        }

        fake.meminfo(100 << 10, 50 << 10);
        let (_, update) = fake.source().sample();
        let info = sweep(&update).low_memory.clone().expect("low memory info");
        assert_eq!(info.psi_full_pct, None);
        assert!(!info.critical);
    }

    #[test]
    fn psi_full_share_from_total_delta() {
        let fake = three();
        fake.psi(1_000_000);
        NOW.with(|n| n.set(10_000_000_000));
        let (src, first) = fake.source().sample();
        let info = sweep(&first).low_memory.clone().expect("low memory info");
        assert_eq!(info.psi_full_pct, None);
        assert!(!info.critical);

        fake.psi(1_300_000);
        NOW.with(|n| n.set(12_000_000_000));
        let (_, second) = src.sample();
        let info = sweep(&second).low_memory.clone().expect("low memory info");
        let pct = info.psi_full_pct.expect("full share");
        assert!((pct - 15.0).abs() < 1e-4, "{pct}");
        assert!(info.critical);
    }

    #[test]
    fn period_always_two_seconds() {
        let src = Fake::new().source();
        let table = Demand {
            process_table: true,
            ..Demand::default()
        };
        for d in [Demand::default(), table] {
            for interval_ms in [250, 1000, 10_000] {
                let s = EngineSettings {
                    interval_ms,
                    ..EngineSettings::default()
                };
                assert_eq!(src.period(&d, &s), Some(Duration::from_secs(2)));
            }
        }
    }

    #[test]
    fn unread_fields_unsupported() {
        let fake = three();
        let (_, update) = fake.source().sample();
        for r in &sweep(&update).records {
            for f in UNREAD_FIELDS {
                assert_eq!(r.validity.get(f), Validity::Unsupported, "{f:?}");
            }
            assert_eq!(r.validity.get(ProcessField::RssAnon), Validity::Valid);
            assert_eq!(r.validity.get(ProcessField::Swap), Validity::Valid);
            assert_eq!(r.validity.get(ProcessField::Threads), Validity::Valid);
            assert_eq!(r.cpu_pct, 0.0);
            assert_eq!((r.rss_bytes, r.pss_bytes, r.uss_bytes), (0, 0, 0));
            assert_eq!((r.io_read_bps, r.io_write_bps), (0.0, 0.0));
            assert_eq!(
                (r.net_rx_bps, r.net_tx_bps, r.gpu_util_pct),
                (0.0, 0.0, 0.0)
            );
            assert_eq!(r.gpu_mem_bytes, 0);
        }
    }

    #[test]
    fn missing_proc_reports_failed() {
        let fake = Fake::new();
        // The boot id is read at `new`; then `/proc` goes away.
        let src = fake.source();
        fs::remove_dir_all(fake.path("proc")).expect("remove proc");
        let (_, update) = src.sample();
        match update {
            SourceUpdate::Health {
                id,
                state,
                last_error,
                ..
            } => {
                assert_eq!(id, SourceId::Processes);
                assert_eq!(state, SourceState::Failed);
                assert!(last_error.is_some());
            }
            other => panic!("not a health update: {other:?}"),
        }
    }

    /// E32: the `Processes` factory of a `LowMemory` engine runs `LowMemSource` under its
    /// supervisor, every 2 s whatever the demand.
    #[test]
    fn lab_lowmem_mode_selects_stripped_source() {
        use asupersync::channel::{oneshot, watch};
        use asupersync::types::Budget;
        use asupersync::{Cx, LabConfig, LabRuntime};

        use crate::config::{EngineConfig, EngineMode};
        use crate::source::{WorkerControl, WorkerIo};
        use crate::supervise::build_supervisor;
        use crate::update::EngineMsg;
        use crate::workers::processes;

        let fake = three();
        let cfg = Arc::new(
            EngineConfig::new(EngineMode::LowMemory, fake.path("state")).with_sys_root(fake.root()),
        );
        let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(1_000_000));
        let region = lab.state.create_root_region(Budget::INFINITE);
        let (updates, mut rx) = asupersync::channel::mpsc::channel::<EngineMsg>(256);
        let (_demand_tx, demand) = watch::channel(Demand::default());
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (_control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(
            updates,
            demand,
            settings,
            control,
            crate::dbus::SharedHandles::default(),
        );
        let factory = processes::factory(cfg, io);
        let (abort_tx, mut abort_rx) = oneshot::channel::<()>();
        let (task, _handle) = lab
            .state
            .create_task(region, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let supervisor = build_supervisor("processes", factory).expect("supervisor");
                let mut handle = supervisor.spawn(&cx).expect("supervisor spawns");
                let _ = abort_rx.recv(&cx).await;
                handle.abort();
                let _ = handle.join().await;
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();

        // 4 s in 100 ms steps; a sweep due at 4 s is past the window.
        let step = Duration::from_millis(100);
        let mut sweeps = Vec::new();
        for n in 0..40u64 {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    EngineMsg::Source(SourceUpdate::Processes(s)) => sweeps.push((n * 100, s)),
                    other => panic!("unexpected message {other:?}"),
                }
            }
            if n < 39 {
                lab.advance_time(u64::try_from(step.as_nanos()).expect("fits in u64"));
                lab.step_for_test();
                lab.run_until_idle();
            }
        }
        let times: Vec<u64> = sweeps.iter().map(|(t, _)| *t).collect();
        assert_eq!(times, [0, 2_000], "one sweep every 2 s");
        for (t, s) in &sweeps {
            assert!(s.low_memory.is_some(), "full sweep at {t} ms");
            assert_eq!(s.records.len(), 3);
            for r in &s.records {
                assert_eq!(r.validity.get(ProcessField::Cpu), Validity::Unsupported);
            }
        }

        abort_tx.send_blocking(()).expect("the root task waits");
        let virtual_time = lab.run_with_auto_advance();
        assert_ne!(
            virtual_time.termination,
            asupersync::lab::AutoAdvanceTermination::StepLimitReached,
            "{virtual_time:?}"
        );
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn psi_full_total_parses_the_full_line() {
        let text = b"some avg10=1.00 avg60=0.00 avg300=0.00 total=99\n\
                     full avg10=0.50 avg60=0.00 avg300=0.00 total=4242\n";
        assert_eq!(psi_full_total(text), Some(4242));
        assert_eq!(psi_full_total(b"some avg10=1.00 total=99\n"), None);
        assert_eq!(psi_full_total(b""), None);
    }
}
