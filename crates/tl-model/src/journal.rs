//! Journal queries and events for service logs (ADR-005).
//!
//! The engine runs `journalctl` per handle and sends entries in batches at most every 100 ms.
//! Dropping the handle closes the control channel, and the engine's task then kills the child.
//! Log messages can contain secrets, so neither the engine nor the UI writes them to tasklens's
//! own log.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::table::UnitRef;

/// Entries per initial load and per `load_older`.
pub const JOURNAL_PAGE: usize = 200;

/// What to read: the last `JOURNAL_PAGE` entries of this boot for one unit, then new entries
/// while `follow` is on.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct JournalQuery {
    pub unit: UnitRef,
    pub follow: bool,
}

/// One journal entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEntry {
    /// `__REALTIME_TIMESTAMP` in microseconds since the Unix epoch.
    pub realtime_us: u64,
    /// syslog priority, 0 (emerg) to 7 (debug). A missing `PRIORITY` field is 6.
    pub priority: u8,
    pub pid: Option<u32>,
    pub message: Arc<str>,
    /// `__CURSOR`, used to resume with `--after-cursor`.
    pub cursor: Arc<str>,
}

/// Which entries the user may read.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum JournalAccess {
    All,
    /// Only the user's own entries are visible. Sent for system units only (R-167).
    OwnOnly,
}

/// Why the journal could not be read. The UI owns the texts: `NotFound` is
/// "journalctl not found", `Timeout` is "Reading the journal took too long.", and `Exit` is
/// "Could not read the journal".
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum JournalError {
    NotFound,
    Timeout,
    Exit {
        code: i32,
        stderr_first_line: Arc<str>,
    },
}

/// Events from the engine's journal task to the UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JournalEvent {
    /// `older` is true for a `LoadOlder` page; `exhausted` is true when no older entries remain.
    Batch {
        entries: Arc<[JournalEntry]>,
        older: bool,
        exhausted: bool,
    },
    Access(JournalAccess),
    Error(JournalError),
}

/// Control messages from the handle to the engine's journal task.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum JournalControl {
    /// `Some(cursor)` pages back from that entry; `None` pages back from the oldest entry the
    /// engine has sent.
    LoadOlder(Option<Arc<str>>),
    SetFollow(bool),
}

/// UI side of one journal stream: an event receiver plus a control sender.
///
/// Dropping the handle drops `control`; the engine's task sees the closed channel and kills
/// the child.
pub struct JournalHandle {
    events: async_channel::Receiver<JournalEvent>,
    control: async_channel::Sender<JournalControl>,
}

impl JournalHandle {
    /// The engine builds both channels and keeps the event sender and the control receiver.
    pub fn new(
        events: async_channel::Receiver<JournalEvent>,
        control: async_channel::Sender<JournalControl>,
    ) -> Self {
        Self { events, control }
    }

    /// A clone of the event receiver.
    pub fn events(&self) -> async_channel::Receiver<JournalEvent> {
        self.events.clone()
    }

    /// Asks for the `JOURNAL_PAGE` entries before `from`, the `__CURSOR` of the oldest entry the
    /// UI still holds. `None` pages back from the oldest entry the engine has sent. A full or
    /// closed channel is ignored.
    pub fn load_older(&self, from: Option<Arc<str>>) {
        let _ = self.control.try_send(JournalControl::LoadOlder(from));
    }

    /// Turns follow on or off. Off kills the `-f` child; on resumes with `--after-cursor`.
    /// A full or closed channel is ignored.
    pub fn set_follow(&self, on: bool) {
        let _ = self.control.try_send(JournalControl::SetFollow(on));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (
        JournalHandle,
        async_channel::Sender<JournalEvent>,
        async_channel::Receiver<JournalControl>,
    ) {
        let (event_tx, event_rx) = async_channel::bounded(8);
        let (control_tx, control_rx) = async_channel::bounded(8);
        (
            JournalHandle::new(event_rx, control_tx),
            event_tx,
            control_rx,
        )
    }

    #[test]
    fn handle_control_order() {
        let (handle, _event_tx, control_rx) = pair();
        handle.load_older(Some(Arc::from("s=abc;i=1")));
        handle.set_follow(false);
        assert_eq!(
            control_rx.recv_blocking().unwrap(),
            JournalControl::LoadOlder(Some(Arc::from("s=abc;i=1")))
        );
        assert_eq!(
            control_rx.recv_blocking().unwrap(),
            JournalControl::SetFollow(false)
        );
    }

    #[test]
    fn handle_drop_closes_control() {
        let (handle, _event_tx, control_rx) = pair();
        drop(handle);
        assert!(control_rx.recv_blocking().is_err());
    }

    #[test]
    fn handle_closed_control_no_panic() {
        let (handle, _event_tx, control_rx) = pair();
        control_rx.close();
        handle.load_older(None);
        handle.set_follow(true);
    }

    #[test]
    fn handle_events_pass_through() {
        let (handle, event_tx, _control_rx) = pair();
        let entry = JournalEntry {
            realtime_us: 1_700_000_000_000_000,
            priority: 6,
            pid: Some(42),
            message: Arc::from("started"),
            cursor: Arc::from("s=abc;i=1"),
        };
        event_tx
            .send_blocking(JournalEvent::Batch {
                entries: Arc::from(vec![entry]),
                older: false,
                exhausted: true,
            })
            .unwrap();
        event_tx
            .send_blocking(JournalEvent::Error(JournalError::Timeout))
            .unwrap();

        let events = handle.events();
        match events.recv_blocking().unwrap() {
            JournalEvent::Batch {
                entries,
                older,
                exhausted,
            } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].realtime_us, 1_700_000_000_000_000);
                assert_eq!(entries[0].priority, 6);
                assert_eq!(entries[0].pid, Some(42));
                assert_eq!(&*entries[0].message, "started");
                assert_eq!(&*entries[0].cursor, "s=abc;i=1");
                assert!(!older);
                assert!(exhausted);
            }
            other => panic!("expected a batch, got {other:?}"),
        }
        assert!(matches!(
            events.recv_blocking().unwrap(),
            JournalEvent::Error(JournalError::Timeout)
        ));
    }
}
