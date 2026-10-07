//! The `tasklens-lowmem.service` user unit: template, install and enable, disable and remove
//! (E32-002, PROC-097, U-24, R-152 OI-3).
//!
//! Every systemd call goes to the user manager on the session bus through blocking zbus
//! (ADR-001). These functions block for up to a few seconds, so callers run them on a plain
//! thread, never on the GPUI main thread.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// 1. write_atomic(unit_path(), render_unit(current_exe, -900)), file mode 0644, dir created 0700.
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
    let exec = std::env::current_exe().map_err(LowmemUnitError::Write)?;
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
