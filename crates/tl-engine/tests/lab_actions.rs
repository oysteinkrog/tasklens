//! LabRuntime tests for the action pipeline (ADR-013, E11-007, E11-017): admission limits,
//! deadlines, event order and summaries, and every P1 process kind, through `run_action` with a
//! scripted `FakeOps`. Part 2 adds the targetless kinds and unit jobs through the real `Gate`
//! with a `FakeUnitBus` per manager. The per-module cases for sched, I/O priority, thread
//! targets, cgroups, units and startup entries are unit tests in their own `actions/*.rs`.
//!
//! `Cx::spawn_blocking` runs the job inline in the lab and `ActionOps` gets no `Cx`, so a fake
//! job cannot await a sleep. The harness installs its own `VirtualClock` as the timer driver and
//! a `Block` step advances that clock while the job runs, as `fake_sources.rs` does. Drive time
//! only with `Harness::advance`.
//!
//! `Harness::submit` does what the reducer's `Gate` (E11-A1) does: `admit` with the in-flight
//! count, one `Rejected` event with `target: None` on a reject, otherwise one `run_action` task.
//! An action counts as in flight until the test drains its `ActionFinished`, as in the reducer.
//! The engine-level tests at the end run the real `Gate` inside `engine_root`.

use std::collections::VecDeque;
use std::future::Future;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use asupersync::Cx;
use asupersync::channel::mpsc;
use asupersync::lab::chaos::ChaosConfig;
use asupersync::lab::{LabConfig, LabRuntime};
use asupersync::time::{TimeSource, TimerDriverHandle, VirtualClock};
use asupersync::types::{Budget, RegionId, Time};
use rustc_hash::{FxHashMap, FxHashSet};
use rustix::io::Errno;
use tl_engine::actions::process::PidOp;
use tl_engine::actions::unit::{
    BusFuture, JobRemovedSignal, JobStream, UnitBus, UnitBusError, UnitBusOverride,
};
use tl_engine::actions::{
    ActionCtx, ActionOps, Admission, Gate, LaunchOutcome, Openers, admit, deadline, run_action,
    summarize,
};
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::dbus::ManagerConnSlot;
use tl_engine::reducer::{ActionGate, AdmissionView};
use tl_engine::update::EngineMsg;
use tl_model::actions::{
    ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    LaunchSpec, LaunchWhat, RejectReason, Signal,
};
use tl_model::ids::ProcessKey;
use tl_model::process::ProcFlags;
use tl_model::system::CpuSet;
use tl_model::table::{Manager, StartupRef, StartupSource, UnitRef};
use tl_model::time::BootId;
use tl_ops::OpsError;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const BOOT: BootId = BootId(1);
const EXITED_DURING: &str = "process exited during the change";

/// One scripted result. Each `ActionOps` call for a PID takes the next step of that PID's
/// script; an empty script is `Ok`.
#[derive(Clone, Debug)]
enum FakeStep {
    Ok,
    IdentityChanged,
    Errno(i32),
    Partial(u16, u16),
    ExitedDuring,
    /// Advances the virtual clock by this much inside the blocking job, then succeeds.
    Block(Duration),
    Panic,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FakeCall {
    Open(u32),
    Send(u32, Signal),
    Close(u32),
    Pid(u32, PidOp),
}

struct FakeOps {
    script: Mutex<FxHashMap<u32 /* pid */, VecDeque<FakeStep>>>,
    log: Mutex<Vec<FakeCall>>,
    /// The PID behind each open fake pidfd.
    fds: Mutex<FxHashMap<RawFd, u32>>,
    clock: Arc<VirtualClock>,
}

/// A poisoned lock still holds the log: a `Panic` step panics while no lock is held.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeOps {
    fn new(clock: &Arc<VirtualClock>) -> Self {
        Self {
            script: Mutex::new(FxHashMap::default()),
            log: Mutex::new(Vec::new()),
            fds: Mutex::new(FxHashMap::default()),
            clock: Arc::clone(clock),
        }
    }

    fn script(&self, pid: u32, steps: &[FakeStep]) {
        lock(&self.script).insert(pid, steps.iter().cloned().collect());
    }

    fn calls(&self) -> Vec<FakeCall> {
        lock(&self.log).clone()
    }

    fn next(&self, pid: u32) -> Result<(), OpsError> {
        let step = lock(&self.script)
            .get_mut(&pid)
            .and_then(VecDeque::pop_front)
            .unwrap_or(FakeStep::Ok);
        match step {
            FakeStep::Ok => Ok(()),
            FakeStep::IdentityChanged => Err(OpsError::IdentityChanged),
            FakeStep::Errno(n) => Err(OpsError::Os(n)),
            FakeStep::Partial(done, total) => Err(OpsError::Partial { done, total }),
            FakeStep::ExitedDuring => Err(OpsError::OutcomeUnknown(EXITED_DURING)),
            FakeStep::Block(d) => {
                self.clock.advance(d.as_nanos() as u64);
                Ok(())
            }
            FakeStep::Panic => panic!("fake op panicked"),
        }
    }
}

impl ActionOps for FakeOps {
    fn open_verified(&self, key: ProcessKey, _boot: BootId) -> Result<OwnedFd, OpsError> {
        lock(&self.log).push(FakeCall::Open(key.pid));
        self.next(key.pid)?;
        let fd: OwnedFd = std::fs::File::open("/dev/null")
            .expect("open /dev/null")
            .into();
        lock(&self.fds).insert(fd.as_raw_fd(), key.pid);
        Ok(fd)
    }

    fn send_signal(&self, pidfd: &OwnedFd, sig: Signal) -> Result<(), OpsError> {
        let pid = lock(&self.fds)[&pidfd.as_raw_fd()];
        lock(&self.log).push(FakeCall::Send(pid, sig));
        self.next(pid)
    }

    fn close_pidfd(&self, pidfd: OwnedFd) {
        let pid = lock(&self.fds).remove(&pidfd.as_raw_fd());
        lock(&self.log).push(FakeCall::Close(pid.expect("fd is open")));
    }

    fn apply_pid_op(&self, key: ProcessKey, _boot: BootId, op: &PidOp) -> Result<(), OpsError> {
        lock(&self.log).push(FakeCall::Pid(key.pid, *op));
        self.next(key.pid)
    }
}

/// A lab whose timers read `clock`, an `ActionCtx` over `ops`, and the gate state.
struct Harness {
    lab: LabRuntime,
    clock: Arc<VirtualClock>,
    root: RegionId,
    actx: Arc<ActionCtx>,
    ops: Arc<FakeOps>,
    events: async_channel::Receiver<ActionEvent>,
    msgs: mpsc::Receiver<EngineMsg>,
    /// Events read from `events` and not yet asked for.
    seen: Vec<ActionEvent>,
    in_flight: FxHashSet<ActionId>,
    live: FxHashMap<ProcessKey, ProcFlags>,
    /// The real `Gate` over `actx`, when set. `submit` and `finished` then go through it.
    gate: Option<Arc<Mutex<Gate>>>,
}

fn harness() -> Harness {
    harness_with(LabConfig::new(7).max_steps(1_000_000))
}

fn harness_with(config: LabConfig) -> Harness {
    harness_full(config, None, None)
}

fn harness_full(
    config: LabConfig,
    openers: Option<Arc<dyn Openers>>,
    unit_bus: Option<UnitBusOverride>,
) -> Harness {
    let mut lab = LabRuntime::new(config);
    let clock = Arc::new(VirtualClock::starting_at(Time::ZERO));
    // Before any task exists: each `Cx` keeps the driver it was made with.
    lab.state
        .set_timer_driver(TimerDriverHandle::with_virtual_clock(Arc::clone(&clock)));
    let root = lab.state.create_root_region(Budget::INFINITE);
    let ops = Arc::new(FakeOps::new(&clock));
    let (events_tx, events) = async_channel::bounded(256);
    let (msgs_tx, msgs) = mpsc::channel(256);
    let actx = Arc::new(ActionCtx {
        events: events_tx,
        msgs: msgs_tx,
        ops: ops.clone(),
        boot_id: BOOT,
        uid: 1000,
        config: Arc::new(EngineConfig::new(
            EngineMode::Full,
            PathBuf::from("/nonexistent"),
        )),
        systemd_system: ManagerConnSlot::new(),
        systemd_user: ManagerConnSlot::new(),
        openers,
        helper: None,
        destroy_support_sent: AtomicBool::new(false),
        unit_bus_override: unit_bus,
        startup_paths: tl_engine::actions::startup::StartupPaths::from_env(),
    });
    Harness {
        lab,
        clock,
        root,
        actx,
        ops,
        events,
        msgs,
        seen: Vec::new(),
        in_flight: FxHashSet::default(),
        live: FxHashMap::default(),
        gate: None,
    }
}

