//! The EngineClient trait the UI talks to (ADR-005).
//!
//! `tl-ui` reaches the engine only through this trait. D-001 keeps asupersync types out of
//! every crate but `tl-engine`, so the trait uses `async_channel` and plain values.

use std::sync::Arc;

use crate::actions::ActionEvent;
use crate::command::{Command, CommandError};
use crate::demand::Demand;
use crate::journal::{JournalHandle, JournalQuery};
use crate::remote_proto::{HostSettingsChanges, SetHostSettingsReply};
use crate::request::RequestEvent;
use crate::settings::EngineSettings;
use crate::snapshot::Snapshot;

pub trait EngineClient: Send + Sync + 'static {
    /// Latest published snapshot. Cheap: one ArcSwap load_full.
    fn latest(&self) -> Arc<Snapshot>;
    /// One-token wake channel. Exactly one UI task calls recv on it.
    fn wake(&self) -> async_channel::Receiver<()>;
    /// Ordered action events. Exactly one UI task calls recv on it.
    fn action_events(&self) -> async_channel::Receiver<ActionEvent>;
    /// The one end of each action, scan, handle search and eject, in order (ADR-018). Exactly
    /// one UI task calls recv on it. The engine's channel is unbounded, so an end is never
    /// dropped. The default is a channel that is already closed: a client with no engine behind
    /// it ends no request.
    fn request_events(&self) -> async_channel::Receiver<RequestEvent> {
        async_channel::unbounded().1
    }
    /// Changes when the snapshots start over from another engine session: a reconnect or a new
    /// engine instance (ADR-018 Delta flow). Component versions in snapshots from different
    /// generations cannot be compared, so the hub treats a new value as everything changed.
    /// It also goes up the moment a connection is lost or a switch starts (ADR-018
    /// Generations), so it can be newer than the generation `latest_with_generation` gives:
    /// the stored snapshot's session has ended and no new one has arrived yet.
    /// The default is 0 for a client whose engine never changes.
    fn generation(&self) -> u64 {
        0
    }
    /// `latest` with the generation it belongs to. A client whose generation changes returns
    /// both from one load, so a snapshot is never paired with another session's generation.
    fn latest_with_generation(&self) -> (u64, Arc<Snapshot>) {
        (self.generation(), self.latest())
    }
    /// Replace demand. Never blocks. Last value wins.
    fn set_demand(&self, demand: Demand);
    /// Replace engine settings. Never blocks. Last value wins.
    fn set_settings(&self, settings: EngineSettings);
    /// Changes the host settings (ADR-018 Settings). `base_rev` is the `rev` of the
    /// `Snapshot.host_settings` the change was made from, so a client never writes values it
    /// has not read: the engine applies `changes` only when `base_rev` is its current revision.
    /// `generation` is the `generation` that snapshot came with. Two engines can have the same
    /// revision, so a write whose `generation` is not the client's current one belongs to a
    /// session that has ended: it is never sent, and its channel is closed with no reply, as
    /// for a write whose connection dropped (ADR-018 Generations). Never blocks. The reply
    /// comes on the returned channel: `Ack { rev }` with the new revision, or
    /// `Conflict { current }`. The new values reach every session in its next
    /// snapshot either way. The channel closes with no reply when the connection drops first.
    /// `Err(Busy)` when the queue is full. The default is `Err(EngineStopped)`: a client with no
    /// engine behind it has no host settings.
    fn set_host_settings(
        &self,
        generation: u64,
        base_rev: u64,
        changes: HostSettingsChanges,
    ) -> Result<async_channel::Receiver<SetHostSettingsReply>, CommandError> {
        let _ = (generation, base_rev, changes);
        Err(CommandError::EngineStopped)
    }
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
    fn default_request_events_are_closed() {
        let client = NullClient::new();
        assert!(client.request_events().is_closed());
    }

    #[test]
    fn default_generation_is_zero() {
        assert_eq!(NullClient::new().generation(), 0);
        let (generation, snap) = NullClient::new().latest_with_generation();
        assert_eq!((generation, snap.sequence), (0, 0));
    }

    #[test]
    fn default_set_host_settings_is_stopped() {
        let client = NullClient::new();
        assert!(matches!(
            client.set_host_settings(0, 1, HostSettingsChanges::default()),
            Err(CommandError::EngineStopped)
        ));
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
