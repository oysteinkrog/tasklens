//! Unit actions on the systemd manager that owns the unit (ADR-005, ADR-006, SYS-005, SYS-006,
//! PROC-072). Part 1: Start, Stop, Restart, Reload and Reset failed. Part 2: Freeze, Thaw and
//! Kill for units outside `app.slice` (R-092). Part 3: Enable, Disable, Mask and Unmask, each
//! followed by `Manager.Reload()` as systemctl does (ADR-006, SYS-007).
//!
//! Every call goes over the connection the `SystemdSystem` or `SystemdUser` worker holds, with
//! `ALLOW_INTERACTIVE_AUTHORIZATION` so polkit can prompt for system units. tasklens never
//! escalates by itself. A job call adds the `JobRemoved` match before the method call, so a fast
//! job cannot be missed, then waits for the returned job path with no client timeout. The method
//! call itself has one.
//!
//! Log the unit name at debug only, and never log method arguments beyond it.

use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use asupersync::Cx;
use asupersync::time;
use futures_lite::{Stream, StreamExt};
use tl_model::actions::{ActionError, ActionKind, ActionState, RejectReason};
use tl_model::table::{Manager, UnitRef};
use zbus::message::{Flags, Type};
use zbus::zvariant::{OwnedObjectPath, OwnedStructure, OwnedValue, StructureBuilder, Value};

use super::ActionCtx;
use crate::dbus::{EXECUTOR_TICK, READ_TIMEOUT, error_name};

pub const MANAGER_BUS: &str = "org.freedesktop.systemd1";
pub const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
pub const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";
pub const JOB_MODE: &str = "replace";
/// The method call deadline on the user manager.
pub const CALL_TIMEOUT_USER: Duration = Duration::from_secs(10);
/// The method call deadline on the system manager. It may show a polkit prompt.
pub const CALL_TIMEOUT_SYSTEM: Duration = Duration::from_secs(120);
/// How often a freeze reads `FreezerState`.
pub const FREEZE_POLL: Duration = Duration::from_millis(50);
/// How long a freeze waits for `FreezerState` to read `frozen`.
pub const FREEZE_WAIT: Duration = Duration::from_secs(2);
pub const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";
/// The `KillUnit` signal for Kill App.
const SIGKILL: i32 = 9;

const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
const INTERACTIVE_AUTH_REQUIRED: &str =
    "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired";
const NO_SUCH_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";
const UNIT_MASKED: &str = "org.freedesktop.systemd1.UnitMasked";
const JOB_TYPE_NOT_APPLICABLE: &str = "org.freedesktop.systemd1.JobTypeNotApplicable";
const NOT_CONNECTED: &str = "manager not connected";
const CONNECTION_LOST: &str = "connection lost before the job ended";
const FREEZE_IN_PROGRESS: &str = "freeze still in progress";
const RELOAD_FAILED: &str = "changed, but the manager reload failed";
const UNIT_PATH_PREFIX: &str = "/org/freedesktop/systemd1/unit/";

/// A boxed future of a `UnitBus` call.
pub type BusFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, UnitBusError>> + Send + 'a>>;

/// Picks the bus for a manager in place of `ZbusUnitBus`. Tests set it on `ActionCtx`.
pub type UnitBusOverride = Arc<dyn Fn(Manager) -> Option<Arc<dyn UnitBus>> + Send + Sync>;

/// Why a manager call failed. `Call` carries the D-Bus error name; its message can hold a unit
/// name, so only the name is logged.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum UnitBusError {
    NoConnection,
    Timeout,
    /// The connection or the signal stream ended, or the `Cx` was cancelled.
    Closed,
    Call {
        name: Arc<str>,
        message: Arc<str>,
    },
}

/// The manager calls this module makes, behind a trait so tests fake the bus. `ZbusUnitBus`
/// wraps the connection from the worker's slot.
pub trait UnitBus: Send + Sync {
    /// Calls `method` on the manager object with `args` as the body, and returns the reply body:
    /// its only field, or a structure of its fields. An empty reply gives `Value::Bool(true)`.
    /// Sets `ALLOW_INTERACTIVE_AUTHORIZATION` on every call.
    fn call<'a>(
        &'a self,
        cx: &'a Cx,
        method: &'a str,
        args: &'a [Value<'static>],
        timeout: Duration,
    ) -> BusFuture<'a, OwnedValue>;
    /// Adds the `JobRemoved` match rule and returns the stream. Call it before the method.
    fn job_removed<'a>(&'a self, cx: &'a Cx) -> BusFuture<'a, JobStream>;
    /// `Properties.Get(iface, name)` on `unit_path`, with the 2 s read deadline.
    fn get_property<'a>(
        &'a self,
        cx: &'a Cx,
        unit_path: &'a str,
        iface: &'a str,
        name: &'a str,
    ) -> BusFuture<'a, OwnedValue>;
}

/// One `JobRemoved(u id, o job, s unit, s result)` signal.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct JobRemovedSignal {
    pub id: u32,
    pub job: Arc<str>,
    pub unit: Arc<str>,
    pub result: Arc<str>,
}

/// The `JobRemoved` signals of one manager. Dropping it removes the match.
pub struct JobStream {
    inner: Pin<Box<dyn Stream<Item = JobRemovedSignal> + Send>>,
}

impl JobStream {
    pub fn new(signals: impl Stream<Item = JobRemovedSignal> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(signals),
        }
    }

    /// Waits up to 500 ms for the `JobRemoved` of `job`. `Ok(Some(result))` when it arrives,
    /// `Ok(None)` when the wait ends with no match (callers checkpoint and loop), `Err(Closed)`
    /// when the stream ends or `cx` is cancelled. Signals for other jobs are skipped.
    pub async fn next_result(
        &mut self,
        cx: &Cx,
        job: &str,
    ) -> Result<Option<Arc<str>>, UnitBusError> {
        let start = cx.now();
        loop {
            let waited = Duration::from_nanos(cx.now().duration_since(start));
            if waited >= EXECUTOR_TICK {
                return Ok(None);
            }
            let next = time::timeout(cx.now(), EXECUTOR_TICK - waited, self.inner.next()).await;
            if cx.checkpoint().is_err() {
                return Err(UnitBusError::Closed);
            }
            match next {
                Ok(Some(signal)) if &*signal.job == job => return Ok(Some(signal.result)),
                Ok(Some(_)) => {}
                Ok(None) => return Err(UnitBusError::Closed),
                Err(_) => return Ok(None),
            }
        }
    }
}

/// The production `UnitBus` over a systemd manager connection (E08-009).
pub struct ZbusUnitBus {
    conn: Arc<zbus::Connection>,
}

impl ZbusUnitBus {
    pub fn new(conn: Arc<zbus::Connection>) -> Self {
        Self { conn }
    }