impl Harness {
    /// Admits `req` the way the E11-A1 gate does. A process target not marked otherwise with
    /// `self.live` is a live process with no flags. Does not run the lab.
    fn submit(&mut self, req: ActionRequest) {
        for target in &req.targets {
            if let ActionTarget::Process(key) = target {
                self.live.entry(*key).or_insert(ProcFlags::empty());
            }
        }
        if let Some(gate) = &self.gate {
            self.submit_gate(Arc::clone(gate), req);
            return;
        }
        let exited = FxHashSet::default();
        let view = AdmissionView {
            boot_id: BOOT,
            live: &self.live,
            exited_within_linger: &exited,
        };
        match admit(&req, &view, self.in_flight.len(), &FxHashSet::default()) {
            Admission::Reject(reason) => {
                let ev = ActionEvent {
                    id: req.id,
                    target: None,
                    state: ActionState::Rejected(reason),
                };
                self.actx.events.try_send(ev).expect("event queue has room");
            }
            Admission::Run { per_target } => {
                self.in_flight.insert(req.id);
                let actx = Arc::clone(&self.actx);
                let (task, _handle) = self
                    .lab
                    .state
                    .create_task(self.root, Budget::INFINITE, async move {
                        let cx = Cx::current().expect("task has a Cx");
                        run_action(cx, actx, req, per_target).await;
                    })
                    .expect("task is created");
                self.lab.scheduler.lock().schedule(task, 0);
            }
        }
    }

    /// Hands `req` to the real gate inside a task, since `ActionGate::submit` needs a `Cx`.
    /// The tests that use the gate never fill its 16 slots, so every request counts as in
    /// flight.
    fn submit_gate(&mut self, gate: Arc<Mutex<Gate>>, req: ActionRequest) {
        self.in_flight.insert(req.id);
        let live = self.live.clone();
        let (task, _handle) = self
            .lab
            .state
            .create_task(self.root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let exited = FxHashSet::default();
                let view = AdmissionView {
                    boot_id: BOOT,
                    live: &live,
                    exited_within_linger: &exited,
                };
                lock(&gate).submit(&cx, req, &view);
            })
            .expect("task is created");
        self.lab.scheduler.lock().schedule(task, 0);
    }

    fn run(&mut self) {
        self.lab.run_until_idle();
    }

    /// Moves the clock by `d` and runs every task that became ready.
    fn advance(&mut self, d: Duration) {
        self.clock.advance(d.as_nanos() as u64);
        self.lab.advance_time_to(self.clock.now());
        // `run_until_idle` stops when no task is ready; one step fires the due timers.
        self.lab.step_for_test();
        self.lab.run_until_idle();
    }

    /// Every event for `id` so far, as `(target, state)`.
    fn events_for(&mut self, id: ActionId) -> Vec<(Option<u16>, ActionState)> {
        while let Ok(ev) = self.events.try_recv() {
            self.seen.push(ev);
        }
        let (mine, rest) = std::mem::take(&mut self.seen)
            .into_iter()
            .partition(|ev| ev.id == id);
        self.seen = rest;
        mine.into_iter().map(|ev| (ev.target, ev.state)).collect()
    }

    /// The ids of every `ActionFinished` so far, in arrival order. Frees their admission slots.
    fn finished(&mut self) -> Vec<ActionId> {
        let mut out = Vec::new();
        while let Ok(msg) = self.msgs.try_recv() {
            match msg {
                EngineMsg::ActionFinished { id } => {
                    assert!(self.in_flight.remove(&id), "{id:?} finished twice");
                    if let Some(gate) = &self.gate {
                        lock(gate).finished(id);
                    }
                    out.push(id);
                }
                other => panic!("unexpected message {other:?}"),
            }
        }
        out
    }

    /// Runs to quiescence and checks the lab's oracles.
    fn passed(mut self) {
        let report = self.lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }
}

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 100 + u64::from(pid),
        boot_id: BOOT,
        pid,
    }
}

fn req(id: u64, kind: ActionKind, targets: &[ActionTarget]) -> ActionRequest {
    ActionRequest {
        id: ActionId(id),
        kind,
        targets: targets.iter().cloned().collect(),
        privileged: false,
    }
}

fn procs(pids: &[u32]) -> Vec<ActionTarget> {
    pids.iter()
        .map(|p| ActionTarget::Process(key(*p)))
        .collect()
}

fn failed(e: ActionError) -> ActionState {
    ActionState::Failed(e)
}

fn internal() -> ActionState {
    failed(ActionError::Internal(Arc::from("internal error")))
}

fn timed_out() -> ActionState {
    ActionState::OutcomeUnknown(Arc::from("timed out"))
}

/// `Started`, one event per target with `states`, then `summary`.
fn expected(states: &[ActionState], summary: ActionState) -> Vec<(Option<u16>, ActionState)> {
    let mut out = vec![(None, ActionState::Started)];
    for (i, s) in states.iter().enumerate() {
        out.push((Some(i as u16), s.clone()));
    }
    out.push((None, summary));
    out
}

/// Runs `kind` on PID 500 with `script`, checks one `ActionFinished`, and returns the
/// single target's state and the fake's calls.
fn one(kind: ActionKind, script: &[FakeStep]) -> (ActionState, Vec<FakeCall>) {
    let mut h = harness();
    h.ops.script(500, script);
    h.submit(req(1, kind, &procs(&[500])));
    h.run();
    let events = h.events_for(ActionId(1));
    assert_eq!(events.len(), 3, "{events:?}");
    let state = events[1].1.clone();
    assert_eq!(
        events,
        expected(std::slice::from_ref(&state), state.clone())
    );
    assert_eq!(h.finished(), [ActionId(1)]);
    let calls = h.ops.calls();
    h.passed();
    (state, calls)
}

fn eperm() -> FakeStep {
    FakeStep::Errno(Errno::PERM.raw_os_error())
}

// Pipeline (E11-001, E11-002) ----------------------------------------------------------------

#[test]
fn timeout_is_outcome_unknown() {
    let mut h = harness();
    h.ops
        .script(500, &[FakeStep::Block(Duration::from_secs(3))]);
    h.submit(req(1, ActionKind::Signal(Signal::Term), &procs(&[500])));
    h.run();
    // The job moved the clock 3 s, past the 2 s deadline.
    h.advance(Duration::ZERO);
    assert_eq!(
        h.events_for(ActionId(1)),
        expected(&[timed_out()], timed_out())
    );
    assert_eq!(h.finished(), [ActionId(1)]);
    h.passed();
}

#[test]
fn partial_failure_summary() {
    let mut h = harness();
    h.ops.script(502, &[FakeStep::Ok, eperm()]);
    h.submit(req(
        1,
        ActionKind::Signal(Signal::Term),
        &procs(&[501, 502, 503]),
    ));
    h.run();
    assert_eq!(
        h.events_for(ActionId(1)),
        expected(
            &[
                ActionState::Succeeded,
                failed(ActionError::PermissionDenied),
                ActionState::Succeeded,
            ],
            failed(ActionError::Partial { done: 2, total: 3 }),
        )
    );
    assert_eq!(h.finished(), [ActionId(1)]);
    h.passed();
}

#[test]
fn panic_is_internal_error() {
    let (state, calls) = one(ActionKind::Signal(Signal::Term), &[FakeStep::Panic]);
    assert_eq!(state, internal());
    assert_eq!(calls, [FakeCall::Open(500)]);
}

#[test]
fn emit_drops_after_1s() {
    let mut h = harness();
    let fill = ActionEvent {
        id: ActionId(99),
        target: None,
        state: ActionState::Started,
    };
    for _ in 0..256 {
        h.actx.events.try_send(fill.clone()).expect("room left");
    }
    h.submit(req(1, ActionKind::Signal(Signal::Term), &procs(&[500])));
    h.run();
    // Started, the target state and the summary each wait 1 s, then are dropped.
    for _ in 0..3 {
        assert!(h.finished().is_empty());
        h.advance(Duration::from_secs(1));
    }
    assert_eq!(h.finished(), [ActionId(1)]);
    assert_eq!(h.events.len(), 256);
    assert!(h.events_for(ActionId(1)).is_empty(), "every event dropped");
    // The action itself ran.
    assert_eq!(
        h.ops.calls(),
        [FakeCall::Open(500), FakeCall::Send(500, Signal::Term)]
    );
    h.passed();
}

