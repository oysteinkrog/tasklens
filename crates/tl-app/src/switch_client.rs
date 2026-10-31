//! `SwitchClient`: the window's `EngineClient`, which forwards to one engine and can move to
//! another while the window runs (ADR-018 Window fallback and Lifetime, tasklens-q4x76.8).
//!
//! tl-ui takes one client at start and keeps it, so the window always talks to this proxy. The
//! proxy holds the current backend: the engine service's `RemoteClient` or the in-process
//! `EngineHandle`. `switch_to` replaces the backend and sends it the window's session state again:
//! the newest demand, engine settings, table query and system table queries. Host settings
//! writes go to the current backend and are never sent again: each backend has its own revision
//! (ADR-018 Settings).
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
use tl_model::remote_proto::{HostSettingsChanges, SetHostSettingsReply};
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

    fn set_host_settings(
        &self,
        base_rev: u64,
        changes: HostSettingsChanges,
    ) -> Result<async_channel::Receiver<SetHostSettingsReply>, CommandError> {
        self.current().0.set_host_settings(base_rev, changes)
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

    use std::os::unix::net::UnixStream;

    use gpui_kit::{AppContext as _, TestAppContext};
    use tl_model::actions::{ActionId, ActionState};
    use tl_model::remote_proto::{
        BootPrefix, ClientMsg, HelloTail, HostIdentity, ServerMsg, Want, Welcome, decode_hello,
        encode_welcome,
    };
    use tl_model::request::{IdKind, RequestEnd};
    use tl_model::table::{SysColumnId, SysSortSpec, SysTable};
    use tl_model::time::BootId;
    use tl_remote::client::{ClientConfig, Connection, LinkStatus, RemoteClient};
    use tl_remote::delta::DeltaEncoder;
    use tl_remote::envelope::{FrameReader, FrameWriter};
    use tl_ui::hub::SnapshotHub;

    /// A backend that records what it gets and publishes what a test gives it.
    pub(crate) struct TestBackend {
        latest: Mutex<Arc<Snapshot>>,
        generation: AtomicU64,
        pub(crate) demands: Mutex<Vec<Demand>>,
        pub(crate) settings: Mutex<Vec<EngineSettings>>,
        pub(crate) commands: Mutex<Vec<Command>>,
        pub(crate) host_writes: Mutex<Vec<u64>>,
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
                host_writes: Mutex::default(),
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
        fn set_host_settings(
            &self,
            base_rev: u64,
            _changes: HostSettingsChanges,
        ) -> Result<async_channel::Receiver<SetHostSettingsReply>, CommandError> {
            lock(&self.host_writes).push(base_rev);
            Ok(async_channel::bounded(1).1)
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
        {
            let sent = lock(&service.commands);
            assert_eq!(sent.len(), 2);
            assert!(matches!(&sent[0], Command::SetTableQuery(q) if *q == table));
            assert!(matches!(&sent[1], Command::SetSysTableQuery(q) if *q == sys(2)));
        }
        // Later calls go to the new backend only.
        client.send(Command::ResetBaselines).unwrap();
        assert_eq!(lock(&service.commands).len(), 3);
        assert_eq!(lock(&local.commands).len(), 4);
    }

    #[test]
    fn host_settings_writes_go_to_the_current_backend_only() {
        let local = TestBackend::new(0, 1);
        let client = SwitchClient::new(local.clone());
        client
            .set_host_settings(3, HostSettingsChanges::default())
            .unwrap();
        let service = TestBackend::new(0, 0);
        client.switch_to(service.clone());
        assert!(lock(&service.host_writes).is_empty(), "never sent again");
        client
            .set_host_settings(1, HostSettingsChanges::default())
            .unwrap();
        assert_eq!(*lock(&local.host_writes), [3]);
        assert_eq!(*lock(&service.host_writes), [1]);
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

    /// The service side of one connection: answers the `Hello` with a `Welcome` after `delay`,
    /// sends each of `snaps` after the `Ack` of the one before, then reads until the client goes
    /// or sends nothing for 5 s.
    pub(crate) fn serve(mut stream: UnixStream, delay: Duration, snaps: Vec<Snapshot>) {
        use std::io::Write;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut frames = FrameReader::new().expect("frame reader");
        let hello = decode_hello(&frames.read_frame(&mut stream).expect("hello")).expect("hello");
        assert!(hello.tail.is_some(), "a Hello on our proto");
        thread::sleep(delay);
        let mut out = Vec::new();
        let welcome = Welcome {
            prefix: BootPrefix::ours(),
            tail: Some(HostIdentity {
                host_name: Arc::from("desk"),
                machine_id: Arc::from("m1"),
                boot_id: BootId(7),
                engine_instance: 1,
            }),
        };
        encode_welcome(&welcome, &mut out).expect("welcome");
        stream.write_all(&out).expect("send welcome");
        let mut writer = FrameWriter::new().expect("frame writer");
        let mut encoder = DeltaEncoder::default();
        for snap in snaps {
            let built = encoder
                .next_frame(&snap, 0)
                .expect("frame")
                .expect("no frame in flight");
            out.clear();
            writer
                .encode_server(&ServerMsg::Snapshot(Box::new(built.frame)), &mut out)
                .expect("encode");
            stream.write_all(&out).expect("send snapshot");
            loop {
                let raw = frames.read_frame(&mut stream).expect("client frame");
                if let ClientMsg::Ack { frame } = frames.decode_client(&raw).expect("decode") {
                    encoder.on_ack(frame).expect("ack");
                    break;
                }
            }
        }
        while frames.read_frame(&mut stream).is_ok() {}
    }

    /// A `RemoteClient` whose only connection is `stream`.
    fn remote_on(stream: UnixStream) -> Arc<RemoteClient> {
        let conn = Mutex::new(Some(Connection::unix(stream).expect("connection")));
        let connector = move || {
            lock(&conn)
                .take()
                .ok_or_else(|| std::io::Error::other("no more connections"))
        };
        let config = ClientConfig {
            hello: HelloTail {
                pid: 1,
                app_id: Arc::from("test"),
                bridge: false,
                want: Want { snapshots: true },
            },
            backoff: vec![Duration::from_secs(60)],
        };
        Arc::new(RemoteClient::start(connector, config).expect("client"))
    }

    /// The switch client with a wake channel that only the test thread feeds: GPUI's test
    /// scheduler fails a test when another thread wakes one of its tasks.
    struct Pumped {
        switch: Arc<SwitchClient>,
        wake_tx: async_channel::Sender<()>,
        wake_rx: async_channel::Receiver<()>,
    }

    impl Pumped {
        /// Waits for the switch client's next wake and passes it to the hub.
        fn pump(&self) {
            recv(&self.switch.wake());
            let _ = self.wake_tx.try_send(());
        }
    }

    impl EngineClient for Pumped {
        fn latest(&self) -> Arc<Snapshot> {
            self.switch.latest()
        }
        fn wake(&self) -> async_channel::Receiver<()> {
            self.wake_rx.clone()
        }
        fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
            self.switch.action_events()
        }
        fn generation(&self) -> u64 {
            self.switch.generation()
        }
        fn latest_with_generation(&self) -> (u64, Arc<Snapshot>) {
            self.switch.latest_with_generation()
        }
        fn set_demand(&self, demand: Demand) {
            self.switch.set_demand(demand);
        }
        fn set_settings(&self, settings: EngineSettings) {
            self.switch.set_settings(settings);
        }
        fn send(&self, cmd: Command) -> Result<(), CommandError> {
            self.switch.send(cmd)
        }
        fn journal(&self, query: JournalQuery) -> JournalHandle {
            self.switch.journal(query)
        }
    }

    /// ADR-018 Lifetime: the window moves from its in-process engine, far ahead in sequence, to
    /// the service's `RemoteClient`, which numbers from 1. The hub shows the service's first
    /// snapshot, and the old engine's snapshot that the UI loaded before the switch and installs
    /// after it is dropped.
    #[gpui_kit::test]
    fn hub_shows_the_service_after_a_switch(cx: &mut TestAppContext) {
        let local = TestBackend::new(0, 500);
        let switch = Arc::new(SwitchClient::new(local.clone()));
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let pumped = Arc::new(Pumped {
            switch: Arc::clone(&switch),
            wake_tx,
            wake_rx,
        });
        let hub = cx.new(|cx| SnapshotHub::new(pumped.clone(), cx));
        hub.read_with(cx, |hub, _| assert_eq!(hub.current().sequence, 500));

        // The old engine publishes once more; the UI loads it just before the switch.
        local.publish(501);
        pumped.pump();
        let late = switch.latest_with_generation();
        assert_eq!(late.1.sequence, 501);

        let (ours, theirs) = UnixStream::pair().expect("socket pair");
        let mut first = Snapshot::empty(BootId(7), 1);
        let system = Arc::make_mut(&mut first.system);
        system.header.version = 1;
        system.boot_time_s = 200;
        thread::spawn(move || serve(ours, Duration::ZERO, vec![first]));
        let remote = remote_on(theirs);
        let begin = std::time::Instant::now();
        while remote.status() != LinkStatus::Connected {
            assert!(
                begin.elapsed() < Duration::from_secs(5),
                "no service snapshot"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(remote.latest().sequence, 1);

        switch.switch_to(remote.clone());
        pumped.pump();
        cx.run_until_parked();
        hub.read_with(cx, |hub, _| {
            assert_eq!(hub.current().sequence, 1);
            assert_eq!(hub.current().system.boot_time_s, 200);
        });

        hub.update(cx, |hub, cx| hub.install(late.0, late.1, cx));
        hub.read_with(cx, |hub, _| {
            assert_eq!(hub.current().sequence, 1);
            assert_eq!(hub.current().system.boot_time_s, 200);
        });
    }
}
