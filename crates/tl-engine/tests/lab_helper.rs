//! LabRuntime tests for the helper session task in `helper/mod.rs` (E29-A2, ADR-013).
//!
//! No test runs pkexec or anything as root: every test uses `FakeLink` through `LinkEnv`.
//!
//! The `privileged_*` tests run `run_action` with the real `HelperHandle` over the fake
//! (E29-014): the action pipeline, the `PrivilegedOps` impl and the helper task together.
//!
//! The four `EngineRig` tests run a whole engine session with the fake as `EngineHooks.helper`
//! (E29-020): commands go through the reducer and the helper state comes back in
//! `Snapshot.health`.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::collections::VecDeque;
use std::io;
use std::os::fd::BorrowedFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::lab::chaos::ChaosConfig;
use asupersync::runtime::TaskHandle;
use asupersync::time;
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::actions::startup::StartupPaths;
use tl_engine::actions::{ActionCtx, ActionOps, PrivilegedOps, dump, run_action};
use tl_engine::bridge::EngineHooks;
use tl_engine::config::{EngineConfig, EngineMode};
use tl_engine::dbus::ManagerConnSlot;
use tl_engine::helper::{
    HelperCallError, HelperHandle, HelperLink, HelperMsg, HelperReadError, LinkEnv, LinkError,
    StartError, helper_queue, helper_task_with,
};
use tl_engine::session::engine_root;
use tl_engine::update::{CpuCoreSample, CpuSample, EngineMsg, HelperUpdate, SourceUpdate};
use tl_model::actions::{
    ActionError, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget, CgroupRef,
    DumpMode, IoPriority, RejectReason, Signal,
};
use tl_model::command::{
    Command, RaplStatus, SetupPhase, SetupRun, SetupState, SetupStatus, SetupStep,
};
use tl_model::devices::DeviceCatalog;
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::helper_proto::{
    HELPER_PROTO_VERSION, HelperError, HelperFrame, HelperReply, HelperReplyBody, HelperRequest,
    HelperTarget,
};
use tl_model::history::RecordSink;
use tl_model::ids::ProcessKey;
use tl_model::ids::SeriesId;
use tl_model::process::ProcessRecord;
use tl_model::series::Sample;
use tl_model::snapshot::Snapshot;
use tl_model::system::{CpuSet, CpuTopology, CpuTotals, Metric};
use tl_model::table::{Manager, UnitRef};
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::Validity;

use fake_sources::{FakeLab, UiSide};

/// How the fake answers one request after the `Hello`.
#[derive(Clone, Copy)]
enum Answer {
    /// This many replies, the last with `last` set.
    Replies(u32),
    /// No reply: the call runs into its deadline.
    Never,
    /// One final `Err` reply.
    Err(HelperError),
}

/// What the fake helper does and saw. Shared by every link the factory makes.
#[derive(Default)]
struct FakeState {
    /// Exit status of the `Hello` exchange: `Some(126)` is a dismissed prompt.
    start_exit: Option<i32>,
    answers: VecDeque<Answer>,
    pending: VecDeque<HelperReply>,
    /// Set by the test for a helper that exits by itself.
    exit: Option<i32>,
    killed: bool,
    closed: bool,
    /// `send <seq>` and `reply <seq> last` in order.
    log: Vec<String>,
    /// Every request after the `Hello`, and whether a descriptor came with it.
    requests: Vec<(HelperRequest, bool)>,
}

type Shared = Arc<Mutex<FakeState>>;

fn lock(state: &Shared) -> std::sync::MutexGuard<'_, FakeState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

struct FakeLink(Shared);

impl HelperLink for FakeLink {
    fn send(&mut self, frame: &HelperFrame, fd: Option<BorrowedFd<'_>>) -> io::Result<()> {
        let mut s = lock(&self.0);
        if s.exit.is_some() || s.killed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        s.log.push(format!("send {}", frame.seq));
        if let HelperRequest::Hello { .. } = frame.request {
            match s.start_exit {
                Some(code) => s.exit = Some(code),
                None => s.pending.push_back(HelperReply {
                    seq: 0,
                    last: true,
                    body: HelperReplyBody::Hello {
                        proto: HELPER_PROTO_VERSION,
                    },
                }),
            }
            return Ok(());
        }
        s.requests.push((frame.request.clone(), fd.is_some()));
        match s.answers.pop_front() {
            Some(Answer::Replies(n)) => {
                for i in 1..=n {
                    s.pending.push_back(HelperReply {
                        seq: frame.seq,
                        last: i == n,
                        body: HelperReplyBody::Ok,
                    });
                }
            }
            Some(Answer::Err(e)) => s.pending.push_back(HelperReply {
                seq: frame.seq,
                last: true,
                body: HelperReplyBody::Err(e),
            }),
            Some(Answer::Never) | None => {}
        }
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<HelperReply, LinkError> {
        let mut s = lock(&self.0);
        match s.pending.pop_front() {
            Some(reply) => {
                if reply.last {
                    s.log.push(format!("reply {} last", reply.seq));
                }
                Ok(reply)
            }
            None if s.exit.is_some() => Err(LinkError::Eof),
            // In the lab the job runs inline, so "never answers" is an immediate timeout.
            None => Err(LinkError::Timeout),
        }
    }

    fn exit_status(&mut self, _wait: Duration) -> Option<i32> {
        let s = lock(&self.0);
        if s.killed { Some(137) } else { s.exit }
    }

    fn kill(&mut self) {
        lock(&self.0).killed = true;
    }

    fn close(&mut self) {
        let mut s = lock(&self.0);
        s.closed = true;
        s.exit.get_or_insert(0);
    }
}

struct Fake {
    state: Shared,
    spawns: Arc<AtomicUsize>,
}

impl Fake {
    fn new(installed: bool) -> (Fake, LinkEnv) {
        let fake = Fake {
            state: Shared::default(),
            spawns: Arc::new(AtomicUsize::new(0)),
        };
        let env = fake.env(installed);
        (fake, env)
    }

    /// A new `LinkEnv` on this fake's shared state, one per engine session.
    fn env(&self, installed: bool) -> LinkEnv {
        let (link_state, count) = (Arc::clone(&self.state), Arc::clone(&self.spawns));
        LinkEnv {
            installed: Arc::new(move || installed),
            spawn: Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
                let link: Box<dyn HelperLink> = Box::new(FakeLink(Arc::clone(&link_state)));
                Ok::<_, StartError>(link)
            }),
            setup_probe: Arc::new(|| PROBED),
            setup_runner: Arc::new(|_, _| SetupPhase::Succeeded),
        }
    }
}

/// What the fake setup probe answers.
const PROBED: SetupStatus = SetupStatus {
    rapl: RaplStatus::Readable {
        tasklens_rule: true,
    },
    journal_group: true,
    helper_installed: true,
    lm_sensors: false,
    hwmon_fan: true,
    sensors_conf: false,
};

/// Every `HelperUpdate` with the virtual time it arrived, in ms.
type Seen = Arc<Mutex<Vec<(u64, HelperUpdate)>>>;

fn states(seen_list: &[(u64, HelperUpdate)]) -> Vec<HelperState> {
    seen_list.iter().map(|(_, u)| u.state).collect()
}

fn lab(seed: u64) -> LabRuntime {
    LabRuntime::new(LabConfig::new(seed).max_steps(1_000_000))
}

