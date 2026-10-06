//! The `Disk` worker (ADR-004, ADR-008): wraps the tl-collect `DiskSampler` and turns each
//! `DiskReading` into a `SourceUpdate::Disks(DiskSample)`.
//!
//! It samples `/proc/diskstats` at 1 Hz while disks are demanded or history recording is on,
//! and not at all otherwise. The block devices are enumerated again every 5 s; each enumeration
//! also sends a full `DeviceList` for `DISKS` and then the sysfs `DiskDetails` (kind, usage,
//! system disk, swap) through `take_extra` (R-052, PERF-005, PERF-047 to PERF-049). The drive
//! temperature is read every sample from the hwmon input found at enumeration (PERF-053).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use tl_collect::counters::{Clock, Measured};
use tl_collect::parse::block::{self, BlockDevice, DiskKind};
use tl_collect::parse::mountinfo::{MountEntry, parse_mountinfo};
use tl_collect::parse::swaps::{SwapEntry, parse_swaps};
use tl_collect::sample::disk::{
    DiskId, DiskReading, DiskSampler, DiskUsage, FsStat, disk_usage, host_statvfs, host_swap_dev,
};
use tl_collect::sysroot::read_file;
use tl_collect::{CollectError, SysRoot};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::health::SourceId;
use tl_model::ids::DeviceIdx;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{DiskDetails, InvField};
use tl_model::system::{DiskTotals, Metric};
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerCommand, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{DeviceDetailsUpdate, DiskSample, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;
use crate::workers::enumerate::{self, EnumClock, GenTracker};

pub const PERIOD: Duration = Duration::from_secs(1);

/// Most bytes read from `/proc/self/mountinfo` or `/proc/swaps`.
const TABLE_LIMIT: usize = 1 << 20;

/// The `statvfs` callback given to `disk_usage`.
type StatVfs = Box<dyn FnMut(&str) -> Result<FsStat, CollectError> + Send>;

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
    /// The `Devices` list and then the `DeviceDetails` waiting for `take_extra`.
    pending: VecDeque<SourceUpdate>,
    statvfs: StatVfs,
    /// Reused on each enumeration.
    buf: Vec<u8>,
    mounts: Vec<MountEntry>,
    swaps: Vec<SwapEntry>,
}

impl DiskSource {
    /// On the host, usage comes from the real `statvfs`. Under any other root the mount points
    /// belong to the fixture, so every `statvfs` reads `Missing`.
    pub fn new(root: SysRoot) -> Self {
        let statvfs: StatVfs = if root == SysRoot::host() {
            Box::new(host_statvfs)
        } else {
            Box::new(|_: &str| Err(CollectError::NotFound))
        };
        Self::with_statvfs(root, statvfs)
    }

    /// Test hook: `statvfs` answers in place of the real call.
    #[doc(hidden)]
    pub fn with_statvfs(root: SysRoot, statvfs: StatVfs) -> Self {
        Self {
            sampler: DiskSampler::new(root.clone()),
            root,
            reading: DiskReading::default(),
            disks: Vec::new(),
            ids: Vec::new(),
            gens: GenTracker::default(),
            clock: EnumClock::default(),
            pending: VecDeque::new(),
            statvfs,
            buf: Vec::new(),
            mounts: Vec::new(),
            swaps: Vec::new(),
        }
    }

    /// The sampler ids of the last successful enumeration, disks then their children.
    pub fn disk_ids(&self) -> impl Iterator<Item = DiskId> + '_ {
        self.ids.iter().map(|(_, id, _)| *id)
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        // `run` drains every extra after each sample, so anything left is from an older tick.
        self.pending.clear();
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
                    // Children have no temperature input, so they read `Unsupported`.
                    temp_c: metric(r.temp_c),
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

