//! Process metadata cache: live records, exited records within the linger time, and the
//! `NEW` and `EXITED` flags (ADR-003, ADR-004, PROC-043, R-041).
//!
//! The reducer keeps every live process plus at most `MAX_EXITED` exited ones. A new key gets
//! `NEW` and a key missing from a sweep moves to the exited list with `EXITED`. Both marks last
//! for the linger time, `2 × processes.interval_ms` clamped to 1..=3 s; `expire` on `Tick`
//! clears `NEW` and drops exited records after it.
//!
//! Two workers enrich the records after the sweep (ADR-007, ADR-008, PROC-033). GPU samples map
//! to keys through the pids of the live table; a pid not in the table is dropped. PSS and USS come
//! for the detail keys only, and `coverage` lists the keys that have them. Both kinds of values are
//! kept per key and copied onto the next sweep's records, because the sweep sends them as zero.
//!
//! Privacy: never log `ProcessMeta` text from here. Debug logs carry counts only (ADR-015).

use std::collections::VecDeque;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tl_collect::net::NetRate;
use tl_model::devices::DeviceIdentity;
use tl_model::gpu_proto::{GpuProcessSample, Reading};
use tl_model::history::RecordSink;
use tl_model::ids::ProcessKey;
use tl_model::process::{
    AppCgroupSample, Coverage, GpuVram, LowMemoryInfo, ProcFlags, ProcessExtras, ProcessExtrasP2,
    ProcessRecord, linger_ms,
};
use tl_model::snapshot::{ComponentHeader, ProcessSnapshot};
use tl_model::time::TimeNs;
use tl_model::validity::{FieldValidity, ProcessField, Validity};

use crate::update::{CgroupStats, DrmProcessVram, ProcessSweep, PssEntry};

/// Exited records kept at most. The oldest by exit time go first.
pub const MAX_EXITED: usize = 256;

/// Flags the sweep sets. The reducer owns `NEW` and `EXITED`.
const SWEEP_FLAGS: ProcFlags = ProcFlags::KERNEL_THREAD
    .union(ProcFlags::OTHER_USER)
    .union(ProcFlags::IS_SELF)
    .union(ProcFlags::PROTECTED);

/// Linger time in nanoseconds: `clamp(2 × interval_ms, 1000, 3000) × 1e6` (R-041).
pub fn linger_ns(interval_ms: u32) -> u64 {
    u64::from(linger_ms(interval_ms)) * 1_000_000
}

struct Live {
    record: ProcessRecord,
    first_seen_ns: TimeNs,
}

/// Per-process GPU values of the last `apply_gpu_processes`.
#[derive(Clone)]
struct GpuValue {
    util_pct: f32,
    mem_bytes: u64,
    /// Per GPU column, see `ProcessExtrasP2::gpu_vram_bytes`.
    vram: GpuVram,
    validity: Validity,
}

/// Per-process PSS values of the last `apply_pss` (or of a sweep that read them).
#[derive(Clone, Copy)]
struct PssValue {
    pss_bytes: u64,
    uss_bytes: u64,
    swap_bytes: u64,
    validity: Validity,
}

pub struct ProcessState {
    live: FxHashMap<ProcessKey, Live>,
    /// Flags of every live key, kept equal to `live[key].record.flags`.
    live_flags: FxHashMap<ProcessKey, ProcFlags>,
    /// Pid of every live key, for GPU pid mapping (ADR-007).
    live_pids: FxHashMap<u32, ProcessKey>,
    /// `(key, exit seen, last record)`, oldest exit first.
    exited: VecDeque<(ProcessKey, TimeNs, ProcessRecord)>,
    /// From `ProcessSweep.extras` (R-141 item 1). Exited keys keep their last extras.
    extras: FxHashMap<ProcessKey, ProcessExtras>,
    /// From `ProcessSweep.extras_p2`, filled entries only (R-163 item 2).
    extras_p2: FxHashMap<ProcessKey, ProcessExtrasP2>,
    /// Sorted by `GroupKey` (R-141 item 2).
    app_stats: Vec<AppCgroupSample>,
    unit_stats: Vec<CgroupStats>,
    /// From `ProcessSweep.low_memory` (R-150 item 2).
    low_memory: Option<Arc<LowMemoryInfo>>,
    /// GPU values of the keys in the last GPU sample.
    gpu: FxHashMap<ProcessKey, GpuValue>,
    /// Per GPU UUID: each key's utilisation on that GPU in the last GPU sample, for the power
    /// estimate (E09-030).
    gpu_by_dev: FxHashMap<[u8; 16], FxHashMap<ProcessKey, f32>>,
    /// GPU validity of live keys without a sample; `None` before the first GPU sample.
    gpu_default: Option<Validity>,
    /// True when the last GPU sample had process rows.
    gpu_process: bool,
    /// UUID and PCI bus id of each GPU, in the order of the `gpu_0` to `gpu_15` columns, from
    /// `set_gpu_slots`.
    gpu_slots: SmallVec<[([u8; 16], Arc<str>); 4]>,
    /// The NVIDIA rows of the last `apply_gpu_processes` and the DRM rows of the last
    /// `apply_drm_vram`. A sample from one GPU worker keeps the other worker's rows.
    gpu_rows: Vec<GpuProcessSample>,
    drm_rows: Vec<DrmProcessVram>,
    /// PSS values of the detail keys.
    pss: FxHashMap<ProcessKey, PssValue>,
    /// Attributed TCP rates of the last `ConnectionsUpdate`; `None` before the first one.
    net: Option<FxHashMap<ProcessKey, NetRate>>,
    system_threads: u32,
    file_nr: Option<u64>,
    online_cpus: u16,
    last_sweep_ns: TimeNs,
    /// False when the sweep's extras were not index aligned with its records. `publish` then
    /// sends empty extras rather than made-up values.
    extras_complete: bool,
    version: u64,
    dirty: bool,
}

impl Default for ProcessState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessState {
    pub fn new() -> Self {
        Self {
            live: FxHashMap::default(),
            live_flags: FxHashMap::default(),
            live_pids: FxHashMap::default(),
            exited: VecDeque::new(),
            extras: FxHashMap::default(),
            extras_p2: FxHashMap::default(),
            app_stats: Vec::new(),
            unit_stats: Vec::new(),
            low_memory: None,
            gpu: FxHashMap::default(),
            gpu_by_dev: FxHashMap::default(),
            gpu_default: None,
            gpu_process: false,
            gpu_slots: SmallVec::new(),
            gpu_rows: Vec::new(),
            drm_rows: Vec::new(),
            pss: FxHashMap::default(),
            net: None,
            system_threads: 0,
            file_nr: None,
            online_cpus: 0,
            last_sweep_ns: 0,
            extras_complete: true,
            version: 0,
            dirty: false,
        }
    }

