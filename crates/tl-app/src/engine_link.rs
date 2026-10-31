//! The window's engine: the engine service, or an in-process engine when the service does not
//! answer (ADR-018 Window fallback, Lifetime and "One engine per user per machine",
//! tasklens-q4x76.8).
//!
//! The service answers when its `Welcome` arrives on a connection. A socket that accepts the
//! connection and sends nothing does not count.
//!
//! At start (`EngineLink::open`):
//! 1. When the socket accepts a connection at once and the service answers within 3 s of start,
//!    the window becomes a `RemoteClient` of the service.
//! 2. Otherwise tl-app runs `systemctl --user start tasklens-engine.service`, which gets at most
//!    3 s. When it succeeds, the socket is tried every 50 ms, and the service must answer within
//!    3 s of that moment. A unit that fails to start skips the wait.
//! 3. Otherwise the window takes the engine lock and runs its own engine, and the status chip
//!    shows "Engine service not running". While it runs that engine it tries the socket every
//!    30 s and logs the first answer; the window keeps its own engine until it restarts. When
//!    another process holds the lock, the window runs no engine, shows "Can't start the engine"
//!    and its `RemoteClient` keeps trying the socket.
//!
//! "Switch to engine service" (`EngineLink::switch_to_service`) stops the window's engine, which
//! flushes history, releases the lock and runs the same three steps on a thread of its own. So
//! when the service does not come up, the window takes the lock again and runs its own engine.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use tl_model::client::EngineClient;
use tl_model::remote_proto::HelloTail;
use tl_remote::client::{BACKOFF, ClientConfig, Connection, LinkStatus, RemoteClient};
use tl_remote::server;
use tl_ui::engine_link::EngineLinkState;
use tracing::{info, warn};

use crate::switch_client::SwitchClient;

/// The engine service's user unit.
pub const UNIT: &str = "tasklens-engine.service";
/// In the state directory. Whoever holds its `flock` runs the engine (ADR-018).
pub const LOCK_FILE: &str = "engine.lock";
/// The service must answer within this time of start, or of a switch, and again within this time
/// of `systemctl --user start` succeeding, else the window runs its own engine. `systemctl` also
/// gets at most this time.
pub const CONNECT_DEADLINE: Duration = Duration::from_secs(3);
/// How often a window that runs its own engine tries the socket (ADR-018 Window fallback).
pub const FALLBACK_RETRY: Duration = Duration::from_secs(30);
/// How often the socket is tried until the deadline.
const POLL: Duration = Duration::from_millis(50);
/// How often a window with no engine checks whether its client has connected.
const WATCH: Duration = Duration::from_millis(500);

/// An in-process engine and the way to stop it.
pub struct LocalEngine {
    pub client: Arc<dyn EngineClient>,
    /// Stops the engine and flushes history. Called once.
    pub stop: Box<dyn FnOnce() + Send>,
}

/// What the link needs from the process, so tests can stand in for systemd and the engine.
pub struct LinkEnv {
    /// `$XDG_RUNTIME_DIR/<app>/engine.sock`, or why there is none.
    pub socket: Result<PathBuf, String>,
    /// `<state_dir>/engine.lock`.
    pub lock_file: PathBuf,
    /// Starts the unit and waits at most the given time. True when it started. `None` when the
    /// window cannot start a unit for its paths (an `--app-id` instance).
    pub start_unit: Option<Box<dyn Fn(Duration) -> bool + Send + Sync>>,
    /// Starts the in-process engine. Called only while the lock is held.
    pub start_engine: Box<dyn Fn() -> Result<LocalEngine, String> + Send + Sync>,
    pub hello: HelloTail,
    /// The user the service must run as.
    pub uid: u32,
    /// Gets every state change. The UI shows it in the status chip.
    pub notify: Box<dyn Fn(EngineLinkState) + Send + Sync>,
    /// `CONNECT_DEADLINE` in the app.
    pub deadline: Duration,
    /// `FALLBACK_RETRY` in the app.
    pub retry: Duration,
}

