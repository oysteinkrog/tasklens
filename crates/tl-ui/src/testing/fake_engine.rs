//! `FakeEngine`: the `EngineClient` every UI test uses instead of the real engine (ADR-013).
//!
//! Tests publish scripted snapshots and read back every demand, settings value, command and
//! journal query the UI sent. The channels match the real engine (ADR-005, ADR-018): a one-token
//! wake channel, a 256-slot action event channel and an unbounded request end channel.
//!
//! Host settings (ADR-018 Settings) start known, at revision 1 with the defaults, as from an
//! engine that has given them. `set_host_settings` checks the revision as the engine does and
//! publishes the result; `change_host_settings_elsewhere` changes them as another window would,
//! without a publish, so a test can make a write conflict.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::journal::{JournalControl, JournalEvent, JournalHandle, JournalQuery};
use tl_model::remote_proto::{
    HostSettings, HostSettingsChanges, HostSettingsValues, SetHostSettingsReply,
};
use tl_model::request::RequestEvent;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tl_model::time::BootId;

/// A scripted engine. Never blocks and never returns `CommandError::EngineStopped`.
pub struct FakeEngine {
    inner: Arc<Mutex<FakeState>>,
    wake_tx: async_channel::Sender<()>,
    wake_rx: async_channel::Receiver<()>,
    ev_tx: async_channel::Sender<ActionEvent>,
    ev_rx: async_channel::Receiver<ActionEvent>,
    req_tx: async_channel::Sender<RequestEvent>,
    req_rx: async_channel::Receiver<RequestEvent>,
    /// `EngineClient::generation`, changed by `restart`.
    generation: AtomicU64,
}

struct FakeState {
    latest: Arc<Snapshot>,
    demands: Vec<Demand>,
    settings: Vec<EngineSettings>,
    commands: Vec<Command>,
    journals: Vec<JournalQuery>,
    /// Every `set_host_settings` call, in call order.
    host_writes: Vec<(u64, HostSettingsChanges)>,
    /// Host settings changed elsewhere and not published yet. `None`: the latest snapshot's.
    host_elsewhere: Option<HostSettings>,
    /// The engine ends of each journal handle. Kept so the handle's channels stay open.
    journal_ends: Vec<(
        async_channel::Sender<JournalEvent>,
        async_channel::Receiver<JournalControl>,
    )>,
    busy: bool,
}

impl FakeEngine {
    /// `latest` is `empty_snapshot()` with sequence 0.
    pub fn new() -> Arc<Self> {
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (ev_tx, ev_rx) = async_channel::bounded(256);
        let (req_tx, req_rx) = async_channel::unbounded();
        Arc::new(Self {
            inner: Arc::new(Mutex::new(FakeState {
                latest: Arc::new(empty_snapshot()),
                demands: Vec::new(),
                settings: Vec::new(),
                commands: Vec::new(),
                journals: Vec::new(),
                host_writes: Vec::new(),
                host_elsewhere: None,
                journal_ends: Vec::new(),
                busy: false,
            })),
            wake_tx,
            wake_rx,
            ev_tx,
            ev_rx,
            req_tx,
            req_rx,
            generation: AtomicU64::new(0),
        })
    }

    fn state(&self) -> MutexGuard<'_, FakeState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores `snap`, then sends one wake token. A full wake channel is ignored, so this never
    /// blocks (ADR-005 publish order).
    pub fn publish(&self, snap: Snapshot) {
        self.state().latest = Arc::new(snap);
        let _ = self.wake_tx.try_send(());
    }

    /// Clones the latest snapshot, adds 1 to its sequence, applies `f` and publishes the result.
    pub fn publish_with(&self, f: impl FnOnce(&mut Snapshot)) {
        let mut snap = Snapshot::clone(&self.state().latest);
        snap.sequence += 1;
        f(&mut snap);
        self.publish(snap);
    }