    /// Diff the sweep against `live`: new keys get NEW (first_seen = observed_end); missing keys
    /// move to `exited` with EXITED; cap 256 (drop oldest).
    pub fn apply_sweep(
        &mut self,
        sweep: ProcessSweep,
        now_ns: TimeNs,
        interval_ms: u32,
        sink: Option<&mut dyn RecordSink>,
    ) {
        let linger = linger_ns(interval_ms);
        let seen_ns = sweep.observed_end;
        let mut old = std::mem::take(&mut self.live);
        self.live_flags.clear();
        self.live_pids.clear();
        self.live.reserve(sweep.records.len());
        self.live_flags.reserve(sweep.records.len());
        self.live_pids.reserve(sweep.records.len());

        let aligned = sweep.extras.len() == sweep.records.len();
        debug_assert!(
            aligned,
            "ProcessSweep.extras is not index aligned with records"
        );
        self.extras_complete = aligned;
        let p2_aligned = sweep.extras_p2.len() == sweep.records.len();
        let mut extras = sweep.extras.into_iter();
        let mut extras_p2 = sweep.extras_p2.into_iter();

        for mut record in sweep.records {
            let key = record.key;
            let first_seen_ns = match old.remove(&key) {
                Some(prev) => prev.first_seen_ns,
                None => {
                    // A key never leaves `exited` and comes back, but if it does, keep one copy.
                    if let Some(i) = self.exited.iter().position(|(k, _, _)| *k == key) {
                        self.exited.remove(i);
                    }
                    seen_ns
                }
            };
            let mut flags = record.flags & SWEEP_FLAGS;
            if now_ns.saturating_sub(first_seen_ns) < linger {
                flags |= ProcFlags::NEW;
            }
            record.flags = flags;
            self.overlay(&mut record);

            if aligned && let Some(e) = extras.next() {
                self.extras.insert(key, e);
            }
            match extras_p2.next() {
                Some(p2) if p2_aligned && p2.key.is_some() => {
                    self.extras_p2.insert(key, p2);
                }
                _ => {
                    self.extras_p2.remove(&key);
                }
            }

            self.live_flags.insert(key, flags);
            self.live_pids.insert(key.pid, key);
            self.live.insert(
                key,
                Live {
                    record,
                    first_seen_ns,
                },
            );
        }

        // Keys missing from this sweep exited. Key order keeps the cap deterministic.
        let mut gone: Vec<(ProcessKey, ProcessRecord)> =
            old.into_iter().map(|(k, l)| (k, l.record)).collect();
        gone.sort_unstable_by_key(|(k, _)| *k);
        for (key, mut record) in gone {
            record.flags = (record.flags - ProcFlags::NEW) | ProcFlags::EXITED;
            self.exited.push_back((key, seen_ns, record));
        }
        while self.exited.len() > MAX_EXITED {
            if let Some((key, _, _)) = self.exited.pop_front() {
                self.forget(&key);
            }
        }

        let mut app_stats = sweep.app_stats;
        app_stats.sort_unstable_by_key(|a| a.group);
        self.app_stats = app_stats;
        self.unit_stats = sweep.unit_stats;
        self.low_memory = sweep.low_memory;
        self.system_threads = sweep.system_threads;
        self.file_nr = sweep.file_nr;
        self.online_cpus = sweep.online_cpus;
        self.last_sweep_ns = seen_ns;
        self.dirty = true;

        if let Some(sink) = sink {
            let mut records: Vec<ProcessRecord> =
                self.live.values().map(|l| l.record.clone()).collect();
            records.sort_unstable_by_key(|r| r.key);
            let records: Arc<[ProcessRecord]> = records.into();
            sink.on_process_sweep(seen_ns, &records);
        }
    }

    /// Copies the kept GPU and PSS values onto a new sweep record. A record whose PSS the sweep
    /// read itself (validity `Valid`) keeps it and updates the kept value.
    fn overlay(&mut self, record: &mut ProcessRecord) {
        let key = record.key;
        if let Some(g) = self.gpu.get(&key) {
            set_gpu(record, g);
        } else if let Some(v) = self.gpu_default {
            set_gpu(record, &GpuValue::none(v));
        }
        if let Some(net) = &self.net {
            set_net(record, net.get(&key));
        }
        if record.validity.get(ProcessField::Pss) == Validity::Valid {
            self.pss.insert(
                key,
                PssValue {
                    pss_bytes: record.pss_bytes,
                    uss_bytes: record.uss_bytes,
                    swap_bytes: record.swap_bytes,
                    validity: Validity::Valid,
                },
            );
        } else if let Some(p) = self.pss.get(&key) {
            set_pss(record, p);
        }
    }

