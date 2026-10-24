//! The journal worker (E14-A4, E14-009): one `journalctl` child per open `JournalHandle`, used
//! by the Service Properties Logs tab (ADR-005, ADR-016 "Service logs", SYS-015, R-167).
//!
//! `open` spawns one task per handle. The task sends `Access(..)` first, then one `Batch` with
//! the last `JOURNAL_PAGE` entries of the current boot, or one `Error(..)` in its place. The
//! fetch runs `journalctl` with `std::process::Command` inside one `Cx::spawn_blocking` job,
//! which polls the child's pipes against a 5 s deadline.
//!
//! After that the task handles control messages one at a time, in order:
//! - Follow mode keeps one `journalctl -f` child, read by one blocking job. New entries go out
//!   in batches at most every 100 ms (UI-SYS-037). Turning follow off kills and reaps the
//!   child; turning it on again resumes after the newest cursor, so no entry is lost or
//!   repeated (U-49).
//! - Load older runs one 5 s fetch of the `JOURNAL_PAGE` entries before the cursor the UI
//!   passes, or before the oldest entry sent when it passes none.
//!
//! Journal messages can hold secrets. Nothing here logs message text; logs carry only counts,
//! exit codes and durations (ADR-015).

use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use asupersync::Cx;
use asupersync::runtime::{RuntimeHandle, TaskHandle};
use asupersync::time;
use rustix::event::{PollFd, PollFlags, Timespec};
use serde_json::Value;
use tl_model::journal::{
    JOURNAL_PAGE, JournalAccess, JournalControl, JournalEntry, JournalError, JournalEvent,
    JournalHandle, JournalQuery,
};
use tl_model::table::Manager;

/// Capacity of the event channel (R-167).
const EVENTS_CAPACITY: usize = 64;
/// Capacity of the control channel. `JournalHandle` drops a control message when it is full.
const CONTROL_CAPACITY: usize = 8;
/// The initial fetch and each Load older fetch are killed after this long (ADR-016).
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// How often a fetch or the follow reader checks for a stop while it waits on the pipes.
const POLL_SLICE: Duration = Duration::from_millis(100);
/// A follow batch goes out this long after its first entry arrived (UI-SYS-037).
const BATCH_INTERVAL: Duration = Duration::from_millis(100);
/// How often the idle task checks for a dropped handle or a cancel.
const IDLE_SLICE: Duration = Duration::from_millis(250);
/// `stderr_first_line` is cut to this many characters.
const STDERR_LINE_MAX: usize = 200;
/// Only the start of stderr is kept; the first line is all that is reported.
const STDERR_KEEP: usize = 64 * 1024;
/// A missing or invalid `PRIORITY` is "info" (R-167).
const DEFAULT_PRIORITY: u8 = 6;
/// Groups whose members may read every entry of a system unit (SYS-015).
const READER_GROUPS: [&str; 3] = ["wheel", "adm", "systemd-journal"];

/// What the fetch reads from its surroundings. Tests replace each part.
#[derive(Clone, Debug)]
struct FetchEnv {
    /// `PATH` for the child. `None` inherits the engine's.
    path: Option<OsString>,
    group_file: PathBuf,
    /// Effective uid and the process's gids. `None` reads the process's own.
    ids: Option<(u32, Vec<u32>)>,
}

impl FetchEnv {
    fn system() -> Self {
        Self {
            path: None,
            group_file: PathBuf::from("/etc/group"),
            ids: None,
        }
    }
}

/// The cursors of one session, shared by the task and its blocking jobs.
#[derive(Default)]
struct Cursors {
    /// `__CURSOR` of the newest entry sent, for `--after-cursor`.
    newest: Option<Arc<str>>,
    /// `__CURSOR` of the oldest entry sent, for Load older.
    oldest: Option<Arc<str>>,
}

/// A job that panicked while holding the lock left whole cursors behind, so the poison is
/// ignored.
fn lock(cursors: &Mutex<Cursors>) -> MutexGuard<'_, Cursors> {
    cursors.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The running follow reader.
struct Follow {
    /// Set by the task to make the reader kill and reap its child and return.
    stop: Arc<AtomicBool>,
    /// Set by the reader before it reports a failed child, so a `set_follow(true)` right
    /// after the error starts a new child.
    ended: Arc<AtomicBool>,
    job: TaskHandle<()>,
}

/// What one journal task keeps between control messages.
struct Session {
    query: JournalQuery,
    env: FetchEnv,
    events: async_channel::Sender<JournalEvent>,
    cursors: Arc<Mutex<Cursors>>,
    follow: Option<Follow>,
    older: Option<Older>,
}

/// The running Load older fetch.
struct Older {
    /// Set by the job before it sends its event, so a `load_older()` right after the batch
    /// is not taken for a second call while one runs.
    done: Arc<AtomicBool>,
    job: TaskHandle<()>,
}

