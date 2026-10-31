//! The remote transport: `ssh <target> tasklens bridge` as a `Connector` (ADR-018 Remote
//! transport and Implementation contracts "Endpoints", D-038).
//!
//! The window runs `ssh` with fixed options and `--` before the target, so a target can never
//! become an ssh option. Host-key checking is left to the user's ssh config. stdout carries only
//! protocol bytes; a thread drains stderr into a ring of 64 lines and 16 KiB.
//!
//! Process start plus handshake have `SSH_DEADLINE` (20 s). When the deadline passes, or the
//! stream ends or fails before the first frame from the server (`Welcome` or `Refused`) has been
//! read whole, the child is reaped (killed first when it is still running at the deadline) and
//! the read or write returns an error whose text names the failure (`classify`). After that
//! frame, bytes pass through unchanged and an error ends the session as on any other transport.
//!
//! The window's ends of ssh's stdin and stdout are non-blocking. A read or write that has to
//! wait polls the pipe together with a stop pipe and a pidfd for ssh, so it wakes when
//! `Connection::shutdown` runs or ssh exits, even while a process that ssh started keeps the
//! pipe open. Before the first frame it also wakes at the deadline.
//!
//! `Connection::shutdown` and the drop of the connection's last half kill and reap the child,
//! close the stop pipe and so end the stderr thread and every waiting read and write.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::{Errno, ioctl_fionbio};
use rustix::pipe::{PipeFlags, pipe_with};
use rustix::process::{Pid, PidfdFlags, pidfd_open};
use tl_model::remote_proto::{FrameHeader, HEADER_LEN};

use crate::client::{Connection, Connector};

/// Process start plus handshake, per connection attempt.
pub const SSH_DEADLINE: Duration = Duration::from_secs(20);
/// The variable that names another ssh program. Only test builds read it (`ssh_program`).
pub const SSH_PROGRAM_ENV: &str = "TASKLENS_SSH_PROGRAM";
/// stderr lines kept, newest last.
pub const STDERR_LINES: usize = 64;
/// stderr bytes kept in total. One longer line is cut to this length.
pub const STDERR_BYTES: usize = 16 * 1024;
/// How often a failed attempt checks whether the child has exited.
const POLL: Duration = Duration::from_millis(10);
/// How often a write that waits for room in the pipe before the first frame checks whether that
/// frame has been read meanwhile, so the write then waits without the deadline.
const WRITE_SLICE: Duration = Duration::from_millis(100);
/// How long a failed attempt waits for the end of stderr after the child has exited. Then the
/// stderr thread is stopped, so a grandchild that keeps the pipe open does not hold the attempt
/// or the thread longer.
const STDERR_GRACE: Duration = Duration::from_secs(1);

/// Why a target was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetError {
    Empty,
    /// Starts with `-`, so ssh would read it as an option.
    LeadingDash,
    /// Contains whitespace or a control character.
    BadChar,
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TargetError::Empty => "the ssh target is empty",
            TargetError::LeadingDash => "the ssh target starts with -",
            TargetError::BadChar => "the ssh target contains a space or a control character",
        })
    }
}

impl std::error::Error for TargetError {}

/// Rejects a target that is empty, starts with `-`, or contains whitespace or a control
/// character.
pub fn validate_target(target: &str) -> Result<(), TargetError> {
    if target.is_empty() {
        return Err(TargetError::Empty);
    }
    if target.starts_with('-') {
        return Err(TargetError::LeadingDash);
    }
    if target.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(TargetError::BadChar);
    }
    Ok(())
}

/// The ssh command for `target`, with stdin, stdout and stderr piped. Validate the target
/// first.
pub fn ssh_command(program: &OsStr, target: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "--",
    ])
    .arg(target)
    .args(["tasklens", "bridge"])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    cmd
}

