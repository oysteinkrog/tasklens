//! LabRuntime tests for per-session history (q4x76.18, ADR-018): each session gets the device
//! history answer for its own range and its own `HistoryQuery` answers, and has at most
//! `HISTORY_ACTIVE_MAX` history queries in flight.
//!
//! Each `run` call gets its own task. The messages are queued before the task starts and the
//! sender is dropped, so `run` drains the queue and then returns.

use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::{ArcSwap, ArcSwapOption};
use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::client_session::{
    ClientMsg, ClientSessionId, HISTORY_ACTIVE_MAX, IdKind, RequestEnd, RequestEvent, SessionLink,
};
use tl_engine::reducer::{Publisher, Reducer};
use tl_engine::source::WorkerControl;
use tl_engine::update::{EngineMsg, HistoryUpdate, SourceUpdate};
use tl_engine::workers::history::{HistoryMsg, HistorySink, HistoryTx};
use tl_engine::workers::metric_history::{MetricMsg, MetricSink, MetricTx};
use tl_model::command::{Command, HistoryWhat};
use tl_model::demand::{Demand, DeviceHistoryRequest, DeviceRange};
use tl_model::history::{HistoryMetric, HistoryRange, HistoryTop, HistoryView};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::{DeviceHistory, HistPoint, Snapshot};
use tl_model::time::BootId;

const BOOT: BootId = BootId(0x1234);
const REMOTE: ClientSessionId = ClientSessionId(7);
const DAY: DeviceHistoryRequest = DeviceHistoryRequest {
    range: DeviceRange::H24,
    end_realtime_ns: None,
};
const WEEK: DeviceHistoryRequest = DeviceHistoryRequest {
    range: DeviceRange::D7,
    end_realtime_ns: None,
};

struct Rig {
    lab: LabRuntime,
    root: asupersync::types::RegionId,
    reducer: Option<Reducer>,
    publisher: Option<Publisher>,
    _wake_rx: async_channel::Receiver<()>,
    demand_tx: watch::Sender<Demand>,
    local_requests: async_channel::Receiver<RequestEvent>,
    history_rx: mpsc::Receiver<HistoryMsg>,
    metric_rx: mpsc::Receiver<MetricMsg>,
}

type RunOutput = Arc<Mutex<Option<(Reducer, Publisher)>>>;

impl Rig {
    /// A reducer with the local demand watch, a history sink taken by generation 1 and a
    /// metric sink with an open store.
    fn new() -> Self {
        let (control_tx, _control_rx) = watch::channel(WorkerControl::default());
        let (events_tx, _events_rx) = async_channel::bounded(64);
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let publisher = Publisher {
            latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
            wake_tx,
            stop_publishing: false,
        };
        let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let mut reducer = Reducer::new(1, BOOT, control_tx, events_tx);
        let (demand_tx, demand_rx) = watch::channel(Demand::default());
        reducer.demand_rx = Some(demand_rx);
        let (requests_tx, local_requests) = async_channel::unbounded();
        reducer.clients.set_local_requests(requests_tx);
        let (tx, history_rx) = mpsc::channel(64);
        let slot = Arc::new(ArcSwapOption::from_pointee(HistoryTx { generation: 1, tx }));
        reducer.history_cmd = Some(HistorySink::new(slot));
        let (tx, metric_rx) = mpsc::channel(64);
        let slot = Arc::new(ArcSwapOption::from_pointee(MetricTx { generation: 1, tx }));
        reducer.metric_sink = Some(MetricSink::new(slot));
        Rig {
            lab,
            root,
            reducer: Some(reducer),
            publisher: Some(publisher),
            _wake_rx: wake_rx,
            demand_tx,
            local_requests,
            history_rx,
            metric_rx,
        }
    }

    fn reducer(&self) -> &Reducer {
        self.reducer.as_ref().expect("the reducer is back")
    }

