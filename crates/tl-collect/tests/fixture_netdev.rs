//! Fixture tests for the /proc/net/dev parser (E04-025).

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

mod common;

use std::fmt::Write as _;

use common::fixture_bytes;
use tl_collect::parse::netdev::{NetDevLine, parse_net_dev};

const HEADERS: &str = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
";

fn parse_bytes(buf: &[u8]) -> Vec<NetDevLine> {
    let mut out = Vec::new();
    parse_net_dev(buf, &mut out).unwrap();
    out
}

fn name(line: &NetDevLine) -> &str {
    std::str::from_utf8(&line.name).unwrap()
}

#[test]
fn owner_desktop_snapshot() {
    let lines = parse_bytes(&fixture_bytes("owner-desktop", "proc/net/dev"));
    let names: Vec<&str> = lines.iter().map(name).collect();
    for want in ["lo", "tailscale0", "docker0"] {
        assert!(names.contains(&want), "missing {want} in {names:?}");
    }
    // One text line per interface so the snapshot shows names, not byte arrays.
    let mut text = String::new();
    for l in &lines {
        writeln!(
            text,
            "{} rx {} {} {} {} tx {} {} {} {}",
            name(l),
            l.rx_bytes,
            l.rx_packets,
            l.rx_errors,
            l.rx_drop,
            l.tx_bytes,
            l.tx_packets,
            l.tx_errors,
            l.tx_drop
        )
        .unwrap();
    }
    insta::assert_snapshot!(text);
}

#[test]
fn no_space_after_colon() {
    let buf = format!("{HEADERS}  eth0:123456 10 0 0 0 0 0 0 654321 20 0 0 0 0 0 0\n");
    let lines = parse_bytes(buf.as_bytes());
    assert_eq!(lines.len(), 1);
    let eth0 = &lines[0];
    assert_eq!(name(eth0), "eth0");
    assert_eq!(eth0.rx_bytes, 123456);
    assert_eq!(eth0.rx_packets, 10);
    assert_eq!(eth0.tx_bytes, 654321);
    assert_eq!(eth0.tx_packets, 20);
}

#[test]
fn short_line_skipped() {
    let buf = format!(
        "{HEADERS}  eth0: 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15\n    lo: 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16\n"
    );
    let lines = parse_bytes(buf.as_bytes());
    let names: Vec<&str> = lines.iter().map(name).collect();
    assert_eq!(names, ["lo"]);
    let lo = &lines[0];
    assert_eq!(
        [lo.rx_bytes, lo.rx_packets, lo.rx_errors, lo.rx_drop],
        [1, 2, 3, 4]
    );
    assert_eq!(
        [lo.tx_bytes, lo.tx_packets, lo.tx_errors, lo.tx_drop],
        [9, 10, 11, 12]
    );
}

#[test]
fn headers_only() {
    let mut out = vec![NetDevLine::default()];
    parse_net_dev(HEADERS.as_bytes(), &mut out).unwrap();
    assert!(out.is_empty());
}

#[test]
fn reparse_keeps_capacity() {
    let buf = fixture_bytes("owner-desktop", "proc/net/dev");
    let mut out = Vec::new();
    parse_net_dev(&buf, &mut out).unwrap();
    let capacity = out.capacity();
    let first = out.clone();
    parse_net_dev(&buf, &mut out).unwrap();
    assert_eq!(out.capacity(), capacity);
    assert_eq!(out, first);
    assert!(out.iter().all(|l| !l.name.spilled()));
}
