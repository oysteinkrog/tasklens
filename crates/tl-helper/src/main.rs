//! tl-helper: the privileged helper that pkexec runs as root (ADR-006, R-003).
//!
//! The only accepted invocation is `tl-helper --session`. There is no `--once` mode and no other
//! flag. The helper reads framed requests on stdin and answers on stdout, both one end of the
//! engine's socketpair (ADR-007). The first request must be `Hello` with this protocol version.
//! The helper exits 0 at stdin EOF or after 300 s without a request, and dies with its parent
//! through `PR_SET_PDEATHSIG` (PROC-076, APP-003).
//!
//! Exit status: 0 for EOF, idle or a gone engine; 1 when the helper cannot start; 2 for bad
//! arguments; 3 for a protocol error. The helper logs nothing. Its only stderr output is the
//! usage line and the line `tl-helper: protocol error`.

mod dispatch;
mod dropin;
mod files;
mod ops;
mod sockets;

use std::io::{IoSliceMut, Read};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fs::Mode;
use rustix::io::Errno;
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags};
use rustix::process::{Pid, Signal};
use tl_model::helper_proto::{
    HELPER_PROTO_VERSION, HelperError, HelperFrame, HelperReplyBody, HelperRequest,
};
use tl_model::wire::{self, MAX_HELPER_FRAME, WireError};

use crate::dispatch::{Ctx, Next, Writer};

const USAGE: &str = "usage: tl-helper --session";
/// Time without a request after which the helper exits.
const IDLE: Duration = Duration::from_secs(300);
/// Test override of `IDLE`, in milliseconds. Read once at start.
const IDLE_ENV: &str = "TL_HELPER_IDLE_MS";

/// Exit status when the helper cannot start.
const EXIT_START: u8 = 1;
/// Exit status for bad arguments.
const EXIT_USAGE: u8 = 2;
/// Exit status for a protocol error.
const EXIT_PROTOCOL: u8 = 3;

/// Parses the arguments after the program name. `Ok` only for exactly `["--session"]`.
fn parse_args(args: &[&str]) -> Result<(), ()> {
    match args {
        ["--session"] => Ok(()),
        _ => Err(()),
    }
}

/// One request frame and the fds that came with its bytes as `SCM_RIGHTS` (R-145 item 3).
struct ReceivedFrame {
    frame: HelperFrame,
    fds: Vec<OwnedFd>,
    /// The kernel dropped fds that did not fit the ancillary buffer (`MSG_CTRUNC`).
    truncated: bool,
}

/// Reads one request frame from the socket `sock` (fd 0) with `tl_model::wire`, collecting every
/// fd that arrives with the frame's bytes. Each read asks for no more than the frame still needs,
/// so fds sent with the next frame stay in the socket. Received fds always have `O_CLOEXEC`. On
/// error every received fd is closed.
fn read_request(sock: BorrowedFd<'_>) -> Result<ReceivedFrame, WireError> {
    let mut reader = FdReader {
        sock,
        fds: Vec::new(),
        truncated: false,
    };
    let frame = wire::read_frame(&mut reader, MAX_HELPER_FRAME)?;
    Ok(ReceivedFrame {
        frame,
        fds: reader.fds,
        truncated: reader.truncated,
    })
}

/// The fd a request may use. A `Dump` must carry exactly one fd; any other request must carry
/// none. Otherwise `Err(NotAllowed)` (rule "unexpected fd"), and every received fd is closed
/// when `fds` drops.
fn request_fd(received: ReceivedFrame) -> (HelperFrame, Result<Option<OwnedFd>, HelperError>) {
    let ReceivedFrame {
        frame,
        mut fds,
        truncated,
    } = received;
    let is_dump = matches!(frame.request, HelperRequest::Dump { .. });
    let fd = match (is_dump, truncated, fds.len()) {
        (true, false, 1) => Ok(fds.pop()),
        (false, false, 0) => Ok(None),
        _ => Err(HelperError::NotAllowed),
    };
    (frame, fd)
}

/// A `Read` over `recvmsg` with room for one fd per call.
struct FdReader<'a> {
    sock: BorrowedFd<'a>,
    fds: Vec<OwnedFd>,
    truncated: bool,
}

impl Read for FdReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut control = RecvAncillaryBuffer::new(&mut space);
        let msg = rustix::net::recvmsg(
            self.sock,
            &mut [IoSliceMut::new(buf)],
            &mut control,
            RecvFlags::CMSG_CLOEXEC,
        )?;
        if msg.flags.contains(ReturnFlags::CTRUNC) {
            self.truncated = true;
        }
        for message in control.drain() {
            if let RecvAncillaryMessage::ScmRights(fds) = message {
                self.fds.extend(fds);
            }
        }
        Ok(msg.bytes)
    }
}

/// Writes the one protocol error line to stderr and returns the exit status for it.
fn protocol_error() -> u8 {
    eprintln!("tl-helper: protocol error");
    EXIT_PROTOCOL
}

