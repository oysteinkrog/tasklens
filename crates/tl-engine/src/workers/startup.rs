//! The `Startup` worker (ADR-004, ADR-008 row "Startup entries", ADR-016 row "Startup apps";
//! SYS-019, SYS-020, R-082).
//!
//! It wakes every `IDLE_RECHECK` and scans only when the Startup Apps tab opens
//! (`Demand.startup` goes from false to true) or on `RefreshNow(Startup)` while the tab is open.
//! A scan is one blocking job over the tl-collect startup scanner. Precedence, overridden rows
//! and the effectively-disabled rules are the scanner's.
//!
//! While the tab is open it also holds an inotify watch on the startup directories and a
//! session-bus match for the user manager's `UnitFilesChanged` (R-082). Each wake drains both
//! without waiting; any event gives one rescan. Both are dropped when the tab closes. Without a
//! session bus only the inotify part runs.
//!
//! `command` holds `Exec` lines, which can carry secrets. Never log it or file contents.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::sleep_until;
use asupersync::{Cx, Outcome};
use futures_lite::StreamExt;
use rustc_hash::FxHashMap;
use tl_collect::CollectError;
use tl_collect::startup::{
    StartupEnv, StartupKindRaw, StartupRaw, StartupStatusRaw, StartupWatch, UnitFileListing,
    scan_startup,
};
use tl_model::health::SourceId;
use tl_model::snapshot::{Impact, StartupEntry, StartupKind, StartupStatus};
use tl_model::table::{StartupRef, StartupSource};
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{Bus, DbusConn, DbusError, connect};
use crate::source::{CommandProgress, IDLE_RECHECK, WorkerCommand, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{EngineMsg, SourceUpdate, StartupUpdate, WorkerError};
use crate::workers::WorkerSpec;
use crate::workers::dbus_systemd::{BUS_NAME, MANAGER_IFACE, PATH};

/// What one scan reads. The environment is read once per scan.
#[derive(Clone, Debug, Default)]
pub struct ScanInput {
    pub env: StartupEnv,
    /// `$HOME`, for the "User" publisher. `None` when unset or not absolute.
    pub home: Option<PathBuf>,
    /// The user manager's unit files as (path, state), from `ListUnitFilesByPatterns`. Empty
    /// until E14-013 lists them over D-Bus.
    pub units: Vec<(String, String)>,
}

impl ScanInput {
    fn from_env() -> Self {
        Self {
            env: StartupEnv::from_env(),
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute()),
            units: Vec::new(),
        }
    }
}

/// Runs on the blocking pool. Tests replace it with a scripted fake.
pub type Scanner = Arc<dyn Fn(&ScanInput) -> Result<Vec<StartupRaw>, CollectError> + Send + Sync>;

/// The real scanner: XDG autostart, user units and, on KDE, Plasma scripts.
fn host_scan(input: &ScanInput) -> Result<Vec<StartupRaw>, CollectError> {
    let units: Vec<UnitFileListing<'_>> = input
        .units
        .iter()
        .map(|(path, state)| UnitFileListing { path, state })
        .collect();
    let mut out = Vec::new();
    scan_startup(&input.env, &units, &mut out)?;
    Ok(out)
}

pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Startup,
        name: "startup",
        modes: &[EngineMode::Full],
        factory,
    }
}

pub(crate) fn factory(_cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let scanner: Scanner = Arc::new(host_scan);
        async move { run(cx, io, scanner, System).await }
    })
}

