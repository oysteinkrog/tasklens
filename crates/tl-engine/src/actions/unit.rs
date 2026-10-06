//! Unit actions on the systemd manager that owns the unit (ADR-005, ADR-006, SYS-005, SYS-006,
//! PROC-072). Part 1: Start, Stop, Restart, Reload and Reset failed.
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

const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
const INTERACTIVE_AUTH_REQUIRED: &str =
    "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired";
const NO_SUCH_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";
const UNIT_MASKED: &str = "org.freedesktop.systemd1.UnitMasked";
const JOB_TYPE_NOT_APPLICABLE: &str = "org.freedesktop.systemd1.JobTypeNotApplicable";
const NOT_CONNECTED: &str = "manager not connected";
const CONNECTION_LOST: &str = "connection lost before the job ended";

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
            let subscribe = zbus::MessageStream::for_match_rule(rule, &*self.conn, None);
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

/// Runs one unit target. This bead handles the five job kinds; the other unit kinds are added by
/// E11-012 and E11-013.
pub async fn run_target(
    cx: &Cx,
    actx: &ActionCtx,
    kind: &ActionKind,
    unit: &UnitRef,
) -> ActionState {
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
            })
        }

        fn calls(&self) -> Vec<FakeCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn job_reply() -> Result<OwnedValue, UnitBusError> {
        Ok(OwnedValue::from(OwnedObjectPath::try_from(JOB).unwrap()))
    }

    fn show(v: &Value<'_>) -> String {
        match v {
            Value::Str(s) => s.to_string(),
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
            _unit_path: &'a str,
            _iface: &'a str,
            _name: &'a str,
        ) -> BusFuture<'a, OwnedValue> {
            Box::pin(async { Err(UnitBusError::NoConnection) })
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
