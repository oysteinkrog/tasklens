//! History view types and the RecordSink hook (ADR-012).
//!
//! Command lines are never stored in history and never appear in these types (ADR-015). Rates are
//! in bytes per second, memory in bytes, CPU in percent of the whole machine (0..=100) and GPU in
//! percent. Every point list holds at most `HISTORY_MAX_POINTS` points.

use std::sync::Arc;

use crate::devices::DeviceCatalog;
use crate::ids::{ProcessKey, SeriesId};
use crate::process::ProcessRecord;
use crate::series::Sample;
use crate::snapshot::ComponentHeader;
use crate::time::TimeNs;

/// The hook the reducer holds. `None` in P1 and P2; the P3 recorder and rolling window set it.
pub trait RecordSink: Send + 'static {
    fn on_system_sample(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]);
    fn on_process_sweep(&mut self, t_ns: TimeNs, records: &Arc<[ProcessRecord]>);
    fn on_device_catalog(&mut self, catalog: &DeviceCatalog);
}

/// Metric chips in display order. Keys 1 to 5 select them (UI-PRC-035).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HistoryMetric {
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
}

/// Defined here (not in command.rs) so `HistoryView` can hold it. command.rs uses it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HistoryRange {
    #[default]
    Live,
    At(TimeNs),
    Span { from: TimeNs, to: TimeNs },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HistorySource {
    #[default]
    None,
    RollingWindow,
    Store,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HistoryLoadProblem {
    OpenFailed,
    EmptyOrTruncated,
    VersionMismatch,
    NoInRange,
    Stale,
    TimedOut,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HistoryStoreState {
    #[default]
    Off,
    Loading,
    Ready,
    Problem(HistoryLoadProblem),
}

/// One decimated bucket (ADR-009).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HistoryPoint {
    pub t_ns: TimeNs,
    pub min: f32,
    pub max: f32,
}

#[derive(Clone, Debug)]
pub struct HistorySeries {
    /// `None` is the system total.
    pub key: Option<ProcessKey>,
    pub metric: HistoryMetric,
    pub points: Arc<[HistoryPoint]>,
}

/// Per bucket: the three largest per-process values of the metric and the sum of the rest,
/// drawn stacked in the metric colour (R-169 OI-5, D-020).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HistoryStackBucket {
    pub t_ns: TimeNs,
    pub top: [f32; 3],
    pub other: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HistoryPeaks {
    pub cpu_pct: f32,
    pub mem_anon_bytes: u64,
    pub disk_bps: f32,
    pub net_bps: f32,
    pub gpu_pct: f32,
}

/// One process at the frame time, or over a span. Holds no command line (ADR-015).
#[derive(Clone, Debug)]
pub struct HistoryProcessRow {
    pub key: ProcessKey,
    pub name: Arc<str>,
    pub comm: Arc<str>,
    pub user: Arc<str>,
    pub exe: Option<Arc<str>>,
    pub app_id: Option<Arc<str>>,
    pub system: bool,
    pub present: bool,
    pub previous_boot: bool,
    pub cpu_pct: f32,
    pub mem_anon_bytes: u64,
    pub disk_bps: f32,
    pub net_bps: f32,
    pub gpu_pct: f32,
    /// `Some` only for a `Span` range.
    pub peak: Option<HistoryPeaks>,
}

/// Averages over a `Span` (R-169 OI-5).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HistoryTotals {
    pub cpu_pct: f32,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct HistoryTopRow {
    pub key: ProcessKey,
    pub name: Arc<str>,
    pub app_id: Option<Arc<str>>,
    pub value: f32,
}

#[derive(Clone, Debug)]
pub struct HistoryTop {
    pub query_id: u64,
    pub t_ns: TimeNs,
    pub metric: HistoryMetric,
    pub cpu_total_pct: f32,
    pub mem_total_bytes: u64,
    pub rows: Arc<[HistoryTopRow]>,
}

/// The history component that answers `Command::HistoryQuery`.
#[derive(Clone, Debug, Default)]
pub struct HistoryView {
    pub header: ComponentHeader,
    /// Id of the Frame query this answers.
    pub query_id: u64,
    pub source: HistorySource,
    pub store_state: HistoryStoreState,
    /// Oldest and newest retained frame.
    pub span: Option<(TimeNs, TimeNs)>,
    /// Gaps inside the requested view.
    pub gaps: Arc<[(TimeNs, TimeNs)]>,
    /// The second the rows describe. `None` for a `Span`.
    pub frame_t: Option<TimeNs>,
    pub range: HistoryRange,
    /// Processes alive at `frame_t`.
    pub process_count: u32,
    pub rows: Arc<[HistoryProcessRow]>,
    pub totals: Option<HistoryTotals>,
    /// System total of the metric, then one per requested key.
    pub series: Arc<[HistorySeries]>,
    /// Top 3 plus other, in the same buckets as `series`.
    pub stack: Arc<[HistoryStackBucket]>,
    /// Whole span, system total of the metric. Empty unless the overview was asked for.
    pub overview: Arc<[HistoryPoint]>,
    /// The latest `TopAt` answer.
    pub top: Option<Arc<HistoryTop>>,
    /// CLOCK_REALTIME minus CLOCK_BOOTTIME, for wall-clock labels.
    pub realtime_offset_ns: i64,
}

/// Most points in any history point list (ADR-012).
pub const HISTORY_MAX_POINTS: usize = 1024;

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// Counts calls in shared atomics so the test can read them after the box moves.
    #[derive(Clone, Default)]
    struct CountingSink(Arc<[AtomicU32; 3]>);

    impl RecordSink for CountingSink {
        fn on_system_sample(&mut self, _t_ns: TimeNs, _series: &[(SeriesId, Sample)]) {
            self.0[0].fetch_add(1, Ordering::Relaxed);
        }
        fn on_process_sweep(&mut self, _t_ns: TimeNs, _records: &Arc<[ProcessRecord]>) {
            self.0[1].fetch_add(1, Ordering::Relaxed);
        }
        fn on_device_catalog(&mut self, _catalog: &DeviceCatalog) {
            self.0[2].fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn record_sink_object_safe_send() {
        let counter = CountingSink::default();
        let mut sink: Box<dyn RecordSink> = Box::new(counter.clone());
        let records: Arc<[ProcessRecord]> = Arc::from(Vec::new());
        // Call from another thread to prove the boxed trait object is `Send`.
        std::thread::spawn(move || {
            sink.on_system_sample(1, &[]);
            sink.on_process_sweep(2, &records);
            sink.on_device_catalog(&DeviceCatalog::default());
        })
        .join()
        .unwrap();
        let counts = counter.0.each_ref().map(|c| c.load(Ordering::Relaxed));
        assert_eq!(counts, [1, 1, 1]);
    }

    #[test]
    fn history_view_default() {
        let v = HistoryView::default();
        assert_eq!(v.source, HistorySource::None);
        assert_eq!(v.store_state, HistoryStoreState::Off);
        assert_eq!(v.range, HistoryRange::Live);
        assert!(v.span.is_none());
        assert!(v.frame_t.is_none());
        assert!(v.totals.is_none());
        assert!(v.top.is_none());
        assert!(v.gaps.is_empty());
        assert!(v.rows.is_empty());
        assert!(v.series.is_empty());
        assert!(v.stack.is_empty());
        assert!(v.overview.is_empty());
        assert_eq!(v.query_id, 0);
        assert_eq!(v.process_count, 0);
        assert_eq!(v.realtime_offset_ns, 0);
    }

    #[test]
    fn history_max_points() {
        assert_eq!(HISTORY_MAX_POINTS, 1024);
    }
}
