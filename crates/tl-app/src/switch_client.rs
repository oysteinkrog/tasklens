//! `SwitchClient`: the window's `EngineClient`, which forwards to one engine and can move to
//! another while the window runs (ADR-018 Window fallback and Lifetime, tasklens-q4x76.8).
//!
//! tl-ui takes one client at start and keeps it, so the window always talks to this proxy. The
//! proxy holds the current backend: the engine service's `RemoteClient` or the in-process
//! `EngineHandle`. `switch_to` replaces the backend and sends it the window's session state again:
//! the newest demand, engine settings, table query and system table queries.
//!
//! Each backend gets three forwarding threads, for its wake token, its action events and its
//! request ends. They run until the backend's channel closes, so an action or request that was
//! still open on an old backend still ends in the UI (a stopped engine ends them with `Lost`).
//!
//! `generation` is `(switches << 32) | backend generation`. Every switch raises it above every
//! value the old backend gave, so the hub, which orders snapshots by `(generation, sequence)`,
//! shows the new backend's snapshots from sequence 1 and drops a late snapshot of the old one.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::journal::{JournalHandle, JournalQuery};
use tl_model::request::RequestEvent;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::table::{SysTableQuery, TableQuery};
use tracing::warn;

/// Action events waiting for the UI, as in the in-process engine. Beyond this a progress event
/// is dropped; a final state is never dropped.
const ACTION_EVENTS: usize = 256;
/// The backend's generation sits in the low 32 bits, the switch count above it.
const SWITCH_SHIFT: u32 = 32;

/// See the module docs.
pub struct SwitchClient {
    inner: Mutex<Inner>,
    channels: Arc<Channels>,
}

/// The channels the UI reads. The forwarding threads write to them.
struct Channels {
    wake_tx: async_channel::Sender<()>,
    wake_rx: async_channel::Receiver<()>,
    events_tx: async_channel::Sender<ActionEvent>,
    events_rx: async_channel::Receiver<ActionEvent>,
    requests_tx: async_channel::Sender<RequestEvent>,
    requests_rx: async_channel::Receiver<RequestEvent>,
}

struct Inner {
    backend: Arc<dyn EngineClient>,
    /// How many times the backend was replaced.
    switches: u64,
    demand: Option<Demand>,
    settings: Option<EngineSettings>,
    table_query: Option<TableQuery>,
    /// The newest query per system table.
    sys_queries: Vec<SysTableQuery>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl SwitchClient {
    /// A proxy for `backend`.
    pub fn new(backend: Arc<dyn EngineClient>) -> Self {
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        // Unbounded so final states always fit; `forward` caps progress at `ACTION_EVENTS`.
        let (events_tx, events_rx) = async_channel::unbounded();
        let (requests_tx, requests_rx) = async_channel::unbounded();
        let channels = Arc::new(Channels {
            wake_tx,
            wake_rx,
            events_tx,
            events_rx,
            requests_tx,
            requests_rx,
        });
        forward(&backend, &channels);
        Self {
            inner: Mutex::new(Inner {
                backend,
                switches: 0,
                demand: None,
                settings: None,
                table_query: None,
                sys_queries: Vec::new(),
            }),
            channels,
        }
    }

    /// Makes `backend` the current engine, sends it the window's session state and wakes the
    /// UI, so it loads the new backend's snapshot. Returns the old backend, whose forwarding
    /// threads keep running until its channels close.
    pub fn switch_to(&self, backend: Arc<dyn EngineClient>) -> Arc<dyn EngineClient> {
        forward(&backend, &self.channels);
        let old = {
            let mut inner = lock(&self.inner);
            if let Some(settings) = &inner.settings {
                backend.set_settings(settings.clone());
            }
            if let Some(demand) = &inner.demand {
                backend.set_demand(demand.clone());
            }
            // A client that is not connected yet keeps these and sends them when it connects;
            // a full queue loses them, which the next change from the UI repairs.
            if let Some(q) = &inner.table_query {
                let _ = backend.send(Command::SetTableQuery(q.clone()));
            }
            for q in &inner.sys_queries {
                let _ = backend.send(Command::SetSysTableQuery(q.clone()));
            }
            inner.switches += 1;
            std::mem::replace(&mut inner.backend, backend)
        };
        let _ = self.channels.wake_tx.try_send(());
        old
    }

    /// The current backend.
    pub fn backend(&self) -> Arc<dyn EngineClient> {
        Arc::clone(&lock(&self.inner).backend)
    }

    /// The backend and the switch count, from one lock.
    fn current(&self) -> (Arc<dyn EngineClient>, u64) {
        let inner = lock(&self.inner);
        (Arc::clone(&inner.backend), inner.switches)
    }
}

/// The proxy's generation for the backend's `generation` after `switches` switches.
fn combined(switches: u64, generation: u64) -> u64 {
    (switches << SWITCH_SHIFT) | generation.min(u64::from(u32::MAX))
}

/// Starts the three forwarding threads for `backend`. A thread that cannot start is logged; the
/// UI then misses that backend's wake tokens or events.
fn forward(backend: &Arc<dyn EngineClient>, channels: &Arc<Channels>) {
    let wake = backend.wake();
    let to = Arc::clone(channels);
    spawn("tl-switch-wake", move || {
        while wake.recv_blocking().is_ok() {
            let _ = to.wake_tx.try_send(());
        }
    });
    let events = backend.action_events();
    let to = Arc::clone(channels);
    spawn("tl-switch-events", move || {
        while let Ok(ev) = events.recv_blocking() {
            if ev.state.is_final() || to.events_tx.len() < ACTION_EVENTS {
                let _ = to.events_tx.try_send(ev);
            }
        }
    });
    let requests = backend.request_events();
    let to = Arc::clone(channels);
    spawn("tl-switch-requests", move || {
        while let Ok(ev) = requests.recv_blocking() {
            let _ = to.requests_tx.try_send(ev);
        }
    });
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) {
    if let Err(e) = thread::Builder::new().name(name.into()).spawn(body) {
        warn!(target: "tasklens::engine", "{name} thread did not start: {e}");
    }
}

impl EngineClient for SwitchClient {
    fn latest(&self) -> Arc<Snapshot> {
        self.current().0.latest()
    }

