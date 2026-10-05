//! Real-system tests that read restricted or kernel-dependent /proc data. Run with
//! `cargo test -p tl-collect --test real_restricted -- --ignored`.
use std::os::fd::{AsFd, OwnedFd};
use std::process::{Child, Command, Stdio};

use rustix::fs::{Mode, OFlags};
use tl_collect::proc::fdcount::{FdCount, fd_count, fd_count_fallbacks};

fn open_dir(path: &str) -> OwnedFd {
    rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .expect("open proc dir")
}

fn spawn_sleep() -> Child {
    Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sleep")
}

/// Kernel (major, minor) from uname, or None when the release string does not parse.
fn kernel_version() -> Option<(u32, u32)> {
    let uname = rustix::system::uname();
    let release = uname.release().to_str().ok()?;
    let mut parts = release.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// R-268: the kernel's `st_size` count is shown for any owner. Since kernel 6.2 `st_size` of
/// another user's fd dir gives the count even though readdir there gives EACCES.
#[test]
#[ignore = "real system: needs a non-root user"]
fn fdcount_pid1_other_user() {
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let dir = open_dir("/proc/1");
    let got = fd_count(dir.as_fd());
    assert!(
        matches!(got, FdCount::Count(n) if n > 0) || got == FdCount::Restricted,
        "PID 1 gave {got:?}"
    );
    if kernel_version().is_some_and(|v| v >= (6, 2)) {
        assert!(
            matches!(got, FdCount::Count(_)),
            "kernel 6.2+ must count, got {got:?}"
        );
    }
}

/// Settles U-34: `st_size` of /proc/<pid>/fd gives the count on this kernel.
#[test]
#[ignore = "real system: kernel dependent"]
fn fdcount_st_size_works_on_this_kernel() {
    let before = fd_count_fallbacks();
    let mut children: Vec<Child> = (0..20).map(|_| spawn_sleep()).collect();
    for child in &children {
        let dir = open_dir(&format!("/proc/{}", child.id()));
        let got = fd_count(dir.as_fd());
        assert!(
            matches!(got, FdCount::Count(n) if n >= 3),
            "child {} gave {got:?}",
            child.id()
        );
    }
    let used = fd_count_fallbacks() - before;
    for child in &mut children {
        child.kill().expect("kill own child");
        child.wait().expect("reap own child");
    }
    assert_eq!(
        used, 0,
        "st_size was 0, readdir fallback used {used} times (U-34 failed)"
    );
}

/// PROC-030: PSS of another user's process is Restricted, never zero.
#[test]
#[ignore = "real system: needs a non-root user"]
fn rollup_pid1_restricted() {
    use tl_collect::SysRoot;
    use tl_collect::proc::pid_stat::read_start_ticks;
    use tl_collect::proc::smaps::{PssReader, RollupRead};
    use tl_model::ids::ProcessKey;
    use tl_model::time::BootId;

    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let dir = open_dir("/proc/1");
    let start_ticks = read_start_ticks(dir.as_fd(), &mut Vec::new()).expect("pid 1 stat");
    let key = ProcessKey {
        start_ticks,
        boot_id: BootId(1),
        pid: 1,
    };
    let mut reader = PssReader::new();
    let (mut buf, mut out) = (Vec::new(), Vec::new());
    reader.read(&SysRoot::host(), &[key], &mut buf, &mut out);
    assert_eq!(out, vec![(key, RollupRead::Restricted)]);
    assert_eq!(reader.cached(), 0);
}

/// PROC-027: the exe link of another user's process is Restricted.
#[test]
#[ignore = "real system: needs a non-root user"]
fn meta_pid1_exe_restricted() {
    use tl_collect::proc::meta::{ExeLink, MetaCache, ReadBufs};
    use tl_model::ids::ProcessKey;
    use tl_model::time::BootId;

    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let dir = open_dir("/proc/1");
    let key = ProcessKey {
        start_ticks: 0,
        boot_id: BootId(1),
        pid: 1,
    };
    let mut cache = MetaCache::new();
    let mut bufs = ReadBufs::default();
    let (meta, _) = cache.get_or_refresh(key, dir.as_fd(), b"systemd", &mut bufs);
    assert_eq!(meta.exe, ExeLink::Restricted);
}

/// PROC-081: the memory maps of another user's process are Restricted.
#[test]
#[ignore = "real system: needs a non-root user"]
fn smaps_pid1_restricted() {
    use tl_collect::proc::smaps::{SmapsRead, read_smaps};

    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let dir = open_dir("/proc/1");
    let (mut buf, mut out) = (Vec::new(), Vec::new());
    assert_eq!(
        read_smaps(dir.as_fd(), &mut buf, &mut out).expect("read pid 1 smaps"),
        SmapsRead::Restricted
    );
    assert!(out.is_empty());
}

/// PROC-080: the open fds of another user's process are Restricted without the helper
/// (security regression).
#[test]
#[ignore = "real system: needs a non-root user"]
fn fds_pid1_restricted() {
    use tl_collect::SysRoot;
    use tl_collect::proc::detail::{DetailRead, read_fds};
    use tl_collect::proc::meta::ReadBufs;
    use tl_collect::proc::pid_stat::read_start_ticks;
    use tl_model::ids::ProcessKey;
    use tl_model::time::BootId;

    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let dir = open_dir("/proc/1");
    let start_ticks = read_start_ticks(dir.as_fd(), &mut Vec::new()).expect("pid 1 start ticks");
    let key = ProcessKey {
        start_ticks,
        boot_id: BootId(1),
        pid: 1,
    };
    let mut out = Vec::new();
    let r = read_fds(&SysRoot::host(), key, &mut ReadBufs::default(), &mut out);
    assert_eq!(r.expect("read pid 1 fds"), DetailRead::Restricted);
    assert!(out.is_empty());
}

/// PROC-081: the memory maps of another user's process are Restricted without the helper
/// (security regression).
#[test]
#[ignore = "real system: needs a non-root user"]
fn maps_pid1_restricted() {
    use tl_collect::SysRoot;
    use tl_collect::proc::detail::{DetailRead, read_maps};
    use tl_collect::proc::meta::ReadBufs;
    use tl_collect::proc::pid_stat::read_start_ticks;
    use tl_model::ids::ProcessKey;
    use tl_model::time::BootId;

    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let dir = open_dir("/proc/1");
    let start_ticks = read_start_ticks(dir.as_fd(), &mut Vec::new()).expect("pid 1 start ticks");
    let key = ProcessKey {
        start_ticks,
        boot_id: BootId(1),
        pid: 1,
    };
    let (mut regions, mut nodes) = (Vec::new(), Vec::new());
    let r = read_maps(
        &SysRoot::host(),
        key,
        true,
        &mut ReadBufs::default(),
        &mut regions,
        &mut nodes,
    );
    assert_eq!(r.expect("read pid 1 maps"), DetailRead::Restricted);
    assert!(regions.is_empty() && nodes.is_empty());
}

/// The full `ProcessSampler` on the live `/proc` and `/sys`. The icon cache goes to `cache`, so a
/// sweep never writes the user's own cache file.
fn live_sampler(cache: &std::path::Path) -> tl_collect::proc::sweep::ProcessSampler {
    use tl_collect::SysRoot;
    use tl_collect::apps::AppPaths;
    use tl_collect::proc::meta::NumericUserNames;
    use tl_collect::proc::sweep::ProcessSampler;

    let root = SysRoot {
        proc: "/proc".into(),
        sys: "/sys".into(),
        ..SysRoot::host()
    };
    let paths = AppPaths {
        home: std::env::var_os("HOME").expect("HOME is set").into(),
        xdg_data_home: std::env::var_os("XDG_DATA_HOME"),
        xdg_data_dirs: std::env::var_os("XDG_DATA_DIRS"),
        xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
        icon_cache: cache.join("icons.toml"),
    };
    ProcessSampler::new(root, &paths, Box::new(NumericUserNames::new())).expect("open sampler")
}

/// Boot id folded as ADR-003 says, the real uid, 100 ticks per second and the online CPU count.
fn live_ctx() -> tl_collect::proc::sweep::SweepCtx {
    use smallvec::SmallVec;
    use tl_collect::parse::topology::parse_cpu_list;
    use tl_collect::proc::sweep::SweepCtx;
    use tl_model::time::BootId;

    let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").expect("read boot_id");
    let online = std::fs::read("/sys/devices/system/cpu/online").expect("read online cpus");
    let mut cpus: SmallVec<[u16; 64]> = SmallVec::new();
    parse_cpu_list(&online, &mut cpus).expect("parse online cpus");
    SweepCtx {
        boot_id: BootId::from_uuid_text(&text).expect("fold boot_id"),
        self_uid: rustix::process::getuid().as_raw(),
        ticks_per_sec: 100,
        online_cpus: u32::try_from(cpus.len()).expect("cpu count"),
        want_fd_count: true,
    }
}

/// Two sweeps 1 s apart, so the second one has rates.
fn two_sweeps(
    protect: &tl_collect::apps::classify::ProtectInputs,
) -> tl_collect::proc::sweep::SweepOutput {
    use tl_collect::proc::sweep::{SweepInputs, SweepOutput};

    let cache = tempfile::tempdir().expect("temp dir");
    let mut sampler = live_sampler(cache.path());
    let ctx = live_ctx();
    let inp = SweepInputs {
        protect,
        detail_keys: &[],
        want_pss: false,
        window_pids: None,
        wm_class: None,
    };
    let mut out = SweepOutput::default();
    sampler.sweep(&ctx, &inp, &mut out);
    std::thread::sleep(std::time::Duration::from_secs(1));
    sampler.sweep(&ctx, &inp, &mut out);
    out
}

/// ADR-016, PROC-002, PROC-031: another user's io counters are Restricted, never a measured zero
/// (security regression). Failures print pid and comm only (ADR-015).
#[test]
#[ignore = "real system: needs a non-root user"]
fn other_user_io_restricted_not_zero() {
    use tl_collect::apps::classify::ProtectInputs;
    use tl_model::process::ProcFlags;
    use tl_model::validity::{ProcessField, Validity};

    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: run as a normal user");
        return;
    }
    let self_uid = rustix::process::getuid().as_raw();
    let out = two_sweeps(&ProtectInputs::default());

    let i = out
        .records
        .iter()
        .position(|r| r.key.pid == 1)
        .expect("PID 1 swept");
    let (init, extras) = (&out.records[i], &out.extras[i]);
    assert_eq!(
        init.validity.get(ProcessField::IoRead),
        Validity::Restricted
    );
    assert_eq!(
        init.validity.get(ProcessField::IoWrite),
        Validity::Restricted
    );
    assert!(
        init.flags
            .contains(ProcFlags::OTHER_USER | ProcFlags::PROTECTED),
        "PID 1 flags {:?}",
        init.flags
    );
    // R-268: the fd count of another user's process is the kernel's st_size number, and only
    // the readdir fallback (before kernel 6.2) gives Restricted, which the record shows as None.
    if kernel_version().is_some_and(|v| v >= (6, 2)) {
        assert!(
            matches!(extras.handles, Some(n) if n > 0),
            "PID 1 handles {:?}",
            extras.handles
        );
    }

    let mut restricted = 0usize;
    for r in out.records.iter().filter(|r| r.uid != self_uid) {
        let v = r.validity.get(ProcessField::IoRead);
        assert_ne!(
            v,
            Validity::Valid,
            "pid {} comm {} shows another user's io as measured ({} B/s)",
            r.key.pid,
            r.meta.comm,
            r.io_read_bps
        );
        restricted += usize::from(v == Validity::Restricted);
    }
    assert!(restricted > 0, "no other user's process was Restricted");
}

