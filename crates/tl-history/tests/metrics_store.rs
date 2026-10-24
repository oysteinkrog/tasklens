//! Device metric history store (ADR-017): header, aggregation, series identity across a
//! renumbering and a reboot, crash recovery, retention cap and the span cache.

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tl_history::HistoryError;
use tl_history::format::HEADER_LEN;
use tl_history::metrics::{
    EventKind, METRIC_BLOCK_NS, MetricBlock, MetricConfig, MetricHeader, MetricHistory,
    MetricLoadReport, MetricSpan, SeriesKey, Tier, decode_metric_block, metric_segment_file_name,
    purge,
};
use tl_model::ids::MetricId;
use tl_model::series::{Sample, SampleFlags};
use tl_model::time::{BootId, TimeNs};

const SEC: u64 = 1_000_000_000;
const MIN: u64 = 60 * SEC;
/// A whole number of minutes, so realtime buckets line up with boottime minutes.
const OFF_S: i64 = 1_699_999_980;
const OFF: i64 = OFF_S * SEC as i64;
const BOOT_A: BootId = BootId(0xa);
const BOOT_B: BootId = BootId(0xb);

fn cfg(root: &Path, boot: BootId, offset: i64) -> MetricConfig {
    MetricConfig {
        root: root.to_path_buf(),
        boot,
        realtime_offset_ns: offset,
        max_disk_bytes: 1 << 30,
        sync: false,
    }
}

fn open(cfg: MetricConfig, now: TimeNs) -> (MetricHistory, MetricLoadReport) {
    MetricHistory::open(cfg, now, Instant::now() + Duration::from_secs(60)).unwrap()
}

fn key(device: &str, metric: MetricId) -> SeriesKey {
    SeriesKey {
        device_key: Arc::from(device),
        metric,
    }
}

fn series<'a>(span: &'a MetricSpan, k: &SeriesKey) -> &'a [tl_history::metrics::MetricPoint] {
    &span.series.iter().find(|(s, _)| s == k).unwrap().1
}

/// Pushes one sample per second of `value(sec)` for seconds `from..to` and ticks each second.
fn feed(
    h: &mut MetricHistory,
    slots: &[tl_history::metrics::MetricSlot],
    from: u64,
    to: u64,
    value: impl Fn(usize, u64) -> f32,
) {
    for sec in from..to {
        for (i, &slot) in slots.iter().enumerate() {
            h.push(slot, Sample::valid(sec * SEC, value(i, sec)));
        }
        h.tick(sec * SEC).unwrap();
    }
}

fn files(root: &Path, tier: Tier) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = fs::read_dir(root.join(tier.dir_name()))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

/// Every block of a segment file, decoded.
fn blocks(path: &Path) -> Vec<MetricBlock> {
    let bytes = fs::read(path).unwrap();
    let mut off = HEADER_LEN;
    let mut out = Vec::new();
    while off < bytes.len() {
        let (b, used) = decode_metric_block(&bytes[off..], off as u64).unwrap();
        out.push(b);
        off += used;
    }
    out
}

#[test]
fn metric_header_roundtrip() {
    let h = MetricHeader {
        tier: Tier::S10,
        boot_id: BootId(0x0123_4567_89ab_cdef),
        realtime_offset_ns: -5,
        start_t_ns: 42,
    };
    let b = h.encode();
    assert_eq!(&b[0..4], b"TLM\x01");
    assert_eq!(&b[4..6], &[1, 0]);
    assert_eq!(b[6], 1);
    assert_eq!(&b[8..16], &0x0123_4567_89ab_cdefu64.to_le_bytes());
    assert_eq!(&b[16..24], &(-5i64).to_le_bytes());
    assert_eq!(&b[24..32], &42u64.to_le_bytes());
    assert!(b[32..].iter().all(|&x| x == 0));
    assert_eq!(MetricHeader::decode(&b).unwrap(), h);

    let mut bad = b;
    bad[6] = 3;
    assert!(matches!(
        MetricHeader::decode(&bad),
        Err(HistoryError::BadHeader)
    ));
    let mut bad = b;
    bad[3] = 2;
    assert!(matches!(
        MetricHeader::decode(&bad),
        Err(HistoryError::BadHeader)
    ));
    let mut bad = b;
    bad[4] = 2;
    assert!(matches!(
        MetricHeader::decode(&bad),
        Err(HistoryError::Version(2))
    ));
    assert!(matches!(
        MetricHeader::decode(&b[..63]),
        Err(HistoryError::BadHeader)
    ));

    assert_eq!(
        metric_segment_file_name(1_700_000_000),
        "seg-1700000000.tlm"
    );
    assert_eq!(metric_segment_file_name(42), "seg-0000000042.tlm");
}

