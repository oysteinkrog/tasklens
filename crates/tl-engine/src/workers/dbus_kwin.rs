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

use std::fs::{DirBuilder, Permissions};
use std::future::Future;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time;
use asupersync::{Cx, Outcome};
use futures_lite::StreamExt;
use futures_lite::stream::Boxed;
use tl_collect::sanitize::sanitize_text;
use tl_model::health::{SourceId, SourceState};
use tl_model::snapshot::{ComponentHeader, FrameGeometry, WindowEntry, WindowsSnapshot};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{Bus, DbusConn, DbusError, READ_TIMEOUT, connect};
use crate::source::WorkerIo;
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

/// Full engine only. The low-memory `KeepAbove` use comes with the window commands (E14-018).
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::KWin,
        name: "kwin",
        modes: &[EngineMode::Full],
        factory,
    }
}

pub(crate) fn factory(_cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        async move {
            let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
            run(cx, io, desktop.as_deref()).await
        }
    })
}

/// True when the colon-separated `XDG_CURRENT_DESKTOP` list holds `KDE` (case-sensitive).
fn kde_desktop(desktop: Option<&str>) -> bool {
    desktop.is_some_and(|list| list.split(':').any(|name| name == "KDE"))
}

async fn run(cx: Cx, io: WorkerIo, desktop: Option<&str>) -> Outcome<(), WorkerError> {
    if !kde_desktop(desktop) {
        return unsupported(&cx, &io).await;
    }
    let conn = match connect(&cx, Bus::Session).await {
        Ok(conn) => conn,
        Err(e) => return failed(&cx, &io, e.into()).await,
    };
    match kwin_present(&cx, &conn).await {
        Ok(true) => {}
        Ok(false) => return unsupported(&cx, &io).await,
        Err(e) => return failed(&cx, &io, e.into()).await,
    }
    let Some(path) = script_path() else {
        let e = WorkerError::Other("XDG_RUNTIME_DIR is not set".into());
        return failed(&cx, &io, e).await;
    };
    let result = serve(&cx, &io, &conn, &path).await;
    unload_on_exit(&cx, &conn).await;
    // One unlink on the runtime tmpfs. It runs inline because the blocking pool refuses new
    // jobs once the region is cancelled.
    let _ = std::fs::remove_file(&path);
    match result {
        Ok(()) => Outcome::Ok(()),
        Err(_) if cx.is_cancel_requested() => Outcome::Ok(()),
        Err(e) => failed(&cx, &io, e).await,
    }
}

/// Publishes the empty `Unsupported` snapshot and the terminal health once, then idles.
async fn unsupported(cx: &Cx, io: &WorkerIo) -> Outcome<(), WorkerError> {
    let empty = SourceUpdate::Windows(Box::new(snapshot(Vec::new(), Validity::Unsupported)));
    if let Some(stop) = send(cx, io, empty).await {
        return stop;
    }
    let state = health(SourceState::Unsupported, Some(NOT_AVAILABLE.into()));
    if let Some(stop) = send(cx, io, state).await {
        return stop;
    }
    loop {
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
}

/// Serves the bridge until `cx` is cancelled (`Ok`) or the bus fails (`Err`).
async fn serve(cx: &Cx, io: &WorkerIo, conn: &DbusConn, path: &Path) -> Result<(), WorkerError> {
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
    let mut events: Boxed<Event> = rx.or(owner_events).boxed();
    loop {
        match conn.next_signal(cx, &mut events, READ_TIMEOUT).await {
            Ok(None) => {}
            Ok(Some(Event::Windows(windows))) => {
                let update = SourceUpdate::Windows(Box::new(snapshot(windows, Validity::Valid)));
                if send(cx, io, update).await.is_some() {
                    return closed(cx);
                }
            }
            Ok(Some(Event::Owner(new))) => {
                let back = new.is_some();
                *owner.lock().unwrap_or_else(PoisonError::into_inner) = new;
                if back {
                    // KWin restarted: the new instance has no script loaded.
                    load_script(cx, conn, path).await?;
                } else {
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
    }
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
    let mut job = cx
        .spawn_blocking(move |_| write_private(&path, text.as_bytes()))
        .map_err(|e| WorkerError::Other(format!("spawn: {e}").into()))?;
    match job.join(cx).await {
        Ok(result) => result,
        Err(JoinError::Cancelled(_)) => Err(WorkerError::Closed),
        Err(JoinError::Panicked(_)) => Err(WorkerError::Other("script write panicked".into())),
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
    let scripting = conn
        .read(
            cx,
            zbus::Proxy::new(conn.raw(), KWIN_NAME, SCRIPTING_PATH, SCRIPTING_INTERFACE),
        )
        .await?;
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
                let _ = run(cx, io, Some("GNOME")).await;
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
    fn spec_is_full_mode_only() {
        let s = spec();
        assert_eq!(s.id, SourceId::KWin);
        assert_eq!(s.name, "kwin");
        assert_eq!(s.modes, &[EngineMode::Full]);
        let full = crate::workers::workers_for(EngineMode::Full);
        assert_eq!(full.iter().filter(|w| w.id == SourceId::KWin).count(), 1);
    }
}
