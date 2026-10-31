//! The engine service's endpoint (ADR-018 Local transport, Implementation contracts
//! "Endpoints"): the runtime directory and socket directory checks, binding the socket with
//! stale-socket handling, the `SO_PEERCRED` check on each connection, and the bootstrap
//! handshake. The window's client reuses the directory and peer checks (q4x76.8). After the
//! handshake, `serve_session` (module `session`) runs the session's wire side.
//!
//! Every check names the path it refused. Nothing here fixes a wrong owner or mode: an existing
//! directory that is wrong is an error.

use std::ffi::OsStr;
use std::fmt;
use std::fs::DirBuilder;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tl_model::remote_proto::{
    BootPrefix, HelloTail, HostIdentity, RefuseReason, Refused, Welcome, decode_hello,
    encode_refused, encode_welcome,
};

use crate::envelope::{EnvelopeError, FrameReader};

mod session;

#[cfg(feature = "test-hooks")]
pub use session::Step;
pub use session::{
    ACK_TIMEOUT, ACTION_QUEUE, Item, JOURNAL_BATCHES_MAX, JOURNAL_BYTES_MAX, JOURNAL_STREAMS_MAX,
    Port, REPLIES_MAX, REPLY_BYTES_MAX, SNAPSHOT_FLOOR, SessionConfig, SessionEnd, SessionReport,
    WRITE_TIMEOUT, serve_session,
};

/// The socket's file name in the socket directory.
pub const SOCKET_NAME: &str = "engine.sock";
/// The only mode the runtime directory and the socket directory may have.
pub const DIR_MODE: u32 = 0o700;
/// How long a connection may take for its whole handshake, from accept. One deadline for all
/// reads, so a client that sends its `Hello` a byte at a time cannot hold its place longer.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Why an endpoint check, a bind or a peer check failed.
#[derive(Debug)]
pub enum EndpointError {
    /// `XDG_RUNTIME_DIR` is not set or is empty.
    RuntimeDirUnset,
    /// `XDG_RUNTIME_DIR` is not an absolute path.
    RuntimeDirRelative(PathBuf),
    NotADirectory(PathBuf),
    /// The socket directory is a symlink.
    Symlink(PathBuf),
    WrongOwner {
        path: PathBuf,
        uid: u32,
        expected: u32,
    },
    WrongMode {
        path: PathBuf,
        mode: u32,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// Another process answers on the socket.
    InUse(PathBuf),
    /// The socket path holds something that is not a socket of this user, so it is left alone.
    NotOurSocket(PathBuf),
    /// The peer runs as another user; its connection was closed.
    PeerUid {
        uid: u32,
        expected: u32,
    },
    /// `accept` or `SO_PEERCRED` failed.
    Accept(io::Error),
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeDirUnset => write!(f, "XDG_RUNTIME_DIR is not set"),
            Self::RuntimeDirRelative(path) => write!(
                f,
                "XDG_RUNTIME_DIR {} is not an absolute path",
                path.display()
            ),
            Self::NotADirectory(path) => write!(f, "{} is not a directory", path.display()),
            Self::Symlink(path) => write!(f, "{} is a symlink", path.display()),
            Self::WrongOwner {
                path,
                uid,
                expected,
            } => write!(
                f,
                "{} is owned by uid {uid}, not by uid {expected}",
                path.display()
            ),
            Self::WrongMode { path, mode } => write!(
                f,
                "{} has mode {mode:04o}, not {DIR_MODE:04o}",
                path.display()
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::InUse(path) => write!(f, "another engine answers on {}", path.display()),
            Self::NotOurSocket(path) => write!(
                f,
                "{} exists and is not a socket of this user",
                path.display()
            ),
            Self::PeerUid { uid, expected } => {
                write!(f, "peer runs as uid {uid}, not as uid {expected}")
            }
            Self::Accept(e) => write!(f, "accept: {e}"),
        }
    }
}

impl std::error::Error for EndpointError {}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> EndpointError + '_ {
    move |source| EndpointError::Io {
        path: path.to_owned(),
        source,
    }
}

