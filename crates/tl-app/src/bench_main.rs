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

/// Commands to the sampler thread. Dropping the sender ends the thread and shuts the probe down.
enum SamplerCmd {
    /// Start sampling for the window with this id.
    Start(u64),
    /// Stop sampling and wait for the next window.
    Stop,
}

/// What the UI thread and the sampler thread share. The sampler holds the lock only to push one
/// sample or to set `failed`, never across a probe request, so [`side_stop`] never waits on the
/// probe.
#[derive(Default)]
struct WindowSamples {
    /// The window being sampled, 0 between windows. A reply for another window is dropped.
    id: u64,
    samples: Vec<Vec<GpuPoint>>,
    /// Set after a sample error; the thread has then given the probe up and ended.
    failed: bool,
}

/// The open measured window: the probe's CPU ticks and the time when it began.
struct OpenWindow {
    ticks_at: Option<u64>,
    wall_at: Instant,
}

/// The bench's own GPU probe. A `tl-bench-gpu` thread owns the client from start to shutdown and
/// sends the 1 Hz sample requests while a window is open. [`GpuMeter::begin`] and
/// [`GpuMeter::end`] only send a command and read shared state, so the UI thread never blocks on
/// a probe request.
struct GpuMeter {
    pid: u32,
    cmd: mpsc::Sender<SamplerCmd>,
    shared: Arc<Mutex<WindowSamples>>,
    thread: JoinHandle<()>,
    last_id: u64,
    open: Option<OpenWindow>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl GpuMeter {
    /// Moves a ready client to a new sampler thread. `None` (with a warning) when the thread
    /// cannot start; the client is then dropped with the closure.
    fn new(client: ProbeClient) -> Option<GpuMeter> {
        let pid = client.pid();
        let shared = Arc::new(Mutex::new(WindowSamples::default()));
        let (cmd, cmd_rx) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name(GPU_THREAD_NAME.to_owned())
            .spawn(move || sampler_thread(client, &cmd_rx, &thread_shared));
        match spawned {
            Ok(thread) => Some(GpuMeter {
                pid,
                cmd,
                shared,
                thread,
                last_id: 0,
                open: None,
            }),
            Err(e) => {
                warn!(target: "tasklens::perf", "cannot start the GPU sampler thread: {e}; gpu and probe_cpu_pct stay null");
                None
            }
        }
    }

    /// Opens a window: reads the probe's CPU ticks, then tells the thread to sample. `false` when
    /// the probe is gone and the meter must be dropped.
    fn begin(&mut self) -> bool {
        if self.open.is_some() {
            warn!(target: "tasklens::perf", "GPU window already open; this window is not sampled");
            return true;
        }
        self.last_id += 1;
        let id = self.last_id;
        {
            let mut w = lock(&self.shared);
            if w.failed {
                return false;
            }
            w.id = id;
            w.samples.clear();
        }
        let ticks_at = probe_ticks(self.pid);
        let wall_at = Instant::now();
        if self.cmd.send(SamplerCmd::Start(id)).is_err() {
            warn!(target: "tasklens::perf", "GPU sampler thread is gone; gpu and probe_cpu_pct stay null");
            return false;
        }
        self.open = Some(OpenWindow { ticks_at, wall_at });
        true
    }

    /// Closes the window: reads the probe's CPU ticks and the wall time first, so the window
    /// ends here, then takes the samples whose reply came in before this point. A request still
    /// in flight is not waited for and does not count. The second field is `false` after a probe
    /// error: the measure is then empty and the meter must be dropped.
    fn end(&mut self) -> (SideMeasure, bool) {
        let Some(open) = self.open.take() else {
            return (SideMeasure::default(), true);
        };
        let ticks_end = probe_ticks(self.pid);
        let wall = open.wall_at.elapsed();
        let (samples, failed) = {
            let mut w = lock(&self.shared);
            w.id = 0;
            (std::mem::take(&mut w.samples), w.failed)
        };
        let _ = self.cmd.send(SamplerCmd::Stop);
        if failed {
            return (SideMeasure::default(), false);
        }
        let cpu = match (open.ticks_at, ticks_end) {
            (Some(at), Some(end)) => probe_cpu_pct(
                end.saturating_sub(at),
                rustix::param::clock_ticks_per_second(),
                wall,
            ),
            _ => None,
        };
        let gpu = gpu_means(&samples);
        match (gpu, cpu) {
            (Some(gpu), Some(cpu)) => (
                SideMeasure {
                    gpu: Some(gpu),
                    probe_cpu_pct: Some(cpu),
                },
                true,
            ),
            _ => {
                warn!(target: "tasklens::perf", samples = samples.len(), cpu_read = cpu.is_some(),
                    "GPU means not available; gpu and probe_cpu_pct stay null");
                (SideMeasure::default(), true)
            }
        }
    }

    /// Ends the thread, which shuts the probe down, and waits for it. Called after the scene, so
    /// the wait is outside any measured window.
    fn shutdown(self) {
        drop(self.cmd);
        if self.thread.join().is_err() {
            warn!(target: "tasklens::perf", "GPU sampler thread panicked");
        }
    }
}

/// `None` without a probe or after a probe error. The scene reaches it only through the
/// [`side_start`] and [`side_stop`] function pointers.
static METER: Mutex<Option<GpuMeter>> = Mutex::new(None);

fn meter() -> MutexGuard<'static, Option<GpuMeter>> {
    lock(&METER)
}

