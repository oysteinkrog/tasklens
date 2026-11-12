//! `tasklens --bench <scene> --duration 30 --warmup 5 --json <out>` (ADR-014, table (c),
//! APP-052): runs one bench scene.
//!
//! Compiled only with the `bench` feature. tl-app supplies the clocks tl-ui cannot read, starts
//! the real engine for the `bridge` scene only, runs the scene in its GPUI application and shuts
//! the engine down after it (ADR-010). `main.rs` sets up logging, the panic hook and the
//! arguments before calling [`run`].
//!
//! With the GPU probe installed (ADR-007 lookup order), the bench starts its own probe before the
//! scene and samples NVML power and utilization at 1 Hz over the measured window, plus the probe
//! child's CPU (E01-023, ADR-014). Report only, no gate. Without a probe, or after a probe error,
//! `gpu` and `probe_cpu_pct` stay null and the run goes on.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::time::{ClockId, clock_gettime};
use tl_engine::gpu::client::ProbeClient;
use tl_engine::{Engine, EngineConfig, EngineHandle, EngineHooks, EngineMode};
use tl_model::client::EngineClient;
use tl_model::demand::{CpuDemand, Demand};
use tl_ui::bench::stats::{FrameBudget, GpuReport, REFRESH_ENV, SideMeasure};
use tl_ui::bench::{BenchClocks, BenchOptions, BenchScene};
use tracing::{info, warn};

use crate::paths::Paths;

/// `--bench`, `--duration`, `--warmup` and `--json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchArgs {
    pub scene: String,
    pub duration_s: u32,
    pub warmup_s: u32,
    pub json: Option<PathBuf>,
}

/// Runs the scene and returns the process exit code: 0 on a finished run, 1 on a scene
/// failure, 2 on an unknown scene or an invalid `TL_BENCH_REFRESH_HZ`.
pub fn run(args: BenchArgs) -> i32 {
    let scene = match parse_scene(&args.scene) {
        Ok(scene) => scene,
        Err(text) => {
            eprintln!("{text}");
            return 2;
        }
    };

    let budget = match FrameBudget::from_env_value(std::env::var_os(REFRESH_ENV).as_deref()) {
        Ok(budget) => budget,
        Err(text) => {
            eprintln!("bench failed: {text}");
            return 2;
        }
    };
    info!(target: "tasklens::perf", refresh_hz = budget.refresh_hz, period_ns = budget.period_ns, "bench frame budget");

    let nested = nested_session(std::env::var_os(NESTED_ENV).as_deref());
    if nested {
        info!(target: "tasklens::perf", "TL_BENCH_NESTED=1: nested session, the session lock check is skipped");
    }

    if let Some(text) = lock_refusal(checked_lock(nested, session_locked)) {
        eprintln!("bench failed: {text}");
        return 1;
    }

    let engine = if scene == BenchScene::Bridge {
        match start_engine() {
            Ok(engine) => Some(engine),
            Err(text) => {
                eprintln!("bench failed: {text}");
                return 1;
            }
        }
    } else {
        None
    };
    let real = engine
        .as_ref()
        .map(|e| Arc::new(e.clone()) as Arc<dyn EngineClient>);

    // The probe's `Hello` runs here, outside the measured window.
    meter_start();

    let opts = BenchOptions {
        duration_s: args.duration_s,
        warmup_s: args.warmup_s,
        json: args.json,
        budget,
    };
    let clocks = BenchClocks {
        thread_cpu_ns,
        process_cpu_ns,
        side_start,
        side_stop,
    };
    let code = match tl_ui::bench::run_app(scene, opts, clocks, real) {
        // A lock during the run stops frame callbacks, so the report is not a measurement. A
        // lock that also ended before this check is not caught here; the 0-frame rule in tl-ui
        // catches a run that drew nothing.
        Ok(_) if checked_lock(nested, session_locked) == Some(true) => {
            eprintln!("bench failed: {}", locked_mid_run());
            1
        }
        Ok(_) => 0,
        Err(e) => {
            eprintln!("bench failed: {e}");
            1
        }
    };
    meter_shutdown();

    if let Some(engine) = engine {
        let report = engine.shutdown();
        info!(target: "tasklens::perf", drained = report.drained, torn_down = report.torn_down,
            elapsed_ms = report.elapsed.as_millis() as u64, "bench engine shutdown");
    }
    code
}

