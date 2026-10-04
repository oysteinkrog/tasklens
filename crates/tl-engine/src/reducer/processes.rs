//! Process metadata cache: live records, exited records within the linger time, and the
//! `NEW` and `EXITED` flags (ADR-003, ADR-004, PROC-043, R-041).
//!
//! The reducer keeps every live process plus at most `MAX_EXITED` exited ones. A new key gets
//! `NEW` and a key missing from a sweep moves to the exited list with `EXITED`. Both marks last
//! for the linger time, `2 × processes.interval_ms` clamped to 1..=3 s; `expire` on `Tick`
//! clears `NEW` and drops exited records after it.
//!
//! Privacy: never log `ProcessMeta` text from here. Debug logs carry counts only (ADR-015).

use std::collections::VecDeque;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use tl_model::history::RecordSink;
use tl_model::ids::ProcessKey;
use tl_model::process::{
    AppCgroupSample, Coverage, LowMemoryInfo, ProcFlags, ProcessExtras, ProcessExtrasP2,
    ProcessRecord, linger_ms,
};
use tl_model::snapshot::{ComponentHeader, ProcessSnapshot};
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::update::{CgroupStats, ProcessSweep};

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

pub struct ProcessState {
    live: FxHashMap<ProcessKey, Live>,
    /// Flags of every live key, kept equal to `live[key].record.flags`.
    live_flags: FxHashMap<ProcessKey, ProcFlags>,
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
            exited: VecDeque::new(),
            extras: FxHashMap::default(),
            extras_p2: FxHashMap::default(),
            app_stats: Vec::new(),
            unit_stats: Vec::new(),
            low_memory: None,
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
        self.live.reserve(sweep.records.len());
        self.live_flags.reserve(sweep.records.len());

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
            .map(|r| self.extras_p2.get(&r.key).cloned().unwrap_or_default())
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
    use tl_model::validity::FieldValidity;

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
            validity: FieldValidity::all(Validity::Valid),
        }
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
}