#[test]
fn deadline_table() {
    let s = Duration::from_secs;
    let process = ActionTarget::Process(key(500));
    let term = ActionKind::Signal(Signal::Term);
    for kind in [
        term.clone(),
        ActionKind::KillTree,
        ActionKind::Suspend,
        ActionKind::Resume,
        ActionKind::SetNice(5),
        ActionKind::SetAffinity(CpuSet::empty()),
    ] {
        assert_eq!(
            deadline(&kind, Some(&process), false),
            Some(s(2)),
            "{kind:?}"
        );
    }
    let unit = |manager| {
        ActionTarget::Unit(UnitRef {
            manager,
            name: Arc::from("foo.service"),
        })
    };
    let system = unit(Manager::System);
    let user = unit(Manager::User);
    assert_eq!(deadline(&ActionKind::UnitStart, Some(&system), false), None);
    assert_eq!(
        deadline(&ActionKind::UnitEnable, Some(&system), false),
        Some(s(120))
    );
    assert_eq!(
        deadline(&ActionKind::UnitEnable, Some(&user), false),
        Some(s(10))
    );
    assert_eq!(
        deadline(&ActionKind::FreezeGroup, Some(&user), false),
        Some(s(2))
    );
    assert_eq!(
        deadline(&ActionKind::FreezeGroup, Some(&system), false),
        Some(s(120))
    );
    let entry = |source, id: &str| {
        ActionTarget::Startup(StartupRef {
            source,
            id: Arc::from(id),
        })
    };
    let user_unit_entry = entry(StartupSource::UserUnit, "foo.service");
    let autostart = entry(StartupSource::AutostartUser, "foo.desktop");
    assert_eq!(
        deadline(&ActionKind::StartupDisable, Some(&user_unit_entry), false),
        Some(s(10))
    );
    assert_eq!(
        deadline(&ActionKind::StartupDisable, Some(&autostart), false),
        Some(s(2))
    );
    assert_eq!(
        deadline(&ActionKind::StartupRunNow, Some(&autostart), false),
        None
    );
    let launch = |as_admin| {
        ActionKind::Launch(LaunchSpec {
            what: LaunchWhat::Command(Arc::from("true")),
            as_admin,
            cwd: None,
        })
    };
    assert_eq!(deadline(&launch(true), None, false), Some(s(120)));
    assert_eq!(deadline(&launch(false), None, false), Some(s(2)));
    for kind in [term, ActionKind::UnitStart, launch(false)] {
        assert_eq!(
            deadline(&kind, Some(&process), true),
            Some(s(120)),
            "{kind:?}"
        );
    }
}

#[test]
fn summarize_order() {
    let unknown = ActionState::OutcomeUnknown(Arc::from("x"));
    assert_eq!(
        summarize(&[
            ActionState::Rejected(RejectReason::IdentityChanged),
            unknown.clone()
        ]),
        unknown
    );
    assert!(matches!(
        summarize(&[
            ActionState::Rejected(RejectReason::Protected),
            failed(ActionError::NoSuchProcess),
        ]),
        ActionState::Failed(_)
    ));
    assert_eq!(
        summarize(&[ActionState::Succeeded, ActionState::Succeeded]),
        ActionState::Succeeded
    );
    assert_eq!(
        summarize(&[
            ActionState::Succeeded,
            failed(ActionError::PermissionDenied)
        ]),
        failed(ActionError::Partial { done: 1, total: 2 })
    );
    // One target keeps its own failure.
    assert_eq!(
        summarize(&[failed(ActionError::PermissionDenied)]),
        failed(ActionError::PermissionDenied)
    );
}

#[test]
fn seventeenth_action_is_busy() {
    let mut h = harness();
    for id in 1..=16 {
        let pid = 1000 + id as u32;
        h.ops
            .script(pid, &[FakeStep::Block(Duration::from_secs(10))]);
        h.submit(req(id, ActionKind::Signal(Signal::Term), &procs(&[pid])));
    }
    h.run();
    for id in 1..=16 {
        assert_eq!(
            h.events_for(ActionId(id)).first(),
            Some(&(None, ActionState::Started)),
            "action {id}"
        );
    }
    // The gate has not seen any `ActionFinished` yet, so 16 are in flight.
    h.submit(req(17, ActionKind::Signal(Signal::Term), &procs(&[2000])));
    h.advance(Duration::from_secs(10));
    assert_eq!(
        h.events_for(ActionId(17)),
        [(None, ActionState::Rejected(RejectReason::Busy))]
    );
    // No task ran for the 17th.
    assert!(!h.ops.calls().contains(&FakeCall::Open(2000)));
    let mut done = h.finished();
    done.sort_by_key(|id| id.0);
    assert_eq!(done, (1..=16).map(ActionId).collect::<Vec<_>>());
    // A free slot admits the next one.
    h.submit(req(18, ActionKind::Signal(Signal::Term), &procs(&[2000])));
    h.run();
    assert_eq!(
        h.events_for(ActionId(18)),
        expected(&[ActionState::Succeeded], ActionState::Succeeded)
    );
    assert_eq!(h.finished(), [ActionId(18)]);
    h.passed();
}

#[test]
fn event_order_started_targets_summary() {
    let mut h = harness();
    h.ops.script(512, &[FakeStep::IdentityChanged]);
    // PID 2 is a kernel thread: admission rejects it and its event keeps its place.
    h.live.insert(key(2), ProcFlags::KERNEL_THREAD);
    h.submit(req(
        1,
        ActionKind::Signal(Signal::Hup),
        &procs(&[511, 2, 512]),
    ));
    h.submit(req(2, ActionKind::Suspend, &procs(&[521, 522])));
    h.run();
    assert_eq!(
        h.events_for(ActionId(1)),
        expected(
            &[
                ActionState::Succeeded,
                ActionState::Rejected(RejectReason::KernelThread),
                ActionState::Rejected(RejectReason::IdentityChanged),
            ],
            ActionState::Rejected(RejectReason::KernelThread),
        )
    );
    assert_eq!(
        h.events_for(ActionId(2)),
        expected(
            &[ActionState::Succeeded, ActionState::Succeeded],
            ActionState::Succeeded
        )
    );
    // The rejected target was never opened.
    assert!(!h.ops.calls().contains(&FakeCall::Open(2)));
    let mut done = h.finished();
    done.sort_by_key(|id| id.0);
    assert_eq!(done, [ActionId(1), ActionId(2)]);
    h.passed();
}

#[test]
fn action_finished_sent_once() {
    let mut h = harness();
    h.ops.script(531, &[FakeStep::Panic]);
    h.ops
        .script(541, &[FakeStep::Block(Duration::from_secs(3))]);
    h.submit(req(
        1,
        ActionKind::Signal(Signal::Term),
        &procs(&[531, 532]),
    ));
    h.submit(req(2, ActionKind::Signal(Signal::Term), &procs(&[541])));
    h.submit(req(3, ActionKind::KillTree, &procs(&[551, 552])));
    h.submit(req(4, ActionKind::SetNice(30), &procs(&[561])));
    h.run();
    h.advance(Duration::ZERO);
    h.advance(Duration::from_secs(5));
    let mut done = h.finished();
    done.sort_by_key(|id| id.0);
    assert_eq!(done, (1..=4).map(ActionId).collect::<Vec<_>>());
    h.advance(Duration::from_secs(5));
    assert!(h.finished().is_empty(), "no second ActionFinished");
    h.passed();
}

// Signals, Suspend and Resume (E11-003) ------------------------------------------------------

#[test]
fn signal_succeeds() {
    let (state, calls) = one(ActionKind::Signal(Signal::Term), &[]);
    assert_eq!(state, ActionState::Succeeded);
    assert_eq!(
        calls,
        [FakeCall::Open(500), FakeCall::Send(500, Signal::Term)]
    );
}

#[test]
fn signal_identity_changed() {
    let (state, calls) = one(
        ActionKind::Signal(Signal::Kill),
        &[FakeStep::IdentityChanged],
    );
    assert_eq!(state, ActionState::Rejected(RejectReason::IdentityChanged));
    // No signal goes out when the identity check fails.
    assert_eq!(calls, [FakeCall::Open(500)]);
}

#[test]
fn signal_eperm() {
    let (state, _) = one(ActionKind::Signal(Signal::Term), &[FakeStep::Ok, eperm()]);
    assert_eq!(state, failed(ActionError::PermissionDenied));
}

#[test]
fn signal_esrch() {
    let esrch = FakeStep::Errno(Errno::SRCH.raw_os_error());
    let (state, _) = one(ActionKind::Signal(Signal::Term), &[FakeStep::Ok, esrch]);
    assert_eq!(state, failed(ActionError::NoSuchProcess));
}

#[test]
fn signal_other_passes_through() {
    let (state, calls) = one(ActionKind::Signal(Signal::Other(40)), &[]);
    assert_eq!(state, ActionState::Succeeded);
    assert_eq!(
        calls,
        [FakeCall::Open(500), FakeCall::Send(500, Signal::Other(40))]
    );
}

#[test]
fn signal_protected_targets_rejected() {
    let mut h = harness();
    h.live.insert(key(600), ProcFlags::IS_SELF);
    let other_boot = ProcessKey {
        boot_id: BootId(2),
        ..key(700)
    };
    let mut targets = procs(&[1, 600]);
    targets.push(ActionTarget::Process(other_boot));
    h.submit(req(1, ActionKind::Signal(Signal::Kill), &targets));
    h.run();
    assert_eq!(
        h.events_for(ActionId(1)),
        expected(
            &[
                ActionState::Rejected(RejectReason::Protected),
                ActionState::Rejected(RejectReason::Protected),
                ActionState::Rejected(RejectReason::IdentityChanged),
            ],
            ActionState::Rejected(RejectReason::Protected),
        )
    );
    assert!(h.ops.calls().is_empty(), "{:?}", h.ops.calls());
    assert_eq!(h.finished(), [ActionId(1)]);
    h.passed();
}

#[test]
fn suspend_resume_signals() {
    let (state, calls) = one(ActionKind::Suspend, &[]);
    assert_eq!(state, ActionState::Succeeded);
    assert_eq!(
        calls,
        [FakeCall::Open(500), FakeCall::Send(500, Signal::Stop)]
    );
    let (state, calls) = one(ActionKind::Resume, &[]);
    assert_eq!(state, ActionState::Succeeded);
    assert_eq!(
        calls,
        [FakeCall::Open(500), FakeCall::Send(500, Signal::Cont)]
    );
}

