//! The `Logind` worker (ADR-004, ADR-008 suspend signal 1).
//!
//! It listens for `PrepareForSleep` on the logind manager and sends `SourceUpdate::Sleep` so the
//! reducer can pause rates and mark gaps. It is the first event-driven worker and the pattern
//! for the other D-Bus workers. It only subscribes to a signal and never calls a logind method.
//!
//! No tab lists `Logind` as a surface, so the Collector Health chip never shows it.

use std::sync::Arc;
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use tl_model::health::{SourceId, SourceState};
use tl_model::time::now_boot_ns;

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{Bus, DbusConn, DbusError, READ_TIMEOUT, connect};
use crate::source::WorkerIo;
use crate::supervise::WorkerFactory;
use crate::update::{EngineMsg, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;

pub const BUS_NAME: &str = "org.freedesktop.login1";
pub const PATH: &str = "/org/freedesktop/login1";
pub const INTERFACE: &str = "org.freedesktop.login1.Manager";
pub const SIGNAL: &str = "PrepareForSleep";

/// The health text when the bus or logind cannot be reached.
const NOT_AVAILABLE: &str = "logind not available";

/// Always on, in the full engine only: the low-memory process has no graphs to gap.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Logind,
        name: "logind",
        modes: &[EngineMode::Full],
        factory,
    }
}

pub fn factory(_cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        async move { run(cx, io).await }
    })
}

async fn run(cx: Cx, io: WorkerIo) -> Outcome<(), WorkerError> {
    let mut signals = match LogindSignals::open(&cx).await {
        Ok(signals) => signals,
        Err(e) => {
            if cx.checkpoint().is_err() {
                return Outcome::Ok(());
            }
            let failed = health(SourceState::Failed, Some(NOT_AVAILABLE.into()));
            if let Some(stop) = send(&cx, &io, failed).await {
                return stop;
            }
            return Outcome::Err(e.into());
        }
    };
    if let Some(stop) = send(&cx, &io, health(SourceState::Running, None)).await {
        return stop;
    }
    pump(&cx, &io, &mut signals).await
}

/// Forwards each `PrepareForSleep` argument until `cx` is cancelled or the bus fails. It wakes
/// once per `READ_TIMEOUT` while idle to check for cancellation.
async fn pump<S: SleepSignals>(
    cx: &Cx,
    io: &WorkerIo,
    signals: &mut S,
) -> Outcome<(), WorkerError> {
    loop {
        match signals.next(cx, READ_TIMEOUT).await {
            Ok(Some(entering)) => {
                let sleep = SourceUpdate::Sleep {
                    entering,
                    t_ns: now_boot_ns(),
                };
                if let Some(stop) = send(cx, io, sleep).await {
                    return stop;
                }
            }
            Ok(None) => {
                if cx.checkpoint().is_err() {
                    return Outcome::Ok(());
                }
            }
            Err(e) => {
                // `next_signal` reports a cancelled `cx` as `Closed`.
                if cx.checkpoint().is_err() {
                    return Outcome::Ok(());
                }
                return Outcome::Err(e.into());
            }
        }
    }
}