    /// Sends the call, then reads the incoming messages until its reply. The message stream is
    /// opened before the send, so the reply cannot be missed.
    async fn call_raw(&self, method: &str, args: &[Value<'static>]) -> zbus::Result<OwnedValue> {
        let mut body = StructureBuilder::new();
        for arg in args {
            body = body.append_field(arg.try_clone()?);
        }
        let body = body.build()?;
        let msg = zbus::Message::method_call(MANAGER_PATH, method)?
            .destination(MANAGER_BUS)?
            .interface(MANAGER_IFACE)?
            .with_flags(Flags::AllowInteractiveAuth)?
            .build(&body)?;
        let serial = msg.primary_header().serial_num();
        let mut incoming = zbus::MessageStream::from(&*self.conn);
        self.conn.send(&msg).await?;
        let reply = reply_to(&mut incoming, serial).await?;
        if reply.message_type() == Type::Error {
            return Err(zbus::Error::from(reply));
        }
        reply_value(&reply)
    }

    async fn get_raw(&self, unit_path: &str, iface: &str, name: &str) -> zbus::Result<OwnedValue> {
        let reply = self
            .conn
            .call_method(
                Some(MANAGER_BUS),
                unit_path,
                Some(PROPERTIES_IFACE),
                "Get",
                &(iface, name),
            )
            .await?;
        reply.body().deserialize::<OwnedValue>()
    }
}

/// The first message on `incoming` that answers `serial`.
async fn reply_to(
    incoming: &mut zbus::MessageStream,
    serial: NonZeroU32,
) -> zbus::Result<zbus::Message> {
    while let Some(msg) = incoming.next().await {
        let msg = msg?;
        let answers = matches!(msg.message_type(), Type::MethodReturn | Type::Error)
            && msg.header().reply_serial() == Some(serial);
        if answers {
            return Ok(msg);
        }
    }
    Err(zbus::Error::InputOutput(Arc::new(std::io::Error::from(
        std::io::ErrorKind::BrokenPipe,
    ))))
}

/// The reply body as one value. See `UnitBus::call`.
fn reply_value(reply: &zbus::Message) -> zbus::Result<OwnedValue> {
    let body = reply.body();
    if body.is_empty() {
        return Ok(OwnedValue::from(true));
    }
    let OwnedStructure(fields) = body.deserialize::<OwnedStructure>()?;
    let mut fields = fields.into_fields();
    let value = if fields.len() == 1 {
        fields.remove(0)
    } else {
        let mut all = StructureBuilder::new();
        for field in fields {
            all = all.append_field(field);
        }
        Value::Structure(all.build()?)
    };
    Ok(value.try_into_owned()?)
}

/// `time::timeout` then a checkpoint, with the errors as `UnitBusError`.
async fn timed<T>(
    cx: &Cx,
    d: Duration,
    fut: impl Future<Output = zbus::Result<T>>,
) -> Result<T, UnitBusError> {
    let result = time::timeout(cx.now(), d, fut).await;
    if cx.checkpoint().is_err() {
        return Err(UnitBusError::Closed);
    }
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(bus_error(&e)),
        Err(_) => Err(UnitBusError::Timeout),
    }
}

fn bus_error(e: &zbus::Error) -> UnitBusError {
    match e {
        zbus::Error::InputOutput(_) => UnitBusError::Closed,
        zbus::Error::MethodError(name, detail, _) => UnitBusError::Call {
            name: name.as_str().into(),
            message: detail.as_deref().unwrap_or_default().into(),
        },
        other => UnitBusError::Call {
            name: error_name(other).unwrap_or("zbus").into(),
            message: other.to_string().into(),
        },
    }
}

impl UnitBus for ZbusUnitBus {
    fn call<'a>(
        &'a self,
        cx: &'a Cx,
        method: &'a str,
        args: &'a [Value<'static>],
        timeout: Duration,
    ) -> BusFuture<'a, OwnedValue> {
        Box::pin(timed(cx, timeout, self.call_raw(method, args)))
    }

    fn job_removed<'a>(&'a self, cx: &'a Cx) -> BusFuture<'a, JobStream> {
        Box::pin(async move {
            // The sender and path stop another peer on the bus from faking a job result.
            let rule = zbus::MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(MANAGER_BUS)
                .and_then(|b| b.path(MANAGER_PATH))
                .and_then(|b| b.interface(MANAGER_IFACE))
                .and_then(|b| b.member("JobRemoved"))
                .map_err(|e| bus_error(&e))?
                .build();
            let subscribe = zbus::MessageStream::for_match_rule(rule, &self.conn, None);
            let stream = timed(cx, READ_TIMEOUT, subscribe).await?;
            let signals = stream.filter_map(|msg| {
                let msg = msg.ok()?;
                let (id, job, unit, result) = msg
                    .body()
                    .deserialize::<(u32, OwnedObjectPath, String, String)>()
                    .ok()?;
                Some(JobRemovedSignal {
                    id,
                    job: job.as_str().into(),
                    unit: unit.into(),
                    result: result.into(),
                })
            });
            Ok(JobStream::new(signals))
        })
    }

    fn get_property<'a>(
        &'a self,
        cx: &'a Cx,
        unit_path: &'a str,
        iface: &'a str,
        name: &'a str,
    ) -> BusFuture<'a, OwnedValue> {
        Box::pin(timed(
            cx,
            READ_TIMEOUT,
            self.get_raw(unit_path, iface, name),
        ))
    }
}

/// The bus of `manager`: `ActionCtx::unit_bus_override` when set, else a `ZbusUnitBus` over
/// that manager's slot. `None` when the manager is not connected.
pub fn bus_for(actx: &ActionCtx, manager: Manager) -> Option<Arc<dyn UnitBus>> {
    if let Some(pick) = &actx.unit_bus_override {
        return pick(manager);
    }
    let slot = match manager {
        Manager::System => &actx.systemd_system,
        Manager::User => &actx.systemd_user,
    };
    slot.get()
        .map(|conn| Arc::new(ZbusUnitBus::new(conn)) as Arc<dyn UnitBus>)
}

/// Runs one unit target: the five job kinds, Freeze, Thaw and Kill, and the four unit file
/// kinds.
pub async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    unit: &UnitRef,
) -> ActionState {
    if matches!(
        kind,
        ActionKind::FreezeGroup | ActionKind::ThawGroup | ActionKind::KillGroup
    ) {
        let Some(bus) = bus_for(actx, unit.manager) else {
            return not_connected();
        };
        return freeze_thaw_kill(cx, &*bus, kind, unit).await;
    }
    if matches!(
        kind,
        ActionKind::UnitEnable
            | ActionKind::UnitDisable
            | ActionKind::UnitMask
            | ActionKind::UnitUnmask
    ) {
        let Some(bus) = bus_for(actx, unit.manager) else {
            return not_connected();
        };
        return set_unit_file_state(cx, &*bus, unit, kind).await;
    }
    let method = match kind {
        ActionKind::UnitStart => "StartUnit",
        ActionKind::UnitStop => "StopUnit",
        ActionKind::UnitRestart => "RestartUnit",
        ActionKind::UnitReload => "ReloadUnit",
        ActionKind::UnitResetFailed => "ResetFailedUnit",
        _ => return ActionState::Rejected(RejectReason::Unsupported),
    };
    let Some(bus) = bus_for(actx, unit.manager) else {
        return not_connected();
    };
    tracing::debug!(target: "tasklens::action", unit = &*unit.name, method, "unit call");
    if method == "ResetFailedUnit" {
        let args = [Value::from(unit.name.to_string())];
        return match bus
            .call(cx, method, &args, call_timeout(unit.manager))
            .await
        {
            Ok(_) => ActionState::Succeeded,
            Err(e) => map_bus_error(&e, method),
        };
    }
    job_call(cx, &*bus, method, unit, unit.manager).await
}