    /// Sets the GPUs in column order (`gpu_0` first). The next `apply_gpu_processes` or
    /// `apply_drm_vram` puts each device's memory at its position in the published
    /// `ProcessExtrasP2::gpu_vram_bytes`; a device not in the list counts only in
    /// `gpu_mem_bytes`. NVIDIA rows match a GPU by UUID, DRM rows by PCI slot.
    pub fn set_gpu_slots<'a>(&mut self, gpus: impl IntoIterator<Item = &'a DeviceIdentity>) {
        self.gpu_slots.clear();
        self.gpu_slots
            .extend(gpus.into_iter().filter_map(|id| match id {
                DeviceIdentity::Gpu { uuid, pci_bus_id } => Some((*uuid, Arc::clone(pci_bus_id))),
                _ => None,
            }));
    }

    /// pid → key through `live`; sums over devices per pid; sets gpu_util_pct = max over devices
    /// of max(sm, enc, dec), gpu_mem_bytes = Σ mem_used and the memory per GPU column;
    /// validity Valid, or Unsupported when the device has `supports_process_util == false`.
    /// Returns the number of keys matched, with the DRM rows of the last `apply_drm_vram`
    /// counted too.
    ///
    /// Live keys without a sample get 0 with validity `Valid` when some device has
    /// `supports_process_util`, else `Unsupported` (PROC-033). A device missing from
    /// `device_caps` counts as unsupported. Utilization is never divided among processes.
    pub fn apply_gpu_processes(
        &mut self,
        samples: &[GpuProcessSample],
        device_caps: &FxHashMap<[u8; 16], bool>,
    ) -> u32 {
        self.gpu_rows.clear();
        self.gpu_rows.extend_from_slice(samples);
        self.apply_gpu_rows(device_caps)
    }

    /// Replaces the DRM rows (AMD and Intel) and applies them with the NVIDIA rows of the last
    /// `apply_gpu_processes`. Each row adds its memory to `gpu_mem_bytes` and to its card's
    /// column. Utilization and validity come from the NVIDIA rows only, as DRM cards have no
    /// per-process utilization. Returns the number of keys matched.
    pub fn apply_drm_vram(
        &mut self,
        rows: &[DrmProcessVram],
        device_caps: &FxHashMap<[u8; 16], bool>,
    ) -> u32 {
        self.drm_rows.clear();
        self.drm_rows.extend_from_slice(rows);
        self.apply_gpu_rows(device_caps)
    }

    /// Rebuilds the per-key GPU values from `gpu_rows` and `drm_rows`.
    fn apply_gpu_rows(&mut self, device_caps: &FxHashMap<[u8; 16], bool>) -> u32 {
        let any_supported = device_caps.values().any(|&s| s);
        let default = if any_supported {
            Validity::Valid
        } else {
            Validity::Unsupported
        };

        self.gpu.clear();
        self.gpu_by_dev.clear();
        for s in &self.gpu_rows {
            let Some(&key) = self.live_pids.get(&s.pid) else {
                continue;
            };
            let supported = device_caps.get(&s.uuid).copied().unwrap_or(false);
            let util = [s.sm_pct, s.enc_pct, s.dec_pct]
                .into_iter()
                .filter_map(Reading::ok)
                .max()
                .unwrap_or(0) as f32;
            let mem = s.mem_used.ok().unwrap_or(0);
            let on_dev = self
                .gpu_by_dev
                .entry(s.uuid)
                .or_default()
                .entry(key)
                .or_insert(0.0);
            *on_dev = on_dev.max(util);
            let entry = self
                .gpu
                .entry(key)
                .or_insert_with(|| GpuValue::none(Validity::Unsupported));
            entry.util_pct = entry.util_pct.max(util);
            entry.mem_bytes = entry.mem_bytes.saturating_add(mem);
            let slot = self.gpu_slots.iter().position(|(u, _)| *u == s.uuid);
            add_vram(entry, slot, mem);
            if supported {
                entry.validity = Validity::Valid;
            }
        }
        for r in &self.drm_rows {
            let Some(&key) = self.live_pids.get(&r.pid) else {
                continue;
            };
            let entry = self
                .gpu
                .entry(key)
                .or_insert_with(|| GpuValue::none(default));
            entry.mem_bytes = entry.mem_bytes.saturating_add(r.vram_bytes);
            // DRM cards have no UUID (R-166).
            let slot = self
                .gpu_slots
                .iter()
                .position(|(u, pci)| *u == [0; 16] && *pci == r.pdev);
            add_vram(entry, slot, r.vram_bytes);
        }

        let none = GpuValue::none(default);
        for live in self.live.values_mut() {
            let g = self.gpu.get(&live.record.key).unwrap_or(&none);
            set_gpu(&mut live.record, g);
        }
        self.gpu_default = Some(default);
        self.gpu_process = !self.gpu_rows.is_empty() || !self.drm_rows.is_empty();
        self.dirty = true;
        u32::try_from(self.gpu.len()).unwrap_or(u32::MAX)
    }

    /// Sets the Network columns of every live record from the attributed TCP rates (PROC-032)
    /// and keeps them for the next sweeps. A live key with no attributed socket gets 0 with
    /// validity `Valid`.
    pub fn apply_net_rates(&mut self, rates: FxHashMap<ProcessKey, NetRate>) {
        for live in self.live.values_mut() {
            let rate = rates.get(&live.record.key);
            set_net(&mut live.record, rate);
        }
        self.net = Some(rates);
        self.dirty = true;
    }

    /// For each entry: set pss_bytes, uss_bytes, swap_bytes and the Pss/Uss validity; keys not
    /// in `live` are ignored. An entry with validity `Valid` puts its key in `coverage().pss_uss`.
    pub fn apply_pss(&mut self, entries: &[PssEntry]) {
        for e in entries {
            let Some(live) = self.live.get_mut(&e.key) else {
                continue;
            };
            let value = PssValue {
                pss_bytes: e.pss_bytes,
                uss_bytes: e.uss_bytes,
                swap_bytes: e.swap_bytes,
                validity: e.validity,
            };
            set_pss(&mut live.record, &value);
            self.pss.insert(e.key, value);
            self.dirty = true;
        }
    }

    /// Keys with valid PSS and USS, sorted ascending.
    pub fn coverage(&self) -> Coverage {
        let mut keys: Vec<ProcessKey> = self
            .pss
            .iter()
            .filter(|(_, p)| p.validity == Validity::Valid)
            .map(|(k, _)| *k)
            .collect();
        keys.sort_unstable();
        Coverage {
            pss_uss: keys.into(),
        }
    }

    /// Per GPU UUID: each key's utilisation on that GPU in the last GPU sample (E09-030).
    pub fn gpu_by_dev(&self) -> &FxHashMap<[u8; 16], FxHashMap<ProcessKey, f32>> {
        &self.gpu_by_dev
    }

    /// True when the last GPU sample had process rows. `Coverage` has no field for it yet.
    pub fn gpu_process(&self) -> bool {
        self.gpu_process
    }

    /// When detail_keys shrink, PSS of dropped keys becomes WarmingUp.
    pub fn clear_pss_outside(&mut self, keep: &[ProcessKey]) {
        let keep: FxHashSet<ProcessKey> = keep.iter().copied().collect();
        let dropped: Vec<ProcessKey> = self
            .pss
            .keys()
            .filter(|k| !keep.contains(k))
            .copied()
            .collect();
        for key in dropped {
            self.pss.remove(&key);
            if let Some(live) = self.live.get_mut(&key) {
                live.record.pss_bytes = 0;
                live.record.uss_bytes = 0;
                live.record
                    .validity
                    .set(ProcessField::Pss, Validity::WarmingUp);
                live.record
                    .validity
                    .set(ProcessField::Uss, Validity::WarmingUp);
                self.dirty = true;
            }
        }
    }

    /// Called on Tick: clears NEW after linger, drops exited after linger; returns changed.
    pub fn expire(&mut self, now_ns: TimeNs, interval_ms: u32) -> bool {
        let linger = linger_ns(interval_ms);
        let mut changed = false;

        for (key, live) in &mut self.live {
            if live.record.flags.contains(ProcFlags::NEW)
                && now_ns.saturating_sub(live.first_seen_ns) >= linger
            {
                live.record.flags.remove(ProcFlags::NEW);
                if let Some(flags) = self.live_flags.get_mut(key) {
                    flags.remove(ProcFlags::NEW);
                }
                changed = true;
            }
        }

        let before = self.exited.len();
        let mut dropped = Vec::new();
        self.exited.retain(|(key, exit_ns, _)| {
            let keep = now_ns.saturating_sub(*exit_ns) < linger;
            if !keep {
                dropped.push(*key);
            }
            keep
        });
        for key in &dropped {
            self.forget(key);
        }
        changed |= self.exited.len() != before;

        self.dirty |= changed;
        changed
    }

    /// Flags of a live or exited key.
    pub fn flags(&self, key: &ProcessKey) -> Option<ProcFlags> {
        self.live_flags.get(key).copied().or_else(|| {
            self.exited
                .iter()
                .find(|(k, _, _)| k == key)
                .map(|(_, _, r)| r.flags)
        })
    }

    /// Live keys and their flags, for action admission (E11).
    pub fn live_keys(&self) -> &FxHashMap<ProcessKey, ProcFlags> {
        &self.live_flags
    }

    /// Keys that exited and are still within the linger time.
    pub fn exited_within_linger(&self) -> FxHashSet<ProcessKey> {
        self.exited.iter().map(|(k, _, _)| *k).collect()
    }

    /// Live plus exited records, sorted by `ProcessKey`.
    pub fn records_sorted(&self) -> Vec<ProcessRecord> {
        let mut records = Vec::with_capacity(self.live.len() + self.exited.len());
        records.extend(self.live.values().map(|l| l.record.clone()));
        records.extend(self.exited.iter().map(|(_, _, r)| r.clone()));
        records.sort_unstable_by_key(|r| r.key);
        records
    }

    /// Builds the snapshot when anything changed since the last publish. `extras` and
    /// `extras_p2` are index aligned with `records`; `apps` is sorted by `GroupKey`.
    pub fn publish(
        &mut self,
        coverage: Coverage,
        observed: (TimeNs, TimeNs),
    ) -> Option<Arc<ProcessSnapshot>> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        self.version += 1;

        let records = self.records_sorted();
        let extras: Arc<[ProcessExtras]> = if self.extras_complete {
            let aligned: Option<Vec<ProcessExtras>> = records
                .iter()
                .map(|r| self.extras.get(&r.key).cloned())
                .collect();
            aligned.map(Arc::from).unwrap_or_else(|| Arc::from([]))
        } else {
            Arc::from([])
        };
        let extras_p2: Arc<[ProcessExtrasP2]> = records
            .iter()
            .map(|r| {
                let mut p2 = self.extras_p2.get(&r.key).cloned().unwrap_or_default();
                if let Some(g) = self.gpu.get(&r.key) {
                    p2.gpu_vram_bytes.clone_from(&g.vram);
                }
                p2
            })
            .collect();

        Some(Arc::new(ProcessSnapshot {
            header: ComponentHeader {
                version: self.version,
                observed_start: observed.0,
                observed_end: observed.1,
                validity: Validity::Valid,
            },
            records: records.into(),
            coverage,
            extras,
            apps: self.app_stats.as_slice().into(),
            extras_p2,
            low_memory: self.low_memory.clone(),
        }))
    }

    /// Cgroup samples of the units in the last sweep.
    pub fn unit_stats(&self) -> &[CgroupStats] {
        &self.unit_stats
    }

    /// App cgroup samples of the last sweep, sorted by `GroupKey`.
    pub fn app_stats(&self) -> &[AppCgroupSample] {
        &self.app_stats
    }

    pub fn system_threads(&self) -> u32 {
        self.system_threads
    }

    /// `/proc/sys/fs/file-nr` from the last sweep.
    pub fn file_nr(&self) -> Option<u64> {
        self.file_nr
    }

    pub fn online_cpus(&self) -> u16 {
        self.online_cpus
    }

    /// `observed_end` of the last sweep, 0 before the first.
    pub fn last_sweep_ns(&self) -> TimeNs {
        self.last_sweep_ns
    }

    /// Drops the extras of a key that left the cache.
    fn forget(&mut self, key: &ProcessKey) {
        self.extras.remove(key);
        self.extras_p2.remove(key);
        self.gpu.remove(key);
        self.pss.remove(key);
    }
}