/// The idle limit: `TL_HELPER_IDLE_MS` when it is set and parses, else 300 s.
fn idle_limit() -> Duration {
    std::env::var(IDLE_ENV)
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok())
        .map_or(IDLE, Duration::from_millis)
}

/// What waiting on fd 0 gave.
enum Wait {
    Frame(ReceivedFrame),
    /// Idle limit reached or EOF: exit 0.
    Done,
    /// Exit with this status.
    Exit(u8),
}

/// Waits until `deadline` for fd 0 to become readable, then reads one frame. Blocking poll, so
/// an idle helper uses no CPU.
fn wait_for_request(sock: BorrowedFd<'_>, deadline: Instant) -> Wait {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(left) = Timespec::try_from(left) else {
            return Wait::Exit(EXIT_START);
        };
        let mut fds = [PollFd::new(&sock, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&left)) {
            Ok(0) => return Wait::Done,
            Ok(_) => break,
            Err(Errno::INTR) => {}
            Err(_) => return Wait::Exit(EXIT_START),
        }
    }
    match read_request(sock) {
        Ok(received) => Wait::Frame(received),
        Err(WireError::Eof) => Wait::Done,
        // Oversize length, short read, postcard error or a socket error.
        Err(_) => Wait::Exit(protocol_error()),
    }
}

/// Answers the first frame. It must be `Hello` with `HELPER_PROTO_VERSION` and no fd. Returns
/// `Next::Continue` when the session may go on.
fn handshake(received: ReceivedFrame, out: &mut Writer<'_>) -> Next {
    let (frame, fd) = request_fd(received);
    let hello = matches!(
        frame.request,
        HelperRequest::Hello { proto } if proto == HELPER_PROTO_VERSION
    );
    let ok = hello && matches!(fd, Ok(None));
    let body = if ok {
        HelperReplyBody::Hello {
            proto: HELPER_PROTO_VERSION,
        }
    } else {
        // Rule "protocol version".
        HelperReplyBody::Err(HelperError::NotAllowed)
    };
    match out.send_all(frame.seq, vec![body]) {
        Err(_) => Next::Exit(0),
        Ok(()) if ok => Next::Continue,
        Ok(()) => Next::Exit(EXIT_PROTOCOL),
    }
}

