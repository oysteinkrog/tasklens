//! Fixture tests for the /proc/stat parser (E04-008).

mod common;

use common::fixture_bytes;
use tl_collect::CollectError;
use tl_collect::parse::stat::{CpuTimes, StatData, parse_stat};

fn parse(buf: &[u8]) -> StatData {
    let mut out = StatData::default();
    parse_stat(buf, &mut out).unwrap();
    out
}

#[test]
fn owner_desktop_snapshot() {
    let buf = fixture_bytes("owner-desktop", "proc/stat");
    let out = parse(&buf);

    let cpu_lines = buf
        .split(|&b| b == b'\n')
        .filter(|l| l.starts_with(b"cpu") && !l.starts_with(b"cpu "))
        .count();
    assert_eq!(out.per_cpu.len(), cpu_lines);
    let ids: Vec<u16> = out.per_cpu.iter().map(|(n, _)| *n).collect();
    let expected: Vec<u16> = (0..u16::try_from(cpu_lines).unwrap()).collect();
    assert_eq!(ids, expected);

    insta::assert_debug_snapshot!(out);
}

#[test]
fn hotplug_ids() {
    let out = parse(&fixture_bytes("cpu-hotplug", "step1/proc/stat"));
    let ids: Vec<u16> = out.per_cpu.iter().map(|(n, _)| *n).collect();
    assert_eq!(ids, [0, 1, 3]);

    let step0 = parse(&fixture_bytes("cpu-hotplug", "step0/proc/stat"));
    let ids0: Vec<u16> = step0.per_cpu.iter().map(|(n, _)| *n).collect();
    assert_eq!(ids0, [0, 1, 2, 3]);
    assert_eq!(step0.ctxt, 1000);
    assert_eq!(step0.btime, 1_700_000_000);
    assert_eq!(step0.processes, 500);
    assert_eq!(step0.procs_running, 2);
    assert_eq!(step0.procs_blocked, 0);
}

#[test]
fn short_line_zero_fills() {
    let out = parse(b"cpu  10 20 30 40\n");
    assert_eq!(
        out.total,
        CpuTimes {
            user: 10,
            nice: 20,
            system: 30,
            idle: 40,
            ..CpuTimes::default()
        }
    );
    assert_eq!(out.total.total(), 100);
    assert!(out.per_cpu.is_empty());
}

#[test]
fn too_short_cpu_line_errors() {
    let mut out = StatData::default();
    let err = parse_stat(b"cpu  1 2 3\n", &mut out).unwrap_err();
    assert!(matches!(
        err,
        CollectError::Parse {
            file: "/proc/stat",
            what: "cpu line"
        }
    ));
}

#[test]
fn no_cpu_line_errors() {
    let mut out = StatData::default();
    let err = parse_stat(b"cpu0 1 2 3 4\nctxt 5\n", &mut out).unwrap_err();
    assert!(matches!(
        err,
        CollectError::Parse {
            file: "/proc/stat",
            what: "no cpu line"
        }
    ));
    let err = parse_stat(b"", &mut out).unwrap_err();
    assert!(matches!(err, CollectError::Parse { what: "no cpu line", .. }));
}

#[test]
fn cpu_math_helpers() {
    let out = parse(b"cpu  100 5 50 800 10 3 7 2 40 1\n");
    let t = out.total;
    assert_eq!(t.guest, 40);
    assert_eq!(t.guest_nice, 1);
    assert_eq!(t.busy(), 167);
    assert_eq!(t.total(), 977);
    assert_eq!(t.kernel(), 60);
    assert_eq!(t.interrupts(), 10);
}

#[test]
fn intr_line_skipped() {
    let mut buf = b"cpu  1 2 3 4 5 6 7 8 9 10 11 12\ncpu0 1 2 3 4\nintr".to_vec();
    for i in 0..10_000 {
        buf.extend_from_slice(format!(" {i}").as_bytes());
    }
    buf.extend_from_slice(b" not-a-number x\nctxt 42\nsoftirq junk !! 1\nprocesses 7\n");
    let out = parse(&buf);
    assert_eq!(out.total.user, 1);
    assert_eq!(out.total.guest_nice, 10);
    assert_eq!(out.per_cpu.len(), 1);
    assert_eq!(out.ctxt, 42);
    assert_eq!(out.processes, 7);
    assert_eq!(out.btime, 0);
}

#[test]
fn reparse_keeps_capacity() {
    let buf = fixture_bytes("owner-desktop", "proc/stat");
    let mut out = StatData::default();
    parse_stat(&buf, &mut out).unwrap();
    let first = out.clone();
    let capacity = out.per_cpu.capacity();
    parse_stat(&buf, &mut out).unwrap();
    assert_eq!(out.per_cpu.capacity(), capacity);
    assert_eq!(out, first);
}
