//! "Open with Meta+Esc" through KDE kglobalaccel (APP-051, R-080, U-16).
//!
//! On Plasma 6 the Meta+Esc key opens Plasma System Monitor. Turning the option on gives that
//! key to the tasklens `.desktop` launch action and takes it away from System Monitor. Turning
//! it off clears the tasklens key and gives System Monitor back its default keys. There is no
//! settings key: the state is read from kglobalaccel.
//!
//! Every call uses its own blocking session-bus connection with a 1 s method timeout.
//!
//! kglobalacceld v6.8.2 facts this module relies on (src/kglobalacceld.cpp and
//! src/globalshortcutsregistry.cpp):
//! - `setForeignShortcutKeys` and `setShortcutKeys` do nothing when `findAction` finds no action
//!   (kglobalacceld.cpp:561 and :502). `findAction` only looks up loaded components
//!   (kglobalacceld.cpp:103, globalshortcutsregistry.cpp:300). A `.desktop` component is loaded
//!   at start only when its file has `X-KDE-Shortcuts` or `kglobalshortcutsrc` has its
//!   `[services]` group (globalshortcutsregistry.cpp:697, :752). So tasklens calls `doRegister`
//!   first, which creates the service component through `getOrCreateComponent`
//!   (kglobalacceld.cpp:360, :136, globalshortcutsregistry.cpp:306).
//! - The action `doRegister` creates is not "present", so its keys are not grabbed until it is
//!   marked present (globalshortcut.cpp:85, :178). `setForeignShortcutKeys` never sets that
//!   flag; `setShortcutKeys` with `SetPresent` does (kglobalacceld.cpp:532). tasklens therefore
//!   sets its own key with `setShortcutKeys(SetPresent | NoAutoloading)` and uses
//!   `setForeignShortcutKeys` for System Monitor, which is loaded and present already.
//! - Keys are not checked for clashes (globalshortcut.cpp:131), so System Monitor is unbound
//!   before tasklens takes the key.

use std::path::Path;
use std::time::Duration;

use tracing::warn;

use crate::atomic::write_atomic;

const SERVICE: &str = "org.kde.kglobalaccel";
const PATH: &str = "/kglobalaccel";
const INTERFACE: &str = "org.kde.KGlobalAccel";
const TIMEOUT: Duration = Duration::from_secs(1);

/// The tasklens launch component (the installed `.desktop` file name).
const TL_COMPONENT: &str = "io.github.oysteinkrog.Tasklens.desktop";
/// The Plasma System Monitor launch component.
const SM_COMPONENT: &str = "org.kde.plasma-systemmonitor.desktop";
/// The action of a `.desktop` component that starts the application.
const LAUNCH: &str = "_launch";

/// Qt `Qt::META | Qt::Key_Escape` (0x1000_0000 | 0x0100_0000).
const META_ESC: i32 = 0x1100_0000;
/// The key as KDE writes it in `kglobalshortcutsrc` and as the settings row shows it.
pub const META_ESC_TEXT: &str = "Meta+Esc";

/// `KGlobalAccelD::SetShortcutFlag` (kglobalacceld.h:39-41).
const SET_PRESENT: u32 = 2;
const NO_AUTOLOADING: u32 = 4;

/// The `kglobalshortcutsrc` group of the tasklens launch component.
const RC_GROUP: &str = "[services][io.github.oysteinkrog.Tasklens.desktop]";

/// One key sequence: up to 4 Qt key codes, padded with 0. D-Bus type `(ai)`.
type KeySeq = (Vec<i32>,);

/// How `set_enabled` applied the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// kglobalaccel took the change at once.
    Live,
    /// The change did not stick; `kglobalshortcutsrc` was edited and KDE applies it at the
    /// next login (U-16).
    NeedsRelogin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KgaError {
    /// No session bus or no kglobalaccel service.
    Unavailable,
    /// A kglobalaccel call failed.
    Call(String),
    /// Reading or writing `kglobalshortcutsrc` failed.
    Io(String),
}

impl std::fmt::Display for KgaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KgaError::Unavailable => f.write_str("KDE global shortcuts are not available"),
            KgaError::Call(e) => write!(f, "KDE global shortcut call failed: {e}"),
            KgaError::Io(e) => write!(f, "could not update kglobalshortcutsrc: {e}"),
        }
    }
}

/// True when the tasklens launch action is bound to Meta+Esc.
pub fn is_enabled() -> Result<bool, KgaError> {
    let result = connect().and_then(|conn| tasklens_bound(&conn));
    if let Err(e) = &result {
        warn!(target: "tasklens::shortcut", "reading the Meta+Esc shortcut failed: {e}");
    }
    result
}

