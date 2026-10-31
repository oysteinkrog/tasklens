//! `RemoteClient` against an in-memory server on a Unix socket pair (ADR-018): snapshot install,
//! wake and `Ack` order, the bounded command queue, a connection lost during requests, a new
//! engine instance, generation and snapshot read together, action summaries with a full event
//! queue, and journal close.

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::io::{self, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tl_model::actions::{ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, Signal};
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::details::{ScanId, StringScanOpts};
use tl_model::ids::ProcessKey;
use tl_model::journal::{JournalAccess, JournalEvent, JournalQuery};
use tl_model::remote_proto::{
    BootPrefix, ClientMsg, HelloTail, HostIdentity, ServerMsg, StreamId, Want, Welcome,
    decode_hello, encode_welcome,
};
use tl_model::request::{IdKind, RequestEnd, RequestEvent};
use tl_model::snapshot::Snapshot;
use tl_model::table::{Manager, TableQuery, UnitRef};
use tl_model::time::BootId;
use tl_remote::client::{ACTION_EVENTS, ClientConfig, Connection, LinkStatus, RemoteClient};
use tl_remote::delta::DeltaEncoder;
use tl_remote::envelope::{FrameReader, FrameWriter};

const TIMEOUT: Duration = Duration::from_secs(5);

/// Polls `f` until it returns `Some`, for at most `TIMEOUT`.
fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn recv<T>(what: &str, rx: &async_channel::Receiver<T>) -> T {
    wait_for(what, || rx.try_recv().ok())
}

/// A client whose connector hands out the connections a test pushes, with a 10 ms backoff.
fn client() -> (RemoteClient, mpsc::Sender<Connection>) {
    let (tx, rx) = mpsc::channel::<Connection>();
    let rx = Mutex::new(rx);
    let connector = move || {
        rx.lock()
            .unwrap()
            .recv_timeout(Duration::from_millis(20))
            .map_err(|_| io::Error::from(ErrorKind::ConnectionRefused))
    };
    let config = ClientConfig {
        hello: HelloTail {
            pid: 4242,
            app_id: Arc::from("io.github.oysteinkrog.Tasklens"),
            bridge: false,
            want: Want { snapshots: true },
        },
        backoff: vec![Duration::from_millis(10)],
    };
    (RemoteClient::start(connector, config).unwrap(), tx)
}

/// The engine side of one connection.
struct Server {
    stream: UnixStream,
    frames: FrameReader,
    writer: FrameWriter,
    encoder: DeltaEncoder,
}

impl Server {
    /// Pushes a new socket pair to the client's connector and answers its `Hello`.
    fn connect(to: &mpsc::Sender<Connection>, host: &str, instance: u128) -> Server {
        let (ours, theirs) = UnixStream::pair().unwrap();
        to.send(Connection::unix(theirs).unwrap()).unwrap();
        Server::accept(ours, host, instance)
    }

    fn accept(mut stream: UnixStream, host: &str, instance: u128) -> Server {
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        let frames = FrameReader::new().unwrap();
        let hello = decode_hello(&frames.read_frame(&mut stream).unwrap()).unwrap();
        assert_eq!(hello.tail.unwrap().pid, 4242);
        let mut out = Vec::new();
        encode_welcome(
            &Welcome {
                prefix: BootPrefix::ours(),
                tail: Some(HostIdentity {
                    host_name: Arc::from(host),
                    machine_id: Arc::from("m1"),
                    boot_id: BootId(7),
                    engine_instance: instance,
                }),
            },
            &mut out,
        )
        .unwrap();
        stream.write_all(&out).unwrap();
        Server {
            stream,
            frames,
            writer: FrameWriter::new().unwrap(),
            encoder: DeltaEncoder::default(),
        }
    }

    fn send(&mut self, msg: &ServerMsg) {
        let mut out = Vec::new();
        self.writer.encode_server(msg, &mut out).unwrap();
        self.stream.write_all(&out).unwrap();
    }

    fn recv(&mut self) -> ClientMsg {
        let raw = self.frames.read_frame(&mut self.stream).unwrap();
        self.frames.decode_client(&raw).unwrap()
    }

    /// Sends the next frame for `snap`. Returns its number and base.
    fn publish(&mut self, snap: &Snapshot) -> (u64, u64) {
        let built = self.encoder.next_frame(snap, 0).unwrap().unwrap();
        let numbers = (built.frame.frame, built.frame.base);
        self.send(&ServerMsg::Snapshot(Box::new(built.frame)));
        numbers
    }

    /// Reads until an `Ack`, passes it to the encoder and returns it with the messages before it.
    fn ack(&mut self) -> (u64, Vec<ClientMsg>) {
        let mut before = Vec::new();
        loop {
            match self.recv() {
                ClientMsg::Ack { frame } => {
                    self.encoder.on_ack(frame).unwrap();
                    return (frame, before);
                }
                other => before.push(other),
            }
        }
    }
}

fn snapshot(version: u64, boot_time_s: i64) -> Snapshot {
    let mut s = Snapshot::empty(BootId(7), 1);
    let system = Arc::make_mut(&mut s.system);
    system.header.version = version;
    system.boot_time_s = boot_time_s;
    s
}

fn action(id: u64) -> Command {
    Command::Action(ActionRequest {
        id: ActionId(id),
        kind: ActionKind::Signal(Signal::Term),
        targets: Default::default(),
        privileged: false,
    })
}

fn scan(id: u64) -> Command {
    Command::StringScan {
        scan: ScanId(id),
        key: ProcessKey {
            start_ticks: 1,
            boot_id: BootId(7),
            pid: 10,
        },
        opts: StringScanOpts::default(),
    }
}

fn is_command(msg: &ClientMsg, f: impl Fn(&Command) -> bool) -> bool {
    matches!(msg, ClientMsg::Command(cmd) if f(cmd))
}

#[test]
fn snapshot_installed_before_wake_and_ack() {
    let (client, conns) = client();
    let wake = client.wake();
    assert_eq!(client.latest().sequence, 0);
    assert_eq!(client.generation(), 0);
    let mut server = Server::connect(&conns, "home", 1);

    let (first, base) = server.publish(&snapshot(1, 100));
    assert_eq!(base, 0);
    // The Ack comes after the install: the snapshot is there when the server reads it.
    assert_eq!(server.ack().0, first);
    let latest = client.latest();
    assert_eq!((latest.sequence, latest.system.boot_time_s), (1, 100));
    assert!(wake.try_recv().is_ok(), "wake token sent before the Ack");
    assert_eq!(client.generation(), 1);
    assert_eq!(client.status(), LinkStatus::Connected);
    assert_eq!(&*client.host().unwrap().host_name, "home");

    // The next frame is a delta against the acknowledged one.
    let (second, base) = server.publish(&snapshot(2, 101));
    assert_eq!(base, first);
    assert_eq!(server.ack().0, second);
    let latest = client.latest();
    assert_eq!((latest.sequence, latest.system.boot_time_s), (2, 101));
    assert!(wake.try_recv().is_ok());
    assert_eq!(client.generation(), 1);
}

/// Lets a test hold every write to the server.
#[derive(Default)]
struct Gate {
    closed: Mutex<bool>,
    opened: Condvar,
    /// Writes waiting for the gate.
    waiting: AtomicU32,
}

impl Gate {
    fn set(&self, closed: bool) {
        *self.closed.lock().unwrap() = closed;
        self.opened.notify_all();
    }
}

struct GatedWriter {
    inner: UnixStream,
    gate: Arc<Gate>,
}

impl Write for GatedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut closed = self.gate.closed.lock().unwrap();
        if *closed {
            self.gate.waiting.fetch_add(1, Ordering::SeqCst);
            while *closed {
                closed = self.gate.opened.wait(closed).unwrap();
            }
            self.gate.waiting.fetch_sub(1, Ordering::SeqCst);
        }
        drop(closed);
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Connects a server whose reads of the client's writes pass through `Gate`, and installs its
/// first snapshot.
fn gated_server(conns: &mpsc::Sender<Connection>) -> (Server, Arc<Gate>) {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let gate = Arc::new(Gate::default());
    let mut conn = Connection::unix(theirs.try_clone().unwrap()).unwrap();
    conn.writer = Box::new(GatedWriter {
        inner: theirs,
        gate: Arc::clone(&gate),
    });
    conns.send(conn).unwrap();
    let mut server = Server::accept(ours, "home", 1);
    server.publish(&snapshot(1, 100));
    server.ack();
    (server, gate)
}

#[test]
fn full_queue_is_busy_and_ack_goes_first() {
    let (client, conns) = client();
    assert_eq!(
        client.send(Command::ResetBaselines),
        Err(CommandError::EngineStopped)
    );

    let (mut server, gate) = gated_server(&conns);

    // The writer takes the first command and waits at the gate with it.
    gate.set(true);
    client.send(Command::SetupStatus).unwrap();
    wait_for("the writer at the gate", || {
        (gate.waiting.load(Ordering::SeqCst) == 1).then_some(())
    });
    for _ in 0..64 {
        client.send(Command::ResetBaselines).unwrap();
    }
    assert_eq!(client.send(action(1)), Err(CommandError::Busy));

    // An Ack travels on the control path, ahead of the 64 queued commands.
    let (frame, _) = server.publish(&snapshot(2, 101));
    wait_for("the second snapshot", || {
        (client.latest().sequence == 2).then_some(())
    });
    gate.set(false);
    assert!(is_command(&server.recv(), |c| matches!(
        c,
        Command::SetupStatus
    )));
    assert!(matches!(server.recv(), ClientMsg::Ack { frame: f } if f == frame));
    for _ in 0..64 {
        assert!(is_command(&server.recv(), |c| matches!(
            c,
            Command::ResetBaselines
        )));
    }
    // The refused action was never counted: a drop ends nothing for it.
    drop(server);
    wait_for("the drop", || {
        (client.send(Command::ResetBaselines) == Err(CommandError::EngineStopped)).then_some(())
    });
    assert!(client.request_events().try_recv().is_err());
}

#[test]
fn drop_during_action_and_scan_ends_each_once() {
    let (client, conns) = client();
    let events = client.action_events();
    let ends = client.request_events();
    let mut server = Server::connect(&conns, "home", 1);
    server.publish(&snapshot(1, 100));
    server.ack();

    client.send(action(7)).unwrap();
    client.send(action(8)).unwrap();
    client.send(scan(3)).unwrap();
    for _ in 0..3 {
        assert!(matches!(server.recv(), ClientMsg::Command(_)));
    }
    // Action 8 finishes before the drop.
    server.send(&ServerMsg::Action(ActionEvent {
        id: ActionId(8),
        target: None,
        state: ActionState::Succeeded,
    }));
    server.send(&ServerMsg::Request(RequestEvent {
        kind: IdKind::Action,
        id: 8,
        end: RequestEnd::Done,
    }));
    let done = recv("the end of action 8", &ends);
    assert_eq!(done.end, RequestEnd::Done);
    drop(server);

    let first = recv("action 8's summary", &events);
    assert_eq!(first.id, ActionId(8));
    let lost = recv("action 7's summary", &events);
    assert_eq!(lost.id, ActionId(7));
    assert_eq!(lost.target, None);
    assert_eq!(
        lost.state,
        ActionState::OutcomeUnknown(Arc::from("connection to home lost"))
    );
    let mut rest = vec![recv("a lost end", &ends), recv("a lost end", &ends)];
    rest.sort_by_key(|e| (e.kind == IdKind::Scan, e.id));
    assert_eq!(
        rest,
        [
            RequestEvent {
                kind: IdKind::Action,
                id: 7,
                end: RequestEnd::Lost,
            },
            RequestEvent {
                kind: IdKind::Scan,
                id: 3,
                end: RequestEnd::Lost,
            },
        ]
    );
    assert_eq!(
        client.send(Command::ResetBaselines),
        Err(CommandError::EngineStopped)
    );

    // Commands are not replayed, and late ends from the next session are not passed on.
    let mut server = Server::connect(&conns, "home", 1);
    for (kind, id) in [(IdKind::Scan, 3), (IdKind::Action, 7), (IdKind::Action, 8)] {
        server.send(&ServerMsg::Request(RequestEvent {
            kind,
            id,
            end: RequestEnd::Done,
        }));
    }
    server.publish(&snapshot(1, 100));
    let (_, before) = server.ack();
    assert!(before.is_empty(), "nothing replayed: {before:?}");
    assert!(ends.try_recv().is_err());
    assert!(events.try_recv().is_err());
}

#[test]
fn engine_instance_change_starts_a_new_generation() {
    let (client, conns) = client();
    let demand = Demand {
        any_surface_visible: true,
        ..Demand::default()
    };
    let query = TableQuery {
        generation: 3,
        filter: Arc::from("fire"),
        ..TableQuery::default()
    };
    let mut server = Server::connect(&conns, "home", 1);
    server.publish(&snapshot(5, 100));
    server.ack();
    client.set_demand(demand.clone());
    client.send(Command::SetTableQuery(query.clone())).unwrap();
    assert_eq!(client.generation(), 1);
    drop(server);

    let mut server = Server::connect(&conns, "home", 2);
    wait_for("the new instance", || {
        (client.host()?.engine_instance == 2).then_some(())
    });
    // Until the new session's first snapshot the last data stays.
    assert_eq!(client.latest().system.boot_time_s, 100);
    assert_eq!(client.generation(), 1);
    assert_eq!(client.status(), LinkStatus::Connecting);

    // The new engine's versions start over and reach the same value with other data.
    let (_, base) = server.publish(&snapshot(5, 200));
    assert_eq!(base, 0);
    let (_, before) = server.ack();
    let latest = client.latest();
    assert_eq!((latest.sequence, latest.system.boot_time_s), (2, 200));
    assert_eq!(client.generation(), 2);
    assert_eq!(client.status(), LinkStatus::Connected);

    // The session state went to the new session first: demand, then the table query.
    assert_eq!(before.len(), 2, "{before:?}");
    assert!(matches!(&before[0], ClientMsg::SetDemand(d) if *d == demand));
    assert!(is_command(&before[1], |c| matches!(
        c,
        Command::SetTableQuery(q) if *q == query
    )));
}

#[test]
fn generation_and_snapshot_are_read_together() {
    let (client, conns) = client();
    let client = Arc::new(client);
    let stop = Arc::new(AtomicBool::new(false));
    // Reads while the client moves to a new engine instance. Each pair must hold that
    // generation's own data, or the hub would install old data under the new generation.
    let reader = {
        let client = Arc::clone(&client);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut newest = 0;
            while !stop.load(Ordering::Acquire) {
                let (generation, snap) = client.latest_with_generation();
                let expected = match generation {
                    0 => 0,
                    1 => 100,
                    2 => 200,
                    other => panic!("generation {other}"),
                };
                assert_eq!(snap.system.boot_time_s, expected, "generation {generation}");
                newest = newest.max(generation);
            }
            newest
        })
    };
    let mut server = Server::connect(&conns, "home", 1);
    server.publish(&snapshot(5, 100));
    server.ack();
    drop(server);
    let mut server = Server::connect(&conns, "home", 2);
    server.publish(&snapshot(5, 200));
    server.ack();
    stop.store(true, Ordering::Release);
    assert_eq!(reader.join().unwrap(), 2);
    assert_eq!(client.generation(), 2);
}

