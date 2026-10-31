//! The host manager: one `RemoteClient` per enabled `[[hosts]]` entry (ADR-018, D-038,
//! tasklens-q4x76.26).
//!
//! The local host keeps the `SwitchClient` of `engine_link`; this manager owns only the remote
//! hosts. Each entry gets a `RemoteClient` over a `Connector` that `make` builds (an
//! `SshConnector` in the app, a fake in tests). The client connects on its own thread, reports
//! the link only as up after the handshake and the first complete frame, reconnects with the
//! ADR-018 backoff, and numbers its sessions (generations). A host that fails stays in its own
//! client: it never blocks or changes another host.
//!
//! Hosts can be added, removed and reconnected while the window runs. Removing or replacing a
//! client drops the manager's handle; the session ends when the last handle is gone, so the
//! UI must drop its own clone of a removed host's client.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use tl_model::client::EngineClient;
use tl_model::remote_proto::HelloTail;
use tl_model::settings::{HostEntry, check_hosts};
use tl_remote::client::{BACKOFF, ClientConfig, Connector, LinkStatus, RemoteClient};
use tl_remote::ssh::{SSH_PROGRAM_ENV, SshConnector, ssh_program, validate_target};
use tl_ui::hosts_api::{HostLinkStatus, HostTestResult, HostsApi};
use tracing::{info, warn};

/// Builds the connector for one entry.
pub type MakeConnector = Arc<dyn Fn(&HostEntry) -> Box<dyn Connector> + Send + Sync>;

struct Host {
    name: String,
    target: String,
    client: Arc<RemoteClient>,
}

/// See the module docs.
pub struct HostManager {
    make: MakeConnector,
    hello: HelloTail,
    backoff: Vec<Duration>,
    hosts: Mutex<Vec<Host>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The connector the app uses: `ssh <target> tasklens bridge`, with the program from
/// `TASKLENS_SSH_PROGRAM` when set.
pub fn ssh_connector_factory() -> MakeConnector {
    let program = ssh_program(std::env::var_os(SSH_PROGRAM_ENV));
    Arc::new(move |entry: &HostEntry| {
        Box::new(SshConnector::new(
            entry.name.as_str(),
            entry.target.clone(),
            program.clone(),
        ))
    })
}

fn convert(status: LinkStatus) -> HostLinkStatus {
    match status {
        LinkStatus::Connecting => HostLinkStatus::Connecting,
        LinkStatus::Connected => HostLinkStatus::Connected,
        LinkStatus::Failed(why) => HostLinkStatus::Failed(why),
    }
}

impl HostManager {
    /// A manager with no hosts. `backoff` is `BACKOFF` in the app.
    pub fn new(make: MakeConnector, hello: HelloTail, backoff: Vec<Duration>) -> Self {
        Self {
            make,
            hello,
            backoff,
            hosts: Mutex::new(Vec::new()),
        }
    }

    fn start(&self, entry: &HostEntry) -> Result<Arc<RemoteClient>, String> {
        let connector = (self.make)(entry);
        let config = ClientConfig {
            hello: self.hello.clone(),
            backoff: self.backoff.clone(),
        };
        RemoteClient::start(move || connector.connect(), config)
            .map(Arc::new)
            .map_err(|e| format!("host client thread: {e}"))
    }

    /// Checks `entry` alone and against the hosts that already exist under another name.
    fn check(&self, entry: &HostEntry, hosts: &[Host]) -> Result<(), String> {
        let others: Vec<HostEntry> = hosts
            .iter()
            .filter(|h| h.name != entry.name)
            .map(|h| HostEntry {
                name: h.name.clone(),
                target: h.target.clone(),
                enabled: true,
            })
            .collect();
        let mut all = others;
        all.push(entry.clone());
        let last = all.len() - 1;
        match check_hosts(&all, None)
            .into_iter()
            .find(|(i, _)| *i == last)
        {
            Some((_, why)) => Err(format!("invalid host entry: {why:?}")),
            None => Ok(()),
        }
    }

