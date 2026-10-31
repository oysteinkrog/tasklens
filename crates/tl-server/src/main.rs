//! `tasklens-engine`, the engine service (ADR-018, D-038): one engine per user per machine,
//! reached by windows and by `tasklens bridge` over `$XDG_RUNTIME_DIR/tasklens/engine.sock`.
//!
//! Start order: logging; SIGTERM and SIGINT blocked, so only `sigwait` sees them; the engine lock
//! (exit 0 when another engine holds it); the host settings in `engine.toml`, which the lock
//! holder owns; the runtime and socket directories; the socket; the engine, given the host
//! settings; the accept thread; `READY=1` once history is open or after 5 s. On SIGTERM the
//! engine stops, which stops sampling and flushes history, and the service exits.
//!
//! Each connection whose peer runs as this user gets a handshake and, when welcomed, a
//! `SessionPort`. At most `HANDSHAKES_MAX` connections are in their handshake at once, and the
//! engine admits at most 8 sessions, so a client that never sends its `Hello` cannot make the
//! service start threads or hold sockets without bound. A welcomed connection then runs
//! `tl_remote::server::serve_session` over its port, which drops the port when the session ends.

use std::ffi::OsString;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, IsTerminal, Read};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use host_settings::{HostSettingsFile, Origin, SharedHostSettings};
use sd_notify::NotifyState;
use tl_engine::client_session::{ClientSessionId, OpenError, SESSIONS_MAX};
use tl_engine::{Engine, EngineConfig, EngineHandle, EngineHooks, EngineMode, SessionPort};
use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::journal::{JournalHandle, JournalQuery};
use tl_model::remote_proto::{
    HostIdentity, HostSettings, HostSettingsChanges, HostSettingsValues, RefuseReason,
    SetHostSettingsReply,
};
use tl_model::request::RequestEvent;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_remote::envelope::FrameReader;
use tl_remote::server::{self, EndpointError, Port, Receiver, SessionConfig};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

mod host_settings;

/// The directory under each XDG root.
const APP_DIR: &str = "tasklens";
/// In the state directory. Whoever holds its `flock` runs the engine (ADR-018).
const LOCK_FILE: &str = "engine.lock";
/// `READY=1` goes out at the latest this long after the engine started.
const READY_WAIT: Duration = Duration::from_secs(5);
/// How often `wait_ready` looks at the published health.
const READY_POLL: Duration = Duration::from_millis(50);
/// Connections in their handshake at once. Each has `server::HELLO_TIMEOUT` from accept for its
/// whole handshake (reads, the engine's admission and the answer), then it is closed and its
/// place frees. A connection beyond this is closed at
/// once, before a thread starts for it; with the 8 sessions that bounds the connection threads
/// to 12.
const HANDSHAKES_MAX: usize = 4;
/// The privileged helper, as in `tasklens` (ADR-006).
const HELPER_PATH: &str = "/usr/lib/tasklens/tl-helper";
/// The log line when another engine holds the lock (ADR-018).
const ALREADY_RUNNING: &str = "engine already running in a window";

fn main() -> ExitCode {
    init_logging();
    // Before any thread starts: every thread inherits the mask, so the signals stay pending until
    // the main thread's `sigwait`.
    let signals = block_stop_signals();
    match run(&signals) {
        Ok(code) => code,
        Err(e) => {
            error!(target: "tasklens::engine", "{e}");
            ExitCode::from(1)
        }
    }
}

