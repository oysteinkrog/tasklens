//! Per-process, name and app series for `Demand.graph_targets` (E09-029, R-154).
//!
//! The UI lists up to `GRAPH_TARGETS_MAX` targets. Each process sweep appends one sample per
//! target: the value the table cell shows for that column, summed over the matching records for
//! `Name` and `App`. No match, or a matching value that is not `Valid`, appends a gap. Each entry
//! keeps the 60 s fast ring and the 1 Hz tier (per-second maximum, 602 samples). The published
//! list holds the fast views sorted by id, then the slow views in the same order.
//!
//! Privacy: process names are never logged from here (ADR-015).

use std::sync::Arc;

use rustc_hash::FxHashMap;
use tl_model::demand::GRAPH_TARGETS_MAX;
use tl_model::ids::{ColumnId, DeviceIdx, GraphTarget, MetricId, ProcessSeriesId, SeriesId};
use tl_model::process::{GroupKind, ProcessExtras, ProcessRecord};
use tl_model::series::{Sample, SeriesView, TieredRing};
use tl_model::snapshot::{ComponentHeader, ProcessSeriesSet};
use tl_model::time::TimeNs;
use tl_model::validity::{ProcessField, Validity};

use super::table::{SortKey, sort_key};

/// Columns that may be graph sources (R-154 item 2); other columns are ignored.
pub(crate) const GRAPH_COLUMNS: [ColumnId; 6] = [
    ColumnId::Cpu,
    ColumnId::Memory,
    ColumnId::Disk,
    ColumnId::Network,
    ColumnId::Gpu,
    ColumnId::GpuMemDedicated,
];

/// The `SeriesView.id` of every process series view. Process series are keyed by
/// `ProcessSeriesId`, not by a device metric, so the view id carries no meaning and no tier bit.
/// The slow view is told apart by its 1 s `period_ns`.
const VIEW_ID: SeriesId = SeriesId {
    device: DeviceIdx(0),
    metric: MetricId(0),
};

/// The 1 Hz tier period.
const SLOW_PERIOD_NS: u64 = 1_000_000_000;

/// The sweep period a ring is sized for before the first sweep reports one.
const DEFAULT_PERIOD_NS: u64 = 1_000_000_000;

struct Entry {
    ring: TieredRing,
    period_ns: u64,
    /// Cached until the fast ring's version changes.
    fast_view: Option<Arc<SeriesView>>,
    /// Cached until the slow ring's version changes.
    slow_view: Option<Arc<SeriesView>>,
}

impl Entry {
    fn new(period_ns: u64) -> Entry {
        Entry {
            ring: TieredRing::new(period_ns),
            period_ns,
            fast_view: None,
            slow_view: None,
        }
    }

    /// The fast view, rebuilt only when the fast ring changed.
    fn fast(&mut self) -> Arc<SeriesView> {
        let version = self.ring.fast.version();
        match &self.fast_view {
            Some(v) if v.version == version => Arc::clone(v),
            _ => {
                let v = Arc::new(self.ring.fast.to_view(VIEW_ID, self.period_ns));
                self.fast_view = Some(Arc::clone(&v));
                v
            }
        }
    }

    /// The slow view, rebuilt only when the slow ring changed.
    fn slow(&mut self) -> Arc<SeriesView> {
        let version = self.ring.slow.version();
        match &self.slow_view {
            Some(v) if v.version == version => Arc::clone(v),
            _ => {
                let v = Arc::new(self.ring.slow.to_view(VIEW_ID, SLOW_PERIOD_NS));
                self.slow_view = Some(Arc::clone(&v));
                v
            }
        }
    }
}

/// The reducer side of `Snapshot.process_series`.
pub(crate) struct ProcessSeries {
    entries: FxHashMap<ProcessSeriesId, Entry>,
    /// The period of the last sweep; new entries size their fast ring for it.
    period_ns: u64,
    /// Something changed since the last `publish`.
    changed: bool,
}

