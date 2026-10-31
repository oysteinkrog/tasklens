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
//!
//! The window picker (E14-019) runs beside the loop: `queryWindowInfo` waits in a child task for
//! the user's click, and the answer goes out as `WindowsSnapshot.pick` with the window list. On
//! KWin 6.7.5 the reply carries `pid` (U-14); the `tasklens-pick` script is the fallback when it
//! does not.

use std::collections::HashMap;
use std::fs::{DirBuilder, Permissions};
use std::future::Future;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use asupersync::channel::{mpsc, watch};
use asupersync::runtime::{JoinError, TaskHandle};
use asupersync::supervision::ManagedGeneration;
use asupersync::time;
use asupersync::types::Time;
use asupersync::{Cx, Outcome};
use futures_lite::stream::Boxed;
use futures_lite::{Stream, StreamExt};
use tl_collect::sanitize::sanitize_text;
use tl_model::actions::ActionError;
use tl_model::command::KWinCommand;
use tl_model::health::{SourceId, SourceState};
use tl_model::snapshot::{
    ComponentHeader, FrameGeometry, PickOutcome, PickResult, WindowEntry, WindowsSnapshot,
};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{Bus, DbusConn, DbusError, READ_TIMEOUT, connect, error_name};
use crate::desktop::{ActivateCall, DesktopEnv, DesktopSession, DesktopSource};
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
/// The window picker lookup script (E14-017), loaded only when `queryWindowInfo` has no `pid`.
const PICK_SCRIPT: &str = include_str!("../../../../assets/kwin/pick.js");
/// The pick script's plugin name (table (c)).
const PICK_PLUGIN: &str = "tasklens-pick";
/// `queryWindowInfo` lives here.
const KWIN_PATH: &str = "/KWin";
const KWIN_INTERFACE: &str = "org.kde.KWin";
/// The deadline for `queryWindowInfo`: the user picks interactively.
const PICK_TIMEOUT: Duration = Duration::from_secs(120);
/// How long the pick script has to call `Picked`.
const PICK_SCRIPT_WAIT: Duration = Duration::from_secs(2);
/// The `queryWindowInfo` error for a click on something KWin does not manage (KWin 6.7.5).
const INVALID_WINDOW: &str = "org.kde.KWin.Error.InvalidWindow";
/// KWin `WindowType` values of shell windows (KWin 6.7.5 `effect/globals.h`): desktop 1, dock 2,
/// notification 13, on-screen display 16, critical notification 17.
const SHELL_TYPES: [i32; 5] = [1, 2, 13, 16, 17];
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
/// The health text while the engine service's host has no desktop session (ADR-018).
const NO_SESSION: &str = "No desktop session";

