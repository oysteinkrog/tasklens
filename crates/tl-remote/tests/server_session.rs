//! The engine service's per-session wire side (`serve_session`, ADR-018 Queues and
//! back-pressure, Implementation contracts) against a fake engine port, over an in-memory Unix
//! socket pair: snapshot frames with one in flight and their `Ack`, request ends after their
//! action events and never dropped from a full queue, `Busy` from a full engine queue, the write
//! and ack timeouts, journal streams and their limits, and the port dropped when the session ends.
//!
//! A test waits for the exact step it then asserts on, through the session's `Step` reports, the
//! fake port or the messages the client reads.

// The helpers below are not `#[test]` functions, so `allow-unwrap-in-tests` does not cover them.
#![allow(clippy::unwrap_used)]

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use tl_model::actions::{
    ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, RejectReason, Signal,
};
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::details::{ScanId, StringScanOpts};
use tl_model::ids::ProcessKey;
use tl_model::journal::{
    JournalAccess, JournalControl, JournalEntry, JournalError, JournalEvent, JournalHandle,
    JournalQuery,
};
use tl_model::remote_proto::{
    ClientMsg, HostSettings, ServerMsg, SetHostSettingsReply, SnapshotFrame, StreamId,
};
use tl_model::request::{IdKind, RequestEnd, RequestEvent};
use tl_model::snapshot::Snapshot;
use tl_model::table::{Manager, TableQuery, UnitRef};
use tl_model::time::BootId;
use tl_remote::envelope::{FrameReader, FrameWriter};
use tl_remote::server::{
    Item, Port, Receiver, SessionConfig, SessionEnd, SessionReport, Step, WRITE_TIMEOUT,
    serve_session,
};

/// At least the 2 s floor for wall-clock limits in tests (tasklens-cr0.32).
const TIMEOUT: Duration = Duration::from_secs(5);

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

/// What the fake engine shares with the test.
struct Engine {
    latest: ArcSwap<Snapshot>,
    wake_tx: async_channel::Sender<()>,
    wake_rx: async_channel::Receiver<()>,
    events_tx: async_channel::Sender<ActionEvent>,
    events_rx: async_channel::Receiver<ActionEvent>,
    requests_tx: async_channel::Sender<RequestEvent>,
    requests_rx: async_channel::Receiver<RequestEvent>,
    /// `send` answers `Busy` while set.
    busy: AtomicBool,
    sent: Mutex<Vec<Command>>,
    demands: Mutex<Vec<Demand>>,
    /// Event senders and control receivers of the journal streams, in the order opened.
    journals: Mutex<
        Vec<(
            async_channel::Sender<JournalEvent>,
            async_channel::Receiver<JournalControl>,
        )>,
    >,
    dropped: AtomicBool,
}

impl Engine {
    fn new() -> Arc<Self> {
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (events_tx, events_rx) = async_channel::unbounded();
        let (requests_tx, requests_rx) = async_channel::unbounded();
        Arc::new(Engine {
            latest: ArcSwap::from_pointee(Snapshot::empty(BootId(7), 1)),
            wake_tx,
            wake_rx,
            events_tx,
            events_rx,
            requests_tx,
            requests_rx,
            busy: AtomicBool::new(false),
            sent: Mutex::new(Vec::new()),
            demands: Mutex::new(Vec::new()),
            journals: Mutex::new(Vec::new()),
            dropped: AtomicBool::new(false),
        })
    }

    /// Publishes snapshot `n`: sequence and `published_ns` are both `n`.
    fn publish(&self, n: u64) {
        let mut snap = Snapshot::empty(BootId(7), 1);
        snap.sequence = n;
        snap.published_ns = n;
        self.latest.store(Arc::new(snap));
        let _ = self.wake_tx.try_send(());
    }

    fn event(&self, id: u64, state: ActionState) {
        self.events_tx
            .try_send(ActionEvent {
                id: ActionId(id),
                target: None,
                state,
            })
            .unwrap();
    }

    fn end(&self, kind: IdKind, id: u64, end: RequestEnd) {
        self.requests_tx
            .try_send(RequestEvent { kind, id, end })
            .unwrap();
    }

