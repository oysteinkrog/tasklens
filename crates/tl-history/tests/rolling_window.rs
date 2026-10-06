//! Rolling window: 10 closed blocks plus the open one, in RAM only (ADR-012, D-010, R-103).

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use tl_history::format::Block;
use tl_history::read::{MetaTable, Metric, state_at, system_series};
use tl_history::rolling::{ROLLING_CLOSED_BLOCKS, RollingWindow};
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::history::RecordSink;
use tl_model::ids::{DeviceIdx, GroupKey, MetricId, ProcessKey, SeriesId};
use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::series::Sample;
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};

const SEC: u64 = 1_000_000_000;
const MIN: u64 = 60 * SEC;
const BOOT: BootId = BootId(0xb007);

fn key(i: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 5_000 + u64::from(i),
        boot_id: BOOT,
        pid: 1_000 + i,
    }
}

fn record(i: u32) -> ProcessRecord {
    let text: Arc<str> = Arc::from(format!("proc{i}"));
    ProcessRecord {
        key: key(i),
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: text.clone(),
            name: text.clone(),
            exe: Some(Arc::from(format!("/usr/bin/proc{i}"))),
            cmdline: None,
            user: Arc::from("alice"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: text,
        }),
        group: GroupKey(0),
        state: ProcState::Running,
        flags: ProcFlags::default(),
        nice: 0,
        threads: 1,
        cpu_pct: 1.0,
        rss_anon_bytes: 8 << 20,
        rss_bytes: 0,
        pss_bytes: 0,
        uss_bytes: 0,
        swap_bytes: 0,
        io_read_bps: 0.0,
        io_write_bps: 0.0,
        net_rx_bps: 0.0,
        net_tx_bps: 0.0,
        gpu_util_pct: 0.0,
        gpu_mem_bytes: 0,
        validity: FieldValidity::all(Validity::Valid),
    }
}

fn sweep_of(ids: impl Iterator<Item = u32>) -> Arc<[ProcessRecord]> {
    ids.map(record).collect::<Vec<_>>().into()
}

fn cpu() -> SeriesId {
    SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::CPU_TOTAL,
    }
}

/// Feeds one process sweep and one system sample per second for `secs` seconds.
fn feed_steady(w: &mut RollingWindow, secs: u64, procs: u32) {
    let records = sweep_of(0..procs);
    for s in 0..secs {
        let t = s * SEC;
        w.on_system_sample(t, &[(cpu(), Sample::valid(t, 10.0))]);
        w.on_process_sweep(t, &records);
    }
}

fn names(metas: &MetaTable, blocks: &[Arc<Block>], t: u64) -> Vec<(ProcessKey, String)> {
    let mut out = Vec::new();
    for b in blocks {
        if let Some(frame) = state_at(b, t) {
            for p in &frame.procs {
                let name = metas.get(p.meta).map(|m| m.name.to_string());
                out.push((p.key, name.unwrap_or_default()));
            }
        }
    }
    out
}

#[test]
fn keeps_newest_10_minutes_after_15() {
    let mut w = RollingWindow::new(BOOT);
    assert_eq!(w.span(), None);
    feed_steady(&mut w, 15 * 60, 20);
    let (start, end) = w.span().unwrap();
    assert_eq!(end, 899 * SEC);
    let covered = end - start;
    assert!(covered <= 11 * MIN, "covers {covered} ns");
    assert!(covered >= 10 * MIN - SEC, "covers {covered} ns");
    assert_eq!(start, 4 * MIN);

    // 10 decoded closed blocks plus the open block, in time order.
    let (blocks, _) = w.blocks(0, u64::MAX).unwrap();
    assert_eq!(blocks.len(), ROLLING_CLOSED_BLOCKS + 1);
    let starts: Vec<u64> = blocks.iter().map(|b| b.start_t_ns / MIN).collect();
    assert_eq!(starts, (4..=14).collect::<Vec<_>>());
    assert_eq!(blocks[10].deltas.len(), 59);
    assert_eq!(blocks[10].system[0].1.len(), 60);

    // A narrow range returns only the blocks it overlaps.
    let (some, _) = w.blocks(6 * MIN + 30 * SEC, 7 * MIN).unwrap();
    let starts: Vec<u64> = some.iter().map(|b| b.start_t_ns / MIN).collect();
    assert_eq!(starts, vec![6, 7]);
    let (none, _) = w.blocks(0, MIN).unwrap();
    assert!(none.is_empty());
}