/// `TASKLENS_LOG` as in `tasklens`, `info` when it is unset or invalid. stderr goes to the
/// journal under systemd.
fn init_logging() {
    let filter = std::env::var("TASKLENS_LOG")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .and_then(|v| EnvFilter::builder().parse(v).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_ansi(io::stderr().is_terminal())
        .init();
}

fn run(signals: &libc::sigset_t) -> Result<ExitCode, String> {
    let state_dir = state_dir(|name| std::env::var_os(name))?;
    let Some(lock) = lock_engine(&state_dir)
        .map_err(|e| format!("{}: {e}", state_dir.join(LOCK_FILE).display()))?
    else {
        info!(target: "tasklens::engine", "{ALREADY_RUNNING}");
        // Under Type=notify, a clean exit before READY=1 counts as a failure and
        // Restart=on-failure would start the unit again every 2 s. Report ready, then stop.
        let _ = sd_notify::notify(&[NotifyState::Ready]);
        let _ = sd_notify::notify(&[NotifyState::Stopping]);
        return Ok(ExitCode::SUCCESS);
    };

    let config_dir = config_dir(|name| std::env::var_os(name))?;
    let (file, origin) = HostSettingsFile::open(&config_dir, now_unix());
    match &origin {
        Origin::Loaded => {}
        Origin::Imported => {
            info!(target: "tasklens::engine", "host settings imported from settings.toml");
        }
        Origin::Created => info!(target: "tasklens::engine", "host settings created"),
        Origin::Corrupt { kept_as } => warn!(
            target: "tasklens::engine",
            "engine.toml was unreadable and has been reset; the old file was kept as {kept_as}"
        ),
    }
    let start_values = file.current().values.clone();

    let uid = server::own_uid();
    let runtime = server::runtime_dir(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), uid)
        .map_err(|e| e.to_string())?;
    let socket_dir = server::socket_dir(&runtime, APP_DIR, uid).map_err(|e| e.to_string())?;
    let socket = socket_dir.join(server::SOCKET_NAME);
    let listener = server::bind(&socket, uid).map_err(|e| e.to_string())?;

    let mut config = EngineConfig::new(EngineMode::Full, state_dir.clone())
        .with_history_dir(state_dir.join("history"));
    if Path::new(HELPER_PATH).exists() {
        config = config.with_helper_path(HELPER_PATH.into());
    }
    let engine = Engine::start(config, EngineHooks::default())
        .map_err(|e| format!("engine start failed: {e}"))?;
    // As the window does: the settings follow the start at once.
    engine.set_settings(engine_settings(&start_values));
    info!(target: "tasklens::engine", socket = %socket.display(), "engine started");

    let applier = engine.clone();
    let host = Arc::new(SharedHostSettings::new(
        file,
        Box::new(move |values: &HostSettingsValues| {
            applier.set_settings(engine_settings(values));
        }),
    ));
    let identity = Arc::new(host_identity(&engine));
    spawn_accept(listener, uid, engine.clone(), identity, host)
        .map_err(|e| format!("accept thread: {e}"))?;

    if !wait_ready(&engine) {
        info!(target: "tasklens::engine", "history not open after {} s; ready anyway", READY_WAIT.as_secs());
    }
    if let Err(e) = sd_notify::notify(&[NotifyState::Ready]) {
        warn!(target: "tasklens::engine", "READY=1 not sent: {e}");
    }

    let signal = wait_signal(signals);
    info!(target: "tasklens::engine", signal, "stopping");
    let _ = sd_notify::notify(&[NotifyState::Stopping]);
    let report = engine.shutdown();
    if !report.clean() {
        warn!(target: "tasklens::engine", ?report, "engine shutdown did not finish cleanly");
    }
    // The lock is still held, so the socket at this path is ours.
    let _ = std::fs::remove_file(&socket);
    drop(lock);
    Ok(ExitCode::SUCCESS)
}

