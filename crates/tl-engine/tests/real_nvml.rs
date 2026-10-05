//! Real-system tests of the GPU probe path on the owner machine (ADR-013 rows "NVML `Hello` and
//! samples" and "GPU probe kill and restart", ADR-007, U-12, U-35, E13-009 to E13-011).
//!
//! Every test is `#[ignore]` and needs an RTX 5070 on driver 615 and a release probe:
//!
//! ```text
//! cargo build --release -p tl-gpu-probe
//! TASKLENS_GPU_PROBE=$PWD/target/release/tl-gpu-probe \
//!     cargo test -p tl-engine --test real_nvml -- --ignored
//! ```
//!
//! The tests take one lock, so at most one probe of this binary runs at a time and the kill
//! test can find its child by name. Timings are printed for the ADR-014 record.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use asupersync::runtime::RuntimeBuilder;
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::gpu::client::{ProbeClient, api_versions};
use tl_engine::gpu::{API_VERSIONS_DEADLINE, PROBE_ENV, PROBE_NAME, ProbeError, SAMPLE_DEADLINE};
use tl_engine::session::engine_root;
use tl_engine::workers::gpu_nvidia;
use tl_model::demand::Demand;
use tl_model::devices::DeviceKind;
use tl_model::gpu_proto::Reading;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{MetricId, SeriesId};
use tl_model::snapshot::Snapshot;

/// Held by every test, so only one probe of this binary runs at a time.
static ONE_PROBE: Mutex<()> = Mutex::new(());

fn one_probe() -> MutexGuard<'static, ()> {
    ONE_PROBE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The probe from `gpu::locate()`. The test binary has no sibling probe, so this needs
/// `TASKLENS_GPU_PROBE`.
fn probe() -> PathBuf {
    tl_engine::gpu::locate().unwrap_or_else(|| {
        panic!("set {PROBE_ENV} to a release build, for example target/release/{PROBE_NAME}")
    })
}

