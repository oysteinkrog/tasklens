//! The `Cpu` worker's source (ADR-004, ADR-008): wraps the tl-collect `CpuSampler` and turns
//! each `CpuReading` into a `SourceUpdate::Cpu(CpuSample)`.
//!
//! It samples at 10 Hz while a visible CPU chart animates, at 1 Hz while CPU is otherwise
//! demanded or history recording is on, and not at all otherwise. Clock, temperature, live stats
//! and RAPL power are read at most once per second; fast ticks repeat the last values (PERF-028,
//! PERF-033, PERF-038, PERF-039, PERF-040).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use tl_collect::counters::{Clock, Measured};
use tl_collect::parse::cpufreq::{FreqReading, read_base_mhz};
use tl_collect::parse::rapl::RaplReading;
use tl_collect::sample::cpu::{CpuOpts, CpuReading, CpuSampler, LiveStats};
use tl_collect::{CollectError, SysRoot};
use tl_model::demand::{CpuDemand, Demand};
use tl_model::health::SourceId;
use tl_model::settings::EngineSettings;
use tl_model::system::{CpuTopology, CpuTotals, Metric};
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{CpuCoreSample, CpuSample, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;

/// `cpu_fast`: a visible CPU chart animates.
pub const FAST_PERIOD: Duration = Duration::from_millis(100);
pub const SLOW_PERIOD: Duration = Duration::from_secs(1);
/// Clock, temperature, live stats and power are read at most this often.
pub const SLOW_EVERY_NS: u64 = 1_000_000_000;

/// The CPU rate table (ADR-008). The process table keeps it at 1 Hz for the "Interrupts"
/// pseudo row (PROC-012, R-166). `tests/lab_demand.rs` holds an E08-008 copy of this function
/// that predates the pseudo-row rule.
pub fn cpu_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.cpu == CpuDemand::Off && !d.recording && !d.process_table {
        None
    } else if d.cpu_fast {
        Some(FAST_PERIOD)
    } else {
        Some(SLOW_PERIOD)
    }
}

/// The values read only on slow ticks, repeated on fast ticks.
#[derive(Default)]
struct SlowFields {
    freq: FreqReading,
    temp_c: Measured<f32>,
    live: LiveStats,
    power: RaplReading,
}

pub struct CpuSource {
    sampler: CpuSampler,
    /// Reused every sample.
    reading: CpuReading,
    /// Written by `period()`, read by `sample()`.
    per_cpu_wanted: Arc<AtomicBool>,
    /// Boot ns of the last sample that read clock, temperature, live stats and power.
    last_slow_ns: Option<TimeNs>,
    last: SlowFields,
    /// Read once in `new`.
    base_mhz: Measured<u32>,
    topology: Arc<CpuTopology>,
    /// True on the first sample and after a topology change.
    send_topology: bool,
}

impl CpuSource {
    /// Fails only when the sampler cannot read the CPU topology.
    pub fn new(root: SysRoot) -> Result<Self, CollectError> {
        let sampler = CpuSampler::new(root.clone())?;
        let base_mhz = read_base_mhz(&root);
        let topology = sampler.topology().topology.clone();
        Ok(Self {
            sampler,
            reading: CpuReading::default(),
            per_cpu_wanted: Arc::new(AtomicBool::new(false)),
            last_slow_ns: None,
            last: SlowFields::default(),
            base_mhz,
            topology,
            send_topology: true,
        })
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        let per_cpu = self.per_cpu_wanted.load(Ordering::Relaxed);
        let slow = self
            .last_slow_ns
            .is_none_or(|t| clock.boot_ns.saturating_sub(t) >= SLOW_EVERY_NS);
        let opts = CpuOpts {
            per_cpu,
            freq: slow,
            temp: slow,
            live_stats: slow,
            power: slow,
        };
        self.sampler.sample_at(clock, opts, &mut self.reading);
        let r = &self.reading;
        if slow {
            let freq = &mut self.last.freq;
            freq.source = r.freq.source;
            freq.avg_mhz = r.freq.avg_mhz;
            freq.max_mhz = r.freq.max_mhz;
            freq.per_cpu_mhz.clone_from(&r.freq.per_cpu_mhz);
            self.last.temp_c = r.temp_c;
            self.last.live = r.live;
            self.last.power = r.power;
            self.last_slow_ns = Some(clock.boot_ns);
        }
        if r.topology_changed {
            self.topology = self.sampler.topology().topology.clone();
            self.send_topology = true;
        }

        let last = &self.last;
        let totals = CpuTotals {
            util_pct: metric(r.total_pct),
            kernel_pct: metric(r.kernel_pct),
            iowait_pct: metric(r.iowait_pct),
            freq_mhz: metric(last.freq.avg_mhz),
            freq_fastest_mhz: metric(last.freq.max_mhz),
            base_mhz: metric(self.base_mhz),
            temp_c: metric(last.temp_c),
            package_power_w: metric(last.power.package_w),
            core_power_w: metric(last.power.core_w),
            processes: metric(last.live.process_count),
            // The reducer fills Threads from `ProcessSweep.system_threads` (PERF-033).
            threads: Metric::with(Validity::WarmingUp),
            handles: metric(last.live.open_files),
            load_avg: Metric {
                value: [
                    last.live.load.value.one,
                    last.live.load.value.five,
                    last.live.load.value.fifteen,
                ],
                validity: last.live.load.validity,
            },
            sockets: Metric::valid(self.sampler.topology().sockets),
        };
        let irq_pct = if r.interrupts_pct.validity.is_valid() {
            r.interrupts_pct.value
        } else {
            0.0
        };
        let per_cpu = r
            .per_cpu
            .iter()
            .enumerate()
            .map(|(id, core)| CpuCoreSample {
                cpu: u16::try_from(id).unwrap_or(u16::MAX),
                pct: core.total_pct.value,
                kernel_pct: core.kernel_pct.value,
                freq_mhz: valid_value(last.freq.per_cpu_mhz.get(id).copied()),
                validity: core.total_pct.validity,
            })
            .collect();
        let sample = CpuSample {
            t_ns: r.t_ns,
            totals,
            irq_pct,
            per_cpu,
            freq_mhz: valid_value(Some(last.freq.avg_mhz)),
            temp_c: valid_value(Some(last.temp_c)),
            temp_validity: last.temp_c.validity,
            topology: self.send_topology.then(|| self.topology.clone()),
            validity: r.total_pct.validity,
        };
        self.send_topology = false;
        (self, SourceUpdate::Cpu(sample))
    }
}

