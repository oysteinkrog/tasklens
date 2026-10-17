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
//! Each scan also asks the user manager on the session bus (SYS-019 to SYS-021, SYS-026): the
//! unit files with their state, which feed the user unit rows, and the loaded units. An entry's
//! unit is its generated `app-<escaped id>@autostart.service` or, for a user unit row, the unit
//! itself. A loaded unit gives the state code ("active (running)"). For enabled entries one
//! `GetAll` on the unit gives the activation timestamps and CPU time behind the impact. Every
//! call has the 2 s read timeout. Without a session bus the entries keep the scanner's values,
//! impact Not measured (None when disabled) and no state code.
//!
//! `command` holds `Exec` lines, which can carry secrets. Never log it or file contents.

use std::collections::HashMap;
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
    ImpactRaw, StartupEnv, StartupKindRaw, StartupRaw, StartupStatusRaw, StartupWatch,
    UnitFileListing, UnitTiming, classify, scan_startup,
};
use tl_model::health::SourceId;
use tl_model::snapshot::{
    Impact, ImpactDetail, StartupEntry, StartupKind, StartupScope, StartupStatus,
};
use tl_model::table::{StartupRef, StartupSource};
use tl_model::validity::Validity;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

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
    /// `$XDG_DATA_HOME` when set and absolute. User units there are user scope.
    pub data_home: Option<PathBuf>,
    /// `$XDG_RUNTIME_DIR` when set and absolute. User units there are user scope.
    pub runtime_dir: Option<PathBuf>,
    /// The user manager's unit files as (path, state), from `ListUnitFilesByPatterns`. Empty
    /// without a session bus.
    pub units: Vec<(String, String)>,
}

impl ScanInput {
    fn from_env() -> Self {
        Self {
            env: StartupEnv::from_env(),
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute()),
            data_home: std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute()),
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute()),
            units: Vec::new(),
        }
    }
}

/// Runs on the blocking pool. Tests replace it with a scripted fake.
pub type Scanner = Arc<dyn Fn(&ScanInput) -> Result<Vec<StartupRaw>, CollectError> + Send + Sync>;

/// `$HOME`, `$XDG_CONFIG_HOME`, `$XDG_DATA_HOME` and `$XDG_RUNTIME_DIR`. A file under any is a
/// user file.
#[derive(Clone, Copy, Default)]
struct UserDirs<'a> {
    home: Option<&'a Path>,
    config_home: Option<&'a Path>,
    data_home: Option<&'a Path>,
    runtime_dir: Option<&'a Path>,
}

impl<'a> UserDirs<'a> {
    /// A relative `config_home` matches nothing.
    fn new(home: Option<&'a Path>, config_home: &'a Path) -> Self {
        Self {
            home,
            config_home: Some(config_home).filter(|p| p.is_absolute()),
            data_home: None,
            runtime_dir: None,
        }
    }

    /// A relative `data_home` and a relative `runtime_dir` match nothing.
    fn with_data_home(self, data_home: Option<&'a Path>, runtime_dir: Option<&'a Path>) -> Self {
        Self {
            data_home: data_home.filter(|p| p.is_absolute()),
            runtime_dir: runtime_dir.filter(|p| p.is_absolute()),
            ..self
        }
    }
}

