//! Model-based test of the request lifecycle (q4x76.17, ADR-018): 5000 seeded runs of random
//! interleavings of submit, cancel, queue copy, claim, progress, complete, fail, eviction by
//! queue overflow, worker restart, reducer restart, session open and session end, over 2 to 4
//! sessions and every request kind: actions, scans, handle searches and ejects. The reducer is
//! the real one; the two workers are fakes that behave as `process_detail` and `dbus_udisks` do:
//! each copies the command list, claims each command through `CommandProgress::claim` later, and
//! reports in slot order.
//!
//! Actions run through the real `actions::Gate` and its action task. Each is an `OpenUri` whose
//! opener waits until the model answers it, or a `UnitResetFailed` on one unit whose bus call
//! waits the same way, so the model picks when an action succeeds, fails, or is still running
//! when the reducer stops. `ActionFinished` goes to a queue the model delivers to the reducer
//! later. A session's reader can stall: its action channel is full, so action tasks wait to
//! push their events, and a reducer restart then ends actions whose events are still waiting.
//!
//! A reducer restart takes the path `engine_session` takes between epochs: `Reducer::stopped`
//! (which drops the gate), then a new reducer with the same worker control, sequence and local
//! request channel, which continues the engine ids. The linked sessions end with the old
//! reducer; the workers start a new generation.
//!
//! After every step: each event names a request its own session sent; nothing of a request
//! comes after its end; an action's end comes after its summary and matches it; the end comes
//! from the request's owner (`Busy` and an unclaimed `Lost` from the queue, `Done`, `Failed` and
//! `Cancelled` as the worker or opener reported, a claimed or running `Lost` only from a worker
//! or reducer restart); no view shows another session's request or a running state after the
//! end; the gate holds exactly the actions the model expects. Right after a reducer restart,
//! every request that was active has its end. At the end, after the workers and openers finish
//! everything: every request of an open session got exactly one end, no session has an active
//! request left and the gate has no action in flight.
//!
//! `TL_LIFECYCLE_SEED=<n>` runs one seed and prints its steps.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use arc_swap::ArcSwap;
use asupersync::channel::{mpsc, watch};
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::actions::startup::StartupPaths;
use tl_engine::actions::unit::{BusFuture, JobStream, UnitBus, UnitBusError, UnitBusOverride};
use tl_engine::actions::{
    ActionCtx, ActionOps, ENGINE_STOPPED, Gate, LaunchOutcome, MAX_IN_FLIGHT, Openers, action_end,
};
use tl_engine::client_session::{
    ClientMsg, ClientSessionId, IdKind, RequestEnd, RequestEvent, SessionLink,
};
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::dbus::ManagerConnSlot;
use tl_engine::reducer::apply::QUEUE_MAX;
use tl_engine::reducer::{ActionGate, ActionRoute, AdmissionView, Publisher, Reducer};
use tl_engine::source::{CommandProgress, QueuedCommand, WorkerCommand, WorkerControl};
use tl_engine::update::{DetailsUpdate, DeviceDetailsUpdate, EngineMsg, SourceUpdate};
use tl_model::actions::{
    ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    LaunchSpec, RejectReason,
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
use tl_model::table::{Manager, UnitRef};
use tl_model::time::BootId;
use tl_model::validity::Validity;
use zbus::zvariant::{OwnedValue, Value};

const BOOT: BootId = BootId(0x77);
const RUNS: u64 = 5000;
const DISKS: [&str; 2] = ["sda", "sdb"];
const KINDS: [IdKind; 3] = [IdKind::Scan, IdKind::Search, IdKind::Eject];
/// The model's kind index of actions, after the three `KINDS`.
const ACTION: usize = 3;
const KIND_NAMES: [&str; 4] = ["Scan", "Search", "Eject", "Action"];
/// Room in each session's action channel. A step never fills it; a stalled reader does.
const ACTION_ROOM: usize = 512;
/// The id of the events a stalled reader leaves unread. Client ids start at 1.
const FILLER: ActionId = ActionId(0);

fn key(i: usize) -> ProcessKey {
    let pid = 100 + u32::try_from(i).expect("small");
    ProcessKey {
        start_ticks: u64::from(pid),
        boot_id: BOOT,
        pid,
    }
}

/// splitmix64: a small seeded generator, so a failing seed replays exactly.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).expect("fits")).expect("fits")
    }
}

type RunOutput = Arc<Mutex<Option<(Reducer, Publisher)>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The model's URI or unit name for request `i`: the opener or bus reads the request back from
/// it.
fn uri(i: usize) -> Arc<str> {
    Arc::from(format!("req/{i}"))
}

fn request_index(name: &str) -> usize {
    name.strip_prefix("req/")
        .and_then(|n| n.parse().ok())
        .expect("a model URI")
}

/// The openers and the unit bus the real action task calls. Each `OpenUri` and each
/// `ResetFailedUnit` call waits until the model answers it.
#[derive(Default)]
struct ModelOpeners {
    /// By request index, the answer channel of each call still waiting.
    waiting: Mutex<HashMap<usize, async_channel::Sender<Result<(), ActionError>>>>,
}

impl ModelOpeners {
    fn waiting(&self) -> Vec<usize> {
        let mut out: Vec<usize> = lock(&self.waiting).keys().copied().collect();
        out.sort_unstable();
        out
    }

    fn is_waiting(&self, i: usize) -> bool {
        lock(&self.waiting).contains_key(&i)
    }

    /// The opener of request `i` returns `result`. Its task runs at the next settle.
    fn answer(&self, i: usize, result: Result<(), ActionError>) {
        let tx = lock(&self.waiting).remove(&i).expect("a waiting call");
        tx.try_send(result).expect("room for the answer");
    }

    /// Every waiting call fails: the tasks the stopped reducer's gate aborted wake up.
    fn fail_all(&self) {
        lock(&self.waiting).clear();
    }

    /// A call for request `i` that waits until the model answers it.
    fn wait(&self, i: usize) -> impl Future<Output = Result<(), ActionError>> + use<> {
        let (tx, rx) = async_channel::bounded(1);
        lock(&self.waiting).insert(i, tx);
        async move {
            rx.recv()
                .await
                .unwrap_or(Err(ActionError::PermissionDenied))
        }
    }
}

/// The unit bus of the targeted actions: `ResetFailedUnit` waits for the model's answer.
struct ModelBus(Arc<ModelOpeners>);