/// Binds Meta+Esc to tasklens (`on`) or gives it back to System Monitor. `rc_file` is
/// `$XDG_CONFIG_HOME/kglobalshortcutsrc`, edited only when kglobalaccel did not take the change.
pub fn set_enabled(on: bool, rc_file: &Path) -> Result<Applied, KgaError> {
    let result = connect().and_then(|conn| set_on(&conn, on, rc_file));
    if let Err(e) = &result {
        warn!(target: "tasklens::shortcut", on, "setting the Meta+Esc shortcut failed: {e}");
    }
    result
}

fn set_on(
    conn: &zbus::blocking::Connection,
    on: bool,
    rc_file: &Path,
) -> Result<Applied, KgaError> {
    let tl = action_id(TL_COMPONENT, "tasklens");
    let sm = action_id(SM_COMPONENT, "");
    // Loads the tasklens component and its launch action; a no-op when they exist.
    call(conn, "doRegister", &(&tl,))?;
    if on {
        call(conn, "setForeignShortcutKeys", &(&sm, Vec::<KeySeq>::new()))?;
        if let Err(e) = call(
            conn,
            "setShortcutKeys",
            &(&tl, vec![key_seq(META_ESC)], SET_PRESENT | NO_AUTOLOADING),
        ) {
            // Never leave the user with no Meta+Esc at all.
            let _ = restore_system_monitor(conn, &sm);
            return Err(e);
        }
    } else {
        call(
            conn,
            "setShortcutKeys",
            &(&tl, Vec::<KeySeq>::new(), SET_PRESENT | NO_AUTOLOADING),
        )?;
        restore_system_monitor(conn, &sm)?;
    }
    if tasklens_bound(conn)? == on {
        return Ok(Applied::Live);
    }
    warn!(target: "tasklens::shortcut", on, "kglobalaccel did not take the change; editing kglobalshortcutsrc");
    rc_fallback(rc_file, on)?;
    Ok(Applied::NeedsRelogin)
}

/// Gives System Monitor its default launch keys back (Meta+Esc on Plasma 6), never a fixed key.
fn restore_system_monitor(
    conn: &zbus::blocking::Connection,
    sm: &[String],
) -> Result<(), KgaError> {
    let defaults: Vec<KeySeq> = call(conn, "defaultShortcutKeys", &(sm,))?
        .body()
        .deserialize()
        .map_err(|e| KgaError::Call(e.to_string()))?;
    call(conn, "setForeignShortcutKeys", &(sm, defaults)).map(|_| ())
}

/// Reads the tasklens launch keys back.
fn tasklens_bound(conn: &zbus::blocking::Connection) -> Result<bool, KgaError> {
    let keys: Vec<KeySeq> = call(
        conn,
        "shortcutKeys",
        &(action_id(TL_COMPONENT, "tasklens"),),
    )?
    .body()
    .deserialize()
    .map_err(|e| KgaError::Call(e.to_string()))?;
    Ok(keys.iter().any(|(seq,)| seq.first() == Some(&META_ESC)))
}

/// The 4-string action id: component, action, component friendly name, action friendly name.
/// A `Vec`, so it goes over D-Bus as `as`; an array would be the struct `(ssss)`.
fn action_id(component: &str, friendly: &str) -> Vec<String> {
    vec![
        component.to_owned(),
        LAUNCH.to_owned(),
        friendly.to_owned(),
        friendly.to_owned(),
    ]
}

fn key_seq(key: i32) -> KeySeq {
    (vec![key, 0, 0, 0],)
}

/// A fresh session-bus connection whose method calls time out after 1 s.
fn connect() -> Result<zbus::blocking::Connection, KgaError> {
    zbus::blocking::connection::Builder::session()
        .and_then(|b| b.method_timeout(TIMEOUT).build())
        .map_err(|e| map_error(&e))
}

fn call<B>(
    conn: &zbus::blocking::Connection,
    method: &str,
    body: &B,
) -> Result<zbus::Message, KgaError>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(Some(SERVICE), PATH, Some(INTERFACE), method, body)
        .map_err(|e| map_error(&e))
}

/// No bus, a timeout or a missing kglobalaccel service is `Unavailable`; anything else is a
/// failed call.
fn map_error(e: &zbus::Error) -> KgaError {
    const GONE: [&str; 3] = [
        "org.freedesktop.DBus.Error.ServiceUnknown",
        "org.freedesktop.DBus.Error.NameHasNoOwner",
        "org.freedesktop.DBus.Error.UnknownObject",
    ];
    match e {
        zbus::Error::Address(_)
        | zbus::Error::InputOutput(_)
        | zbus::Error::Connection(..)
        | zbus::Error::Handshake(_) => KgaError::Unavailable,
        zbus::Error::MethodError(name, ..) if GONE.contains(&name.as_str()) => {
            KgaError::Unavailable
        }
        zbus::Error::FDO(fdo) => match **fdo {
            zbus::fdo::Error::ServiceUnknown(_)
            | zbus::fdo::Error::NameHasNoOwner(_)
            | zbus::fdo::Error::UnknownObject(_)
            | zbus::fdo::Error::IOError(_)
            | zbus::fdo::Error::NoReply(_)
            | zbus::fdo::Error::Timeout(_) => KgaError::Unavailable,
            _ => KgaError::Call(e.to_string()),
        },
        _ => KgaError::Call(e.to_string()),
    }
}