fn call_timeout(manager: Manager) -> Duration {
    match manager {
        Manager::System => CALL_TIMEOUT_SYSTEM,
        Manager::User => CALL_TIMEOUT_USER,
    }
}

/// `FreezeUnit`, `ThawUnit` or `KillUnit(name, "all", SIGKILL)` on the owning manager. A freeze
/// then reads `FreezerState` every 50 ms, at most 40 times, until it reads `frozen`.
async fn freeze_thaw_kill(
    cx: &Cx,
    bus: &dyn UnitBus,
    kind: &ActionKind,
    unit: &UnitRef,
) -> ActionState {
    let name = Value::from(unit.name.to_string());
    let (method, args) = match kind {
        ActionKind::FreezeGroup => ("FreezeUnit", vec![name]),
        ActionKind::ThawGroup => ("ThawUnit", vec![name]),
        ActionKind::KillGroup => (
            "KillUnit",
            vec![name, Value::from("all".to_owned()), Value::from(SIGKILL)],
        ),
        _ => return ActionState::Rejected(RejectReason::Unsupported),
    };
    tracing::debug!(target: "tasklens::action", unit = &*unit.name, method, "unit call");
    if let Err(e) = bus
        .call(cx, method, &args, call_timeout(unit.manager))
        .await
    {
        return map_bus_error(&e, method);
    }
    if method != "FreezeUnit" {
        return ActionState::Succeeded;
    }
    let path = unit_object_path(&unit.name);
    let start = cx.now();
    loop {
        match bus
            .get_property(cx, &path, UNIT_IFACE, "FreezerState")
            .await
        {
            Ok(state) if matches!(&*state, Value::Str(s) if s.as_str() == "frozen") => {
                return ActionState::Succeeded;
            }
            // "running", "freezing" or any other word: read again.
            Ok(_) => {}
            Err(e) => return map_bus_error(&e, "FreezerState"),
        }
        let waited = Duration::from_nanos(cx.now().duration_since(start));
        if waited + FREEZE_POLL >= FREEZE_WAIT {
            return ActionState::OutcomeUnknown(Arc::from(FREEZE_IN_PROGRESS));
        }
        time::sleep(cx.now(), FREEZE_POLL).await;
        if cx.checkpoint().is_err() {
            return ActionState::OutcomeUnknown(Arc::from(FREEZE_IN_PROGRESS));
        }
    }
}

/// Enable, Disable, Mask or Unmask `unit`, then `reload`. Enable and Disable on a masked unit
/// unmask it first, inside the same action (R-170 item 10). A failed reload after a successful
/// change is `OutcomeUnknown`: the unit file changed, but the manager may not have seen it.
pub async fn set_unit_file_state(
    cx: &Cx,
    bus: &dyn UnitBus,
    unit: &UnitRef,
    kind: &ActionKind,
) -> ActionState {
    let names = || Value::from(vec![unit.name.to_string()]);
    let timeout = call_timeout(unit.manager);
    if matches!(kind, ActionKind::UnitEnable | ActionKind::UnitDisable) {
        let state = match unit_file_state(cx, bus, &unit.name).await {
            Ok(state) => state,
            Err(e) => return map_bus_error(&e, "GetUnitFileState"),
        };
        if matches!(&*state, "masked" | "masked-runtime") {
            tracing::debug!(target: "tasklens::action", unit = &*unit.name, "unmask first");
            let args = [names(), Value::from(false)];
            if let Err(e) = bus.call(cx, "UnmaskUnitFiles", &args, timeout).await {
                return map_bus_error(&e, "UnmaskUnitFiles");
            }
        }
    }
    let (method, args) = match kind {
        ActionKind::UnitEnable => (
            "EnableUnitFiles",
            vec![names(), Value::from(false), Value::from(true)],
        ),
        ActionKind::UnitDisable => ("DisableUnitFiles", vec![names(), Value::from(false)]),
        ActionKind::UnitMask => (
            "MaskUnitFiles",
            vec![names(), Value::from(false), Value::from(true)],
        ),
        ActionKind::UnitUnmask => ("UnmaskUnitFiles", vec![names(), Value::from(false)]),
        _ => return ActionState::Rejected(RejectReason::Unsupported),
    };
    tracing::debug!(target: "tasklens::action", unit = &*unit.name, method, "unit call");
    let reply = match bus.call(cx, method, &args, timeout).await {
        Ok(reply) => reply,
        Err(e) => return map_bus_error(&e, method),
    };
    // `EnableUnitFiles` replies `(b carries_install_info, a(sss) changes)`. A unit with no
    // `[Install]` section still succeeds, as in systemctl.
    if let Value::Structure(reply) = &*reply
        && let Some(Value::Bool(false)) = reply.fields().first()
    {
        tracing::debug!(target: "tasklens::action", unit = &*unit.name, "no install info");
    }
    match reload(cx, bus, unit.manager).await {
        Ok(()) => ActionState::Succeeded,
        Err(e) => {
            // Only the error name: the message can hold a unit name.
            let error = match &e {
                UnitBusError::Call { name, .. } => &**name,
                UnitBusError::Timeout => "timed out",
                UnitBusError::Closed => "closed",
                UnitBusError::NoConnection => "no connection",
            };
            tracing::debug!(target: "tasklens::action", error, "manager reload failed");
            ActionState::OutcomeUnknown(Arc::from(RELOAD_FAILED))
        }
    }
}

/// `Manager.Reload()`, with the call deadline of `manager`.
pub async fn reload(cx: &Cx, bus: &dyn UnitBus, manager: Manager) -> Result<(), UnitBusError> {
    bus.call(cx, "Reload", &[], call_timeout(manager))
        .await
        .map(drop)
}

/// `Manager.GetUnitFileState(name)`: `enabled`, `disabled`, `masked`, `static` and the like.
async fn unit_file_state(cx: &Cx, bus: &dyn UnitBus, name: &str) -> Result<Arc<str>, UnitBusError> {
    let args = [Value::from(name.to_owned())];
    // A read needs no polkit prompt on either manager, so the shorter deadline serves both.
    let reply = bus
        .call(cx, "GetUnitFileState", &args, CALL_TIMEOUT_USER)
        .await?;
    match &*reply {
        Value::Str(state) => Ok(state.as_str().into()),
        _ => Err(UnitBusError::Call {
            name: "zbus".into(),
            message: "bad GetUnitFileState reply".into(),
        }),
    }
}

/// The manager's object path for unit `name`, escaped the way systemd's `bus_label_escape` does:
/// each byte outside `[A-Za-z0-9]`, and a leading digit, becomes `_` and two lowercase hex digits.
pub fn unit_object_path(name: &str) -> String {
    let mut path = String::from(UNIT_PATH_PREFIX);
    if name.is_empty() {
        path.push('_');
        return path;
    }
    for (i, b) in name.bytes().enumerate() {
        if b.is_ascii_alphabetic() || (b.is_ascii_digit() && i > 0) {
            path.push(char::from(b));
        } else {
            path.push_str(&format!("_{b:02x}"));
        }
    }
    path
}

