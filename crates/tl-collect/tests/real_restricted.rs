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

#[test]
#[ignore = "real system: needs a non-root user"]
fn fdcount_pid1_restricted() {
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: running as root");
        return;
    }
    let dir = open_dir("/proc/1");
    assert_eq!(fd_count(dir.as_fd()), FdCount::Restricted);
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