#[test]
fn ram_under_16_mib_2000_changing() {
    const PROCS: u32 = 2000;
    let mut w = RollingWindow::new(BOOT);
    w.set_mem_total_kib(64 << 20);
    let mut records: Vec<ProcessRecord> = (0..PROCS).map(record).collect();
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut peak = 0;
    for s in 0..15 * 60u64 {
        // Every process changes CPU and memory past the thresholds every second.
        for r in &mut records {
            let n = next();
            r.cpu_pct = (n % 1000) as f32 / 10.0;
            r.rss_anon_bytes = (64 << 20) + ((n >> 16) % 4096) * (512 << 10);
        }
        let sweep: Arc<[ProcessRecord]> = records.clone().into();
        w.on_process_sweep(s * SEC, &sweep);
        peak = peak.max(w.ram_bytes());
    }
    assert_eq!(w.closed_len(), ROLLING_CLOSED_BLOCKS);
    assert_eq!(w.encode_errors(), 0);
    let ram = w.ram_bytes();
    assert!(ram > 0);
    assert!(peak < 16 << 20, "peak {peak} bytes");
}

#[test]
fn drops_oldest_on_close() {
    let mut w = RollingWindow::new(BOOT);
    // 11 minutes: 10 closed blocks (0..=9) and an open one at 10 min.
    feed_steady(&mut w, 11 * 60, 5);
    assert_eq!(w.closed_len(), ROLLING_CLOSED_BLOCKS);
    assert_eq!(w.span().unwrap().0, 0);

    // The sweep at 11 min closes block 10 and drops block 0.
    let records = sweep_of(0..5);
    w.on_process_sweep(11 * MIN, &records);
    assert_eq!(w.closed_len(), ROLLING_CLOSED_BLOCKS);
    assert_eq!(w.span().unwrap().0, MIN);
    w.on_process_sweep(12 * MIN, &records);
    assert_eq!(w.closed_len(), ROLLING_CLOSED_BLOCKS);
    assert_eq!(w.span().unwrap().0, 2 * MIN);

    // A gap closes the open block too; the window still keeps 10.
    w.mark_gap(12 * MIN + SEC);
    assert_eq!(w.closed_len(), ROLLING_CLOSED_BLOCKS);
    assert_eq!(w.span().unwrap().0, 3 * MIN);
    w.on_process_sweep(20 * MIN, &records);
    let (blocks, _) = w.blocks(20 * MIN, 20 * MIN).unwrap();
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].gap_before);
}

#[test]
fn meta_survives_block_drop() {
    let mut w = RollingWindow::new(BOOT);
    // proc0 lives for 15 minutes; proc1 starts at 5 s and exits at 10 s, in block 0.
    for s in 0..15 * 60u64 {
        let short = (5..10).contains(&s);
        let records = sweep_of((0..2).filter(|i| *i == 0 || short));
        w.on_process_sweep(s * SEC, &records);
    }
    let (start, end) = w.span().unwrap();
    assert_eq!(start, 4 * MIN);
    let (blocks, metas) = w.blocks(start, end).unwrap();
    assert!(blocks.iter().all(|b| b.new_meta.is_empty()));
    let at = names(metas, &blocks, start);
    assert_eq!(at, vec![(key(0), "proc0".to_string())]);
    // The meta of proc0 came from block 0, which was dropped.
    assert_eq!(&*metas.get(0).unwrap().name, "proc0");
    // proc1 is referenced by no kept block, so its meta was pruned.
    assert!(metas.get(1).is_none());

    // A process that starts after the drops gets a new id that resolves too.
    let records = sweep_of([0, 7].into_iter());
    w.on_process_sweep(15 * MIN, &records);
    w.on_process_sweep(15 * MIN + SEC, &records);
    let (blocks, metas) = w.blocks(15 * MIN, 15 * MIN).unwrap();
    let open = blocks.last().unwrap();
    let new = open.keyframe.iter().find(|p| p.key == key(7)).unwrap();
    assert_eq!(new.meta, 2);
    assert_eq!(&*metas.get(new.meta).unwrap().name, "proc7");
}

