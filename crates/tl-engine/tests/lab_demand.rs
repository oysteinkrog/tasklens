//! LabRuntime tests for demand-driven sampling rates (ADR-008, ADR-012, ADR-013).
//!
//! `cpu_period` and `processes_period` are the `period()` tables E12-A1 uses. The loop reads
//! demand once per turn, before it sleeps, so the sample already scheduled at a change still
//! runs at the old rate. A change of rate resets baselines and samples at once.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::sync::{Arc, PoisonError};
use std::time::Duration;

use asupersync::Cx;
use asupersync::runtime::TaskHandle;
use fake_sources::{FakeLab, FakeSource, Log, Step, channels, demand_with, io, times_ms};
use tl_engine::source::{self, Source, WorkerIo};
use tl_engine::update::SourceUpdate;
use tl_model::demand::{CpuDemand, Demand};
use tl_model::health::SourceId;
use tl_model::settings::EngineSettings;

const TICK: Duration = Duration::from_millis(10);

fn cpu_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.cpu == CpuDemand::Off && !d.recording {
        None
    } else if d.cpu_fast {
        Some(Duration::from_millis(100))
    } else {
        Some(Duration::from_secs(1))
    }
}

fn processes_period(d: &Demand, s: &EngineSettings) -> Option<Duration> {
    if d.process_table || !d.detail_keys.is_empty() || d.gpu_process || d.recording {
        Some(Duration::from_millis(s.interval_ms as u64))
    } else {
        None
    }
}

type PeriodFn = fn(&Demand, &EngineSettings) -> Option<Duration>;

/// A `FakeSource` whose `period()` is one of the tables above.
struct TableSource {
    inner: FakeSource,
    period: PeriodFn,
}

impl Source for TableSource {
    fn id(&self) -> SourceId {
        self.inner.id
    }

    fn period(&self, demand: &Demand, settings: &EngineSettings) -> Option<Duration> {
        (self.period)(demand, settings)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        let (inner, update) = self.inner.sample();
        let src = TableSource {
            inner,
            period: self.period,
        };
        (src, update)
    }

    fn reset_baselines(&mut self) {
        self.inner.reset_baselines();
    }
}

/// Starts `source::run` with a `TableSource` and returns its log.
fn spawn_table(fl: &mut FakeLab, io: WorkerIo, period: PeriodFn) -> (TaskHandle<()>, Log) {
    // `period_on` is unused: `TableSource` answers `period()` itself.
    let inner = FakeSource::new(&fl.clock, vec![Step::Ok], Duration::from_secs(1));
    let log = Arc::clone(&inner.log);
    let src = TableSource { inner, period };
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        let _ = source::run(cx, io, src).await;
    });
    (handle, log)
}

fn after(log: &Log, what: &str, t_ms: u64) -> Vec<u64> {
    times_ms(log, what)
        .into_iter()
        .filter(|t| *t > t_ms)
        .collect()
}

#[test]
fn cpu_fast_to_1hz_to_off() {
    let mut fl = FakeLab::new();
    let (updates, _rx, demand, settings, control) = channels();
    demand
        .send(demand_with(|d| {
            d.cpu = CpuDemand::Total;
            d.cpu_fast = true;
        }))
        .expect("watch has receivers");
    let (handle, log) = spawn_table(
        &mut fl,
        io(&updates, &demand, &settings, &control),
        cpu_period,
    );

    fl.run_for(Duration::from_millis(1_950), TICK);
    let fast: Vec<u64> = (0..20).map(|k| k * 100).collect();
    assert_eq!(times_ms(&log, "sample"), fast);
    assert!(times_ms(&log, "reset").is_empty());

    // 1 Hz. The 2000 ms sample was already scheduled at 10 Hz; the next turn sees the new
    // period, resets and samples at once, then every second.
    let changed = fl.now_ms();
    demand
        .send(demand_with(|d| d.cpu = CpuDemand::Total))
        .expect("watch has receivers");
    fl.run_for(Duration::from_millis(3_050), TICK);
    let slow = after(&log, "sample", changed);
    assert_eq!(slow, [2_000, 2_000, 3_000, 4_000, 5_000]);
    assert_eq!(times_ms(&log, "reset"), [2_000]);
    let first_new = slow[1];
    assert!(
        first_new - changed <= 500,
        "new rate {} ms late",
        first_new - changed
    );
    // The reset came right before the first sample at the new rate, in log order.
    let entries: Vec<&str> = log
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .map(|e| e.1)
        .collect();
    let reset_at = entries
        .iter()
        .position(|e| *e == "reset")
        .expect("one reset");
    assert_eq!(entries[reset_at + 1], "sample");
    assert_eq!(entries[..reset_at].len(), fast.len() + 1);

    // Off. The 6000 ms sample was already scheduled; nothing after it for 60 s.
    let off_at = fl.now_ms();
    demand
        .send(demand_with(|d| {
            d.cpu = CpuDemand::Off;
            d.recording = false;
        }))
        .expect("watch has receivers");
    fl.run_for(Duration::from_secs(61), TICK);
    assert_eq!(after(&log, "sample", off_at), [6_000]);
    assert_eq!(times_ms(&log, "reset"), [2_000]);

    handle.abort();
    fl.finish();
}

