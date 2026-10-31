//! `engine.toml` in the window (ADR-018 Settings, leader decision Q3 on tasklens-q4x76.9,
//! tasklens-q4x76.24).
//!
//! Whoever holds the engine lock owns `engine.toml`. The window's own engine (the fallback when
//! the service does not answer) holds the lock, so it loads and writes the file with the
//! service's own code: `crates/tl-server/src/host_settings.rs` is compiled into tl-app as well.
//! The rescue window (`--low-memory`) holds no lock and only reads the file; a change made there
//! stays in its engine's memory.
//!
//! A switch to the service closes the file before the window's engine stops and before the lock
//! is released. A write that is under way finishes first, and any later write gets
//! `EngineStopped`. So the service, which loads the file only after it has taken the lock, sees
//! every change the window made.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::demand::Demand;
use tl_model::journal::{JournalHandle, JournalQuery};
use tl_model::remote_proto::{HostSettings, HostSettingsChanges, SetHostSettingsReply};
use tl_model::request::RequestEvent;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::Snapshot;
use tracing::{info, warn};

use crate::engine_link::LocalEngine;

#[path = "../../tl-server/src/host_settings.rs"]
#[allow(
    dead_code,
    reason = "tasklens-engine's own module; the window does not use all of it"
)]
mod host_settings;

pub use host_settings::HostSettingsFile;

/// Hands new host settings to the engine.
pub type Install = Box<dyn Fn(&HostSettings) + Send + Sync>;