/// PROC-031, PROC-035: the test's own process has io rates, a CPU share and an fd count.
#[test]
#[ignore = "real system: needs a non-root user"]
fn own_process_io_valid() {
    use tl_collect::apps::classify::ProtectInputs;
    use tl_model::validity::{ProcessField, Validity};

    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: run as a normal user");
        return;
    }
    let out = two_sweeps(&ProtectInputs::default());
    let me = std::process::id();
    let i = out
        .records
        .iter()
        .position(|r| r.key.pid == me)
        .expect("own process swept");
    let (rec, extras) = (&out.records[i], &out.extras[i]);
    assert_eq!(rec.validity.get(ProcessField::IoRead), Validity::Valid);
    assert!(
        rec.cpu_pct.is_finite() && rec.cpu_pct >= 0.0,
        "cpu_pct {}",
        rec.cpu_pct
    );
    assert!(
        matches!(extras.handles, Some(n) if n >= 3),
        "handles {:?}",
        extras.handles
    );
}

/// ADR-006, PROC-059: the owner session's `kwin_wayland` is protected.
#[test]
#[ignore = "real system: needs a non-root user in a Wayland session"]
fn compositor_protected() {
    use tl_collect::apps::classify::{ProtectInputs, wayland_compositor_pid};
    use tl_model::process::ProcFlags;

    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: run as a normal user");
        return;
    }
    let Some(display) = std::env::var_os("WAYLAND_DISPLAY") else {
        eprintln!("skipped: WAYLAND_DISPLAY is not set");
        return;
    };
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR is set");
    let protect = ProtectInputs {
        compositor_pid: wayland_compositor_pid(std::path::Path::new(&runtime), &display),
        ..ProtectInputs::default()
    };
    let self_uid = rustix::process::getuid().as_raw();
    let out = two_sweeps(&protect);
    let kwin: Vec<_> = out
        .records
        .iter()
        .filter(|r| r.uid == self_uid && &*r.meta.comm == "kwin_wayland")
        .collect();
    if kwin.is_empty() {
        eprintln!("skipped: no kwin_wayland in this session");
        return;
    }
    for r in kwin {
        assert!(
            r.flags.contains(ProcFlags::PROTECTED),
            "pid {} comm {} flags {:?}",
            r.key.pid,
            r.meta.comm,
            r.flags
        );
    }
}
