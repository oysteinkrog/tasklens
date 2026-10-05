//! The shared zbus connection recipe for every D-Bus worker (ADR-004, API notes section 9).
//!
//! A connection runs with `internal_executor(false)`. A child task ticks its executor, each tick
//! under `EXECUTOR_TICK`, so the task sees cancellation at least that often. zbus futures do not
//! see `Cx` cancellation, so every call goes through `DbusConn::call` or `DbusConn::read`: a
//! `time::timeout` followed by a checkpoint. Proxies built on a connection must not outlive the
//! worker generation that made it.
//!
//! Log D-Bus error names only. Error messages can carry unit or window names; never log them
//! above `debug`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwapOption;
use asupersync::Cx;
use asupersync::runtime::TaskHandle;
use asupersync::time;
use futures_lite::{Stream, StreamExt};

use crate::update::WorkerError;

/// The default deadline for a read (a property get or a query method).
pub const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// The longest single executor tick. The tick task checks for cancellation after each one.
pub const EXECUTOR_TICK: Duration = Duration::from_millis(500);
/// The deadline for opening a bus connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum DbusError {
    #[error("connect: {0}")]
    Connect(Arc<str>),
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("{name}: {message}")]
    Call { name: Arc<str>, message: Arc<str> },
    /// The `Cx` was cancelled, or a signal stream ended.
    #[error("closed")]
    Closed,
}

impl From<DbusError> for WorkerError {
    fn from(e: DbusError) -> Self {
        WorkerError::Dbus(e.to_string().into())
    }
}

impl DbusError {
    fn from_zbus(e: &zbus::Error) -> Self {
        let message = match e {
            zbus::Error::MethodError(_, detail, _) => detail.clone().unwrap_or_default(),
            other => other.to_string(),
        };
        DbusError::Call {
            name: error_name(e).unwrap_or("zbus").into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bus {
    Session,
    System,
}

/// One bus connection and the task that drives it. Dropping it ends the tick task.
pub struct DbusConn {
    conn: zbus::Connection,
    tick: TaskHandle<()>,
}

/// Opens a connection to `bus` and spawns its executor tick task in the caller's region.
pub async fn connect(cx: &Cx, bus: Bus) -> Result<DbusConn, DbusError> {
    let builder = match bus {
        Bus::Session => zbus::connection::Builder::session(),
        Bus::System => zbus::connection::Builder::system(),
    }
    .map_err(|e| DbusError::Connect(e.to_string().into()))?;
    let build = builder.internal_executor(false).build();
    let conn = match time::timeout(cx.now(), CONNECT_TIMEOUT, build).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(e)) => return Err(DbusError::Connect(e.to_string().into())),
        Err(_) => return Err(DbusError::Timeout(CONNECT_TIMEOUT)),
    };
    if cx.checkpoint().is_err() {
        return Err(DbusError::Closed);
    }
    let driven = conn.clone();
    let tick = cx
        .spawn(move |cx| async move {
            loop {
                let _ = time::timeout(cx.now(), EXECUTOR_TICK, driven.executor().tick()).await;
                if cx.checkpoint().is_err() {
                    break;
                }
            }
        })
        .map_err(|e| DbusError::Connect(format!("spawn: {e}").into()))?;
    Ok(DbusConn { conn, tick })
}

impl DbusConn {
    pub fn raw(&self) -> &zbus::Connection {
        &self.conn
    }

    /// `time::timeout(cx.now(), d, fut)` then `cx.checkpoint()`. `Elapsed` becomes
    /// `DbusError::Timeout(d)`; a failed checkpoint becomes `DbusError::Closed`.
    pub async fn call<T>(
        &self,
        cx: &Cx,
        d: Duration,
        fut: impl Future<Output = zbus::Result<T>>,
    ) -> Result<T, DbusError> {
        timed(cx, d, fut).await
    }

    /// `call` with `READ_TIMEOUT`.
    pub async fn read<T>(
        &self,
        cx: &Cx,
        fut: impl Future<Output = zbus::Result<T>>,
    ) -> Result<T, DbusError> {
        timed(cx, READ_TIMEOUT, fut).await
    }

    /// The next item of a signal stream, waiting at most `d`. Returns `Ok(None)` on timeout so
    /// callers can loop and checkpoint, and `Err(Closed)` when the stream ends or `cx` is
    /// cancelled.
    pub async fn next_signal<S: Stream + Unpin>(
        &self,
        cx: &Cx,
        s: &mut S,
        d: Duration,
    ) -> Result<Option<S::Item>, DbusError> {
        let next = time::timeout(cx.now(), d, s.next()).await;
        if cx.checkpoint().is_err() {
            return Err(DbusError::Closed);
        }
        match next {
            Ok(Some(item)) => Ok(Some(item)),
            Ok(None) => Err(DbusError::Closed),
            Err(_) => Ok(None),
        }
    }