/// The real scanner: XDG autostart, user units and, on KDE, Plasma scripts. A row keeps
/// `file_exists` only when its program also exists: a resolved path that is a file after
/// following symlinks. This runs here, on the blocking pool, because it reads the disk.
fn host_scan(input: &ScanInput) -> Result<Vec<StartupRaw>, CollectError> {
    let units: Vec<UnitFileListing<'_>> = input
        .units
        .iter()
        .map(|(path, state)| UnitFileListing { path, state })
        .collect();
    let mut out = Vec::new();
    scan_startup(&input.env, &units, &mut out)?;
    for raw in &mut out {
        raw.file_exists &= raw
            .resolved_path
            .as_deref()
            .is_some_and(|program| Path::new(program).is_file());
    }
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
        let mut input = host.input();
        let home = input.home.clone();
        let config_home = input.env.config_home.clone();
        let data_home = input.data_home.clone();
        let runtime_dir = input.runtime_dir.clone();
        let mut systemd = Systemd::default();
        // Kept for the timing reads after the scan, then dropped with the scan.
        let mut manager = match host.manager(&cx).await {
            Ok(manager) => Some(manager),
            Err(e) => {
                tracing::debug!(target: "tasklens::engine", error = %e, "startup: no user manager");
                None
            }
        };
        if let Some(m) = manager.as_mut() {
            match m.list(&cx).await {
                Ok(listing) => {
                    input.units = listing.files;
                    systemd.loaded = listing.loaded;
                    systemd.userspace_us = listing.userspace_us;
                }
                Err(e) => {
                    tracing::debug!(target: "tasklens::engine", error = %e, "startup: unit listing failed");
                    manager = None;
                }
            }
        }
        if cx.checkpoint().is_err() {
            return Outcome::Ok(());
        }

        let scan = Arc::clone(&scanner);
        let mut job = match cx.spawn_blocking(move |_| scan(&input)) {
            Ok(job) => job,
            Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
        };
        let scanned = match job.join(&cx).await {
            Ok(scanned) => scanned,
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

        match scanned {
            Ok(raws) => {
                if let Some(m) = manager.as_mut() {
                    systemd.read_timings(&cx, m, &raws).await;
                    if cx.checkpoint().is_err() {
                        return Outcome::Ok(());
                    }
                }
                drop(manager);
                let dirs = UserDirs::new(home.as_deref(), &config_home)
                    .with_data_home(data_home.as_deref(), runtime_dir.as_deref());
                let entries = cache.merge(raws, &systemd, dirs);
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
    type Manager: UserManager;
    /// The scan input. Read once per scan and once when the tab opens.
    fn input(&self) -> ScanInput;
    /// An inotify watch on the startup directories of `env`.
    fn watch(&self, env: &StartupEnv) -> Result<Self::Files, CollectError>;
    /// A session-bus subscription to the user manager's `UnitFilesChanged`.
    async fn subscribe(&self, cx: &Cx) -> Result<Self::Signals, DbusError>;
    /// A session-bus connection to the user manager, for one scan.
    async fn manager(&self, cx: &Cx) -> Result<Self::Manager, DbusError>;
}

/// Reads from the user manager. Each call has the 2 s read timeout.
trait UserManager {
    /// `ListUnitFilesByPatterns([], ["*"])`, `ListUnitsByPatterns([], ["*"])` and the manager
    /// property `UserspaceTimestampMonotonic`.
    async fn list(&mut self, cx: &Cx) -> Result<Listing, DbusError>;
    /// `GetAll` on the unit at `path`, over every interface, so `CPUUsageNSec` of
    /// `org.freedesktop.systemd1.Service` comes in the same call.
    async fn timing(&mut self, cx: &Cx, path: &str) -> Result<Timing, DbusError>;
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
    type Manager = SessionManager;

    fn input(&self) -> ScanInput {
        ScanInput::from_env()
    }

    fn watch(&self, env: &StartupEnv) -> Result<StartupWatch, CollectError> {
        StartupWatch::new(env)
    }

    async fn subscribe(&self, cx: &Cx) -> Result<UnitFilesSignals, DbusError> {
        UnitFilesSignals::open(cx).await
    }

    async fn manager(&self, cx: &Cx) -> Result<SessionManager, DbusError> {
        Ok(SessionManager {
            conn: connect(cx, Bus::Session).await?,
        })
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
    /// Kept so its tick task feeds the stream.
    _conn: DbusConn,
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
        Ok(Self {
            stream,
            _conn: conn,
        })
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

/// `ListUnitsByPatterns` row: name, description, load state, active state, sub state,
/// following, object path, job id, job type, job path.
type ListedUnit = (
    String,
    String,
    String,
    String,
    String,
    String,
    OwnedObjectPath,
    u32,
    String,
    OwnedObjectPath,
);

/// A property map from `GetAll`.
type Props = HashMap<String, OwnedValue>;

const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";

/// The real user manager over its own session-bus connection. No proxy is kept.
struct SessionManager {
    conn: DbusConn,
}

impl SessionManager {
    async fn manager_call<B, T>(&self, cx: &Cx, method: &str, body: &B) -> Result<T, DbusError>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        T: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let call =
            self.conn
                .raw()
                .call_method(Some(BUS_NAME), PATH, Some(MANAGER_IFACE), method, body);
        let reply = self.conn.read(cx, call).await?;
        reply.body().deserialize::<T>().map_err(decode_error)
    }
}

impl UserManager for SessionManager {
    async fn list(&mut self, cx: &Cx) -> Result<Listing, DbusError> {
        let patterns = (Vec::<&str>::new(), vec!["*"]);
        let files: Vec<(String, String)> = self
            .manager_call(cx, "ListUnitFilesByPatterns", &patterns)
            .await?;
        let units: Vec<ListedUnit> = self
            .manager_call(cx, "ListUnitsByPatterns", &patterns)
            .await?;
        let get = self.conn.raw().call_method(
            Some(BUS_NAME),
            PATH,
            Some(PROPERTIES_IFACE),
            "Get",
            &(MANAGER_IFACE, "UserspaceTimestampMonotonic"),
        );
        let reply = self.conn.read(cx, get).await?;
        let userspace: OwnedValue = reply.body().deserialize().map_err(decode_error)?;
        let loaded = units
            .into_iter()
            .filter(|unit| unit.2 == "loaded")
            .map(|(name, _, _, active_state, sub_state, _, path, ..)| {
                let unit = LoadedUnit {
                    active_state,
                    sub_state,
                    path: path.as_str().to_owned(),
                };
                (name, unit)
            })
            .collect();
        Ok(Listing {
            files,
            loaded,
            userspace_us: value_u64(&userspace).unwrap_or(0),
        })
    }

    async fn timing(&mut self, cx: &Cx, path: &str) -> Result<Timing, DbusError> {
        let path = ObjectPath::try_from(path).map_err(decode_error)?;
        let call = self.conn.raw().call_method(
            Some(BUS_NAME),
            &path,
            Some(PROPERTIES_IFACE),
            "GetAll",
            &("",),
        );
        let reply = self.conn.read(cx, call).await?;
        let props: Props = reply.body().deserialize().map_err(decode_error)?;
        Ok(Timing::from_props(&props))
    }
}

fn decode_error(e: impl std::fmt::Display) -> DbusError {
    DbusError::Call {
        name: "zbus".into(),
        message: e.to_string().into(),
    }
}

/// A `u64` property value, looking through variant wrapping.
fn value_u64(v: &Value<'_>) -> Option<u64> {
    match v {
        Value::Value(inner) => value_u64(inner),
        Value::U64(n) => Some(*n),
        _ => None,
    }
}

/// What the user manager reported for one scan.
#[derive(Clone, Debug, Default)]
struct Listing {
    /// Unit files as (path, state).
    files: Vec<(String, String)>,
    /// Units with load state "loaded", by name.
    loaded: FxHashMap<String, LoadedUnit>,
    /// The manager's `UserspaceTimestampMonotonic`, in µs.
    userspace_us: u64,
}

/// One loaded unit of the user manager.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LoadedUnit {
    active_state: String,
    sub_state: String,
    /// The unit's object path.
    path: String,
}

/// The `GetAll` values behind the impact of one unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Timing {
    /// `InactiveExitTimestampMonotonic` in µs, 0 when missing.
    inactive_exit_us: u64,
    /// `ActiveEnterTimestampMonotonic` in µs, 0 when missing.
    active_enter_us: u64,
    /// `CPUUsageNSec`, `u64::MAX` when missing or not available.
    cpu_ns: u64,
}

impl Timing {
    fn from_props(props: &Props) -> Self {
        let get = |key: &str| props.get(key).and_then(|v| value_u64(v));
        Self {
            inactive_exit_us: get("InactiveExitTimestampMonotonic").unwrap_or(0),
            active_enter_us: get("ActiveEnterTimestampMonotonic").unwrap_or(0),
            cpu_ns: get("CPUUsageNSec").unwrap_or(u64::MAX),
        }
    }
}

/// The systemd side of one scan. `Default` is what a scan without a user manager uses.
#[derive(Clone, Debug, Default)]
struct Systemd {
    loaded: FxHashMap<String, LoadedUnit>,
    userspace_us: u64,
    /// Timings read this scan, by unit name.
    timings: FxHashMap<String, Timing>,
}

impl Systemd {
    /// One `GetAll` per enabled entry with a loaded unit. A failed call leaves that entry
    /// without timing (Not measured) and is logged at debug.
    async fn read_timings<M: UserManager>(&mut self, cx: &Cx, m: &mut M, raws: &[StartupRaw]) {
        for raw in raws {
            if raw.status != StartupStatusRaw::Enabled {
                continue;
            }
            let Some(name) = unit_of(raw) else {
                continue;
            };
            let Some(unit) = self.loaded.get(name) else {
                continue;
            };
            if self.timings.contains_key(name) {
                continue;
            }
            match m.timing(cx, &unit.path).await {
                Ok(timing) => {
                    self.timings.insert(name.to_owned(), timing);
                }
                Err(DbusError::Closed) => return,
                Err(e) => {
                    tracing::debug!(target: "tasklens::engine", unit = %name, error = %e, "startup: unit timing failed")
                }
            }
        }
    }

    /// The state code and impact of `raw` (SYS-021, SYS-026).
    fn facts(&self, raw: &StartupRaw) -> UnitFacts {
        let unit = unit_of(raw).and_then(|name| self.loaded.get(name).map(|unit| (name, unit)));
        let state_code =
            unit.map(|(_, unit)| Arc::from(format!("{} ({})", unit.active_state, unit.sub_state)));
        let timing = unit
            .and_then(|(name, _)| self.timings.get(name))
            .map(|t| UnitTiming {
                inactive_exit_mono_us: t.inactive_exit_us,
                active_enter_mono_us: t.active_enter_us,
                userspace_mono_us: self.userspace_us,
                cpu_usage_ns: t.cpu_ns,
                service_type: raw.service_type.clone(),
            });
        let (impact, detail) = classify(raw.status == StartupStatusRaw::Enabled, timing.as_ref());
        UnitFacts {
            state_code,
            impact: match impact {
                ImpactRaw::High => Impact::High,
                ImpactRaw::Medium => Impact::Medium,
                ImpactRaw::Low => Impact::Low,
                ImpactRaw::None => Impact::None,
                ImpactRaw::NotMeasured => Impact::NotMeasured,
            },
            impact_detail: detail.map(|d| ImpactDetail {
                activation_ms: d.activation_ms,
                started_after_login_ms: d.start_offset_ms,
                cpu_nsec: d.cpu_ns,
                fork_exec_only: d.fork_exec_only,
            }),
        }
    }
}

/// The unit that runs `raw`: a desktop entry's generated unit, or a user unit row's own unit.
/// A system entry shadowed by a user file has none: the generator reads the user file.
fn unit_of(raw: &StartupRaw) -> Option<&str> {
    match raw.kind {
        StartupKindRaw::AutostartUser | StartupKindRaw::AutostartSystem => {
            raw.generated_unit.as_deref()
        }
        StartupKindRaw::UserUnit => Some(&raw.file_name),
        StartupKindRaw::AutostartSystemOverridden
        | StartupKindRaw::PlasmaPreStartup
        | StartupKindRaw::PlasmaLogout => None,
    }
}

/// The systemd facts of one entry.
#[derive(Clone, Debug, PartialEq)]
struct UnitFacts {
    /// "<ActiveState> (<SubState>)" of the entry's loaded unit. `None` without one.
    state_code: Option<Arc<str>>,
    impact: Impact,
    impact_detail: Option<ImpactDetail>,
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
    rows: FxHashMap<StartupRef, (StartupRaw, UnitFacts, StartupEntry)>,
}

impl Cache {
    /// Maps `raws` to entries in scan order, with the systemd facts of `systemd`. A row and
    /// facts equal to the previous scan's with the same `StartupRef` reuse the previous entry.
    fn merge(
        &mut self,
        raws: Vec<StartupRaw>,
        systemd: &Systemd,
        dirs: UserDirs<'_>,
    ) -> Vec<StartupEntry> {
        let mut next = FxHashMap::default();
        let mut entries = Vec::with_capacity(raws.len());
        for raw in raws {
            let id = startup_ref(&raw);
            let facts = systemd.facts(&raw);
            let entry = match self.rows.remove(&id) {
                Some((old, old_facts, entry)) if old == raw && old_facts == facts => entry,
                _ => to_entry(id.clone(), &raw, &facts, dirs),
            };
            entries.push(entry.clone());
            next.entry(id).or_insert((raw, facts, entry));
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

/// One model entry. The publisher is "User" for files under `$HOME` and blank otherwise;
/// package lookup is not done here. Only an absolute `Icon=` is an icon path. Scope is user
/// for user autostart entries and Plasma scripts, system for system autostart entries, and
/// for user units it follows whether the unit file is under `$HOME`, `$XDG_CONFIG_HOME`, `$XDG_DATA_HOME` or `$XDG_RUNTIME_DIR`.
fn to_entry(
    id: StartupRef,
    raw: &StartupRaw,
    facts: &UnitFacts,
    dirs: UserDirs<'_>,
) -> StartupEntry {
    let source = Path::new(&*raw.source_path);
    let under_home = dirs.home.is_some_and(|home| source.starts_with(home));
    let under_user_dir = [dirs.config_home, dirs.data_home, dirs.runtime_dir]
        .into_iter()
        .flatten()
        .any(|dir| source.starts_with(dir));
    let kind = match raw.kind {
        StartupKindRaw::AutostartUser => StartupKind::AutostartUser,
        StartupKindRaw::AutostartSystem => StartupKind::AutostartSystem,
        StartupKindRaw::AutostartSystemOverridden => StartupKind::AutostartSystemOverridden,
        StartupKindRaw::UserUnit => StartupKind::UserUnit,
        StartupKindRaw::PlasmaPreStartup => StartupKind::PlasmaPreStartup,
        StartupKindRaw::PlasmaLogout => StartupKind::PlasmaLogout,
    };
    let scope = match kind {
        StartupKind::AutostartUser | StartupKind::PlasmaPreStartup | StartupKind::PlasmaLogout => {
            StartupScope::User
        }
        StartupKind::AutostartSystem | StartupKind::AutostartSystemOverridden => {
            StartupScope::System
        }
        StartupKind::UserUnit if under_home || under_user_dir => StartupScope::User,
        StartupKind::UserUnit => StartupScope::System,
    };
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
        impact: facts.impact,
        impact_detail: facts.impact_detail,
        state_code: facts.state_code.clone(),
        kind,
        location: Arc::clone(&raw.location),
        command: Arc::clone(&raw.command),
        source_path: Arc::clone(&raw.source_path),
        can_toggle: raw.can_toggle,
        can_delete: raw.can_delete,
        can_run: raw.can_run,
        overridden_by_user: raw.overridden_by_user,
        resolved_path: raw.resolved_path.clone(),
        file_exists: raw.file_exists,
        source_word: kind.source_word(),
        scope,
        only_show_in: raw.only_show_in.clone(),
        not_show_in: raw.not_show_in.clone(),
        try_exec: raw.try_exec.clone(),
        autostart_phase: raw.kde_autostart_phase.clone(),
        generated_unit: raw.generated_unit.clone(),
        overrides: raw.overrides.clone(),
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
        /// What the fake user manager lists.
        listing: Listing,
        /// `GetAll` answers by object path. Other paths time out.
        timings: FxHashMap<String, Timing>,
    }

    struct FakeManager {
        listing: Listing,
        timings: FxHashMap<String, Timing>,
    }

    impl UserManager for FakeManager {
        async fn list(&mut self, _cx: &Cx) -> Result<Listing, DbusError> {
            Ok(self.listing.clone())
        }

        async fn timing(&mut self, _cx: &Cx, path: &str) -> Result<Timing, DbusError> {
            self.timings
                .get(path)
                .copied()
                .ok_or(DbusError::Timeout(crate::dbus::READ_TIMEOUT))
        }
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
        type Manager = FakeManager;

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

        async fn manager(&self, _cx: &Cx) -> Result<FakeManager, DbusError> {
            if self.no_bus {
                return Err(DbusError::Connect("no session bus".into()));
            }
            Ok(FakeManager {
                listing: self.listing.clone(),
                timings: self.timings.clone(),
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
        type Manager = FakeManager;

        fn input(&self) -> ScanInput {
            self.input.clone()
        }

        fn watch(&self, env: &StartupEnv) -> Result<StartupWatch, CollectError> {
            StartupWatch::new(env)
        }

        async fn subscribe(&self, _cx: &Cx) -> Result<FakeSignals, DbusError> {
            Err(DbusError::Connect("no session bus".into()))
        }

        async fn manager(&self, _cx: &Cx) -> Result<FakeManager, DbusError> {
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
                data_home: None,
                runtime_dir: None,
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
            &Systemd::default(),
            UserDirs::default(),
        );
        // A new scan builds new strings; only the changed row gets a new entry.
        let mut b = raw(StartupKindRaw::AutostartUser, "b.desktop");
        b.status = StartupStatusRaw::Disabled;
        let second = cache.merge(
            vec![raw(StartupKindRaw::AutostartUser, "a.desktop"), b],
            &Systemd::default(),
            UserDirs::default(),
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
            data_home: None,
            runtime_dir: None,
            units: vec![(
                unit.to_str().expect("utf-8").to_owned(),
                "enabled".to_owned(),
            )],
        };
        let raws = host_scan(&input).expect("fixture scans");
        assert!(!raws.is_empty());
        let entries = Cache::default().merge(
            raws.clone(),
            &Systemd::default(),
            UserDirs::new(input.home.as_deref(), &input.env.config_home),
        );
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
        // Without a user manager: None when not enabled, otherwise Not measured.
        for e in &entries {
            let want = if e.status == StartupStatus::Enabled {
                Impact::NotMeasured
            } else {
                Impact::None
            };
            assert_eq!(e.impact, want, "{:?}", e.id);
        }
    }

    /// A directory under `target/tmp`, removed on drop.
    fn scratch() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&base).expect("create target/tmp");
        tempfile::Builder::new()
            .prefix("startup-fields-")
            .tempdir_in(&base)
            .expect("temp dir")
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
        std::fs::write(path, text).expect("write");
    }

    /// A user desktop entry with TryExec, OnlyShowIn, NotShowIn and an autostart phase over a
    /// system copy, its program on `PATH`; an entry whose absolute program is missing; a listed
    /// user unit whose file is gone; units under an `$XDG_CONFIG_HOME` outside `$HOME` and
    /// under a system directory (B8-E28-12).
    #[test]
    fn host_scan_fills_properties_fields() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch();
        let root = dir.path();
        let program = root.join("usr/bin/tray-tool");
        write(&program, "#!/bin/sh\n");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let entry = "[Desktop Entry]\nType=Application\nName=Tray Tool\nExec=tray-tool --tray\n\
                     TryExec=tray-tool\nOnlyShowIn=KDE;\nNotShowIn=GNOME;\n\
                     X-KDE-autostart-phase=2\n";
        let user_file = root.join("xdg-config/autostart/tray-tool.desktop");
        let system_file = root.join("etc/xdg/autostart/tray-tool.desktop");
        write(&user_file, entry);
        write(&system_file, entry);
        let missing = root.join("opt/missing/bin/missing-tool");
        write(
            &root.join("xdg-config/autostart/missing-tool.desktop"),
            &format!(
                "[Desktop Entry]\nType=Application\nName=Missing Tool\nExec={}\n",
                missing.display()
            ),
        );
        let gone = root.join("xdg-config/systemd/user/gone.service");
        let unit = format!(
            "[Service]\nExecStart={}\n[Install]\nWantedBy=default.target\n",
            program.display()
        );
        let external = root.join("xdg-config/systemd/user/external.service");
        let data_unit = root.join("xdg-data/systemd/user/data.service");
        let runtime_unit = root.join("run/user/1000/systemd/transient/run.service");
        let packaged = root.join("usr/lib/systemd/user/packaged.service");
        write(&runtime_unit, &unit);
        write(&data_unit, &unit);
        write(&external, &unit);
        write(&packaged, &unit);
        let input = ScanInput {
            env: StartupEnv {
                config_home: root.join("xdg-config"),
                config_dirs: vec![root.join("etc/xdg")],
                current_desktop: vec!["KDE".to_owned()],
                path_dirs: vec![root.join("usr/bin")],
                locales: SmallVec::new(),
            },
            home: Some(root.join("home")),
            data_home: Some(root.join("xdg-data")),
            runtime_dir: Some(root.join("run/user/1000")),
            units: [&gone, &external, &data_unit, &runtime_unit, &packaged]
                .iter()
                .map(|p| (p.to_str().expect("utf-8").to_owned(), "enabled".to_owned()))
                .collect(),
        };
        let raws = host_scan(&input).expect("scans");
        let entries = Cache::default().merge(
            raws,
            &Systemd::default(),
            UserDirs::new(input.home.as_deref(), &input.env.config_home)
                .with_data_home(input.data_home.as_deref(), input.runtime_dir.as_deref()),
        );

        let by_id = |id: &str| {
            entries
                .iter()
                .find(|e| &*e.id.id == id && e.kind != StartupKind::AutostartSystemOverridden)
                .unwrap_or_else(|| panic!("the {id} row"))
        };
        let user = by_id("tray-tool.desktop");
        assert_eq!(user.kind, StartupKind::AutostartUser);
        assert_eq!(user.resolved_path.as_deref(), program.to_str());
        assert!(user.file_exists);
        assert_eq!(user.source_word, "XDG autostart");
        assert_eq!(user.scope, StartupScope::User);
        let words =
            |list: &SmallVec<[Arc<str>; 2]>| list.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(words(&user.only_show_in), ["KDE"]);
        assert_eq!(words(&user.not_show_in), ["GNOME"]);
        assert_eq!(user.try_exec.as_deref(), Some("tray-tool"));
        assert_eq!(user.autostart_phase.as_deref(), Some("2"));
        assert_eq!(
            user.generated_unit.as_deref(),
            Some(tl_collect::startup::autostart_unit_name("tray-tool.desktop").as_str())
        );
        assert_eq!(user.overrides.as_deref(), system_file.to_str());

        let system = entries
            .iter()
            .find(|e| e.kind == StartupKind::AutostartSystemOverridden)
            .expect("the shadowed system row");
        assert_eq!(system.scope, StartupScope::System);
        assert_eq!(system.overrides, None);

        // The program path is absolute, so it resolves, but the file is not there.
        let missing_row = by_id("missing-tool.desktop");
        assert_eq!(missing_row.resolved_path.as_deref(), missing.to_str());
        assert!(!missing_row.file_exists);

        let unit = by_id("gone.service");
        assert!(!unit.file_exists);
        assert_eq!(unit.resolved_path, None);
        assert_eq!(unit.source_word, "systemd user unit");
        assert_eq!(unit.scope, StartupScope::User);
        assert_eq!(unit.generated_unit, None);
        assert!(unit.only_show_in.is_empty() && unit.try_exec.is_none());

        // `$XDG_CONFIG_HOME` is outside `$HOME`; its units are still the user's.
        let external_row = by_id("external.service");
        assert!(external_row.file_exists);
        assert_eq!(external_row.scope, StartupScope::User);
        // `$XDG_DATA_HOME` is outside `$HOME` too.
        let data_row = by_id("data.service");
        assert!(data_row.file_exists);
        assert_eq!(data_row.scope, StartupScope::User);
        // `$XDG_RUNTIME_DIR` holds transient and generated user units.
        let runtime_row = by_id("run.service");
        assert!(runtime_row.file_exists);
        assert_eq!(runtime_row.scope, StartupScope::User);
        let packaged_row = by_id("packaged.service");
        assert!(packaged_row.file_exists);
        assert_eq!(packaged_row.scope, StartupScope::System);
    }

    const UNIT: &str = "app-org.kde.foo@autostart.service";
    const UNIT_PATH: &str =
        "/org/freedesktop/systemd1/unit/app_2dorg_2ekde_2efoo_40autostart_2eservice";

    /// The desktop entry `org.kde.foo` with its generated unit, as the scanner links it.
    fn foo_raw() -> StartupRaw {
        let mut r = raw(StartupKindRaw::AutostartUser, "org.kde.foo.desktop");
        r.generated_unit = Some(Arc::from(
            tl_collect::startup::autostart_unit_name("org.kde.foo.desktop").as_str(),
        ));
        r
    }

    fn timing(inactive_exit_us: u64, activation_ms: u64) -> Timing {
        Timing {
            inactive_exit_us,
            active_enter_us: inactive_exit_us + activation_ms * 1000,
            cpu_ns: 42_000,
        }
    }

    /// The user manager with `org.kde.foo`'s generated unit loaded and running.
    fn listing() -> Listing {
        let mut loaded = FxHashMap::default();
        loaded.insert(
            UNIT.to_owned(),
            LoadedUnit {
                active_state: "active".into(),
                sub_state: "running".into(),
                path: UNIT_PATH.into(),
            },
        );
        Listing {
            files: vec![(
                format!("/run/user/1000/systemd/generator.late/{UNIT}"),
                "generated".to_owned(),
            )],
            loaded,
            userspace_us: 5_000_000,
        }
    }

    /// `listing()` plus a timing for the generated unit.
    fn systemd_with(t: Timing) -> Systemd {
        let listing = listing();
        let mut timings = FxHashMap::default();
        timings.insert(UNIT.to_owned(), t);
        Systemd {
            loaded: listing.loaded,
            userspace_us: listing.userspace_us,
            timings,
        }
    }

    #[test]
    fn impact_thresholds() {
        for (ms, want) in [
            (1200, Impact::High),
            (1000, Impact::High),
            (999, Impact::Medium),
            (300, Impact::Medium),
            (299, Impact::Low),
        ] {
            let facts = systemd_with(timing(8_000_000, ms)).facts(&foo_raw());
            assert_eq!(facts.impact, want, "activation {ms} ms");
            let detail = facts.impact_detail.expect("detail");
            assert_eq!(detail.activation_ms, ms);
            assert_eq!(detail.started_after_login_ms, 3_000);
            assert_eq!(detail.cpu_nsec, Some(42_000));
            assert!(detail.fork_exec_only);
        }

        // CPUUsageNSec u64::MAX is not available.
        let mut t = timing(8_000_000, 100);
        t.cpu_ns = u64::MAX;
        let facts = systemd_with(t).facts(&foo_raw());
        assert_eq!(facts.impact_detail.expect("detail").cpu_nsec, None);
    }

    #[test]
    fn impact_fork_exec_only_from_service_type() {
        for (service_type, want) in [
            (None, true),
            (Some("simple"), true),
            (Some("exec"), true),
            (Some("forking"), false),
            (Some("oneshot"), false),
        ] {
            let mut r = foo_raw();
            r.service_type = service_type.map(Into::into);
            let facts = systemd_with(timing(8_000_000, 400)).facts(&r);
            let detail = facts.impact_detail.expect("detail");
            assert_eq!(detail.fork_exec_only, want, "{service_type:?}");
        }
    }

    #[test]
    fn impact_none_when_disabled() {
        for status in [StartupStatusRaw::Disabled, StartupStatusRaw::Masked] {
            let mut r = foo_raw();
            r.status = status;
            let facts = systemd_with(timing(8_000_000, 1200)).facts(&r);
            assert_eq!(facts.impact, Impact::None, "{status:?}");
            assert_eq!(facts.impact_detail, None);
        }
    }

    #[test]
    fn impact_not_measured_zero_timestamp() {
        let facts = systemd_with(timing(0, 1200)).facts(&foo_raw());
        assert_eq!(facts.impact, Impact::NotMeasured);
        assert_eq!(facts.impact_detail, None);

        let mut t = timing(8_000_000, 0);
        t.active_enter_us = 0;
        assert_eq!(
            systemd_with(t).facts(&foo_raw()).impact,
            Impact::NotMeasured
        );

        // Loaded but no timing read (the GetAll failed).
        let mut systemd = systemd_with(timing(8_000_000, 1200));
        systemd.timings.clear();
        let facts = systemd.facts(&foo_raw());
        assert_eq!(facts.impact, Impact::NotMeasured);
        assert_eq!(facts.state_code.as_deref(), Some("active (running)"));

        // A Plasma script has no unit.
        let script = raw(StartupKindRaw::PlasmaPreStartup, "env.sh");
        let facts = systemd_with(timing(8_000_000, 1200)).facts(&script);
        assert_eq!(facts.impact, Impact::NotMeasured);
        assert_eq!(facts.state_code, None);
    }

    #[test]
    fn generated_unit_linked_not_duplicated() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let config_home = tmp.path().join("home/.config");
        let autostart = config_home.join("autostart");
        std::fs::create_dir_all(&autostart).expect("autostart dir");
        std::fs::write(
            autostart.join("org.kde.foo.desktop"),
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=/bin/true\n",
        )
        .expect("write desktop file");
        let listing = listing();
        let input = ScanInput {
            env: StartupEnv {
                config_home,
                config_dirs: vec![tmp.path().join("xdg")],
                ..StartupEnv::default()
            },
            home: Some(tmp.path().join("home")),
            data_home: None,
            runtime_dir: None,
            units: listing.files.clone(),
        };
        let raws = host_scan(&input).expect("scan");
        assert_eq!(raws.len(), 1, "{raws:?}");
        assert_eq!(raws[0].generated_unit.as_deref(), Some(UNIT));

        let mut systemd = systemd_with(timing(8_000_000, 120));
        systemd.loaded = listing.loaded;
        let facts = systemd.facts(&raws[0]);
        assert_eq!(facts.state_code.as_deref(), Some("active (running)"));
        let entries = Cache::default().merge(
            raws,
            &systemd,
            UserDirs::new(input.home.as_deref(), &input.env.config_home),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, StartupKind::AutostartUser);
        assert_eq!(entries[0].impact, Impact::Low);
        assert_eq!(entries[0].state_code.as_deref(), Some("active (running)"));

        // A user file that shadows a system entry: only the user row is linked.
        let mut shadowed = foo_raw();
        shadowed.kind = StartupKindRaw::AutostartSystemOverridden;
        assert_eq!(systemd.facts(&shadowed).state_code, None);
    }

    #[test]
    fn user_unit_masked_status() {
        let root = fixture_root();
        let unit = root.join("home/.config/systemd/user/sync.service");
        let input = ScanInput {
            env: StartupEnv {
                config_home: root.join("home/.config"),
                ..StartupEnv::default()
            },
            home: Some(root.join("home")),
            data_home: None,
            runtime_dir: None,
            units: vec![(
                unit.to_str().expect("utf-8").to_owned(),
                "masked".to_owned(),
            )],
        };
        let raws = host_scan(&input).expect("fixture scans");
        let row = raws
            .iter()
            .find(|r| r.kind == StartupKindRaw::UserUnit)
            .expect("the sync.service row")
            .clone();
        assert_eq!(row.status, StartupStatusRaw::Masked);

        // Loaded and timed, it still has no impact.
        let mut systemd = systemd_with(timing(8_000_000, 1200));
        let mut loaded = systemd.loaded.remove(UNIT).expect("loaded");
        loaded.path = "/org/freedesktop/systemd1/unit/sync_2eservice".into();
        systemd.loaded.insert("sync.service".into(), loaded);
        let t = systemd.timings.remove(UNIT).expect("timed");
        systemd.timings.insert("sync.service".into(), t);
        let entries = Cache::default().merge(
            raws,
            &systemd,
            UserDirs::new(input.home.as_deref(), &input.env.config_home),
        );
        let entry = entries
            .iter()
            .find(|e| e.kind == StartupKind::UserUnit)
            .expect("the sync.service entry");
        assert_eq!(entry.status, StartupStatus::Masked);
        assert_eq!(entry.impact, Impact::None);
        assert_eq!(
            systemd.facts(&row).state_code.as_deref(),
            Some("active (running)")
        );
    }

    /// Returns `foo_raw()` and records how many unit files the scan input carried.
    fn foo_scanner() -> (Scanner, Arc<AtomicUsize>) {
        let files = Arc::new(AtomicUsize::new(usize::MAX));
        let seen = Arc::clone(&files);
        let scanner: Scanner = Arc::new(move |input: &ScanInput| {
            seen.store(input.units.len(), Ordering::SeqCst);
            Ok(vec![foo_raw()])
        });
        (scanner, files)
    }

    #[test]
    fn no_bus_defaults() {
        let mut rig = Rig::new();
        let (scanner, files) = foo_scanner();
        let host = FakeHost {
            no_bus: true,
            ..FakeHost::default()
        };
        rig.start_with(scanner, host);
        rig.set_open(true);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(files.load(Ordering::SeqCst), 0);
        let updates = rig.updates();
        assert_eq!(updates.len(), 1);
        let entry = &updates[0].entries[0];
        assert_eq!(entry.impact, Impact::NotMeasured);
        assert_eq!(entry.impact_detail, None);
        assert_eq!(entry.state_code, None);
        assert_eq!(Systemd::default().facts(&foo_raw()).state_code, None);
        rig.finish();
    }

    #[test]
    fn scan_reads_unit_timing() {
        let mut rig = Rig::new();
        let (scanner, files) = foo_scanner();
        let mut timings = FxHashMap::default();
        timings.insert(UNIT_PATH.to_owned(), timing(8_000_000, 1200));
        let host = FakeHost {
            listing: listing(),
            timings,
            ..FakeHost::default()
        };
        rig.start_with(scanner, host);
        rig.set_open(true);
        rig.run_for(Duration::from_millis(500));
        assert_eq!(files.load(Ordering::SeqCst), 1);
        let updates = rig.updates();
        assert_eq!(updates.len(), 1);
        let entry = &updates[0].entries[0];
        assert_eq!(entry.impact, Impact::High);
        assert_eq!(entry.impact_detail.map(|d| d.activation_ms), Some(1200));
        rig.finish();
    }

    /// Read-only calls on the real user manager, to check the reply types. Skipped without a
    /// session bus.
    #[test]
    fn live_user_manager_reads() {
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async move {
            let cx = Cx::current().expect("task has a Cx");
            let Ok(mut m) = System.manager(&cx).await else {
                return None;
            };
            let listing = m.list(&cx).await.expect("listing");
            let path = listing
                .loaded
                .values()
                .find(|u| u.active_state == "active")
                .map(|u| u.path.clone());
            let timing = match path {
                Some(path) => Some(m.timing(&cx, &path).await.expect("GetAll")),
                None => None,
            };
            Some((listing, timing))
        });
        let Some((listing, timing)) = rt.block_on(join).expect("task completes") else {
            eprintln!("no session bus: skipped");
            return;
        };
        eprintln!("{} loaded units, timing {timing:?}", listing.loaded.len());
        assert!(listing.userspace_us > 0);
        assert!(!listing.loaded.is_empty());
        if let Some(t) = timing {
            assert!(t.active_enter_us >= t.inactive_exit_us, "{t:?}");
        }
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
