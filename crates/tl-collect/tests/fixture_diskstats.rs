//! Fixture tests for the /proc/diskstats parser (E04-020).

mod common;

use std::fmt::Write as _;

use common::fixture_bytes;
use tl_collect::parse::diskstats::{DiskStatLine, parse_diskstats};

fn parse(fixture: &str) -> Vec<DiskStatLine> {
    let mut out = Vec::new();
    parse_diskstats(&fixture_bytes(fixture, "proc/diskstats"), &mut out).unwrap();
    out
}

fn name(line: &DiskStatLine) -> &str {
    std::str::from_utf8(&line.name).unwrap()
}

/// The 17 numbers of a line in file order.
fn numbers(l: &DiskStatLine) -> [u64; 17] {
    [
        l.reads,
        l.reads_merged,
        l.sectors_read,
        l.read_ms,
        l.writes,
        l.writes_merged,
        l.sectors_written,
        l.write_ms,
        l.in_flight,
        l.io_ms,
        l.weighted_ms,
        l.discards,
        l.discards_merged,
        l.sectors_discarded,
        l.discard_ms,
        l.flushes,
        l.flush_ms,
    ]
}

#[test]
fn owner_desktop_snapshot() {
    let lines = parse("owner-desktop");
    let names: Vec<&str> = lines.iter().map(name).collect();
    for want in ["nvme0n1", "nvme1n1", "sda"] {
        assert!(names.contains(&want), "missing {want} in {names:?}");
    }
    // One text line per disk so the snapshot shows names, not byte arrays.
    let mut text = String::new();
    for l in &lines {
        writeln!(text, "{} {} {} {:?}", l.major, l.minor, name(l), numbers(l)).unwrap();
    }
    insta::assert_snapshot!(text);
}

#[test]
fn three_formats_and_bad_line() {
    let lines = parse("diskstats-formats");
    let names: Vec<&str> = lines.iter().map(name).collect();
    assert_eq!(names, ["sda", "nvme0n1", "nvme0n1p1"]);

    let sda = &lines[0];
    assert_eq!((sda.major, sda.minor), (8, 0));
    assert_eq!(
        numbers(sda),
        [101, 2, 3003, 404, 505, 6, 7007, 808, 0, 909, 1212, 0, 0, 0, 0, 0, 0]
    );

    let nvme = &lines[1];
    assert_eq!((nvme.major, nvme.minor), (259, 0));
    assert_eq!(
        numbers(nvme),
        [11, 12, 13, 14, 15, 16, 17, 18, 0, 20, 21, 22, 23, 24, 25, 0, 0]
    );

    let part = &lines[2];
    assert_eq!((part.major, part.minor), (259, 1));
    assert_eq!(
        numbers(part),
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17]
    );
}

#[test]
fn empty_buffer() {
    let mut out = vec![DiskStatLine::default()];
    parse_diskstats(b"", &mut out).unwrap();
    assert!(out.is_empty());
}

#[test]
fn reparse_keeps_capacity() {
    let buf = fixture_bytes("owner-desktop", "proc/diskstats");
    let mut out = Vec::new();
    parse_diskstats(&buf, &mut out).unwrap();
    let capacity = out.capacity();
    let first = out.clone();
    parse_diskstats(&buf, &mut out).unwrap();
    assert_eq!(out.capacity(), capacity);
    assert_eq!(out, first);
    assert!(out.iter().all(|l| !l.name.spilled()));
}