impl UnitBus for ModelBus {
    fn call<'a>(
        &'a self,
        _cx: &'a Cx,
        method: &'a str,
        args: &'a [Value<'static>],
        _timeout: std::time::Duration,
    ) -> BusFuture<'a, OwnedValue> {
        assert_eq!(method, "ResetFailedUnit", "the model only resets units");
        let Some(Value::Str(name)) = args.first() else {
            panic!("ResetFailedUnit takes the unit name");
        };
        let answer = self.0.wait(request_index(name.as_str()));
        Box::pin(async move {
            match answer.await {
                Ok(()) => Ok(OwnedValue::from(true)),
                Err(_) => Err(UnitBusError::Closed),
            }
        })
    }

    fn job_removed<'a>(&'a self, _cx: &'a Cx) -> BusFuture<'a, JobStream> {
        panic!("ResetFailedUnit waits for no job")
    }

    fn get_property<'a>(
        &'a self,
        _cx: &'a Cx,
        _unit_path: &'a str,
        _iface: &'a str,
        _name: &'a str,
    ) -> BusFuture<'a, OwnedValue> {
        panic!("ResetFailedUnit reads no property")
    }
}

impl Openers for ModelOpeners {
    fn open_uri(
        &self,
        _cx: &Cx,
        target: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ActionError>> + Send + '_>> {
        Box::pin(self.wait(request_index(target)))
    }

    fn show_in_file_manager(
        &self,
        _cx: &Cx,
        _path: &str,
        _properties: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), ActionError>> + Send + '_>> {
        panic!("the model only opens URIs")
    }

    fn launch(
        &self,
        _cx: &Cx,
        _spec: &LaunchSpec,
    ) -> Pin<Box<dyn Future<Output = Result<LaunchOutcome, ActionError>> + Send + '_>> {
        panic!("the model only opens URIs")
    }
}

struct NoOps;
impl ActionOps for NoOps {}

/// What the gate was given and holds. Shared between the gate in the reducer and the model.
#[derive(Default)]
struct GateLog {
    /// Every route the reducer gave a gate: engine id, client id, events channel.
    routes: Vec<(u64, ActionId, async_channel::Sender<ActionEvent>)>,
    /// `Gate::in_flight` of the current gate after its last call.
    in_flight: usize,
    /// The engine ids of the actions a gate admitted.
    admitted: HashSet<u64>,
}

/// The real `Gate`, with a record of what the reducer gave it. Dropping it drops the gate.
struct Recorded {
    gate: Gate,
    log: Arc<Mutex<GateLog>>,
}

impl ActionGate for Recorded {
    fn submit(&mut self, _cx: &Cx, _req: ActionRequest, _view: &AdmissionView<'_>) {
        panic!("the reducer routes every action");
    }

    fn submit_routed(
        &mut self,
        cx: &Cx,
        req: ActionRequest,
        view: &AdmissionView<'_>,
        route: ActionRoute,
    ) {
        let engine = route.id.0;
        lock(&self.log)
            .routes
            .push((engine, req.id, route.events.clone()));
        let before = self.gate.in_flight();
        self.gate.submit_routed(cx, req, view, route);
        let mut log = lock(&self.log);
        log.in_flight = self.gate.in_flight();
        if log.in_flight > before {
            log.admitted.insert(engine);
        }
    }

    fn finished(&mut self, id: ActionId) -> Option<SourceId> {
        let source = self.gate.finished(id);
        lock(&self.log).in_flight = self.gate.in_flight();
        source
    }
}

/// The reducer in a lab runtime, as in `lab_client_ids.rs`, with the real action gate.
struct Rig {
    lab: LabRuntime,
    root: asupersync::types::RegionId,
    reducer: Option<Reducer>,
    publisher: Option<Publisher>,
    control: watch::Receiver<WorkerControl>,
    _wake_rx: async_channel::Receiver<()>,
    epoch: u32,
    local_requests: async_channel::Sender<RequestEvent>,
    local_actions: async_channel::Sender<ActionEvent>,
    openers: Arc<ModelOpeners>,
    /// The `ActionFinished` messages the action tasks sent, which the model delivers.
    finished: mpsc::Receiver<EngineMsg>,
    finished_tx: mpsc::Sender<EngineMsg>,
    gate_log: Arc<Mutex<GateLog>>,
}

impl Rig {
    fn new(
        local_requests: async_channel::Sender<RequestEvent>,
        local_actions: async_channel::Sender<ActionEvent>,
    ) -> Self {
        let (control_tx, control) = watch::channel(WorkerControl::default());
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let publisher = Publisher {
            latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
            wake_tx,
            stop_publishing: false,
        };
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (finished_tx, finished) = mpsc::channel(256);
        let mut rig = Rig {
            lab,
            root,
            reducer: None,
            publisher: Some(publisher),
            control,
            _wake_rx: wake_rx,
            epoch: 1,
            local_requests,
            local_actions,
            openers: Arc::default(),
            finished,
            finished_tx,
            gate_log: Arc::default(),
        };
        let mut reducer = Reducer::new(1, BOOT, control_tx, rig.local_actions.clone());
        reducer
            .clients
            .set_local_requests(rig.local_requests.clone());
        reducer.action_gate = Some(rig.gate());
        rig.reducer = Some(reducer);
        rig
    }

    /// A new real gate, as the engine's action gate hook makes one per reducer.
    fn gate(&self) -> Box<dyn ActionGate> {
        let openers: Arc<dyn Openers> = self.openers.clone();
        let bus = Arc::new(ModelBus(Arc::clone(&self.openers)));
        let pick: UnitBusOverride = Arc::new(move |_| Some(Arc::clone(&bus) as Arc<dyn UnitBus>));
        let ctx = ActionCtx {
            events: self.local_actions.clone(),
            msgs: self.finished_tx.clone(),
            ops: Arc::new(NoOps),
            boot_id: BOOT,
            uid: 1000,
            config: Arc::new(EngineConfig::new(
                EngineMode::Full,
                PathBuf::from("/nonexistent"),
            )),
            systemd_system: ManagerConnSlot::new(),
            systemd_user: ManagerConnSlot::new(),
            openers: Some(openers),
            helper: None,
            destroy_support_sent: AtomicBool::new(false),
            unit_bus_override: Some(pick),
            startup_paths: StartupPaths::from_env(),
        };
        lock(&self.gate_log).in_flight = 0;
        Box::new(Recorded {
            gate: Gate::new(Arc::new(ctx)),
            log: Arc::clone(&self.gate_log),
        })
    }

    fn reducer(&self) -> &Reducer {
        self.reducer.as_ref().expect("the reducer is back")
    }

    /// Runs every task that is ready: the action tasks.
    fn settle(&mut self) {
        self.lab.run_until_idle();
    }