    /// Rebuilds `ids`, the sampler's device and temperature lists, and the pending `Devices` list
    /// and `DeviceDetails`. A failed enumeration keeps the previous devices and sends nothing.
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
        let temps: Vec<_> = self
            .disks
            .iter()
            .map(|d| {
                let input = block::disk_temp_input(&self.root, &d.name);
                (d.major, d.minor, input)
            })
            .collect();
        self.sampler.set_temp_inputs(&temps);
        self.pending
            .push_back(SourceUpdate::Devices(enumerate::disk_list(
                t_ns,
                &self.disks,
                &self.root.run,
            )));
        let details = self.details();
        self.pending
            .push_back(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks {
                from: SourceId::Disk,
                entries: details,
            }));
    }

    /// Kind, model, removable, usage, system disk and swap of every whole disk. Mount points and
    /// swap paths are never logged.
    fn details(&mut self) -> Vec<(DeviceIdentity, DiskDetails)> {
        let mounts = read_file(
            &self.root.proc_path("self/mountinfo"),
            &mut self.buf,
            TABLE_LIMIT,
        )
        .and_then(|_| parse_mountinfo(&self.buf, &mut self.mounts));
        if mounts.is_err() {
            self.mounts.clear();
        }
        let swaps = read_file(&self.root.proc_path("swaps"), &mut self.buf, TABLE_LIMIT)
            .and_then(|_| parse_swaps(&self.buf, &mut self.swaps));
        if swaps.is_err() {
            self.swaps.clear();
        }
        // A kernel built without swap has no `/proc/swaps`: no disk holds swap.
        let swaps_ok = matches!(swaps, Ok(()) | Err(CollectError::NotFound));

        let usage = if self.root == SysRoot::host() {
            disk_usage(
                &self.disks,
                &self.mounts,
                &self.swaps,
                &mut self.statvfs,
                &mut host_swap_dev,
            )
        } else {
            // A fixture's swap paths name its own devices, never the host's `/dev`.
            let disks = &self.disks;
            disk_usage(
                disks,
                &self.mounts,
                &self.swaps,
                &mut self.statvfs,
                &mut |name: &str| fixture_swap_dev(disks, name),
            )
        };

        self.disks
            .iter()
            .zip(usage)
            .map(|(disk, u)| {
                let details = disk_details(disk, &u, mounts.as_ref().err(), swaps_ok);
                (enumerate::disk_identity(disk), details)
            })
            .collect()
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
        self.pending.pop_front()
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

/// The sysfs fields of one disk's `DiskDetails`; the rest stay `Default` (R-139 item 7). A
/// failed mount table read gives the usage fields its validity and `system_disk` `Failed`.
fn disk_details(
    disk: &BlockDevice,
    usage: &DiskUsage,
    mounts_err: Option<&CollectError>,
    swaps_ok: bool,
) -> DiskDetails {
    let bytes = |m: Measured<u64>| match mounts_err {
        Some(err) => InvField {
            value: 0,
            validity: err.validity(),
        },
        None => InvField {
            value: m.value,
            validity: m.validity,
        },
    };
    let flag = |value: bool, ok: bool| InvField {
        value: ok && value,
        validity: if ok {
            Validity::Valid
        } else {
            Validity::Failed
        },
    };
    DiskDetails {
        kind: Arc::from(kind_name(disk.kind)),
        removable: disk.removable,
        model: match &disk.model {
            Some(model) => InvField {
                value: model.clone(),
                validity: Validity::Valid,
            },
            None => InvField {
                value: Arc::from(""),
                validity: Validity::Unsupported,
            },
        },
        formatted_bytes: bytes(usage.formatted_bytes),
        used_bytes: bytes(usage.used_bytes),
        free_bytes: bytes(usage.free_bytes),
        system_disk: flag(usage.system_disk, mounts_err.is_none()),
        swap: flag(usage.swap, swaps_ok),
        ..DiskDetails::default()
    }
}

/// The PERF-047 kind names.
fn kind_name(kind: DiskKind) -> &'static str {
    match kind {
        DiskKind::Nvme => "NVMe",
        DiskKind::Ssd => "SSD",
        DiskKind::Hdd => "HDD",
        DiskKind::Emmc => "eMMC",
        DiskKind::Sd => "SD",
        DiskKind::Usb => "USB",
        DiskKind::Optical => "Optical",
        DiskKind::Virtual => "Virtual",
    }
}

/// `/dev/<name>` of an enumerated disk or child, for fixture roots.
fn fixture_swap_dev(disks: &[BlockDevice], name: &str) -> Option<(u32, u32)> {
    let name = name.strip_prefix("/dev/")?;
    disks.iter().find_map(|d| {
        if &*d.name == name {
            return Some((d.major, d.minor));
        }
        d.children
            .iter()
            .find(|c| &*c.name == name)
            .map(|c| (c.major, c.minor))
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