// End Process Tree through run_action (E11-004) ---------------------------------------------

/// Runs `KillTree` on PIDs 501, 502, 503 and returns the events and the fake's calls.
fn tree(h: &mut Harness, pids: &[u32]) -> (Vec<(Option<u16>, ActionState)>, Vec<FakeCall>) {
    h.submit(req(1, ActionKind::KillTree, &procs(pids)));
    h.run();
    let events = h.events_for(ActionId(1));
    assert_eq!(h.finished(), [ActionId(1)]);
    (events, h.ops.calls())
}

#[test]
fn kill_tree_stop_all_then_kill_all() {
    let mut h = harness();
    let (events, calls) = tree(&mut h, &[501, 502, 503]);
    assert_eq!(
        events,
        expected(&vec![ActionState::Succeeded; 3], ActionState::Succeeded)
    );
    let mut want: Vec<FakeCall> = [501, 502, 503].map(FakeCall::Open).into();
    for sig in [Signal::Stop, Signal::Kill] {
        want.extend([501, 502, 503].map(|p| FakeCall::Send(p, sig)));
    }
    want.extend([501, 502, 503].map(FakeCall::Close));
    assert_eq!(calls, want);
    h.passed();
}

#[test]
fn kill_tree_skips_identity_changed() {
    let mut h = harness();
    h.ops.script(502, &[FakeStep::IdentityChanged]);
    let (events, calls) = tree(&mut h, &[501, 502, 503]);
    let changed = ActionState::Rejected(RejectReason::IdentityChanged);
    assert_eq!(
        events,
        expected(
            &[
                ActionState::Succeeded,
                changed.clone(),
                ActionState::Succeeded
            ],
            changed,
        )
    );
    assert!(!calls.iter().any(|c| matches!(c, FakeCall::Send(502, _))));
    assert_eq!(calls[calls.len() - 2..], [501, 503].map(FakeCall::Close));
    h.passed();
}

#[test]
fn kill_tree_partial_eperm() {
    let mut h = harness();
    h.ops.script(503, &[FakeStep::Ok, FakeStep::Ok, eperm()]);
    let (events, calls) = tree(&mut h, &[501, 502, 503]);
    assert_eq!(
        events,
        expected(
            &[
                ActionState::Succeeded,
                ActionState::Succeeded,
                failed(ActionError::PermissionDenied),
            ],
            failed(ActionError::Partial { done: 2, total: 3 }),
        )
    );
    // Every fd is still closed.
    assert_eq!(
        calls[calls.len() - 3..],
        [501, 502, 503].map(FakeCall::Close)
    );
    h.passed();
}

#[test]
fn kill_tree_admission_rejected_target_keeps_its_place() {
    let mut h = harness();
    h.live.insert(key(2), ProcFlags::KERNEL_THREAD);
    let (events, calls) = tree(&mut h, &[501, 2, 503]);
    assert_eq!(
        events,
        expected(
            &[
                ActionState::Succeeded,
                ActionState::Rejected(RejectReason::KernelThread),
                ActionState::Succeeded,
            ],
            ActionState::Rejected(RejectReason::KernelThread),
        )
    );
    // Only the admitted targets reach `kill_tree`.
    let mut want: Vec<FakeCall> = [501, 503].map(FakeCall::Open).into();
    for sig in [Signal::Stop, Signal::Kill] {
        want.extend([501, 503].map(|p| FakeCall::Send(p, sig)));
    }
    want.extend([501, 503].map(FakeCall::Close));
    assert_eq!(calls, want);
    h.passed();
}

#[test]
fn kill_tree_timeout_is_outcome_unknown() {
    let mut h = harness();
    // Two targets get 4 s; the job takes 5 s.
    h.ops
        .script(501, &[FakeStep::Block(Duration::from_secs(5))]);
    h.submit(req(1, ActionKind::KillTree, &procs(&[501, 502])));
    h.run();
    h.advance(Duration::ZERO);
    assert_eq!(
        h.events_for(ActionId(1)),
        expected(&[timed_out(), timed_out()], timed_out())
    );
    assert_eq!(h.finished(), [ActionId(1)]);
    h.passed();
}

#[test]
fn kill_tree_panic_is_internal_error() {
    let mut h = harness();
    h.ops.script(502, &[FakeStep::Panic]);
    let (events, _) = tree(&mut h, &[501, 502]);
    assert_eq!(
        events,
        expected(
            &[internal(), internal()],
            failed(ActionError::Partial { done: 0, total: 2 })
        )
    );
    h.passed();
}

// Nice and affinity (E11-005) ----------------------------------------------------------------

#[test]
fn nice_done() {
    let (state, calls) = one(ActionKind::SetNice(5), &[]);
    assert_eq!(state, ActionState::Succeeded);
    assert_eq!(calls, [FakeCall::Pid(500, PidOp::Nice(5))]);
}

#[test]
fn nice_partial() {
    let (state, _) = one(ActionKind::SetNice(5), &[FakeStep::Partial(3, 4)]);
    assert_eq!(state, failed(ActionError::Partial { done: 3, total: 4 }));
}

#[test]
fn nice_exited_during() {
    let (state, _) = one(ActionKind::SetNice(5), &[FakeStep::ExitedDuring]);
    assert_eq!(state, ActionState::OutcomeUnknown(Arc::from(EXITED_DURING)));
}

#[test]
fn nice_eperm() {
    let (state, _) = one(ActionKind::SetNice(-10), &[eperm()]);
    assert_eq!(state, failed(ActionError::PermissionDenied));
}

#[test]
fn nice_identity_changed() {
    let (state, _) = one(ActionKind::SetNice(5), &[FakeStep::IdentityChanged]);
    assert_eq!(state, ActionState::Rejected(RejectReason::IdentityChanged));
}

#[test]
fn nice_timeout_is_outcome_unknown() {
    let mut h = harness();
    h.ops
        .script(500, &[FakeStep::Block(Duration::from_secs(3))]);
    h.submit(req(1, ActionKind::SetNice(5), &procs(&[500])));
    h.run();
    h.advance(Duration::ZERO);
    assert_eq!(
        h.events_for(ActionId(1)),
        expected(&[timed_out()], timed_out())
    );
    h.passed();
}

#[test]
fn nice_out_of_range_rejected() {
    for n in [-21, 20, i8::MIN, i8::MAX] {
        let (state, calls) = one(ActionKind::SetNice(n), &[]);
        assert_eq!(
            state,
            ActionState::Rejected(RejectReason::Unsupported),
            "{n}"
        );
        assert!(calls.is_empty(), "{n}: {calls:?}");
    }
}

#[test]
fn affinity_empty_rejected() {
    let (state, calls) = one(ActionKind::SetAffinity(CpuSet::empty()), &[]);
    assert_eq!(state, ActionState::Rejected(RejectReason::Unsupported));
    assert!(calls.is_empty(), "the fake saw a call: {calls:?}");
}

#[test]
fn affinity_done() {
    let mut set = CpuSet::empty();
    set.insert(0);
    set.insert(3);
    let (state, calls) = one(ActionKind::SetAffinity(set), &[]);
    assert_eq!(state, ActionState::Succeeded);
    assert_eq!(calls, [FakeCall::Pid(500, PidOp::Affinity(set))]);
}

// Chaos (ADR-013) ----------------------------------------------------------------------------

/// Mirrors `#[explore_seeds(seeds = 0..16, workers = 2, chaos)]`, which asupersync 0.5.0 does
/// not re-export (see `lab_supervise.rs::explore_restart`). No cancel injection: a chaos cancel
/// ends an action task before its summary, and this test checks that every action gets one.
/// Delays and wakeup storms still reorder the tasks.
#[test]
fn explore_signal_pipeline() {
    for seed in 0..16 {
        let chaos = ChaosConfig::default()
            .with_cancel_probability(0.0)
            .with_delay_probability(0.05)
            .with_delay_range(Duration::ZERO..Duration::from_millis(10))
            .with_wakeup_storm_probability(0.01);
        let mut h = harness_with(
            LabConfig::new(seed)
                .worker_count(2)
                .max_steps(1_000_000)
                .with_chaos(chaos),
        );
        h.ops.script(602, &[FakeStep::Ok, eperm()]);
        h.ops.script(611, &[FakeStep::IdentityChanged]);
        h.submit(req(
            1,
            ActionKind::Signal(Signal::Term),
            &procs(&[601, 602, 603]),
        ));
        h.submit(req(2, ActionKind::Suspend, &procs(&[611, 612])));
        h.submit(req(3, ActionKind::KillTree, &procs(&[621, 622, 623])));
        h.submit(req(4, ActionKind::Resume, &procs(&[631])));
        h.run();
        h.advance(Duration::from_secs(1));
        for (id, targets) in [(1, 3), (2, 2), (3, 3), (4, 1)] {
            let events = h.events_for(ActionId(id));
            assert_eq!(
                events.len(),
                targets + 2,
                "seed {seed} action {id}: {events:?}"
            );
            assert_eq!(
                events[0],
                (None, ActionState::Started),
                "seed {seed} action {id}"
            );
            let (target, summary) = events.last().expect("a summary");
            assert_eq!(*target, None, "seed {seed} action {id}");
            assert!(summary.is_final(), "seed {seed} action {id}");
            let per_target: Vec<Option<u16>> =
                events[1..=targets].iter().map(|(t, _)| *t).collect();
            let want: Vec<Option<u16>> = (0..targets as u16).map(Some).collect();
            assert_eq!(per_target, want, "seed {seed} action {id}");
        }
        let mut done = h.finished();
        done.sort_by_key(|id| id.0);
        assert_eq!(
            done,
            (1..=4).map(ActionId).collect::<Vec<_>>(),
            "seed {seed}"
        );
        h.passed();
    }
}