/// Runs `helper_task_with` and `script` in one root region with auto-advanced time. `script`
/// gets the queue handle and the helper task's handle; when it returns, the queue closes and
/// the helper task ends. Returns every published update and checks the drain.
fn run<F, Fut>(mut lab: LabRuntime, env: LinkEnv, script: F) -> Vec<(u64, HelperUpdate)>
where
    F: FnOnce(Cx, HelperHandle, TaskHandle<()>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let log = Seen::default();
    let sink = Arc::clone(&log);
    let root = lab.state.create_root_region(Budget::INFINITE);
    let (task, _handle) = lab
        .state
        .create_task(root, Budget::INFINITE, async move {
            let cx = Cx::current().expect("task has a Cx");
            let (handle, queue) = helper_queue();
            let (updates, mut rx) = mpsc::channel::<EngineMsg>(64);
            let mut collector = cx
                .spawn(move |cx| async move {
                    while let Ok(msg) = rx.recv(&cx).await {
                        if let EngineMsg::Source(SourceUpdate::Helper(update)) = msg {
                            let ms = cx.now().as_nanos() / 1_000_000;
                            lock_seen(&sink).push((ms, update));
                        }
                    }
                })
                .expect("collector spawns");
            let helper = cx
                .spawn(move |cx| helper_task_with(cx, queue, updates, env))
                .expect("helper task spawns");
            script(cx.clone(), handle, helper).await;
            // The queue is closed (or the task aborted), so the helper drops `updates`.
            let _ = collector.join(&cx).await;
        })
        .expect("task is created");
    lab.scheduler.lock().schedule(task, 0);
    lab.run_with_auto_advance();
    let report = lab.run_until_quiescent_with_report();
    assert!(report.lab_test_passed(), "{report:?}");
    lock_seen(&log).clone()
}

fn lock_seen(seen: &Seen) -> std::sync::MutexGuard<'_, Vec<(u64, HelperUpdate)>> {
    seen.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Waits in 10 ms steps until the handle shows `want`.
async fn wait_for(cx: &Cx, handle: &HelperHandle, want: HelperState) {
    for _ in 0..1000 {
        if handle.state() == want {
            return;
        }
        time::sleep(cx.now(), Duration::from_millis(10)).await;
    }
    panic!("state stayed {:?}, wanted {want:?}", handle.state());
}

fn signal_req() -> HelperRequest {
    HelperRequest::Signal {
        target: HelperTarget::Process(ProcessKey {
            start_ticks: 4242,
            boot_id: BootId(7),
            pid: 1234,
        }),
        signal: Signal::Term,
    }
}

/// `HelperUpdate` as a comparable tuple.
type Pub = (HelperState, Option<(u64, HelperStartOutcome)>);

fn update(state: HelperState, start: Option<(u64, HelperStartOutcome)>) -> Pub {
    (state, start)
}

fn updates_only(seen: &[(u64, HelperUpdate)]) -> Vec<Pub> {
    seen.iter()
        .map(|(_, u)| (u.state, u.start.clone()))
        .collect()
}

#[test]
fn start_publishes_starting_then_unlocked() {
    let (fake, env) = Fake::new(true);
    let seen = run(lab(1), env, |cx, handle, _helper| async move {
        handle.send(&cx, HelperMsg::Start).await.expect("queued");
        wait_for(&cx, &handle, HelperState::Unlocked).await;
    });
    assert_eq!(
        states(&seen)[..3],
        [
            HelperState::Locked,
            HelperState::Starting,
            HelperState::Unlocked
        ]
    );
    assert!(seen.iter().all(|(_, u)| u.start.is_none()));
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 1);
}

#[test]
fn start_cancelled_126_back_to_locked() {
    let (fake, env) = Fake::new(true);
    lock(&fake.state).start_exit = Some(126);
    let seen = run(lab(2), env, |cx, handle, _helper| async move {
        handle.send(&cx, HelperMsg::Start).await.expect("queued");
        // The task is serial: the call below waits until the start ran.
        handle.send(&cx, HelperMsg::End).await.expect("queued");
        time::sleep(cx.now(), Duration::from_millis(10)).await;
    });
    assert_eq!(
        updates_only(&seen),
        [
            update(HelperState::Locked, None),
            update(HelperState::Starting, None),
            update(
                HelperState::Locked,
                Some((1, HelperStartOutcome::Cancelled))
            ),
        ]
    );
}

#[test]
fn missing_helper_never_spawns() {
    let (fake, env) = Fake::new(false);
    let seen = run(lab(3), env, |cx, handle, _helper| async move {
        let got = handle
            .call(&cx, signal_req(), None, Duration::from_secs(2))
            .await;
        assert_eq!(got, Err(HelperCallError::Missing));
        assert_eq!(handle.state(), HelperState::Missing);
    });
    assert_eq!(states(&seen), [HelperState::Missing]);
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 0);
}

#[test]
fn call_while_locked_starts_once() {
    let (fake, env) = Fake::new(true);
    lock(&fake.state).answers = [Answer::Replies(1), Answer::Replies(2)].into();
    let seen = run(lab(4), env, |cx, handle, _helper| async move {
        let first = handle
            .call(&cx, signal_req(), None, Duration::from_secs(2))
            .await
            .expect("first call");
        assert_eq!(first.len(), 1);
        let second = handle
            .call(&cx, signal_req(), None, Duration::from_secs(2))
            .await
            .expect("second call");
        assert_eq!(second.len(), 2);
        assert!(second[1].last);
    });
    assert_eq!(
        states(&seen),
        [
            HelperState::Locked,
            HelperState::Starting,
            HelperState::Unlocked,
            // The closed queue ends the session at the end of the run; nothing is published.
        ]
    );
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 1);
    assert!(lock(&fake.state).closed, "the session ended with the task");
}

#[test]
fn end_closes_and_locks_without_toast() {
    let (fake, env) = Fake::new(true);
    let state = Arc::clone(&fake.state);
    let seen = run(lab(5), env, |cx, handle, _helper| async move {
        handle.send(&cx, HelperMsg::Start).await.expect("queued");
        wait_for(&cx, &handle, HelperState::Unlocked).await;
        handle.send(&cx, HelperMsg::End).await.expect("queued");
        wait_for(&cx, &handle, HelperState::Locked).await;
        assert!(lock(&state).closed, "the fake saw close");
    });
    assert_eq!(
        updates_only(&seen).last().cloned(),
        Some(update(HelperState::Locked, None))
    );
    assert_eq!(seen.len(), 4, "{seen:?}");
}

#[test]
fn idle_exit_detected_within_1s() {
    let (fake, env) = Fake::new(true);
    let state = Arc::clone(&fake.state);
    let exited_at = Arc::new(Mutex::new(0u64));
    let mark = Arc::clone(&exited_at);
    let seen = run(lab(6), env, |cx, handle, _helper| async move {
        handle.send(&cx, HelperMsg::Start).await.expect("queued");
        wait_for(&cx, &handle, HelperState::Unlocked).await;
        // The helper's own 300 s idle exit.
        lock(&state).exit = Some(0);
        *mark.lock().unwrap_or_else(PoisonError::into_inner) = cx.now().as_nanos() / 1_000_000;
        wait_for(&cx, &handle, HelperState::Locked).await;
    });
    let exited = *exited_at.lock().unwrap_or_else(PoisonError::into_inner);
    let (t, last) = seen.last().cloned().expect("an update");
    assert_eq!((last.state, last.start), update(HelperState::Locked, None));
    assert!(t - exited <= 1000, "locked {t} ms, exit {exited} ms");
    assert!(
        !lock(&fake.state).closed,
        "an exited helper is not closed again"
    );
}

#[test]
fn call_timeout_drops_session() {
    let (fake, env) = Fake::new(true);
    lock(&fake.state).answers = [Answer::Never].into();
    let seen = run(lab(7), env, |cx, handle, _helper| async move {
        let got = handle
            .call(&cx, signal_req(), None, Duration::from_secs(2))
            .await;
        assert_eq!(got, Err(HelperCallError::Timeout));
        assert_eq!(handle.state(), HelperState::Locked);
    });
    assert_eq!(
        updates_only(&seen).last().cloned(),
        Some(update(HelperState::Locked, None))
    );
    assert!(lock(&fake.state).killed, "the timed-out helper was killed");
}

