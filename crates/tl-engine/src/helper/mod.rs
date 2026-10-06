//! The helper session task (E29-A2, ADR-006, R-246, R-247).
//!
//! `helper_task` owns at most one running helper. It starts one on `Start` or on the first
//! privileged call (one polkit prompt), never on a background `Read`. It ends it on `End` or at
//! shutdown, notices the helper's 300 s idle exit, and publishes the shield state as
//! `SourceUpdate::Helper`. Every privileged engine path calls the helper through
//! `HelperHandle`.
//!
//! The module is `pub` and `#[doc(hidden)]` only so `tests/lab_helper.rs` can drive it with a
//! fake link. Nothing outside tl-engine uses it.
//!
//! Privacy: requests and replies are never logged. Session start and end go to `info` on
//! target `tasklens::action`.

mod session;
mod setup;

use std::ops::ControlFlow;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use asupersync::Cx;
use asupersync::channel::{mpsc, oneshot};
use asupersync::runtime::JoinError;
use asupersync::time;
use tl_model::command::{SetupState, SetupStep};
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::helper_proto::{HelperFrame, HelperReply, HelperRequest};

pub use crate::actions::HelperCallError;
use crate::update::{EngineMsg, HelperUpdate, SourceUpdate};
use session::{CallError, HELPER_PATH, HelperSession, PKEXEC_PATH, ProcLink, START_DEADLINE};
pub use session::{HelperLink, LinkError, StartError};
pub use setup::{SetupProbe, SetupRunner};

/// Capacity of the helper queue.
const QUEUE: usize = 64;
/// Replies a `call_stream` caller may leave unread before the helper call waits.
const STREAM_QUEUE: usize = 64;
/// Extra time the task waits for a blocking job beyond the job's own deadline.
const JOIN_SLACK: Duration = Duration::from_secs(5);
/// How long the task waits for `end()` to finish.
const END_JOIN: Duration = Duration::from_secs(2);
/// Idle check period while `Unlocked`.
const IDLE_CHECK: Duration = Duration::from_secs(1);

/// One message on the helper queue.
pub enum HelperMsg {
    /// `Command::StartHelperSession` (R-148 item 1).
    Start,
    /// `Command::EndHelperSession`.
    End,
    /// `Command::SetupStatus` (R-247), answered by `setup::handle_setup_status`.
    SetupStatus,
    /// `Command::RunSetup { step }` (R-247), run by `setup::handle_run_setup`.
    RunSetup(SetupStep),
    /// One privileged request. Each reply goes to `replies`, then `done` gets the result.
    Call {
        req: HelperRequest,
        fd: Option<OwnedFd>,
        deadline: Duration,
        replies: async_channel::Sender<HelperReply>,
        done: oneshot::Sender<Result<(), HelperCallError>>,
    },
    /// One background read. It never starts a session: the task refuses it with `NotRunning`
    /// unless the helper is `Unlocked` when the task takes it (qfx.36).
    Read {
        req: HelperRequest,
        deadline: Duration,
        replies: async_channel::Sender<HelperReply>,
        done: oneshot::Sender<Result<(), HelperReadError>>,
    },
}

/// Why a background read (`HelperHandle::read`) did not return replies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperReadError {
    /// The helper was not `Unlocked` when the task took the read. Nothing was started and no
    /// polkit prompt was shown.
    NotRunning,
    /// The read reached the running helper and failed.
    Call(HelperCallError),
}

/// The sending side of the helper queue.
#[derive(Clone)]
pub struct HelperHandle {
    tx: mpsc::Sender<HelperMsg>,
    state: Arc<AtomicU8>,
}

/// The task's side of the helper queue, for `helper_task`.
pub struct HelperQueue {
    rx: mpsc::Receiver<HelperMsg>,
    state: Arc<AtomicU8>,
}

/// A new helper queue with capacity 64.
pub fn helper_queue() -> (HelperHandle, HelperQueue) {
    let (tx, rx) = mpsc::channel(QUEUE);
    let state = Arc::new(AtomicU8::new(state_code(HelperState::Missing)));
    let handle = HelperHandle {
        tx,
        state: Arc::clone(&state),
    };
    (handle, HelperQueue { rx, state })
}

