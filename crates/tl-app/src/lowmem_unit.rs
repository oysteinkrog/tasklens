//! The `tasklens-lowmem.service` user unit: template, install and enable, disable and remove
//! (E32-002, PROC-097, U-24, R-152 OI-3).
//!
//! Every systemd call goes to the user manager on the session bus through blocking zbus
//! (ADR-001). These functions block for up to a few seconds, so callers run them on a plain
//! thread, never on the GPUI main thread. `wrap_persist` (E32-004) runs them on the
//! "tl-lowmem-unit" thread when `lowmem.enabled` changes.

use std::borrow::Cow;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tl_model::settings::{Settings, UiState};
use tracing::{info, warn};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::atomic::write_atomic;
use crate::paths::{Paths, ensure_dir};

pub const UNIT_NAME: &str = "tasklens-lowmem.service";
pub const OOM_SCORE_PREFERRED: i32 = -900; // U-24
pub const OOM_SCORE_FALLBACK: i32 = 0; // PROC-097
/// systemd exit status EXIT_OOM_ADJUST, set when the manager refuses OOMScoreAdjust.
pub const EXIT_OOM_ADJUST: i32 = 214;
/// The watcher binary the unit runs (crate tl-watch, tasklens-m1f.19).
pub const WATCHER_NAME: &str = "tasklens-watch";

const TEMPLATE: &str = include_str!("../../../packaging/tasklens-lowmem.service.in");
/// Unit files are read by the user manager, so they are not private (0600) like settings.
const UNIT_FILE_MODE: u32 = 0o644;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLL_LIMIT: Duration = Duration::from_secs(2);

const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";
const SERVICE_IFACE: &str = "org.freedesktop.systemd1.Service";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// D-Bus errors that mean "already stopped, disabled or gone", which removal ignores.
const ALREADY_GONE: [&str; 3] = [
    "org.freedesktop.systemd1.NoSuchUnit",
    "org.freedesktop.systemd1.UnitNotLoaded",
    "org.freedesktop.DBus.Error.FileNotFound",
];

/// The unit's state after `install_and_enable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Installed {
    /// The OOMScoreAdjust value the user manager accepted.
    pub oom_score_adjust: i32,
}

#[derive(Debug)]
pub enum LowmemUnitError {
    /// Writing or deleting the unit file failed, or `HOME` is not usable.
    Write(std::io::Error),
    /// A systemd manager call failed.
    Bus(String),
    /// The unit did not reach `active`. Holds the last ActiveState seen.
    StartFailed(String),
}

impl fmt::Display for LowmemUnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LowmemUnitError::Write(e) => write!(f, "could not write the unit file: {e}"),
            LowmemUnitError::Bus(e) => write!(f, "systemd call failed: {e}"),
            LowmemUnitError::StartFailed(state) => write!(f, "unit did not start: {state}"),
        }
    }
}

impl std::error::Error for LowmemUnitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LowmemUnitError::Write(e) => Some(e),
            _ => None,
        }
    }
}

impl From<zbus::Error> for LowmemUnitError {
    fn from(e: zbus::Error) -> Self {
        LowmemUnitError::Bus(e.to_string())
    }
}

/// Replaces @EXEC@ and @OOM_SCORE_ADJUST@. `exec` is an absolute path; it is quoted with
/// systemd C-style quoting if it contains a space.
pub fn render_unit(exec: &Path, oom_score_adjust: i32) -> String {
    TEMPLATE
        .replace("@EXEC@", &quote_exec(exec))
        .replace("@OOM_SCORE_ADJUST@", &oom_score_adjust.to_string())
}