#[test]
fn calls_are_serial() {
    let (fake, env) = Fake::new(true);
    lock(&fake.state).answers = [Answer::Replies(3), Answer::Replies(2)].into();
    run(lab(8), env, |cx, handle, _helper| async move {
        let other = handle.clone();
        let mut first = cx
            .spawn(move |cx| async move {
                other
                    .call(&cx, signal_req(), None, Duration::from_secs(2))
                    .await
            })
            .expect("first caller spawns");
        let second = handle
            .call(&cx, signal_req(), None, Duration::from_secs(2))
            .await;
        let first = first.join(&cx).await.expect("first caller joins");
        // Either caller may go first; the first call processed gets 3 replies.
        let mut lens = [
            first.expect("first call").len(),
            second.expect("second call").len(),
        ];
        lens.sort_unstable();
        assert_eq!(lens, [2, 3]);
    });
    let log = lock(&fake.state).log.clone();
    assert_eq!(
        log,
        [
            "send 0",
            "reply 0 last",
            "send 1",
            "reply 1 last",
            "send 2",
            "reply 2 last"
        ]
    );
}

// Background reads (qfx.36) ---------------------------------------------------------------------

/// Security regression (qfx.36): the helper exits after the reader saw `Unlocked` and before
/// its read reaches the task. The read gets `NotRunning`; no spawn and no prompt follow.
#[test]
fn read_after_exit_never_starts() {
    let (fake, env) = Fake::new(true);
    let state = Arc::clone(&fake.state);
    let spawns = Arc::clone(&fake.spawns);
    let seen = run(lab(20), env, |cx, handle, _helper| async move {
        handle.send(&cx, HelperMsg::Start).await.expect("queued");
        wait_for(&cx, &handle, HelperState::Unlocked).await;
        // The sweep's state check passes.
        assert_eq!(handle.state(), HelperState::Unlocked);
        // The helper's own idle exit; the task notices it within 1 s.
        lock(&state).exit = Some(0);
        time::sleep(cx.now(), Duration::from_millis(1_500)).await;
        // The read sent after the passed check.
        let got = handle.read(&cx, signal_req(), Duration::from_secs(2)).await;
        assert_eq!(got, Err(HelperReadError::NotRunning));
        assert_eq!(handle.state(), HelperState::Locked);
        assert_eq!(spawns.load(Ordering::SeqCst), 1, "only the user's start");
    });
    assert_eq!(
        states(&seen),
        [
            HelperState::Locked,
            HelperState::Starting,
            HelperState::Unlocked,
            HelperState::Locked,
        ]
    );
    assert!(seen.iter().all(|(_, u)| u.start.is_none()));
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 1);
    assert!(
        lock(&fake.state).requests.is_empty(),
        "nothing reached the helper"
    );
}

/// Security regression (qfx.36): an `End` queued after the reader's state check and before its
/// read. The task takes the `End` first, so the read finds no session and is refused.
#[test]
fn read_after_end_never_starts() {
    let (fake, env) = Fake::new(true);
    let spawns = Arc::clone(&fake.spawns);
    let seen = run(lab(21), env, |cx, handle, _helper| async move {
        handle.send(&cx, HelperMsg::Start).await.expect("queued");
        wait_for(&cx, &handle, HelperState::Unlocked).await;
        assert_eq!(handle.state(), HelperState::Unlocked);
        handle.send(&cx, HelperMsg::End).await.expect("queued");
        let got = handle.read(&cx, signal_req(), Duration::from_secs(2)).await;
        assert_eq!(got, Err(HelperReadError::NotRunning));
        assert_eq!(spawns.load(Ordering::SeqCst), 1, "only the user's start");
    });
    assert_eq!(
        states(&seen),
        [
            HelperState::Locked,
            HelperState::Starting,
            HelperState::Unlocked,
            HelperState::Locked,
        ]
    );
    assert!(
        lock(&fake.state).requests.is_empty(),
        "nothing reached the helper"
    );
}

/// A read while `Locked` or `Missing` is refused at once and never spawns.
#[test]
fn read_without_session_never_spawns() {
    for installed in [true, false] {
        let (fake, env) = Fake::new(installed);
        let seen = run(lab(22), env, |cx, handle, _helper| async move {
            let got = handle.read(&cx, signal_req(), Duration::from_secs(2)).await;
            assert_eq!(got, Err(HelperReadError::NotRunning));
        });
        let want = if installed {
            HelperState::Locked
        } else {
            HelperState::Missing
        };
        assert_eq!(states(&seen), [want]);
        assert_eq!(fake.spawns.load(Ordering::SeqCst), 0);
    }
}

/// The helper exits before the task notices: the read fails with `Exited`, the shield locks,
/// and the next read is refused. Neither starts a session.
#[test]
fn read_after_unnoticed_exit_fails_without_start() {
    let (fake, env) = Fake::new(true);
    let state = Arc::clone(&fake.state);
    let seen = run(lab(23), env, |cx, handle, _helper| async move {
        handle.send(&cx, HelperMsg::Start).await.expect("queued");
        wait_for(&cx, &handle, HelperState::Unlocked).await;
        lock(&state).exit = Some(0);
        let got = handle.read(&cx, signal_req(), Duration::from_secs(2)).await;
        assert_eq!(got, Err(HelperReadError::Call(HelperCallError::Exited)));
        assert_eq!(handle.state(), HelperState::Locked);
        let again = handle.read(&cx, signal_req(), Duration::from_secs(2)).await;
        assert_eq!(again, Err(HelperReadError::NotRunning));
    });
    assert_eq!(
        states(&seen),
        [
            HelperState::Locked,
            HelperState::Starting,
            HelperState::Unlocked,
            HelperState::Locked,
        ]
    );
    assert_eq!(
        fake.spawns.load(Ordering::SeqCst),
        1,
        "only the user's start"
    );
}

#[test]
fn read_on_unlocked_session_returns_replies() {
    let (fake, env) = Fake::new(true);
    lock(&fake.state).answers = [Answer::Replies(2)].into();
    run(lab(24), env, |cx, handle, _helper| async move {
        handle.send(&cx, HelperMsg::Start).await.expect("queued");
        wait_for(&cx, &handle, HelperState::Unlocked).await;
        let got = handle
            .read(&cx, signal_req(), Duration::from_secs(2))
            .await
            .expect("read");
        assert_eq!(got.len(), 2);
        assert!(got[1].last);
        assert_eq!(handle.state(), HelperState::Unlocked);
    });
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 1);
    assert_eq!(lock(&fake.state).requests, [(signal_req(), false)]);
}

/// Mirrors `#[explore_seeds(seeds = 0..16, workers = 2, chaos)]`, which asupersync 0.5.0 does
/// not re-export (see `lab_supervise.rs::explore_restart`). No cancel injection: a chaos
/// cancel before `Unlocked` would end the run before the shutdown under test.
#[test]
fn shutdown_ends_session() {
    for seed in 0..16 {
        let chaos = ChaosConfig::default()
            .with_delay_probability(0.05)
            .with_delay_range(Duration::ZERO..Duration::from_millis(10))
            .with_wakeup_storm_probability(0.01);
        let lab = LabRuntime::new(
            LabConfig::new(seed)
                .worker_count(2)
                .max_steps(1_000_000)
                .with_chaos(chaos),
        );
        let (fake, env) = Fake::new(true);
        let seen = run(lab, env, |cx, handle, mut helper| async move {
            handle.send(&cx, HelperMsg::Start).await.expect("queued");
            wait_for(&cx, &handle, HelperState::Unlocked).await;
            // Engine shutdown: the queue stays open, the task is cancelled.
            helper.abort();
            let _ = helper.join(&cx).await;
            drop(handle);
        });
        assert_eq!(
            states(&seen).last(),
            Some(&HelperState::Unlocked),
            "seed {seed}"
        );
        let s = lock(&fake.state);
        assert!(s.closed, "seed {seed}: end() ran");
        assert!(!s.killed, "seed {seed}: the helper exited on EOF");
    }
}

