//! The `KWin` bridge worker (ADR-004, R-046, PROC-052).
//!
//! On KDE Plasma it loads the `tasklens-bridge` script into KWin and serves the `WindowBridge`
//! object the script calls with the full window list on every window event. Each accepted
//! `Update` becomes one `SourceUpdate::Windows`. Without KDE or KWin it publishes an empty
//! `Unsupported` snapshot once and idles; that is not a failure.
//!
//! Only the current owner of `org.kde.KWin` may call `Update` (R-167 item 5). Window text is
//! untrusted (ADR-015): captions and class names are sanitized and cut at 4 KiB, and they are
//! never logged.
//!
//! Window commands (`KWinCommand`, E14-018) run one at a time, in arrival order, as one-shot
//! scripts `tasklens-cmd-<n>`. A command must name a window in the current list, and every value
//! enters the script only through `serde_json`. Commands have no result event (R-170); a failed
//! one is logged at `warn` and dropped. KWin may still read a script file after `start()` returns,
//! so the file is kept until the next command, the next `Update` or worker exit (`CmdFiles`).

use std::fs::{DirBuilder, Permissions};
use std::future::Future;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use asupersync::channel::{mpsc, watch};
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time;
use asupersync::{Cx, Outcome};
use futures_lite::stream::Boxed;
use futures_lite::{Stream, StreamExt};
use tl_collect::sanitize::sanitize_text;
use tl_model::command::KWinCommand;
use tl_model::health::{SourceId, SourceState};
use tl_model::snapshot::{ComponentHeader, FrameGeometry, WindowEntry, WindowsSnapshot};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{Bus, DbusConn, DbusError, READ_TIMEOUT, connect};
use crate::source::{CommandProgress, WorkerCommand, WorkerControl, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{EngineMsg, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;

pub const KWIN_NAME: &str = "org.kde.KWin";
pub const SCRIPTING_PATH: &str = "/Scripting";
pub const SCRIPTING_INTERFACE: &str = "org.kde.kwin.Scripting";
/// The KWin script plugin name.
pub const PLUGIN: &str = "tasklens-bridge";
pub const BRIDGE_PATH: &str = "/io/github/oysteinkrog/Tasklens/WindowBridge";

const DBUS_NAME: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";

/// The bridge script (E14-016). The placeholder becomes this connection's unique name as a JSON
/// string.
const SCRIPT: &str = include_str!("../../../../assets/kwin/kwin-bridge.js");
const PLACEHOLDER: &str = "__TL_BUS_NAME__";
/// Written to `$XDG_RUNTIME_DIR/tasklens/`.
const SCRIPT_FILE: &str = "kwin-bridge.js";
/// The one-shot command script (E14-017). The placeholder becomes the args object as JSON.
const CMD_TEMPLATE: &str = include_str!("../../../../assets/kwin/cmd-template.js");
const CMD_PLACEHOLDER: &str = "__TL_ARGS__";
/// Every tasklens window carries this app ID as its `resourceClass` (R-086, APP-009).
const APP_ID: &str = "io.github.oysteinkrog.Tasklens";
/// The main window's title (R-176).
const MAIN_CAPTION: &str = "tasklens";

/// Longer `Update` payloads are rejected (R-167 item 15).
pub const MAX_PAYLOAD: usize = 1024 * 1024;
/// Window text is cut at this many bytes (R-167 item 15).
pub const TEXT_MAX: usize = 4096;
/// The unsupported worker wakes this often to see a cancel.
const IDLE_WAKE: Duration = Duration::from_secs(60);
/// The deadline for `unloadScript` on exit (PROC-052).
const UNLOAD_TIMEOUT: Duration = Duration::from_millis(500);
/// Accepted updates waiting for the worker loop.
const UPDATE_QUEUE: usize = 8;

/// The health text when KDE or KWin is missing (R-155 item 4).
const NOT_AVAILABLE: &str = "KWin bridge not available";

/// Both engines. The low-memory engine runs only `KWinCommand::KeepAbove` (R-152).
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::KWin,
        name: "kwin",
        modes: &[EngineMode::Full, EngineMode::LowMemory],
        factory,
    }
}

pub(crate) fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    // Shared by every generation, so a plugin name is never used twice while the engine runs.
    let counter = Arc::new(AtomicU64::new(0));
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let commands = Commands {
            mode: cfg.mode,
            counter: Arc::clone(&counter),
        };
        async move {
            let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
            run(cx, io, commands, desktop.as_deref()).await
        }
    })
}

/// True when the colon-separated `XDG_CURRENT_DESKTOP` list holds `KDE` (case-sensitive).
fn kde_desktop(desktop: Option<&str>) -> bool {
    desktop.is_some_and(|list| list.split(':').any(|name| name == "KDE"))
}

async fn run(
    cx: Cx,
    io: WorkerIo,
    commands: Commands,
    desktop: Option<&str>,
) -> Outcome<(), WorkerError> {
    let mut queue = match resume(&cx, &io).await {
        Ok(queue) => queue,
        Err(stop) => return stop,
    };
    if !kde_desktop(desktop) {
        return unsupported(&cx, &io, &mut queue).await;
    }
    let conn = match connect(&cx, Bus::Session).await {
        Ok(conn) => conn,
        Err(e) => return failed(&cx, &io, e.into()).await,
    };
    match kwin_present(&cx, &conn).await {
        Ok(true) => {}
        Ok(false) => return unsupported(&cx, &io, &mut queue).await,
        Err(e) => return failed(&cx, &io, e.into()).await,
    }
    let Some(path) = script_path() else {
        let e = WorkerError::Other("XDG_RUNTIME_DIR is not set".into());
        return failed(&cx, &io, e).await;
    };
    if let Some(dir) = path.parent() {
        remove_stale(&cx, dir).await;
    }
    let mut files = CmdFiles::default();
    let result = serve(&cx, &io, &conn, &path, &commands, &mut queue, &mut files).await;
    unload_on_exit(&cx, &conn).await;
    // One unlink each on the runtime tmpfs. They run inline because the blocking pool refuses
    // new jobs once the region is cancelled.
    let _ = std::fs::remove_file(&path);
    drop(files);
    match result {
        Ok(()) => Outcome::Ok(()),
        Err(_) if cx.is_cancel_requested() => Outcome::Ok(()),
        Err(e) => failed(&cx, &io, e).await,
    }
}

/// Publishes the empty `Unsupported` snapshot and the terminal health once, then idles. Queued
/// commands are dropped.
async fn unsupported(cx: &Cx, io: &WorkerIo, queue: &mut Queue) -> Outcome<(), WorkerError> {
    let empty = SourceUpdate::Windows(Box::new(snapshot(Vec::new(), Validity::Unsupported)));
    if let Some(stop) = send(cx, io, empty).await {
        return stop;
    }
    let state = health(SourceState::Unsupported, Some(NOT_AVAILABLE.into()));
    if let Some(stop) = send(cx, io, state).await {
        return stop;
    }
    loop {
        queue.drop_all(&io.control.borrow_and_clone());
        time::sleep(cx.now(), IDLE_WAKE).await;
        if cx.checkpoint().is_err() {
            return Outcome::Ok(());
        }
    }
}

