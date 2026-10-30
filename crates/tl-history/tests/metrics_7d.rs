//! Accelerated ADR-017 verification: 7 days of synthetic data at 150 series stays under the
//! 64 MB cap, keeps exactly 1 h, 24 h and 7 d per tier, reloads the 1 s ring within 2 s and
//! serves a 7 d request from disk within 5 s (asserted in release builds).
//!
//! One 1 Hz sample per series per second, with synthetic timestamps, so no real time passes.
//! Values come from a hash of (series, second), so every bucket can be checked.

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tl_history::MAX_POINTS;
use tl_history::metrics::{
    DEFAULT_MAX_DISK_BYTES, MetricConfig, MetricHistory, MetricPoint, MetricSpan, SeriesKey, Tier,
};
use tl_model::ids::MetricId;
use tl_model::series::Sample;
use tl_model::time::BootId;

const SEC: u64 = 1_000_000_000;
const DAY_S: u64 = 86_400;
const SECS: u64 = 7 * DAY_S;
const SERIES: usize = 150;
const BOOT: BootId = BootId(0x7d);
/// A whole number of 10 minutes, so realtime buckets line up with boottime.
const OFF_S: i64 = 1_699_999_800;
const OFF: i64 = OFF_S * SEC as i64;
const RING_BUDGET: Duration = Duration::from_secs(2);
/// Test limit for the 7-day read. The ADR-017 budget is 300 ms; the limit is 5 s because this
/// read of a week of blocks from disk can take over 2 s on a busy machine (tasklens-cr0.32). A
/// speed budget belongs in `cargo xtask bench`, and no bench scene covers this one yet.
const READ_7D_BUDGET: Duration = Duration::from_secs(5);

/// 0.0 to 99.9 in steps of 0.1, from a splitmix64 hash.
fn value(series: usize, sec: u64) -> f32 {
    let mut z = sec
        .wrapping_mul(1_000)
        .wrapping_add(series as u64)
        .wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z % 1000) as f32 / 10.0
}

fn key(i: usize) -> SeriesKey {
    SeriesKey {
        device_key: Arc::from(format!("dev:{i}")),
        metric: MetricId::CPU_TOTAL,
    }
}

fn config(root: &Path) -> MetricConfig {
    MetricConfig {
        root: root.to_path_buf(),
        boot: BOOT,
        realtime_offset_ns: OFF,
        max_disk_bytes: DEFAULT_MAX_DISK_BYTES,
        sync: false,
    }
}

fn dir_bytes(root: &Path) -> u64 {
    Tier::ALL
        .into_iter()
        .flat_map(|t| fs::read_dir(root.join(t.dir_name())).unwrap())
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum()
}

fn series<'a>(span: &'a MetricSpan, k: &SeriesKey) -> &'a [MetricPoint] {
    &span.series.iter().find(|(s, _)| s == k).unwrap().1
}

/// The bucket `p` of series `i` against the 1 s values it covers.
fn check_bucket(p: &MetricPoint, step: u64, i: usize) {
    let first = (p.t_realtime_ns / SEC as i64 - OFF_S) as u64;
    let vals: Vec<f32> = (first..first + step / SEC).map(|s| value(i, s)).collect();
    let min = vals.iter().copied().fold(f32::INFINITY, f32::min);
    let max = vals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mean = vals.iter().map(|v| f64::from(*v)).sum::<f64>() / vals.len() as f64;
    assert_eq!(
        (p.min, p.max),
        (min, max),
        "series {i} at {}",
        p.t_realtime_ns
    );
    assert!(
        (f64::from(p.mean) - mean).abs() < 1e-3,
        "series {i}: mean {} want {mean}",
        p.mean
    );
}