/// `RunSetup` publishes `Running` with empty output, then the outcome with the output, then
/// the new status (R-149 item 4, R-247). The fake runner prints two lines.
#[test]
fn run_setup_publishes_running_then_outcome() {
    let (_fake, mut env) = Fake::new(true);
    env.setup_runner = Arc::new(|step, out| {
        assert_eq!(step, SetupStep::RaplEnable);
        out("one");
        out("two");
        SetupPhase::Succeeded
    });
    let seen = Arc::new(Mutex::new(Vec::<SetupState>::new()));
    let sink = Arc::clone(&seen);
    let mut lab = lab(7);
    let root = lab.state.create_root_region(Budget::INFINITE);
    let (task, _handle) = lab
        .state
        .create_task(root, Budget::INFINITE, async move {
            let cx = Cx::current().expect("task has a Cx");
            let (handle, queue) = helper_queue();
            let (updates, mut rx) = mpsc::channel::<EngineMsg>(64);
            let mut collector = cx
                .spawn(move |cx| async move {
                    while let Ok(msg) = rx.recv(&cx).await {
                        if let EngineMsg::Source(SourceUpdate::Setup(state)) = msg {
                            sink.lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push((*state).clone());
                        }
                    }
                })
                .expect("collector spawns");
            let _helper = cx
                .spawn(move |cx| helper_task_with(cx, queue, updates, env))
                .expect("helper task spawns");
            let step = SetupStep::RaplEnable;
            handle
                .send(&cx, HelperMsg::RunSetup(step))
                .await
                .expect("queued");
            // Closing the queue ends the helper task once the run is done.
            drop(handle);
            let _ = collector.join(&cx).await;
        })
        .expect("task is created");
    lab.scheduler.lock().schedule(task, 0);
    lab.run_with_auto_advance();
    let report = lab.run_until_quiescent_with_report();
    assert!(report.lab_test_passed(), "{report:?}");

    let run = |phase, output: &str| {
        Some(SetupRun {
            seq: 1,
            step: SetupStep::RaplEnable,
            phase,
            output: output.into(),
        })
    };
    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(
        seen,
        [
            SetupState {
                status: None,
                run: run(SetupPhase::Running, ""),
            },
            SetupState {
                status: None,
                run: run(SetupPhase::Succeeded, "one\ntwo\n"),
            },
            SetupState {
                status: Some(PROBED),
                run: run(SetupPhase::Succeeded, "one\ntwo\n"),
            },
        ]
    );
}

// Engine routing (E29-020) ----------------------------------------------------------------------

struct NoRecord;

impl RecordSink for NoRecord {
    fn on_system_sample(&mut self, _t_ns: TimeNs, _series: &[(SeriesId, Sample)]) {}
    fn on_process_sweep(&mut self, _t_ns: TimeNs, _records: &Arc<[ProcessRecord]>) {}
    fn on_device_catalog(&mut self, _catalog: &DeviceCatalog) {}
}

/// `engine_root` with no workers, a test record sink and `fake` as the helper.
struct EngineRig {
    fake: FakeLab,
    root: TaskHandle<()>,
    ui: UiSide,
}

impl EngineRig {
    fn new(fake: &Fake) -> EngineRig {
        let config = Arc::new(EngineConfig::new(
            EngineMode::Full,
            "/var/tmp/claude/qfx.23-state".into(),
        ));
        let (mut ep, ui) = fake_sources::root_endpoints(config, Vec::new());
        let (state, spawns) = (Arc::clone(&fake.state), Arc::clone(&fake.spawns));
        ep.hooks = EngineHooks {
            // A test sink keeps the history worker out.
            record_sink: Some(Arc::new(|_: &EngineConfig| {
                Box::new(NoRecord) as Box<dyn RecordSink>
            })),
            helper: Some(Arc::new(move || {
                Fake {
                    state: Arc::clone(&state),
                    spawns: Arc::clone(&spawns),
                }
                .env(true)
            })),
            ..EngineHooks::default()
        };
        let mut lab = FakeLab::new();
        let root = lab.spawn(async move {
            let cx = Cx::current().expect("task has a Cx");
            engine_root(cx, ep).await;
        });
        EngineRig {
            fake: lab,
            root,
            ui,
        }
    }

    fn send(&mut self, msg: EngineMsg) {
        self.ui
            .msgs_tx
            .try_send(msg)
            .expect("the queue has room for the message");
        self.fake.lab.run_until_idle();
    }

    fn command(&mut self, cmd: Command) {
        self.send(EngineMsg::Command(cmd));
    }

    fn latest(&self) -> Arc<Snapshot> {
        self.ui.latest.load_full()
    }

    /// Cancels the root, runs to quiescence and checks the lab's oracles.
    fn finish(mut self) {
        self.root.abort();
        self.fake.advance(Duration::from_millis(10));
        assert!(self.root.is_finished(), "engine_root returned after cancel");
        self.fake.finish();
    }
}

fn cpu_msg(t_ns: TimeNs) -> EngineMsg {
    let mut online = CpuSet::empty();
    online.insert(0);
    EngineMsg::Source(SourceUpdate::Cpu(CpuSample {
        t_ns,
        totals: CpuTotals {
            util_pct: Metric::valid(40.0),
            ..CpuTotals::default()
        },
        irq_pct: 0.0,
        per_cpu: [CpuCoreSample {
            cpu: 0,
            pct: 40.0,
            kernel_pct: 0.0,
            freq_mhz: Some(3000),
            validity: Validity::Valid,
        }]
        .into_iter()
        .collect(),
        freq_mhz: Some(3000),
        temp_c: None,
        temp_validity: Validity::Unsupported,
        topology: Some(Arc::new(CpuTopology {
            online,
            cores: Arc::from([]),
        })),
        validity: Validity::Valid,
    }))
}

#[test]
fn command_start_reaches_helper() {
    let (fake, _env) = Fake::new(true);
    let mut rig = EngineRig::new(&fake);
    assert_eq!(rig.latest().health.helper, HelperState::Locked);
    rig.command(Command::StartHelperSession);
    let health = rig.latest().health.clone();
    assert_eq!(health.helper, HelperState::Unlocked);
    assert_eq!(health.helper_start, None, "a good start has no toast");
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 1);
    rig.command(Command::EndHelperSession);
    assert_eq!(rig.latest().health.helper, HelperState::Locked);
    assert!(lock(&fake.state).closed, "End closed the helper");
    rig.finish();
}

#[test]
fn start_failure_seq_counts() {
    let (fake, _env) = Fake::new(true);
    lock(&fake.state).start_exit = Some(126);
    let mut rig = EngineRig::new(&fake);
    rig.command(Command::StartHelperSession);
    let health = rig.latest().health.clone();
    assert_eq!(health.helper, HelperState::Locked);
    assert_eq!(
        health.helper_start,
        Some((1, HelperStartOutcome::Cancelled))
    );
    rig.command(Command::StartHelperSession);
    let health = rig.latest().health.clone();
    assert_eq!(
        health.helper_start,
        Some((2, HelperStartOutcome::Cancelled))
    );
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 2);
    rig.finish();
}

#[test]
fn setup_status_published() {
    let (fake, _env) = Fake::new(true);
    let mut rig = EngineRig::new(&fake);
    assert_eq!(rig.latest().health.setup.status, None);
    rig.command(Command::SetupStatus);
    assert_eq!(rig.latest().health.setup.status, Some(PROBED));
    rig.finish();
}