    /// A new engine generation, as after a reconnect: changes `generation`, then publishes
    /// `snap`, whose component versions start over.
    pub fn restart(&self, snap: Snapshot) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.publish(snap);
    }

    /// Changes `generation` and publishes nothing.
    pub fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Queues one action event. A full channel drops the event.
    pub fn emit(&self, ev: ActionEvent) {
        let _ = self.ev_tx.try_send(ev);
    }

    /// Queues one request end. The channel is unbounded, so it is never dropped.
    pub fn end_request(&self, ev: RequestEvent) {
        let _ = self.req_tx.try_send(ev);
    }

    /// Every `set_demand` value, in call order.
    pub fn demands(&self) -> Vec<Demand> {
        self.state().demands.clone()
    }

    /// Every `set_settings` value, in call order.
    pub fn settings(&self) -> Vec<EngineSettings> {
        self.state().settings.clone()
    }

    /// Every accepted command, in call order.
    pub fn commands(&self) -> Vec<Command> {
        self.state().commands.clone()
    }

    /// Every `journal` query, in call order.
    pub fn journals(&self) -> Vec<JournalQuery> {
        self.state().journals.clone()
    }

    /// The engine ends of the `i`th journal handle: the event sender a test feeds and the
    /// control receiver it reads. The event channel holds 1 event.
    pub fn journal_end(
        &self,
        i: usize,
    ) -> Option<(
        async_channel::Sender<JournalEvent>,
        async_channel::Receiver<JournalControl>,
    )> {
        self.state().journal_ends.get(i).cloned()
    }

    /// Every `set_host_settings` call: the base revision and the changes, in call order.
    pub fn host_writes(&self) -> Vec<(u64, HostSettingsChanges)> {
        self.state().host_writes.clone()
    }

    /// Publishes the latest snapshot with host settings at the next revision, `f` applied to
    /// their values.
    pub fn publish_host_settings(&self, f: impl FnOnce(&mut HostSettingsValues)) {
        self.publish_with(|s| {
            let host = Arc::make_mut(&mut s.host_settings);
            host.rev += 1;
            f(&mut host.values);
        });
    }

    /// Changes the host settings as another window would: the next revision, `f` applied, not
    /// published. A write from the published revision then conflicts.
    pub fn change_host_settings_elsewhere(&self, f: impl FnOnce(&mut HostSettingsValues)) {
        let mut state = self.state();
        let mut next = state
            .host_elsewhere
            .clone()
            .unwrap_or_else(|| (*state.latest.host_settings).clone());
        next.rev += 1;
        f(&mut next.values);
        state.host_elsewhere = Some(next);
    }

    /// While `busy` is true, `send` returns `Err(CommandError::Busy)` and records nothing.
    pub fn set_busy(&self, busy: bool) {
        self.state().busy = busy;
    }
}

impl EngineClient for FakeEngine {
    fn latest(&self) -> Arc<Snapshot> {
        Arc::clone(&self.state().latest)
    }

    fn wake(&self) -> async_channel::Receiver<()> {
        self.wake_rx.clone()
    }

    fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        self.ev_rx.clone()
    }

    fn request_events(&self) -> async_channel::Receiver<RequestEvent> {
        self.req_rx.clone()
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn set_demand(&self, demand: Demand) {
        self.state().demands.push(demand);
    }

    fn set_settings(&self, settings: EngineSettings) {
        self.state().settings.push(settings);
    }

    /// Records the call, then answers as the engine does: `Conflict` unless `base_rev` is the
    /// current revision, else the next revision. Either way the current settings are published.
    /// A `generation` that is not `generation()` gets a closed channel, as in every client, and
    /// is not recorded.
    fn set_host_settings(
        &self,
        generation: u64,
        base_rev: u64,
        changes: HostSettingsChanges,
    ) -> Result<async_channel::Receiver<SetHostSettingsReply>, CommandError> {
        if generation != self.generation() {
            return Ok(async_channel::bounded(1).1);
        }
        let (reply, current) = {
            let mut state = self.state();
            if state.busy {
                return Err(CommandError::Busy);
            }
            state.host_writes.push((base_rev, changes.clone()));
            let current = state
                .host_elsewhere
                .take()
                .unwrap_or_else(|| (*state.latest.host_settings).clone());
            if current.rev == 0 || base_rev != current.rev {
                let reply = SetHostSettingsReply::Conflict {
                    current: current.clone(),
                };
                (reply, current)
            } else {
                let next = HostSettings {
                    rev: current.rev + 1,
                    values: current.values.with_changes(&changes),
                    read_only: current.read_only,
                };
                (SetHostSettingsReply::Ack { rev: next.rev }, next)
            }
        };
        if *self.latest().host_settings != current {
            self.publish_with(|s| s.host_settings = Arc::new(current));
        }
        let (tx, rx) = async_channel::bounded(1);
        let _ = tx.try_send(reply);
        Ok(rx)
    }

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        let mut state = self.state();
        if state.busy {
            return Err(CommandError::Busy);
        }
        state.commands.push(cmd);
        Ok(())
    }

    fn journal(&self, query: JournalQuery) -> JournalHandle {
        let (events_tx, events) = async_channel::bounded(1);
        let (control, control_rx) = async_channel::bounded(1);
        let mut state = self.state();
        state.journals.push(query);
        state.journal_ends.push((events_tx, control_rx));
        JournalHandle::new(events, control)
    }
}

