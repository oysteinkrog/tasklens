//! The per-session wire side of the engine service (ADR-018 Queues and back-pressure, Identity
//! and reconnects, Implementation contracts "Delta flow" and "Queues in bytes").
//!
//! `serve_session` runs one welcomed connection over the engine's side of the session, a `Port`
//! (tl-engine's `SessionPort`), with three threads:
//! - the reader, on the caller's thread, reads the client's messages. `Ack` goes to the writer,
//!   demand and commands go to the port, journal messages to their stream, and host settings
//!   requests get their replies;
//! - the pump moves what the engine sends into the session's queues, so the engine never waits
//!   on the socket. Action events and request ends go to the ordered action queue, journal events
//!   to their stream's queue, and each publish rings the writer;
//! - the writer is the only thread that writes. It sends in the order actions, replies, journal,
//!   snapshot, except that a snapshot goes first once `SNAPSHOT_FLOOR` passed since the last one.
//!
//! Queues and limits:
//! - Snapshots: at most one frame in flight. The next is built only after its `Ack`, from the
//!   newest publish, against the acknowledged frame (`DeltaEncoder`). Nothing is sent before the
//!   session's first publish, so the client keeps its last data until then.
//! - The ordered action queue holds `ACTION_QUEUE` action events and request ends. A request end
//!   follows that request's action events and is never dropped. When the queue is full a progress
//!   event (`Started`) is dropped, and a final event or a request end takes the place of the
//!   oldest queued progress event. With no progress event left to replace, the session closes.
//! - Replies: `REPLIES_MAX` messages or `REPLY_BYTES_MAX` bytes; more closes the session.
//! - Journal: at most `JOURNAL_STREAMS_MAX` streams, each with `JOURNAL_BATCHES_MAX` events or
//!   `JOURNAL_BYTES_MAX` bytes queued. A stream over its limit, even with one event, ends with
//!   `JournalError::Overflow` and its `journalctl` stops; the session goes on. Each opening of a
//!   stream gets a new generation, and the pump keys what it holds by stream id and generation.
//!   An event or end from an earlier opening of a reused id is dropped, so it cannot reach or end
//!   the stream opened after it. The writer keeps the generation with each journal message it
//!   takes and writes it only if that opening is still open; a close waits for a write of its
//!   stream that has started. So nothing of an opening goes on the wire once its close is
//!   handled.
//! - `Ack`: one waits for the writer at most, since one frame is in flight at most. A second
//!   `Ack` before the writer applied the first closes the session.
//! - A message not written within `WRITE_TIMEOUT` of the start of its write, or a frame with no
//!   `Ack` within `ACK_TIMEOUT` of its write, closes the session. Both are one deadline per
//!   message on the monotonic clock: a client that keeps taking a few bytes cannot put the write
//!   deadline off. The writer checks the ack deadline before each message and cuts a write short
//!   at it, so neither a steady flow of other messages nor one slow write can put it off. The
//!   reader notes when it read each `Ack`, and an `Ack` read after the deadline closes the
//!   session even when the writer gets to it only later.
//!
//! A command the engine refuses with `Busy` cannot be refused to the client any more, since the
//! client's `send` already returned. A request (action, scan, handle search, eject, history
//! query) ends with `RequestEnd::Busy`; an action gets a `Rejected(Busy)` summary first. The
//! newest table query and the newest query per system table are kept and offered again before
//! each later client message and on each pass of the pump, which passes at least every `TICK`,
//! until the engine takes them. A quiet client's query so gets in at most `TICK` after the
//! engine has room. Demand needs no retry: the port keeps the newest and never refuses it. Any
//! other refused command is dropped and counted.
//!
//! When the session ends, for any reason, the socket is shut down, the threads stop and the port
//! is dropped, which closes the engine's session and frees its slot.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::ops::Bound;
use std::os::unix::net::UnixStream;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
use rustix::net::{SendFlags, send};
use tl_model::actions::{ActionEvent, ActionId, ActionState, RejectReason};
use tl_model::command::{Command, CommandError, DeviceCommand};
use tl_model::demand::Demand;
use tl_model::journal::{JournalControl, JournalError, JournalEvent, JournalHandle, JournalQuery};
use tl_model::remote_proto::{
    ClientMsg, HostSettings, HostSettingsChanges, ProtoError, ServerMsg, SetHostSettingsReply,
    StreamId,
};
use tl_model::request::{IdKind, RequestEnd, RequestEvent};
use tl_model::snapshot::Snapshot;

use crate::delta::DeltaEncoder;

use crate::envelope::{EnvelopeError, FrameReader, FrameWriter};
/// The receiver type of the `Port` channels, so an implementer needs no async-channel
/// dependency of its own.
pub use async_channel::Receiver;

/// A message not written this long after its write started closes the session.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// A snapshot frame with no `Ack` for this long closes the session.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(30);
/// A snapshot that is ready goes out ahead of other messages once this long passed since the
/// last one.
pub const SNAPSHOT_FLOOR: Duration = Duration::from_secs(2);
/// Action events and request ends queued per session.
pub const ACTION_QUEUE: usize = 256;
/// Replies queued per session, by count and in bytes.
pub const REPLIES_MAX: usize = 16;
pub const REPLY_BYTES_MAX: usize = 4 * 1024 * 1024;
/// Journal streams open per session.
pub const JOURNAL_STREAMS_MAX: usize = 4;
/// Journal events queued per stream, by count and in bytes.
pub const JOURNAL_BATCHES_MAX: usize = 64;
pub const JOURNAL_BYTES_MAX: usize = 1024 * 1024;
/// The writer looks at least this often whether the engine ended the session, and the pump
/// offers refused table queries to the engine again at least this often.
const TICK: Duration = Duration::from_secs(1);

/// The engine's side of one session, as the wire side uses it. tl-server implements it over
/// tl-engine's `SessionPort`, whose methods it mirrors; dropping it closes the session.
pub trait Port: Send + Sync + 'static {
    /// The session's newest snapshot; sequence 0 until the engine first publishes for it.
    fn latest(&self) -> Arc<Snapshot>;
    /// One token after each publish. Closed once the session ended.
    fn wake(&self) -> Receiver<()>;
    fn action_events(&self) -> Receiver<ActionEvent>;
    fn request_events(&self) -> Receiver<RequestEvent>;
    fn set_demand(&self, demand: Demand);
    /// Never blocks: a full engine queue is `Err(Busy)`.
    fn send(&self, cmd: Command) -> Result<(), CommandError>;
    /// Starts a journal stream. Dropping the handle stops it.
    fn journal(&self, query: JournalQuery) -> JournalHandle;
    /// The engine ended the session or stopped.
    fn is_ended(&self) -> bool;
    /// The host settings and their revision (ADR-018 Settings).
    fn host_settings(&self) -> HostSettings;
    /// Applies `changes` when `base_rev` is the current revision, else `Conflict` with the
    /// current settings. Blocks only for the `engine.toml` write.
    fn set_host_settings(
        &self,
        base_rev: u64,
        changes: HostSettingsChanges,
    ) -> SetHostSettingsReply;
}

