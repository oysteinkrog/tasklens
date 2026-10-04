//! LabRuntime tests for the per-worker supervisor (ADR-004, ADR-013).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use asupersync::Cx;
use asupersync::channel::oneshot;
use asupersync::lab::{LabConfig, LabRuntime};
use asupersync::runtime::JoinError;
use asupersync::supervision::{ManagedGeneration, ManagedSupervisorReport};
use asupersync::types::{Budget, Outcome, RegionId};
use tl_engine::supervise::{WorkerFactory, budget_exhausted, build_supervisor, run_supervised};
use tl_engine::update::WorkerError;

type Report = ManagedSupervisorReport<WorkerError>;
type Starts = Arc<Mutex<Vec<(u64, u64)>>>;
type Slot = Arc<Mutex<Option<Report>>>;

/// What a generation does after it records its start.
#[derive(Clone, Copy)]
enum Step {
    Fail,
    Panic,
    Finish,
    WaitForCancel,
}

/// A factory that records `(generation, start nanos)` and runs `plan(generation)`.
fn factory(starts: Starts, plan: fn(u64) -> Step) -> WorkerFactory {
    Arc::new(move |cx: Cx, generation: ManagedGeneration| {
        let starts = Arc::clone(&starts);
        async move {
            let number = generation.number;
            starts
                .lock()
                .expect("starts lock")
                .push((number, cx.now().as_nanos()));
            match plan(number) {
                Step::Fail => Outcome::Err(WorkerError::Other("fail".into())),
                Step::Panic => panic!("generation {number} panics"),
                Step::Finish => Outcome::Ok(()),
                Step::WaitForCancel => {
                    // `_keep` holds the sender open, so `recv` ends only on cancel.
                    let (_keep, mut rx) = oneshot::channel::<()>();
                    let _ = rx.recv(&cx).await;
                    Outcome::Ok(())
                }
            }
        }
    })
}

/// The lab's root region, created on first use. A lab has only one.
fn root(lab: &mut LabRuntime) -> RegionId {
    match lab.state.root_region {
        Some(root) => root,
        None => lab.state.create_root_region(Budget::INFINITE),
    }
}

fn lab(seed: u64) -> LabRuntime {
    LabRuntime::new(LabConfig::new(seed).max_steps(1_000_000))
}

/// Runs `run_supervised` as a root task and returns where its report lands.
fn spawn_run_supervised(lab: &mut LabRuntime, name: &'static str, factory: WorkerFactory) -> Slot {
    let slot: Slot = Arc::new(Mutex::new(None));
    let out = Arc::clone(&slot);
    let root = root(lab);
    let (task, _join) = lab
        .state
        .create_task(root, Budget::INFINITE, async move {
            let cx = Cx::current().expect("lab task has a Cx");
            let report = run_supervised(&cx, name, factory)
                .await
                .expect("supervisor runs");
            *out.lock().expect("slot lock") = Some(report);
        })
        .expect("create task");
    lab.scheduler.lock().schedule(task, 0);
    slot
}

/// Spawns the supervisor as a root task that aborts its handle when the returned sender fires.
fn spawn_abortable(
    lab: &mut LabRuntime,
    name: &'static str,
    factory: WorkerFactory,
) -> (oneshot::Sender<()>, Slot) {
    let slot: Slot = Arc::new(Mutex::new(None));
    let out = Arc::clone(&slot);
    let (abort_tx, mut abort_rx) = oneshot::channel::<()>();
    let root = root(lab);
    let (task, _join) = lab
        .state
        .create_task(root, Budget::INFINITE, async move {
            let cx = Cx::current().expect("lab task has a Cx");
            let supervisor = build_supervisor(name, factory).expect("supervisor builds");
            let mut handle = supervisor.spawn(&cx).expect("supervisor spawns");
            // A chaos cancel also ends the wait; abort either way.
            let _ = abort_rx.recv(&cx).await;
            handle.abort();
            let report = handle.join().await.expect("report");
            *out.lock().expect("slot lock") = Some(report);
        })
        .expect("create task");
    lab.scheduler.lock().schedule(task, 0);
    (abort_tx, slot)
}

