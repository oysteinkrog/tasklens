//! The dispatch table of the helper session (ADR-006, R-004, R-157).
//!
//! `handle` answers one request after the first `Hello`. Every `HelperRequest` variant has its
//! own arm and there is no wildcard arm, so a new variant does not compile until it is reviewed
//! here. Requests run one at a time in arrival order, and the reply frames of one request are
//! never interleaved with another's.
//!
//! `Writer` is the multi-frame reply writer on fd 1. Frames are at most `MAX_HELPER_FRAME`
//! bytes. A non-streamed reply is encoded in full before any byte is written, so a reply over
//! the 4 MiB total becomes one `Err(TooLarge)`. A streamed reply (`ScanStrings`,
//! `NetnsSockDiagDump`) is written one message at a time, and the cap applies to each message.

use std::io;
use std::os::fd::BorrowedFd;

use rustix::io::Errno;
use tl_model::details::StringScanOpts;
use tl_model::helper_proto::{
    HelperError, HelperFrame, HelperReply, HelperReplyBody, HelperRequest,
};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;
use tl_model::wire::{self, MAX_HELPER_FRAME, MAX_HELPER_REPLY_TOTAL, WireError};

use crate::{FRAME_PREFIX, FrameReader, ReceivedFrame, dropin, files, ops, sockets};
/// Pairs per `SocketOwners` body. One pair is at most 35 postcard bytes, so a body stays under
/// one 64 KiB frame.
const OWNERS_PER_BODY: usize = 1_800;

/// What the session loop does after a request.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    Continue,
    /// Exit with this status. 0 when the engine is gone, 3 after a protocol error.
    Exit(u8),
}

/// Session state shared by the loop and the handlers.
pub struct Ctx<'a> {
    /// fd 0, the socket the requests come on.
    pub sock: BorrowedFd<'a>,
    /// The request frame being read from `sock`, kept across reads so none blocks.
    pub reader: FrameReader,
    /// Read once at start.
    pub boot: BootId,
    /// A frame read during a scan, handled next by the loop.
    pub pending: Option<ReceivedFrame>,
    /// The engine closed its end during a scan.
    pub stdin_closed: bool,
}

/// Writes reply frames to fd 1. Nothing else ever writes to fd 1.
pub struct Writer<'a> {
    fd: BorrowedFd<'a>,
    buf: Vec<u8>,
}

impl<'a> Writer<'a> {
    pub fn new(fd: BorrowedFd<'a>) -> Self {
        Writer {
            fd,
            buf: Vec::new(),
        }
    }

    /// Sends the replies of one non-streamed request: one frame per body, `seq` echoed, `last`
    /// on the final one. All frames are encoded before the first byte is written. A body that
    /// does not fit one frame, or payloads that sum to more than `MAX_HELPER_REPLY_TOTAL`, turn
    /// the whole reply into one `Err(TooLarge)` (R-157). No bodies gives one `Ok`.
    pub fn send_all(&mut self, seq: u32, bodies: Vec<HelperReplyBody>) -> io::Result<()> {
        self.buf.clear();
        if bodies.is_empty() {
            encode_final(seq, HelperReplyBody::Ok, &mut self.buf);
            return write_all(self.fd, &self.buf);
        }
        let count = bodies.len();
        let mut payload = 0usize;
        for (i, body) in bodies.into_iter().enumerate() {
            let reply = HelperReply {
                seq,
                last: i + 1 == count,
                body,
            };
            let start = self.buf.len();
            let fits = wire::encode_frame(&reply, MAX_HELPER_FRAME, &mut self.buf).is_ok() && {
                payload += self.buf.len() - start - FRAME_PREFIX;
                payload <= MAX_HELPER_REPLY_TOTAL
            };
            if !fits {
                self.buf.clear();
                encode_final(
                    seq,
                    HelperReplyBody::Err(HelperError::TooLarge),
                    &mut self.buf,
                );
                break;
            }
        }
        write_all(self.fd, &self.buf)
    }

