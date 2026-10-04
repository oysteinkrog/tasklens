//! `FakeEngine`: the `EngineClient` every UI test uses instead of the real engine (ADR-013).
//!
//! Tests publish scripted snapshots and read back every demand, settings value, command and
//! journal query the UI sent. The channels match the real engine (ADR-005): a one-token wake
//! channel and a 256-slot action event channel.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::journal::{JournalControl, JournalEvent, JournalHandle, JournalQuery};
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
}

struct FakeState {
    latest: Arc<Snapshot>,
    demands: Vec<Demand>,
    settings: Vec<EngineSettings>,
    commands: Vec<Command>,
    journals: Vec<JournalQuery>,
    /// The engine ends of each journal handle. Kept so the handle's channels stay open and empty.
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
        Arc::new(Self {
            inner: Arc::new(Mutex::new(FakeState {
                latest: Arc::new(empty_snapshot()),
                demands: Vec::new(),
                settings: Vec::new(),
                commands: Vec::new(),
                journals: Vec::new(),
                journal_ends: Vec::new(),
                busy: false,
            })),
            wake_tx,
            wake_rx,
            ev_tx,
            ev_rx,
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

    /// Queues one action event. A full channel drops the event.
    pub fn emit(&self, ev: ActionEvent) {
        let _ = self.ev_tx.try_send(ev);
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

    fn set_demand(&self, demand: Demand) {
        self.state().demands.push(demand);
    }

    fn set_settings(&self, settings: EngineSettings) {
        self.state().settings.push(settings);
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

/// Every component at its default, every header `WarmingUp`, sequence 0.
pub fn empty_snapshot() -> Snapshot {
    Snapshot::empty(BootId(0), 0)
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
        let mut second = Demand::default();
        second.process_table = !first.process_table;
        engine.set_demand(first.clone());
        engine.set_demand(second.clone());
        assert_eq!(engine.demands(), vec![first, second]);

        let mut settings = EngineSettings::default();
        engine.set_settings(settings.clone());
        settings.interval_ms = 2000;
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