impl Session {
    /// Starts a follow reader unless one is running.
    async fn start_follow(&mut self, cx: &Cx) {
        if let Some(running) = &self.follow {
            if !running.ended.load(Ordering::Acquire) {
                return;
            }
            self.stop_follow(cx).await;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let (job_stop, job_ended) = (stop.clone(), ended.clone());
        let query = self.query.clone();
        let env = self.env.clone();
        let events = self.events.clone();
        let cursors = self.cursors.clone();
        match cx.spawn_blocking(move |job_cx| {
            let failure = follow(&job_cx, &query, &env, &events, &cursors, &job_stop);
            job_ended.store(true, Ordering::Release);
            if let Some(err) = failure {
                let _ = events.send_blocking(JournalEvent::Error(err));
            }
        }) {
            Ok(job) => self.follow = Some(Follow { stop, ended, job }),
            Err(e) => {
                tracing::warn!(target: "tasklens::journal", error = %e, "journal follow not admitted");
            }
        }
    }

    /// Stops the follow reader and waits until it has reaped its child, so no batch arrives
    /// afterwards.
    async fn stop_follow(&mut self, cx: &Cx) {
        if let Some(mut running) = self.follow.take() {
            running.stop.store(true, Ordering::Release);
            if let Err(e) = running.job.join(cx).await {
                tracing::debug!(target: "tasklens::journal", error = ?e, "journal follow ended early");
            }
        }
    }

    /// Starts a Load older fetch. Ignored while one is running.
    async fn load_older(&mut self, cx: &Cx, from: Option<Arc<str>>) {
        if let Some(running) = &mut self.older {
            if !running.done.load(Ordering::Acquire) {
                return;
            }
            if let Err(e) = running.job.join(cx).await {
                tracing::debug!(target: "tasklens::journal", error = ?e, "journal load older ended early");
            }
            self.older = None;
        }
        let done = Arc::new(AtomicBool::new(false));
        let job_done = done.clone();
        let query = self.query.clone();
        let env = self.env.clone();
        let events = self.events.clone();
        let cursors = self.cursors.clone();
        match cx.spawn_blocking(move |job_cx| {
            let event = load_older(&job_cx, &query, &env, &events, &cursors, from.as_ref());
            job_done.store(true, Ordering::Release);
            if let Some(event) = event {
                let _ = events.send_blocking(event);
            }
        }) {
            Ok(job) => self.older = Some(Older { done, job }),
            Err(e) => {
                tracing::warn!(target: "tasklens::journal", error = %e, "journal load older not admitted");
            }
        }
    }

    /// Forgets jobs that have ended on their own, so follow can start again after its child
    /// exited and Load older is accepted again.
    fn reap(&mut self) {
        if let Some(running) = &mut self.follow
            && running.job.is_finished()
        {
            let _ = running.job.try_join();
            self.follow = None;
        }
        if let Some(running) = &mut self.older
            && running.job.is_finished()
        {
            let _ = running.job.try_join();
            self.older = None;
        }
    }

    async fn shut_down(&mut self, cx: &Cx) {
        self.stop_follow(cx).await;
        if let Some(running) = self.older.take() {
            // The fetch sees the cancel or the closed channel and reaps its child.
            running.job.abort();
        }
    }
}

/// Opens one journal stream for `query` and returns the UI side of it.
pub(crate) fn open(rt: &RuntimeHandle, query: JournalQuery) -> JournalHandle {
    open_with(rt, query, FetchEnv::system())
}

fn open_with(rt: &RuntimeHandle, query: JournalQuery, env: FetchEnv) -> JournalHandle {
    let (events_tx, events_rx) = async_channel::bounded::<JournalEvent>(EVENTS_CAPACITY);
    let (control_tx, control_rx) = async_channel::bounded::<JournalControl>(CONTROL_CAPACITY);
    spawn_task(rt, query, env, events_tx, control_rx);
    JournalHandle::new(events_rx, control_tx)
}

fn spawn_task(
    rt: &RuntimeHandle,
    query: JournalQuery,
    env: FetchEnv,
    events: async_channel::Sender<JournalEvent>,
    control: async_channel::Receiver<JournalControl>,
) {
    let task_events = events.clone();
    let spawned = rt.try_spawn_checked(async move {
        let Some(cx) = Cx::current() else {
            tracing::warn!(target: "tasklens::journal", "journal task has no Cx");
            return;
        };
        run(cx, query, env, task_events, control).await;
    });
    if spawned.is_err() {
        // The runtime is gone. Report it the way `EngineHandle::journal` does after shutdown.
        tracing::warn!(target: "tasklens::journal", "journal task not started");
        let _ = events.try_send(JournalEvent::Error(JournalError::NotFound));
    }
}

/// The task that owns one session: the initial fetch, then control messages until the handle
/// is dropped.
async fn run(
    cx: Cx,
    query: JournalQuery,
    env: FetchEnv,
    events: async_channel::Sender<JournalEvent>,
    control: async_channel::Receiver<JournalControl>,
) {
    let fetch_query = query.clone();
    let fetch_env = env.clone();
    let fetch_events = events.clone();
    let mut job = match cx
        .spawn_blocking(move |job_cx| fetch(&job_cx, &fetch_query, &fetch_env, &fetch_events))
    {
        Ok(job) => job,
        Err(e) => {
            tracing::warn!(target: "tasklens::journal", error = %e, "journal fetch not admitted");
            return;
        }
    };
    let entries = match job.join(&cx).await {
        Ok(FetchOutcome::Entries(entries)) => entries,
        Ok(FetchOutcome::Failed(err)) => {
            let _ = events.try_send(JournalEvent::Error(err));
            return;
        }
        Ok(FetchOutcome::Stopped) => return,
        Err(e) => {
            tracing::debug!(target: "tasklens::journal", error = ?e, "journal fetch ended early");
            return;
        }
    };
    let cursors = Cursors {
        oldest: entries.first().map(|e| e.cursor.clone()),
        newest: entries.last().map(|e| e.cursor.clone()),
    };
    let exhausted = entries.len() < JOURNAL_PAGE;
    let batch = JournalEvent::Batch {
        entries: entries.into(),
        older: false,
        exhausted,
    };
    // A new channel holds at most `Access` before this, so only a closed channel fails.
    if events.try_send(batch).is_err() {
        return;
    }

    let mut session = Session {
        query,
        env,
        events,
        cursors: Arc::new(Mutex::new(cursors)),
        follow: None,
        older: None,
    };
    if session.query.follow {
        session.start_follow(&cx).await;
    }
    loop {
        let msg = time::timeout(cx.now(), IDLE_SLICE, control.recv()).await;
        session.reap();
        match msg {
            Ok(Ok(JournalControl::SetFollow(true))) => session.start_follow(&cx).await,
            Ok(Ok(JournalControl::SetFollow(false))) => session.stop_follow(&cx).await,
            Ok(Ok(JournalControl::LoadOlder(from))) => session.load_older(&cx, from).await,
            Ok(Err(_closed)) => break,
            Err(_elapsed) => {}
        }
        if cx.checkpoint().is_err() || session.events.is_closed() {
            break;
        }
    }
    session.shut_down(&cx).await;
}

/// What a blocking fetch returns.
#[derive(Debug)]
enum FetchOutcome {
    Entries(Vec<JournalEntry>),
    Failed(JournalError),
    /// The handle was dropped or the engine is shutting down. The child is reaped.
    Stopped,
}

/// The initial fetch. Blocking: sends `Access` at once, then runs `journalctl` with a 5 s
/// deadline.
fn fetch(
    cx: &Cx,
    query: &JournalQuery,
    env: &FetchEnv,
    events: &async_channel::Sender<JournalEvent>,
) -> FetchOutcome {
    let access = match query.unit.manager {
        // R-167 item 9: a user's own manager always shows every entry.
        Manager::User => JournalAccess::All,
        Manager::System => {
            let (euid, gids) = env.ids.clone().unwrap_or_else(process_ids);
            access_for(euid, &gids, &env.group_file)
        }
    };
    if events.try_send(JournalEvent::Access(access)).is_err() {
        return FetchOutcome::Stopped;
    }
    let args = ["-b", "-n", "200", "-o", "json", "--no-pager"];
    run_fetch(cx, query, &args, env, events)
}

/// Load older. Blocking: fetches the `JOURNAL_PAGE` entries before `from`, or before the oldest
/// entry sent when `from` is `None`. Returns the event to send: the batch in chronological
/// order, or the error.
fn load_older(
    cx: &Cx,
    query: &JournalQuery,
    env: &FetchEnv,
    events: &async_channel::Sender<JournalEvent>,
    cursors: &Mutex<Cursors>,
    from: Option<&Arc<str>>,
) -> Option<JournalEvent> {
    let oldest = from.cloned().or_else(|| lock(cursors).oldest.clone());
    let Some(oldest) = oldest else {
        return Some(JournalEvent::Batch {
            entries: Arc::from(Vec::new()),
            older: true,
            exhausted: true,
        });
    };
    // `--cursor` includes the anchor entry itself, so one more than a page is asked for and
    // that entry is dropped (R-167 item 8).
    let args = [
        "-b".to_owned(),
        "-o".to_owned(),
        "json".to_owned(),
        "--no-pager".to_owned(),
        "--reverse".to_owned(),
        format!("--cursor={oldest}"),
        "-n".to_owned(),
        (JOURNAL_PAGE + 1).to_string(),
    ];
    let mut entries = match run_fetch(cx, query, &args, env, events) {
        FetchOutcome::Entries(entries) => entries,
        FetchOutcome::Failed(err) => return Some(JournalEvent::Error(err)),
        FetchOutcome::Stopped => return None,
    };
    if entries.first().is_some_and(|e| e.cursor == oldest) {
        entries.remove(0);
    }
    entries.truncate(JOURNAL_PAGE);
    entries.reverse();
    let exhausted = entries.len() < JOURNAL_PAGE;
    if let Some(first) = entries.first() {
        lock(cursors).oldest = Some(first.cursor.clone());
    }
    Some(JournalEvent::Batch {
        entries: entries.into(),
        older: true,
        exhausted,
    })
}

/// Starts `journalctl <unit args> <args>` with piped stdout and stderr.
fn spawn_journalctl(
    query: &JournalQuery,
    args: &[impl AsRef<OsStr>],
    env: &FetchEnv,
) -> Result<Child, JournalError> {
    let unit_flag = match query.unit.manager {
        Manager::System => "-u",
        Manager::User => "--user-unit",
    };
    let mut cmd = Command::new("journalctl");
    cmd.arg(unit_flag)
        .arg(&*query.unit.name)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = &env.path {
        cmd.env("PATH", path);
    }
    match cmd.spawn() {
        Ok(child) => Ok(child),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(JournalError::NotFound),
        Err(e) => Err(io_failure(&e)),
    }
}

/// Runs one `journalctl` to the end with a 5 s deadline and parses its output.
fn run_fetch(
    cx: &Cx,
    query: &JournalQuery,
    args: &[impl AsRef<OsStr>],
    env: &FetchEnv,
    events: &async_channel::Sender<JournalEvent>,
) -> FetchOutcome {
    let begin = Instant::now();
    let child = match spawn_journalctl(query, args, env) {
        Ok(child) => child,
        Err(err) => return FetchOutcome::Failed(err),
    };
    let stop = || events.is_closed() || cx.is_cancel_requested();
    let out = match collect(child, begin + FETCH_TIMEOUT, stop) {
        Ok(out) => out,
        Err(CollectError::Timeout) => {
            tracing::debug!(target: "tasklens::journal", "journal fetch timed out");
            return FetchOutcome::Failed(JournalError::Timeout);
        }
        Err(CollectError::Stopped) => return FetchOutcome::Stopped,
        Err(CollectError::Io(e)) => return FetchOutcome::Failed(io_failure(&e)),
    };
    if !out.status.success() {
        return FetchOutcome::Failed(exit_failure(out.status, &out.stderr));
    }
    let (entries, skipped) = parse_lines(&out.stdout);
    tracing::debug!(
        target: "tasklens::journal",
        entries = entries.len(),
        skipped,
        elapsed_ms = begin.elapsed().as_millis() as u64,
        "journal fetch done",
    );
    FetchOutcome::Entries(entries)
}

/// `Exit` for a child that ended with a failure status.
fn exit_failure(status: ExitStatus, stderr: &[u8]) -> JournalError {
    // A child ended by a signal has no exit code; -1 stands for that.
    let code = status.code().unwrap_or(-1);
    tracing::debug!(target: "tasklens::journal", code, "journalctl failed");
    JournalError::Exit {
        code,
        stderr_first_line: first_line(stderr),
    }
}

/// The follow reader. Blocking: runs one `journalctl -f` child until it exits, `stop` is set,
/// the handle is dropped or the engine shuts down. Every path out of here reaps the child.
/// Returns the failure to report, if any.
fn follow(
    cx: &Cx,
    query: &JournalQuery,
    env: &FetchEnv,
    events: &async_channel::Sender<JournalEvent>,
    cursors: &Mutex<Cursors>,
    stop: &AtomicBool,
) -> Option<JournalError> {
    let after = lock(cursors).newest.clone();
    let mut args: Vec<String> = ["-b", "-o", "json", "--no-pager", "-f"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    match after {
        Some(cursor) => args.push(format!("--after-cursor={cursor}")),
        None => args.extend(["-n".to_owned(), "0".to_owned()]),
    }
    let mut child = match spawn_journalctl(query, &args, env) {
        Ok(child) => child,
        Err(err) => return Some(err),
    };
    let quit = || stop.load(Ordering::Acquire) || events.is_closed() || cx.is_cancel_requested();
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        kill_and_reap(&mut child);
        return Some(io_failure(&std::io::Error::other("child pipes missing")));
    };
    let stderr = match stream(stdout, stderr, events, cursors, &quit) {
        Streamed::Eof(stderr) => stderr,
        Streamed::Quit => {
            kill_and_reap(&mut child);
            return None;
        }
        Streamed::Io(e) => {
            kill_and_reap(&mut child);
            return Some(io_failure(&e));
        }
    };
    // Both pipes are at EOF, so the child is exiting.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_and_reap(&mut child);
                return Some(io_failure(&e));
            }
        }
        if quit() {
            kill_and_reap(&mut child);
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if status.success() {
        tracing::debug!(target: "tasklens::journal", "journal follow child exited");
        None
    } else {
        Some(exit_failure(status, &stderr))
    }
}

