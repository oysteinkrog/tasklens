//! `tasklens bridge`: the far end of the ssh transport (ADR-018 Remote transport, Implementation
//! contracts "Endpoints").
//!
//! The window runs `ssh <target> tasklens bridge`. The bridge connects its stdin and stdout to
//! this user's engine socket and copies bytes both ways. When connecting fails for any reason it
//! runs `systemctl --user start tasklens-engine.service` once and connects once more.
//!
//! It reads the window's `Hello` and sets its bridge flag, so the engine knows the session is
//! remote. A `Hello` it cannot decode (another protocol version) goes on unchanged, so the
//! engine answers with `Refused` and the window shows why.
//!
//! The bridge writes nothing but protocol bytes to stdout. Its own errors go to stderr, where
//! the window's ssh connector reads them.

use std::fs::File;
use std::io;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::{Errno, ioctl_fionbio};

use tl_model::remote_proto::{
    FrameHeader, HEADER_LEN, Hello, RawFrame, decode_hello, encode_hello,
};
use tl_remote::server;

use crate::engine_link::{self, CONNECT_DEADLINE};
use crate::paths;

/// Runs `tasklens bridge` on stdin and stdout. `app_id` is the checked `--app-id`.
pub fn main(app_id: Option<&str>) -> ExitCode {
    match main_inner(app_id) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tasklens bridge: {e}");
            ExitCode::from(1)
        }
    }
}

fn main_inner(app_id: Option<&str>) -> Result<(), String> {
    let app_dir = app_id.unwrap_or(paths::APP_DIR);
    let socket = engine_link::socket_path(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), app_dir)?;
    // Not `Stdout`: it is line-buffered and would hold binary frames back.
    let input = io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map(File::from)
        .map_err(|e| format!("stdin: {e}"))?;
    let output = io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .map(File::from)
        .map_err(|e| format!("stdout: {e}"))?;
    let unit: &dyn Fn(Duration) -> bool = &engine_link::start_unit;
    // The unit serves the default paths only; an `--app-id` instance stays on its own (D-016).
    let start_unit = app_id.is_none().then_some(unit);
    run(input, output, &socket, server::own_uid(), start_unit)
}

/// Connects `input` and `output` to the engine at `socket`, whose peer must run as `uid`, and
/// copies until either side ends. `start_unit` runs once when the first connect fails.
pub fn run(
    input: File,
    output: File,
    socket: &Path,
    uid: u32,
    start_unit: Option<&dyn Fn(Duration) -> bool>,
) -> Result<(), String> {
    let stream = match connect(socket, uid) {
        Ok(stream) => stream,
        Err(_) => {
            if let Some(start_unit) = start_unit {
                start_unit(CONNECT_DEADLINE);
            }
            connect(socket, uid)
                .map_err(|e| format!("cannot connect to {}: {e}", socket.display()))?
        }
    };
    pump(&input, &output, &stream).map_err(|e| format!("bridge: {e}"))
}

/// The most bytes held for one direction.
const BUF: usize = 64 * 1024;

/// How long the bytes one side sent before it ended may take to reach the other side.
const DRAIN: Duration = Duration::from_millis(500);