/// Reports `Failed` and returns the error so the supervisor restarts the generation.
async fn failed(cx: &Cx, io: &WorkerIo, e: WorkerError) -> Outcome<(), WorkerError> {
    if cx.checkpoint().is_err() {
        return Outcome::Ok(());
    }
    let state = health(SourceState::Failed, Some("KWin bridge failed".into()));
    if let Some(stop) = send(cx, io, state).await {
        return stop;
    }
    Outcome::Err(e)
}

/// `NameHasOwner("org.kde.KWin")` on the session bus.
async fn kwin_present(cx: &Cx, conn: &DbusConn) -> Result<bool, DbusError> {
    let bus = conn
        .read(
            cx,
            zbus::Proxy::new(conn.raw(), DBUS_NAME, DBUS_PATH, DBUS_INTERFACE),
        )
        .await?;
    conn.read(cx, bus.call("NameHasOwner", &(KWIN_NAME,))).await
}

fn script_path() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    runtime
        .is_absolute()
        .then(|| runtime.join("tasklens").join(SCRIPT_FILE))
}

/// What the worker loop waits for.
enum Event {
    /// An accepted `Update`.
    Windows(Vec<WindowEntry>),
    /// `NameOwnerChanged` for `org.kde.KWin`: the new owner, `None` when KWin is gone.
    Owner(Option<String>),
    /// The control watch changed: there may be new commands.
    Control,
}

/// Serves the bridge until `cx` is cancelled (`Ok`) or the bus fails (`Err`).
async fn serve(
    cx: &Cx,
    io: &WorkerIo,
    conn: &DbusConn,
    path: &Path,
    commands: &Commands,
    queue: &mut Queue,
    files: &mut CmdFiles,
) -> Result<(), WorkerError> {
    let raw = conn.raw();
    let unique = raw
        .unique_name()
        .map(|name| name.as_str().to_owned())
        .ok_or_else(|| WorkerError::Dbus("no unique name".into()))?;

    // Subscribe before reading the owner so no change is missed in between.
    let bus = conn
        .read(
            cx,
            zbus::Proxy::new(raw, DBUS_NAME, DBUS_PATH, DBUS_INTERFACE),
        )
        .await?;
    let owner_signals = conn
        .read(
            cx,
            bus.receive_signal_with_args("NameOwnerChanged", &[(0, KWIN_NAME)]),
        )
        .await?;
    let owner: String = conn
        .read(cx, bus.call("GetNameOwner", &(KWIN_NAME,)))
        .await?;
    drop(bus);

    let owner = Arc::new(Mutex::new(Some(owner)));
    let (tx, rx) = async_channel::bounded(UPDATE_QUEUE);
    let bridge = WindowBridge {
        owner: Arc::clone(&owner),
        tx,
    };
    conn.read(cx, raw.object_server().at(BRIDGE_PATH, bridge))
        .await?;

    write_script(cx, path, bridge_script(&unique)).await?;
    load_script(cx, conn, path).await?;
    if send(cx, io, health(SourceState::Running, None))
        .await
        .is_some()
    {
        return closed(cx);
    }

    let owner_events = owner_signals.filter_map(|msg| owner_event(&msg));
    let control_events = control_changes(cx, io.control.clone());
    let mut events: Boxed<Event> = rx.or(owner_events).or(control_events).boxed();
    let dir = path
        .parent()
        .ok_or_else(|| WorkerError::Other("script path has no directory".into()))?;
    let host = KWinScripting(conn);
    // The list commands are checked against. `None` until the first `Update`, so commands queued
    // across a restart wait for it.
    let mut current: Option<Arc<[WindowEntry]>> = None;
    loop {
        match conn.next_signal(cx, &mut events, READ_TIMEOUT).await {
            Ok(None | Some(Event::Control)) => {}
            Ok(Some(Event::Windows(windows))) => {
                // The first `Update` read after a command: KWin has run its script.
                files.clear(cx).await;
                let snap = snapshot(windows, Validity::Valid);
                current = Some(Arc::clone(&snap.windows));
                let update = SourceUpdate::Windows(Box::new(snap));
                if send(cx, io, update).await.is_some() {
                    return closed(cx);
                }
            }
            Ok(Some(Event::Owner(new))) => {
                let back = new.is_some();
                *owner.lock().unwrap_or_else(PoisonError::into_inner) = new;
                if back {
                    // KWin restarted: the new instance has no script loaded.
                    current = None;
                    load_script(cx, conn, path).await?;
                } else {
                    current = Some(Arc::from([]));
                    let gone = SourceUpdate::Windows(Box::new(snapshot(
                        Vec::new(),
                        Validity::Unsupported,
                    )));
                    if send(cx, io, gone).await.is_some() {
                        return closed(cx);
                    }
                }
            }
            Err(DbusError::Closed) if cx.is_cancel_requested() => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        if let Some(windows) = &current {
            run_commands(cx, io, &host, dir, commands, queue, windows, files).await;
        }
    }
}

/// One `Event::Control` per change of the control watch. The stream never ends, so a dropped
/// sender does not end the event stream.
fn control_changes(
    cx: &Cx,
    control: watch::Receiver<WorkerControl>,
) -> impl Stream<Item = Event> + Send + 'static {
    let cx = cx.clone();
    futures_lite::stream::unfold(control, move |mut control| {
        let cx = cx.clone();
        async move {
            control.changed(&cx).await.ok()?;
            Some((Event::Control, control))
        }
    })
    .chain(futures_lite::stream::pending())
}

/// `Ok` on cancel, `Err(Closed)` when the reducer is gone.
fn closed(cx: &Cx) -> Result<(), WorkerError> {
    if cx.is_cancel_requested() {
        Ok(())
    } else {
        Err(WorkerError::Closed)
    }
}

/// The new owner from a `NameOwnerChanged(s name, s old, s new)` body. `None` for a body that
/// does not parse; the bus daemon never sends one.
fn owner_event(msg: &zbus::Message) -> Option<Event> {
    let (_name, _old, new) = msg.body().deserialize::<(String, String, String)>().ok()?;
    Some(Event::Owner((!new.is_empty()).then_some(new)))
}

/// The bridge script with the placeholder replaced by `unique_name` as a JSON string
/// (R-167 item 5).
fn bridge_script(unique_name: &str) -> String {
    let quoted = serde_json::to_string(unique_name).expect("a str always serializes");
    SCRIPT.replace(PLACEHOLDER, &quoted)
}

/// Writes the script on the blocking pool: directory mode 0700, file mode 0600.
async fn write_script(cx: &Cx, path: &Path, text: String) -> Result<(), WorkerError> {
    let path = path.to_owned();
    blocking(cx, move || write_private(&path, text.as_bytes())).await
}