    /// Sends one reply of a streamed request at once. Returns true when it ended the request:
    /// `last` was set, or the body did not fit one frame and `Err(TooLarge)` with `last` went
    /// out in its place.
    fn send_streamed(&mut self, seq: u32, body: HelperReplyBody, last: bool) -> io::Result<bool> {
        self.buf.clear();
        let reply = HelperReply { seq, last, body };
        let ended = match wire::encode_frame(&reply, MAX_HELPER_FRAME, &mut self.buf) {
            Ok(()) => last,
            Err(_) => {
                encode_final(
                    seq,
                    HelperReplyBody::Err(HelperError::TooLarge),
                    &mut self.buf,
                );
                true
            }
        };
        write_all(self.fd, &self.buf)?;
        Ok(ended)
    }
}

/// Encodes `body` as the final reply of `seq`. Only `Ok` and `Err` bodies come here; they are a
/// few bytes and always fit one frame.
fn encode_final(seq: u32, body: HelperReplyBody, buf: &mut Vec<u8>) {
    let reply = HelperReply {
        seq,
        last: true,
        body,
    };
    let _ = wire::encode_frame(&reply, MAX_HELPER_FRAME, buf);
}

/// Writes all of `bytes` to `fd`, retrying on `EINTR`. An error means the engine is gone.
fn write_all(fd: BorrowedFd<'_>, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match rustix::io::write(fd, bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => bytes = &bytes[n..],
            Err(Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Answers one request after the first `Hello`.
pub fn handle(received: ReceivedFrame, ctx: &mut Ctx<'_>, out: &mut Writer<'_>) -> Next {
    let (frame, fd) = crate::request_fd(received);
    let HelperFrame { seq, request } = frame;
    let fd = match fd {
        Ok(fd) => fd,
        // Rule "unexpected fd": a Dump without exactly one fd, or another request with any.
        Err(e) => return finish(out.send_all(seq, vec![HelperReplyBody::Err(e)])),
    };
    let boot = ctx.boot;
    let bodies = match &request {
        req @ (HelperRequest::Signal { .. }
        | HelperRequest::SetNice { .. }
        | HelperRequest::SetSched { .. }
        | HelperRequest::SetAffinity { .. }
        | HelperRequest::SetIoPriority { .. }) => vec![ops::run_process_op(req, boot)],
        req @ (HelperRequest::FreezeGroup(_)
        | HelperRequest::ThawGroup(_)
        | HelperRequest::KillGroup(_)) => vec![ops::run_cgroup_op(req)],
        HelperRequest::ReadProcessFiles { key, files } => {
            files::read_process_files(*key, *files, boot)
        }
        HelperRequest::ScanStrings { key, opts } => return scan(seq, *key, opts, ctx, out),
        // Outside a scan there is nothing to stop.
        HelperRequest::CancelScan { .. } => vec![HelperReplyBody::Ok],
        HelperRequest::Dump { key, mode } => match fd {
            Some(fd) => vec![ops::run_dump(*key, *mode, fd, boot)],
            // Rule "unexpected fd". `request_fd` already refuses a Dump without its fd.
            None => vec![HelperReplyBody::Err(HelperError::NotAllowed)],
        },
        HelperRequest::ResolveSocketInodes { inodes } => {
            split_owners(sockets::resolve_socket_inodes(inodes, boot))
        }
        HelperRequest::SockDestroy {
            family,
            proto,
            four_tuple,
            cookie,
        } => vec![sockets::sock_destroy(*family, *proto, four_tuple, *cookie)],
        HelperRequest::WriteServiceDropIn { unit, keys } => {
            vec![dropin::write_service_dropin(unit, keys)]
        }
        HelperRequest::NetnsSockDiagDump { netns_ino } => return netns_dump(seq, *netns_ino, out),
        // Rule "hello twice".
        HelperRequest::Hello { .. } => vec![HelperReplyBody::Err(HelperError::NotAllowed)],
    };
    finish(out.send_all(seq, bodies))
}

/// A failed write means the engine closed its end: exit 0, quietly.
fn finish(sent: io::Result<()>) -> Next {
    match sent {
        Ok(()) => Next::Continue,
        Err(_) => Next::Exit(0),
    }
}

/// Splits a `SocketOwners` body into bodies of at most `OWNERS_PER_BODY` pairs, so each fits
/// one frame. The engine merges them. Other bodies pass through.
fn split_owners(body: HelperReplyBody) -> Vec<HelperReplyBody> {
    match body {
        HelperReplyBody::SocketOwners(pairs) if pairs.len() > OWNERS_PER_BODY => pairs
            .chunks(OWNERS_PER_BODY)
            .map(|chunk| HelperReplyBody::SocketOwners(chunk.to_vec()))
            .collect(),
        body => vec![body],
    }
}

/// Runs `NetnsSockDiagDump` and streams its replies. It cannot be cancelled.
fn netns_dump(seq: u32, netns_ino: u64, out: &mut Writer<'_>) -> Next {
    let mut write_failed = false;
    let mut send = |body: HelperReplyBody, last: bool| match out.send_streamed(seq, body, last) {
        Ok(ended) if ended && !last => {
            // A body did not fit one frame and `Err(TooLarge)` ended the request: stop the dump.
            Err(io::Error::other("reply too large"))
        }
        Ok(_) => Ok(()),
        Err(e) => {
            write_failed = true;
            Err(e)
        }
    };
    // The result only says that `send` failed; the flag says how.
    let _ = sockets::netns_sock_diag_dump(netns_ino, &mut send);
    if write_failed {
        Next::Exit(0)
    } else {
        Next::Continue
    }
}

/// Runs `ScanStrings` and streams its replies. Between chunks it reads what fd 0 holds without
/// blocking, so a partial frame never stalls the scan: a whole frame that arrived is kept in `ctx.pending` for the loop, and the scan stops when
/// that frame is the `CancelScan` for this `seq` or when the engine closed its end. The loop
/// then answers the kept `CancelScan` with `Ok`, after the scan's final reply (R-157).
fn scan(
    seq: u32,
    key: ProcessKey,
    opts: &StringScanOpts,
    ctx: &mut Ctx<'_>,
    out: &mut Writer<'_>,
) -> Next {
    let boot = ctx.boot;
    let mut write_failed = false;
    let mut protocol_error = false;
    let mut send = |body: HelperReplyBody, last: bool| match out.send_streamed(seq, body, last) {
        Ok(ended) if ended && !last => {
            // A batch did not fit one frame and `Err(TooLarge)` ended the request: stop the scan.
            Err(io::Error::other("reply too large"))
        }
        Ok(_) => Ok(()),
        Err(e) => {
            write_failed = true;
            Err(e)
        }
    };
    let mut cancel = || {
        if ctx.pending.is_some() {
            return false;
        }
        if ctx.stdin_closed || protocol_error {
            return true;
        }
        match ctx.reader.read_ready(ctx.sock) {
            Ok(Some(received)) => {
                let stop = files::is_cancel_for(&received.frame, seq);
                ctx.pending = Some(received);
                stop
            }
            // Nothing, or part of a frame: the reader keeps the bytes for the next look.
            Ok(None) => false,
            Err(WireError::Eof) => {
                ctx.stdin_closed = true;
                true
            }
            Err(_) => {
                protocol_error = true;
                true
            }
        }
    };
    // The result only says that `send` failed; the flags say how.
    let _ = files::scan_strings(key, opts, boot, &mut send, &mut cancel);
    if protocol_error {
        Next::Exit(crate::protocol_error())
    } else if write_failed || ctx.stdin_closed {
        Next::Exit(0)
    } else {
        Next::Continue
    }
}
