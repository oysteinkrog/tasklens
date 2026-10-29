//! LabRuntime tests for per-session id namespaces (q4x76.17, ADR-018): two sessions that use
//! the same action ids, scan ids, search ids and eject `seq`s never see each other's events or
//! results, and each one's ids reach the workers as distinct engine ids.
//!
//! Each `run` call gets its own task. The messages are queued before the task starts and the
//! sender is dropped, so `run` drains the queue and then returns.

use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::ArcSwap;
use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use smallvec::SmallVec;
use tl_engine::client_session::{ClientMsg, ClientSessionId, SessionLink};
use tl_engine::reducer::{ActionGate, ActionRoute, AdmissionView, Publisher, Reducer};
use tl_engine::source::{WorkerCommand, WorkerControl};
use tl_engine::update::{DetailsUpdate, DeviceDetailsUpdate, EngineMsg, SourceUpdate};
use tl_model::actions::{
    ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, RejectReason, Signal,
};
use tl_model::command::{Command, DeviceCommand};
use tl_model::details::{
    DetailTab, DetailTarget, HandlePhase, HandleSearchState, ScanId, ScanPhase, SearchId,
    StringScanOpts, StringScanState, TabPayload,
};
use tl_model::devices::DeviceIdentity;
use tl_model::health::SourceId;
use tl_model::ids::ProcessKey;
use tl_model::snapshot::{DiskDetails, EjectState, Snapshot};
use tl_model::time::BootId;
use tl_model::validity::Validity;

const BOOT: BootId = BootId(0x1234);
const REMOTE: ClientSessionId = ClientSessionId(7);
const KEY: ProcessKey = ProcessKey {
    start_ticks: 420,
    boot_id: BOOT,
    pid: 42,
};

struct Rig {
    lab: LabRuntime,
    root: asupersync::types::RegionId,
    reducer: Option<Reducer>,
    publisher: Option<Publisher>,
    control: watch::Receiver<WorkerControl>,
    /// The local session's action events.
    events: async_channel::Receiver<ActionEvent>,
    _wake_rx: async_channel::Receiver<()>,
}

type RunOutput = Arc<Mutex<Option<(Reducer, Publisher)>>>;

impl Rig {
    fn new() -> Self {
        let (control_tx, control) = watch::channel(WorkerControl::default());
        let (events_tx, events) = async_channel::bounded(64);
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let publisher = Publisher {
            latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
            wake_tx,
            stop_publishing: false,
        };
        let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        Rig {
            lab,
            root,
            reducer: Some(Reducer::new(1, BOOT, control_tx, events_tx)),
            publisher: Some(publisher),
            control,
            events,
            _wake_rx: wake_rx,
        }
    }