#[test]
fn action_summary_survives_a_full_event_queue() {
    let (client, conns) = client();
    let events = client.action_events();
    let ends = client.request_events();
    let mut server = Server::connect(&conns, "home", 1);
    server.publish(&snapshot(1, 100));
    server.ack();
    client.send(action(7)).unwrap();
    assert!(matches!(server.recv(), ClientMsg::Command(_)));
    // The UI reads nothing while more progress arrives than the queue holds.
    for _ in 0..ACTION_EVENTS + 10 {
        server.send(&ServerMsg::Action(ActionEvent {
            id: ActionId(7),
            target: None,
            state: ActionState::Started,
        }));
    }
    // Messages are read in order: once this snapshot is acknowledged, all progress was read.
    server.publish(&snapshot(2, 101));
    server.ack();
    assert_eq!(events.len(), ACTION_EVENTS);
    drop(server);

    let end = recv("the lost end", &ends);
    assert_eq!(
        (end.kind, end.id, end.end),
        (IdKind::Action, 7, RequestEnd::Lost)
    );
    let mut all = Vec::new();
    while let Ok(ev) = events.try_recv() {
        all.push(ev);
    }
    // Only progress was dropped: the summary is the last event.
    assert_eq!(all.len(), ACTION_EVENTS + 1);
    assert_eq!(
        all.last().unwrap().state,
        ActionState::OutcomeUnknown(Arc::from("connection to home lost"))
    );
}

