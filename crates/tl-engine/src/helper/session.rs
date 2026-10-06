//! The engine's link to one running `tl-helper` process (ADR-006, ADR-007, R-003, R-157).
//!
//! `ProcLink` starts `pkexec /usr/lib/tasklens/tl-helper --session` with its stdin and stdout on
//! one Unix socketpair, so the `Dump` fd can travel as `SCM_RIGHTS` data (R-145 item 3).
//! `HelperSession` does the `Hello` handshake and runs one request at a time with a deadline.
//! Every function blocks: callers run them inside `spawn_blocking`.
//!
//! Privacy: requests and replies are never logged. Only the request kind, `seq` and the result
//! go to `debug` on target `tasklens::action`.

#![allow(
    dead_code,
    reason = "the helper session task (E29-A2) is the first caller"
)]

use std::ffi::OsStr;
use std::io::{self, IoSlice, Read};
use std::mem::MaybeUninit;
use std::ops::ControlFlow;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
use rustix::net::{
    AddressFamily, SendAncillaryBuffer, SendAncillaryMessage, SendFlags, Shutdown, SocketFlags,
    SocketType, send, sendmsg, shutdown, socketpair,
};
use tl_model::helper_proto::{
    HELPER_PROTO_VERSION, HelperFrame, HelperReply, HelperReplyBody, HelperRequest,
};
use tl_model::wire::{
    MAX_HELPER_FRAME, MAX_HELPER_REPLY_TOTAL, WireError, encode_frame, read_reply,
};

pub(crate) const HELPER_PATH: &str = "/usr/lib/tasklens/tl-helper";
pub(crate) const PKEXEC_PATH: &str = "/usr/bin/pkexec";
/// Deadline of any call that may show a polkit prompt (ADR-006).
pub(crate) const START_DEADLINE: Duration = Duration::from_secs(120);

/// How long `start` waits for the exit status after the helper closed the socket.
const EXIT_WAIT: Duration = Duration::from_secs(2);
/// How long `end` waits for the helper to exit after closing its stdin.
const END_WAIT: Duration = Duration::from_millis(500);
/// Poll step of `ProcLink::exit_status`.
const REAP_STEP: Duration = Duration::from_millis(10);