    fn reducer_mut(&mut self) -> &mut Reducer {
        self.reducer.as_mut().expect("the reducer is back")
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

    /// The forwarded worker commands, oldest first.
    fn commands(&self) -> Vec<WorkerCommand> {
        self.control
            .borrow_and_clone()
            .commands
            .iter()
            .map(|q| q.command.clone())
            .collect()
    }
}

/// A remote session's link and the receivers that keep it open.
struct Remote {
    link: SessionLink,
    events: async_channel::Receiver<ActionEvent>,
    _wake_rx: async_channel::Receiver<()>,
}

impl Remote {
    fn new() -> Self {
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (events_tx, events) = async_channel::bounded(64);
        Remote {
            link: SessionLink {
                latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
                wake_tx,
                events_tx,
            },
            events,
            _wake_rx: wake_rx,
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

fn drain(rx: &async_channel::Receiver<ActionEvent>) -> Vec<ActionEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn action(id: u64) -> Command {
    Command::Action(ActionRequest {
        id: ActionId(id),
        kind: ActionKind::Signal(Signal::Term),
        targets: SmallVec::new(),
        privileged: false,
    })
}

fn scan_state(scan: u64) -> EngineMsg {
    let state = StringScanState {
        scan: ScanId(scan),
        progress_pct: 50,
        found: 0,
        chunks: Arc::from([]),
        phase: ScanPhase::Scanning,
    };
    EngineMsg::Source(SourceUpdate::Details(DetailsUpdate::Tab {
        target: DetailTarget::Process(KEY),
        tab: DetailTab::Strings,
        payload: Arc::new(TabPayload::Strings(Arc::new(state))),
        validity: Validity::Valid,
    }))
}

/// The scan id in the Strings tab of `KEY`, if any.
fn shown_scan(snap: &Snapshot) -> Option<ScanId> {
    let detail = snap.details.targets.get(&DetailTarget::Process(KEY))?;
    match detail.tab(DetailTab::Strings).payload.as_deref()? {
        TabPayload::Strings(state) => Some(state.scan),
        _ => None,
    }
}

fn search_state(search: u64) -> EngineMsg {
    EngineMsg::Source(SourceUpdate::Details(DetailsUpdate::HandleSearch(
        Arc::new(HandleSearchState {
            search: SearchId(search),
            phase: HandlePhase::Searching,
            progress_pct: 10,
            total_handles: 0,
            chunks: Arc::from([]),
            searched: 0,
            total: 1,
            skipped: 0,
        }),
    )))
}

fn disk_eject(seq: u64) -> EngineMsg {
    let identity = DeviceIdentity::Disk {
        major: 8,
        minor: 0,
        name: Arc::from("sda"),
        wwn_or_serial: None,
    };
    let details = DiskDetails {
        ejectable: true,
        eject: Some((seq, EjectState::Running)),
        ..DiskDetails::default()
    };
    EngineMsg::Source(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks {
        from: SourceId::Udisks,
        entries: vec![(identity, details)],
    }))
}

fn shown_eject(snap: &Snapshot) -> Option<u64> {
    snap.device_details
        .disks
        .iter()
        .find_map(|(_, d)| d.eject.as_ref().map(|(seq, _)| *seq))
}

/// Records the engine id of every routed request and sends one `Started` event down its route.
struct RoutingGate(Arc<Mutex<Vec<ActionId>>>);

impl ActionGate for RoutingGate {
    fn submit(&mut self, _cx: &Cx, _req: ActionRequest, _view: &AdmissionView<'_>) {
        panic!("the reducer submits every request with its route");
    }

    fn submit_routed(
        &mut self,
        _cx: &Cx,
        req: ActionRequest,
        _view: &AdmissionView<'_>,
        route: ActionRoute,
    ) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(route.id);
        let ev = ActionEvent {
            id: req.id,
            target: None,
            state: ActionState::Started,
        };
        route.events.try_send(ev).expect("room for the event");
    }

    fn finished(&mut self, _id: ActionId) -> Option<SourceId> {
        None
    }
}

#[test]
fn same_action_id_reaches_only_its_own_session() {
    let mut rig = Rig::new();
    let engine_ids = Arc::new(Mutex::new(Vec::new()));
    rig.reducer_mut().action_gate = Some(Box::new(RoutingGate(Arc::clone(&engine_ids))));
    let remote = Remote::new();
    rig.run(vec![
        remote.open(),
        on(ClientSessionId::LOCAL, action(1)),
        on(REMOTE, action(1)),
        on(REMOTE, action(2)),
    ]);

    let local: Vec<_> = drain(&rig.events).iter().map(|e| e.id).collect();
    let remote_ids: Vec<_> = drain(&remote.events).iter().map(|e| e.id).collect();
    assert_eq!(
        local,
        [ActionId(1)],
        "the local window gets only its own action"
    );
    assert_eq!(remote_ids, [ActionId(1), ActionId(2)]);

    let engine_ids = engine_ids.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(engine_ids.len(), 3);
    assert!(
        engine_ids[0] != engine_ids[1]
            && engine_ids[1] != engine_ids[2]
            && engine_ids[0] != engine_ids[2],
        "each request gets its own engine id: {engine_ids:?}"
    );
}

#[test]
fn rejection_without_a_gate_goes_to_the_sender() {
    let mut rig = Rig::new();
    let remote = Remote::new();
    rig.run(vec![remote.open(), on(REMOTE, action(5))]);

    assert!(
        drain(&rig.events).is_empty(),
        "the local window sees nothing"
    );
    let events = drain(&remote.events);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, ActionId(5));
    assert_eq!(
        events[0].state,
        ActionState::Rejected(RejectReason::Unsupported)
    );
}

#[test]
fn same_scan_id_reaches_only_its_own_session() {
    let mut rig = Rig::new();
    let remote = Remote::new();
    let scan = || Command::StringScan {
        scan: ScanId(1),
        key: KEY,
        opts: StringScanOpts::default(),
    };
    rig.run(vec![
        remote.open(),
        on(ClientSessionId::LOCAL, scan()),
        on(REMOTE, scan()),
    ]);

    let engine: Vec<ScanId> = rig
        .commands()
        .iter()
        .filter_map(|c| match c {
            WorkerCommand::StringScan { scan, key, .. } if *key == KEY => Some(*scan),
            _ => None,
        })
        .collect();
    assert_eq!(engine.len(), 2);
    assert_ne!(engine[0], engine[1], "two sessions, two engine scan ids");
    let (local_engine, remote_engine) = (engine[0], engine[1]);

    // The remote scan's results: only the remote session sees them, under its own id.
    let local = rig.run(vec![scan_state(remote_engine.0)]);
    assert_eq!(
        shown_scan(&local),
        None,
        "the local window hides the remote scan"
    );
    assert_eq!(shown_scan(&remote.snapshot()), Some(ScanId(1)));

    // Then the local scan replaces it in the shared slot.
    let local = rig.run(vec![scan_state(local_engine.0)]);
    assert_eq!(shown_scan(&local), Some(ScanId(1)));
    assert_eq!(shown_scan(&remote.snapshot()), None);

    // A cancel names the sender's own scan.
    rig.run(vec![on(REMOTE, Command::CancelStringScan(ScanId(1)))]);
    rig.run(vec![on(REMOTE, Command::CancelStringScan(ScanId(9)))]);
    let cancels: Vec<_> = rig
        .commands()
        .into_iter()
        .filter(|c| matches!(c, WorkerCommand::CancelStringScan(_)))
        .collect();
    assert_eq!(
        cancels,
        [WorkerCommand::CancelStringScan(remote_engine)],
        "an unknown scan id is dropped"
    );
}

#[test]
fn unchanged_scan_keeps_its_rewritten_arc() {
    let mut rig = Rig::new();
    let first = rig.run(vec![on(
        ClientSessionId::LOCAL,
        Command::StringScan {
            scan: ScanId(3),
            key: KEY,
            opts: StringScanOpts::default(),
        },
    )]);
    assert_eq!(shown_scan(&first), None);
    let engine = rig
        .commands()
        .iter()
        .find_map(|c| match c {
            WorkerCommand::StringScan { scan, .. } => Some(*scan),
            _ => None,
        })
        .expect("the scan was forwarded");

    let a = rig.run(vec![scan_state(engine.0)]);
    // Another tab of the same target changes; the Strings payload does not.
    let b = rig.run(vec![EngineMsg::Source(SourceUpdate::Details(
        DetailsUpdate::Tab {
            target: DetailTarget::Process(KEY),
            tab: DetailTab::Threads,
            payload: Arc::new(TabPayload::Threads(Arc::from([]))),
            validity: Validity::Valid,
        },
    ))]);
    let payload = |s: &Snapshot| {
        s.details.targets[&DetailTarget::Process(KEY)]
            .tab(DetailTab::Strings)
            .payload
            .clone()
            .expect("the scan is shown")
    };
    assert_eq!(shown_scan(&a), Some(ScanId(3)));
    assert!(Arc::ptr_eq(&payload(&a), &payload(&b)));
}

#[test]
fn same_search_id_reaches_only_its_own_session() {
    let mut rig = Rig::new();
    let remote = Remote::new();
    let search = || Command::HandleSearch {
        search: SearchId(1),
        pattern: Arc::from("*.log"),
        include_maps: true,
        privileged: false,
    };
    rig.run(vec![
        remote.open(),
        on(ClientSessionId::LOCAL, search()),
        on(REMOTE, search()),
    ]);
    let engine: Vec<SearchId> = rig
        .commands()
        .iter()
        .filter_map(|c| match c {
            WorkerCommand::HandleSearch { search, .. } => Some(*search),
            _ => None,
        })
        .collect();
    assert_eq!(engine.len(), 2);
    assert_ne!(engine[0], engine[1]);

    let local = rig.run(vec![search_state(engine[1].0)]);
    assert!(
        local.details.handle_search.is_none(),
        "remote search hidden"
    );
    let remote_search = remote.snapshot().details.handle_search.clone();
    assert_eq!(remote_search.map(|s| s.search), Some(SearchId(1)));

    let local = rig.run(vec![search_state(engine[0].0)]);
    assert_eq!(
        local.details.handle_search.as_ref().map(|s| s.search),
        Some(SearchId(1))
    );
    assert!(remote.snapshot().details.handle_search.is_none());

    rig.run(vec![on(
        ClientSessionId::LOCAL,
        Command::CancelHandleSearch(SearchId(1)),
    )]);
    assert_eq!(
        rig.commands().last(),
        Some(&WorkerCommand::CancelHandleSearch(engine[0]))
    );
}

#[test]
fn same_eject_seq_reaches_only_its_own_session() {
    let mut rig = Rig::new();
    let remote = Remote::new();
    let eject = || {
        Command::Device(DeviceCommand::Eject {
            seq: 1,
            disk: Arc::from("sda"),
        })
    };
    rig.run(vec![
        remote.open(),
        on(REMOTE, eject()),
        on(ClientSessionId::LOCAL, eject()),
    ]);
    let engine: Vec<u64> = rig
        .commands()
        .iter()
        .filter_map(|c| match c {
            WorkerCommand::Device(DeviceCommand::Eject { seq, .. }) => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(engine.len(), 2);
    assert!(engine[0] < engine[1], "engine seqs only grow: {engine:?}");

    let local = rig.run(vec![disk_eject(engine[0])]);
    assert_eq!(shown_eject(&local), None, "remote eject hidden");
    assert_eq!(shown_eject(&remote.snapshot()), Some(1));

    let local = rig.run(vec![disk_eject(engine[1])]);
    assert_eq!(shown_eject(&local), Some(1));
    assert_eq!(shown_eject(&remote.snapshot()), None);
}