#[test]
fn recording_keeps_1hz() {
    let mut fl = FakeLab::new();
    let (updates, _rx, demand, settings, control) = channels();
    demand
        .send(demand_with(|d| {
            d.cpu = CpuDemand::Off;
            d.recording = true;
        }))
        .expect("watch has receivers");
    let (handle, log) = spawn_table(
        &mut fl,
        io(&updates, &demand, &settings, &control),
        cpu_period,
    );

    fl.run_for(Duration::from_millis(9_990), TICK);
    let expected: Vec<u64> = (0..10).map(|k| k * 1_000).collect();
    assert_eq!(times_ms(&log, "sample"), expected);

    handle.abort();
    fl.finish();
}

#[test]
fn hidden_recording_off_zero_samples_60s() {
    let mut fl = FakeLab::new();
    let (updates, _rx, demand, settings, control) = channels();
    demand
        .send(demand_with(|d| {
            d.any_surface_visible = false;
            d.recording = false;
        }))
        .expect("watch has receivers");
    let (procs, procs_log) = spawn_table(
        &mut fl,
        io(&updates, &demand, &settings, &control),
        processes_period,
    );
    let (cpu, cpu_log) = spawn_table(
        &mut fl,
        io(&updates, &demand, &settings, &control),
        cpu_period,
    );

    fl.run_for(Duration::from_secs(60), TICK);
    assert_eq!(times_ms(&procs_log, "sample"), [0u64; 0]);
    assert_eq!(times_ms(&cpu_log, "sample"), [0u64; 0]);
    assert!(times_ms(&procs_log, "reset").is_empty());

    procs.abort();
    cpu.abort();
    fl.finish();
}

/// ADR-008 verification: hidden with device history on gives exactly the 1 Hz sources of the
/// "Not visible, device history on" column, with the engine's own rate tables (ADR-017).
#[test]
fn hidden_device_history_keeps_its_1hz_sources() {
    use tl_engine::workers::{
        connections::connections_period, cpu, disk::disk_period, gpu_drm::gpu_period,
        memory::memory_period, net::net_period, processes, sensors::sensors_period,
    };

    let mut fl = FakeLab::new();
    let (updates, _rx, demand, settings, control) = channels();
    demand
        .send(demand_with(|d| {
            d.any_surface_visible = false;
            d.device_history = true;
        }))
        .expect("watch has receivers");
    let on: [(&str, PeriodFn); 6] = [
        ("cpu", cpu::cpu_period),
        ("memory", memory_period),
        ("disk", disk_period),
        ("net", net_period),
        ("sensors", sensors_period),
        ("gpu", gpu_period),
    ];
    let off: [(&str, PeriodFn); 2] = [
        ("processes", processes::processes_period),
        ("connections", connections_period),
    ];
    let mut handles = Vec::new();
    let mut logs = Vec::new();
    for (name, period) in on.iter().chain(off.iter()) {
        let (handle, log) =
            spawn_table(&mut fl, io(&updates, &demand, &settings, &control), *period);
        handles.push(handle);
        logs.push((*name, log));
    }

    fl.run_for(Duration::from_millis(59_990), TICK);
    let every_second: Vec<u64> = (0..60).map(|k| k * 1_000).collect();
    for (name, log) in &logs[..on.len()] {
        assert_eq!(times_ms(log, "sample"), every_second, "{name}");
    }
    for (name, log) in &logs[on.len()..] {
        assert_eq!(times_ms(log, "sample"), [0u64; 0], "{name}");
    }

    for handle in handles {
        handle.abort();
    }
    fl.finish();
}

#[test]
fn processes_follow_interval_ms() {
    let mut fl = FakeLab::new();
    let (updates, _rx, demand, settings, control) = channels();
    demand
        .send(demand_with(|d| d.process_table = true))
        .expect("watch has receivers");
    settings
        .send_modify(|s| s.interval_ms = 250)
        .expect("watch has receivers");
    let (handle, log) = spawn_table(
        &mut fl,
        io(&updates, &demand, &settings, &control),
        processes_period,
    );

    fl.run_for(Duration::from_millis(990), TICK);
    assert_eq!(times_ms(&log, "sample"), [0, 250, 500, 750]);

    // The 1000 ms sample was already scheduled at 250 ms. The next turn sees 2000 ms, resets
    // and samples at once; the sample after that comes 2 s later.
    let changed = fl.now_ms();
    settings
        .send_modify(|s| s.interval_ms = 2_000)
        .expect("watch has receivers");
    fl.run_for(Duration::from_millis(4_100), TICK);
    assert_eq!(after(&log, "sample", changed), [1_000, 1_000, 3_000, 5_000]);
    assert_eq!(times_ms(&log, "reset"), [1_000]);

    handle.abort();
    fl.finish();
}