/// ADR-017: a single 100% 10 Hz sample in a second gives 100 in the 1 s tier and as the max of
/// the 10 s and 1 min points.
#[test]
fn aggregation_keeps_single_spike() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    let cpu = h
        .register(
            &Arc::from("cpu"),
            &Arc::from("CPU"),
            MetricId::CPU_TOTAL.with_tier_1hz(),
        )
        .unwrap();
    for tenth in 0..1200u64 {
        let v = if tenth == 305 { 100.0 } else { 0.0 };
        h.push(cpu, Sample::valid(tenth * SEC / 10, v));
        if tenth % 10 == 0 {
            h.tick(tenth * SEC / 10).unwrap();
        }
    }
    h.close(120 * SEC).unwrap();
    let k = key("cpu", MetricId::CPU_TOTAL);

    let s1 = h
        .read(Tier::S1, OFF, OFF + 119 * SEC as i64, 120 * SEC)
        .unwrap();
    assert_eq!(s1.step_ns, SEC);
    let p = series(&s1, &k);
    assert_eq!(p.len(), 120);
    assert_eq!(p[30].mean, 100.0);
    assert!(p.iter().enumerate().all(|(i, p)| i == 30 || p.mean == 0.0));

    // The blocks on disk.
    let s1_blocks = blocks(&files(dir.path(), Tier::S1)[0]);
    assert_eq!(s1_blocks.len(), 2);
    assert_eq!(s1_blocks[0].columns[0].mean[30], 100.0);
    assert!(s1_blocks[0].columns[0].min.is_none());
    let s10 = blocks(&files(dir.path(), Tier::S10)[0]);
    let c = &s10[0].columns[0];
    assert_eq!(c.mean.len(), 6);
    assert_eq!(c.max.as_ref().unwrap()[3], 100.0);
    assert_eq!(c.min.as_ref().unwrap()[3], 0.0);
    assert!((c.mean[3] - 10.0).abs() < 1e-5);
    let m1 = blocks(&files(dir.path(), Tier::M1)[0]);
    let c = &m1[0].columns[0];
    assert_eq!(c.max.as_ref().unwrap()[0], 100.0);
    assert!((c.mean[0] - 100.0 / 60.0).abs() < 1e-5);
    assert_eq!(m1[1].columns[0].max.as_ref().unwrap()[0], 0.0);
    // The catalog carries the base metric, without the 1 Hz tier bit.
    assert_eq!(
        m1[0].catalog.as_ref().unwrap()[0].metric,
        MetricId::CPU_TOTAL
    );
    assert!(m1[0].gap_before);
    assert!(!m1[1].gap_before);
    assert!(m1[1].catalog.is_none());

    // The same through reads served from disk.
    let s10 = h
        .read(Tier::S10, OFF, OFF + 119 * SEC as i64, 120 * SEC)
        .unwrap();
    let p = series(&s10, &k);
    assert_eq!((p[3].max, p[3].min), (100.0, 0.0));
    assert!(p.iter().enumerate().all(|(i, p)| i == 3 || p.max == 0.0));
    let m1 = h
        .read(Tier::M1, OFF, OFF + 119 * SEC as i64, 120 * SEC)
        .unwrap();
    let p = series(&m1, &k);
    assert_eq!(p[0].max, 100.0);
    assert_eq!(p[1].max, 0.0);
}