async fn run<H: Host>(cx: Cx, io: WorkerIo, scanner: Scanner, host: H) -> Outcome<(), WorkerError> {
    let mut queue = Queue::resume(&io);
    let mut open = false;
    let mut live: Option<Live<H::Files, H::Signals>> = None;
    let mut cache = Cache::default();
    loop {
        sleep_until(cx.now() + IDLE_RECHECK).await;
        if cx.checkpoint().is_err() {
            return Outcome::Ok(());
        }

        let now_open = io.demand.borrow_and_clone().startup;
        let opened = now_open && !open;
        open = now_open;
        if !open {
            live = None;
        } else if opened {
            // Watch before the first scan so a change during the scan is not lost.
            live = Some(Live::open(&cx, &host).await);
            if cx.checkpoint().is_err() {
                return Outcome::Ok(());
            }
        }
        let changed = match live.as_mut() {
            Some(live) => live.drain().await,
            None => false,
        };
        let refresh = queue.take(&io);
        if !open || !(opened || refresh.is_some() || changed) {
            // A RefreshNow while the tab is hidden counts as done without a scan.
            if let Some(seq) = refresh {
                queue.progress.set_done(SourceId::Startup, seq);
            }
            continue;
        }

        // Before the scan, so a directory made since the last scan is watched from now on.
        if let Some(files) = live.as_mut().and_then(|live| live.files.as_mut()) {
            files.rewatch();
        }
        let input = host.input();
        let scan = Arc::clone(&scanner);
        let mut job = match cx.spawn_blocking(move |_| {
            let result = scan(&input);
            let entries = result.map(|raws| cache.merge(raws, input.home.as_deref()));
            (cache, entries)
        }) {
            Ok(job) => job,
            Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
        };
        let entries = match job.join(&cx).await {
            Ok((c, entries)) => {
                cache = c;
                entries
            }
            Err(JoinError::Cancelled(reason)) => {
                if cx.checkpoint().is_err() {
                    return Outcome::Ok(());
                }
                return Outcome::Cancelled(reason);
            }
            Err(JoinError::Panicked(payload)) => return Outcome::Panicked(payload),
            Err(JoinError::PolledAfterCompletion) => {
                return Outcome::Err(WorkerError::Other("job polled after completion".into()));
            }
        };

        match entries {
            Ok(entries) => {
                let update = SourceUpdate::Startup(StartupUpdate {
                    entries,
                    validity: Validity::Valid,
                });
                match io.updates.send(&cx, EngineMsg::Source(update)).await {
                    Ok(()) => {}
                    Err(mpsc::SendError::Cancelled(_)) => return Outcome::Ok(()),
                    Err(_) => return Outcome::Err(WorkerError::Closed),
                }
            }
            // A scan never fails the worker. The last published entries stay.
            Err(e) => {
                tracing::debug!(target: "tasklens::engine", error = %e, "startup scan failed")
            }
        }
        if let Some(seq) = refresh {
            queue.progress.set_done(SourceId::Startup, seq);
        }
    }
}

/// What the worker reads from the machine besides the scan itself. Tests replace it.
trait Host {
    type Files: FileEvents;
    type Signals: UnitFileSignals;
    /// The scan input. Read once per scan and once when the tab opens.
    fn input(&self) -> ScanInput;
    /// An inotify watch on the startup directories of `env`.
    fn watch(&self, env: &StartupEnv) -> Result<Self::Files, CollectError>;
    /// A session-bus subscription to the user manager's `UnitFilesChanged`.
    async fn subscribe(&self, cx: &Cx) -> Result<Self::Signals, DbusError>;
}

/// Directory change events.
trait FileEvents {
    /// Reads every pending event without blocking. True if any arrived.
    fn drain(&mut self) -> Result<bool, CollectError>;
    /// Adds watches for directories that did not exist before.
    fn rewatch(&mut self);
}

/// `UnitFilesChanged` signals.
trait UnitFileSignals {
    /// Takes every queued signal without waiting. True if any arrived. `Err` when the stream
    /// ended.
    async fn drain(&mut self) -> Result<bool, DbusError>;
}

/// The real machine: the process environment, inotify and the session bus.
struct System;

impl Host for System {
    type Files = StartupWatch;
    type Signals = UnitFilesSignals;

    fn input(&self) -> ScanInput {
        ScanInput::from_env()
    }

    fn watch(&self, env: &StartupEnv) -> Result<StartupWatch, CollectError> {
        StartupWatch::new(env)
    }

    async fn subscribe(&self, cx: &Cx) -> Result<UnitFilesSignals, DbusError> {
        UnitFilesSignals::open(cx).await
    }
}

impl FileEvents for StartupWatch {
    fn drain(&mut self) -> Result<bool, CollectError> {
        StartupWatch::drain(self)
    }

    fn rewatch(&mut self) {
        StartupWatch::rewatch(self);
    }
}

/// Queued `UnitFilesChanged` signals before the oldest is dropped. The worker drains the queue
/// every `IDLE_RECHECK`.
const SIGNAL_QUEUE: usize = 64;

/// The real subscription. Fields drop in order, so the stream (and its match rule) goes before
/// the connection's tick task stops. No proxy is kept.
struct UnitFilesSignals {
    stream: zbus::MessageStream,
    conn: DbusConn,
}

impl UnitFilesSignals {
    /// Connects to the session bus, adds the match rule, then calls `Manager.Subscribe` so the
    /// user manager sends `UnitFilesChanged`.
    async fn open(cx: &Cx) -> Result<Self, DbusError> {
        let conn = connect(cx, Bus::Session).await?;
        let rule = || -> zbus::Result<_> {
            Ok(zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(BUS_NAME)?
                .path(PATH)?
                .interface(MANAGER_IFACE)?
                .member("UnitFilesChanged")?
                .build()
                .to_owned())
        };
        let rule = rule().map_err(|e| DbusError::Call {
            name: "zbus".into(),
            message: e.to_string().into(),
        })?;
        let raw = conn.raw();
        let stream = conn
            .read(
                cx,
                zbus::MessageStream::for_match_rule(rule, raw, Some(SIGNAL_QUEUE)),
            )
            .await?;
        // A fresh connection, so `AlreadySubscribed` cannot happen.
        let subscribe =
            raw.call_method(Some(BUS_NAME), PATH, Some(MANAGER_IFACE), "Subscribe", &());
        conn.read(cx, subscribe).await?;
        Ok(Self { stream, conn })
    }
}