/// What runs now.
enum Running {
    /// Connected to the service, or connecting after a drop.
    Service(Arc<RemoteClient>),
    /// The window's own engine, with the lock it holds.
    Local { engine: LocalEngine, lock: File },
    /// No engine; the client keeps trying the socket.
    Waiting(Arc<RemoteClient>),
    /// During a switch and after `shutdown`.
    Stopped,
}

impl Running {
    fn state(&self, can_switch: bool) -> EngineLinkState {
        match self {
            Running::Service(_) | Running::Stopped => EngineLinkState::Service,
            Running::Local { .. } => EngineLinkState::Fallback { can_switch },
            Running::Waiting(_) => EngineLinkState::NoEngine,
        }
    }

    fn backend(&self) -> Option<Arc<dyn EngineClient>> {
        match self {
            Running::Service(c) | Running::Waiting(c) => Some(c.clone()),
            Running::Local { engine, .. } => Some(Arc::clone(&engine.client)),
            Running::Stopped => None,
        }
    }
}

/// See the module docs.
pub struct EngineLink {
    env: LinkEnv,
    client: Arc<SwitchClient>,
    running: Mutex<Running>,
    switching: AtomicBool,
    /// Raised each time the window's own engine stops, which ends that engine's socket checks.
    local_runs: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl EngineLink {
    /// Runs the start steps of the module docs. `Err` when the window holds the lock and its
    /// engine does not start, or when no client thread starts.
    pub fn open(env: LinkEnv) -> Result<Arc<Self>, String> {
        let running = establish(&env)?;
        let backend = running.backend().ok_or("no engine")?;
        let link = Arc::new(Self {
            client: Arc::new(SwitchClient::new(backend)),
            running: Mutex::new(running),
            switching: AtomicBool::new(false),
            local_runs: AtomicU64::new(0),
            env,
        });
        link.announce();
        Ok(link)
    }

    /// The client the UI uses for the whole run.
    pub fn client(&self) -> Arc<SwitchClient> {
        Arc::clone(&self.client)
    }

    /// The current state.
    #[cfg(test)]
    fn state(&self) -> EngineLinkState {
        lock(&self.running).state(self.can_switch())
    }

    fn can_switch(&self) -> bool {
        self.env.start_unit.is_some()
    }

    /// Sends the current state. Watches a client with no engine behind it until it connects,
    /// and has the window's own engine try the socket every `retry`.
    fn announce(self: &Arc<Self>) {
        let (state, waiting, local) = {
            let running = lock(&self.running);
            let waiting = match &*running {
                Running::Waiting(c) => Some(Arc::clone(c)),
                _ => None,
            };
            let local = matches!(&*running, Running::Local { .. });
            (running.state(self.can_switch()), waiting, local)
        };
        (self.env.notify)(state);
        if local {
            self.check_socket_while_local();
        }
        if let Some(remote) = waiting {
            let link = Arc::downgrade(self);
            spawn("tl-engine-wait", move || {
                while let Some(link) = link.upgrade() {
                    if link.promote(&remote) {
                        return;
                    }
                    drop(link);
                    thread::sleep(WATCH);
                }
            });
        }
    }

    /// Tries the socket every `retry` until the window's current engine stops, and logs the
    /// first time the service answers. Nothing else changes: the window keeps its own engine
    /// until it restarts or the user switches (ADR-018 Window fallback). The service cannot run
    /// while the window holds the lock, so this normally finds nothing.
    fn check_socket_while_local(self: &Arc<Self>) {
        let run = self.local_runs.load(Ordering::Acquire);
        let link = Arc::downgrade(self);
        let retry = self.env.retry;
        spawn("tl-engine-check", move || {
            loop {
                thread::sleep(retry);
                let Some(link) = link.upgrade() else {
                    return;
                };
                if link.local_runs.load(Ordering::Acquire) != run {
                    return;
                }
                if try_socket(&link.env).is_some() {
                    info!(target: "tasklens::engine", "the engine service answers; the window keeps its own engine until it restarts");
                    return;
                }
            }
        });
    }

    /// True when `remote` is no longer the waiting client, or has connected, which makes it
    /// the service client.
    fn promote(&self, remote: &Arc<RemoteClient>) -> bool {
        let mut running = lock(&self.running);
        let Running::Waiting(current) = &*running else {
            return true;
        };
        if !Arc::ptr_eq(current, remote) {
            return true;
        }
        if remote.status() != LinkStatus::Connected {
            return false;
        }
        info!(target: "tasklens::engine", "engine service answered");
        *running = Running::Service(Arc::clone(remote));
        drop(running);
        (self.env.notify)(EngineLinkState::Service);
        true
    }

    /// Starts the switch on its own thread and returns. Does nothing unless the window runs its
    /// own engine and a unit can be started, or while a switch runs.
    pub fn switch_to_service(self: &Arc<Self>) {
        if !self.can_switch() || self.switching.swap(true, Ordering::AcqRel) {
            return;
        }
        let link = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("tl-engine-switch".into())
            .spawn(move || {
                link.run_switch();
                link.switching.store(false, Ordering::Release);
            });
        if let Err(e) = spawned {
            warn!(target: "tasklens::engine", "switch thread did not start: {e}");
            self.switching.store(false, Ordering::Release);
        }
    }

    /// Stops the window's engine, releases the lock and runs the start steps again (ADR-018
    /// Lifetime).
    pub fn run_switch(self: &Arc<Self>) {
        let old = {
            let mut running = lock(&self.running);
            if !matches!(&*running, Running::Local { .. }) {
                return;
            }
            std::mem::replace(&mut *running, Running::Stopped)
        };
        let Running::Local { engine, lock: file } = old else {
            return;
        };
        self.local_runs.fetch_add(1, Ordering::AcqRel);
        info!(target: "tasklens::engine", "switching to the engine service");
        (engine.stop)();
        drop(file);
        let next = match establish(&self.env) {
            Ok(next) => next,
            Err(e) => {
                warn!(target: "tasklens::engine", "after the switch: {e}");
                match remote(&self.env, None) {
                    Ok(c) => Running::Waiting(c),
                    Err(e) => {
                        warn!(target: "tasklens::engine", "{e}");
                        return;
                    }
                }
            }
        };
        if let Some(backend) = next.backend() {
            self.client.switch_to(backend);
        }
        *lock(&self.running) = next;
        self.announce();
    }

    /// Stops the window's own engine, if it runs one, and releases the lock. The service and
    /// its client are left alone: the service keeps running when the window closes.
    pub fn shutdown(&self) {
        let old = std::mem::replace(&mut *lock(&self.running), Running::Stopped);
        if let Running::Local { engine, lock: file } = old {
            self.local_runs.fetch_add(1, Ordering::AcqRel);
            (engine.stop)();
            drop(file);
        }
    }
}

/// The start steps of the module docs.
fn establish(env: &LinkEnv) -> Result<Running, String> {
    if let Some(conn) = try_socket(env) {
        let client = remote(env, Some(conn))?;
        if answered(&client, Instant::now() + env.deadline) {
            info!(target: "tasklens::engine", "using the engine service");
            return Ok(Running::Service(client));
        }
        info!(target: "tasklens::engine", "the engine socket accepted but the service did not answer within {} ms", env.deadline.as_millis());
    }
    if let Some(start_unit) = &env.start_unit {
        if start_unit(env.deadline) {
            if let Some(client) = wait_for_service(env, Instant::now() + env.deadline)? {
                info!(target: "tasklens::engine", "started the engine service");
                return Ok(Running::Service(client));
            }
            info!(target: "tasklens::engine", "the engine service did not answer within {} ms of {UNIT} starting", env.deadline.as_millis());
        } else {
            info!(target: "tasklens::engine", "{UNIT} did not start");
        }
    }
    match lock_engine(&env.lock_file) {
        Ok(Some(file)) => {
            let engine = (env.start_engine)()?;
            info!(target: "tasklens::engine", "running the engine in the window");
            Ok(Running::Local { engine, lock: file })
        }
        Ok(None) => {
            warn!(target: "tasklens::engine", "another process holds the engine lock and the engine service does not answer");
            Ok(Running::Waiting(remote(env, None)?))
        }
        Err(e) => {
            warn!(target: "tasklens::engine", "engine lock: {e}");
            Ok(Running::Waiting(remote(env, None)?))
        }
    }
}

/// Tries the socket every `POLL` until it accepts, then waits for the service to answer on that
/// connection. `None` when no answer came by `deadline`.
fn wait_for_service(env: &LinkEnv, deadline: Instant) -> Result<Option<Arc<RemoteClient>>, String> {
    loop {
        if let Some(conn) = try_socket(env) {
            let client = remote(env, Some(conn))?;
            return Ok(answered(&client, deadline).then_some(client));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(POLL);
    }
}

/// True when the service's `Welcome` reaches `client` by `deadline`. The client reconnects on
/// its own after a refused or dropped connection, and an answer on a later connection counts.
fn answered(client: &RemoteClient, deadline: Instant) -> bool {
    loop {
        if client.host().is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
}

/// A connection to the socket whose peer runs as `env.uid`, if one answers now.
fn try_socket(env: &LinkEnv) -> Option<Connection> {
    let path = env.socket.as_ref().ok()?;
    connect(path, env.uid).ok()
}

fn connect(path: &Path, uid: u32) -> io::Result<Connection> {
    let stream = UnixStream::connect(path)?;
    server::check_peer(&stream, uid).map_err(|e| io::Error::other(e.to_string()))?;
    Connection::unix(stream)
}

/// A `RemoteClient` of the socket. Its first connection is `first` when given; every later one
/// connects again, with the client's backoff.
fn remote(env: &LinkEnv, first: Option<Connection>) -> Result<Arc<RemoteClient>, String> {
    let first = Mutex::new(first);
    let socket = env.socket.clone();
    let uid = env.uid;
    let connector = move || {
        if let Some(conn) = lock(&first).take() {
            return Ok(conn);
        }
        match &socket {
            Ok(path) => connect(path, uid),
            Err(e) => Err(io::Error::other(e.clone())),
        }
    };
    let config = ClientConfig {
        hello: env.hello.clone(),
        backoff: BACKOFF.to_vec(),
    };
    RemoteClient::start(connector, config)
        .map(Arc::new)
        .map_err(|e| format!("engine client thread: {e}"))
}

/// Takes `flock(LOCK_EX | LOCK_NB)` on `path` (created with mode 0600). `None` when another
/// process holds it. The lock lasts as long as the file stays open.
pub fn lock_engine(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// `$XDG_RUNTIME_DIR/<app_dir>/engine.sock`, with the runtime directory and the `app_dir`
/// directory checked as the service checks them (ADR-018 Local transport).
pub fn socket_path(runtime: Option<&std::ffi::OsStr>, app_dir: &str) -> Result<PathBuf, String> {
    let uid = server::own_uid();
    let runtime = server::runtime_dir(runtime, uid).map_err(|e| e.to_string())?;
    let dir = server::socket_dir(&runtime, app_dir, uid).map_err(|e| e.to_string())?;
    Ok(dir.join(server::SOCKET_NAME))
}

/// Runs `systemctl --user start` for the unit and waits at most `timeout`. A run that takes
/// longer is killed and counts as failed.
pub fn start_unit(timeout: Duration) -> bool {
    let child = Command::new("systemctl")
        .args(["--user", "start", UNIT])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            info!(target: "tasklens::engine", "systemctl: {e}");
            return false;
        }
    };
    let begin = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if begin.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                info!(target: "tasklens::engine", "systemctl did not finish within {} ms", timeout.as_millis());
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Err(e) => {
                info!(target: "tasklens::engine", "systemctl: {e}");
                return false;
            }
        }
    }
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) {
    if let Err(e) = thread::Builder::new().name(name.into()).spawn(body) {
        warn!(target: "tasklens::engine", "{name} thread did not start: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::AtomicU32;

    use tl_model::remote_proto::Want;

    use crate::switch_client::tests::{TestBackend, serve};

    struct Rig {
        dir: tempfile::TempDir,
        states: Arc<Mutex<Vec<EngineLinkState>>>,
        engines_started: Arc<AtomicU32>,
        engines_stopped: Arc<AtomicU32>,
        unit_runs: Arc<AtomicU32>,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().expect("tempdir"),
                states: Arc::default(),
                engines_started: Arc::default(),
                engines_stopped: Arc::default(),
                unit_runs: Arc::default(),
            }
        }

        fn socket(&self) -> PathBuf {
            self.dir.path().join("engine.sock")
        }

        fn lock_file(&self) -> PathBuf {
            self.dir.path().join(LOCK_FILE)
        }

        /// A link whose unit does `unit` when started.
        fn env(&self, unit: Option<Box<dyn Fn(Duration) -> bool + Send + Sync>>) -> LinkEnv {
            let started = Arc::clone(&self.engines_started);
            let stopped = Arc::clone(&self.engines_stopped);
            let states = Arc::clone(&self.states);
            let runs = Arc::clone(&self.unit_runs);
            LinkEnv {
                socket: Ok(self.socket()),
                lock_file: self.lock_file(),
                start_unit: unit.map(|unit| {
                    Box::new(move |left| {
                        runs.fetch_add(1, Ordering::AcqRel);
                        unit(left)
                    }) as Box<dyn Fn(Duration) -> bool + Send + Sync>
                }),
                start_engine: Box::new(move || {
                    started.fetch_add(1, Ordering::AcqRel);
                    let stopped = Arc::clone(&stopped);
                    Ok(LocalEngine {
                        client: TestBackend::new(0, 500),
                        stop: Box::new(move || {
                            stopped.fetch_add(1, Ordering::AcqRel);
                        }),
                    })
                }),
                hello: HelloTail {
                    pid: 1,
                    app_id: Arc::from("test"),
                    bridge: false,
                    want: Want { snapshots: true },
                },
                uid: server::own_uid(),
                notify: Box::new(move |s| lock(&states).push(s)),
                deadline: Duration::from_millis(300),
                retry: Duration::from_secs(60),
            }
        }

        fn states(&self) -> Vec<EngineLinkState> {
            lock(&self.states).clone()
        }
    }

    /// A service on `listener`: it answers each connection's `Hello` with a `Welcome` after
    /// `delay`.
    fn service(listener: UnixListener, delay: Duration) {
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                thread::spawn(move || serve(stream, delay, Vec::new()));
            }
        });
    }

    /// Accepts one connection on `listener` within 5 s. True when one came.
    fn accepts(listener: &UnixListener) -> bool {
        listener.set_nonblocking(true).unwrap();
        let begin = Instant::now();
        while begin.elapsed() < Duration::from_secs(5) {
            if listener.accept().is_ok() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// Accepts one connection on `listener` and returns its first bytes: the client's `Hello`.
    fn first_bytes(listener: &UnixListener) -> Vec<u8> {
        use std::io::Read;
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).expect("hello bytes");
        buf.to_vec()
    }

    #[test]
    fn socket_present_uses_the_service() {
        let rig = Rig::new();
        service(UnixListener::bind(rig.socket()).unwrap(), Duration::ZERO);
        let link = EngineLink::open(rig.env(Some(Box::new(|_| panic!("no unit start"))))).unwrap();
        assert_eq!(link.state(), EngineLinkState::Service);
        assert_eq!(rig.states(), [EngineLinkState::Service]);
        assert_eq!(rig.engines_started.load(Ordering::Acquire), 0);
        assert_eq!(rig.unit_runs.load(Ordering::Acquire), 0);
        // The window holds no lock while the service runs.
        assert!(lock_engine(&rig.lock_file()).unwrap().is_some());
    }

    /// A socket that accepts and never sends a `Welcome` is no service.
    #[test]
    fn silent_socket_falls_back_at_the_deadline() {
        let rig = Rig::new();
        let _listener = UnixListener::bind(rig.socket()).unwrap();
        let begin = Instant::now();
        let link = EngineLink::open(rig.env(Some(Box::new(|_| false)))).unwrap();
        assert!(begin.elapsed() >= Duration::from_millis(300));
        assert_eq!(link.state(), EngineLinkState::Fallback { can_switch: true });
        assert_eq!(rig.engines_started.load(Ordering::Acquire), 1);
        link.shutdown();
    }

    /// The service gets the whole deadline after `systemctl` succeeds, however long that took.
    #[test]
    fn deadline_counts_from_the_unit_start() {
        let rig = Rig::new();
        let socket = rig.socket();
        let link = EngineLink::open(rig.env(Some(Box::new(move |_| {
            thread::sleep(Duration::from_millis(200));
            // Answers 250 ms after the unit started: 450 ms after start, within 300 ms of
            // systemctl.
            service(
                UnixListener::bind(&socket).unwrap(),
                Duration::from_millis(250),
            );
            true
        }))))
        .unwrap();
        assert_eq!(link.state(), EngineLinkState::Service);
        assert_eq!(rig.engines_started.load(Ordering::Acquire), 0);
    }

    #[test]
    fn no_socket_and_no_systemd_uses_the_fallback() {
        let rig = Rig::new();
        let begin = Instant::now();
        let link = EngineLink::open(rig.env(Some(Box::new(|_| false)))).unwrap();
        // A unit that fails ends the wait at once.
        assert!(begin.elapsed() < Duration::from_millis(250));
        assert_eq!(rig.unit_runs.load(Ordering::Acquire), 1);
        assert_eq!(rig.engines_started.load(Ordering::Acquire), 1);
        assert_eq!(
            rig.states(),
            [EngineLinkState::Fallback { can_switch: true }]
        );
        assert_eq!(link.client().latest().sequence, 500);
        // The window holds the engine lock.
        assert!(lock_engine(&rig.lock_file()).unwrap().is_none());
        link.shutdown();
        assert_eq!(rig.engines_stopped.load(Ordering::Acquire), 1);
        assert!(lock_engine(&rig.lock_file()).unwrap().is_some());
    }

    #[test]
    fn unit_started_but_silent_falls_back_at_the_deadline() {
        let rig = Rig::new();
        let begin = Instant::now();
        let link = EngineLink::open(rig.env(Some(Box::new(|_| {
            thread::sleep(Duration::from_millis(200));
            true
        }))))
        .unwrap();
        // 200 ms for the unit, then the whole 300 ms deadline.
        assert!(begin.elapsed() >= Duration::from_millis(500));
        assert_eq!(link.state(), EngineLinkState::Fallback { can_switch: true });
        link.shutdown();
    }

    /// ADR-018 Window fallback: the window's own engine tries the socket every `retry` and
    /// keeps running when something answers.
    #[test]
    fn fallback_tries_the_socket_while_it_runs() {
        let rig = Rig::new();
        let mut env = rig.env(Some(Box::new(|_| false)));
        env.retry = Duration::from_millis(20);
        let link = EngineLink::open(env).unwrap();
        assert_eq!(link.state(), EngineLinkState::Fallback { can_switch: true });
        let listener = UnixListener::bind(rig.socket()).unwrap();
        assert!(accepts(&listener), "the window tried the socket");
        assert_eq!(link.state(), EngineLinkState::Fallback { can_switch: true });
        assert_eq!(rig.engines_stopped.load(Ordering::Acquire), 0);
        link.shutdown();
    }

    #[test]
    fn stopped_engine_no_longer_tries_the_socket() {
        let rig = Rig::new();
        let mut env = rig.env(Some(Box::new(|_| false)));
        env.retry = Duration::from_millis(20);
        let link = EngineLink::open(env).unwrap();
        link.shutdown();
        let listener = UnixListener::bind(rig.socket()).unwrap();
        listener.set_nonblocking(true).unwrap();
        thread::sleep(Duration::from_millis(150));
        assert!(listener.accept().is_err());
    }

    #[test]
    fn app_id_instance_offers_no_switch() {
        let rig = Rig::new();
        let link = EngineLink::open(rig.env(None)).unwrap();
        assert_eq!(
            link.state(),
            EngineLinkState::Fallback { can_switch: false }
        );
        link.switch_to_service();
        assert_eq!(rig.engines_stopped.load(Ordering::Acquire), 0);
        link.shutdown();
    }

    #[test]
    fn lock_taken_and_no_socket_runs_no_engine() {
        let rig = Rig::new();
        let _other = lock_engine(&rig.lock_file()).unwrap().expect("lock");
        let link = EngineLink::open(rig.env(Some(Box::new(|_| false)))).unwrap();
        assert_eq!(link.state(), EngineLinkState::NoEngine);
        assert_eq!(rig.engines_started.load(Ordering::Acquire), 0);
        // The client keeps trying the socket; when the service answers, the chip clears.
        let listener = UnixListener::bind(rig.socket()).unwrap();
        assert_eq!(first_bytes(&listener), b"TLR1");
        link.shutdown();
    }

    #[test]
    fn switch_releases_the_lock_and_connects() {
        let rig = Rig::new();
        let socket = rig.socket();
        let lock_file = rig.lock_file();
        // The unit starts once the window has released the lock: like the service, it takes the
        // lock first and then binds the socket. The first run, at start, fails.
        let service_lock: Arc<Mutex<Option<File>>> = Arc::default();
        let held = Arc::clone(&service_lock);
        let runs = AtomicU32::new(0);
        let env = rig.env(Some(Box::new(move |_| {
            if runs.fetch_add(1, Ordering::AcqRel) == 0 {
                return false;
            }
            let Ok(Some(file)) = lock_engine(&lock_file) else {
                return false;
            };
            *lock(&held) = Some(file);
            service(UnixListener::bind(&socket).unwrap(), Duration::ZERO);
            true
        })));
        let link = EngineLink::open(env).unwrap();
        assert_eq!(link.state(), EngineLinkState::Fallback { can_switch: true });
        let before = link.client().generation();

        link.run_switch();
        assert_eq!(rig.engines_stopped.load(Ordering::Acquire), 1);
        assert_eq!(link.state(), EngineLinkState::Service);
        assert_eq!(
            rig.states(),
            [
                EngineLinkState::Fallback { can_switch: true },
                EngineLinkState::Service,
            ]
        );
        assert!(lock(&service_lock).is_some(), "the service took the lock");
        // The proxy now reads the service client, on a higher generation.
        assert!(link.client().generation() > before);
        assert_eq!(link.client().latest().sequence, 0);
        // No engine of the window's own ran again.
        assert_eq!(rig.engines_started.load(Ordering::Acquire), 1);
    }

    #[test]
    fn switch_without_service_runs_the_engine_again() {
        let rig = Rig::new();
        let link = EngineLink::open(rig.env(Some(Box::new(|_| false)))).unwrap();
        link.run_switch();
        assert_eq!(rig.engines_stopped.load(Ordering::Acquire), 1);
        assert_eq!(rig.engines_started.load(Ordering::Acquire), 2);
        assert_eq!(link.state(), EngineLinkState::Fallback { can_switch: true });
        assert!(lock_engine(&rig.lock_file()).unwrap().is_none());
        link.shutdown();
    }

    #[test]
    fn socket_path_needs_a_valid_runtime_dir() {
        assert!(socket_path(None, "tasklens").is_err());
        assert!(socket_path(Some("relative".as_ref()), "tasklens").is_err());
    }
}