    fn stop_tick(&self) {
        self.tick.abort();
    }
}

impl Drop for DbusConn {
    fn drop(&mut self) {
        self.stop_tick();
    }
}

/// The body of `DbusConn::call`. It needs no connection, so the lab tests call it directly.
async fn timed<T>(
    cx: &Cx,
    d: Duration,
    fut: impl Future<Output = zbus::Result<T>>,
) -> Result<T, DbusError> {
    let result = time::timeout(cx.now(), d, fut).await;
    if cx.checkpoint().is_err() {
        return Err(DbusError::Closed);
    }
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(DbusError::from_zbus(&e)),
        Err(_) => Err(DbusError::Timeout(d)),
    }
}

/// The D-Bus error name, such as `org.freedesktop.DBus.Error.AccessDenied`, for the action error
/// mapping (E11). `None` for errors that did not come from a D-Bus error reply.
pub fn error_name(e: &zbus::Error) -> Option<&str> {
    match e {
        zbus::Error::MethodError(name, _, _) => Some(name.as_str()),
        zbus::Error::FDO(fdo) => fdo_error_name(fdo),
        _ => None,
    }
}

/// `fdo::Error::name()` borrows from a temporary, so the names are spelled out here. Same
/// prefix and `Spawn.` renames as the `DBusError` derive on `zbus::fdo::Error` 5.19.0.
fn fdo_error_name(e: &zbus::fdo::Error) -> Option<&str> {
    use zbus::fdo::Error as E;
    macro_rules! names {
        ($($v:ident),* ; $($s:ident => $n:literal),*) => {
            match e {
                E::ZBus(inner) => error_name(inner),
                $(E::$v(_) => Some(concat!("org.freedesktop.DBus.Error.", stringify!($v))),)*
                $(E::$s(_) => Some(concat!("org.freedesktop.DBus.Error.Spawn.", $n)),)*
            }
        };
    }
    names!(
        Failed, NoMemory, ServiceUnknown, NameHasNoOwner, NoReply, IOError, BadAddress,
        NotSupported, LimitsExceeded, AccessDenied, AuthFailed, NoServer, Timeout, NoNetwork,
        AddressInUse, Disconnected, InvalidArgs, FileNotFound, FileExists, UnknownMethod,
        UnknownObject, UnknownInterface, UnknownProperty, PropertyReadOnly, TimedOut,
        MatchRuleNotFound, MatchRuleInvalid, UnixProcessIdUnknown, InvalidSignature,
        InvalidFileContent, SELinuxSecurityContextUnknown, AdtAuditDataUnknown, ObjectPathInUse,
        InconsistentMessage, InteractiveAuthorizationRequired, NotContainer;
        SpawnExecFailed => "ExecFailed",
        SpawnForkFailed => "ForkFailed",
        SpawnChildExited => "ChildExited",
        SpawnChildSignaled => "ChildSignaled",
        SpawnFailed => "Failed",
        SpawnFailedToSetup => "FailedToSetup",
        SpawnConfigInvalid => "ConfigInvalid",
        SpawnServiceNotValid => "ServiceNotValid",
        SpawnServiceNotFound => "ServiceNotFound",
        SpawnPermissionsInvalid => "PermissionsInvalid",
        SpawnFileInvalid => "FileInvalid",
        SpawnNoMemory => "NoMemory"
    )
}

/// A systemd manager connection that the systemd workers fill and actions, the launcher and the
/// opener read (E11, E14).
#[derive(Clone, Default)]
pub struct ManagerConnSlot(Arc<ArcSwapOption<zbus::Connection>>);

impl ManagerConnSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, conn: Arc<zbus::Connection>) {
        self.0.store(Some(conn));
    }

    pub fn clear(&self) {
        self.0.store(None);
    }

    pub fn get(&self) -> Option<Arc<zbus::Connection>> {
        self.0.load_full()
    }
}

