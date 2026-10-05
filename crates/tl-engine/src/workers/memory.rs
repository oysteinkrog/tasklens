//! The `Memory` worker (ADR-004, ADR-008): wraps the tl-collect `MemorySampler` and turns each
//! `MemoryReading` into a `SourceUpdate::Memory(MemorySample)`.
//!
//! It samples at 1 Hz while memory is demanded or history recording is on, and not at all
//! otherwise. zram devices are enumerated again every 5 s (PERF-041 to PERF-043, PROC-012).

use std::sync::Arc;
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use tl_collect::SysRoot;
use tl_collect::counters::{Clock, Rate};
use tl_collect::sample::memory::{MemOpts, MemoryReading, MemorySampler, ZramReading};
use tl_model::demand::Demand;
use tl_model::health::SourceId;
use tl_model::settings::EngineSettings;
use tl_model::system::{MemoryTotals, Metric, ZramTotals};
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{MemorySample, SourceUpdate, WorkerError, ZramStat};
use crate::workers::WorkerSpec;
use crate::workers::enumerate::EnumClock;

pub const PERIOD: Duration = Duration::from_secs(1);

/// The memory rate table (ADR-008 row "Memory, swap, zram").
pub fn memory_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.memory || d.recording {
        Some(PERIOD)
    } else {
        None
    }
}

pub struct MemorySource {
    sampler: MemorySampler,
    /// Reused every sample.
    reading: MemoryReading,
    /// When the zram devices are enumerated again.
    zram_clock: EnumClock,
}

impl MemorySource {
    pub fn new(root: SysRoot) -> Self {
        Self {
            sampler: MemorySampler::new(root),
            reading: MemoryReading::default(),
            zram_clock: EnumClock::default(),
        }
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        if self.zram_clock.due(clock.boot_ns) {
            self.sampler.rescan_zram();
        }
        self.sampler
            .sample_at(clock, MemOpts { details: false }, &mut self.reading);
        let r = &self.reading;
        let info = &r.info;

        let zram = if r.zram.is_empty() {
            Metric::with(Validity::Unsupported)
        } else {
            let (orig_data, compr_data) = r
                .zram
                .iter()
                .filter(|z| z.mm.validity == Validity::Valid)
                .fold((0u64, 0u64), |(o, c), z| {
                    (
                        o.saturating_add(z.mm.value.orig_data_size),
                        c.saturating_add(z.mm.value.compr_data_size),
                    )
                });
            Metric::valid(ZramTotals {
                orig_data,
                compr_data,
                mem_used: r.zram_mem_used_total,
            })
        };
        let totals = MemoryTotals {
            validity: r.validity,
            total: info.mem_total,
            available: info.mem_available,
            used: r.used_bytes,
            free: info.mem_free,
            cached: info.cached,
            dirty: info.dirty,
            writeback: info.writeback,
            committed: info.committed_as,
            commit_limit: info.commit_limit,
            slab_reclaimable: info.sreclaimable,
            slab_unreclaimable: info.sunreclaim,
            swap_total: info.swap_total,
            swap_free: info.swap_free,
            huge_pages_total: info.hugepages_total.unwrap_or(0),
            huge_pages_free: info.hugepages_free.unwrap_or(0),
            huge_page_size: info.hugepagesize.unwrap_or(0),
            zswap: info.zswap.unwrap_or(0),
            zswapped: info.zswapped.unwrap_or(0),
            zram,
        };
        let sample = MemorySample {
            t_ns: r.t_ns,
            totals,
            zram: r.zram.iter().map(zram_stat).collect(),
            validity: r.validity,
        };
        (self, SourceUpdate::Memory(sample))
    }
}

impl Source for MemorySource {
    fn id(&self) -> SourceId {
        SourceId::Memory
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        memory_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    fn reset_baselines(&mut self) {
        self.sampler.reset_baselines();
        self.zram_clock.force();
    }
}

/// In the full engine only.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Memory,
        name: "memory",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// A new `MemorySource` per generation, built on the blocking pool (it reads sysfs), then
/// `source::run`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let root = cfg.sys_root.clone();
        async move {
            let mut job = match cx.spawn_blocking(move |_| MemorySource::new(root)) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let src = match job.join(&cx).await {
                Ok(src) => src,
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

/// The rates are 0 unless `Valid`.
fn zram_stat(z: &ZramReading) -> ZramStat {
    let bps = |rate: &Rate| {
        if rate.validity == Validity::Valid {
            rate.per_sec as f32
        } else {
            0.0
        }
    };
    ZramStat {
        name: Arc::clone(&z.name),
        disksize: z.disksize_bytes,
        orig_bytes: z.mm.value.orig_data_size,
        compr_bytes: z.mm.value.compr_data_size,
        mem_used_total: z.mm.value.mem_used_total,
        read_bps: bps(&z.read_bps),
        write_bps: bps(&z.write_bps),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_table() {
        let s = EngineSettings::default();
        assert_eq!(memory_period(&Demand::default(), &s), None);

        let memory = Demand {
            memory: true,
            ..Demand::default()
        };
        assert_eq!(memory_period(&memory, &s), Some(Duration::from_secs(1)));

        let recording = Demand {
            recording: true,
            ..Demand::default()
        };
        assert_eq!(memory_period(&recording, &s), Some(Duration::from_secs(1)));
    }
}