impl UnitFileSignals for UnitFilesSignals {
    async fn drain(&mut self) -> Result<bool, DbusError> {
        let mut any = false;
        loop {
            match futures_lite::future::poll_once(self.stream.next()).await {
                None => return Ok(any),
                Some(Some(Ok(_))) => any = true,
                Some(Some(Err(e))) => {
                    tracing::debug!(target: "tasklens::engine", error = %e, "startup: bad signal message")
                }
                Some(None) => return Err(DbusError::Closed),
            }
        }
    }
}

/// The live-refresh sources while the tab is open. A source that failed is `None`. Dropping
/// this closes the inotify fd and the bus connection.
struct Live<F, S> {
    files: Option<F>,
    signals: Option<S>,
}

impl<F: FileEvents, S: UnitFileSignals> Live<F, S> {
    /// Creation failures are logged at debug and that source is skipped.
    async fn open<H: Host<Files = F, Signals = S>>(cx: &Cx, host: &H) -> Self {
        let files = match host.watch(&host.input().env) {
            Ok(files) => Some(files),
            Err(e) => {
                tracing::debug!(target: "tasklens::engine", error = %e, "startup watch failed");
                None
            }
        };
        let signals = match host.subscribe(cx).await {
            Ok(signals) => Some(signals),
            Err(e) => {
                tracing::debug!(target: "tasklens::engine", error = %e, "startup: no UnitFilesChanged");
                None
            }
        };
        Self { files, signals }
    }

    /// True if either source had an event since the last call. A source that fails is dropped
    /// until the tab opens again.
    async fn drain(&mut self) -> bool {
        let mut changed = false;
        if let Some(files) = self.files.as_mut() {
            match files.drain() {
                Ok(any) => changed |= any,
                Err(e) => {
                    tracing::debug!(target: "tasklens::engine", error = %e, "startup watch lost");
                    self.files = None;
                }
            }
        }
        if let Some(signals) = self.signals.as_mut() {
            match signals.drain().await {
                Ok(any) => changed |= any,
                Err(e) => {
                    tracing::debug!(target: "tasklens::engine", error = %e, "startup: UnitFilesChanged lost");
                    self.signals = None;
                }
            }
        }
        changed
    }
}

/// Where this generation is in the forwarded command list (R-276).
struct Queue {
    last_seq: u64,
    progress: Arc<CommandProgress>,
}

impl Queue {
    /// Starts after the last command an earlier generation started. A RefreshNow that was
    /// running when that generation died is dropped: the next open or refresh scans again.
    fn resume(io: &WorkerIo) -> Self {
        let progress = Arc::clone(&io.control.borrow_and_clone().progress);
        let last_seq = progress.started(SourceId::Startup);
        if last_seq > progress.done(SourceId::Startup) {
            progress.set_done(SourceId::Startup, last_seq);
        }
        Self { last_seq, progress }
    }

    /// Takes the Startup commands queued since the last wake. When a RefreshNow is
    /// among them, returns the newest seq to mark done after the scan. Other commands alone are
    /// counted as done at once.
    fn take(&mut self, io: &WorkerIo) -> Option<u64> {
        let control = io.control.borrow_and_clone();
        let mut refresh = None;
        for queued in control
            .commands
            .iter()
            .filter(|c| c.seq > self.last_seq && c.target == SourceId::Startup)
        {
            self.progress.set_started(SourceId::Startup, queued.seq);
            // A command after a pending RefreshNow is done when that scan is done.
            if queued.command == WorkerCommand::RefreshNow || refresh.is_some() {
                refresh = Some(queued.seq);
            } else {
                self.progress.set_done(SourceId::Startup, queued.seq);
            }
        }
        if let Some(newest) = control.commands.last() {
            self.last_seq = self.last_seq.max(newest.seq);
        }
        refresh
    }
}

/// The previous scan's rows and entries by identity, so unchanged entries keep their `Arc`s.
#[derive(Default)]
struct Cache {
    rows: FxHashMap<StartupRef, (StartupRaw, StartupEntry)>,
}