/// Pids of every running `tl-gpu-probe`, from `pgrep -x`.
fn probe_pids() -> BTreeSet<u32> {
    let out = Command::new("pgrep")
        .args(["-x", PROBE_NAME])
        .output()
        .expect("pgrep runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// The `tl-gpu-probe` children of this process, from `/proc/<pid>/stat`.
fn own_probe_children() -> Vec<u32> {
    let me = std::process::id();
    probe_pids()
        .into_iter()
        .filter(|pid| parent_of(*pid) == Some(me))
        .collect()
}

/// Field 4 of `/proc/<pid>/stat`. The name in field 2 can hold spaces, so this splits after
/// the last `)`.
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// A child this test started. Killed and reaped on drop.
struct OwnChild(Child);

impl Drop for OwnChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "owner machine: RTX 5070 and a release tl-gpu-probe"]
fn real_hello_5070() {
    let _lock = one_probe();
    let begin = Instant::now();
    let client = ProbeClient::start(&probe()).expect("probe starts");
    let took = begin.elapsed();
    let info = client.info.clone().unwrap_or_else(|| {
        panic!("Hello is not Ready: {:?}", client.init_failed);
    });
    eprintln!(
        "real_hello_5070: start and Hello in {took:?}, driver {}, NVML {}",
        info.driver, info.nvml
    );
    assert_eq!(info.devices.len(), 1, "{:?}", info.devices);
    let dev = &info.devices[0];
    assert!(dev.name.contains("5070"), "name {:?}", dev.name);
    assert!(info.driver.starts_with("615."), "driver {:?}", info.driver);
    assert_eq!(dev.pci_bus_id, "00000000:2D:00.0");
    eprintln!("U-35: power_limit_mw = {:?}", dev.power_limit_mw);
    assert!(dev.power_limit_mw.is_some());
    client.shutdown();
}

#[test]
#[ignore = "owner machine: RTX 5070 and a release tl-gpu-probe"]
fn real_ten_samples_valid() {
    let _lock = one_probe();
    let mut client = ProbeClient::start(&probe()).expect("probe starts");
    let mut times = Vec::with_capacity(10);
    for i in 0..10 {
        if i > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        let begin = Instant::now();
        let sample = client.sample(false).expect("sample");
        let took = begin.elapsed();
        times.push(took);
        assert_eq!(sample.devices.len(), 1, "sample {i}");
        let d = &sample.devices[0];
        assert!(matches!(d.util_pct, Reading::Ok(_)), "sample {i}: {d:?}");
        assert!(matches!(d.mem_used, Reading::Ok(_)), "sample {i}: {d:?}");
        assert!(matches!(d.temp_c, Reading::Ok(_)), "sample {i}: {d:?}");
        assert!(matches!(d.power_mw, Reading::Ok(_)), "sample {i}: {d:?}");
        assert!(
            matches!(d.clock_gfx_mhz, Reading::Ok(_)),
            "sample {i}: {d:?}"
        );
        assert!(sample.processes.is_none(), "sample {i} has process rows");
        assert!(
            took < Duration::from_millis(100),
            "sample {i} took {took:?}"
        );
    }
    eprintln!("real_ten_samples_valid: sample(false) times {times:?}");
    client.shutdown();
}

#[test]
#[ignore = "owner machine: RTX 5070 and a release tl-gpu-probe"]
fn real_process_rows_u12() {
    let _lock = one_probe();
    let mut gears = match Command::new("glxgears")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => OwnChild(child),
        Err(e) => {
            eprintln!("real_process_rows_u12: skipped, U-12 not verified: glxgears absent: {e}");
            return;
        }
    };
    let pid = gears.0.id();
    std::thread::sleep(Duration::from_secs(2));
    // Without DISPLAY, glxgears exits at once with "couldn't open display".
    if let Ok(Some(status)) = gears.0.try_wait() {
        eprintln!(
            "real_process_rows_u12: skipped, U-12 not verified: glxgears exited ({status}); \
             run from a desktop session"
        );
        return;
    }

    let mut client = ProbeClient::start(&probe()).expect("probe starts");
    let first = client.sample(true).expect("first sample");
    assert!(first.processes.is_some(), "process rows were asked for");
    std::thread::sleep(Duration::from_secs(1));
    let begin = Instant::now();
    let second = client.sample(true).expect("second sample");
    let took = begin.elapsed();
    let rows = second.processes.expect("process rows were asked for");
    let row = rows.iter().find(|r| r.pid == pid).unwrap_or_else(|| {
        panic!("no row for glxgears pid {pid} in {rows:?}");
    });
    assert!(matches!(row.sm_pct, Reading::Ok(_)), "{row:?}");
    eprintln!(
        "real_process_rows_u12: U-12 verified, glxgears sm_pct {:?}, {} rows, sample(true) {took:?}",
        row.sm_pct,
        rows.len()
    );
    client.shutdown();
    drop(gears);
}

#[test]
#[ignore = "owner machine: RTX 5070 and a release tl-gpu-probe"]
fn real_kill_and_restart() {
    let _lock = one_probe();
    let probe = probe();
    let before = own_probe_children();
    let mut client = ProbeClient::start(&probe).expect("probe starts");
    let started: Vec<u32> = own_probe_children()
        .into_iter()
        .filter(|pid| !before.contains(pid))
        .collect();
    let [pid] = started[..] else {
        panic!("expected one new probe child, found {started:?}");
    };

    // SIGSTOP only the probe this test started.
    let status = Command::new("kill")
        .args(["-STOP", &pid.to_string()])
        .status()
        .expect("kill runs");
    assert!(status.success(), "kill -STOP {pid}: {status}");

    let begin = Instant::now();
    let result = client.sample(false);
    let took = begin.elapsed();
    eprintln!("real_kill_and_restart: stopped probe answered {result:?} after {took:?}");
    assert!(
        matches!(result, Err(ProbeError::Deadline(d)) if d == SAMPLE_DEADLINE),
        "{result:?}"
    );
    assert!(took < Duration::from_millis(700), "took {took:?}");
    // Reaped, not a zombie: the /proc entry is gone.
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "probe {pid} is still in /proc"
    );
    drop(client);

    let begin = Instant::now();
    let restarted = ProbeClient::start(&probe).expect("restart");
    let took = begin.elapsed();
    eprintln!("real_kill_and_restart: restart and Hello in {took:?}");
    assert!(restarted.info.is_some(), "{:?}", restarted.init_failed);
    assert!(took < Duration::from_secs(3), "restart took {took:?}");
    restarted.shutdown();
}