/// One running helper process, or the test fake.
pub trait HelperLink: Send {
    /// Writes one frame. `fd` travels as `SCM_RIGHTS` data on the frame's first byte.
    fn send(&mut self, frame: &HelperFrame, fd: Option<BorrowedFd<'_>>) -> io::Result<()>;
    /// Reads one reply, waiting at most `timeout`.
    fn recv(&mut self, timeout: Duration) -> Result<HelperReply, LinkError>;
    /// Reaps the process. `None` if it still runs after `wait`.
    fn exit_status(&mut self, wait: Duration) -> Option<i32>;
    /// `SIGKILL`, then `wait()`.
    fn kill(&mut self);
    /// Shuts down the write half, so the helper sees EOF.
    fn close(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    Timeout,
    Eof,
    Protocol,
}

/// `Cancelled`, `NotAuthorized` and `Failed` map one to one to `HelperStartOutcome` (R-148
/// item 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartError {
    /// pkexec exit 126: the polkit prompt was dismissed.
    Cancelled,
    /// pkexec exit 127: not authorized.
    NotAuthorized,
    /// pkexec or the helper binary is not installed.
    Missing,
    Failed(Arc<str>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CallError {
    Timeout,
    Exited,
    Protocol,
}

/// The real helper process and the engine's end of its socketpair.
pub(crate) struct ProcLink {
    child: Child,
    /// Engine end of the socketpair, `CLOEXEC`.
    sock: OwnedFd,
}

impl ProcLink {
    /// Starts `pkexec /usr/lib/tasklens/tl-helper --session` with an empty environment.
    pub(crate) fn spawn() -> Result<ProcLink, StartError> {
        Self::spawn_paths(Path::new(PKEXEC_PATH), Path::new(HELPER_PATH))
    }

    /// `spawn` with other paths, for the `Missing` test.
    #[cfg(test)]
    pub(crate) fn spawn_at(pkexec: &Path, helper: &Path) -> Result<ProcLink, StartError> {
        Self::spawn_paths(pkexec, helper)
    }

    /// Runs any program on the socketpair, for the transport test.
    #[cfg(test)]
    pub(crate) fn spawn_program(program: &str, args: &[&str]) -> Result<ProcLink, StartError> {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        Self::spawn_command(Path::new(program), &args)
    }

    fn spawn_paths(pkexec: &Path, helper: &Path) -> Result<ProcLink, StartError> {
        if !is_regular_file(pkexec) || !is_regular_file(helper) {
            return Err(StartError::Missing);
        }
        Self::spawn_command(pkexec, &[helper.as_os_str(), OsStr::new("--session")])
    }

    /// Starts `program args` with stdin and stdout on one end of a new socketpair and stderr
    /// inherited. No `pre_exec` and no `unsafe`: the helper sets its own death signal.
    /// `asupersync::process` is never used (zombie bug `bi2462.114`).
    fn spawn_command(program: &Path, args: &[&OsStr]) -> Result<ProcLink, StartError> {
        let (sock, child_end) = socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .map_err(|e| spawn_failed(&io::Error::from(e)))?;
        let child_out = child_end.try_clone().map_err(|e| spawn_failed(&e))?;
        let child = Command::new(program)
            .args(args)
            .env_clear()
            .stdin(Stdio::from(child_end))
            .stdout(Stdio::from(child_out))
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => StartError::Missing,
                _ => spawn_failed(&e),
            })?;
        // The `Command` and with it the child's copies of the socket are dropped here.
        Ok(ProcLink { child, sock })
    }
}

impl HelperLink for ProcLink {
    fn send(&mut self, frame: &HelperFrame, fd: Option<BorrowedFd<'_>>) -> io::Result<()> {
        send_frame(self.sock.as_fd(), frame, fd)
    }

    fn recv(&mut self, timeout: Duration) -> Result<HelperReply, LinkError> {
        recv_reply(self.sock.as_fd(), timeout)
    }

    fn exit_status(&mut self, wait: Duration) -> Option<i32> {
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status_code(status)),
                Ok(None) => {}
                Err(_) => return None,
            }
            let left = wait.saturating_sub(started.elapsed());
            if left.is_zero() {
                return None;
            }
            std::thread::sleep(left.min(REAP_STEP));
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        // Once pkexec has run the helper as root, SIGKILL from this user fails with EPERM.
        // Shutting down the socket makes the helper see EOF and exit, so the wait ends.
        let _ = shutdown(&self.sock, Shutdown::Both);
        let _ = self.child.wait();
    }

    fn close(&mut self) {
        let _ = shutdown(&self.sock, Shutdown::Write);
    }
}

impl Drop for ProcLink {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill();
        }
    }
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

fn spawn_failed(e: &io::Error) -> StartError {
    StartError::Failed(format!("could not start the helper: {e}").into())
}

/// The exit code, or 128 plus the signal number for a process ended by a signal.
fn status_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

