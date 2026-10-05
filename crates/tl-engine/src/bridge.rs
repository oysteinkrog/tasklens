//! `EngineHandle`, the UI's only view of the engine, and the `EngineHooks` later epics fill in
//! (ADR-005, ADR-001 rule 2).
//!
//! No public signature here names an asupersync type (D-001). The one exception is `HookCtx`,
//! which is `#[doc(hidden)]` and used only inside the crate and by E11's gate factory.
//! `Engine::start` (part 2) and the shutdown steps (E09-A2) are added by later beads.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::ArcSwap;
use asupersync::channel::{mpsc, watch};
use asupersync::runtime::{Runtime, RuntimeHandle};
use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::history::RecordSink;
use tl_model::journal::{JournalError, JournalEvent, JournalHandle, JournalQuery};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::time::BootId;

use crate::config::EngineConfig;
use crate::dbus::SharedHandles;
use crate::reducer::ActionGate;
use crate::update::EngineMsg;

/// Opens one journal stream. Installed by E14-A4 through `EngineHooks`.
pub type JournalSpawner = Arc<dyn Fn(JournalQuery) -> JournalHandle + Send + Sync>;

/// Builds the action gate. Called once per reducer start.
pub type ActionGateFactory = Arc<dyn Fn(HookCtx) -> Box<dyn ActionGate> + Send + Sync>;

/// Builds the record sink from the engine config. Called once per reducer start.
pub type RecordSinkFactory = Arc<dyn Fn(&EngineConfig) -> Box<dyn RecordSink> + Send + Sync>;

/// Optional parts that later epics install: the action gate (E11-A1), the journal (E14-A4) and
/// the record sink (E30-A2). `Default` is `crate::default_hooks()`.
#[derive(Clone)]
pub struct EngineHooks {
    pub action_gate: Option<ActionGateFactory>,
    pub record_sink: Option<RecordSinkFactory>,
    pub journal: Option<JournalSpawner>,
}

impl Default for EngineHooks {
    fn default() -> Self {
        crate::default_hooks()
    }
}

/// What the action gate factory gets. It names asupersync's `mpsc::Sender`, so it stays out of
/// the documented public API (ADR-001).
#[doc(hidden)]
pub struct HookCtx {
    pub events_tx: async_channel::Sender<ActionEvent>,
    pub msgs_tx: mpsc::Sender<EngineMsg>,
    pub boot_id: BootId,
    pub config: Arc<EngineConfig>,
    pub shared: SharedHandles,
}

/// The UI side of a running engine. Cheap to clone: every clone shares one `Inner`.
#[derive(Clone)]
pub struct EngineHandle {
    inner: Arc<Inner>,
}

struct Inner {
    latest: Arc<ArcSwap<Snapshot>>,
    wake_rx: async_channel::Receiver<()>,
    events_rx: async_channel::Receiver<ActionEvent>,
    demand_tx: watch::Sender<Demand>,
    settings_tx: watch::Sender<EngineSettings>,
    msgs_tx: mpsc::Sender<EngineMsg>,
    /// The last demand sent, so an equal demand does not wake the workers.
    last_demand: Mutex<Demand>,
    journal: Option<JournalSpawner>,
    #[allow(dead_code, reason = "Engine::start and shutdown (E09) own the runtime")]
    rt: Mutex<Option<Runtime>>,
    #[allow(dead_code, reason = "Engine::start and shutdown (E09) own the runtime")]
    rt_handle: Mutex<Option<RuntimeHandle>>,
    stopped: AtomicBool,
}

impl EngineHandle {
    /// Builds a handle from the channel ends `Engine::start` creates. The runtime slots start
    /// empty; the last demand starts as the demand channel's current value.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "Engine::start (E09) is the first caller")
    )]
    pub(crate) fn new(
        latest: Arc<ArcSwap<Snapshot>>,
        wake_rx: async_channel::Receiver<()>,
        events_rx: async_channel::Receiver<ActionEvent>,
        demand_tx: watch::Sender<Demand>,
        settings_tx: watch::Sender<EngineSettings>,
        msgs_tx: mpsc::Sender<EngineMsg>,
        journal: Option<JournalSpawner>,
    ) -> Self {
        let last_demand = Mutex::new(demand_tx.borrow().clone());
        Self {
            inner: Arc::new(Inner {
                latest,
                wake_rx,
                events_rx,
                demand_tx,
                settings_tx,
                msgs_tx,
                last_demand,
                journal,
                rt: Mutex::new(None),
                rt_handle: Mutex::new(None),
                stopped: AtomicBool::new(false),
            }),
        }
    }
}

/// A journal handle whose stream yields one `Error(err)` and then ends.
fn closed_journal(err: JournalError) -> JournalHandle {
    let (events_tx, events_rx) = async_channel::bounded(1);
    let (control_tx, _control_rx) = async_channel::bounded(1);
    // The channel is new and has one free slot, so this cannot fail.
    let _ = events_tx.try_send(JournalEvent::Error(err));
    JournalHandle::new(events_rx, control_tx)
}

impl EngineClient for EngineHandle {
    fn latest(&self) -> Arc<Snapshot> {
        self.inner.latest.load_full()
    }