/// Escapes `%` and `$`, which systemd expands in ExecStart, and wraps the path in double
/// quotes (escaping `\` and `"`) when it holds whitespace or a quote.
fn quote_exec(exec: &Path) -> String {
    let text = exec.to_string_lossy();
    let needs_quotes = text
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\'' || c == '\\');
    let mut out = String::with_capacity(text.len() + 2);
    if needs_quotes {
        out.push('"');
    }
    for c in text.chars() {
        match c {
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            '\\' | '"' if needs_quotes => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    if needs_quotes {
        out.push('"');
    }
    out
}

/// `tasklens-watch` in the folder of the running tasklens binary, else the first match on `PATH`.
/// Only absolute, executable regular files count.
pub fn watcher_exe() -> Option<PathBuf> {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(WATCHER_NAME)));
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(WATCHER_NAME))
            .collect::<Vec<_>>()
    });
    sibling
        .into_iter()
        .chain(on_path)
        .find(|p| p.is_absolute() && is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// $XDG_CONFIG_HOME/systemd/user/tasklens-lowmem.service; XDG_CONFIG_HOME unset, empty or not
/// absolute falls back to $HOME/.config (ADR-011 rule, through paths.rs). Fails when `HOME`
/// is not an absolute path.
pub fn unit_path() -> Result<PathBuf, LowmemUnitError> {
    unit_path_from(|name| std::env::var_os(name))
}

/// `unit_path` with the environment read through `get`.
pub fn unit_path_from(get: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, LowmemUnitError> {
    let paths = Paths::from_vars(get).map_err(|e| {
        LowmemUnitError::Write(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            e.to_string(),
        ))
    })?;
    // `config_dir` is `<config root>/tasklens`.
    let config_root = paths.config_dir.parent().unwrap_or(&paths.home);
    Ok(config_root.join("systemd/user").join(UNIT_NAME))
}

/// Writes the rendered unit to `path` (mode 0644), creating missing directories with mode 0700.
pub fn write_unit_file(
    path: &Path,
    exec: &Path,
    oom_score_adjust: i32,
) -> Result<(), LowmemUnitError> {
    if let Some(dir) = path.parent() {
        ensure_dir(dir).map_err(LowmemUnitError::Write)?;
    }
    let text = render_unit(exec, oom_score_adjust);
    write_atomic(path, text.as_bytes(), UNIT_FILE_MODE)
        .map_err(|e| LowmemUnitError::Write(std::io::Error::other(e.to_string())))
}

/// Deletes `path`. A missing file is fine.
pub fn remove_unit_file(path: &Path) -> Result<(), LowmemUnitError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(LowmemUnitError::Write(e)),
    }
}

/// 1. write_atomic(unit_path(), render_unit(watcher_exe, -900)), file mode 0644, dir created 0700.
/// 2. Manager.Reload(); Manager.EnableUnitFiles([UNIT_NAME], false, true);
///    Manager.StartUnit(UNIT_NAME, "replace").
/// 3. Poll the unit's ActiveState every 100 ms for up to 2 s. If the service exited with
///    ExecMainStatus 214, rewrite the file with OOMScoreAdjust=0, Reload(),
///    ResetFailedUnit(UNIT_NAME), StartUnit(UNIT_NAME, "replace"), and return
///    Installed { oom_score_adjust: 0 }.
/// 4. "active" returns Installed { oom_score_adjust: -900 }. Any other end state is StartFailed.
///
/// A `Type=simple` unit reports "active" right after the fork, before systemd applies
/// OOMScoreAdjust, so an early "active" is not trusted: the poll runs the full 2 s unless
/// the unit fails first.
pub fn install_and_enable() -> Result<Installed, LowmemUnitError> {
    let path = unit_path()?;
    let exec = watcher_exe().ok_or_else(|| {
        LowmemUnitError::Write(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "tasklens-watch not found next to tasklens or on PATH",
        ))
    })?;
    write_unit_file(&path, &exec, OOM_SCORE_PREFERRED)?;
    info!(path = %path.display(), "wrote the low-memory unit");

    let conn = Connection::session()?;
    manager_call(&conn, "Reload", &())?;
    manager_call(&conn, "EnableUnitFiles", &(vec![UNIT_NAME], false, true))?;
    manager_call(&conn, "StartUnit", &(UNIT_NAME, "replace"))?;

    let mut accepted = OOM_SCORE_PREFERRED;
    let mut end = poll_unit(&conn)?;
    if end.oom_adjust_refused() {
        warn!("systemd refused OOMScoreAdjust={OOM_SCORE_PREFERRED}; using {OOM_SCORE_FALLBACK}");
        write_unit_file(&path, &exec, OOM_SCORE_FALLBACK)?;
        manager_call(&conn, "Reload", &())?;
        manager_call(&conn, "ResetFailedUnit", &(UNIT_NAME,))?;
        manager_call(&conn, "StartUnit", &(UNIT_NAME, "replace"))?;
        accepted = OOM_SCORE_FALLBACK;
        end = poll_unit(&conn)?;
    }
    if end.active_state != "active" {
        warn!(path = %path.display(), state = %end.active_state, "low-memory unit did not start");
        return Err(LowmemUnitError::StartFailed(end.active_state));
    }
    info!(path = %path.display(), oom_score_adjust = accepted, "low-memory unit is active");
    Ok(Installed {
        oom_score_adjust: accepted,
    })
}