/// Copies `input` to `engine` and `engine` to `output` in one poll loop on non-blocking fds,
/// with at most `BUF` held per direction. No step blocks while the other direction waits, and
/// all three fds stay in the poll set, so a hang-up on any of them is seen even while its
/// buffer is full.
///
/// The window's first frame is held back until it is whole and goes on as `hello` rewrites it.
/// Waiting for it is part of the loop, so the engine's end (its `Hello` deadline is 5 s) or the
/// window's ends that wait too.
///
/// Returns when either side ends. After EOF or a hang-up from the window, the engine gets up to
/// `DRAIN` for what the window sent; after the same from the engine, the window gets the same.
/// A failed write to the window ends the bridge at once. `Ok` covers every such end; `Err` is a
/// window that ends before its first frame is whole, a first frame over `BUF`, or a failed poll
/// or `fcntl`.
fn pump(input: &File, output: &File, engine: &UnixStream) -> io::Result<()> {
    for fd in [input.as_fd(), output.as_fd(), engine.as_fd()] {
        ioctl_fionbio(fd, true)?;
    }
    let gone = PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL;
    let mut up = Vec::with_capacity(BUF);
    // The window's first frame has gone into `up` as it goes to the engine.
    let mut greeted = false;
    let mut down = Vec::with_capacity(BUF);
    let mut chunk = vec![0u8; BUF];
    loop {
        let mut input_flags = PollFlags::empty();
        if up.len() < BUF {
            input_flags |= PollFlags::IN;
        }
        let mut engine_flags = PollFlags::empty();
        if down.len() < BUF {
            engine_flags |= PollFlags::IN;
        }
        if greeted && !up.is_empty() {
            engine_flags |= PollFlags::OUT;
        }
        let mut output_flags = PollFlags::empty();
        if !down.is_empty() {
            output_flags |= PollFlags::OUT;
        }
        // Empty flags still report HUP and ERR.
        let mut fds = [
            PollFd::new(input, input_flags),
            PollFd::new(engine, engine_flags),
            PollFd::new(output, output_flags),
        ];
        match poll(&mut fds, None) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
        let [input_ready, engine_ready, output_ready] = fds.map(|fd| fd.revents());

        if !input_ready.is_empty() {
            let ended = if up.len() < BUF {
                !fill(input, &mut up, &mut chunk)
            } else {
                input_ready.intersects(gone)
            };
            if !ended && !greeted {
                greeted = hello(&mut up)?;
            }
            if ended {
                if !greeted {
                    return Err(io::Error::other("the window ended before its Hello"));
                }
                drain(engine, &mut up);
                return Ok(());
            }
        }
        if !engine_ready.is_empty() {
            // Read first, so what the engine sent before it ended still reaches the window.
            let mut ended = if down.len() < BUF {
                !fill(engine, &mut down, &mut chunk)
            } else {
                engine_ready.intersects(gone)
            };
            if !ended && greeted && !up.is_empty() && engine_ready.intersects(PollFlags::OUT | gone)
            {
                ended = !send(engine, &mut up);
            }
            if ended {
                drain(output, &mut down);
                return Ok(());
            }
        }
        if !output_ready.is_empty() {
            let ended = if down.is_empty() {
                output_ready.intersects(gone)
            } else {
                !send(output, &mut down)
            };
            if ended {
                return Ok(());
            }
        }
    }
}

/// Rewrites the window's first frame at the front of `buf` once it is whole: a `Hello` on our
/// protocol gets its bridge flag set, so the engine knows the session is remote. Anything else
/// (another protocol version, a header that does not parse) stays as it is, so the engine
/// refuses it and the window shows why. `true` once the frame is ready to go on; `Err` for a
/// frame that cannot fit in `BUF`, as no `Hello` is that large.
fn hello(buf: &mut Vec<u8>) -> io::Result<bool> {
    let Some(head) = buf.first_chunk::<HEADER_LEN>() else {
        return Ok(false);
    };
    let Ok(header) = FrameHeader::parse(head) else {
        return Ok(true);
    };
    let len = HEADER_LEN + header.encoded_len as usize;
    if len > BUF {
        return Err(io::Error::other(format!(
            "the window's Hello has {len} bytes, more than {BUF}"
        )));
    }
    if buf.len() < len {
        return Ok(false);
    }
    let frame = RawFrame {
        header,
        payload: buf[HEADER_LEN..len].to_vec(),
    };
    if let Ok(Hello {
        prefix,
        tail: Some(mut tail),
    }) = decode_hello(&frame)
    {
        tail.bridge = true;
        let mut out = Vec::new();
        encode_hello(
            &Hello {
                prefix,
                tail: Some(tail),
            },
            &mut out,
        )
        .map_err(|e| io::Error::other(format!("encoding Hello: {e}")))?;
        buf.splice(..len, out);
    }
    Ok(true)
}

/// Reads what `fd` has into `buf`, up to `BUF` in total. `false` when `fd` has ended or failed.
fn fill(fd: impl AsFd, buf: &mut Vec<u8>, chunk: &mut [u8]) -> bool {
    let room = BUF.saturating_sub(buf.len()).min(chunk.len());
    match rustix::io::read(fd, &mut chunk[..room]) {
        Ok(0) => false,
        Ok(n) => {
            buf.extend_from_slice(&chunk[..n]);
            true
        }
        Err(Errno::AGAIN | Errno::INTR) => true,
        Err(_) => false,
    }
}