/// The session: start-up checks, the handshake, then one request at a time until EOF, the idle
/// limit or an error.
fn run_session() -> u8 {
    // First action: die with the parent (APP-003).
    if rustix::process::set_parent_process_death_signal(Some(Signal::KILL)).is_err() {
        return EXIT_START;
    }
    // The parent was already gone before the line above ran.
    if rustix::process::getppid() == Some(Pid::INIT) {
        return 0;
    }
    rustix::process::umask(Mode::from_raw_mode(0o022));
    // The engine sees EOF before its Hello reply.
    let Some(boot) = ops::read_boot_id() else {
        return EXIT_START;
    };
    let idle = idle_limit();

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut ctx = Ctx {
        sock: stdin.as_fd(),
        boot,
        pending: None,
        stdin_closed: false,
    };
    let mut out = Writer::new(stdout.as_fd());
    let mut deadline = Instant::now() + idle;
    let mut greeted = false;
    loop {
        let received = match ctx.pending.take() {
            Some(received) => received,
            None => match wait_for_request(ctx.sock, deadline) {
                Wait::Frame(received) => received,
                Wait::Done => return 0,
                Wait::Exit(status) => return status,
            },
        };
        let next = if greeted {
            dispatch::handle(received, &mut ctx, &mut out)
        } else {
            greeted = true;
            handshake(received, &mut out)
        };
        if let Next::Exit(status) = next {
            return status;
        }
        deadline = Instant::now() + idle;
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if parse_args(&args).is_err() {
        eprintln!("{USAGE}");
        return ExitCode::from(EXIT_USAGE);
    }
    ExitCode::from(run_session())
}

#[cfg(test)]
mod tests {
    use std::io::IoSlice;
    use std::os::fd::AsFd;

    use rustix::fs::OFlags;
    use rustix::io::FdFlags;
    use rustix::net::{
        AddressFamily, SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketFlags,
        SocketType,
    };
    use tl_model::actions::{DumpMode, Signal};
    use tl_model::helper_proto::HelperTarget;
    use tl_model::ids::ProcessKey;
    use tl_model::time::BootId;

    use super::*;

    const KEY: ProcessKey = ProcessKey {
        start_ticks: 1,
        boot_id: BootId(0x5eed_0000_0000_0009),
        pid: 2,
    };

    fn socketpair() -> (OwnedFd, OwnedFd) {
        rustix::net::socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .expect("socketpair")
    }

    /// Sends `request` as one frame in one `sendmsg`, with `fds` as `SCM_RIGHTS`.
    fn send(sock: BorrowedFd<'_>, request: HelperRequest, fds: &[BorrowedFd<'_>]) {
        let mut bytes = Vec::new();
        let frame = HelperFrame { seq: 7, request };
        wire::encode_frame(&frame, MAX_HELPER_FRAME, &mut bytes).expect("encode frame");
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2))];
        let mut control = SendAncillaryBuffer::new(&mut space);
        if !fds.is_empty() {
            assert!(control.push(SendAncillaryMessage::ScmRights(fds)));
        }
        let sent = rustix::net::sendmsg(
            sock,
            &[IoSlice::new(&bytes)],
            &mut control,
            SendFlags::empty(),
        )
        .expect("sendmsg");
        assert_eq!(sent, bytes.len());
    }

    fn signal() -> HelperRequest {
        HelperRequest::Signal {
            target: HelperTarget::Process(KEY),
            signal: Signal::Term,
        }
    }

    fn dump() -> HelperRequest {
        HelperRequest::Dump {
            key: KEY,
            mode: DumpMode::Mini,
        }
    }

    /// True when the pipe's write end is closed everywhere: a read on the non-blocking read end
    /// gives EOF instead of `EAGAIN`.
    fn write_end_closed(read: &OwnedFd) -> bool {
        let mut byte = [0u8; 1];
        match rustix::io::read(read, &mut byte) {
            Ok(0) => true,
            Err(rustix::io::Errno::AGAIN) => false,
            other => panic!("unexpected pipe read result {other:?}"),
        }
    }

    fn nonblocking_pipe() -> (OwnedFd, OwnedFd) {
        rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )
        .expect("pipe")
    }

    #[test]
    fn args_require_session() {
        assert_eq!(parse_args(&["--session"]), Ok(()));
        assert_eq!(parse_args(&[]), Err(()));
        assert_eq!(parse_args(&["--once"]), Err(()));
        assert_eq!(parse_args(&["--session", "x"]), Err(()));
    }

    #[test]
    fn unexpected_fd_closed() {
        let (engine, helper) = socketpair();
        let (pipe_read, pipe_write) = nonblocking_pipe();
        send(engine.as_fd(), signal(), &[pipe_write.as_fd()]);
        drop(pipe_write);
        assert!(!write_end_closed(&pipe_read), "the fd was not in flight");

        let received = read_request(helper.as_fd()).expect("read frame");
        assert_eq!(received.fds.len(), 1);
        let (frame, fd) = request_fd(received);
        assert_eq!(frame.seq, 7);
        assert!(matches!(fd, Err(HelperError::NotAllowed)));
        assert!(
            write_end_closed(&pipe_read),
            "the received fd is still open"
        );
    }

    #[test]
    fn dump_takes_one_cloexec_fd() {
        let (engine, helper) = socketpair();
        let (_pipe_read, pipe_write) = nonblocking_pipe();
        send(engine.as_fd(), dump(), &[pipe_write.as_fd()]);
        let (frame, fd) = request_fd(read_request(helper.as_fd()).expect("read frame"));
        assert!(matches!(frame.request, HelperRequest::Dump { .. }));
        let fd = fd.expect("one fd").expect("an fd");
        let flags = rustix::io::fcntl_getfd(&fd).expect("getfd");
        assert!(flags.contains(FdFlags::CLOEXEC));
        let mode = rustix::fs::fcntl_getfl(&fd).expect("getfl") & OFlags::RWMODE;
        assert_eq!(mode, OFlags::WRONLY);
    }

    #[test]
    fn dump_without_fd_refused() {
        let (engine, helper) = socketpair();
        send(engine.as_fd(), dump(), &[]);
        let (_, fd) = request_fd(read_request(helper.as_fd()).expect("read frame"));
        assert!(matches!(fd, Err(HelperError::NotAllowed)));
    }

    #[test]
    fn dump_with_two_fds_refused_and_closed() {
        let (engine, helper) = socketpair();
        let (read_a, write_a) = nonblocking_pipe();
        let (read_b, write_b) = nonblocking_pipe();
        send(engine.as_fd(), dump(), &[write_a.as_fd(), write_b.as_fd()]);
        drop((write_a, write_b));

        let (_, fd) = request_fd(read_request(helper.as_fd()).expect("read frame"));
        assert!(matches!(fd, Err(HelperError::NotAllowed)));
        assert!(write_end_closed(&read_a), "the first fd is still open");
        assert!(write_end_closed(&read_b), "the second fd is still open");
    }

    #[test]
    fn request_without_fd_passes() {
        let (engine, helper) = socketpair();
        send(engine.as_fd(), signal(), &[]);
        let (frame, fd) = request_fd(read_request(helper.as_fd()).expect("read frame"));
        assert!(matches!(frame.request, HelperRequest::Signal { .. }));
        assert!(matches!(fd, Ok(None)));
    }

    #[test]
    fn next_frame_keeps_its_fd() {
        let (engine, helper) = socketpair();
        let (_pipe_read, pipe_write) = nonblocking_pipe();
        send(engine.as_fd(), signal(), &[]);
        send(engine.as_fd(), dump(), &[pipe_write.as_fd()]);
        let (_, first) = request_fd(read_request(helper.as_fd()).expect("read first frame"));
        assert!(matches!(first, Ok(None)));
        let (_, second) = request_fd(read_request(helper.as_fd()).expect("read second frame"));
        assert!(matches!(second, Ok(Some(_))));
    }
}