    fn journal(
        &self,
        n: usize,
    ) -> (
        async_channel::Sender<JournalEvent>,
        async_channel::Receiver<JournalControl>,
    ) {
        wait_for("the journal stream", || {
            self.journals.lock().unwrap().get(n).cloned()
        })
    }
}

/// The port `serve_session` owns. Dropping it is what closes the engine's session.
struct FakePort(Arc<Engine>);

impl Drop for FakePort {
    fn drop(&mut self) {
        self.0.dropped.store(true, Ordering::SeqCst);
    }
}

impl Port for FakePort {
    fn latest(&self) -> Arc<Snapshot> {
        self.0.latest.load_full()
    }

    fn wake(&self) -> Receiver<()> {
        self.0.wake_rx.clone()
    }

    fn action_events(&self) -> Receiver<ActionEvent> {
        self.0.events_rx.clone()
    }

    fn request_events(&self) -> Receiver<RequestEvent> {
        self.0.requests_rx.clone()
    }

    fn set_demand(&self, demand: Demand) {
        self.0.demands.lock().unwrap().push(demand);
    }

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        if self.0.busy.load(Ordering::SeqCst) {
            return Err(CommandError::Busy);
        }
        self.0.sent.lock().unwrap().push(cmd);
        Ok(())
    }

    fn journal(&self, _query: JournalQuery) -> JournalHandle {
        let (events_tx, events_rx) = async_channel::unbounded();
        let (control_tx, control_rx) = async_channel::unbounded();
        self.0
            .journals
            .lock()
            .unwrap()
            .push((events_tx, control_rx));
        JournalHandle::new(events_rx, control_tx)
    }

    fn is_ended(&self) -> bool {
        false
    }
}

/// One session served on a thread, with the client's end of the socket.
struct Rig {
    engine: Arc<Engine>,
    client: UnixStream,
    reader: FrameReader,
    writer: FrameWriter,
    steps: async_channel::Receiver<Step>,
    server: Option<JoinHandle<SessionReport>>,
}

impl Rig {
    fn new(configure: impl FnOnce(&mut SessionConfig)) -> Self {
        let engine = Engine::new();
        let (server, client) = UnixStream::pair().unwrap();
        client.set_read_timeout(Some(TIMEOUT)).unwrap();
        let (tx, steps) = async_channel::unbounded();
        let mut config = SessionConfig {
            observer: Some(tx),
            ..SessionConfig::default()
        };
        configure(&mut config);
        let port = FakePort(Arc::clone(&engine));
        let server = std::thread::spawn(move || serve_session(server, port, config));
        Rig {
            engine,
            client,
            reader: FrameReader::new().unwrap(),
            writer: FrameWriter::new().unwrap(),
            steps,
            server: Some(server),
        }
    }

    fn send(&mut self, msg: &ClientMsg) {
        let mut out = Vec::new();
        self.writer.encode_client(msg, &mut out).unwrap();
        std::io::Write::write_all(&mut &self.client, &out).unwrap();
    }

    fn recv(&mut self) -> ServerMsg {
        let raw = self.reader.read_frame(&mut &self.client).unwrap();
        self.reader.decode_server(&raw).unwrap()
    }

    fn frame(&mut self) -> SnapshotFrame {
        match self.recv() {
            ServerMsg::Snapshot(frame) => *frame,
            other => panic!("expected a snapshot frame, got {other:?}"),
        }
    }

    /// Waits for a step `pick` accepts. Steps before it are skipped.
    fn wait_step(&self, what: &str, pick: impl Fn(&Step) -> bool) -> Step {
        wait_for(what, || {
            while let Ok(step) = self.steps.try_recv() {
                if pick(&step) {
                    return Some(step);
                }
            }
            None
        })
    }

    /// Waits for the session to end and returns its report.
    fn join(&mut self) -> SessionReport {
        let server = self.server.take().unwrap();
        wait_for("the session to end", || server.is_finished().then_some(()));
        server.join().unwrap()
    }
}

/// `n` bytes of text that zstd cannot shrink much, so writing it fills the socket buffer.
fn noise(n: usize) -> Arc<str> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let text: String = (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            char::from(b'!' + (x % 90) as u8)
        })
        .collect();
    Arc::from(text)
}

