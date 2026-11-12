//! Bench frame statistics and the JSON report (ADR-014 measurement tools, UI-PRF-053).
//!
//! [`FrameProbe`] records, per `on_next_frame` callback, the interval since the previous callback
//! and the UI thread CPU time spent in between. The scene loop (E01-007) turns the samples into a
//! [`BenchReport`] and writes it with [`write_json`].

use std::cell::RefCell;
use std::ffi::OsStr;
use std::fs;
use std::mem;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::Window;
use serde::Serialize;
use tracing::warn;

/// Clocks the UI crate cannot read itself (tl-ui has no rustix or libc, ADR-001).
/// tl-app supplies them (E01-009) as function pointers over rustix::time::clock_gettime (R-144 item 5).
#[derive(Clone, Copy)]
pub struct BenchClocks {
    /// `ClockId::ThreadCPUTime` of the calling thread.
    pub thread_cpu_ns: fn() -> u64,
    /// `ClockId::ProcessCPUTime` (utime + stime of the process).
    pub process_cpu_ns: fn() -> u64,
    /// Called at the start of the measured window: starts the 1 Hz GPU sampler (E01-023).
    pub side_start: fn(),
    /// Called at the end of the measured window: stops the sampler and returns its means.
    pub side_stop: fn() -> SideMeasure,
}

/// What tl-app measures beside the scene over the measured window (E01-023, ADR-014): the GPU
/// probe's NVML means and the probe child's CPU. Both are `None` without a probe or after a probe
/// error.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SideMeasure {
    pub gpu: Option<GpuReport>,
    pub probe_cpu_pct: Option<f64>,
}

/// A [`BenchClocks::side_start`] that does nothing, for runs without a probe.
pub fn no_side_start() {}

/// A [`BenchClocks::side_stop`] that measures nothing.
pub fn no_side_stop() -> SideMeasure {
    SideMeasure::default()
}

/// Environment variable that sets the output refresh rate the bench is judged against.
pub const REFRESH_ENV: &str = "TL_BENCH_REFRESH_HZ";
/// Refresh rate used when [`REFRESH_ENV`] is unset.
pub const DEFAULT_REFRESH_HZ: u32 = 144;
/// Highest refresh rate [`FrameBudget::from_hz`] accepts.
pub const MAX_REFRESH_HZ: u32 = 1000;

/// Frame period of the default 144 Hz output.
pub const OUTPUT_PERIOD_NS: u64 = 6_944_444;
/// 1.5 x period (10.42 ms) at 144 Hz. An interval above it is a missed frame.
pub const MISSED_FRAME_NS: u64 = 10_416_667;
/// 3 x period at 144 Hz. An interval above it is a long missed frame.
pub const LONG_MISS_NS: u64 = 20_833_333;
/// Samples a probe holds at 144 Hz: 40 s.
pub const PROBE_CAPACITY: usize = 144 * 40;

/// Frame thresholds derived from one output refresh rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameBudget {
    pub refresh_hz: u32,
    /// One output period.
    pub period_ns: u64,
    /// 1.5 x period. An interval above it is a missed frame.
    pub missed_ns: u64,
    /// 3 x period. An interval above it is a long missed frame.
    pub long_miss_ns: u64,
    /// Samples a probe holds: 40 s at this rate.
    pub capacity: usize,
}

impl FrameBudget {
    /// Budget for `hz`, which must be 1 to 1000.
    pub fn from_hz(hz: u32) -> Result<Self, String> {
        if hz == 0 || hz > MAX_REFRESH_HZ {
            return Err(format!(
                "{REFRESH_ENV} must be an integer from 1 to {MAX_REFRESH_HZ}, got \"{hz}\""
            ));
        }
        let per = |n: u64| (n + u64::from(hz) / 2) / u64::from(hz);
        Ok(Self {
            refresh_hz: hz,
            period_ns: per(1_000_000_000),
            missed_ns: per(1_500_000_000),
            long_miss_ns: per(3_000_000_000),
            capacity: hz as usize * 40,
        })
    }

