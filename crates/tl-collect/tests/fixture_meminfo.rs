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
fn kernel_split_fields() {
    let buf = fixture_bytes("owner-desktop", "proc/meminfo");
    let info = parse(&buf).expect("owner-desktop meminfo parses");
    let kb = |key| Some(raw_value(&buf, key) * 1024);
    assert_eq!(info.anon_pages, kb("AnonPages"));
    assert_eq!(info.page_tables, kb("PageTables"));
    assert_eq!(info.sec_page_tables, kb("SecPageTables"));
    assert_eq!(info.kernel_stack, kb("KernelStack"));
    assert_eq!(info.percpu, kb("Percpu"));
    assert_eq!(info.vmalloc_used, kb("VmallocUsed"));
    assert_eq!(info.zswap, kb("Zswap"));
}

/// A kernel without Zswap and SecPageTables lines.
#[test]
fn kernel_split_fields_missing() {
    let buf = fixture_bytes("owner-desktop", "proc/meminfo");
    let text = String::from_utf8(buf.clone()).expect("ASCII");
    let old: Vec<u8> = text
        .lines()
        .filter(|l| !l.starts_with("Zswap:") && !l.starts_with("SecPageTables:"))
        .map(|l| format!("{l}\n"))
        .collect::<String>()
        .into_bytes();
    let info = parse(&old).expect("meminfo without the two lines parses");
    assert_eq!(info.zswap, None);
    assert_eq!(info.sec_page_tables, None);
    assert_eq!(info.page_tables, Some(raw_value(&buf, "PageTables") * 1024));
    assert_eq!(info.anon_pages, Some(raw_value(&buf, "AnonPages") * 1024));
    assert_eq!(
        info.kernel_stack,
        Some(raw_value(&buf, "KernelStack") * 1024)
    );
    assert_eq!(info.percpu, Some(raw_value(&buf, "Percpu") * 1024));
    assert_eq!(
        info.vmalloc_used,
        Some(raw_value(&buf, "VmallocUsed") * 1024)
    );
}

#[test]
fn minimal_optional_none() {
    let buf = minimal();
    let info = parse(&buf).expect("minimal meminfo parses");
    assert_eq!(info.zswap, None);
    assert_eq!(info.zswapped, None);
    assert_eq!(info.anon_pages, None);
    assert_eq!(info.page_tables, None);
    assert_eq!(info.sec_page_tables, None);
    assert_eq!(info.kernel_stack, None);
    assert_eq!(info.percpu, None);
    assert_eq!(info.vmalloc_used, None);
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