/// Deletes a command script on the blocking pool. When the pool refuses the job (the region is
/// cancelled), it runs inline: one file removal on the runtime tmpfs.
async fn remove_script(cx: &Cx, path: &Path) {
    let owned = path.to_owned();
    let result = blocking(cx, move || match std::fs::remove_file(&owned) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(WorkerError::Other(format!("remove script: {e}").into())),
    })
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
}

/// Deletes `tasklens-cmd-<n>.js` files left in `dir` by an earlier run. A missing directory is
/// fine; other errors are logged at `debug`.
async fn remove_stale(cx: &Cx, dir: &Path) {
    let dir = dir.to_owned();
    let result = blocking(cx, move || {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(WorkerError::Other(format!("read script dir: {e}").into())),
        };
        for entry in entries.flatten() {
            let stale = entry.file_name().to_str().is_some_and(cmd_file_name);
            if stale && entry.file_type().is_ok_and(|t| !t.is_dir()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    })
    .await;
    if let Err(e) = result {
        tracing::debug!(target: "tasklens::engine", error = %e, "stale command scripts not removed");
    }
}

/// True for `tasklens-cmd-<digits>.js`.
fn cmd_file_name(name: &str) -> bool {
    name.strip_prefix("tasklens-cmd-")
        .and_then(|rest| rest.strip_suffix(".js"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Runs `job` on the blocking pool and waits for it.
async fn blocking(
    cx: &Cx,
    job: impl FnOnce() -> Result<(), WorkerError> + Send + 'static,
) -> Result<(), WorkerError> {
    let mut job = cx
        .spawn_blocking(move |_| job())
        .map_err(|e| WorkerError::Other(format!("spawn: {e}").into()))?;
    match job.join(cx).await {
        Ok(result) => result,
        Err(JoinError::Cancelled(_)) => Err(WorkerError::Closed),
        Err(JoinError::Panicked(_)) => Err(WorkerError::Other("script file job panicked".into())),
        Err(JoinError::PolledAfterCompletion) => {
            Err(WorkerError::Other("job polled after completion".into()))
        }
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), WorkerError> {
    let io_err = |what: &str, e: std::io::Error| WorkerError::Other(format!("{what}: {e}").into());
    let dir = path
        .parent()
        .ok_or_else(|| WorkerError::Other("script path has no directory".into()))?;
    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(io_err("create script directory", e)),
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| io_err("script directory", e))?;
    if !meta.is_dir() || meta.uid() != rustix::process::getuid().as_raw() {
        return Err(WorkerError::Other(
            "script directory is not a directory owned by this user".into(),
        ));
    }
    if meta.mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, Permissions::from_mode(0o700))
            .map_err(|e| io_err("script directory mode", e))?;
    }
    tl_ops::write_atomic(path, bytes, tl_ops::PRIVATE_FILE_MODE)
        .map_err(|e| WorkerError::Other(format!("write script: {e}").into()))
}

/// `unloadScript` (clears a stale copy from a crashed run or an earlier KWin), `loadScript` and
/// `start` on `/Scripting`.
async fn load_script(cx: &Cx, conn: &DbusConn, path: &Path) -> Result<(), WorkerError> {
    let file = path
        .to_str()
        .ok_or_else(|| WorkerError::Other("script path is not UTF-8".into()))?;
    let scripting = scripting(cx, conn).await?;
    let _was_loaded: bool = conn
        .read(cx, scripting.call("unloadScript", &(PLUGIN,)))
        .await?;
    let id: i32 = conn
        .read(cx, scripting.call("loadScript", &(file, PLUGIN)))
        .await?;
    if id < 0 {
        return Err(WorkerError::Dbus("loadScript returned -1".into()));
    }
    conn.read(cx, scripting.call::<_, _, ()>("start", &()))
        .await?;
    Ok(())
}

/// A proxy for `/Scripting` on `org.kde.KWin`.
async fn scripting<'a>(cx: &Cx, conn: &'a DbusConn) -> Result<zbus::Proxy<'a>, DbusError> {
    conn.read(
        cx,
        zbus::Proxy::new(conn.raw(), KWIN_NAME, SCRIPTING_PATH, SCRIPTING_INTERFACE),
    )
    .await
}

/// `unloadScript` with a 500 ms deadline, sent even when `cx` is cancelled. Cancellation is
/// masked while the call runs, so the timeout's own sleep does not end at once. No reply is
/// awaited: the connection's tick task may already have stopped.
async fn unload_on_exit(cx: &Cx, conn: &DbusConn) {
    let raw = conn.raw().clone();
    let call = async move {
        let scripting =
            zbus::Proxy::new(&raw, KWIN_NAME, SCRIPTING_PATH, SCRIPTING_INTERFACE).await?;
        scripting.call_noreply("unloadScript", &(PLUGIN,)).await
    };
    let mut call = std::pin::pin!(time::timeout(cx.now(), UNLOAD_TIMEOUT, call));
    let result = std::future::poll_fn(|task| cx.masked(|| call.as_mut().poll(task))).await;
    if !matches!(result, Ok(Ok(()))) {
        tracing::debug!(target: "tasklens::engine", "unloadScript on exit did not complete");
    }
}

/// Where this generation is in the forwarded command list (R-276).
struct Queue {
    last_seq: u64,
    progress: Arc<CommandProgress>,
}

impl Queue {
    /// The KWin commands queued after `last_seq`, oldest first. Moves `last_seq` to the newest
    /// queued seq.
    fn take(&mut self, control: &WorkerControl) -> Vec<(u64, KWinCommand)> {
        let new = control
            .commands
            .iter()
            .filter(|c| c.seq > self.last_seq && c.target == SourceId::KWin)
            .filter_map(|c| match &c.command {
                WorkerCommand::KWin(cmd) => Some((c.seq, cmd.clone())),
                _ => None,
            })
            .collect();
        if let Some(newest) = control.commands.last() {
            self.last_seq = self.last_seq.max(newest.seq);
        }
        new
    }

    /// Takes the queued commands and counts them as done without running them.
    fn drop_all(&mut self, control: &WorkerControl) {
        if let Some(&(seq, _)) = self.take(control).last() {
            self.progress.set_started(SourceId::KWin, seq);
            self.progress.set_done(SourceId::KWin, seq);
        }
    }
}

/// Starts after the last command an earlier generation started (R-276). When that command never
/// finished, it is skipped and reported once as `Failed`.
async fn resume(cx: &Cx, io: &WorkerIo) -> Result<Queue, Outcome<(), WorkerError>> {
    let progress = Arc::clone(&io.control.borrow_and_clone().progress);
    let last_seq = progress.started(SourceId::KWin);
    if last_seq > progress.done(SourceId::KWin) {
        progress.set_done(SourceId::KWin, last_seq);
        let text = "KWin was not run again after the worker restarted";
        if let Some(stop) = send(cx, io, health(SourceState::Failed, Some(text.into()))).await {
            return Err(stop);
        }
    }
    Ok(Queue { last_seq, progress })
}

/// Turns window commands into one-shot scripts.
struct Commands {
    mode: EngineMode,
    /// The last `n` used in a plugin name. Shared by every generation of the worker.
    counter: Arc<AtomicU64>,
}

