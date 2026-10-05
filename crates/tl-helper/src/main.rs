//! tl-helper: the privileged helper that pkexec runs as root (ADR-006, R-003).
//!
//! The only accepted invocation is `tl-helper --session`. There is no `--once` mode and no other
//! flag. The session loop arrives with E29-A1.

mod dropin;
mod files;
mod ops;
mod sockets;

use std::io::{IoSliceMut, Read};
use std::mem::MaybeUninit;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::process::ExitCode;

use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags};
use tl_model::helper_proto::{HelperError, HelperFrame, HelperRequest};
use tl_model::wire::{self, MAX_HELPER_FRAME, WireError};

const USAGE: &str = "usage: tl-helper --session";

/// Parses the arguments after the program name. `Ok` only for exactly `["--session"]`.
fn parse_args(args: &[&str]) -> Result<(), ()> {
    match args {
        ["--session"] => Ok(()),
        _ => Err(()),
    }
}

/// One request frame and the fds that came with its bytes as `SCM_RIGHTS` (R-145 item 3).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the session loop arrives with E29-A1")
)]
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the session loop arrives with E29-A1")
)]
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the session loop arrives with E29-A1")
)]
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if parse_args(&args).is_err() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
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