/// How `stream` ended.
enum Streamed {
    /// Both pipes reached EOF and every entry was sent. Holds the start of stderr.
    Eof(Vec<u8>),
    /// A stop, a dropped handle or a cancel. The caller kills the child.
    Quit,
    Io(std::io::Error),
}

/// Reads follow output line by line and sends it in batches until both pipes reach EOF or
/// `quit` returns true. `quit` is checked on every poll tick, at most 100 ms apart.
fn stream(
    mut stdout: ChildStdout,
    mut stderr: ChildStderr,
    events: &async_channel::Sender<JournalEvent>,
    cursors: &Mutex<Cursors>,
    quit: &impl Fn() -> bool,
) -> Streamed {
    let mut batcher = Batcher::default();
    let mut line = Vec::new();
    let mut err = Vec::new();
    let mut out_open = true;
    let mut err_open = true;
    let mut chunk = [0u8; 16 * 1024];
    while out_open || err_open {
        if quit() {
            return Streamed::Quit;
        }
        if batcher.is_due(Instant::now()) && !batcher.flush(events, cursors) {
            return Streamed::Quit;
        }
        let wait = batcher.wait(Instant::now());
        let timeout = Timespec {
            tv_sec: wait.as_secs() as i64,
            tv_nsec: i64::from(wait.subsec_nanos()),
        };
        let flags = PollFlags::IN | PollFlags::HUP;
        let mut fds = Vec::with_capacity(2);
        if out_open {
            fds.push(PollFd::new(&stdout, flags));
        }
        if err_open {
            fds.push(PollFd::new(&stderr, flags));
        }
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Streamed::Io(e.into()),
        }
        let ready: Vec<bool> = fds.iter().map(|fd| !fd.revents().is_empty()).collect();
        drop(fds);
        let mut slot = ready.into_iter();
        if out_open && slot.next() == Some(true) {
            match read_some(&mut stdout, &mut line, &mut chunk, usize::MAX) {
                Ok(open) => out_open = open,
                Err(CollectError::Io(e)) => return Streamed::Io(e),
                Err(_) => return Streamed::Quit,
            }
            // Only complete lines are parsed; the rest waits for its newline.
            if let Some(end) = line.iter().rposition(|&b| b == b'\n') {
                batcher.push(parse_lines(&line[..end]).0, Instant::now());
                line.drain(..=end);
            }
        }
        if err_open && slot.next() == Some(true) {
            match read_some(&mut stderr, &mut err, &mut chunk, STDERR_KEEP) {
                Ok(open) => err_open = open,
                Err(CollectError::Io(e)) => return Streamed::Io(e),
                Err(_) => return Streamed::Quit,
            }
        }
    }
    // At EOF the last line may lack its newline.
    batcher.push(parse_lines(&line).0, Instant::now());
    while batcher.due.is_some() {
        if quit() {
            return Streamed::Quit;
        }
        let now = Instant::now();
        if batcher.is_due(now) {
            if !batcher.flush(events, cursors) {
                return Streamed::Quit;
            }
        } else {
            std::thread::sleep(batcher.wait(now));
        }
    }
    Streamed::Eof(err)
}

