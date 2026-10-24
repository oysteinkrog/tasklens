//! The shared zbus connection recipe for every D-Bus worker (ADR-004, API notes section 9).
//!
//! A connection runs with `internal_executor(false)`. A child task ticks its executor, each tick
//! under `EXECUTOR_TICK`, so the task sees cancellation at least that often. When the owner drops
//! the `DbusConn` or its region is cancelled, the task closes the connection (`release`), so the
//! socket is closed on every path out of a generation. zbus futures do not
//! see `Cx` cancellation, so every call goes through `DbusConn::call` or `DbusConn::read`: a
//! `time::timeout` followed by a checkpoint. Proxies built on a connection must not outlive the
//! worker generation that made it.
//!
//! Log D-Bus error names only. Error messages can carry unit or window names; never log them
//! above `debug`.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
/// The longest the tick task keeps a connection going for the owner's last calls after its
/// region is cancelled (`release`).
pub const RELEASE_WAIT: Duration = Duration::from_millis(500);
/// The longest `release` takes to close the connection after that. With `RELEASE_WAIT` this
/// stays under the 1 s that shutdown drains the root region for.
pub const CLOSE_WAIT: Duration = Duration::from_millis(200);
/// How often `release` checks whether the owner dropped the `DbusConn`.
const RELEASE_STEP: Duration = Duration::from_millis(20);

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

/// One bus connection and the task that drives it. Dropping it closes the connection and ends
/// the tick task.
pub struct DbusConn {
    conn: zbus::Connection,
    tick: TaskHandle<()>,
    /// Set when the owner is done with the connection, so the tick task may close it.
    dropped: Arc<AtomicBool>,
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
    drive(cx, conn)
}

/// Spawns the executor tick task of `conn` in the caller's region.
fn drive(cx: &Cx, conn: zbus::Connection) -> Result<DbusConn, DbusError> {
    let driven = conn.clone();
    let dropped = Arc::new(AtomicBool::new(false));
    let owner_done = Arc::clone(&dropped);
    let tick = cx
        .spawn(move |cx| async move {
            loop {
                let _ = time::timeout(cx.now(), EXECUTOR_TICK, driven.executor().tick()).await;
                if cx.checkpoint().is_err() {
                    break;
                }
            }
            let mut release = std::pin::pin!(release(&cx, &driven, &owner_done));
            std::future::poll_fn(|task| cx.masked(|| release.as_mut().poll(task))).await;
        })
        .map_err(|e| DbusError::Connect(format!("spawn: {e}").into()))?;
    Ok(DbusConn {
        conn,
        tick,
        dropped,
    })
}

/// Closes `conn` once its owner dropped the `DbusConn`, or once `RELEASE_WAIT` runs out,
/// whichever comes first. Runs with cancellation masked, so its timers still run.
///
/// zbus queues a `RemoveMatch` task on the connection's own executor when a match-rule stream
/// drops, and that task holds a clone of the connection. With no tick task the executor never
/// runs it, so the connection keeps itself alive: the socket stays open and the bus queues every
/// signal for it against the user's quota. When the quota runs out, dbus-broker disconnects
/// other peers of the same user, and their workers restart (y6k.33). Closing the socket frees
/// the peer on the bus at once and fails the queued tasks; ticking until the executor is empty
/// lets them drop their clones.
async fn release(cx: &Cx, conn: &zbus::Connection, dropped: &AtomicBool) {
    let start = cx.now();
    let left = || RELEASE_WAIT.saturating_sub(Duration::from_nanos(cx.now().duration_since(start)));
    // A cancelled region stops the tick loop before the owner drops the `DbusConn`. Keep driving
    // the connection for the owner's last calls (the KWin script unload) until it does. An owner
    // that is still there after `RELEASE_WAIT` gets a closed connection: the task ends here, so
    // nothing would close it later.
    while !dropped.load(Ordering::Acquire) && !left().is_zero() {
        let step = RELEASE_STEP.min(left());
        let _ = time::timeout(cx.now(), step, conn.executor().tick()).await;
    }
    let close = async {
        let _ = conn.clone().close().await;
        while !conn.executor().is_empty() {
            conn.executor().tick().await;
        }
    };
    let _ = time::timeout(cx.now(), CLOSE_WAIT, close).await;
}