    fn wake(&self) -> async_channel::Receiver<()> {
        self.inner.wake_rx.clone()
    }

    fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        self.inner.events_rx.clone()
    }

    fn set_demand(&self, demand: Demand) {
        let mut last = self
            .inner
            .last_demand
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *last == demand {
            return;
        }
        *last = demand.clone();
        // `watch::Sender::send` needs no `Cx` and never blocks. An error means the engine has
        // stopped, and then the demand no longer matters.
        let _ = self.inner.demand_tx.send(demand);
    }

    fn set_settings(&self, settings: EngineSettings) {
        let _ = self.inner.settings_tx.send(settings);
    }

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        if self.inner.stopped.load(Ordering::Acquire) {
            return Err(CommandError::EngineStopped);
        }
        self.inner
            .msgs_tx
            .try_send(EngineMsg::Command(cmd))
            .map_err(|e| match e {
                mpsc::SendError::Full(_) => CommandError::Busy,
                mpsc::SendError::Disconnected(_) | mpsc::SendError::Cancelled(_) => {
                    CommandError::EngineStopped
                }
            })
    }

    fn journal(&self, query: JournalQuery) -> JournalHandle {
        match &self.inner.journal {
            Some(spawn) => spawn(query),
            None => closed_journal(JournalError::NotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tl_model::table::{Manager, UnitRef};

    /// Command queue size from ADR-005.
    const QUEUE: usize = 256;

    struct Rig {
        handle: EngineHandle,
        demand_rx: watch::Receiver<Demand>,
        msgs_rx: mpsc::Receiver<EngineMsg>,
    }

    fn rig(journal: Option<JournalSpawner>) -> Rig {
        let latest = Arc::new(ArcSwap::from_pointee(Snapshot::empty(BootId(0), 0)));
        let (_wake_tx, wake_rx) = async_channel::bounded(1);
        let (_events_tx, events_rx) = async_channel::bounded(256);
        let (demand_tx, demand_rx) = watch::channel(Demand::default());
        let (settings_tx, _settings_rx) = watch::channel(EngineSettings::default());
        let (msgs_tx, msgs_rx) = mpsc::channel(QUEUE);
        let handle = EngineHandle::new(
            latest,
            wake_rx,
            events_rx,
            demand_tx,
            settings_tx,
            msgs_tx,
            journal,
        );
        Rig {
            handle,
            demand_rx,
            msgs_rx,
        }
    }

    fn query() -> JournalQuery {
        JournalQuery {
            unit: UnitRef {
                manager: Manager::User,
                name: Arc::from("demo.service"),
            },
            follow: false,
        }
    }

    #[test]
    fn handle_is_send_sync_clone() {
        fn check<T: Send + Sync + Clone>() {}
        check::<EngineHandle>();
    }

    #[test]
    fn send_busy_when_full() {
        let r = rig(None);
        for _ in 0..QUEUE {
            assert_eq!(r.handle.send(Command::ResetBaselines), Ok(()));
        }
        assert_eq!(
            r.handle.send(Command::ResetBaselines),
            Err(CommandError::Busy)
        );
    }

    #[test]
    fn set_demand_dedups() {
        let mut r = rig(None);
        let d = Demand {
            memory: true,
            ..Demand::default()
        };
        r.handle.set_demand(d.clone());
        r.handle.set_demand(d.clone());
        assert!(r.demand_rx.has_changed());
        assert_eq!(*r.demand_rx.borrow_and_update(), d);
        // One send means one version step, so a worker's `changed()` wakes once.
        assert_eq!(r.demand_rx.seen_version(), 1);
        assert!(!r.demand_rx.has_changed());
        // A demand equal to the initial value is not sent either.
        let r2 = rig(None);
        r2.handle.set_demand(Demand::default());
        assert!(!r2.demand_rx.has_changed());
    }

    #[test]
    fn send_after_stop() {
        let r = rig(None);
        r.handle.inner.stopped.store(true, Ordering::Release);
        assert_eq!(
            r.handle.send(Command::ResetBaselines),
            Err(CommandError::EngineStopped)
        );
    }

    #[test]
    fn send_after_receiver_dropped() {
        let r = rig(None);
        drop(r.msgs_rx);
        assert_eq!(
            r.handle.send(Command::ResetBaselines),
            Err(CommandError::EngineStopped)
        );
    }

    #[test]
    fn journal_without_spawner_errors_not_found() {
        let r = rig(None);
        let events = r.handle.journal(query()).events();
        match events.try_recv() {
            Ok(JournalEvent::Error(JournalError::NotFound)) => {}
            other => panic!("expected Error(NotFound), got {other:?}"),
        }
        assert!(matches!(
            events.try_recv(),
            Err(async_channel::TryRecvError::Closed)
        ));
    }

    #[test]
    fn journal_uses_spawner() {
        let spawner: JournalSpawner = Arc::new(|_q| closed_journal(JournalError::Timeout));
        let r = rig(Some(spawner));
        let events = r.handle.journal(query()).events();
        assert!(matches!(
            events.try_recv(),
            Ok(JournalEvent::Error(JournalError::Timeout))
        ));
    }
}