/// Follow entries not sent yet. A batch goes out `BATCH_INTERVAL` after its first entry
/// arrived. The next batch's first entry arrives after that send, so batches are at least
/// `BATCH_INTERVAL` apart.
#[derive(Default)]
struct Batcher {
    pending: Vec<JournalEntry>,
    /// When the pending entries are due. `None` while nothing is pending.
    due: Option<Instant>,
}

impl Batcher {
    fn push(&mut self, entries: Vec<JournalEntry>, now: Instant) {
        if entries.is_empty() {
            return;
        }
        self.due.get_or_insert(now + BATCH_INTERVAL);
        self.pending.extend(entries);
    }

    fn is_due(&self, now: Instant) -> bool {
        self.due.is_some_and(|due| now >= due)
    }

    /// How long to wait for output before the next check.
    fn wait(&self, now: Instant) -> Duration {
        self.due.map_or(POLL_SLICE, |due| {
            due.saturating_duration_since(now).min(POLL_SLICE)
        })
    }

    /// Sends the pending entries and records their cursors. False when the handle is gone.
    fn flush(
        &mut self,
        events: &async_channel::Sender<JournalEvent>,
        cursors: &Mutex<Cursors>,
    ) -> bool {
        self.due = None;
        let entries = std::mem::take(&mut self.pending);
        let (Some(first), Some(last)) = (entries.first(), entries.last()) else {
            return true;
        };
        let (first, last) = (first.cursor.clone(), last.cursor.clone());
        let count = entries.len();
        let batch = JournalEvent::Batch {
            entries: entries.into(),
            older: false,
            exhausted: false,
        };
        if events.send_blocking(batch).is_err() {
            return false;
        }
        let mut cursors = lock(cursors);
        cursors.newest = Some(last);
        cursors.oldest.get_or_insert(first);
        drop(cursors);
        tracing::trace!(
            target: "tasklens::journal",
            entries = count,
            "journal follow batch"
        );
        true
    }
}

/// An I/O error other than "not found" while starting or reading the child.
fn io_failure(e: &std::io::Error) -> JournalError {
    JournalError::Exit {
        code: -1,
        stderr_first_line: cut(&e.to_string()),
    }
}