/// `method(name, "replace")` with the `JobRemoved` match added first, then waits for the job.
/// The wait has no client timeout (ADR-005, SYS-005); a cancel ends it within 500 ms.
async fn job_call(
    cx: &Cx,
    bus: &dyn UnitBus,
    method: &str,
    unit: &UnitRef,
    manager: Manager,
) -> ActionState {
    let mut jobs = match bus.job_removed(cx).await {
        Ok(jobs) => jobs,
        Err(e) => return map_bus_error(&e, "JobRemoved match"),
    };
    let args = [
        Value::from(unit.name.to_string()),
        Value::from(JOB_MODE.to_owned()),
    ];
    let reply = match bus.call(cx, method, &args, call_timeout(manager)).await {
        Ok(reply) => reply,
        Err(e) => return map_bus_error(&e, method),
    };
    let job: Arc<str> = match &*reply {
        Value::ObjectPath(path) => path.as_str().into(),
        _ => {
            return ActionState::Failed(ActionError::Internal(Arc::from("bad job reply")));
        }
    };
    loop {
        match jobs.next_result(cx, &job).await {
            Ok(Some(result)) if &*result == "done" => return ActionState::Succeeded,
            // "failed", "timeout", "canceled", "dependency", "skipped", "invalid" or any other
            // word (R-170 item 9).
            Ok(Some(result)) => {
                return ActionState::Failed(ActionError::UnitJob {
                    result,
                    error_name: None,
                });
            }
            Ok(None) => {
                if cx.checkpoint().is_err() {
                    return connection_lost();
                }
            }
            Err(_) => return connection_lost(),
        }
    }
}

/// The action state for a failed manager call. `context` names the call in the debug log.
///
/// A dismissed polkit prompt and a denial both arrive as `AccessDenied` on this path (R-117,
/// U-39).
pub fn map_bus_error(e: &UnitBusError, context: &str) -> ActionState {
    let name = match e {
        UnitBusError::NoConnection => return not_connected(),
        UnitBusError::Timeout => {
            tracing::debug!(target: "tasklens::action", context, "unit call timed out");
            return ActionState::OutcomeUnknown(Arc::from("timed out"));
        }
        UnitBusError::Closed => return connection_lost(),
        UnitBusError::Call { name, .. } => name,
    };
    tracing::debug!(target: "tasklens::action", context, error = &**name, "unit call failed");
    match &**name {
        ACCESS_DENIED => ActionState::Failed(ActionError::NotAuthorized),
        INTERACTIVE_AUTH_REQUIRED => ActionState::Failed(ActionError::Internal(Arc::from(
            "no authentication agent is running",
        ))),
        NO_SUCH_UNIT | JOB_TYPE_NOT_APPLICABLE => ActionState::Rejected(RejectReason::Unsupported),
        UNIT_MASKED => ActionState::Failed(ActionError::UnitJob {
            result: Arc::from("masked"),
            error_name: Some(Arc::from(UNIT_MASKED)),
        }),
        _ => ActionState::Failed(ActionError::UnitJob {
            result: Arc::from("failed"),
            error_name: Some(Arc::clone(name)),
        }),
    }
}

fn not_connected() -> ActionState {
    ActionState::Failed(ActionError::Internal(Arc::from(NOT_CONNECTED)))
}