impl Default for ProcessSeries {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessSeries {
    pub(crate) fn new() -> Self {
        ProcessSeries {
            entries: FxHashMap::default(),
            period_ns: DEFAULT_PERIOD_NS,
            changed: false,
        }
    }

    /// Keeps entries for the ids in `targets` (after the GRAPH_COLUMNS filter), drops the rest at once.
    /// Only the first `GRAPH_TARGETS_MAX` targets count, as `Demand::clamp_lists` keeps them.
    pub(crate) fn set_targets(&mut self, targets: &[(GraphTarget, ColumnId)]) {
        let wanted: Vec<ProcessSeriesId> = targets
            .iter()
            .take(GRAPH_TARGETS_MAX)
            .filter(|(_, column)| GRAPH_COLUMNS.contains(column))
            .map(|(target, column)| ProcessSeriesId {
                target: target.clone(),
                column: *column,
            })
            .collect();
        let before = self.entries.len();
        self.entries.retain(|id, _| wanted.contains(id));
        if self.entries.len() != before {
            self.changed = true;
        }
        for id in wanted {
            if !self.entries.contains_key(&id) {
                self.entries.insert(id, Entry::new(self.period_ns));
                self.changed = true;
            }
        }
    }

    /// One sample per entry from one process sweep. `period_ns` is the sweep period. A change
    /// resizes an empty ring; a ring with samples keeps them and puts a gap before the next one.
    pub(crate) fn on_sweep(
        &mut self,
        t_ns: TimeNs,
        period_ns: u64,
        records: &[ProcessRecord],
        extras: &[ProcessExtras],
    ) {
        if period_ns == 0 {
            return;
        }
        self.period_ns = period_ns;
        if self.entries.is_empty() {
            return;
        }
        for (id, entry) in &mut self.entries {
            if entry.period_ns != period_ns {
                if entry.ring.fast.is_empty() {
                    *entry = Entry::new(period_ns);
                } else {
                    entry.period_ns = period_ns;
                    entry.ring.mark_gap();
                }
            }
            let sample = match target_value(&id.target, id.column, records, extras) {
                Some(value) => Sample::valid(t_ns, value),
                None => Sample::gap(t_ns),
            };
            entry.ring.push(sample);
        }
        self.changed = true;
    }

    /// The next sample of every series gets `GAP_BEFORE` (suspend, resume, reset).
    pub(crate) fn mark_gap_all(&mut self) {
        for entry in self.entries.values_mut() {
            entry.ring.mark_gap();
        }
    }

    /// Arc-reusing publish; `None` when nothing changed since the last call.
    pub(crate) fn publish(&mut self, header: ComponentHeader) -> Option<Arc<ProcessSeriesSet>> {
        if !std::mem::take(&mut self.changed) {
            return None;
        }
        let mut ids: Vec<&ProcessSeriesId> = self.entries.keys().collect();
        ids.sort_unstable();
        let ids: Vec<ProcessSeriesId> = ids.into_iter().cloned().collect();
        let mut series = Vec::with_capacity(ids.len() * 2);
        for id in &ids {
            if let Some(entry) = self.entries.get_mut(id) {
                series.push((id.clone(), entry.fast()));
            }
        }
        for id in &ids {
            if let Some(entry) = self.entries.get_mut(id) {
                series.push((id.clone(), entry.slow()));
            }
        }
        Some(Arc::new(ProcessSeriesSet {
            header,
            series: Arc::from(series),
        }))
    }
}

/// The record fields behind a graph column. The value counts only when all are `Valid`.
fn column_fields(column: ColumnId) -> &'static [ProcessField] {
    match column {
        ColumnId::Cpu => &[ProcessField::Cpu],
        ColumnId::Memory => &[ProcessField::RssAnon],
        ColumnId::Disk => &[ProcessField::IoRead, ProcessField::IoWrite],
        ColumnId::Network => &[ProcessField::NetRx, ProcessField::NetTx],
        ColumnId::Gpu => &[ProcessField::GpuUtil],
        ColumnId::GpuMemDedicated => &[ProcessField::GpuMem],
        _ => &[],
    }
}

