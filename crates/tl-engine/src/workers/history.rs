//! The `HistoryRecorder` worker (ADR-012, ADR-004) and its reducer side.
//!
//! Disk and compression work must not run on the reducer. `HistorySink` is the reducer's
//! `RecordSink`: each callback builds one `HistoryMsg` and pushes it into the worker's channel
//! with `try_send`, so it never blocks. With no sender, a full queue or a closed channel the
//! message is dropped and counted; the sink passes the count on with `HistoryMsg::Dropped`, and
//! the worker reports it in its health message.
//!
//! Each worker generation creates its own channel and stores the sender in the `HistorySlot`, so
//! no `mpsc::Receiver` moves into a restartable worker (ADR-004 factory contract).
//!
//! The worker switches between three modes each turn (ADR-012, D-010, PROC-086): recording to
//! the store when `history.enabled` is true (E30-014), the 10-minute `RollingWindow` when
//! recording is off and the Processes tab is open (`Demand.process_table`), and nothing
//! otherwise. Hiding the window stops the sweep but keeps `process_table`, so the rolling window
//! survives a minimize with a gap in its data.
//!
//! Never hold a `watch` `Ref` across another channel call. This module only uses
//! `borrow_and_clone()` (bead `c11v6v`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwapOption;
use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::time;
use asupersync::{Cx, Outcome};
use rustix::time::{ClockId, Timespec, clock_gettime};
use tl_collect::SysRoot;
use tl_collect::parse::meminfo::{MemInfo, parse_meminfo};
use tl_history::rolling::RollingWindow;
use tl_model::command::HistoryWhat;
use tl_model::demand::Demand;
use tl_model::devices::DeviceCatalog;
use tl_model::health::{SourceId, SourceState};
use tl_model::history::{HistoryRange, HistorySource, HistoryStoreState, HistoryView, RecordSink};
use tl_model::ids::{ProcessKey, SeriesId};
use tl_model::process::ProcessRecord;
use tl_model::series::Sample;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::ComponentHeader;
use tl_model::time::{BootId, TimeNs, now_boot_ns};

use crate::source::WorkerIo;
use crate::update::{EngineMsg, SourceUpdate, WorkerError};

/// Capacity of the channel each worker generation creates.
pub(crate) const HISTORY_CHANNEL_CAPACITY: usize = 512;

/// One event for the `HistoryRecorder` worker.
pub(crate) enum HistoryMsg {
    SessionStart {
        epoch: u32,
        t_ns: TimeNs,
    },
    System {
        t_ns: TimeNs,
        series: Box<[(SeriesId, Sample)]>,
    },
    Sweep {
        t_ns: TimeNs,
        records: Arc<[ProcessRecord]>,
    },
    /// `DeviceCatalog` holds its entries in an `Arc`, so the clone is cheap.
    Catalog(DeviceCatalog),
    Query {
        id: u64,
        range: HistoryRange,
        what: HistoryWhat,
    },
    Delete {
        keys: Arc<[ProcessKey]>,
    },
    /// The sink's running count of dropped messages, sent after the next message that got
    /// through.
    Dropped {
        total: u64,
    },
}

/// Each worker generation creates a fresh `mpsc::channel(512)` and stores its `Sender` here; the
/// sink reads it.
pub(crate) type HistorySlot = Arc<ArcSwapOption<mpsc::Sender<HistoryMsg>>>;

/// The reducer's `RecordSink` while history is on. A thin forwarder to the worker's channel.
pub(crate) struct HistorySink {
    slot: HistorySlot,
    dropped: u64,
    /// The last `dropped` value the worker received.
    reported: u64,
}

// The session wiring (E30-A2) creates the sink and routes the history commands to it.
#[allow(dead_code)]
impl HistorySink {
    pub(crate) fn new(slot: HistorySlot) -> Self {
        HistorySink {
            slot,
            dropped: 0,
            reported: 0,
        }
    }

    /// For `Command::HistoryQuery` and `Command::HistoryDelete` routed by the reducer (E30-A2).
    pub(crate) fn command(&mut self, msg: HistoryMsg) {
        self.send(msg);
    }

    /// Messages dropped so far: no sender, queue full or channel closed.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    fn send(&mut self, msg: HistoryMsg) {
        let guard = self.slot.load();
        let Some(tx) = guard.as_deref() else {
            self.dropped += 1;
            return;
        };
        if tx.try_send(msg).is_err() {
            self.dropped += 1;
            return;
        }
        if self.dropped > self.reported
            && tx
                .try_send(HistoryMsg::Dropped {
                    total: self.dropped,
                })
                .is_ok()
        {
            self.reported = self.dropped;
        }
    }
}