impl Cache {
    /// Maps `raws` to entries in scan order. A row equal to the previous scan's row with the
    /// same `StartupRef` reuses the previous entry.
    fn merge(&mut self, raws: Vec<StartupRaw>, home: Option<&Path>) -> Vec<StartupEntry> {
        let mut next = FxHashMap::default();
        let mut entries = Vec::with_capacity(raws.len());
        for raw in raws {
            let id = startup_ref(&raw);
            let entry = match self.rows.remove(&id) {
                Some((old, entry)) if old == raw => entry,
                _ => to_entry(id.clone(), &raw, home),
            };
            entries.push(entry.clone());
            next.entry(id).or_insert((raw, entry));
        }
        self.rows = next;
        entries
    }
}

fn startup_ref(raw: &StartupRaw) -> StartupRef {
    let source = match raw.kind {
        StartupKindRaw::AutostartUser => StartupSource::AutostartUser,
        StartupKindRaw::AutostartSystem | StartupKindRaw::AutostartSystemOverridden => {
            StartupSource::AutostartSystem
        }
        StartupKindRaw::UserUnit => StartupSource::UserUnit,
        StartupKindRaw::PlasmaPreStartup => StartupSource::PlasmaEnv,
        StartupKindRaw::PlasmaLogout => StartupSource::PlasmaShutdown,
    };
    StartupRef {
        source,
        id: Arc::clone(&raw.file_name),
    }
}