/// How one session runs. `Default` has the ADR-018 values.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    /// The client wants snapshots (`Want::snapshots`). False for a control-only client.
    pub snapshots: bool,
    pub write_timeout: Duration,
    pub ack_timeout: Duration,
    pub snapshot_floor: Duration,
    pub action_queue: usize,
    /// Gets the session's steps in order (`test-hooks`).
    #[cfg(feature = "test-hooks")]
    pub observer: Option<async_channel::Sender<Step>>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            snapshots: true,
            write_timeout: WRITE_TIMEOUT,
            ack_timeout: ACK_TIMEOUT,
            snapshot_floor: SNAPSHOT_FLOOR,
            action_queue: ACTION_QUEUE,
            #[cfg(feature = "test-hooks")]
            observer: None,
        }
    }
}

/// Why a session ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SessionEnd {
    /// The client closed the connection.
    ClientClosed,
    /// The client sent something that is not a valid frame or message.
    BadMessage(Arc<str>),
    /// The engine ended the session or stopped.
    EngineEnded,
    /// A message was not written within the write timeout.
    WriteTimeout,
    /// Frame `frame` had no `Ack` within the ack timeout.
    AckTimeout { frame: u64 },
    /// The ordered action queue was full of final events and request ends.
    ActionOverflow,
    /// Too many replies waited.
    ReplyOverflow,
    /// A write failed, a frame could not be built, or a thread could not start.
    Failed(Arc<str>),
}

impl fmt::Display for SessionEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClientClosed => write!(f, "the client closed the connection"),
            Self::BadMessage(what) => write!(f, "bad message from the client: {what}"),
            Self::EngineEnded => write!(f, "the engine ended the session"),
            Self::WriteTimeout => write!(f, "a message was not written within the write timeout"),
            Self::AckTimeout { frame } => write!(f, "snapshot frame {frame} was not acknowledged"),
            Self::ActionOverflow => write!(f, "the action event queue overflowed"),
            Self::ReplyOverflow => write!(f, "the reply queue overflowed"),
            Self::Failed(what) => write!(f, "{what}"),
        }
    }
}

/// How a session ended, with what it dropped on the way.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SessionReport {
    pub end: SessionEnd,
    /// Progress events dropped or replaced because the action queue was full.
    pub progress_dropped: u64,
    /// Commands that were not requests or table queries, refused by the engine with `Busy`.
    pub commands_dropped: u64,
    /// Snapshot frames sent without `process_series` and `device_history` to fit the limit.
    pub frames_trimmed: u64,
}

/// What a queued or written message is, for the observer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Item {
    Action { id: ActionId, progress: bool },
    Request(RequestEvent),
    Reply,
    Journal(StreamId),
    Snapshot { frame: u64 },
}

/// One step of a session, reported in the order it happened (`test-hooks`). Each is reported
/// while the queue lock is held or before the write, so the next stage cannot act on it first.
#[cfg(feature = "test-hooks")]
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    /// The message entered its queue.
    Queued(Item),
    /// A progress event found the action queue full and was dropped.
    Dropped(Item),
    /// A queued progress event was dropped to make room for a final event or a request end.
    Evicted(Item),
    /// A journal stream went over its limit and ended.
    JournalOverflow(StreamId),
    /// The writer took a journal message off its queue, but its stream closed before the write,
    /// so it was not written.
    JournalClosed(StreamId),
    /// The writer is about to write the message.
    Writing(Item),
    /// The writer applied `Ack { frame }`.
    Acked(u64),
}

macro_rules! observe {
    ($shared:expr, $step:expr) => {
        #[cfg(feature = "test-hooks")]
        {
            if let Some(tx) = &$shared.config.observer {
                let _ = tx.try_send($step);
            }
        }
    };
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One encoded message in the ordered action queue.
struct Entry {
    bytes: Vec<u8>,
    /// A `Started` event, which may be dropped.
    progress: bool,
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    what: Item,
}

/// One opening of a journal stream: its id and the generation it got when it opened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct StreamKey {
    id: StreamId,
    generation: u64,
}

struct Stream {
    /// Which opening of its id this is. What the pump got from an earlier one is dropped.
    generation: u64,
    /// Dropping it stops the stream's `journalctl`. `None` once the stream ended here.
    handle: Option<JournalHandle>,
    /// What the pump waits on. `None` once the stream ended, here or in the engine.
    events: Option<async_channel::Receiver<JournalEvent>>,
    queue: VecDeque<Vec<u8>>,
    bytes: usize,
}

#[derive(Default)]
struct Queues {
    /// The `Ack` read and not yet applied by the writer, and when the reader read it.
    ack: Option<(u64, Instant)>,
    actions: VecDeque<Entry>,
    replies: VecDeque<Vec<u8>>,
    reply_bytes: usize,
    streams: BTreeMap<StreamId, Stream>,
    /// The stream the writer served last, so each stream gets its turn.
    journal_turn: Option<StreamId>,
    /// The generation the next opened stream gets.
    next_generation: u64,
}

impl Queues {
    /// The stream `key` names, if it is still that opening of its id.
    fn stream_mut(&mut self, key: StreamKey) -> Option<&mut Stream> {
        self.streams
            .get_mut(&key.id)
            .filter(|s| s.generation == key.generation)
    }
}

struct Shared<P> {
    port: P,
    stream: UnixStream,
    config: SessionConfig,
    queues: Mutex<Queues>,
    /// Held by the writer from its last check of a journal message's stream to the end of the
    /// write, and by `journal_close` while it removes the stream. Nothing of a stream is
    /// written once its close is handled, so a reopened id gets nothing from before.
    journal_gate: Mutex<()>,
    /// Table queries the engine refused with `Busy`, the newest per table. Held across each
    /// send of a table query, so a retry cannot overtake a newer query.
    refused: Mutex<Vec<Command>>,
    writer_bell: mpsc::SyncSender<()>,
    /// Rung when the journal streams change. Closed when the session ends.
    pump_bell: async_channel::Sender<()>,
    stopping: AtomicBool,
    end: Mutex<Option<SessionEnd>>,
    progress_dropped: AtomicU64,
    commands_dropped: AtomicU64,
    frames_trimmed: AtomicU64,
}