impl HelperHandle {
    /// Queues `msg`. Waits while the queue is full.
    pub async fn send(&self, cx: &Cx, msg: HelperMsg) -> Result<(), HelperCallError> {
        self.tx.send(cx, msg).await.map_err(|e| match e {
            mpsc::SendError::Cancelled(_) => HelperCallError::Cancelled,
            mpsc::SendError::Disconnected(_) | mpsc::SendError::Full(_) => HelperCallError::Exited,
        })
    }

    /// Queues the request; the task starts the session first when `Locked` (one polkit prompt,
    /// 120 s). Returns every reply in order.
    pub async fn call(
        &self,
        cx: &Cx,
        req: HelperRequest,
        fd: Option<OwnedFd>,
        deadline: Duration,
    ) -> Result<Vec<HelperReply>, HelperCallError> {
        let (replies, rx) = async_channel::unbounded();
        let (done, mut done_rx) = oneshot::channel();
        self.send(
            cx,
            HelperMsg::Call {
                req,
                fd,
                deadline,
                replies,
                done,
            },
        )
        .await?;
        // The task sends every reply before `done`.
        match done_rx.recv(cx).await {
            Ok(Ok(())) => Ok(std::iter::from_fn(|| rx.try_recv().ok()).collect()),
            Ok(Err(e)) => Err(e),
            Err(oneshot::RecvError::Cancelled) => Err(HelperCallError::Cancelled),
            Err(_) => Err(HelperCallError::Exited),
        }
    }

    /// `call` for background readers: never starts a session, so it never shows a polkit
    /// prompt. The task checks the state when it takes the read, so a helper that exits after
    /// the caller's own state check gives `NotRunning`, not a start. Only a user action may
    /// prompt, so every worker read uses this.
    pub async fn read(
        &self,
        cx: &Cx,
        req: HelperRequest,
        deadline: Duration,
    ) -> Result<Vec<HelperReply>, HelperReadError> {
        let (replies, rx) = async_channel::unbounded();
        let (done, mut done_rx) = oneshot::channel();
        let msg = HelperMsg::Read {
            req,
            deadline,
            replies,
            done,
        };
        self.send(cx, msg).await.map_err(HelperReadError::Call)?;
        // The task sends every reply before `done`.
        match done_rx.recv(cx).await {
            Ok(Ok(())) => Ok(std::iter::from_fn(|| rx.try_recv().ok()).collect()),
            Ok(Err(e)) => Err(e),
            Err(oneshot::RecvError::Cancelled) => {
                Err(HelperReadError::Call(HelperCallError::Cancelled))
            }
            Err(_) => Err(HelperReadError::Call(HelperCallError::Exited)),
        }
    }

    /// Streaming form for `ScanStrings` and `NetnsSockDiagDump`. The final reply has `last`
    /// set; a stream that closes without it failed. Dropping the receiver stops the call (a
    /// scan gets `CancelScan`).
    pub async fn call_stream(
        &self,
        cx: &Cx,
        req: HelperRequest,
        deadline: Duration,
    ) -> Result<async_channel::Receiver<HelperReply>, HelperCallError> {
        if self.state() == HelperState::Missing {
            return Err(HelperCallError::Missing);
        }
        let (replies, rx) = async_channel::bounded(STREAM_QUEUE);
        // Nobody waits for the result: the stream's end tells it.
        let (done, _) = oneshot::channel();
        self.send(
            cx,
            HelperMsg::Call {
                req,
                fd: None,
                deadline,
                replies,
                done,
            },
        )
        .await?;
        Ok(rx)
    }

    /// The last published state.
    pub fn state(&self) -> HelperState {
        state_from(self.state.load(Ordering::Acquire))
    }
}

/// How the task checks for and starts the helper. `LinkEnv::real()` outside tests.
pub struct LinkEnv {
    /// True when `/usr/bin/pkexec` and `/usr/lib/tasklens/tl-helper` are regular files.
    pub installed: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Starts the helper process.
    pub spawn: Arc<dyn Fn() -> Result<Box<dyn HelperLink>, StartError> + Send + Sync>,
    /// Answers `SetupStatus` (E29-019).
    pub setup_probe: SetupProbe,
    /// Runs one `tasklens-setup` step (E29-019).
    pub setup_runner: SetupRunner,
}