/// The scene named `name`, or the error line for an unknown name.
fn parse_scene(name: &str) -> Result<BenchScene, String> {
    BenchScene::from_arg(name).ok_or_else(|| {
        format!("unknown bench scene \"{name}\"; expected perf-cpu, table-2000, bridge or static")
    })
}

/// Set to 1 by the tl-smoke harness for a hidden nested KWin. It keeps drawing while the real
/// screen is locked, so the session lock check does not apply (owner decision 2026-11-04).
const NESTED_ENV: &str = "TL_BENCH_NESTED";

/// True only when the value is exactly `1`.
fn nested_session(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

/// The lock state, or `None` without asking logind in a nested session.
fn checked_lock(nested: bool, ask: impl FnOnce() -> Option<bool>) -> Option<bool> {
    if nested { None } else { ask() }
}

/// Why a run must not start: a locked session gets no frame callbacks from the compositor, so
/// every frame figure would be 0. `None` when unlocked or unknown.
fn lock_refusal(locked: Option<bool>) -> Option<&'static str> {
    (locked == Some(true)).then_some("session is locked; frames will not be delivered")
}

/// The error line for a session that was locked when the run ended.
fn locked_mid_run() -> &'static str {
    "the session locked during the run; the report is not valid"
}

/// logind's `LockedHint` over this user's graphical sessions (wayland or x11): true when any is
/// locked. `None` when logind cannot be asked; the run then goes ahead with a warning.
fn session_locked() -> Option<bool> {
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::OwnedObjectPath;

    const LOGIN1: &str = "org.freedesktop.login1";
    let ask = || -> zbus::Result<bool> {
        let conn = Connection::system()?;
        let manager = Proxy::new(
            &conn,
            LOGIN1,
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )?;
        let sessions: Vec<(String, u32, String, String, OwnedObjectPath)> =
            manager.call("ListSessions", &())?;
        let uid = rustix::process::getuid().as_raw();
        let mut locked = false;
        for (_, session_uid, _, _, path) in sessions {
            if session_uid != uid {
                continue;
            }
            let session = Proxy::new(&conn, LOGIN1, path, "org.freedesktop.login1.Session")?;
            let kind: String = session.get_property("Type")?;
            if (kind == "wayland" || kind == "x11") && session.get_property::<bool>("LockedHint")? {
                locked = true;
            }
        }
        Ok(locked)
    };
    match ask() {
        Ok(locked) => Some(locked),
        Err(e) => {
            warn!(target: "tasklens::perf", "cannot read the session lock state: {e}");
            None
        }
    }
}

/// The real engine for `bridge`, with the demand the Performance tab's CPU page sends (ADR-008):
/// a surface visible, per-core CPU and fast (10 Hz) CPU sampling. No history directory and no
/// helper: the run records nothing.
fn start_engine() -> Result<EngineHandle, String> {
    let paths =
        Paths::from_env().map_err(|e| format!("cannot resolve the state directory: {e}"))?;
    let config = EngineConfig::new(EngineMode::Full, paths.state_dir);
    let engine = Engine::start(config, EngineHooks::default())
        .map_err(|e| format!("cannot start the engine: {e}"))?;
    engine.set_demand(Demand {
        any_surface_visible: true,
        cpu: CpuDemand::PerCore,
        cpu_fast: true,
        ..Demand::default()
    });
    info!(target: "tasklens::perf", "bench engine started");
    Ok(engine)
}

/// Time between two GPU samples (ADR-014: 1 Hz).
const GPU_SAMPLE_PERIOD: Duration = Duration::from_secs(1);

/// Name of the thread that sends the 1 Hz sample requests.
const GPU_THREAD_NAME: &str = "tl-bench-gpu";

/// One GPU's reading in one sample: power in mW and utilization in percent, `None` when NVML
/// gave no value.
type GpuPoint = (Option<u32>, Option<u32>);

/// The bench's own GPU probe. Its client sits in `idle` between windows and moves to the sampler
/// thread during the measured window.
struct GpuMeter {
    pid: u32,
    idle: Option<ProbeClient>,
    running: Option<Sampler>,
}

/// The sampler of the window being measured.
struct Sampler {
    stop: mpsc::Sender<()>,
    thread: JoinHandle<SamplerEnd>,
    ticks_at: Option<u64>,
    wall_at: Instant,
}

