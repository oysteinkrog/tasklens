//! LabRuntime tests for the action pipeline (ADR-013, E11-007): admission limits, deadlines,
//! event order and summaries, and every P1 process kind, through `run_action` with a scripted
//! `FakeOps`.
//!
//! `Cx::spawn_blocking` runs the job inline in the lab and `ActionOps` gets no `Cx`, so a fake
//! job cannot await a sleep. The harness installs its own `VirtualClock` as the timer driver and
//! a `Block` step advances that clock while the job runs, as `fake_sources.rs` does. Drive time
//! only with `Harness::advance`.
//!
//! The `ActionGate` that the reducer calls is E11-A1. Until it lands, `Harness::submit` does
//! what that gate does: `admit` with the in-flight count, one `Rejected` event with
//! `target: None` on a reject, otherwise one `run_action` task. An action counts as in flight
//! until the test drains its `ActionFinished`, as in the reducer.

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
use tl_engine::actions::{
    ActionCtx, ActionOps, Admission, LaunchOutcome, Openers, admit, deadline, run_action, summarize,
};
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::dbus::ManagerConnSlot;
use tl_engine::reducer::AdmissionView;
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
}

fn harness() -> Harness {
    harness_with(LabConfig::new(7).max_steps(1_000_000))
}

fn harness_with(config: LabConfig) -> Harness {
    harness_full(config, None)
}

fn harness_full(config: LabConfig, openers: Option<Arc<dyn Openers>>) -> Harness {
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
    harness_full(LabConfig::new(7).max_steps(1_000_000), Some(openers))
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
