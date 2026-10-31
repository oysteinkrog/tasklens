//! LabRuntime tests for the combined demand (q4x76.19, ADR-018): the workers collect for every
//! client session's demand plus the background demand of the host settings, with the list caps
//! filled round robin across the sessions in connect order, the local session first.
//!
//! Each `run` call gets its own task. The messages are queued before the task starts and the
//! sender is dropped, so `run` drains the queue and then returns.

use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::ArcSwap;
use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::client_session::{
    ClientMsg, ClientSessionId, SessionLink, background_demand, combine,
};
use tl_engine::reducer::{Publisher, Reducer};
use tl_engine::source::WorkerControl;
use tl_engine::update::EngineMsg;
use tl_model::demand::{
    DETAILS_MAX, Demand, DetailDemand, DeviceHistoryRequest, DeviceRange, GRAPH_TARGETS_MAX,
};
use tl_model::details::{DetailTab, DetailTarget};
use tl_model::ids::{ColumnId, GraphTarget, ProcessKey};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::time::BootId;

const BOOT: BootId = BootId(0x1234);
const FIRST: ClientSessionId = ClientSessionId(7);
const SECOND: ClientSessionId = ClientSessionId(8);

struct Rig {
    lab: LabRuntime,
    root: asupersync::types::RegionId,
    reducer: Option<Reducer>,
    publisher: Option<Publisher>,
    _wake_rx: async_channel::Receiver<()>,
    /// The local window's demand.
    local_tx: watch::Sender<Demand>,
    settings_tx: watch::Sender<EngineSettings>,
    /// What the workers read.
    workers: watch::Receiver<Demand>,
    /// Keeps the session wake receivers, so no session is closed as gone.
    links: Vec<async_channel::Receiver<()>>,
}

type RunOutput = Arc<Mutex<Option<(Reducer, Publisher)>>>;

impl Rig {
    fn new(settings: EngineSettings) -> Self {
        let (control_tx, _control_rx) = watch::channel(WorkerControl::default());
        let (events_tx, _events_rx) = async_channel::bounded(64);
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let publisher = Publisher {
            latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
            wake_tx,
            stop_publishing: false,
        };
        let mut reducer = Reducer::new(1, BOOT, control_tx, events_tx);
        let (local_tx, local_rx) = watch::channel(Demand::default());
        let (settings_tx, settings_rx) = watch::channel(settings);
        let (out_tx, workers) = watch::channel(Demand::default());
        reducer.demand_rx = Some(local_rx);
        reducer.settings_rx = Some(settings_rx);
        reducer.demand_out = Some(out_tx);
        let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        Rig {
            lab,
            root,
            reducer: Some(reducer),
            publisher: Some(publisher),
            _wake_rx: wake_rx,
            local_tx,
            settings_tx,
            workers,
            links: Vec::new(),
        }
    }

    /// Queues `msgs` as one batch, runs the loop until the channel closes and returns the
    /// demand the workers read after it.
    fn run(&mut self, msgs: Vec<EngineMsg>) -> Demand {
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
        self.reducer = Some(reducer);
        self.publisher = Some(publisher);
        self.workers.borrow_and_clone()
    }

    fn open(&mut self, id: ClientSessionId) -> EngineMsg {
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (events_tx, _events_rx) = async_channel::bounded(1);
        let (requests_tx, _requests_rx) = async_channel::unbounded();
        self.links.push(wake_rx);
        EngineMsg::Client(ClientMsg::Open {
            id,
            link: SessionLink {
                latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
                wake_tx,
                events_tx,
                requests_tx,
            },
        })
    }
}

fn history(device_metrics: bool) -> EngineSettings {
    let mut s = EngineSettings::default();
    s.history.device_metrics = device_metrics;
    s
}

fn demand_of(id: ClientSessionId, demand: Demand) -> EngineMsg {
    EngineMsg::Client(ClientMsg::Demand {
        id,
        demand: Box::new(demand),
    })
}

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 1000 + u64::from(pid),
        boot_id: BOOT,
        pid,
    }
}

/// `n` Properties windows on processes `first..first + n`, and graphs of the same processes.
fn windows(first: u32, n: u32) -> Demand {
    let details: Vec<DetailDemand> = (first..first + n)
        .map(|pid| DetailDemand {
            target: DetailTarget::Process(key(pid)),
            tab: DetailTab::General,
        })
        .collect();
    let graphs: Vec<(GraphTarget, ColumnId)> = (first..first + n)
        .map(|pid| (GraphTarget::Process(key(pid)), ColumnId::Cpu))
        .collect();
    Demand {
        any_surface_visible: true,
        details: details.into(),
        graph_targets: graphs.into(),
        ..Demand::default()
    }
}

/// How many entries of `details` lie in each of the pid ranges `ranges`.
fn share(details: &[DetailDemand], ranges: &[std::ops::Range<u32>]) -> Vec<usize> {
    ranges
        .iter()
        .map(|r| {
            details
                .iter()
                .filter(|d| matches!(d.target, DetailTarget::Process(k) if r.contains(&k.pid)))
                .count()
        })
        .collect()
}

fn graph_share(graphs: &[(GraphTarget, ColumnId)], ranges: &[std::ops::Range<u32>]) -> Vec<usize> {
    ranges
        .iter()
        .map(|r| {
            graphs
                .iter()
                .filter(|(t, _)| matches!(t, GraphTarget::Process(k) if r.contains(&k.pid)))
                .count()
        })
        .collect()
}