impl<P: Port> Shared<P> {
    /// The session's shared state, with the receivers of the writer's and the pump's bells.
    fn new(
        stream: UnixStream,
        port: P,
        config: SessionConfig,
    ) -> (Arc<Self>, mpsc::Receiver<()>, async_channel::Receiver<()>) {
        let (writer_bell, writer_rx) = mpsc::sync_channel(1);
        let (pump_bell, pump_rx) = async_channel::bounded(1);
        let shared = Arc::new(Self {
            port,
            stream,
            config,
            queues: Mutex::new(Queues::default()),
            journal_gate: Mutex::new(()),
            refused: Mutex::new(Vec::new()),
            writer_bell,
            pump_bell,
            stopping: AtomicBool::new(false),
            end: Mutex::new(None),
            progress_dropped: AtomicU64::new(0),
            commands_dropped: AtomicU64::new(0),
            frames_trimmed: AtomicU64::new(0),
        });
        (shared, writer_rx, pump_rx)
    }

    /// Ends the session: the first reason wins. Shuts the socket down, so a blocked read or
    /// write returns, and wakes the pump and the writer.
    fn finish(&self, end: SessionEnd) {
        lock(&self.end).get_or_insert(end);
        self.stopping.store(true, Ordering::Release);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        self.pump_bell.close();
        self.ring_writer();
    }

    fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    fn ring_writer(&self) {
        let _ = self.writer_bell.try_send(());
    }

    /// Puts action events and request ends in the ordered queue, in order. False when the
    /// queue overflowed and the session is closing.
    fn queue_ordered(&self, entries: Vec<Entry>) -> bool {
        let limit = self.config.action_queue;
        let mut q = lock(&self.queues);
        for entry in entries {
            if q.actions.len() < limit {
                observe!(self, Step::Queued(entry.what.clone()));
                q.actions.push_back(entry);
            } else if entry.progress {
                self.progress_dropped.fetch_add(1, Ordering::Relaxed);
                observe!(self, Step::Dropped(entry.what.clone()));
            } else if let Some(at) = q.actions.iter().position(|e| e.progress) {
                self.progress_dropped.fetch_add(1, Ordering::Relaxed);
                if let Some(_old) = q.actions.remove(at) {
                    observe!(self, Step::Evicted(_old.what));
                }
                observe!(self, Step::Queued(entry.what.clone()));
                q.actions.push_back(entry);
            } else {
                drop(q);
                self.finish(SessionEnd::ActionOverflow);
                return false;
            }
        }
        drop(q);
        self.ring_writer();
        true
    }

    /// Queues a reply. False when the reply queue overflowed and the session is closing.
    fn queue_reply(&self, frames: &mut FrameWriter, msg: &ServerMsg) -> bool {
        let Some(bytes) = self.encode(frames, msg) else {
            return false;
        };
        let mut q = lock(&self.queues);
        if q.replies.len() >= REPLIES_MAX || q.reply_bytes + bytes.len() > REPLY_BYTES_MAX {
            drop(q);
            self.finish(SessionEnd::ReplyOverflow);
            return false;
        }
        observe!(self, Step::Queued(Item::Reply));
        q.reply_bytes += bytes.len();
        q.replies.push_back(bytes);
        drop(q);
        self.ring_writer();
        true
    }

    /// Queues a journal event on its stream. A stream over its limit gets an error event and
    /// ends. An event for a stream that is closed or ended, or for an earlier opening of its id,
    /// is dropped.
    fn queue_journal(&self, frames: &mut FrameWriter, key: StreamKey, event: JournalEvent) {
        let id = key.id;
        let Some(bytes) = self.encode(frames, &ServerMsg::Journal { stream: id, event }) else {
            return;
        };
        let mut q = lock(&self.queues);
        let Some(s) = q.stream_mut(key) else {
            return;
        };
        if s.handle.is_none() {
            return;
        }
        let full =
            s.queue.len() >= JOURNAL_BATCHES_MAX || s.bytes + bytes.len() > JOURNAL_BYTES_MAX;
        let handle = if full {
            let error = ServerMsg::Journal {
                stream: id,
                event: JournalEvent::Error(JournalError::Overflow),
            };
            // Small and plain, so it encodes.
            let mut end = Vec::new();
            let _ = frames.encode_server(&error, &mut end);
            s.bytes += end.len();
            s.queue.push_back(end);
            s.events = None;
            observe!(self, Step::JournalOverflow(id));
            s.handle.take()
        } else {
            s.bytes += bytes.len();
            s.queue.push_back(bytes);
            observe!(self, Step::Queued(Item::Journal(id)));
            None
        };
        drop(q);
        // Stops the stream's journalctl, outside the lock.
        drop(handle);
        self.ring_writer();
    }

    /// The engine closed the events of stream `key`. An end from an earlier opening of its id
    /// is dropped, so it cannot end the stream opened after it.
    fn journal_done(&self, key: StreamKey) {
        if let Some(s) = lock(&self.queues).stream_mut(key) {
            s.events = None;
        }
    }

    /// The event receivers of the open streams, keyed by opening, for the pump to wait on.
    fn journal_receivers(&self) -> Vec<(StreamKey, async_channel::Receiver<JournalEvent>)> {
        lock(&self.queues)
            .streams
            .iter()
            .filter_map(|(id, s)| {
                let key = StreamKey {
                    id: *id,
                    generation: s.generation,
                };
                s.events.clone().map(|rx| (key, rx))
            })
            .collect()
    }