impl Commands {
    /// The plugin name and script text for `cmd`, or `None` when the command is dropped.
    fn prepare(&self, cmd: &KWinCommand, windows: &[WindowEntry]) -> Option<(String, String)> {
        let target = match resolve(cmd, windows, self.mode) {
            Ok(target) => target,
            Err(reason) => {
                tracing::debug!(target: "tasklens::engine", reason, "kwin command dropped");
                return None;
            }
        };
        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let plugin = format!("tasklens-cmd-{n}");
        let script = cmd_script(&plugin, &target);
        Some((plugin, script))
    }
}

/// A checked command: the KWin id of a window in the current list and what to do with it.
#[derive(Clone, Debug, PartialEq)]
struct Target {
    id: Arc<str>,
    op: CmdOp,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CmdOp {
    Activate,
    Close,
    KeepAbove(bool),
    Geometry(FrameGeometry),
}

/// Finds the window `cmd` names in `windows`. `Err` holds the reason the command is dropped;
/// it never holds the id or a caption.
fn resolve(
    cmd: &KWinCommand,
    windows: &[WindowEntry],
    mode: EngineMode,
) -> Result<Target, &'static str> {
    if mode == EngineMode::LowMemory && !matches!(cmd, KWinCommand::KeepAbove { .. }) {
        return Err("the low-memory engine runs only keep above");
    }
    let (window, op) = match cmd {
        KWinCommand::Activate { internal_id } => (by_id(windows, internal_id)?, CmdOp::Activate),
        KWinCommand::Close { internal_id } => (by_id(windows, internal_id)?, CmdOp::Close),
        KWinCommand::KeepAbove { pid, caption, on } => (
            own_window(windows, *pid, caption.as_deref())?,
            CmdOp::KeepAbove(*on),
        ),
        KWinCommand::SetFrameGeometry {
            pid,
            caption,
            geometry,
        } => {
            let g = FrameGeometry {
                width: geometry.width.max(1),
                height: geometry.height.max(1),
                ..*geometry
            };
            (
                own_window(windows, *pid, Some(caption))?,
                CmdOp::Geometry(g),
            )
        }
        // The window picker (E14-019).
        KWinCommand::Pick { .. } | KWinCommand::CancelPick { .. } => {
            return Err("pick is not a window command");
        }
    };
    // List entries are checked when parsed; this keeps the script safe if that ever changes.
    if !braced_uuid(&window.internal_id) {
        return Err("window id is not a braced lowercase UUID");
    }
    Ok(Target {
        id: Arc::clone(&window.internal_id),
        op,
    })
}

/// The window with KWin id `id`.
fn by_id<'a>(windows: &'a [WindowEntry], id: &str) -> Result<&'a WindowEntry, &'static str> {
    if !braced_uuid(id) {
        return Err("window id is not a braced lowercase UUID");
    }
    windows
        .iter()
        .find(|w| &*w.internal_id == id)
        .ok_or("window is not in the current list")
}

/// A tasklens window: `pid` and the tasklens app ID, plus the caption for a secondary window
/// (APP-009, R-086). `None` is the main window: the one titled "tasklens", or the only tasklens
/// window of `pid` (the low-memory rescue window).
fn own_window<'a>(
    windows: &'a [WindowEntry],
    pid: u32,
    caption: Option<&str>,
) -> Result<&'a WindowEntry, &'static str> {
    let mut ours = windows
        .iter()
        .filter(|w| w.pid == pid && &*w.resource_class == APP_ID);
    let found = match caption {
        Some(caption) => ours.find(|w| &*w.caption == caption),
        None => {
            let ours: Vec<&WindowEntry> = ours.collect();
            match ours.iter().find(|w| &*w.caption == MAIN_CAPTION) {
                Some(main) => Some(*main),
                None if ours.len() == 1 => Some(ours[0]),
                None => None,
            }
        }
    };
    found.ok_or("window is not in the current list")
}

/// The args object the template reads. Fields an op does not use are left out.
#[derive(serde::Serialize)]
struct CmdArgs<'a> {
    plugin: &'a str,
    id: &'a str,
    op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    on: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    y: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<i32>,
}

/// The command template with the placeholder replaced by the args object, built by
/// `serde_json`.
fn cmd_script(plugin: &str, target: &Target) -> String {
    let mut args = CmdArgs {
        plugin,
        id: &target.id,
        op: "",
        on: None,
        x: None,
        y: None,
        width: None,
        height: None,
    };
    match target.op {
        CmdOp::Activate => args.op = "activate",
        CmdOp::Close => args.op = "close",
        CmdOp::KeepAbove(on) => {
            args.op = "keep_above";
            args.on = Some(on);
        }
        CmdOp::Geometry(g) => {
            args.op = "geometry";
            args.x = Some(g.x);
            args.y = Some(g.y);
            args.width = Some(g.width);
            args.height = Some(g.height);
        }
    }
    let json = serde_json::to_string(&args).expect("strings and numbers always serialize");
    CMD_TEMPLATE.replace(CMD_PLACEHOLDER, &json)
}

/// Loads and starts a written script. The worker uses KWin; tests use a fake.
trait ScriptHost {
    async fn run(&self, cx: &Cx, path: &Path, plugin: &str) -> Result<(), WorkerError>;
}

/// `loadScript` and `start` on KWin's `/Scripting`, 2 s each.
struct KWinScripting<'a>(&'a DbusConn);

impl ScriptHost for KWinScripting<'_> {
    async fn run(&self, cx: &Cx, path: &Path, plugin: &str) -> Result<(), WorkerError> {
        let conn = self.0;
        let file = path
            .to_str()
            .ok_or_else(|| WorkerError::Other("script path is not UTF-8".into()))?;
        let scripting = scripting(cx, conn).await?;
        let id: i32 = conn
            .read(cx, scripting.call("loadScript", &(file, plugin)))
            .await?;
        if id < 0 {
            return Err(WorkerError::Dbus("loadScript returned -1".into()));
        }
        conn.read(cx, scripting.call::<_, _, ()>("start", &()))
            .await?;
        Ok(())
    }
}

/// The last command's script file. KWin may read it after `start()` returns (unverified), so it
/// is deleted only when the next command runs, the next `Update` arrives or the worker exits.
/// At most 2 command files exist at once: this one and the one being run.
#[derive(Default)]
struct CmdFiles {
    last: Option<PathBuf>,
}

impl CmdFiles {
    /// Deletes the kept file, if any.
    async fn clear(&mut self, cx: &Cx) {
        if let Some(path) = self.last.take() {
            remove_script(cx, &path).await;
        }
    }
}