/// ADR-017: a series written in one session and read back after its slot changed and after a
/// boot id change continues under its device key, with a gap and a `Boot` event between.
#[test]
fn series_continue_across_renumbering_and_boot() {
    let dir = tempfile::tempdir().unwrap();
    let disk = key("disk:wwn-1", MetricId::CPU_TOTAL);
    let other = key("memory", MetricId::MEM_USED);

    let (mut a, report) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    assert_eq!(report.previous_boot, None);
    let slot_a = a
        .register(&disk.device_key, &Arc::from("Samsung 990"), disk.metric)
        .unwrap();
    feed(&mut a, &[slot_a], 0, 600, |_, _| 5.0);
    a.close(600 * SEC).unwrap();
    drop(a);

    // An hour after session A ended, a new boot: boottime starts at 0 again.
    let off_b = OFF + 4200 * SEC as i64;
    let (mut b, report) = open(cfg(dir.path(), BOOT_B, off_b), 0);
    assert_eq!(report.previous_boot, Some(BOOT_A));
    let mem = b
        .register(&other.device_key, &Arc::from("Memory"), other.metric)
        .unwrap();
    let slot_b = b
        .register(&disk.device_key, &Arc::from("Samsung 990"), disk.metric)
        .unwrap();
    assert_ne!(slot_a, slot_b);
    feed(&mut b, &[mem, slot_b], 0, 600, |i, _| {
        if i == 0 { 1.0 } else { 7.0 }
    });
    b.close(600 * SEC).unwrap();
    drop(b);

    let (mut c, report) = open(cfg(dir.path(), BOOT_B, off_b), 600 * SEC);
    assert_eq!(report.previous_boot, Some(BOOT_B));
    let span = c
        .read(Tier::M1, OFF, OFF + 4800 * SEC as i64, 600 * SEC)
        .unwrap();
    assert_eq!(span.step_ns, MIN);
    let p = series(&span, &disk);
    assert_eq!(p.len(), 81);
    assert!(p[..10].iter().all(|p| p.mean == 5.0), "{:?}", &p[..10]);
    assert!(p[10..70].iter().all(|p| p.mean.is_nan()), "gap");
    assert!(p[70..80].iter().all(|p| p.mean == 7.0));
    assert!(p[80].mean.is_nan());
    let p = series(&span, &other);
    assert!(p[..70].iter().all(|p| p.mean.is_nan()));
    assert!(p[70..80].iter().all(|p| p.mean == 1.0));

    assert_eq!(span.events.len(), 1);
    assert_eq!(span.events[0].event.kind, EventKind::Boot);
    assert_eq!(span.events[0].start_realtime_ns, off_b);

    // The 1 s ring of the restarted session holds session B's last hour.
    let s1 = c
        .read(Tier::S1, off_b, off_b + 599 * SEC as i64, 600 * SEC)
        .unwrap();
    assert!(series(&s1, &disk).iter().all(|p| p.mean == 7.0));
}

/// ADR-017: a segment whose last block was cut short is read up to the bad crc.
#[test]
fn truncated_last_block_read_up_to_bad_crc() {
    let dir = tempfile::tempdir().unwrap();
    let k = key("cpu", MetricId::CPU_TOTAL);
    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    let slot = h
        .register(&k.device_key, &Arc::from("CPU"), k.metric)
        .unwrap();
    feed(&mut h, &[slot], 0, 300, |_, s| (s / 60) as f32);
    h.close(300 * SEC).unwrap();
    drop(h);

    let path = files(dir.path(), Tier::M1).remove(0);
    assert_eq!(blocks(&path).len(), 5);
    let len = fs::metadata(&path).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(len - 3)
        .unwrap();

    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 300 * SEC);
    let span = h
        .read(Tier::M1, OFF, OFF + 299 * SEC as i64, 300 * SEC)
        .unwrap();
    let p = series(&span, &k);
    let means: Vec<f32> = p.iter().map(|p| p.mean).collect();
    assert_eq!(&means[..4], &[0.0, 1.0, 2.0, 3.0]);
    assert!(means[4].is_nan());
    // Segment end follows the last good block.
    let segs = h.segments(Tier::M1);
    assert_eq!(segs[0].2, OFF + 4 * MIN as i64);
    // The 10 s tier is untouched.
    let span = h
        .read(Tier::S10, OFF, OFF + 299 * SEC as i64, 300 * SEC)
        .unwrap();
    assert_eq!(series(&span, &k)[29].mean, 4.0);
}