#[test]
#[ignore = "owner machine: RTX 5070 and a release tl-gpu-probe"]
fn real_api_versions() {
    let _lock = one_probe();
    let begin = Instant::now();
    let versions = api_versions(&probe(), API_VERSIONS_DEADLINE).expect("api versions");
    eprintln!("real_api_versions: {versions:?} in {:?}", begin.elapsed());
    assert_eq!(versions.devices.len(), 1, "{versions:?}");
    let dev = &versions.devices[0];
    assert!(
        dev.vulkan_api
            .as_deref()
            .is_some_and(|v| v.starts_with("1.")),
        "{dev:?}"
    );
    assert!(
        dev.opengl.as_deref().is_some_and(|v| v.starts_with("4.")),
        "{dev:?}"
    );
}

/// Samples in the `GPU_UTIL` series of the first present GPU, and the time of the last one.
fn gpu_util(snap: &Snapshot) -> Option<(usize, u64)> {
    let device = snap.devices.present_of(DeviceKind::Gpu).next()?.idx;
    let series = snap.series.get(SeriesId {
        device,
        metric: MetricId::GPU_UTIL,
    })?;
    let last = series.samples.last().map_or(0, |s| s.t_ns);
    Some((series.samples.len(), last))
}

/// The real runtime with only the `GpuNvidia` worker, driven like `lab_bridge.rs`
/// `start_with_fake_workers_publishes`: `Engine::start` always takes `workers_for(mode)`.
#[test]
#[ignore = "owner machine: RTX 5070 and a release tl-gpu-probe"]
fn real_worker_end_to_end() {
    let _lock = one_probe();
    // The worker calls `gpu::locate()` itself; fail early with the hint when it would not
    // find the probe.
    let _ = probe();
    let older = probe_pids();

    let rt = RuntimeBuilder::new()
        .worker_threads(tl_engine::runtime::WORKER_THREADS)
        .blocking_threads(
            tl_engine::runtime::BLOCKING_THREADS_MIN,
            tl_engine::runtime::BLOCKING_THREADS_MAX,
        )
        .thread_name_prefix(tl_engine::runtime::THREAD_NAME_PREFIX)
        .global_queue_limit(tl_engine::runtime::GLOBAL_QUEUE_LIMIT)
        .build()
        .expect("runtime builds");
    let state_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("real_nvml-state");
    let config = Arc::new(EngineConfig::new(EngineMode::Full, state_dir));
    let (ep, ui) = fake_sources::root_endpoints(config, vec![gpu_nvidia::spec()]);
    rt.handle()
        .try_spawn_with_cx(move |cx| engine_root(cx, ep))
        .expect("root spawns");

    ui.demand_tx
        .send(Demand {
            any_surface_visible: true,
            gpus: true,
            ..Demand::default()
        })
        .expect("demand sends");
    std::thread::sleep(Duration::from_secs(3));

    let snap = ui.latest.load_full();
    let gpus: Vec<_> = snap.devices.present_of(DeviceKind::Gpu).collect();
    assert_eq!(gpus.len(), 1, "{:?}", snap.devices.entries);
    let samples = gpu_util(&snap);
    eprintln!("real_worker_end_to_end: after 3 s, GPU_UTIL (samples, last t_ns) {samples:?}");
    assert!(samples.is_some_and(|(n, _)| n >= 2), "{samples:?}");
    let health = snap.health.get(SourceId::GpuNvidia);
    assert_eq!(health.state, SourceState::Running, "{health:?}");

    ui.demand_tx.send(Demand::default()).expect("demand sends");
    // One sample can be in flight when the demand changes.
    std::thread::sleep(Duration::from_millis(1500));
    let idle_from = gpu_util(&ui.latest.load_full());
    std::thread::sleep(Duration::from_secs(5));
    let idle_to = gpu_util(&ui.latest.load_full());
    eprintln!("real_worker_end_to_end: idle GPU_UTIL {idle_from:?} then {idle_to:?}");
    assert_eq!(
        idle_from.map(|(_, t)| t),
        idle_to.map(|(_, t)| t),
        "a GPU_UTIL sample was added without demand"
    );

    drop(ui);
    let dropped = Instant::now();
    drop(rt);
    let mut left: Vec<u32> = Vec::new();
    while dropped.elapsed() < Duration::from_secs(2) {
        left = probe_pids().difference(&older).copied().collect();
        if left.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!(
        "real_worker_end_to_end: runtime dropped, probes left {left:?} after {:?}",
        dropped.elapsed()
    );
    assert!(left.is_empty(), "tl-gpu-probe still runs: {left:?}");
}