/// StopUnit(UNIT_NAME, "replace"); DisableUnitFiles([UNIT_NAME], false); delete unit_path()
/// (missing file is fine); Reload(). Idempotent.
pub fn disable_and_remove() -> Result<(), LowmemUnitError> {
    let path = unit_path()?;
    let conn = Connection::session()?;
    ignore_gone(manager_call(&conn, "StopUnit", &(UNIT_NAME, "replace")))?;
    ignore_gone(manager_call(
        &conn,
        "DisableUnitFiles",
        &(vec![UNIT_NAME], false),
    ))?;
    remove_unit_file(&path)?;
    manager_call(&conn, "Reload", &())?;
    info!(path = %path.display(), "removed the low-memory unit");
    Ok(())
}

/// What the poll saw last.
struct UnitEnd {
    active_state: String,
    sub_state: String,
    exec_main_status: i32,
}

impl UnitEnd {
    /// The service exited with EXIT_OOM_ADJUST. With `Restart=on-failure` the unit can sit in
    /// "activating"/"auto-restart" instead of "failed", so both count.
    fn oom_adjust_refused(&self) -> bool {
        let stopped = self.active_state == "failed" || self.sub_state == "auto-restart";
        stopped && self.exec_main_status == EXIT_OOM_ADJUST
    }
}