impl LinkEnv {
    pub fn real() -> Self {
        LinkEnv {
            installed: Arc::new(|| {
                is_regular_file(Path::new(PKEXEC_PATH)) && is_regular_file(Path::new(HELPER_PATH))
            }),
            spawn: Arc::new(|| ProcLink::spawn().map(|l| Box::new(l) as Box<dyn HelperLink>)),
            setup_probe: setup::real_probe(),
            setup_runner: setup::real_runner(),
        }
    }
}

impl HelperLink for Box<dyn HelperLink> {
    fn send(&mut self, frame: &HelperFrame, fd: Option<BorrowedFd<'_>>) -> std::io::Result<()> {
        (**self).send(frame, fd)
    }

    fn recv(&mut self, timeout: Duration) -> Result<HelperReply, LinkError> {
        (**self).recv(timeout)
    }

    fn exit_status(&mut self, wait: Duration) -> Option<i32> {
        (**self).exit_status(wait)
    }

    fn kill(&mut self) {
        (**self).kill();
    }

    fn close(&mut self) {
        (**self).close();
    }
}

type Session = HelperSession<Box<dyn HelperLink>>;

/// Runs the helper session with the real helper until cancelled or the queue closes.
pub async fn helper_task(cx: Cx, queue: HelperQueue, updates: mpsc::Sender<EngineMsg>) {
    helper_task_with(cx, queue, updates, LinkEnv::real()).await;
}

/// `helper_task` with another `LinkEnv`, the test hook.
pub async fn helper_task_with(
    cx: Cx,
    queue: HelperQueue,
    updates: mpsc::Sender<EngineMsg>,
    env: LinkEnv,
) {
    let HelperQueue { mut rx, state } = queue;
    let mut task = Task {
        updates,
        env,
        shared: state,
        state: HelperState::Missing,
        live: Live(None),
        start_seq: 0,
        setup: SetupState::default(),
    };
    let first = if (task.env.installed)() {
        HelperState::Locked
    } else {
        HelperState::Missing
    };
    task.publish(&cx, first, None).await;
    loop {
        let msg = if task.state == HelperState::Unlocked {
            match time::timeout(cx.now(), IDLE_CHECK, rx.recv(&cx)).await {
                Ok(msg) => msg,
                Err(_) => {
                    task.idle_check(&cx).await;
                    continue;
                }
            }
        } else {
            rx.recv(&cx).await
        };
        // A closed queue or a cancel ends the task; the drop of `live` ends the session.
        let Ok(msg) = msg else { break };
        if cx.checkpoint().is_err() {
            break;
        }
        match msg {
            HelperMsg::Start => task.start_requested(&cx).await,
            HelperMsg::End => task.end(&cx).await,
            HelperMsg::SetupStatus => {
                let probe = &task.env.setup_probe;
                setup::handle_setup_status(&cx, probe, &mut task.setup, &task.updates).await;
            }
            HelperMsg::RunSetup(step) => {
                let (runner, probe) = (&task.env.setup_runner, &task.env.setup_probe);
                setup::handle_run_setup(&cx, step, runner, probe, &mut task.setup, &task.updates)
                    .await;
            }
            HelperMsg::Call {
                req,
                fd,
                deadline,
                replies,
                done,
            } => {
                let result = task.call(&cx, req, fd, deadline, replies).await;
                let _ = done.send(&cx, result);
            }
            HelperMsg::Read {
                req,
                deadline,
                replies,
                done,
            } => {
                let result = task.read(&cx, req, deadline, replies).await;
                let _ = done.send(&cx, result);
            }
        }
        if cx.checkpoint().is_err() {
            break;
        }
    }
    task.live.end();
}

/// The running helper. Dropping it ends the session, so a task that is dropped or cancelled
/// mid-await still closes the helper.
struct Live(Option<Session>);

