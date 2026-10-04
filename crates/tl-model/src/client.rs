//! The EngineClient trait the UI talks to (ADR-005).
//!
//! `tl-ui` reaches the engine only through this trait. D-001 keeps asupersync types out of
//! every crate but `tl-engine`, so the trait uses `async_channel` and plain values.

use std::sync::Arc;

use crate::actions::ActionEvent;
use crate::command::{Command, CommandError};
use crate::demand::Demand;
use crate::journal::{JournalHandle, JournalQuery};
use crate::settings::EngineSettings;
use crate::snapshot::Snapshot;

pub trait EngineClient: Send + Sync + 'static {
    /// Latest published snapshot. Cheap: one ArcSwap load_full.
    fn latest(&self) -> Arc<Snapshot>;
    /// One-token wake channel. Exactly one UI task calls recv on it.
    fn wake(&self) -> async_channel::Receiver<()>;
    /// Ordered action events. Exactly one UI task calls recv on it.
    fn action_events(&self) -> async_channel::Receiver<ActionEvent>;
    /// Replace demand. Never blocks. Last value wins.
    fn set_demand(&self, demand: Demand);
    /// Replace engine settings. Never blocks. Last value wins.
    fn set_settings(&self, settings: EngineSettings);
    /// Queue a command. Never blocks. Err(Busy) when the queue is full.
    fn send(&self, cmd: Command) -> Result<(), CommandError>;
    /// Open a journal stream for one unit. The engine runs journalctl and batches entries at most
    /// every 100 ms. Dropping the handle kills the child.
    fn journal(&self, query: JournalQuery) -> JournalHandle;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::BootId;

    /// A client with no engine behind it. Used only by this file's tests.
    struct NullClient {
        snapshot: Arc<Snapshot>,
        wake: async_channel::Receiver<()>,
        action_events: async_channel::Receiver<ActionEvent>,
    }

    impl NullClient {
        fn new() -> Self {
            let (_wake_tx, wake) = async_channel::bounded(1);
            let (_events_tx, action_events) = async_channel::bounded(256);
            Self {
                snapshot: Arc::new(Snapshot::empty(BootId(0), 0)),
                wake,
                action_events,
            }
        }
    }

    impl EngineClient for NullClient {
        fn latest(&self) -> Arc<Snapshot> {
            Arc::clone(&self.snapshot)
        }

        fn wake(&self) -> async_channel::Receiver<()> {
            self.wake.clone()
        }

        fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
            self.action_events.clone()
        }

        fn set_demand(&self, _demand: Demand) {}

        fn set_settings(&self, _settings: EngineSettings) {}

        fn send(&self, _cmd: Command) -> Result<(), CommandError> {
            Err(CommandError::EngineStopped)
        }

        fn journal(&self, _query: JournalQuery) -> JournalHandle {
            let (_events_tx, events) = async_channel::bounded(1);
            let (control, _control_rx) = async_channel::bounded(1);
            JournalHandle::new(events, control)
        }
    }

    #[test]
    fn engine_client_object_safe_send_sync() {
        let client: Arc<dyn EngineClient> = Arc::new(NullClient::new());
        let sequence = std::thread::spawn(move || client.latest().sequence)
            .join()
            .unwrap();
        assert_eq!(sequence, 0);
    }

    #[test]
    fn null_client_latest() {
        let client = NullClient::new();
        assert_eq!(client.latest().sequence, 0);
        assert_eq!(client.latest().sequence, 0);
    }

    #[test]
    fn null_client_send_stopped() {
        let client = NullClient::new();
        assert_eq!(
            client.send(Command::ResetBaselines),
            Err(CommandError::EngineStopped)
        );
    }
}
