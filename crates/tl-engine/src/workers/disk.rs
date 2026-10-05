//! The `Disk` worker (ADR-004, ADR-008): wraps the tl-collect `DiskSampler` and turns each
//! `DiskReading` into a `SourceUpdate::Disks(DiskSample)`.
//!
//! It samples `/proc/diskstats` at 1 Hz while disks are demanded or history recording is on,
//! and not at all otherwise. The block devices are enumerated again every 5 s; each enumeration
//! also sends a full `DeviceList` for `DISKS` through `take_extra` (R-052, PERF-005, PERF-047 to
//! PERF-049).

use std::sync::Arc;
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use tl_collect::SysRoot;
use tl_collect::counters::{Clock, Measured};
use tl_collect::parse::block::{self, BlockDevice};
use tl_collect::sample::disk::{DiskId, DiskReading, DiskSampler};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::health::SourceId;
use tl_model::ids::DeviceIdx;
use tl_model::settings::EngineSettings;
use tl_model::system::{DiskTotals, Metric};
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerCommand, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{DiskSample, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;
use crate::workers::enumerate::{self, EnumClock, GenTracker};

pub const PERIOD: Duration = Duration::from_secs(1);

/// The disk rate table (ADR-008 row "Disks").
pub fn disk_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.disks || d.recording {
        Some(PERIOD)
    } else {
        None
    }
}

pub struct DiskSource {
    root: SysRoot,
    sampler: DiskSampler,
    /// Reused every sample.
    reading: DiskReading,
    /// The last successful enumeration.
    disks: Vec<BlockDevice>,
    /// Identity, sampler id and capacity in bytes of every disk and child, in sampler order.
    ids: Vec<(DeviceIdentity, DiskId, u64)>,
    /// Keyed by (major, minor, name).
    gens: GenTracker<(u32, u32, Arc<str>)>,
    /// When the block devices are enumerated again.
    clock: EnumClock,
    /// The `Devices` list waiting for `take_extra`.
    pending: Option<SourceUpdate>,
}

impl DiskSource {
    pub fn new(root: SysRoot) -> Self {
        Self {
            sampler: DiskSampler::new(root.clone()),
            root,
            reading: DiskReading::default(),
            disks: Vec::new(),
            ids: Vec::new(),
            gens: GenTracker::default(),
            clock: EnumClock::default(),
            pending: None,
        }
    }

    /// The sampler ids of the last successful enumeration, disks then their children.
    pub fn disk_ids(&self) -> impl Iterator<Item = DiskId> + '_ {
        self.ids.iter().map(|(_, id, _)| *id)
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        if self.clock.due(clock.boot_ns) {
            self.enumerate(clock.boot_ns);
        }
        self.sampler.sample_at(clock, &mut self.reading);

        let disks = self
            .ids
            .iter()
            .zip(&self.reading.devices)
            .map(|((identity, _, capacity), r)| {
                let totals = DiskTotals {
                    // The worker cannot know the catalog index; the reducer sets it.
                    device: DeviceIdx(0),
                    active_pct: metric(r.active_pct),
                    read_bps: metric(r.read_bps),
                    write_bps: metric(r.write_bps),
                    response_ms: metric(r.latency_ms),
                    read_total: r.total_read_bytes.value,
                    written_total: r.total_written_bytes.value,
                    capacity: *capacity,
                    // E12-012 reads the drive temperature.
                    temp_c: Metric::with(Validity::Unsupported),
                };
                (identity.clone(), totals)
            })
            .collect();
        let sample = DiskSample {
            t_ns: self.reading.t_ns,
            disks,
        };
        (self, SourceUpdate::Disks(sample))
    }

    /// Rebuilds `ids`, the sampler's device list and the pending `Devices` list. A failed
    /// enumeration keeps the previous devices and sends no list.
    fn enumerate(&mut self, t_ns: u64) {
        let mut found = Vec::new();
        if block::enumerate_disks(&self.root, &mut found).is_err() {
            return;
        }
        self.disks = found;

        let mut flat: Vec<(DeviceIdentity, u32, u32, Arc<str>, u64)> = Vec::new();
        for disk in &self.disks {
            flat.push((
                enumerate::disk_identity(disk),
                disk.major,
                disk.minor,
                disk.name.clone(),
                disk.size_bytes,
            ));
            for child in &disk.children {
                flat.push((
                    enumerate::disk_child_identity(child),
                    child.major,
                    child.minor,
                    child.name.clone(),
                    child.size_bytes,
                ));
            }
        }
        let keys: Vec<(u32, u32, Arc<str>)> = flat
            .iter()
            .map(|(_, major, minor, name, _)| (*major, *minor, name.clone()))
            .collect();
        let generations = self.gens.update(&keys);
        self.ids = flat
            .into_iter()
            .zip(generations)
            .map(|((identity, major, minor, _, capacity), generation)| {
                let id = DiskId {
                    major,
                    minor,
                    generation,
                };
                (identity, id, capacity)
            })
            .collect();
        let sampler_ids: Vec<DiskId> = self.ids.iter().map(|(_, id, _)| *id).collect();
        self.sampler.set_devices(&sampler_ids);
        self.pending = Some(SourceUpdate::Devices(enumerate::disk_list(
            t_ns,
            &self.disks,
            &self.root.run,
        )));
    }
}

impl Source for DiskSource {
    fn id(&self) -> SourceId {
        SourceId::Disk
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        disk_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    fn reset_baselines(&mut self) {
        self.sampler.reset_baselines();
        self.clock.force();
    }

    /// `RefreshNow` is handled by `source::run`.
    fn command(&mut self, _cmd: &WorkerCommand) -> bool {
        false
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.pending.take()
    }
}

/// In the full engine only.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Disk,
        name: "disk",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// A new `DiskSource` per generation, built on the blocking pool, then `source::run`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let root = cfg.sys_root.clone();
        async move {
            let mut job = match cx.spawn_blocking(move |_| DiskSource::new(root)) {
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

fn metric<T: Copy + Default>(m: Measured<T>) -> Metric<T> {
    Metric {
        value: m.value,
        validity: m.validity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_table() {
        let s = EngineSettings::default();
        assert_eq!(disk_period(&Demand::default(), &s), None);

        let disks = Demand {
            disks: true,
            ..Demand::default()
        };
        assert_eq!(disk_period(&disks, &s), Some(Duration::from_secs(1)));

        let recording = Demand {
            recording: true,
            ..Demand::default()
        };
        assert_eq!(disk_period(&recording, &s), Some(Duration::from_secs(1)));
    }
}