/// Writes what `fd` takes from the front of `buf`. `false` when the write failed.
fn send(fd: impl AsFd, buf: &mut Vec<u8>) -> bool {
    match rustix::io::write(fd, buf) {
        Ok(n) => {
            buf.drain(..n);
            true
        }
        Err(Errno::AGAIN | Errno::INTR) => true,
        Err(_) => false,
    }
}

/// Writes `buf` to `fd` until it is empty, a write fails or `DRAIN` has passed.
fn drain(fd: impl AsFd, buf: &mut Vec<u8>) {
    let until = Instant::now() + DRAIN;
    while !buf.is_empty() {
        if !send(&fd, buf) {
            return;
        }
        if buf.is_empty() {
            return;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        let Ok(left) = Timespec::try_from(left) else {
            return;
        };
        let mut fds = [PollFd::new(&fd, PollFlags::OUT)];
        match poll(&mut fds, Some(&left)) {
            Ok(0) => return,
            Ok(_) | Err(Errno::INTR) => {}
            Err(_) => return,
        }
    }
}

/// One connect to the socket within `CONNECT_DEADLINE`, with the peer's uid checked.
fn connect(socket: &Path, uid: u32) -> io::Result<UnixStream> {
    let stream = engine_link::connect_by(socket, Instant::now() + CONNECT_DEADLINE)?;
    server::check_peer(&stream, uid).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;

    use tl_model::remote_proto::{
        BootPrefix, HelloTail, HostIdentity, ServerBootstrap, Want, Welcome,
        decode_server_bootstrap, encode_welcome, read_frame,
    };
    use tl_model::time::BootId;

    /// The engine side: accepts one connection, checks that the `Hello` came through a bridge
    /// with the window's pid, answers with `Welcome` and reads until the bridge goes.
    fn engine(listener: UnixListener) -> thread::JoinHandle<HelloTail> {
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            let hello = decode_hello(&read_frame(&mut stream).expect("hello")).expect("hello");
            let tail = hello.tail.expect("a Hello on our proto");
            let mut out = Vec::new();
            encode_welcome(
                &Welcome {
                    prefix: BootPrefix::ours(),
                    tail: Some(HostIdentity {
                        host_name: Arc::from("home"),
                        machine_id: Arc::from("m1"),
                        boot_id: BootId(7),
                        engine_instance: 1,
                    }),
                },
                &mut out,
            )
            .expect("welcome");
            stream.write_all(&out).expect("send welcome");
            while read_frame(&mut stream).is_ok() {}
            tail
        })
    }

    /// The window's end of a bridge whose stdin and stdout are one socket pair.
    fn pipes() -> (UnixStream, File, File) {
        let (window, bridge) = UnixStream::pair().expect("pair");
        let input = File::from(OwnedFd::from(bridge.try_clone().expect("clone")));
        let output = File::from(OwnedFd::from(bridge));
        (window, input, output)
    }

    /// Runs the bridge on a thread; its result arrives on the receiver.
    fn spawn_bridge(
        socket: PathBuf,
        input: File,
        output: File,
        start_unit: Option<Box<dyn Fn(Duration) -> bool + Send>>,
    ) -> mpsc::Receiver<Result<(), String>> {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = run(
                input,
                output,
                &socket,
                server::own_uid(),
                start_unit
                    .as_deref()
                    .map(|f| f as &dyn Fn(Duration) -> bool),
            );
            let _ = tx.send(result);
        });
        rx
    }

    /// Sends the window's `Hello` and returns the engine's answer.
    fn handshake(window: &mut UnixStream) -> ServerBootstrap {
        window
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut out = Vec::new();
        encode_hello(
            &Hello {
                prefix: BootPrefix::ours(),
                tail: Some(HelloTail {
                    pid: 4242,
                    app_id: Arc::from("tasklens"),
                    bridge: false,
                    want: Want { snapshots: true },
                }),
            },
            &mut out,
        )
        .expect("hello");
        window.write_all(&out).expect("send hello");
        decode_server_bootstrap(&read_frame(window).expect("answer")).expect("answer")
    }

    fn assert_welcome(answer: ServerBootstrap) {
        match answer {
            ServerBootstrap::Welcome(Welcome {
                tail: Some(identity),
                ..
            }) => assert_eq!(&*identity.host_name, "home"),
            other => panic!("expected Welcome, got {other:?}"),
        }
    }

    #[test]
    fn bridge_sets_flag_and_completes_handshake() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("engine.sock");
        let engine = engine(UnixListener::bind(&socket).expect("bind"));
        let (mut window, input, output) = pipes();
        let done = spawn_bridge(
            socket,
            input,
            output,
            Some(Box::new(|_| panic!("no unit start"))),
        );
        assert_welcome(handshake(&mut window));
        // The engine reads until the bridge goes, so the window's end closes first.
        drop(window);
        let tail = engine.join().expect("engine");
        assert!(tail.bridge, "the bridge sets the flag");
        assert_eq!(tail.pid, 4242, "the rest of the Hello is unchanged");
        let result = done
            .recv_timeout(Duration::from_secs(5))
            .expect("bridge ends");
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn bridge_starts_unit_once_when_socket_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("engine.sock");
        let runs = Arc::new(AtomicU32::new(0));
        let engine_thread = Arc::new(Mutex::new(None));
        let unit = {
            let socket = socket.clone();
            let runs = Arc::clone(&runs);
            let engine_thread = Arc::clone(&engine_thread);
            move |_| {
                runs.fetch_add(1, Ordering::AcqRel);
                let listener = UnixListener::bind(&socket).expect("bind");
                *engine_thread.lock().expect("lock") = Some(engine(listener));
                true
            }
        };
        let (mut window, input, output) = pipes();
        let done = spawn_bridge(socket, input, output, Some(Box::new(unit)));
        assert_welcome(handshake(&mut window));
        drop(window);
        let result = done
            .recv_timeout(Duration::from_secs(5))
            .expect("bridge ends");
        assert_eq!(result, Ok(()));
        assert_eq!(runs.load(Ordering::Acquire), 1);
        let engine = engine_thread.lock().expect("lock").take().expect("engine");
        assert!(engine.join().expect("engine").bridge);
    }

    /// An engine that reads the `Hello`, then writes until its writes block: the window does
    /// not read, so the bridge's output, its buffer and the socket are all full. It reports
    /// that on `blocked`. With `close` it then hangs up; without, it writes on until the bridge
    /// goes.
    fn flooding_engine(
        listener: UnixListener,
        close: bool,
    ) -> (thread::JoinHandle<()>, mpsc::Receiver<()>) {
        let (blocked_tx, blocked) = mpsc::channel();
        let engine = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            read_frame(&mut stream).expect("hello");
            stream
                .set_write_timeout(Some(Duration::from_millis(100)))
                .expect("write timeout");
            let junk = vec![0x5a_u8; 16 * 1024];
            let until = Instant::now() + Duration::from_secs(20);
            let mut told = false;
            while Instant::now() < until {
                match stream.write(&junk) {
                    Ok(_) => {}
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        if !told {
                            told = true;
                            let _ = blocked_tx.send(());
                        }
                        if close {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });
        (engine, blocked)
    }

    /// The window's two ends: it writes the bridge's input and never reads its output.
    fn split_pipes() -> (UnixStream, UnixStream, File, File) {
        let (window_in, bridge_in) = UnixStream::pair().expect("pair");
        let (bridge_out, window_out) = UnixStream::pair().expect("pair");
        (
            window_in,
            window_out,
            File::from(OwnedFd::from(bridge_in)),
            File::from(OwnedFd::from(bridge_out)),
        )
    }

    fn send_hello(window: &mut UnixStream) {
        let mut out = Vec::new();
        encode_hello(
            &Hello {
                prefix: BootPrefix::ours(),
                tail: Some(HelloTail {
                    pid: 4242,
                    app_id: Arc::from("tasklens"),
                    bridge: false,
                    want: Want { snapshots: true },
                }),
            },
            &mut out,
        )
        .expect("hello");
        window.write_all(&out).expect("send hello");
    }

    /// The review case: the window stops reading, the engine fills the output, then the
    /// window's input ends. The bridge must not stay blocked on the full output.
    #[test]
    fn bridge_returns_when_window_ends_while_output_is_full() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("engine.sock");
        let (engine, blocked) = flooding_engine(UnixListener::bind(&socket).expect("bind"), false);
        let (mut window_in, window_out, input, output) = split_pipes();
        let done = spawn_bridge(socket, input, output, None);
        send_hello(&mut window_in);
        blocked
            .recv_timeout(Duration::from_secs(10))
            .expect("the output fills");
        drop(window_in);
        let result = done
            .recv_timeout(Duration::from_secs(1))
            .expect("the bridge returns within 1 s");
        assert_eq!(result, Ok(()));
        // The bridge closed the socket, so the engine's next write fails.
        engine.join().expect("engine");
        drop(window_out);
    }

    /// The other way round: the window stops reading, the engine fills the output and hangs
    /// up. The bridge gives the window `DRAIN` to take the rest, then returns.
    #[test]
    fn bridge_returns_when_engine_ends_while_output_is_full() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("engine.sock");
        let (engine, blocked) = flooding_engine(UnixListener::bind(&socket).expect("bind"), true);
        let (mut window_in, mut window_out, input, output) = split_pipes();
        let done = spawn_bridge(socket, input, output, None);
        send_hello(&mut window_in);
        blocked
            .recv_timeout(Duration::from_secs(10))
            .expect("the output fills");
        engine.join().expect("engine");
        let result = done
            .recv_timeout(Duration::from_secs(1))
            .expect("the bridge returns within 1 s");
        assert_eq!(result, Ok(()));
        // The window still gets what the bridge passed on before it ended.
        window_out
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("read timeout");
        let mut first = [0u8; 4];
        window_out.read_exact(&mut first).expect("bytes");
        assert_eq!(first, [0x5a; 4]);
        drop(window_in);
    }

    /// The window has not sent its `Hello` when the engine hangs up (as at its 5 s `Hello`
    /// deadline). The bridge must not wait on stdin for a frame nobody will take.
    #[test]
    fn bridge_returns_when_engine_ends_before_hello() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("engine.sock");
        let listener = UnixListener::bind(&socket).expect("bind");
        let (accepted_tx, accepted) = mpsc::channel();
        let engine = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let _ = accepted_tx.send(());
            // Holds the connection a moment, then hangs up without a Hello.
            thread::sleep(Duration::from_millis(100));
            drop(stream);
        });
        let (window_in, window_out, input, output) = split_pipes();
        let done = spawn_bridge(socket, input, output, None);
        accepted
            .recv_timeout(Duration::from_secs(5))
            .expect("the bridge connects");
        engine.join().expect("engine");
        let result = done
            .recv_timeout(Duration::from_secs(1))
            .expect("the bridge returns within 1 s");
        assert_eq!(result, Ok(()));
        drop((window_in, window_out));
    }

    /// The window stops reading the bridge's stdout before it sends its `Hello`. The bridge
    /// must see that while it waits for the `Hello`.
    #[test]
    fn bridge_returns_when_output_closes_before_hello() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("engine.sock");
        let listener = UnixListener::bind(&socket).expect("bind");
        let (accepted_tx, accepted) = mpsc::channel();
        let engine = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = accepted_tx.send(());
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            // Ends when the bridge closes the socket.
            let mut buf = [0u8; 64];
            while matches!(stream.read(&mut buf), Ok(n) if n > 0) {}
        });
        let (window_in, window_out, input, output) = split_pipes();
        let done = spawn_bridge(socket, input, output, None);
        accepted
            .recv_timeout(Duration::from_secs(5))
            .expect("the bridge connects");
        drop(window_out);
        let result = done
            .recv_timeout(Duration::from_secs(1))
            .expect("the bridge returns within 1 s");
        assert_eq!(result, Ok(()));
        engine.join().expect("engine");
        drop(window_in);
    }

    #[test]
    fn bridge_fails_after_one_retry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("engine.sock");
        let runs = Arc::new(AtomicU32::new(0));
        let unit = {
            let runs = Arc::clone(&runs);
            move |_| {
                runs.fetch_add(1, Ordering::AcqRel);
                false
            }
        };
        let (_window, input, output) = pipes();
        let done = spawn_bridge(socket, input, output, Some(Box::new(unit)));
        let result = done
            .recv_timeout(Duration::from_secs(10))
            .expect("bridge ends");
        let err = result.expect_err("no engine");
        assert!(err.starts_with("cannot connect to "), "{err}");
        assert_eq!(runs.load(Ordering::Acquire), 1);
    }
}