/// The U-16 fallback: writes or removes `_launch=Meta+Esc` in the tasklens group of
/// `kglobalshortcutsrc`. Every other byte of the file stays the same. A new file gets mode 0600.
fn rc_fallback(rc_file: &Path, on: bool) -> Result<(), KgaError> {
    use std::os::unix::fs::PermissionsExt;

    let (text, mode) = match std::fs::read(rc_file) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes).map_err(|e| KgaError::Io(e.to_string()))?;
            let mode = std::fs::metadata(rc_file)
                .map(|m| m.permissions().mode() & 0o7777)
                .unwrap_or(0o600);
            (text, mode)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), 0o600),
        Err(e) => return Err(KgaError::Io(e.to_string())),
    };
    let new = rc_set_launch(&text, on);
    if new == text {
        return Ok(());
    }
    write_atomic(rc_file, new.as_bytes(), mode).map_err(|e| KgaError::Io(e.to_string()))
}

/// Sets (`on`) or removes the `_launch` line of the tasklens group in `kglobalshortcutsrc`
/// text. On: replaces the value, or inserts the line after the group header, or appends the
/// group after one blank line. Off: removes the line and keeps the header. Every other line
/// keeps its bytes.
pub fn rc_set_launch(text: &str, on: bool) -> String {
    let line = format!("{LAUNCH}={META_ESC_TEXT}\n");
    let mut out = String::with_capacity(text.len() + RC_GROUP.len() + line.len() + 2);
    let mut in_group = false;
    let mut found_group = false;
    let mut done = false;
    for raw in text.split_inclusive('\n') {
        let content = raw.trim_end_matches(['\n', '\r']);
        if content.starts_with('[') {
            if in_group && on && !done {
                out.push_str(&line);
                done = true;
            }
            in_group = content == RC_GROUP;
            if in_group {
                found_group = true;
                if on && !done {
                    // The header, then the line right after it; a later `_launch` is dropped.
                    push_line(&mut out, raw);
                    out.push_str(&line);
                    done = true;
                    continue;
                }
            }
        } else if in_group && is_launch_line(content) {
            continue;
        }
        out.push_str(raw);
    }
    if on && !found_group {
        if !out.is_empty() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
        }
        out.push_str(RC_GROUP);
        out.push('\n');
        out.push_str(&line);
    }
    out
}

/// Pushes `raw` and makes sure it ends with a newline.
fn push_line(out: &mut String, raw: &str) {
    out.push_str(raw);
    if !raw.ends_with('\n') {
        out.push('\n');
    }
}