/// Queues one 4 MiB action summary, far more than the socket buffer, and waits until the
/// writer is writing it. Until the client reads, the writer stays in that write.
fn block_writer(rig: &Rig) {
    rig.engine
        .event(1, ActionState::OutcomeUnknown(noise(4 << 20)));
    rig.wait_step("the writer in the big write", |s| {
        *s == Step::Writing(Item::Action {
            id: ActionId(1),
            progress: false,
        })
    });
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

fn query() -> JournalQuery {
    JournalQuery {
        unit: UnitRef {
            manager: Manager::User,
            name: Arc::from("a.service"),
        },
        follow: true,
    }
}

#[test]
fn one_frame_in_flight_until_its_ack_and_the_next_from_the_newest_publish() {
    let mut rig = Rig::new(|c| c.snapshot_floor = Duration::from_secs(3600));
    rig.engine.publish(1);
    let first = rig.frame();
    assert_eq!((first.frame, first.base, first.published_ns), (1, 0, 1));

    // Two publishes while frame 1 waits for its Ack: no frame is built for them yet.
    rig.engine.publish(2);
    rig.engine.publish(3);
    rig.send(&ClientMsg::Ack { frame: 1 });
    rig.wait_step("Ack 1", |s| *s == Step::Acked(1));
    let second = rig.frame();
    assert_eq!(
        (second.frame, second.base, second.published_ns),
        (2, 1, 3),
        "a delta against frame 1, built from the newest publish"
    );

    // Action events and request ends go out before the next snapshot.
    rig.engine.event(5, ActionState::Started);
    rig.engine.event(5, ActionState::Succeeded);
    rig.engine.end(IdKind::Action, 5, RequestEnd::Done);
    rig.wait_step("the end queued", |s| {
        matches!(s, Step::Queued(Item::Request(_)))
    });
    rig.engine.publish(4);
    rig.send(&ClientMsg::Ack { frame: 2 });
    assert!(matches!(
        rig.recv(),
        ServerMsg::Action(ActionEvent {
            id: ActionId(5),
            state: ActionState::Started,
            ..
        })
    ));
    assert!(matches!(
        rig.recv(),
        ServerMsg::Action(ActionEvent {
            id: ActionId(5),
            state: ActionState::Succeeded,
            ..
        })
    ));
    assert!(matches!(
        rig.recv(),
        ServerMsg::Request(RequestEvent {
            id: 5,
            end: RequestEnd::Done,
            ..
        })
    ));
    let third = rig.frame();
    assert_eq!((third.frame, third.base, third.published_ns), (3, 2, 4));
}

#[test]
fn nothing_is_sent_before_the_first_publish_or_to_a_client_that_wants_no_snapshots() {
    let mut rig = Rig::new(|c| c.snapshots = false);
    rig.engine.publish(1);
    // A reply sent after the publish arrives first and alone.
    rig.send(&ClientMsg::GetHostSettings);
    assert!(matches!(rig.recv(), ServerMsg::HostSettings(h) if h == HostSettings::default()));

    let mut rig = Rig::new(|_| {});
    rig.send(&ClientMsg::GetHostSettings);
    assert!(
        matches!(rig.recv(), ServerMsg::HostSettings(_)),
        "no frame of the empty snapshot"
    );
    rig.send(&ClientMsg::SetHostSettings {
        base_rev: 0,
        changes: Default::default(),
    });
    assert!(matches!(
        rig.recv(),
        ServerMsg::SetHostSettings(SetHostSettingsReply::Conflict { current }) if current == HostSettings::default()
    ));
}

#[test]
fn an_ack_for_a_frame_not_in_flight_closes_the_session() {
    let mut rig = Rig::new(|_| {});
    rig.send(&ClientMsg::Ack { frame: 7 });
    let report = rig.join();
    assert!(
        matches!(report.end, SessionEnd::BadMessage(_)),
        "{:?}",
        report.end
    );
    assert!(rig.engine.dropped.load(Ordering::SeqCst));
}

#[test]
fn each_request_end_follows_its_action_events() {
    let mut rig = Rig::new(|_| {});
    let engine = Arc::clone(&rig.engine);
    // The engine sends each action's events and then its end, as the action gate does, with
    // at most 16 actions in flight per session (the gate's admission limit). Round `n` starts
    // once the client has read the ends of round `n - 1`.
    let (read_tx, read_rx) = std::sync::mpsc::channel::<()>();
    let feeder = std::thread::spawn(move || {
        for round in 0..300 / 16 + 1 {
            if round > 0 && read_rx.recv().is_err() {
                return;
            }
            for id in (round * 16 + 1..=round * 16 + 16).filter(|id| *id <= 300) {
                engine.event(id, ActionState::Started);
                engine.event(id, ActionState::Succeeded);
                engine.end(IdKind::Action, id, RequestEnd::Done);
            }
        }
    });
    let mut summary_seen = vec![false; 301];
    let mut ended = 0;
    while ended < 300 {
        match rig.recv() {
            ServerMsg::Action(ev) if ev.state == ActionState::Succeeded => {
                summary_seen[ev.id.0 as usize] = true;
            }
            ServerMsg::Action(ev) => {
                assert!(
                    !summary_seen[ev.id.0 as usize],
                    "progress after the summary of {}",
                    ev.id.0
                );
            }
            ServerMsg::Request(end) => {
                assert!(
                    summary_seen[end.id as usize],
                    "the end of {} before its summary",
                    end.id
                );
                ended += 1;
                if ended % 16 == 0 {
                    let _ = read_tx.send(());
                }
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    feeder.join().unwrap();
}

#[test]
fn request_ends_and_final_events_replace_progress_in_a_full_queue() {
    let mut rig = Rig::new(|c| c.action_queue = 8);
    block_writer(&rig);
    for id in 100..120 {
        rig.engine.event(id, ActionState::Started);
    }
    for id in 200..204 {
        rig.engine.event(id, ActionState::Succeeded);
        rig.engine.end(IdKind::Action, id, RequestEnd::Done);
    }
    rig.wait_step("the last end queued", |s| {
        matches!(s, Step::Queued(Item::Request(RequestEvent { id: 203, .. })))
    });

    assert!(matches!(
        rig.recv(),
        ServerMsg::Action(ActionEvent {
            id: ActionId(1),
            ..
        })
    ));
    let mut got = Vec::new();
    for _ in 0..8 {
        got.push(rig.recv());
    }
    let mut summaries = Vec::new();
    let mut ends = Vec::new();
    for msg in &got {
        match msg {
            ServerMsg::Action(ev) => {
                assert_ne!(
                    ev.state,
                    ActionState::Started,
                    "every progress event made room"
                );
                summaries.push(ev.id.0);
            }
            ServerMsg::Request(end) => {
                assert!(
                    summaries.contains(&end.id),
                    "the end of {} before its summary",
                    end.id
                );
                ends.push(end.id);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(summaries, vec![200, 201, 202, 203]);
    assert_eq!(ends, vec![200, 201, 202, 203], "no end was dropped");

    drop(rig.client);
    let report = rig.server.take().unwrap().join().unwrap();
    assert_eq!(report.progress_dropped, 20);
}

#[test]
fn a_queue_full_of_ends_closes_the_session() {
    let mut rig = Rig::new(|c| c.action_queue = 8);
    block_writer(&rig);
    for id in 200..209 {
        rig.engine.end(IdKind::Scan, id, RequestEnd::Done);
    }
    let report = rig.join();
    assert_eq!(report.end, SessionEnd::ActionOverflow);
    assert!(rig.engine.dropped.load(Ordering::SeqCst));
}

#[test]
fn a_full_engine_queue_ends_requests_busy_and_resends_the_table_query() {
    let mut rig = Rig::new(|_| {});
    rig.engine.busy.store(true, Ordering::SeqCst);

    rig.send(&ClientMsg::Command(action(5)));
    assert!(matches!(
        rig.recv(),
        ServerMsg::Action(ActionEvent {
            id: ActionId(5),
            target: None,
            state: ActionState::Rejected(RejectReason::Busy),
        })
    ));
    assert!(matches!(
        rig.recv(),
        ServerMsg::Request(RequestEvent {
            kind: IdKind::Action,
            id: 5,
            end: RequestEnd::Busy
        })
    ));
    rig.send(&ClientMsg::Command(scan(9)));
    assert!(matches!(
        rig.recv(),
        ServerMsg::Request(RequestEvent {
            kind: IdKind::Scan,
            id: 9,
            end: RequestEnd::Busy
        })
    ));

    let table = TableQuery {
        generation: 3,
        ..TableQuery::default()
    };
    rig.send(&ClientMsg::Command(Command::SetTableQuery(table.clone())));
    rig.send(&ClientMsg::Command(Command::ResetBaselines));
    // Both were refused; the table query waits for the next message.
    rig.send(&ClientMsg::SetDemand(Demand::default()));
    wait_for("the first demand", || {
        (!rig.engine.demands.lock().unwrap().is_empty()).then_some(())
    });
    assert!(rig.engine.sent.lock().unwrap().is_empty());

    rig.engine.busy.store(false, Ordering::SeqCst);
    rig.send(&ClientMsg::SetDemand(Demand::default()));
    wait_for("the second demand", || {
        (rig.engine.demands.lock().unwrap().len() == 2).then_some(())
    });
    // By the next message at the latest.
    let sent = rig.engine.sent.lock().unwrap().clone();
    assert!(
        matches!(&sent[..], [Command::SetTableQuery(q)] if *q == table),
        "{sent:?}"
    );

    drop(rig.client);
    let report = rig.server.take().unwrap().join().unwrap();
    assert_eq!(report.end, SessionEnd::ClientClosed);
    assert_eq!(report.commands_dropped, 1, "ResetBaselines");
}

#[test]
fn a_quiet_clients_refused_table_query_gets_in_within_2_s_of_room() {
    let mut rig = Rig::new(|_| {});
    rig.engine.busy.store(true, Ordering::SeqCst);
    let query = |generation| {
        Command::SetTableQuery(TableQuery {
            generation,
            ..TableQuery::default()
        })
    };
    rig.send(&ClientMsg::Command(query(1)));
    rig.send(&ClientMsg::Command(query(2)));
    // The reader takes messages in order, so both queries were refused once the demand is in.
    rig.send(&ClientMsg::SetDemand(Demand::default()));
    wait_for("the demand", || {
        (!rig.engine.demands.lock().unwrap().is_empty()).then_some(())
    });
    assert!(rig.engine.sent.lock().unwrap().is_empty());

    // The client sends nothing more.
    rig.engine.busy.store(false, Ordering::SeqCst);
    let room = Instant::now();
    wait_for("the table query", || {
        (!rig.engine.sent.lock().unwrap().is_empty()).then_some(())
    });
    assert!(
        room.elapsed() < Duration::from_secs(2),
        "{:?}",
        room.elapsed()
    );
    let sent = rig.engine.sent.lock().unwrap().clone();
    assert!(
        matches!(&sent[..], [Command::SetTableQuery(q)] if q.generation == 2),
        "only the newest query: {sent:?}"
    );

    drop(rig.client);
    let report = rig.server.take().unwrap().join().unwrap();
    assert_eq!(report.end, SessionEnd::ClientClosed);
}

/// A client that reads `chunk` bytes every `every` while the writer is in a 4 MiB message. The
/// session must close at the default write timeout, counted from the start of that write.
fn slow_client_closed_at_the_write_timeout(chunk: usize, every: Duration) {
    let mut rig = Rig::new(|_| {});
    let slow = rig.client.try_clone().unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let reading = {
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut buf = vec![0; chunk];
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(every);
                if matches!((&slow).read(&mut buf), Ok(0) | Err(_)) {
                    return;
                }
            }
        })
    };
    // Before the write starts, so the deadline cannot fall before `begin + WRITE_TIMEOUT`.
    let begin = Instant::now();
    block_writer(&rig);
    let margin = Duration::from_secs(3);
    let server = rig.server.take().unwrap();
    while !server.is_finished() {
        assert!(
            begin.elapsed() < WRITE_TIMEOUT + margin,
            "the session outlived the write timeout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let elapsed = begin.elapsed();
    let report = server.join().unwrap();
    done.store(true, Ordering::SeqCst);
    reading.join().unwrap();
    assert_eq!(report.end, SessionEnd::WriteTimeout);
    assert!(elapsed >= WRITE_TIMEOUT, "{elapsed:?}");
    assert!(
        rig.engine.dropped.load(Ordering::SeqCst),
        "the slot is free"
    );
}

#[test]
fn a_client_that_takes_1_byte_per_second_is_closed_at_the_write_timeout() {
    slow_client_closed_at_the_write_timeout(1, Duration::from_secs(1));
}

#[test]
fn a_client_that_keeps_taking_a_little_is_closed_at_the_write_timeout() {
    // About 80 KiB/s: each send gets some room, but the message needs far more than 10 s.
    slow_client_closed_at_the_write_timeout(4096, Duration::from_millis(50));
}

#[test]
fn a_write_blocked_past_the_timeout_closes_the_session_and_drops_the_port() {
    let mut rig = Rig::new(|c| c.write_timeout = Duration::from_millis(300));
    let begin = Instant::now();
    rig.engine
        .event(1, ActionState::OutcomeUnknown(noise(4 << 20)));
    let report = rig.join();
    assert_eq!(report.end, SessionEnd::WriteTimeout);
    assert!(begin.elapsed() >= Duration::from_millis(300));
    assert!(
        rig.engine.dropped.load(Ordering::SeqCst),
        "the slot is free"
    );
}

#[test]
fn a_frame_without_its_ack_closes_the_session() {
    let mut rig = Rig::new(|c| c.ack_timeout = Duration::from_millis(300));
    rig.engine.publish(1);
    let first = rig.frame();
    let begin = Instant::now();
    let report = rig.join();
    assert_eq!(report.end, SessionEnd::AckTimeout { frame: first.frame });
    assert!(begin.elapsed() < TIMEOUT);
    assert!(rig.engine.dropped.load(Ordering::SeqCst));
}

#[test]
fn the_ack_deadline_holds_while_other_messages_keep_coming() {
    let mut rig = Rig::new(|c| c.ack_timeout = Duration::from_millis(300));
    rig.engine.publish(1);
    let first = rig.frame();
    let sent = Instant::now();
    block_writer(&rig);
    for id in 100..300 {
        rig.engine.event(id, ActionState::Started);
    }
    rig.wait_step("the last progress event queued", |s| {
        *s == Step::Queued(Item::Action {
            id: ActionId(299),
            progress: true,
        })
    });
    std::thread::sleep(Duration::from_millis(400).saturating_sub(sent.elapsed()));

    // The writer finishes the big write, finds the deadline passed and writes nothing more.
    assert!(matches!(
        rig.recv(),
        ServerMsg::Action(ActionEvent {
            id: ActionId(1),
            ..
        })
    ));
    assert!(
        rig.reader.read_frame(&mut &rig.client).is_err(),
        "no queued event goes out after the deadline"
    );
    let report = rig.join();
    assert_eq!(report.end, SessionEnd::AckTimeout { frame: first.frame });
}

#[test]
fn a_second_ack_before_the_first_is_applied_closes_the_session() {
    let mut rig = Rig::new(|_| {});
    rig.engine.publish(1);
    let first = rig.frame();
    // The writer is busy, so the first Ack waits for it.
    block_writer(&rig);
    rig.send(&ClientMsg::Ack { frame: first.frame });
    rig.send(&ClientMsg::Ack { frame: first.frame });
    let report = rig.join();
    assert!(
        matches!(&report.end, SessionEnd::BadMessage(why) if why.contains("second Ack")),
        "{:?}",
        report.end
    );
    assert!(rig.engine.dropped.load(Ordering::SeqCst));
}

#[test]
fn a_closed_connection_drops_the_port() {
    let mut rig = Rig::new(|_| {});
    let (other, _keep) = UnixStream::pair().unwrap();
    // Swap the client out and drop it.
    drop(std::mem::replace(&mut rig.client, other));
    let report = rig.join();
    assert_eq!(report.end, SessionEnd::ClientClosed);
    assert!(rig.engine.dropped.load(Ordering::SeqCst));
}

#[test]
fn an_ended_engine_session_closes_the_connection() {
    let mut rig = Rig::new(|_| {});
    rig.engine.wake_tx.close();
    let report = rig.join();
    assert_eq!(report.end, SessionEnd::EngineEnded);
    let mut rest = Vec::new();
    std::io::Read::read_to_end(&mut &rig.client, &mut rest).unwrap();
    assert!(rest.is_empty());
    assert!(rig.engine.dropped.load(Ordering::SeqCst));
}

#[test]
fn journal_streams_forward_events_take_controls_and_close() {
    let mut rig = Rig::new(|_| {});
    rig.send(&ClientMsg::JournalOpen {
        stream: StreamId(1),
        query: query(),
    });
    let (events, controls) = rig.engine.journal(0);
    events
        .try_send(JournalEvent::Access(JournalAccess::All))
        .unwrap();
    assert!(matches!(
        rig.recv(),
        ServerMsg::Journal {
            stream: StreamId(1),
            event: JournalEvent::Access(JournalAccess::All)
        }
    ));
    rig.send(&ClientMsg::JournalCtl {
        stream: StreamId(1),
        control: JournalControl::SetFollow(false),
    });
    assert_eq!(
        controls.recv_blocking().unwrap(),
        JournalControl::SetFollow(false)
    );
    rig.send(&ClientMsg::JournalClose {
        stream: StreamId(1),
    });
    // The handle is dropped, which stops the stream's journalctl.
    assert!(controls.recv_blocking().is_err());
}

#[test]
fn a_journal_stream_over_its_limit_ends_and_a_fifth_stream_is_refused() {
    let mut rig = Rig::new(|_| {});
    rig.send(&ClientMsg::JournalOpen {
        stream: StreamId(1),
        query: query(),
    });
    let (events, controls) = rig.engine.journal(0);
    block_writer(&rig);
    for _ in 0..65 {
        events
            .try_send(JournalEvent::Access(JournalAccess::All))
            .unwrap();
    }
    rig.wait_step("the overflow", |s| *s == Step::JournalOverflow(StreamId(1)));
    assert!(
        controls.recv_blocking().is_err(),
        "the stream's journalctl stops"
    );

    assert!(matches!(
        rig.recv(),
        ServerMsg::Action(ActionEvent {
            id: ActionId(1),
            ..
        })
    ));
    for n in 0..64 {
        assert!(
            matches!(
                rig.recv(),
                ServerMsg::Journal {
                    stream: StreamId(1),
                    event: JournalEvent::Access(_)
                }
            ),
            "batch {n}"
        );
    }
    assert!(matches!(
        rig.recv(),
        ServerMsg::Journal {
            stream: StreamId(1),
            event: JournalEvent::Error(JournalError::Overflow)
        }
    ));

    // The ended stream keeps its place until the client closes it: three more fit.
    for id in 2..=4 {
        rig.send(&ClientMsg::JournalOpen {
            stream: StreamId(id),
            query: query(),
        });
    }
    rig.engine.journal(3);
    rig.send(&ClientMsg::JournalOpen {
        stream: StreamId(5),
        query: query(),
    });
    assert!(matches!(
        rig.recv(),
        ServerMsg::Journal {
            stream: StreamId(5),
            event: JournalEvent::Error(_)
        }
    ));
    assert_eq!(
        rig.engine.journals.lock().unwrap().len(),
        4,
        "no fifth journalctl"
    );
}

#[test]
fn one_journal_event_over_the_byte_limit_ends_the_stream() {
    let mut rig = Rig::new(|_| {});
    rig.send(&ClientMsg::JournalOpen {
        stream: StreamId(1),
        query: query(),
    });
    let (events, controls) = rig.engine.journal(0);
    // About 1.6 MiB once compressed, alone in an empty queue.
    let entry = JournalEntry {
        realtime_us: 1,
        priority: 6,
        pid: None,
        message: noise(2 << 20),
        cursor: Arc::from("c1"),
    };
    events
        .try_send(JournalEvent::Batch {
            entries: vec![entry].into(),
            older: false,
            exhausted: false,
        })
        .unwrap();
    rig.wait_step("the overflow", |s| *s == Step::JournalOverflow(StreamId(1)));
    assert!(
        controls.recv_blocking().is_err(),
        "the stream's journalctl stops"
    );
    assert!(matches!(
        rig.recv(),
        ServerMsg::Journal {
            stream: StreamId(1),
            event: JournalEvent::Error(JournalError::Overflow)
        }
    ));
}