/// Every component at its default, every header `WarmingUp`, sequence 0. The host settings are
/// the defaults at revision 1, as from an engine that has given them.
pub fn empty_snapshot() -> Snapshot {
    let mut snap = Snapshot::empty(BootId(0), 0);
    snap.host_settings = Arc::new(HostSettings {
        rev: 1,
        ..HostSettings::default()
    });
    snap
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::health::SourceId;

    #[test]
    fn publish_wakes_and_latest() {
        let engine = FakeEngine::new();
        assert_eq!(engine.latest().sequence, 0);
        let mut snap = empty_snapshot();
        snap.sequence = 7;
        engine.publish(snap);
        assert!(engine.wake().try_recv().is_ok());
        assert_eq!(engine.latest().sequence, 7);

        engine.publish_with(|s| s.published_ns = 5);
        assert_eq!(engine.latest().sequence, 8);
        assert_eq!(engine.latest().published_ns, 5);
    }

    #[test]
    fn publish_never_blocks() {
        let engine = FakeEngine::new();
        for i in 1..=1_000 {
            let mut snap = empty_snapshot();
            snap.sequence = i;
            engine.publish(snap);
        }
        assert_eq!(engine.latest().sequence, 1_000);
        let wake = engine.wake();
        assert_eq!(wake.len(), 1);
        assert!(wake.try_recv().is_ok());
        assert!(wake.try_recv().is_err());
    }

    #[test]
    fn records_commands_and_demand() {
        let engine = FakeEngine::new();
        engine
            .send(Command::RefreshNow(SourceId::Cpu))
            .expect("not busy");
        engine.send(Command::ResetBaselines).expect("not busy");
        let commands = engine.commands();
        assert_eq!(commands.len(), 2);
        assert!(matches!(commands[0], Command::RefreshNow(SourceId::Cpu)));
        assert!(matches!(commands[1], Command::ResetBaselines));

        let first = Demand::default();
        let second = Demand {
            process_table: !first.process_table,
            ..Demand::default()
        };
        engine.set_demand(first.clone());
        engine.set_demand(second.clone());
        assert_eq!(engine.demands(), vec![first, second]);

        let settings = EngineSettings {
            interval_ms: 2000,
            ..EngineSettings::default()
        };
        engine.set_settings(EngineSettings::default());
        engine.set_settings(settings.clone());
        let recorded = engine.settings();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[1], settings);
        assert_ne!(recorded[0], recorded[1]);
    }

    #[test]
    fn busy_rejects() {
        let engine = FakeEngine::new();
        engine.set_busy(true);
        assert_eq!(
            engine.send(Command::RefreshNow(SourceId::Cpu)),
            Err(CommandError::Busy)
        );
        assert!(engine.commands().is_empty());

        engine.set_busy(false);
        assert_eq!(engine.send(Command::ResetBaselines), Ok(()));
        assert_eq!(engine.commands().len(), 1);
    }
}