/// Files are 0600 in 0700 directories, named by their CLOCK_REALTIME start.
#[test]
fn segment_files_and_modes() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    let slot = h
        .register(&Arc::from("cpu"), &Arc::from("CPU"), MetricId::CPU_TOTAL)
        .unwrap();
    feed(&mut h, &[slot], 0, 1300, |_, _| 1.0);
    h.close(1300 * SEC).unwrap();
    for tier in Tier::ALL {
        let mode = fs::metadata(dir.path().join(tier.dir_name()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        for f in files(dir.path(), tier) {
            assert_eq!(
                fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    let names = |tier| -> Vec<String> {
        files(dir.path(), tier)
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
            .collect()
    };
    // 1 s segments cover 10 minutes; 22 minutes give three.
    assert_eq!(
        names(Tier::S1),
        [0, 600, 1200].map(|s| metric_segment_file_name(OFF_S + s))
    );
    assert_eq!(names(Tier::S10), [metric_segment_file_name(OFF_S)]);
    assert_eq!(names(Tier::M1), [metric_segment_file_name(OFF_S)]);

    // A restart in the same boot that writes minute 0 again takes the next free name.
    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 1300 * SEC);
    let slot = h
        .register(&Arc::from("cpu"), &Arc::from("CPU"), MetricId::CPU_TOTAL)
        .unwrap();
    h.push(slot, Sample::valid(5 * SEC, 1.0));
    h.close(6 * SEC).unwrap();
    assert_eq!(
        names(Tier::M1),
        [
            metric_segment_file_name(OFF_S),
            metric_segment_file_name(OFF_S + 1)
        ]
    );
}

/// ADR-017 cap: over `history.device_metrics_max_mb`, the closed segments with the oldest end
/// go first, of any tier.
#[test]
fn cap_deletes_oldest_segments_first() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    let slots: Vec<_> = (0..20)
        .map(|i| {
            h.register(
                &Arc::from(format!("disk:{i}")),
                &Arc::from("Disk"),
                MetricId::CPU_TOTAL,
            )
            .unwrap()
        })
        .collect();
    feed(&mut h, &slots, 0, 3600, |i, s| {
        ((s * 7 + i as u64 * 13) % 100) as f32
    });
    h.close(3600 * SEC).unwrap();
    let all: Vec<(std::path::PathBuf, i64)> = Tier::ALL
        .into_iter()
        .flat_map(|t| h.segments(t))
        .map(|(p, _, end)| (p, end))
        .collect();
    let total = h.disk_bytes();
    drop(h);

    let mut capped = cfg(dir.path(), BOOT_A, OFF);
    capped.max_disk_bytes = total / 2;
    let (h, _) = open(capped, 3600 * SEC);
    assert!(h.disk_bytes() <= total / 2);
    let kept: Vec<std::path::PathBuf> = Tier::ALL
        .into_iter()
        .flat_map(|t| h.segments(t))
        .map(|(p, _, _)| p)
        .collect();
    let deleted: Vec<&(std::path::PathBuf, i64)> =
        all.iter().filter(|(p, _)| !kept.contains(p)).collect();
    assert!(!deleted.is_empty());
    let newest_deleted = deleted.iter().map(|(_, e)| *e).max().unwrap();
    for (p, end) in &all {
        if kept.contains(p) {
            assert!(
                *end >= newest_deleted,
                "{p:?} kept but ends before a deleted segment"
            );
            assert!(p.exists());
        } else {
            assert!(!p.exists());
        }
    }
}

/// `GAP_BEFORE` ends the point being built; a sample that is not valid is no input.
#[test]
fn gap_before_ends_point() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    let k = key("cpu", MetricId::CPU_TOTAL);
    let slot = h
        .register(&k.device_key, &Arc::from("CPU"), k.metric)
        .unwrap();
    h.push(slot, Sample::valid(5 * SEC + 200_000_000, 50.0));
    let mut gap = Sample::valid(5 * SEC + 500_000_000, 90.0);
    gap.flags |= SampleFlags::GAP_BEFORE;
    h.push(slot, gap);
    h.push(slot, Sample::valid(6 * SEC, 10.0));
    h.push(slot, Sample::gap(7 * SEC));
    h.push(slot, Sample::valid(8 * SEC, 20.0));
    h.mark_gap();
    h.push(slot, Sample::valid(8 * SEC + 500_000_000, 99.0));
    h.push(slot, Sample::valid(9 * SEC, f32::NAN));
    h.close(10 * SEC).unwrap();
    let span = h
        .read(Tier::S1, OFF, OFF + 9 * SEC as i64, 10 * SEC)
        .unwrap();
    let means: Vec<f32> = series(&span, &k).iter().map(|p| p.mean).collect();
    assert_eq!(&means[5..7], &[50.0, 10.0]);
    assert!(means[7].is_nan());
    assert_eq!(means[8], 20.0);
    assert!(means[9].is_nan());
}

/// A span follows the blocks written after it was filled, shows the minute not yet written,
/// and is dropped 60 s after its last request.
#[test]
fn span_cache_follows_writes_and_expires() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    let k = key("net", MetricId::CPU_TOTAL);
    let slot = h
        .register(&k.device_key, &Arc::from("eth0"), k.metric)
        .unwrap();
    let value = |_: usize, s: u64| (s / 60) as f32;
    feed(&mut h, &[slot], 0, 300, value);
    // Six hours in 1 min buckets.
    let range = 6 * 3600 * SEC as i64;
    let read = |h: &mut MetricHistory, now: u64| {
        let to = OFF + now as i64;
        h.read(Tier::M1, to - range, to, now).unwrap()
    };
    let span = read(&mut h, 300 * SEC);
    assert_eq!(span.step_ns, MIN);
    assert_eq!(h.cached_spans(), 1);
    let valid = |s: &MetricSpan| -> Vec<f32> {
        series(s, &k)
            .iter()
            .map(|p| p.mean)
            .filter(|m| !m.is_nan())
            .collect()
    };
    // Minutes 0..=3 are written; minute 4 is still in RAM.
    assert_eq!(valid(&span), [0.0, 1.0, 2.0, 3.0, 4.0]);
    assert!(span.series[0].1.len() <= tl_history::MAX_POINTS);

    feed(&mut h, &[slot], 300, 420, value);
    let span = read(&mut h, 420 * SEC);
    assert_eq!(h.cached_spans(), 1);
    assert_eq!(valid(&span), [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

    h.tick(479 * SEC).unwrap();
    assert_eq!(h.cached_spans(), 1);
    h.tick(480 * SEC).unwrap();
    assert_eq!(h.cached_spans(), 0);
    assert_eq!(h.since_realtime_ns(Tier::M1), Some(OFF));
}

/// Events ride on the next 1 min block, or a block of their own when no data comes.
#[test]
fn events_stored_in_minute_tier() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    h.push_event(tl_history::metrics::HistEvent {
        kind: EventKind::OomKill,
        start_t_ns: 30 * SEC,
        duration_ms: 0,
        peak: 0.0,
        device_key: None,
        process: Some(Arc::from("firefox")),
    });
    h.tick(30 * SEC).unwrap();
    h.tick(2 * MIN + 2 * SEC).unwrap();
    assert!(files(dir.path(), Tier::S1).is_empty());
    let m1 = blocks(&files(dir.path(), Tier::M1)[0]);
    assert_eq!(m1.len(), 1);
    assert_eq!(m1[0].events[0].kind, EventKind::OomKill);
    assert!(m1[0].columns.is_empty());
    let span = h
        .read(Tier::M1, OFF, OFF + 3 * MIN as i64, 3 * MIN)
        .unwrap();
    assert_eq!(span.events.len(), 1);
    assert_eq!(span.events[0].start_realtime_ns, OFF + 30 * SEC as i64);
    assert_eq!(span.events[0].event.process.as_deref(), Some("firefox"));
}

/// `history.device_metrics = false` deletes `metrics/`.
#[test]
fn purge_deletes_directory() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("metrics");
    let (mut h, _) = open(cfg(&root, BOOT_A, OFF), 0);
    let slot = h
        .register(&Arc::from("cpu"), &Arc::from("CPU"), MetricId::CPU_TOTAL)
        .unwrap();
    feed(&mut h, &[slot], 0, 70, |_, _| 1.0);
    h.close(70 * SEC).unwrap();
    drop(h);
    assert!(root.join("1m").exists());
    purge(&root).unwrap();
    assert!(!root.exists());
    purge(&root).unwrap();
}

