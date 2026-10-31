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
use std::io::{self, Write};
use std::net::Shutdown;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use tl_model::remote_proto::{Hello, decode_hello, encode_hello, read_frame};
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
    mut input: File,
    mut output: File,
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
    let frame = read_frame(&mut input).map_err(|e| format!("reading the window's Hello: {e}"))?;
    let mut first = Vec::new();
    match decode_hello(&frame) {
        Ok(Hello {
            prefix,
            tail: Some(mut tail),
        }) => {
            tail.bridge = true;
            encode_hello(
                &Hello {
                    prefix,
                    tail: Some(tail),
                },
                &mut first,
            )
            .map_err(|e| format!("encoding Hello: {e}"))?;
        }
        _ => {
            first.extend_from_slice(&frame.header.to_bytes());
            first.extend_from_slice(&frame.payload);
        }
    }
    (&stream)
        .write_all(&first)
        .map_err(|e| format!("sending Hello to the engine: {e}"))?;

    let up = stream
        .try_clone()
        .map_err(|e| format!("engine socket: {e}"))?;
    thread::Builder::new()
        .name("tl-bridge-up".into())
        .spawn(move || {
            let _ = io::copy(&mut input, &mut &up);
            // Ends the copy below too.
            let _ = up.shutdown(Shutdown::Both);
        })
        .map_err(|e| format!("bridge thread: {e}"))?;
    let _ = io::copy(&mut &stream, &mut output);
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
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
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, mpsc};

    use tl_model::remote_proto::{
        BootPrefix, HelloTail, HostIdentity, ServerBootstrap, Want, Welcome,
        decode_server_bootstrap, encode_welcome,
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