/// One model entry. Impact is `NotMeasured` until E14-013 reads the systemd timestamps. The
/// publisher is "User" for files under `$HOME` and blank otherwise; package lookup is not done
/// here. Only an absolute `Icon=` is an icon path.
fn to_entry(id: StartupRef, raw: &StartupRaw, home: Option<&Path>) -> StartupEntry {
    let under_home = home.is_some_and(|home| Path::new(&*raw.source_path).starts_with(home));
    StartupEntry {
        id,
        name: Arc::clone(&raw.name),
        icon_path: raw
            .icon
            .as_ref()
            .filter(|icon| Path::new(&***icon).is_absolute())
            .cloned(),
        publisher: under_home.then(|| Arc::from("User")),
        status: match raw.status {
            StartupStatusRaw::Enabled => StartupStatus::Enabled,
            StartupStatusRaw::Disabled => StartupStatus::Disabled,
            StartupStatusRaw::Masked => StartupStatus::Masked,
            StartupStatusRaw::NotInThisDesktop => StartupStatus::NotInThisDesktop,
            StartupStatusRaw::MissingProgram => StartupStatus::MissingProgram,
        },
        impact: Impact::NotMeasured,
        impact_detail: None,
        kind: match raw.kind {
            StartupKindRaw::AutostartUser => StartupKind::AutostartUser,
            StartupKindRaw::AutostartSystem => StartupKind::AutostartSystem,
            StartupKindRaw::AutostartSystemOverridden => StartupKind::AutostartSystemOverridden,
            StartupKindRaw::UserUnit => StartupKind::UserUnit,
            StartupKindRaw::PlasmaPreStartup => StartupKind::PlasmaPreStartup,
            StartupKindRaw::PlasmaLogout => StartupKind::PlasmaLogout,
        },
        location: Arc::clone(&raw.location),
        command: Arc::clone(&raw.command),
        source_path: Arc::clone(&raw.source_path),
        can_toggle: raw.can_toggle,
        can_delete: raw.can_delete,
        can_run: raw.can_run,
        overridden_by_user: raw.overridden_by_user,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use asupersync::channel::watch;
    use asupersync::types::{Budget, RegionId};
    use asupersync::{LabConfig, LabRuntime};
    use smallvec::SmallVec;
    use tl_model::demand::Demand;
    use tl_model::settings::EngineSettings;

    use crate::source::{QueuedCommand, WorkerControl};

    fn raw(kind: StartupKindRaw, file: &str) -> StartupRaw {
        let path = format!("/home/u/.config/autostart/{file}");
        StartupRaw {
            kind,
            file_name: Arc::from(file),
            source_path: Arc::from(path.as_str()),
            location: Arc::from("/home/u/.config/autostart"),
            name: Arc::from(file.trim_end_matches(".desktop")),
            icon: None,
            command: Arc::from("tool --flag"),
            resolved_path: None,
            status: StartupStatusRaw::Enabled,
            try_exec: None,
            only_show_in: SmallVec::new(),
            not_show_in: SmallVec::new(),
            kde_autostart_phase: None,
            generated_unit: None,
            overrides: None,
            overridden_by_user: false,
            tasklens_override: false,
            unit_file_state: None,
            service_type: None,
            can_toggle: true,
            can_delete: true,
            can_run: true,
            file_exists: true,
        }
    }

    /// Returns the same two rows on every call and counts the calls.
    fn fake_scanner() -> (Scanner, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let scanner: Scanner = Arc::new(move |_input: &ScanInput| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(vec![
                raw(StartupKindRaw::AutostartUser, "a.desktop"),
                raw(StartupKindRaw::AutostartUser, "b.desktop"),
            ])
        });
        (scanner, calls)
    }

    /// Fake live-refresh sources. Tests push events with `file_events` and `signal_events`;
    /// `files_open` and `signals_open` count the handles the worker holds.
    #[derive(Clone, Default)]
    struct FakeHost {
        file_events: Arc<AtomicUsize>,
        signal_events: Arc<AtomicUsize>,
        files_open: Arc<AtomicUsize>,
        signals_open: Arc<AtomicUsize>,
        /// The bus connector fails, as with no session bus.
        no_bus: bool,
    }

    struct FakeFiles {
        events: Arc<AtomicUsize>,
        open: Arc<AtomicUsize>,
    }

    impl Drop for FakeFiles {
        fn drop(&mut self) {
            self.open.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl FileEvents for FakeFiles {
        fn drain(&mut self) -> Result<bool, CollectError> {
            Ok(self.events.swap(0, Ordering::SeqCst) > 0)
        }

        fn rewatch(&mut self) {}
    }

    struct FakeSignals {
        events: Arc<AtomicUsize>,
        open: Arc<AtomicUsize>,
    }

    impl Drop for FakeSignals {
        fn drop(&mut self) {
            self.open.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl UnitFileSignals for FakeSignals {
        async fn drain(&mut self) -> Result<bool, DbusError> {
            Ok(self.events.swap(0, Ordering::SeqCst) > 0)
        }
    }

    impl Host for FakeHost {
        type Files = FakeFiles;
        type Signals = FakeSignals;

        fn input(&self) -> ScanInput {
            ScanInput::default()
        }

        fn watch(&self, _env: &StartupEnv) -> Result<FakeFiles, CollectError> {
            self.files_open.fetch_add(1, Ordering::SeqCst);
            Ok(FakeFiles {
                events: Arc::clone(&self.file_events),
                open: Arc::clone(&self.files_open),
            })
        }

        async fn subscribe(&self, _cx: &Cx) -> Result<FakeSignals, DbusError> {
            if self.no_bus {
                return Err(DbusError::Connect("no session bus".into()));
            }
            self.signals_open.fetch_add(1, Ordering::SeqCst);
            Ok(FakeSignals {
                events: Arc::clone(&self.signal_events),
                open: Arc::clone(&self.signals_open),
            })
        }
    }

    struct Rig {
        lab: LabRuntime,
        root: RegionId,
        rx: mpsc::Receiver<EngineMsg>,
        io: WorkerIo,
        demand_tx: watch::Sender<Demand>,
        control_tx: watch::Sender<WorkerControl>,
        _settings_tx: watch::Sender<EngineSettings>,
        progress: Arc<CommandProgress>,
        queue: Vec<QueuedCommand>,
        handle: Option<asupersync::runtime::TaskHandle<()>>,
    }

    impl Rig {
        fn new() -> Self {
            let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(1_000_000));
            let root = lab.state.create_root_region(Budget::INFINITE);
            let (updates, rx) = mpsc::channel::<EngineMsg>(256);
            let (demand_tx, demand) = watch::channel(Demand::default());
            let (settings_tx, settings) = watch::channel(EngineSettings::default());
            let control = WorkerControl::default();
            let progress = Arc::clone(&control.progress);
            let (control_tx, control) = watch::channel(control);
            let io = WorkerIo::new(
                updates,
                demand,
                settings,
                control,
                crate::dbus::SharedHandles::default(),
            );
            Rig {
                lab,
                root,
                rx,
                io,
                demand_tx,
                control_tx,
                _settings_tx: settings_tx,
                progress,
                queue: Vec::new(),
                handle: None,
            }
        }

        fn start(&mut self, scanner: Scanner) {
            self.start_with(scanner, FakeHost::default());
        }

        fn start_with(&mut self, scanner: Scanner, host: FakeHost) {
            let io = self.io.clone();
            let (task, handle) = self
                .lab
                .state
                .create_task(self.root, Budget::INFINITE, async move {
                    let cx = Cx::current().expect("task has a Cx");
                    let _ = run(cx, io, scanner, host).await;
                })
                .expect("task is created");
            self.lab.scheduler.lock().schedule(task, 0);
            self.lab.run_until_idle();
            self.handle = Some(handle);
        }

        fn set_open(&mut self, open: bool) {
            let demand = Demand {
                startup: open,
                ..Demand::default()
            };
            self.demand_tx.send(demand).expect("watch has receivers");
        }

        /// What the reducer's `forward()` does.
        fn refresh_now(&mut self) {
            let seq = self.queue.last().map_or(0, |c| c.seq) + 1;
            self.queue.push(QueuedCommand {
                seq,
                target: SourceId::Startup,
                command: WorkerCommand::RefreshNow,
            });
            let control = WorkerControl {
                reset_generation: 0,
                commands: self.queue.clone().into(),
                progress: Arc::clone(&self.progress),
            };
            self.control_tx.send(control).expect("watch has receivers");
        }

        fn advance(&mut self, d: Duration) {
            self.lab
                .advance_time(u64::try_from(d.as_nanos()).expect("fits in u64"));
            // `run_until_idle` stops when no task is ready; one step fires the due timers.
            self.lab.step_for_test();
            self.lab.run_until_idle();
        }

        /// Advances `total` in 100 ms steps.
        fn run_for(&mut self, total: Duration) {
            let step = Duration::from_millis(100);
            let mut left = total;
            while !left.is_zero() {
                let d = step.min(left);
                self.advance(d);
                left -= d;
            }
        }

        fn updates(&mut self) -> Vec<StartupUpdate> {
            let mut out = Vec::new();
            while let Ok(msg) = self.rx.try_recv() {
                match msg {
                    EngineMsg::Source(SourceUpdate::Startup(update)) => out.push(update),
                    other => panic!("unexpected message {other:?}"),
                }
            }
            out
        }

        fn finish(mut self) {
            if let Some(handle) = self.handle.take() {
                handle.abort();
            }
            let virtual_time = self.lab.run_with_auto_advance();
            assert_ne!(
                virtual_time.termination,
                asupersync::lab::AutoAdvanceTermination::StepLimitReached,
                "{virtual_time:?}"
            );
            let report = self.lab.run_until_quiescent_with_report();
            assert!(report.lab_test_passed(), "{report:?}");
        }
    }

    #[test]
    fn startup_no_scan_when_hidden() {
        let mut rig = Rig::new();
        let (scanner, calls) = fake_scanner();
        rig.start(scanner);
        rig.run_for(Duration::from_secs(60));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(rig.updates().is_empty());
        rig.finish();
    }

    #[test]
    fn startup_scan_on_open() {
        let mut rig = Rig::new();
        let (scanner, calls) = fake_scanner();
        rig.start(scanner);
        rig.run_for(Duration::from_millis(1_250));
        rig.set_open(true);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let updates = rig.updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].entries.len(), 2);
        assert_eq!(updates[0].validity, Validity::Valid);
        rig.finish();
    }

    #[test]
    fn startup_no_rescan_while_open() {
        let mut rig = Rig::new();
        let (scanner, calls) = fake_scanner();
        rig.start(scanner);
        rig.set_open(true);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        rig.run_for(Duration::from_secs(60));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(rig.updates().len(), 1);

        // Closing and opening again scans once more.
        rig.set_open(false);
        rig.run_for(Duration::from_secs(1));
        rig.set_open(true);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        rig.finish();
    }

    #[test]
    fn startup_scan_on_refresh_now() {
        let mut rig = Rig::new();
        let (scanner, calls) = fake_scanner();
        rig.start(scanner);

        // Hidden: the refresh is counted as done without a scan.
        rig.refresh_now();
        rig.run_for(Duration::from_secs(1));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(rig.progress.done(SourceId::Startup), 1);

        rig.set_open(true);
        rig.run_for(Duration::from_secs(5));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        rig.refresh_now();
        rig.run_for(Duration::from_millis(500));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(rig.progress.done(SourceId::Startup), 2);
        rig.run_for(Duration::from_secs(5));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(rig.updates().len(), 2);
        rig.finish();
    }

    #[test]
    fn startup_unchanged_entries_keep_arc() {
        let mut rig = Rig::new();
        let (scanner, _calls) = fake_scanner();
        rig.start(scanner);
        rig.set_open(true);
        rig.run_for(Duration::from_millis(500));
        rig.refresh_now();
        rig.run_for(Duration::from_millis(500));
        let updates = rig.updates();
        assert_eq!(updates.len(), 2);
        let (first, second) = (&updates[0].entries, &updates[1].entries);
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second) {
            assert_eq!(a.id, b.id);
            assert!(Arc::ptr_eq(&a.name, &b.name), "{:?}", a.id);
            assert!(Arc::ptr_eq(&a.command, &b.command), "{:?}", a.id);
            assert!(Arc::ptr_eq(&a.source_path, &b.source_path), "{:?}", a.id);
            assert!(Arc::ptr_eq(&a.location, &b.location), "{:?}", a.id);
            assert!(Arc::ptr_eq(&a.id.id, &b.id.id), "{:?}", a.id);
        }
        rig.finish();
    }

    #[test]
    fn events_coalesce_to_one_rescan() {
        let mut rig = Rig::new();
        let (scanner, calls) = fake_scanner();
        let host = FakeHost::default();
        rig.start_with(scanner, host.clone());
        rig.set_open(true);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Three file changes and two UnitFilesChanged within one 500 ms window.
        host.file_events.fetch_add(3, Ordering::SeqCst);
        host.signal_events.fetch_add(2, Ordering::SeqCst);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        rig.run_for(Duration::from_secs(5));
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // A signal alone also rescans.
        host.signal_events.fetch_add(1, Ordering::SeqCst);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(rig.updates().len(), 3);
        rig.finish();
    }

    #[test]
    fn watches_closed_when_hidden() {
        let mut rig = Rig::new();
        let (scanner, calls) = fake_scanner();
        let host = FakeHost::default();
        rig.start_with(scanner, host.clone());
        rig.run_for(Duration::from_secs(1));
        assert_eq!(host.files_open.load(Ordering::SeqCst), 0);
        assert_eq!(host.signals_open.load(Ordering::SeqCst), 0);

        rig.set_open(true);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(host.files_open.load(Ordering::SeqCst), 1);
        assert_eq!(host.signals_open.load(Ordering::SeqCst), 1);

        // Hidden: both handles are dropped and events give no rescan.
        rig.set_open(false);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(host.files_open.load(Ordering::SeqCst), 0);
        assert_eq!(host.signals_open.load(Ordering::SeqCst), 0);
        host.file_events.fetch_add(1, Ordering::SeqCst);
        host.signal_events.fetch_add(1, Ordering::SeqCst);
        rig.run_for(Duration::from_secs(2));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Opening again creates them again.
        host.file_events.store(0, Ordering::SeqCst);
        host.signal_events.store(0, Ordering::SeqCst);
        rig.set_open(true);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(host.files_open.load(Ordering::SeqCst), 1);
        assert_eq!(host.signals_open.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        rig.finish();
    }

    #[test]
    fn no_session_bus_keeps_inotify() {
        let mut rig = Rig::new();
        let (scanner, calls) = fake_scanner();
        let host = FakeHost {
            no_bus: true,
            ..FakeHost::default()
        };
        rig.start_with(scanner, host.clone());
        rig.set_open(true);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.files_open.load(Ordering::SeqCst), 1);
        assert_eq!(host.signals_open.load(Ordering::SeqCst), 0);

        host.file_events.fetch_add(1, Ordering::SeqCst);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(rig.updates().len(), 2);
        rig.finish();
    }

    /// A temp directory as the whole startup environment: real inotify, no session bus.
    struct TempHost {
        input: ScanInput,
    }

    impl Host for TempHost {
        type Files = StartupWatch;
        type Signals = FakeSignals;

        fn input(&self) -> ScanInput {
            self.input.clone()
        }

        fn watch(&self, env: &StartupEnv) -> Result<StartupWatch, CollectError> {
            StartupWatch::new(env)
        }

        async fn subscribe(&self, _cx: &Cx) -> Result<FakeSignals, DbusError> {
            Err(DbusError::Connect("no session bus".into()))
        }
    }

    /// The next Startup update, or `None` when none comes within `d`.
    async fn recv_startup(
        cx: &Cx,
        rx: &mut mpsc::Receiver<EngineMsg>,
        d: Duration,
    ) -> Option<StartupUpdate> {
        match asupersync::time::timeout(cx.now(), d, rx.recv(cx)).await {
            Ok(Ok(EngineMsg::Source(SourceUpdate::Startup(update)))) => Some(update),
            Ok(other) => panic!("unexpected {other:?}"),
            Err(_) => None,
        }
    }

    /// Real temp directory, real inotify, real runtime and wall-clock time.
    #[test]
    fn inotify_rescan_on_new_file() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let config_home = tmp.path().join("home/.config");
        let autostart = config_home.join("autostart");
        std::fs::create_dir_all(&autostart).expect("autostart dir");
        let host = TempHost {
            input: ScanInput {
                env: StartupEnv {
                    config_home,
                    config_dirs: vec![tmp.path().join("xdg")],
                    ..StartupEnv::default()
                },
                home: Some(tmp.path().join("home")),
                units: Vec::new(),
            },
        };

        let (updates, mut rx) = mpsc::channel::<EngineMsg>(16);
        let (_demand_tx, demand) = watch::channel(Demand {
            startup: true,
            ..Demand::default()
        });
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (_control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(
            updates,
            demand,
            settings,
            control,
            crate::dbus::SharedHandles::default(),
        );

        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async move {
            let cx = Cx::current().expect("task has a Cx");
            let scanner: Scanner = Arc::new(host_scan);
            let mut worker = cx
                .spawn(move |cx| async move {
                    let _ = run(cx, io, scanner, host).await;
                })
                .expect("worker spawns");
            let first = recv_startup(&cx, &mut rx, Duration::from_secs(5))
                .await
                .expect("scan on open");
            assert!(first.entries.is_empty(), "{:?}", first.entries);

            std::fs::write(
                autostart.join("new.desktop"),
                "[Desktop Entry]\nType=Application\nName=New\nExec=/bin/true\n",
            )
            .expect("write desktop file");
            let second = recv_startup(&cx, &mut rx, Duration::from_secs(1)).await;

            worker.abort();
            let _ = worker.join(&cx).await;
            second
        });
        let second = rt
            .block_on(join)
            .expect("task completes")
            .expect("a rescan within 1 s");
        assert_eq!(second.entries.len(), 1, "{:?}", second.entries);
        assert_eq!(&*second.entries[0].id.id, "new.desktop");
        assert_eq!(second.entries[0].kind, StartupKind::AutostartUser);
    }

    #[test]
    fn merge_replaces_changed_entries_only() {
        let mut cache = Cache::default();
        let first = cache.merge(
            vec![
                raw(StartupKindRaw::AutostartUser, "a.desktop"),
                raw(StartupKindRaw::AutostartUser, "b.desktop"),
            ],
            None,
        );
        // A new scan builds new strings; only the changed row gets a new entry.
        let mut b = raw(StartupKindRaw::AutostartUser, "b.desktop");
        b.status = StartupStatusRaw::Disabled;
        let second = cache.merge(
            vec![raw(StartupKindRaw::AutostartUser, "a.desktop"), b],
            None,
        );
        assert!(Arc::ptr_eq(&first[0].name, &second[0].name));
        assert!(!Arc::ptr_eq(&first[1].name, &second[1].name));
        assert_eq!(second[1].status, StartupStatus::Disabled);
    }

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tl-collect/tests/fixtures/autostart-kde")
    }

    /// The real scanner over the committed `autostart-kde` fixture, mapped to model entries.
    #[test]
    fn host_scan_maps_fixture() {
        let root = fixture_root();
        let unit = root.join("home/.config/systemd/user/sync.service");
        let input = ScanInput {
            env: StartupEnv {
                config_home: root.join("home/.config"),
                config_dirs: vec![root.join("etc/xdg"), root.join("etc/xdg2")],
                current_desktop: vec!["KDE".to_owned()],
                path_dirs: vec![root.join("usr/bin")],
                locales: SmallVec::new(),
            },
            home: Some(root.join("home")),
            units: vec![(
                unit.to_str().expect("utf-8").to_owned(),
                "enabled".to_owned(),
            )],
        };
        let raws = host_scan(&input).expect("fixture scans");
        assert!(!raws.is_empty());
        let entries = Cache::default().merge(raws.clone(), input.home.as_deref());
        assert_eq!(entries.len(), raws.len());

        let user = entries
            .iter()
            .find(|e| e.kind == StartupKind::AutostartUser)
            .expect("a user autostart row");
        assert_eq!(user.id.source, StartupSource::AutostartUser);
        assert_eq!(user.publisher.as_deref(), Some("User"));

        let system = entries
            .iter()
            .find(|e| e.kind == StartupKind::AutostartSystem)
            .expect("a system autostart row");
        assert_eq!(system.id.source, StartupSource::AutostartSystem);
        assert_eq!(system.publisher, None);

        let overridden = entries
            .iter()
            .find(|e| e.kind == StartupKind::AutostartSystemOverridden)
            .expect("an overridden system row");
        assert!(overridden.overridden_by_user);
        assert_eq!(overridden.id.source, StartupSource::AutostartSystem);

        let unit = entries
            .iter()
            .find(|e| e.kind == StartupKind::UserUnit)
            .expect("the sync.service row");
        assert_eq!(&*unit.id.id, "sync.service");
        assert_eq!(unit.status, StartupStatus::Enabled);
        assert!(entries.iter().all(|e| e.impact == Impact::NotMeasured));
    }

    #[test]
    fn spec_is_full_mode_only() {
        let s = spec();
        assert_eq!(s.id, SourceId::Startup);
        assert_eq!(s.name, "startup");
        assert_eq!(s.modes, &[EngineMode::Full]);
        let full = crate::workers::workers_for(EngineMode::Full);
        assert_eq!(full.iter().filter(|w| w.id == SourceId::Startup).count(), 1);
    }
}