/// The ssh program: `env` (the value of `SSH_PROGRAM_ENV`) in test builds when it is set and
/// not empty, else `ssh`.
pub fn ssh_program(env: Option<OsString>) -> OsString {
    #[cfg(feature = "test-hooks")]
    if let Some(program) = env.filter(|p| !p.is_empty()) {
        return program;
    }
    #[cfg(not(feature = "test-hooks"))]
    drop(env);
    OsString::from("ssh")
}

/// The newest stderr lines of one ssh run: at most `STDERR_LINES` lines and `STDERR_BYTES`
/// bytes, the oldest dropped first.
#[derive(Default, Debug)]
pub struct StderrRing {
    lines: std::collections::VecDeque<String>,
    bytes: usize,
    /// The line being read, cut at `STDERR_BYTES`.
    partial: Vec<u8>,
}

impl StderrRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one line without its line end.
    pub fn push_line(&mut self, line: &str) {
        let mut end = line.len().min(STDERR_BYTES);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        let line = &line[..end];
        self.bytes += line.len();
        self.lines.push_back(line.to_owned());
        while self.lines.len() > STDERR_LINES || self.bytes > STDERR_BYTES {
            if let Some(old) = self.lines.pop_front() {
                self.bytes -= old.len();
            }
        }
    }

    /// Adds bytes as read from the pipe; each `\n` ends a line.
    pub fn feed(&mut self, bytes: &[u8]) {
        for chunk in bytes.split_inclusive(|&b| b == b'\n') {
            let (text, ends) = match chunk.strip_suffix(b"\n") {
                Some(text) => (text, true),
                None => (chunk, false),
            };
            let room = STDERR_BYTES.saturating_sub(self.partial.len());
            self.partial
                .extend_from_slice(&text[..text.len().min(room)]);
            if ends {
                self.end_line();
            }
        }
    }

    /// Adds the last line when the stream ended without a line end.
    pub fn finish(&mut self) {
        if !self.partial.is_empty() {
            self.end_line();
        }
    }

    fn end_line(&mut self) {
        let raw = std::mem::take(&mut self.partial);
        let text = String::from_utf8_lossy(&raw);
        self.push_line(text.trim_end_matches('\r'));
    }

    /// The kept lines, oldest first.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    /// Bytes of the kept lines.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The newest line that is not blank, trimmed.
    pub fn last_line(&self) -> Option<&str> {
        self.lines
            .iter()
            .rev()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.lines.iter().any(|l| l.contains(needle))
    }
}

/// A failed ssh connection attempt, named for the user. `Display` is the reason the host tab
/// shows after "Can't connect: " (UI-SHL-008).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SshFailure {
    /// Process start plus handshake took longer than `after`.
    TimedOut { name: Arc<str>, after: Duration },
    /// ssh exited 255 with "Permission denied".
    NeedsKey { name: Arc<str> },
    /// "Host key verification failed".
    HostKey { name: Arc<str> },
    /// Exit 127 or "not found": the remote shell has no `tasklens`.
    NotInstalled { name: Arc<str> },
    /// Anything else, with ssh's exit code and its last stderr line.
    Other {
        name: Arc<str>,
        exit: Option<i32>,
        line: Option<String>,
    },
}

impl fmt::Display for SshFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SshFailure::TimedOut { name, after } => {
                write!(f, "{name} did not answer within ")?;
                if after.subsec_millis() == 0 && after.as_secs() > 0 {
                    write!(f, "{} s", after.as_secs())
                } else {
                    write!(f, "{} ms", after.as_millis())
                }
            }
            SshFailure::NeedsKey { name } => write!(f, "ssh needs a key for {name}"),
            SshFailure::HostKey { name } => write!(f, "ssh does not trust the host key of {name}"),
            SshFailure::NotInstalled { name } => write!(f, "tasklens is not installed on {name}"),
            SshFailure::Other { name, exit, line } => {
                write!(f, "ssh to {name} failed")?;
                match exit {
                    Some(code) => write!(f, " (exit {code})")?,
                    None => f.write_str(" (no exit code)")?,
                }
                match line {
                    Some(line) => write!(f, ": {line}"),
                    None => Ok(()),
                }
            }
        }
    }
}