    /// Starts a client for `entry`. The running client is kept when its target is unchanged,
    /// and replaced when it changed.
    pub fn connect(&self, entry: &HostEntry) -> Result<Arc<RemoteClient>, String> {
        let mut hosts = lock(&self.hosts);
        self.check(entry, &hosts)?;
        validate_target(&entry.target).map_err(|e| e.to_string())?;
        if let Some(host) = hosts.iter().find(|h| h.name == entry.name)
            && host.target == entry.target
        {
            return Ok(Arc::clone(&host.client));
        }
        let client = self.start(entry)?;
        info!(target: "tasklens::engine", host = %entry.name, "host client started");
        let host = Host {
            name: entry.name.clone(),
            target: entry.target.clone(),
            client: Arc::clone(&client),
        };
        match hosts.iter_mut().find(|h| h.name == entry.name) {
            Some(slot) => *slot = host,
            None => hosts.push(host),
        }
        Ok(client)
    }

    /// Stops the named host's client. `false` when there is none.
    pub fn remove(&self, name: &str) -> bool {
        let mut hosts = lock(&self.hosts);
        let before = hosts.len();
        hosts.retain(|h| h.name != name);
        hosts.len() != before
    }

    /// Replaces the named host's client with a new one on the same target.
    pub fn reconnect(&self, name: &str) -> Result<Arc<RemoteClient>, String> {
        let mut hosts = lock(&self.hosts);
        let Some(slot) = hosts.iter_mut().find(|h| h.name == name) else {
            return Err(format!("no host named {name}"));
        };
        let entry = HostEntry {
            name: slot.name.clone(),
            target: slot.target.clone(),
            enabled: true,
        };
        let client = self.start(&entry)?;
        slot.client = Arc::clone(&client);
        Ok(client)
    }

    /// The named host's client.
    pub fn client(&self, name: &str) -> Option<Arc<RemoteClient>> {
        lock(&self.hosts)
            .iter()
            .find(|h| h.name == name)
            .map(|h| Arc::clone(&h.client))
    }

    /// The named host's link status.
    pub fn status(&self, name: &str) -> Option<HostLinkStatus> {
        self.client(name).map(|c| convert(c.status()))
    }

    /// The names of the hosts that have a client, in the order they were added.
    pub fn names(&self) -> Vec<String> {
        lock(&self.hosts).iter().map(|h| h.name.clone()).collect()
    }

    /// Makes the running hosts match `entries`: starts a client for each enabled entry that has
    /// none, replaces one whose target changed, and removes every host that is gone or
    /// disabled. One entry that fails does not stop the others; each result is returned.
    pub fn sync(&self, entries: &[HostEntry]) -> Vec<(String, Result<(), String>)> {
        let wanted: Vec<&HostEntry> = entries.iter().filter(|e| e.enabled).collect();
        for name in self.names() {
            if !wanted.iter().any(|e| e.name == name) {
                self.remove(&name);
            }
        }
        wanted
            .into_iter()
            .map(|e| {
                let r = self.connect(e).map(|_| ());
                if let Err(why) = &r {
                    warn!(target: "tasklens::engine", host = %e.name, "host not started: {why}");
                }
                (e.name.clone(), r)
            })
            .collect()
    }

    /// Opens one connection to `target` and closes it. The result arrives on the channel.
    pub fn test(&self, target: &str) -> async_channel::Receiver<HostTestResult> {
        let (tx, rx) = async_channel::bounded(1);
        let entry = HostEntry {
            name: "test".into(),
            target: target.to_owned(),
            enabled: true,
        };
        if let Err(e) = validate_target(target) {
            let _ = tx.try_send(Err(e.to_string()));
            return rx;
        }
        let connector = (self.make)(&entry);
        let spawned = thread::Builder::new()
            .name("tl-host-test".into())
            .spawn({
                let tx = tx.clone();
                move || {
                    let result = match connector.connect() {
                        Ok(conn) => {
                            (conn.shutdown)();
                            Ok(())
                        }
                        Err(e) => Err(e.to_string()),
                    };
                    let _ = tx.try_send(result);
                }
            })
            .map(|_| ());
        if let Err(e) = spawned {
            let _ = tx.try_send(Err(e.to_string()));
        }
        rx
    }