/// What a finished child left behind.
pub(super) struct Collected {
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

pub(super) enum CollectError {
    Timeout,
    Stopped,
    Io(std::io::Error),
}

/// Reads stdout and stderr of `child` to the end and reaps it. Past `deadline`, or when `stop`
/// returns true, the child is killed and reaped. Every path out of here reaps the child.
pub(super) fn collect(
    mut child: Child,
    deadline: Instant,
    stop: impl Fn() -> bool,
) -> Result<Collected, CollectError> {
    let result = drain_pipes(&mut child, deadline, &stop);
    let (stdout, stderr) = match result {
        Ok(bufs) => bufs,
        Err(e) => {
            kill_and_reap(&mut child);
            return Err(e);
        }
    };
    // Both pipes are at EOF, so the child is exiting or has handed its pipes away.
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(Collected {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {}
            Err(e) => {
                kill_and_reap(&mut child);
                return Err(CollectError::Io(e));
            }
        }
        if stop() {
            kill_and_reap(&mut child);
            return Err(CollectError::Stopped);
        }
        if Instant::now() >= deadline {
            kill_and_reap(&mut child);
            return Err(CollectError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn kill_and_reap(child: &mut Child) {
    // `kill` fails only when the child has already exited; `wait` reaps it either way.
    let _ = child.kill();
    let _ = child.wait();
}

/// Polls both pipes until each reaches EOF.
fn drain_pipes(
    child: &mut Child,
    deadline: Instant,
    stop: &impl Fn() -> bool,
) -> Result<(Vec<u8>, Vec<u8>), CollectError> {
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(CollectError::Io(std::io::Error::other(
            "child pipes missing",
        )));
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut out_open = true;
    let mut err_open = true;
    let mut chunk = [0u8; 16 * 1024];
    while out_open || err_open {
        if stop() {
            return Err(CollectError::Stopped);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(CollectError::Timeout);
        }
        let wait = (deadline - now).min(POLL_SLICE);
        let timeout = Timespec {
            tv_sec: wait.as_secs() as i64,
            tv_nsec: i64::from(wait.subsec_nanos()),
        };
        let flags = PollFlags::IN | PollFlags::HUP;
        let mut fds = Vec::with_capacity(2);
        if out_open {
            fds.push(PollFd::new(&stdout, flags));
        }
        if err_open {
            fds.push(PollFd::new(&stderr, flags));
        }
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(CollectError::Io(e.into())),
        }
        let ready: Vec<bool> = fds.iter().map(|fd| !fd.revents().is_empty()).collect();
        drop(fds);
        let mut slot = ready.into_iter();
        if out_open && slot.next() == Some(true) {
            out_open = read_some(&mut stdout, &mut out, &mut chunk, usize::MAX)?;
        }
        if err_open && slot.next() == Some(true) {
            err_open = read_some(&mut stderr, &mut err, &mut chunk, STDERR_KEEP)?;
        }
    }
    Ok((out, err))
}

/// One read from a ready pipe. Keeps at most `keep` bytes in `buf`. Returns false at EOF.
fn read_some(
    pipe: &mut impl Read,
    buf: &mut Vec<u8>,
    chunk: &mut [u8],
    keep: usize,
) -> Result<bool, CollectError> {
    match pipe.read(chunk) {
        Ok(0) => Ok(false),
        Ok(n) => {
            let room = keep.saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..n.min(room)]);
            Ok(true)
        }
        Err(e) if e.kind() == ErrorKind::Interrupted => Ok(true),
        Err(e) => Err(CollectError::Io(e)),
    }
}

/// The first stderr line, lossy UTF-8, at most `STDERR_LINE_MAX` characters.
fn first_line(stderr: &[u8]) -> Arc<str> {
    let text = String::from_utf8_lossy(stderr);
    cut(text.lines().next().unwrap_or(""))
}

fn cut(line: &str) -> Arc<str> {
    match line.char_indices().nth(STDERR_LINE_MAX) {
        Some((end, _)) => Arc::from(&line[..end]),
        None => Arc::from(line),
    }
}

/// Parses `journalctl -o json` output: one JSON object per line. Returns the entries and the
/// number of lines skipped because they are not JSON objects or lack `__CURSOR`.
fn parse_lines(stdout: &[u8]) -> (Vec<JournalEntry>, usize) {
    let mut entries = Vec::new();
    let mut skipped = 0;
    for line in stdout.split(|&b| b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match parse_entry(line) {
            Some(entry) => entries.push(entry),
            None => skipped += 1,
        }
    }
    (entries, skipped)
}

fn parse_entry(line: &[u8]) -> Option<JournalEntry> {
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(line) else {
        return None;
    };
    let cursor = map.get("__CURSOR")?.as_str()?;
    let decimal = |key: &str| map.get(key).and_then(Value::as_str).map(str::parse::<u64>);
    let realtime_us = decimal("__REALTIME_TIMESTAMP")
        .and_then(Result::ok)
        .unwrap_or(0);
    let priority = map
        .get("PRIORITY")
        .and_then(Value::as_str)
        .and_then(|p| p.parse::<u8>().ok())
        .filter(|p| *p <= 7)
        .unwrap_or(DEFAULT_PRIORITY);
    let pid = map
        .get("_PID")
        .and_then(Value::as_str)
        .and_then(|p| p.parse::<u32>().ok());
    Some(JournalEntry {
        realtime_us,
        priority,
        pid,
        message: message_text(map.get("MESSAGE")),
        cursor: Arc::from(cursor),
    })
}

/// `MESSAGE` is a string, or an array of bytes when it is not valid UTF-8. Anything else is
/// an empty message.
pub(super) fn message_text(value: Option<&Value>) -> Arc<str> {
    match value {
        Some(Value::String(s)) => Arc::from(s.as_str()),
        Some(Value::Array(items)) => {
            let bytes: Vec<u8> = items
                .iter()
                .filter_map(|v| v.as_u64().and_then(|b| u8::try_from(b).ok()))
                .collect();
            Arc::from(String::from_utf8_lossy(&bytes))
        }
        _ => Arc::from(""),
    }
}

/// The process's effective uid and every gid it holds: `getgroups()` plus the real and
/// effective primary gid.
pub(super) fn process_ids() -> (u32, Vec<u32>) {
    let mut gids: Vec<u32> = rustix::process::getgroups()
        .unwrap_or_default()
        .into_iter()
        .map(|g| g.as_raw())
        .collect();
    gids.push(rustix::process::getgid().as_raw());
    gids.push(rustix::process::getegid().as_raw());
    (rustix::process::geteuid().as_raw(), gids)
}

/// SYS-015 for a system unit: `All` for root or a member of `wheel`, `adm` or
/// `systemd-journal` (gids looked up in `group_file`), else `OwnOnly`. An unreadable group
/// file proves no membership.
pub(super) fn access_for(euid: u32, gids: &[u32], group_file: &Path) -> JournalAccess {
    if euid == 0 {
        return JournalAccess::All;
    }
    let text = std::fs::read_to_string(group_file).unwrap_or_default();
    let member = text.lines().any(|line| {
        let mut fields = line.split(':');
        let (Some(name), Some(_pw), Some(gid)) = (fields.next(), fields.next(), fields.next())
        else {
            return false;
        };
        READER_GROUPS.contains(&name) && gid.parse::<u32>().is_ok_and(|g| gids.contains(&g))
    });
    if member {
        JournalAccess::All
    } else {
        JournalAccess::OwnOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tl_model::table::UnitRef;

    const FIXTURE: &str = concat!(
        r#"{"__CURSOR":"s=a;i=1","__REALTIME_TIMESTAMP":"1700000000000001","PRIORITY":"3","_PID":"42","MESSAGE":"first"}"#,
        "\n",
        r#"{"__CURSOR":"s=a;i=2","__REALTIME_TIMESTAMP":"1700000000000002","PRIORITY":"7","MESSAGE":"second"}"#,
        "\n",
        "{not json\n",
        r#"{"__CURSOR":"s=a;i=3","__REALTIME_TIMESTAMP":"1700000000000003","PRIORITY":"0","_PID":"7","MESSAGE":"third"}"#,
        "\n",
    );

    fn query(manager: Manager) -> JournalQuery {
        JournalQuery {
            unit: UnitRef {
                manager,
                name: Arc::from("demo.service"),
            },
            follow: false,
        }
    }

    /// A fresh directory under the target dir for one test.
    fn scratch(name: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("journal-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// A directory holding a fake `journalctl` with `body` as its shell script.
    fn fake_journalctl(name: &str, body: &str) -> PathBuf {
        let dir = scratch(name);
        let path = dir.join("journalctl");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        dir
    }

    /// `dir` first on `PATH`, then the system directories for `sleep` and friends.
    fn env_with(dir: &Path, ids: Option<(u32, Vec<u32>)>) -> FetchEnv {
        let mut path = OsString::from(dir);
        path.push(":/usr/bin:/bin");
        FetchEnv {
            path: Some(path),
            group_file: PathBuf::from("/etc/group"),
            ids,
        }
    }

    /// Opens a stream on a real runtime and collects events until the stream ends or the
    /// limit passes.
    fn run_open(q: JournalQuery, env: FetchEnv, limit: Duration) -> (Vec<JournalEvent>, Duration) {
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), q, env);
        let events = handle.events();
        let begin = Instant::now();
        let mut got = Vec::new();
        while begin.elapsed() < limit {
            match events.try_recv() {
                Ok(ev) => {
                    let terminal =
                        matches!(ev, JournalEvent::Error(_) | JournalEvent::Batch { .. });
                    got.push(ev);
                    if terminal {
                        break;
                    }
                }
                Err(async_channel::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(async_channel::TryRecvError::Closed) => break,
            }
        }
        (got, begin.elapsed())
    }

    /// True once `/proc/<pid>` is gone, so the process is dead and reaped.
    fn reaped_within(pid: u32, limit: Duration) -> bool {
        let begin = Instant::now();
        while begin.elapsed() < limit {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn read_pid(file: &Path, limit: Duration) -> u32 {
        let begin = Instant::now();
        while begin.elapsed() < limit {
            if let Ok(text) = std::fs::read_to_string(file)
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("fake journalctl did not write its pid");
    }

    #[test]
    fn parse_journal_json_lines() {
        let (entries, skipped) = parse_lines(FIXTURE.as_bytes());
        assert_eq!(skipped, 1);
        assert_eq!(entries.len(), 3);
        let e = &entries[0];
        assert_eq!(e.realtime_us, 1_700_000_000_000_001);
        assert_eq!(e.priority, 3);
        assert_eq!(e.pid, Some(42));
        assert_eq!(&*e.message, "first");
        assert_eq!(&*e.cursor, "s=a;i=1");
        assert_eq!(entries[1].pid, None);
        assert_eq!(entries[1].priority, 7);
        assert_eq!(&*entries[2].cursor, "s=a;i=3");
        assert_eq!(entries[2].priority, 0);
        assert_eq!(entries[2].pid, Some(7));
        // A JSON object without `__CURSOR` is skipped too.
        let (none, skipped) = parse_lines(br#"{"MESSAGE":"x"}"#);
        assert!(none.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn parse_message_byte_array() {
        let (entries, _) = parse_lines(br#"{"__CURSOR":"c","MESSAGE":[104,105]}"#);
        assert_eq!(&*entries[0].message, "hi");
        let (entries, _) = parse_lines(br#"{"__CURSOR":"c","MESSAGE":null}"#);
        assert_eq!(&*entries[0].message, "");
        let (entries, _) = parse_lines(br#"{"__CURSOR":"c"}"#);
        assert_eq!(&*entries[0].message, "");
    }

    #[test]
    fn parse_missing_priority_default() {
        let (entries, _) = parse_lines(br#"{"__CURSOR":"c","MESSAGE":"m"}"#);
        assert_eq!(entries[0].priority, 6);
        let (entries, _) = parse_lines(br#"{"__CURSOR":"c","PRIORITY":"9"}"#);
        assert_eq!(entries[0].priority, 6);
        let (entries, _) = parse_lines(br#"{"__CURSOR":"c","PRIORITY":"x"}"#);
        assert_eq!(entries[0].priority, 6);
    }

    fn batch_of(count: usize) -> (Vec<JournalEvent>, Vec<String>) {
        let dir = fake_journalctl(
            &format!("batch{count}"),
            &format!(
                "printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/args\"\n\
                 i=0\n\
                 while [ $i -lt {count} ]; do\n\
                 printf '{{\"__CURSOR\":\"c%d\",\"MESSAGE\":\"m\"}}\\n' $i\n\
                 i=$((i+1))\n\
                 done"
            ),
        );
        let (events, _) = run_open(
            query(Manager::User),
            env_with(&dir, None),
            Duration::from_secs(5),
        );
        let args = std::fs::read_to_string(dir.join("args")).expect("args file");
        (events, args.lines().map(str::to_owned).collect())
    }

    #[test]
    fn batch_exhausted_flag() {
        let (events, args) = batch_of(150);
        assert_eq!(
            args,
            [
                "--user-unit",
                "demo.service",
                "-b",
                "-n",
                "200",
                "-o",
                "json",
                "--no-pager"
            ]
        );
        assert!(matches!(
            events[0],
            JournalEvent::Access(JournalAccess::All)
        ));
        match &events[1] {
            JournalEvent::Batch {
                entries,
                older,
                exhausted,
            } => {
                assert_eq!(entries.len(), 150);
                assert!(!older);
                assert!(exhausted);
                assert_eq!(&*entries[0].cursor, "c0");
            }
            other => panic!("expected a batch, got {other:?}"),
        }
        let (events, _) = batch_of(200);
        match &events[1] {
            JournalEvent::Batch {
                entries, exhausted, ..
            } => {
                assert_eq!(entries.len(), 200);
                assert!(!exhausted);
            }
            other => panic!("expected a batch, got {other:?}"),
        }
    }

    #[test]
    fn fetch_not_found() {
        let empty = scratch("empty");
        let env = FetchEnv {
            path: Some(empty.into_os_string()),
            group_file: PathBuf::from("/etc/group"),
            ids: None,
        };
        let (events, _) = run_open(query(Manager::User), env, Duration::from_secs(5));
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(matches!(events[0], JournalEvent::Access(_)));
        assert!(matches!(
            events[1],
            JournalEvent::Error(JournalError::NotFound)
        ));
    }

    #[test]
    fn fetch_timeout_kills_child() {
        let dir = fake_journalctl(
            "timeout",
            "echo $$ > \"$(dirname \"$0\")/pid\"\nexec sleep 10",
        );
        let (events, took) = run_open(
            query(Manager::User),
            env_with(&dir, None),
            Duration::from_secs(8),
        );
        assert!(
            matches!(
                events.last(),
                Some(JournalEvent::Error(JournalError::Timeout))
            ),
            "{events:?}"
        );
        assert!(took < Duration::from_secs(6), "timeout took {took:?}");
        let pid = read_pid(&dir.join("pid"), Duration::from_secs(1));
        assert!(
            reaped_within(pid, Duration::from_secs(1)),
            "child {pid} not reaped"
        );
    }

    #[test]
    fn fetch_exit_error_first_line() {
        let dir = fake_journalctl("exit", "echo boom >&2\necho second >&2\nexit 1");
        let (events, _) = run_open(
            query(Manager::User),
            env_with(&dir, None),
            Duration::from_secs(5),
        );
        assert_eq!(
            events.last().cloned().map(|e| match e {
                JournalEvent::Error(err) => Some(err),
                _ => None,
            }),
            Some(Some(JournalError::Exit {
                code: 1,
                stderr_first_line: Arc::from("boom"),
            }))
        );
        assert_eq!(&*cut(&"x".repeat(300)), "x".repeat(200));
    }

    #[test]
    fn access_own_only_without_groups() {
        let dir = scratch("groups");
        let group = dir.join("group");
        std::fs::write(
            &group,
            "root:x:0:\nwheel:x:998:alice\nadm:x:4:\nsystemd-journal:x:190:\nusers:x:1000:\n",
        )
        .expect("group file");
        assert_eq!(access_for(1000, &[1000], &group), JournalAccess::OwnOnly);
        assert_eq!(access_for(1000, &[1000, 998], &group), JournalAccess::All);
        assert_eq!(access_for(1000, &[190], &group), JournalAccess::All);
        assert_eq!(access_for(0, &[], &group), JournalAccess::All);
        assert_eq!(
            access_for(1000, &[998], &dir.join("missing")),
            JournalAccess::OwnOnly
        );

        // Opening a system unit sends `Access(OwnOnly)` first.
        let fake = fake_journalctl("ownonly", "exit 0");
        let env = FetchEnv {
            group_file: group,
            ..env_with(&fake, Some((1000, vec![1000])))
        };
        let (events, _) = run_open(query(Manager::System), env, Duration::from_secs(5));
        assert!(
            matches!(events[0], JournalEvent::Access(JournalAccess::OwnOnly)),
            "{events:?}"
        );
        assert!(matches!(events[1], JournalEvent::Batch { .. }));
    }

    #[test]
    fn drop_handle_kills_child() {
        let dir = fake_journalctl("drop", "echo $$ > \"$(dirname \"$0\")/pid\"\nexec sleep 10");
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), query(Manager::User), env_with(&dir, None));
        let pid = read_pid(&dir.join("pid"), Duration::from_secs(3));
        drop(handle);
        assert!(
            reaped_within(pid, Duration::from_secs(2)),
            "child {pid} not reaped"
        );
    }

    /// One `printf` per cursor, each writing one JSON line.
    fn emit(cursors: impl IntoIterator<Item = String>) -> String {
        cursors
            .into_iter()
            .map(|c| format!("printf '%s\\n' '{{\"__CURSOR\":\"{c}\",\"MESSAGE\":\"m\"}}'\n"))
            .collect()
    }

    /// A fake `journalctl` that appends its argv to `calls`, then runs `follow` for `-f`,
    /// `reverse` for `--reverse` and `initial` for anything else. `$d` is its directory.
    fn modes(name: &str, initial: &str, follow: &str, reverse: &str) -> PathBuf {
        fake_journalctl(
            name,
            &format!(
                "d=\"$(dirname \"$0\")\"\n\
                 echo \"$*\" >> \"$d/calls\"\n\
                 case \" $* \" in\n\
                 *' -f '*)\n{follow}\n;;\n\
                 *' --reverse '*)\n{reverse}\n;;\n\
                 *)\n{initial}\n;;\n\
                 esac"
            ),
        )
    }

    /// A follow body: records its pid in `fpid`, runs `body`, then waits to be killed.
    fn held(body: &str) -> String {
        format!("echo $$ > \"$d/fpid\"\n{body}\nexec sleep 10")
    }

    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn wait_calls(dir: &Path, count: usize) -> Vec<String> {
        let begin = Instant::now();
        while begin.elapsed() < Duration::from_secs(3) {
            let got = calls(dir);
            if got.len() >= count {
                return got;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "fake journalctl ran fewer than {count} times: {:?}",
            calls(dir)
        );
    }

    fn user_query(follow: bool) -> JournalQuery {
        JournalQuery {
            follow,
            ..query(Manager::User)
        }
    }

    /// The next event within `limit`.
    fn next_event(
        events: &async_channel::Receiver<JournalEvent>,
        limit: Duration,
    ) -> Option<JournalEvent> {
        let begin = Instant::now();
        loop {
            match events.try_recv() {
                Ok(ev) => return Some(ev),
                Err(async_channel::TryRecvError::Empty) if begin.elapsed() < limit => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return None,
            }
        }
    }

    /// Every event that arrives within `span`.
    fn events_for(
        events: &async_channel::Receiver<JournalEvent>,
        span: Duration,
    ) -> Vec<JournalEvent> {
        let begin = Instant::now();
        let mut got = Vec::new();
        while let Some(ev) = next_event(events, span.saturating_sub(begin.elapsed())) {
            got.push(ev);
        }
        got
    }

    /// Takes `Access` and the initial batch off the channel.
    fn skip_initial(events: &async_channel::Receiver<JournalEvent>) {
        for _ in 0..2 {
            let ev = next_event(events, Duration::from_secs(5)).expect("initial events");
            assert!(
                matches!(
                    ev,
                    JournalEvent::Access(_) | JournalEvent::Batch { older: false, .. }
                ),
                "{ev:?}"
            );
        }
    }

    fn cursors_of(entries: &[JournalEntry]) -> Vec<String> {
        entries.iter().map(|e| e.cursor.to_string()).collect()
    }

    /// The cursors of each follow batch in `events`.
    fn follow_batches(events: &[JournalEvent]) -> Vec<Vec<String>> {
        events
            .iter()
            .filter_map(|ev| match ev {
                JournalEvent::Batch {
                    entries,
                    older: false,
                    exhausted: false,
                } => Some(cursors_of(entries)),
                _ => None,
            })
            .collect()
    }

    fn older_batch(events: &async_channel::Receiver<JournalEvent>) -> (Vec<String>, bool) {
        match next_event(events, Duration::from_secs(5)) {
            Some(JournalEvent::Batch {
                entries,
                older: true,
                exhausted,
            }) => (cursors_of(&entries), exhausted),
            other => panic!("expected an older batch, got {other:?}"),
        }
    }

    fn numbered(prefix: &str, range: impl Iterator<Item = usize>) -> Vec<String> {
        range.map(|i| format!("{prefix}{i}")).collect()
    }

    /// All `cursors` in one `printf`: one write of under PIPE_BUF bytes, so the reader gets them
    /// in one read whatever the machine load.
    fn emit_at_once(cursors: impl IntoIterator<Item = String>) -> String {
        let lines: String = cursors
            .into_iter()
            .map(|c| format!("{{\"__CURSOR\":\"{c}\",\"MESSAGE\":\"m\"}}\\n"))
            .collect();
        format!("printf '{lines}'\n")
    }

    /// Follow events until `want` follow entries arrived or `limit` passed, then `quiet` more, so
    /// a repeated or extra batch would show.
    fn follow_until(
        events: &async_channel::Receiver<JournalEvent>,
        want: usize,
        limit: Duration,
        quiet: Duration,
    ) -> Vec<Vec<String>> {
        let begin = Instant::now();
        let mut got = Vec::new();
        while got.iter().map(Vec::len).sum::<usize>() < want {
            let Some(ev) = next_event(events, limit.saturating_sub(begin.elapsed())) else {
                break;
            };
            got.extend(follow_batches(std::slice::from_ref(&ev)));
        }
        got.extend(follow_batches(&events_for(events, quiet)));
        got
    }

    #[test]
    fn follow_batches_at_most_every_100ms() {
        let burst = emit_at_once(numbered("f", 0..50));
        let dir = modes("burst", "exit 0", &held(&burst), "exit 0");
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), user_query(true), env_with(&dir, None));
        let events = handle.events();
        skip_initial(&events);
        let batches = follow_batches(&events_for(&events, Duration::from_millis(600)));
        assert_eq!(batches, [numbered("f", 0..50)]);
        // With nothing loaded, follow starts at the end of the journal.
        assert_eq!(
            calls(&dir)[1],
            "--user-unit demo.service -b -o json --no-pager -f -n 0"
        );
        let pid = read_pid(&dir.join("fpid"), Duration::from_secs(1));
        drop(handle);
        drop(events);
        assert!(
            reaped_within(pid, Duration::from_secs(2)),
            "child {pid} not reaped"
        );
    }

    #[test]
    fn follow_every_line_once() {
        let slow: String = (0..50)
            .map(|i| format!("{}sleep 0.02\n", emit([format!("f{i}")])))
            .collect();
        let dir = modes("slow", "exit 0", &held(&slow), "exit 0");
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), user_query(true), env_with(&dir, None));
        let events = handle.events();
        skip_initial(&events);
        // Waits for the lines, not for a fixed time: under load the child's 50 sleeps take far
        // longer than 1 s, and the number of batches grows with that time. The batch spacing is
        // `batcher_spacing_is_batch_interval`'s job, on virtual time.
        let batches = follow_until(&events, 50, Duration::from_secs(30), BATCH_INTERVAL * 3);
        assert_eq!(
            batches.concat(),
            numbered("f", 0..50),
            "every line exactly once"
        );
        assert!(batches.len() > 1, "lines 20 ms apart span several batches");
        let pid = read_pid(&dir.join("fpid"), Duration::from_secs(1));
        drop(handle);
        assert!(
            reaped_within(pid, Duration::from_secs(2)),
            "child {pid} not reaped"
        );
    }

    fn entry(cursor: &str) -> Vec<JournalEntry> {
        let line = format!("{{\"__CURSOR\":\"{cursor}\",\"MESSAGE\":\"m\"}}");
        parse_lines(line.as_bytes()).0
    }

    /// `stream`'s batching on virtual time: an entry every 20 ms for 1 s gives batches that each
    /// cover one `BATCH_INTERVAL` from their first entry, sent at least `BATCH_INTERVAL` apart,
    /// every entry once and in order.
    #[test]
    fn batcher_spacing_is_batch_interval() {
        let (tx, rx) = async_channel::unbounded();
        let cursors = Mutex::new(Cursors::default());
        let start = Instant::now();
        let mut batcher = Batcher::default();
        let mut sent_at = Vec::new();
        let step = Duration::from_millis(20);
        for i in 0..50u32 {
            let now = start + step * i;
            // `stream` checks for a due batch before it reads more output.
            if batcher.is_due(now) {
                assert!(batcher.flush(&tx, &cursors));
                sent_at.push(now);
            }
            batcher.push(entry(&format!("f{i}")), now);
        }
        let end = start + step * 50 + BATCH_INTERVAL;
        assert!(batcher.is_due(end));
        assert!(batcher.flush(&tx, &cursors));
        sent_at.push(end);
        drop(tx);

        let mut batches = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            batches.extend(follow_batches(std::slice::from_ref(&ev)));
        }
        assert_eq!(batches.concat(), numbered("f", 0..50));
        // 100 ms / 20 ms: five entries per batch.
        assert_eq!(batches.len(), 10);
        assert!(batches.iter().all(|b| b.len() == 5), "{batches:?}");
        for pair in sent_at.windows(2) {
            assert!(pair[1] - pair[0] >= BATCH_INTERVAL, "{sent_at:?}");
        }
        assert_eq!(lock(&cursors).newest.as_deref(), Some("f49"));
        assert_eq!(lock(&cursors).oldest.as_deref(), Some("f0"));
    }

    #[test]
    fn follow_off_kills_child() {
        // Prints a line every 30 ms until killed, so a live child would keep sending batches.
        let endless = r#"i=0
while :; do
printf '{"__CURSOR":"f%d","MESSAGE":"m"}\n' $i
i=$((i+1))
sleep 0.03
done"#;
        let dir = modes("off", "exit 0", &held(endless), "exit 0");
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), user_query(true), env_with(&dir, None));
        let events = handle.events();
        skip_initial(&events);
        assert!(
            matches!(
                next_event(&events, Duration::from_secs(2)),
                Some(JournalEvent::Batch { older: false, .. })
            ),
            "no follow batch"
        );
        let pid = read_pid(&dir.join("fpid"), Duration::from_secs(1));
        handle.set_follow(false);
        assert!(
            reaped_within(pid, Duration::from_millis(200)),
            "child {pid} not reaped within 200 ms"
        );
        // A batch sent just before the stop may still be in the channel.
        let _ = events_for(&events, Duration::from_millis(50));
        let late = events_for(&events, Duration::from_millis(400));
        assert!(late.is_empty(), "{late:?}");
    }

    #[test]
    fn follow_resume_after_cursor_argv() {
        let initial = emit(numbered("c", 40..43));
        let dir = modes(
            "resume",
            &initial,
            &held(&emit(["n1".to_owned()])),
            "exit 0",
        );
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), user_query(false), env_with(&dir, None));
        let events = handle.events();
        skip_initial(&events);
        handle.set_follow(true);
        assert_eq!(
            wait_calls(&dir, 2)[1],
            "--user-unit demo.service -b -o json --no-pager -f --after-cursor=c42"
        );
        // The batch from the follow child moves the newest cursor.
        assert_eq!(
            follow_batches(&[next_event(&events, Duration::from_secs(2)).expect("batch")]),
            [["n1"]]
        );
        let pid = read_pid(&dir.join("fpid"), Duration::from_secs(1));
        handle.set_follow(false);
        assert!(
            reaped_within(pid, Duration::from_secs(1)),
            "child {pid} not reaped"
        );
        handle.set_follow(true);
        assert_eq!(
            wait_calls(&dir, 3)[2],
            "--user-unit demo.service -b -o json --no-pager -f --after-cursor=n1"
        );
        let _ = next_event(&events, Duration::from_secs(2));
        let pid = read_pid(&dir.join("fpid"), Duration::from_secs(1));
        drop(handle);
        assert!(
            reaped_within(pid, Duration::from_secs(2)),
            "child {pid} not reaped"
        );
    }

    #[test]
    fn follow_exit_error_then_retry() {
        let dir = modes("ferr", "exit 0", "echo bad >&2\nexit 3", "exit 0");
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), user_query(true), env_with(&dir, None));
        let events = handle.events();
        skip_initial(&events);
        let expected = JournalError::Exit {
            code: 3,
            stderr_first_line: Arc::from("bad"),
        };
        match next_event(&events, Duration::from_secs(2)) {
            Some(JournalEvent::Error(err)) => assert_eq!(err, expected),
            other => panic!("expected an error, got {other:?}"),
        }
        // The task is still alive and starts a new child at once.
        handle.set_follow(true);
        match next_event(&events, Duration::from_secs(2)) {
            Some(JournalEvent::Error(err)) => assert_eq!(err, expected),
            other => panic!("expected an error, got {other:?}"),
        }
        assert_eq!(calls(&dir).len(), 3);
    }

    #[test]
    fn follow_receiver_dropped_kills_child() {
        let dir = modes("rxdrop", "exit 0", &held(""), "exit 0");
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let (events_tx, events) = async_channel::bounded(EVENTS_CAPACITY);
        let (control, control_rx) = async_channel::bounded(CONTROL_CAPACITY);
        spawn_task(
            &rt.handle(),
            user_query(true),
            env_with(&dir, None),
            events_tx,
            control_rx,
        );
        skip_initial(&events);
        let pid = read_pid(&dir.join("fpid"), Duration::from_secs(2));
        // The control channel stays open, so only the reader's own check can end the child.
        drop(events);
        assert!(
            reaped_within(pid, Duration::from_millis(200)),
            "child {pid} not reaped"
        );
        drop(control);
    }

    #[test]
    fn load_older_reverse_and_drop_cursor_entry() {
        let initial = emit(numbered("c", 10..13));
        let reverse = emit(numbered("c", (0..11).rev()));
        let dir = modes("older", &initial, "exit 0", &reverse);
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), user_query(false), env_with(&dir, None));
        let events = handle.events();
        skip_initial(&events);
        handle.load_older(None);
        assert_eq!(older_batch(&events), (numbered("c", 0..10), true));
        assert_eq!(
            calls(&dir)[1],
            "--user-unit demo.service -b -o json --no-pager --reverse --cursor=c10 -n 201"
        );
        // The next page starts at the new oldest entry.
        handle.load_older(None);
        let _ = older_batch(&events);
        assert!(calls(&dir)[2].contains("--cursor=c0 "), "{:?}", calls(&dir));
    }

    #[test]
    fn load_older_from_ui_cursor() {
        // The engine sent c10..c12, but the UI dropped its oldest lines and holds c350 and up.
        let initial = emit(numbered("c", 10..13));
        let reverse = emit(numbered("c", (150..351).rev()));
        let dir = modes("olderfrom", &initial, "exit 0", &reverse);
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), user_query(false), env_with(&dir, None));
        let events = handle.events();
        skip_initial(&events);
        handle.load_older(Some(Arc::from("c350")));
        // The 200 entries before c350: the anchor and anything after it are absent.
        assert_eq!(older_batch(&events), (numbered("c", 150..350), false));
        assert_eq!(
            calls(&dir)[1],
            "--user-unit demo.service -b -o json --no-pager --reverse --cursor=c350 -n 201"
        );
        // None pages back from the oldest entry sent, which is now the oldest of that batch.
        handle.load_older(None);
        let _ = older_batch(&events);
        assert!(
            calls(&dir)[2].contains("--cursor=c150 "),
            "{:?}",
            calls(&dir)
        );
    }