fn connection_lost() -> ActionState {
    ActionState::OutcomeUnknown(Arc::from(CONNECTION_LOST))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    use asupersync::runtime::TaskHandle;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::time::BootId;
    use zbus::object_server::SignalEmitter;
    use zbus::zvariant::ObjectPath;

    use super::*;
    use crate::actions::ActionOps;
    use crate::config::{EngineConfig, EngineMode};
    use crate::dbus::ManagerConnSlot;

    const JOB: &str = "/org/freedesktop/systemd1/job/7";
    const OTHER_JOB: &str = "/org/freedesktop/systemd1/job/6";

    struct NoOps;
    impl ActionOps for NoOps {}

    #[derive(Clone, PartialEq, Eq, Debug)]
    enum FakeCall {
        JobRemoved,
        Call {
            method: String,
            args: Vec<String>,
            timeout: Duration,
        },
        GetProperty {
            path: String,
            iface: String,
            name: String,
        },
    }

    /// What the fake stream does after its queued signals.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Tail {
        Pending,
        Close,
    }

    struct FakeUnitBus {
        calls: Mutex<Vec<FakeCall>>,
        replies: Mutex<VecDeque<Result<OwnedValue, UnitBusError>>>,
        signals: Vec<JobRemovedSignal>,
        tail: Tail,
        /// `FreezerState` readings in order; the last one repeats.
        freezer: Mutex<VecDeque<&'static str>>,
    }

    impl FakeUnitBus {
        fn new(
            replies: Vec<Result<OwnedValue, UnitBusError>>,
            signals: Vec<(&str, &str)>,
            tail: Tail,
        ) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::default(),
                replies: Mutex::new(replies.into()),
                signals: signals
                    .into_iter()
                    .enumerate()
                    .map(|(i, (job, result))| JobRemovedSignal {
                        id: u32::try_from(i).unwrap(),
                        job: job.into(),
                        unit: "app.service".into(),
                        result: result.into(),
                    })
                    .collect(),
                tail,
                freezer: Mutex::default(),
            })
        }

        /// A bus that answers one call with `reply`, then reads `states` as `FreezerState`.
        fn freezer(
            reply: Result<OwnedValue, UnitBusError>,
            states: Vec<&'static str>,
        ) -> Arc<Self> {
            let bus = Self::new(vec![reply], vec![], Tail::Pending);
            *bus.freezer.lock().unwrap() = states.into();
            bus
        }

        fn calls(&self) -> Vec<FakeCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn job_reply() -> Result<OwnedValue, UnitBusError> {
        Ok(Value::from(ObjectPath::try_from(JOB).unwrap())
            .try_into_owned()
            .unwrap())
    }

    fn show(v: &Value<'_>) -> String {
        match v {
            Value::Str(s) => s.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Array(a) => format!("[{}]", a.iter().map(show).collect::<Vec<_>>().join(", ")),
            other => format!("{other:?}"),
        }
    }

    impl UnitBus for FakeUnitBus {
        fn call<'a>(
            &'a self,
            _cx: &'a Cx,
            method: &'a str,
            args: &'a [Value<'static>],
            timeout: Duration,
        ) -> BusFuture<'a, OwnedValue> {
            self.calls.lock().unwrap().push(FakeCall::Call {
                method: method.into(),
                args: args.iter().map(show).collect(),
                timeout,
            });
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("a scripted reply");
            Box::pin(async move { reply })
        }

        fn job_removed<'a>(&'a self, _cx: &'a Cx) -> BusFuture<'a, JobStream> {
            self.calls.lock().unwrap().push(FakeCall::JobRemoved);
            let queued = futures_lite::stream::iter(self.signals.clone());
            let stream = match self.tail {
                Tail::Close => JobStream::new(queued),
                Tail::Pending => JobStream::new(queued.chain(futures_lite::stream::pending())),
            };
            Box::pin(async move { Ok(stream) })
        }

        fn get_property<'a>(
            &'a self,
            _cx: &'a Cx,
            unit_path: &'a str,
            iface: &'a str,
            name: &'a str,
        ) -> BusFuture<'a, OwnedValue> {
            self.calls.lock().unwrap().push(FakeCall::GetProperty {
                path: unit_path.into(),
                iface: iface.into(),
                name: name.into(),
            });
            let mut states = self.freezer.lock().unwrap();
            let state = if states.len() > 1 {
                states.pop_front()
            } else {
                states.front().copied()
            };
            let reply = match state {
                Some(state) => Ok(OwnedValue::try_from(Value::from(state)).unwrap()),
                None => Err(UnitBusError::NoConnection),
            };
            Box::pin(async move { reply })
        }
    }

    fn actx(bus: Option<Arc<FakeUnitBus>>) -> Arc<ActionCtx> {
        let (events, _events_rx) = async_channel::bounded(16);
        let (msgs, _msgs_rx) = asupersync::channel::mpsc::channel(16);
        let pick: UnitBusOverride =
            Arc::new(move |_| bus.clone().map(|bus| bus as Arc<dyn UnitBus>));
        Arc::new(ActionCtx {
            events,
            msgs,
            ops: Arc::new(NoOps),
            boot_id: BootId(1),
            uid: 1000,
            config: Arc::new(EngineConfig::new(
                EngineMode::Full,
                PathBuf::from("/nonexistent"),
            )),
            systemd_system: ManagerConnSlot::new(),
            systemd_user: ManagerConnSlot::new(),
            openers: None,
            helper: None,
            destroy_support_sent: AtomicBool::new(false),
            unit_bus_override: Some(pick),
        })
    }

    fn unit(manager: Manager) -> UnitRef {
        UnitRef {
            manager,
            name: "app.service".into(),
        }
    }

    type Out = Arc<Mutex<Option<ActionState>>>;

    /// Starts `run_target` on a lab runtime and runs it until idle.
    fn start(
        actx: Arc<ActionCtx>,
        kind: ActionKind,
        unit: UnitRef,
    ) -> (LabRuntime, Out, TaskHandle<()>) {
        let out: Out = Arc::default();
        let slot = Arc::clone(&out);
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let state = run_target(&cx, &actx, &kind, &unit).await;
                *slot.lock().unwrap() = Some(state);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        (lab, out, handle)
    }

    fn run(actx: Arc<ActionCtx>, kind: ActionKind, unit: UnitRef) -> ActionState {
        let (mut lab, out, _handle) = start(actx, kind, unit);
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
        out.lock().unwrap().take().expect("run_target finished")
    }

    fn advance(lab: &mut LabRuntime, d: Duration) {
        lab.advance_time(u64::try_from(d.as_nanos()).expect("fits in u64"));
        // `run_until_idle` does not fire due timers when no task is ready.
        lab.step_for_test();
        lab.run_until_idle();
    }

    fn start_call(method: &str, timeout: Duration) -> FakeCall {
        FakeCall::Call {
            method: method.into(),
            args: vec!["app.service".into(), "replace".into()],
            timeout,
        }
    }

    #[test]
    fn unit_start_match_before_call() {
        let bus = FakeUnitBus::new(vec![job_reply()], vec![(JOB, "done")], Tail::Pending);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::UnitStart,
            unit(Manager::User),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            bus.calls(),
            [
                FakeCall::JobRemoved,
                start_call("StartUnit", CALL_TIMEOUT_USER)
            ]
        );
    }

    #[test]
    fn unit_job_kinds_call_their_method() {
        for (kind, method) in [
            (ActionKind::UnitStop, "StopUnit"),
            (ActionKind::UnitRestart, "RestartUnit"),
            (ActionKind::UnitReload, "ReloadUnit"),
        ] {
            let bus = FakeUnitBus::new(vec![job_reply()], vec![(JOB, "done")], Tail::Pending);
            let state = run(actx(Some(Arc::clone(&bus))), kind, unit(Manager::User));
            assert_eq!(state, ActionState::Succeeded);
            assert_eq!(
                bus.calls(),
                [FakeCall::JobRemoved, start_call(method, CALL_TIMEOUT_USER)]
            );
        }
    }

    #[test]
    fn unit_job_failed_word() {
        let bus = FakeUnitBus::new(
            vec![job_reply()],
            vec![(OTHER_JOB, "done"), (JOB, "failed")],
            Tail::Pending,
        );
        let state = run(actx(Some(bus)), ActionKind::UnitStart, unit(Manager::User));
        assert_eq!(
            state,
            ActionState::Failed(ActionError::UnitJob {
                result: "failed".into(),
                error_name: None
            })
        );
        // Any other word is passed through.
        let bus = FakeUnitBus::new(vec![job_reply()], vec![(JOB, "dependency")], Tail::Pending);
        let state = run(
            actx(Some(bus)),
            ActionKind::UnitRestart,
            unit(Manager::User),
        );
        assert_eq!(
            state,
            ActionState::Failed(ActionError::UnitJob {
                result: "dependency".into(),
                error_name: None
            })
        );
    }

    /// The fake queues the signal when the match is added, before the call replies.
    #[test]
    fn unit_job_removed_before_reply() {
        let bus = FakeUnitBus::new(vec![job_reply()], vec![(JOB, "done")], Tail::Close);
        let state = run(
            actx(Some(bus)),
            ActionKind::UnitStart,
            unit(Manager::System),
        );
        assert_eq!(state, ActionState::Succeeded);
    }

    #[test]
    fn unit_bus_closed_is_outcome_unknown() {
        let bus = FakeUnitBus::new(vec![job_reply()], vec![(OTHER_JOB, "done")], Tail::Close);
        let state = run(actx(Some(bus)), ActionKind::UnitStart, unit(Manager::User));
        assert_eq!(
            state,
            ActionState::OutcomeUnknown("connection lost before the job ended".into())
        );
    }

    #[test]
    fn unit_reset_failed_no_job() {
        let bus = FakeUnitBus::new(vec![Ok(OwnedValue::from(true))], vec![], Tail::Pending);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::UnitResetFailed,
            unit(Manager::System),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            bus.calls(),
            [FakeCall::Call {
                method: "ResetFailedUnit".into(),
                args: vec!["app.service".into()],
                timeout: CALL_TIMEOUT_SYSTEM,
            }]
        );
    }

    fn call_error(name: &str) -> UnitBusError {
        UnitBusError::Call {
            name: name.into(),
            message: "detail".into(),
        }
    }

    #[test]
    fn unit_error_name_mapping() {
        let cases = [
            (
                call_error(ACCESS_DENIED),
                ActionState::Failed(ActionError::NotAuthorized),
            ),
            (
                call_error(INTERACTIVE_AUTH_REQUIRED),
                ActionState::Failed(ActionError::Internal(
                    "no authentication agent is running".into(),
                )),
            ),
            (
                call_error(NO_SUCH_UNIT),
                ActionState::Rejected(RejectReason::Unsupported),
            ),
            (
                call_error(JOB_TYPE_NOT_APPLICABLE),
                ActionState::Rejected(RejectReason::Unsupported),
            ),
            (
                call_error(UNIT_MASKED),
                ActionState::Failed(ActionError::UnitJob {
                    result: "masked".into(),
                    error_name: Some(UNIT_MASKED.into()),
                }),
            ),
            (
                call_error("org.example.Odd"),
                ActionState::Failed(ActionError::UnitJob {
                    result: "failed".into(),
                    error_name: Some("org.example.Odd".into()),
                }),
            ),
            (
                UnitBusError::Timeout,
                ActionState::OutcomeUnknown("timed out".into()),
            ),
            (
                UnitBusError::NoConnection,
                ActionState::Failed(ActionError::Internal("manager not connected".into())),
            ),
        ];
        for (err, want) in cases {
            assert_eq!(map_bus_error(&err, "test"), want, "{err:?}");
            let bus = FakeUnitBus::new(vec![Err(err.clone())], vec![], Tail::Pending);
            let state = run(actx(Some(bus)), ActionKind::UnitStart, unit(Manager::User));
            assert_eq!(state, want, "{err:?}");
        }
    }

    #[test]
    fn unit_no_connection() {
        let state = run(actx(None), ActionKind::UnitStart, unit(Manager::System));
        assert_eq!(
            state,
            ActionState::Failed(ActionError::Internal("manager not connected".into()))
        );
        // Without an override, empty slots give no bus.
        let mut bare = Arc::try_unwrap(actx(None)).ok().expect("one owner");
        bare.unit_bus_override = None;
        assert!(bus_for(&bare, Manager::System).is_none());
        assert!(bus_for(&bare, Manager::User).is_none());
    }

    #[test]
    fn unit_call_timeout_by_manager() {
        for (manager, timeout) in [
            (Manager::System, CALL_TIMEOUT_SYSTEM),
            (Manager::User, CALL_TIMEOUT_USER),
        ] {
            let bus = FakeUnitBus::new(vec![job_reply()], vec![(JOB, "done")], Tail::Pending);
            run(
                actx(Some(Arc::clone(&bus))),
                ActionKind::UnitStart,
                unit(manager),
            );
            assert_eq!(bus.calls()[1], start_call("StartUnit", timeout));
        }
        assert_eq!(CALL_TIMEOUT_SYSTEM, Duration::from_secs(120));
        assert_eq!(CALL_TIMEOUT_USER, Duration::from_secs(10));
    }

    #[test]
    fn unit_job_wait_has_no_client_timeout() {
        let bus = FakeUnitBus::new(vec![job_reply()], vec![], Tail::Pending);
        let (mut lab, out, handle) = start(
            actx(Some(bus)),
            ActionKind::UnitStart,
            unit(Manager::System),
        );
        for _ in 0..300 {
            advance(&mut lab, Duration::from_secs(1));
        }
        assert!(out.lock().unwrap().is_none(), "still waiting after 300 s");

        handle.abort();
        lab.run_until_idle();
        advance(&mut lab, EXECUTOR_TICK);
        assert!(handle.is_finished(), "a cancel ends the wait within 500 ms");
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn unit_other_kind_unsupported() {
        let bus = FakeUnitBus::new(vec![], vec![], Tail::Pending);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::KillTree,
            unit(Manager::User),
        );
        assert_eq!(state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(bus.calls().is_empty());
    }

    fn ok() -> Result<OwnedValue, UnitBusError> {
        Ok(OwnedValue::from(true))
    }

    fn name_call(method: &str, timeout: Duration) -> FakeCall {
        FakeCall::Call {
            method: method.into(),
            args: vec!["app.service".into()],
            timeout,
        }
    }

    fn freezer_read() -> FakeCall {
        FakeCall::GetProperty {
            path: "/org/freedesktop/systemd1/unit/app_2eservice".into(),
            iface: UNIT_IFACE.into(),
            name: "FreezerState".into(),
        }
    }

    /// Runs `run_target` and advances virtual time 10 ms at a time until it ends. Returns the
    /// state and the virtual time it took.
    fn run_timed(actx: Arc<ActionCtx>, kind: ActionKind, unit: UnitRef) -> (ActionState, Duration) {
        let (mut lab, out, _handle) = start(actx, kind, unit);
        let step = Duration::from_millis(10);
        let mut elapsed = Duration::ZERO;
        while out.lock().unwrap().is_none() {
            assert!(elapsed < Duration::from_secs(10), "run_target never ended");
            advance(&mut lab, step);
            elapsed += step;
        }
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
        let state = out.lock().unwrap().take().expect("run_target finished");
        (state, elapsed)
    }

    #[test]
    fn unit_freeze_polls_frozen() {
        let bus = FakeUnitBus::freezer(ok(), vec!["freezing", "freezing", "frozen"]);
        let (state, elapsed) = run_timed(
            actx(Some(Arc::clone(&bus))),
            ActionKind::FreezeGroup,
            unit(Manager::User),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            bus.calls(),
            [
                name_call("FreezeUnit", CALL_TIMEOUT_USER),
                freezer_read(),
                freezer_read(),
                freezer_read(),
            ]
        );
        assert!(
            (Duration::from_millis(100)..=Duration::from_millis(110)).contains(&elapsed),
            "{elapsed:?}"
        );
    }

    #[test]
    fn unit_freeze_not_frozen_unknown() {
        let bus = FakeUnitBus::freezer(ok(), vec!["freezing"]);
        let (state, elapsed) = run_timed(
            actx(Some(Arc::clone(&bus))),
            ActionKind::FreezeGroup,
            unit(Manager::System),
        );
        assert_eq!(
            state,
            ActionState::OutcomeUnknown("freeze still in progress".into())
        );
        let reads = bus
            .calls()
            .iter()
            .filter(|c| matches!(c, FakeCall::GetProperty { .. }))
            .count();
        assert_eq!(reads, 40);
        assert!(elapsed <= FREEZE_WAIT, "{elapsed:?}");
    }

    #[test]
    fn unit_thaw_no_property_read() {
        let bus = FakeUnitBus::freezer(ok(), vec!["frozen"]);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::ThawGroup,
            unit(Manager::User),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(bus.calls(), [name_call("ThawUnit", CALL_TIMEOUT_USER)]);
    }

    #[test]
    fn unit_kill_all_sigkill() {
        let bus = FakeUnitBus::freezer(ok(), vec![]);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::KillGroup,
            unit(Manager::System),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            bus.calls(),
            [FakeCall::Call {
                method: "KillUnit".into(),
                args: vec!["app.service".into(), "all".into(), "I32(9)".into()],
                timeout: CALL_TIMEOUT_SYSTEM,
            }]
        );
    }

    /// systemd's `AccessDenied` maps to `NotAuthorized`, as on every manager call (ADR-015,
    /// R-117).
    #[test]
    fn unit_freeze_access_denied() {
        let bus = FakeUnitBus::freezer(Err(call_error(ACCESS_DENIED)), vec!["frozen"]);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::FreezeGroup,
            unit(Manager::System),
        );
        assert_eq!(state, ActionState::Failed(ActionError::NotAuthorized));
        assert_eq!(bus.calls(), [name_call("FreezeUnit", CALL_TIMEOUT_SYSTEM)]);
    }

    #[test]
    fn unit_wrong_kind_unsupported() {
        let bus = FakeUnitBus::freezer(ok(), vec![]);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::Signal(tl_model::actions::Signal::Term),
            unit(Manager::User),
        );
        assert_eq!(state, ActionState::Rejected(RejectReason::Unsupported));
        assert!(bus.calls().is_empty());
    }

    fn files_call(method: &str, args: &[&str], timeout: Duration) -> FakeCall {
        FakeCall::Call {
            method: method.into(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            timeout,
        }
    }

    fn state_read() -> FakeCall {
        files_call("GetUnitFileState", &["app.service"], CALL_TIMEOUT_USER)
    }

    fn reload_call(timeout: Duration) -> FakeCall {
        files_call("Reload", &[], timeout)
    }

    fn word(s: &str) -> Result<OwnedValue, UnitBusError> {
        Ok(OwnedValue::try_from(Value::from(s)).unwrap())
    }

    #[test]
    fn unit_enable_then_reload() {
        let bus = FakeUnitBus::new(vec![word("disabled"), ok(), ok()], vec![], Tail::Pending);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::UnitEnable,
            unit(Manager::System),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            bus.calls(),
            [
                state_read(),
                files_call(
                    "EnableUnitFiles",
                    &["[app.service]", "false", "true"],
                    CALL_TIMEOUT_SYSTEM
                ),
                reload_call(CALL_TIMEOUT_SYSTEM),
            ]
        );
    }

    #[test]
    fn unit_enable_unmasks_first() {
        for masked in ["masked", "masked-runtime"] {
            let bus = FakeUnitBus::new(vec![word(masked), ok(), ok(), ok()], vec![], Tail::Pending);
            let state = run(
                actx(Some(Arc::clone(&bus))),
                ActionKind::UnitEnable,
                unit(Manager::User),
            );
            assert_eq!(state, ActionState::Succeeded);
            assert_eq!(
                bus.calls(),
                [
                    state_read(),
                    files_call(
                        "UnmaskUnitFiles",
                        &["[app.service]", "false"],
                        CALL_TIMEOUT_USER
                    ),
                    files_call(
                        "EnableUnitFiles",
                        &["[app.service]", "false", "true"],
                        CALL_TIMEOUT_USER
                    ),
                    reload_call(CALL_TIMEOUT_USER),
                ]
            );
        }
        // Disable unmasks first too.
        let bus = FakeUnitBus::new(
            vec![word("masked"), ok(), ok(), ok()],
            vec![],
            Tail::Pending,
        );
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::UnitDisable,
            unit(Manager::User),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            bus.calls()[1..3],
            [
                files_call(
                    "UnmaskUnitFiles",
                    &["[app.service]", "false"],
                    CALL_TIMEOUT_USER
                ),
                files_call(
                    "DisableUnitFiles",
                    &["[app.service]", "false"],
                    CALL_TIMEOUT_USER
                ),
            ]
        );
    }

    #[test]
    fn unit_mask_calls() {
        let bus = FakeUnitBus::new(vec![ok(), ok()], vec![], Tail::Pending);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::UnitMask,
            unit(Manager::User),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            bus.calls(),
            [
                files_call(
                    "MaskUnitFiles",
                    &["[app.service]", "false", "true"],
                    CALL_TIMEOUT_USER
                ),
                reload_call(CALL_TIMEOUT_USER),
            ]
        );
    }

    #[test]
    fn unit_unmask_calls() {
        let bus = FakeUnitBus::new(vec![ok(), ok()], vec![], Tail::Pending);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::UnitUnmask,
            unit(Manager::System),
        );
        assert_eq!(state, ActionState::Succeeded);
        assert_eq!(
            bus.calls(),
            [
                files_call(
                    "UnmaskUnitFiles",
                    &["[app.service]", "false"],
                    CALL_TIMEOUT_SYSTEM
                ),
                reload_call(CALL_TIMEOUT_SYSTEM),
            ]
        );
    }

    #[test]
    fn unit_reload_failure_is_unknown() {
        let bus = FakeUnitBus::new(
            vec![word("enabled"), ok(), Err(call_error(ACCESS_DENIED))],
            vec![],
            Tail::Pending,
        );
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::UnitDisable,
            unit(Manager::System),
        );
        assert_eq!(
            state,
            ActionState::OutcomeUnknown("changed, but the manager reload failed".into())
        );
        assert_eq!(
            bus.calls(),
            [
                state_read(),
                files_call(
                    "DisableUnitFiles",
                    &["[app.service]", "false"],
                    CALL_TIMEOUT_SYSTEM
                ),
                reload_call(CALL_TIMEOUT_SYSTEM),
            ]
        );
    }

    #[test]
    fn unit_enable_denied_no_reload() {
        let bus = FakeUnitBus::new(
            vec![word("disabled"), Err(call_error(ACCESS_DENIED))],
            vec![],
            Tail::Pending,
        );
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::UnitEnable,
            unit(Manager::System),
        );
        assert_eq!(state, ActionState::Failed(ActionError::NotAuthorized));
        assert_eq!(bus.calls().len(), 2, "no Reload: {:?}", bus.calls());
    }

    #[test]
    fn unit_file_state_read_error_stops() {
        let bus = FakeUnitBus::new(vec![Err(call_error(NO_SUCH_UNIT))], vec![], Tail::Pending);
        let state = run(
            actx(Some(Arc::clone(&bus))),
            ActionKind::UnitEnable,
            unit(Manager::User),
        );
        assert_eq!(state, ActionState::Rejected(RejectReason::Unsupported));
        assert_eq!(bus.calls(), [state_read()]);
    }

    #[test]
    fn unit_file_no_connection() {
        let state = run(actx(None), ActionKind::UnitMask, unit(Manager::System));
        assert_eq!(
            state,
            ActionState::Failed(ActionError::Internal("manager not connected".into()))
        );
    }

    #[test]
    fn unit_object_path_escaping() {
        assert_eq!(
            unit_object_path("plasma-plasmashell.service"),
            "/org/freedesktop/systemd1/unit/plasma_2dplasmashell_2eservice"
        );
        assert_eq!(
            unit_object_path("1foo.service"),
            "/org/freedesktop/systemd1/unit/_31foo_2eservice"
        );
        assert_eq!(
            unit_object_path("a1@x_y.slice"),
            "/org/freedesktop/systemd1/unit/a1_40x_5fy_2eslice"
        );
        assert_eq!(unit_object_path(""), "/org/freedesktop/systemd1/unit/_");
    }

    // The real `ZbusUnitBus` against a fake manager over a peer-to-peer pair.

    type Log = Arc<Mutex<Vec<String>>>;

    struct FakeManager {
        log: Log,
        job_result: &'static str,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl FakeManager {
        async fn start_unit(
            &self,
            #[zbus(header)] header: zbus::message::Header<'_>,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
            name: &str,
            mode: &str,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            let interactive = header
                .primary()
                .flags()
                .contains(Flags::AllowInteractiveAuth);
            self.log
                .lock()
                .unwrap()
                .push(format!("StartUnit {name} {mode} interactive={interactive}"));
            let other = ObjectPath::try_from(OTHER_JOB).unwrap();
            let job = ObjectPath::try_from(JOB).unwrap();
            // Both signals go out before the reply.
            Self::job_removed(&emitter, 6, other, name, "done").await?;
            Self::job_removed(&emitter, 7, job, name, self.job_result).await?;
            Ok(OwnedObjectPath::try_from(JOB).unwrap())
        }

        fn reset_failed_unit(&self, name: &str) -> zbus::fdo::Result<()> {
            self.log
                .lock()
                .unwrap()
                .push(format!("ResetFailedUnit {name}"));
            Ok(())
        }

        fn kill_unit(
            &self,
            #[zbus(header)] header: zbus::message::Header<'_>,
            name: &str,
            whom: &str,
            signal: i32,
        ) -> zbus::fdo::Result<()> {
            let interactive = header
                .primary()
                .flags()
                .contains(Flags::AllowInteractiveAuth);
            self.log.lock().unwrap().push(format!(
                "KillUnit {name} {whom} {signal} interactive={interactive}"
            ));
            Ok(())
        }

        fn get_unit_file_state(&self, name: &str) -> String {
            self.log
                .lock()
                .unwrap()
                .push(format!("GetUnitFileState {name}"));
            "masked".into()
        }

        fn unmask_unit_files(
            &self,
            files: Vec<String>,
            runtime: bool,
        ) -> Vec<(String, String, String)> {
            self.log
                .lock()
                .unwrap()
                .push(format!("UnmaskUnitFiles {files:?} {runtime}"));
            vec![]
        }

        fn enable_unit_files(
            &self,
            #[zbus(header)] header: zbus::message::Header<'_>,
            files: Vec<String>,
            runtime: bool,
            force: bool,
        ) -> (bool, Vec<(String, String, String)>) {
            let interactive = header
                .primary()
                .flags()
                .contains(Flags::AllowInteractiveAuth);
            self.log.lock().unwrap().push(format!(
                "EnableUnitFiles {files:?} {runtime} {force} interactive={interactive}"
            ));
            (false, vec![])
        }

        fn reload(&self) {
            self.log.lock().unwrap().push("Reload".into());
        }

        fn stop_unit(&self, _name: &str, _mode: &str) -> zbus::fdo::Result<OwnedObjectPath> {
            Err(zbus::fdo::Error::AccessDenied("no".into()))
        }

        #[zbus(signal)]
        async fn job_removed(
            emitter: &SignalEmitter<'_>,
            id: u32,
            job: ObjectPath<'_>,
            unit: &str,
            result: &str,
        ) -> zbus::Result<()>;
    }

    struct FakeSystemd {
        log: Log,
        _server: zbus::Connection,
        client: Arc<zbus::Connection>,
    }

    fn fake_systemd(job_result: &'static str) -> FakeSystemd {
        let log = Log::default();
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let server = zbus::connection::Builder::async_io_unix_stream(a)
            .server(zbus::Guid::generate())
            .expect("server builder")
            .p2p()
            .serve_at(
                MANAGER_PATH,
                FakeManager {
                    log: Arc::clone(&log),
                    job_result,
                },
            )
            .expect("manager");
        let client = zbus::connection::Builder::async_io_unix_stream(b)
            .p2p()
            .build();
        let (server, client) =
            futures_lite::future::block_on(futures_lite::future::zip(server.build(), client));
        FakeSystemd {
            log,
            _server: server.expect("server connects"),
            client: Arc::new(client.expect("client connects")),
        }
    }

    fn run_real<T: Send + 'static>(f: impl Future<Output = T> + Send + 'static) -> T {
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(f);
        rt.block_on(join).expect("task completes")
    }

    fn real(fake: &FakeSystemd, kind: ActionKind) -> ActionState {
        let conn = Arc::clone(&fake.client);
        let (events, _events_rx) = async_channel::bounded(16);
        let (msgs, _msgs_rx) = asupersync::channel::mpsc::channel(16);
        let user = ManagerConnSlot::new();
        user.set(conn);
        let actx = ActionCtx {
            events,
            msgs,
            ops: Arc::new(NoOps),
            boot_id: BootId(1),
            uid: 1000,
            config: Arc::new(EngineConfig::new(
                EngineMode::Full,
                PathBuf::from("/nonexistent"),
            )),
            systemd_system: ManagerConnSlot::new(),
            systemd_user: user,
            openers: None,
            helper: None,
            destroy_support_sent: AtomicBool::new(false),
            unit_bus_override: None,
        };
        run_real(async move {
            let cx = Cx::current().expect("task has a Cx");
            run_target(&cx, &actx, &kind, &unit(Manager::User)).await
        })
    }

    #[test]
    fn zbus_bus_start_sets_interactive_flag_and_waits() {
        let fake = fake_systemd("done");
        assert_eq!(real(&fake, ActionKind::UnitStart), ActionState::Succeeded);
        assert_eq!(
            *fake.log.lock().unwrap(),
            ["StartUnit app.service replace interactive=true"]
        );

        let failed = fake_systemd("canceled");
        assert_eq!(
            real(&failed, ActionKind::UnitStart),
            ActionState::Failed(ActionError::UnitJob {
                result: "canceled".into(),
                error_name: None
            })
        );
    }

    /// The real argument and reply signatures: `as`, `b`, `b` in and `(b, a(sss))` out.
    #[test]
    fn zbus_bus_enable_masked_unit() {
        let fake = fake_systemd("done");
        assert_eq!(real(&fake, ActionKind::UnitEnable), ActionState::Succeeded);
        assert_eq!(
            *fake.log.lock().unwrap(),
            [
                "GetUnitFileState app.service",
                "UnmaskUnitFiles [\"app.service\"] false",
                "EnableUnitFiles [\"app.service\"] false true interactive=true",
                "Reload",
            ]
        );
    }

    #[test]
    fn zbus_bus_reset_failed_and_error_name() {
        let fake = fake_systemd("done");
        assert_eq!(
            real(&fake, ActionKind::UnitResetFailed),
            ActionState::Succeeded
        );
        assert_eq!(*fake.log.lock().unwrap(), ["ResetFailedUnit app.service"]);
        assert_eq!(
            real(&fake, ActionKind::UnitStop),
            ActionState::Failed(ActionError::NotAuthorized)
        );
        assert_eq!(real(&fake, ActionKind::KillGroup), ActionState::Succeeded);
        assert_eq!(
            fake.log.lock().unwrap()[1],
            "KillUnit app.service all 9 interactive=true"
        );
        // Unknown methods come back as their D-Bus error name.
        assert_eq!(
            real(&fake, ActionKind::UnitReload),
            ActionState::Failed(ActionError::UnitJob {
                result: "failed".into(),
                error_name: Some("org.freedesktop.DBus.Error.UnknownMethod".into()),
            })
        );
    }
}