    /// Queues `msgs` as one batch, runs the loop until the channel closes and returns the
    /// local session's last snapshot.
    fn run(&mut self, msgs: Vec<EngineMsg>) -> Arc<Snapshot> {
        let (tx, mut rx) = mpsc::channel::<EngineMsg>(msgs.len().max(1));
        for msg in msgs {
            tx.try_send(msg)
                .expect("the channel has room for every message");
        }
        drop(tx);
        let mut reducer = self.reducer.take().expect("one run at a time");
        let mut publisher = self.publisher.take().expect("one run at a time");
        let out: RunOutput = Arc::default();
        let slot = Arc::clone(&out);
        let (task, _handle) = self
            .lab
            .state
            .create_task(self.root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                reducer
                    .run(&cx, &mut rx, &mut publisher)
                    .await
                    .expect("no invariant violation");
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((reducer, publisher));
            })
            .expect("task is created");
        self.lab.scheduler.lock().schedule(task, 0);
        self.lab.run_until_idle();
        let (reducer, publisher) = out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("run returned");
        let latest = publisher.latest.load_full();
        self.reducer = Some(reducer);
        self.publisher = Some(publisher);
        latest
    }

    /// The local window shows `range`.
    fn local_range(&mut self, range: Option<DeviceHistoryRequest>) {
        self.demand_tx
            .send_modify(|d| d.device_history_range = range)
            .expect("the reducer holds the receiver");
    }

    /// The queries the history worker received, as `(owner, engine id)`.
    fn queries(&mut self) -> Vec<(ClientSessionId, u64)> {
        let mut out = Vec::new();
        while let Ok(msg) = self.history_rx.try_recv() {
            if let HistoryMsg::Query { owner, id, .. } = msg {
                out.push((owner, id));
            }
        }
        out
    }

    /// The last `MetricMsg::Ranges` the metric worker received.
    fn last_ranges(&mut self) -> Option<Vec<Option<DeviceHistoryRequest>>> {
        let mut last = None;
        while let Ok(msg) = self.metric_rx.try_recv() {
            if let MetricMsg::Ranges(ranges) = msg {
                last = Some(ranges.to_vec());
            }
        }
        last
    }
}

/// A remote session's link, its wake receiver and its request receiver.
struct Remote {
    link: SessionLink,
    _wake_rx: async_channel::Receiver<()>,
    requests: async_channel::Receiver<RequestEvent>,
}

impl Remote {
    fn new() -> Self {
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (events_tx, _events_rx) = async_channel::bounded(1);
        let (requests_tx, requests) = async_channel::unbounded();
        let link = SessionLink {
            latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
            wake_tx,
            events_tx,
            requests_tx,
        };
        Remote {
            link,
            _wake_rx: wake_rx,
            requests,
        }
    }

    fn open(&self) -> EngineMsg {
        EngineMsg::Client(ClientMsg::Open {
            id: REMOTE,
            link: self.link.clone(),
        })
    }

    fn snapshot(&self) -> Arc<Snapshot> {
        self.link.latest.load_full()
    }
}

fn drain(rx: &async_channel::Receiver<RequestEvent>) -> Vec<RequestEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

fn demand(id: ClientSessionId, range: Option<DeviceHistoryRequest>) -> EngineMsg {
    EngineMsg::Client(ClientMsg::Demand {
        id,
        demand: Box::new(Demand {
            device_history_range: range,
            ..Demand::default()
        }),
    })
}

fn on(id: ClientSessionId, cmd: Command) -> EngineMsg {
    if id == ClientSessionId::LOCAL {
        EngineMsg::Command(cmd)
    } else {
        EngineMsg::Client(ClientMsg::Command {
            id,
            cmd: Box::new(cmd),
        })
    }
}