/// Loads `dir/engine.toml` for an engine that holds the lock, or creates it from the host keys
/// in `settings.toml`, as the service does at start. Creates `dir` when it is missing.
pub fn open(dir: &Path) -> HostSettingsFile {
    if let Err(e) = crate::paths::ensure_dir(dir) {
        warn!(target: "tasklens::engine", "{}: {e}", dir.display());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (file, origin) = HostSettingsFile::open(dir, now);
    match origin {
        host_settings::Origin::Loaded => {}
        host_settings::Origin::Imported => {
            info!(target: "tasklens::engine", "host settings imported from settings.toml");
        }
        host_settings::Origin::Created => {
            info!(target: "tasklens::engine", "host settings created");
        }
        host_settings::Origin::Corrupt { kept_as } => warn!(
            target: "tasklens::engine",
            "engine.toml was unreadable and has been reset; the old file was kept as {kept_as}"
        ),
    }
    file
}

/// The host settings for the rescue window, which holds no lock: read from `dir/engine.toml`,
/// never written (ADR-018 Rescue mode).
pub fn read_only(dir: &Path) -> HostSettings {
    host_settings::read_without_lock(dir)
}

/// The window's own engine as the owner of `file`. `install` gets the settings at once and after
/// each stored change. `stop` stops the engine; the returned `LocalEngine`'s stop closes the file
/// first.
pub fn local_engine(
    file: HostSettingsFile,
    engine: Arc<dyn EngineClient>,
    install: Install,
    stop: Box<dyn FnOnce() + Send>,
) -> LocalEngine {
    install(file.current());
    let client = Arc::new(OwningClient {
        engine,
        file: Mutex::new(Some(host_settings::SharedHostSettings::new(file, install))),
    });
    let closer = Arc::clone(&client);
    LocalEngine {
        client,
        stop: Box::new(move || {
            closer.close();
            stop();
        }),
    }
}

/// The window's own engine with `engine.toml`: host settings writes go through the file, every
/// other call goes to the engine.
struct OwningClient {
    engine: Arc<dyn EngineClient>,
    /// `None` once closed.
    file: Mutex<Option<host_settings::SharedHostSettings>>,
}

impl OwningClient {
    fn file(&self) -> MutexGuard<'_, Option<host_settings::SharedHostSettings>> {
        self.file.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for a write that is under way, then refuses every later one.
    fn close(&self) {
        *self.file() = None;
    }
}

impl EngineClient for OwningClient {
    fn latest(&self) -> Arc<Snapshot> {
        self.engine.latest()
    }

    fn wake(&self) -> async_channel::Receiver<()> {
        self.engine.wake()
    }

    fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        self.engine.action_events()
    }

    fn request_events(&self) -> async_channel::Receiver<RequestEvent> {
        self.engine.request_events()
    }

    fn generation(&self) -> u64 {
        self.engine.generation()
    }

    fn latest_with_generation(&self) -> (u64, Arc<Snapshot>) {
        self.engine.latest_with_generation()
    }

    fn set_demand(&self, demand: Demand) {
        self.engine.set_demand(demand);
    }

    fn set_settings(&self, settings: EngineSettings) {
        self.engine.set_settings(settings);
    }

    /// Checks `base_rev` against `engine.toml`'s revision, stores the change and hands it to the
    /// engine, as the service does. A write from another generation gets a closed channel, as in
    /// the engine. `Err(EngineStopped)` once the file is closed.
    fn set_host_settings(
        &self,
        generation: u64,
        base_rev: u64,
        changes: HostSettingsChanges,
    ) -> Result<async_channel::Receiver<SetHostSettingsReply>, CommandError> {
        if generation != self.engine.generation() {
            return Ok(async_channel::bounded(1).1);
        }
        let reply = {
            let file = self.file();
            let Some(shared) = &*file else {
                return Err(CommandError::EngineStopped);
            };
            shared.set(base_rev, &changes)
        };
        let (tx, rx) = async_channel::bounded(1);
        // The channel is new and has one free slot, so this cannot fail.
        let _ = tx.try_send(reply);
        Ok(rx)
    }

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        self.engine.send(cmd)
    }

    fn journal(&self, query: JournalQuery) -> JournalHandle {
        self.engine.journal(query)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use tl_model::remote_proto::{HelloTail, Want};

    use super::*;
    use crate::engine_link::{EngineLink, LOCK_FILE, LinkEnv, lock_engine};
    use crate::switch_client::tests::TestBackend;

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn interval(ms: u32) -> HostSettingsChanges {
        HostSettingsChanges {
            interval_ms: Some(ms),
            ..HostSettingsChanges::default()
        }
    }

    /// The window's own engine as main.rs builds it, with a test backend that publishes every
    /// installed revision. `stopped` counts the stops.
    fn start(dir: &Path, stopped: &Arc<AtomicU32>) -> LocalEngine {
        let backend = TestBackend::new(0, 1);
        let publisher = Arc::clone(&backend);
        let stopped = Arc::clone(stopped);
        local_engine(
            open(dir),
            backend,
            Box::new(move |h: &HostSettings| publisher.publish_host(h.rev, h.clone())),
            Box::new(move || {
                stopped.fetch_add(1, Ordering::AcqRel);
            }),
        )
    }

    fn write(client: &dyn EngineClient, base_rev: u64, ms: u32) -> SetHostSettingsReply {
        let rx = client
            .set_host_settings(client.generation(), base_rev, interval(ms))
            .unwrap();
        rx.try_recv().unwrap()
    }

    /// The test that the bead names: a setting changed under the fallback engine is seen by the
    /// service after a switch. The "service" is the unit start: it takes the engine lock, which
    /// works only once the window has released it, and loads `engine.toml` with the service's
    /// code, as `tasklens-engine` does at start. It then lets go and fails, so the window runs its
    /// own engine again.
    #[test]
    fn a_change_under_the_fallback_engine_is_seen_by_the_service_after_a_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let config: PathBuf = tmp.path().join("config");
        let lock_file = tmp.path().join(LOCK_FILE);
        let stopped = Arc::new(AtomicU32::new(0));
        let seen: Arc<Mutex<Vec<HostSettings>>> = Arc::default();

        let runs = AtomicU32::new(0);
        let (unit_lock, unit_config, unit_seen) =
            (lock_file.clone(), config.clone(), Arc::clone(&seen));
        let engine_config = config.clone();
        let engine_stopped = Arc::clone(&stopped);
        let env = LinkEnv {
            socket: Err("no socket in this test".into()),
            lock_file: lock_file.clone(),
            start_unit: Some(Box::new(move |_| {
                if runs.fetch_add(1, Ordering::AcqRel) == 0 {
                    return false;
                }
                let held = lock_engine(&unit_lock).unwrap();
                assert!(held.is_some(), "the window released the lock");
                let (file, _) = HostSettingsFile::open(&unit_config, 0);
                lock(&unit_seen).push(file.current().clone());
                false
            })),
            start_engine: Box::new(move || Ok(start(&engine_config, &engine_stopped))),
            hello: HelloTail {
                pid: 1,
                app_id: Arc::from("test"),
                bridge: false,
                want: Want { snapshots: true },
            },
            uid: tl_remote::server::own_uid(),
            notify: Box::new(|_| {}),
            deadline: Duration::from_millis(100),
            retry: Duration::from_secs(60),
        };
        let link = EngineLink::open(env).unwrap();
        let client = link.client();
        // A fresh install: the file was created at revision 1, and the snapshot carries it.
        assert_eq!(client.latest().host_settings.rev, 1);
        assert_eq!(
            write(client.as_ref(), 1, 2500),
            SetHostSettingsReply::Ack { rev: 2 }
        );
        assert_eq!(client.latest().host_settings.values.interval_ms, 2500);

        link.run_switch();
        assert_eq!(stopped.load(Ordering::Acquire), 1);
        let seen = lock(&seen).clone();
        assert_eq!(seen.len(), 1, "the unit started once during the switch");
        assert_eq!((seen[0].rev, seen[0].values.interval_ms), (2, 2500));
        assert!(!seen[0].read_only);

        // The service did not come up, so the window owns the file again and reads it back.
        assert_eq!(
            link.client().latest().host_settings.rev,
            2,
            "the window's engine loaded the stored revision"
        );
        link.shutdown();
        assert_eq!(stopped.load(Ordering::Acquire), 2);
    }

    #[test]
    fn a_closed_file_refuses_writes_and_is_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        let stopped = Arc::new(AtomicU32::new(0));
        let engine = start(tmp.path(), &stopped);
        let client = Arc::clone(&engine.client);
        let path = tmp.path().join(host_settings::ENGINE_FILE);
        assert_eq!(
            write(client.as_ref(), 1, 500),
            SetHostSettingsReply::Ack { rev: 2 }
        );
        // A conflict leaves the file as it is.
        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(matches!(
            write(client.as_ref(), 1, 5000),
            SetHostSettingsReply::Conflict { current } if current.rev == 2
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), stored);

        (engine.stop)();
        assert_eq!(stopped.load(Ordering::Acquire), 1);
        assert!(matches!(
            client.set_host_settings(client.generation(), 2, interval(5000)),
            Err(CommandError::EngineStopped)
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), stored);
    }

    #[test]
    fn a_write_from_another_generation_is_not_applied() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = start(tmp.path(), &Arc::default());
        let client = engine.client;
        let rx = client
            .set_host_settings(client.generation() + 1, 1, interval(500))
            .unwrap();
        assert!(rx.recv_blocking().is_err());
        assert_eq!(client.latest().host_settings.rev, 1);
    }

    /// ADR-018 Rescue mode: the rescue window reads `engine.toml` and leaves the directory as it
    /// was, also when the file does not exist yet.
    #[test]
    fn the_rescue_window_reads_and_never_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let read = read_only(tmp.path());
        assert_eq!(read.rev, 1);
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());

        let stopped = Arc::new(AtomicU32::new(0));
        let engine = start(tmp.path(), &stopped);
        assert_eq!(
            write(engine.client.as_ref(), 1, 750),
            SetHostSettingsReply::Ack { rev: 2 }
        );
        (engine.stop)();
        let path = tmp.path().join(host_settings::ENGINE_FILE);
        let stored = std::fs::read_to_string(&path).unwrap();
        let read = read_only(tmp.path());
        assert_eq!((read.rev, read.values.interval_ms), (2, 750));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), stored);
        // Only engine.toml is there: nothing was renamed or added.
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }
}