/// The real uid of this process.
pub fn own_uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// Checks that `meta`, read from `path`, is a directory owned by `uid` with mode 0700.
fn check_dir(path: &Path, meta: &std::fs::Metadata, uid: u32) -> Result<(), EndpointError> {
    if meta.file_type().is_symlink() {
        return Err(EndpointError::Symlink(path.to_owned()));
    }
    if !meta.is_dir() {
        return Err(EndpointError::NotADirectory(path.to_owned()));
    }
    if meta.uid() != uid {
        return Err(EndpointError::WrongOwner {
            path: path.to_owned(),
            uid: meta.uid(),
            expected: uid,
        });
    }
    let mode = meta.mode() & 0o7777;
    if mode != DIR_MODE {
        return Err(EndpointError::WrongMode {
            path: path.to_owned(),
            mode,
        });
    }
    Ok(())
}

/// The value of `XDG_RUNTIME_DIR`, checked: an absolute path to a directory owned by `uid`
/// with mode 0700.
pub fn runtime_dir(value: Option<&OsStr>, uid: u32) -> Result<PathBuf, EndpointError> {
    let path = match value {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => return Err(EndpointError::RuntimeDirUnset),
    };
    if !path.is_absolute() {
        return Err(EndpointError::RuntimeDirRelative(path));
    }
    let meta = std::fs::metadata(&path).map_err(io_error(&path))?;
    check_dir(&path, &meta, uid)?;
    Ok(path)
}

/// `runtime/app_dir`, created with mode 0700 when it is missing. It is checked with `lstat`: a
/// directory, not a symlink, owned by `uid`, mode 0700.
pub fn socket_dir(runtime: &Path, app_dir: &str, uid: u32) -> Result<PathBuf, EndpointError> {
    let path = runtime.join(app_dir);
    match DirBuilder::new().mode(DIR_MODE).create(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(io_error(&path)(e)),
    }
    let meta = std::fs::symlink_metadata(&path).map_err(io_error(&path))?;
    check_dir(&path, &meta, uid)?;
    Ok(path)
}

/// Binds the engine socket at `path`. Call it only while holding the engine lock: an existing
/// path is then removed only when it is a socket owned by `uid` and connecting to it is refused,
/// so no engine is listening there. Anything else at the path is an error and stays.
pub fn bind(path: &Path, uid: u32) -> Result<UnixListener, EndpointError> {
    match UnixListener::bind(path) {
        Ok(listener) => return Ok(listener),
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {}
        Err(e) => return Err(io_error(path)(e)),
    }
    let meta = std::fs::symlink_metadata(path).map_err(io_error(path))?;
    if !meta.file_type().is_socket() || meta.uid() != uid {
        return Err(EndpointError::NotOurSocket(path.to_owned()));
    }
    match UnixStream::connect(path) {
        Ok(_) => return Err(EndpointError::InUse(path.to_owned())),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {}
        Err(e) => return Err(io_error(path)(e)),
    }
    std::fs::remove_file(path).map_err(io_error(path))?;
    UnixListener::bind(path).map_err(io_error(path))
}

/// The uid of the process at the other end of `stream`, from `SO_PEERCRED`.
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    Ok(rustix::net::sockopt::socket_peercred(stream)?.uid.as_raw())
}

/// Checks that the peer of `stream` runs as `uid`.
pub fn check_peer(stream: &UnixStream, uid: u32) -> Result<(), EndpointError> {
    let peer = peer_uid(stream).map_err(EndpointError::Accept)?;
    if peer != uid {
        return Err(EndpointError::PeerUid {
            uid: peer,
            expected: uid,
        });
    }
    Ok(())
}

/// Accepts one connection and checks its peer. A peer that does not run as `uid` is closed at
/// once and returned as `Err(PeerUid)`; the caller logs it and accepts the next one.
pub fn accept_peer(listener: &UnixListener, uid: u32) -> Result<UnixStream, EndpointError> {
    let (stream, _) = listener.accept().map_err(EndpointError::Accept)?;
    match check_peer(&stream, uid) {
        Ok(()) => Ok(stream),
        Err(e) => {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            Err(e)
        }
    }
}

