//! The one end of each client request (ADR-018).
//!
//! Every action, Strings scan, handle search, eject and history query a window sends ends with
//! exactly one `RequestEvent` on the window's request channel, `EngineClient::request_events`.
//! The ids are the window's own. Progress and results still come through the snapshot and the action
//! events; this channel only says that a request ended and how, so a request the engine lost
//! does not wait forever.

use serde::{Deserialize, Serialize};

/// The kinds of client request.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum IdKind {
    /// `ScanId` of `StringScan` and `CancelStringScan`.
    Scan,
    /// `SearchId` of `HandleSearch` and `CancelHandleSearch`.
    Search,
    /// `seq` of `DeviceCommand::Eject`.
    Eject,
    /// `ActionId` of `Command::Action`.
    Action,
    /// `id` of `Command::HistoryQuery`.
    History,
}

/// How a request ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum RequestEnd {
    /// A scan `Finished`, `StoppedLimit` or `StoppedExited`; a handle search or eject `Done`;
    /// an action whose summary is `Succeeded`; a history query answered.
    Done,
    /// A scan or handle search `Failed`; an eject `Failed` or `Blocked`; an action whose summary
    /// is `Failed`, `OutcomeUnknown` or `Rejected` for a reason other than `Busy`.
    Failed,
    /// A scan `Cancelled`, a handle search `Stopped`, or the worker dropped the request or ran a
    /// newer one in its slot instead.
    Cancelled,
    /// Refused at admission: too many requests of its kind were active.
    Busy,
    /// The engine lost it: its command never reached a worker, the worker that claimed it
    /// stopped, or the reducer stopped while it ran.
    Lost,
}

/// The one end of one request.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RequestEvent {
    pub kind: IdKind,
    /// The window's own id: `ScanId`, `SearchId`, eject `seq`, `ActionId` or history query `id`.
    pub id: u64,
    pub end: RequestEnd,
}