    fn encode(&self, frames: &mut FrameWriter, msg: &ServerMsg) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        match frames.encode_server(msg, &mut out) {
            Ok(()) => Some(out),
            Err(e) => {
                self.finish(SessionEnd::Failed(Arc::from(e.to_string())));
                None
            }
        }
    }

    fn entry(&self, frames: &mut FrameWriter, msg: ServerMsg) -> Option<Entry> {
        let (progress, what) = match &msg {
            ServerMsg::Action(ev) => {
                let progress = !ev.state.is_final();
                (
                    progress,
                    Item::Action {
                        id: ev.id,
                        progress,
                    },
                )
            }
            ServerMsg::Request(end) => (false, Item::Request(*end)),
            _ => (false, Item::Reply),
        };
        let bytes = self.encode(frames, &msg)?;
        Some(Entry {
            bytes,
            progress,
            what,
        })
    }

    /// Writes `bytes` within one write timeout from the start. Each send takes what fits and
    /// never blocks; between sends the writer waits for room at most the time left. A socket
    /// send timeout would not do: the kernel starts it again for each buffer within one write,
    /// so a client that keeps reading a little could hold one write for ever.
    ///
    /// `ack_due` is the frame in flight and its ack deadline. The write also stops there, unless
    /// an `Ack` read before the deadline is waiting for the writer by then.
    fn write(&self, bytes: &[u8], mut ack_due: Option<(u64, Instant)>) -> Result<(), SessionEnd> {
        let deadline = Instant::now() + self.config.write_timeout;
        let mut rest = bytes;
        while !rest.is_empty() {
            let now = Instant::now();
            if let Some((frame, due)) = ack_due
                && now >= due
            {
                if !lock(&self.queues).ack.is_some_and(|(_, read)| read < due) {
                    return Err(SessionEnd::AckTimeout { frame });
                }
                // The Ack came in time. The writer applies it after this write.
                ack_due = None;
            }
            if now >= deadline {
                return Err(SessionEnd::WriteTimeout);
            }
            let until = ack_due.map_or(deadline, |(_, due)| due.min(deadline));
            let left = until.saturating_duration_since(now);
            match send(
                &self.stream,
                rest,
                SendFlags::DONTWAIT | SendFlags::NOSIGNAL,
            ) {
                Ok(n) => rest = &rest[n..],
                Err(Errno::INTR) => {}
                Err(Errno::AGAIN) => {
                    let left = Timespec::try_from(left)
                        .map_err(|_| SessionEnd::Failed(Arc::from("write timeout too long")))?;
                    let mut fds = [PollFd::new(&self.stream, PollFlags::OUT)];
                    // On a timeout the next pass finds which deadline passed.
                    match poll(&mut fds, Some(&left)) {
                        Ok(_) | Err(Errno::INTR) => {}
                        Err(e) => return Err(SessionEnd::Failed(Arc::from(format!("poll: {e}")))),
                    }
                }
                Err(e) => return Err(SessionEnd::Failed(Arc::from(format!("write: {e}")))),
            }
        }
        Ok(())
    }
}

fn journal_error(text: &str) -> JournalError {
    JournalError::Exit {
        code: -1,
        stderr_first_line: Arc::from(text),
    }
}

/// Serves one welcomed connection over `port` until the client goes, the engine ends the
/// session or a limit closes it. Runs the reader on this thread and the pump and the writer on
/// two more, which have stopped when it returns. The port is dropped before it returns.
pub fn serve_session<P: Port>(stream: UnixStream, port: P, config: SessionConfig) -> SessionReport {
    let _ = stream.set_read_timeout(None);
    // The writer keeps its own deadline (`Shared::write`).
    let _ = stream.set_write_timeout(None);
    let (shared, writer_rx, pump_rx) = Shared::new(stream, port, config);
    let mut threads = Vec::new();
    let writer = {
        let sh = Arc::clone(&shared);
        thread::Builder::new()
            .name("tl-session-writer".into())
            .spawn(move || write_loop(&sh, &writer_rx))
    };
    let pump = {
        let sh = Arc::clone(&shared);
        thread::Builder::new()
            .name("tl-session-pump".into())
            .spawn(move || pump_loop(&sh, &pump_rx))
    };
    for spawned in [writer, pump] {
        match spawned {
            Ok(handle) => threads.push(handle),
            Err(e) => shared.finish(SessionEnd::Failed(Arc::from(format!("thread: {e}")))),
        }
    }
    let end = read_loop(&shared);
    shared.finish(end);
    for handle in threads {
        let _ = handle.join();
    }
    let end = lock(&shared.end).take().unwrap_or(SessionEnd::ClientClosed);
    let report = SessionReport {
        end,
        progress_dropped: shared.progress_dropped.load(Ordering::Relaxed),
        commands_dropped: shared.commands_dropped.load(Ordering::Relaxed),
        frames_trimmed: shared.frames_trimmed.load(Ordering::Relaxed),
    };
    // The last reference: the port drops here, which closes the engine's session.
    drop(shared);
    report
}

// ---- Reader ----

/// Reads the client's messages until the stream ends or the session is closing.
fn read_loop<P: Port>(sh: &Shared<P>) -> SessionEnd {
    let (mut frames, mut out) = match (FrameReader::new(), FrameWriter::new()) {
        (Ok(r), Ok(w)) => (r, w),
        _ => return SessionEnd::Failed(Arc::from("zstd context")),
    };
    loop {
        let raw = match frames.read_frame(&mut &sh.stream) {
            Ok(raw) => raw,
            Err(EnvelopeError::Proto(
                ProtoError::Eof | ProtoError::Truncated | ProtoError::Io(_),
            )) => return SessionEnd::ClientClosed,
            Err(e) => return SessionEnd::BadMessage(Arc::from(e.to_string())),
        };
        let msg = match frames.decode_client(&raw) {
            Ok(msg) => msg,
            Err(e) => return SessionEnd::BadMessage(Arc::from(e.to_string())),
        };
        if sh.stopping() {
            return SessionEnd::ClientClosed;
        }
        if !retry_refused(sh) {
            return SessionEnd::EngineEnded;
        }
        let going = match msg {
            ClientMsg::Ack { frame } => {
                if lock(&sh.queues)
                    .ack
                    .replace((frame, Instant::now()))
                    .is_some()
                {
                    return SessionEnd::BadMessage(Arc::from(
                        "a second Ack before the writer applied the first",
                    ));
                }
                sh.ring_writer();
                true
            }
            ClientMsg::SetDemand(demand) => {
                sh.port.set_demand(demand);
                true
            }
            ClientMsg::Command(cmd) => command(sh, &mut out, cmd),
            ClientMsg::JournalOpen { stream, query } => journal_open(sh, &mut out, stream, query),
            ClientMsg::JournalCtl { stream, control } => {
                let q = lock(&sh.queues);
                if let Some(h) = q.streams.get(&stream).and_then(|s| s.handle.as_ref()) {
                    match control {
                        JournalControl::LoadOlder(from) => h.load_older(from),
                        JournalControl::SetFollow(on) => h.set_follow(on),
                    }
                }
                true
            }
            ClientMsg::JournalClose { stream } => {
                journal_close(sh, stream);
                true
            }
            ClientMsg::GetHostSettings => {
                let current = sh.port.host_settings();
                sh.queue_reply(&mut out, &ServerMsg::HostSettings(current))
            }
            ClientMsg::SetHostSettings { base_rev, changes } => {
                let reply = sh.port.set_host_settings(base_rev, changes);
                sh.queue_reply(&mut out, &ServerMsg::SetHostSettings(reply))
            }
        };
        if !going {
            // The reason is set already.
            return SessionEnd::ClientClosed;
        }
    }
}