/// Reads from and writes to a stream until one fixed `deadline` (a monotonic `Instant`). Before
/// every read or write it checks the clock and sets the stream's timeout for it to the time
/// left; a read or write at or after the deadline fails with `TimedOut`. A handshake uses one
/// for its reads and one for its answer, so neither a slow `Hello` nor a client that does not
/// read its answer keeps the handshake past the deadline.
pub struct DeadlineStream<'a> {
    stream: &'a UnixStream,
    deadline: Instant,
}

impl<'a> DeadlineStream<'a> {
    pub fn new(stream: &'a UnixStream, deadline: Instant) -> Self {
        Self { stream, deadline }
    }

    /// The time left, or `TimedOut` when there is none.
    fn left(&self) -> io::Result<Duration> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the handshake deadline passed",
            ));
        }
        Ok(left)
    }
}

impl Read for DeadlineStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.left()?))?;
        let mut stream = self.stream;
        stream.read(buf)
    }
}

impl Write for DeadlineStream<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.left()?))?;
        let mut stream = self.stream;
        stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut stream = self.stream;
        stream.flush()
    }
}

/// Why a handshake ended without a session.
#[derive(Debug)]
pub enum HandshakeError {
    /// No whole `Hello` frame arrived: end of stream, timeout or a bad envelope.
    Read(EnvelopeError),
    /// The `Hello` did not decode. `Refused(BadHello)` was sent.
    BadHello,
    /// The client speaks another protocol. `Refused(Proto)` was sent.
    Proto {
        proto: u16,
        app_version: Arc<str>,
    },
    /// The server refused the session. `Refused` with this reason was sent.
    Refused(RefuseReason),
    Write(io::Error),
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(e) => write!(f, "reading Hello: {e}"),
            Self::BadHello => write!(f, "the Hello could not be read"),
            Self::Proto { proto, app_version } => write!(
                f,
                "the client runs tasklens {app_version} (protocol {proto})"
            ),
            Self::Refused(reason) => write!(f, "refused: {reason:?}"),
            Self::Write(e) => write!(f, "writing the answer: {e}"),
        }
    }
}

impl std::error::Error for HandshakeError {}

/// A `Refused` with this build's prefix.
pub fn refused(reason: RefuseReason, text: impl Into<Arc<str>>) -> Refused {
    Refused {
        prefix: BootPrefix::ours(),
        reason,
        text: text.into(),
    }
}

fn send_refused<W: Write>(w: &mut W, refused: &Refused) -> Result<(), HandshakeError> {
    let mut out = Vec::new();
    // A `Refused` with our own prefix and a short text always encodes.
    let _ = encode_refused(refused, &mut out);
    w.write_all(&out)
        .and_then(|()| w.flush())
        .map_err(HandshakeError::Write)
}

