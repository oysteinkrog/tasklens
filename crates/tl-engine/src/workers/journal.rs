//! The journal worker (E14-A4): one `journalctl` child per open `JournalHandle`, used by the
//! Service Properties Logs tab (ADR-005, ADR-016 "Service logs", SYS-015, R-167).
//!
//! `open` spawns one task per handle. The task sends `Access(..)` first, then one `Batch` with
//! the last `JOURNAL_PAGE` entries of the current boot, or one `Error(..)` in its place. The
//! fetch runs `journalctl` with `std::process::Command` inside one `Cx::spawn_blocking` job,
//! which polls the child's pipes against a 5 s deadline.
//!
//! Journal messages can hold secrets. Nothing here logs message text; logs carry only counts,
//! exit codes and durations (ADR-015).

use std::ffi::OsString;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use asupersync::Cx;
use asupersync::runtime::RuntimeHandle;
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
/// The initial fetch is killed after this long (ADR-016).
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the fetch checks for a dropped handle or a cancel while it waits on the pipes.
const POLL_SLICE: Duration = Duration::from_millis(100);
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

/// State one journal task keeps across fetches. E14-009 resumes and pages from the cursors.
#[derive(Default)]
struct Session {
    /// `__CURSOR` of the newest entry sent, for `--after-cursor`.
    #[allow(dead_code, reason = "E14-009 (follow mode) reads it")]
    newest_cursor: Option<Arc<str>>,
    /// `__CURSOR` of the oldest entry sent, for Load older.
    #[allow(dead_code, reason = "E14-009 (Load older) reads it")]
    oldest_cursor: Option<Arc<str>>,
}

/// Opens one journal stream for `query` and returns the UI side of it.
pub(crate) fn open(rt: &RuntimeHandle, query: JournalQuery) -> JournalHandle {
    open_with(rt, query, FetchEnv::system())
}

fn open_with(rt: &RuntimeHandle, query: JournalQuery, env: FetchEnv) -> JournalHandle {
    let (events_tx, events_rx) = async_channel::bounded::<JournalEvent>(EVENTS_CAPACITY);
    let (control_tx, control_rx) = async_channel::bounded::<JournalControl>(CONTROL_CAPACITY);
    let task_events = events_tx.clone();
    let spawned = rt.try_spawn_checked(async move {
        let Some(cx) = Cx::current() else {
            tracing::warn!(target: "tasklens::journal", "journal task has no Cx");
            return;
        };
        run(cx, query, env, task_events, control_rx).await;
    });
    if spawned.is_err() {
        // The runtime is gone. Report it the way `EngineHandle::journal` does after shutdown.
        tracing::warn!(target: "tasklens::journal", "journal task not started");
        let _ = events_tx.try_send(JournalEvent::Error(JournalError::NotFound));
    }
    JournalHandle::new(events_rx, control_tx)
}

/// The task that owns one session: the initial fetch, then idle until the handle is dropped.
async fn run(
    cx: Cx,
    query: JournalQuery,
    env: FetchEnv,
    events: async_channel::Sender<JournalEvent>,
    control: async_channel::Receiver<JournalControl>,
) {
    let mut session = Session::default();
    let fetch_events = events.clone();
    let mut job = match cx.spawn_blocking(move |job_cx| fetch(&job_cx, &query, &env, &fetch_events))
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
    session.oldest_cursor = entries.first().map(|e| e.cursor.clone());
    session.newest_cursor = entries.last().map(|e| e.cursor.clone());
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

    // Follow mode and Load older come with E14-009. Until then the task holds the session
    // and ends when the handle is dropped or the engine shuts down.
    loop {
        match time::timeout(cx.now(), IDLE_SLICE, control.recv()).await {
            Ok(Ok(_msg)) => {}
            Ok(Err(_closed)) => return,
            Err(_elapsed) => {}
        }
        if cx.checkpoint().is_err() || events.is_closed() {
            return;
        }
    }
}

/// What the blocking fetch job returns.
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

    let begin = Instant::now();
    let unit_flag = match query.unit.manager {
        Manager::System => "-u",
        Manager::User => "--user-unit",
    };
    let mut cmd = Command::new("journalctl");
    cmd.arg(unit_flag)
        .arg(&*query.unit.name)
        .args(["-b", "-n", "200", "-o", "json", "--no-pager"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = &env.path {
        cmd.env("PATH", path);
    }
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return FetchOutcome::Failed(JournalError::NotFound);
        }
        Err(e) => return FetchOutcome::Failed(io_failure(&e)),
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
        // A child ended by a signal has no exit code; -1 stands for that.
        let code = out.status.code().unwrap_or(-1);
        tracing::debug!(target: "tasklens::journal", code, "journalctl failed");
        return FetchOutcome::Failed(JournalError::Exit {
            code,
            stderr_first_line: first_line(&out.stderr),
        });
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

/// An I/O error other than "not found" while starting or reading the child.
fn io_failure(e: &std::io::Error) -> JournalError {
    JournalError::Exit {
        code: -1,
        stderr_first_line: cut(&e.to_string()),
    }
}

/// What a finished child left behind.
struct Collected {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

enum CollectError {
    Timeout,
    Stopped,
    Io(std::io::Error),
}

/// Reads stdout and stderr of `child` to the end and reaps it. Past `deadline`, or when `stop`
/// returns true, the child is killed and reaped. Every path out of here reaps the child.
fn collect(
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
fn message_text(value: Option<&Value>) -> Arc<str> {
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
fn process_ids() -> (u32, Vec<u32>) {
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
fn access_for(euid: u32, gids: &[u32], group_file: &Path) -> JournalAccess {
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
}