    /// The calls for `AppEnv::hosts`.
    pub fn api(self: &Arc<Self>) -> HostsApi {
        let m = Arc::clone(self);
        let connect =
            Arc::new(move |e: &HostEntry| m.connect(e).map(|c| c as Arc<dyn EngineClient>));
        let m = Arc::clone(self);
        let sync = Arc::new(move |e: &[HostEntry]| m.sync(e));
        let m = Arc::clone(self);
        let remove = Arc::new(move |n: &str| m.remove(n));
        let m = Arc::clone(self);
        let reconnect = Arc::new(move |n: &str| m.reconnect(n).map(|c| c as Arc<dyn EngineClient>));
        let m = Arc::clone(self);
        let test = Arc::new(move |t: &str| m.test(t));
        let m = Arc::clone(self);
        let status = Arc::new(move |n: &str| m.status(n));
        HostsApi {
            connect,
            sync,
            remove,
            reconnect,
            test,
            status,
        }
    }
}

/// The ADR-018 reconnect backoff.
pub fn backoff() -> Vec<Duration> {
    BACKOFF.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use tl_model::remote_proto::Want;
    use tl_model::snapshot::Snapshot;
    use tl_model::time::BootId;
    use tl_remote::client::Connection;

    use crate::switch_client::tests::serve;

    fn hello() -> HelloTail {
        HelloTail {
            pid: 1,
            app_id: Arc::from("test"),
            bridge: false,
            want: Want { snapshots: true },
        }
    }

    fn entry(name: &str, target: &str) -> HostEntry {
        HostEntry {
            name: name.into(),
            target: target.into(),
            enabled: true,
        }
    }

    /// Targets starting with `bad` never connect; every other target gets a fake engine that
    /// sends one snapshot. `connects` counts the attempts.
    fn fake(connects: Arc<AtomicUsize>) -> MakeConnector {
        Arc::new(move |e: &HostEntry| {
            let bad = e.target.starts_with("bad");
            let connects = Arc::clone(&connects);
            Box::new(move || -> io::Result<Connection> {
                connects.fetch_add(1, Ordering::SeqCst);
                if bad {
                    return Err(io::Error::other("Permission denied (publickey)"));
                }
                let (ours, theirs) = UnixStream::pair()?;
                thread::spawn(move || {
                    serve(theirs, Duration::ZERO, vec![Snapshot::empty(BootId(7), 0)]);
                });
                Connection::unix(ours)
            })
        })
    }

    fn manager(connects: &Arc<AtomicUsize>) -> Arc<HostManager> {
        Arc::new(HostManager::new(
            fake(Arc::clone(connects)),
            hello(),
            vec![Duration::from_secs(60)],
        ))
    }

    fn wait_until(what: &str, f: impl Fn() -> bool) {
        let begin = Instant::now();
        while !f() {
            assert!(begin.elapsed() < Duration::from_secs(5), "{what}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn failed(m: &HostManager, name: &str) -> bool {
        matches!(m.status(name), Some(HostLinkStatus::Failed(_)))
    }

    #[test]
    fn add_and_remove_a_host_at_runtime() {
        let n = Arc::new(AtomicUsize::new(0));
        let m = manager(&n);
        assert!(m.names().is_empty());
        m.connect(&entry("home", "me@home")).expect("add");
        wait_until("home connects", || {
            m.status("home") == Some(HostLinkStatus::Connected)
        });
        m.connect(&entry("lab", "lab")).expect("add second");
        assert_eq!(m.names(), ["home", "lab"]);
        assert!(m.remove("home"));
        assert!(!m.remove("home"));
        assert_eq!(m.status("home"), None);
        assert_eq!(m.names(), ["lab"]);
        wait_until("lab connects", || {
            m.status("lab") == Some(HostLinkStatus::Connected)
        });
    }

    #[test]
    fn a_failing_host_does_not_affect_the_others() {
        let n = Arc::new(AtomicUsize::new(0));
        let m = manager(&n);
        let results = m.sync(&[
            entry("good", "good"),
            entry("broken", "bad-host"),
            entry("good2", "good2"),
        ]);
        assert!(results.iter().all(|(_, r)| r.is_ok()));
        wait_until("broken fails", || failed(&m, "broken"));
        wait_until("both good hosts connect", || {
            m.status("good") == Some(HostLinkStatus::Connected)
                && m.status("good2") == Some(HostLinkStatus::Connected)
        });
        match m.status("broken") {
            Some(HostLinkStatus::Failed(why)) => assert!(why.contains("Permission denied")),
            other => panic!("{other:?}"),
        }
        // Removing the failing host leaves the others up.
        assert!(m.remove("broken"));
        assert_eq!(m.status("good"), Some(HostLinkStatus::Connected));
        assert_eq!(m.status("good2"), Some(HostLinkStatus::Connected));
    }

    #[test]
    fn sync_adds_removes_and_skips_disabled() {
        let n = Arc::new(AtomicUsize::new(0));
        let m = manager(&n);
        let mut off = entry("off", "off");
        off.enabled = false;
        m.sync(&[entry("a", "a"), entry("b", "b"), off.clone()]);
        assert_eq!(m.names(), ["a", "b"]);
        let a = m.client("a").expect("a");
        m.sync(&[entry("b", "b2"), entry("c", "c"), off]);
        assert_eq!(m.names(), ["b", "c"]);
        assert!(m.client("a").is_none());
        drop(a);
    }

    #[test]
    fn connect_keeps_a_running_client_and_replaces_one_with_a_new_target() {
        let n = Arc::new(AtomicUsize::new(0));
        let m = manager(&n);
        let first = m.connect(&entry("home", "h1")).expect("add");
        let same = m.connect(&entry("home", "h1")).expect("again");
        assert!(Arc::ptr_eq(&first, &same));
        let moved = m.connect(&entry("home", "h2")).expect("moved");
        assert!(!Arc::ptr_eq(&first, &moved));
        assert_eq!(m.names(), ["home"]);
    }

    #[test]
    fn reconnect_gives_a_new_client() {
        let n = Arc::new(AtomicUsize::new(0));
        let m = manager(&n);
        let first = m.connect(&entry("home", "bad")).expect("add");
        wait_until("fails", || failed(&m, "home"));
        let second = m.reconnect("home").expect("reconnect");
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(m.reconnect("nope").is_err());
        wait_until("second attempt", || n.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn bad_entries_are_refused() {
        let n = Arc::new(AtomicUsize::new(0));
        let m = manager(&n);
        assert!(m.connect(&entry("", "x")).is_err());
        assert!(m.connect(&entry("a", "")).is_err());
        assert!(m.connect(&entry("a", "-oProxyCommand=x")).is_err());
        m.connect(&entry("a", "a")).expect("ok");
        assert!(m.connect(&entry("A", "other")).is_err());
        assert_eq!(m.names(), ["a"]);
        wait_until("a connects", || {
            m.status("a") == Some(HostLinkStatus::Connected)
        });
        assert_eq!(n.load(Ordering::SeqCst), 1, "refused entries never connect");
    }

    #[test]
    fn test_connection_reports_the_result_and_adds_no_host() {
        let n = Arc::new(AtomicUsize::new(0));
        let m = manager(&n);
        let ok = m.test("good").recv_blocking().expect("result");
        assert_eq!(ok, Ok(()));
        let bad = m.test("bad").recv_blocking().expect("result");
        assert!(bad.expect_err("fails").contains("Permission denied"));
        let refused = m.test("bad host").recv_blocking().expect("result");
        assert!(refused.is_err());
        assert!(m.names().is_empty());
    }
}
