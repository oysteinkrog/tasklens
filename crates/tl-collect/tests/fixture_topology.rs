//! `read_topology` against the captured `owner-desktop` fixture and the hand-made CPU edge
//! fixtures (E04-009).

mod common;

use tl_collect::parse::topology::{CacheLevel, parse_cache_size_kib, read_topology};
use tl_model::system::CoreKind;

fn kinds(info: &tl_collect::parse::topology::TopologyInfo) -> Vec<CoreKind> {
    info.topology.cores.iter().map(|c| c.kind).collect()
}

#[test]
fn owner_desktop_snapshot() {
    let info = read_topology(&common::fixture("owner-desktop")).unwrap();
    assert_eq!(
        u32::from(info.logical_count),
        info.topology.online.count(),
        "logical_count is the number of online ids"
    );
    assert_eq!(usize::from(info.cores), info.topology.cores.len());
    insta::assert_debug_snapshot!(info);
}

#[test]
fn hotplug_offline_cpu() {
    let before = read_topology(&common::fixture_step("cpu-hotplug", 0)).unwrap();
    assert_eq!(before.logical_count, 4);
    assert_eq!(before.topology.cores.len(), 2);

    let info = read_topology(&common::fixture_step("cpu-hotplug", 1)).unwrap();
    assert_eq!(info.logical.len(), 4);
    let cpu2 = &info.logical[2];
    assert_eq!(cpu2.id, 2);
    assert!(!cpu2.online);
    assert_eq!(cpu2.package, None);
    assert_eq!(cpu2.core_id, None);
    assert_eq!(cpu2.thread_index, None);
    assert_eq!(info.logical[3].thread_index, Some(1));

    let logical: Vec<Vec<u16>> = info
        .topology
        .cores
        .iter()
        .map(|c| c.logical.to_vec())
        .collect();
    assert_eq!(logical, [vec![0, 1], vec![3]]);
    assert_eq!(info.logical_count, 3);
    assert_eq!(info.cores, 2);
    assert_eq!(info.sockets, 1);
    assert!(!info.topology.online.contains(2));
}

#[test]
fn hybrid_intel_kinds_and_l3() {
    let info = read_topology(&common::fixture("hybrid-intel")).unwrap();
    assert_eq!(
        kinds(&info),
        [
            CoreKind::Performance,
            CoreKind::Performance,
            CoreKind::Efficiency,
            CoreKind::Efficiency
        ]
    );
    assert_eq!(
        info.caches.l3,
        Some(CacheLevel {
            instances: 1,
            instance_kib: 12288,
            total_kib: 12288
        })
    );
    assert!(info.topology.cores.iter().all(|c| c.l3_group == 0));
}

#[test]
fn capacity_kinds() {
    let info = read_topology(&common::fixture("capacity-hybrid")).unwrap();
    assert_eq!(
        kinds(&info),
        [
            CoreKind::Performance,
            CoreKind::Performance,
            CoreKind::Efficiency,
            CoreKind::Efficiency
        ]
    );
    assert_eq!(info.caches.l3, None);
}

#[test]
fn uniform_capacity_is_standard() {
    // Every owner-desktop CPU has cpu_capacity 1024, and there are no hybrid lists.
    let info = read_topology(&common::fixture("owner-desktop")).unwrap();
    assert!(info.logical.iter().all(|c| c.capacity == Some(1024)));
    assert!(!info.topology.cores.is_empty());
    assert!(kinds(&info).iter().all(|k| *k == CoreKind::Standard));
}

#[test]
fn cache_size_suffixes() {
    assert_eq!(parse_cache_size_kib(b"32K").unwrap(), 32);
    assert_eq!(parse_cache_size_kib(b"4M").unwrap(), 4096);
    assert_eq!(parse_cache_size_kib(b"1536").unwrap(), 1);
    assert_eq!(parse_cache_size_kib(b"1G\n").unwrap(), 1024 * 1024);
    for bad in [&b""[..], b"K", b"x", b"-1K"] {
        assert!(parse_cache_size_kib(bad).is_err());
    }
}

#[test]
fn missing_present_and_possible_is_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let cpu = dir.path().join("sys/devices/system/cpu");
    std::fs::create_dir_all(&cpu).unwrap();
    std::fs::write(cpu.join("online"), "0\n").unwrap();
    let err = read_topology(&tl_collect::SysRoot::at(dir.path())).unwrap_err();
    assert!(matches!(
        err,
        tl_collect::CollectError::Parse {
            file: "cpu/present",
            what: "missing"
        }
    ));

    // `possible` stands in for a missing `present`.
    std::fs::write(cpu.join("possible"), "0\n").unwrap();
    let info = read_topology(&tl_collect::SysRoot::at(dir.path())).unwrap();
    assert_eq!(info.logical.len(), 1);
    assert!(info.topology.cores.is_empty());
}
