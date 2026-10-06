//! Single instance through `org.freedesktop.Application` on the session bus (ADR-010, APP-011).
//!
//! The first copy owns `BUS_NAME` and passes requests to the GPUI main thread as `AppRequest`
//! values. A second copy forwards its arguments to the first and exits 0. Any session client
//! can call these methods, so every parameter is validated before anything is sent. Logs name
//! the method only, never `platform_data` or tokens.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tl_model::command::{AppRequest, Tab};
use tracing::{debug, info, warn};
use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::zvariant::{OwnedValue, Value};

pub const BUS_NAME: &str = "io.github.oysteinkrog.Tasklens";
pub const OBJECT_PATH: &str = "/io/github/oysteinkrog/Tasklens";
const INTERFACE: &str = "org.freedesktop.Application";
/// Bound on every forwarding call and on the name request.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// Requests queued for the UI. A fifth is dropped while the UI is busy.
const QUEUE: usize = 4;
/// Longer activation tokens are ignored.
const MAX_TOKEN_BYTES: usize = 256;
const TOKEN_KEY: &str = "activation-token";
const BAD_ACTION: &str = "unknown action or bad parameter";

pub enum Outcome {
    /// This copy owns the name. `main` keeps `conn` alive until exit.
    Primary {
        conn: Connection,
        requests: async_channel::Receiver<AppRequest>,
    },
    /// Another copy runs and got the arguments. The caller exits 0.
    Forwarded,
    /// No session bus. The caller runs without single instance.
    NoBus,
}

/// Command-line arguments a second copy forwards. `--start-minimized` is not forwarded.
pub struct Forward {
    pub tab: Option<Tab>,
    pub select_pid: Option<u32>,
}

/// Takes `BUS_NAME` on the session bus, or forwards to the copy that owns it.
pub fn acquire(forward: &Forward, activation_token: Option<String>) -> Outcome {
    acquire_named(BUS_NAME, forward, activation_token)
}

/// `acquire` with another bus name, for tests.
pub fn acquire_named(name: &str, forward: &Forward, activation_token: Option<String>) -> Outcome {
    let conn = Builder::session().and_then(|b| b.method_timeout(CALL_TIMEOUT).build());
    acquire_on(conn, name, forward, activation_token)
}

/// `acquire_named` on a given connection, for tests on a private bus.
pub fn acquire_on(
    conn: zbus::Result<Connection>,
    name: &str,
    forward: &Forward,
    activation_token: Option<String>,
) -> Outcome {
    let conn = match conn {
        Ok(conn) => conn,
        Err(e) => {
            warn!("Session bus unavailable; running without single instance: {e}");
            return Outcome::NoBus;
        }
    };
    // Serve before taking the name, so a second copy never finds the name without the object.
    let (tx, requests) = async_channel::bounded(QUEUE);
    if let Err(e) = conn.object_server().at(OBJECT_PATH, AppIface { tx }) {
        warn!("Could not serve {INTERFACE}; running without single instance: {e}");
        return Outcome::NoBus;
    }
    match conn.request_name_with_flags(name, RequestNameFlags::DoNotQueue.into()) {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
            Outcome::Primary { conn, requests }
        }
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(zbus::Error::NameTaken) => {
            let _ = conn.object_server().remove::<AppIface, _>(OBJECT_PATH);
            forward_to(&conn, name, forward, activation_token);
            Outcome::Forwarded
        }
        Err(e) => {
            warn!("Could not request the bus name; running without single instance: {e}");
            Outcome::NoBus
        }
    }
}

/// Calls `Activate`, then one `ActivateAction` per forwarded argument. Failures are logged
/// and the copy still exits 0.
fn forward_to(conn: &Connection, name: &str, forward: &Forward, token: Option<String>) {
    let mut platform_data: HashMap<&str, Value<'_>> = HashMap::new();
    if let Some(token) = token.as_deref() {
        platform_data.insert(TOKEN_KEY, Value::from(token));
    }
    call(conn, name, "Activate", &(&platform_data,));
    if let Some(tab) = forward.tab {
        let parameter = vec![Value::from(tab.as_str())];
        call(
            conn,
            name,
            "ActivateAction",
            &("tab", parameter, &platform_data),
        );
    }
    if let Some(pid) = forward.select_pid {
        let parameter = vec![Value::from(pid)];
        call(
            conn,
            name,
            "ActivateAction",
            &("select-pid", parameter, &platform_data),
        );
    }
}

/// Calls `method` on the running copy. The connection's method timeout bounds the wait.
fn call<B>(conn: &Connection, name: &str, method: &str, body: &B)
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    if let Err(e) = conn.call_method(Some(name), OBJECT_PATH, Some(INTERFACE), method, body) {
        warn!("Forwarding {method} to the running copy failed: {e}");
    }
}

/// Why `parse_action` rejected a call.
#[derive(Debug, PartialEq, Eq)]
pub enum ActionErr {
    UnknownAction,
    BadParameter,
}

/// Validates an `ActivateAction` call. `Activate` requests carry no token; the caller adds it.
fn parse_action(name: &str, params: &[OwnedValue]) -> Result<Vec<AppRequest>, ActionErr> {
    let activate = AppRequest::Activate { token: None };
    match name {
        "quit" => match params {
            [] => Ok(vec![AppRequest::Quit]),
            _ => Err(ActionErr::BadParameter),
        },
        "tab" => match params {
            [value] => match &**value {
                Value::Str(s) => Tab::from_str(s.as_str())
                    .map(|tab| vec![activate, AppRequest::SelectTab(tab)])
                    .ok_or(ActionErr::BadParameter),
                _ => Err(ActionErr::BadParameter),
            },
            _ => Err(ActionErr::BadParameter),
        },
        "select-pid" => match params {
            [value] => match &**value {
                Value::U32(pid) if *pid > 0 => Ok(vec![activate, AppRequest::SelectPid(*pid)]),
                _ => Err(ActionErr::BadParameter),
            },
            _ => Err(ActionErr::BadParameter),
        },
        _ => Err(ActionErr::UnknownAction),
    }
}