/// The request a command starts, if it is one that ends with a `RequestEvent`.
fn request_id(cmd: &Command) -> Option<(IdKind, u64)> {
    match cmd {
        Command::Action(req) => Some((IdKind::Action, req.id.0)),
        Command::StringScan { scan, .. } => Some((IdKind::Scan, scan.0)),
        Command::HandleSearch { search, .. } => Some((IdKind::Search, search.0)),
        Command::Device(DeviceCommand::Eject { seq, .. }) => Some((IdKind::Eject, *seq)),
        Command::HistoryQuery { id, .. } => Some((IdKind::History, *id)),
        _ => None,
    }
}

/// Table queries are session state: the newest one must reach the engine.
fn is_state(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::SetTableQuery(_) | Command::SetSysTableQuery(_)
    )
}

/// Whether `b` replaces `a`: both the table query, or both queries of one system table.
fn same_state(a: &Command, b: &Command) -> bool {
    match (a, b) {
        (Command::SetTableQuery(_), Command::SetTableQuery(_)) => true,
        (Command::SetSysTableQuery(a), Command::SetSysTableQuery(b)) => a.table == b.table,
        _ => false,
    }
}

/// Offers the table queries the engine refused before to it again. Called by the reader before
/// each client message and by the pump on each pass. False when the engine stopped.
fn retry_refused<P: Port>(sh: &Shared<P>) -> bool {
    let mut refused = lock(&sh.refused);
    for cmd in std::mem::take(&mut *refused) {
        match sh.port.send(cmd.clone()) {
            Ok(()) => {}
            Err(CommandError::Busy) => refused.push(cmd),
            Err(CommandError::EngineStopped) => {
                drop(refused);
                sh.finish(SessionEnd::EngineEnded);
                return false;
            }
        }
    }
    true
}

/// Passes a command to the port. False when the session is closing.
fn command<P: Port>(sh: &Shared<P>, frames: &mut FrameWriter, cmd: Command) -> bool {
    if is_state(&cmd) {
        return table_query(sh, cmd);
    }
    let request = request_id(&cmd);
    match sh.port.send(cmd) {
        Ok(()) => true,
        Err(CommandError::Busy) => match request {
            Some((kind, id)) => queue_busy(sh, frames, kind, id),
            None => {
                sh.commands_dropped.fetch_add(1, Ordering::Relaxed);
                true
            }
        },
        Err(CommandError::EngineStopped) => {
            sh.finish(SessionEnd::EngineEnded);
            false
        }
    }
}

/// Passes a table query to the port. It replaces the refused query of its table, if any, and
/// is kept in its place when the engine refuses it too. False when the session is closing.
fn table_query<P: Port>(sh: &Shared<P>, cmd: Command) -> bool {
    let mut refused = lock(&sh.refused);
    refused.retain(|old| !same_state(old, &cmd));
    match sh.port.send(cmd.clone()) {
        Ok(()) => true,
        Err(CommandError::Busy) => {
            refused.push(cmd);
            true
        }
        Err(CommandError::EngineStopped) => {
            drop(refused);
            sh.finish(SessionEnd::EngineEnded);
            false
        }
    }
}

/// Ends a request the engine refused with `Busy`: an action with a `Rejected(Busy)` summary and
/// then its end, any other kind with its end.
fn queue_busy<P: Port>(sh: &Shared<P>, frames: &mut FrameWriter, kind: IdKind, id: u64) -> bool {
    let mut msgs = Vec::with_capacity(2);
    if kind == IdKind::Action {
        msgs.push(ServerMsg::Action(ActionEvent {
            id: ActionId(id),
            target: None,
            state: ActionState::Rejected(RejectReason::Busy),
        }));
    }
    msgs.push(ServerMsg::Request(RequestEvent {
        kind,
        id,
        end: RequestEnd::Busy,
    }));
    let mut entries = Vec::with_capacity(msgs.len());
    for msg in msgs {
        let Some(entry) = sh.entry(frames, msg) else {
            return false;
        };
        entries.push(entry);
    }
    sh.queue_ordered(entries)
}

/// Opens a journal stream. A stream id in use, or one stream too many, gets an error event and
/// nothing opens. False when the session is closing.
fn journal_open<P: Port>(
    sh: &Shared<P>,
    frames: &mut FrameWriter,
    stream: StreamId,
    query: JournalQuery,
) -> bool {
    let refusal = {
        let q = lock(&sh.queues);
        if q.streams.contains_key(&stream) {
            Some("this journal stream is open already".to_owned())
        } else if q.streams.len() >= JOURNAL_STREAMS_MAX {
            Some(format!(
                "{JOURNAL_STREAMS_MAX} journal streams are open in this window"
            ))
        } else {
            None
        }
    };
    if let Some(text) = refusal {
        let event = JournalEvent::Error(journal_error(&text));
        return sh.queue_reply(frames, &ServerMsg::Journal { stream, event });
    }
    let handle = sh.port.journal(query);
    let events = handle.events();
    let mut q = lock(&sh.queues);
    let generation = q.next_generation;
    q.next_generation += 1;
    q.streams.insert(
        stream,
        Stream {
            generation,
            handle: Some(handle),
            events: Some(events),
            queue: VecDeque::new(),
            bytes: 0,
        },
    );
    drop(q);
    let _ = sh.pump_bell.try_send(());
    true
}

/// Closes a journal stream and drops what it had queued. The id may be opened again at once.
/// Waits for a write of the stream's message that has started, at most the write timeout.
fn journal_close<P: Port>(sh: &Shared<P>, stream: StreamId) {
    let gate = lock(&sh.journal_gate);
    let closed = lock(&sh.queues).streams.remove(&stream);
    drop(gate);
    // Stops its journalctl, outside the lock.
    drop(closed);
    let _ = sh.pump_bell.try_send(());
}

// ---- Pump ----

/// What the pump woke up for.
enum Got {
    /// The session is closing.
    Stop,
    /// The journal streams changed.
    Bell,
    /// `TICK` passed with nothing new.
    Tick,
    Wake,
    Event(ActionEvent),
    End(RequestEvent),
    Journal(StreamKey, JournalEvent),
    /// The engine closed the stream's events.
    JournalDone(StreamKey),
    /// One of the port's channels closed: the engine ended the session.
    PortEnded,
}