/// Finds and starts the probe and runs its `Hello`. Any failure leaves the meter off.
fn meter_start() {
    let Some(path) = tl_engine::gpu::locate() else {
        info!(target: "tasklens::perf", "GPU probe not found; gpu and probe_cpu_pct stay null");
        return;
    };
    match ProbeClient::start(&path) {
        Ok(client) if client.info.is_some() => {
            info!(target: "tasklens::perf", path = %path.display(), pid = client.pid(), "bench GPU probe started");
            *meter() = GpuMeter::new(client);
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

/// Ends the sampler thread and shuts the probe down.
fn meter_shutdown() {
    let taken = meter().take();
    if let Some(m) = taken {
        m.shutdown();
    }
}

/// [`tl_ui::bench::BenchClocks::side_start`]: opens the GPU window. Never blocks on the probe.
fn side_start() {
    let mut guard = meter();
    if let Some(m) = guard.as_mut()
        && !m.begin()
    {
        *guard = None;
    }
}

/// [`tl_ui::bench::BenchClocks::side_stop`]: closes the GPU window and returns its means. Never
/// blocks on the probe. Any error gives both fields `None`.
fn side_stop() -> SideMeasure {
    let mut guard = meter();
    let Some(m) = guard.as_mut() else {
        return SideMeasure::default();
    };
    let (side, alive) = m.end();
    if !alive {
        // The thread warned, gave the probe up and ended.
        *guard = None;
    }
    side
}

/// Owns the client. Idle until a [`SamplerCmd::Start`]; then samples at once and every
/// [`GPU_SAMPLE_PERIOD`] until a [`SamplerCmd::Stop`]. A disconnected channel shuts the probe
/// down and ends the thread. The first sample error kills the probe, sets `failed` and ends it.
fn sampler_thread(
    mut client: ProbeClient,
    cmds: &mpsc::Receiver<SamplerCmd>,
    shared: &Mutex<WindowSamples>,
) {
    let mut next_window = None;
    'idle: loop {
        let id = match next_window.take() {
            Some(id) => id,
            None => match cmds.recv() {
                Ok(SamplerCmd::Start(id)) => id,
                Ok(SamplerCmd::Stop) => continue,
                Err(_) => break,
            },
        };
        let start = Instant::now();
        let mut n: u32 = 0;
        loop {
            match client.sample(false) {
                Ok(reply) => {
                    let point = reply
                        .devices
                        .iter()
                        .map(|d| (d.power_mw.ok(), d.util_pct.ok()))
                        .collect();
                    let mut w = lock(shared);
                    if w.id == id {
                        w.samples.push(point);
                    }
                }
                Err(e) => {
                    warn!(target: "tasklens::perf", "GPU probe sample failed: {e}; gpu and probe_cpu_pct stay null");
                    client.kill();
                    lock(shared).failed = true;
                    return;
                }
            }
            n += 1;
            let next = start + GPU_SAMPLE_PERIOD * n;
            match cmds.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(SamplerCmd::Stop) => continue 'idle,
                Ok(SamplerCmd::Start(id)) => {
                    next_window = Some(id);
                    continue 'idle;
                }
                Err(RecvTimeoutError::Disconnected) => break 'idle,
            }
        }
    }
    client.shutdown();
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

    /// The workspace debug probe when it was built with `--features fake`
    /// (`cargo build -p tl-gpu-probe --features fake`), else `None` with a note.
    fn fake_meter(script: &str) -> Option<GpuMeter> {
        let probe = std::env::var_os(tl_engine::gpu::PROBE_ENV)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/tl-gpu-probe")
            });
        let fake = std::process::Command::new(&probe)
            .args(["--fake", "no-such-script"])
            .stderr(std::process::Stdio::piped())
            .output()
            .is_ok_and(|out| String::from_utf8_lossy(&out.stderr).contains("unknown fake script"));
        if !fake {
            eprintln!(
                "skipped: no fake probe at {} (build it with `cargo build -p tl-gpu-probe --features fake`)",
                probe.display()
            );
            return None;
        }
        let client =
            ProbeClient::start_with_args(&probe, &[OsStr::new("--fake"), OsStr::new(script)])
                .expect("the fake probe starts");
        assert!(client.info.is_some(), "{:?}", client.init_failed);
        GpuMeter::new(client)
    }

    #[test]
    fn fake_probe_windows_have_means() {
        let Some(mut m) = fake_meter("normal") else {
            return;
        };
        // Two windows: the thread goes idle after the first and samples again for the second.
        for window in 0..2 {
            assert!(m.begin(), "window {window}");
            thread::sleep(Duration::from_millis(1_200));
            let (side, alive) = m.end();
            assert!(alive, "window {window}");
            let gpu = side.gpu.expect("gpu means");
            assert!(
                (gpu.power_w - 31.9).abs() < 1e-9,
                "window {window}: {gpu:?}"
            );
            assert!(side.probe_cpu_pct.is_some(), "window {window}: {side:?}");
        }
        m.shutdown();
    }

    #[test]
    fn end_does_not_wait_for_a_sample_in_flight() {
        // The first sample hangs for 6 s, so the request is in flight until its 500 ms deadline.
        let Some(mut m) = fake_meter("sleep-on-sample") else {
            return;
        };
        assert!(m.begin());
        thread::sleep(Duration::from_millis(50));
        // No wall-clock bound here (check_timing bans limits under 2 s). `alive` is the proof:
        // had `end` waited for the in-flight sample, the 500 ms deadline would have passed and
        // the thread would have given the probe up, so `alive` would be false.
        let (side, alive) = m.end();
        assert!(alive, "end waited past the sample deadline for the probe");
        assert_eq!(side, SideMeasure::default(), "no reply came in the window");
        // After the deadline the thread gives the probe up, and the next window refuses.
        thread::sleep(tl_engine::gpu::SAMPLE_DEADLINE + Duration::from_millis(500));
        assert!(!m.begin(), "a failed probe opens no window");
        m.shutdown();
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