impl RecordSink for HistorySink {
    fn on_system_sample(&mut self, t_ns: TimeNs, series: &[(SeriesId, Sample)]) {
        self.send(HistoryMsg::System {
            t_ns,
            series: series.into(),
        });
    }

    fn on_process_sweep(&mut self, t_ns: TimeNs, records: &Arc<[ProcessRecord]>) {
        self.send(HistoryMsg::Sweep {
            t_ns,
            records: Arc::clone(records),
        });
    }

    fn on_device_catalog(&mut self, catalog: &DeviceCatalog) {
        self.send(HistoryMsg::Catalog(catalog.clone()));
    }
}

/// How long one `recv` waits before the loop re-reads demand and settings.
const RECV_WAIT: Duration = Duration::from_millis(500);
/// At most this many messages go into one blocking job: one `recv` plus 63 `try_recv`.
const BATCH_MAX: usize = 64;
/// MemTotal and the clock offset are read this often, and drops are reported at most this often.
const MINUTE_NS: u64 = 60_000_000_000;

pub(crate) struct HistoryWorkerConfig {
    pub boot: BootId,
    /// `EngineConfig.history_dir` (R-150 item 3). The store mode (E30-014) reads it.
    #[allow(dead_code)]
    pub history_dir: Option<PathBuf>,
}

/// What the worker holds. E30-014 adds `Recording(RecordingState)` (R-262).
pub(crate) enum Mode {
    Off,
    Rolling(RollingWindow),
}

/// The mode that demand and settings ask for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wanted {
    Off,
    Rolling,
    Recording,
}

fn wanted(demand: &Demand, settings: &EngineSettings) -> Wanted {
    if settings.history.enabled {
        Wanted::Recording
    } else if demand.process_table {
        Wanted::Rolling
    } else {
        Wanted::Off
    }
}

/// The state one blocking job takes by value and returns.
struct State {
    mode: Mode,
    /// CLOCK_REALTIME minus CLOCK_BOOTTIME, read once per minute.
    realtime_offset_ns: i64,
    /// `header.version` of the last answer.
    version: u64,
    /// The highest drop total the sink sent.
    dropped: u64,
}

impl State {
    /// Off to Rolling creates the window. Rolling to Off drops it, which frees its RAM at once.
    /// Returns true when a window was created.
    fn switch(&mut self, want: Wanted, boot: BootId) -> bool {
        // E30-014: Rolling to Recording drops the window and starts the store; Recording to
        // anything flushes and closes the store. Until then recording on keeps no history.
        let rolling = want == Wanted::Rolling;
        match (&self.mode, rolling) {
            (Mode::Off, true) => {
                self.mode = Mode::Rolling(RollingWindow::new(boot));
                true
            }
            (Mode::Rolling(_), false) => {
                self.mode = Mode::Off;
                false
            }
            _ => false,
        }
    }

    /// Reads MemTotal and the clock offset. Runs on the blocking pool.
    fn refresh(&mut self) {
        self.realtime_offset_ns = realtime_offset_ns();
        if let Mode::Rolling(window) = &mut self.mode
            && let Some(kib) = read_mem_total_kib()
        {
            window.set_mem_total_kib(kib);
        }
    }

    /// Applies a batch and pushes the answers to `out`. Data that arrives while the mode is Off
    /// is dropped. Runs on the blocking pool.
    fn apply(&mut self, msgs: &mut Vec<HistoryMsg>, out: &mut Vec<SourceUpdate>) {
        for msg in msgs.drain(..) {
            match msg {
                HistoryMsg::Dropped { total } => self.dropped = self.dropped.max(total),
                HistoryMsg::Query { id, range, what } => {
                    let view = self.answer(id, range, &what);
                    out.push(SourceUpdate::History(Arc::new(view)));
                }
                msg => {
                    if let Mode::Rolling(window) = &mut self.mode {
                        record(window, msg);
                    }
                }
            }
        }
    }