/// The `activation-token` entry when it is a string of at most `MAX_TOKEN_BYTES` bytes.
fn activation_token(platform_data: &HashMap<String, OwnedValue>) -> Option<Arc<str>> {
    match &**platform_data.get(TOKEN_KEY)? {
        Value::Str(s) if s.len() <= MAX_TOKEN_BYTES => Some(Arc::from(s.as_str())),
        _ => None,
    }
}

struct AppIface {
    tx: async_channel::Sender<AppRequest>,
}

impl AppIface {
    /// Queues `request`. A full or closed channel drops it.
    fn send(&self, request: AppRequest) {
        if let Err(e) = self.tx.try_send(request) {
            let kind = if e.is_full() { "full" } else { "closed" };
            debug!("App request dropped; the queue is {kind}");
        }
    }
}

#[zbus::interface(name = "org.freedesktop.Application")]
impl AppIface {
    fn activate(&self, platform_data: HashMap<String, OwnedValue>) {
        info!("D-Bus Activate received");
        self.send(AppRequest::Activate {
            token: activation_token(&platform_data),
        });
    }

    fn open(
        &self,
        _uris: Vec<String>,
        _platform_data: HashMap<String, OwnedValue>,
    ) -> fdo::Result<()> {
        info!("D-Bus Open received");
        Err(fdo::Error::NotSupported(
            "tasklens does not open files".into(),
        ))
    }

    fn activate_action(
        &self,
        action_name: String,
        parameter: Vec<OwnedValue>,
        platform_data: HashMap<String, OwnedValue>,
    ) -> fdo::Result<()> {
        info!("D-Bus ActivateAction received");
        let requests = parse_action(&action_name, &parameter)
            .map_err(|_| fdo::Error::InvalidArgs(BAD_ACTION.into()))?;
        let token = activation_token(&platform_data);
        for request in requests {
            match request {
                AppRequest::Activate { .. } => self.send(AppRequest::Activate {
                    token: token.clone(),
                }),
                other => self.send(other),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(v: impl Into<Value<'static>>) -> OwnedValue {
        v.into().try_into().expect("owned value")
    }

    #[test]
    fn parse_tab_action() {
        for tab in Tab::ALL {
            let got = parse_action("tab", &[val(tab.as_str())]).expect("valid");
            assert_eq!(
                got,
                vec![
                    AppRequest::Activate { token: None },
                    AppRequest::SelectTab(tab)
                ]
            );
        }
        assert_eq!(
            parse_action("tab", &[val("nowhere")]),
            Err(ActionErr::BadParameter)
        );
        assert_eq!(parse_action("tab", &[]), Err(ActionErr::BadParameter));
        assert_eq!(
            parse_action("tab", &[val("services"), val("services")]),
            Err(ActionErr::BadParameter)
        );
    }

    #[test]
    fn parse_select_pid_action() {
        assert_eq!(
            parse_action("select-pid", &[val(4242u32)]),
            Ok(vec![
                AppRequest::Activate { token: None },
                AppRequest::SelectPid(4242)
            ])
        );
        assert_eq!(
            parse_action("select-pid", &[val(u32::MAX)]),
            Ok(vec![
                AppRequest::Activate { token: None },
                AppRequest::SelectPid(u32::MAX)
            ])
        );
    }

    #[test]
    fn reject_zero_pid() {
        assert_eq!(
            parse_action("select-pid", &[val(0u32)]),
            Err(ActionErr::BadParameter)
        );
    }

    #[test]
    fn reject_wrong_type() {
        let bad = [
            ("select-pid", val(42i32)),
            ("select-pid", val(42u64)),
            ("select-pid", val("42")),
            ("tab", val(1u32)),
            ("quit", val(true)),
        ];
        for (name, param) in bad {
            assert_eq!(
                parse_action(name, &[param]),
                Err(ActionErr::BadParameter),
                "{name}"
            );
        }
        // A variant nested in a variant is not unwrapped.
        let nested = val(Value::Value(Box::new(Value::from(42u32))));
        assert_eq!(
            parse_action("select-pid", &[nested]),
            Err(ActionErr::BadParameter)
        );
        assert_eq!(
            parse_action("select-pid", &[]),
            Err(ActionErr::BadParameter)
        );
    }

    #[test]
    fn reject_unknown_action() {
        for name in ["", "Quit", "open", "tab ", "select_pid", "\0"] {
            assert_eq!(
                parse_action(name, &[]),
                Err(ActionErr::UnknownAction),
                "{name:?}"
            );
        }
        assert_eq!(parse_action("quit", &[]), Ok(vec![AppRequest::Quit]));
    }

    #[test]
    fn token_length_limit() {
        let data = |v: OwnedValue| HashMap::from([(TOKEN_KEY.to_owned(), v)]);
        let ok = "a".repeat(MAX_TOKEN_BYTES);
        assert_eq!(
            activation_token(&data(val(ok.as_str()))).as_deref(),
            Some(ok.as_str())
        );
        let long = "a".repeat(300);
        assert_eq!(activation_token(&data(val(long.as_str()))), None);
        // 86 three-byte characters are 258 bytes.
        let wide = "\u{20ac}".repeat(86);
        assert_eq!(activation_token(&data(val(wide.as_str()))), None);
        assert_eq!(activation_token(&data(val(7u32))), None);
        assert_eq!(activation_token(&HashMap::new()), None);
    }
}