/// D-Bus handles shared between workers through `WorkerIo` (R-166, R-262).
#[derive(Clone, Default)]
pub struct SharedHandles {
    pub systemd_system: ManagerConnSlot,
    pub systemd_user: ManagerConnSlot,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::pin::Pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};

    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};

    type Slot<T> = Arc<Mutex<Option<T>>>;

    fn advance(lab: &mut LabRuntime, d: Duration) {
        lab.advance_time(u64::try_from(d.as_nanos()).expect("fits in u64"));
        // `run_until_idle` stops when no task is ready; one step fires the due timers.
        lab.step_for_test();
        lab.run_until_idle();
    }

    /// Pending until `open` is called, then `Ok(7)`. It never looks at the `Cx`.
    #[derive(Clone, Default)]
    struct Gate(Arc<Mutex<(bool, Option<Waker>)>>);

    impl Gate {
        fn open(&self) {
            let mut state = self.0.lock().expect("gate lock");
            state.0 = true;
            if let Some(waker) = state.1.take() {
                waker.wake();
            }
        }
    }

    impl Future for Gate {
        type Output = zbus::Result<u32>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let mut state = self.0.lock().expect("gate lock");
            if state.0 {
                Poll::Ready(Ok(7))
            } else {
                state.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    #[test]
    fn call_times_out_in_virtual_time() {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let d = Duration::from_millis(10);
        let out: Slot<(Result<u32, DbusError>, Duration)> = Slot::default();
        let sink = Arc::clone(&out);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let start = cx.now();
                let result = timed(&cx, d, std::future::pending::<zbus::Result<u32>>()).await;
                let took = Duration::from_nanos(cx.now().duration_since(start));
                *sink.lock().expect("slot lock") = Some((result, took));
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();

        advance(&mut lab, Duration::from_millis(9));
        assert!(
            out.lock().expect("slot lock").is_none(),
            "done before 10 ms"
        );

        advance(&mut lab, Duration::from_millis(1));
        let (result, took) = out
            .lock()
            .expect("slot lock")
            .take()
            .expect("done at 10 ms");
        assert!(
            matches!(result, Err(DbusError::Timeout(t)) if t == d),
            "result {result:?}"
        );
        assert_eq!(took, d);

        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn call_after_cancel_is_closed() {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let gate = Gate::default();
        let inner = gate.clone();
        let out: Slot<Result<u32, DbusError>> = Slot::default();
        let sink = Arc::clone(&out);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let result = timed(&cx, READ_TIMEOUT, inner).await;
                *sink.lock().expect("slot lock") = Some(result);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        assert!(out.lock().expect("slot lock").is_none(), "the gate is shut");

        handle.abort();
        lab.run_until_idle();
        gate.open();
        lab.run_until_idle();

        let result = out
            .lock()
            .expect("slot lock")
            .take()
            .expect("call returned");
        assert!(
            matches!(result, Err(DbusError::Closed)),
            "result {result:?}"
        );

        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn error_name_extracts_dbus_name() {
        let name = "org.freedesktop.DBus.Error.AccessDenied";
        let call = zbus::Message::method_call("/org/freedesktop/systemd1", "StartUnit")
            .expect("call builder")
            .build(&())
            .expect("call message");
        let reply = zbus::Message::error(&call.header(), name)
            .expect("error builder")
            .build(&("Access denied",))
            .expect("error message");
        let e = zbus::Error::from(reply);
        assert!(matches!(e, zbus::Error::MethodError(..)), "{e:?}");
        assert_eq!(error_name(&e), Some(name));

        let fdo = zbus::Error::from(zbus::fdo::Error::AccessDenied("no".into()));
        assert_eq!(error_name(&fdo), Some(name));
        let spawn = zbus::Error::from(zbus::fdo::Error::SpawnExecFailed("no".into()));
        assert_eq!(
            error_name(&spawn),
            Some("org.freedesktop.DBus.Error.Spawn.ExecFailed")
        );
        assert_eq!(error_name(&zbus::Error::InvalidReply), None);

        let mapped = DbusError::from_zbus(&e);
        assert!(
            matches!(&mapped, DbusError::Call { name: n, message } if &**n == name && &**message == "Access denied"),
            "{mapped:?}"
        );
    }

    #[test]
    fn slot_set_get_clear() {
        let shared = SharedHandles::default();
        let slot = shared.systemd_user.clone();
        assert!(slot.get().is_none());
        assert!(ManagerConnSlot::new().get().is_none());
        // A peer-to-peer pair over a socketpair gives a real connection without a bus.
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let conn = futures_lite::future::block_on(async {
            let server = zbus::connection::Builder::async_io_unix_stream(a)
                .server(zbus::Guid::generate())
                .expect("server builder")
                .p2p()
                .build();
            let client = zbus::connection::Builder::async_io_unix_stream(b)
                .p2p()
                .build();
            let (server, client) = futures_lite::future::zip(server, client).await;
            drop(client.expect("client connects"));
            server.expect("server connects")
        });
        let conn = Arc::new(conn);
        shared.systemd_user.set(Arc::clone(&conn));
        let got = slot.get().expect("set through a clone is visible");
        assert!(Arc::ptr_eq(&got, &conn));
        assert!(shared.systemd_system.get().is_none());
        slot.clear();
        assert!(shared.systemd_user.get().is_none());
    }

    /// Needs a real session bus. Read-only: it connects and reads its own unique name.
    #[test]
    #[ignore = "needs a session bus (owner machine)"]
    fn real_session_connect() {
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async {
            let cx = Cx::current().expect("task has a Cx");
            let conn = connect(&cx, Bus::Session).await.expect("session bus");
            assert!(conn.raw().unique_name().is_some(), "unique name");
            let ping = zbus::fdo::DBusProxy::new(conn.raw())
                .await
                .expect("bus proxy");
            let id = conn
                .read(&cx, async {
                    ping.get_id().await.map_err(zbus::Error::from)
                })
                .await
                .expect("GetId answers through the tick task");
            assert!(!id.as_str().is_empty());
            drop(ping);
            // Drop does exactly this.
            conn.stop_tick();
            time::sleep(cx.now(), EXECUTOR_TICK).await;
            let finished = conn.tick.is_finished();
            drop(conn);
            finished
        });
        let finished = rt.block_on(join).expect("task completes");
        assert!(finished, "the tick task ended within 500 ms");
    }
}
