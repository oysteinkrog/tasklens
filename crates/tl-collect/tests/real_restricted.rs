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
