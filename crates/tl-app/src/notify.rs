//! Desktop notifications for fatal errors (UI-SHL-114, APP-055, ADR-015, R-049).
//!
//! These failures can happen when no GPUI window can show a toast, so tl-app sends them through
//! `org.freedesktop.Notifications.Notify` on its own blocking session-bus connection. `send` may
//! run from the panic hook: it never panics, shares no lock with the GPUI thread, logs every error
//! at `warn!` and returns within about 1 s.
//!
//! The texts are fixed English (`&'static str`) so the panic path needs no string bundle. The
//! same texts are in the catalogue under [`Fatal::key`] for the later locales (R-132).

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use tracing::warn;
use zbus::zvariant::Value;

const APP_ID: &str = "io.github.oysteinkrog.Tasklens";
const TIMEOUT: Duration = Duration::from_secs(1);
/// "critical" in the freedesktop spec (R-159 OI-7).
const URGENCY_CRITICAL: u8 = 2;

/// A failure that is reported as a desktop notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fatal {
    Crashed,
    RendererFailed,
    DeviceLost,
}

impl Fatal {
    /// The notification summary, exact (UI-SHL-114).
    pub fn text(self) -> &'static str {
        match self {
            Fatal::Crashed => "tasklens stopped after an unrecoverable error.",
            Fatal::RendererFailed => "tasklens could not start the renderer",
            Fatal::DeviceLost => "tasklens lost the graphics device and must restart",
        }
    }

    /// The catalogue key that holds the same text (R-132).
    pub fn key(self) -> &'static str {
        match self {
            Fatal::Crashed => "app-114-tasklens-stopped-after-an-unrecoverable",
            Fatal::RendererFailed => "app-114-tasklens-could-not-start-the",
            Fatal::DeviceLost => "app-114-tasklens-lost-the-graphics-device",
        }
    }
}

/// Sends the notification for `kind`. Blocking, best effort, at most 1 s.
pub fn send(kind: Fatal) {
    let (done_tx, done_rx) = mpsc::channel();
    // A worker thread bounds the whole call, connection set-up included, to 1 s.
    let spawned = std::thread::Builder::new()
        .name("tl-notify".into())
        .spawn(move || {
            send_on(connect(), kind);
            let _ = done_tx.send(());
        });
    match spawned {
        Ok(_) => {
            if done_rx.recv_timeout(TIMEOUT).is_err() {
                warn!(?kind, "desktop notification did not finish within 1 s");
            }
        }
        Err(e) => warn!(?kind, "could not start the notification thread: {e}"),
    }
}

/// A fresh session-bus connection whose method calls time out after 1 s.
fn connect() -> zbus::Result<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::session()?
        .method_timeout(TIMEOUT)
        .build()
}

/// Calls `Notify` on `conn`. Every error is logged and swallowed.
fn send_on(conn: zbus::Result<zbus::blocking::Connection>, kind: Fatal) {
    let conn = match conn {
        Ok(conn) => conn,
        Err(e) => {
            warn!(?kind, "no session bus for the desktop notification: {e}");
            return;
        }
    };
    let actions: Vec<&str> = Vec::new();
    let body = (
        "tasklens",
        0u32,
        APP_ID,
        kind.text(),
        "",
        actions,
        hints(),
        -1i32,
    );
    if let Err(e) = conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &body,
    ) {
        warn!(?kind, "desktop notification failed: {e}");
    }
}

/// The `Notify` hints: the desktop entry and critical urgency.
fn hints() -> HashMap<&'static str, Value<'static>> {
    HashMap::from([
        ("desktop-entry", Value::from(APP_ID)),
        ("urgency", Value::U8(URGENCY_CRITICAL)),
    ])
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    const ALL: [Fatal; 3] = [Fatal::Crashed, Fatal::RendererFailed, Fatal::DeviceLost];

    #[test]
    fn texts_exact() {
        assert_eq!(
            Fatal::Crashed.text(),
            "tasklens stopped after an unrecoverable error."
        );
        assert_eq!(
            Fatal::RendererFailed.text(),
            "tasklens could not start the renderer"
        );
        assert_eq!(
            Fatal::DeviceLost.text(),
            "tasklens lost the graphics device and must restart"
        );
    }

    #[test]
    fn catalogue_matches_texts() {
        for kind in ALL {
            assert_eq!(tl_ui::i18n::tr_en(kind.key(), &[]).as_ref(), kind.text());
        }
    }

    #[test]
    fn no_bus_is_silent() {
        let start = Instant::now();
        send_on(Err(zbus::Error::Failure("no bus".into())), Fatal::Crashed);
        let address = "unix:path=/nonexistent/tasklens-test-bus";
        let conn = zbus::blocking::connection::Builder::address(address)
            .and_then(|b| b.method_timeout(TIMEOUT).build());
        assert!(conn.is_err());
        send_on(conn, Fatal::DeviceLost);
        assert!(start.elapsed() < TIMEOUT);
    }

    #[test]
    fn hints_fields() {
        let hints = hints();
        assert_eq!(hints.len(), 2);
        assert_eq!(hints["desktop-entry"], Value::from(APP_ID));
        assert_eq!(hints["urgency"], Value::U8(2));
    }
}