// Targetless kinds (E11-016) -----------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Debug)]
enum OpenerCall {
    OpenUri(String),
    ShowInFileManager(String, bool),
    Launch(LaunchSpec),
}

/// Records each call and answers with `result` (open and show) or `launch`, after sleeping
/// `block` on the lab clock. Never opens, shows or starts anything.
struct FakeOpeners {
    log: Mutex<Vec<OpenerCall>>,
    block: Duration,
    result: Result<(), ActionError>,
    launch: Result<LaunchOutcome, ActionError>,
}

impl FakeOpeners {
    fn new() -> Self {
        Self {
            log: Mutex::new(Vec::new()),
            block: Duration::ZERO,
            result: Ok(()),
            launch: Ok(LaunchOutcome::Launched),
        }
    }

    fn calls(&self) -> Vec<OpenerCall> {
        lock(&self.log).clone()
    }

    async fn wait(&self, cx: &Cx) {
        if !self.block.is_zero() {
            asupersync::time::sleep(cx.now(), self.block).await;
        }
    }
}

impl Openers for FakeOpeners {
    fn open_uri(
        &self,
        cx: &Cx,
        target: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ActionError>> + Send + '_>> {
        lock(&self.log).push(OpenerCall::OpenUri(target.to_owned()));
        let cx = cx.clone();
        Box::pin(async move {
            self.wait(&cx).await;
            self.result.clone()
        })
    }

    fn show_in_file_manager(
        &self,
        cx: &Cx,
        path: &str,
        properties: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), ActionError>> + Send + '_>> {
        lock(&self.log).push(OpenerCall::ShowInFileManager(path.to_owned(), properties));
        let cx = cx.clone();
        Box::pin(async move {
            self.wait(&cx).await;
            self.result.clone()
        })
    }

    fn launch(
        &self,
        cx: &Cx,
        spec: &LaunchSpec,
    ) -> Pin<Box<dyn Future<Output = Result<LaunchOutcome, ActionError>> + Send + '_>> {
        lock(&self.log).push(OpenerCall::Launch(spec.clone()));
        let cx = cx.clone();
        Box::pin(async move {
            self.wait(&cx).await;
            self.launch.clone()
        })
    }
}

fn openers_harness(fake: &Arc<FakeOpeners>) -> Harness {
    let openers: Arc<dyn Openers> = fake.clone();
    harness_full(LabConfig::new(7).max_steps(1_000_000), Some(openers), None)
}

fn launch_kind(as_admin: bool) -> ActionKind {
    ActionKind::Launch(LaunchSpec {
        what: LaunchWhat::Command(Arc::from("kate")),
        as_admin,
        cwd: None,
    })
}

/// `Started` then the one summary, with no per-target event.
fn summary_only(state: ActionState) -> Vec<(Option<u16>, ActionState)> {
    expected(&[], state)
}

/// Runs targetless `kind` with `fake` and no clock movement. Returns the events.
fn targetless_events(fake: &Arc<FakeOpeners>, kind: ActionKind) -> Vec<(Option<u16>, ActionState)> {
    let mut h = openers_harness(fake);
    h.submit(req(1, kind, &[]));
    h.run();
    let events = h.events_for(ActionId(1));
    assert_eq!(h.finished(), [ActionId(1)]);
    h.passed();
    events
}

#[test]
fn targetless_open_uri() {
    let fake = Arc::new(FakeOpeners::new());
    let events = targetless_events(&fake, ActionKind::OpenUri(Arc::from("https://example.org")));
    assert_eq!(events, summary_only(ActionState::Succeeded));
    assert_eq!(
        fake.calls(),
        [OpenerCall::OpenUri("https://example.org".to_owned())]
    );
}

#[test]
fn targetless_show_in_file_manager() {
    let fake = Arc::new(FakeOpeners::new());
    let kind = ActionKind::ShowInFileManager {
        path: Arc::from("/usr/bin/foo"),
        properties: true,
    };
    let events = targetless_events(&fake, kind);
    assert_eq!(events, summary_only(ActionState::Succeeded));
    assert_eq!(
        fake.calls(),
        [OpenerCall::ShowInFileManager(
            "/usr/bin/foo".to_owned(),
            true
        )]
    );
}

#[test]
fn targetless_launch_deadline_and_admin() {
    for as_admin in [false, true] {
        let fake = Arc::new(FakeOpeners {
            block: Duration::from_secs(3),
            ..FakeOpeners::new()
        });
        let mut h = openers_harness(&fake);
        h.submit(req(1, launch_kind(as_admin), &[]));
        h.run();
        assert_eq!(h.events_for(ActionId(1)), [(None, ActionState::Started)]);
        h.advance(Duration::from_secs(2));
        if as_admin {
            // The 120 s deadline has not passed: the launch finishes at 3 s.
            assert!(h.events_for(ActionId(1)).is_empty());
            h.advance(Duration::from_secs(1));
            assert_eq!(h.events_for(ActionId(1)), [(None, ActionState::Succeeded)]);
        } else {
            assert_eq!(h.events_for(ActionId(1)), [(None, timed_out())]);
        }
        assert_eq!(h.finished(), [ActionId(1)]);
        assert_eq!(fake.calls().len(), 1, "as_admin {as_admin}");
        h.passed();
    }
}

#[test]
fn targetless_error_passthrough() {
    let err = ActionError::Internal(Arc::from("not an absolute path"));
    let fake = Arc::new(FakeOpeners {
        result: Err(err.clone()),
        launch: Err(ActionError::AuthCancelled),
        ..FakeOpeners::new()
    });
    let events = targetless_events(&fake, ActionKind::OpenUri(Arc::from("relative/path")));
    assert_eq!(events, summary_only(failed(err)));
    let events = targetless_events(&fake, launch_kind(true));
    assert_eq!(events, summary_only(failed(ActionError::AuthCancelled)));
}

#[test]
fn targetless_already_running() {
    let fake = Arc::new(FakeOpeners {
        launch: Ok(LaunchOutcome::AlreadyRunning),
        ..FakeOpeners::new()
    });
    let events = targetless_events(&fake, launch_kind(false));
    assert_eq!(
        events,
        summary_only(ActionState::Rejected(RejectReason::AlreadyRunning))
    );
}

#[test]
fn targetless_without_openers() {
    let kinds = [
        ActionKind::OpenUri(Arc::from("https://example.org")),
        ActionKind::ShowInFileManager {
            path: Arc::from("/usr/bin/foo"),
            properties: false,
        },
        launch_kind(false),
    ];
    for kind in kinds {
        let mut h = harness();
        h.submit(req(1, kind, &[]));
        h.run();
        assert_eq!(
            h.events_for(ActionId(1)),
            summary_only(ActionState::Rejected(RejectReason::Unsupported))
        );
        assert_eq!(h.finished(), [ActionId(1)]);
        h.passed();
    }
}

// Unit jobs through the real Gate (E11-011, E11-A1) -----------------------------------------

#[derive(Clone, PartialEq, Eq, Debug)]
enum FakeBusCall {
    JobRemoved,
    /// A manager method on `unit`, answered with job number `job`.
    Call {
        method: String,
        unit: String,
        job: u32,
    },
}

/// One manager. Every job call answers with a fresh job path `JOB_PREFIX<n>`, n counting from
/// 1. Each `job_removed` stream gets every signal `finish` sends after it opened, and stays open.
struct FakeUnitBus {
    calls: Mutex<Vec<FakeBusCall>>,
    next_job: Mutex<u32>,
    streams: Mutex<Vec<async_channel::Sender<JobRemovedSignal>>>,
}

const JOB_PREFIX: &str = "/org/freedesktop/systemd1/job/";

impl FakeUnitBus {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            next_job: Mutex::new(0),
            streams: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<FakeBusCall> {
        lock(&self.calls).clone()
    }

    /// The job number of the latest call on `unit`, if any.
    fn job_of(&self, unit: &str) -> Option<u32> {
        lock(&self.calls).iter().rev().find_map(|c| match c {
            FakeBusCall::Call { unit: u, job, .. } if u == unit => Some(*job),
            _ => None,
        })
    }

    /// Sends `JobRemoved` for job `job` with `result` to every open stream.
    fn finish(&self, job: u32, unit: &str, result: &str) {
        let signal = JobRemovedSignal {
            id: job,
            job: Arc::from(format!("{JOB_PREFIX}{job}")),
            unit: Arc::from(unit),
            result: Arc::from(result),
        };
        // A stream dropped by a finished job closes its receiver.
        lock(&self.streams).retain(|tx| tx.try_send(signal.clone()).is_ok());
    }
}

