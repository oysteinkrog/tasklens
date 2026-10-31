//! `engine.toml` in the window (ADR-018 Settings, leader decision Q3 on tasklens-q4x76.9,
//! tasklens-q4x76.24).
//!
//! Whoever holds the engine lock owns `engine.toml`. The window's own engine (the fallback when
//! the service does not answer) holds the lock, so it loads and writes the file with the
//! service's own code: `crates/tl-server/src/host_settings.rs` is compiled into tl-app as well.
//! The rescue window (`--low-memory`) holds no lock and only reads the file; a change made there
//! stays in its engine's memory.
//!
//! Writes run on their own thread, one at a time in the order they were sent, so the caller (the
//! UI thread) never waits for the disk. A switch to the service closes the file before the
//! window's engine stops and before the lock is released. Every write sent before that finishes
//! first, and any later write gets `EngineStopped`. So the service, which loads the file only after it has taken the lock, sees
//! every change the window made.

use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
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
    let shared = host_settings::SharedHostSettings::new(file, install);
    let (tx, writes) = mpsc::channel();
    let writer = match thread::Builder::new()
        .name("tl-host-settings".into())
        .spawn(move || write_all(&shared, &writes))
    {
        Ok(thread) => Some(Writer { tx, thread }),
        Err(e) => {
            warn!(
                target: "tasklens::engine",
                "host settings thread did not start, so host settings cannot be changed: {e}"
            );
            None
        }
    };
    let client = Arc::new(OwningClient {
        engine,
        writer: Mutex::new(writer),
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

/// A host settings change waiting for the writer thread.
struct Write {
    base_rev: u64,
    changes: HostSettingsChanges,
    reply: async_channel::Sender<SetHostSettingsReply>,
}

/// The thread that owns `engine.toml` and the queue to it.
struct Writer {
    tx: mpsc::Sender<Write>,
    thread: JoinHandle<()>,
}

/// The writer thread: stores each change in the order it was sent and replies. Ends once every
/// sender is gone and the queue is empty.
fn write_all(shared: &host_settings::SharedHostSettings, writes: &mpsc::Receiver<Write>) {
    while let Ok(write) = writes.recv() {
        let reply = shared.set(write.base_rev, &write.changes);
        // The caller may have dropped its receiver; the change is stored either way.
        let _ = write.reply.try_send(reply);
    }
}

/// The window's own engine with `engine.toml`: host settings writes go through the file, every
/// other call goes to the engine.
struct OwningClient {
    engine: Arc<dyn EngineClient>,
    /// `None` once closed.
    writer: Mutex<Option<Writer>>,
}

impl OwningClient {
    fn writer(&self) -> MutexGuard<'_, Option<Writer>> {
        self.writer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Refuses every later write, then waits until every write sent before has been stored.
    fn close(&self) {
        // The guard is dropped at the end of this statement, so callers never wait on the join.
        let writer = self.writer().take();
        if let Some(Writer { tx, thread }) = writer {
            drop(tx);
            if thread.join().is_err() {
                warn!(target: "tasklens::engine", "host settings thread panicked");
            }
        }
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

    /// Queues the change for the writer thread and returns at once. The thread checks `base_rev`
    /// against `engine.toml`'s revision, stores the change and hands it to the engine, as the
    /// service does. A write from another generation gets a closed channel, as in the engine.
    /// `Err(EngineStopped)` once the file is closed.
    fn set_host_settings(
        &self,
        generation: u64,
        base_rev: u64,
        changes: HostSettingsChanges,
    ) -> Result<async_channel::Receiver<SetHostSettingsReply>, CommandError> {
        if generation != self.engine.generation() {
            return Ok(async_channel::bounded(1).1);
        }
        let (reply, rx) = async_channel::bounded(1);
        let writer = self.writer();
        let Some(writer) = &*writer else {
            return Err(CommandError::EngineStopped);
        };
        // Sent under the lock, so `close` cannot drop the sender between the check and the send.
        writer
            .tx
            .send(Write {
                base_rev,
                changes,
                reply,
            })
            .map_err(|_| CommandError::EngineStopped)?;
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
        rx.recv_blocking().unwrap()
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

    /// The UI thread never waits for the disk: `set_host_settings` returns while the writer thread
    /// is still busy. Writes are stored in the order they were sent, and `stop` returns only once
    /// every write sent before it is stored.
    #[test]
    fn writes_run_off_the_calling_thread_in_order_and_finish_before_stop() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = TestBackend::new(0, 1);
        let publisher = Arc::clone(&backend);
        let (entered_tx, entered) = async_channel::unbounded::<u64>();
        let (release, gate) = async_channel::unbounded::<()>();
        let engine = local_engine(
            open(tmp.path()),
            backend,
            Box::new(move |h: &HostSettings| {
                if h.rev > 1 {
                    let _ = entered_tx.send_blocking(h.rev);
                    let _ = gate.recv_blocking();
                }
                publisher.publish_host(h.rev, h.clone());
            }),
            Box::new(|| {}),
        );
        let client = Arc::clone(&engine.client);
        let send = |base_rev, ms| {
            client
                .set_host_settings(client.generation(), base_rev, interval(ms))
                .unwrap()
        };

        let first = send(1, 500);
        // The writer thread now holds the first write until the gate opens, yet the calls return.
        assert_eq!(entered.recv_blocking().unwrap(), 2);
        let second = send(2, 600);
        let third = send(3, 700);
        assert!(
            first.try_recv().is_err(),
            "the first reply waits for the writer thread"
        );

        for _ in 0..3 {
            release.send_blocking(()).unwrap();
        }
        (engine.stop)();
        assert_eq!(
            first.try_recv().unwrap(),
            SetHostSettingsReply::Ack { rev: 2 }
        );
        assert_eq!(
            second.try_recv().unwrap(),
            SetHostSettingsReply::Ack { rev: 3 }
        );
        assert_eq!(
            third.try_recv().unwrap(),
            SetHostSettingsReply::Ack { rev: 4 }
        );
        let read = read_only(tmp.path());
        assert_eq!((read.rev, read.values.interval_ms), (4, 700));
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
