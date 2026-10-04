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
use tl_collect::proc::status::{
    PidStatus, STATUS_READ_MAX, commit_bytes, parse_status, ws_shared_bytes,
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

fn status(fixture: &str, pid: u32) -> PidStatus {
    let mut out = PidStatus::default();
    parse_status(
        &fixture_bytes(fixture, &format!("proc/{pid}/status")),
        &mut out,
    )
    .expect("parse status");
    out
}

#[test]
fn status_memory_fields_in_bytes() {
    let s = status("process-edge", 4201);
    assert_eq!(s.uid, [1000; 4]);
    assert_eq!(s.gid, [1000; 4]);
    assert_eq!(s.groups.as_slice(), [10, 998, 1000]);
    assert_eq!(s.threads, 27);
    assert_eq!(s.rss_anon, Some(1_263_616));
    assert_eq!(s.rss_file, Some(198_520 * 1024));
    assert_eq!(s.rss_shmem, Some(45_182 * 1024));
    assert_eq!(s.vm_rss, Some(244_936 * 1024));
    assert_eq!(s.vm_swap, Some(5120 * 1024));
    assert_eq!(s.vm_hwm, Some(301_220 * 1024));
    assert_eq!(s.vm_peak, Some(2_734_592 * 1024));
    assert_eq!(s.vm_size, Some(2_667_832 * 1024));
    assert_eq!(s.vm_data, Some(100 * 1024));
    assert_eq!(s.vm_stk, Some(132 * 1024));
    assert_eq!(s.vm_pte, Some(1388 * 1024));
    assert_eq!(commit_bytes(&s), Some(237_568));
    assert_eq!(ws_shared_bytes(&s), Some((198_520 + 45_182) * 1024));
    assert_eq!(s.cpus_allowed_list.as_slice(), b"0-7,16-23");
    assert_eq!(s.cap_eff, 0);
    assert_eq!(s.cap_bnd, 0x1ff_ffff_ffff);
    assert_eq!(s.no_new_privs, Some(true));
    assert_eq!(s.seccomp, Some(2));
    assert_eq!(s.seccomp_filters, Some(3));
    assert_eq!(
        s.speculation_store_bypass.as_slice(),
        b"thread force mitigated"
    );
    assert_eq!(
        s.speculation_indirect_branch.as_slice(),
        b"conditional force disabled"
    );
    assert_eq!(s.x86_thread_features.as_deref(), Some(&b"shstk wrss"[..]));
    assert_eq!(s.voluntary_ctxt_switches, Some(88120));
    assert_eq!(s.nonvoluntary_ctxt_switches, Some(1542));
}

#[test]
fn status_kernel_thread_has_no_rss() {
    let s = status("process-edge", 57);
    assert_eq!(s.rss_anon, None);
    assert_eq!(s.vm_rss, None);
    assert_eq!(commit_bytes(&s), None);
    assert_eq!(ws_shared_bytes(&s), None);
    assert!(s.groups.is_empty());
    assert_eq!(s.x86_thread_features, None);
    assert_eq!(s.threads, 1);
}

#[test]
fn status_setuid_uids_and_caps() {
    let s = status("process-edge", 4204);
    assert_eq!(s.uid, [1000, 0, 0, 0]);
    assert_ne!(s.cap_eff, 0);
}

#[test]
fn status_missing_uid_is_error() {
    let buf = fixture_bytes("process-edge", "proc/4201/status");
    let without_uid: Vec<u8> = buf
        .split_inclusive(|&b| b == b'\n')
        .filter(|line| !line.starts_with(b"Uid:"))
        .flatten()
        .copied()
        .collect();
    let mut out = PidStatus::default();
    let err = parse_status(&without_uid, &mut out).unwrap_err();
    assert!(matches!(
        err,
        CollectError::ProcParse {
            pid: 0,
            file: "status"
        }
    ));
    assert!(parse_status(b"", &mut out).is_err());
}

#[test]
fn status_long_file_parsed_up_to_cut() {
    // A 20 KiB file: the real lines, filler lines with unknown keys, then a `VmSwap:` line that
    // the cut at STATUS_READ_MAX splits after `VmSwap:\t9`. The partial line must be skipped.
    let mut buf = fixture_bytes("process-edge", "proc/4201/status");
    let swap_line = b"VmSwap:\t99999 kB\n";
    let filler_len = STATUS_READ_MAX - 9 - buf.len();
    buf.extend_from_slice(b"Filler:");
    buf.resize(buf.len() + filler_len - 8, b'x');
    buf.push(b'\n');
    buf.extend_from_slice(swap_line);
    while buf.len() < 20 * 1024 {
        buf.extend_from_slice(b"Filler:\t1\n");
    }
    let cut = &buf[..STATUS_READ_MAX];
    assert!(cut.ends_with(b"VmSwap:\t9"));

    let mut out = PidStatus::default();
    parse_status(cut, &mut out).expect("parse cut status");
    assert_eq!(out.uid, [1000; 4]);
    assert_eq!(out.rss_anon, Some(1_263_616));
    assert_eq!(out.vm_swap, Some(5120 * 1024));
    assert_eq!(out.nonvoluntary_ctxt_switches, Some(1542));
}

#[test]
fn status_owner_desktop_snapshot() {
    let proc_dir = fixture_dir("owner-desktop").join("proc");
    let mut pids: Vec<u32> = fs::read_dir(&proc_dir)
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_str()?.parse().ok())
        .filter(|pid: &u32| proc_dir.join(pid.to_string()).join("status").exists())
        .collect();
    pids.sort_unstable();
    assert!(!pids.is_empty());

    // (pid, real uid, threads, RssAnon, VmRSS)
    type Row = (u32, u32, u32, Option<u64>, Option<u64>);
    let rows: Vec<Row> = pids
        .iter()
        .map(|&pid| {
            let s = status("owner-desktop", pid);
            (pid, s.uid[0], s.threads, s.rss_anon, s.vm_rss)
        })
        .collect();
    insta::assert_debug_snapshot!(rows);
}