const SYSTEMD_NAME: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
/// How long the engine service keeps reading the user manager's environment after KWin
/// appeared while it names no display, and the first wait between reads. Each wait doubles:
/// reads at 0, 0.25, 0.75, 1.75, 3.75, 7.75 and 10 s, then none until KWin's name changes.
const ENV_WAIT: Duration = Duration::from_secs(10);
const ENV_RETRY: Duration = Duration::from_millis(250);

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
        let desktop = cfg.desktop.clone();
        let commands = Commands {
            mode: cfg.mode,
            counter: Arc::clone(&counter),
            desktop: desktop.clone(),
        };
        async move {
            match desktop.source() {
                DesktopSource::Process => {
                    let current = std::env::var("XDG_CURRENT_DESKTOP").ok();
                    run(cx, io, commands, current.as_deref()).await
                }
                DesktopSource::UserManager => run_service(cx, io, commands, desktop).await,
            }
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
    let result = serve(
        &cx,
        &io,
        &conn,
        &path,
        &commands,
        &mut queue,
        &mut files,
        KWinGone::Wait,
    )
    .await;
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

/// The engine service's bridge (ADR-018 "Commands that act on the host's desktop"). The
/// service starts with the user manager, often before anyone logs in, so its own environment
/// says nothing about the desktop. It watches KWin's name on the user bus instead: when KWin
/// appears it reads the user manager's environment, which Plasma fills at login, records the
/// session in `desktop` and runs the bridge; when KWin leaves it records that there is no
/// session and waits for the next one.
async fn run_service(
    cx: Cx,
    io: WorkerIo,
    commands: Commands,
    desktop: DesktopSession,
) -> Outcome<(), WorkerError> {
    let mut queue = match resume(&cx, &io).await {
        Ok(queue) => queue,
        Err(stop) => return stop,
    };
    let mut bus = UserBus { conn: None };
    match session_cycles(&cx, &io, &desktop, &mut bus, &commands, &mut queue).await {
        Ok(()) => Outcome::Ok(()),
        Err(_) if cx.is_cancel_requested() => Outcome::Ok(()),
        Err(e) => failed(&cx, &io, e).await,
    }
}

/// The user bus side of the service's desktop session. The worker uses D-Bus; tests use a
/// fake.
trait SessionBus {
    /// Returns once KWin owns its name. Commands queued meanwhile are dropped: they act on a
    /// screen nobody is logged in to.
    async fn wait_kwin(
        &mut self,
        cx: &Cx,
        io: &WorkerIo,
        queue: &mut Queue,
    ) -> Result<(), WorkerError>;

    /// The user manager's `Environment`, as `KEY=VALUE` lines.
    async fn manager_env(&mut self, cx: &Cx) -> Result<Vec<String>, WorkerError>;

    /// Runs the bridge until KWin leaves the bus.
    async fn bridge(
        &mut self,
        cx: &Cx,
        io: &WorkerIo,
        commands: &Commands,
        queue: &mut Queue,
    ) -> Result<(), WorkerError>;
}

/// One desktop session after another until `cx` is cancelled (`Ok`) or the bus fails (`Err`).
/// However it ends, the session is cleared: the worker that would notice KWin leaving is gone,
/// so the host has no desktop session until the next generation finds KWin again.
async fn session_cycles(
    cx: &Cx,
    io: &WorkerIo,
    desktop: &DesktopSession,
    bus: &mut impl SessionBus,
    commands: &Commands,
    queue: &mut Queue,
) -> Result<(), WorkerError> {
    let result = cycles(cx, io, desktop, bus, commands, queue).await;
    desktop.set(None);
    result
}

async fn cycles(
    cx: &Cx,
    io: &WorkerIo,
    desktop: &DesktopSession,
    bus: &mut impl SessionBus,
    commands: &Commands,
    queue: &mut Queue,
) -> Result<(), WorkerError> {
    loop {
        desktop.set(None);
        let empty = SourceUpdate::Windows(Box::new(snapshot(Vec::new(), Validity::Unsupported)));
        if send(cx, io, empty).await.is_some() {
            return closed(cx);
        }
        let state = health(SourceState::Unsupported, Some(NO_SESSION.into()));
        if send(cx, io, state).await.is_some() {
            return closed(cx);
        }
        bus.wait_kwin(cx, io, queue).await?;
        if cx.is_cancel_requested() {
            return Ok(());
        }
        let env = session_env(cx, bus).await?;
        if cx.is_cancel_requested() {
            return Ok(());
        }
        desktop.set(Some(env));
        bus.bridge(cx, io, commands, queue).await?;
        if cx.is_cancel_requested() {
            return Ok(());
        }
    }
}

/// The session's variables from the user manager, read when KWin's name appears. The user
/// manager sends no signal when its environment changes, and Plasma may import
/// `WAYLAND_DISPLAY` a moment after KWin takes its name. So a read that names no display is
/// read again with a doubling wait from `ENV_RETRY`, for at most `ENV_WAIT`. Then the session
/// starts with what it has, and nothing reads the environment again until KWin's name changes.
/// There is no steady polling (leader decision on q4x76.12).
async fn session_env(cx: &Cx, bus: &mut impl SessionBus) -> Result<DesktopEnv, WorkerError> {
    let start = cx.now();
    let mut wait = ENV_RETRY;
    loop {
        let env = DesktopEnv::from_lines(&bus.manager_env(cx).await?);
        let waited = Duration::from_nanos(cx.now().duration_since(start));
        if env.has_display() || waited >= ENV_WAIT {
            if !env.has_display() {
                tracing::info!(target: "tasklens::engine", "desktop session has no display variable");
            }
            return Ok(env);
        }
        time::sleep(cx.now(), wait.min(ENV_WAIT - waited)).await;
        if cx.checkpoint().is_err() {
            return Ok(env);
        }
        wait = wait.saturating_mul(2);
    }
}

/// `SessionBus` on the user bus, which the user manager gives the service as its session bus.
/// Each bridge run gets its own connection, so a new run never finds the last run's
/// `WindowBridge` object still registered.
struct UserBus {
    conn: Option<DbusConn>,
}

impl UserBus {
    async fn conn(&mut self, cx: &Cx) -> Result<&DbusConn, WorkerError> {
        if self.conn.is_none() {
            self.conn = Some(connect(cx, Bus::Session).await?);
        }
        Ok(self.conn.as_ref().expect("connected above"))
    }
}

impl SessionBus for UserBus {
    async fn wait_kwin(
        &mut self,
        cx: &Cx,
        io: &WorkerIo,
        queue: &mut Queue,
    ) -> Result<(), WorkerError> {
        let conn = self.conn(cx).await?;
        // Subscribe before asking so no change is missed in between.
        let bus = conn
            .read(
                cx,
                zbus::Proxy::new(conn.raw(), DBUS_NAME, DBUS_PATH, DBUS_INTERFACE),
            )
            .await?;
        let owner_signals = conn
            .read(
                cx,
                bus.receive_signal_with_args("NameOwnerChanged", &[(0, KWIN_NAME)]),
            )
            .await?;
        let present: bool = conn
            .read(cx, bus.call("NameHasOwner", &(KWIN_NAME,)))
            .await?;
        drop(bus);
        if present {
            return Ok(());
        }
        let owner_events = owner_signals.filter_map(|msg| owner_event(&msg));
        let control_events = control_changes(cx, io.control.clone());
        let mut events: Boxed<Event> = owner_events.or(control_events).boxed();
        loop {
            queue.drop_all(&io.control.borrow_and_clone());
            match conn.next_signal(cx, &mut events, READ_TIMEOUT).await {
                Ok(Some(Event::Owner(Some(_)))) => return Ok(()),
                Ok(_) => {}
                Err(DbusError::Closed) if cx.is_cancel_requested() => return Ok(()),
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn manager_env(&mut self, cx: &Cx) -> Result<Vec<String>, WorkerError> {
        let conn = self.conn(cx).await?;
        let call = conn.raw().call_method(
            Some(SYSTEMD_NAME),
            SYSTEMD_PATH,
            Some(PROPERTIES_INTERFACE),
            "Get",
            &(SYSTEMD_MANAGER, "Environment"),
        );
        let reply = conn.read(cx, call).await?;
        let value: zbus::zvariant::OwnedValue = reply
            .body()
            .deserialize()
            .map_err(|e| WorkerError::Dbus(format!("Environment: {e}").into()))?;
        Vec::<String>::try_from(value)
            .map_err(|e| WorkerError::Dbus(format!("Environment: {e}").into()))
    }

    async fn bridge(
        &mut self,
        cx: &Cx,
        io: &WorkerIo,
        commands: &Commands,
        queue: &mut Queue,
    ) -> Result<(), WorkerError> {
        let conn = match self.conn.take() {
            Some(conn) => conn,
            None => connect(cx, Bus::Session).await?,
        };
        let Some(path) = script_path() else {
            return Err(WorkerError::Other("XDG_RUNTIME_DIR is not set".into()));
        };
        if let Some(dir) = path.parent() {
            remove_stale(cx, dir).await;
        }
        let mut files = CmdFiles::default();
        let result = serve(
            cx,
            io,
            &conn,
            &path,
            commands,
            queue,
            &mut files,
            KWinGone::Return,
        )
        .await;
        unload_on_exit(cx, &conn).await;
        let _ = std::fs::remove_file(&path);
        drop(files);
        result
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
    /// The answer to the `queryWindowInfo` of pick `seq`.
    PickReply {
        seq: u64,
        reply: Result<PickInfo, QueryError>,
    },
    /// An accepted `Picked` call: the window's pid, `None` for `"0"` or text that is not a `u32`.
    Picked { uuid: String, pid: Option<u32> },
    /// Switch to window from the action gate (`DesktopSession::activate_window`).
    Activate(ActivateCall),
}

/// What `serve` does when KWin leaves the bus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum KWinGone {
    /// Publish an empty list and wait for KWin to come back (the window's engine).
    Wait,
    /// Return `Ok`: the desktop session ended (the engine service).
    Return,
}

/// Serves the bridge until `cx` is cancelled (`Ok`), KWin leaves under `KWinGone::Return`
/// (`Ok`) or the bus fails (`Err`).
#[allow(clippy::too_many_arguments)]
async fn serve(
    cx: &Cx,
    io: &WorkerIo,
    conn: &DbusConn,
    path: &Path,
    commands: &Commands,
    queue: &mut Queue,
    files: &mut CmdFiles,
    gone: KWinGone,
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
    let mut picker = Picker::new(commands.mode, &unique, tx.clone());
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
    // Switch to window calls are taken while this lives; dropping it fails those still queued.
    // A call checked against an earlier bridge is dropped (ADR-018 "Generations").
    let serving = commands.desktop.bridge_serving();

    let owner_events = owner_signals.filter_map(|msg| owner_event(&msg));
    let control_events = control_changes(cx, io.control.clone());
    let activate_events = commands.desktop.activate_calls().map(Event::Activate);
    let mut events: Boxed<Event> = rx
        .or(owner_events)
        .or(control_events)
        .or(activate_events)
        .boxed();
    let dir = path
        .parent()
        .ok_or_else(|| WorkerError::Other("script path has no directory".into()))?;
    let host = KWinScripting(conn);
    // The list commands are checked against. `None` until the first `Update`, so commands queued
    // across a restart wait for it.
    let mut current: Option<Arc<[WindowEntry]>> = None;
    // The last published list, so a pick result can go out with it.
    let mut shown: Option<(Arc<[WindowEntry]>, Validity)> = None;
    // Switch to window calls waiting, like queued commands, for the first `Update`.
    let mut activations: Vec<ActivateCall> = Vec::new();
    loop {
        let wait = picker
            .wait(cx.now())
            .map_or(READ_TIMEOUT, |d| d.min(READ_TIMEOUT));
        // True when `picker.last` changed and must be published.
        let mut picked = false;
        match conn.next_signal(cx, &mut events, wait).await {
            Ok(None | Some(Event::Control)) => {}
            Ok(Some(Event::Windows(windows))) => {
                // The first `Update` read after a command: KWin has run its script.
                files.clear(cx).await;
                let mut snap = snapshot(windows, Validity::Valid);
                snap.pick = picker.last;
                current = Some(Arc::clone(&snap.windows));
                shown = Some((Arc::clone(&snap.windows), Validity::Valid));
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
                } else if gone == KWinGone::Return {
                    return Ok(());
                } else {
                    current = Some(Arc::from([]));
                    shown = Some((Arc::from([]), Validity::Unsupported));
                    let gone = windows_update(Arc::from([]), Validity::Unsupported, picker.last);
                    if send(cx, io, gone).await.is_some() {
                        return closed(cx);
                    }
                }
            }
            Ok(Some(event @ (Event::PickReply { .. } | Event::Picked { .. }))) => {
                picked = picker.handle(cx, &host, dir, files, event).await;
            }
            Ok(Some(Event::Activate(call))) => activations.extend(serving.accept(call)),
            Err(DbusError::Closed) if cx.is_cancel_requested() => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        picked |= picker.expire(cx.now());
        if let Some(windows) = &current {
            picked |= run_commands(
                cx,
                io,
                &host,
                dir,
                commands,
                queue,
                windows,
                files,
                &mut picker,
            )
            .await;
            for call in activations.drain(..) {
                let result = activate(cx, &host, dir, commands, windows, files, &call).await;
                // The caller may have timed out and gone.
                let _ = call.reply.try_send(result);
            }
        }
        // Without a published list the result waits for the next `Update`.
        if picked && let Some((windows, validity)) = &shown {
            let update = windows_update(Arc::clone(windows), *validity, picker.last);
            if send(cx, io, update).await.is_some() {
                return closed(cx);
            }
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
    /// The new commands, each with the app id its session announced (`WorkerCommand::KWin`).
    fn take(&mut self, control: &WorkerControl) -> Vec<(u64, KWinCommand, Option<Arc<str>>)> {
        let new = control
            .commands
            .iter()
            .filter(|c| c.seq > self.last_seq && c.target == SourceId::KWin)
            .filter_map(|c| match &c.command {
                WorkerCommand::KWin { cmd, app_id } => Some((c.seq, cmd.clone(), app_id.clone())),
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
        if let Some(&(seq, ..)) = self.take(control).last() {
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
    /// The app ids own-window commands match, and the Switch to window calls.
    desktop: DesktopSession,
}

impl Commands {
    /// `prepare_from` for a command no session announced an app id for: the engine's own.
    fn prepare(&self, cmd: &KWinCommand, windows: &[WindowEntry]) -> Option<(String, String)> {
        self.prepare_from(cmd, None, windows)
    }

    /// The plugin name and script text for `cmd`, or `None` when the command is dropped.
    /// `app_id` is the app id the sending session announced (`WorkerCommand::KWin`).
    fn prepare_from(
        &self,
        cmd: &KWinCommand,
        app_id: Option<&Arc<str>>,
        windows: &[WindowEntry],
    ) -> Option<(String, String)> {
        let target = match resolve(cmd, app_id, windows, self.mode, &self.desktop) {
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

/// Finds the window `cmd` names in `windows`. Keep above and frame geometry act only on the
/// sending session's own window: `pid` with the session's announced `app_id`
/// (`DesktopSession::own_app_id`). `Err` holds the reason the command is dropped; it never holds
/// the id or a caption.
fn resolve(
    cmd: &KWinCommand,
    app_id: Option<&Arc<str>>,
    windows: &[WindowEntry],
    mode: EngineMode,
    desktop: &DesktopSession,
) -> Result<Target, &'static str> {
    if mode == EngineMode::LowMemory && !matches!(cmd, KWinCommand::KeepAbove { .. }) {
        return Err("the low-memory engine runs only keep above");
    }
    let (window, op) = match cmd {
        KWinCommand::Activate { internal_id } => (by_id(windows, internal_id)?, CmdOp::Activate),
        KWinCommand::Close { internal_id } => (by_id(windows, internal_id)?, CmdOp::Close),
        KWinCommand::KeepAbove { pid, caption, on } => (
            own_window(
                windows,
                *pid,
                &own_class(desktop, *pid, app_id)?,
                caption.as_deref(),
            )?,
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
                own_window(
                    windows,
                    *pid,
                    &own_class(desktop, *pid, app_id)?,
                    Some(caption),
                )?,
                CmdOp::Geometry(g),
            )
        }
        // `run_commands` hands these to the `Picker`.
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

/// The `resourceClass` the sending session's own windows carry.
fn own_class(
    desktop: &DesktopSession,
    pid: u32,
    app_id: Option<&Arc<str>>,
) -> Result<Arc<str>, &'static str> {
    desktop
        .own_app_id(pid, app_id)
        .ok_or("not the sending session's own window")
}

/// A tasklens window: `pid` and `class`, the app id the sending session announced, plus the
/// caption for a secondary window (APP-009, R-086). `None` is the main window: the one titled
/// "tasklens", or the only tasklens window of `pid` (the low-memory rescue window).
fn own_window<'a>(
    windows: &'a [WindowEntry],
    pid: u32,
    class: &str,
    caption: Option<&str>,
) -> Result<&'a WindowEntry, &'static str> {
    let mut ours = windows
        .iter()
        .filter(|w| w.pid == pid && &*w.resource_class == class);
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

/// The KWin calls of the window picker. The worker uses KWin; tests use a fake.
trait PickHost: ScriptHost {
    /// Starts `queryWindowInfo` in a child task. The answer arrives on `tx` as
    /// `Event::PickReply { seq, .. }`.
    fn query(
        &self,
        cx: &Cx,
        seq: u64,
        tx: async_channel::Sender<Event>,
    ) -> Result<TaskHandle<()>, WorkerError>;

    /// `unloadScript(plugin)`: clears a pick script that never ran to its end.
    async fn unload(&self, cx: &Cx, plugin: &str) -> Result<(), WorkerError>;
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

impl PickHost for KWinScripting<'_> {
    fn query(
        &self,
        cx: &Cx,
        seq: u64,
        tx: async_channel::Sender<Event>,
    ) -> Result<TaskHandle<()>, WorkerError> {
        // The task owns a clone of the connection and builds its proxy inside, so neither
        // outlives the generation: the `Picker` aborts the task when it is dropped.
        let raw = self.0.raw().clone();
        cx.spawn(move |cx| async move {
            let call = async {
                let kwin = zbus::Proxy::new(&raw, KWIN_NAME, KWIN_PATH, KWIN_INTERFACE).await?;
                kwin.call::<_, _, HashMap<String, zbus::zvariant::OwnedValue>>(
                    "queryWindowInfo",
                    &(),
                )
                .await
            };
            let result = time::timeout(cx.now(), PICK_TIMEOUT, call).await;
            if cx.checkpoint().is_err() {
                return;
            }
            let reply = match result {
                Ok(Ok(map)) => Ok(pick_info(&map)),
                Ok(Err(e)) => Err(QueryError::Failed(error_name(&e).unwrap_or("zbus").into())),
                Err(_) => Err(QueryError::Timeout),
            };
            let _ = tx.send(Event::PickReply { seq, reply }).await;
        })
        .map_err(|e| WorkerError::Other(format!("spawn: {e}").into()))
    }

    async fn unload(&self, cx: &Cx, plugin: &str) -> Result<(), WorkerError> {
        let conn = self.0;
        let scripting = scripting(cx, conn).await?;
        let _was_loaded: bool = conn
            .read(cx, scripting.call("unloadScript", &(plugin,)))
            .await?;
        Ok(())
    }
}

/// What `queryWindowInfo` said about the picked window. A missing or mistyped key is `None`.
#[derive(Clone, Debug, Default, PartialEq)]
struct PickInfo {
    uuid: Option<String>,
    /// `None` also for a pid of 0 or less.
    pid: Option<u32>,
    window_type: Option<i32>,
}

/// Why `queryWindowInfo` gave no map.
#[derive(Clone, Debug, PartialEq)]
enum QueryError {
    /// No answer within `PICK_TIMEOUT`.
    Timeout,
    /// A D-Bus error, such as `org.kde.KWin.Error.UserCancel` when the user pressed Esc. Holds
    /// the error name only.
    Failed(Arc<str>),
}

/// Reads `uuid`, `pid` (D-Bus `i`) and `type` (D-Bus `i`) from the `queryWindowInfo` map.
fn pick_info(map: &HashMap<String, zbus::zvariant::OwnedValue>) -> PickInfo {
    let int = |key: &str| map.get(key).and_then(|v| i32::try_from(&**v).ok());
    PickInfo {
        uuid: map.get("uuid").and_then(|v| String::try_from(&**v).ok()),
        pid: int("pid")
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|&pid| pid > 0),
        window_type: int("type"),
    }
}

/// The pick script with the bus name and the args object `{ plugin, uuid }` put in as JSON.
fn pick_script(unique_name: &str, uuid: &str) -> String {
    let name = serde_json::to_string(unique_name).expect("a str always serializes");
    let args = serde_json::json!({ "plugin": PICK_PLUGIN, "uuid": uuid }).to_string();
    PICK_SCRIPT
        .replace(PLACEHOLDER, &name)
        .replace(CMD_PLACEHOLDER, &args)
}

/// The one running window pick and the last result (R-149 items 3 and 4).
struct Picker {
    /// False in the low-memory engine, which runs keep above only (R-152).
    enabled: bool,
    /// This connection's unique name, for the pick script.
    unique: String,
    tx: async_channel::Sender<Event>,
    pending: Option<Pending>,
    /// Sent with every window-list publish after it is set.
    last: Option<PickResult>,
}

struct Pending {
    seq: u64,
    stage: Stage,
    /// The `queryWindowInfo` task.
    query: TaskHandle<()>,
}

enum Stage {
    /// Waiting for the user's click.
    Query,
    /// The pick script runs; `Picked` must come for `uuid` before `deadline`.
    Script { uuid: String, deadline: Time },
}

impl Picker {
    fn new(mode: EngineMode, unique: &str, tx: async_channel::Sender<Event>) -> Self {
        Picker {
            enabled: mode == EngineMode::Full,
            unique: unique.to_owned(),
            tx,
            pending: None,
            last: None,
        }
    }

    /// `Pick { seq }`. Ignored while a pick runs. Returns true when there is a new result.
    fn start(&mut self, cx: &Cx, host: &impl PickHost, seq: u64) -> bool {
        if !self.enabled || self.pending.is_some() {
            tracing::debug!(target: "tasklens::engine", seq, "window pick ignored");
            return false;
        }
        match host.query(cx, seq, self.tx.clone()) {
            Ok(query) => {
                self.pending = Some(Pending {
                    seq,
                    stage: Stage::Query,
                    query,
                });
                false
            }
            Err(e) => {
                tracing::warn!(target: "tasklens::engine", error = %e, "window pick did not start");
                self.last = Some(PickResult {
                    seq,
                    outcome: PickOutcome::Cancelled,
                });
                true
            }
        }
    }

    /// `CancelPick { seq }`: ends the running pick when `seq` is its seq.
    fn cancel(&mut self, seq: u64) -> bool {
        if self.pending.as_ref().is_some_and(|p| p.seq == seq) {
            self.finish(PickOutcome::Cancelled)
        } else {
            false
        }
    }

    /// A `PickReply` or `Picked` event. Returns true when there is a new result.
    async fn handle(
        &mut self,
        cx: &Cx,
        host: &impl PickHost,
        dir: &Path,
        files: &mut CmdFiles,
        event: Event,
    ) -> bool {
        match event {
            Event::PickReply { seq, reply } => self.reply(cx, host, dir, files, seq, reply).await,
            Event::Picked { uuid, pid } => self.picked(&uuid, pid),
            _ => false,
        }
    }

    async fn reply(
        &mut self,
        cx: &Cx,
        host: &impl PickHost,
        dir: &Path,
        files: &mut CmdFiles,
        seq: u64,
        reply: Result<PickInfo, QueryError>,
    ) -> bool {
        // A reply for a cancelled or finished pick is ignored.
        if !self
            .pending
            .as_ref()
            .is_some_and(|p| p.seq == seq && matches!(p.stage, Stage::Query))
        {
            return false;
        }
        let info = match reply {
            Ok(info) => info,
            Err(QueryError::Timeout) => return self.finish(PickOutcome::NoWindow),
            // The user clicked something KWin does not manage: a pick, but not a window.
            Err(QueryError::Failed(name)) if &*name == INVALID_WINDOW => {
                return self.finish(PickOutcome::NoWindow);
            }
            Err(QueryError::Failed(name)) => {
                if &*name == "org.kde.KWin.Error.UserCancel" {
                    tracing::debug!(target: "tasklens::engine", "window pick cancelled by the user");
                } else {
                    tracing::warn!(target: "tasklens::engine", error = %name, "window pick failed");
                }
                return self.finish(PickOutcome::Cancelled);
            }
        };
        if info.window_type.is_some_and(|t| SHELL_TYPES.contains(&t)) {
            return self.finish(PickOutcome::NoWindow);
        }
        if let Some(pid) = info.pid {
            return self.finish(PickOutcome::Picked { pid });
        }
        // KWin always sends a braced lowercase UUID; anything else never enters a script.
        let Some(uuid) = info.uuid.filter(|uuid| braced_uuid(uuid)) else {
            return self.finish(PickOutcome::NoWindow);
        };
        files.clear(cx).await;
        let path = dir.join(format!("{PICK_PLUGIN}.js"));
        // A copy left by an earlier pick that never called back blocks `loadScript`.
        if let Err(e) = host.unload(cx, PICK_PLUGIN).await {
            tracing::debug!(target: "tasklens::engine", error = %e, "pick script unload failed");
        }
        let result = match write_script(cx, &path, pick_script(&self.unique, &uuid)).await {
            Ok(()) => host.run(cx, &path, PICK_PLUGIN).await,
            Err(e) => Err(e),
        };
        files.last = Some(path);
        if let Err(e) = result {
            tracing::warn!(target: "tasklens::engine", error = %e, "pick script failed");
            return self.finish(PickOutcome::Cancelled);
        }
        if let Some(pending) = &mut self.pending {
            pending.stage = Stage::Script {
                uuid,
                deadline: cx.now() + PICK_SCRIPT_WAIT,
            };
        }
        false
    }

    /// An accepted `Picked` call. A uuid that is not the pending pick's is ignored.
    fn picked(&mut self, uuid: &str, pid: Option<u32>) -> bool {
        let waiting = matches!(
            &self.pending,
            Some(Pending { stage: Stage::Script { uuid: want, .. }, .. }) if want == uuid
        );
        if !waiting {
            return false;
        }
        self.finish(pid.map_or(PickOutcome::NoWindow, |pid| PickOutcome::Picked { pid }))
    }

    /// Ends a pick script wait that passed its deadline with `NoWindow`.
    fn expire(&mut self, now: Time) -> bool {
        match &self.pending {
            Some(Pending {
                stage: Stage::Script { deadline, .. },
                ..
            }) if *deadline <= now => self.finish(PickOutcome::NoWindow),
            _ => false,
        }
    }

    /// The time left until the pick script deadline, if one runs.
    fn wait(&self, now: Time) -> Option<Duration> {
        match &self.pending {
            Some(Pending {
                stage: Stage::Script { deadline, .. },
                ..
            }) => Some(Duration::from_nanos(deadline.duration_since(now))),
            _ => None,
        }
    }

    fn finish(&mut self, outcome: PickOutcome) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        pending.query.abort();
        self.last = Some(PickResult {
            seq: pending.seq,
            outcome,
        });
        true
    }
}

impl Drop for Picker {
    /// The query task holds a connection clone; it must not outlive the generation.
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.query.abort();
        }
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

/// Runs the newly queued commands one at a time, in arrival order. `Pick` and `CancelPick` go to
/// `picker`. Returns true when the pick result changed.
#[allow(clippy::too_many_arguments)]
async fn run_commands(
    cx: &Cx,
    io: &WorkerIo,
    host: &impl PickHost,
    dir: &Path,
    commands: &Commands,
    queue: &mut Queue,
    windows: &[WindowEntry],
    files: &mut CmdFiles,
    picker: &mut Picker,
) -> bool {
    let mut picked = false;
    let new = queue.take(&io.control.borrow_and_clone());
    for (seq, cmd, app_id) in new {
        if cx.checkpoint().is_err() {
            break;
        }
        queue.progress.set_started(SourceId::KWin, seq);
        match cmd {
            KWinCommand::Pick { seq: pick } => picked |= picker.start(cx, host, pick),
            KWinCommand::CancelPick { seq: pick } => picked |= picker.cancel(pick),
            cmd => {
                if let Some((plugin, script)) =
                    commands.prepare_from(&cmd, app_id.as_ref(), windows)
                {
                    let _ = run_script(cx, host, dir, &plugin, script, files).await;
                }
            }
        }
        queue.progress.set_done(SourceId::KWin, seq);
    }
    picked
}

/// Deletes the previous command's file, writes `<dir>/<plugin>.js` (mode 0600) and runs it. The
/// new file is kept in `files`, also after a failure: `loadScript` may have taken it before
/// `start` failed. A failure is logged at `warn`; a queued command is then dropped (R-170), and
/// Switch to window reports it.
async fn run_script(
    cx: &Cx,
    host: &impl ScriptHost,
    dir: &Path,
    plugin: &str,
    script: String,
    files: &mut CmdFiles,
) -> Result<(), WorkerError> {
    files.clear(cx).await;
    let path = dir.join(format!("{plugin}.js"));
    let result = match write_script(cx, &path, script).await {
        Ok(()) => host.run(cx, &path, plugin).await,
        Err(e) => Err(e),
    };
    files.last = Some(path);
    if let Err(e) = &result
        && !cx.is_cancel_requested()
    {
        tracing::warn!(target: "tasklens::engine", plugin, error = %e, "kwin command failed");
    }
    result
}

/// Switch to window (`ActionKind::ActivateWindow`): the activate script for the window in
/// `windows`. `NoSuchProcess` when the window is not in the list, `Internal` when KWin did not
/// take the script.
async fn activate(
    cx: &Cx,
    host: &impl ScriptHost,
    dir: &Path,
    commands: &Commands,
    windows: &[WindowEntry],
    files: &mut CmdFiles,
    call: &ActivateCall,
) -> Result<(), ActionError> {
    let cmd = KWinCommand::Activate {
        internal_id: Arc::clone(&call.internal_id),
    };
    let Some((plugin, script)) = commands.prepare(&cmd, windows) else {
        return Err(ActionError::NoSuchProcess);
    };
    run_script(cx, host, dir, &plugin, script, files)
        .await
        .map_err(|_| ActionError::Internal(Arc::from("KWin did not run the window switch")))
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

    /// The pick script's answer (R-149 item 3, R-241): the pid as decimal text.
    async fn picked(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        uuid: String,
        pid: String,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().map(|name| name.as_str().to_owned());
        let owner = self
            .owner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match check_picked(sender.as_deref(), owner.as_deref(), uuid, &pid) {
            Ok(event) => self
                .tx
                .send(event)
                .await
                .map_err(|_| zbus::fdo::Error::Failed("worker stopped".into())),
            Err(reject) => {
                tracing::debug!(target: "tasklens::engine", reason = reject.as_str(), "kwin pick answer rejected");
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
    let mut windows: Vec<WindowEntry> = values.into_iter().filter_map(window_entry).collect();
    for (i, w) in windows.iter_mut().enumerate() {
        w.stack_index = i as u32;
    }
    Ok(windows)
}

/// The `Picked` checks: only the owner of `org.kde.KWin` may call (R-167 item 5). The pid text
/// is parsed as `u32`; `"0"` or text that does not parse gives `None` (R-241).
fn check_picked(
    sender: Option<&str>,
    owner: Option<&str>,
    uuid: String,
    pid: &str,
) -> Result<Event, Reject> {
    match (sender, owner) {
        (Some(sender), Some(owner)) if sender == owner => {}
        _ => return Err(Reject::Sender),
    }
    let pid = pid.parse::<u32>().ok().filter(|&pid| pid != 0);
    Ok(Event::Picked { uuid, pid })
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
        // Set by `check_update`, which knows the list position.
        stack_index: 0,
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
    with_pick(windows.into(), validity, None)
}

/// A snapshot of an already sorted list with the pick result.
fn with_pick(
    windows: Arc<[WindowEntry]>,
    validity: Validity,
    pick: Option<PickResult>,
) -> WindowsSnapshot {
    let t_ns = now_boot_ns();
    WindowsSnapshot {
        header: ComponentHeader {
            version: 0,
            observed_start: t_ns,
            observed_end: t_ns,
            validity,
        },
        windows,
        pick,
    }
}

/// `SourceUpdate::Windows` for an already sorted list.
fn windows_update(
    windows: Arc<[WindowEntry]>,
    validity: Validity,
    pick: Option<PickResult>,
) -> SourceUpdate {
    SourceUpdate::Windows(Box::new(with_pick(windows, validity, pick)))
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

    use crate::desktop::APP_ID;
    use asupersync::channel::watch;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use serde_json::json;
    use tl_model::demand::Demand;
    use tl_model::settings::EngineSettings;

    use crate::source::{QueuedCommand, WorkerControl};

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
    fn stack_index_follows_bridge_order_across_pids() {
        let json = payload(&[
            window(ID_C, 42, "top"),
            window(ID_B, 7, "other"),
            window(ID_A, 42, "below"),
        ]);
        let windows = check_update(Some(KWIN), Some(KWIN), &json).expect("accepted");
        let snap = snapshot(windows, Validity::Valid);
        let rows: Vec<(&str, u32, u32)> = snap
            .windows
            .iter()
            .map(|w| (&*w.caption, w.pid, w.stack_index))
            .collect();
        assert_eq!(rows, [("other", 7, 1), ("top", 42, 0), ("below", 42, 2)]);

        // A resend after activating "other": it moves to the top.
        let json = payload(&[
            window(ID_B, 7, "other"),
            window(ID_C, 42, "top"),
            window(ID_A, 42, "below"),
        ]);
        let windows = check_update(Some(KWIN), Some(KWIN), &json).expect("accepted");
        let snap = snapshot(windows, Validity::Valid);
        let rows: Vec<(&str, u32, u32)> = snap
            .windows
            .iter()
            .map(|w| (&*w.caption, w.pid, w.stack_index))
            .collect();
        assert_eq!(rows, [("other", 7, 0), ("top", 42, 1), ("below", 42, 2)]);
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
            desktop: DesktopSession::new(DesktopSource::Process),
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
            stack_index: 0,
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

    /// q4x76.12: under the engine service, keep above matches the app id the window announced
    /// in its `Hello`, not the fixed one, and nothing while no window of that pid is connected.
    #[test]
    fn own_window_matches_the_announced_app_id() {
        const DEVEL: &str = "io.github.oysteinkrog.Tasklens.Devel";
        let windows = [
            entry(ID_A, 7, "tasklens", APP_ID),
            entry(ID_B, 7, "tasklens", DEVEL),
        ];
        let desktop = DesktopSession::new(DesktopSource::UserManager);
        let cmds = Commands {
            mode: EngineMode::Full,
            counter: Arc::new(AtomicU64::new(0)),
            desktop: desktop.clone(),
        };
        let keep = KWinCommand::KeepAbove {
            pid: 7,
            caption: None,
            on: true,
        };
        let devel: Arc<str> = Arc::from(DEVEL);
        assert_eq!(
            cmds.prepare_from(&keep, Some(&devel), &windows),
            None,
            "no window connected"
        );
        assert_eq!(cmds.prepare(&keep, &windows), None, "no announced app id");
        let _window = desktop.add_window(7, Arc::clone(&devel));
        let (_, script) = cmds
            .prepare_from(&keep, Some(&devel), &windows)
            .expect("the announced app id");
        assert_eq!(args_of(&script)["id"], ID_B);
    }

    /// q4x76.12 leader decision: own-window checks are keyed by the requesting session. Two
    /// sessions share pid 7 with different app ids; each acts only on its own window.
    #[test]
    fn sessions_sharing_a_pid_keep_to_their_own_windows() {
        const DEVEL: &str = "io.github.oysteinkrog.Tasklens.Devel";
        let windows = [
            entry(ID_A, 7, "tasklens", APP_ID),
            entry(ID_B, 7, "tasklens", DEVEL),
            entry(ID_C, 7, "tasklens Graph 1", DEVEL),
        ];
        let desktop = DesktopSession::new(DesktopSource::UserManager);
        let cmds = Commands {
            mode: EngineMode::Full,
            counter: Arc::new(AtomicU64::new(0)),
            desktop: desktop.clone(),
        };
        let devel: Arc<str> = Arc::from(DEVEL);
        let main: Arc<str> = Arc::from(APP_ID);
        let _a = desktop.add_window(7, Arc::clone(&main));
        let _b = desktop.add_window(7, Arc::clone(&devel));
        let keep = |caption: Option<&str>| KWinCommand::KeepAbove {
            pid: 7,
            caption: caption.map(Arc::from),
            on: true,
        };
        let place = KWinCommand::SetFrameGeometry {
            pid: 7,
            caption: Arc::from("tasklens Graph 1"),
            geometry: FrameGeometry::default(),
        };
        let id_of = |from: &Arc<str>, cmd: &KWinCommand| {
            cmds.prepare_from(cmd, Some(from), &windows)
                .and_then(|(_, script)| args_of(&script)["id"].as_str().map(str::to_owned))
        };
        assert_eq!(id_of(&main, &keep(None)).as_deref(), Some(ID_A));
        assert_eq!(id_of(&devel, &keep(None)).as_deref(), Some(ID_B));
        // The other session's graph window is not this session's.
        assert_eq!(id_of(&main, &keep(Some("tasklens Graph 1"))), None);
        assert_eq!(id_of(&main, &place), None);
        assert_eq!(
            id_of(&devel, &keep(Some("tasklens Graph 1"))).as_deref(),
            Some(ID_C)
        );
        assert_eq!(id_of(&devel, &place).as_deref(), Some(ID_C));
    }

    /// Records each script as KWin would read it, and fails when told to. As the pick call layer
    /// it answers `queryWindowInfo` with `reply` from a child task, or never when that is `None`.
    struct FakeHost {
        seen: Mutex<Vec<(String, String, u32)>>,
        fail: bool,
        reply: Option<Result<PickInfo, QueryError>>,
        queries: Mutex<Vec<u64>>,
        unloads: Mutex<Vec<String>>,
    }

    impl FakeHost {
        fn new(fail: bool) -> Self {
            FakeHost {
                seen: Mutex::new(Vec::new()),
                fail,
                reply: None,
                queries: Mutex::new(Vec::new()),
                unloads: Mutex::new(Vec::new()),
            }
        }

        fn answering(reply: Result<PickInfo, QueryError>) -> Self {
            FakeHost {
                reply: Some(reply),
                ..FakeHost::new(false)
            }
        }
    }

    impl PickHost for FakeHost {
        fn query(
            &self,
            cx: &Cx,
            seq: u64,
            tx: async_channel::Sender<Event>,
        ) -> Result<TaskHandle<()>, WorkerError> {
            self.queries.lock().expect("lock").push(seq);
            let reply = self.reply.clone();
            cx.spawn(move |_cx| async move {
                if let Some(reply) = reply {
                    let _ = tx.send(Event::PickReply { seq, reply }).await;
                }
            })
            .map_err(|e| WorkerError::Other(format!("spawn: {e}").into()))
        }

        async fn unload(&self, _cx: &Cx, plugin: &str) -> Result<(), WorkerError> {
            self.unloads.lock().expect("lock").push(plugin.to_owned());
            Ok(())
        }
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

    /// q4x76.12: Switch to window answers with a result: success once KWin took the script,
    /// `NoSuchProcess` for a window not in the list, `Internal` when KWin refused the script.
    #[test]
    fn activate_reports_its_result() {
        let dir = scratch("activate").join("tasklens");
        let windows = [entry(ID_A, 42, "a", "org.kde.konsole")];
        let cmds = commands(EngineMode::Full);
        let host = FakeHost::new(false);
        let failing = FakeHost::new(true);
        let results = Arc::new(Mutex::new(Vec::new()));
        {
            let results = Arc::clone(&results);
            in_lab(async move {
                let cx = Cx::current().expect("task has a Cx");
                let mut files = CmdFiles::default();
                let call = |id: &str| ActivateCall {
                    internal_id: Arc::from(id),
                    generation: 1,
                    reply: async_channel::bounded(1).0,
                };
                let mut got = Vec::new();
                for (host, id) in [(&host, ID_A), (&host, ID_B), (&failing, ID_A)] {
                    let r =
                        super::activate(&cx, host, &dir, &cmds, &windows, &mut files, &call(id))
                            .await;
                    got.push(r);
                }
                *results.lock().expect("lock") = got;
            });
        }
        let got = results.lock().expect("lock");
        assert_eq!(got[0], Ok(()));
        assert_eq!(got[1], Err(ActionError::NoSuchProcess));
        assert!(matches!(got[2], Err(ActionError::Internal(_))), "{got:?}");
    }

    #[test]
    fn cmd_file_kept_until_next_command() {
        let dir = scratch("cmd-file").join("tasklens");
        let windows = [entry(ID_A, 42, "a", "org.kde.konsole")];
        let cmds = commands(EngineMode::Full);
        let host = Arc::new(FakeHost::new(false));
        let failing = Arc::new(FakeHost::new(true));
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
                let ran = run_script(&cx, &*host, &dir, &plugin, script, &mut files).await;
                assert!(ran.is_ok());
                step();
                let (plugin, script) = cmds.prepare(&activate(ID_A), &windows).expect("known");
                let ran = run_script(&cx, &*failing, &dir, &plugin, script, &mut files).await;
                assert!(ran.is_err());
                step();
                // The next `Update`.
                files.clear(&cx).await;
                step();
                let (plugin, script) = cmds.prepare(&activate(ID_A), &windows).expect("known");
                let ran = run_script(&cx, &*host, &dir, &plugin, script, &mut files).await;
                assert!(ran.is_ok());
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

    const UNIQUE: &str = ":1.4242";

    /// A `queryWindowInfo` map as KWin 6.7.5 sends it, with only the keys the picker reads.
    fn reply_map(
        pid: Option<i32>,
        window_type: i32,
    ) -> HashMap<String, zbus::zvariant::OwnedValue> {
        let owned = |v: zbus::zvariant::Value<'_>| {
            zbus::zvariant::OwnedValue::try_from(v).expect("owned value")
        };
        let mut map = HashMap::new();
        map.insert("uuid".to_owned(), owned(ID_A.into()));
        map.insert("type".to_owned(), owned(window_type.into()));
        map.insert("caption".to_owned(), owned("Konsole".into()));
        if let Some(pid) = pid {
            map.insert("pid".to_owned(), owned(pid.into()));
        }
        map
    }

    /// One picker test: the picker, its event channel, a fake KWin and the command plumbing.
    struct PickRig {
        picker: Picker,
        rx: async_channel::Receiver<Event>,
        host: FakeHost,
        dir: PathBuf,
        files: CmdFiles,
        control: watch::Sender<WorkerControl>,
        io: WorkerIo,
        queue: Queue,
        next_seq: u64,
    }

    impl PickRig {
        fn new(name: &str, host: FakeHost) -> Self {
            let (tx, rx) = async_channel::bounded(UPDATE_QUEUE);
            let (updates, _rx) = mpsc::channel::<EngineMsg>(16);
            let (_demand_tx, demand) = watch::channel(Demand::default());
            let (_settings_tx, settings) = watch::channel(EngineSettings::default());
            let (control, control_rx) = watch::channel(WorkerControl::default());
            let progress = Arc::clone(&control_rx.borrow_and_clone().progress);
            let io = WorkerIo::new(
                updates,
                demand,
                settings,
                control_rx,
                crate::dbus::SharedHandles::default(),
            );
            PickRig {
                picker: Picker::new(EngineMode::Full, UNIQUE, tx),
                rx,
                host,
                dir: scratch(name).join("tasklens"),
                files: CmdFiles::default(),
                control,
                io,
                queue: Queue {
                    last_seq: 0,
                    progress,
                },
                next_seq: 0,
            }
        }

        /// Queues `cmds` as the reducer does and runs them. Returns true when the result changed.
        async fn command(&mut self, cx: &Cx, cmds: &[KWinCommand]) -> bool {
            let mut all: Vec<QueuedCommand> = self.io.control.borrow_and_clone().commands.to_vec();
            for cmd in cmds {
                self.next_seq += 1;
                all.push(QueuedCommand {
                    seq: self.next_seq,
                    target: SourceId::KWin,
                    command: WorkerCommand::KWin {
                        cmd: cmd.clone(),
                        app_id: None,
                    },
                });
            }
            let value = WorkerControl {
                commands: all.into(),
                ..self.io.control.borrow_and_clone()
            };
            let _ = self.control.send(value);
            let windows = [entry(ID_A, 42, "a", "org.kde.konsole")];
            run_commands(
                cx,
                &self.io,
                &self.host,
                &self.dir,
                &commands(EngineMode::Full),
                &mut self.queue,
                &windows,
                &mut self.files,
                &mut self.picker,
            )
            .await
        }

        /// Reads the next event the fake KWin or the bridge sent and hands it to the picker.
        async fn next(&mut self, cx: &Cx) -> bool {
            let event = self.rx.recv().await.expect("an event");
            self.feed(cx, event).await
        }

        async fn feed(&mut self, cx: &Cx, event: Event) -> bool {
            self.picker
                .handle(cx, &self.host, &self.dir, &mut self.files, event)
                .await
        }

        fn last(&self) -> Option<PickResult> {
            self.picker.last
        }
    }

    fn result(seq: u64, outcome: PickOutcome) -> Option<PickResult> {
        Some(PickResult { seq, outcome })
    }

    /// Runs `body` with a fresh rig in the lab and removes the rig's directory afterwards.
    fn pick_test<F, Fut>(name: &str, host: FakeHost, body: F)
    where
        F: FnOnce(Cx, PickRig) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let rig = PickRig::new(name, host);
        let scratch_dir = rig.dir.parent().expect("scratch").to_owned();
        let done = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&done);
        in_lab(async move {
            let cx = Cx::current().expect("task has a Cx");
            body(cx, rig).await;
            *flag.lock().expect("lock") = true;
        });
        assert!(*done.lock().expect("lock"), "the test body did not finish");
        let _ = std::fs::remove_dir_all(scratch_dir);
    }

    #[test]
    fn pick_uses_reply_pid() {
        assert_eq!(
            pick_info(&reply_map(Some(1234), 0)),
            PickInfo {
                uuid: Some(ID_A.into()),
                pid: Some(1234),
                window_type: Some(0),
            }
        );
        let host = FakeHost::answering(Ok(pick_info(&reply_map(Some(1234), 0))));
        pick_test("pick-pid", host, |cx, mut rig| async move {
            assert!(!rig.command(&cx, &[KWinCommand::Pick { seq: 5 }]).await);
            assert_eq!(*rig.host.queries.lock().expect("lock"), [5]);
            assert!(rig.next(&cx).await);
            assert_eq!(rig.last(), result(5, PickOutcome::Picked { pid: 1234 }));
            // No script was loaded.
            assert!(rig.host.seen.lock().expect("lock").is_empty());
            assert!(rig.host.unloads.lock().expect("lock").is_empty());
            assert!(rig.files.last.is_none());
        });
    }

    #[test]
    fn pick_script_fallback() {
        // A pid of 0 counts as missing, like no pid at all.
        assert_eq!(pick_info(&reply_map(Some(0), 0)).pid, None);
        let host = FakeHost::answering(Ok(pick_info(&reply_map(None, 0))));
        pick_test("pick-script", host, |cx, mut rig| async move {
            rig.command(&cx, &[KWinCommand::Pick { seq: 5 }]).await;
            assert!(!rig.next(&cx).await, "waits for the script");
            assert_eq!(rig.last(), None);
            assert_eq!(*rig.host.unloads.lock().expect("lock"), [PICK_PLUGIN]);
            {
                let seen = rig.host.seen.lock().expect("lock");
                assert_eq!(seen.len(), 1);
                let (plugin, text, mode) = &seen[0];
                assert_eq!(plugin, PICK_PLUGIN);
                assert_eq!(*mode, 0o600);
                assert!(!text.contains("__TL_"), "placeholder left");
                assert!(text.contains("const BUS_NAME = \":1.4242\";"), "{text}");
                let start = text.find("const args = ").expect("args line") + "const args = ".len();
                let end = start + text[start..].find(";\n").expect("args line ends");
                let args: serde_json::Value =
                    serde_json::from_str(&text[start..end]).expect("args are JSON");
                assert_eq!(args, json!({ "plugin": PICK_PLUGIN, "uuid": ID_A }));
            }
            assert_eq!(
                rig.files.last.as_deref(),
                Some(rig.dir.join("tasklens-pick.js").as_path())
            );
            // The script's call for another window is ignored.
            let other = check_picked(Some(KWIN), Some(KWIN), ID_B.into(), "99").expect("owner");
            assert!(!rig.feed(&cx, other).await);
            let call = check_picked(Some(KWIN), Some(KWIN), ID_A.into(), "1234").expect("owner");
            assert!(rig.feed(&cx, call).await);
            assert_eq!(rig.last(), result(5, PickOutcome::Picked { pid: 1234 }));
            // The script file is kept until the next command, `Update` or exit.
            assert!(rig.dir.join("tasklens-pick.js").exists());
        });
    }

    #[test]
    fn pick_timeout_no_window() {
        let host = FakeHost::answering(Ok(pick_info(&reply_map(None, 0))));
        pick_test("pick-timeout", host, |cx, mut rig| async move {
            rig.command(&cx, &[KWinCommand::Pick { seq: 5 }]).await;
            assert!(!rig.next(&cx).await);
            let now = cx.now();
            assert_eq!(rig.picker.wait(now), Some(PICK_SCRIPT_WAIT));
            assert!(!rig.picker.expire(now + Duration::from_millis(1999)));
            assert!(rig.picker.expire(now + PICK_SCRIPT_WAIT));
            assert_eq!(rig.last(), result(5, PickOutcome::NoWindow));
            assert_eq!(rig.picker.wait(now), None);
            // A callback after the deadline changes nothing.
            let late = check_picked(Some(KWIN), Some(KWIN), ID_A.into(), "1234").expect("owner");
            assert!(!rig.feed(&cx, late).await);
            assert_eq!(rig.last(), result(5, PickOutcome::NoWindow));
        });
        // No answer from `queryWindowInfo` within 120 s.
        pick_test(
            "pick-timeout-query",
            FakeHost::answering(Err(QueryError::Timeout)),
            |cx, mut rig| async move {
                rig.command(&cx, &[KWinCommand::Pick { seq: 6 }]).await;
                assert!(rig.next(&cx).await);
                assert_eq!(rig.last(), result(6, PickOutcome::NoWindow));
            },
        );
    }

    #[test]
    fn pick_ignores_shell_windows() {
        for (window_type, outcome) in [
            (1, PickOutcome::NoWindow),
            (2, PickOutcome::NoWindow),
            (13, PickOutcome::NoWindow),
            (16, PickOutcome::NoWindow),
            (17, PickOutcome::NoWindow),
            (0, PickOutcome::Picked { pid: 1234 }),
            (5, PickOutcome::Picked { pid: 1234 }),
        ] {
            let host = FakeHost::answering(Ok(pick_info(&reply_map(Some(1234), window_type))));
            pick_test("pick-shell", host, move |cx, mut rig| async move {
                rig.command(&cx, &[KWinCommand::Pick { seq: 5 }]).await;
                assert!(rig.next(&cx).await);
                assert_eq!(rig.last(), result(5, outcome), "type {window_type}");
            });
        }
    }

    #[test]
    fn pick_esc_cancelled() {
        for name in [
            "org.kde.KWin.Error.UserCancel",
            "org.freedesktop.DBus.Error.NoReply",
        ] {
            let host = FakeHost::answering(Err(QueryError::Failed(name.into())));
            pick_test("pick-esc", host, |cx, mut rig| async move {
                rig.command(&cx, &[KWinCommand::Pick { seq: 5 }]).await;
                assert!(rig.next(&cx).await);
                assert_eq!(rig.last(), result(5, PickOutcome::Cancelled));
            });
        }
    }

    #[test]
    fn pick_invalid_window_no_window() {
        let host = FakeHost::answering(Err(QueryError::Failed(INVALID_WINDOW.into())));
        pick_test("pick-invalid", host, |cx, mut rig| async move {
            rig.command(&cx, &[KWinCommand::Pick { seq: 5 }]).await;
            assert!(rig.next(&cx).await);
            assert_eq!(rig.last(), result(5, PickOutcome::NoWindow));
            assert!(rig.host.seen.lock().expect("lock").is_empty());
        });
    }

    #[test]
    fn pick_cancel_command() {
        // The fake never answers by itself; the late reply is fed by hand.
        pick_test(
            "pick-cancel",
            FakeHost::new(false),
            |cx, mut rig| async move {
                rig.command(&cx, &[KWinCommand::Pick { seq: 5 }]).await;
                assert!(
                    rig.command(&cx, &[KWinCommand::CancelPick { seq: 5 }])
                        .await
                );
                assert_eq!(rig.last(), result(5, PickOutcome::Cancelled));
                let late = Event::PickReply {
                    seq: 5,
                    reply: Ok(pick_info(&reply_map(Some(1234), 0))),
                };
                assert!(!rig.feed(&cx, late).await);
                assert_eq!(rig.last(), result(5, PickOutcome::Cancelled));
                // A new pick can start after the cancel.
                rig.command(&cx, &[KWinCommand::Pick { seq: 6 }]).await;
                assert_eq!(*rig.host.queries.lock().expect("lock"), [5, 6]);
            },
        );
    }

    #[test]
    fn pick_cancel_wrong_seq_ignored() {
        let host = FakeHost::answering(Ok(pick_info(&reply_map(Some(1234), 0))));
        pick_test("pick-cancel-seq", host, |cx, mut rig| async move {
            let cmds = [
                KWinCommand::Pick { seq: 5 },
                KWinCommand::CancelPick { seq: 4 },
                // A second pick while one runs is ignored too.
                KWinCommand::Pick { seq: 6 },
            ];
            assert!(!rig.command(&cx, &cmds).await);
            assert_eq!(*rig.host.queries.lock().expect("lock"), [5]);
            assert!(rig.next(&cx).await);
            assert_eq!(rig.last(), result(5, PickOutcome::Picked { pid: 1234 }));
        });
    }

    #[test]
    fn pick_result_in_windows_snapshot() {
        let host = FakeHost::answering(Ok(pick_info(&reply_map(Some(1234), 0))));
        pick_test("pick-snapshot", host, |cx, mut rig| async move {
            rig.command(&cx, &[KWinCommand::Pick { seq: 5 }]).await;
            assert!(rig.next(&cx).await);
            let list = snapshot(
                vec![entry(ID_A, 42, "a", "org.kde.konsole")],
                Validity::Valid,
            );
            let want = result(5, PickOutcome::Picked { pid: 1234 });
            match windows_update(Arc::clone(&list.windows), Validity::Valid, rig.last()) {
                SourceUpdate::Windows(snap) => {
                    assert_eq!(snap.pick, want);
                    assert_eq!(snap.windows.len(), 1);
                    assert_eq!(snap.header.validity, Validity::Valid);
                }
                other => panic!("unexpected update {other:?}"),
            }
            // A later list publish keeps the result.
            assert!(!rig.command(&cx, &[activate(ID_A)]).await);
            let later = with_pick(Arc::from([]), Validity::Unsupported, rig.last());
            assert_eq!(later.pick, want);
        });
    }

    #[test]
    fn pick_callback_bad_pid_text_no_window() {
        for text in ["abc", "0", "-1", "", "4294967296"] {
            match check_picked(Some(KWIN), Some(KWIN), ID_A.into(), text) {
                Ok(Event::Picked { uuid, pid }) => {
                    assert_eq!(uuid, ID_A);
                    assert_eq!(pid, None, "{text:?}");
                }
                _ => panic!("owner call refused"),
            }
        }
        let host = FakeHost::answering(Ok(pick_info(&reply_map(None, 0))));
        pick_test("pick-bad-pid", host, |cx, mut rig| async move {
            rig.command(&cx, &[KWinCommand::Pick { seq: 5 }]).await;
            rig.next(&cx).await;
            let call = check_picked(Some(KWIN), Some(KWIN), ID_A.into(), "abc").expect("owner");
            assert!(rig.feed(&cx, call).await);
            assert_eq!(rig.last(), result(5, PickOutcome::NoWindow));
        });
    }

    #[test]
    fn pick_callback_wrong_sender_rejected() {
        assert!(matches!(
            check_picked(Some(":1.99"), Some(KWIN), ID_A.into(), "1234"),
            Err(Reject::Sender)
        ));
        assert!(matches!(
            check_picked(None, Some(KWIN), ID_A.into(), "1234"),
            Err(Reject::Sender)
        ));
        assert!(matches!(
            check_picked(Some(KWIN), None, ID_A.into(), "1234"),
            Err(Reject::Sender)
        ));
        let host = FakeHost::answering(Ok(pick_info(&reply_map(None, 0))));
        pick_test("pick-sender", host, |cx, mut rig| async move {
            rig.command(&cx, &[KWinCommand::Pick { seq: 5 }]).await;
            rig.next(&cx).await;
            // The rejected call never reaches the picker, so the pick keeps waiting.
            assert!(check_picked(Some(":1.99"), Some(KWIN), ID_A.into(), "1234").is_err());
            assert_eq!(rig.last(), None);
            assert_eq!(rig.picker.wait(cx.now()), Some(PICK_SCRIPT_WAIT));
            let call = check_picked(Some(KWIN), Some(KWIN), ID_A.into(), "1234").expect("owner");
            assert!(rig.feed(&cx, call).await);
            assert_eq!(rig.last(), result(5, PickOutcome::Picked { pid: 1234 }));
        });
    }

    #[test]
    fn pick_ignored_in_low_memory() {
        let (tx, _rx) = async_channel::bounded(UPDATE_QUEUE);
        let mut picker = Picker::new(EngineMode::LowMemory, UNIQUE, tx);
        let host = FakeHost::new(false);
        in_lab(async move {
            let cx = Cx::current().expect("task has a Cx");
            assert!(!picker.start(&cx, &host, 5));
            assert!(picker.pending.is_none());
            assert!(host.queries.lock().expect("lock").is_empty());
        });
    }

    /// What the fake user bus does next.
    #[derive(Debug)]
    enum BusStep {
        KWinUp,
        KWinGone,
        /// The bus fails while the bridge runs.
        Fail,
    }

    /// `SessionBus` driven by the test: KWin comes and goes on `steps`, each bridge run is
    /// counted.
    struct FakeBus {
        steps: async_channel::Receiver<BusStep>,
        /// The user manager's environment; the test may change it.
        env: Arc<Mutex<Vec<String>>>,
        /// How often `manager_env` was read.
        reads: Arc<AtomicU64>,
        bridges: Arc<AtomicU64>,
    }

    fn manager_env(lines: &[&str]) -> Arc<Mutex<Vec<String>>> {
        Arc::new(Mutex::new(lines.iter().map(|l| (*l).to_owned()).collect()))
    }

    impl SessionBus for FakeBus {
        async fn wait_kwin(
            &mut self,
            _cx: &Cx,
            _io: &WorkerIo,
            _queue: &mut Queue,
        ) -> Result<(), WorkerError> {
            loop {
                match self.steps.recv().await {
                    Ok(BusStep::KWinUp) => return Ok(()),
                    Ok(BusStep::KWinGone | BusStep::Fail) => {}
                    Err(_) => return Err(WorkerError::Closed),
                }
            }
        }

        async fn manager_env(&mut self, _cx: &Cx) -> Result<Vec<String>, WorkerError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.env.lock().expect("lock").clone())
        }

        async fn bridge(
            &mut self,
            _cx: &Cx,
            _io: &WorkerIo,
            _commands: &Commands,
            _queue: &mut Queue,
        ) -> Result<(), WorkerError> {
            self.bridges.fetch_add(1, Ordering::Relaxed);
            loop {
                match self.steps.recv().await {
                    Ok(BusStep::KWinGone) => return Ok(()),
                    Ok(BusStep::Fail) => return Err(WorkerError::Dbus("bus failed".into())),
                    Ok(BusStep::KWinUp) => {}
                    Err(_) => return Err(WorkerError::Closed),
                }
            }
        }
    }

    /// The health states in `rx`, in order.
    fn health_states(rx: &mut mpsc::Receiver<EngineMsg>) -> Vec<(SourceState, Option<String>)> {
        let mut got = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let EngineMsg::Source(SourceUpdate::Health {
                state, last_error, ..
            }) = msg
            {
                got.push((state, last_error.as_deref().map(str::to_owned)));
            }
        }
        got
    }

    /// q4x76.12, ADR-018: the engine service has no desktop session until KWin appears; then
    /// it reads the user manager's environment and starts the bridge. When KWin leaves the
    /// session ends, and the next KWin starts the bridge again.
    #[test]
    fn service_bridge_follows_the_desktop_session() {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let (io, mut rx) = worker_io();
        let desktop = DesktopSession::new(DesktopSource::UserManager);
        let (steps_tx, steps) = async_channel::unbounded();
        let bridges = Arc::new(AtomicU64::new(0));
        let mut bus = FakeBus {
            steps,
            env: manager_env(&["WAYLAND_DISPLAY=wayland-1", "XDG_CURRENT_DESKTOP=KDE"]),
            reads: Arc::default(),
            bridges: Arc::clone(&bridges),
        };
        let root = lab.state.create_root_region(Budget::INFINITE);
        let task_desktop = desktop.clone();
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let mut queue = Queue {
                    last_seq: 0,
                    progress: Arc::default(),
                };
                let commands = commands(EngineMode::Full);
                let _ =
                    session_cycles(&cx, &io, &task_desktop, &mut bus, &commands, &mut queue).await;
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();

        let no_session = (SourceState::Unsupported, Some(NO_SESSION.to_owned()));
        assert!(!desktop.is_present(), "nobody logged in yet");
        assert_eq!(bridges.load(Ordering::Relaxed), 0);
        assert_eq!(health_states(&mut rx), std::slice::from_ref(&no_session));

        steps_tx.try_send(BusStep::KWinUp).expect("send");
        lab.run_until_idle();
        assert!(desktop.is_present(), "KWin appeared");
        assert_eq!(desktop.var("WAYLAND_DISPLAY").as_deref(), Some("wayland-1"));
        assert_eq!(bridges.load(Ordering::Relaxed), 1, "the bridge started");

        steps_tx.try_send(BusStep::KWinGone).expect("send");
        lab.run_until_idle();
        assert!(!desktop.is_present(), "the session ended with KWin");
        assert_eq!(health_states(&mut rx), [no_session]);

        steps_tx.try_send(BusStep::KWinUp).expect("send");
        lab.run_until_idle();
        assert!(desktop.is_present());
        assert_eq!(
            bridges.load(Ordering::Relaxed),
            2,
            "the bridge started again"
        );

        // A closed channel ends the fake bridge, so the task ends.
        drop(steps_tx);
        handle.abort();
        lab.run_until_idle();
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    /// q4x76.12 review: a bridge that fails ends `session_cycles` with the error, and the
    /// session is cleared with it, so no desktop command runs against the old session while
    /// the supervisor restarts the worker.
    #[test]
    fn bridge_error_clears_the_desktop_session() {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let (io, _rx) = worker_io();
        let desktop = DesktopSession::new(DesktopSource::UserManager);
        let (steps_tx, steps) = async_channel::unbounded();
        let mut bus = FakeBus {
            steps,
            env: manager_env(&["WAYLAND_DISPLAY=wayland-1"]),
            reads: Arc::default(),
            bridges: Arc::new(AtomicU64::new(0)),
        };
        let ended = Arc::new(Mutex::new(None));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task_desktop, task_ended) = (desktop.clone(), Arc::clone(&ended));
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let mut queue = Queue {
                    last_seq: 0,
                    progress: Arc::default(),
                };
                let commands = commands(EngineMode::Full);
                let result =
                    session_cycles(&cx, &io, &task_desktop, &mut bus, &commands, &mut queue).await;
                *task_ended.lock().expect("lock") = Some(result.is_err());
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();

        steps_tx.try_send(BusStep::KWinUp).expect("send");
        lab.run_until_idle();
        assert!(desktop.is_present(), "the bridge runs");

        steps_tx.try_send(BusStep::Fail).expect("send");
        lab.run_until_idle();
        assert_eq!(
            *ended.lock().expect("lock"),
            Some(true),
            "the error ends it"
        );
        assert!(!desktop.is_present(), "the session is cleared");
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }
    /// Advances lab time by `d`, firing the timers that come due.
    fn advance(lab: &mut LabRuntime, d: Duration) {
        lab.advance_time(d.as_nanos() as u64);
        // `run_until_idle` stops when no task is ready; one step fires the due timers.
        lab.step_for_test();
        lab.run_until_idle();
    }

    /// q4x76.12 leader decision: the environment is read when KWin's name appears, again with
    /// a doubling wait while it names no display, for at most 10 s. A display that appears 2 s
    /// after the name is picked up; once the session runs, nothing reads it while idle. When no
    /// display appears in 10 s the reads stop too.
    #[test]
    fn manager_env_read_with_backoff_then_never_while_idle() {
        for display_after in [Some(Duration::from_secs(2)), None] {
            let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
            let (io, _rx) = worker_io();
            let desktop = DesktopSession::new(DesktopSource::UserManager);
            let (steps_tx, steps) = async_channel::unbounded();
            let env = manager_env(&["XDG_CURRENT_DESKTOP=KDE"]);
            let reads = Arc::new(AtomicU64::new(0));
            let bridges = Arc::new(AtomicU64::new(0));
            let mut bus = FakeBus {
                steps,
                env: Arc::clone(&env),
                reads: Arc::clone(&reads),
                bridges: Arc::clone(&bridges),
            };
            let root = lab.state.create_root_region(Budget::INFINITE);
            let task_desktop = desktop.clone();
            let (task, handle) = lab
                .state
                .create_task(root, Budget::INFINITE, async move {
                    let cx = Cx::current().expect("task has a Cx");
                    let mut queue = Queue {
                        last_seq: 0,
                        progress: Arc::default(),
                    };
                    let commands = commands(EngineMode::Full);
                    let _ =
                        session_cycles(&cx, &io, &task_desktop, &mut bus, &commands, &mut queue)
                            .await;
                })
                .expect("task is created");
            lab.scheduler.lock().schedule(task, 0);
            lab.run_until_idle();
            assert_eq!(reads.load(Ordering::Relaxed), 0, "no read before KWin");

            steps_tx.try_send(BusStep::KWinUp).expect("send");
            lab.run_until_idle();
            assert_eq!(
                reads.load(Ordering::Relaxed),
                1,
                "read when the name appears"
            );
            assert!(!desktop.is_present(), "no display yet");

            let tick = Duration::from_millis(250);
            let mut elapsed = Duration::ZERO;
            while elapsed < Duration::from_secs(12) {
                if display_after == Some(elapsed) {
                    env.lock()
                        .expect("lock")
                        .push("WAYLAND_DISPLAY=wayland-1".to_owned());
                }
                advance(&mut lab, tick);
                elapsed += tick;
            }
            // Reads at 0, 0.25, 0.75, 1.75 and 3.75 s find the display at the fifth; with none,
            // at 7.75 and 10 s too.
            let want = if display_after.is_some() { 5 } else { 7 };
            assert_eq!(reads.load(Ordering::Relaxed), want, "{display_after:?}");
            assert_eq!(bridges.load(Ordering::Relaxed), 1, "the bridge started");
            assert!(desktop.is_present(), "the session started");
            assert_eq!(
                desktop.var("WAYLAND_DISPLAY").is_some(),
                display_after.is_some()
            );

            // Idle with the bridge running: no more reads.
            for _ in 0..8 {
                advance(&mut lab, Duration::from_secs(60));
            }
            assert_eq!(reads.load(Ordering::Relaxed), want, "no reads while idle");

            drop(steps_tx);
            handle.abort();
            lab.run_until_idle();
            let report = lab.run_until_quiescent_with_report();
            assert!(report.lab_test_passed(), "{report:?}");
        }
    }
}