fn query(id: u64) -> Command {
    Command::HistoryQuery {
        id,
        range: HistoryRange::Live,
        what: HistoryWhat::Frame {
            metric: HistoryMetric::Cpu,
            view: (0, u64::MAX),
            keys: Arc::from([]),
            overview: false,
        },
    }
}

fn history_view(owner: ClientSessionId, answers: Option<u64>, view: HistoryView) -> EngineMsg {
    EngineMsg::Source(SourceUpdate::History(HistoryUpdate::View {
        owner,
        answers,
        view: Arc::new(view),
    }))
}

fn ended(id: u64, end: RequestEnd) -> RequestEvent {
    RequestEvent {
        kind: IdKind::History,
        id,
        end,
    }
}

/// An answer for `range` with one CPU series.
fn answer(range: DeviceHistoryRequest, mean: f32) -> Arc<DeviceHistory> {
    let points: Arc<[HistPoint]> = Arc::from([HistPoint {
        t_realtime_ns: 1,
        mean,
        min: mean,
        max: mean,
    }]);
    let id = SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::CPU_TOTAL,
    };
    Arc::new(DeviceHistory {
        request: Some(range),
        series: Arc::from([(id, points)]),
        ..DeviceHistory::default()
    })
}

fn histories(list: &[(DeviceHistoryRequest, &Arc<DeviceHistory>)]) -> EngineMsg {
    let list: Vec<_> = list
        .iter()
        .map(|(k, h)| (Some(*k), Arc::clone(h)))
        .collect();
    EngineMsg::Source(SourceUpdate::DeviceHistory(list.into()))
}

/// ADR-018: the local window on 24 h and a remote session on 7 d each get the answer for their
/// own range. The worker is told the remote range; a new answer for one range leaves the other
/// session's version alone.
#[test]
fn sessions_on_24h_and_7d_each_get_their_own_range() {
    let mut rig = Rig::new();
    let remote = Remote::new();
    rig.local_range(Some(DAY));
    rig.run(vec![remote.open(), demand(REMOTE, Some(WEEK))]);
    assert_eq!(
        rig.last_ranges(),
        Some(vec![Some(WEEK)]),
        "the worker learns the 7 d range"
    );

    let (day, week) = (answer(DAY, 1.0), answer(WEEK, 2.0));
    let local = rig.run(vec![histories(&[(DAY, &day), (WEEK, &week)])]);
    let other = remote.snapshot();
    assert_eq!(local.device_history.request, Some(DAY));
    assert!(Arc::ptr_eq(&local.device_history.series, &day.series));
    assert_eq!(other.device_history.request, Some(WEEK));
    assert!(Arc::ptr_eq(&other.device_history.series, &week.series));

    // A new 7 d answer: the remote session's part changes, the local one keeps its `Arc`.
    let week2 = answer(WEEK, 3.0);
    let local2 = rig.run(vec![histories(&[(DAY, &day), (WEEK, &week2)])]);
    let other2 = remote.snapshot();
    assert!(Arc::ptr_eq(&local.device_history, &local2.device_history));
    assert!(Arc::ptr_eq(&other2.device_history.series, &week2.series));
    assert_eq!(
        other2.device_history.header.version,
        other.device_history.header.version + 1
    );

    // The remote session moves to 24 h: it shows that answer at once, without a new read.
    rig.run(vec![demand(REMOTE, Some(DAY))]);
    assert!(Arc::ptr_eq(
        &remote.snapshot().device_history.series,
        &day.series
    ));
    assert_eq!(rig.last_ranges(), Some(vec![Some(DAY)]));
}