impl DbusConn {
    /// A connection over a peer-to-peer `conn`, so worker tests can talk to a fake service.
    /// Build `conn` with `internal_executor(false)`: the tick task spawned here drives it.
    #[cfg(test)]
    pub(crate) fn p2p(cx: &Cx, conn: zbus::Connection) -> Result<DbusConn, DbusError> {
        drive(cx, conn)
    }

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
        self.dropped.store(true, Ordering::Release);
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

/// Handles shared between workers through `WorkerIo` and with the action gate through
/// `HookCtx` (R-166, R-262).
#[derive(Clone, Default)]
pub struct SharedHandles {
    pub systemd_system: ManagerConnSlot,
    pub systemd_user: ManagerConnSlot,
    /// The session's helper queue (E29-020). `None` when the session runs no helper task.
    pub helper: Option<crate::helper::HelperHandle>,
    /// The package owner index (R-163 item 1): the `Processes` worker runs its batches and the
    /// `ProcessDetail` worker reads it. `None` when no backend is set up.
    pub pkg: Option<tl_collect::pkg::PackageIndex>,
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

    /// Regression (y6k.33): a match-rule stream dropped just before its `DbusConn` left a
    /// `RemoveMatch` task that nothing ran. The connection kept itself alive and its socket
    /// stayed open, so the bus kept queueing signals for it until dbus-broker cut the user's
    /// other peers. Dropping the `DbusConn` now closes the socket.
    /// A peer-to-peer pair over a socketpair: (server, client). The client is built for
    /// `DbusConn::p2p`.
    fn p2p_pair() -> (zbus::Connection, zbus::Connection) {
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let server = zbus::connection::Builder::async_io_unix_stream(a)
            .server(zbus::Guid::generate())
            .expect("server builder")
            .p2p()
            .build();
        let client = zbus::connection::Builder::async_io_unix_stream(b)
            .p2p()
            .internal_executor(false)
            .build();
        let (server, client) =
            futures_lite::future::block_on(futures_lite::future::zip(server, client));
        (
            server.expect("server connects"),
            client.expect("client connects"),
        )
    }

    /// Opens a match-rule stream on `conn` and drops it, so zbus queues a `RemoveMatch` task
    /// that holds a clone of the connection.
    async fn drop_a_match_stream(cx: &Cx, conn: &DbusConn) {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .build();
        let stream = conn
            .read(
                cx,
                zbus::MessageStream::for_match_rule(rule, conn.raw(), Some(8)),
            )
            .await
            .expect("signal stream");
        drop(stream);
    }

    #[test]
    fn dropped_conn_closes_its_socket() {
        let (server, client) = p2p_pair();
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async move {
            let cx = Cx::current().expect("task has a Cx");
            let conn = DbusConn::p2p(&cx, client).expect("p2p connection");
            drop_a_match_stream(&cx, &conn).await;
            drop(conn);
        });
        rt.block_on(join).expect("task completes");
        let end = std::time::Instant::now() + Duration::from_secs(2);
        while !server.is_closed() && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(server.is_closed(), "the peer saw the socket close");
    }

    /// Regression (y6k.33 review): a region cancelled more than `RELEASE_WAIT` before the owner
    /// dropped its `DbusConn` ended the tick task without closing the connection, and the later
    /// drop had no task left to close it. The connection now closes when `RELEASE_WAIT` runs out.
    #[test]
    fn cancelled_conn_closes_before_its_owner_drops() {
        let (server, client) = p2p_pair();
        let peer = server.clone();
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async move {
            let cx = Cx::current().expect("task has a Cx");
            let conn = DbusConn::p2p(&cx, client).expect("p2p connection");
            drop_a_match_stream(&cx, &conn).await;
            // What a cancelled region does to the tick task. The owner keeps the `DbusConn`.
            conn.tick.abort();
            let start = cx.now();
            let waited = || Duration::from_nanos(cx.now().duration_since(start));
            while !peer.is_closed() && waited() < Duration::from_secs(2) {
                time::sleep(cx.now(), RELEASE_STEP).await;
            }
            let closed = peer.is_closed();
            let took = waited();
            drop(conn);
            (closed, took)
        });
        let (closed, took) = rt.block_on(join).expect("task completes");
        assert!(
            closed,
            "the socket closed while the owner still held the DbusConn"
        );
        assert!(
            took >= RELEASE_WAIT,
            "the owner had RELEASE_WAIT first, took {took:?}"
        );
        assert!(server.is_closed());
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
