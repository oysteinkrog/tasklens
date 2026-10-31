//! `tasklens-engine` as a process (ADR-018): one engine per user, the runtime directory check,
//! and a full start with `READY=1`, handshakes up to 8 sessions and a SIGTERM stop.
//!
//! Every directory is a temporary one. The session bus address points nowhere, so the engine
//! under test never reaches the desktop session (KWin bridge, notifications).

#![allow(clippy::unwrap_used)]

use std::fs::{File, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tl_model::remote_proto::{
    BootPrefix, Hello, HelloTail, RefuseReason, ServerBootstrap, Want, decode_server_bootstrap,
    encode_hello,
};
use tl_remote::envelope::FrameReader;

const BIN: &str = env!("CARGO_BIN_EXE_tasklens-engine");

/// Temporary XDG roots: state, runtime (mode 0700) and home.
struct Dirs {
    _tmp: tempfile::TempDir,
    state: PathBuf,
    runtime: PathBuf,
    home: PathBuf,
}

impl Dirs {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let runtime = tmp.path().join("run");
        let home = tmp.path().join("home");
        for dir in [&state, &runtime, &home] {
            std::fs::create_dir(dir).unwrap();
        }
        std::fs::set_permissions(&runtime, Permissions::from_mode(0o700)).unwrap();
        Dirs {
            _tmp: tmp,
            state,
            runtime,
            home,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.env_clear()
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", &self.state)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent/tasklens-test-bus",
            )
            .env("TASKLENS_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd
    }

    fn socket(&self) -> PathBuf {
        self.runtime.join("tasklens/engine.sock")
    }
}

/// Kills the child if the test ends early. Only children this test started.
struct Guard(Option<Child>);

impl Guard {
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }

    /// Waits up to `limit` for the exit, then returns the status and stderr.
    fn wait(&mut self, limit: Duration) -> (ExitStatus, String) {
        let begin = Instant::now();
        while self.child().try_wait().unwrap().is_none() {
            assert!(
                begin.elapsed() < limit,
                "tasklens-engine did not exit within {limit:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut child = self.0.take().unwrap();
        let mut err = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        // It has exited, so this returns at once.
        let status = child.wait().unwrap();
        (status, err)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn hold_lock(state: &Path) -> File {
    let dir = state.join("tasklens");
    std::fs::create_dir_all(&dir).unwrap();
    let file = File::create(dir.join("engine.lock")).unwrap();
    file.try_lock().unwrap();
    file
}

#[test]
fn second_engine_exits_when_the_lock_is_held() {
    let dirs = Dirs::new();
    let _held = hold_lock(&dirs.state);
    let mut child = Guard(Some(dirs.command().spawn().unwrap()));
    let (status, err) = child.wait(Duration::from_secs(10));
    assert!(status.success(), "{status:?}: {err}");
    assert!(err.contains("engine already running in a window"), "{err}");
    assert!(
        !dirs.runtime.join("tasklens").exists(),
        "it touched no socket"
    );
}

#[test]
fn a_shared_runtime_dir_is_refused_by_name() {
    let dirs = Dirs::new();
    std::fs::set_permissions(&dirs.runtime, Permissions::from_mode(0o755)).unwrap();
    let mut child = Guard(Some(dirs.command().spawn().unwrap()));
    let (status, err) = child.wait(Duration::from_secs(10));
    assert_eq!(status.code(), Some(1), "{err}");
    assert!(err.contains(&*dirs.runtime.to_string_lossy()), "{err}");
    assert!(!dirs.runtime.join("tasklens").exists());
    // It released the lock on the way out.
    hold_lock(&dirs.state);
}

/// Connects and sends `Hello`; returns the stream and the server's answer.
fn connect(socket: &Path) -> (UnixStream, ServerBootstrap) {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut out = Vec::new();
    encode_hello(
        &Hello {
            prefix: BootPrefix::ours(),
            tail: Some(HelloTail {
                pid: std::process::id(),
                app_id: Arc::from("tasklens-test"),
                bridge: false,
                want: Want { snapshots: true },
            }),
        },
        &mut out,
    )
    .unwrap();
    stream.write_all(&out).unwrap();
    let frames = FrameReader::new().unwrap();
    let frame = frames.read_frame(&mut stream).unwrap();
    (stream, decode_server_bootstrap(&frame).unwrap())
}

#[test]
fn service_starts_serves_eight_sessions_and_stops_on_sigterm() {
    let dirs = Dirs::new();
    let notify_path = dirs.runtime.join("notify.sock");
    let notify = UnixDatagram::bind(&notify_path).unwrap();
    notify
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let mut cmd = dirs.command();
    cmd.env("NOTIFY_SOCKET", &notify_path);
    let mut child = Guard(Some(cmd.spawn().unwrap()));

    let mut buf = [0u8; 256];
    let n = notify.recv(&mut buf).expect("READY=1 within 20 s");
    assert_eq!(std::str::from_utf8(&buf[..n]).unwrap(), "READY=1\n");
    // The socket is bound before READY=1.
    let socket = dirs.socket();
    assert!(socket.exists());

    let mut sessions = Vec::new();
    let mut instance = None;
    for _ in 0..8 {
        let (stream, answer) = connect(&socket);
        match answer {
            ServerBootstrap::Welcome(w) => {
                let id = w.tail.expect("same proto").engine_instance;
                assert_eq!(*instance.get_or_insert(id), id, "one engine instance");
            }
            ServerBootstrap::Refused(r) => panic!("refused: {}", r.text),
        }
        sessions.push(stream);
    }
    let (_ninth, answer) = connect(&socket);
    match answer {
        ServerBootstrap::Refused(r) => assert_eq!(r.reason, RefuseReason::TooManySessions),
        ServerBootstrap::Welcome(_) => panic!("a ninth session was welcomed"),
    }

    // A closed connection frees its slot once the reducer has seen the port go.
    drop(sessions.pop());
    let begin = Instant::now();
    loop {
        match connect(&socket) {
            (stream, ServerBootstrap::Welcome(_)) => {
                sessions.push(stream);
                break;
            }
            (_, ServerBootstrap::Refused(r)) => {
                assert_eq!(r.reason, RefuseReason::TooManySessions);
                assert!(
                    begin.elapsed() < Duration::from_secs(5),
                    "the closed session's slot did not free"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    let pid = child.child().id().to_string();
    let killed = Command::new("kill").args(["-TERM", &pid]).status().unwrap();
    assert!(killed.success());
    let n = notify.recv(&mut buf).expect("STOPPING=1");
    assert_eq!(std::str::from_utf8(&buf[..n]).unwrap(), "STOPPING=1\n");
    let (status, err) = child.wait(Duration::from_secs(5));
    assert!(status.success(), "{status:?}: {err}");
    assert!(!socket.exists(), "the socket is removed on the way out");
    // Connections still open see the end of the stream.
    let mut byte = [0u8; 1];
    assert_eq!(sessions[0].read(&mut byte).unwrap_or(0), 0);
}