impl GpuValue {
    /// No GPU use, with `validity`.
    fn none(validity: Validity) -> Self {
        Self {
            util_pct: 0.0,
            mem_bytes: 0,
            vram: GpuVram::new(),
            validity,
        }
    }
}

/// Adds `bytes` to the GPU column `slot`; `None` is a GPU without a column.
fn add_vram(entry: &mut GpuValue, slot: Option<usize>, bytes: u64) {
    let Some(slot) = slot else {
        return;
    };
    if entry.vram.len() <= slot {
        entry.vram.resize(slot + 1, 0);
    }
    entry.vram[slot] = entry.vram[slot].saturating_add(bytes);
}

fn set_gpu(record: &mut ProcessRecord, g: &GpuValue) {
    record.gpu_util_pct = g.util_pct;
    record.gpu_mem_bytes = g.mem_bytes;
    set_fields(
        &mut record.validity,
        &[ProcessField::GpuUtil, ProcessField::GpuMem],
        g.validity,
    );
}

fn set_net(record: &mut ProcessRecord, rate: Option<&NetRate>) {
    let (rx, tx, shared, validity) = rate.map_or((0.0, 0.0, false, Validity::Valid), |r| {
        (r.rx_bps, r.tx_bps, r.shared, r.validity)
    });
    record.net_rx_bps = rx;
    record.net_tx_bps = tx;
    record.net_shared = shared;
    set_fields(
        &mut record.validity,
        &[ProcessField::NetRx, ProcessField::NetTx],
        validity,
    );
}

fn set_pss(record: &mut ProcessRecord, p: &PssValue) {
    record.pss_bytes = p.pss_bytes;
    record.uss_bytes = p.uss_bytes;
    record.swap_bytes = p.swap_bytes;
    set_fields(
        &mut record.validity,
        &[ProcessField::Pss, ProcessField::Uss],
        p.validity,
    );
}