#[test]
fn health_arc_reused_when_unchanged() {
    let (fake, _env) = Fake::new(true);
    let mut rig = EngineRig::new(&fake);
    rig.command(Command::SetupStatus);
    rig.send(cpu_msg(100 * 1_000_000));
    let before = rig.latest();
    rig.send(cpu_msg(200 * 1_000_000));
    let after = rig.latest();
    assert!(after.sequence > before.sequence, "the CPU sample published");
    assert!(
        Arc::ptr_eq(&before.health, &after.health),
        "a CPU-only publish keeps the health Arc"
    );
    // The same helper state and setup state again change nothing either.
    let same = HelperUpdate {
        state: HelperState::Locked,
        start: None,
    };
    rig.send(EngineMsg::Source(SourceUpdate::Helper(same)));
    rig.send(EngineMsg::Source(SourceUpdate::Setup(Arc::new(
        (*after.health.setup).clone(),
    ))));
    rig.send(cpu_msg(300 * 1_000_000));
    assert!(Arc::ptr_eq(&after.health, &rig.latest().health));
    rig.finish();
}

/// Every `ActionEvent` of one request as `(target, state)`, in order.
type Events = Vec<(Option<u16>, ActionState)>;

/// `ActionOps` that refuses everything. `dump` makes `dump_available` true.
struct TestOps {
    dump: bool,
}

impl ActionOps for TestOps {
    fn dump_available(&self) -> bool {
        self.dump
    }
}

fn pkey(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 100 + u64::from(pid),
        boot_id: BootId(7),
        pid,
    }
}

fn procs(pids: &[u32]) -> Vec<ActionTarget> {
    pids.iter()
        .map(|p| ActionTarget::Process(pkey(*p)))
        .collect()
}

fn action(kind: ActionKind, targets: Vec<ActionTarget>, privileged: bool) -> ActionRequest {
    ActionRequest {
        id: ActionId(1),
        kind,
        targets: targets.into_iter().collect(),
        privileged,
    }
}

/// Runs `req` through `run_action` with every target admitted and the real `HelperHandle` over
/// `env` as `ActionCtx.helper`. The action starts once the helper task has published its first
/// state, as in an engine session.
fn run_request(
    lab: LabRuntime,
    env: LinkEnv,
    state_dir: PathBuf,
    dump: bool,
    req: ActionRequest,
) -> Events {
    let out: Arc<Mutex<Events>> = Arc::default();
    let sink = Arc::clone(&out);
    run(lab, env, move |cx, handle, _helper| async move {
        time::sleep(cx.now(), Duration::from_millis(10)).await;
        let (events_tx, events) = async_channel::bounded(256);
        let (msgs_tx, _msgs) = mpsc::channel::<EngineMsg>(16);
        let actx = Arc::new(ActionCtx {
            events: events_tx,
            msgs: msgs_tx,
            ops: Arc::new(TestOps { dump }),
            boot_id: BootId(7),
            uid: 1000,
            config: Arc::new(EngineConfig::new(EngineMode::Full, state_dir)),
            systemd_system: ManagerConnSlot::new(),
            systemd_user: ManagerConnSlot::new(),
            openers: None,
            helper: Some(Arc::new(handle) as Arc<dyn PrivilegedOps>),
            destroy_support_sent: AtomicBool::new(false),
            unit_bus_override: None,
            startup_paths: StartupPaths::from_env(),
        });
        let per_target = vec![None; req.targets.len()];
        run_action(cx.clone(), actx, req, per_target).await;
        let got = std::iter::from_fn(|| events.try_recv().ok()).map(|ev| (ev.target, ev.state));
        out_lock(&sink).extend(got);
    });
    out_lock(&out).clone()
}

fn out_lock(events: &Mutex<Events>) -> std::sync::MutexGuard<'_, Events> {
    events.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `run_request` with a state directory nothing writes to.
fn run_plain(lab: LabRuntime, env: LinkEnv, req: ActionRequest) -> Events {
    let state_dir = PathBuf::from("/var/tmp/claude/qfx.17-state");
    run_request(lab, env, state_dir, false, req)
}

fn requests(fake: &Fake) -> Vec<HelperRequest> {
    lock(&fake.state)
        .requests
        .iter()
        .map(|(req, _)| req.clone())
        .collect()
}

fn signal_to(pid: u32, signal: Signal) -> HelperRequest {
    HelperRequest::Signal {
        target: HelperTarget::Process(pkey(pid)),
        signal,
    }
}

const STARTED: (Option<u16>, ActionState) = (None, ActionState::Started);

#[test]
fn privileged_signal_two_targets() {
    let (fake, env) = Fake::new(true);
    lock(&fake.state).answers = [Answer::Replies(1), Answer::Replies(1)].into();
    let req = action(ActionKind::Signal(Signal::Term), procs(&[11, 12]), true);
    let events = run_plain(lab(20), env, req);
    assert_eq!(
        events,
        [
            STARTED,
            (Some(0), ActionState::Succeeded),
            (Some(1), ActionState::Succeeded),
            (None, ActionState::Succeeded),
        ]
    );
    assert_eq!(
        requests(&fake),
        [signal_to(11, Signal::Term), signal_to(12, Signal::Term)]
    );
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 1, "one prompt");
}

/// The summary follows `actions::summarize` like the unprivileged pipeline: with more than one
/// target a failure is `Partial { done, total }` (E03-053).
#[test]
fn privileged_partial_failure_summary() {
    let (fake, env) = Fake::new(true);
    lock(&fake.state).answers = [
        Answer::Replies(1),
        Answer::Err(HelperError::PermissionDenied),
    ]
    .into();
    let req = action(ActionKind::Signal(Signal::Kill), procs(&[11, 12]), true);
    let events = run_plain(lab(21), env, req);
    assert_eq!(
        events,
        [
            STARTED,
            (Some(0), ActionState::Succeeded),
            (Some(1), ActionState::Failed(ActionError::PermissionDenied)),
            (
                None,
                ActionState::Failed(ActionError::Partial { done: 1, total: 2 })
            ),
        ]
    );
}

#[test]
fn privileged_kill_tree_stops_first() {
    let (fake, env) = Fake::new(true);
    lock(&fake.state).answers = vec![Answer::Replies(1); 6].into();
    let req = action(ActionKind::KillTree, procs(&[11, 12, 13]), true);
    let events = run_plain(lab(22), env, req);
    assert_eq!(events.len(), 5, "{events:?}");
    assert_eq!(events[4], (None, ActionState::Succeeded));
    let stops = [11, 12, 13].map(|p| signal_to(p, Signal::Stop));
    let kills = [11, 12, 13].map(|p| signal_to(p, Signal::Kill));
    assert_eq!(requests(&fake), [stops, kills].concat());
}

/// The prompt is dismissed once; no target is sent and no second prompt shows.
fn auth_failure_all_targets(exit: i32, want: ActionError) {
    let (fake, env) = Fake::new(true);
    lock(&fake.state).start_exit = Some(exit);
    let req = action(ActionKind::SetNice(5), procs(&[11, 12]), true);
    let events = run_plain(lab(23), env, req);
    let failed = ActionState::Failed(want);
    assert_eq!(events[0], STARTED);
    assert_eq!(events[1], (Some(0), failed.clone()));
    assert_eq!(events[2], (Some(1), failed));
    assert_eq!(events.len(), 4, "{events:?}");
    assert!(requests(&fake).is_empty(), "no SetNice frame");
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 1, "one prompt");
}

#[test]
fn privileged_auth_cancelled_all_targets() {
    auth_failure_all_targets(126, ActionError::AuthCancelled);
}

#[test]
fn privileged_not_authorized_all_targets() {
    auth_failure_all_targets(127, ActionError::NotAuthorized);
}