/// What the sampler thread hands back: the client (`None` after a probe error) and one entry per
/// sample, one point per GPU.
struct SamplerEnd {
    client: Option<ProbeClient>,
    samples: Vec<Vec<GpuPoint>>,
}

/// `None` without a probe or after a probe error. The scene reaches it only through the
/// [`side_start`] and [`side_stop`] function pointers.
static METER: Mutex<Option<GpuMeter>> = Mutex::new(None);

fn meter() -> MutexGuard<'static, Option<GpuMeter>> {
    METER.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Finds and starts the probe and runs its `Hello`. Any failure leaves the meter off.
fn meter_start() {
    let Some(path) = tl_engine::gpu::locate() else {
        info!(target: "tasklens::perf", "GPU probe not found; gpu and probe_cpu_pct stay null");
        return;
    };
    match ProbeClient::start(&path) {
        Ok(client) if client.info.is_some() => {
            let pid = client.pid();
            info!(target: "tasklens::perf", path = %path.display(), pid, "bench GPU probe started");
            *meter() = Some(GpuMeter {
                pid,
                idle: Some(client),
                running: None,
            });
        }
        Ok(client) => {
            warn!(target: "tasklens::perf", init_failed = ?client.init_failed,
                "GPU probe did not initialise; gpu and probe_cpu_pct stay null");
            client.shutdown();
        }
        Err(e) => {
            warn!(target: "tasklens::perf", path = %path.display(),
                "cannot start the GPU probe: {e}; gpu and probe_cpu_pct stay null");
        }
    }
}

/// Stops a sampler that is still running (a scene that ended inside its window) and shuts the
/// probe down.
fn meter_shutdown() {
    let Some(mut m) = meter().take() else {
        return;
    };
    if let Some(run) = m.running.take() {
        let _ = run.stop.send(());
        if let Ok(end) = run.thread.join() {
            m.idle = end.client;
        }
    }
    if let Some(client) = m.idle.take() {
        client.shutdown();
    }
}

/// [`tl_ui::bench::BenchClocks::side_start`]: reads the probe's CPU ticks and moves the client
/// to the sampler thread.
fn side_start() {
    let mut guard = meter();
    let Some(m) = guard.as_mut() else {
        return;
    };
    let Some(client) = m.idle.take() else {
        warn!(target: "tasklens::perf", "GPU sampler already running; this window is not sampled");
        return;
    };
    let ticks_at = probe_ticks(m.pid);
    let wall_at = Instant::now();
    let (stop, stop_rx) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name(GPU_THREAD_NAME.to_owned())
        .spawn(move || sample_loop(client, &stop_rx));
    match spawned {
        Ok(thread) => {
            m.running = Some(Sampler {
                stop,
                thread,
                ticks_at,
                wall_at,
            });
        }
        Err(e) => {
            // The client moved into the closure that failed to start and is gone with it.
            warn!(target: "tasklens::perf", "cannot start the GPU sampler thread: {e}");
            *guard = None;
        }
    }
}

/// [`tl_ui::bench::BenchClocks::side_stop`]: stops the sampler and returns its means. Any error
/// gives both fields `None`.
fn side_stop() -> SideMeasure {
    let mut guard = meter();
    let Some(m) = guard.as_mut() else {
        return SideMeasure::default();
    };
    let Some(run) = m.running.take() else {
        return SideMeasure::default();
    };
    let _ = run.stop.send(());
    let Ok(end) = run.thread.join() else {
        warn!(target: "tasklens::perf", "GPU sampler thread panicked");
        *guard = None;
        return SideMeasure::default();
    };
    let wall = run.wall_at.elapsed();
    let ticks_end = probe_ticks(m.pid);
    let Some(client) = end.client else {
        // The sampler warned and gave the probe up.
        *guard = None;
        return SideMeasure::default();
    };
    m.idle = Some(client);
    let cpu = match (run.ticks_at, ticks_end) {
        (Some(at), Some(end)) => probe_cpu_pct(
            end.saturating_sub(at),
            rustix::param::clock_ticks_per_second(),
            wall,
        ),
        _ => None,
    };
    let gpu = gpu_means(&end.samples);
    match (gpu, cpu) {
        (Some(gpu), Some(cpu)) => SideMeasure {
            gpu: Some(gpu),
            probe_cpu_pct: Some(cpu),
        },
        _ => {
            warn!(target: "tasklens::perf", samples = end.samples.len(), cpu_read = cpu.is_some(),
                "GPU means not available; gpu and probe_cpu_pct stay null");
            SideMeasure::default()
        }
    }
}