impl Source for CpuSource {
    fn id(&self) -> SourceId {
        SourceId::Cpu
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        self.per_cpu_wanted
            .store(d.cpu.wants_per_core() || d.recording, Ordering::Relaxed);
        cpu_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    fn reset_baselines(&mut self) {
        self.sampler.reset_baselines();
        self.last_slow_ns = None;
    }
}

/// In the full engine only.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Cpu,
        name: "cpu",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// A new `CpuSource` per generation, built on the blocking pool (it reads sysfs), then
/// `source::run`. Nothing is carried across a restart: each generation starts with fresh
/// baselines. A `CpuSource::new` failure is `WorkerError::Collect`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let root = cfg.sys_root.clone();
        async move {
            let mut job = match cx.spawn_blocking(move |_| CpuSource::new(root)) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let src = match job.join(&cx).await {
                Ok(Ok(src)) => src,
                Ok(Err(e)) => return Outcome::Err(WorkerError::Collect(e)),
                Err(JoinError::Cancelled(reason)) => {
                    if cx.checkpoint().is_err() {
                        return Outcome::Ok(());
                    }
                    return Outcome::Cancelled(reason);
                }
                Err(JoinError::Panicked(payload)) => return Outcome::Panicked(payload),
                Err(JoinError::PolledAfterCompletion) => {
                    return Outcome::Err(WorkerError::Other("job polled after completion".into()));
                }
            };
            source::run(cx, io, src).await
        }
    })
}

fn metric<T: Copy + Default>(m: Measured<T>) -> Metric<T> {
    Metric {
        value: m.value,
        validity: m.validity,
    }
}

fn valid_value<T>(m: Option<Measured<T>>) -> Option<T> {
    m.filter(|m| m.validity.is_valid()).map(|m| m.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_table() {
        let s = EngineSettings::default();
        let off = Demand::default();
        assert_eq!(cpu_period(&off, &s), None);

        let fast = Demand {
            cpu: CpuDemand::Total,
            cpu_fast: true,
            ..Demand::default()
        };
        assert_eq!(cpu_period(&fast, &s), Some(Duration::from_millis(100)));

        let slow = Demand {
            cpu: CpuDemand::Total,
            cpu_fast: false,
            ..Demand::default()
        };
        assert_eq!(cpu_period(&slow, &s), Some(Duration::from_secs(1)));

        let recording = Demand {
            cpu: CpuDemand::Off,
            recording: true,
            ..Demand::default()
        };
        assert_eq!(cpu_period(&recording, &s), Some(Duration::from_secs(1)));

        let table = Demand {
            cpu: CpuDemand::Off,
            recording: false,
            process_table: true,
            ..Demand::default()
        };
        assert_eq!(cpu_period(&table, &s), Some(Duration::from_secs(1)));

        let table_fast = Demand {
            cpu: CpuDemand::Total,
            cpu_fast: true,
            process_table: true,
            ..Demand::default()
        };
        assert_eq!(
            cpu_period(&table_fast, &s),
            Some(Duration::from_millis(100))
        );
    }
}