/// Mirrors `#[explore_seeds(seeds = 0..16, workers = 2, chaos)]` like `shutdown_ends_session`.
#[test]
fn privileged_timeout_outcome_unknown() {
    for seed in 0..16 {
        let chaos = ChaosConfig::default()
            .with_delay_probability(0.05)
            .with_delay_range(Duration::ZERO..Duration::from_millis(10))
            .with_wakeup_storm_probability(0.01);
        let lab = LabRuntime::new(
            LabConfig::new(seed)
                .worker_count(2)
                .max_steps(1_000_000)
                .with_chaos(chaos),
        );
        let (fake, env) = Fake::new(true);
        lock(&fake.state).answers = [Answer::Never].into();
        let req = action(ActionKind::Signal(Signal::Term), procs(&[11]), true);
        let events = run_plain(lab, env, req);
        let unknown =
            ActionState::OutcomeUnknown(Arc::from("no answer from the administrator helper"));
        assert_eq!(
            events,
            [STARTED, (Some(0), unknown.clone()), (None, unknown)],
            "seed {seed}"
        );
    }
}

#[test]
fn privileged_unit_kind_unsupported() {
    let unit = ActionTarget::Unit(UnitRef {
        manager: Manager::System,
        name: Arc::from("a.service"),
    });
    let thread = ActionTarget::Thread {
        process: pkey(11),
        tid: 12,
    };
    let cases = [
        (ActionKind::UnitStart, vec![unit]),
        // R-097: no I/O priority for one thread.
        (ActionKind::SetIoPriority(IoPriority::Idle), vec![thread]),
    ];
    for (kind, targets) in cases {
        let (fake, env) = Fake::new(true);
        let events = run_plain(lab(24), env, action(kind.clone(), targets, true));
        assert_eq!(
            events,
            [
                STARTED,
                (None, ActionState::Rejected(RejectReason::Unsupported))
            ],
            "{kind:?}"
        );
        assert_eq!(fake.spawns.load(Ordering::SeqCst), 0, "{kind:?}");
    }
}

/// Security regression: an unprivileged request never escalates by itself.
#[test]
fn unprivileged_never_touches_helper() {
    let (fake, _env) = Fake::new(true);
    let cgroup = ActionTarget::Cgroup(CgroupRef {
        rel_path: Arc::from("user.slice/app.slice/app-a.scope"),
        cgroup_id: 42,
    });
    let dump_path = Arc::from("/var/tmp/claude/qfx.17-state/dumps/a.core");
    let cases = [
        (ActionKind::Signal(Signal::Term), procs(&[11, 12])),
        (ActionKind::Suspend, procs(&[11])),
        (ActionKind::SetNice(5), procs(&[11])),
        (ActionKind::KillTree, procs(&[11, 12])),
        (ActionKind::FreezeGroup, vec![cgroup]),
        (
            ActionKind::Dump {
                mode: DumpMode::Mini,
                path: dump_path,
            },
            procs(&[11]),
        ),
    ];
    for (kind, targets) in cases {
        let events = run_plain(
            lab(25),
            fake.env(true),
            action(kind.clone(), targets, false),
        );
        assert!(
            events
                .last()
                .is_some_and(|(t, s)| t.is_none() && s.is_final()),
            "{kind:?}: {events:?}"
        );
    }
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 0);
    assert!(lock(&fake.state).log.is_empty());
}

#[test]
fn privileged_missing_helper_rejected() {
    let (fake, env) = Fake::new(false);
    let req = action(ActionKind::Signal(Signal::Term), procs(&[11]), true);
    let events = run_plain(lab(26), env, req);
    assert_eq!(
        events,
        [
            STARTED,
            (None, ActionState::Rejected(RejectReason::Unsupported))
        ]
    );
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 0, "no spawn");
}

/// A privileged `Dump` reaches `dump::run_target(.., true)`: the helper gets the request with a
/// descriptor, never a path (R-145).
#[test]
fn privileged_dump_through_helper() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
    std::fs::create_dir_all(&base).expect("create target/tmp");
    let tmp = tempfile::Builder::new()
        .prefix("tasklens-qfx17-")
        .tempdir_in(base)
        .expect("tempdir");
    let state_dir = tmp.path().join("state");
    let config = EngineConfig::new(EngineMode::Full, state_dir.clone());
    let file = dump::dumps_dir(&config).join("a.core");
    let (fake, env) = Fake::new(true);
    lock(&fake.state).answers = [Answer::Replies(1)].into();
    let kind = ActionKind::Dump {
        mode: DumpMode::Mini,
        path: Arc::from(file.to_str().expect("utf-8 path")),
    };
    let events = run_request(
        lab(27),
        env,
        state_dir,
        true,
        action(kind, procs(&[11]), true),
    );
    assert_eq!(
        events,
        [
            STARTED,
            (Some(0), ActionState::Succeeded),
            (None, ActionState::Succeeded)
        ]
    );
    let want = HelperRequest::Dump {
        key: pkey(11),
        mode: DumpMode::Mini,
    };
    assert_eq!(lock(&fake.state).requests, [(want, true)]);
    assert!(file.is_file(), "the engine created the file");
}

// Helper reads in the processes sweep (E29-017) -------------------------------------------------

/// The `Processes` worker on a copy of the `owner-desktop` fixture, with the helper task on a
/// fake link that answers `ReadProcessFiles`. No test runs the real helper or pkexec.
mod sweep_helper {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::os::fd::BorrowedFd;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use asupersync::Cx;
    use asupersync::channel::{mpsc, watch};
    use asupersync::runtime::TaskHandle;
    use asupersync::time::{TimeSource, VirtualClock};
    use tl_collect::SysRoot;
    use tl_collect::apps::AppPaths;
    use tl_engine::config::{EngineConfig, EngineMode};
    use tl_engine::dbus::SharedHandles;
    use tl_engine::helper::{
        HelperHandle, HelperLink, HelperMsg, LinkEnv, LinkError, StartError, helper_queue,
        helper_task_with,
    };
    use tl_engine::source::{WorkerControl, WorkerIo};
    use tl_engine::update::{EngineMsg, ProcessSweep, SourceUpdate};
    use tl_engine::workers::processes::{self, HELPER_READS_MAX, ProcessesSource};
    use tl_model::demand::Demand;
    use tl_model::health::HelperState;
    use tl_model::helper_proto::{
        HELPER_PROTO_VERSION, HelperError, HelperFrame, HelperReply, HelperReplyBody,
        HelperRequest, ProcFiles,
    };
    use tl_model::ids::{ColumnId, ProcessKey};
    use tl_model::process::ProcessRecord;
    use tl_model::settings::EngineSettings;
    use tl_model::time::TimeNs;
    use tl_model::validity::{ProcessField, Validity};

    use super::PROBED;
    use super::fake_sources::{FakeLab, drain};

    /// A real `io` file; the fixture's own pids have none.
    const IO_TEXT: &str = "rchar: 1\nwchar: 1\nsyscr: 1\nsyscw: 1\nread_bytes: 0\nwrite_bytes: 0\n\
                           cancelled_write_bytes: 0\n";

    thread_local! {
        /// The clock of the current test's lab. The lab runs every task on the test thread.
        static CLOCK: RefCell<Option<Arc<VirtualClock>>> = const { RefCell::new(None) };
    }

    fn now_ms() -> u64 {
        lab_clock().0 / 1_000_000
    }

    /// The sampler clock: lab virtual time for both CLOCK_BOOTTIME and CLOCK_MONOTONIC.
    fn lab_clock() -> (TimeNs, u64) {
        let t = CLOCK.with(|c| {
            c.borrow()
                .as_ref()
                .expect("the rig set the clock")
                .now()
                .as_nanos()
        });
        (t, t)
    }