/// A header-less file from a crash at segment creation is deleted; another version is kept.
#[test]
fn open_skips_bad_files() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    drop(h);
    let s10 = dir.path().join("10s");
    fs::write(s10.join("seg-0000000001.tlm"), b"TLM").unwrap();
    let mut other = MetricHeader {
        tier: Tier::S10,
        boot_id: BOOT_A,
        realtime_offset_ns: OFF,
        start_t_ns: 0,
    }
    .encode();
    other[4] = 2;
    fs::write(s10.join("seg-0000000002.tlm"), other).unwrap();
    let (_, report) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    assert_eq!(report.other_version, 1);
    assert_eq!(report.segments, 0);
    assert!(!s10.join("seg-0000000001.tlm").exists());
    assert!(s10.join("seg-0000000002.tlm").exists());
    assert_eq!(METRIC_BLOCK_NS, MIN);
}

/// Boottime seconds of every valid 1 s point stored in the 1 s tier, one entry per stored copy.
fn stored_seconds(root: &Path) -> Vec<u64> {
    let mut out = Vec::new();
    for f in files(root, Tier::S1) {
        for b in blocks(&f) {
            for c in &b.columns {
                for i in (0..c.mean.len()).filter(|&i| c.valid[i / 8] & (1 << (i % 8)) != 0) {
                    out.push(b.start_t_ns / SEC + i as u64);
                }
            }
        }
    }
    out.sort_unstable();
    out
}