    fn wake(&self) -> async_channel::Receiver<()> {
        self.channels.wake_rx.clone()
    }

    fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        self.channels.events_rx.clone()
    }

    fn request_events(&self) -> async_channel::Receiver<RequestEvent> {
        self.channels.requests_rx.clone()
    }

    fn generation(&self) -> u64 {
        let (backend, switches) = self.current();
        combined(switches, backend.generation())
    }

    /// A switch between the lock and the load gives the old backend's snapshot with the old
    /// generation, which the hub then drops as older.
    fn latest_with_generation(&self) -> (u64, Arc<Snapshot>) {
        let (backend, switches) = self.current();
        let (generation, snap) = backend.latest_with_generation();
        (combined(switches, generation), snap)
    }

    fn set_demand(&self, demand: Demand) {
        let mut inner = lock(&self.inner);
        inner.backend.set_demand(demand.clone());
        inner.demand = Some(demand);
    }

    fn set_settings(&self, settings: EngineSettings) {
        let mut inner = lock(&self.inner);
        inner.backend.set_settings(settings.clone());
        inner.settings = Some(settings);
    }

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        let mut inner = lock(&self.inner);
        match &cmd {
            Command::SetTableQuery(q) => inner.table_query = Some(q.clone()),
            Command::SetSysTableQuery(q) => {
                inner.sys_queries.retain(|old| old.table != q.table);
                inner.sys_queries.push(q.clone());
            }
            _ => {}
        }
        inner.backend.send(cmd)
    }

    fn journal(&self, query: JournalQuery) -> JournalHandle {
        self.current().0.journal(query)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use tl_model::actions::{ActionId, ActionState};
    use tl_model::request::{IdKind, RequestEnd};
    use tl_model::table::{SysColumnId, SysSortSpec, SysTable};
    use tl_model::time::BootId;

    /// A backend that records what it gets and publishes what a test gives it.
    pub(crate) struct TestBackend {
        latest: Mutex<Arc<Snapshot>>,
        generation: AtomicU64,
        pub(crate) demands: Mutex<Vec<Demand>>,
        pub(crate) settings: Mutex<Vec<EngineSettings>>,
        pub(crate) commands: Mutex<Vec<Command>>,
        wake_tx: async_channel::Sender<()>,
        wake_rx: async_channel::Receiver<()>,
        pub(crate) events_tx: async_channel::Sender<ActionEvent>,
        events_rx: async_channel::Receiver<ActionEvent>,
        pub(crate) requests_tx: async_channel::Sender<RequestEvent>,
        requests_rx: async_channel::Receiver<RequestEvent>,
    }

    impl TestBackend {
        pub(crate) fn new(generation: u64, sequence: u64) -> Arc<Self> {
            let (wake_tx, wake_rx) = async_channel::bounded(1);
            let (events_tx, events_rx) = async_channel::unbounded();
            let (requests_tx, requests_rx) = async_channel::unbounded();
            let mut snap = Snapshot::empty(BootId(0), 0);
            snap.sequence = sequence;
            Arc::new(Self {
                latest: Mutex::new(Arc::new(snap)),
                generation: AtomicU64::new(generation),
                demands: Mutex::default(),
                settings: Mutex::default(),
                commands: Mutex::default(),
                wake_tx,
                wake_rx,
                events_tx,
                events_rx,
                requests_tx,
                requests_rx,
            })
        }

        pub(crate) fn publish(&self, sequence: u64) {
            let mut snap = Snapshot::empty(BootId(0), 0);
            snap.sequence = sequence;
            *lock(&self.latest) = Arc::new(snap);
            let _ = self.wake_tx.try_send(());
        }
    }

    impl EngineClient for TestBackend {
        fn latest(&self) -> Arc<Snapshot> {
            Arc::clone(&lock(&self.latest))
        }
        fn wake(&self) -> async_channel::Receiver<()> {
            self.wake_rx.clone()
        }
        fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
            self.events_rx.clone()
        }
        fn request_events(&self) -> async_channel::Receiver<RequestEvent> {
            self.requests_rx.clone()
        }
        fn generation(&self) -> u64 {
            self.generation.load(Ordering::Acquire)
        }
        fn set_demand(&self, demand: Demand) {
            lock(&self.demands).push(demand);
        }
        fn set_settings(&self, settings: EngineSettings) {
            lock(&self.settings).push(settings);
        }
        fn send(&self, cmd: Command) -> Result<(), CommandError> {
            lock(&self.commands).push(cmd);
            Ok(())
        }
        fn journal(&self, _query: JournalQuery) -> JournalHandle {
            let (_tx, events) = async_channel::bounded(1);
            let (control, _rx) = async_channel::bounded(1);
            JournalHandle::new(events, control)
        }
    }

    fn recv<T>(rx: &async_channel::Receiver<T>) -> T {
        let begin = std::time::Instant::now();
        loop {
            if let Ok(v) = rx.try_recv() {
                return v;
            }
            assert!(begin.elapsed() < Duration::from_secs(5), "nothing arrived");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn switch_starts_a_higher_generation() {
        // The in-process engine is far ahead; the service numbers from 1.
        let local = TestBackend::new(0, 500);
        let client = SwitchClient::new(local.clone());
        let (before, snap) = client.latest_with_generation();
        assert_eq!((before, snap.sequence), (0, 500));

        let service = TestBackend::new(1, 1);
        let wake = client.wake();
        client.switch_to(service.clone());
        recv(&wake);
        let (after, snap) = client.latest_with_generation();
        assert!(after > before);
        assert_eq!(snap.sequence, 1);
        assert_eq!(client.generation(), after);

        // The service reconnects: its own generation moves, and the proxy's goes up with it.
        service.generation.store(2, Ordering::Release);
        assert!(client.generation() > after);
    }

    #[test]
    fn combined_generation_orders_switches_first() {
        assert!(combined(1, 0) > combined(0, u64::MAX));
        assert!(combined(1, 1) > combined(1, 0));
        assert_eq!(combined(0, 7), 7);
    }

    #[test]
    fn switch_sends_session_state_again() {
        let local = TestBackend::new(0, 1);
        let client = SwitchClient::new(local.clone());
        let demand = Demand::default();
        client.set_demand(demand.clone());
        client.set_settings(EngineSettings::default());
        let table = TableQuery::default();
        client.send(Command::SetTableQuery(table.clone())).unwrap();
        let sys = |generation| SysTableQuery {
            table: SysTable::Startup,
            generation,
            sort: SysSortSpec {
                column: SysColumnId::all(SysTable::Startup)[0],
                ascending: true,
            },
            filter: Arc::from(""),
            expanded: Arc::from([]),
            services: None,
        };
        // The second query for the same table replaces the first.
        client.send(Command::SetSysTableQuery(sys(1))).unwrap();
        client.send(Command::SetSysTableQuery(sys(2))).unwrap();
        client.send(Command::ResetBaselines).unwrap();
        assert_eq!(lock(&local.commands).len(), 4);

        let service = TestBackend::new(0, 0);
        client.switch_to(service.clone());
        assert_eq!(*lock(&service.demands), [demand]);
        assert_eq!(*lock(&service.settings), [EngineSettings::default()]);
        // One table query and one per system table; other commands are never replayed.
        assert_eq!(
            *lock(&service.commands),
            [
                Command::SetTableQuery(table),
                Command::SetSysTableQuery(sys(2)),
            ]
        );
        // Later calls go to the new backend only.
        client.send(Command::ResetBaselines).unwrap();
        assert_eq!(lock(&service.commands).len(), 3);
        assert_eq!(lock(&local.commands).len(), 4);
    }

    #[test]
    fn old_backend_still_ends_its_requests() {
        let local = TestBackend::new(0, 1);
        let client = SwitchClient::new(local.clone());
        let events = client.action_events();
        let requests = client.request_events();
        client.switch_to(TestBackend::new(0, 0));

        // The stopped engine ends what was open; the UI still gets it.
        let lost = ActionEvent {
            id: ActionId(7),
            target: None,
            state: ActionState::OutcomeUnknown(Arc::from("engine stopped")),
        };
        local.events_tx.try_send(lost.clone()).unwrap();
        let end = RequestEvent {
            kind: IdKind::Action,
            id: 7,
            end: RequestEnd::Lost,
        };
        local.requests_tx.try_send(end).unwrap();
        assert_eq!(recv(&events), lost);
        assert_eq!(recv(&requests), end);
    }

    #[test]
    fn wake_follows_the_current_backend() {
        let local = TestBackend::new(0, 1);
        let client = SwitchClient::new(local.clone());
        let wake = client.wake();
        local.publish(2);
        recv(&wake);
        assert_eq!(client.latest().sequence, 2);
    }
}
