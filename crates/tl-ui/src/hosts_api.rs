//! What tl-app gives the UI to reach remote hosts (ADR-018, D-038, tasklens-q4x76.26).
//!
//! tl-app owns one engine client per enabled `[[hosts]]` entry. The UI asks for a client by
//! entry, removes it, asks for a fresh connection, tests a target or reads the link status.
//! The UI takes the returned client into a `HostContext` and drops it when the host goes away:
//! a removed host stops reconnecting once nothing holds its client any more.

use std::sync::Arc;

use tl_model::client::EngineClient;
use tl_model::settings::HostEntry;

/// Where one host's connection is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostLinkStatus {
    /// Connecting, or connected and waiting for the first complete frame.
    Connecting,
    /// The first snapshot of the session is installed.
    Connected,
    /// The last attempt failed for this reason. The next one follows the backoff.
    Failed(Arc<str>),
}

/// Result of one `test` call: `Ok` when ssh started and the handshake completed.
pub type HostTestResult = Result<(), String>;

/// Starts or replaces a host's client from its entry.
pub type ConnectFn = Arc<dyn Fn(&HostEntry) -> Result<Arc<dyn EngineClient>, String> + Send + Sync>;
/// Gives a host's client by name.
pub type ClientByNameFn = Arc<dyn Fn(&str) -> Result<Arc<dyn EngineClient>, String> + Send + Sync>;
/// Tests one ssh target.
pub type TestFn = Arc<dyn Fn(&str) -> async_channel::Receiver<HostTestResult> + Send + Sync>;
/// Reads a host's link status.
pub type StatusFn = Arc<dyn Fn(&str) -> Option<HostLinkStatus> + Send + Sync>;

/// Makes the running hosts match a list of entries; one result per enabled entry.
pub type SyncFn = Arc<dyn Fn(&[HostEntry]) -> Vec<(String, Result<(), String>)> + Send + Sync>;

/// The host manager's calls. Every closure may run on the main thread and returns at once;
/// connecting happens on the client's own thread.
pub struct HostsApi {
    /// Starts a client for `entry`, or returns the running one when its target is unchanged.
    /// A changed target replaces the client. `Err` names why the entry is refused.
    pub connect: ConnectFn,
    /// Starts a client for each enabled entry that has none, replaces one whose target changed
    /// and stops every host that is gone or disabled. A failing entry does not stop the others.
    pub sync: SyncFn,
    /// Stops the host's client. `false` when no client has that name.
    pub remove: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// Replaces the host's client with a new one and returns it. The old client's session
    /// ends when nothing holds it any more.
    pub reconnect: ClientByNameFn,
    /// Opens one connection to `target` and closes it, for "Test connection". The result
    /// arrives on the channel, within the connector's deadline.
    pub test: TestFn,
    /// The link status of the named host, `None` when it has no client.
    pub status: StatusFn,
}