#[test]
fn journal_close_before_its_open_sends_neither() {
    let (client, conns) = client();
    let (mut server, gate) = gated_server(&conns);
    let query = JournalQuery {
        unit: UnitRef {
            manager: Manager::User,
            name: Arc::from("a.service"),
        },
        follow: true,
    };
    // The writer waits at the gate with a command, so the open stays in the command queue.
    gate.set(true);
    client.send(Command::SetupStatus).unwrap();
    wait_for("the writer at the gate", || {
        (gate.waiting.load(Ordering::SeqCst) == 1).then_some(())
    });
    let handle = client.journal(query.clone());
    let events = handle.events();
    drop(handle);
    // The stream's events close when its close is queued on the control path.
    wait_for("the close to be queued", || {
        events.is_closed().then_some(())
    });
    client.send(Command::ResetBaselines).unwrap();
    gate.set(false);
    assert!(is_command(&server.recv(), |c| matches!(
        c,
        Command::SetupStatus
    )));
    let next = server.recv();
    assert!(
        is_command(&next, |c| matches!(c, Command::ResetBaselines)),
        "neither open nor close of stream 1 is sent: {next:?}"
    );

    // The next stream opens as usual.
    let _handle = client.journal(query);
    assert!(matches!(
        server.recv(),
        ClientMsg::JournalOpen {
            stream: StreamId(2),
            ..
        }
    ));
}