    /// Applies `msgs` as one batch and publishes once. The action tasks run too.
    fn run(&mut self, msgs: Vec<EngineMsg>) {
        if msgs.is_empty() {
            self.settle();
            return;
        }
        let (tx, mut rx) = mpsc::channel::<EngineMsg>(msgs.len());
        for msg in msgs {
            tx.try_send(msg).expect("room for every message");
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
                *lock(&slot) = Some((reducer, publisher));
            })
            .expect("task is created");
        self.lab.scheduler.lock().schedule(task, 0);
        self.lab.run_until_idle();
        let (reducer, publisher) = lock(&out).take().expect("run returned");
        self.reducer = Some(reducer);
        self.publisher = Some(publisher);
    }

    /// The reducer stops and a new one starts, as `engine_session` does between epochs:
    /// `Reducer::stopped` ends every active request and drops the gate; the worker control, the
    /// sequence and the local request channel carry over, and the engine ids continue.
    fn restart_reducer(&mut self) {
        let mut old = self.reducer.take().expect("one run at a time");
        old.stopped();
        let control =
            std::mem::replace(&mut old.control, watch::channel(WorkerControl::default()).0);
        self.epoch += 1;
        let mut next = Reducer::new(self.epoch, BOOT, control, self.local_actions.clone());
        next.sequence = old.sequence;
        next.clients.set_local_requests(self.local_requests.clone());
        next.clients.resume_engine_ids(old.clients.last_engine_id());
        drop(old);
        next.action_gate = Some(self.gate());
        self.reducer = Some(next);
    }

    /// The `ActionFinished` messages sent so far.
    fn take_finished(&mut self) -> Vec<EngineMsg> {
        std::iter::from_fn(|| self.finished.try_recv().ok()).collect()
    }

    fn progress(&self) -> Arc<CommandProgress> {
        Arc::clone(&self.control.borrow_and_clone().progress)
    }

    fn commands(&self) -> Arc<[QueuedCommand]> {
        self.control.borrow_and_clone().commands
    }
}

/// One session as the test sees it.
struct Session {
    id: ClientSessionId,
    requests: async_channel::Receiver<RequestEvent>,
    /// The session's action events; `None` once it ended, as the server drops it then.
    actions: Option<async_channel::Receiver<ActionEvent>>,
    /// The sender the reducer must route the session's actions to.
    actions_tx: async_channel::Sender<ActionEvent>,
    /// `None` for the local session, which publishes through the rig's `Publisher`.
    link: Option<SessionLink>,
    /// Keeps the wake channel open: a closed one ends the session.
    _wake: Option<async_channel::Receiver<()>>,
    open: bool,
    /// The reader stalled: its action channel is full and the model reads nothing of it.
    stalled: bool,
    /// The last client id used per kind index; ids start at 1 in every session, so they collide.
    last_id: [u64; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Slot {
    Scan(usize),
    Search,
    Eject(usize),
    /// A signal to one process; actions have no slot of their own.
    Action,
}

impl Slot {
    /// `None` for actions, which keep no mapping in the reducer.
    fn kind(self) -> Option<IdKind> {
        match self {
            Slot::Scan(_) => Some(IdKind::Scan),
            Slot::Search => Some(IdKind::Search),
            Slot::Eject(_) => Some(IdKind::Eject),
            Slot::Action => None,
        }
    }

    /// The model's kind index: the `KINDS` index, or `ACTION`.
    fn index(self) -> usize {
        self.kind().map_or(ACTION, |k| {
            KINDS.iter().position(|x| *x == k).expect("kind")
        })
    }
}

/// One request a session sent.
struct Req {
    session: usize,
    slot: Slot,
    client: u64,
    /// The engine id it got, read right after admission. `None`: refused, or lost in the
    /// batch that admitted it.
    engine: Option<u64>,
    end: Option<RequestEnd>,
    /// An action's summary: its final state with no target.
    summary: Option<ActionState>,
}

/// What the fake workers did, by engine id.
#[derive(Default)]
struct WorkerLog {
    claimed: HashSet<u64>,
    /// The first end a worker reported in a message.
    sent: HashMap<u64, RequestEnd>,
    /// Handle searches a newer one replaced without a message.
    replaced: HashSet<u64>,
    /// Claimed, not ended by a message, when their worker restarted.
    lost_in_restart: HashSet<u64>,
}

/// A fake worker's place in the command list.
#[derive(Default)]
struct Queue {
    last_seq: u64,
    copy: Vec<QueuedCommand>,
}

/// The fake `process_detail` and `dbus_udisks` workers.
#[derive(Default)]
struct Workers {
    detail: Queue,
    udisks: Queue,
    /// Running scan per key.
    scans: HashMap<usize, u64>,
    search: Option<u64>,
    /// Running eject per disk.
    ejects: [Option<u64>; 2],
    /// Each disk's last eject, as every `Disks` list repeats it.
    disk_last: [Option<(u64, EjectState)>; 2],
}

fn scan_msg(k: usize, engine: u64, phase: ScanPhase) -> EngineMsg {
    EngineMsg::Source(SourceUpdate::Details(DetailsUpdate::Tab {
        target: DetailTarget::Process(key(k)),
        tab: DetailTab::Strings,
        payload: Arc::new(TabPayload::Strings(Arc::new(StringScanState {
            scan: ScanId(engine),
            progress_pct: 50,
            found: 0,
            chunks: Arc::from([]),
            phase,
        }))),
        validity: Validity::Valid,
    }))
}

fn search_msg(engine: u64, phase: HandlePhase) -> EngineMsg {
    EngineMsg::Source(SourceUpdate::Details(DetailsUpdate::HandleSearch(
        Arc::new(HandleSearchState {
            search: SearchId(engine),
            phase,
            progress_pct: 10,
            total_handles: 0,
            chunks: Arc::from([]),
            searched: 0,
            total: 1,
            skipped: 0,
        }),
    )))
}

fn disks_msg(last: &[Option<(u64, EjectState)>; 2]) -> EngineMsg {
    let entries = DISKS
        .iter()
        .zip(last)
        .enumerate()
        .map(|(i, (name, eject))| {
            let identity = DeviceIdentity::Disk {
                major: 8,
                minor: u32::try_from(i * 16).expect("small"),
                name: Arc::from(*name),
                wwn_or_serial: None,
            };
            let details = DiskDetails {
                ejectable: true,
                eject: eject.clone(),
                ..DiskDetails::default()
            };
            (identity, details)
        })
        .collect();
    EngineMsg::Source(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks {
        from: SourceId::Udisks,
        entries,
    }))
}

impl Workers {
    fn queue(&mut self, target: SourceId) -> &mut Queue {
        if target == SourceId::Udisks {
            &mut self.udisks
        } else {
            &mut self.detail
        }
    }

    /// The worker copies the commands for it after its last copy, as `source::run` and
    /// `Queue::take` do.
    fn copy(&mut self, target: SourceId, commands: &[QueuedCommand]) {
        let q = self.queue(target);
        q.copy.extend(
            commands
                .iter()
                .filter(|c| c.seq > q.last_seq && c.target == target)
                .cloned(),
        );
        if let Some(newest) = commands.last() {
            q.last_seq = q.last_seq.max(newest.seq);
        }
    }