impl Live {
    fn end(&mut self) {
        if let Some(session) = self.0.take() {
            session.end();
            tracing::info!(target: "tasklens::action", "helper session ended");
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.end();
    }
}

struct Task {
    updates: mpsc::Sender<EngineMsg>,
    env: LinkEnv,
    /// What `HelperHandle::state` reads.
    shared: Arc<AtomicU8>,
    state: HelperState,
    live: Live,
    /// The `seq` of the last published start outcome.
    start_seq: u64,
    /// The Additional Setup state, published as `SourceUpdate::Setup` on every change.
    setup: SetupState,
}

impl Task {
    async fn publish(&mut self, cx: &Cx, state: HelperState, start: Option<HelperStartOutcome>) {
        self.state = state;
        self.shared.store(state_code(state), Ordering::Release);
        let start = start.map(|outcome| {
            self.start_seq += 1;
            (self.start_seq, outcome)
        });
        let update = SourceUpdate::Helper(HelperUpdate { state, start });
        let _ = self.updates.send(cx, EngineMsg::Source(update)).await;
    }

    /// `HelperMsg::Start`: a failed start publishes its outcome.
    async fn start_requested(&mut self, cx: &Cx) {
        if let Err(e) = self.start(cx).await {
            let outcome = match e {
                StartError::Missing => return,
                StartError::Cancelled => HelperStartOutcome::Cancelled,
                StartError::NotAuthorized => HelperStartOutcome::NotAuthorized,
                StartError::Failed(reason) => HelperStartOutcome::Failed(reason),
            };
            self.publish(cx, HelperState::Locked, Some(outcome)).await;
        }
    }

    /// Starts the session when `Locked`. Publishes `Starting`, then `Unlocked` on success or
    /// `Missing` when the helper is not installed. Other failures leave `Starting` for the
    /// caller to replace.
    async fn start(&mut self, cx: &Cx) -> Result<(), StartError> {
        if !(self.env.installed)() {
            if self.state != HelperState::Missing {
                self.publish(cx, HelperState::Missing, None).await;
            }
            return Err(StartError::Missing);
        }
        match self.state {
            HelperState::Starting | HelperState::Unlocked => return Ok(()),
            HelperState::Missing | HelperState::Locked => {}
        }
        self.publish(cx, HelperState::Starting, None).await;
        let spawn = Arc::clone(&self.env.spawn);
        let started = run_blocking(cx, START_DEADLINE + JOIN_SLACK, move || {
            spawn().and_then(HelperSession::start)
        })
        .await;
        let result = match started {
            Ok(result) => result,
            Err(Lost::Timeout) => Err(StartError::Failed("no answer from the helper".into())),
            Err(Lost::Gone(reason)) => Err(StartError::Failed(reason)),
        };
        match result {
            Ok(session) => {
                self.live.0 = Some(session);
                tracing::info!(target: "tasklens::action", "helper session started");
                self.publish(cx, HelperState::Unlocked, None).await;
                Ok(())
            }
            Err(StartError::Missing) => {
                self.publish(cx, HelperState::Missing, None).await;
                Err(StartError::Missing)
            }
            Err(e) => Err(e),
        }
    }

    /// `HelperMsg::End`: no start outcome, so no toast.
    async fn end(&mut self, cx: &Cx) {
        let Some(session) = self.live.0.take() else {
            return;
        };
        let _ = run_blocking(cx, END_JOIN, move || session.end()).await;
        tracing::info!(target: "tasklens::action", "helper session ended");
        self.publish(cx, HelperState::Locked, None).await;
    }

    async fn call(
        &mut self,
        cx: &Cx,
        req: HelperRequest,
        fd: Option<OwnedFd>,
        deadline: Duration,
        replies: async_channel::Sender<HelperReply>,
    ) -> Result<(), HelperCallError> {
        if self.state == HelperState::Missing {
            return Err(HelperCallError::Missing);
        }
        if self.live.0.is_none() {
            // An implicit start publishes no outcome: the action's own toast reports it
            // (R-246).
            if let Err(e) = self.start(cx).await {
                let err = match e {
                    StartError::Missing => return Err(HelperCallError::Missing),
                    StartError::Cancelled => HelperCallError::Cancelled,
                    StartError::NotAuthorized => HelperCallError::NotAuthorized,
                    StartError::Failed(reason) => HelperCallError::StartFailed(reason),
                };
                self.publish(cx, HelperState::Locked, None).await;
                return Err(err);
            }
        }
        self.call_live(cx, req, fd, deadline, replies).await
    }