/// ADR-018: two sessions on one range share one answer, and the worker is asked for the range
/// once. A session whose range has no answer yet keeps what it shows.
#[test]
fn two_sessions_on_one_range_share_one_answer() {
    let mut rig = Rig::new();
    let remote = Remote::new();
    let second = ClientSessionId(8);
    let second_remote = Remote::new();
    rig.local_range(Some(DAY));
    rig.run(vec![
        remote.open(),
        EngineMsg::Client(ClientMsg::Open {
            id: second,
            link: second_remote.link.clone(),
        }),
        demand(REMOTE, Some(DAY)),
        demand(second, Some(DAY)),
    ]);
    assert_eq!(rig.reducer().clients.linked_ranges(), [Some(DAY)]);
    assert_eq!(rig.last_ranges(), Some(vec![Some(DAY)]));

    let day = answer(DAY, 1.0);
    let local = rig.run(vec![histories(&[(DAY, &day)])]);
    for snap in [local, remote.snapshot(), second_remote.snapshot()] {
        assert!(Arc::ptr_eq(&snap.device_history.series, &day.series));
    }

    // The remote session asks for 7 d: until its answer comes it keeps showing 24 h.
    rig.run(vec![demand(REMOTE, Some(WEEK))]);
    assert_eq!(rig.last_ranges(), Some(vec![Some(WEEK), Some(DAY)]));
    assert_eq!(remote.snapshot().device_history.request, Some(DAY));
}

/// ADR-018: a remote session that clears its range while the local window keeps 24 h asks the
/// worker for the key `None`, and shows the empty answer once it comes instead of its old points.
#[test]
fn a_session_that_clears_its_range_shows_the_empty_answer() {
    let mut rig = Rig::new();
    let remote = Remote::new();
    rig.local_range(Some(DAY));
    rig.run(vec![remote.open(), demand(REMOTE, Some(WEEK))]);
    let (day, week) = (answer(DAY, 1.0), answer(WEEK, 2.0));
    rig.run(vec![histories(&[(DAY, &day), (WEEK, &week)])]);
    assert!(Arc::ptr_eq(
        &remote.snapshot().device_history.series,
        &week.series
    ));
    rig.last_ranges();

    rig.run(vec![demand(REMOTE, None)]);
    assert_eq!(rig.reducer().clients.linked_ranges(), [None]);
    assert_eq!(
        rig.last_ranges(),
        Some(vec![None]),
        "the worker learns `None`"
    );

    let empty = Arc::new(DeviceHistory::default());
    let local = rig.run(vec![EngineMsg::Source(SourceUpdate::DeviceHistory(
        vec![(Some(DAY), Arc::clone(&day)), (None, empty)].into(),
    ))]);
    let other = remote.snapshot();
    assert!(Arc::ptr_eq(&local.device_history.series, &day.series));
    assert_eq!(other.device_history.request, None);
    assert!(other.device_history.series.is_empty());
}