impl UnitBus for FakeUnitBus {
    fn call<'a>(
        &'a self,
        _cx: &'a Cx,
        method: &'a str,
        args: &'a [Value<'static>],
        _timeout: Duration,
    ) -> BusFuture<'a, OwnedValue> {
        let job = {
            let mut next = lock(&self.next_job);
            *next += 1;
            *next
        };
        let unit = match args.first() {
            Some(Value::Str(s)) => s.to_string(),
            other => panic!("unit name expected, got {other:?}"),
        };
        lock(&self.calls).push(FakeBusCall::Call {
            method: method.to_owned(),
            unit,
            job,
        });
        let path = format!("{JOB_PREFIX}{job}");
        let reply = Value::from(ObjectPath::try_from(path).expect("a valid path"))
            .try_into_owned()
            .map_err(|_| UnitBusError::Closed);
        Box::pin(async move { reply })
    }

    fn job_removed<'a>(&'a self, _cx: &'a Cx) -> BusFuture<'a, JobStream> {
        lock(&self.calls).push(FakeBusCall::JobRemoved);
        let (tx, rx) = async_channel::unbounded();
        lock(&self.streams).push(tx);
        Box::pin(async move { Ok(JobStream::new(rx)) })
    }

    fn get_property<'a>(
        &'a self,
        _cx: &'a Cx,
        _unit_path: &'a str,
        _iface: &'a str,
        _name: &'a str,
    ) -> BusFuture<'a, OwnedValue> {
        Box::pin(async { Err(UnitBusError::NoConnection) })
    }
}

/// A harness whose unit targets go to `system` or `user` by manager, with the real `Gate`.
fn unit_harness(config: LabConfig) -> (Harness, Arc<FakeUnitBus>, Arc<FakeUnitBus>) {
    let (system, user) = (FakeUnitBus::new(), FakeUnitBus::new());
    let mut h = harness_full(config, None, Some(bus_override(&system, &user)));
    h.gate = Some(Arc::new(Mutex::new(Gate::new(Arc::clone(&h.actx)))));
    (h, system, user)
}

/// Sends each manager's unit calls to its fake bus.
fn bus_override(system: &Arc<FakeUnitBus>, user: &Arc<FakeUnitBus>) -> UnitBusOverride {
    let (s, u) = (Arc::clone(system), Arc::clone(user));
    Arc::new(move |manager| {
        let bus = match manager {
            Manager::System => Arc::clone(&s),
            Manager::User => Arc::clone(&u),
        };
        Some(bus as Arc<dyn UnitBus>)
    })
}

fn unit_target(manager: Manager, name: &str) -> ActionTarget {
    ActionTarget::Unit(UnitRef {
        manager,
        name: Arc::from(name),
    })
}

fn job_failed(result: &str) -> ActionState {
    failed(ActionError::UnitJob {
        result: Arc::from(result),
        error_name: None,
    })
}

#[test]
fn busy_unit_second_action_rejected() {
    let (mut h, _system, user) = unit_harness(LabConfig::new(7).max_steps(1_000_000));
    let app = unit_target(Manager::User, "app.service");
    h.submit(req(1, ActionKind::UnitStart, std::slice::from_ref(&app)));
    h.run();
    assert_eq!(h.events_for(ActionId(1)), [(None, ActionState::Started)]);

    // Action 1 holds the unit while it waits for `JobRemoved`.
    h.submit(req(2, ActionKind::UnitStart, std::slice::from_ref(&app)));
    h.run();
    let busy = ActionState::Rejected(RejectReason::Busy);
    // The gate rejects the one target, and the one-target summary repeats its state.
    assert_eq!(
        h.events_for(ActionId(2)),
        expected(std::slice::from_ref(&busy), busy.clone())
    );
    assert_eq!(h.finished(), [ActionId(2)]);
    assert_eq!(
        user.calls(),
        [
            FakeBusCall::JobRemoved,
            FakeBusCall::Call {
                method: "StartUnit".to_owned(),
                unit: "app.service".to_owned(),
                job: 1,
            },
        ]
    );

    // A whole tick with no signal: the wait has no client timeout.
    h.advance(Duration::from_secs(1));
    assert!(h.events_for(ActionId(1)).is_empty());
    user.finish(1, "app.service", "done");
    h.run();
    assert_eq!(
        h.events_for(ActionId(1)),
        [
            (Some(0), ActionState::Succeeded),
            (None, ActionState::Succeeded)
        ]
    );
    assert_eq!(h.finished(), [ActionId(1)]);

    // `ActionFinished` freed the unit.
    h.submit(req(3, ActionKind::UnitStart, std::slice::from_ref(&app)));
    h.run();
    assert_eq!(user.job_of("app.service"), Some(2));
    user.finish(2, "app.service", "done");
    h.run();
    assert_eq!(
        h.events_for(ActionId(3)),
        expected(&[ActionState::Succeeded], ActionState::Succeeded)
    );
    assert_eq!(h.finished(), [ActionId(3)]);
    h.passed();
}

/// Moves the clock 100 ms at a time, at most 50 times, until `bus` has a call on `unit` other
/// than job `not`. Chaos delays can hold a task back past one `run`.
fn wait_for_job(h: &mut Harness, bus: &FakeUnitBus, unit: &str, not: Option<u32>) -> u32 {
    for _ in 0..50 {
        match bus.job_of(unit) {
            Some(job) if Some(job) != not => return job,
            _ => h.advance(Duration::from_millis(100)),
        }
    }
    panic!("no job call on {unit}");
}

/// Mirrors `#[explore_seeds(seeds = 0..16, workers = 2, chaos)]` the way
/// `explore_signal_pipeline` does, with no cancel injection. Two units on two managers, job
/// signals out of order, and signals for other jobs in between.
#[test]
fn explore_unit_jobs() {
    for seed in 0..16 {
        let chaos = ChaosConfig::default()
            .with_cancel_probability(0.0)
            .with_delay_probability(0.05)
            .with_delay_range(Duration::ZERO..Duration::from_millis(10))
            .with_wakeup_storm_probability(0.01);
        let (mut h, system, user) = unit_harness(
            LabConfig::new(seed)
                .worker_count(2)
                .max_steps(1_000_000)
                .with_chaos(chaos),
        );
        let a = unit_target(Manager::User, "a.service");
        let b = unit_target(Manager::System, "b.service");
        h.submit(req(1, ActionKind::UnitStart, std::slice::from_ref(&a)));
        h.submit(req(2, ActionKind::UnitRestart, std::slice::from_ref(&b)));
        h.run();
        let job_a = wait_for_job(&mut h, &user, "a.service", None);
        let job_b = wait_for_job(&mut h, &system, "b.service", None);
        // Other jobs first, then b before a.
        user.finish(job_a + 100, "other.service", "failed");
        system.finish(job_b + 100, "other.service", "done");
        system.finish(job_b, "b.service", "failed");
        h.advance(Duration::from_millis(600));
        user.finish(job_a, "a.service", "done");
        h.advance(Duration::from_secs(1));
        let mut done = h.finished();
        done.sort_by_key(|id| id.0);
        assert_eq!(done, [ActionId(1), ActionId(2)], "seed {seed}");

        // One action on both units, then a start on a while action 3 holds it.
        h.submit(req(3, ActionKind::UnitStop, &[a.clone(), b.clone()]));
        h.run();
        let stop_a = wait_for_job(&mut h, &user, "a.service", Some(job_a));
        h.submit(req(4, ActionKind::UnitStart, std::slice::from_ref(&a)));
        h.run();
        // A repeat of a finished job's signal is skipped.
        user.finish(job_a, "a.service", "done");
        user.finish(stop_a, "a.service", "done");
        h.advance(Duration::from_millis(100));
        let stop_b = wait_for_job(&mut h, &system, "b.service", Some(job_b));
        system.finish(stop_b, "b.service", "done");
        h.advance(Duration::from_secs(1));

        let busy = ActionState::Rejected(RejectReason::Busy);
        let want = [
            (1, vec![ActionState::Succeeded], ActionState::Succeeded),
            (2, vec![job_failed("failed")], job_failed("failed")),
            (
                3,
                vec![ActionState::Succeeded, ActionState::Succeeded],
                ActionState::Succeeded,
            ),
            (4, vec![busy.clone()], busy),
        ];
        for (id, states, summary) in want {
            assert_eq!(
                h.events_for(ActionId(id)),
                expected(&states, summary),
                "seed {seed} action {id}"
            );
        }
        let mut done = h.finished();
        done.sort_by_key(|id| id.0);
        assert_eq!(done, [ActionId(3), ActionId(4)], "seed {seed}");
        assert_eq!(
            user.calls()
                .iter()
                .filter(|c| **c == FakeBusCall::JobRemoved)
                .count(),
            2,
            "seed {seed}: action 4 never reached the bus"
        );
        h.passed();
    }
}

// Engine level (E11-A1) ----------------------------------------------------------------------
//
// `engine_root` runs in a `FakeLab` with no workers. The real `Gate` comes from `gate_with_ops`
// over `FakeOps`, wrapped so that a request with `PANIC_ID` panics inside the reducer. The test
// sends the `Processes` sweep itself, so the reducer knows the live keys.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::sync::atomic::{AtomicU32, Ordering};