/// The table cell value of record `i` in `column`, through the table's column getter. `None`
/// when a field is not `Valid` or the value is not a number.
fn cell_value(
    records: &[ProcessRecord],
    extras: &[ProcessExtras],
    i: usize,
    column: ColumnId,
) -> Option<f32> {
    let r = &records[i];
    let x = extras.get(i)?;
    if column_fields(column)
        .iter()
        .any(|&f| r.validity.get(f) != Validity::Valid)
    {
        return None;
    }
    match sort_key(r, x, column) {
        SortKey::Float(v) => Some(v),
        // Byte counts; f32 keeps them to 24 bits of precision, enough for a graph.
        SortKey::Int(v) => Some(v as f32),
        SortKey::Text(_) | SortKey::Absent => None,
    }
}

/// The sample value of one target: the matching record's value, or the sum over the matching
/// records. `None` (a gap) when nothing matches or any matching value is missing.
fn target_value(
    target: &GraphTarget,
    column: ColumnId,
    records: &[ProcessRecord],
    extras: &[ProcessExtras],
) -> Option<f32> {
    match target {
        GraphTarget::Process(key) => {
            let i = records.binary_search_by(|r| r.key.cmp(key)).ok()?;
            cell_value(records, extras, i, column)
        }
        GraphTarget::Name(name) => {
            sum_matching(records, extras, column, |r, _| *r.meta.name == **name)
        }
        GraphTarget::App(app_id) => {
            let group = GroupKind::App {
                app_id: Arc::clone(app_id),
            }
            .key();
            sum_matching(records, extras, column, |r, x| {
                r.group == group
                    || x.and_then(|x| x.unit.as_ref())
                        .is_some_and(|u| *u.name == **app_id)
            })
        }
    }
}