#[test]
fn seven_days_150_series() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("metrics_7d");
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let started = Instant::now();
    let (mut h, report) =
        MetricHistory::open(config(&root), 0, Instant::now() + Duration::from_secs(60)).unwrap();
    assert_eq!(report.previous_boot, None);
    let slots: Vec<_> = (0..SERIES)
        .map(|i| {
            let k = key(i);
            h.register(&k.device_key, &Arc::from("Device"), k.metric)
                .unwrap()
        })
        .collect();
    let mut peak_disk = 0u64;
    for sec in 0..SECS {
        let t = sec * SEC;
        for (i, &slot) in slots.iter().enumerate() {
            h.push(slot, Sample::valid(t, value(i, sec)));
        }
        if sec % 10 == 0 {
            h.tick(t).unwrap();
            peak_disk = peak_disk.max(h.disk_bytes());
        }
    }
    let now = SECS * SEC;
    h.close(now).unwrap();
    eprintln!(
        "metrics_7d: generated in {:.1} s, disk {:.2} MiB (peak {:.2} MiB)",
        started.elapsed().as_secs_f64(),
        h.disk_bytes() as f64 / f64::from(1 << 20),
        peak_disk as f64 / f64::from(1 << 20),
    );

    // Under the cap all the time, so the cap never cut into retention.
    assert!(peak_disk < DEFAULT_MAX_DISK_BYTES, "peak {peak_disk} bytes");
    assert_eq!(h.disk_bytes(), dir_bytes(&root));

    // Each tier keeps exactly its retention: the oldest segment reaches back to it, and no
    // segment ends before it.
    let now_rt = OFF + now as i64;
    for tier in Tier::ALL {
        let segs = h.segments(tier);
        let keep_from = now_rt - tier.retention_ns() as i64;
        assert!(
            segs[0].1 <= keep_from,
            "{tier:?} starts after its retention"
        );
        assert!(
            segs.iter().all(|s| s.2 > keep_from),
            "{tier:?} keeps an expired segment"
        );
        let n = (tier.retention_ns() / tier.segment_ns()) as usize;
        assert!(
            segs.len() == n || segs.len() == n + 1,
            "{tier:?}: {} segments",
            segs.len()
        );
    }
    drop(h);

    // Start: the last hour of the 1 s tier comes back into the ring.
    let opened = Instant::now();
    let (mut h, report) =
        MetricHistory::open(config(&root), now, Instant::now() + RING_BUDGET).unwrap();
    let took = opened.elapsed();
    eprintln!("metrics_7d: open and ring load {took:?} (budget {RING_BUDGET:?})");
    assert!(!report.timed_out);
    assert!(took <= RING_BUDGET, "ring load took {took:?}");
    assert_eq!(report.ring_points, SERIES * 3600);
    assert_eq!(report.previous_boot, Some(BOOT));

    // 7 days from disk.
    let started = Instant::now();
    let week = h
        .read(Tier::M1, now_rt - SECS as i64 * SEC as i64, now_rt, now)
        .unwrap();
    let took = started.elapsed();
    eprintln!("metrics_7d: 7 d read {took:?} (budget {READ_7D_BUDGET:?})");
    #[cfg(not(debug_assertions))]
    assert!(took <= READ_7D_BUDGET, "7 d read took {took:?}");
    assert_eq!(week.series.len(), SERIES);
    assert_eq!(week.step_ns, 600 * SEC);
    for (_, points) in &week.series {
        assert!(points.len() <= MAX_POINTS);
        // Every bucket but the one that starts at `now` has data.
        assert!(points[..points.len() - 1].iter().all(|p| !p.mean.is_nan()));
    }
    for i in [0, 77, SERIES - 1] {
        let p = series(&week, &key(i));
        check_bucket(&p[1], week.step_ns, i);
        check_bucket(&p[500], week.step_ns, i);
    }

    // 24 hours from the 10 s tier.
    let day = h
        .read(Tier::S10, now_rt - (DAY_S * SEC) as i64, now_rt, now)
        .unwrap();
    assert_eq!(day.step_ns, 90 * SEC);
    let p = series(&day, &key(5));
    assert!(p.len() <= MAX_POINTS);
    check_bucket(&p[1], day.step_ns, 5);
    check_bucket(&p[900], day.step_ns, 5);

    // The last hour from the ring.
    let hour = h
        .read(Tier::S1, now_rt - (3600 * SEC) as i64, now_rt - 1, now)
        .unwrap();
    assert_eq!(hour.step_ns, 4 * SEC);
    let p = series(&hour, &key(9));
    assert_eq!(p.len(), 900);
    check_bucket(&p[0], hour.step_ns, 9);
    check_bucket(&p[899], hour.step_ns, 9);
    assert_eq!(
        h.since_realtime_ns(Tier::S1),
        Some(now_rt - (3600 * SEC) as i64)
    );
}