use asupersync::channel::watch;
use asupersync::runtime::TaskHandle;
use tl_engine::actions::gate_with_ops;
use tl_engine::bridge::{EngineHooks, HookCtx};
use tl_engine::session::{RESTART_DELAY, engine_root};
use tl_engine::source::{QueuedCommand, WorkerCommand, WorkerControl};
use tl_engine::update::{ProcessSweep, SourceUpdate};
use tl_model::actions::SchedPolicy;
use tl_model::command::Command;
use tl_model::health::SourceId;
use tl_model::process::{GroupKind, ProcState, ProcessExtras, ProcessMeta, ProcessRecord};
use tl_model::validity::{FieldValidity, Validity};

/// A request with this id panics inside the reducer.
const PANIC_ID: ActionId = ActionId(999);

/// The real gate, with one scripted reducer panic.
struct PanicOn(Box<dyn ActionGate>);

impl ActionGate for PanicOn {
    fn submit(&mut self, cx: &Cx, req: ActionRequest, view: &AdmissionView<'_>) {
        assert_ne!(req.id, PANIC_ID, "scripted reducer panic");
        self.0.submit(cx, req, view);
    }

    fn finished(&mut self, id: ActionId) -> Option<SourceId> {
        self.0.finished(id)
    }
}

struct EngineRig {
    fake: fake_sources::FakeLab,
    ops: Arc<FakeOps>,
    ui: fake_sources::UiSide,
    control: watch::Receiver<WorkerControl>,
    root: TaskHandle<()>,
    /// The engine's own event sender, so a test can fill the queue.
    events_tx: async_channel::Sender<ActionEvent>,
    /// Gate factory calls, one per session.
    sessions: Arc<AtomicU32>,
    /// Events read from `ui.events_rx` and not yet asked for.
    seen: Vec<ActionEvent>,
}

fn engine_rig() -> EngineRig {
    engine_rig_with(|hook, ops| Box::new(PanicOn(gate_with_ops(hook, ops))))
}

/// `engine_rig` with `gate` building each session's gate from the hook and the rig's `FakeOps`.
fn engine_rig_with(
    gate: impl Fn(HookCtx, Arc<dyn ActionOps>) -> Box<dyn ActionGate> + Send + Sync + 'static,
) -> EngineRig {
    let mut fake = fake_sources::FakeLab::with_config(LabConfig::new(7).max_steps(1_000_000));
    let ops = Arc::new(FakeOps::new(&fake.clock));
    let config = Arc::new(EngineConfig::new(
        EngineMode::Full,
        PathBuf::from("/nonexistent"),
    ));
    let (mut ep, ui) = fake_sources::root_endpoints(config, Vec::new());
    let control = ep.control.subscribe();
    let events_tx = ep.events_tx.clone();
    let sessions = Arc::new(AtomicU32::new(0));
    let (counter, gate_ops) = (Arc::clone(&sessions), Arc::clone(&ops));
    ep.hooks = EngineHooks {
        action_gate: Some(Arc::new(move |hook: HookCtx| {
            counter.fetch_add(1, Ordering::Relaxed);
            let ops: Arc<dyn ActionOps> = gate_ops.clone();
            gate(hook, ops)
        })),
        ..EngineHooks::default()
    };
    let root = fake.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        engine_root(cx, ep).await;
    });
    EngineRig {
        fake,
        ops,
        ui,
        control,
        root,
        events_tx,
        sessions,
        seen: Vec::new(),
    }
}

impl EngineRig {
    fn send(&mut self, msg: EngineMsg) {
        self.ui
            .msgs_tx
            .try_send(msg)
            .expect("the queue has room for the message");
        self.fake.lab.run_until_idle();
    }

    fn action(&mut self, id: u64, kind: ActionKind, pids: &[u32]) {
        self.queue_action(id, kind, pids);
        self.fake.lab.run_until_idle();
    }

    /// Queues the action on PIDs from `engine_key` without running the lab.
    fn queue_action(&mut self, id: u64, kind: ActionKind, pids: &[u32]) {
        let targets: Vec<ActionTarget> = pids
            .iter()
            .map(|p| ActionTarget::Process(engine_key(*p)))
            .collect();
        self.ui
            .msgs_tx
            .try_send(EngineMsg::Command(Command::Action(req(id, kind, &targets))))
            .expect("the queue has room for the message");
    }

    /// Sends the action on `targets` and runs the lab.
    fn action_on(&mut self, id: u64, kind: ActionKind, targets: &[ActionTarget]) {
        self.send(EngineMsg::Command(Command::Action(req(id, kind, targets))));
    }

    /// One `Processes` sweep with these PIDs, so admission sees them live.
    fn sweep(&mut self, pids: &[u32]) {
        let t_ns = self.fake.clock.now().as_nanos();
        self.send(EngineMsg::Source(SourceUpdate::Processes(ProcessSweep {
            observed_start: t_ns,
            observed_end: t_ns,
            records: pids.iter().map(|p| sweep_record(*p)).collect(),
            extras: pids.iter().map(|_| sweep_extras()).collect(),
            extras_p2: Vec::new(),
            app_stats: Vec::new(),
            unit_stats: Vec::new(),
            system_threads: 0,
            file_nr: None,
            online_cpus: 1,
            low_memory: None,
        })));
    }

    /// Every event for `id` so far, as `(target, state)`.
    fn events_for(&mut self, id: ActionId) -> Vec<(Option<u16>, ActionState)> {
        while let Ok(ev) = self.ui.events_rx.try_recv() {
            self.seen.push(ev);
        }
        let (mine, rest) = std::mem::take(&mut self.seen)
            .into_iter()
            .partition(|ev| ev.id == id);
        self.seen = rest;
        mine.into_iter().map(|ev| (ev.target, ev.state)).collect()
    }

    /// The `RefreshNow` commands forwarded to `Processes` so far.
    fn process_refreshes(&self) -> usize {
        self.refreshes(SourceId::Processes)
    }

    /// The `RefreshNow` commands forwarded to `source` so far.
    fn refreshes(&self, source: SourceId) -> usize {
        let want =
            |q: &&QueuedCommand| q.target == source && q.command == WorkerCommand::RefreshNow;
        self.control
            .borrow_and_clone()
            .commands
            .iter()
            .filter(want)
            .count()
    }

    fn sessions(&self) -> u32 {
        self.sessions.load(Ordering::Relaxed)
    }

    /// Cancels the root, runs to quiescence and checks the lab's oracles.
    fn finish(mut self) {
        self.root.abort();
        self.fake.advance(Duration::from_millis(10));
        assert!(self.root.is_finished(), "engine_root returned after cancel");
        self.fake.finish();
    }
}

/// `start_ticks = pid × 10`, so ascending PIDs are sorted by key.
fn engine_key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: u64::from(pid) * 10,
        boot_id: fake_sources::FAKE_BOOT,
        pid,
    }
}

fn sweep_record(pid: u32) -> ProcessRecord {
    ProcessRecord {
        key: engine_key(pid),
        ppid: 0,
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
            search_lower: "x user".into(),
        }),
        group: GroupKind::Background.key(),
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

fn sweep_extras() -> ProcessExtras {
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
        unit: None,
        state_letter: b'S',
        frozen: false,
        disk_read_bps: 0.0,
        disk_write_raw_bps: 0.0,
        flag_since_ns: 0,
    }
}

#[test]
fn engine_action_end_to_end() {
    let mut rig = engine_rig();
    assert_eq!(rig.sessions(), 1);
    rig.sweep(&[500]);
    assert_eq!(rig.process_refreshes(), 0);
    rig.action(1, ActionKind::Signal(Signal::Term), &[500]);
    assert_eq!(
        rig.events_for(ActionId(1)),
        expected(&[ActionState::Succeeded], ActionState::Succeeded)
    );
    assert_eq!(
        rig.ops.calls(),
        [FakeCall::Open(500), FakeCall::Send(500, Signal::Term)]
    );
    // `ActionFinished` reached the reducer, which asked `Processes` for a fresh sweep.
    assert_eq!(rig.process_refreshes(), 1);
    rig.finish();
}

#[test]
fn engine_seventeenth_busy() {
    let mut rig = engine_rig();
    let pids: Vec<u32> = (1001..=1018).collect();
    rig.sweep(&pids);
    // All 17 wait in the queue, and the reducer applies them in one drain. Each `ActionFinished`
    // queues behind the 17th, so the gate still counts 16 in flight when it sees the 17th.
    for id in 1..=17 {
        let pid = 1000 + id as u32;
        rig.ops
            .script(pid, &[FakeStep::Block(Duration::from_secs(10))]);
        rig.queue_action(id, ActionKind::Signal(Signal::Term), &[pid]);
    }
    rig.fake.lab.run_until_idle();
    assert_eq!(
        rig.events_for(ActionId(17)),
        [(None, ActionState::Rejected(RejectReason::Busy))]
    );
    assert!(!rig.ops.calls().contains(&FakeCall::Open(1017)));

    // Each job moved the clock 10 s, past its 2 s deadline.
    rig.fake.advance(Duration::ZERO);
    for id in 1..=16 {
        assert_eq!(
            rig.events_for(ActionId(id)),
            expected(&[timed_out()], timed_out()),
            "action {id}"
        );
    }
    assert_eq!(rig.process_refreshes(), 16);
    assert!(rig.events_for(ActionId(17)).is_empty(), "one event only");
    // The freed slots admit the next one.
    rig.action(18, ActionKind::Signal(Signal::Term), &[1018]);
    assert_eq!(
        rig.events_for(ActionId(18)),
        expected(&[ActionState::Succeeded], ActionState::Succeeded)
    );
    rig.finish();
}