    /// Answers a query with the source and span of the active history. E30-015 replaces this
    /// with `history_query::answer_frame` and `answer_top`, which fill rows, series and top.
    fn answer(&mut self, id: u64, range: HistoryRange, _what: &HistoryWhat) -> HistoryView {
        self.version += 1;
        let (source, span) = match &self.mode {
            Mode::Off => (HistorySource::None, None),
            Mode::Rolling(window) => (HistorySource::RollingWindow, window.span()),
        };
        let (observed_start, observed_end) = span.unwrap_or_default();
        HistoryView {
            header: ComponentHeader {
                version: self.version,
                observed_start,
                observed_end,
                ..ComponentHeader::default()
            },
            query_id: id,
            source,
            store_state: HistoryStoreState::Off,
            span,
            range,
            realtime_offset_ns: self.realtime_offset_ns,
            ..HistoryView::default()
        }
    }
}

fn record(window: &mut RollingWindow, msg: HistoryMsg) {
    match msg {
        HistoryMsg::System { t_ns, series } => window.on_system_sample(t_ns, &series),
        HistoryMsg::Sweep { t_ns, records } => window.on_process_sweep(t_ns, &records),
        HistoryMsg::Catalog(catalog) => window.on_device_catalog(&catalog),
        HistoryMsg::SessionStart { t_ns, .. } => window.mark_gap(t_ns),
        // E30-015 re-answers the last Frame query after a delete.
        HistoryMsg::Delete { keys } => window.delete_keys(&keys),
        HistoryMsg::Query { .. } | HistoryMsg::Dropped { .. } => {}
    }
}

/// MemTotal in KiB from `/proc/meminfo`, `None` when the file cannot be read or parsed.
fn read_mem_total_kib() -> Option<u64> {
    let buf = std::fs::read(SysRoot::host().proc_path("meminfo")).ok()?;
    let mut info = MemInfo::default();
    parse_meminfo(&buf, &mut info).ok()?;
    Some(info.mem_total / 1024)
}

/// CLOCK_REALTIME minus CLOCK_BOOTTIME, in ns.
fn realtime_offset_ns() -> i64 {
    let ns = |ts: Timespec| i128::from(ts.tv_sec) * 1_000_000_000 + i128::from(ts.tv_nsec);
    let real = ns(clock_gettime(ClockId::Realtime));
    let boot = ns(clock_gettime(ClockId::Boottime));
    i64::try_from(real - boot).unwrap_or(0)
}

/// Clears the slot when the generation ends on any path, a panic included. It only clears the
/// slot while it still holds this generation's sender.
struct SlotGuard {
    slot: HistorySlot,
    mine: Option<Arc<mpsc::Sender<HistoryMsg>>>,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.slot.compare_and_swap(&self.mine, None);
    }
}