/// Polls the unit every 100 ms for up to 2 s, returning early when it has stopped.
fn poll_unit(conn: &Connection) -> Result<UnitEnd, LowmemUnitError> {
    let reply = manager_call(conn, "GetUnit", &(UNIT_NAME,))?;
    let unit: OwnedObjectPath = reply.body().deserialize()?;
    let started = Instant::now();
    loop {
        let end = UnitEnd {
            active_state: property(conn, &unit, UNIT_IFACE, "ActiveState")?,
            sub_state: property(conn, &unit, UNIT_IFACE, "SubState")?,
            exec_main_status: property(conn, &unit, SERVICE_IFACE, "ExecMainStatus")?,
        };
        let stopped = end.active_state == "failed" || end.sub_state == "auto-restart";
        if stopped || started.elapsed() >= POLL_LIMIT {
            return Ok(end);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn manager_call<B>(conn: &Connection, method: &str, body: &B) -> zbus::Result<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(Some(SYSTEMD), MANAGER_PATH, Some(MANAGER), method, body)
}

fn property<T>(
    conn: &Connection,
    unit: &OwnedObjectPath,
    iface: &str,
    name: &str,
) -> Result<T, LowmemUnitError>
where
    T: TryFrom<OwnedValue>,
    T::Error: fmt::Display,
{
    let reply = conn.call_method(Some(SYSTEMD), unit, Some(PROPERTIES), "Get", &(iface, name))?;
    let value: OwnedValue = reply.body().deserialize()?;
    T::try_from(value).map_err(|e| LowmemUnitError::Bus(format!("{iface}.{name}: {e}")))
}

/// Turns the errors that mean "already gone" into success.
fn ignore_gone(result: zbus::Result<zbus::Message>) -> Result<(), LowmemUnitError> {
    match result {
        Ok(_) => Ok(()),
        Err(zbus::Error::MethodError(name, _, _)) if ALREADY_GONE.contains(&name.as_str()) => {
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// The ADR-011 persist callback. The same type as `settings_store::PersistCallback`, spelled out
/// here so this module does not need the store.
pub type PersistFn = Box<dyn Fn(&Settings, &UiState) + Send + Sync>;

const LOG_TARGET: &str = "tasklens::engine";
const THREAD_NAME: &str = "tl-lowmem-unit";

/// The work `wrap_persist` runs. Tests swap these for counters and fakes.
pub struct UnitOps {
    pub install: Box<dyn Fn() -> Result<Installed, LowmemUnitError> + Send + Sync>,
    pub remove: Box<dyn Fn() -> Result<(), LowmemUnitError> + Send + Sync>,
    /// True when the unit file exists. Read once, when the callback is built.
    pub unit_file_exists: Box<dyn Fn() -> bool + Send + Sync>,
    /// Sends the desktop notification "Could not set up the rescue window".
    pub notify_failure: Box<dyn Fn() + Send + Sync>,
}

impl UnitOps {
    fn real() -> UnitOps {
        UnitOps {
            install: Box::new(install_and_enable),
            remove: Box::new(disable_and_remove),
            unit_file_exists: Box::new(|| unit_path().is_ok_and(|p| p.exists())),
            notify_failure: Box::new(|| {
                crate::notify::send(crate::notify::Fatal::LowmemSetupFailed)
            }),
        }
    }
}

/// Wraps the persist callback. Keeps the last seen `settings.lowmem.enabled` (starting at
/// `initial`). When the value changes, it starts one std::thread named "tl-lowmem-unit" that
/// runs install_and_enable() (false -> true) or disable_and_remove() (true -> false), then
/// calls `inner` as before. At most one such thread runs at a time; a change that arrives
/// while one runs is applied after it finishes, using the newest value.
///
/// At launch, when `initial` is true and the unit file is missing, the unit is installed again
/// (R-152 OI-7). When `initial` is false, an existing file is left alone.
///
/// On a failure the setting is reverted in every value handed to `inner` until tl-ui sends a
/// new `lowmem.enabled`, and the desktop notification is sent. tl-app cannot change tl-ui's
/// copy, so the switch shows the new value until the user changes it or restarts.
pub fn wrap_persist(inner: PersistFn, initial: bool) -> PersistFn {
    wrap_persist_with(inner, initial, UnitOps::real())
}

/// `wrap_persist` with the install, remove, file check and notification given by the caller.
pub fn wrap_persist_with(inner: PersistFn, initial: bool, ops: UnitOps) -> PersistFn {
    let applied = initial && (ops.unit_file_exists)();
    let toggle = Arc::new(Toggle {
        inner,
        ops,
        dispatch: tracing::dispatcher::get_default(|d| d.clone()),
        shared: Mutex::new(Shared {
            seen: initial,
            applied,
            failed: None,
            busy: false,
            oom_score_adjust: None,
            last: None,
        }),
    });
    if initial && !applied {
        info!(target: LOG_TARGET, "lowmem.enabled is on and the unit file is missing; installing it again");
        let mut shared = toggle.lock();
        toggle.start_worker(&mut shared);
    }
    Box::new(move |settings, state| toggle.persist(settings, state))
}

struct Toggle {
    inner: PersistFn,
    ops: UnitOps,
    /// The worker thread logs through the subscriber of the thread that built the callback.
    dispatch: tracing::Dispatch,
    shared: Mutex<Shared>,
}

struct Shared {
    /// The newest `lowmem.enabled` from tl-ui.
    seen: bool,
    /// True when the unit is installed, as far as this process knows.
    applied: bool,
    /// The value whose install or remove failed. Not retried until `seen` changes.
    failed: Option<bool>,
    /// True while the worker thread runs.
    busy: bool,
    /// The OOMScoreAdjust of an install done by this process, written into every state.
    oom_score_adjust: Option<i32>,
    /// The newest values from tl-ui while the worker runs, saved again when it ends.
    last: Option<(Settings, UiState)>,
}

impl Toggle {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn persist(self: &Arc<Self>, settings: &Settings, state: &UiState) {
        let enabled = settings.lowmem.enabled;
        let mut shared = self.lock();
        if enabled != shared.seen {
            shared.seen = enabled;
            shared.failed = None;
            if !shared.busy && enabled != shared.applied {
                self.start_worker(&mut shared);
            }
        }
        if shared.busy {
            shared.last = Some((settings.clone(), state.clone()));
        }
        // `inner` runs under the lock, so a save from the worker never overtakes a newer one.
        self.save(&shared, settings, state);
    }

    /// Calls `inner` with the reverted setting and the accepted OOMScoreAdjust applied.
    fn save(&self, shared: &Shared, settings: &Settings, state: &UiState) {
        let mut settings = Cow::Borrowed(settings);
        if shared.failed == Some(settings.lowmem.enabled)
            && settings.lowmem.enabled != shared.applied
        {
            settings.to_mut().lowmem.enabled = shared.applied;
        }
        let mut state = Cow::Borrowed(state);
        if let Some(oom) = shared.oom_score_adjust
            && state.lowmem.oom_score_adjust != Some(oom)
        {
            state.to_mut().lowmem.oom_score_adjust = Some(oom);
        }
        (self.inner)(&settings, &state);
    }

    fn start_worker(self: &Arc<Self>, shared: &mut Shared) {
        shared.busy = true;
        let toggle = self.clone();
        let spawned = std::thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                let dispatch = toggle.dispatch.clone();
                tracing::dispatcher::with_default(&dispatch, || toggle.run_worker());
            });
        if let Err(e) = spawned {
            warn!(target: LOG_TARGET, "could not start the {THREAD_NAME} thread: {e}");
            shared.busy = false;
        }
    }

    /// Applies the newest value until it matches the unit, then ends.
    fn run_worker(&self) {
        loop {
            let target = {
                let mut shared = self.lock();
                if shared.seen == shared.applied || shared.failed == Some(shared.seen) {
                    shared.busy = false;
                    shared.last = None;
                    return;
                }
                shared.seen
            };
            let result = if target {
                (self.ops.install)().map(Some)
            } else {
                (self.ops.remove)().map(|()| None)
            };
            let failed = result.is_err();
            {
                let mut shared = self.lock();
                match result {
                    Ok(Some(installed)) => {
                        info!(
                            target: LOG_TARGET,
                            oom_score_adjust = installed.oom_score_adjust,
                            "installed and enabled {UNIT_NAME}"
                        );
                        shared.applied = true;
                        shared.oom_score_adjust = Some(installed.oom_score_adjust);
                    }
                    Ok(None) => {
                        info!(target: LOG_TARGET, "disabled and removed {UNIT_NAME}");
                        shared.applied = false;
                    }
                    Err(e) => {
                        let action = if target { "install" } else { "remove" };
                        warn!(target: LOG_TARGET, "could not {action} {UNIT_NAME}: {e}");
                        shared.failed = Some(target);
                    }
                }
                if let Some((settings, state)) = &shared.last {
                    self.save(&shared, settings, state);
                }
            }
            if failed {
                (self.ops.notify_failure)();
            }
        }
    }
}