    /// Copies `src` over `dst`, keeping symlinks as links. Later copies replace earlier files.
    fn copy_tree(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).expect("create fixture dir");
        for entry in fs::read_dir(src).expect("list fixture dir") {
            let entry = entry.expect("fixture dir entry");
            let to = dst.join(entry.file_name());
            let file_type = entry.file_type().expect("fixture file type");
            if file_type.is_symlink() {
                let target = fs::read_link(entry.path()).expect("read fixture symlink");
                let _ = fs::remove_file(&to);
                std::os::unix::fs::symlink(target, &to).expect("create fixture symlink");
            } else if file_type.is_dir() {
                copy_tree(&entry.path(), &to);
            } else {
                fs::copy(entry.path(), &to).expect("copy fixture file");
            }
        }
    }

    fn fixtures() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tl-collect/tests/fixtures/"
        ))
    }

    /// A writable copy of `owner-desktop` with `process-edge` over it under the target tmpdir,
    /// removed on drop.
    struct Root {
        dir: PathBuf,
    }

    impl Root {
        fn new() -> Root {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("lab-helper-sweep-{}-{n}", std::process::id()));
            if dir.exists() {
                remove(&dir);
            }
            // `process-edge` adds the cgroup tree the sampler opens.
            for name in ["owner-desktop", "process-edge"] {
                copy_tree(&fixtures().join(name), &dir.join("root"));
            }
            Root { dir }
        }

        fn pid_dir(&self, pid: u32) -> PathBuf {
            self.dir.join(format!("root/proc/{pid}"))
        }

        /// Gives `pid` an `io` file the user cannot read: Restricted, as for another user.
        fn restrict_io(&self, pid: u32) {
            let io = self.pid_dir(pid).join("io");
            fs::write(&io, IO_TEXT).expect("write io");
            fs::set_permissions(&io, fs::Permissions::from_mode(0o000)).expect("chmod io");
        }

        /// A copy of pid 42 as `pid`, with a Restricted `io`.
        fn add_restricted(&self, pid: u32) {
            let to = self.pid_dir(pid);
            fs::create_dir_all(&to).expect("create pid dir");
            for name in ["cgroup", "cmdline", "comm", "statm", "status"] {
                fs::copy(self.pid_dir(42).join(name), to.join(name)).expect("copy pid file");
            }
            let stat = fs::read_to_string(self.pid_dir(42).join("stat")).expect("read stat");
            let rest = stat.strip_prefix("42 ").expect("stat starts with the pid");
            fs::write(to.join("stat"), format!("{pid} {rest}")).expect("write stat");
            self.restrict_io(pid);
        }

        fn config(&self) -> Arc<EngineConfig> {
            let paths = AppPaths {
                home: self.dir.join("home"),
                xdg_data_home: None,
                xdg_data_dirs: Some(self.dir.join("share").into()),
                xdg_config_home: None,
                icon_cache: self.dir.join("cache/icons.toml"),
            };
            Arc::new(
                EngineConfig::new(EngineMode::Full, self.dir.join("state"))
                    .with_sys_root(SysRoot::at(&self.dir.join("root")))
                    .with_app_paths(paths),
            )
        }
    }

    /// Makes the copy writable again so it can be removed.
    fn remove(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir.join("root/proc")) {
            for entry in entries.flatten() {
                let io = entry.path().join("io");
                let _ = fs::set_permissions(&io, fs::Permissions::from_mode(0o600));
            }
        }
        let _ = fs::remove_dir_all(dir);
    }

    impl Drop for Root {
        fn drop(&mut self) {
            remove(&self.dir);
        }
    }

    /// What the fake helper saw.
    #[derive(Default)]
    struct ReadState {
        /// Every `ReadProcessFiles` with the lab time in ms.
        reads: Vec<(u64, ProcessKey, ProcFiles)>,
        pending: VecDeque<HelperReply>,
        spawns: u32,
        closed: bool,
    }

    type Shared = Arc<Mutex<ReadState>>;

    fn lock(state: &Shared) -> std::sync::MutexGuard<'_, ReadState> {
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Answers `ReadProcessFiles` like E29-004: one `File` per requested file in bit order. The
    /// n-th `io` read of a key reports n MB read and n/2 MB written; other files fail with
    /// `EACCES`.
    struct ReadLink(Shared);

    impl HelperLink for ReadLink {
        fn send(&mut self, frame: &HelperFrame, _fd: Option<BorrowedFd<'_>>) -> io::Result<()> {
            let mut s = lock(&self.0);
            let mut bodies = Vec::new();
            match frame.request {
                HelperRequest::Hello { .. } => bodies.push(HelperReplyBody::Hello {
                    proto: HELPER_PROTO_VERSION,
                }),
                HelperRequest::ReadProcessFiles { key, files } => {
                    let n = s.reads.iter().filter(|r| r.1 == key).count() as u64 + 1;
                    s.reads.push((now_ms(), key, files));
                    for file in files.iter() {
                        bodies.push(if file == ProcFiles::IO {
                            let data = format!(
                                "rchar: 0\nwchar: 0\nsyscr: 0\nsyscw: 0\nread_bytes: {}\n\
                                 write_bytes: {}\ncancelled_write_bytes: 0\n",
                                n * 1_000_000,
                                n * 500_000
                            );
                            HelperReplyBody::File {
                                file,
                                data: data.into_bytes(),
                            }
                        } else {
                            HelperReplyBody::Err(HelperError::Os(13))
                        });
                    }
                }
                _ => bodies.push(HelperReplyBody::Err(HelperError::NotAllowed)),
            }
            let count = bodies.len();
            for (i, body) in bodies.into_iter().enumerate() {
                s.pending.push_back(HelperReply {
                    seq: frame.seq,
                    last: i + 1 == count,
                    body,
                });
            }
            Ok(())
        }

        fn recv(&mut self, _timeout: Duration) -> Result<HelperReply, LinkError> {
            lock(&self.0).pending.pop_front().ok_or(LinkError::Timeout)
        }

        fn exit_status(&mut self, _wait: Duration) -> Option<i32> {
            lock(&self.0).closed.then_some(0)
        }

        fn kill(&mut self) {
            lock(&self.0).closed = true;
        }

        fn close(&mut self) {
            lock(&self.0).closed = true;
        }
    }

    fn env(state: &Shared) -> LinkEnv {
        let state = Arc::clone(state);
        LinkEnv {
            installed: Arc::new(|| true),
            spawn: Arc::new(move || {
                lock(&state).spawns += 1;
                let link: Box<dyn HelperLink> = Box::new(ReadLink(Arc::clone(&state)));
                Ok::<_, StartError>(link)
            }),
            setup_probe: Arc::new(|| PROBED),
            setup_runner: Arc::new(|_, _| tl_model::command::SetupPhase::Succeeded),
        }
    }

    /// The helper task and the `Processes` worker in one lab, sweeping every second.
    struct Rig {
        fl: FakeLab,
        rx: mpsc::Receiver<EngineMsg>,
        demand: watch::Sender<Demand>,
        _settings: watch::Sender<EngineSettings>,
        _control: watch::Sender<WorkerControl>,
        helper: HelperHandle,
        tasks: Vec<TaskHandle<()>>,
        fake: Shared,
        sweeps: Vec<(u64, ProcessSweep)>,
    }

    impl Rig {
        /// With `unlock`, the helper session starts before the first sweep.
        fn start(root: &Root, demand: Demand, unlock: bool) -> Rig {
            let mut fl = FakeLab::new();
            CLOCK.with(|c| *c.borrow_mut() = Some(Arc::clone(&fl.clock)));
            let (updates, rx) = mpsc::channel::<EngineMsg>(4096);
            let (demand_tx, _) = watch::channel(demand);
            let (settings_tx, _) = watch::channel(EngineSettings {
                interval_ms: 1000,
                ..EngineSettings::default()
            });
            let (control, _) = watch::channel(WorkerControl::default());
            let fake = Shared::default();
            let (helper, queue) = helper_queue();
            let mut tasks = Vec::new();
            let (helper_updates, link_env) = (updates.clone(), env(&fake));
            tasks.push(fl.spawn(async move {
                let cx = Cx::current().expect("task has a Cx");
                helper_task_with(cx, queue, helper_updates, link_env).await;
            }));
            if unlock {
                let handle = helper.clone();
                tasks.push(fl.spawn(async move {
                    let cx = Cx::current().expect("task has a Cx");
                    handle.send(&cx, HelperMsg::Start).await.expect("queued");
                }));
                assert_eq!(helper.state(), HelperState::Unlocked);
            }
            let io = WorkerIo::new(
                updates,
                demand_tx.subscribe(),
                settings_tx.subscribe(),
                control.subscribe(),
                SharedHandles {
                    helper: Some(helper.clone()),
                    ..SharedHandles::default()
                },
            );
            let src = ProcessesSource::new(root.config())
                .expect("fixture source opens")
                .with_clock(lab_clock);
            tasks.push(fl.spawn(async move {
                let cx = Cx::current().expect("task has a Cx");
                let _ = processes::run(cx, io, src).await;
            }));
            let mut rig = Rig {
                fl,
                rx,
                demand: demand_tx,
                _settings: settings_tx,
                _control: control,
                helper,
                tasks,
                fake,
                sweeps: Vec::new(),
            };
            rig.collect();
            rig
        }

        fn collect(&mut self) {
            for msg in drain(&mut self.rx) {
                if let EngineMsg::Source(SourceUpdate::Processes(sweep)) = msg {
                    self.sweeps.push((self.fl.now_ms(), sweep));
                }
            }
        }

        fn run(&mut self, total: Duration) {
            let end = self.fl.now_ms() + total.as_millis() as u64;
            while self.fl.now_ms() < end {
                self.fl.advance(Duration::from_millis(100));
                self.collect();
            }
        }

        /// Shows `pids` as the visible rows, with their keys from the first sweep.
        fn show(&mut self, pids: &[u32]) {
            let first = &self.sweeps.first().expect("a sweep at 0 ms").1;
            let keys: Vec<ProcessKey> = pids.iter().map(|&p| record(first, p).key).collect();
            let mut demand = Demand::clone(&self.demand.borrow());
            demand.detail_keys = Arc::from(keys);
            self.demand.send(demand).expect("watch has receivers");
        }

        fn reads(&self) -> Vec<(u64, ProcessKey, ProcFiles)> {
            lock(&self.fake).reads.clone()
        }

        fn finish(mut self) {
            for task in &self.tasks {
                task.abort();
            }
            self.fl.advance(Duration::from_millis(10));
            drop((self.demand, self._settings, self._control, self.helper));
            drop(self.rx);
            self.fl.finish();
        }
    }

    /// The table with the I/O read rate column shown, no visible rows yet.
    fn io_demand() -> Demand {
        let mut demand = Demand {
            any_surface_visible: true,
            process_table: true,
            ..Demand::default()
        };
        demand.process_columns.insert(ColumnId::IoReadRate);
        demand
    }

    fn record(sweep: &ProcessSweep, pid: u32) -> &ProcessRecord {
        sweep
            .records
            .iter()
            .find(|r| r.key.pid == pid)
            .unwrap_or_else(|| panic!("pid {pid} not swept"))
    }

    fn is_root() -> bool {
        let root = rustix::process::geteuid().is_root();
        if root {
            eprintln!("skipped: running as root, so no io file is Restricted");
        }
        root
    }

    #[test]
    fn sweep_fills_restricted_io() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.restrict_io(42);
        let mut rig = Rig::start(&root, io_demand(), true);
        rig.show(&[42]);
        rig.run(Duration::from_millis(3_050));

        let seen: Vec<(u64, Validity, f32, f32)> = rig
            .sweeps
            .iter()
            .map(|(t, s)| {
                let r = record(s, 42);
                let v = r.validity.get(ProcessField::IoRead);
                assert_eq!(v, r.validity.get(ProcessField::IoWrite), "at {t} ms");
                (*t, v, r.io_read_bps, r.io_write_bps)
            })
            .collect();
        assert_eq!(
            seen,
            [
                // Not visible yet at 0 ms; the first read is at 1000 ms, after that sweep.
                (0, Validity::Restricted, 0.0, 0.0),
                (1_000, Validity::Restricted, 0.0, 0.0),
                // The first helper value has no rate.
                (2_000, Validity::WarmingUp, 0.0, 0.0),
                // 1 MB read and 0.5 MB written in the second between the reads.
                (3_000, Validity::Valid, 1_000_000.0, 500_000.0),
            ]
        );
        let reads = rig.reads();
        assert!(
            reads.iter().all(|r| r.1.pid == 42 && r.2 == ProcFiles::IO),
            "only the io of pid 42: {reads:?}"
        );
        assert_eq!(
            reads.iter().map(|r| r.0).collect::<Vec<_>>(),
            [1_000, 2_000, 3_000]
        );
        rig.finish();
    }

    /// Security regression: without an unlocked session, the sweep never calls the helper and
    /// never starts a session (no polkit prompt).
    #[test]
    fn sweep_locked_no_helper() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.restrict_io(42);
        let mut rig = Rig::start(&root, io_demand(), false);
        assert_eq!(rig.helper.state(), HelperState::Locked);
        rig.show(&[42]);
        rig.run(Duration::from_millis(3_050));

        assert_eq!(rig.sweeps.len(), 4);
        for (t, sweep) in &rig.sweeps {
            let r = record(sweep, 42);
            assert_eq!(
                r.validity.get(ProcessField::IoRead),
                Validity::Restricted,
                "at {t} ms"
            );
            assert_eq!(r.io_read_bps, 0.0);
        }
        let fake = lock(&rig.fake);
        assert!(fake.reads.is_empty(), "{:?}", fake.reads);
        assert_eq!(fake.spawns, 0, "no session was started");
        drop(fake);
        assert_eq!(rig.helper.state(), HelperState::Locked);
        rig.finish();
    }

    #[test]
    fn sweep_helper_cap_64_per_second() {
        if is_root() {
            return;
        }
        let root = Root::new();
        let pids: Vec<u32> = (6_000..6_100).collect();
        for &pid in &pids {
            root.add_restricted(pid);
        }
        let mut rig = Rig::start(&root, io_demand(), true);
        rig.show(&pids);
        rig.run(Duration::from_millis(3_050));

        let reads = rig.reads();
        for second in 0..4 {
            let window = second * 1_000..(second + 1) * 1_000;
            let n = reads.iter().filter(|r| window.contains(&r.0)).count();
            assert!(n <= HELPER_READS_MAX, "{n} reads in second {second}");
        }
        let first = reads.iter().filter(|r| r.0 == 1_000).count();
        assert_eq!(first, HELPER_READS_MAX, "the first second reads the cap");
        // The rows past the cap get their turn in the next second.
        let mut read_pids: Vec<u32> = reads.iter().map(|r| r.1.pid).collect();
        read_pids.sort_unstable();
        read_pids.dedup();
        assert_eq!(read_pids, pids);
        rig.finish();
    }

    #[test]
    fn sweep_only_visible_rows() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.restrict_io(42);
        root.restrict_io(43);
        let mut rig = Rig::start(&root, io_demand(), true);
        rig.show(&[43]);
        rig.run(Duration::from_millis(3_050));

        let reads = rig.reads();
        assert!(!reads.is_empty(), "the visible row is read");
        assert!(
            reads.iter().all(|r| r.1.pid == 43),
            "pid 42 is not visible: {reads:?}"
        );
        for (t, sweep) in &rig.sweeps {
            assert_eq!(
                record(sweep, 42).validity.get(ProcessField::IoRead),
                Validity::Restricted,
                "at {t} ms"
            );
        }
        rig.finish();
    }
}