    #[test]
    fn load_older_exhausted_flag() {
        let initial = emit(["c1000".to_owned()]);
        let reverse = emit(numbered("c", (800..1001).rev()));
        let dir = modes("older201", &initial, "exit 0", &reverse);
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), user_query(false), env_with(&dir, None));
        let events = handle.events();
        skip_initial(&events);
        handle.load_older(None);
        assert_eq!(older_batch(&events), (numbered("c", 800..1000), false));
    }

    #[test]
    fn load_older_with_nothing_loaded() {
        let dir = modes("older0", "exit 0", "exit 0", "exit 0");
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), user_query(false), env_with(&dir, None));
        let events = handle.events();
        skip_initial(&events);
        handle.load_older(None);
        assert_eq!(older_batch(&events), (Vec::new(), true));
        assert_eq!(calls(&dir).len(), 1);
    }

    #[test]
    fn load_older_ignored_while_running() {
        let initial = emit(["c5".to_owned()]);
        let reverse = format!("sleep 0.3\n{}", emit(numbered("c", (0..6).rev())));
        let dir = modes("older2", &initial, "exit 0", &reverse);
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let handle = open_with(&rt.handle(), user_query(false), env_with(&dir, None));
        let events = handle.events();
        skip_initial(&events);
        handle.load_older(None);
        handle.load_older(None);
        assert_eq!(older_batch(&events), (numbered("c", 0..5), true));
        assert!(events_for(&events, Duration::from_millis(500)).is_empty());
        assert_eq!(calls(&dir).len(), 2);
    }
}