    /// Budget from the value of [`REFRESH_ENV`]. `None` (unset) gives 144 Hz.
    pub fn from_env_value(value: Option<&OsStr>) -> Result<Self, String> {
        let Some(value) = value else {
            return Self::from_hz(DEFAULT_REFRESH_HZ);
        };
        let bad = || {
            format!(
                "{REFRESH_ENV} must be an integer from 1 to {MAX_REFRESH_HZ}, got \"{}\"",
                value.to_string_lossy()
            )
        };
        let hz: u32 = value.to_str().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        Self::from_hz(hz).map_err(|_| bad())
    }
}

impl Default for FrameBudget {
    fn default() -> Self {
        Self::from_hz(DEFAULT_REFRESH_HZ).expect("144 is in range")
    }
}

const NS_PER_MS: f64 = 1_000_000.0;
const BYTES_PER_MB: f64 = 1_000_000.0;

/// Records one sample per frame while active. The sample `Vec`s are allocated once in [`Self::new`]
/// and stop growing when full.
pub struct FrameProbe {
    clocks: BenchClocks,
    intervals_ns: Vec<u64>,
    cpu_ns: Vec<u64>,
    /// Time and thread CPU of the previous callback. `None` until the first callback after
    /// `start` or `reset`.
    last: Option<(Instant, u64)>,
    active: bool,
    /// An `on_next_frame` callback of this probe is registered and has not run yet.
    armed: bool,
}

impl FrameProbe {
    pub fn new(clocks: BenchClocks, capacity: usize) -> Self {
        Self {
            clocks,
            intervals_ns: Vec::with_capacity(capacity),
            cpu_ns: Vec::with_capacity(capacity),
            last: None,
            active: false,
            armed: false,
        }
    }

    /// Registers window.on_next_frame; the callback records one frame and re-registers while active.
    pub fn start(this: Rc<RefCell<Self>>, window: &mut Window) {
        let arm = {
            let mut probe = this.borrow_mut();
            probe.active = true;
            probe.last = None;
            // A callback still pending from before a `stop` carries on; a second one would
            // record every frame twice.
            !mem::replace(&mut probe.armed, true)
        };
        if arm {
            Self::arm(&this, window);
        }
    }

