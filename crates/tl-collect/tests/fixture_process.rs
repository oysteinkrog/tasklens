//! Fixture tests for the per-process readers (E05). `process-edge` holds hand-made edge cases,
//! `owner-desktop` is a real capture.

mod common;

use std::fs;

use common::{fixture_bytes, fixture_dir};
use rustix::fd::AsFd;
use rustix::fs::{Mode, OFlags};
use tl_collect::CollectError;
use tl_collect::proc::pid_stat::{
    PidStat, is_kernel_thread, map_state, parse_pid_stat, read_start_ticks, state_label,
};
use tl_model::process::ProcState;

fn stat(fixture: &str, pid: u32) -> PidStat {
    let mut out = PidStat::default();
    parse_pid_stat(
        &fixture_bytes(fixture, &format!("proc/{pid}/stat")),
        &mut out,
    )
    .expect("parse stat");
    out
}

#[test]
fn stat_comm_with_spaces_and_parens() {
    let s = stat("process-edge", 4202);
    assert_eq!(s.pid, 4202);
    assert_eq!(s.comm_raw.as_slice(), b"a) (b) c");
    assert_eq!(s.state, b'R');
    assert_eq!(s.ppid, 4201);
    assert_eq!(s.utime, 1234);
    assert_eq!(s.stime, 567);
    assert_eq!(s.priority, 19);
    assert_eq!(s.nice, -1);
    assert_eq!(s.num_threads, 4);
    assert_eq!(s.start_ticks, 98765);
    assert_eq!(s.processor, 5);

    let s = stat("process-edge", 4201);
    assert_eq!(s.comm_raw.as_slice(), b"Web Content");
    assert_eq!(s.ppid, 4100);
    assert_eq!(s.minflt, 52311);
    assert_eq!(s.majflt, 12);
    assert_eq!(s.num_threads, 27);
    assert_eq!(s.start_ticks, 351_220);
    assert_eq!(s.processor, 3);
}

#[test]
fn stat_invalid_utf8_comm_kept_raw() {
    let s = stat("process-edge", 4203);
    assert_eq!(s.comm_raw.as_slice(), [0xff, 0xfe]);
    assert_eq!(s.ppid, 1);
    assert_eq!(s.start_ticks, 400_000);
}

#[test]
fn stat_truncated_is_parse_error() {
    let buf = fixture_bytes("process-edge", "proc/4201/stat");
    // After the last `)` come a space and then fields 3, 4, ...: field 15 ends at the 14th space.
    let close = buf.iter().rposition(|&b| b == b')').unwrap();
    let cut = buf[close..]
        .iter()
        .enumerate()
        .filter(|&(_, &b)| b == b' ')
        .nth(13)
        .map(|(i, _)| close + i)
        .unwrap();
    let truncated = &buf[..cut];
    assert!(truncated.ends_with(b" 1204"), "cut after stime");

    let mut out = PidStat::default();
    let err = parse_pid_stat(truncated, &mut out).unwrap_err();
    assert!(matches!(
        err,
        CollectError::ProcParse {
            pid: 4201,
            file: "stat"
        }
    ));

    let err = parse_pid_stat(b"x (a) S 1", &mut out).unwrap_err();
    assert!(matches!(
        err,
        CollectError::ProcParse {
            pid: 0,
            file: "stat"
        }
    ));
    let err = parse_pid_stat(b"", &mut out).unwrap_err();
    assert!(matches!(
        err,
        CollectError::ProcParse {
            pid: 0,
            file: "stat"
        }
    ));
}

#[test]
fn stat_kernel_thread_detection() {
    let kworker = stat("process-edge", 57);
    assert_eq!(kworker.comm_raw.as_slice(), b"kworker/0:1");
    assert!(is_kernel_thread(&kworker));
    assert!(is_kernel_thread(&stat("process-edge", 2)));
    assert!(!is_kernel_thread(&stat("process-edge", 4201)));
}

#[test]
fn stat_state_letters() {
    assert_eq!(state_label(b't'), "Tracing stop");
    assert_eq!(map_state(b't'), ProcState::Stopped);
    assert_eq!(state_label(b'X'), "Dead");
    assert_eq!(map_state(b'X'), ProcState::Other);
    assert_eq!(state_label(b'?'), "Unknown");
    assert_eq!(map_state(b'D'), ProcState::DiskSleep);
}

#[test]
fn stat_read_start_ticks_from_dirfd() {
    let dir = rustix::fs::open(
        fixture_dir("process-edge").join("proc/4202"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .unwrap();
    let mut buf = Vec::new();
    assert_eq!(read_start_ticks(dir.as_fd(), &mut buf).unwrap(), 98765);
}

#[test]
fn stat_owner_desktop_snapshot() {
    let proc_dir = fixture_dir("owner-desktop").join("proc");
    let mut pids: Vec<u32> = fs::read_dir(&proc_dir)
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_str()?.parse().ok())
        .filter(|pid: &u32| proc_dir.join(pid.to_string()).join("stat").exists())
        .collect();
    pids.sort_unstable();
    assert!(!pids.is_empty());

    let rows: Vec<(u32, u32, char, u32)> = pids
        .iter()
        .map(|&pid| {
            let s = stat("owner-desktop", pid);
            assert_eq!(s.pid, pid);
            (s.pid, s.ppid, char::from(s.state), s.num_threads)
        })
        .collect();
    insta::assert_debug_snapshot!(rows);
}
