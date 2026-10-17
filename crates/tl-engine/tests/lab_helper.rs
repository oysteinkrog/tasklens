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
//!
//! The `detail_helper` tests run the `ProcessDetail` worker with the helper task over a fake
//! that answers `ReadProcessFiles` and `ScanStrings` (E29-015).

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
use tl_model::details::StringScanOpts;
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

fn scan_req() -> HelperRequest {
    HelperRequest::ScanStrings {
        key: ProcessKey {
            start_ticks: 4242,
            boot_id: BootId(7),
            pid: 1234,
        },
        opts: StringScanOpts::default(),
    }
}

/// qfx.18: dropping a `read_stream` stops the scan with `CancelScan` for its frame `seq` (1,
/// after the `Hello`). The lab runs the helper call inline, so the stream is dropped before the
/// task takes it and the first reply finds no receiver.
#[test]
fn read_stream_drop_sends_cancel_scan() {
    let (fake, env) = Fake::new(true);
    // The scan's two replies, then the `Ok` of the cancel frame.
    lock(&fake.state).answers = [Answer::Replies(2), Answer::Replies(1)].into();
    run(lab(25), env, |cx, handle, _helper| async move {
        handle.send(&cx, HelperMsg::Start).await.expect("queued");
        wait_for(&cx, &handle, HelperState::Unlocked).await;
        let stream = handle
            .read_stream(&cx, scan_req(), Duration::from_secs(5))
            .await
            .expect("queued");
        drop(stream);
        time::sleep(cx.now(), Duration::from_millis(10)).await;
        assert_eq!(handle.state(), HelperState::Unlocked, "the session stays");
    });
    assert_eq!(
        requests(&fake),
        [scan_req(), HelperRequest::CancelScan { seq: 1 }]
    );
}