/// With no window showing anything, the workers collect what history needs while
/// `history.device_metrics` is on, and nothing while it is off (ADR-018 "History off").
#[test]
fn background_demand_follows_history_setting() {
    let mut rig = Rig::new(history(true));
    let on = rig.run(vec![EngineMsg::Tick]);
    assert_eq!(
        on,
        Demand {
            device_history: true,
            ..Demand::default()
        }
    );
    assert_eq!(on, background_demand(&history(true)));

    rig.settings_tx.send(history(false)).expect("settings sent");
    let off = rig.run(vec![EngineMsg::Tick]);
    assert_eq!(off, Demand::default());
    assert_eq!(background_demand(&history(false)), Demand::default());

    rig.settings_tx.send(history(true)).expect("settings sent");
    assert!(rig.run(vec![EngineMsg::Tick]).device_history);
}

/// The background demand joins what a window asks for: a window that hides keeps history
/// collection running, and its own surfaces come on top.
#[test]
fn background_demand_joins_window_demand() {
    let mut rig = Rig::new(history(true));
    rig.local_tx
        .send(Demand {
            any_surface_visible: true,
            memory: true,
            ..Demand::default()
        })
        .expect("demand sent");
    let shown = rig.run(vec![EngineMsg::Tick]);
    assert!(shown.any_surface_visible && shown.memory && shown.device_history);

    rig.local_tx.send(Demand::default()).expect("demand sent");
    let hidden = rig.run(vec![EngineMsg::Tick]);
    assert_eq!(hidden, background_demand(&history(true)));
}

/// Three sessions with 8 Properties windows and 8 graphs each: the 16 places of each list go
/// round robin in connect order, local first, so they get 6, 5 and 5.
#[test]
fn three_sessions_share_details_round_robin() {
    let mut rig = Rig::new(history(false));
    rig.local_tx.send(windows(100, 8)).expect("demand sent");
    let msgs = vec![
        rig.open(FIRST),
        rig.open(SECOND),
        demand_of(FIRST, windows(200, 8)),
        demand_of(SECOND, windows(300, 8)),
    ];
    let combined = rig.run(msgs);
    let ranges = [100..108, 200..208, 300..308];
    assert_eq!(combined.details.len(), DETAILS_MAX);
    assert_eq!(share(&combined.details, &ranges), vec![6, 5, 5]);
    assert_eq!(combined.graph_targets.len(), GRAPH_TARGETS_MAX);
    assert_eq!(graph_share(&combined.graph_targets, &ranges), vec![6, 5, 5]);
    // Each session's first five come before anyone's sixth.
    assert_eq!(
        combined.details[..3]
            .iter()
            .map(|d| match d.target {
                DetailTarget::Process(k) => k.pid,
                DetailTarget::App(_) => 0,
            })
            .collect::<Vec<_>>(),
        vec![100, 200, 300]
    );
}

/// A session that leaves releases its demand at the next publish; the others keep theirs and
/// take the places it held.
#[test]
fn closed_session_releases_its_demand() {
    let mut rig = Rig::new(history(false));
    rig.local_tx.send(windows(100, 12)).expect("demand sent");
    let mut remote = windows(200, 12);
    remote.connections = true;
    let msgs = vec![rig.open(FIRST), demand_of(FIRST, remote)];
    let both = rig.run(msgs);
    assert!(both.connections);
    assert_eq!(share(&both.details, &[100..112, 200..212]), vec![8, 8]);

    let alone = rig.run(vec![EngineMsg::Client(ClientMsg::Close(FIRST))]);
    assert!(!alone.connections);
    assert_eq!(share(&alone.details, &[100..112, 200..212]), vec![12, 0]);
    assert_eq!(alone, windows(100, 12));
}

/// An entry two sessions share is collected once and counts once; the session whose turn it
/// was takes its next entry instead. A single list that fits keeps its `Arc`.
#[test]
fn combine_dedups_and_keeps_a_lone_list() {
    let a = windows(100, 10);
    // Starts with the same five processes as `a`, then seven of its own.
    let b = windows(105, 12);
    let combined = combine(&[&a, &b]);
    assert_eq!(combined.details.len(), DETAILS_MAX);
    // `a` has 100 to 109, `b` adds 110 to 115 before the list is full.
    assert_eq!(share(&combined.details, &[100..110, 110..116]), vec![10, 6]);

    let alone = combine(&[&a, &Demand::default()]);
    assert!(Arc::ptr_eq(&alone.details, &a.details));
    assert!(Arc::ptr_eq(&alone.graph_targets, &a.graph_targets));
}

/// The range in the combined demand is the local window's: the `MetricHistory` worker answers
/// it from the demand watch and the other sessions' ranges separately.
#[test]
fn combined_range_is_the_local_one() {
    let day = DeviceHistoryRequest {
        range: DeviceRange::H24,
        end_realtime_ns: None,
    };
    let week = DeviceHistoryRequest {
        range: DeviceRange::D7,
        end_realtime_ns: None,
    };
    let mut rig = Rig::new(history(true));
    rig.local_tx
        .send(Demand {
            device_history_range: Some(day),
            ..Demand::default()
        })
        .expect("demand sent");
    let remote = Demand {
        device_history_range: Some(week),
        device_timeline: true,
        ..Demand::default()
    };
    let msgs = vec![rig.open(FIRST), demand_of(FIRST, remote)];
    let combined = rig.run(msgs);
    assert_eq!(combined.device_history_range, Some(day));
    assert!(combined.device_timeline);
}