/// Samples at once, then every [`GPU_SAMPLE_PERIOD`] until `stop` gets a message or closes. The
/// first sample error ends it with a warning and gives the probe up.
fn sample_loop(mut client: ProbeClient, stop: &mpsc::Receiver<()>) -> SamplerEnd {
    let start = Instant::now();
    let mut samples = Vec::new();
    let mut n: u32 = 0;
    loop {
        match client.sample(false) {
            Ok(reply) => samples.push(
                reply
                    .devices
                    .iter()
                    .map(|d| (d.power_mw.ok(), d.util_pct.ok()))
                    .collect(),
            ),
            Err(e) => {
                warn!(target: "tasklens::perf", "GPU probe sample failed: {e}; gpu and probe_cpu_pct stay null");
                client.kill();
                return SamplerEnd {
                    client: None,
                    samples,
                };
            }
        }
        n += 1;
        let next = start + GPU_SAMPLE_PERIOD * n;
        match stop.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                return SamplerEnd {
                    client: Some(client),
                    samples,
                };
            }
        }
    }
}

/// Power and utilization means over `samples`. Per sample, power is the sum over the GPUs and
/// utilization the highest GPU. A sample without any value for a field does not count for that
/// field. `None` when either field has no sample.
fn gpu_means(samples: &[Vec<GpuPoint>]) -> Option<GpuReport> {
    let mean = |per_sample: &mut dyn Iterator<Item = Option<f64>>| {
        let (sum, n) = per_sample
            .flatten()
            .fold((0.0, 0u32), |(s, n), v| (s + v, n + 1));
        (n > 0).then(|| sum / f64::from(n))
    };
    let power_w = mean(&mut samples.iter().map(|gpus| {
        let mut ok = gpus.iter().filter_map(|p| p.0).peekable();
        ok.peek()
            .is_some()
            .then(|| ok.map(f64::from).sum::<f64>() / 1000.0)
    }))?;
    let util_pct = mean(
        &mut samples
            .iter()
            .map(|gpus| gpus.iter().filter_map(|p| p.1).max().map(f64::from)),
    )?;
    Some(GpuReport { power_w, util_pct })
}