/// Writes one `wire` frame to `sock`. With `fd`, the first `sendmsg` carries it as one
/// `SCM_RIGHTS` fd, so it is attached to the frame's first byte (R-145 item 3).
/// `MSG_NOSIGNAL` turns a closed peer into `EPIPE` instead of `SIGPIPE`.
fn send_frame(
    sock: BorrowedFd<'_>,
    frame: &HelperFrame,
    fd: Option<BorrowedFd<'_>>,
) -> io::Result<()> {
    let mut buf = Vec::new();
    encode_frame(frame, MAX_HELPER_FRAME, &mut buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut sent = 0;
    if let Some(fd) = fd {
        let fds = [fd];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut control = SendAncillaryBuffer::new(&mut space);
        if !control.push(SendAncillaryMessage::ScmRights(&fds)) {
            return Err(io::Error::other("no room for the fd"));
        }
        sent = loop {
            match sendmsg(
                sock,
                &[IoSlice::new(&buf)],
                &mut control,
                SendFlags::NOSIGNAL,
            ) {
                Ok(n) => break n,
                Err(Errno::INTR) => {}
                Err(e) => return Err(e.into()),
            }
        };
    }
    while sent < buf.len() {
        match send(sock, &buf[sent..], SendFlags::NOSIGNAL) {
            Ok(n) => sent += n,
            Err(Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Reads one reply frame. A frame is at most 64 KiB and one reply message at most 4 MiB
/// (R-157).
fn recv_reply(sock: BorrowedFd<'_>, timeout: Duration) -> Result<HelperReply, LinkError> {
    let mut reader = DeadlineReader {
        sock,
        started: Instant::now(),
        timeout,
    };
    let one = |_: &HelperReply| true;
    match read_reply(&mut reader, MAX_HELPER_FRAME, MAX_HELPER_REPLY_TOTAL, one) {
        Ok(mut replies) => replies.pop().ok_or(LinkError::Protocol),
        Err(WireError::Io(e)) if e.kind() == io::ErrorKind::TimedOut => Err(LinkError::Timeout),
        Err(WireError::Eof | WireError::Truncated | WireError::Io(_)) => Err(LinkError::Eof),
        Err(_) => Err(LinkError::Protocol),
    }
}

/// Reads the socket, polling before each read so the whole frame finishes within `timeout`.
struct DeadlineReader<'a> {
    sock: BorrowedFd<'a>,
    started: Instant,
    timeout: Duration,
}

impl Read for DeadlineReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let left = self.timeout.saturating_sub(self.started.elapsed());
            let left = Timespec::try_from(left)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
            let mut fds = [PollFd::new(&self.sock, PollFlags::IN)];
            match poll(&mut fds, Some(&left)) {
                Ok(0) => return Err(io::ErrorKind::TimedOut.into()),
                Ok(_) => {}
                Err(Errno::INTR) => continue,
                Err(e) => return Err(e.into()),
            }
            match rustix::io::read(self.sock, &mut *buf) {
                Ok(n) => return Ok(n),
                Err(Errno::INTR | Errno::AGAIN) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// A started helper: the `Hello` exchange is done and requests run one at a time (ADR-007).
pub(crate) struct HelperSession<L: HelperLink> {
    link: L,
    /// The `seq` of the next request. Never 0, which only the `Hello` uses.
    next_seq: u32,
}

impl<L: HelperLink> HelperSession<L> {
    /// Sends `Hello` with `seq = 0` and waits up to `START_DEADLINE` for the helper's `Hello`.
    pub(crate) fn start(mut link: L) -> Result<Self, StartError> {
        let hello = HelperFrame {
            seq: 0,
            request: HelperRequest::Hello {
                proto: HELPER_PROTO_VERSION,
            },
        };
        if link.send(&hello, None).is_err() {
            return Err(exit_error(&mut link));
        }
        let result = match link.recv(START_DEADLINE) {
            Ok(HelperReply {
                seq: 0,
                body: HelperReplyBody::Hello { proto },
                ..
            }) => {
                if proto == HELPER_PROTO_VERSION {
                    Ok(())
                } else {
                    Err(StartError::Failed("helper version mismatch".into()))
                }
            }
            Ok(_) | Err(LinkError::Protocol) => {
                Err(StartError::Failed("helper protocol error".into()))
            }
            Err(LinkError::Eof) => return Err(exit_error(&mut link)),
            Err(LinkError::Timeout) => {
                link.kill();
                return Err(StartError::Failed("no answer from the helper".into()));
            }
        };
        let session = HelperSession { link, next_seq: 1 };
        match result {
            Ok(()) => Ok(session),
            Err(e) => {
                session.end();
                Err(e)
            }
        }
    }

    /// Sends `req` and passes each reply to `on_reply` until the reply with `last: true`.
    ///
    /// `deadline` covers the whole call. On `Timeout` or `Protocol` the link is killed and the
    /// session is unusable. `Break` from `on_reply` stops further callbacks; during a
    /// `ScanStrings` call it also sends `CancelScan` and reads both the scan's final reply and
    /// the cancel frame's `Ok` (R-157).
    pub(crate) fn call(
        &mut self,
        req: HelperRequest,
        fd: Option<BorrowedFd<'_>>,
        deadline: Duration,
        on_reply: &mut dyn FnMut(HelperReply) -> ControlFlow<()>,
    ) -> Result<(), CallError> {
        let kind = request_kind(&req);
        let seq = self.take_seq();
        let result = self.run_call(seq, req, fd, deadline, on_reply);
        tracing::debug!(target: "tasklens::action", kind, seq, ?result, "helper call");
        result
    }

    /// Closes the helper's stdin, waits 500 ms for it to exit, then kills it.
    pub(crate) fn end(mut self) {
        self.link.close();
        if self.link.exit_status(END_WAIT).is_none() {
            self.link.kill();
        }
    }

    pub(crate) fn is_alive(&mut self) -> bool {
        self.link.exit_status(Duration::ZERO).is_none()
    }

    /// Returns the next `seq` and moves on, wrapping from `u32::MAX` to 1.
    fn take_seq(&mut self) -> u32 {
        let seq = self.next_seq;
        self.next_seq = match seq.wrapping_add(1) {
            0 => 1,
            next => next,
        };
        seq
    }

    fn run_call(
        &mut self,
        seq: u32,
        request: HelperRequest,
        fd: Option<BorrowedFd<'_>>,
        deadline: Duration,
        on_reply: &mut dyn FnMut(HelperReply) -> ControlFlow<()>,
    ) -> Result<(), CallError> {
        let started = Instant::now();
        let is_scan = matches!(request, HelperRequest::ScanStrings { .. });
        if self.link.send(&HelperFrame { seq, request }, fd).is_err() {
            return Err(CallError::Exited);
        }
        let mut delivering = true;
        let mut request_done = false;
        // The `seq` of the `CancelScan` frame once one is sent.
        let mut cancel_seq = None;
        loop {
            let left = deadline.saturating_sub(started.elapsed());
            let reply = match self.link.recv(left) {
                Ok(reply) => reply,
                Err(LinkError::Timeout) => return Err(self.fail(CallError::Timeout)),
                Err(LinkError::Eof) => return Err(CallError::Exited),
                Err(LinkError::Protocol) => return Err(self.fail(CallError::Protocol)),
            };
            if request_done {
                // Only the cancel frame's single `Ok` may follow the scan's final reply.
                let cancel_ok = Some(reply.seq) == cancel_seq
                    && reply.last
                    && reply.body == HelperReplyBody::Ok;
                return if cancel_ok {
                    Ok(())
                } else {
                    Err(self.fail(CallError::Protocol))
                };
            }
            if reply.seq != seq {
                return Err(self.fail(CallError::Protocol));
            }
            request_done = reply.last;
            if delivering && on_reply(reply).is_break() {
                delivering = false;
                if is_scan && !request_done {
                    let cancel = HelperFrame {
                        seq: self.take_seq(),
                        request: HelperRequest::CancelScan { seq },
                    };
                    if self.link.send(&cancel, None).is_err() {
                        return Err(CallError::Exited);
                    }
                    cancel_seq = Some(cancel.seq);
                }
            }
            if request_done && cancel_seq.is_none() {
                return Ok(());
            }
        }
    }

    /// Kills and reaps the helper: after a timeout or a protocol error the stream position is
    /// unknown, so no later reply can be trusted.
    fn fail(&mut self, err: CallError) -> CallError {
        self.link.kill();
        err
    }
}

/// Maps the exit status of a helper that closed the socket (R-117, R-148 item 3).
fn exit_error<L: HelperLink>(link: &mut L) -> StartError {
    match link.exit_status(EXIT_WAIT) {
        Some(126) => StartError::Cancelled,
        Some(127) => StartError::NotAuthorized,
        Some(n) => StartError::Failed(format!("helper exited with status {n}").into()),
        None => {
            link.kill();
            StartError::Failed("helper closed the connection".into())
        }
    }
}

/// The request's variant name, the only part of a request that is logged.
fn request_kind(req: &HelperRequest) -> &'static str {
    match req {
        HelperRequest::Hello { .. } => "Hello",
        HelperRequest::Signal { .. } => "Signal",
        HelperRequest::SetNice { .. } => "SetNice",
        HelperRequest::SetSched { .. } => "SetSched",
        HelperRequest::SetAffinity { .. } => "SetAffinity",
        HelperRequest::SetIoPriority { .. } => "SetIoPriority",
        HelperRequest::FreezeGroup(_) => "FreezeGroup",
        HelperRequest::ThawGroup(_) => "ThawGroup",
        HelperRequest::KillGroup(_) => "KillGroup",
        HelperRequest::ReadProcessFiles { .. } => "ReadProcessFiles",
        HelperRequest::ScanStrings { .. } => "ScanStrings",
        HelperRequest::Dump { .. } => "Dump",
        HelperRequest::ResolveSocketInodes { .. } => "ResolveSocketInodes",
        HelperRequest::SockDestroy { .. } => "SockDestroy",
        HelperRequest::WriteServiceDropIn { .. } => "WriteServiceDropIn",
        HelperRequest::NetnsSockDiagDump { .. } => "NetnsSockDiagDump",
        HelperRequest::CancelScan { .. } => "CancelScan",
    }
}

#[cfg(test)]
mod tests {
    //! No test runs pkexec or anything as root: the session tests use `FakeLink`, and the
    //! transport tests use a socketpair inside this process or `/bin/cat`.

    use std::collections::VecDeque;
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;

    use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
    use tl_model::actions::{DumpMode, Signal};
    use tl_model::details::{StringEncoding, StringScanOpts};
    use tl_model::helper_proto::HelperTarget;
    use tl_model::ids::ProcessKey;
    use tl_model::time::BootId;
    use tl_model::wire::read_frame;

    use super::*;

    /// What the fake saw, shared with the test after the session takes the link.
    #[derive(Default)]
    struct FakeState {
        sent: Vec<HelperFrame>,
        replies: VecDeque<Result<HelperReply, LinkError>>,
        recv_timeouts: Vec<Duration>,
        exit: Option<i32>,
        /// A helper that keeps running after its stdin closes.
        ignores_eof: bool,
        killed: bool,
        closed: bool,
    }

    struct FakeLink(Arc<Mutex<FakeState>>);

    impl FakeLink {
        fn new(replies: Vec<Result<HelperReply, LinkError>>) -> (FakeLink, Arc<Mutex<FakeState>>) {
            let state = Arc::new(Mutex::new(FakeState {
                replies: replies.into(),
                ..FakeState::default()
            }));
            (FakeLink(state.clone()), state)
        }
    }

    impl HelperLink for FakeLink {
        fn send(&mut self, frame: &HelperFrame, _fd: Option<BorrowedFd<'_>>) -> io::Result<()> {
            self.0.lock().unwrap().sent.push(frame.clone());
            Ok(())
        }

        fn recv(&mut self, timeout: Duration) -> Result<HelperReply, LinkError> {
            let mut state = self.0.lock().unwrap();
            state.recv_timeouts.push(timeout);
            // An empty script is a helper that never answers.
            state.replies.pop_front().unwrap_or(Err(LinkError::Timeout))
        }

        fn exit_status(&mut self, _wait: Duration) -> Option<i32> {
            let state = self.0.lock().unwrap();
            if state.killed { Some(137) } else { state.exit }
        }

        fn kill(&mut self) {
            self.0.lock().unwrap().killed = true;
        }

        fn close(&mut self) {
            let mut state = self.0.lock().unwrap();
            state.closed = true;
            if !state.ignores_eof {
                state.exit.get_or_insert(0);
            }
        }
    }

    fn reply(seq: u32, last: bool, body: HelperReplyBody) -> Result<HelperReply, LinkError> {
        Ok(HelperReply { seq, last, body })
    }

    fn hello_ok() -> Result<HelperReply, LinkError> {
        reply(
            0,
            true,
            HelperReplyBody::Hello {
                proto: HELPER_PROTO_VERSION,
            },
        )
    }

    fn key() -> ProcessKey {
        ProcessKey {
            start_ticks: 4242,
            boot_id: BootId(7),
            pid: 1234,
        }
    }

    fn signal_req() -> HelperRequest {
        HelperRequest::Signal {
            target: HelperTarget::Process(key()),
            signal: Signal::Term,
        }
    }

    fn scan_req() -> HelperRequest {
        HelperRequest::ScanStrings {
            key: key(),
            opts: StringScanOpts::default(),
        }
    }

    fn strings(n: u64) -> HelperReplyBody {
        HelperReplyBody::Strings(vec![(n, StringEncoding::Ascii, format!("s{n}"))])
    }

    fn started(
        replies: Vec<Result<HelperReply, LinkError>>,
    ) -> (HelperSession<FakeLink>, Arc<Mutex<FakeState>>) {
        let mut script = vec![hello_ok()];
        script.extend(replies);
        let (link, state) = FakeLink::new(script);
        let session = HelperSession::start(link).expect("start");
        (session, state)
    }

    fn start_err(replies: Vec<Result<HelperReply, LinkError>>, exit: Option<i32>) -> StartError {
        let (link, state) = FakeLink::new(replies);
        state.lock().unwrap().exit = exit;
        match HelperSession::start(link) {
            Ok(_) => panic!("start succeeded"),
            Err(e) => e,
        }
    }

    #[test]
    fn start_ok() {
        let (_session, state) = started(vec![]);
        let state = state.lock().unwrap();
        assert_eq!(
            state.sent,
            vec![HelperFrame {
                seq: 0,
                request: HelperRequest::Hello {
                    proto: HELPER_PROTO_VERSION
                },
            }]
        );
        assert_eq!(state.recv_timeouts, vec![START_DEADLINE]);
        assert!(!state.killed && !state.closed);
    }

    #[test]
    fn start_exit_codes_map() {
        let eof = || vec![Err(LinkError::Eof)];
        assert_eq!(start_err(eof(), Some(126)), StartError::Cancelled);
        assert_eq!(start_err(eof(), Some(127)), StartError::NotAuthorized);
        assert_eq!(
            start_err(eof(), Some(1)),
            StartError::Failed("helper exited with status 1".into())
        );
    }

    #[test]
    fn start_timeout_kills() {
        let (link, state) = FakeLink::new(vec![Err(LinkError::Timeout)]);
        let err = HelperSession::start(link).err();
        assert_eq!(
            err,
            Some(StartError::Failed("no answer from the helper".into()))
        );
        assert!(state.lock().unwrap().killed);
    }

    #[test]
    fn start_version_mismatch() {
        let (link, state) =
            FakeLink::new(vec![reply(0, true, HelperReplyBody::Hello { proto: 99 })]);
        let err = HelperSession::start(link).err();
        assert_eq!(
            err,
            Some(StartError::Failed("helper version mismatch".into()))
        );
        assert!(state.lock().unwrap().closed);
    }

    #[test]
    fn call_single_reply() {
        let (mut session, state) = started(vec![reply(1, true, HelperReplyBody::Ok)]);
        let mut seen = Vec::new();
        let result = session.call(signal_req(), None, Duration::from_secs(5), &mut |r| {
            seen.push(r);
            ControlFlow::Continue(())
        });
        assert_eq!(result, Ok(()));
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].body, HelperReplyBody::Ok);
        let state = state.lock().unwrap();
        assert_eq!(state.sent[1].seq, 1);
        assert_eq!(state.sent[1].request, signal_req());
    }

    #[test]
    fn call_streams_until_last() {
        let (mut session, _state) = started(vec![
            reply(1, false, strings(1)),
            reply(1, false, strings(2)),
            reply(1, false, strings(3)),
            reply(1, true, strings(4)),
        ]);
        let mut calls = 0;
        let result = session.call(scan_req(), None, Duration::from_secs(5), &mut |_| {
            calls += 1;
            ControlFlow::Continue(())
        });
        assert_eq!(result, Ok(()));
        assert_eq!(calls, 4);
    }

    #[test]
    fn break_sends_cancel() {
        let (mut session, state) = started(vec![
            reply(1, false, strings(1)),
            // The helper may send more batches before it reads the cancel frame.
            reply(1, false, strings(2)),
            reply(1, true, HelperReplyBody::Strings(vec![])),
            reply(2, true, HelperReplyBody::Ok),
        ]);
        let mut calls = 0;
        let result = session.call(scan_req(), None, Duration::from_secs(5), &mut |_| {
            calls += 1;
            ControlFlow::Break(())
        });
        assert_eq!(result, Ok(()));
        assert_eq!(calls, 1);
        let mut state = state.lock().unwrap();
        assert_eq!(state.sent.len(), 3);
        assert_eq!(
            state.sent[2],
            HelperFrame {
                seq: 2,
                request: HelperRequest::CancelScan { seq: 1 },
            }
        );
        assert!(state.replies.is_empty());
        assert!(!state.killed);
        state.replies.push_back(reply(3, true, HelperReplyBody::Ok));
        drop(state);
        // The next request continues after the cancel frame's seq.
        let result = session.call(signal_req(), None, Duration::from_secs(5), &mut |_| {
            ControlFlow::Continue(())
        });
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn cancel_reply_must_carry_cancel_seq() {
        let (mut session, state) = started(vec![
            reply(1, false, strings(1)),
            reply(1, true, HelperReplyBody::Strings(vec![])),
            reply(1, true, HelperReplyBody::Ok),
        ]);
        let result = session.call(scan_req(), None, Duration::from_secs(5), &mut |_| {
            ControlFlow::Break(())
        });
        assert_eq!(result, Err(CallError::Protocol));
        assert!(state.lock().unwrap().killed);
    }

    #[test]
    fn call_timeout_kills() {
        let (mut session, state) = started(vec![]);
        let deadline = Duration::from_secs(2);
        let result = session.call(signal_req(), None, deadline, &mut |_| {
            ControlFlow::Continue(())
        });
        assert_eq!(result, Err(CallError::Timeout));
        let state = state.lock().unwrap();
        assert!(state.killed);
        assert!(state.recv_timeouts[1] <= deadline);
        drop(state);
        assert!(!session.is_alive());
    }

    #[test]
    fn call_eof_is_exited() {
        let (mut session, _state) = started(vec![Err(LinkError::Eof)]);
        let result = session.call(signal_req(), None, Duration::from_secs(2), &mut |_| {
            ControlFlow::Continue(())
        });
        assert_eq!(result, Err(CallError::Exited));
    }

    /// Security regression: a reply for another request must never reach `on_reply`.
    #[test]
    fn wrong_seq_is_protocol_error() {
        let (mut session, state) = started(vec![reply(2, true, HelperReplyBody::Ok)]);
        let mut calls = 0;
        let result = session.call(signal_req(), None, Duration::from_secs(5), &mut |_| {
            calls += 1;
            ControlFlow::Continue(())
        });
        assert_eq!(result, Err(CallError::Protocol));
        assert_eq!(calls, 0);
        assert!(state.lock().unwrap().killed);
    }

    #[test]
    fn seq_wraps_skipping_zero() {
        let (mut session, _state) = started(vec![]);
        session.next_seq = u32::MAX;
        assert_eq!(session.take_seq(), u32::MAX);
        assert_eq!(session.take_seq(), 1);
        assert_eq!(session.take_seq(), 2);
    }

    #[test]
    fn end_closes_then_waits() {
        let (session, state) = started(vec![]);
        session.end();
        let state = state.lock().unwrap();
        assert!(state.closed);
        assert!(!state.killed);
    }

    #[test]
    fn end_kills_when_helper_stays() {
        let (session, state) = started(vec![]);
        state.lock().unwrap().ignores_eof = true;
        session.end();
        let state = state.lock().unwrap();
        assert!(state.closed);
        assert!(state.killed);
    }

    /// R-145 item 3: the `Dump` frame arrives whole with exactly one fd attached.
    #[test]
    fn dump_fd_travels_as_scm_rights() {
        let (engine, helper) = socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        let (pipe_read, pipe_write) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
        let frame = HelperFrame {
            seq: 5,
            request: HelperRequest::Dump {
                key: key(),
                mode: DumpMode::Full,
            },
        };
        send_frame(engine.as_fd(), &frame, Some(pipe_write.as_fd())).unwrap();
        drop(pipe_write);

        let mut expected = Vec::new();
        encode_frame(&frame, MAX_HELPER_FRAME, &mut expected).unwrap();
        let mut buf = vec![0u8; expected.len() + 64];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2))];
        let mut control = RecvAncillaryBuffer::new(&mut space);
        let msg = recvmsg(
            &helper,
            &mut [io::IoSliceMut::new(&mut buf)],
            &mut control,
            RecvFlags::CMSG_CLOEXEC,
        )
        .unwrap();
        assert_eq!(&buf[..msg.bytes], &expected[..]);
        let mut fds: Vec<OwnedFd> = Vec::new();
        for message in control.drain() {
            if let RecvAncillaryMessage::ScmRights(received) = message {
                fds.extend(received);
            }
        }
        assert_eq!(fds.len(), 1);

        // The received fd is the pipe's write end.
        rustix::io::write(&fds[0], b"core").unwrap();
        drop(fds);
        let mut got = [0u8; 8];
        let n = rustix::io::read(&pipe_read, &mut got).unwrap();
        assert_eq!(&got[..n], b"core");
    }

    #[test]
    fn missing_paths() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        // A regular file that is not executable, so nothing could run even if a check failed.
        let regular = here.join("Cargo.toml");
        let absent = here.join("no-such-dir/tl-helper");
        for (pkexec, helper) in [
            (regular.as_path(), absent.as_path()),
            (absent.as_path(), regular.as_path()),
            (regular.as_path(), here),
        ] {
            assert_eq!(
                ProcLink::spawn_at(pkexec, helper).err(),
                Some(StartError::Missing),
                "{} {}",
                pkexec.display(),
                helper.display()
            );
        }
    }

    /// `cat` echoes stdin to stdout, so a frame comes back only if both are the one socket.
    #[test]
    fn proc_link_socketpair_roundtrip() {
        let mut link = ProcLink::spawn_program("/bin/cat", &[]).expect("spawn cat");
        let frame = HelperFrame {
            seq: 9,
            request: signal_req(),
        };
        link.send(&frame, None).unwrap();

        let mut raw = UnixStream::from(link.sock.try_clone().unwrap());
        raw.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let echoed: HelperFrame = read_frame(&mut raw, MAX_HELPER_FRAME).unwrap();
        assert_eq!(echoed, frame);

        link.close();
        assert_eq!(link.exit_status(Duration::from_secs(5)), Some(0));
    }

    #[test]
    fn recv_reads_one_reply_and_times_out() {
        let (engine, helper) = socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        let mut bytes = Vec::new();
        let first = HelperReply {
            seq: 1,
            last: false,
            body: strings(1),
        };
        encode_frame(&first, MAX_HELPER_FRAME, &mut bytes).unwrap();
        // Half of a second frame: the reader must time out, not block.
        bytes.extend_from_slice(&[8, 0, 0, 0, 1]);
        rustix::io::write(&helper, &bytes).unwrap();

        assert_eq!(
            recv_reply(engine.as_fd(), Duration::from_secs(1)),
            Ok(first)
        );
        assert_eq!(
            recv_reply(engine.as_fd(), Duration::from_millis(50)),
            Err(LinkError::Timeout)
        );
    }

    #[test]
    fn recv_oversize_frame_is_protocol_error() {
        let (engine, helper) = socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        let len = (MAX_HELPER_FRAME as u32 + 1).to_le_bytes();
        rustix::io::write(&helper, &len).unwrap();
        assert_eq!(
            recv_reply(engine.as_fd(), Duration::from_secs(1)),
            Err(LinkError::Protocol)
        );
        drop(helper);
        assert_eq!(
            recv_reply(engine.as_fd(), Duration::from_secs(1)),
            Err(LinkError::Eof)
        );
    }
}