fn take(slot: &Slot) -> Report {
    slot.lock()
        .expect("slot lock")
        .take()
        .expect("supervisor published a report")
}

fn offsets_ms(starts: &Starts) -> Vec<u64> {
    let starts = starts.lock().expect("starts lock");
    let first = starts.first().map_or(0, |s| s.1);
    starts
        .iter()
        .map(|s| Duration::from_nanos(s.1 - first).as_millis() as u64)
        .collect()
}

/// Light chaos delays a poll by up to 10 ms of virtual time, so under chaos
/// each restart may start a little after its backoff.
const CHAOS_SLACK_MS: u64 = 50;

const BACKOFF_SCHEDULE_MS: [u64; 5] = [0, 500, 1_500, 3_500, 7_500];
const BUDGET_SCHEDULE_MS: [u64; 6] = [0, 500, 1_500, 3_500, 7_500, 15_500];

/// Checks start offsets against `expected` ms. Each gap between starts is at
/// least the backoff and at most `slack_ms` longer. With `chaos`, a prefix is
/// enough, because a chaos cancel may end the run early.
fn assert_schedule(starts: &Starts, expected: &[u64], slack_ms: u64, chaos: bool) {
    let got = offsets_ms(starts);
    if chaos {
        assert!(
            !got.is_empty() && got.len() <= expected.len(),
            "starts {got:?}"
        );
    } else {
        assert_eq!(got.len(), expected.len(), "starts {got:?}");
    }
    for i in 1..got.len() {
        let gap = got[i] - got[i - 1];
        let want = expected[i] - expected[i - 1];
        assert!(
            gap >= want && gap <= want + slack_ms,
            "starts {got:?}, expected {expected:?} with {slack_ms} ms slack"
        );
    }
}

/// True when a cancel, not a failure, ended the run: the controller or the
/// last generation was cancelled.
fn cancelled(report: &Report) -> bool {
    report.outcome.is_cancelled()
        || report.children.iter().any(|child| {
            child.outcome.is_cancelled()
                || matches!(child.task_outcome, Err(JoinError::Cancelled(_)))
        })
}

/// The counts in `report` agree with the recorded starts.
fn assert_counts(starts: &Starts, report: &Report) {
    let recorded = starts.lock().expect("starts lock").len() as u64;
    assert_eq!(report.started, recorded);
    assert_eq!(report.joined, recorded);
    assert_eq!(report.restart_batches, recorded - 1);
}

fn fail_then_wait(generation: u64) -> Step {
    if generation <= 4 {
        Step::Fail
    } else {
        Step::WaitForCancel
    }
}

fn always_fail(_: u64) -> Step {
    Step::Fail
}

/// Case 1: four failures, then a generation that waits for cancel.
fn backoff_case(lab: &mut LabRuntime, starts: &Starts, chaos: bool) {
    let (abort, slot) =
        spawn_abortable(lab, "backoff", factory(Arc::clone(starts), fail_then_wait));
    lab.run_with_auto_advance();
    let slack = if chaos { CHAOS_SLACK_MS } else { 0 };
    assert_schedule(starts, &BACKOFF_SCHEDULE_MS, slack, chaos);
    // Under chaos the root task may already have stopped waiting.
    let _ = abort.send_blocking(());
    lab.run_until_idle();
    let report = take(&slot);
    assert_counts(starts, &report);
    if !chaos {
        assert_eq!(report.started, 5);
    }
    assert!(!budget_exhausted(&report));
}

/// Case 2: every generation fails, so 1 start plus 5 restarts, then stop.
fn budget_case(lab: &mut LabRuntime, starts: &Starts) -> Slot {
    spawn_run_supervised(lab, "budget", factory(Arc::clone(starts), always_fail))
}