    /// The worker claims and runs its copied commands in order; one the reducer evicted is
    /// skipped. Returns the reports it sends.
    fn claim(
        &mut self,
        target: SourceId,
        progress: &CommandProgress,
        log: &mut WorkerLog,
    ) -> Vec<EngineMsg> {
        let copy = std::mem::take(&mut self.queue(target).copy);
        let mut out = Vec::new();
        for q in copy {
            if !progress.claim(target, q.seq) {
                continue;
            }
            match q.command {
                WorkerCommand::StringScan { scan, key: k, .. } => {
                    log.claimed.insert(scan.0);
                    let k = usize::try_from(k.pid - 100).expect("model key");
                    if let Some(old) = self.scans.insert(k, scan.0) {
                        log.sent.entry(old).or_insert(RequestEnd::Cancelled);
                        out.push(EngineMsg::Source(SourceUpdate::Details(
                            DetailsUpdate::ScanDropped(ScanId(old)),
                        )));
                    }
                }
                WorkerCommand::CancelStringScan(scan) => {
                    if let Some((&k, _)) = self.scans.iter().find(|(_, e)| **e == scan.0) {
                        self.scans.remove(&k);
                        log.sent.entry(scan.0).or_insert(RequestEnd::Cancelled);
                        out.push(scan_msg(k, scan.0, ScanPhase::Cancelled));
                    }
                }
                WorkerCommand::HandleSearch { search, .. } => {
                    log.claimed.insert(search.0);
                    if let Some(old) = self.search.replace(search.0) {
                        log.replaced.insert(old);
                    }
                }
                WorkerCommand::CancelHandleSearch(search) => {
                    if self.search == Some(search.0) {
                        self.search = None;
                        log.sent.entry(search.0).or_insert(RequestEnd::Cancelled);
                        out.push(search_msg(search.0, HandlePhase::Stopped));
                    }
                }
                WorkerCommand::Device(DeviceCommand::Eject { seq, disk }) => {
                    log.claimed.insert(seq);
                    let d = DISKS.iter().position(|n| **n == *disk).expect("model disk");
                    if self.ejects[d].is_some() {
                        log.sent.entry(seq).or_insert(RequestEnd::Cancelled);
                        out.push(EngineMsg::Source(SourceUpdate::DeviceDetails(
                            DeviceDetailsUpdate::EjectDropped(seq),
                        )));
                    } else {
                        self.ejects[d] = Some(seq);
                        self.disk_last[d] = Some((seq, EjectState::Running));
                        out.push(disks_msg(&self.disk_last));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Engine ids of the running requests, in a fixed order.
    fn running(&self) -> Vec<(Slot, u64)> {
        let mut scans: Vec<_> = self
            .scans
            .iter()
            .map(|(k, e)| (Slot::Scan(*k), *e))
            .collect();
        scans.sort_by_key(|(_, e)| *e);
        scans
            .into_iter()
            .chain(self.search.map(|e| (Slot::Search, e)))
            .chain(
                self.ejects
                    .iter()
                    .enumerate()
                    .filter_map(|(d, e)| e.map(|e| (Slot::Eject(d), e))),
            )
            .collect()
    }

    /// A progress report, or the end `end` when it is set, of running request `engine`.
    fn report(
        &mut self,
        slot: Slot,
        engine: u64,
        end: Option<RequestEnd>,
        log: &mut WorkerLog,
    ) -> EngineMsg {
        if let Some(end) = end {
            log.sent.entry(engine).or_insert(end);
        }
        match slot {
            Slot::Scan(k) => {
                let phase = match end {
                    None => ScanPhase::Scanning,
                    Some(RequestEnd::Done) => ScanPhase::Finished,
                    Some(_) => ScanPhase::Failed(Arc::from("read")),
                };
                if end.is_some() {
                    self.scans.remove(&k);
                }
                scan_msg(k, engine, phase)
            }
            Slot::Search => {
                let phase = match end {
                    None => HandlePhase::Searching,
                    Some(RequestEnd::Done) => HandlePhase::Done,
                    Some(_) => HandlePhase::Failed,
                };
                if end.is_some() {
                    self.search = None;
                }
                search_msg(engine, phase)
            }
            Slot::Eject(d) => {
                let state = match end {
                    None => EjectState::Running,
                    Some(RequestEnd::Done) => EjectState::Done,
                    Some(_) => EjectState::Failed(Arc::from("busy")),
                };
                if end.is_some() {
                    self.ejects[d] = None;
                }
                self.disk_last[d] = Some((engine, state));
                disks_msg(&self.disk_last)
            }
            Slot::Action => unreachable!("the workers run no actions"),
        }
    }

    /// The worker stops; its next generation resumes after the last claimed command and
    /// reports the requests the old one held as lost.
    fn restart(
        &mut self,
        target: SourceId,
        progress: &CommandProgress,
        log: &mut WorkerLog,
    ) -> EngineMsg {
        let held: Vec<u64> = if target == SourceId::Udisks {
            self.ejects.iter_mut().filter_map(Option::take).collect()
        } else {
            let mut held: Vec<u64> = self.scans.drain().map(|(_, e)| e).collect();
            held.extend(self.search.take());
            held
        };
        log.lost_in_restart.extend(held);
        // A search the old generation replaced silently was never reported either.
        if target != SourceId::Udisks {
            let silent: Vec<u64> = log
                .replaced
                .iter()
                .filter(|e| !log.sent.contains_key(e))
                .copied()
                .collect();
            log.lost_in_restart.extend(silent);
        }
        let through = progress.started(target);
        *self.queue(target) = Queue {
            last_seq: through,
            copy: Vec::new(),
        };
        if target == SourceId::Udisks {
            EngineMsg::Source(SourceUpdate::DeviceDetails(
                DeviceDetailsUpdate::EjectsLost { through },
            ))
        } else {
            EngineMsg::Source(SourceUpdate::Details(DetailsUpdate::RequestsLost {
                through,
            }))
        }
    }
}

/// One seeded run.
struct Model {
    seed: u64,
    rng: Rng,
    rig: Rig,
    sessions: Vec<Session>,
    next_session: u32,
    reqs: Vec<Req>,
    /// (session index, kind index, client id) to `reqs` index.
    by_client: HashMap<(usize, usize, u64), usize>,
    workers: Workers,
    log: WorkerLog,
    /// Admitted actions whose `ActionFinished` has not reached the current gate.
    in_gate: HashSet<usize>,
    /// Actions in a gate when its reducer stopped: they may end `Lost`.
    in_stopped_gate: HashSet<usize>,
    /// Actions refused while the gate held `MAX_IN_FLIGHT`: they may end `Busy`.
    gate_full: HashSet<usize>,
    /// The end each answered opener call gives its action.
    answered: HashMap<usize, RequestEnd>,
    /// Actions refused or stopped while their session's reader stalled: the gate's best-effort
    /// summary found no room, so the end may come alone.
    summary_dropped: HashSet<usize>,
    steps: Vec<String>,
    trace: bool,
}

/// The model's kind index of `kind`: the `KINDS` index, or `ACTION`.
fn kind_index(kind: IdKind) -> usize {
    KINDS.iter().position(|k| *k == kind).unwrap_or(ACTION)
}

/// What the workers report when a reducer starts: the process targets and the disks, with
/// results of no session (engine id 0).
fn setup_msgs() -> Vec<EngineMsg> {
    let mut setup: Vec<EngineMsg> = (0..3)
        .map(|k| scan_msg(k, 0, ScanPhase::Finished))
        .collect();
    setup.push(disks_msg(&[None, None]));
    setup
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

impl Model {
    fn new(seed: u64, trace: bool) -> Self {
        let (local_tx, local_rx) = async_channel::unbounded();
        let (actions_tx, actions) = async_channel::bounded(ACTION_ROOM);
        let rig = Rig::new(local_tx, actions_tx.clone());
        let mut m = Model {
            seed,
            rng: Rng(seed),
            rig,
            sessions: vec![Session {
                id: ClientSessionId::LOCAL,
                requests: local_rx,
                actions: Some(actions),
                actions_tx,
                link: None,
                _wake: None,
                open: true,
                stalled: false,
                last_id: [0; 4],
            }],
            next_session: 1,
            reqs: Vec::new(),
            by_client: HashMap::new(),
            workers: Workers::default(),
            log: WorkerLog::default(),
            in_gate: HashSet::new(),
            in_stopped_gate: HashSet::new(),
            gate_full: HashSet::new(),
            answered: HashMap::new(),
            summary_dropped: HashSet::new(),
            steps: Vec::new(),
            trace,
        };
        let mut setup = setup_msgs();
        let extra = 1 + m.rng.below(3);
        for _ in 0..extra {
            setup.push(m.open_session());
        }
        m.step("setup", setup);
        m
    }

    fn open_session(&mut self) -> EngineMsg {
        let id = ClientSessionId(self.next_session);
        self.next_session += 1;
        let (wake_tx, wake) = async_channel::bounded(1);
        let (events_tx, actions) = async_channel::bounded(ACTION_ROOM);
        let (requests_tx, requests) = async_channel::unbounded();
        let link = SessionLink {
            latest: Arc::new(ArcSwap::from_pointee(Snapshot::empty(BOOT, 1))),
            wake_tx,
            events_tx: events_tx.clone(),
            requests_tx,
        };
        let msg = EngineMsg::Client(ClientMsg::Open {
            id,
            link: link.clone(),
        });
        self.sessions.push(Session {
            id,
            requests,
            actions: Some(actions),
            actions_tx: events_tx,
            link: Some(link),
            _wake: Some(wake),
            open: true,
            stalled: false,
            last_id: [0; 4],
        });
        msg
    }

    fn fail(&self, why: &str) -> ! {
        panic!(
            "seed {}: {why}\nsteps:\n{}",
            self.seed,
            self.steps.join("\n")
        );
    }

    /// Runs one batch, then reads every session's events and views and checks them.
    fn step(&mut self, what: &str, msgs: Vec<EngineMsg>) {
        self.steps.push(what.to_owned());
        if self.trace {
            eprintln!("{what}");
        }
        self.rig.run(msgs);
        self.read_events();
        self.check_views();
        self.check_gate();
    }

    /// Reads each session's action events, then its request ends. An action's events are all
    /// sent before its end, so reading them first sees its summary before its end.
    fn read_events(&mut self) {
        for s in 0..self.sessions.len() {
            if self.sessions[s].stalled {
                continue;
            }
            let actions: Vec<ActionEvent> = match &self.sessions[s].actions {
                Some(rx) => std::iter::from_fn(|| rx.try_recv().ok()).collect(),
                None => Vec::new(),
            };
            for ev in actions {
                self.action_event(s, &ev);
            }
            while let Ok(ev) = self.sessions[s].requests.try_recv() {
                if !self.sessions[s].open {
                    // An action outlives its session; the server drops what it sends after.
                    if ev.kind == IdKind::Action {
                        continue;
                    }
                    self.fail(&format!("event {ev:?} after session {s} ended"));
                }
                let Some(&i) = self.by_client.get(&(s, kind_index(ev.kind), ev.id)) else {
                    self.fail(&format!(
                        "session {s} got {ev:?} for a request it never sent"
                    ));
                };
                if let Some(first) = self.reqs[i].end {
                    self.fail(&format!(
                        "session {s} got {ev:?} after its end {first:?}: two terminal events"
                    ));
                }
                if ev.kind == IdKind::Action {
                    self.check_action_end(i, ev.end);
                } else {
                    self.check_owner(i, ev.end);
                }
                self.reqs[i].end = Some(ev.end);
            }
        }
    }

    /// Action event `ev` reached session `s`: one of its own actions, before its end, with one
    /// summary at most.
    fn action_event(&mut self, s: usize, ev: &ActionEvent) {
        if ev.id == FILLER {
            return;
        }
        let Some(&i) = self.by_client.get(&(s, ACTION, ev.id.0)) else {
            self.fail(&format!(
                "session {s} got {ev:?} for an action it never sent"
            ));
        };
        if let Some(first) = self.reqs[i].end {
            self.fail(&format!(
                "session {s} got {ev:?} after the action's end {first:?}"
            ));
        }
        if ev.target.is_some() || !ev.state.is_final() {
            return;
        }
        if let Some(first) = &self.reqs[i].summary {
            self.fail(&format!(
                "session {s} got {ev:?} after the action's summary {first:?}"
            ));
        }
        self.reqs[i].summary = Some(ev.state.clone());
    }

    /// Action `i` ended `end`: after its summary, which names the same end, from its one owner.
    fn check_action_end(&self, i: usize, end: RequestEnd) {
        let req = &self.reqs[i];
        let alone = match end {
            RequestEnd::Lost => self.in_stopped_gate.contains(&i),
            RequestEnd::Busy => self.gate_full.contains(&i),
            _ => false,
        };
        if req.summary.is_none() && alone && self.summary_dropped.contains(&i) {
            return;
        }
        let Some(summary) = &req.summary else {
            self.fail(&format!(
                "action {} of session {} ended {end:?} before its summary",
                req.client, req.session
            ));
        };
        let ok = match end {
            RequestEnd::Lost => {
                self.in_stopped_gate.contains(&i)
                    && *summary == ActionState::OutcomeUnknown(Arc::from(ENGINE_STOPPED))
            }
            RequestEnd::Busy => {
                self.gate_full.contains(&i) && *summary == ActionState::Rejected(RejectReason::Busy)
            }
            RequestEnd::Done | RequestEnd::Failed => {
                self.answered.get(&i) == Some(&end) && action_end(summary) == end
            }
            RequestEnd::Cancelled => false,
        };
        if !ok {
            self.fail(&format!(
                "action {} of session {} (engine {:?}) ended {end:?} with summary {summary:?}, \
                 which is not its owner's end",
                req.client, req.session, req.engine
            ));
        }
    }

    /// The gate holds exactly the admitted actions whose `ActionFinished` it has not had.
    fn check_gate(&self) {
        let held = lock(&self.rig.gate_log).in_flight;
        if held != self.in_gate.len() {
            self.fail(&format!(
                "the gate holds {held} actions, the model {:?}",
                self.in_gate
            ));
        }
    }

    /// No worker runs a request that already ended: the queue owner ended it, so it must not
    /// run (the copy-evict-claim race).
    fn check_claims(&self, before: &HashSet<u64>) {
        for engine in self.log.claimed.difference(before) {
            if let Some(req) = self.reqs.iter().find(|r| r.engine == Some(*engine))
                && let Some(end) = req.end
            {
                self.fail(&format!(
                    "a worker claimed request {:?} {} of session {} (engine {engine}) after it \
                     ended {end:?}",
                    req.slot, req.client, req.session
                ));
            }
        }
    }

    /// The end of request `i` comes from its one owner.
    fn check_owner(&self, i: usize, end: RequestEnd) {
        let req = &self.reqs[i];
        let claimed = req.engine.is_some_and(|e| self.log.claimed.contains(&e));
        let ok = match end {
            RequestEnd::Busy => req.engine.is_none(),
            RequestEnd::Lost => {
                !claimed
                    || req
                        .engine
                        .is_some_and(|e| self.log.lost_in_restart.contains(&e))
            }
            RequestEnd::Done | RequestEnd::Failed => {
                claimed && req.engine.and_then(|e| self.log.sent.get(&e)) == Some(&end)
            }
            RequestEnd::Cancelled => {
                claimed
                    && req.engine.is_some_and(|e| {
                        self.log.sent.get(&e) == Some(&RequestEnd::Cancelled)
                            || self.log.replaced.contains(&e)
                    })
            }
        };
        if !ok {
            self.fail(&format!(
                "request {:?} {} of session {} (engine {:?}, claimed {claimed}) ended {end:?}, \
                 which is not its owner's end",
                req.slot, req.client, req.session, req.engine
            ));
        }
    }

    fn snapshot(&self, s: usize) -> Arc<Snapshot> {
        match &self.sessions[s].link {
            Some(link) => link.latest.load_full(),
            None => self
                .rig
                .publisher
                .as_ref()
                .expect("the publisher is back")
                .latest
                .load_full(),
        }
    }

    /// Each view shows only the session's own requests, and none running after its end.
    fn check_views(&self) {
        for s in 0..self.sessions.len() {
            if !self.sessions[s].open {
                continue;
            }
            let snap = self.snapshot(s);
            let mut shown = Vec::new();
            for k in 0..3 {
                let Some(detail) = snap.details.targets.get(&DetailTarget::Process(key(k))) else {
                    continue;
                };
                if let Some(TabPayload::Strings(st)) =
                    detail.tab(DetailTab::Strings).payload.as_deref()
                {
                    shown.push((Slot::Scan(k), st.scan.0, st.phase == ScanPhase::Scanning));
                }
            }
            if let Some(st) = &snap.details.handle_search {
                let running = matches!(
                    st.phase,
                    HandlePhase::Initializing | HandlePhase::Retrieving | HandlePhase::Searching
                );
                shown.push((Slot::Search, st.search.0, running));
            }
            for (idx, disk) in snap.device_details.disks.iter() {
                let Some((client, state)) = &disk.eject else {
                    continue;
                };
                let d = snap
                    .devices
                    .entries
                    .iter()
                    .find(|e| e.idx == *idx)
                    .and_then(|e| match &e.identity {
                        DeviceIdentity::Disk { name, .. } => {
                            DISKS.iter().position(|n| **n == **name)
                        }
                        _ => None,
                    })
                    .expect("a model disk");
                shown.push((Slot::Eject(d), *client, *state == EjectState::Running));
            }
            for (slot, client, running) in shown {
                let Some(&i) = self.by_client.get(&(s, slot.index(), client)) else {
                    self.fail(&format!(
                        "session {s} shows {slot:?} id {client}, which it never sent"
                    ));
                };
                let req = &self.reqs[i];
                if req.slot != slot {
                    self.fail(&format!(
                        "session {s} shows id {client} in {slot:?}, sent for {:?}",
                        req.slot
                    ));
                }
                if running && req.end.is_some() {
                    self.fail(&format!(
                        "session {s} shows id {client} running after its end {:?}",
                        req.end
                    ));
                }
            }
        }
    }

    fn open_sessions(&self) -> Vec<usize> {
        (0..self.sessions.len())
            .filter(|s| self.sessions[*s].open)
            .collect()
    }

    fn random_slot(&mut self) -> Slot {
        match self.rng.below(4) {
            0 => Slot::Scan(self.rng.below(3)),
            1 => Slot::Search,
            2 => Slot::Eject(self.rng.below(2)),
            _ => Slot::Action,
        }
    }

    /// The command for a new request of session `s` in `slot`, recorded in the model.
    fn submit(&mut self, s: usize, slot: Slot) -> (EngineMsg, usize) {
        let kind = slot.index();
        self.sessions[s].last_id[kind] += 1;
        let client = self.sessions[s].last_id[kind];
        let i = self.reqs.len();
        let cmd = match slot {
            Slot::Scan(k) => Command::StringScan {
                scan: ScanId(client),
                key: key(k),
                opts: StringScanOpts::default(),
            },
            Slot::Search => Command::HandleSearch {
                search: SearchId(client),
                pattern: Arc::from("x"),
                include_maps: false,
                privileged: false,
            },
            Slot::Eject(d) => Command::Device(DeviceCommand::Eject {
                seq: client,
                disk: Arc::from(DISKS[d]),
            }),
            // Half the actions have a target, so a target event can wait for room too.
            Slot::Action if self.rng.below(2) == 0 => Command::Action(ActionRequest {
                id: ActionId(client),
                kind: ActionKind::OpenUri(uri(i)),
                targets: Default::default(),
                privileged: false,
            }),
            Slot::Action => Command::Action(ActionRequest {
                id: ActionId(client),
                kind: ActionKind::UnitResetFailed,
                targets: [ActionTarget::Unit(UnitRef {
                    manager: Manager::System,
                    name: uri(i),
                })]
                .into_iter()
                .collect(),
                privileged: false,
            }),
        };
        self.reqs.push(Req {
            session: s,
            slot,
            client,
            engine: None,
            end: None,
            summary: None,
        });
        self.by_client.insert((s, kind, client), i);
        (on(self.sessions[s].id, cmd), i)
    }

    /// Submits `n` requests of session `s` in one batch and reads the engine ids they got.
    fn submit_batch(&mut self, s: usize, slots: Vec<Slot>) {
        let (msgs, idx): (Vec<_>, Vec<_>) = slots.iter().map(|slot| self.submit(s, *slot)).unzip();
        let what = format!("session {s} submits {slots:?}");
        self.steps.push(what.clone());
        if self.trace {
            eprintln!("{what}");
        }
        self.rig.run(msgs);
        let id = self.sessions[s].id;
        for i in idx {
            let req = &self.reqs[i];
            let engine = match req.slot.kind() {
                Some(kind) => self
                    .rig
                    .reducer()
                    .clients
                    .get(id)
                    .and_then(|session| session.engine_id(kind, req.client)),
                None => Some(self.action_route(s, req.client)),
            };
            if let Some(e) = engine
                && self.reqs.iter().any(|r| r.engine == Some(e))
            {
                self.fail(&format!("engine id {e} given out twice"));
            }
            self.reqs[i].engine = engine;
            if self.reqs[i].slot == Slot::Action {
                let log = lock(&self.rig.gate_log);
                let admitted = engine.is_some_and(|e| log.admitted.contains(&e));
                let full = log.in_flight == MAX_IN_FLIGHT;
                drop(log);
                if admitted {
                    // Its task waits at its opener or bus call, or to push `Started`.
                    let waits = self.sessions[s].stalled || self.rig.openers.is_waiting(i);
                    if !waits {
                        self.fail(&format!("admitted action {i} does not wait at its call"));
                    }
                    self.in_gate.insert(i);
                } else if full {
                    self.gate_full.insert(i);
                    if self.sessions[s].stalled {
                        self.summary_dropped.insert(i);
                    }
                }
            }
        }
        self.read_events();
        self.check_views();
        self.check_gate();
    }

    /// The engine id the reducer gave session `s`'s action `client`. Fails unless the gate got
    /// it exactly once, routed to the session's own channel.
    fn action_route(&self, s: usize, client: u64) -> u64 {
        let st = lock(&self.rig.gate_log);
        let own = &self.sessions[s].actions_tx;
        let mine: Vec<u64> = st
            .routes
            .iter()
            .filter(|(_, id, events)| id.0 == client && events.same_channel(own))
            .map(|(engine, _, _)| *engine)
            .collect();
        if mine.len() != 1 {
            drop(st);
            self.fail(&format!(
                "action {client} of session {s} reached the gate {} times on its own channel",
                mine.len()
            ));
        }
        mine[0]
    }

    fn worker_step(&mut self) {
        let target = if self.rng.below(2) == 0 {
            SourceId::ProcessDetail
        } else {
            SourceId::Udisks
        };
        let progress = self.rig.progress();
        match self.rng.below(3) {
            0 => {
                let commands = self.rig.commands();
                self.workers.copy(target, &commands);
                self.steps.push(format!("{target:?} copies the queue"));
            }
            1 => {
                let before = self.log.claimed.clone();
                let out = self.workers.claim(target, &progress, &mut self.log);
                self.check_claims(&before);
                self.step(&format!("{target:?} claims its copy"), out);
            }
            _ => {
                let msg = self.workers.restart(target, &progress, &mut self.log);
                self.step(&format!("{target:?} restarts"), vec![msg]);
            }
        }
    }

    /// An opener call returns, or the `ActionFinished` messages sent so far reach the reducer.
    fn action_step(&mut self) {
        let waiting = self.rig.openers.waiting();
        if waiting.is_empty() || self.rng.below(3) == 0 {
            self.deliver_finished("");
            return;
        }
        let i = waiting[self.rng.below(waiting.len())];
        let (end, result) = if self.rng.below(3) == 0 {
            (RequestEnd::Failed, Err(ActionError::PermissionDenied))
        } else {
            (RequestEnd::Done, Ok(()))
        };
        self.rig.openers.answer(i, result);
        self.answered.insert(i, end);
        // Half the time its task runs now. Otherwise it runs with the next batch, and a reducer
        // restart before that ends the action as `Lost`.
        if self.rng.below(2) == 0 {
            self.step(
                &format!("the opener of action {i} returns {end:?}"),
                Vec::new(),
            );
        } else {
            self.steps.push(format!(
                "the opener of action {i} returns {end:?}; its task waits"
            ));
        }
    }

    /// The `ActionFinished` messages sent so far reach the reducer, which frees their gate slots.
    fn deliver_finished(&mut self, when: &str) {
        let msgs = self.rig.take_finished();
        let done: Vec<usize> = msgs
            .iter()
            .filter_map(|msg| match msg {
                EngineMsg::ActionFinished { id } => self
                    .reqs
                    .iter()
                    .position(|r| r.slot == Slot::Action && r.engine == Some(id.0)),
                _ => None,
            })
            .collect();
        for i in &done {
            self.in_gate.remove(i);
        }
        self.step(&format!("{when}ActionFinished for {done:?} arrives"), msgs);
    }

    /// The reducer stops and a new one starts. Every request active then ends right away; the
    /// linked sessions end with the old reducer, and the workers start a new generation.
    fn reducer_restarts(&mut self) {
        self.steps.push("the reducer restarts".to_owned());
        if self.trace {
            eprintln!("the reducer restarts");
        }
        let progress = self.rig.progress();
        for target in [SourceId::ProcessDetail, SourceId::Udisks] {
            // The new generation sends no `RequestsLost`: the reducer that knew them is gone.
            let _ = self.workers.restart(target, &progress, &mut self.log);
        }
        self.workers.disk_last = [None, None];
        for i in &self.in_gate {
            if self.sessions[self.reqs[*i].session].stalled {
                self.summary_dropped.insert(*i);
            }
        }
        self.in_stopped_gate.extend(self.in_gate.drain());
        let active: Vec<usize> = (0..self.reqs.len())
            .filter(|i| self.sessions[self.reqs[*i].session].open && self.reqs[*i].end.is_none())
            .collect();
        self.rig.restart_reducer();
        // The stalled readers read on, so action tasks that waited for room wake up after their
        // ends. They must push nothing more.
        self.unstall_all();
        self.read_events();
        // The aborted action tasks wake up; they must not end their actions again.
        self.rig.openers.fail_all();
        self.rig.settle();
        self.read_events();
        for i in active {
            let req = &self.reqs[i];
            if req.end.is_none() {
                self.fail(&format!(
                    "request {i} ({:?} {} of session {}, engine {:?}) got no end when the \
                     reducer stopped",
                    req.slot, req.client, req.session, req.engine
                ));
            }
        }
        for s in 1..self.sessions.len() {
            self.sessions[s].open = false;
            self.sessions[s].actions = None;
        }
        self.step("the new reducer starts", setup_msgs());
    }

    fn one_op(&mut self) {
        let open = self.open_sessions();
        let s = open[self.rng.below(open.len())];
        match self.rng.below(118) {
            0..=29 => {
                let n = 1 + self.rng.below(3);
                let slots = (0..n).map(|_| self.random_slot()).collect();
                self.submit_batch(s, slots);
            }
            30..=31 => {
                // A burst past `ACTIVE_MAX` of one kind: Busy, and queue overflow.
                let kind = self.random_slot();
                let slots = (0..66 + self.rng.below(6))
                    .map(|_| match kind {
                        Slot::Scan(_) => Slot::Scan(self.rng.below(3)),
                        other => other,
                    })
                    .collect();
                self.submit_batch(s, slots);
            }
            32..=39 => {
                let kind = self.rng.below(2);
                let last = self.sessions[s].last_id[kind];
                if last == 0 {
                    return;
                }
                let client = 1 + self.rng.next() % last;
                let cmd = if kind == 0 {
                    Command::CancelStringScan(ScanId(client))
                } else {
                    Command::CancelHandleSearch(SearchId(client))
                };
                let id = self.sessions[s].id;
                self.step(
                    &format!("session {s} cancels {kind} {client}"),
                    vec![on(id, cmd)],
                );
            }
            40..=69 => self.worker_step(),
            70..=89 => {
                let running = self.workers.running();
                if running.is_empty() {
                    return;
                }
                let (slot, engine) = running[self.rng.below(running.len())];
                let end = match self.rng.below(4) {
                    0 | 1 => None,
                    2 => Some(RequestEnd::Done),
                    _ => Some(RequestEnd::Failed),
                };
                let msg = self.workers.report(slot, engine, end, &mut self.log);
                self.step(
                    &format!("worker reports {engine} in {slot:?}: {end:?}"),
                    vec![msg],
                );
            }
            90..=94 => {
                let n = 1 + self.rng.below(QUEUE_MAX);
                let id = self.sessions[s].id;
                let msgs = (0..n)
                    .map(|_| on(id, Command::RefreshNow(SourceId::Cpu)))
                    .collect();
                self.step(&format!("{n} other commands fill the queue"), msgs);
            }
            95..=97 => {
                let remote: Vec<usize> = open.into_iter().filter(|s| *s != 0).collect();
                if remote.is_empty() {
                    return;
                }
                let s = remote[self.rng.below(remote.len())];
                let id = self.sessions[s].id;
                self.sessions[s].stalled = false;
                self.step(
                    &format!("session {s} ends"),
                    vec![EngineMsg::Client(ClientMsg::Close(id))],
                );
                self.sessions[s].open = false;
                // The server drops the session's action events with it.
                self.sessions[s].actions = None;
            }
            98..=99 => {
                if open.len() >= 4 {
                    return;
                }
                let msg = self.open_session();
                self.step("a session opens", vec![msg]);
            }
            100..=111 => self.action_step(),
            112..=115 => self.stall_or_read_on(s),
            _ => self.reducer_restarts(),
        }
    }

    /// Session `s`'s reader stalls: its action channel fills up and the model reads nothing of
    /// it. A stalled reader reads on instead.
    fn stall_or_read_on(&mut self, s: usize) {
        if self.sessions[s].stalled {
            self.sessions[s].stalled = false;
            self.step(&format!("session {s} reads on"), Vec::new());
            return;
        }
        let filler = ActionEvent {
            id: FILLER,
            target: None,
            state: ActionState::Started,
        };
        while self.sessions[s].actions_tx.try_send(filler.clone()).is_ok() {}
        self.sessions[s].stalled = true;
        self.steps.push(format!("session {s} stalls"));
    }

    fn unstall_all(&mut self) {
        for session in &mut self.sessions {
            session.stalled = false;
        }
    }

    /// The workers take and finish everything, then every request of an open session has
    /// exactly one end and no session has an active request.
    fn drain(&mut self) {
        self.unstall_all();
        self.read_events();
        for _ in 0..8 {
            let progress = self.rig.progress();
            let commands = self.rig.commands();
            let mut out = Vec::new();
            for target in [SourceId::ProcessDetail, SourceId::Udisks] {
                self.workers.copy(target, &commands);
                let before = self.log.claimed.clone();
                out.extend(self.workers.claim(target, &progress, &mut self.log));
                self.check_claims(&before);
            }
            for (slot, engine) in self.workers.running() {
                out.push(
                    self.workers
                        .report(slot, engine, Some(RequestEnd::Done), &mut self.log),
                );
            }
            for i in self.rig.openers.waiting() {
                self.rig.openers.answer(i, Ok(()));
                self.answered.insert(i, RequestEnd::Done);
            }
            self.step("drain", out);
            self.deliver_finished("drain: ");
        }
        let in_flight = lock(&self.rig.gate_log).in_flight;
        if in_flight != 0 {
            self.fail(&format!(
                "the gate has {in_flight} actions in flight after the drain"
            ));
        }
        for s in self.open_sessions() {
            let id = self.sessions[s].id;
            let session = self.rig.reducer().clients.get(id).expect("open");
            for kind in KINDS {
                if session.active(kind) != 0 {
                    self.fail(&format!(
                        "session {s} has {} active {kind:?} requests after the drain",
                        session.active(kind)
                    ));
                }
            }
        }
        for (i, req) in self.reqs.iter().enumerate() {
            if self.sessions[req.session].open && req.end.is_none() {
                self.fail(&format!(
                    "request {i} ({:?} {} of session {}, engine {:?}) got no terminal event",
                    req.slot, req.client, req.session, req.engine
                ));
            }
        }
    }
}

/// Runs one seed and adds the ends its requests got, by kind, to `seen`.
fn run_seed(seed: u64, trace: bool, seen: &mut HashSet<(usize, RequestEnd)>) {
    let mut m = Model::new(seed, trace);
    let ops = 20 + m.rng.below(40);
    for _ in 0..ops {
        m.one_op();
    }
    m.drain();
    for req in &m.reqs {
        if let Some(end) = req.end {
            seen.insert((req.slot.index(), end));
        }
    }
}

#[test]
fn every_request_ends_once_on_its_own_session() {
    if let Some(seed) = std::env::var("TL_LIFECYCLE_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        run_seed(seed, true, &mut HashSet::new());
        return;
    }
    let mut seen = HashSet::new();
    for seed in 0..RUNS {
        run_seed(seed, false, &mut seen);
    }
    // The runs reach every end of every kind. Actions have no cancel; they are lost only when
    // the reducer stops.
    for (kind, name) in KIND_NAMES.iter().enumerate() {
        for end in [
            RequestEnd::Done,
            RequestEnd::Failed,
            RequestEnd::Cancelled,
            RequestEnd::Busy,
            RequestEnd::Lost,
        ] {
            if kind == ACTION && end == RequestEnd::Cancelled {
                continue;
            }
            assert!(
                seen.contains(&(kind, end)),
                "no {name} request ended {end:?}"
            );
        }
    }
}