#[test]
fn deleted_keys_in_ram() {
    let mut w = RollingWindow::new(BOOT);
    feed_steady(&mut w, 3 * 60, 4);
    w.delete_keys(&[key(2)]);
    assert!(w.deleted().contains(&key(2)));
    assert!(!w.deleted().contains(&key(1)));

    let (blocks, _) = w.blocks(0, u64::MAX).unwrap();
    let mut seen = 0;
    for b in &blocks {
        let frame = state_at(b, b.start_t_ns).unwrap();
        let visible: Vec<ProcessKey> = frame
            .procs
            .iter()
            .map(|p| p.key)
            .filter(|k| !w.deleted().contains(k))
            .collect();
        assert_eq!(visible, vec![key(0), key(1), key(3)]);
        seen += 1;
    }
    assert_eq!(seen, 3);

    // A new window starts with no deleted keys.
    assert_eq!(*RollingWindow::new(BOOT).deleted(), Default::default());
}

#[test]
fn writes_nothing_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500)).unwrap();
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(&ro).unwrap();

    let mut w = RollingWindow::new(BOOT);
    w.set_mem_total_kib(1 << 20);
    feed_steady(&mut w, 12 * 60, 10);
    w.mark_gap(12 * MIN);
    w.delete_keys(&[key(3)]);
    let (blocks, _) = w.blocks(0, u64::MAX).unwrap();
    assert!(!blocks.is_empty());
    assert!(w.span().is_some());
    assert!(w.ram_bytes() > 0);
    assert_eq!(w.encode_errors(), 0);
    drop(w);

    std::env::set_current_dir(prev).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 0);
}

fn cpu_catalog() -> DeviceCatalog {
    DeviceCatalog {
        version: 1,
        entries: Arc::from(vec![DeviceEntry {
            idx: DeviceIdx(0),
            identity: DeviceIdentity::CpuPackage,
            generation: 0,
            display_name: Arc::from("CPU"),
            present: true,
            parent: None,
            persistent_key: Arc::from("cpu"),
            net_kind: None,
        }]),
    }
}

#[test]
fn catalog_survives_block_drop() {
    let mut w = RollingWindow::new(BOOT);
    let records = sweep_of(0..3);
    let catalog = cpu_catalog();
    // 12 minutes with an unchanged catalog: the block that carried it (block 0) is dropped.
    for s in 0..=12 * 60u64 {
        let t = s * SEC;
        w.on_device_catalog(&catalog);
        w.on_system_sample(t, &[(cpu(), Sample::valid(t, 10.0))]);
        w.on_process_sweep(t, &records);
    }
    let (start, end) = w.span().unwrap();
    assert_eq!(start, 2 * MIN);

    // As the engine reads it: every kept block, then the last 5 minutes.
    let (blocks, _) = w.blocks(0, end).unwrap();
    let from = end - 5 * MIN;
    let points = system_series(blocks.iter().map(|b| &**b), Metric::Cpu, from, end);
    assert!(!points.is_empty());
    assert!(points.len() >= 290, "{} points", points.len());
    assert!(points.iter().all(|p| p.max == 10.0));
    // Only the first returned block carries the catalog; the stored blocks do not grow.
    assert!(blocks[0].catalog.is_some());
    assert!(blocks[1..].iter().all(|b| b.catalog.is_none()));

    // A range that starts after the oldest kept block gets the catalog too.
    let (blocks, _) = w.blocks(from, end).unwrap();
    assert!(blocks[0].catalog.is_some());
    let points = system_series(blocks.iter().map(|b| &**b), Metric::Cpu, from, end);
    assert!(points.len() >= 290, "{} points", points.len());
}