/// A restart in the middle of a minute writes only the new seconds of that minute. The seconds
/// loaded back into the ring are on disk already and stay stored once.
#[test]
fn restart_mid_minute_stores_each_second_once() {
    let dir = tempfile::tempdir().unwrap();
    let k = key("cpu", MetricId::CPU_TOTAL);
    let (mut a, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    let slot = a
        .register(&k.device_key, &Arc::from("CPU"), k.metric)
        .unwrap();
    feed(&mut a, &[slot], 0, 90, |_, s| s as f32);
    a.close(90 * SEC).unwrap();
    drop(a);

    // The same boot 10 s later: the ring loads seconds 0 to 89.
    let (mut b, report) = open(cfg(dir.path(), BOOT_A, OFF), 100 * SEC);
    assert_eq!(report.ring_points, 90);
    let slot = b
        .register(&k.device_key, &Arc::from("CPU"), k.metric)
        .unwrap();
    feed(&mut b, &[slot], 100, 180, |_, s| 1000.0 + s as f32);
    b.close(180 * SEC).unwrap();

    let expected: Vec<u64> = (0..90).chain(100..180).collect();
    assert_eq!(stored_seconds(dir.path()), expected);

    // Minute 1 has a point in each session's segment: 60 to 89 (mean 74.5) and 100 to 119
    // (mean 1109.5). The format stores no counts, so the read averages the two points.
    let span = b
        .read(Tier::M1, OFF, OFF + 179 * SEC as i64, 180 * SEC)
        .unwrap();
    let p = series(&span, &k);
    assert_eq!(p[0].mean, 29.5);
    assert_eq!((p[1].mean, p[1].min, p[1].max), (592.0, 60.0, 1119.0));
    assert_eq!(p[2].mean, 1149.5);
    let span = b
        .read(
            Tier::S1,
            OFF + 60 * SEC as i64,
            OFF + 119 * SEC as i64,
            180 * SEC,
        )
        .unwrap();
    let p = series(&span, &k);
    assert_eq!((p[29].mean, p[40].mean), (89.0, 1100.0));
    assert!(p[30..40].iter().all(|p| p.mean.is_nan()));
}

/// The `Boot` event starts at boottime 0, before the first segment of an engine that started
/// later in that boot. A read of a range that ends before that segment still finds it.
#[test]
fn boot_event_before_its_segment_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let k = key("cpu", MetricId::CPU_TOTAL);
    let (mut a, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    let slot = a
        .register(&k.device_key, &Arc::from("CPU"), k.metric)
        .unwrap();
    feed(&mut a, &[slot], 0, 120, |_, _| 1.0);
    a.close(120 * SEC).unwrap();
    drop(a);

    // Boot B is an hour later and the engine starts 10 minutes into it.
    let off_b = OFF + 3600 * SEC as i64;
    let (mut b, report) = open(cfg(dir.path(), BOOT_B, off_b), 600 * SEC);
    assert_eq!(report.previous_boot, Some(BOOT_A));
    let slot = b
        .register(&k.device_key, &Arc::from("CPU"), k.metric)
        .unwrap();
    feed(&mut b, &[slot], 600, 720, |_, _| 2.0);
    b.close(720 * SEC).unwrap();
    drop(b);

    let (mut c, _) = open(cfg(dir.path(), BOOT_B, off_b), 720 * SEC);
    assert_eq!(c.segments(Tier::M1)[1].1, off_b + 600 * SEC as i64);
    let span = c
        .read(
            Tier::M1,
            off_b - 30 * MIN as i64,
            off_b + 5 * MIN as i64,
            720 * SEC,
        )
        .unwrap();
    assert!(span.series.is_empty());
    assert_eq!(span.events.len(), 1);
    assert_eq!(span.events[0].event.kind, EventKind::Boot);
    assert_eq!(span.events[0].start_realtime_ns, off_b);
}

/// An event goes into a 1 min block that ends after it starts, so it never starts after the end
/// of its segment. One that starts after the minute being written waits for a later block.
#[test]
fn event_waits_for_a_block_that_ends_after_its_start() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    let slot = h
        .register(&Arc::from("cpu"), &Arc::from("CPU"), MetricId::CPU_TOTAL)
        .unwrap();
    feed(&mut h, &[slot], 60, 120, |_, _| 1.0);
    h.push_event(tl_history::metrics::HistEvent {
        kind: EventKind::Heat,
        start_t_ns: 121 * SEC,
        duration_ms: 0,
        peak: 95.0,
        device_key: Some(Arc::from("cpu")),
        process: None,
    });
    // Minute 1 is written without the event; no samples come for minute 2.
    h.tick(122 * SEC).unwrap();
    let m1 = blocks(&files(dir.path(), Tier::M1)[0]);
    assert_eq!(m1.len(), 1);
    assert!(m1[0].events.is_empty());
    h.tick(182 * SEC).unwrap();
    let m1 = blocks(&files(dir.path(), Tier::M1)[0]);
    assert_eq!(m1.len(), 2);
    assert_eq!(m1[1].start_t_ns, 2 * MIN);
    assert_eq!(m1[1].events[0].kind, EventKind::Heat);
    assert!(h.segments(Tier::M1)[0].2 > OFF + 121 * SEC as i64);
}