fn set_fields(validity: &mut FieldValidity, fields: &[ProcessField], v: Validity) {
    for &f in fields {
        validity.set(f, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tl_model::actions::{CgroupRef, SchedPolicy};
    use tl_model::devices::DeviceCatalog;
    use tl_model::ids::{GroupKey, SeriesId};
    use tl_model::process::{ProcState, ProcessMeta};
    use tl_model::series::Sample;
    use tl_model::system::CpuSet;
    use tl_model::time::BootId;

    const MS: u64 = 1_000_000;
    const INTERVAL_MS: u32 = 1000;

    fn key(pid: u32, start_ticks: u64) -> ProcessKey {
        ProcessKey {
            start_ticks,
            boot_id: BootId(1),
            pid,
        }
    }

    fn record(key: ProcessKey) -> ProcessRecord {
        ProcessRecord {
            key,
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: "x".into(),
                name: "x".into(),
                exe: None,
                cmdline: None,
                user: "user".into(),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: "x".into(),
            }),
            group: GroupKey(0),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: sweep_validity(),
        }
    }

    /// What the sweep sends: PSS, USS and GPU fields not measured yet.
    fn sweep_validity() -> FieldValidity {
        let mut v = FieldValidity::all(Validity::Valid);
        for f in [
            ProcessField::Pss,
            ProcessField::Uss,
            ProcessField::GpuUtil,
            ProcessField::GpuMem,
        ] {
            v.set(f, Validity::WarmingUp);
        }
        v
    }

    /// `start_boot_ns` carries the key's `start_ticks` so tests can check index alignment.
    fn extras(key: ProcessKey) -> ProcessExtras {
        ProcessExtras {
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            start_boot_ns: key.start_ticks,
            rss_peak_bytes: 0,
            rss_shared_bytes: 0,
            commit_bytes: 0,
            vsize_bytes: 0,
            vsize_peak_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            policy: SchedPolicy::Other,
            rt_prio: 0,
            affinity: CpuSet::empty(),
            handles: None,
            exe_name: "x".into(),
            exe_path: "/usr/bin/x".into(),
            exe_deleted: false,
            unit: None,
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
        }
    }

    fn sweep(keys: &[ProcessKey], observed_end: TimeNs) -> ProcessSweep {
        let mut keys = keys.to_vec();
        keys.sort_unstable();
        ProcessSweep {
            observed_start: observed_end.saturating_sub(10 * MS),
            observed_end,
            records: keys.iter().map(|&k| record(k)).collect(),
            extras: keys.iter().map(|&k| extras(k)).collect(),
            extras_p2: keys.iter().map(|_| ProcessExtrasP2::default()).collect(),
            app_stats: Vec::new(),
            unit_stats: Vec::new(),
            system_threads: 0,
            file_nr: None,
            online_cpus: 1,
            low_memory: None,
        }
    }

    fn apply(state: &mut ProcessState, keys: &[ProcessKey], t: TimeNs) {
        state.apply_sweep(sweep(keys, t), t, INTERVAL_MS, None);
    }

    fn has(state: &ProcessState, key: ProcessKey, flag: ProcFlags) -> bool {
        state.flags(&key).is_some_and(|f| f.contains(flag))
    }

    #[test]
    fn extras_index_aligned_on_publish() {
        let mut state = ProcessState::new();
        apply(
            &mut state,
            &[key(30, 9), key(10, 5), key(20, 3)],
            1_000 * MS,
        );
        let snap = state.publish(Coverage::default(), (0, 1_000 * MS)).unwrap();
        assert_eq!(snap.records.len(), 3);
        assert_eq!(snap.extras.len(), snap.records.len());
        assert_eq!(snap.extras_p2.len(), snap.records.len());
        for (r, e) in snap.records.iter().zip(snap.extras.iter()) {
            assert_eq!(e.start_boot_ns, r.key.start_ticks);
        }
        assert!(snap.records.windows(2).all(|w| w[0].key < w[1].key));
    }

    #[test]
    fn new_flag_lifecycle() {
        let (a, b) = (key(1, 10), key(2, 20));
        let t0 = 5_000 * MS;
        let mut state = ProcessState::new();
        apply(&mut state, &[a, b], t0);
        assert!(has(&state, a, ProcFlags::NEW));
        assert!(has(&state, b, ProcFlags::NEW));

        let linger = linger_ns(INTERVAL_MS);
        assert!(!state.expire(t0 + linger - MS, INTERVAL_MS));
        assert!(has(&state, a, ProcFlags::NEW));
        assert!(state.expire(t0 + linger + MS, INTERVAL_MS));
        assert!(!has(&state, a, ProcFlags::NEW));
        assert!(!has(&state, b, ProcFlags::NEW));
        assert!(!state.live_keys()[&a].contains(ProcFlags::NEW));
    }

    #[test]
    fn exited_lingers_then_drops() {
        let (a, b) = (key(1, 10), key(2, 20));
        let t0 = 5_000 * MS;
        let t1 = t0 + 1_000 * MS;
        let mut state = ProcessState::new();
        apply(&mut state, &[a, b], t0);
        apply(&mut state, &[a], t1);

        assert!(has(&state, b, ProcFlags::EXITED));
        assert!(!has(&state, b, ProcFlags::NEW));
        assert!(!state.live_keys().contains_key(&b));
        assert!(state.records_sorted().iter().any(|r| r.key == b));
        assert!(state.exited_within_linger().contains(&b));

        let snap = state.publish(Coverage::default(), (t0, t1)).unwrap();
        assert_eq!(snap.extras.len(), 2);
        assert!(snap.get(b).unwrap().flags.contains(ProcFlags::EXITED));

        assert!(state.expire(t1 + linger_ns(INTERVAL_MS) + MS, INTERVAL_MS));
        assert!(state.flags(&b).is_none());
        assert!(!state.records_sorted().iter().any(|r| r.key == b));
        assert!(state.exited_within_linger().is_empty());
        assert!(!state.extras.contains_key(&b));
    }

    #[test]
    fn exited_cap_256() {
        let keys: Vec<ProcessKey> = (0..300)
            .map(|i| key(100 + i, 1_000 + u64::from(i)))
            .collect();
        let t0 = 5_000 * MS;
        let t1 = t0 + 500 * MS;

        // 300 exit in one sweep: the first 44 pushed go.
        let mut state = ProcessState::new();
        apply(&mut state, &keys, t0);
        apply(&mut state, &[], t1);
        assert_eq!(state.exited.len(), MAX_EXITED);
        let kept = state.exited_within_linger();
        assert!(keys[..44].iter().all(|k| !kept.contains(k)));
        assert!(keys[44..].iter().all(|k| kept.contains(k)));
        assert!(keys[..44].iter().all(|k| !state.extras.contains_key(k)));

        // 44 exit first, 256 later: the 44 oldest by exit time go.
        let mut state = ProcessState::new();
        apply(&mut state, &keys, t0);
        apply(&mut state, &keys[44..], t0 + 100 * MS);
        apply(&mut state, &[], t1);
        assert_eq!(state.exited.len(), MAX_EXITED);
        let kept = state.exited_within_linger();
        assert!(keys[..44].iter().all(|k| !kept.contains(k)));
        assert!(keys[44..].iter().all(|k| kept.contains(k)));
    }

    #[test]
    fn linger_clamp() {
        assert_eq!(linger_ns(250), 1_000_000_000);
        assert_eq!(linger_ns(1000), 2_000_000_000);
        assert_eq!(linger_ns(5000), 3_000_000_000);
    }

    #[test]
    fn pid_reuse_is_a_new_key() {
        let old = key(42, 100);
        let new = key(42, 900);
        let t0 = 5_000 * MS;
        let mut state = ProcessState::new();
        apply(&mut state, &[old], t0);
        state.expire(t0 + linger_ns(INTERVAL_MS) + MS, INTERVAL_MS);
        let t1 = t0 + 3_000 * MS;
        apply(&mut state, &[new], t1);

        let records = state.records_sorted();
        assert_eq!(records.len(), 2);
        assert!(has(&state, old, ProcFlags::EXITED));
        assert!(!has(&state, old, ProcFlags::NEW));
        assert!(has(&state, new, ProcFlags::NEW));
        assert!(!has(&state, new, ProcFlags::EXITED));
    }

    #[test]
    fn sweep_flags_copied_new_and_exited_owned() {
        let a = key(1, 10);
        let mut s = sweep(&[a], 1_000 * MS);
        s.records[0].flags = ProcFlags::KERNEL_THREAD | ProcFlags::EXITED | ProcFlags::PROTECTED;
        let mut state = ProcessState::new();
        state.apply_sweep(s, 1_000 * MS, INTERVAL_MS, None);
        assert_eq!(
            state.flags(&a),
            Some(ProcFlags::KERNEL_THREAD | ProcFlags::PROTECTED | ProcFlags::NEW)
        );
    }

    #[test]
    fn identical_sweep_still_publishes() {
        let keys = [key(1, 10), key(2, 20)];
        let mut state = ProcessState::new();
        apply(&mut state, &keys, 1_000 * MS);
        let first = state.publish(Coverage::default(), (0, 1_000 * MS)).unwrap();
        assert!(
            state
                .publish(Coverage::default(), (0, 1_000 * MS))
                .is_none()
        );
        apply(&mut state, &keys, 2_000 * MS);
        let second = state
            .publish(Coverage::default(), (1_000 * MS, 2_000 * MS))
            .unwrap();
        assert!(second.header.version > first.header.version);
        assert_eq!(second.header.observed_end, 2_000 * MS);
    }

    #[test]
    fn publish_copies_apps_p2_and_low_memory() {
        let (a, b) = (key(1, 10), key(2, 20));
        let mut s = sweep(&[a, b], 1_000 * MS);
        s.extras_p2[1] = ProcessExtrasP2 {
            key: Some(b),
            oom_score: Some(7),
            ..ProcessExtrasP2::default()
        };
        let app = |g: u64| AppCgroupSample {
            group: GroupKey(g),
            anon_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            rios: 0,
            wios: 0,
            pids: 1,
            cgroup: CgroupRef {
                rel_path: "app.slice".into(),
                cgroup_id: g,
            },
            validity: FieldValidity::all(Validity::Valid),
        };
        s.app_stats = vec![app(9), app(3), app(5)];
        s.low_memory = Some(Arc::new(LowMemoryInfo {
            groups: Arc::from([]),
            mem_total: 1,
            mem_available: 1,
            psi_full_pct: None,
            critical: false,
        }));
        let mut state = ProcessState::new();
        state.apply_sweep(s, 1_000 * MS, INTERVAL_MS, None);
        let snap = state.publish(Coverage::default(), (0, 1_000 * MS)).unwrap();
        assert_eq!(snap.extras_p2[0], ProcessExtrasP2::default());
        assert_eq!(snap.extras_p2[1].oom_score, Some(7));
        let groups: Vec<u64> = snap.apps.iter().map(|a| a.group.0).collect();
        assert_eq!(groups, [3, 5, 9]);
        assert!(snap.low_memory.is_some());
    }

    #[derive(Default)]
    struct CountingSink {
        sweeps: Vec<(TimeNs, usize)>,
    }

    impl RecordSink for CountingSink {
        fn on_system_sample(&mut self, _t_ns: TimeNs, _series: &[(SeriesId, Sample)]) {}
        fn on_process_sweep(&mut self, t_ns: TimeNs, records: &Arc<[ProcessRecord]>) {
            self.sweeps.push((t_ns, records.len()));
        }
        fn on_device_catalog(&mut self, _catalog: &DeviceCatalog) {}
    }

    #[test]
    fn sink_called_once_per_sweep() {
        let keys = [key(1, 10), key(2, 20), key(3, 30)];
        let mut sink = CountingSink::default();
        let mut state = ProcessState::new();
        state.apply_sweep(
            sweep(&keys, 1_000 * MS),
            1_000 * MS,
            INTERVAL_MS,
            Some(&mut sink),
        );
        state.apply_sweep(
            sweep(&keys[..2], 2_000 * MS),
            2_000 * MS,
            INTERVAL_MS,
            Some(&mut sink),
        );
        state.expire(2_500 * MS, INTERVAL_MS);
        state.apply_sweep(sweep(&keys, 3_000 * MS), 3_000 * MS, INTERVAL_MS, None);
        assert_eq!(sink.sweeps, [(1_000 * MS, 3), (2_000 * MS, 2)]);
    }

    const GIB: u64 = 1 << 30;
    const DEV_A: [u8; 16] = [1; 16];
    const DEV_B: [u8; 16] = [2; 16];

    fn gpu(uuid: [u8; 16], pid: u32, sm: u32, mem: u64) -> GpuProcessSample {
        GpuProcessSample {
            uuid,
            pid,
            sm_pct: Reading::Ok(sm),
            mem_pct: Reading::Ok(0),
            enc_pct: Reading::Ok(0),
            dec_pct: Reading::Unsupported,
            mem_used: Reading::Ok(mem),
        }
    }

    fn caps(supported: bool) -> FxHashMap<[u8; 16], bool> {
        [(DEV_A, supported), (DEV_B, supported)]
            .into_iter()
            .collect()
    }

    fn live_record(state: &ProcessState, key: ProcessKey) -> ProcessRecord {
        state.live[&key].record.clone()
    }

    fn pss(key: ProcessKey, bytes: u64) -> PssEntry {
        PssEntry {
            key,
            pss_bytes: bytes,
            uss_bytes: bytes / 2,
            swap_bytes: 7,
            validity: Validity::Valid,
        }
    }

    #[test]
    fn gpu_pid_mapping_max_and_sum() {
        let (a, b) = (key(100, 10), key(200, 20));
        let mut state = ProcessState::new();
        apply(&mut state, &[a, b], 1_000 * MS);
        let samples = [gpu(DEV_A, 100, 30, GIB), gpu(DEV_B, 100, 60, 2 * GIB)];
        assert_eq!(state.apply_gpu_processes(&samples, &caps(true)), 1);

        let r = live_record(&state, a);
        assert_eq!(r.gpu_util_pct, 60.0);
        assert_eq!(r.gpu_mem_bytes, 3 * GIB);
        // The power estimate needs the value per GPU, not only the maximum.
        assert_eq!(state.gpu_by_dev()[&DEV_A][&a], 30.0);
        assert_eq!(state.gpu_by_dev()[&DEV_B][&a], 60.0);
        assert_eq!(r.validity.get(ProcessField::GpuUtil), Validity::Valid);
        assert_eq!(r.validity.get(ProcessField::GpuMem), Validity::Valid);
        // A live key without a sample shows a valid 0 because a device supports process util.
        let r = live_record(&state, b);
        assert_eq!(r.gpu_util_pct, 0.0);
        assert_eq!(r.validity.get(ProcessField::GpuUtil), Validity::Valid);
        assert!(state.gpu_process());

        // The next sweep sends GPU fields as zero; the reducer keeps the last values.
        apply(&mut state, &[a, b], 2_000 * MS);
        let r = live_record(&state, a);
        assert_eq!(r.gpu_util_pct, 60.0);
        assert_eq!(r.gpu_mem_bytes, 3 * GIB);
        assert_eq!(r.validity.get(ProcessField::GpuUtil), Validity::Valid);
    }

    #[test]
    fn gpu_vram_per_column() {
        const DEV_C: [u8; 16] = [3; 16];
        let (a, b) = (key(100, 10), key(200, 20));
        let mut state = ProcessState::new();
        apply(&mut state, &[a, b], 1_000 * MS);
        // DEV_B is column 0 and DEV_A column 1; DEV_C has no column.
        state.set_gpu_slots(&gpus(&[(DEV_B, "0000:02:00.0"), (DEV_A, "0000:03:00.0")]));
        let samples = [
            gpu(DEV_A, 100, 30, GIB),
            gpu(DEV_C, 100, 10, 4 * GIB),
            gpu(DEV_B, 200, 60, 2 * GIB),
        ];
        state.apply_gpu_processes(&samples, &caps(true));

        assert_eq!(live_record(&state, a).gpu_mem_bytes, 5 * GIB);
        let (va, vb) = published_vram(&mut state, a, b);
        assert_eq!(va.as_slice(), [0, GIB]);
        assert!(!va.spilled());
        assert_eq!(vb.as_slice(), [2 * GIB]);

        // The next sweep keeps the values; a key gone from the GPU sample is cleared.
        apply(&mut state, &[a, b], 2_000 * MS);
        assert_eq!(published_vram(&mut state, a, b).0.as_slice(), [0, GIB]);
        state.apply_gpu_processes(&samples[..2], &caps(true));
        assert!(published_vram(&mut state, a, b).1.is_empty());
    }

    /// GPU identities from (UUID, PCI bus id) pairs.
    fn gpus(list: &[([u8; 16], &str)]) -> Vec<DeviceIdentity> {
        list.iter()
            .map(|&(uuid, pci)| DeviceIdentity::Gpu {
                uuid,
                pci_bus_id: Arc::from(pci),
            })
            .collect()
    }

    fn drm(pdev: &str, pid: u32, vram_bytes: u64) -> DrmProcessVram {
        DrmProcessVram {
            pdev: Arc::from(pdev),
            pid,
            vram_bytes,
        }
    }

    #[test]
    fn drm_vram_by_pci_slot() {
        const AMD: &str = "0000:03:00.0";
        const INTEL: &str = "0000:00:02.0";
        let (a, b) = (key(100, 10), key(200, 20));
        let mut state = ProcessState::new();
        apply(&mut state, &[a, b], 1_000 * MS);
        // Columns: Intel 0, NVIDIA DEV_A 1, AMD 2. "0000:09:00.0" has no column.
        state.set_gpu_slots(&gpus(&[
            ([0; 16], INTEL),
            (DEV_A, "00000000:01:00.0"),
            ([0; 16], AMD),
        ]));
        let rows = [
            drm(AMD, 100, 3 * GIB),
            drm(AMD, 100, GIB),
            drm(INTEL, 100, 2 * GIB),
            drm("0000:09:00.0", 200, GIB),
            drm(AMD, 999, GIB),
        ];
        let amd_only: FxHashMap<[u8; 16], bool> = [([0; 16], false)].into_iter().collect();
        assert_eq!(state.apply_drm_vram(&rows, &amd_only), 2);

        let r = live_record(&state, a);
        assert_eq!(r.gpu_mem_bytes, 6 * GIB);
        // DRM cards have no per-process utilization.
        assert_eq!(r.gpu_util_pct, 0.0);
        assert_eq!(r.validity.get(ProcessField::GpuMem), Validity::Unsupported);
        assert_eq!(live_record(&state, b).gpu_mem_bytes, GIB);
        let (va, vb) = published_vram(&mut state, a, b);
        assert_eq!(va.as_slice(), [2 * GIB, 0, 4 * GIB]);
        assert!(vb.is_empty());
        assert!(state.gpu_process());
        assert!(state.gpu_by_dev().is_empty());

        // An NVIDIA sample keeps the DRM rows, and a DRM sample keeps the NVIDIA rows.
        state.apply_gpu_processes(&[gpu(DEV_A, 100, 30, GIB)], &caps(true));
        let r = live_record(&state, a);
        assert_eq!(r.gpu_mem_bytes, 7 * GIB);
        assert_eq!(r.gpu_util_pct, 30.0);
        assert_eq!(r.validity.get(ProcessField::GpuMem), Validity::Valid);
        assert_eq!(
            published_vram(&mut state, a, b).0.as_slice(),
            [2 * GIB, GIB, 4 * GIB]
        );
        state.apply_drm_vram(&rows[2..3], &caps(true));
        assert_eq!(live_record(&state, a).gpu_mem_bytes, 3 * GIB);
        assert_eq!(
            published_vram(&mut state, a, b).0.as_slice(),
            [2 * GIB, GIB]
        );
        // A DRM-only key takes the default validity.
        state.apply_drm_vram(&rows[3..4], &caps(true));
        let r = live_record(&state, b);
        assert_eq!(r.gpu_mem_bytes, GIB);
        assert_eq!(r.validity.get(ProcessField::GpuMem), Validity::Valid);
    }

    /// `gpu_vram_bytes` of keys `a` and `b` in the next published snapshot.
    fn published_vram(
        state: &mut ProcessState,
        a: ProcessKey,
        b: ProcessKey,
    ) -> (GpuVram, GpuVram) {
        let snap = state.publish(Coverage::default(), (0, MS)).expect("dirty");
        let of = |k: ProcessKey| {
            let i = snap
                .records
                .iter()
                .position(|r| r.key == k)
                .expect("published");
            snap.extras_p2[i].gpu_vram_bytes.clone()
        };
        (of(a), of(b))
    }

    #[test]
    fn net_shared_from_attribution() {
        let (a, b) = (key(100, 10), key(200, 20));
        let mut state = ProcessState::new();
        apply(&mut state, &[a, b], 1_000 * MS);
        let rate = |shared| NetRate {
            rx_bps: 10.0,
            tx_bps: 20.0,
            shared,
            validity: Validity::Valid,
        };
        state.apply_net_rates([(a, rate(true)), (b, rate(false))].into_iter().collect());
        assert!(live_record(&state, a).net_shared);
        assert!(!live_record(&state, b).net_shared);

        // The next sweep sends false; the reducer keeps the last attribution.
        apply(&mut state, &[a, b], 2_000 * MS);
        assert!(live_record(&state, a).net_shared);
        state.apply_net_rates(FxHashMap::default());
        assert!(!live_record(&state, a).net_shared);
    }

    #[test]
    fn gpu_unknown_pid_dropped() {
        let a = key(100, 10);
        let mut state = ProcessState::new();
        apply(&mut state, &[a], 1_000 * MS);
        let before = live_record(&state, a);
        assert_eq!(
            state.apply_gpu_processes(&[gpu(DEV_A, 999, 50, GIB)], &caps(true)),
            0
        );
        let after = live_record(&state, a);
        assert_eq!(after.gpu_util_pct, before.gpu_util_pct);
        assert_eq!(after.gpu_mem_bytes, before.gpu_mem_bytes);
        assert!(!state.gpu.values().any(|g| g.mem_bytes == GIB));
    }

    #[test]
    fn gpu_unsupported_without_capability() {
        let keys = [key(1, 10), key(2, 20)];
        let mut state = ProcessState::new();
        apply(&mut state, &keys, 1_000 * MS);
        assert_eq!(state.apply_gpu_processes(&[], &caps(false)), 0);
        for k in keys {
            let r = live_record(&state, k);
            assert_eq!(r.validity.get(ProcessField::GpuUtil), Validity::Unsupported);
            assert_eq!(r.validity.get(ProcessField::GpuMem), Validity::Unsupported);
        }
        assert!(!state.gpu_process());

        // A sample from an unsupported device is matched but stays Unsupported.
        assert_eq!(
            state.apply_gpu_processes(&[gpu(DEV_A, 1, 40, GIB)], &caps(false)),
            1
        );
        let r = live_record(&state, keys[0]);
        assert_eq!(r.validity.get(ProcessField::GpuUtil), Validity::Unsupported);
    }

    #[test]
    fn pss_coverage_tracks_keys() {
        let (a, b) = (key(1, 10), key(2, 20));
        let mut state = ProcessState::new();
        apply(&mut state, &[a, b], 1_000 * MS);
        state.apply_pss(&[pss(a, 4096), pss(key(9, 90), 1)]);

        let cov = state.coverage();
        assert!(cov.has_pss_uss(&a));
        assert!(!cov.has_pss_uss(&b));
        assert_eq!(cov.pss_uss.len(), 1);

        let r = live_record(&state, a);
        assert_eq!((r.pss_bytes, r.uss_bytes, r.swap_bytes), (4096, 2048, 7));
        assert_eq!(r.validity.get(ProcessField::Pss), Validity::Valid);
        assert_eq!(r.validity.get(ProcessField::Uss), Validity::Valid);
        let r = live_record(&state, b);
        assert_eq!(r.validity.get(ProcessField::Pss), Validity::WarmingUp);

        // PSS survives the next sweep, and the snapshot carries it.
        apply(&mut state, &[a, b], 2_000 * MS);
        assert_eq!(live_record(&state, a).pss_bytes, 4096);
        let snap = state
            .publish(state.coverage(), (1_000 * MS, 2_000 * MS))
            .unwrap();
        assert!(snap.coverage.has_pss_uss(&a));
        assert_eq!(snap.get(a).unwrap().pss_bytes, 4096);
    }

    #[test]
    fn pss_cleared_when_key_leaves_demand() {
        let (a, b) = (key(1, 10), key(2, 20));
        let mut state = ProcessState::new();
        apply(&mut state, &[a, b], 1_000 * MS);
        state.apply_pss(&[pss(a, 4096)]);
        state.clear_pss_outside(&[b]);

        let r = live_record(&state, a);
        assert_eq!(r.validity.get(ProcessField::Pss), Validity::WarmingUp);
        assert_eq!(r.validity.get(ProcessField::Uss), Validity::WarmingUp);
        assert_eq!(r.pss_bytes, 0);
        assert!(state.coverage().pss_uss.is_empty());

        apply(&mut state, &[a, b], 2_000 * MS);
        assert_eq!(
            live_record(&state, a).validity.get(ProcessField::Pss),
            Validity::WarmingUp
        );
    }
}