/// Unparks the pump thread when a channel it waits on has news.
struct Unpark(thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `futures` in order until one is ready and returns its output, parking the thread in
/// between. `None` once `wait` passed with none ready. A wake between a poll and the park leaves
/// the park token set, so none is lost.
fn first_ready<T>(
    futures: &mut [Pin<Box<dyn Future<Output = T> + '_>>],
    cx: &mut Context<'_>,
    wait: Duration,
) -> Option<T> {
    let deadline = Instant::now() + wait;
    loop {
        for f in futures.iter_mut() {
            if let Poll::Ready(v) = f.as_mut().poll(cx) {
                return Some(v);
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        thread::park_timeout(left);
    }
}

/// Moves what the engine sends into the session's queues until the session closes, and offers
/// refused table queries to the engine again on each pass, at least every `TICK`.
fn pump_loop<P: Port>(sh: &Shared<P>, bell: &async_channel::Receiver<()>) {
    let Ok(mut frames) = FrameWriter::new() else {
        sh.finish(SessionEnd::Failed(Arc::from("zstd context")));
        return;
    };
    let wake = sh.port.wake();
    let events = sh.port.action_events();
    let requests = sh.port.request_events();
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    while !sh.stopping() {
        // On each pass, so a refused table query gets in once the engine has room, even when
        // the client sends nothing more.
        if !retry_refused(sh) {
            return;
        }
        // Keyed by opening: the client may close a stream and open its id again before this
        // pass ends, and what is taken here then belongs to the stream it closed.
        let journals = sh.journal_receivers();
        let got = {
            let mut futures: Vec<Pin<Box<dyn Future<Output = Got> + '_>>> =
                Vec::with_capacity(4 + journals.len());
            futures.push(Box::pin(async {
                bell.recv().await.map_or(Got::Stop, |()| Got::Bell)
            }));
            futures.push(Box::pin(async {
                wake.recv().await.map_or(Got::PortEnded, |()| Got::Wake)
            }));
            futures.push(Box::pin(async {
                requests.recv().await.map_or(Got::PortEnded, Got::End)
            }));
            futures.push(Box::pin(async {
                events.recv().await.map_or(Got::PortEnded, Got::Event)
            }));
            for (key, rx) in &journals {
                let key = *key;
                futures.push(Box::pin(async move {
                    rx.recv()
                        .await
                        .map_or(Got::JournalDone(key), |ev| Got::Journal(key, ev))
                }));
            }
            first_ready(&mut futures, &mut cx, TICK).unwrap_or(Got::Tick)
        };
        let mut ends = Vec::new();
        let mut first_event = None;
        let mut port_ended = false;
        match got {
            Got::Stop => return,
            Got::Bell | Got::Tick => {}
            Got::Wake => sh.ring_writer(),
            Got::Event(ev) => first_event = Some(ev),
            Got::End(end) => ends.push(end),
            Got::Journal(key, ev) => {
                sh.queue_journal(&mut frames, key, ev);
                if let Some((_, rx)) = journals.iter().find(|(k, _)| *k == key) {
                    while let Ok(ev) = rx.try_recv() {
                        sh.queue_journal(&mut frames, key, ev);
                    }
                }
            }
            Got::JournalDone(key) => sh.journal_done(key),
            Got::PortEnded => port_ended = true,
        }
        // Ends first, then events: every end taken here was sent after its request's events,
        // so those events are in the channel by now or were taken before. Queued in the other
        // order, each end follows its request's events.
        while let Ok(end) = requests.try_recv() {
            ends.push(end);
        }
        let mut msgs: Vec<ServerMsg> = first_event.into_iter().map(ServerMsg::Action).collect();
        while let Ok(ev) = events.try_recv() {
            msgs.push(ServerMsg::Action(ev));
        }
        msgs.extend(ends.into_iter().map(ServerMsg::Request));
        if !msgs.is_empty() {
            let mut entries = Vec::with_capacity(msgs.len());
            for msg in msgs {
                let Some(entry) = sh.entry(&mut frames, msg) else {
                    return;
                };
                entries.push(entry);
            }
            if !sh.queue_ordered(entries) {
                return;
            }
        }
        if port_ended {
            sh.finish(SessionEnd::EngineEnded);
            return;
        }
    }
}

// ---- Writer ----

/// What the writer sends next.
enum Next {
    Bytes(Vec<u8>, Item),
    /// A journal message and the opening of its stream it belongs to.
    Journal(Vec<u8>, StreamKey),
    Snapshot,
    Nothing,
}

/// The next message in the order actions, replies, journal, snapshot. A snapshot that is due
/// goes first once the floor has passed.
fn pick(q: &mut Queues, snapshot_due: bool, floor_passed: bool) -> Next {
    if snapshot_due && floor_passed {
        return Next::Snapshot;
    }
    if let Some(entry) = q.actions.pop_front() {
        return Next::Bytes(entry.bytes, entry.what);
    }
    if let Some(bytes) = q.replies.pop_front() {
        q.reply_bytes -= bytes.len();
        return Next::Bytes(bytes, Item::Reply);
    }
    let after = q.journal_turn.unwrap_or(StreamId(0));
    let turn = q
        .streams
        .range((Bound::Excluded(after), Bound::Unbounded))
        .chain(q.streams.range(..=after))
        .find(|(_, s)| !s.queue.is_empty())
        .map(|(id, _)| *id);
    if let Some(id) = turn
        && let Some(s) = q.streams.get_mut(&id)
        && let Some(bytes) = s.queue.pop_front()
    {
        s.bytes -= bytes.len();
        let key = StreamKey {
            id,
            generation: s.generation,
        };
        q.journal_turn = Some(id);
        return Next::Journal(bytes, key);
    }
    if snapshot_due {
        return Next::Snapshot;
    }
    Next::Nothing
}

/// Applies the waiting `Ack`, then checks the in-flight frame's deadline. An `Ack` the reader
/// read after the deadline is late, even when the writer gets to it only now.
fn take_ack<P: Port>(
    sh: &Shared<P>,
    encoder: &mut DeltaEncoder,
    sent_at: &mut Option<Instant>,
) -> Result<(), SessionEnd> {
    let waiting = lock(&sh.queues).ack.take();
    if let Some((frame, read)) = waiting {
        if let (Some(in_flight), Some(at)) = (encoder.in_flight(), *sent_at)
            && read.saturating_duration_since(at) >= sh.config.ack_timeout
        {
            return Err(SessionEnd::AckTimeout { frame: in_flight });
        }
        encoder
            .on_ack(frame)
            .map_err(|e| SessionEnd::BadMessage(Arc::from(e.to_string())))?;
        *sent_at = None;
        observe!(sh, Step::Acked(frame));
    }
    match (encoder.in_flight(), *sent_at) {
        (Some(frame), Some(at)) if at.elapsed() >= sh.config.ack_timeout => {
            Err(SessionEnd::AckTimeout { frame })
        }
        _ => Ok(()),
    }
}

/// Writes a journal message the writer took off its stream's queue, if that opening of the
/// stream is still open. The stream may have closed, and its id opened again, since then.
fn write_journal<P: Port>(
    sh: &Shared<P>,
    bytes: &[u8],
    key: StreamKey,
    ack_due: Option<(u64, Instant)>,
) -> Result<(), SessionEnd> {
    let _gate = lock(&sh.journal_gate);
    if lock(&sh.queues).stream_mut(key).is_none() {
        observe!(sh, Step::JournalClosed(key.id));
        return Ok(());
    }
    observe!(sh, Step::Writing(Item::Journal(key.id)));
    sh.write(bytes, ack_due)
}

/// Writes what the queues hold and the snapshot frames, until the session closes.
fn write_loop<P: Port>(sh: &Shared<P>, bell: &mpsc::Receiver<()>) {
    let Ok(mut frames) = FrameWriter::new() else {
        sh.finish(SessionEnd::Failed(Arc::from("zstd context")));
        return;
    };
    let mut encoder = DeltaEncoder::default();
    let start = Instant::now();
    let mut last_snapshot = start;
    // The publish the last frame was built from, and when that frame was written.
    let mut last_built: Option<Arc<Snapshot>> = None;
    let mut sent_at: Option<Instant> = None;
    let mut out = Vec::new();
    loop {
        if sh.stopping() {
            return;
        }
        if sh.port.is_ended() {
            sh.finish(SessionEnd::EngineEnded);
            return;
        }
        loop {
            // Before each message, so a steady flow of other messages cannot put off the Ack
            // deadline.
            if let Err(end) = take_ack(sh, &mut encoder, &mut sent_at) {
                sh.finish(end);
                return;
            }
            // `take_ack` applied any waiting Ack, so the next write stops at the in-flight
            // frame's deadline unless an Ack comes in time.
            let ack_due = encoder
                .in_flight()
                .zip(sent_at)
                .map(|(frame, at)| (frame, at + sh.config.ack_timeout));
            let latest = sh.port.latest();
            // Nothing before the first publish: the client keeps its last data until then.
            let snapshot_due = sh.config.snapshots
                && latest.sequence != 0
                && encoder.in_flight().is_none()
                && last_built.as_ref().is_none_or(|b| !Arc::ptr_eq(b, &latest));
            let floor_passed = last_snapshot.elapsed() >= sh.config.snapshot_floor;
            let next = pick(&mut lock(&sh.queues), snapshot_due, floor_passed);
            let written = match next {
                Next::Nothing => break,
                Next::Bytes(bytes, _what) => {
                    observe!(sh, Step::Writing(_what));
                    sh.write(&bytes, ack_due)
                }
                Next::Journal(bytes, key) => write_journal(sh, &bytes, key, ack_due),
                Next::Snapshot => {
                    let now_ns = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
                    let built = match encoder.next_frame(&latest, now_ns) {
                        Ok(Some(built)) => built,
                        Ok(None) => break,
                        Err(e) => {
                            sh.finish(SessionEnd::Failed(Arc::from(e.to_string())));
                            return;
                        }
                    };
                    if built.trimmed {
                        sh.frames_trimmed.fetch_add(1, Ordering::Relaxed);
                    }
                    let _frame = built.frame.frame;
                    out.clear();
                    if let Err(e) =
                        frames.encode_server(&ServerMsg::Snapshot(Box::new(built.frame)), &mut out)
                    {
                        sh.finish(SessionEnd::Failed(Arc::from(e.to_string())));
                        return;
                    }
                    observe!(sh, Step::Writing(Item::Snapshot { frame: _frame }));
                    // No frame is in flight before this one, so only the write deadline holds.
                    let written = sh.write(&out, None);
                    let now = Instant::now();
                    sent_at = Some(now);
                    last_snapshot = now;
                    last_built = Some(latest);
                    written
                }
            };
            if let Err(end) = written {
                sh.finish(end);
                return;
            }
            if sh.stopping() {
                return;
            }
        }
        let wait = match sent_at {
            Some(at) => TICK.min(sh.config.ack_timeout.saturating_sub(at.elapsed())),
            None => TICK,
        };
        match bell.recv_timeout(wait) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(bytes: u8, progress: bool) -> Entry {
        Entry {
            bytes: vec![bytes],
            progress,
            what: Item::Reply,
        }
    }

    fn stream(queued: &[u8]) -> Stream {
        Stream {
            generation: 0,
            handle: None,
            events: None,
            queue: queued.iter().map(|b| vec![*b]).collect(),
            bytes: queued.len(),
        }
    }

    fn order(q: &mut Queues, snapshot_due: bool, floor_passed: bool) -> Vec<u8> {
        let mut got = Vec::new();
        loop {
            match pick(q, snapshot_due, floor_passed) {
                Next::Bytes(b, _) | Next::Journal(b, _) => got.push(b[0]),
                // A snapshot is one byte 0 here, and the writer sends one per frame.
                Next::Snapshot => {
                    got.push(0);
                    return got;
                }
                Next::Nothing => return got,
            }
        }
    }

    #[test]
    fn writer_order_is_actions_replies_journal_snapshot() {
        let mut q = Queues::default();
        q.actions.push_back(entry(1, false));
        q.actions.push_back(entry(2, true));
        q.replies.push_back(vec![3]);
        q.reply_bytes = 1;
        q.streams.insert(StreamId(1), stream(&[4, 6]));
        q.streams.insert(StreamId(2), stream(&[5]));
        // The journal streams take turns.
        assert_eq!(order(&mut q, true, false), vec![1, 2, 3, 4, 5, 6, 0]);
        assert_eq!(q.reply_bytes, 0);
        assert!(q.streams.values().all(|s| s.bytes == 0));
    }

    #[test]
    fn a_due_snapshot_goes_first_once_the_floor_passed() {
        let mut q = Queues::default();
        q.actions.push_back(entry(1, false));
        assert_eq!(order(&mut q, true, true), vec![0]);
        assert_eq!(q.actions.len(), 1, "the action waits for the next pick");
        // Without a snapshot due, the floor changes nothing.
        assert_eq!(order(&mut q, false, true), vec![1]);
    }

    /// A port with journal streams only. Keeps each stream's event sender, in the order opened.
    #[derive(Default)]
    struct Journals(Mutex<Vec<Option<async_channel::Sender<JournalEvent>>>>);

    impl Port for Arc<Journals> {
        fn latest(&self) -> Arc<Snapshot> {
            unreachable!("no snapshots in this test")
        }

        fn wake(&self) -> Receiver<()> {
            async_channel::bounded(1).1
        }

        fn action_events(&self) -> Receiver<ActionEvent> {
            async_channel::bounded(1).1
        }

        fn request_events(&self) -> Receiver<RequestEvent> {
            async_channel::bounded(1).1
        }

        fn set_demand(&self, _demand: Demand) {}

        fn send(&self, _cmd: Command) -> Result<(), CommandError> {
            Ok(())
        }

        fn journal(&self, _query: JournalQuery) -> JournalHandle {
            let (events_tx, events_rx) = async_channel::unbounded();
            lock(&self.0).push(Some(events_tx));
            JournalHandle::new(events_rx, async_channel::unbounded().0)
        }

        fn is_ended(&self) -> bool {
            false
        }

        fn host_settings(&self) -> HostSettings {
            unreachable!("no host settings in this test")
        }

        fn set_host_settings(
            &self,
            _base_rev: u64,
            _changes: HostSettingsChanges,
        ) -> SetHostSettingsReply {
            unreachable!("no host settings in this test")
        }
    }

    fn query() -> JournalQuery {
        JournalQuery {
            unit: tl_model::table::UnitRef {
                manager: tl_model::table::Manager::User,
                name: Arc::from("a.service"),
            },
            follow: true,
        }
    }

    fn old_event() -> JournalEvent {
        JournalEvent::Access(tl_model::journal::JournalAccess::All)
    }

    fn new_event() -> JournalEvent {
        JournalEvent::Access(tl_model::journal::JournalAccess::OwnOnly)
    }

    #[test]
    fn a_reopened_journal_stream_takes_nothing_from_the_one_before() {
        let engine = Arc::new(Journals::default());
        let (socket, _client) = UnixStream::pair().unwrap();
        let (sh, _writer_bell, _pump_bell) =
            Shared::new(socket, Arc::clone(&engine), SessionConfig::default());
        let mut frames = FrameWriter::new().unwrap();
        let id = StreamId(1);

        assert!(journal_open(&sh, &mut frames, id, query()));
        // The pump takes the first opening's receiver on a pass, and the old journalctl has
        // events on the way.
        let pass = sh.journal_receivers();
        let [(old, old_rx)] = pass.as_slice() else {
            panic!("one stream is open");
        };
        let old_tx = lock(&engine.0)[0].clone().unwrap();
        old_tx.try_send(old_event()).unwrap();
        old_tx.try_send(old_event()).unwrap();

        // Before that pass ends, the client closes the stream and opens its id again.
        journal_close(&sh, id);
        assert!(journal_open(&sh, &mut frames, id, query()));
        let reopened = sh.journal_receivers();
        let [(new, _)] = reopened.as_slice() else {
            panic!("one stream is open");
        };
        assert_eq!(new.id, old.id);
        assert_ne!(
            new.generation, old.generation,
            "the new opening has its own generation"
        );

        // The pump goes on with what it took: the old events, then the old stream's end.
        while let Ok(ev) = old_rx.try_recv() {
            sh.queue_journal(&mut frames, *old, ev);
        }
        lock(&engine.0)[0] = None;
        drop(old_tx);
        assert!(old_rx.try_recv().is_err(), "the old journalctl is done");
        sh.journal_done(*old);

        {
            let q = lock(&sh.queues);
            let s = &q.streams[&id];
            assert!(s.queue.is_empty(), "no old event reached the new stream");
            assert_eq!(s.bytes, 0);
            assert!(s.events.is_some(), "the old end did not end the new stream");
            assert!(s.handle.is_some());
        }

        // The new opening's own events still get in.
        let new_tx = lock(&engine.0)[1].clone().unwrap();
        new_tx.try_send(new_event()).unwrap();
        let pass = sh.journal_receivers();
        let [(key, rx)] = pass.as_slice() else {
            panic!("the new stream still waits for events");
        };
        assert_eq!(key, new);
        sh.queue_journal(&mut frames, *key, rx.try_recv().unwrap());
        assert_eq!(lock(&sh.queues).streams[&id].queue.len(), 1);
    }

    /// What the client end of the socket has received so far.
    fn received(client: &mut UnixStream) -> Vec<u8> {
        use std::io::Read;
        client.set_nonblocking(true).unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match client.read(&mut buf) {
                Ok(0) => return got,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return got,
                Err(e) => panic!("read: {e}"),
            }
        }
    }

    #[test]
    fn the_writer_drops_a_taken_journal_message_whose_stream_was_reopened() {
        let engine = Arc::new(Journals::default());
        let (socket, mut client) = UnixStream::pair().unwrap();
        let (sh, _writer_bell, _pump_bell) =
            Shared::new(socket, Arc::clone(&engine), SessionConfig::default());
        let mut frames = FrameWriter::new().unwrap();
        let id = StreamId(1);

        assert!(journal_open(&sh, &mut frames, id, query()));
        let pass = sh.journal_receivers();
        let [(old, _)] = pass.as_slice() else {
            panic!("one stream is open");
        };
        let old = *old;
        sh.queue_journal(&mut frames, old, old_event());
        // The writer takes the old event off the queue, with its opening.
        let Next::Journal(old_bytes, taken) = pick(&mut lock(&sh.queues), false, false) else {
            panic!("the old event is next");
        };
        assert_eq!(taken, old);

        // Before the writer writes it, the client closes the stream and opens its id again.
        journal_close(&sh, id);
        assert!(journal_open(&sh, &mut frames, id, query()));
        write_journal(&sh, &old_bytes, taken, None).unwrap();
        assert!(
            received(&mut client).is_empty(),
            "nothing of the old opening went out after its close"
        );

        // The new opening's events are written.
        let pass = sh.journal_receivers();
        let [(new, _)] = pass.as_slice() else {
            panic!("one stream is open");
        };
        let new = *new;
        sh.queue_journal(&mut frames, new, new_event());
        let Next::Journal(new_bytes, taken) = pick(&mut lock(&sh.queues), false, false) else {
            panic!("the new event is next");
        };
        assert_eq!(taken, new);
        write_journal(&sh, &new_bytes, taken, None).unwrap();
        assert_eq!(received(&mut client), new_bytes);
    }
}