/// Names the failure of one attempt, checked in this order: the deadline (`timed_out` is the
/// deadline that passed), 255 with "Permission denied", "Host key verification failed", 127 or
/// "not found", anything else.
pub fn classify(
    exit: Option<i32>,
    stderr: &StderrRing,
    timed_out: Option<Duration>,
    name: &str,
) -> SshFailure {
    let name: Arc<str> = Arc::from(name);
    if let Some(after) = timed_out {
        return SshFailure::TimedOut { name, after };
    }
    if exit == Some(255) && stderr.contains("Permission denied") {
        return SshFailure::NeedsKey { name };
    }
    if stderr.contains("Host key verification failed") {
        return SshFailure::HostKey { name };
    }
    if exit == Some(127) || stderr.contains("not found") {
        return SshFailure::NotInstalled { name };
    }
    SshFailure::Other {
        name,
        exit,
        line: stderr.last_line().map(str::to_owned),
    }
}

/// Connects to one host through `ssh <target> tasklens bridge`.
pub struct SshConnector {
    name: Arc<str>,
    target: String,
    program: OsString,
    deadline: Duration,
}

impl SshConnector {
    /// `name` is the host's name in the window; `program` comes from `ssh_program`.
    pub fn new(name: impl Into<Arc<str>>, target: impl Into<String>, program: OsString) -> Self {
        Self {
            name: name.into(),
            target: target.into(),
            program,
            deadline: SSH_DEADLINE,
        }
    }

    /// The deadline for process start plus handshake, `SSH_DEADLINE` by default.
    #[must_use]
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }
}