/// Reads the client's `Hello` and answers it. `accept` runs only for a `Hello` on this
/// protocol: it returns who the engine is plus a value for the caller (the session), or a
/// `Refused`. Returns the client's `Hello` tail with that value once `Welcome` is sent.
pub fn handshake<R: Read, W: Write, T>(
    r: &mut R,
    w: &mut W,
    frames: &FrameReader,
    accept: impl FnOnce(&HelloTail) -> Result<(HostIdentity, T), Refused>,
) -> Result<(HelloTail, T), HandshakeError> {
    let frame = frames.read_frame(r).map_err(HandshakeError::Read)?;
    let hello = match decode_hello(&frame) {
        Ok(hello) => hello,
        Err(_) => {
            send_refused(
                w,
                &refused(RefuseReason::BadHello, "the Hello could not be read"),
            )?;
            return Err(HandshakeError::BadHello);
        }
    };
    let Some(tail) = hello.tail else {
        send_refused(w, &Refused::proto_mismatch(&hello.prefix))?;
        return Err(HandshakeError::Proto {
            proto: hello.prefix.proto,
            app_version: hello.prefix.app_version,
        });
    };
    match accept(&tail) {
        Ok((identity, value)) => {
            let mut out = Vec::new();
            let welcome = Welcome {
                prefix: BootPrefix::ours(),
                tail: Some(identity),
            };
            encode_welcome(&welcome, &mut out)
                .map_err(|e| HandshakeError::Write(io::Error::other(e.to_string())))?;
            w.write_all(&out)
                .and_then(|()| w.flush())
                .map_err(HandshakeError::Write)?;
            Ok((tail, value))
        }
        Err(answer) => {
            let reason = answer.reason;
            send_refused(w, &answer)?;
            Err(HandshakeError::Refused(reason))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    use std::fs::Permissions;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;

    use tl_model::remote_proto::{
        Hello, PROTO, ServerBootstrap, Want, decode_server_bootstrap, encode_hello,
    };
    use tl_model::time::BootId;

    fn private_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn runtime_dir_must_be_absolute_private_and_ours() {
        let uid = own_uid();
        assert!(matches!(
            runtime_dir(None, uid),
            Err(EndpointError::RuntimeDirUnset)
        ));
        assert!(matches!(
            runtime_dir(Some(OsStr::new("")), uid),
            Err(EndpointError::RuntimeDirUnset)
        ));
        assert!(matches!(
            runtime_dir(Some(OsStr::new("run/user/1000")), uid),
            Err(EndpointError::RuntimeDirRelative(_))
        ));

        let dir = private_dir();
        let value = Some(dir.path().as_os_str());
        assert_eq!(runtime_dir(value, uid).unwrap(), dir.path());
        assert!(matches!(
            runtime_dir(value, uid + 1),
            Err(EndpointError::WrongOwner { .. })
        ));
        std::fs::set_permissions(dir.path(), Permissions::from_mode(0o755)).unwrap();
        let err = runtime_dir(value, uid).unwrap_err();
        assert!(
            matches!(err, EndpointError::WrongMode { mode: 0o755, .. }),
            "{err}"
        );
        assert!(err.to_string().contains(&*dir.path().to_string_lossy()));

        let file = dir.path().join("file");
        std::fs::write(&file, b"").unwrap();
        assert!(matches!(
            runtime_dir(Some(file.as_os_str()), uid),
            Err(EndpointError::NotADirectory(_))
        ));
    }

    #[test]
    fn socket_dir_is_created_private_and_never_fixed() {
        let uid = own_uid();
        let runtime = private_dir();
        let made = socket_dir(runtime.path(), "tasklens", uid).unwrap();
        let meta = std::fs::symlink_metadata(&made).unwrap();
        assert_eq!(meta.mode() & 0o7777, DIR_MODE);
        // An existing good one is fine.
        assert_eq!(socket_dir(runtime.path(), "tasklens", uid).unwrap(), made);

        std::fs::set_permissions(&made, Permissions::from_mode(0o750)).unwrap();
        assert!(matches!(
            socket_dir(runtime.path(), "tasklens", uid),
            Err(EndpointError::WrongMode { mode: 0o750, .. })
        ));
        let meta = std::fs::symlink_metadata(&made).unwrap();
        assert_eq!(meta.mode() & 0o7777, 0o750, "a wrong mode is not fixed");

        let target = private_dir();
        std::os::unix::fs::symlink(target.path(), runtime.path().join("linked")).unwrap();
        assert!(matches!(
            socket_dir(runtime.path(), "linked", uid),
            Err(EndpointError::Symlink(_))
        ));
    }

    #[test]
    fn a_stale_socket_is_replaced() {
        let uid = own_uid();
        let dir = private_dir();
        let path = dir.path().join(SOCKET_NAME);
        // std does not remove the file when a listener drops, which leaves a stale socket.
        drop(UnixListener::bind(&path).unwrap());
        assert!(UnixStream::connect(&path).is_err());

        let listener = bind(&path, uid).unwrap();
        let _client = UnixStream::connect(&path).unwrap();
        listener.accept().unwrap();
    }

    #[test]
    fn a_live_socket_is_left_alone() {
        let uid = own_uid();
        let dir = private_dir();
        let path = dir.path().join(SOCKET_NAME);
        let live = UnixListener::bind(&path).unwrap();
        assert!(matches!(bind(&path, uid), Err(EndpointError::InUse(_))));
        // The first engine still answers on its socket.
        let _client = UnixStream::connect(&path).unwrap();
        live.accept().unwrap();
    }

    #[test]
    fn a_path_that_is_not_our_socket_is_left_alone() {
        let uid = own_uid();
        let dir = private_dir();
        let path = dir.path().join(SOCKET_NAME);
        std::fs::write(&path, b"keep").unwrap();
        assert!(matches!(
            bind(&path, uid),
            Err(EndpointError::NotOurSocket(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");

        // A stale socket of another user is not removed either.
        std::fs::remove_file(&path).unwrap();
        drop(UnixListener::bind(&path).unwrap());
        assert!(matches!(
            bind(&path, uid + 1),
            Err(EndpointError::NotOurSocket(_))
        ));
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_socket()
        );
    }

    #[test]
    fn a_peer_of_another_user_is_refused_and_closed() {
        let uid = own_uid();
        let dir = private_dir();
        let path = dir.path().join(SOCKET_NAME);
        let listener = bind(&path, uid).unwrap();

        let mut client = UnixStream::connect(&path).unwrap();
        // The test runs as one user, so the server pretends to be another one.
        let err = accept_peer(&listener, uid + 1).unwrap_err();
        assert!(
            matches!(err, EndpointError::PeerUid { uid: peer, expected } if peer == uid && expected == uid + 1),
            "{err}"
        );
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buf = [0u8; 1];
        assert_eq!(client.read(&mut buf).unwrap(), 0, "the server closed it");

        let _ok = UnixStream::connect(&path).unwrap();
        let stream = accept_peer(&listener, uid).unwrap();
        assert_eq!(peer_uid(&stream).unwrap(), uid);
    }

    fn hello_bytes(proto: u16) -> Vec<u8> {
        let mut out = Vec::new();
        let tail = HelloTail {
            pid: 42,
            app_id: Arc::from("tasklens"),
            bridge: false,
            want: Want { snapshots: true },
        };
        let prefix = BootPrefix {
            proto,
            ..BootPrefix::ours()
        };
        // Another proto has no tail the server reads, but the prefix is the same.
        encode_hello(
            &Hello {
                prefix,
                tail: Some(tail),
            },
            &mut out,
        )
        .unwrap();
        out
    }

    fn identity() -> HostIdentity {
        HostIdentity {
            host_name: Arc::from("home"),
            machine_id: Arc::from("0123"),
            boot_id: BootId(7),
            engine_instance: 99,
        }
    }

    fn answer(written: &[u8]) -> ServerBootstrap {
        let frames = FrameReader::new().unwrap();
        let frame = frames.read_frame(&mut Cursor::new(written)).unwrap();
        decode_server_bootstrap(&frame).unwrap()
    }

    #[test]
    fn handshake_welcomes_a_hello_on_our_protocol() {
        let frames = FrameReader::new().unwrap();
        let mut out = Vec::new();
        let (tail, value) = handshake(
            &mut Cursor::new(hello_bytes(PROTO)),
            &mut out,
            &frames,
            |tail| Ok((identity(), tail.pid)),
        )
        .unwrap();
        assert_eq!((tail.pid, value), (42, 42));
        match answer(&out) {
            ServerBootstrap::Welcome(w) => assert_eq!(w.tail, Some(identity())),
            other => panic!("expected Welcome, got {other:?}"),
        }
    }

    #[test]
    fn handshake_refuses_another_protocol_and_a_full_server() {
        let frames = FrameReader::new().unwrap();
        let mut out = Vec::new();
        let err = handshake(
            &mut Cursor::new(hello_bytes(PROTO + 1)),
            &mut out,
            &frames,
            |_| -> Result<(HostIdentity, ()), Refused> { panic!("not called for another proto") },
        )
        .unwrap_err();
        assert!(matches!(err, HandshakeError::Proto { proto, .. } if proto == PROTO + 1));
        assert!(matches!(
            answer(&out),
            ServerBootstrap::Refused(Refused {
                reason: RefuseReason::Proto,
                ..
            })
        ));

        let mut out = Vec::new();
        let err = handshake(
            &mut Cursor::new(hello_bytes(PROTO)),
            &mut out,
            &frames,
            |_| -> Result<(HostIdentity, ()), Refused> {
                Err(refused(
                    RefuseReason::TooManySessions,
                    "8 windows are connected",
                ))
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            HandshakeError::Refused(RefuseReason::TooManySessions)
        ));
        assert!(matches!(
            answer(&out),
            ServerBootstrap::Refused(Refused {
                reason: RefuseReason::TooManySessions,
                ..
            })
        ));
    }

    #[test]
    fn handshake_without_a_hello_sends_nothing() {
        let frames = FrameReader::new().unwrap();
        let mut out = Vec::new();
        let err = handshake(
            &mut Cursor::new(Vec::<u8>::new()),
            &mut out,
            &frames,
            |_| Ok((identity(), ())),
        )
        .unwrap_err();
        assert!(matches!(err, HandshakeError::Read(_)));
        assert!(out.is_empty());
    }

    #[test]
    fn a_trickled_hello_fails_at_the_deadline() {
        let (server, client) = UnixStream::pair().unwrap();
        let hello = hello_bytes(BootPrefix::ours().proto);
        // One byte every 50 ms: each read gets a byte well within any per-read timeout.
        let writer = std::thread::spawn(move || {
            let mut client = client;
            for byte in hello {
                if client.write_all(&[byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let begin = Instant::now();
        let mut reader = DeadlineStream::new(&server, begin + Duration::from_millis(300));
        let frames = FrameReader::new().unwrap();
        let mut out = Vec::new();
        let err = handshake(&mut reader, &mut out, &frames, |_| Ok((identity(), ()))).unwrap_err();
        let elapsed = begin.elapsed();
        assert!(matches!(err, HandshakeError::Read(_)), "{err}");
        assert!(out.is_empty());
        assert!(
            elapsed >= Duration::from_millis(300) && elapsed < Duration::from_secs(2),
            "{elapsed:?}"
        );
        drop(server);
        writer.join().unwrap();
    }

    #[test]
    fn an_answer_ready_after_the_deadline_is_not_sent() {
        let (server, client) = UnixStream::pair().unwrap();
        (&client)
            .write_all(&hello_bytes(BootPrefix::ours().proto))
            .unwrap();
        let begin = Instant::now();
        let deadline = begin + Duration::from_millis(200);
        let mut reader = DeadlineStream::new(&server, deadline);
        let mut writer = DeadlineStream::new(&server, deadline);
        let frames = FrameReader::new().unwrap();
        // The Hello arrives in time, but the session is admitted only after the deadline.
        let err = handshake(&mut reader, &mut writer, &frames, |_| {
            std::thread::sleep(Duration::from_millis(300));
            Ok((identity(), ()))
        })
        .unwrap_err();
        assert!(
            matches!(&err, HandshakeError::Write(e) if e.kind() == io::ErrorKind::TimedOut),
            "{err}"
        );
        drop(server);
        let mut sent = Vec::new();
        (&client).read_to_end(&mut sent).unwrap();
        assert!(sent.is_empty(), "no Welcome after the deadline");
    }

    #[test]
    fn a_write_to_a_client_that_does_not_read_ends_at_the_deadline() {
        let (server, _client) = UnixStream::pair().unwrap();
        let begin = Instant::now();
        let mut writer = DeadlineStream::new(&server, begin + Duration::from_millis(300));
        // Far more than the socket buffer holds, so the write blocks once it is full.
        let err = writer.write_all(&vec![0u8; 64 << 20]).unwrap_err();
        let elapsed = begin.elapsed();
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ),
            "{err}"
        );
        assert!(
            elapsed >= Duration::from_millis(300) && elapsed < Duration::from_secs(2),
            "{elapsed:?}"
        );
    }
}
