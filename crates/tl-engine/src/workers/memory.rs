//! The `Memory` worker (ADR-004, ADR-008): wraps the tl-collect `MemorySampler` and turns each
//! `MemoryReading` into a `SourceUpdate::Memory(MemorySample)`.
//!
//! It samples at 1 Hz while memory is demanded or history recording is on, and not at all
//! otherwise. zram devices are enumerated again every 5 s (PERF-041 to PERF-043, PROC-012).
//! While memory is demanded, the swap list and zswap parameters are read every 5 s and sent as
//! `DeviceDetails::Memory` when they change (PERF-044, PERF-045).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use tl_collect::SysRoot;
use tl_collect::counters::{Clock, Rate};
use tl_collect::parse::swaps::{SwapEntry, SwapKind};
use tl_collect::parse::zswap::ZswapParams;
use tl_collect::sample::memory::{MemOpts, MemoryReading, MemorySampler, ZramReading};
use tl_model::demand::Demand;
use tl_model::health::SourceId;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    InvField, MemoryDetails, SwapDevice, SwapKindModel, ZramDetail, ZswapInfo,
};
use tl_model::system::{MemoryTotals, Metric, ZramTotals};
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{DeviceDetailsUpdate, MemorySample, SourceUpdate, WorkerError, ZramStat};
use crate::workers::WorkerSpec;
use crate::workers::enumerate::EnumClock;

pub const PERIOD: Duration = Duration::from_secs(1);

/// The memory rate table (ADR-008 row "Memory, swap, zram"). The process table keeps it at
/// 1 Hz for the "Memory compression (zram)" pseudo row (PROC-012, R-166).
pub fn memory_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.memory || d.recording || d.process_table {
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
    /// `Demand.memory`. Written by `period()`, read by `sample()`.
    details_wanted: AtomicBool,
    /// When the swap list and zswap parameters are read again.
    details_clock: EnumClock,
    /// The last details sent.
    last_details: Option<Arc<MemoryDetails>>,
    /// Details not yet taken by `take_extra()`.
    pending_details: Option<Arc<MemoryDetails>>,
    /// Number of samples that read details. For tests.
    details_reads: u32,
}

impl MemorySource {
    pub fn new(root: SysRoot) -> Self {
        Self {
            sampler: MemorySampler::new(root),
            reading: MemoryReading::default(),
            zram_clock: EnumClock::default(),
            details_wanted: AtomicBool::new(false),
            details_clock: EnumClock::default(),
            last_details: None,
            pending_details: None,
            details_reads: 0,
        }
    }

    /// How many samples read the swap list and zswap parameters.
    pub fn details_reads(&self) -> u32 {
        self.details_reads
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        if self.zram_clock.due(clock.boot_ns) {
            self.sampler.rescan_zram();
        }
        let details =
            self.details_wanted.load(Ordering::Relaxed) && self.details_clock.due(clock.boot_ns);
        self.sampler
            .sample_at(clock, MemOpts { details }, &mut self.reading);
        if details {
            self.details_reads = self.details_reads.saturating_add(1);
            self.update_details();
        }
        let r = &self.reading;
        let info = &r.info;

        // The sum covers the devices whose `mm_stat` was read. When none was, it is not a
        // value: the first device's validity says why.
        let zram = if r.zram.is_empty() {
            Metric::with(Validity::Unsupported)
        } else if r.zram.iter().all(|z| z.mm.validity != Validity::Valid) {
            Metric::with(r.zram[0].mm.validity)
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

    /// Builds `MemoryDetails` from the last detailed reading. Queues it for `take_extra()` when
    /// it differs from the last one sent.
    fn update_details(&mut self) {
        let r = &self.reading;
        let next = MemoryDetails {
            zram: r.zram.iter().map(zram_detail).collect(),
            zswap: InvField {
                value: r.zswap.value.as_ref().map(zswap_info),
                validity: r.zswap.validity,
            },
            swaps: InvField {
                value: r.swaps.value.iter().map(swap_device).collect(),
                validity: r.swaps.validity,
            },
        };
        let changed = self.last_details.as_ref().is_none_or(|last| {
            last.zram != next.zram || last.zswap != next.zswap || last.swaps != next.swaps
        });
        if changed {
            let next = Arc::new(next);
            self.last_details = Some(Arc::clone(&next));
            self.pending_details = Some(next);
        }
    }
}

impl Source for MemorySource {
    fn id(&self) -> SourceId {
        SourceId::Memory
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        self.details_wanted.store(d.memory, Ordering::Relaxed);
        memory_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    fn reset_baselines(&mut self) {
        self.sampler.reset_baselines();
        self.zram_clock.force();
        self.details_clock.force();
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.pending_details
            .take()
            .map(|d| SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Memory(d)))
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

fn zram_detail(z: &ZramReading) -> ZramDetail {
    ZramDetail {
        name: Arc::clone(&z.name),
        disksize: z.disksize_bytes,
        algorithm: z.algorithm.clone(),
    }
}

fn zswap_info(p: &ZswapParams) -> ZswapInfo {
    ZswapInfo {
        enabled: p.enabled,
        compressor: p.compressor.clone(),
        max_pool_percent: p.max_pool_percent,
    }
}

fn swap_device(e: &SwapEntry) -> SwapDevice {
    SwapDevice {
        name: Arc::clone(&e.name),
        kind: match e.kind {
            SwapKind::Partition => SwapKindModel::Partition,
            SwapKind::File => SwapKindModel::File,
            SwapKind::Zram => SwapKindModel::Zram,
            SwapKind::Other => SwapKindModel::Other,
        },
        size_bytes: e.size_bytes,
        used_bytes: e.used_bytes,
        priority: e.priority,
    }
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

        let table = Demand {
            process_table: true,
            ..Demand::default()
        };
        assert_eq!(memory_period(&table, &s), Some(Duration::from_secs(1)));
    }
}