/// qfx.18: a `read_stream` while `Locked` starts nothing and ends with `NotRunning`.
#[test]
fn read_stream_locked_never_starts() {
    let (fake, env) = Fake::new(true);
    run(lab(26), env, |cx, handle, _helper| async move {
        wait_for(&cx, &handle, HelperState::Locked).await;
        let stream = handle
            .read_stream(&cx, scan_req(), Duration::from_secs(5))
            .await
            .expect("queued");
        assert!(stream.replies.recv().await.is_err(), "no reply");
        assert_eq!(stream.result(&cx).await, Err(HelperReadError::NotRunning));
    });
    assert_eq!(fake.spawns.load(Ordering::SeqCst), 0);
    assert!(requests(&fake).is_empty());
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

/// The `Connections` worker on scripted socket dumps, with the helper task on a fake link that
/// answers `ResolveSocketInodes` (E29-018). No test runs the real helper or pkexec.
mod connections_helper {
    use std::collections::VecDeque;
    use std::io;
    use std::net::{IpAddr, Ipv4Addr};
    use std::os::fd::BorrowedFd;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use asupersync::Cx;
    use asupersync::channel::{mpsc, watch};
    use asupersync::runtime::TaskHandle;
    use rustc_hash::FxHashMap;
    use tl_collect::CollectError;
    use tl_collect::net::sock_diag::{self, SockRecord, TcpInfoLite};
    use tl_collect::net::{IfNames, SocketMap};
    use tl_engine::dbus::SharedHandles;
    use tl_engine::helper::{
        HelperHandle, HelperLink, HelperMsg, LinkEnv, LinkError, StartError, helper_queue,
        helper_task_with,
    };
    use tl_engine::source::{WorkerControl, WorkerIo};
    use tl_engine::update::{ConnectionsUpdate, EngineMsg, SourceUpdate};
    use tl_engine::workers::connections::{self, ConnectionsSource, RESOLVE_MAX, SocketReader};
    use tl_model::demand::Demand;
    use tl_model::health::HelperState;
    use tl_model::helper_proto::{
        HELPER_PROTO_VERSION, HelperError, HelperFrame, HelperReply, HelperReplyBody, HelperRequest,
    };
    use tl_model::ids::ProcessKey;
    use tl_model::settings::EngineSettings;
    use tl_model::snapshot::ConnRecord;
    use tl_model::time::BootId;
    use tl_model::validity::Validity;

    use super::PROBED;
    use super::fake_sources::{FakeLab, drain};

    /// The uid of the other user whose process holds the unattributed sockets.
    const OTHER_UID: u32 = 0;
    /// That user's process, whose fd directory cannot be read.
    const OTHER_PID: u32 = 900;
    /// A socket of our own process 100.
    const OWN_INODE: u64 = 55;

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid) * 10,
            boot_id: BootId(7),
            pid,
        }
    }

    fn tcp(cookie: u64, inode: u64, uid: u32, state: u8) -> SockRecord {
        SockRecord {
            family: sock_diag::Family::V4,
            proto: sock_diag::Proto::Tcp,
            state,
            src: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            dst: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9)),
            sport: 40000,
            dport: 443,
            ifindex: 0,
            cookie,
            rqueue: 0,
            wqueue: 0,
            uid,
            inode,
            tcp_info: Some(TcpInfoLite::default()),
        }
    }

    /// One own socket, one TIME-WAIT socket of the other user (inode 0), and an ESTABLISHED
    /// socket of the other user per inode of `other`.
    fn dump(other: impl IntoIterator<Item = u64>) -> Vec<SockRecord> {
        let mut socks = vec![tcp(1, OWN_INODE, 1000, 1), tcp(2, 0, OTHER_UID, 6)];
        socks.extend(
            other
                .into_iter()
                .enumerate()
                .map(|(i, inode)| tcp(10 + i as u64, inode, OTHER_UID, 1)),
        );
        socks
    }

    /// Scripted dumps; the map has our own socket and `OTHER_PID` as restricted.
    struct Reader {
        tcp: Vec<SockRecord>,
    }

    impl SocketReader for Reader {
        fn dump_tcp(&mut self, out: &mut Vec<SockRecord>) -> Result<(), CollectError> {
            out.clear();
            out.extend(self.tcp.iter().cloned());
            Ok(())
        }

        fn dump_udp(&mut self, _out: &mut Vec<SockRecord>) -> Result<(), CollectError> {
            Ok(())
        }

        fn rebuild_map(&mut self, map: &mut SocketMap) -> Result<(), CollectError> {
            map.owners.clear();
            map.restricted.clear();
            map.owners
                .insert(OWN_INODE, std::iter::once(key(100)).collect());
            map.restricted.insert(OTHER_PID, OTHER_UID);
            Ok(())
        }

        fn if_names(&mut self) -> IfNames {
            IfNames::default()
        }
    }

    /// What the fake helper saw, and the owners it knows.
    #[derive(Default)]
    struct ResolveState {
        /// The inodes of every `ResolveSocketInodes`.
        requests: Vec<Vec<u64>>,
        owners: FxHashMap<u64, Vec<ProcessKey>>,
        pending: VecDeque<HelperReply>,
        spawns: u32,
        closed: bool,
    }

    type Shared = Arc<Mutex<ResolveState>>;

    fn lock(state: &Shared) -> std::sync::MutexGuard<'_, ResolveState> {
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Answers `ResolveSocketInodes` like tl-helper `sockets.rs`: one `SocketOwners` with a
    /// pair per known holder of an asked inode.
    struct ResolveLink(Shared);

    impl HelperLink for ResolveLink {
        fn send(&mut self, frame: &HelperFrame, _fd: Option<BorrowedFd<'_>>) -> io::Result<()> {
            let mut s = lock(&self.0);
            let body = match &frame.request {
                HelperRequest::Hello { .. } => HelperReplyBody::Hello {
                    proto: HELPER_PROTO_VERSION,
                },
                HelperRequest::ResolveSocketInodes { inodes } => {
                    s.requests.push(inodes.clone());
                    let pairs = inodes
                        .iter()
                        .flat_map(|inode| {
                            s.owners
                                .get(inode)
                                .into_iter()
                                .flatten()
                                .map(move |&k| (*inode, k))
                        })
                        .collect();
                    HelperReplyBody::SocketOwners(pairs)
                }
                _ => HelperReplyBody::Err(HelperError::NotAllowed),
            };
            s.pending.push_back(HelperReply {
                seq: frame.seq,
                last: true,
                body,
            });
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
                let link: Box<dyn HelperLink> = Box::new(ResolveLink(Arc::clone(&state)));
                Ok::<_, StartError>(link)
            }),
            setup_probe: Arc::new(|| PROBED),
            setup_runner: Arc::new(|_, _| tl_model::command::SetupPhase::Succeeded),
        }
    }

    /// The helper task and the `Connections` worker in one lab, with the Connections tab shown.
    struct Rig {
        fl: FakeLab,
        rx: mpsc::Receiver<EngineMsg>,
        demand: watch::Sender<Demand>,
        _settings: watch::Sender<EngineSettings>,
        _control: watch::Sender<WorkerControl>,
        helper: HelperHandle,
        tasks: Vec<TaskHandle<()>>,
        fake: Shared,
        updates: Vec<ConnectionsUpdate>,
    }

    impl Rig {
        /// With `unlock`, the helper session starts before the first rebuild.
        fn start(tcp: Vec<SockRecord>, owners: &[(u64, &[u32])], unlock: bool) -> Rig {
            let mut fl = FakeLab::new();
            let (updates, rx) = mpsc::channel::<EngineMsg>(4096);
            let (demand_tx, _) = watch::channel(Demand {
                connections: true,
                ..Demand::default()
            });
            let (settings_tx, _) = watch::channel(EngineSettings::default());
            let (control, _) = watch::channel(WorkerControl::default());
            let fake = Shared::default();
            lock(&fake).owners = owners
                .iter()
                .map(|&(inode, pids)| (inode, pids.iter().map(|&p| key(p)).collect()))
                .collect();
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
            let src = ConnectionsSource::new(Reader { tcp });
            tasks.push(fl.spawn(async move {
                let cx = Cx::current().expect("task has a Cx");
                let _ = connections::run(cx, io, src).await;
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
                updates: Vec::new(),
            };
            rig.collect();
            rig
        }

        fn collect(&mut self) {
            for msg in drain(&mut self.rx) {
                if let EngineMsg::Source(SourceUpdate::Connections(update)) = msg {
                    self.updates.push(update);
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

        fn requests(&self) -> Vec<Vec<u64>> {
            lock(&self.fake).requests.clone()
        }

        fn last(&self) -> &ConnectionsUpdate {
            self.updates.last().expect("a rebuild ran")
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

    fn row(update: &ConnectionsUpdate, inode: u64) -> &ConnRecord {
        update
            .records
            .iter()
            .find(|r| r.inode == inode)
            .unwrap_or_else(|| panic!("no row for inode {inode}"))
    }

    fn owner_pids(r: &ConnRecord) -> Vec<u32> {
        r.owners.iter().map(|k| k.pid).collect()
    }

    #[test]
    fn connections_resolve_other_users() {
        let mut rig = Rig::start(dump([101, 102, 103]), &[(101, &[500]), (102, &[501])], true);
        rig.run(Duration::from_millis(4_050));

        let requests = rig.requests();
        assert!(!requests.is_empty());
        assert!(
            requests.iter().all(|r| r == &[101, 102, 103]),
            "only the unattributed inodes, never our own or TIME-WAIT: {requests:?}"
        );
        let last = rig.last();
        assert_eq!(owner_pids(row(last, 101)), [500]);
        assert_eq!(row(last, 101).owner_validity, Validity::Valid);
        assert_eq!(owner_pids(row(last, 102)), [501]);
        // The one the helper did not resolve keeps "?" with the restricted glyph.
        assert!(row(last, 103).owners.is_empty());
        assert_eq!(row(last, 103).owner_validity, Validity::Restricted);
        // TIME-WAIT stays "No owner (kernel)".
        assert!(row(last, 0).owners.is_empty());
        assert_eq!(row(last, 0).owner_validity, Validity::Valid);
        // The process TCP Network column gets the same owners.
        assert!(last.attribution.contains_key(&key(500)));
        assert!(last.attribution.contains_key(&key(501)));
        rig.finish();
    }

    /// Security regression: without an unlocked session, the rebuild never calls the helper
    /// and never starts a session (no polkit prompt).
    #[test]
    fn connections_locked_no_helper() {
        let mut rig = Rig::start(dump([101, 102, 103]), &[(101, &[500])], false);
        assert_eq!(rig.helper.state(), HelperState::Locked);
        rig.run(Duration::from_millis(6_050));

        assert!(rig.updates.len() >= 3, "{} rebuilds", rig.updates.len());
        for update in &rig.updates {
            for inode in [101, 102, 103] {
                assert_eq!(row(update, inode).owner_validity, Validity::Restricted);
            }
        }
        let fake = lock(&rig.fake);
        assert!(fake.requests.is_empty(), "{:?}", fake.requests);
        assert_eq!(fake.spawns, 0, "no session was started");
        drop(fake);
        assert_eq!(rig.helper.state(), HelperState::Locked);
        rig.finish();
    }

    #[test]
    fn connections_shared_inode_two_keys() {
        let mut rig = Rig::start(dump([101]), &[(101, &[501, 500])], true);
        rig.run(Duration::from_millis(4_050));

        // Both owners, sorted, so the row shows "pid +1".
        let r = row(rig.last(), 101);
        assert_eq!(owner_pids(r), [500, 501]);
        assert_eq!(r.owner_validity, Validity::Valid);
        rig.finish();
    }

    #[test]
    fn connections_inode_batch_cap() {
        let inodes: Vec<u64> = (10_000..15_000).collect();
        let mut rig = Rig::start(dump(inodes.iter().copied()), &[], true);
        rig.run(Duration::from_millis(1_050));

        let requests = rig.requests();
        assert_eq!(requests.len(), 1, "one request per rebuild");
        assert_eq!(requests[0].len(), RESOLVE_MAX);
        assert_eq!(requests[0], inodes[..RESOLVE_MAX]);

        // The rest go first in the next rebuild's request, which wraps round to the cap.
        rig.run(Duration::from_millis(2_000));
        let requests = rig.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].len(), RESOLVE_MAX);
        assert!(
            inodes[RESOLVE_MAX..]
                .iter()
                .all(|i| requests[1].binary_search(i).is_ok())
        );
        rig.finish();
    }
}

/// The `ProcessDetail` worker with the helper task on a fake link that answers
/// `ReadProcessFiles` and `ScanStrings` (E29-015, qfx.18). No test runs the real helper or
/// pkexec. A file is Restricted by mode 000, so those tests skip as root.
mod detail_helper {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::os::fd::BorrowedFd;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    use asupersync::Cx;
    use asupersync::channel::{mpsc, watch};
    use asupersync::runtime::{BlockingPool, TaskHandle};
    use asupersync::time::{TimeSource, VirtualClock};
    use tl_collect::SysRoot;
    use tl_engine::config::{EngineConfig, EngineMode};
    use tl_engine::dbus::SharedHandles;
    use tl_engine::helper::{
        HelperHandle, HelperLink, HelperMsg, LinkEnv, LinkError, StartError, helper_queue,
        helper_task_with,
    };
    use tl_engine::source::{QueuedCommand, WorkerCommand, WorkerControl, WorkerIo};
    use tl_engine::update::{DetailsUpdate, EngineMsg, PssEntry, SourceUpdate};
    use tl_engine::workers::process_detail::{self, ProcessDetailSource};
    use tl_model::demand::{Demand, DetailDemand};
    use tl_model::details::{
        DetailTab, DetailTarget, ScanId, ScanPhase, StringEncoding, StringScanOpts, TabPayload,
    };
    use tl_model::health::{HelperState, SourceId};
    use tl_model::helper_proto::{
        HELPER_PROTO_VERSION, HelperError, HelperFrame, HelperReply, HelperReplyBody,
        HelperRequest, ProcFiles,
    };
    use tl_model::ids::{ColumnId, ProcessKey};
    use tl_model::settings::EngineSettings;
    use tl_model::time::{BootId, TimeNs};
    use tl_model::validity::Validity;

    use super::PROBED;
    use super::fake_sources::{FakeLab, drain};

    const PID: u32 = 4201;
    const ENVIRON: &[u8] = b"HOME=/root\0SHELL=/bin/sh\0";
    const MAPS: &str = "00400000-00452000 r-xp 00000000 08:02 173521 /usr/bin/helper-test\n\
                        7ffd00000000-7ffd00021000 rw-p 00000000 00:00 0 [stack]\n";
    const SMAPS: &str = "00400000-00452000 r-xp 00000000 08:02 173521 /usr/bin/helper-test\n\
                         Rss: 8 kB\nPss: 8 kB\nSwap: 0 kB\n\
                         7ffd00000000-7ffd00021000 rw-p 00000000 00:00 0 [stack]\n\
                         Rss: 12 kB\nPss: 12 kB\nSwap: 0 kB\n";
    const ROLLUP: &str = "Rss: 400 kB\nPss: 300 kB\nPrivate_Clean: 40 kB\n\
                          Private_Dirty: 60 kB\nSwap: 8 kB\n";
    /// A root-owned `status`, so the helper's uids differ from the fixture's 1000.
    const STATUS: &str = "Name:\thelper-test\nUid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\n\
                          NoNewPrivs:\t1\nSeccomp:\t0\nCpus_allowed_list:\t0-3\n";

    thread_local! {
        /// The clock of the current test's lab. The lab runs every task on the test thread.
        static CLOCK: RefCell<Option<Arc<VirtualClock>>> = const { RefCell::new(None) };
    }

    fn now_ms() -> u64 {
        lab_clock() / 1_000_000
    }

    /// The sample clock: lab virtual time.
    fn lab_clock() -> TimeNs {
        CLOCK.with(|c| {
            c.borrow()
                .as_ref()
                .expect("the rig set the clock")
                .now()
                .as_nanos()
        })
    }

    fn key() -> ProcessKey {
        ProcessKey {
            start_ticks: 351_220,
            boot_id: BootId(1),
            pid: PID,
        }
    }

    /// Copies `src` over `dst`, keeping symlinks as links.
    fn copy_tree(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).expect("create fixture dir");
        for entry in fs::read_dir(src).expect("list fixture dir") {
            let entry = entry.expect("fixture dir entry");
            let to = dst.join(entry.file_name());
            let file_type = entry.file_type().expect("fixture file type");
            if file_type.is_symlink() {
                let target = fs::read_link(entry.path()).expect("read fixture symlink");
                std::os::unix::fs::symlink(target, &to).expect("create fixture symlink");
            } else if file_type.is_dir() {
                copy_tree(&entry.path(), &to);
            } else {
                fs::copy(entry.path(), &to).expect("copy fixture file");
            }
        }
    }

    /// A writable copy of `process-edge` under the target tmpdir, removed on drop.
    struct Root {
        dir: PathBuf,
    }

    impl Root {
        fn new() -> Root {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("lab-helper-detail-{}-{n}", std::process::id()));
            if dir.exists() {
                remove(&dir);
            }
            let src = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tl-collect/tests/fixtures/process-edge");
            copy_tree(&src, &dir);
            Root { dir }
        }

        /// Makes `/proc/4201/<name>` unreadable: Restricted, as for another user's process.
        fn restrict(&self, name: &str) {
            let path = self.dir.join(format!("proc/{PID}/{name}"));
            fs::set_permissions(path, fs::Permissions::from_mode(0o000)).expect("chmod");
        }

        /// Yama scope 2: tasklens may attach to no process, so a Strings scan is Restricted.
        fn ptrace_scope_2(&self) {
            let dir = self.dir.join("proc/sys/kernel/yama");
            fs::create_dir_all(&dir).expect("create yama dir");
            fs::write(dir.join("ptrace_scope"), "2\n").expect("write ptrace_scope");
        }

        fn config(&self) -> Arc<EngineConfig> {
            Arc::new(
                EngineConfig::new(EngineMode::Full, self.dir.join("state"))
                    .with_sys_root(SysRoot::at(&self.dir)),
            )
        }
    }

    /// Makes the copy readable again so it can be removed.
    fn remove(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir.join(format!("proc/{PID}"))) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_file()) {
                    let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o644));
                }
            }
        }
        let _ = fs::remove_dir_all(dir);
    }

    impl Drop for Root {
        fn drop(&mut self) {
            remove(&self.dir);
        }
    }

    fn is_root() -> bool {
        let root = rustix::process::geteuid().is_root();
        if root {
            eprintln!("skipped: running as root, so no file is Restricted");
        }
        root
    }

    /// What the fake helper saw and how it answers.
    #[derive(Default)]
    struct DetailState {
        /// Every `ReadProcessFiles` with the lab time in ms.
        reads: Vec<(u64, ProcessKey, ProcFiles)>,
        /// Every `ScanStrings` and `CancelScan`, in order.
        scans: Vec<HelperRequest>,
        /// The frame `seq` of each `ScanStrings`.
        scan_seqs: Vec<u32>,
        /// With `hold_scan`, a scan sends its first batch and keeps the rest here until
        /// `released`. Only a rig with a blocking pool may hold: the call then waits on its own
        /// thread.
        hold_scan: bool,
        held: Vec<HelperReply>,
        released: bool,
        /// Files answered with this errno instead of their data.
        errors: Vec<(ProcFiles, i32)>,
        pending: VecDeque<HelperReply>,
        spawns: u32,
        closed: bool,
    }

    /// The fake's state, and the condvar a held scan waits on.
    type Shared = Arc<(Mutex<DetailState>, Condvar)>;

    fn lock(state: &Shared) -> std::sync::MutexGuard<'_, DetailState> {
        state
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Lets a held scan send the rest of its replies.
    fn release(state: &Shared) {
        lock(state).released = true;
        state.1.notify_all();
    }

    /// Answers like E29-004: one `File` body per requested file in bit order, or `Err(Os(_))`.
    /// A scan sends two batches, the second one final, or with `hold_scan` the first batch, then
    /// after `release` the second and an empty final one.
    struct DetailLink(Shared);

    fn file_data(file: ProcFiles) -> Option<Vec<u8>> {
        let data: &[u8] = match file {
            ProcFiles::ENVIRON => ENVIRON,
            ProcFiles::MAPS => MAPS.as_bytes(),
            ProcFiles::SMAPS => SMAPS.as_bytes(),
            ProcFiles::SMAPS_ROLLUP => ROLLUP.as_bytes(),
            ProcFiles::STATUS => STATUS.as_bytes(),
            _ => return None,
        };
        Some(data.to_vec())
    }

    fn batch(address: u64, text: &str) -> HelperReplyBody {
        HelperReplyBody::Strings(vec![(address, StringEncoding::Ascii, text.into())])
    }

    impl HelperLink for DetailLink {
        fn send(&mut self, frame: &HelperFrame, _fd: Option<BorrowedFd<'_>>) -> io::Result<()> {
            let mut s = lock(&self.0);
            let mut bodies = Vec::new();
            match &frame.request {
                HelperRequest::Hello { .. } => bodies.push(HelperReplyBody::Hello {
                    proto: HELPER_PROTO_VERSION,
                }),
                HelperRequest::ReadProcessFiles { key, files } => {
                    s.reads.push((now_ms(), *key, *files));
                    for file in files.iter() {
                        let errno = s.errors.iter().find(|e| e.0 == file).map(|e| e.1);
                        bodies.push(match (errno, file_data(file)) {
                            (None, Some(data)) => HelperReplyBody::File { file, data },
                            (Some(errno), _) => HelperReplyBody::Err(HelperError::Os(errno)),
                            (None, None) => HelperReplyBody::Err(HelperError::Os(13)),
                        });
                    }
                }
                req @ HelperRequest::ScanStrings { .. } => {
                    s.scans.push(req.clone());
                    s.scan_seqs.push(frame.seq);
                    if s.hold_scan {
                        let reply = |last, body| HelperReply {
                            seq: frame.seq,
                            last,
                            body,
                        };
                        let first = reply(false, batch(0x1000, "first"));
                        s.held = vec![
                            reply(false, batch(0x2000, "second")),
                            reply(true, HelperReplyBody::Strings(Vec::new())),
                        ];
                        s.pending.push_back(first);
                        return Ok(());
                    }
                    bodies.push(batch(0x1000, "first"));
                    bodies.push(batch(0x2000, "second"));
                }
                req @ HelperRequest::CancelScan { .. } => {
                    s.scans.push(req.clone());
                    bodies.push(HelperReplyBody::Ok);
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
            let mut s = lock(&self.0);
            loop {
                if let Some(reply) = s.pending.pop_front() {
                    return Ok(reply);
                }
                if s.held.is_empty() {
                    return Err(LinkError::Timeout);
                }
                if s.released {
                    let held = std::mem::take(&mut s.held);
                    s.pending.extend(held);
                    continue;
                }
                // Real time: this runs on a blocking pool thread while the lab goes on.
                let (guard, wait) = self
                    .0
                    .1
                    .wait_timeout(s, Duration::from_secs(10))
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                s = guard;
                if wait.timed_out() {
                    return Err(LinkError::Timeout);
                }
            }
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
                let link: Box<dyn HelperLink> = Box::new(DetailLink(Arc::clone(&state)));
                Ok::<_, StartError>(link)
            }),
            setup_probe: Arc::new(|| PROBED),
            setup_runner: Arc::new(|_, _| tl_model::command::SetupPhase::Succeeded),
        }
    }

    /// One tab update the worker sent.
    struct TabOut {
        ms: u64,
        tab: DetailTab,
        payload: Arc<TabPayload>,
        validity: Validity,
    }

    /// The helper task and the `ProcessDetail` worker in one lab.
    struct Rig {
        fl: FakeLab,
        rx: mpsc::Receiver<EngineMsg>,
        _demand: watch::Sender<Demand>,
        _settings: watch::Sender<EngineSettings>,
        control: watch::Sender<WorkerControl>,
        next_seq: u64,
        helper: HelperHandle,
        tasks: Vec<TaskHandle<()>>,
        fake: Shared,
        tabs: Vec<TabOut>,
        pss: Vec<(u64, PssEntry)>,
        errors: Vec<Arc<str>>,
        /// Set by `start_pooled`: helper calls run on this pool's threads, not inline.
        _pool: Option<BlockingPool>,
    }

    impl Rig {
        /// With `unlock`, the helper session starts before the first sample.
        fn start(root: &Root, demand: Demand, interval_ms: u32, unlock: bool) -> Rig {
            Rig::start_with(root, demand, interval_ms, unlock, Vec::new())
        }

        fn start_with(
            root: &Root,
            demand: Demand,
            interval_ms: u32,
            unlock: bool,
            errors: Vec<(ProcFiles, i32)>,
        ) -> Rig {
            Rig::start_full(root, demand, interval_ms, unlock, errors, None)
        }

        /// Helper calls run on a blocking pool thread, so a held scan waits there while the
        /// worker goes on, and the fake holds every scan after its first batch. The worker's first
        /// sample comes before the unlock.
        fn start_pooled(root: &Root, demand: Demand) -> Rig {
            Rig::start_full(
                root,
                demand,
                1000,
                true,
                Vec::new(),
                Some(BlockingPool::new(1, 2)),
            )
        }

        fn start_full(
            root: &Root,
            demand: Demand,
            interval_ms: u32,
            unlock: bool,
            errors: Vec<(ProcFiles, i32)>,
            pool: Option<BlockingPool>,
        ) -> Rig {
            let mut fl = FakeLab::new();
            CLOCK.with(|c| *c.borrow_mut() = Some(Arc::clone(&fl.clock)));
            let (updates, rx) = mpsc::channel::<EngineMsg>(4096);
            let (demand_tx, _) = watch::channel(demand);
            let (settings_tx, _) = watch::channel(EngineSettings {
                interval_ms,
                ..EngineSettings::default()
            });
            let (control, _) = watch::channel(WorkerControl::default());
            let fake = Shared::default();
            lock(&fake).errors = errors;
            lock(&fake).hold_scan = pool.is_some();
            let (helper, queue) = helper_queue();
            let io = WorkerIo::new(
                updates.clone(),
                demand_tx.subscribe(),
                settings_tx.subscribe(),
                control.subscribe(),
                SharedHandles {
                    helper: Some(helper.clone()),
                    ..SharedHandles::default()
                },
            );
            let src = ProcessDetailSource::new(root.config()).with_clock(lab_clock);
            let mut worker = Some(async move {
                let cx = Cx::current().expect("task has a Cx");
                let _ = process_detail::run(cx, io, src).await;
            });
            let mut rig_tasks = Vec::new();
            if let Some(pool) = &pool {
                // Each `Cx` keeps the pool it was made with. The worker starts first, so its
                // samples stay inline on the lab thread and read the lab clock; only the helper
                // task's calls run on the pool.
                rig_tasks.push(fl.spawn(worker.take().expect("not started")));
                fl.lab.state.set_blocking_pool(pool.handle());
            }
            let (helper_updates, link_env) = (updates, env(&fake));
            rig_tasks.push(fl.spawn(async move {
                let cx = Cx::current().expect("task has a Cx");
                helper_task_with(cx, queue, helper_updates, link_env).await;
            }));
            let mut rig = Rig {
                fl,
                rx,
                _demand: demand_tx,
                _settings: settings_tx,
                control,
                next_seq: 1,
                helper,
                tasks: rig_tasks,
                fake,
                tabs: Vec::new(),
                pss: Vec::new(),
                errors: Vec::new(),
                _pool: pool,
            };
            if unlock {
                rig.unlock();
            }
            if let Some(worker) = worker {
                rig.tasks.push(rig.fl.spawn(worker));
            }
            rig.collect();
            rig
        }

        /// Starts the helper session, as the Grant access button does.
        fn unlock(&mut self) {
            let handle = self.helper.clone();
            self.tasks.push(self.fl.spawn(async move {
                let cx = Cx::current().expect("task has a Cx");
                handle.send(&cx, HelperMsg::Start).await.expect("queued");
            }));
            // With a pool the start runs on its thread.
            self.wait_real(5_000, |rig| rig.helper.state() == HelperState::Unlocked);
            assert_eq!(self.helper.state(), HelperState::Unlocked);
        }

        /// Runs the lab without moving the clock, in real time, until `done` or `limit_ms` pass:
        /// work on a pool thread wakes lab tasks from there.
        fn wait_real(&mut self, limit_ms: u32, done: impl Fn(&Rig) -> bool) {
            for _ in 0..limit_ms {
                if done(self) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
                self.fl.lab.run_until_idle();
                self.collect();
            }
        }

        /// Moves the clock in 50 ms steps, with up to 20 ms of real time after each for the pool
        /// thread, until `done` or 5 s of virtual time pass.
        fn run_until(&mut self, done: impl Fn(&Rig) -> bool) {
            for _ in 0..100 {
                if done(self) {
                    return;
                }
                self.run(Duration::from_millis(50));
                self.wait_real(20, &done);
            }
        }

        /// Forwards `command` to the worker, as the reducer does. The worker takes the commands
        /// newer than the last one it saw at its next sample.
        fn command(&mut self, command: WorkerCommand) {
            let mut control = WorkerControl::clone(&self.control.borrow());
            let mut commands = control.commands.to_vec();
            commands.push(QueuedCommand {
                seq: self.next_seq,
                target: SourceId::ProcessDetail,
                command,
            });
            control.commands = Arc::from(commands);
            self.next_seq += 1;
            self.control.send(control).expect("watch has receivers");
        }

        fn collect(&mut self) {
            let ms = self.fl.now_ms();
            for msg in drain(&mut self.rx) {
                match msg {
                    EngineMsg::Source(SourceUpdate::Details(DetailsUpdate::Tab {
                        tab,
                        payload,
                        validity,
                        ..
                    })) => self.tabs.push(TabOut {
                        ms,
                        tab,
                        payload,
                        validity,
                    }),
                    EngineMsg::Source(SourceUpdate::Details(DetailsUpdate::Pss {
                        entries,
                        ..
                    })) => self.pss.extend(entries.into_iter().map(|e| (ms, e))),
                    EngineMsg::Source(SourceUpdate::Health {
                        last_error: Some(error),
                        ..
                    }) => self.errors.push(error),
                    _ => {}
                }
            }
        }

        /// Runs `total` of virtual time in 50 ms steps.
        fn run(&mut self, total: Duration) {
            let end = self.fl.now_ms() + total.as_millis() as u64;
            while self.fl.now_ms() < end {
                self.fl.advance(Duration::from_millis(50));
                self.collect();
            }
        }

        fn reads(&self) -> Vec<(u64, ProcessKey, ProcFiles)> {
            lock(&self.fake).reads.clone()
        }

        fn tab(&self, tab: DetailTab) -> Vec<&TabOut> {
            self.tabs.iter().filter(|t| t.tab == tab).collect()
        }

        fn finish(mut self) {
            for task in &self.tasks {
                task.abort();
            }
            self.fl.advance(Duration::from_millis(10));
            drop((self._demand, self._settings, self.control, self.helper));
            drop(self.rx);
            self.fl.finish();
        }
    }

    fn demand(tabs: &[DetailTab]) -> Demand {
        let details: Vec<DetailDemand> = tabs
            .iter()
            .map(|&tab| DetailDemand {
                target: DetailTarget::Process(key()),
                tab,
            })
            .collect();
        Demand {
            any_surface_visible: true,
            details: details.into(),
            ..Demand::default()
        }
    }

    fn env_vars(p: &TabPayload) -> Vec<(String, String)> {
        match p {
            TabPayload::Environ(e) => e
                .vars
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            other => panic!("expected Environ, got {other:?}"),
        }
    }

    fn want_vars() -> Vec<(String, String)> {
        vec![
            ("HOME".into(), "/root".into()),
            ("SHELL".into(), "/bin/sh".into()),
        ]
    }

    /// AC 1: one `ReadProcessFiles { ENVIRON }`, then the tab is Valid with the helper's
    /// variables. No Restricted payload goes out while the read waits.
    #[test]
    fn detail_environment_through_helper() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.restrict("environ");
        let mut rig = Rig::start(&root, demand(&[DetailTab::Environment]), 1000, true);
        rig.run(Duration::from_millis(3_000));

        let reads = rig.reads();
        assert_eq!(reads.len(), 1, "{reads:?}");
        assert_eq!((reads[0].1, reads[0].2), (key(), ProcFiles::ENVIRON));
        let env = rig.tab(DetailTab::Environment);
        assert_eq!(env.len(), 1, "one payload, from the helper");
        assert_eq!(env[0].validity, Validity::Valid);
        assert_eq!(env_vars(&env[0].payload), want_vars());
        rig.finish();
    }

    /// AC 2, security regression: while `Locked` no helper read is sent, no session starts (no
    /// polkit prompt) and the tab stays Restricted.
    #[test]
    fn detail_locked_stays_restricted() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.restrict("environ");
        root.restrict("maps");
        let mut demand = demand(&[DetailTab::Environment, DetailTab::Maps]);
        demand.process_columns.insert(ColumnId::Pss);
        demand.detail_keys = Arc::from([key()]);
        root.restrict("smaps_rollup");
        let mut rig = Rig::start(&root, demand, 1000, false);
        rig.run(Duration::from_millis(3_000));

        assert_eq!(rig.helper.state(), HelperState::Locked);
        let fake = lock(&rig.fake);
        assert!(fake.reads.is_empty(), "{:?}", fake.reads);
        assert_eq!(fake.spawns, 0, "no session was started");
        drop(fake);
        for tab in [DetailTab::Environment, DetailTab::Maps] {
            let outs = rig.tab(tab);
            assert!(!outs.is_empty(), "{tab:?} sent");
            assert!(
                outs.iter().all(|t| t.validity == Validity::Restricted),
                "{tab:?} stays Restricted"
            );
        }
        assert!(!rig.pss.is_empty());
        assert!(
            rig.pss
                .iter()
                .all(|(_, e)| e.validity == Validity::Restricted)
        );
        rig.finish();
    }

    /// AC 3: Memory maps read every 250 ms for 5 s sends at most one helper read per second.
    #[test]
    fn detail_helper_reads_at_1hz() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.restrict("maps");
        let mut rig = Rig::start(&root, demand(&[DetailTab::Maps]), 250, true);
        rig.run(Duration::from_millis(4_950));

        let reads = rig.reads();
        assert!(
            reads
                .iter()
                .all(|r| r.1 == key() && r.2 == ProcFiles::MAPS | ProcFiles::SMAPS),
            "{reads:?}"
        );
        let times: Vec<u64> = reads.iter().map(|r| r.0).collect();
        assert_eq!(times, [0, 1_000, 2_000, 3_000, 4_000]);
        let maps = rig.tab(DetailTab::Maps);
        assert!(
            maps.iter().all(|t| t.validity == Validity::Valid),
            "the helper payload stays between reads"
        );
        let TabPayload::Maps(detail) = &*maps[0].payload else {
            panic!("expected Maps");
        };
        let starts: Vec<u64> = detail.regions.iter().map(|r| r.start).collect();
        assert_eq!(starts, [0x0040_0000, 0x7ffd_0000_0000]);
        let rss: Vec<u64> = detail.regions.iter().map(|r| r.rss_bytes.value).collect();
        assert_eq!(rss, [8 * 1024, 12 * 1024]);
        rig.finish();
    }

    /// AC 4: a target tasklens can read never goes through the helper.
    #[test]
    fn detail_own_process_no_helper() {
        let root = Root::new();
        let tabs = [
            DetailTab::General,
            DetailTab::Environment,
            DetailTab::Maps,
            DetailTab::Fds,
            DetailTab::Security,
            DetailTab::Threads,
        ];
        let mut demand = demand(&tabs);
        demand.process_columns.insert(ColumnId::Pss);
        demand.detail_keys = Arc::from([key()]);
        let mut rig = Rig::start(&root, demand, 1000, true);
        rig.run(Duration::from_millis(3_000));

        assert!(rig.reads().is_empty(), "{:?}", rig.reads());
        assert!(lock(&rig.fake).scans.is_empty());
        for tab in tabs {
            assert!(!rig.tab(tab).is_empty(), "{tab:?} sent");
        }
        rig.finish();
    }

    fn scan_state(out: &TabOut) -> (ScanId, ScanPhase, Vec<String>) {
        let TabPayload::Strings(state) = &*out.payload else {
            panic!("expected Strings");
        };
        let hits = state
            .chunks
            .iter()
            .flat_map(|c| c.iter().map(|h| h.value.to_string()))
            .collect();
        (state.scan, state.phase.clone(), hits)
    }

    /// A scan Yama refuses runs as a helper `ScanStrings` to its end.
    #[test]
    fn strings_through_helper_finished() {
        let root = Root::new();
        root.ptrace_scope_2();
        let mut rig = Rig::start(&root, demand(&[DetailTab::Strings]), 1000, true);
        let opts = StringScanOpts::default();

        // Commands wait for the next sample, at 1000 ms.
        rig.command(WorkerCommand::StringScan {
            scan: ScanId(1),
            key: key(),
            opts,
        });
        rig.run(Duration::from_millis(1_500));
        let strings = rig.tab(DetailTab::Strings);
        let last = scan_state(strings.last().expect("a Strings payload"));
        assert_eq!(
            last,
            (
                ScanId(1),
                ScanPhase::Finished,
                vec!["first".to_string(), "second".to_string()]
            )
        );
        assert!(strings.iter().all(|t| t.validity == Validity::Valid));
        let scans = lock(&rig.fake).scans.clone();
        assert_eq!(scans, [HelperRequest::ScanStrings { key: key(), opts }]);
        assert!(rig.reads().is_empty());
        rig.finish();
    }

    /// AC 5: a scan Yama refuses runs as a helper `ScanStrings`. `CancelStringScan` after the
    /// first batch, while the helper still scans, sends `CancelScan` with the scan frame's `seq`
    /// and ends the scan `Cancelled`. The helper call runs on a pool thread and the fake holds
    /// the rest of the scan until the cancel is in.
    #[test]
    fn strings_through_helper_cancel() {
        let root = Root::new();
        root.ptrace_scope_2();
        let mut rig = Rig::start_pooled(&root, demand(&[DetailTab::Strings]));
        let opts = StringScanOpts::default();
        rig.command(WorkerCommand::StringScan {
            scan: ScanId(1),
            key: key(),
            opts,
        });
        let first_batch = |rig: &Rig| {
            rig.tab(DetailTab::Strings).iter().any(|t| {
                let (scan, phase, hits) = scan_state(t);
                scan == ScanId(1) && phase == ScanPhase::Scanning && hits == ["first"]
            })
        };
        rig.run_until(first_batch);
        assert!(first_batch(&rig), "the first batch is shown");
        assert_eq!(lock(&rig.fake).held.len(), 2, "the helper still scans");

        rig.command(WorkerCommand::CancelStringScan(ScanId(1)));
        let cancelled = |rig: &Rig| {
            rig.tab(DetailTab::Strings)
                .last()
                .is_some_and(|t| scan_state(t).1 == ScanPhase::Cancelled)
        };
        rig.run_until(cancelled);
        assert!(cancelled(&rig), "the scan ends Cancelled");

        // The helper sends its next batch: it finds the stream dropped and cancels.
        release(&rig.fake);
        rig.wait_real(5_000, |rig| lock(&rig.fake).scans.len() == 2);
        rig.run_until(|rig| lock(&rig.fake).pending.is_empty());
        rig.run(Duration::from_millis(500));
        let (scans, seqs) = {
            let fake = lock(&rig.fake);
            (fake.scans.clone(), fake.scan_seqs.clone())
        };
        assert_eq!(
            scans,
            [
                HelperRequest::ScanStrings { key: key(), opts },
                HelperRequest::CancelScan { seq: seqs[0] },
            ]
        );
        assert_eq!(
            rig.helper.state(),
            HelperState::Unlocked,
            "the session stays"
        );
        let strings = rig.tab(DetailTab::Strings);
        let last = strings.last().expect("a Strings payload");
        assert_eq!(scan_state(last).1, ScanPhase::Cancelled);
        let cancel_ms = strings
            .iter()
            .find(|t| scan_state(t).1 == ScanPhase::Cancelled)
            .map(|t| t.ms);
        assert_eq!(cancel_ms, Some(last.ms), "nothing after Cancelled");
        assert!(
            strings
                .iter()
                .all(|t| !scan_state(t).2.contains(&"second".to_string())),
            "no batch after the cancel is shown"
        );
        assert!(rig.reads().is_empty());
        rig.finish();
    }

    /// qfx.18 review, security: a read queued behind a running helper scan for a tab that then
    /// closes is never sent.
    #[test]
    fn detail_closed_tab_read_not_sent() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.ptrace_scope_2();
        root.restrict("environ");
        let mut rig = Rig::start_pooled(&root, demand(&[DetailTab::Strings]));
        rig.command(WorkerCommand::StringScan {
            scan: ScanId(1),
            key: key(),
            opts: StringScanOpts::default(),
        });
        rig.run_until(|rig| {
            let fake = lock(&rig.fake);
            fake.held.len() == 2 && fake.pending.is_empty()
        });
        assert_eq!(lock(&rig.fake).scans.len(), 1, "the helper scans");

        // The Environment tab opens: its Restricted read waits behind the scan, so no payload
        // goes out. Then the tab closes.
        rig._demand
            .send(demand(&[DetailTab::Strings, DetailTab::Environment]))
            .expect("watch has receivers");
        rig.run(Duration::from_millis(1_000));
        assert!(rig.tab(DetailTab::Environment).is_empty(), "the read waits");
        rig._demand
            .send(demand(&[DetailTab::Strings]))
            .expect("watch has receivers");
        rig.run(Duration::from_millis(1_000));

        release(&rig.fake);
        let finished = |rig: &Rig| {
            rig.tab(DetailTab::Strings)
                .last()
                .is_some_and(|t| scan_state(t).1 == ScanPhase::Finished)
        };
        rig.run_until(finished);
        assert!(finished(&rig), "the scan ends");
        // A read sent now would run on the pool thread: give it time to show.
        rig.run(Duration::from_millis(1_000));
        rig.wait_real(200, |rig| !rig.reads().is_empty());
        assert!(rig.reads().is_empty(), "{:?}", rig.reads());
        rig.finish();
    }

    /// AC 6, R-170 item 13: after the Grant access button unlocks the helper, the UI's one
    /// `DetailRefresh` reads the Restricted tab through the helper.
    #[test]
    fn detail_refresh_after_unlock_reads_through_helper() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.restrict("environ");
        let mut rig = Rig::start(&root, demand(&[DetailTab::Environment]), 1000, false);
        rig.run(Duration::from_millis(1_000));
        let env = rig.tab(DetailTab::Environment);
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].validity, Validity::Restricted);

        rig.unlock();
        rig.run(Duration::from_millis(1_000));
        assert!(rig.reads().is_empty(), "no read before the refresh");

        rig.command(WorkerCommand::DetailRefresh {
            target: DetailTarget::Process(key()),
            tab: DetailTab::Environment,
        });
        rig.run(Duration::from_millis(2_000));
        let reads = rig.reads();
        assert_eq!(reads.len(), 1, "{reads:?}");
        assert_eq!(reads[0].2, ProcFiles::ENVIRON);
        let env = rig.tab(DetailTab::Environment);
        let last = env.last().expect("an Environment payload");
        assert_eq!(last.validity, Validity::Valid);
        assert_eq!(env_vars(&last.payload), want_vars());
        rig.finish();
    }

    /// qfx.18 review round 2: a General read whose `status` is Restricted asks the helper for
    /// `STATUS`, and the status fields (here the uids and No new privileges) fill from it.
    #[test]
    fn detail_general_status_through_helper() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.restrict("status");
        let mut rig = Rig::start(&root, demand(&[DetailTab::General]), 1000, true);
        rig.run(Duration::from_millis(2_050));

        let reads = rig.reads();
        assert!(!reads.is_empty());
        assert!(
            reads.iter().all(|r| r.2.contains(ProcFiles::STATUS)),
            "{reads:?}"
        );
        let general = rig.tab(DetailTab::General);
        let first = match &*general.first().expect("a General payload").payload {
            TabPayload::General(g) => g.uids.validity,
            other => panic!("expected General, got {other:?}"),
        };
        assert_eq!(first, Validity::Restricted, "the direct read");
        let TabPayload::General(g) = &*general.last().expect("a General payload").payload else {
            panic!("expected General");
        };
        assert_eq!(g.uids.validity, Validity::Valid);
        assert_eq!(g.uids.value.real, 0);
        assert_eq!(
            (g.no_new_privs.value, g.no_new_privs.validity),
            (true, Validity::Valid)
        );
        rig.finish();
    }

    /// AC 7: an `smaps` the helper cannot fit (EFBIG) fails the Memory maps tab with
    /// "too large to read through the helper".
    #[test]
    fn detail_too_large_failed() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.restrict("maps");
        let errors = vec![(ProcFiles::SMAPS, 27)];
        let mut rig = Rig::start_with(&root, demand(&[DetailTab::Maps]), 1000, true, errors);
        rig.run(Duration::from_millis(1_500));

        let maps = rig.tab(DetailTab::Maps);
        let last = maps.last().expect("a Maps payload");
        assert_eq!(last.validity, Validity::Failed);
        assert_eq!(
            rig.errors,
            [Arc::<str>::from("too large to read through the helper")]
        );
        rig.finish();
    }

    /// PSS and USS of a Restricted key come from the helper's `smaps_rollup`.
    #[test]
    fn detail_pss_through_helper() {
        if is_root() {
            return;
        }
        let root = Root::new();
        root.restrict("smaps_rollup");
        let mut demand = demand(&[]);
        demand.process_columns.insert(ColumnId::Pss);
        demand.detail_keys = Arc::from([key()]);
        let mut rig = Rig::start(&root, demand, 1000, true);
        rig.run(Duration::from_millis(2_050));

        let reads = rig.reads();
        assert!(reads.iter().all(|r| r.2 == ProcFiles::SMAPS_ROLLUP));
        assert_eq!(
            reads.iter().map(|r| r.0).collect::<Vec<_>>(),
            [0, 1_000, 2_000]
        );
        let seen: Vec<(u64, Validity, u64, u64)> = rig
            .pss
            .iter()
            .map(|(ms, e)| (*ms, e.validity, e.pss_bytes, e.uss_bytes))
            .collect();
        assert_eq!(
            seen,
            [
                // The first helper value comes with the next PSS round.
                (0, Validity::Restricted, 0, 0),
                (1_000, Validity::Valid, 300 * 1024, 100 * 1024),
                (2_000, Validity::Valid, 300 * 1024, 100 * 1024),
            ]
        );
        rig.finish();
    }
}
