//! Fixture tests for the /proc/meminfo parser (ADR-013, ADR-016).

mod common;

use common::fixture_bytes;
use tl_collect::CollectError;
use tl_collect::parse::meminfo::{MemInfo, parse_meminfo};

/// The value of `key` in a meminfo buffer, as written (before any scaling).
fn raw_value(buf: &[u8], key: &str) -> u64 {
    let text = std::str::from_utf8(buf).expect("meminfo is ASCII");
    let prefix = format!("{key}:");
    let line = text
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no {key} line"));
    line[prefix.len()..]
        .split_whitespace()
        .next()
        .expect("value")
        .parse()
        .expect("number")
}

fn parse(buf: &[u8]) -> Result<MemInfo, CollectError> {
    let mut out = MemInfo::default();
    parse_meminfo(buf, &mut out).map(|()| out)
}

fn minimal() -> Vec<u8> {
    fixture_bytes("meminfo-minimal", "proc/meminfo")
}

/// The minimal fixture with every line for `key` removed.
fn without(key: &str) -> Vec<u8> {
    let text = String::from_utf8(minimal()).expect("ASCII");
    let prefix = format!("{key}:");
    text.lines()
        .filter(|l| !l.starts_with(&prefix))
        .map(|l| format!("{l}\n"))
        .collect::<String>()
        .into_bytes()
}

#[test]
fn owner_desktop_snapshot() {
    let buf = fixture_bytes("owner-desktop", "proc/meminfo");
    let info = parse(&buf).expect("owner-desktop meminfo parses");
    assert_eq!(info.mem_total, raw_value(&buf, "MemTotal") * 1024);
    insta::assert_debug_snapshot!(info);
}

#[test]
fn minimal_optional_none() {
    let buf = minimal();
    let info = parse(&buf).expect("minimal meminfo parses");
    assert_eq!(info.zswap, None);
    assert_eq!(info.zswapped, None);
    assert_eq!(info.hugepages_total, None);
    assert_eq!(info.hugepages_free, None);
    assert_eq!(info.hugepagesize, None);
    let required = [
        ("MemTotal", info.mem_total),
        ("MemFree", info.mem_free),
        ("MemAvailable", info.mem_available),
        ("Buffers", info.buffers),
        ("Cached", info.cached),
        ("SwapCached", info.swap_cached),
        ("Dirty", info.dirty),
        ("Writeback", info.writeback),
        ("Shmem", info.shmem),
        ("SReclaimable", info.sreclaimable),
        ("SUnreclaim", info.sunreclaim),
        ("Committed_AS", info.committed_as),
        ("CommitLimit", info.commit_limit),
        ("SwapTotal", info.swap_total),
        ("SwapFree", info.swap_free),
    ];
    for (key, value) in required {
        assert_eq!(value, raw_value(&buf, key) * 1024, "{key}");
    }
}

#[test]
fn missing_required_errors() {
    let err = parse(&without("MemAvailable")).expect_err("MemAvailable is required");
    assert!(
        matches!(
            err,
            CollectError::Parse {
                file: "/proc/meminfo",
                what: "missing field"
            }
        ),
        "{err:?}"
    );
}

#[test]
fn hugepage_counts_not_scaled() {
    let mut buf = minimal();
    buf.extend_from_slice(
        b"HugePages_Total:       4\nHugePages_Free:        3\nHugepagesize:       2048 kB\n",
    );
    let info = parse(&buf).expect("parses");
    assert_eq!(info.hugepages_total, Some(4));
    assert_eq!(info.hugepages_free, Some(3));
    assert_eq!(info.hugepagesize, Some(2048 * 1024));
}

#[test]
fn overflow_errors() {
    let mut buf = without("MemTotal");
    buf.extend_from_slice(b"MemTotal: 99999999999999999999 kB\n");
    let err = parse(&buf).expect_err("overflow");
    assert!(
        matches!(
            err,
            CollectError::Parse {
                file: "/proc/meminfo",
                what: "number"
            }
        ),
        "{err:?}"
    );
    // Fits in u64 but not after × 1024.
    let mut buf = without("MemTotal");
    buf.extend_from_slice(format!("MemTotal: {} kB\n", u64::MAX / 1000).as_bytes());
    assert!(matches!(
        parse(&buf),
        Err(CollectError::Parse { what: "number", .. })
    ));
}