impl Connector for SshConnector {
    fn connect(&self) -> io::Result<Connection> {
        validate_target(&self.target)
            .map_err(|e| io::Error::other(format!("{}: {e}", self.name)))?;
        let deadline = Instant::now() + self.deadline;
        let mut child = ssh_command(&self.program, &self.target)
            .spawn()
            .map_err(|e| match e.kind() {
                ErrorKind::NotFound => io::Error::other("ssh is not installed"),
                _ => io::Error::other(format!("ssh did not start: {e}")),
            })?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            reap(&mut child);
            return Err(io::Error::other("ssh started without its pipes"));
        };
        // No read or write may block where the stop pipe cannot wake it (`Proc::wait`).
        for fd in [stdin.as_fd(), stdout.as_fd()] {
            if let Err(e) = ioctl_fionbio(fd, true) {
                reap(&mut child);
                return Err(io::Error::other(format!("ssh pipes: {e}")));
            }
        }
        // Closing `stop_tx` wakes the stderr thread and every waiting read and write.
        let (stop_rx, stop_tx) = match pipe_with(PipeFlags::CLOEXEC) {
            Ok(pair) => pair,
            Err(e) => {
                reap(&mut child);
                return Err(io::Error::other(format!("ssh stop pipe: {e}")));
            }
        };
        let stop_rx = Arc::new(stop_rx);
        // Readable once ssh has exited. A kernel without pidfds (before 5.3) leaves it out, and
        // only `shutdown` then wakes a read or write that a process ssh started holds open.
        let exited = pidfd_open(Pid::from_child(&child), PidfdFlags::empty()).ok();
        let ring = Arc::new(Mutex::new(StderrRing::new()));
        let drain_ring = Arc::clone(&ring);
        let drain_stop = Arc::clone(&stop_rx);
        let drain = thread::Builder::new()
            .name("tl-ssh-stderr".into())
            .spawn(move || drain_stderr(&stderr, &*drain_stop, &drain_ring));
        let drain = match drain {
            Ok(handle) => handle,
            Err(e) => {
                reap(&mut child);
                return Err(io::Error::other(format!("ssh stderr thread: {e}")));
            }
        };
        let proc = Arc::new(Proc {
            name: Arc::clone(&self.name),
            wait: self.deadline,
            deadline,
            child: Mutex::new(Some(child)),
            stderr: ring,
            drain: Mutex::new(Some(drain)),
            stop: Mutex::new(Some(stop_tx)),
            stop_rx,
            exited,
            lifted: AtomicBool::new(false),
            failure: Mutex::new(None),
        });
        let control = Arc::clone(&proc);
        Ok(Connection {
            reader: Box::new(SshReader {
                out: stdout,
                proc: Arc::clone(&proc),
                header: [0; HEADER_LEN],
                filled: 0,
                rest: None,
            }),
            writer: Box::new(SshWriter { input: stdin, proc }),
            shutdown: Arc::new(move || control.terminate()),
        })
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads the child's stderr into `ring` until the pipe ends or `stop` becomes readable (its
/// write end was closed).
fn drain_stderr(stderr: &impl AsFd, stop: &impl AsFd, ring: &Mutex<StderrRing>) {
    let mut buf = [0u8; 4096];
    loop {
        let mut fds = [
            PollFd::new(stderr, PollFlags::IN),
            PollFd::new(stop, PollFlags::IN),
        ];
        match poll(&mut fds, None) {
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(_) => break,
        }
        if !fds[1].revents().is_empty() {
            break;
        }
        if fds[0].revents().is_empty() {
            continue;
        }
        // `poll` saw bytes or the end, so this read does not block.
        match rustix::io::read(stderr, &mut buf[..]) {
            Ok(0) => break,
            Ok(n) => lock(ring).feed(&buf[..n]),
            Err(Errno::INTR | Errno::AGAIN) => {}
            Err(_) => break,
        }
    }
    lock(ring).finish();
}

/// One ssh child and what its connection halves share.
struct Proc {
    name: Arc<str>,
    /// The length of the deadline, for the message.
    wait: Duration,
    deadline: Instant,
    /// `None` once reaped.
    child: Mutex<Option<Child>>,
    stderr: Arc<Mutex<StderrRing>>,
    drain: Mutex<Option<JoinHandle<()>>>,
    /// The write end of the stop pipe. Dropping it ends the stderr thread and wakes every
    /// waiting read and write.
    stop: Mutex<Option<OwnedFd>>,
    /// The read end of the stop pipe.
    stop_rx: Arc<OwnedFd>,
    /// A pidfd for ssh, readable once it has exited. `None` when the kernel has no pidfds.
    exited: Option<OwnedFd>,
    /// The first frame from the server has been read whole.
    lifted: AtomicBool,
    /// The named failure, once known. Reader and writer report the same one.
    failure: Mutex<Option<Arc<str>>>,
}

impl Proc {
    /// Ends the attempt before its first frame: reaps the child, killing it when it still runs
    /// at the deadline (or at once when `timed_out`), and names the failure.
    fn failed(&self, timed_out: bool) -> io::Error {
        let mut failure = lock(&self.failure);
        if let Some(text) = &*failure {
            return io::Error::other(text.to_string());
        }
        let (exit, timed_out) = self.end_child(timed_out);
        self.stop_drain(STDERR_GRACE);
        let text = classify(
            exit,
            &lock(&self.stderr),
            timed_out.then_some(self.wait),
            &self.name,
        )
        .to_string();
        *failure = Some(Arc::from(text.as_str()));
        io::Error::other(text)
    }