#[test]
fn journal_close_on_handle_drop() {
    let (client, conns) = client();
    let query = JournalQuery {
        unit: UnitRef {
            manager: Manager::User,
            name: Arc::from("a.service"),
        },
        follow: true,
    };
    // While disconnected the handle's events are closed at once.
    assert!(client.journal(query.clone()).events().is_closed());

    let mut server = Server::connect(&conns, "home", 1);
    server.publish(&snapshot(1, 100));
    server.ack();
    let handle = client.journal(query.clone());
    assert!(matches!(
        server.recv(),
        ClientMsg::JournalOpen { stream: StreamId(1), query: q } if q == query
    ));
    server.send(&ServerMsg::Journal {
        stream: StreamId(1),
        event: JournalEvent::Access(JournalAccess::OwnOnly),
    });
    assert!(matches!(
        recv("the journal event", &handle.events()),
        JournalEvent::Access(JournalAccess::OwnOnly)
    ));
    handle.set_follow(false);
    assert!(matches!(
        server.recv(),
        ClientMsg::JournalCtl {
            stream: StreamId(1),
            ..
        }
    ));
    drop(handle);
    assert!(matches!(
        server.recv(),
        ClientMsg::JournalClose {
            stream: StreamId(1)
        }
    ));

    // A stream still open when the connection drops ends with it.
    let handle = client.journal(query);
    assert!(matches!(
        server.recv(),
        ClientMsg::JournalOpen {
            stream: StreamId(2),
            ..
        }
    ));
    drop(server);
    let events = handle.events();
    wait_for("the stream to close", || events.is_closed().then_some(()));
}