impl Drop for CmdFiles {
    /// Worker exit: one unlink on the runtime tmpfs, inline.
    fn drop(&mut self) {
        if let Some(path) = self.last.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Runs the newly queued commands one at a time, in arrival order.
#[allow(clippy::too_many_arguments)]
async fn run_commands(
    cx: &Cx,
    io: &WorkerIo,
    host: &impl ScriptHost,
    dir: &Path,
    commands: &Commands,
    queue: &mut Queue,
    windows: &[WindowEntry],
    files: &mut CmdFiles,
) {
    let new = queue.take(&io.control.borrow_and_clone());
    for (seq, cmd) in new {
        if cx.checkpoint().is_err() {
            return;
        }
        queue.progress.set_started(SourceId::KWin, seq);
        if let Some((plugin, script)) = commands.prepare(&cmd, windows) {
            run_script(cx, host, dir, &plugin, script, files).await;
        }
        queue.progress.set_done(SourceId::KWin, seq);
    }
}

/// Deletes the previous command's file, writes `<dir>/<plugin>.js` (mode 0600) and runs it. The
/// new file is kept in `files`, also after a failure: `loadScript` may have taken it before
/// `start` failed. A failure is logged at `warn` and the command is dropped (R-170).
async fn run_script(
    cx: &Cx,
    host: &impl ScriptHost,
    dir: &Path,
    plugin: &str,
    script: String,
    files: &mut CmdFiles,
) {
    files.clear(cx).await;
    let path = dir.join(format!("{plugin}.js"));
    let result = match write_script(cx, &path, script).await {
        Ok(()) => host.run(cx, &path, plugin).await,
        Err(e) => Err(e),
    };
    files.last = Some(path);
    if let Err(e) = result
        && !cx.is_cancel_requested()
    {
        tracing::warn!(target: "tasklens::engine", plugin, error = %e, "kwin command failed");
    }
}

/// The object KWin's script calls. It checks and parses on the zbus executor and hands
/// accepted lists to the worker loop.
struct WindowBridge {
    /// The current unique name of `org.kde.KWin`, kept up to date by the worker loop.
    owner: Arc<Mutex<Option<String>>>,
    tx: async_channel::Sender<Event>,
}

#[zbus::interface(name = "io.github.oysteinkrog.Tasklens.WindowBridge")]
impl WindowBridge {
    async fn update(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        json: String,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(|name| name.as_str().to_owned());
        let owner = self
            .owner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match check_update(sender.as_deref(), owner.as_deref(), &json) {
            Ok(windows) => self
                .tx
                .send(Event::Windows(windows))
                .await
                .map_err(|_| zbus::fdo::Error::Failed("worker stopped".into())),
            Err(reject) => {
                tracing::debug!(target: "tasklens::engine", reason = reject.as_str(), "kwin update rejected");
                Err(reject.into())
            }
        }
    }
}

/// Why an `Update` was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reject {
    /// The caller is not the current owner of `org.kde.KWin`.
    Sender,
    /// The payload is over `MAX_PAYLOAD` bytes.
    Size,
    /// The payload is not a JSON array.
    Json,
}

impl Reject {
    fn as_str(self) -> &'static str {
        match self {
            Reject::Sender => "sender is not the KWin owner",
            Reject::Size => "payload over 1 MiB",
            Reject::Json => "payload is not a JSON array",
        }
    }
}

impl From<Reject> for zbus::fdo::Error {
    fn from(reject: Reject) -> Self {
        match reject {
            Reject::Sender => zbus::fdo::Error::AccessDenied(reject.as_str().into()),
            Reject::Size => zbus::fdo::Error::LimitsExceeded(reject.as_str().into()),
            Reject::Json => zbus::fdo::Error::InvalidArgs(reject.as_str().into()),
        }
    }
}

/// The `Update` checks and parse (R-167 items 5 and 15). The sender check comes first, then the
/// size; entries that fail to parse or carry a bad `internalId` are dropped.
fn check_update(
    sender: Option<&str>,
    owner: Option<&str>,
    json: &str,
) -> Result<Vec<WindowEntry>, Reject> {
    match (sender, owner) {
        (Some(sender), Some(owner)) if sender == owner => {}
        _ => return Err(Reject::Sender),
    }
    if json.len() > MAX_PAYLOAD {
        return Err(Reject::Size);
    }
    let values: Vec<serde_json::Value> = serde_json::from_str(json).map_err(|_| Reject::Json)?;
    Ok(values.into_iter().filter_map(window_entry).collect())
}

/// One window as the bridge script sends it.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWindow {
    internal_id: String,
    pid: u32,
    caption: String,
    resource_class: String,
    desktop_file_name: String,
    minimized: bool,
    maximized: bool,
    keep_above: bool,
    frame_geometry: RawGeometry,
    window_type: i32,
}