fn health(state: SourceState, last_error: Option<Arc<str>>) -> SourceUpdate {
    SourceUpdate::Health {
        id: SourceId::Logind,
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

/// The `PrepareForSleep` stream. Tests replace the bus with a fake.
trait SleepSignals {
    /// The next signal's argument, or `Ok(None)` when none arrived within `d`.
    async fn next(&mut self, cx: &Cx, d: Duration) -> Result<Option<bool>, DbusError>;
}

/// The real subscription. Fields drop in order, so the stream goes before the connection's
/// tick task stops. The stream holds its own connection handle, so the proxy is not kept.
struct LogindSignals {
    stream: zbus::proxy::SignalStream<'static>,
    conn: DbusConn,
}

impl LogindSignals {
    /// Connects to the system bus, checks that logind owns its name and subscribes. A missing
    /// logind fails with the `NameHasNoOwner` error from `GetNameOwner`.
    async fn open(cx: &Cx) -> Result<Self, DbusError> {
        let conn = connect(cx, Bus::System).await?;
        let raw = conn.raw();
        let bus = conn.read(cx, zbus::fdo::DBusProxy::new(raw)).await?;
        conn.read(cx, async {
            let name = zbus::names::BusName::try_from(BUS_NAME)?;
            bus.get_name_owner(name).await.map_err(zbus::Error::from)
        })
        .await?;
        let proxy = conn
            .read(cx, zbus::Proxy::new(raw, BUS_NAME, PATH, INTERFACE))
            .await?;
        let stream = conn.read(cx, proxy.receive_signal(SIGNAL)).await?;
        drop(proxy);
        Ok(Self { stream, conn })
    }
}

impl SleepSignals for LogindSignals {
    async fn next(&mut self, cx: &Cx, d: Duration) -> Result<Option<bool>, DbusError> {
        let Some(msg) = self.conn.next_signal(cx, &mut self.stream, d).await? else {
            return Ok(None);
        };
        // The signal body is a single boolean `b`.
        match msg.body().deserialize::<bool>() {
            Ok(entering) => Ok(Some(entering)),
            Err(e) => Err(DbusError::Call {
                name: "zbus".into(),
                message: e.to_string().into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use asupersync::channel::watch;
    use asupersync::time;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::demand::Demand;
    use tl_model::settings::EngineSettings;

    use crate::source::WorkerControl;

    type Slot<T> = Arc<Mutex<Option<T>>>;
    /// The pump's outcome and the lab time in ns when it returned.
    type Ended = Slot<(Outcome<(), WorkerError>, u64)>;

    /// Yields the queued values, then nothing: each later call waits `d` and returns `Ok(None)`.
    struct FakeSignals(VecDeque<bool>);

    impl SleepSignals for FakeSignals {
        async fn next(&mut self, cx: &Cx, d: Duration) -> Result<Option<bool>, DbusError> {
            if let Some(entering) = self.0.pop_front() {
                return Ok(Some(entering));
            }
            let _ = time::timeout(cx.now(), d, std::future::pending::<()>()).await;
            Ok(None)
        }
    }

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

    fn advance(lab: &mut LabRuntime, d: Duration) {
        lab.advance_time(u64::try_from(d.as_nanos()).expect("fits in u64"));
        // `run_until_idle` stops when no task is ready; one step fires the due timers.
        lab.step_for_test();
        lab.run_until_idle();
    }

    /// Starts `pump` over `signals` as a root task. Its outcome and the lab time it ended at
    /// land in the returned slot.
    fn spawn_pump(
        lab: &mut LabRuntime,
        io: WorkerIo,
        mut signals: FakeSignals,
    ) -> (asupersync::runtime::TaskHandle<()>, Ended) {
        let out = Ended::default();
        let sink = Arc::clone(&out);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let outcome = pump(&cx, &io, &mut signals).await;
                let ended = cx.now().as_nanos();
                *sink.lock().expect("slot lock") = Some((outcome, ended));
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        (handle, out)
    }

    #[test]
    fn sleep_events_in_order() {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let (io, mut rx) = worker_io();
        let signals = FakeSignals(VecDeque::from([true, false]));
        let (handle, _out) = spawn_pump(&mut lab, io, signals);

        let mut got = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            got.push(msg);
        }
        let sleeps: Vec<(bool, u64)> = got
            .iter()
            .map(|msg| match msg {
                EngineMsg::Source(SourceUpdate::Sleep { entering, t_ns }) => (*entering, *t_ns),
                other => panic!("unexpected message {other:?}"),
            })
            .collect();
        assert_eq!(sleeps.len(), 2, "{sleeps:?}");
        assert!(sleeps[0].0, "entering first: {sleeps:?}");
        assert!(!sleeps[1].0, "leaving second: {sleeps:?}");
        assert!(sleeps[0].1 < sleeps[1].1, "t_ns increases: {sleeps:?}");

        handle.abort();
        advance(&mut lab, READ_TIMEOUT);
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn cancel_ends_within_read_timeout() {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(100_000));
        let (io, mut rx) = worker_io();
        let (handle, out) = spawn_pump(&mut lab, io, FakeSignals(VecDeque::new()));

        let step = Duration::from_millis(100);
        for _ in 0..100 {
            advance(&mut lab, step);
        }
        assert!(out.lock().expect("slot lock").is_none(), "runs for 10 s");
        assert!(rx.try_recv().is_err(), "no messages without signals");

        let cancelled_at = Duration::from_secs(10).as_nanos();
        handle.abort();
        lab.run_until_idle();
        for _ in 0..20 {
            if out.lock().expect("slot lock").is_some() {
                break;
            }
            advance(&mut lab, step);
        }
        let (outcome, ended) = out
            .lock()
            .expect("slot lock")
            .take()
            .expect("pump returned within 2 s of the cancel");
        assert!(matches!(outcome, Outcome::Ok(())), "{outcome:?}");
        let took = u128::from(ended) - cancelled_at;
        assert!(took <= READ_TIMEOUT.as_nanos(), "took {took} ns");

        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn spec_is_full_mode_only() {
        let s = spec();
        assert_eq!(s.id, SourceId::Logind);
        assert_eq!(s.name, "logind");
        assert_eq!(s.modes, &[EngineMode::Full]);
        let low = crate::workers::workers_for(EngineMode::LowMemory);
        assert!(!low.iter().any(|w| w.id == SourceId::Logind));
        let full = crate::workers::workers_for(EngineMode::Full);
        let count = full.iter().filter(|w| w.id == SourceId::Logind).count();
        assert_eq!(count, 1);
    }

    /// Needs a real system bus with logind. Read-only: it subscribes to a signal and reads one
    /// property.
    #[test]
    #[ignore = "needs a system bus with logind (owner machine)"]
    fn real_logind_connect() {
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async {
            let cx = Cx::current().expect("task has a Cx");
            let (io, mut rx) = worker_io();
            let mut worker = cx
                .spawn(move |cx| async move {
                    let _ = run(cx, io).await;
                })
                .expect("worker spawns");
            let first = time::timeout(cx.now(), Duration::from_secs(5), rx.recv(&cx))
                .await
                .expect("a message within 5 s")
                .expect("channel open");
            assert!(
                matches!(
                    first,
                    EngineMsg::Source(SourceUpdate::Health {
                        id: SourceId::Logind,
                        state: SourceState::Running,
                        ..
                    })
                ),
                "{first:?}"
            );
            worker.abort();
            let _ = worker.join(&cx).await;

            let signals = LogindSignals::open(&cx).await.expect("logind");
            let conn = &signals.conn;
            let proxy = conn
                .read(&cx, zbus::Proxy::new(conn.raw(), BUS_NAME, PATH, INTERFACE))
                .await
                .expect("logind proxy");
            let preparing: bool = conn
                .read(&cx, proxy.get_property("PreparingForSleep"))
                .await
                .expect("PreparingForSleep reads");
            drop(proxy);
            preparing
        });
        let preparing = rt.block_on(join).expect("task completes");
        assert!(!preparing, "the machine is not about to sleep");
    }
}