    /// Waits for the child to exit until the deadline, then kills it. Returns its exit code and
    /// whether the deadline passed.
    fn end_child(&self, mut timed_out: bool) -> (Option<i32>, bool) {
        loop {
            let mut guard = lock(&self.child);
            let Some(child) = guard.as_mut() else {
                return (None, timed_out);
            };
            if !timed_out {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        *guard = None;
                        return (status.code(), false);
                    }
                    Ok(None) if Instant::now() < self.deadline => {
                        drop(guard);
                        thread::sleep(POLL);
                        continue;
                    }
                    Ok(None) => timed_out = true,
                    Err(_) => {}
                }
            }
            let _ = child.kill();
            let code = child.wait().ok().and_then(|s| s.code());
            *guard = None;
            return (code, timed_out);
        }
    }

    /// Ends the stderr thread and returns once it has ended. The thread first gets up to
    /// `grace` to read stderr to its end, so the ring holds all of it. Then it is told to stop,
    /// so a process that keeps the pipe open cannot leave it blocked.
    fn stop_drain(&self, grace: Duration) {
        let Some(handle) = lock(&self.drain).take() else {
            return;
        };
        let until = Instant::now() + grace;
        while !handle.is_finished() && Instant::now() < until {
            thread::sleep(POLL);
        }
        drop(lock(&self.stop).take());
        let _ = handle.join();
    }

    /// The error for a read or write that the stop pipe woke: the named failure when the
    /// attempt failed, else a closed connection.
    fn closed(&self) -> io::Error {
        match &*lock(&self.failure) {
            Some(text) => io::Error::other(text.to_string()),
            None => io::Error::new(ErrorKind::BrokenPipe, "the ssh connection was closed"),
        }
    }

    /// Waits until `fd` is ready for `flags` (or has ended or failed), the stop pipe closes, ssh
    /// exits or `until` passes. `None` waits with no limit.
    fn wait(&self, fd: &impl AsFd, flags: PollFlags, until: Option<Instant>) -> io::Result<Woke> {
        loop {
            let left = match until {
                None => None,
                Some(until) => {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok(Woke::TimedOut);
                    }
                    Some(Timespec::try_from(left).map_err(io::Error::other)?)
                }
            };
            // Without a pidfd the stop pipe takes its place, so the set keeps its size.
            let exited: &OwnedFd = self.exited.as_ref().unwrap_or(&*self.stop_rx);
            let mut fds = [
                PollFd::new(fd, flags),
                PollFd::new(&*self.stop_rx, PollFlags::IN),
                PollFd::new(exited, PollFlags::IN),
            ];
            match poll(&mut fds, left.as_ref()) {
                Ok(_) => {}
                Err(Errno::INTR) => continue,
                Err(e) => return Err(e.into()),
            }
            let [ready, stopped, exited] = fds.map(|fd| fd.revents());
            if !stopped.is_empty() {
                return Ok(Woke::Stopped);
            }
            if !ready.is_empty() {
                return Ok(Woke::Ready);
            }
            if !exited.is_empty() {
                return Ok(Woke::Exited);
            }
        }
    }

    /// Kills and reaps the child if it still runs, and ends the stderr thread.
    fn terminate(&self) {
        if let Some(mut child) = lock(&self.child).take() {
            reap(&mut child);
        }
        self.stop_drain(Duration::ZERO);
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// What ended a wait on one of ssh's pipes (`Proc::wait`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Woke {
    /// The pipe is ready, or has ended or failed.
    Ready,
    /// ssh has exited. A process it started may still hold the pipe open.
    Exited,
    /// The stop pipe closed: the connection was shut down or the attempt failed.
    Stopped,
    /// The wait's limit passed.
    TimedOut,
}

/// What one read from ssh's stdout gave.
enum Got {
    Bytes(usize),
    /// The pipe ended, or ssh exited and left nothing more in it.
    End,
    Stopped,
    TimedOut,
}

/// ssh's stdout. Until the first frame is read whole, a read waits only until the deadline and
/// reads no further than the end of that frame.
struct SshReader {
    out: ChildStdout,
    proc: Arc<Proc>,
    header: [u8; HEADER_LEN],
    filled: usize,
    /// Payload bytes of the first frame still to read, once its header is read.
    rest: Option<usize>,
}