/// ADR-018: history queries are per session. Each gets a new engine id, the answer goes to the
/// session that asked under its own id, and the fifth query in flight is refused with `Busy`.
#[test]
fn history_queries_are_per_session_and_the_fifth_is_busy() {
    let mut rig = Rig::new();
    let remote = Remote::new();
    rig.run(vec![remote.open()]);
    let mut msgs: Vec<EngineMsg> = (1..=5).map(|id| on(REMOTE, query(id))).collect();
    msgs.push(on(ClientSessionId::LOCAL, query(1)));
    rig.run(msgs);
    let sent = rig.queries();
    assert_eq!(
        sent.len(),
        HISTORY_ACTIVE_MAX + 1,
        "four remote queries and the local one"
    );
    assert!(sent[..4].iter().all(|(owner, _)| *owner == REMOTE));
    assert_eq!(sent[4].0, ClientSessionId::LOCAL);
    let mut engines: Vec<u64> = sent.iter().map(|(_, e)| *e).collect();
    engines.dedup();
    assert_eq!(engines.len(), 5, "every query has its own engine id");
    assert_eq!(drain(&remote.requests), [ended(5, RequestEnd::Busy)]);
    assert!(drain(&rig.local_requests).is_empty());

    // The local query's answer goes to the local session under its id 1. The first answer
    // also makes the shared validity `Valid`.
    let before = rig.run(vec![history_view(
        ClientSessionId::LOCAL,
        Some(sent[4].1),
        HistoryView {
            query_id: sent[4].1,
            ..HistoryView::default()
        },
    )]);
    assert_eq!(before.history.query_id, 1);
    assert_eq!(drain(&rig.local_requests), [ended(1, RequestEnd::Done)]);
    assert_eq!(remote.snapshot().history.query_id, 0);

    // The answer to the remote query 2 reaches the remote session only.
    let top = Arc::new(HistoryTop {
        query_id: sent[1].1,
        t_ns: 0,
        metric: HistoryMetric::Cpu,
        cpu_total_pct: 0.0,
        mem_total_bytes: 0,
        rows: Arc::from([]),
    });
    let view = HistoryView {
        query_id: sent[1].1,
        top: Some(Arc::clone(&top)),
        ..HistoryView::default()
    };
    let local = rig.run(vec![history_view(REMOTE, Some(sent[1].1), view.clone())]);
    let other = remote.snapshot();
    assert_eq!(other.history.query_id, 2);
    assert_eq!(other.history.top.as_ref().map(|t| t.query_id), Some(2));
    assert_eq!(drain(&remote.requests), [ended(2, RequestEnd::Done)]);
    assert!(
        Arc::ptr_eq(&local.history, &before.history),
        "local view unchanged"
    );

    // One slot is free again: a fifth query is taken.
    rig.run(vec![on(REMOTE, query(6))]);
    assert_eq!(rig.queries().len(), 1);
    assert!(drain(&remote.requests).is_empty());

    // After a delete the worker sends the view again: same ids, no second end, same top `Arc`.
    rig.run(vec![history_view(REMOTE, None, view)]);
    let again = remote.snapshot();
    assert_eq!(again.history.query_id, 2);
    assert!(Arc::ptr_eq(
        again.history.top.as_ref().expect("a top list"),
        other.history.top.as_ref().expect("a top list")
    ));
    assert!(drain(&remote.requests).is_empty());
}

/// A query the history sink could not take ends as `Lost` at once; a query an older worker
/// generation took ends as `Lost` when a new generation starts; a closed session's queries are
/// forgotten by the worker.
#[test]
fn lost_history_queries_end_and_free_their_slot() {
    let mut rig = Rig::new();
    let remote = Remote::new();
    rig.run(vec![
        remote.open(),
        on(REMOTE, query(1)),
        on(REMOTE, query(2)),
    ]);
    assert_eq!(rig.queries().len(), 2);

    // Generation 1 stopped; generation 2 starts.
    rig.run(vec![EngineMsg::Source(SourceUpdate::History(
        HistoryUpdate::Started { generation: 2 },
    ))]);
    assert_eq!(
        drain(&remote.requests),
        [ended(1, RequestEnd::Lost), ended(2, RequestEnd::Lost)]
    );

    // No sender: the sink drops the query.
    rig.reducer
        .as_mut()
        .expect("the reducer is back")
        .history_cmd = Some(HistorySink::new(Arc::new(ArcSwapOption::empty())));
    rig.run(vec![on(REMOTE, query(3))]);
    assert_eq!(drain(&remote.requests), [ended(3, RequestEnd::Lost)]);
    let session = rig.reducer().clients.get(REMOTE).expect("open");
    assert_eq!(session.active(IdKind::History), 0);
}

/// Closing a session tells the history worker to forget its queries.
#[test]
fn closing_a_session_forgets_its_history() {
    let mut rig = Rig::new();
    let remote = Remote::new();
    rig.run(vec![remote.open()]);
    rig.run(vec![EngineMsg::Client(ClientMsg::Close(REMOTE))]);
    let forgot = std::iter::from_fn(|| rig.history_rx.try_recv().ok())
        .any(|msg| matches!(msg, HistoryMsg::Forget(id) if id == REMOTE));
    assert!(forgot);
}
