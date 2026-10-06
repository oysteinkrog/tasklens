//! LabRuntime tests for the helper session task in `helper/mod.rs` (E29-A2, ADR-013).
//!
//! No test runs pkexec or anything as root: every test uses `FakeLink` through `LinkEnv`.

use std::collections::VecDeque;
use std::io;
use std::os::fd::BorrowedFd;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::lab::chaos::ChaosConfig;
use asupersync::runtime::TaskHandle;
use asupersync::time;
use asupersync::types::Budget;
use asupersync::{Cx, LabConfig, LabRuntime};
use tl_engine::helper::{
    HelperCallError, HelperHandle, HelperLink, HelperMsg, LinkEnv, LinkError, StartError,
    helper_queue, helper_task_with,
};
use tl_engine::update::{EngineMsg, HelperUpdate, SourceUpdate};
use tl_model::actions::Signal;
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::helper_proto::{
    HELPER_PROTO_VERSION, HelperFrame, HelperReply, HelperReplyBody, HelperRequest, HelperTarget,
};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;

/// How the fake answers one request after the `Hello`.
#[derive(Clone, Copy)]
enum Answer {
    /// This many replies, the last with `last` set.
    Replies(u32),
    /// No reply: the call runs into its deadline.
    Never,
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
}

type Shared = Arc<Mutex<FakeState>>;

fn lock(state: &Shared) -> std::sync::MutexGuard<'_, FakeState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

struct FakeLink(Shared);

impl HelperLink for FakeLink {
    fn send(&mut self, frame: &HelperFrame, _fd: Option<BorrowedFd<'_>>) -> io::Result<()> {
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
        if let Some(Answer::Replies(n)) = s.answers.pop_front() {
            for i in 1..=n {
                s.pending.push_back(HelperReply {
                    seq: frame.seq,
                    last: i == n,
                    body: HelperReplyBody::Ok,
                });
            }
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
        let state = Shared::default();
        let spawns = Arc::new(AtomicUsize::new(0));
        let (link_state, count) = (Arc::clone(&state), Arc::clone(&spawns));
        let env = LinkEnv {
            installed: Arc::new(move || installed),
            spawn: Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
                let link: Box<dyn HelperLink> = Box::new(FakeLink(Arc::clone(&link_state)));
                Ok::<_, StartError>(link)
            }),
        };
        (Fake { state, spawns }, env)
    }
}

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

fn update(state: HelperState, start: Option<(u64, HelperStartOutcome)>) -> HelperUpdate {
    HelperUpdate { state, start }
}

fn updates_only(seen: &[(u64, HelperUpdate)]) -> Vec<HelperUpdate> {
    seen.iter().map(|(_, u)| u.clone()).collect()
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
        seen.last().map(|(_, u)| u.clone()),
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
    assert_eq!(last, update(HelperState::Locked, None));
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
        seen.last().map(|(_, u)| u.clone()),
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
        ["send 0", "send 1", "reply 1 last", "send 2", "reply 2 last"]
    );
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