/// One generation of the `HistoryRecorder` worker. Returns `Outcome::Ok(())` when `cx` is
/// cancelled and `Outcome::Err` when a job cannot be spawned or the reducer's queue is closed,
/// so the supervisor restarts it.
// The session wiring (E30-A2) starts this worker under its own supervisor.
#[allow(dead_code)]
pub(crate) async fn run_history(
    cx: Cx,
    io: WorkerIo,
    slot: HistorySlot,
    cfg: HistoryWorkerConfig,
) -> Outcome<(), WorkerError> {
    let (tx, mut rx) = mpsc::channel::<HistoryMsg>(HISTORY_CHANNEL_CAPACITY);
    let mine = Some(Arc::new(tx));
    slot.store(mine.clone());
    let _guard = SlotGuard { slot, mine };

    let mut state = State {
        mode: Mode::Off,
        realtime_offset_ns: 0,
        version: 0,
        dropped: 0,
    };
    let mut msgs: Vec<HistoryMsg> = Vec::with_capacity(BATCH_MAX);
    // Runtime time in ns of the next MemTotal and clock offset read.
    let mut next_refresh = 0u64;
    let mut reported = 0u64;
    let mut last_report: Option<u64> = None;

    loop {
        if let Ok(Ok(msg)) = time::timeout(cx.now(), RECV_WAIT, rx.recv(&cx)).await {
            msgs.push(msg);
            while msgs.len() < BATCH_MAX {
                match rx.try_recv() {
                    Ok(msg) => msgs.push(msg),
                    Err(_) => break,
                }
            }
        }
        if cx.checkpoint().is_err() {
            // E30-014 flushes the store here. The rolling window is dropped with the state.
            return Outcome::Ok(());
        }

        let want = wanted(
            &io.demand.borrow_and_clone(),
            &io.settings.borrow_and_clone(),
        );
        let now = cx.now().as_nanos();
        if state.switch(want, cfg.boot) {
            next_refresh = now;
        }
        let refresh = !matches!(state.mode, Mode::Off) && now >= next_refresh;
        if refresh {
            next_refresh = now.saturating_add(MINUTE_NS);
        }

        if !msgs.is_empty() || refresh {
            let job = cx.spawn_blocking(move |_| {
                let mut out = Vec::new();
                if refresh {
                    state.refresh();
                }
                state.apply(&mut msgs, &mut out);
                (state, msgs, out)
            });
            let mut job = match job {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let out;
            (state, msgs, out) = match job.join(&cx).await {
                Ok(done) => done,
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
            for update in out {
                if let Some(stop) = send(&cx, &io, update).await {
                    return stop;
                }
            }
        }

        // R-249: the state stays `Running`; the text says how many events were dropped.
        let now = cx.now().as_nanos();
        let due = last_report.is_none_or(|t| now.saturating_sub(t) >= MINUTE_NS);
        if state.dropped > reported && due {
            reported = state.dropped;
            last_report = Some(now);
            let health = SourceUpdate::Health {
                id: SourceId::HistoryRecorder,
                state: SourceState::Running,
                last_error: Some(format!("History dropped {reported} events").into()),
                t_ns: now_boot_ns(),
            };
            if let Some(stop) = send(&cx, &io, health).await {
                return stop;
            }
        }
    }
}

/// Sends one update. Returns the loop's result when the loop must stop: `Ok` on cancel,
/// `Err(Closed)` when the reducer is gone.
async fn send(cx: &Cx, io: &WorkerIo, update: SourceUpdate) -> Option<Outcome<(), WorkerError>> {
    match io.updates.send(cx, EngineMsg::Source(update)).await {
        Ok(()) => None,
        Err(mpsc::SendError::Cancelled(_)) => Some(Outcome::Ok(())),
        Err(_) => Some(Outcome::Err(WorkerError::Closed)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tl_model::history::HistoryMetric;
    use tl_model::ids::GroupKey;
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(1),
            pid,
        }
    }

    fn record(pid: u32, meta: &Arc<ProcessMeta>) -> ProcessRecord {
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: meta.clone(),
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

    fn sweep(n: u32) -> Arc<[ProcessRecord]> {
        let meta = Arc::new(ProcessMeta {
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
        });
        (1..=n).map(|pid| record(pid, &meta)).collect()
    }

    fn slot_with_channel() -> (HistorySlot, mpsc::Receiver<HistoryMsg>) {
        let (tx, rx) = mpsc::channel(HISTORY_CHANNEL_CAPACITY);
        let slot: HistorySlot = Arc::new(ArcSwapOption::from_pointee(tx));
        (slot, rx)
    }

    #[test]
    fn sink_without_sender_drops() {
        let mut sink = HistorySink::new(Arc::new(ArcSwapOption::empty()));
        sink.on_process_sweep(1, &sweep(3));
        assert_eq!(sink.dropped(), 1);
    }

    #[test]
    fn sink_never_blocks_when_full() {
        let (slot, rx) = slot_with_channel();
        let mut sink = HistorySink::new(slot);
        let records = sweep(1);
        for t in 0..600 {
            sink.on_process_sweep(t, &records);
        }
        assert_eq!(rx.len(), 512);
        assert_eq!(sink.dropped(), 88);
    }

    #[test]
    fn sweep_arc_not_copied() {
        let (slot, mut rx) = slot_with_channel();
        let mut sink = HistorySink::new(slot);
        let records = sweep(2_000);
        sink.on_process_sweep(7, &records);
        match rx.try_recv() {
            Ok(HistoryMsg::Sweep { t_ns, records: got }) => {
                assert_eq!(t_ns, 7);
                assert!(Arc::ptr_eq(&got, &records));
            }
            _ => panic!("expected one Sweep message"),
        }
        assert_eq!(sink.dropped(), 0);
    }

    #[test]
    fn command_forwards_query() {
        let (slot, mut rx) = slot_with_channel();
        let mut sink = HistorySink::new(slot);
        let what = HistoryWhat::TopAt {
            t: 5,
            metric: HistoryMetric::Cpu,
            n: 3,
        };
        sink.command(HistoryMsg::Query {
            id: 42,
            range: HistoryRange::At(5),
            what: what.clone(),
        });
        match rx.try_recv() {
            Ok(HistoryMsg::Query {
                id,
                range,
                what: got,
            }) => {
                assert_eq!(id, 42);
                assert_eq!(range, HistoryRange::At(5));
                assert_eq!(got, what);
            }
            _ => panic!("expected one Query message"),
        }
        assert_eq!(sink.dropped(), 0);
    }
}