/// Returns false when a chaos cancel ended the run early.
fn check_budget(starts: &Starts, slot: &Slot, chaos: bool) -> bool {
    let report = take(slot);
    let slack = if chaos { CHAOS_SLACK_MS } else { 0 };
    assert_schedule(starts, &BUDGET_SCHEDULE_MS, slack, chaos);
    assert_counts(starts, &report);
    if chaos && cancelled(&report) {
        assert!(!budget_exhausted(&report));
        return false;
    }
    assert_eq!(report.started, 6);
    assert_eq!(report.restart_batches, 5);
    assert!(matches!(report.outcome, Outcome::Ok(())));
    assert!(budget_exhausted(&report));
    true
}

#[test]
fn restart_backoff_timings() {
    let mut lab = lab(0x8d7_0001);
    let starts = Starts::default();
    backoff_case(&mut lab, &starts, false);
    assert!(lab.run_until_quiescent_with_report().lab_test_passed());
}

#[test]
fn budget_exhausted_after_five() {
    let mut lab = lab(0x8d7_0002);
    let starts = Starts::default();
    let slot = budget_case(&mut lab, &starts);
    lab.run_with_auto_advance();
    assert!(check_budget(&starts, &slot, false));
    assert!(lab.run_until_quiescent_with_report().lab_test_passed());
}

#[test]
fn panic_restarts_like_err() {
    let mut lab = lab(0x8d7_0003);
    let starts = Starts::default();
    let plan: fn(u64) -> Step = |g| if g == 1 { Step::Panic } else { Step::Finish };
    let slot = spawn_run_supervised(&mut lab, "panic", factory(Arc::clone(&starts), plan));
    lab.run_with_auto_advance();
    let report = take(&slot);
    assert_eq!(offsets_ms(&starts), [0, 500]);
    assert_eq!(report.started, 2);
    assert_eq!(report.restart_batches, 1);
    assert!(report.children[0].outcome.is_ok());
    assert!(!budget_exhausted(&report));
    assert!(lab.run_until_quiescent_with_report().lab_test_passed());
}

#[test]
fn no_restart_after_cancel() {
    let mut lab = lab(0x8d7_0004);
    let starts = Starts::default();
    let plan: fn(u64) -> Step = |_| Step::WaitForCancel;
    let (abort, slot) = spawn_abortable(&mut lab, "cancel", factory(Arc::clone(&starts), plan));
    lab.run_with_auto_advance();
    assert_eq!(starts.lock().expect("starts lock").len(), 1);
    abort.send_blocking(()).expect("abort send");
    lab.run_with_auto_advance();
    let report = take(&slot);
    assert_eq!(starts.lock().expect("starts lock").len(), 1);
    assert_eq!(report.started, 1);
    assert_eq!(report.joined, 1);
    assert_eq!(report.restart_batches, 0);
    assert!(report.outcome.is_cancelled());
    assert!(!budget_exhausted(&report));
    assert!(lab.run_until_quiescent_with_report().lab_test_passed());
}

/// Both supervisors share one lab, which also shows that one worker's
/// failures do not spend the other's budget.
///
/// asupersync 0.5.0 does not re-export `#[explore_seeds]`, so this loop builds
/// the same config the macro does: seeds 0..16, 2 workers, light chaos.
#[test]
fn explore_restart() {
    let mut full_runs = 0;
    for seed in 0..16 {
        let mut lab = LabRuntime::new(
            LabConfig::new(seed)
                .worker_count(2)
                .max_steps(1_000_000)
                .with_light_chaos(),
        );
        let budget_starts = Starts::default();
        let budget_slot = budget_case(&mut lab, &budget_starts);
        let backoff_starts = Starts::default();
        backoff_case(&mut lab, &backoff_starts, true);
        lab.run_with_auto_advance();
        if check_budget(&budget_starts, &budget_slot, true) {
            full_runs += 1;
        }
        assert!(
            lab.run_until_quiescent_with_report().lab_test_passed(),
            "seed {seed}"
        );
    }
    // Seeds are fixed, so this count is fixed too (9 of 16 today). At least
    // one seed must run the whole budget case without a chaos cancel.
    assert!(full_runs > 0);
}