/// The engine's settings: the host settings, and defaults for the keys each window sets for
/// itself (tasklens-q4x76.22 moves those into the session's table query and demand).
fn engine_settings(values: &HostSettingsValues) -> EngineSettings {
    EngineSettings {
        interval_ms: values.interval_ms,
        history: values.history.clone(),
        ..EngineSettings::default()
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `$XDG_STATE_HOME/tasklens`, or `$HOME/.local/state/tasklens`, created with mode 0700.
fn state_dir(get: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, String> {
    app_dir(get, "XDG_STATE_HOME", ".local/state")
}

/// `$XDG_CONFIG_HOME/tasklens`, or `$HOME/.config/tasklens`, created with mode 0700. It holds
/// `engine.toml` and the window's `settings.toml`.
fn config_dir(get: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, String> {
    app_dir(get, "XDG_CONFIG_HOME", ".config")
}

/// `$<var>/tasklens`, or `$HOME/<fallback>/tasklens` when `var` is unset or not absolute,
/// created with mode 0700.
fn app_dir(
    get: impl Fn(&str) -> Option<OsString>,
    var: &str,
    fallback: &str,
) -> Result<PathBuf, String> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let root = match absolute(get(var)) {
        Some(root) => root,
        None => absolute(get("HOME"))
            .ok_or("HOME is not set to an absolute path")?
            .join(fallback),
    };
    let dir = root.join(APP_DIR);
    DirBuilder::new()
        .recursive(true)
        .mode(server::DIR_MODE)
        .create(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// Takes `flock(LOCK_EX | LOCK_NB)` on `dir/engine.lock` (created with mode 0600). `None` when
/// another process holds it. The lock lasts as long as the file stays open.
fn lock_engine(dir: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Who this engine is, for every `Welcome` (ADR-018 Identity and reconnects).
fn host_identity(engine: &EngineHandle) -> HostIdentity {
    let line = |path: &str| {
        std::fs::read_to_string(path)
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    };
    let mut host_name = line("/proc/sys/kernel/hostname");
    if host_name.is_empty() {
        host_name = "localhost".to_owned();
    }
    HostIdentity {
        host_name: Arc::from(host_name),
        machine_id: Arc::from(line("/etc/machine-id")),
        boot_id: engine.latest().boot_id,
        engine_instance: engine_instance(),
    }
}

/// 128 random bits from the kernel.
fn engine_instance() -> u128 {
    let mut bytes = [0u8; 16];
    match File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)) {
        Ok(()) => u128::from_le_bytes(bytes),
        Err(e) => {
            // Still differs between two engine processes: the pid and the start time do.
            warn!(target: "tasklens::engine", "/dev/urandom: {e}; engine instance from pid and time");
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            nanos ^ (u128::from(std::process::id()) << 96)
        }
    }
}

/// True once a published snapshot shows the metric history running, false after
/// `READY_WAIT`.
fn wait_ready(engine: &EngineHandle) -> bool {
    let begin = Instant::now();
    loop {
        if engine.latest().health.get(SourceId::MetricHistory).state == SourceState::Running {
            return true;
        }
        if begin.elapsed() >= READY_WAIT {
            return false;
        }
        thread::sleep(READY_POLL);
    }
}

fn spawn_accept(
    listener: UnixListener,
    uid: u32,
    engine: EngineHandle,
    identity: Arc<HostIdentity>,
    host: Arc<SharedHostSettings>,
) -> io::Result<()> {
    let next_id = Arc::new(AtomicU32::new(1));
    let handshakes = Arc::new(AtomicUsize::new(0));
    thread::Builder::new()
        .name("tl-accept".into())
        .spawn(move || {
            loop {
                match server::accept_peer(&listener, uid) {
                    Ok(stream) => {
                        let deadline = Instant::now() + server::HELLO_TIMEOUT;
                        let Some(slot) = HandshakeSlot::take(&handshakes) else {
                            warn!(target: "tasklens::engine", "connection closed: {HANDSHAKES_MAX} connections are in their handshake");
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                            continue;
                        };
                        let engine = engine.clone();
                        let identity = Arc::clone(&identity);
                        let next_id = Arc::clone(&next_id);
                        let host = Arc::clone(&host);
                        // A failed spawn drops the closure, and with it the slot and the stream.
                        let spawned = thread::Builder::new()
                            .name("tl-session".into())
                            .spawn(move || {
                                serve(stream, slot, deadline, &engine, &identity, &next_id, host)
                            });
                        if let Err(e) = spawned {
                            warn!(target: "tasklens::engine", "session thread: {e}");
                        }
                    }
                    Err(e @ EndpointError::PeerUid { .. }) => {
                        warn!(target: "tasklens::engine", "connection refused: {e}");
                    }
                    Err(e) => {
                        // EMFILE and the like: wait, so a full fd table does not spin.
                        warn!(target: "tasklens::engine", "{e}");
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        })?;
    Ok(())
}

/// One of the `HANDSHAKES_MAX` places for a connection in its handshake. Dropping it frees the
/// place.
struct HandshakeSlot(Arc<AtomicUsize>);

impl HandshakeSlot {
    /// `None` when all `HANDSHAKES_MAX` are taken.
    fn take(count: &Arc<AtomicUsize>) -> Option<Self> {
        count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < HANDSHAKES_MAX).then_some(n + 1)
            })
            .ok()
            .map(|_| HandshakeSlot(Arc::clone(count)))
    }
}

impl Drop for HandshakeSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A new session id, never 0 (the local session) and never one in use.
fn session_id(next_id: &AtomicU32) -> ClientSessionId {
    loop {
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        if id != ClientSessionId::LOCAL.0 {
            return ClientSessionId(id);
        }
    }
}

/// One connection: the handshake, which holds `slot` and ends by `deadline` (reads, admission
/// and answer), then the session's port until the client goes or the engine ends the session.
fn serve(
    stream: UnixStream,
    slot: HandshakeSlot,
    deadline: Instant,
    engine: &EngineHandle,
    identity: &HostIdentity,
    next_id: &AtomicU32,
    host: Arc<SharedHostSettings>,
) {
    let Ok(frames) = FrameReader::new() else {
        return;
    };
    // The reads, the engine's admission and the answer all end by `deadline`, so the slot
    // frees by then.
    let mut reader = server::DeadlineStream::new(&stream, deadline);
    let mut writer = server::DeadlineStream::new(&stream, deadline);
    let welcomed = server::handshake(&mut reader, &mut writer, &frames, |_| {
        match engine.open_session_until(session_id(next_id), deadline) {
            Ok(port) => Ok((identity.clone(), port)),
            Err(OpenError::Full) => Err(server::refused(
                RefuseReason::TooManySessions,
                format!(
                    "{} windows are connected to tasklens on {}",
                    SESSIONS_MAX - 1,
                    identity.host_name
                ),
            )),
            // The engine is busy or stopping. The text says which; the client tries again.
            Err(e) => Err(server::refused(RefuseReason::Other(0), e.to_string())),
        }
    });
    drop(slot);
    let (hello, port) = match welcomed {
        Ok(welcomed) => welcomed,
        Err(e) => {
            debug!(target: "tasklens::engine", "handshake: {e}");
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
    };
    let session = port.id().0;
    info!(
        target: "tasklens::engine",
        session,
        pid = hello.pid,
        bridge = hello.bridge,
        "client connected",
    );
    let config = SessionConfig {
        snapshots: hello.want.snapshots,
        ..SessionConfig::default()
    };
    let port = EnginePort {
        port,
        engine: engine.clone(),
        host,
    };
    let report = server::serve_session(stream, port, config);
    info!(
        target: "tasklens::engine",
        session,
        progress_dropped = report.progress_dropped,
        commands_dropped = report.commands_dropped,
        frames_trimmed = report.frames_trimmed,
        "client disconnected: {}",
        report.end,
    );
}

/// The engine's side of one session for `serve_session`: the session's port, plus the engine
/// for journal streams, which are not tied to a session, and the host settings every session
/// shares.
struct EnginePort {
    port: SessionPort,
    engine: EngineHandle,
    host: Arc<SharedHostSettings>,
}

impl Port for EnginePort {
    fn latest(&self) -> Arc<Snapshot> {
        self.port.latest()
    }

    fn wake(&self) -> Receiver<()> {
        self.port.wake()
    }

    fn action_events(&self) -> Receiver<ActionEvent> {
        self.port.action_events()
    }

    fn request_events(&self) -> Receiver<RequestEvent> {
        self.port.request_events()
    }

    fn set_demand(&self, demand: Demand) {
        self.port.set_demand(demand);
    }

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        self.port.send(cmd)
    }

    fn journal(&self, query: JournalQuery) -> JournalHandle {
        self.engine.journal(query)
    }

    fn is_ended(&self) -> bool {
        self.port.is_ended()
    }

    fn host_settings(&self) -> HostSettings {
        self.host.get()
    }

    fn set_host_settings(
        &self,
        base_rev: u64,
        changes: HostSettingsChanges,
    ) -> SetHostSettingsReply {
        self.host.set(base_rev, &changes)
    }
}

/// Blocks SIGTERM and SIGINT in this thread and returns the set for `wait_signal`.
#[allow(unsafe_code)]
fn block_stop_signals() -> libc::sigset_t {
    // SAFETY: `sigset_t` is plain data that `sigemptyset` fully initialises. Each call gets a
    // pointer to that local set; `pthread_sigmask` reads it and is told not to write an old mask.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    }
}

/// Waits for one of the signals in `set`, which every thread blocks, and returns it.
#[allow(unsafe_code)]
fn wait_signal(set: &libc::sigset_t) -> i32 {
    let mut signal = 0;
    loop {
        // SAFETY: `set` is an initialised signal set and `signal` a live local the call writes.
        if unsafe { libc::sigwait(set, &mut signal) } == 0 {
            return signal;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn state_dir_prefers_xdg_state_home() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let home = tmp.path().join("home");
        let state_s = state.clone().into_os_string();
        let home_s = home.clone().into_os_string();
        let dir = state_dir(|name| match name {
            "XDG_STATE_HOME" => Some(state_s.clone()),
            "HOME" => Some(home_s.clone()),
            _ => None,
        })
        .unwrap();
        assert_eq!(dir, state.join(APP_DIR));
        assert!(dir.is_dir());

        // A relative XDG_STATE_HOME is ignored, as in `tasklens`.
        let dir = state_dir(|name| match name {
            "XDG_STATE_HOME" => Some("relative".into()),
            "HOME" => Some(home_s.clone()),
            _ => None,
        })
        .unwrap();
        assert_eq!(dir, home.join(".local/state").join(APP_DIR));
        assert!(state_dir(|_| None).is_err());
    }

    #[test]
    fn config_dir_prefers_xdg_config_home() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config").into_os_string();
        let home = tmp.path().join("home");
        let home_s = home.clone().into_os_string();
        let dir = config_dir(|name| match name {
            "XDG_CONFIG_HOME" => Some(config.clone()),
            "HOME" => Some(home_s.clone()),
            _ => None,
        })
        .unwrap();
        assert_eq!(dir, tmp.path().join("config").join(APP_DIR));
        let dir = config_dir(|name| (name == "HOME").then(|| home_s.clone())).unwrap();
        assert_eq!(dir, home.join(".config").join(APP_DIR));
        assert!(dir.is_dir());
    }

    #[test]
    fn one_lock_holder_at_a_time() {
        let tmp = tempfile::tempdir().unwrap();
        let held = lock_engine(tmp.path()).unwrap().expect("free");
        // flock locks belong to the open file, so a second open in this process conflicts too.
        assert!(lock_engine(tmp.path()).unwrap().is_none());
        drop(held);
        assert!(lock_engine(tmp.path()).unwrap().is_some());
    }

    #[test]
    fn handshake_slots_are_bounded_and_freed_on_drop() {
        let count = Arc::new(AtomicUsize::new(0));
        let slots: Vec<_> = (0..HANDSHAKES_MAX)
            .map(|_| HandshakeSlot::take(&count).expect("free"))
            .collect();
        assert!(HandshakeSlot::take(&count).is_none());
        drop(slots);
        assert_eq!(count.load(Ordering::Acquire), 0);
        assert!(HandshakeSlot::take(&count).is_some());
    }

    #[test]
    fn session_ids_skip_the_local_session() {
        let next = AtomicU32::new(u32::MAX);
        assert_eq!(session_id(&next), ClientSessionId(u32::MAX));
        assert_eq!(session_id(&next), ClientSessionId(1));
    }
}