#[test]
fn session_restart_drops_in_flight() {
    let mut rig = engine_rig();
    rig.sweep(&[500, 501]);
    // A full event queue blocks the action in its `Started` send for up to 1 s.
    let fill = ActionEvent {
        id: ActionId(77),
        target: None,
        state: ActionState::Started,
    };
    for _ in 0..256 {
        rig.events_tx.try_send(fill.clone()).expect("room left");
    }
    rig.action(1, ActionKind::Signal(Signal::Term), &[500]);
    assert!(
        rig.ops.calls().is_empty(),
        "the action waits on its first event"
    );

    rig.send(EngineMsg::Command(Command::Action(req(
        PANIC_ID.0,
        ActionKind::Signal(Signal::Term),
        &[],
    ))));
    rig.fake.advance(RESTART_DELAY);
    assert_eq!(rig.sessions(), 2, "session 2 starts after 1 s");

    // Long past every 1 s send timeout and the 2 s deadline: session 1's gate aborted the task.
    rig.fake
        .run_for(Duration::from_secs(30), Duration::from_secs(1));
    assert_eq!(rig.events_for(ActionId(77)).len(), 256);
    assert!(
        rig.events_for(ActionId(1)).is_empty(),
        "no event for the old id"
    );
    assert!(rig.events_for(PANIC_ID).is_empty());
    assert!(rig.ops.calls().is_empty(), "the old action never ran");
    assert_eq!(rig.process_refreshes(), 0);

    // The new reducer starts empty, so it needs a sweep before it admits PID 501.
    rig.sweep(&[501]);
    rig.action(2, ActionKind::Signal(Signal::Term), &[501]);
    assert_eq!(
        rig.events_for(ActionId(2)),
        expected(&[ActionState::Succeeded], ActionState::Succeeded)
    );
    assert_eq!(rig.process_refreshes(), 1);
    rig.finish();
}

// Unit, startup and cgroup actions through the engine (E11-A2) -------------------------------

use tl_engine::actions::RealOps;
use tl_engine::actions::startup::StartupPaths;
use tl_model::actions::CgroupRef;
use tl_ops::cgroup::CgroupDir;
use tl_ops::fs::StartupRoots;

/// A real `Gate` as `gate_with_ops` builds it, with the test's ops, unit buses and startup
/// paths.
fn test_gate(
    hook: HookCtx,
    ops: Arc<dyn ActionOps>,
    units: Option<UnitBusOverride>,
    startup_paths: StartupPaths,
) -> Box<dyn ActionGate> {
    Box::new(Gate::new(Arc::new(ActionCtx {
        events: hook.events_tx,
        msgs: hook.msgs_tx,
        ops,
        boot_id: hook.boot_id,
        uid: 1000,
        config: hook.config,
        systemd_system: hook.shared.systemd_system,
        systemd_user: hook.shared.systemd_user,
        openers: None,
        helper: None,
        destroy_support_sent: AtomicBool::new(false),
        unit_bus_override: units,
        startup_paths,
    })))
}

/// A directory under `target/tmp`, removed on drop.
fn scratch() -> tempfile::TempDir {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&base).expect("create target/tmp");
    tempfile::Builder::new()
        .prefix("tasklens-i3o2-")
        .tempdir_in(base)
        .expect("tempdir")
}

/// `$XDG_CONFIG_HOME` is `<base>/config`, and `<base>/etc/xdg/autostart` is the only system
/// directory.
fn scratch_paths(base: &std::path::Path) -> StartupPaths {
    StartupPaths {
        roots: StartupRoots::from_config_home(&base.join("config")),
        system_autostart_dirs: vec![base.join("etc/xdg/autostart")],
    }
}

#[test]
fn engine_unit_action_end_to_end() {
    let (system, user) = (FakeUnitBus::new(), FakeUnitBus::new());
    let units = bus_override(&system, &user);
    let paths = scratch_paths(std::path::Path::new("/nonexistent"));
    let mut rig = engine_rig_with(move |hook, ops| {
        test_gate(hook, ops, Some(Arc::clone(&units)), paths.clone())
    });
    let foo = unit_target(Manager::User, "foo.service");
    rig.action_on(1, ActionKind::UnitStart, std::slice::from_ref(&foo));
    assert_eq!(rig.events_for(ActionId(1)), [(None, ActionState::Started)]);
    let job = user.job_of("foo.service").expect("StartUnit was called");

    // Action 1 holds the unit while it waits for `JobRemoved`.
    rig.action_on(2, ActionKind::UnitStart, std::slice::from_ref(&foo));
    let busy = ActionState::Rejected(RejectReason::Busy);
    assert_eq!(
        rig.events_for(ActionId(2)),
        expected(std::slice::from_ref(&busy), busy.clone())
    );
    assert_eq!(user.job_of("foo.service"), Some(job), "no second job call");
    let before = rig.refreshes(SourceId::SystemdUser);

    user.finish(job, "foo.service", "done");
    rig.fake.lab.run_until_idle();
    assert_eq!(
        rig.events_for(ActionId(1)),
        [
            (Some(0), ActionState::Succeeded),
            (None, ActionState::Succeeded)
        ]
    );
    // `ActionFinished` asked the user manager source for a fresh read.
    assert_eq!(rig.refreshes(SourceId::SystemdUser), before + 1);
    assert_eq!(rig.refreshes(SourceId::SystemdSystem), 0);
    assert!(system.calls().is_empty());
    rig.finish();
}

#[test]
fn engine_startup_action_end_to_end() {
    let dir = scratch();
    let paths = scratch_paths(dir.path());
    let file = paths.roots.autostart.join("foo.desktop");
    std::fs::create_dir_all(&paths.roots.autostart).expect("create autostart");
    std::fs::write(
        &file,
        "[Desktop Entry]\nType=Application\nName=Foo\nExec=true\n",
    )
    .expect("write entry");
    let gate_paths = paths.clone();
    let mut rig = engine_rig_with(move |hook, _| {
        test_gate(hook, Arc::new(RealOps), None, gate_paths.clone())
    });
    let entry = ActionTarget::Startup(StartupRef {
        source: StartupSource::AutostartUser,
        id: Arc::from("foo.desktop"),
    });
    rig.action_on(1, ActionKind::StartupDisable, &[entry]);
    assert_eq!(
        rig.events_for(ActionId(1)),
        expected(&[ActionState::Succeeded], ActionState::Succeeded)
    );
    let text = std::fs::read_to_string(&file).expect("read entry");
    assert!(text.contains("Hidden=true"), "{text}");
    assert_eq!(rig.refreshes(SourceId::Startup), 1);
    rig.finish();
}

/// Cgroup ops over a scratch directory, so nothing writes to a real cgroup.
struct CgroupFake {
    dir: PathBuf,
    writes: Mutex<Vec<(String, String)>>,
}

impl ActionOps for CgroupFake {
    fn cgroup_open_validated(&self, _cg: &CgroupRef, _uid: u32) -> Result<CgroupDir, OpsError> {
        let dir = std::fs::File::open(&self.dir)
            .map_err(|e| OpsError::Os(e.raw_os_error().unwrap_or(Errno::IO.raw_os_error())))?;
        Ok(CgroupDir::from_validated_fd(OwnedFd::from(dir)))
    }

    fn cgroup_write(&self, _dir: &CgroupDir, file: &str, value: &str) -> Result<(), OpsError> {
        lock(&self.writes).push((file.to_owned(), value.to_owned()));
        Ok(())
    }

    fn cgroup_wait_frozen(&self, _dir: &CgroupDir, _wait: Duration) -> Result<(), OpsError> {
        Ok(())
    }
}

#[test]
fn engine_cgroup_action_end_to_end() {
    let dir = scratch();
    let ops = Arc::new(CgroupFake {
        dir: dir.path().to_owned(),
        writes: Mutex::new(Vec::new()),
    });
    let gate_ops = Arc::clone(&ops);
    let paths = scratch_paths(dir.path());
    let mut rig =
        engine_rig_with(move |hook, _| test_gate(hook, gate_ops.clone(), None, paths.clone()));
    let cg = ActionTarget::Cgroup(CgroupRef {
        rel_path: Arc::from("user.slice/user-1000.slice/user@1000.service/app.slice/app-x.scope"),
        cgroup_id: 7,
    });
    rig.action_on(1, ActionKind::FreezeGroup, &[cg]);
    assert_eq!(
        rig.events_for(ActionId(1)),
        expected(&[ActionState::Succeeded], ActionState::Succeeded)
    );
    assert_eq!(
        *lock(&ops.writes),
        [("cgroup.freeze".to_owned(), "1".to_owned())]
    );
    assert_eq!(rig.process_refreshes(), 1);
    rig.finish();
}