/// `_launch=…` with optional spaces around the key, as KConfig reads it.
fn is_launch_line(content: &str) -> bool {
    content
        .split_once('=')
        .is_some_and(|(key, _)| key.trim() == LAUNCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOOBAR: &str = "[services][foobar2000.desktop]\n_launch=Meta+F\tCtrl+Alt+F\n\n\
[kwin]\nExpose=Ctrl+F9\tnone,Ctrl+F9,Toggle Present Windows (Current desktop)\n";

    #[test]
    fn meta_esc_key_code() {
        assert_eq!(META_ESC, 285_212_672);
        assert_eq!(key_seq(META_ESC), (vec![285_212_672, 0, 0, 0],));
    }

    #[test]
    fn dbus_signatures_match_kglobalaccel() {
        use zbus::zvariant::DynamicType;

        // kf6_org.kde.KGlobalAccel.xml: shortcutKeys(as), setShortcutKeys(as, a(ai), u),
        // setForeignShortcutKeys(as, a(ai)).
        let id = action_id(TL_COMPONENT, "tasklens");
        assert_eq!((&id,).signature().to_string(), "(as)");
        let keys = vec![key_seq(META_ESC)];
        assert_eq!(
            (&id, keys.clone(), SET_PRESENT | NO_AUTOLOADING)
                .signature()
                .to_string(),
            "(asa(ai)u)"
        );
        assert_eq!((&id, keys).signature().to_string(), "(asa(ai))");
    }

    #[test]
    fn rc_fallback_writes_line() {
        // New file.
        assert_eq!(
            rc_set_launch("", true),
            "[services][io.github.oysteinkrog.Tasklens.desktop]\n_launch=Meta+Esc\n"
        );
        // Existing group without the key: the line goes right after the header.
        let text = "[a]\nx=1\n\n[services][io.github.oysteinkrog.Tasklens.desktop]\nfoo=bar\n";
        assert_eq!(
            rc_set_launch(text, true),
            "[a]\nx=1\n\n[services][io.github.oysteinkrog.Tasklens.desktop]\n\
_launch=Meta+Esc\nfoo=bar\n"
        );
        // Existing value replaced, in place of the old line's group.
        let text = "[services][io.github.oysteinkrog.Tasklens.desktop]\n_launch=none\n[b]\ny=2\n";
        assert_eq!(
            rc_set_launch(text, true),
            "[services][io.github.oysteinkrog.Tasklens.desktop]\n_launch=Meta+Esc\n[b]\ny=2\n"
        );
        // Already set: unchanged.
        let set = rc_set_launch("", true);
        assert_eq!(rc_set_launch(&set, true), set);
    }

    #[test]
    fn rc_fallback_removes_line() {
        let text = "[a]\nx=1\n\n[services][io.github.oysteinkrog.Tasklens.desktop]\n\
_launch=Meta+Esc\nfoo=bar\n\n[b]\n_launch=keep\n";
        assert_eq!(
            rc_set_launch(text, false),
            "[a]\nx=1\n\n[services][io.github.oysteinkrog.Tasklens.desktop]\nfoo=bar\n\n\
[b]\n_launch=keep\n"
        );
        // No group: unchanged.
        assert_eq!(rc_set_launch(FOOBAR, false), FOOBAR);
        // On then off leaves only the empty group behind.
        let on = rc_set_launch(FOOBAR, true);
        assert_eq!(
            rc_set_launch(&on, false),
            format!("{FOOBAR}\n[services][io.github.oysteinkrog.Tasklens.desktop]\n")
        );
    }

    #[test]
    fn rc_keeps_other_groups() {
        let on = rc_set_launch(FOOBAR, true);
        assert!(
            on.starts_with(FOOBAR),
            "other groups keep every byte: {on:?}"
        );
        assert_eq!(
            &on[FOOBAR.len()..],
            "\n[services][io.github.oysteinkrog.Tasklens.desktop]\n_launch=Meta+Esc\n"
        );
        // A CRLF file keeps its line ends.
        let crlf = "[a]\r\nx=1\r\n";
        assert!(rc_set_launch(crlf, true).starts_with(crlf));
    }

    #[test]
    fn rc_no_trailing_newline() {
        assert_eq!(
            rc_set_launch("[a]\nx=1", true),
            "[a]\nx=1\n\n[services][io.github.oysteinkrog.Tasklens.desktop]\n_launch=Meta+Esc\n"
        );
        // The group is the last line, with no newline after its header.
        assert_eq!(
            rc_set_launch("[services][io.github.oysteinkrog.Tasklens.desktop]", true),
            "[services][io.github.oysteinkrog.Tasklens.desktop]\n_launch=Meta+Esc\n"
        );
        assert_eq!(
            rc_set_launch(
                "[services][io.github.oysteinkrog.Tasklens.desktop]\n_launch=Meta+Esc",
                false
            ),
            "[services][io.github.oysteinkrog.Tasklens.desktop]\n"
        );
    }

    #[test]
    fn no_bus_is_unavailable() {
        let e = zbus::Error::Address("no session bus".into());
        assert_eq!(map_error(&e), KgaError::Unavailable);
        let io = zbus::Error::InputOutput(std::sync::Arc::new(std::io::Error::from(
            std::io::ErrorKind::TimedOut,
        )));
        assert_eq!(map_error(&io), KgaError::Unavailable);
        let fdo = zbus::Error::FDO(Box::new(zbus::fdo::Error::ServiceUnknown(
            "org.kde.kglobalaccel".into(),
        )));
        assert_eq!(map_error(&fdo), KgaError::Unavailable);
        assert!(matches!(
            map_error(&zbus::Error::InvalidReply),
            KgaError::Call(_)
        ));
    }

    #[test]
    fn rc_fallback_file_round_trip() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join("kglobalshortcutsrc");
        rc_fallback(&rc, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&rc).unwrap(),
            rc_set_launch("", true)
        );
        assert_eq!(
            std::fs::metadata(&rc).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o644)).unwrap();
        rc_fallback(&rc, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&rc).unwrap(),
            "[services][io.github.oysteinkrog.Tasklens.desktop]\n"
        );
        assert_eq!(
            std::fs::metadata(&rc).unwrap().permissions().mode() & 0o777,
            0o644,
            "an existing file keeps its mode"
        );
    }
}