impl SshReader {
    /// One read of what stdout has, waiting for bytes until `until` (`None`: no limit).
    fn read_some(&self, buf: &mut [u8], until: Option<Instant>) -> io::Result<Got> {
        let mut exited = false;
        loop {
            match rustix::io::read(&self.out, &mut *buf) {
                Ok(0) => return Ok(Got::End),
                Ok(n) => return Ok(Got::Bytes(n)),
                Err(Errno::INTR) => continue,
                // All that ssh wrote before it exited has been read. Only a process it started
                // could still write, and that is not ssh's stream.
                Err(Errno::AGAIN) if exited => return Ok(Got::End),
                Err(Errno::AGAIN) => {}
                Err(e) => return Err(e.into()),
            }
            match self.proc.wait(&self.out, PollFlags::IN, until)? {
                Woke::Ready => {}
                Woke::Exited => exited = true,
                Woke::Stopped => return Ok(Got::Stopped),
                Woke::TimedOut => return Ok(Got::TimedOut),
            }
        }
    }
}

impl Read for SshReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.proc.lifted.load(Ordering::Acquire) {
            return match self.read_some(buf, None)? {
                Got::Bytes(n) => Ok(n),
                Got::End => Ok(0),
                Got::Stopped | Got::TimedOut => Err(self.proc.closed()),
            };
        }
        let want = self.rest.unwrap_or(HEADER_LEN - self.filled);
        let len = buf.len().min(want);
        let n = match self.read_some(&mut buf[..len], Some(self.proc.deadline)) {
            Ok(Got::Bytes(n)) => n,
            Ok(Got::TimedOut) => return Err(self.proc.failed(true)),
            Ok(Got::Stopped) => return Err(self.proc.closed()),
            Ok(Got::End) | Err(_) => return Err(self.proc.failed(false)),
        };
        match &mut self.rest {
            None => {
                self.header[self.filled..self.filled + n].copy_from_slice(&buf[..n]);
                self.filled += n;
                if self.filled == HEADER_LEN {
                    // A header that does not parse ends the session, so the deadline stays.
                    self.rest = Some(
                        FrameHeader::parse(&self.header)
                            .map_or(usize::MAX, |h| h.encoded_len as usize),
                    );
                }
            }
            Some(rest) => *rest -= n,
        }
        if self.rest == Some(0) {
            self.proc.lifted.store(true, Ordering::Release);
        }
        Ok(n)
    }
}

/// ssh's stdin. Until the first frame from the server is read whole, a write that finds the
/// pipe full waits only until the deadline, and a write that fails returns the named failure,
/// since ssh has then exited or not read in time. After that frame, a write waits until there
/// is room, `shutdown` runs or ssh exits.
struct SshWriter {
    input: ChildStdin,
    proc: Arc<Proc>,
}

impl Write for SshWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let lifted = self.proc.lifted.load(Ordering::Acquire);
            match rustix::io::write(&self.input, buf) {
                Ok(n) => return Ok(n),
                Err(Errno::INTR) => continue,
                Err(Errno::AGAIN) => {}
                Err(e) if lifted => return Err(e.into()),
                Err(_) => return Err(self.proc.failed(false)),
            }
            let until = if lifted {
                None
            } else {
                let now = Instant::now();
                if now >= self.proc.deadline {
                    return Err(self.proc.failed(true));
                }
                Some(self.proc.deadline.min(now + WRITE_SLICE))
            };
            match self.proc.wait(&self.input, PollFlags::OUT, until) {
                Ok(Woke::Ready | Woke::TimedOut) => {}
                Ok(Woke::Stopped) => return Err(self.proc.closed()),
                Ok(Woke::Exited) if lifted => {
                    return Err(io::Error::new(ErrorKind::BrokenPipe, "ssh has exited"));
                }
                Err(e) if lifted => return Err(e),
                Ok(Woke::Exited) | Err(_) => return Err(self.proc.failed(false)),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        // A pipe has no buffer of its own to flush.
        Ok(())
    }
}