/// utime + stime of `pid` in clock ticks, from `/proc/<pid>/stat`.
fn probe_ticks(pid: u32) -> Option<u64> {
    stat_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// utime + stime (fields 14 and 15) of a `/proc/<pid>/stat` line. The name in field 2 can hold
/// spaces and `)`, so this splits after the last `)`.
fn stat_ticks(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(utime + stime)
}

/// `100 x ticks / ticks_per_s / wall_s`. `None` when no wall time passed or the tick rate is 0.
fn probe_cpu_pct(ticks: u64, ticks_per_s: u64, wall: Duration) -> Option<f64> {
    let wall_s = wall.as_secs_f64();
    (ticks_per_s > 0 && wall_s > 0.0).then(|| 100.0 * ticks as f64 / ticks_per_s as f64 / wall_s)
}

/// `CLOCK_THREAD_CPUTIME_ID` of the calling thread, in ns.
fn thread_cpu_ns() -> u64 {
    ns(clock_gettime(ClockId::ThreadCPUTime))
}

/// `CLOCK_PROCESS_CPUTIME_ID` (utime + stime of the process), in ns.
fn process_cpu_ns() -> u64 {
    ns(clock_gettime(ClockId::ProcessCPUTime))
}

fn ns(t: rustix::time::Timespec) -> u64 {
    match (u64::try_from(t.tv_sec), u64::try_from(t.tv_nsec)) {
        (Ok(s), Ok(n)) => s * 1_000_000_000 + n,
        _ => {
            warn!(target: "tasklens::perf", "negative clock value");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_ticks_parses() {
        let stat = "4242 (tl gpu) probe) S 1 4242 4242 0 -1 4194560 120 0 0 0 37 5 0 0 20 0 3 0";
        assert_eq!(stat_ticks(stat), Some(42));
        assert_eq!(stat_ticks("4242 (x) S 1 2"), None, "too few fields");
        assert_eq!(stat_ticks("no paren"), None);
        let own = std::fs::read_to_string("/proc/self/stat").unwrap();
        assert!(stat_ticks(&own).is_some(), "{own}");
    }

    #[test]
    fn gpu_means_sum_and_max() {
        let samples = vec![
            // Two GPUs: power sums, utilization takes the higher.
            vec![(Some(30_000), Some(10)), (Some(5_000), Some(40))],
            // A failed power reading on one GPU: the other still counts.
            vec![(None, Some(20)), (Some(15_000), None)],
            // No value at all: skipped for both fields.
            vec![(None, None)],
        ];
        let report = gpu_means(&samples).unwrap();
        assert!((report.power_w - 25.0).abs() < 1e-9, "{report:?}");
        assert!((report.util_pct - 30.0).abs() < 1e-9, "{report:?}");
        assert_eq!(gpu_means(&[]), None);
        assert_eq!(gpu_means(&[vec![(None, None)]]), None);
        assert_eq!(
            gpu_means(&[vec![(Some(1_000), None)]]),
            None,
            "no utilization"
        );
    }

    #[test]
    fn probe_cpu_pct_rate() {
        let pct = probe_cpu_pct(30, 100, Duration::from_secs(30)).unwrap();
        assert!((pct - 1.0).abs() < 1e-9, "{pct}");
        assert_eq!(probe_cpu_pct(30, 100, Duration::ZERO), None);
        assert_eq!(probe_cpu_pct(30, 0, Duration::from_secs(1)), None);
    }

    #[test]
    fn no_probe_gives_nulls() {
        assert!(meter().is_none(), "no test starts a probe");
        side_start();
        assert_eq!(side_stop(), SideMeasure::default());
        meter_shutdown();
    }

    #[test]
    fn unknown_scene_exit_2() {
        assert_eq!(
            parse_scene("charts"),
            Err(
                "unknown bench scene \"charts\"; expected perf-cpu, table-2000, bridge or static"
                    .to_owned()
            )
        );
        let code = run(BenchArgs {
            scene: "charts".to_owned(),
            duration_s: 30,
            warmup_s: 5,
            json: None,
        });
        assert_eq!(code, 2, "no window is opened for an unknown scene");
        assert_eq!(parse_scene("perf-cpu"), Ok(BenchScene::PerfCpu));
        assert_eq!(parse_scene("table-2000"), Ok(BenchScene::Table2000));
    }

    #[test]
    fn locked_session_refused() {
        assert_eq!(
            lock_refusal(Some(true)),
            Some("session is locked; frames will not be delivered")
        );
        assert_eq!(lock_refusal(Some(false)), None);
        assert_eq!(
            lock_refusal(None),
            None,
            "unknown state does not block a run"
        );
        assert_eq!(
            locked_mid_run(),
            "the session locked during the run; the report is not valid"
        );
    }

    #[test]
    fn nested_env_value() {
        assert!(nested_session(Some(OsStr::new("1"))));
        assert!(!nested_session(None));
        for v in ["0", "", "true"] {
            assert!(!nested_session(Some(OsStr::new(v))), "{v:?}");
        }
    }

    #[test]
    fn nested_skips_lock_check() {
        let state = checked_lock(true, || panic!("logind asked"));
        assert_eq!(state, None);
        assert_eq!(lock_refusal(state), None);
    }

    #[test]
    fn unnested_keeps_lock_check() {
        let state = checked_lock(false, || Some(true));
        assert_eq!(state, Some(true));
        assert_eq!(
            lock_refusal(state),
            Some("session is locked; frames will not be delivered")
        );
        assert_eq!(checked_lock(false, || None), None);
    }

    #[test]
    fn thread_cpu_clock_moves() {
        let before = thread_cpu_ns();
        let start = Instant::now();
        let mut x = 0u64;
        while start.elapsed() < Duration::from_millis(10) {
            x = std::hint::black_box(x.wrapping_add(1));
        }
        let delta = thread_cpu_ns() - before;
        assert!(
            (5_000_000..=50_000_000).contains(&delta),
            "delta {delta} ns"
        );
        assert!(process_cpu_ns() >= delta);
    }
}