    /// `HelperMsg::Read`: the state check and the refusal happen here, in the task, so no
    /// state change can come between them. A helper that exited unnoticed fails the call with
    /// `Exited` and locks the shield; it is not started again.
    async fn read(
        &mut self,
        cx: &Cx,
        req: HelperRequest,
        deadline: Duration,
        replies: async_channel::Sender<HelperReply>,
    ) -> Result<(), HelperReadError> {
        if self.state != HelperState::Unlocked || self.live.0.is_none() {
            return Err(HelperReadError::NotRunning);
        }
        self.call_live(cx, req, None, deadline, replies)
            .await
            .map_err(HelperReadError::Call)
    }

    /// Runs one request on the live session. Never starts one.
    async fn call_live(
        &mut self,
        cx: &Cx,
        req: HelperRequest,
        fd: Option<OwnedFd>,
        deadline: Duration,
        replies: async_channel::Sender<HelperReply>,
    ) -> Result<(), HelperCallError> {
        let Some(mut session) = self.live.0.take() else {
            return Err(HelperCallError::Exited);
        };
        let joined = run_blocking(cx, deadline + JOIN_SLACK, move || {
            let fd = fd.as_ref().map(AsFd::as_fd);
            // A dropped receiver stops the delivery; a scan then gets `CancelScan`.
            let result = session.call(req, fd, deadline, &mut |reply| match replies
                .send_blocking(reply)
            {
                Ok(()) => ControlFlow::Continue(()),
                Err(_) => ControlFlow::Break(()),
            });
            (session, result)
        })
        .await;
        let err = match joined {
            Ok((session, Ok(()))) => {
                self.live.0 = Some(session);
                return Ok(());
            }
            // `call` killed the link on `Timeout` and `Protocol`; the drop reaps it.
            Ok((_, Err(CallError::Timeout))) | Err(Lost::Timeout) => HelperCallError::Timeout,
            Ok((_, Err(CallError::Exited))) => HelperCallError::Exited,
            Ok((_, Err(CallError::Protocol))) => {
                HelperCallError::Protocol("helper protocol error".into())
            }
            Err(Lost::Gone(reason)) => HelperCallError::Protocol(reason),
        };
        self.publish(cx, HelperState::Locked, None).await;
        Err(err)
    }

    /// While `Unlocked`: a helper that exited by itself (its 300 s idle exit) locks the shield
    /// with no toast (APP-003).
    async fn idle_check(&mut self, cx: &Cx) {
        let alive = self.live.0.as_mut().is_some_and(Session::is_alive);
        if !alive {
            self.live.0 = None;
            tracing::info!(target: "tasklens::action", "helper session ended");
            self.publish(cx, HelperState::Locked, None).await;
        }
    }
}

/// Why a blocking job gave no value.
enum Lost {
    /// It ran past its join deadline. It is not interrupted; its result is dropped.
    Timeout,
    /// It panicked or was cancelled.
    Gone(Arc<str>),
}

/// Runs `f` in `spawn_blocking` and waits at most `wait` for it (ASX section 6).
async fn run_blocking<T: Send + 'static>(
    cx: &Cx,
    wait: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Lost> {
    let mut job = cx
        .spawn_blocking(move |_| f())
        .map_err(|e| Lost::Gone(format!("could not start a job: {e}").into()))?;
    match time::timeout(cx.now(), wait, job.join(cx)).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(JoinError::Panicked(_))) => Err(Lost::Gone("helper job panicked".into())),
        Ok(Err(_)) => Err(Lost::Gone("helper job cancelled".into())),
        Err(_) => Err(Lost::Timeout),
    }
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

fn state_code(state: HelperState) -> u8 {
    match state {
        HelperState::Missing => 0,
        HelperState::Locked => 1,
        HelperState::Starting => 2,
        HelperState::Unlocked => 3,
    }
}

fn state_from(code: u8) -> HelperState {
    match code {
        1 => HelperState::Locked,
        2 => HelperState::Starting,
        3 => HelperState::Unlocked,
        _ => HelperState::Missing,
    }
}