    /// The next callback records nothing and does not re-register.
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Drops samples taken during warm-up. The next callback records nothing (fresh baseline).
    pub fn reset(&mut self) {
        self.intervals_ns.clear();
        self.cpu_ns.clear();
        self.last = None;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Callback intervals in ns.
    pub fn intervals_ns(&self) -> &[u64] {
        &self.intervals_ns
    }

    /// UI thread CPU between consecutive callbacks in ns, one per interval.
    pub fn cpu_ns(&self) -> &[u64] {
        &self.cpu_ns
    }

    fn arm(this: &Rc<RefCell<Self>>, window: &Window) {
        let probe = Rc::clone(this);
        window.on_next_frame(move |window, _| Self::on_frame(&probe, window));
    }

    fn on_frame(this: &Rc<RefCell<Self>>, window: &mut Window) {
        let mut probe = this.borrow_mut();
        probe.armed = false;
        if !probe.active {
            return;
        }
        let cpu = (probe.clocks.thread_cpu_ns)();
        probe.record(Instant::now(), cpu);
        probe.armed = true;
        drop(probe);
        Self::arm(this, window);
    }

    /// Two pushes at most, never past the capacity set in `new`.
    fn record(&mut self, now: Instant, cpu: u64) {
        if let Some((at, cpu_at)) = self.last
            && self.intervals_ns.len() < self.intervals_ns.capacity()
            && self.cpu_ns.len() < self.cpu_ns.capacity()
        {
            let interval = u64::try_from(now.duration_since(at).as_nanos()).unwrap_or(u64::MAX);
            self.intervals_ns.push(interval);
            self.cpu_ns.push(cpu.saturating_sub(cpu_at));
        }
        self.last = Some((now, cpu));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Percentiles {
    pub p50: f64,
    pub p99: f64,
    pub max: f64,
}

/// p50, p99 and max in ms, nearest-rank on a sorted copy. All 0.0 for an empty slice.
pub fn percentiles_ms(ns: &[u64]) -> Percentiles {
    if ns.is_empty() {
        return Percentiles {
            p50: 0.0,
            p99: 0.0,
            max: 0.0,
        };
    }
    let mut sorted = ns.to_vec();
    sorted.sort_unstable();
    let rank = |pct: usize| {
        // Nearest rank: ceil(pct / 100 x n), 1-based.
        let r = (pct * sorted.len()).div_ceil(100).max(1);
        sorted[r - 1] as f64 / NS_PER_MS
    };
    Percentiles {
        p50: rank(50),
        p99: rank(99),
        max: sorted[sorted.len() - 1] as f64 / NS_PER_MS,
    }
}

/// Counts of intervals above 1.5 x period and above 3 x period of `budget`.
pub fn count_missed(intervals_ns: &[u64], budget: &FrameBudget) -> (u32, u32) {
    let mut missed = 0u32;
    let mut long = 0u32;
    for &ns in intervals_ns {
        if ns > budget.missed_ns {
            missed += 1;
        }
        if ns > budget.long_miss_ns {
            long += 1;
        }
    }
    (missed, long)
}

/// `100 x cpu / wall` over the measured window. 0.0 when no wall time passed.
pub fn process_cpu_pct(cpu_delta_ns: u64, wall_delta_ns: u64) -> f64 {
    if wall_delta_ns == 0 {
        return 0.0;
    }
    100.0 * cpu_delta_ns as f64 / wall_delta_ns as f64
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LatencyPct {
    pub p50: f64,
    pub p99: f64,
}

/// GPU means over the measured window, sampled through the probe at 1 Hz (E01-023). Report only,
/// no gate (ADR-014).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct GpuReport {
    /// Mean NVML board power, summed over the GPUs, in W.
    pub power_w: f64,
    /// Mean NVML GPU utilization, the highest GPU per sample, in percent.
    pub util_pct: f64,
}

/// The bench JSON (UI-PRF-053 with R-160 OI-3). Field order is the JSON key order.
/// MB means 1,000,000 bytes (R-160 OI-4).
#[derive(Clone, Debug, Serialize)]
pub struct BenchReport {
    pub scene: &'static str,
    /// The output refresh rate the missed-frame counts were judged against.
    pub refresh_hz: u32,
    /// Pixel pacing of the window's driver: `off` or `timer` (wlf.31).
    pub pacing: &'static str,
    /// With pacing on, the driver's frames that notified a chart; the probe does not run then.
    pub frames: u64,
    /// Window paints in the measured window. `history-scrub` reports its probe frames here.
    pub drawn_frames: u64,
    /// One physical px per step of the fastest paced chart, per second. 0 with pacing off.
    pub pixel_rate_hz: f64,
    pub frame_cpu_ms: Percentiles,
    pub missed_frames: u32,
    /// Intervals above 3 x period (R-160 OI-3).
    pub long_missed_frames: u32,
    pub process_cpu_pct: f64,
    pub probe_cpu_pct: Option<f64>,
    pub rss_mb: f64,
    pub pss_mb: f64,
    /// Private_Clean + Private_Dirty from smaps_rollup; information only.
    pub uss_mb: f64,
    /// RssAnon + RssShmem from /proc/self/status, the app's own data; the RSS gate (wlf.47).
    pub app_mem_mb: f64,
    /// Stalls in the measured window.
    pub stalls: u32,
    /// Stalls during the warm-up, kept apart so they are not hidden.
    pub warmup_stalls: u32,
    /// The bench window was active at the start and the end of the measured window.
    pub window_active: bool,
    pub bridge_latency_ms: Option<LatencyPct>,
    /// The probe's GPU means (E01-023); `None` without a probe or after a probe error.
    pub gpu: Option<GpuReport>,
}

/// Memory figures in MB. `app` is the gated one (wlf.47); the rest are information.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MemoryMb {
    /// VmRSS.
    pub rss: f64,
    /// Pss from smaps_rollup.
    pub pss: f64,
    /// Private_Clean + Private_Dirty from smaps_rollup.
    pub uss: f64,
    /// RssAnon + RssShmem from /proc/self/status.
    pub app: f64,
}

/// Reads the memory figures. A missing file or line gives 0.0 for that part and a warning.
pub fn read_memory_mb() -> MemoryMb {
    let status = read_text("/proc/self/status");
    let rollup = read_text("/proc/self/smaps_rollup");
    memory_from(&status, &rollup)
}

fn read_text(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| {
        warn!(target: "tasklens::perf", path, %e, "cannot read memory figure");
        String::new()
    })
}

fn memory_from(status: &str, rollup: &str) -> MemoryMb {
    let get = |text: &str, key: &str| {
        parse_kb_mb(text, key).unwrap_or_else(|| {
            warn!(target: "tasklens::perf", key, "memory line missing");
            0.0
        })
    };
    MemoryMb {
        rss: get(status, "VmRSS:"),
        pss: get(rollup, "Pss:"),
        uss: get(rollup, "Private_Clean:") + get(rollup, "Private_Dirty:"),
        app: get(status, "RssAnon:") + get(status, "RssShmem:"),
    }
}

/// The value of the `key` line (for example `VmRSS:\t  123456 kB`) in MB, with kB = 1024 bytes.
fn parse_kb_mb(text: &str, key: &str) -> Option<f64> {
    let rest = text.lines().find_map(|line| line.strip_prefix(key))?;
    let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
    Some((kb * 1024) as f64 / BYTES_PER_MB)
}

/// Writes `report` as pretty JSON.
pub fn write_json(report: &BenchReport, path: &Path) -> std::io::Result<()> {
    let mut json = serde_json::to_vec_pretty(report)?;
    json.push(b'\n');
    fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use gpui_kit::{Context, IntoElement, Render, TestAppContext, div};

    use super::*;

    const MS: u64 = 1_000_000;

    static FAKE_CPU: AtomicU64 = AtomicU64::new(0);

    fn fake_cpu() -> u64 {
        FAKE_CPU.fetch_add(1_000, Ordering::Relaxed)
    }

    fn clocks() -> BenchClocks {
        BenchClocks {
            thread_cpu_ns: fake_cpu,
            process_cpu_ns: fake_cpu,
            side_start: no_side_start,
            side_stop: no_side_stop,
        }
    }

    fn report() -> BenchReport {
        BenchReport {
            scene: "perf-cpu",
            refresh_hz: 144,
            pacing: "off",
            frames: 5_760,
            drawn_frames: 5_700,
            pixel_rate_hz: 0.0,
            frame_cpu_ms: Percentiles {
                p50: 0.5,
                p99: 1.5,
                max: 2.0,
            },
            missed_frames: 2,
            long_missed_frames: 1,
            process_cpu_pct: 3.5,
            probe_cpu_pct: None,
            rss_mb: 120.0,
            pss_mb: 110.0,
            uss_mb: 100.0,
            app_mem_mb: 80.0,
            stalls: 0,
            warmup_stalls: 0,
            window_active: true,
            bridge_latency_ms: None,
            gpu: None,
        }
    }

    #[test]
    fn count_missed_thresholds() {
        let intervals = [6_900_000, 6_900_000, 11 * MS, 25 * MS];
        let b = FrameBudget::default();
        assert_eq!(count_missed(&intervals, &b), (2, 1));
        assert_eq!(count_missed(&[MISSED_FRAME_NS, LONG_MISS_NS], &b), (1, 0));
    }

    #[test]
    fn frame_budget_144() {
        let b = FrameBudget::from_hz(144).unwrap();
        assert_eq!(b, FrameBudget::default());
        assert_eq!(b.refresh_hz, 144);
        assert_eq!(b.period_ns, OUTPUT_PERIOD_NS);
        assert_eq!(b.missed_ns, MISSED_FRAME_NS);
        assert_eq!(b.long_miss_ns, LONG_MISS_NS);
        assert_eq!(b.capacity, PROBE_CAPACITY);
    }

    #[test]
    fn frame_budget_60() {
        let b = FrameBudget::from_hz(60).unwrap();
        assert_eq!(b.period_ns, 16_666_667);
        assert_eq!(b.missed_ns, 25_000_000);
        assert_eq!(b.long_miss_ns, 50_000_000);
        assert_eq!(b.capacity, 2400);
        assert_eq!(
            count_missed(&[25_000_000, 25_000_001, 50_000_001], &b),
            (2, 1)
        );
    }

    #[test]
    fn frame_budget_invalid() {
        for bad in ["0", "1001", "abc", "", "-5", "60.5"] {
            let err = FrameBudget::from_env_value(Some(OsStr::new(bad))).unwrap_err();
            assert!(err.contains("TL_BENCH_REFRESH_HZ"), "{err}");
        }
        assert_eq!(
            FrameBudget::from_env_value(None).unwrap(),
            FrameBudget::default()
        );
        let b = FrameBudget::from_env_value(Some(OsStr::new("60"))).unwrap();
        assert_eq!(b.refresh_hz, 60);
        assert_eq!(FrameBudget::from_hz(1000).unwrap().refresh_hz, 1000);
    }

    #[test]
    fn percentiles_nearest_rank() {
        let values: Vec<u64> = (1..=100).rev().map(|ms| ms * MS).collect();
        let p = percentiles_ms(&values);
        assert_eq!(
            p,
            Percentiles {
                p50: 50.0,
                p99: 99.0,
                max: 100.0
            }
        );
        let one = percentiles_ms(&[3 * MS]);
        assert_eq!((one.p50, one.p99, one.max), (3.0, 3.0, 3.0));
    }

    #[test]
    fn percentiles_empty() {
        let p = percentiles_ms(&[]);
        assert_eq!((p.p50, p.p99, p.max), (0.0, 0.0, 0.0));
    }

    #[test]
    fn parse_vmrss_and_pss() {
        let status = "Name:\ttasklens\nVmPeak:\t  999999 kB\nVmRSS:\t  123456 kB\nVmData:\t 1 kB\n";
        let rss = parse_kb_mb(status, "VmRSS:").unwrap();
        assert!((rss - 126.418944).abs() < 1e-9, "{rss}");

        let rollup = "55d0-7ffd ---p 00000000 00:00 0 [rollup]\nRss:  2048 kB\nPss:  1000 kB\nPss_Anon:  10 kB\n";
        let pss = parse_kb_mb(rollup, "Pss:").unwrap();
        assert!((pss - 1.024).abs() < 1e-9, "{pss}");

        assert_eq!(parse_kb_mb("Name:\tx\n", "VmRSS:"), None);
        assert_eq!(parse_kb_mb("VmRSS:\t garbage kB\n", "VmRSS:"), None);
        assert!((process_cpu_pct(5, 10) - 50.0).abs() < 1e-9);
        assert_eq!(process_cpu_pct(5, 0), 0.0);
    }

    #[test]
    fn memory_from_status_and_rollup() {
        let status = "VmRSS:\t232000 kB\nRssAnon:\t64000 kB\nRssShmem:\t13400 kB\n";
        let rollup = "Pss:\t152000 kB\nPrivate_Clean:\t64400 kB\nPrivate_Dirty:\t66100 kB\n";
        let m = memory_from(status, rollup);
        let mb = |kb: u64| (kb * 1024) as f64 / 1e6;
        assert!((m.app - mb(64000 + 13400)).abs() < 1e-9);
        assert!((m.uss - mb(64400 + 66100)).abs() < 1e-9);
        assert!((m.rss - mb(232000)).abs() < 1e-9);
        assert!((m.pss - mb(152000)).abs() < 1e-9);
    }

    #[test]
    fn memory_from_missing_lines() {
        let m = memory_from("", "");
        assert_eq!((m.rss, m.pss, m.uss, m.app), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn gpu_field_serializes() {
        let mut with = report();
        with.gpu = Some(GpuReport {
            power_w: 31.5,
            util_pct: 12.0,
        });
        with.probe_cpu_pct = Some(0.4);
        let text = serde_json::to_string_pretty(&with).unwrap();
        assert!(text.contains("\"power_w\": 31.5"), "{text}");
        assert!(text.contains("\"util_pct\": 12.0"), "{text}");
        assert!(text.contains("\"probe_cpu_pct\": 0.4"), "{text}");

        let text = serde_json::to_string_pretty(&report()).unwrap();
        assert!(text.contains("\"gpu\": null"), "{text}");
        assert!(text.contains("\"probe_cpu_pct\": null"), "{text}");
        assert_eq!(no_side_stop(), SideMeasure::default());
    }

    #[test]
    fn json_shape_and_order() {
        let value = serde_json::to_value(report()).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        // serde_json::Map sorts keys unless preserve_order is on, so check the text order.
        let text = serde_json::to_string_pretty(&report()).unwrap();
        let expected = [
            "scene",
            "refresh_hz",
            "pacing",
            "frames",
            "drawn_frames",
            "pixel_rate_hz",
            "frame_cpu_ms",
            "missed_frames",
            "long_missed_frames",
            "process_cpu_pct",
            "probe_cpu_pct",
            "rss_mb",
            "pss_mb",
            "uss_mb",
            "app_mem_mb",
            "stalls",
            "warmup_stalls",
            "window_active",
            "bridge_latency_ms",
            "gpu",
        ];
        assert_eq!(keys.len(), expected.len());
        let positions: Vec<usize> = expected
            .iter()
            .map(|k| {
                text.find(&format!("\n  \"{k}\":"))
                    .unwrap_or_else(|| panic!("{k} missing"))
            })
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{text}");
        assert!(text.contains("\"bridge_latency_ms\": null"), "{text}");
        assert!(text.contains("\"probe_cpu_pct\": null"), "{text}");
        assert!(text.contains("\"gpu\": null"), "{text}");
        assert!(text.contains("\"refresh_hz\": 144"), "{text}");
        assert!(text.contains("\"pacing\": \"off\""), "{text}");
        assert_eq!(value["frame_cpu_ms"]["p99"], 1.5);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");
        write_json(&report(), &path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{text}\n"));
    }

    struct Empty;

    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui_kit::test]
    fn frame_probe_stops(cx: &mut TestAppContext) {
        let (_, vcx) = cx.add_window_view(|_, _| Empty);
        let probe = Rc::new(RefCell::new(FrameProbe::new(clocks(), PROBE_CAPACITY)));
        vcx.update(|window, _| FrameProbe::start(Rc::clone(&probe), window));
        for _ in 0..10 {
            assert_eq!(vcx.update(|window, cx| window.simulate_next_frame(cx)), 1);
        }
        probe.borrow_mut().stop();
        assert_eq!(probe.borrow().intervals_ns().len(), 9);
        assert_eq!(probe.borrow().cpu_ns().len(), 9);
        assert!(probe.borrow().cpu_ns().iter().all(|&ns| ns > 0));

        // The callback registered before `stop` runs once, records nothing and does not re-register.
        assert_eq!(vcx.update(|window, cx| window.simulate_next_frame(cx)), 1);
        assert_eq!(vcx.update(|window, cx| window.simulate_next_frame(cx)), 0);
        assert_eq!(probe.borrow().intervals_ns().len(), 9);

        // Stop then start before the pending callback runs keeps a single callback chain.
        vcx.update(|window, _| FrameProbe::start(Rc::clone(&probe), window));
        probe.borrow_mut().stop();
        vcx.update(|window, _| FrameProbe::start(Rc::clone(&probe), window));
        assert_eq!(vcx.update(|window, cx| window.simulate_next_frame(cx)), 1);
        assert_eq!(vcx.update(|window, cx| window.simulate_next_frame(cx)), 1);
        probe.borrow_mut().stop();
        assert_eq!(vcx.update(|window, cx| window.simulate_next_frame(cx)), 1);
        assert_eq!(vcx.update(|window, cx| window.simulate_next_frame(cx)), 0);
    }

    #[gpui_kit::test]
    fn frame_probe_reset(cx: &mut TestAppContext) {
        let (_, vcx) = cx.add_window_view(|_, _| Empty);
        let probe = Rc::new(RefCell::new(FrameProbe::new(clocks(), PROBE_CAPACITY)));
        vcx.update(|window, _| FrameProbe::start(Rc::clone(&probe), window));
        for _ in 0..5 {
            vcx.update(|window, cx| window.simulate_next_frame(cx));
        }
        assert_eq!(probe.borrow().intervals_ns().len(), 4);

        probe.borrow_mut().reset();
        assert!(probe.borrow().intervals_ns().is_empty());
        assert!(probe.borrow().cpu_ns().is_empty());
        vcx.update(|window, cx| window.simulate_next_frame(cx));
        assert!(
            probe.borrow().intervals_ns().is_empty(),
            "first callback after reset is the baseline"
        );
        vcx.update(|window, cx| window.simulate_next_frame(cx));
        assert_eq!(probe.borrow().intervals_ns().len(), 1);
        assert!(probe.borrow().is_active());
    }

    #[test]
    fn frame_probe_stops_growing_at_capacity() {
        let mut probe = FrameProbe::new(clocks(), 2);
        let t0 = Instant::now();
        for i in 0..5u64 {
            probe.record(t0 + std::time::Duration::from_millis(i * 7), i * 100);
        }
        assert_eq!(probe.intervals_ns(), &[7 * MS, 7 * MS]);
        assert_eq!(probe.cpu_ns(), &[100, 100]);
        assert_eq!(probe.intervals_ns.capacity(), 2);
    }
}