#[derive(serde::Deserialize)]
struct RawGeometry {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn window_entry(value: serde_json::Value) -> Option<WindowEntry> {
    let raw: RawWindow = serde_json::from_value(value).ok()?;
    if !braced_uuid(&raw.internal_id) {
        return None;
    }
    let g = raw.frame_geometry;
    Some(WindowEntry {
        internal_id: raw.internal_id.into(),
        pid: raw.pid,
        caption: clean(&raw.caption),
        resource_class: clean(&raw.resource_class),
        desktop_file_name: clean(&raw.desktop_file_name),
        minimized: raw.minimized,
        maximized: raw.maximized,
        keep_above: raw.keep_above,
        frame_geometry: FrameGeometry {
            x: g.x,
            y: g.y,
            width: g.width,
            height: g.height,
        },
        window_type: raw.window_type,
    })
}

/// `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` with lowercase hex digits.
fn braced_uuid(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 38
        && b[0] == b'{'
        && b[37] == b'}'
        && b[1..37].iter().enumerate().all(|(i, &c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(&c),
        })
}

/// The tl-collect sanitizer with a 4 KiB cut. Line breaks and NUL are C0 controls in window
/// text, so they become U+FFFD here instead of the space process text gets (ADR-015).
fn clean(raw: &str) -> Arc<str> {
    let controls: String = raw
        .chars()
        .map(|c| match c {
            '\n' | '\r' | '\0' => char::REPLACEMENT_CHARACTER,
            c => c,
        })
        .collect();
    let mut out = String::new();
    sanitize_text(controls.as_bytes(), TEXT_MAX, &mut out);
    out.into()
}

/// Entries grouped by pid in ascending order. The sort is stable, so windows of one pid keep
/// the order KWin sent them in (topmost first).
fn snapshot(mut windows: Vec<WindowEntry>, validity: Validity) -> WindowsSnapshot {
    windows.sort_by_key(|w| w.pid);
    let t_ns = now_boot_ns();
    WindowsSnapshot {
        header: ComponentHeader {
            version: 0,
            observed_start: t_ns,
            observed_end: t_ns,
            validity,
        },
        windows: windows.into(),
        pick: None,
    }
}

fn health(state: SourceState, last_error: Option<Arc<str>>) -> SourceUpdate {
    SourceUpdate::Health {
        id: SourceId::KWin,
        state,
        last_error,
        t_ns: now_boot_ns(),
    }
}

/// Sends one update. Returns the worker's result when it must stop: `Ok` on cancel,
/// `Err(Closed)` when the reducer is gone.
async fn send(cx: &Cx, io: &WorkerIo, update: SourceUpdate) -> Option<Outcome<(), WorkerError>> {
    match io.updates.send(cx, EngineMsg::Source(update)).await {
        Ok(()) => None,
        Err(mpsc::SendError::Cancelled(_)) => Some(Outcome::Ok(())),
        Err(_) => Some(Outcome::Err(WorkerError::Closed)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use asupersync::channel::watch;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use serde_json::json;
    use tl_model::demand::Demand;
    use tl_model::settings::EngineSettings;

    use crate::source::WorkerControl;

    const KWIN: &str = ":1.7";
    const ID_A: &str = "{0a1b2c3d-0000-4000-8000-00000000000a}";
    const ID_B: &str = "{0a1b2c3d-0000-4000-8000-00000000000b}";
    const ID_C: &str = "{0a1b2c3d-0000-4000-8000-00000000000c}";

    fn worker_io() -> (WorkerIo, mpsc::Receiver<EngineMsg>) {
        let (updates, rx) = mpsc::channel::<EngineMsg>(256);
        let (_demand_tx, demand) = watch::channel(Demand::default());
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (_control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(
            updates,
            demand,
            settings,
            control,
            crate::dbus::SharedHandles::default(),
        );
        (io, rx)
    }

    fn window(id: &str, pid: u32, caption: &str) -> serde_json::Value {
        json!({
            "internalId": id,
            "pid": pid,
            "caption": caption,
            "resourceClass": "org.kde.konsole",
            "desktopFileName": "org.kde.konsole",
            "minimized": false,
            "maximized": true,
            "keepAbove": false,
            "frameGeometry": { "x": 10, "y": -20, "width": 800, "height": 600 },
            "windowType": 0,
        })
    }

    fn payload(windows: &[serde_json::Value]) -> String {
        serde_json::to_string(windows).expect("json")
    }

    #[test]
    fn kde_desktop_list() {
        assert!(kde_desktop(Some("KDE")));
        assert!(kde_desktop(Some("ubuntu:KDE")));
        assert!(!kde_desktop(Some("kde")));
        assert!(!kde_desktop(Some("GNOME")));
        assert!(!kde_desktop(Some("KDE-ish")));
        assert!(!kde_desktop(None));
    }

    #[test]
    fn unsupported_without_kde() {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let (io, mut rx) = worker_io();
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let _ = run(cx, io, commands(EngineMode::Full), Some("GNOME")).await;
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();

        let mut got = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            got.push(msg);
        }
        assert_eq!(got.len(), 2, "{got:?}");
        match &got[0] {
            EngineMsg::Source(SourceUpdate::Windows(snap)) => {
                assert_eq!(snap.header.validity, Validity::Unsupported);
                assert!(snap.windows.is_empty());
            }
            other => panic!("unexpected message {other:?}"),
        }
        match &got[1] {
            EngineMsg::Source(SourceUpdate::Health {
                id,
                state,
                last_error,
                ..
            }) => {
                assert_eq!(*id, SourceId::KWin);
                assert_eq!(*state, SourceState::Unsupported);
                assert_eq!(last_error.as_deref(), Some(NOT_AVAILABLE));
            }
            other => panic!("unexpected message {other:?}"),
        }

        handle.abort();
        lab.run_until_idle();
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn update_rejected_wrong_sender() {
        let json = payload(&[window(ID_A, 42, "a")]);
        assert_eq!(
            check_update(Some(":1.99"), Some(KWIN), &json),
            Err(Reject::Sender)
        );
        assert_eq!(check_update(None, Some(KWIN), &json), Err(Reject::Sender));
        assert_eq!(check_update(Some(KWIN), None, &json), Err(Reject::Sender));
        assert_eq!(
            check_update(Some(KWIN), Some(KWIN), &json).map(|w| w.len()),
            Ok(1)
        );
    }

    #[test]
    fn update_rejected_oversize() {
        let mut json = payload(&[window(ID_A, 42, "a")]);
        json.push_str(&" ".repeat(MAX_PAYLOAD - json.len()));
        assert_eq!(json.len(), MAX_PAYLOAD);
        assert!(check_update(Some(KWIN), Some(KWIN), &json).is_ok());
        json.push(' ');
        assert_eq!(
            check_update(Some(KWIN), Some(KWIN), &json),
            Err(Reject::Size)
        );
        // The sender check runs first.
        assert_eq!(
            check_update(Some(":1.99"), Some(KWIN), &json),
            Err(Reject::Sender)
        );
        assert_eq!(
            check_update(Some(KWIN), Some(KWIN), "{}"),
            Err(Reject::Json)
        );
    }

    #[test]
    fn update_drops_bad_internal_id() {
        let json = payload(&[
            window(ID_A, 1, "a"),
            window("x\"); evil(\"", 2, "b"),
            window("{0A1B2C3D-0000-4000-8000-00000000000A}", 3, "upper"),
            window("0a1b2c3d-0000-4000-8000-00000000000a", 4, "no braces"),
            json!({ "internalId": ID_C, "pid": -1 }),
            window(ID_B, 5, "e"),
        ]);
        let windows = check_update(Some(KWIN), Some(KWIN), &json).expect("accepted");
        let ids: Vec<&str> = windows.iter().map(|w| &*w.internal_id).collect();
        assert_eq!(ids, [ID_A, ID_B]);
        let w = &windows[0];
        assert_eq!(w.pid, 1);
        assert!(w.maximized && !w.minimized && !w.keep_above);
        assert_eq!(
            w.frame_geometry,
            FrameGeometry {
                x: 10,
                y: -20,
                width: 800,
                height: 600
            }
        );
        assert_eq!(&*w.resource_class, "org.kde.konsole");
    }

    #[test]
    fn update_sanitizes_caption() {
        let json = payload(&[window(ID_A, 1, "abc\u{202E}def\nghi\u{2066}\u{0007}")]);
        let windows = check_update(Some(KWIN), Some(KWIN), &json).expect("accepted");
        let caption = &*windows[0].caption;
        assert!(!caption.contains('\u{202E}'), "{caption:?}");
        assert!(!caption.contains('\u{2066}'), "{caption:?}");
        assert!(!caption.contains('\n'), "{caption:?}");
        assert_eq!(caption, "abcdef\u{FFFD}ghi\u{FFFD}");

        let long = "x".repeat(TEXT_MAX + 10);
        let json = payload(&[window(ID_A, 1, &long)]);
        let windows = check_update(Some(KWIN), Some(KWIN), &json).expect("accepted");
        let caption = &*windows[0].caption;
        assert!(caption.len() <= TEXT_MAX + '\u{2026}'.len_utf8());
        assert!(caption.ends_with('\u{2026}'));
    }

    #[test]
    fn update_groups_by_pid() {
        let json = payload(&[
            window(ID_C, 42, "top"),
            window(ID_B, 7, "other"),
            window(ID_A, 42, "below"),
        ]);
        let windows = check_update(Some(KWIN), Some(KWIN), &json).expect("accepted");
        let snap = snapshot(windows, Validity::Valid);
        assert_eq!(snap.header.validity, Validity::Valid);
        let pids: Vec<u32> = snap.windows.iter().map(|w| w.pid).collect();
        assert_eq!(pids, [7, 42, 42]);
        let ours: Vec<&str> = snap.for_pid(42).iter().map(|w| &*w.caption).collect();
        assert_eq!(ours, ["top", "below"]);
        assert_eq!(snap.for_pid(7).len(), 1);
    }

    #[test]
    fn bridge_script_placeholder_replaced() {
        let name = ":1.4242";
        let script = bridge_script(name);
        assert!(!script.contains("__TL_"), "placeholder left");
        assert_eq!(script.matches(name).count(), 1);
        assert!(script.contains("const BUS_NAME = \":1.4242\";"));
        // The name goes in as a JSON string, so a quote cannot end it.
        let hostile = bridge_script("\"); evil(\"");
        assert!(hostile.contains(r#"const BUS_NAME = "\"); evil(\"";"#));
        // The script calls the path and interface this worker serves.
        let iface = <WindowBridge as zbus::object_server::Interface>::name();
        assert_eq!(
            iface.as_str(),
            "io.github.oysteinkrog.Tasklens.WindowBridge"
        );
        assert!(script.contains(&format!("\"{BRIDGE_PATH}\"")));
        assert!(script.contains(&format!("\"{iface}\"")));
        assert!(script.contains("\"Update\""));
    }

    #[test]
    fn spec_runs_in_both_modes() {
        let s = spec();
        assert_eq!(s.id, SourceId::KWin);
        assert_eq!(s.name, "kwin");
        assert_eq!(s.modes, &[EngineMode::Full, EngineMode::LowMemory]);
        for mode in [EngineMode::Full, EngineMode::LowMemory] {
            let set = crate::workers::workers_for(mode);
            assert_eq!(set.iter().filter(|w| w.id == SourceId::KWin).count(), 1);
        }
    }

    const INJECTION: &str = "{00000000-0000-0000-0000-000000000000}\"); workspace.windowList().forEach(w=>w.closeWindow()); (\"";

    fn commands(mode: EngineMode) -> Commands {
        Commands {
            mode,
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    fn entry(id: &str, pid: u32, caption: &str, class: &str) -> WindowEntry {
        WindowEntry {
            internal_id: id.into(),
            pid,
            caption: caption.into(),
            resource_class: class.into(),
            desktop_file_name: class.into(),
            minimized: false,
            maximized: false,
            keep_above: false,
            frame_geometry: FrameGeometry::default(),
            window_type: 0,
        }
    }

    fn activate(id: &str) -> KWinCommand {
        KWinCommand::Activate {
            internal_id: id.into(),
        }
    }

    /// The args object as the script holds it.
    fn args_of(script: &str) -> serde_json::Value {
        let start = script.find("const args = ").expect("args line") + "const args = ".len();
        let end = start + script[start..].find(";\n").expect("args line ends");
        serde_json::from_str(&script[start..end]).expect("args are JSON")
    }

    #[test]
    fn cmd_script_activate_args() {
        let windows = [entry(ID_A, 42, "a", "org.kde.konsole")];
        let (plugin, script) = commands(EngineMode::Full)
            .prepare(&activate(ID_A), &windows)
            .expect("known window");
        assert_eq!(plugin, "tasklens-cmd-1");
        assert!(!script.contains(CMD_PLACEHOLDER));
        assert!(script.contains("\"op\":\"activate\""), "{script}");
        assert!(script.contains(&format!("\"id\":\"{ID_A}\"")), "{script}");
        assert_eq!(
            args_of(&script),
            json!({ "plugin": "tasklens-cmd-1", "id": ID_A, "op": "activate" })
        );

        let (_, script) = commands(EngineMode::Full)
            .prepare(
                &KWinCommand::Close {
                    internal_id: ID_A.into(),
                },
                &windows,
            )
            .expect("known window");
        assert_eq!(args_of(&script)["op"], "close");
    }

    #[test]
    fn cmd_rejects_injection_id() {
        // Even a list entry with this id does not let it through.
        let windows = [
            entry(ID_A, 42, "a", "org.kde.konsole"),
            entry(INJECTION, 42, "b", "org.kde.konsole"),
        ];
        let cmds = commands(EngineMode::Full);
        assert_eq!(cmds.prepare(&activate(INJECTION), &windows), None);
        let close = KWinCommand::Close {
            internal_id: INJECTION.into(),
        };
        assert_eq!(cmds.prepare(&close, &windows), None);
        // A secondary window lookup that lands on the bad entry is refused too.
        let windows = [entry(INJECTION, 7, "tasklens", APP_ID)];
        let keep = KWinCommand::KeepAbove {
            pid: 7,
            caption: None,
            on: true,
        };
        assert_eq!(cmds.prepare(&keep, &windows), None);
        // Nothing was numbered, so no script was built.
        assert_eq!(cmds.counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cmd_unknown_id_dropped() {
        let windows = [entry(ID_A, 42, "a", "org.kde.konsole")];
        let cmds = commands(EngineMode::Full);
        assert_eq!(cmds.prepare(&activate(ID_B), &windows), None);
        assert_eq!(cmds.prepare(&activate(ID_A), &[]), None);
        let geometry = KWinCommand::SetFrameGeometry {
            pid: 42,
            caption: "tasklens Graph 1".into(),
            geometry: FrameGeometry::default(),
        };
        assert_eq!(cmds.prepare(&geometry, &windows), None);
        assert_eq!(cmds.prepare(&KWinCommand::Pick { seq: 1 }, &windows), None);
        assert_eq!(cmds.counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cmd_plugin_counter_increments() {
        let windows = [
            entry(ID_A, 42, "a", "org.kde.konsole"),
            entry(ID_B, 42, "b", "org.kde.konsole"),
        ];
        let cmds = commands(EngineMode::Full);
        let names: Vec<String> = [
            activate(ID_A),
            activate(ID_B),
            activate(ID_C),
            activate(ID_A),
        ]
        .iter()
        .filter_map(|cmd| cmds.prepare(cmd, &windows))
        .map(|(plugin, script)| {
            assert_eq!(args_of(&script)["plugin"], plugin.as_str());
            plugin
        })
        .collect();
        assert_eq!(
            names,
            ["tasklens-cmd-1", "tasklens-cmd-2", "tasklens-cmd-3"]
        );
    }

    #[test]
    fn cmd_geometry_clamped() {
        let windows = [entry(ID_A, 7, "tasklens Graph 1", APP_ID)];
        let cmd = KWinCommand::SetFrameGeometry {
            pid: 7,
            caption: "tasklens Graph 1".into(),
            geometry: FrameGeometry {
                x: i32::MIN,
                y: i32::MAX,
                width: 0,
                height: -5,
            },
        };
        let (_, script) = commands(EngineMode::Full)
            .prepare(&cmd, &windows)
            .expect("own window");
        assert_eq!(
            args_of(&script),
            json!({
                "plugin": "tasklens-cmd-1",
                "id": ID_A,
                "op": "geometry",
                "x": i32::MIN,
                "y": i32::MAX,
                "width": 1,
                "height": 1,
            })
        );
    }

    #[test]
    fn cmd_keep_above_finds_own_window() {
        let windows = [
            entry(ID_A, 7, "tasklens Graph 1", APP_ID),
            entry(ID_B, 7, "tasklens", APP_ID),
            entry(ID_C, 7, "tasklens Graph 2", "org.kde.konsole"),
        ];
        let keep = |caption: Option<&str>, on| KWinCommand::KeepAbove {
            pid: 7,
            caption: caption.map(Into::into),
            on,
        };
        let cmds = commands(EngineMode::Full);
        let (_, script) = cmds.prepare(&keep(None, true), &windows).expect("main");
        let args = args_of(&script);
        assert_eq!(
            (&args["id"], &args["op"], &args["on"]),
            (&json!(ID_B), &json!("keep_above"), &json!(true))
        );
        let (_, script) = cmds
            .prepare(&keep(Some("tasklens Graph 1"), false), &windows)
            .expect("secondary");
        let args = args_of(&script);
        assert_eq!((&args["id"], &args["on"]), (&json!(ID_A), &json!(false)));
        // Another app's window with a matching caption is never chosen.
        assert_eq!(
            cmds.prepare(&keep(Some("tasklens Graph 2"), true), &windows),
            None
        );

        // The rescue window is the process's only tasklens window. The low-memory engine runs
        // keep above and nothing else.
        let rescue = [entry(ID_C, 9, "tasklens - Low memory", APP_ID)];
        let low = commands(EngineMode::LowMemory);
        let pinned = KWinCommand::KeepAbove {
            pid: 9,
            caption: None,
            on: true,
        };
        let (_, script) = low.prepare(&pinned, &rescue).expect("rescue window");
        assert_eq!(args_of(&script)["id"], ID_C);
        assert_eq!(low.prepare(&activate(ID_C), &rescue), None);
        // Two unnamed tasklens windows and no main one: no guess.
        let two = [
            entry(ID_A, 9, "tasklens Graph 1", APP_ID),
            entry(ID_B, 9, "tasklens Graph 2", APP_ID),
        ];
        assert_eq!(low.prepare(&pinned, &two), None);
    }

    /// Records each script as KWin would read it, and fails when told to.
    struct FakeHost {
        seen: Mutex<Vec<(String, String, u32)>>,
        fail: bool,
    }

    impl ScriptHost for FakeHost {
        async fn run(&self, _cx: &Cx, path: &Path, plugin: &str) -> Result<(), WorkerError> {
            let text = std::fs::read_to_string(path).expect("script was written");
            let mode = std::fs::metadata(path).expect("script metadata").mode() & 0o777;
            self.seen
                .lock()
                .expect("lock")
                .push((plugin.to_owned(), text, mode));
            if self.fail {
                Err(WorkerError::Dbus("loadScript returned -1".into()))
            } else {
                Ok(())
            }
        }
    }

    /// A fresh directory under the target dir for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("kwin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The file names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Runs `body` as one lab task and checks the lab report.
    fn in_lab(body: impl Future<Output = ()> + Send + 'static) {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, body)
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn cmd_file_kept_until_next_command() {
        let dir = scratch("cmd-file").join("tasklens");
        let windows = [entry(ID_A, 42, "a", "org.kde.konsole")];
        let cmds = commands(EngineMode::Full);
        let host = Arc::new(FakeHost {
            seen: Mutex::new(Vec::new()),
            fail: false,
        });
        let failing = Arc::new(FakeHost {
            seen: Mutex::new(Vec::new()),
            fail: true,
        });
        // The directory listing after each step.
        let steps = Arc::new(Mutex::new(Vec::new()));

        {
            let (dir, host, failing) = (dir.clone(), Arc::clone(&host), Arc::clone(&failing));
            let steps = Arc::clone(&steps);
            in_lab(async move {
                let cx = Cx::current().expect("task has a Cx");
                let step = || steps.lock().expect("lock").push(names(&dir));
                let mut files = CmdFiles::default();
                let (plugin, script) = cmds.prepare(&activate(ID_A), &windows).expect("known");
                run_script(&cx, &*host, &dir, &plugin, script, &mut files).await;
                step();
                let (plugin, script) = cmds.prepare(&activate(ID_A), &windows).expect("known");
                run_script(&cx, &*failing, &dir, &plugin, script, &mut files).await;
                step();
                // The next `Update`.
                files.clear(&cx).await;
                step();
                let (plugin, script) = cmds.prepare(&activate(ID_A), &windows).expect("known");
                run_script(&cx, &*host, &dir, &plugin, script, &mut files).await;
                step();
                // Worker exit.
                drop(files);
                step();
            });
        }

        let seen = host.seen.lock().expect("lock");
        assert_eq!(seen.len(), 2);
        let (plugin, text, mode) = &seen[0];
        assert_eq!(plugin, "tasklens-cmd-1");
        assert!(text.contains("\"op\":\"activate\""), "{text}");
        assert!(text.contains(&format!("\"{ID_A}\"")), "{text}");
        assert_eq!(*mode, 0o600);
        assert_eq!(failing.seen.lock().expect("lock")[0].0, "tasklens-cmd-2");
        let none: Vec<String> = Vec::new();
        assert_eq!(
            *steps.lock().expect("lock"),
            [
                vec!["tasklens-cmd-1.js".to_owned()],
                // The second command deleted the first file. A failed command keeps its file.
                vec!["tasklens-cmd-2.js".to_owned()],
                none.clone(),
                vec!["tasklens-cmd-3.js".to_owned()],
                none,
            ]
        );
        let _ = std::fs::remove_dir_all(dir.parent().expect("scratch"));
    }

    #[test]
    fn stale_cmd_files_removed_at_start() {
        let dir = scratch("stale").join("tasklens");
        std::fs::create_dir_all(&dir).expect("dir");
        for name in [
            "tasklens-cmd-7.js",
            "tasklens-cmd-12.js",
            "kwin-bridge.js",
            "tasklens-cmd-.js",
            "tasklens-cmd-x.js",
            "tasklens-cmd-7.js.tmp",
        ] {
            std::fs::write(dir.join(name), "").expect("write");
        }
        std::fs::create_dir(dir.join("tasklens-cmd-8.js")).expect("subdir");
        {
            let dir = dir.clone();
            in_lab(async move {
                let cx = Cx::current().expect("task has a Cx");
                remove_stale(&cx, &dir).await;
                // A missing directory is fine.
                remove_stale(&cx, &dir.join("missing")).await;
            });
        }
        assert_eq!(
            names(&dir),
            [
                "kwin-bridge.js",
                "tasklens-cmd-.js",
                "tasklens-cmd-7.js.tmp",
                "tasklens-cmd-8.js",
                "tasklens-cmd-x.js",
            ]
        );
        let _ = std::fs::remove_dir_all(dir.parent().expect("scratch"));
    }
}