/// One pass over `records`: the sum of `column` over the records `matches` accepts.
fn sum_matching(
    records: &[ProcessRecord],
    extras: &[ProcessExtras],
    column: ColumnId,
    matches: impl Fn(&ProcessRecord, Option<&ProcessExtras>) -> bool,
) -> Option<f32> {
    let mut sum: Option<f32> = None;
    for (i, r) in records.iter().enumerate() {
        if !matches(r, extras.get(i)) {
            continue;
        }
        let v = cell_value(records, extras, i, column)?;
        sum = Some(sum.unwrap_or(0.0) + v);
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    use tl_model::actions::SchedPolicy;
    use tl_model::demand::Demand;
    use tl_model::ids::{GroupKey, ProcessKey};
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta};
    use tl_model::system::CpuSet;
    use tl_model::table::{Manager, UnitRef};
    use tl_model::time::BootId;
    use tl_model::validity::FieldValidity;

    const PERIOD: u64 = 1_000_000_000;
    const SEC: u64 = 1_000_000_000;

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(1),
            pid,
        }
    }

    fn rec(pid: u32, name: &str, cpu: f32, memory: u64) -> ProcessRecord {
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: name.into(),
                name: name.into(),
                exe: None,
                cmdline: None,
                user: "user".into(),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                search_lower: name.to_lowercase().into(),
            }),
            group: GroupKey(0),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: cpu,
            rss_anon_bytes: memory,
            rss_bytes: memory,
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

    fn extras(unit: Option<&str>) -> ProcessExtras {
        ProcessExtras {
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            start_boot_ns: 0,
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
            unit: unit.map(|name| UnitRef {
                manager: Manager::User,
                name: name.into(),
            }),
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
        }
    }

    fn sweep(ps: &mut ProcessSeries, t: u64, records: &[ProcessRecord]) {
        let x: Vec<ProcessExtras> = records.iter().map(|_| extras(None)).collect();
        ps.on_sweep(t, PERIOD, records, &x);
    }

    fn fast_samples(set: &ProcessSeriesSet, id: &ProcessSeriesId) -> Vec<Sample> {
        let (_, view) = set
            .series
            .iter()
            .find(|(k, v)| k == id && v.period_ns == PERIOD)
            .expect("fast view");
        view.samples.to_vec()
    }

    fn id(target: GraphTarget, column: ColumnId) -> ProcessSeriesId {
        ProcessSeriesId { target, column }
    }

    #[test]
    fn process_target_values() {
        let mut ps = ProcessSeries::new();
        let target = (GraphTarget::Process(key(7)), ColumnId::Cpu);
        ps.set_targets(std::slice::from_ref(&target));
        for (i, cpu) in [10.0, 20.0, 30.0].into_iter().enumerate() {
            sweep(&mut ps, (i as u64 + 1) * SEC, &[rec(7, "a", cpu, 0)]);
        }
        let set = ps.publish(ComponentHeader::default()).unwrap();
        let values: Vec<f32> = fast_samples(&set, &id(target.0, target.1))
            .iter()
            .map(|s| s.value)
            .collect();
        assert_eq!(values, [10.0, 20.0, 30.0]);
        // The fast view, then the slow view of the same id.
        assert_eq!(set.series.len(), 2);
        assert_eq!(set.series[1].1.period_ns, SLOW_PERIOD_NS);
    }

    #[test]
    fn name_target_sums() {
        let mut ps = ProcessSeries::new();
        let target = (GraphTarget::Name(Arc::from("firefox")), ColumnId::Memory);
        ps.set_targets(std::slice::from_ref(&target));
        let records = [
            rec(1, "firefox", 0.0, 100_000_000),
            rec(2, "firefox", 0.0, 50_000_000),
            rec(3, "firefox-bin", 0.0, 7),
        ];
        sweep(&mut ps, SEC, &records);
        let set = ps.publish(ComponentHeader::default()).unwrap();
        let samples = fast_samples(&set, &id(target.0, target.1));
        assert_eq!(samples.len(), 1);
        assert!(samples[0].is_valid());
        assert_eq!(samples[0].value, 150_000_000.0);
    }

    #[test]
    fn app_target_gap_when_absent() {
        let mut ps = ProcessSeries::new();
        let target = (
            GraphTarget::App(Arc::from("org.kde.konsole")),
            ColumnId::Cpu,
        );
        ps.set_targets(std::slice::from_ref(&target));
        sweep(&mut ps, SEC, &[rec(1, "bash", 5.0, 0)]);
        let set = ps.publish(ComponentHeader::default()).unwrap();
        let samples = fast_samples(&set, &id(target.0.clone(), target.1));
        assert_eq!(samples.len(), 1);
        assert!(!samples[0].is_valid());

        // A record in the app group and a record in the unit of that name both count.
        let mut in_group = rec(2, "konsole", 4.0, 0);
        in_group.group = GroupKind::App {
            app_id: Arc::from("org.kde.konsole"),
        }
        .key();
        let records = [rec(1, "bash", 5.0, 0), in_group, rec(3, "helper", 1.5, 0)];
        let x = [extras(None), extras(None), extras(Some("org.kde.konsole"))];
        ps.on_sweep(2 * SEC, PERIOD, &records, &x);
        let set = ps.publish(ComponentHeader::default()).unwrap();
        let samples = fast_samples(&set, &id(target.0, target.1));
        assert_eq!(samples[1].value, 5.5);

        // A matching value that is not `Valid` is a gap, never a zero.
        let mut stale = rec(7, "a", 3.0, 0);
        stale.validity.set(ProcessField::Cpu, Validity::Stale);
        let mut ps = ProcessSeries::new();
        let target = (GraphTarget::Process(key(7)), ColumnId::Cpu);
        ps.set_targets(std::slice::from_ref(&target));
        sweep(&mut ps, SEC, &[stale]);
        let set = ps.publish(ComponentHeader::default()).unwrap();
        assert!(!fast_samples(&set, &id(target.0, target.1))[0].is_valid());
    }

    #[test]
    fn non_graph_column_ignored() {
        let mut ps = ProcessSeries::new();
        ps.set_targets(&[(GraphTarget::Process(key(1)), ColumnId::Pid)]);
        assert!(ps.entries.is_empty());
    }

    #[test]
    fn removed_target_dropped() {
        let mut ps = ProcessSeries::new();
        let a = (GraphTarget::Process(key(1)), ColumnId::Cpu);
        let b = (GraphTarget::Name(Arc::from("b")), ColumnId::Memory);
        ps.set_targets(&[a.clone(), b.clone()]);
        sweep(&mut ps, SEC, &[rec(1, "b", 1.0, 2)]);
        assert_eq!(
            ps.publish(ComponentHeader::default()).unwrap().series.len(),
            4
        );
        ps.set_targets(std::slice::from_ref(&a));
        let set = ps.publish(ComponentHeader::default()).unwrap();
        assert_eq!(set.series.len(), 2);
        assert!(set.series.iter().all(|(k, _)| *k == id(a.0.clone(), a.1)));
    }

    #[test]
    fn publish_arc_reuse_and_none() {
        let mut ps = ProcessSeries::new();
        let a = (GraphTarget::Process(key(1)), ColumnId::Cpu);
        let b = (GraphTarget::Process(key(2)), ColumnId::Cpu);
        ps.set_targets(&[a.clone(), b.clone()]);
        sweep(&mut ps, SEC, &[rec(1, "a", 1.0, 0), rec(2, "b", 2.0, 0)]);
        let first = ps.publish(ComponentHeader::default()).unwrap();
        assert!(ps.publish(ComponentHeader::default()).is_none());

        // A sweep in the same second: the fast views change, the slow views stay.
        sweep(
            &mut ps,
            SEC + PERIOD / 2,
            &[rec(1, "a", 3.0, 0), rec(2, "b", 4.0, 0)],
        );
        let second = ps.publish(ComponentHeader::default()).unwrap();
        assert_eq!(first.series.len(), second.series.len());
        for ((ka, va), (kb, vb)) in first.series.iter().zip(second.series.iter()) {
            assert_eq!(ka, kb);
            assert_eq!(Arc::ptr_eq(va, vb), va.period_ns == SLOW_PERIOD_NS);
        }

        // Removing one target leaves the other one's views pointer-equal.
        ps.set_targets(std::slice::from_ref(&a));
        let third = ps.publish(ComponentHeader::default()).unwrap();
        let a_id = id(a.0, a.1);
        let kept: Vec<_> = second.series.iter().filter(|(k, _)| *k == a_id).collect();
        assert_eq!(third.series.len(), kept.len());
        for ((_, prev), (_, next)) in kept.iter().zip(third.series.iter()) {
            assert!(Arc::ptr_eq(prev, next));
        }
    }

    #[test]
    fn targets_clamped_16() {
        let targets: Vec<(GraphTarget, ColumnId)> = (0..17)
            .map(|pid| (GraphTarget::Process(key(pid)), ColumnId::Cpu))
            .collect();
        let mut demand = Demand {
            graph_targets: Arc::from(targets.clone()),
            ..Demand::default()
        };
        demand.clamp_lists();
        let mut ps = ProcessSeries::new();
        ps.set_targets(&demand.graph_targets);
        assert_eq!(ps.entries.len(), GRAPH_TARGETS_MAX);
        assert!(
            !ps.entries
                .contains_key(&id(targets[16].0.clone(), ColumnId::Cpu))
        );

        // An unclamped list is cut the same way.
        let mut ps = ProcessSeries::new();
        ps.set_targets(&targets);
        assert_eq!(ps.entries.len(), GRAPH_TARGETS_MAX);
        assert!(
            !ps.entries
                .contains_key(&id(targets[16].0.clone(), ColumnId::Cpu))
        );
    }
}