/// Retention also runs while no samples come: a segment expires as `tick` goes on, the open
/// segment of a tier included, and the next block starts a new segment.
#[test]
fn retention_runs_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _) = open(cfg(dir.path(), BOOT_A, OFF), 0);
    let slot = h
        .register(&Arc::from("cpu"), &Arc::from("CPU"), MetricId::CPU_TOTAL)
        .unwrap();
    feed(&mut h, &[slot], 0, 120, |_, _| 1.0);
    h.tick(122 * SEC).unwrap();
    assert_eq!(files(dir.path(), Tier::S1).len(), 1);

    // The segment expires an hour after its last block ends, at 3,720 s. With no samples in
    // between, retention runs once a minute, so the delete comes at most a minute later.
    h.tick(3719 * SEC).unwrap();
    assert_eq!(files(dir.path(), Tier::S1).len(), 1);
    h.tick(3779 * SEC).unwrap();
    assert!(files(dir.path(), Tier::S1).is_empty());
    assert!(h.segments(Tier::S1).is_empty());
    assert_eq!(files(dir.path(), Tier::S10).len(), 1);
    assert_eq!(files(dir.path(), Tier::M1).len(), 1);

    feed(&mut h, &[slot], 3800, 3870, |_, _| 2.0);
    assert_eq!(
        files(dir.path(), Tier::S1),
        [dir.path()
            .join("1s")
            .join(metric_segment_file_name(OFF_S + 3780))]
    );
}
