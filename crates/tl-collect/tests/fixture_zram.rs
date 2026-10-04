//! zram enumeration and parsers against the `zram-device` hand fixture and the captured
//! `owner-desktop` fixture (ADR-013, PERF-042 to PERF-044, PROC-012).
use std::fmt::Write as _;

use tl_collect::CollectError;
use tl_collect::parse::zram::{self, ZramDevice, ZramMmStat};

mod common;

fn devices(name: &str) -> Vec<ZramDevice> {
    let mut out = Vec::new();
    zram::enumerate(&common::fixture(name), &mut out).expect("enumerate zram devices");
    out
}

#[test]
fn enumerate_two_devices() {
    let devs = devices("zram-device");
    assert_eq!(devs.len(), 2);
    assert_eq!(&*devs[0].name, "zram0");
    assert_eq!(devs[0].disksize_bytes, 8_589_934_592);
    assert_eq!(devs[0].algorithm.as_deref(), Some("zstd"));
    assert_eq!(&*devs[1].name, "zram1");
    assert_eq!(devs[1].disksize_bytes, 0);
    assert_eq!(devs[1].algorithm.as_deref(), Some("lzo-rle"));
}

#[test]
fn mm_stat_fields() {
    let mut devs = devices("zram-device");
    let mut buf = Vec::new();
    let mut out = ZramMmStat::default();
    devs[0].read_mm_stat(&mut buf, &mut out).unwrap();
    assert_eq!(out.orig_data_size, 1_048_576);
    assert_eq!(out.compr_data_size, 262_144);
    assert_eq!(out.mem_used_total, 300_000);
    assert_eq!(out.same_pages, 10);
    // A second read goes through the kept fd and gives the same result.
    let mut again = ZramMmStat::default();
    devs[0].read_mm_stat(&mut buf, &mut again).unwrap();
    assert_eq!(again, out);
}

#[test]
fn stat_sectors() {
    let devs = devices("zram-device");
    let mut buf = Vec::new();
    assert_eq!(devs[0].read_sectors(&mut buf).unwrap(), (800, 1600));
    assert_eq!(devs[1].read_sectors(&mut buf).unwrap(), (0, 0));
}

#[test]
fn short_mm_stat() {
    let mut out = ZramMmStat::default();
    assert!(matches!(
        zram::parse_mm_stat(b"1 2\n", &mut out),
        Err(CollectError::Parse {
            file: "zram mm_stat",
            what: "too few fields"
        })
    ));
    zram::parse_mm_stat(b"1 2 3\n", &mut out).unwrap();
    assert_eq!(
        out,
        ZramMmStat {
            orig_data_size: 1,
            compr_data_size: 2,
            mem_used_total: 3,
            ..ZramMmStat::default()
        }
    );
}

#[test]
fn no_bracket_algorithm() {
    assert_eq!(zram::parse_comp_algorithm(b"lzo zstd\n"), None);
    assert_eq!(
        zram::parse_comp_algorithm(b"lzo lzo-rle [zstd] lz4\n"),
        Some(&b"zstd"[..])
    );
}

#[test]
fn owner_desktop_snapshot() {
    let mut devs = devices("owner-desktop");
    let mut buf = Vec::new();
    let mut report = String::new();
    for dev in &mut devs {
        let mut mm = ZramMmStat::default();
        dev.read_mm_stat(&mut buf, &mut mm).unwrap();
        let sectors = dev.read_sectors(&mut buf).unwrap();
        writeln!(
            report,
            "{} disksize={} algorithm={:?} sectors={sectors:?}\n{mm:#?}",
            dev.name, dev.disksize_bytes, dev.algorithm
        )
        .unwrap();
    }
    insta::assert_snapshot!(report);
}
