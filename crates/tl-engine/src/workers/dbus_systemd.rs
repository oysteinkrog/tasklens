//! The `SystemdSystem` and `SystemdUser` workers (ADR-004, SYS-001 to SYS-005, SYS-016).
//!
//! One body serves both managers: `Manager::System` on the system bus, `Manager::User` on the
//! session bus. On connect it lists every loaded unit and every installed unit file, reads
//! their properties and sends one `SourceUpdate::Services` with the unit records. Then it
//! subscribes to the manager and keeps the records live from unit signals (`ServicesState`),
//! with one publish per signal batch. A full listing runs again on `RefreshNow`, on
//! `Reloading(false)` and every 60 s while the Services page wants data (ADR-008). Cgroup data
//! and unit details come in E14-003 and E14-004.
//!
//! A bus that cannot be opened, or a bus without `org.freedesktop.systemd1`, is an expected
//! condition: the worker publishes the manager as `Unavailable` and tries again on `RefreshNow`
//! or after 60 s. A call timeout or a connection error during the first listing ends the
//! generation with `Err` so the supervisor restarts it. Once connected, a lost bus publishes
//! the manager as `Reconnecting` with the last records, then ends with `Err`.
//!
//! Unit descriptions and paths are logged at `debug` level only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::runtime::{JoinError, TaskHandle};
use asupersync::supervision::ManagedGeneration;
use asupersync::time;
use asupersync::types::Time;
use asupersync::{Cx, Outcome};
use futures_lite::StreamExt;
use futures_lite::stream::Boxed;
use tl_collect::SysRoot;
use tl_collect::proc::meta::{PasswdUserNames, UserNames};
use tl_collect::proc::status::{PidStatus, STATUS_READ_MAX, parse_status};
use tl_collect::sysroot::read_file;
use tl_model::health::{ManagerState, SourceId};
use tl_model::snapshot::{ServiceBackend, ServiceStatus, StartupType, UnitRecord};
use tl_model::table::{Manager, Total, UnitRef, UnitType};
use tl_model::validity::Validity;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{Bus, DbusConn, DbusError, ManagerConnSlot, connect};
use crate::source::{IDLE_RECHECK, WorkerCommand, WorkerControl, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{EngineMsg, ServicesUpdate, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;

pub const BUS_NAME: &str = "org.freedesktop.systemd1";
pub const PATH: &str = "/org/freedesktop/systemd1";
pub const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";
const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
/// Every unit object lives under this path.
const UNIT_PATH_NAMESPACE: &str = "/org/freedesktop/systemd1/unit";
/// The error `Subscribe` returns when this connection is already subscribed (E11 may subscribe
/// on the same connection).
const ALREADY_SUBSCRIBED: &str = "org.freedesktop.systemd1.AlreadySubscribed";

/// How long an unavailable manager waits before the next try when no `RefreshNow` comes.
pub const RETRY_UNAVAILABLE: Duration = Duration::from_secs(60);
/// ADR-008: a full listing every 60 s while `Demand.services` is true.
pub const RESYNC_PERIOD: Duration = Duration::from_secs(60);
/// The longest wait on the signal stream. Cancellation, `RefreshNow` and the resync schedule
/// are checked after each wait.
const SIGNAL_WAIT: Duration = Duration::from_millis(500);
/// Signals each match rule may queue. zbus stops reading the socket when a queue is full, which
/// would also hold back method replies; one listing loads hundreds of unit files with
/// `LoadUnit`, and each load sends `UnitNew`.
const SIGNAL_QUEUE: usize = 8192;
/// The most signals applied as one batch, so a signal storm still publishes and checks
/// cancellation.
const MAX_BATCH: usize = 4096;

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

/// The system manager worker, in the full engine only.
pub fn spec_system() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::SystemdSystem,
        name: "systemd-system",
        modes: &[EngineMode::Full],
        factory: |cfg, io| factory(Manager::System, cfg, io),
    }
}

/// The user manager worker, in the full engine only.
pub fn spec_user() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::SystemdUser,
        name: "systemd-user",
        modes: &[EngineMode::Full],
        factory: |cfg, io| factory(Manager::User, cfg, io),
    }
}

pub(crate) fn factory(manager: Manager, cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let cfg = Arc::clone(&cfg);
        async move { run(cx, io, cfg, manager, &BusOpener).await }
    })
}

pub fn source_id(manager: Manager) -> SourceId {
    match manager {
        Manager::System => SourceId::SystemdSystem,
        Manager::User => SourceId::SystemdUser,
    }
}

fn bus_of(manager: Manager) -> Bus {
    match manager {
        Manager::System => Bus::System,
        Manager::User => Bus::Session,
    }
}

fn slot_of(io: &WorkerIo, manager: Manager) -> &ManagerConnSlot {
    match manager {
        Manager::System => &io.shared.systemd_system,
        Manager::User => &io.shared.systemd_user,
    }
}

/// Why a connection was not made.
#[derive(Debug)]
enum OpenError {
    /// No bus, or no systemd on it. Published as `Unavailable`, never a worker failure.
    Unavailable(DbusError),
    /// Any other error. The generation ends with `Err`.
    Failed(DbusError),
}

/// Opens the manager's bus. Tests replace it with a fake.
trait Opener {
    async fn open(&self, cx: &Cx, manager: Manager) -> Result<DbusConn, OpenError>;
}

/// The real bus, checked for an owner of `org.freedesktop.systemd1`.
struct BusOpener;

impl Opener for BusOpener {
    async fn open(&self, cx: &Cx, manager: Manager) -> Result<DbusConn, OpenError> {
        let conn = match connect(cx, bus_of(manager)).await {
            Ok(conn) => conn,
            Err(e @ (DbusError::Connect(_) | DbusError::Timeout(_))) => {
                return Err(OpenError::Unavailable(e));
            }
            Err(e) => return Err(OpenError::Failed(e)),
        };
        let owner = conn
            .read(cx, async {
                let bus = zbus::fdo::DBusProxy::new(conn.raw()).await?;
                let name = zbus::names::BusName::try_from(BUS_NAME)?;
                bus.name_has_owner(name).await.map_err(zbus::Error::from)
            })
            .await;
        match owner {
            Ok(true) => Ok(conn),
            Ok(false) => Err(OpenError::Unavailable(DbusError::Call {
                name: "org.freedesktop.DBus.Error.NameHasNoOwner".into(),
                message: BUS_NAME.into(),
            })),
            Err(e) => Err(OpenError::Failed(e)),
        }
    }
}

async fn run<O: Opener>(
    cx: Cx,
    io: WorkerIo,
    cfg: Arc<EngineConfig>,
    manager: Manager,
    opener: &O,
) -> Outcome<(), WorkerError> {
    let id = source_id(manager);
    // Commands queued before this generation started belong to an earlier one.
    let mut last_seq = newest_seq(&io.control.borrow_and_clone());
    loop {
        match opener.open(&cx, manager).await {
            Ok(conn) => return connected(&cx, &io, &cfg, manager, conn, last_seq).await,
            Err(OpenError::Unavailable(reason)) => {
                if cx.checkpoint().is_err() {
                    return Outcome::Ok(());
                }
                tracing::debug!(?manager, %reason, "systemd: manager not available");
                let update = services(manager, ManagerState::Unavailable, Vec::new());
                if let Some(stop) = send(&cx, &io, update).await {
                    return stop;
                }
                let limit = Some(RETRY_UNAVAILABLE);
                if !wait_refresh(&cx, &io, id, limit, &mut last_seq).await {
                    return Outcome::Ok(());
                }
            }
            Err(OpenError::Failed(e)) => {
                if cx.checkpoint().is_err() {
                    return Outcome::Ok(());
                }
                return Outcome::Err(e.into());
            }
        }
    }
}

/// One connected generation: fill the slot, start the colour scheme watch, list, subscribe,
/// then apply signals and resync on schedule until cancelled or the bus is lost.
async fn connected(
    cx: &Cx,
    io: &WorkerIo,
    cfg: &EngineConfig,
    manager: Manager,
    conn: DbusConn,
    mut last_seq: u64,
) -> Outcome<(), WorkerError> {
    let id = source_id(manager);
    let slot = slot_of(io, manager);
    slot.set(Arc::new(conn.raw().clone()));
    // Locals drop in reverse order, so both guards go before `conn` stops its tick task.
    let _clear = ClearOnDrop(slot.clone());
    let _scheme = start_color_scheme(cx, cfg.mode, manager, conn.raw(), &io.updates);
    let mut state = ServicesState::new(manager);
    if let Err(e) = resync(cx, &conn, manager, &cfg.sys_root, &mut state).await {
        // Nothing was published by this generation, so there is nothing to keep.
        if cx.checkpoint().is_err() {
            return Outcome::Ok(());
        }
        return Outcome::Err(e.into());
    }
    if let Some(stop) = send(cx, io, state.update(ManagerState::Connected)).await {
        return stop;
    }
    let mut signals = match SystemdSignals::open(cx, &conn).await {
        Ok(signals) => signals,
        Err(e) => return lost(cx, io, &state, e).await,
    };
    let mut schedule = Schedule::new(cx.now());
    loop {
        let (batch, mut full) =
            match wait_next(cx, io, id, &mut signals, &mut schedule, &mut last_seq).await {
                Wake::Stop => return Outcome::Ok(()),
                Wake::Lost(e) => return lost(cx, io, &state, e).await,
                Wake::Batch { signals, resync } => (signals, resync),
            };
        let mut changed = false;
        if !full {
            match apply_batch(cx, &conn, manager, &cfg.sys_root, &mut state, &batch).await {
                Ok(BatchResult::Applied(c)) => changed = c,
                Ok(BatchResult::Resync) => full = true,
                Err(e) => return lost(cx, io, &state, e).await,
            }
        }
        if full {
            match resync(cx, &conn, manager, &cfg.sys_root, &mut state).await {
                Ok(c) => changed = c,
                Err(e) => return lost(cx, io, &state, e).await,
            }
            schedule.done(cx.now());
        }
        if changed && let Some(stop) = send(cx, io, state.update(ManagerState::Connected)).await {
            return stop;
        }
    }
}

/// The bus is gone after a successful connect: publish the manager as `Reconnecting` with the
/// last records and end the generation with `Err`, so the supervisor restarts it with backoff.
/// A cancelled `cx` ends with `Ok` and publishes nothing.
async fn lost(
    cx: &Cx,
    io: &WorkerIo,
    state: &ServicesState,
    e: DbusError,
) -> Outcome<(), WorkerError> {
    if cx.is_cancel_requested() {
        return Outcome::Ok(());
    }
    tracing::debug!(manager = ?state.manager, error = %e, "systemd: bus lost");
    if let Some(stop) = send(cx, io, state.update(ManagerState::Reconnecting)).await {
        return stop;
    }
    Outcome::Err(e.into())
}

/// A full listing into `state`. True when the record list changed.
async fn resync(
    cx: &Cx,
    conn: &DbusConn,
    manager: Manager,
    sys_root: &SysRoot,
    state: &mut ServicesState,
) -> Result<bool, DbusError> {
    let started = cx.now();
    let listing = list_units(cx, conn).await?;
    let users = resolve_users(cx, manager, sys_root, listing.units.iter().map(|f| &f.raw)).await?;
    let took = Duration::from_nanos(cx.now().duration_since(started));
    tracing::debug!(
        ?manager,
        units = listing.units.len(),
        ?took,
        "systemd: listing done"
    );
    Ok(state.replace(
        listing.units.into_iter().zip(users).collect(),
        listing.files,
    ))
}

enum BatchResult {
    /// The batch was applied; true when the record list changed.
    Applied(bool),
    /// The batch asked for a full listing.
    Resync,
}

/// Applies one signal batch: the state changes first, then the bus reads they ask for.
async fn apply_batch(
    cx: &Cx,
    conn: &DbusConn,
    manager: Manager,
    sys_root: &SysRoot,
    state: &mut ServicesState,
    batch: &[Signal],
) -> Result<BatchResult, DbusError> {
    let mut fx = Effects::default();
    for signal in batch {
        state.apply(signal, &mut fx);
    }
    if fx.resync {
        return Ok(BatchResult::Resync);
    }
    let mut changed = fx.changed;
    if fx.list_files {
        changed |= state.set_files(list_files(cx, conn).await?);
    }
    let mut fetched = Vec::with_capacity(fx.fetch.len());
    for (name, path) in &fx.fetch {
        if let Some(unit) = fetch_unit(cx, conn, name, path).await? {
            fetched.push(unit);
        }
    }
    let rewho: Vec<(Arc<str>, RawUnit)> = fx
        .users
        .iter()
        .filter(|name| !fetched.iter().any(|f| f.raw.name == **name))
        .filter_map(|name| state.raw(name).map(|raw| (Arc::clone(name), raw.clone())))
        .collect();
    if fetched.is_empty() && rewho.is_empty() {
        return Ok(BatchResult::Applied(changed));
    }
    let raws = fetched
        .iter()
        .map(|f| &f.raw)
        .chain(rewho.iter().map(|(_, raw)| raw));
    let mut users = resolve_users(cx, manager, sys_root, raws)
        .await?
        .into_iter();
    for (unit, user) in fetched.into_iter().zip(users.by_ref()) {
        changed |= state.upsert(unit, user);
    }
    for ((name, _), user) in rewho.iter().zip(users) {
        changed |= state.set_user(name, user);
    }
    Ok(BatchResult::Applied(changed))
}

/// Clears the manager slot when the generation ends, however it ends.
struct ClearOnDrop(ManagerConnSlot);

impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        self.0.clear();
    }
}

/// Aborts a child task when the generation ends.
struct AbortOnDrop(TaskHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// R-244: the `SystemdUser` generation in the full engine watches the portal colour scheme on
/// its session connection. An `Err` from the watch is logged and never restarts the worker.
fn start_color_scheme(
    cx: &Cx,
    mode: EngineMode,
    manager: Manager,
    session: &zbus::Connection,
    tx: &mpsc::Sender<EngineMsg>,
) -> Option<AbortOnDrop> {
    if manager != Manager::User || mode != EngineMode::Full {
        return None;
    }
    let session = session.clone();
    let tx = tx.clone();
    let spawned = cx.spawn(move |cx| async move {
        if let Err(e) = crate::opener::watch_color_scheme(&cx, &session, &tx).await {
            tracing::debug!(error = %e, "systemd: color scheme watch ended");
        }
    });
    match spawned {
        Ok(handle) => Some(AbortOnDrop(handle)),
        Err(e) => {
            tracing::debug!(error = %e, "systemd: color scheme watch did not start");
            None
        }
    }
}

/// The highest command seq in `control`, or 0.
fn newest_seq(control: &WorkerControl) -> u64 {
    control.commands.last().map_or(0, |c| c.seq)
}

/// True when a `RefreshNow` for `id` newer than `last_seq` is queued. Moves `last_seq` to the
/// newest queued seq.
fn take_refresh(control: &WorkerControl, id: SourceId, last_seq: &mut u64) -> bool {
    let found = control
        .commands
        .iter()
        .any(|c| c.seq > *last_seq && c.target == id && c.command == WorkerCommand::RefreshNow);
    *last_seq = (*last_seq).max(newest_seq(control));
    found
}

/// Waits for a `RefreshNow` for `id`, or for `limit` when it is set. Checks every
/// `IDLE_RECHECK`. Returns false when `cx` is cancelled.
async fn wait_refresh(
    cx: &Cx,
    io: &WorkerIo,
    id: SourceId,
    limit: Option<Duration>,
    last_seq: &mut u64,
) -> bool {
    let start = cx.now();
    loop {
        time::sleep(cx.now(), IDLE_RECHECK).await;
        if cx.checkpoint().is_err() {
            return false;
        }
        if take_refresh(&io.control.borrow_and_clone(), id, last_seq) {
            return true;
        }
        let waited = Duration::from_nanos(cx.now().duration_since(start));
        if limit.is_some_and(|limit| waited >= limit) {
            return true;
        }
    }
}

fn services(manager: Manager, state: ManagerState, units: Vec<UnitRecord>) -> SourceUpdate {
    // UI-SYS-014: during `Reconnecting` the records are the last good data, drawn as stale.
    let validity = match state {
        ManagerState::Connected => Validity::Valid,
        ManagerState::Reconnecting => Validity::Stale,
        ManagerState::Unavailable => Validity::Missing,
    };
    SourceUpdate::Services(ServicesUpdate {
        manager,
        state,
        units,
        details: Vec::new(),
        backend: ServiceBackend::Systemd,
        validity,
    })
}

/// Sends one update. Returns the worker's result when it must stop: `Ok` on cancel,
/// `Err(Closed)` when the reducer is gone.
async fn send(cx: &Cx, io: &WorkerIo, update: SourceUpdate) -> Option<Outcome<(), WorkerError>> {
    match io.updates.send(cx, EngineMsg::Source(update)).await {
        Ok(()) => None,
        Err(mpsc::SendError::Cancelled(_)) => Some(Outcome::Ok(())),
        Err(_) => Some(Outcome::Err(WorkerError::Closed)),
    }
}

// Listing

/// The unit type from the id suffix. `None` for `.device` and any other type the table does
/// not show.
pub fn unit_kind(name: &str) -> Option<UnitType> {
    let (_, suffix) = name.rsplit_once('.')?;
    Some(match suffix {
        "service" => UnitType::Service,
        "socket" => UnitType::Socket,
        "timer" => UnitType::Timer,
        "mount" => UnitType::Mount,
        "automount" => UnitType::Automount,
        "path" => UnitType::Path,
        "target" => UnitType::Target,
        "swap" => UnitType::Swap,
        "slice" => UnitType::Slice,
        "scope" => UnitType::Scope,
        _ => return None,
    })
}

/// SYS-002: uninstantiated templates (`foo@.service`), `.device` units and unknown types are
/// never published.
fn name_shown(name: &str) -> bool {
    !name.contains("@.") && unit_kind(name).is_some()
}

/// SYS-002: a `not-found` unit without a unit file is never published.
fn is_hidden(name: &str, load_state: &str, has_file: bool) -> bool {
    !name_shown(name) || (load_state == "not-found" && !has_file)
}

/// The properties one record needs, read from the `Unit` map and the type interface map.
#[derive(Clone, Debug, Default, PartialEq)]
struct RawUnit {
    name: Arc<str>,
    description: Arc<str>,
    load_state: Arc<str>,
    active_state: Arc<str>,
    sub_state: Arc<str>,
    unit_file_state: Arc<str>,
    fragment_path: Arc<str>,
    /// ActiveEnterTimestamp, CLOCK_REALTIME µs, 0 for none.
    active_enter_us: u64,
    triggered_by: Arc<[Arc<str>]>,
    /// Services only, else 0.
    main_pid: u32,
    /// `User=`, services only.
    user: Arc<str>,
    /// `ExecStart[0].path`, services only.
    exec_start: Arc<str>,
    /// The address of the first `Listen` entry, sockets only.
    listen: Arc<str>,
    /// Mounts only.
    what: Arc<str>,
    /// The unit a timer starts, timers only.
    timer_unit: Arc<str>,
    /// From the type interface; empty for kinds that are not read.
    control_group: Arc<str>,
    can_reload: bool,
}

impl RawUnit {
    fn from_props(name: &str, unit: &Props, typed: &Props) -> Self {
        let mut raw = RawUnit {
            name: Arc::from(name),
            ..RawUnit::default()
        };
        raw.apply(unit, false);
        raw.apply(typed, true);
        raw
    }

    /// Sets the fields whose properties are in `props`: a `Unit` map, or a type interface map
    /// when `typed` is true. Other properties are ignored.
    fn apply(&mut self, props: &Props, typed: bool) {
        let has = |key: &str| props.contains_key(key);
        if typed {
            if has("MainPID") && unit_kind(&self.name) == Some(UnitType::Service) {
                self.main_pid = prop_u32(props, "MainPID");
            }
            if has("User") {
                self.user = prop_str(props, "User");
            }
            if has("ExecStart") {
                self.exec_start = first_field(props, "ExecStart", 0);
            }
            if has("Listen") {
                self.listen = first_field(props, "Listen", 1);
            }
            if has("What") {
                self.what = prop_str(props, "What");
            }
            if has("Unit") {
                self.timer_unit = prop_str(props, "Unit");
            }
            if has("ControlGroup") {
                self.control_group = prop_str(props, "ControlGroup");
            }
            return;
        }
        if has("Description") {
            self.description = prop_str(props, "Description");
        }
        if has("LoadState") {
            self.load_state = prop_str(props, "LoadState");
        }
        if has("ActiveState") {
            self.active_state = prop_str(props, "ActiveState");
        }
        if has("SubState") {
            self.sub_state = prop_str(props, "SubState");
        }
        if has("UnitFileState") {
            self.unit_file_state = prop_str(props, "UnitFileState");
        }
        if has("FragmentPath") {
            self.fragment_path = prop_str(props, "FragmentPath");
        }
        if has("ActiveEnterTimestamp") {
            self.active_enter_us = prop_u64(props, "ActiveEnterTimestamp");
        }
        if has("TriggeredBy") {
            self.triggered_by = prop_strv(props, "TriggeredBy");
        }
        if has("CanReload") {
            self.can_reload = prop_bool(props, "CanReload");
        }
    }
}

/// The properties a record keeps, per interface: a `PropertiesChanged` that names one of them
/// in `invalidated` asks for a `GetAll`.
fn is_tracked(name: &str, typed: bool) -> bool {
    if typed {
        matches!(
            name,
            "MainPID" | "User" | "ExecStart" | "Listen" | "What" | "Unit" | "ControlGroup"
        )
    } else {
        matches!(
            name,
            "Description"
                | "LoadState"
                | "ActiveState"
                | "SubState"
                | "UnitFileState"
                | "FragmentPath"
                | "ActiveEnterTimestamp"
                | "TriggeredBy"
                | "CanReload"
        )
    }
}

/// Field `index` of the first row of the array property `key`, or empty.
fn first_field(props: &Props, key: &str, index: usize) -> Arc<str> {
    let Some(Value::Array(rows)) = props.get(key).map(|v| unwrap(v)) else {
        return Arc::from("");
    };
    match rows.inner().first().map(unwrap) {
        Some(Value::Structure(row)) => row.fields().get(index).map_or_else(
            || Arc::from(""),
            |v| Arc::from(str_of(v).unwrap_or_default()),
        ),
        _ => Arc::from(""),
    }
}

/// A variant inside a variant is unwrapped.
fn unwrap<'a>(v: &'a Value<'a>) -> &'a Value<'a> {
    match v {
        Value::Value(inner) => unwrap(inner),
        other => other,
    }
}

fn str_of<'a>(v: &'a Value<'a>) -> Option<&'a str> {
    match unwrap(v) {
        Value::Str(s) => Some(s.as_str()),
        Value::ObjectPath(p) => Some(p.as_str()),
        _ => None,
    }
}

fn prop_str(props: &Props, key: &str) -> Arc<str> {
    Arc::from(props.get(key).and_then(|v| str_of(v)).unwrap_or_default())
}

fn prop_u32(props: &Props, key: &str) -> u32 {
    match props.get(key).map(|v| unwrap(v)) {
        Some(Value::U32(n)) => *n,
        _ => 0,
    }
}

fn prop_bool(props: &Props, key: &str) -> bool {
    matches!(props.get(key).map(|v| unwrap(v)), Some(Value::Bool(true)))
}

fn prop_u64(props: &Props, key: &str) -> u64 {
    match props.get(key).map(|v| unwrap(v)) {
        Some(Value::U64(n)) => *n,
        _ => 0,
    }
}

fn prop_strv(props: &Props, key: &str) -> Arc<[Arc<str>]> {
    match props.get(key).map(|v| unwrap(v)) {
        Some(Value::Array(items)) => items
            .inner()
            .iter()
            .filter_map(|v| str_of(v).map(Arc::from))
            .collect(),
        _ => Arc::from([]),
    }
}

/// SYS-003 Path column: services `ExecStart[0].path`, sockets the first `Listen` address,
/// mounts `What`, timers `Unit`, every other kind empty.
fn path_of(kind: UnitType, raw: &RawUnit) -> Arc<str> {
    match kind {
        UnitType::Service => Arc::clone(&raw.exec_start),
        UnitType::Socket => Arc::clone(&raw.listen),
        UnitType::Mount => Arc::clone(&raw.what),
        UnitType::Timer => Arc::clone(&raw.timer_unit),
        _ => Arc::from(""),
    }
}

/// SYS-003 User column: the owner of the real uid of MainPID, else `User=`, else root for the
/// system manager or the current user for the user manager.
fn resolve_user(
    manager: Manager,
    pid_uid: Option<u32>,
    user_prop: &str,
    names: &mut dyn UserNames,
    current_uid: u32,
) -> Arc<str> {
    if let Some(uid) = pid_uid {
        return names.name(uid);
    }
    if !user_prop.is_empty() {
        return Arc::from(user_prop);
    }
    match manager {
        Manager::System => Arc::from("root"),
        Manager::User => names.name(current_uid),
    }
}

/// The real uid in `<proc>/<pid>/status`, or `None` when the process is gone or unreadable.
fn pid_real_uid(
    sys_root: &SysRoot,
    pid: u32,
    buf: &mut Vec<u8>,
    st: &mut PidStatus,
) -> Option<u32> {
    let path = sys_root.proc_path(&format!("{pid}/status"));
    read_file(&path, buf, STATUS_READ_MAX).ok()?;
    parse_status(buf, st).ok()?;
    Some(st.uid[0])
}

fn missing<T: Default>() -> Total<T> {
    Total {
        value: T::default(),
        validity: Validity::Missing,
    }
}

fn record(manager: Manager, kind: UnitType, raw: &RawUnit, user: Arc<str>) -> UnitRecord {
    UnitRecord {
        unit: UnitRef {
            manager,
            name: Arc::clone(&raw.name),
        },
        kind,
        description: Arc::clone(&raw.description),
        status: ServiceStatus::from_states(&raw.active_state, &raw.sub_state),
        sub_state: Arc::clone(&raw.sub_state),
        load_state: Arc::clone(&raw.load_state),
        startup: StartupType::from_unit_file_state(&raw.unit_file_state),
        main_pid: raw.main_pid,
        user,
        path: path_of(kind, raw),
        unit_file: (!raw.fragment_path.is_empty()).then(|| Arc::clone(&raw.fragment_path)),
        active_since_us: (raw.active_enter_us != 0).then_some(raw.active_enter_us),
        triggered_by: Arc::clone(&raw.triggered_by),
        control_group: (!raw.control_group.is_empty()).then(|| Arc::clone(&raw.control_group)),
        can_reload: raw.can_reload,
        has_cgroup_procs: false,
        cgroup_pids: Arc::from([]),
        cpu_pct: missing(),
        memory_bytes: missing(),
        disk_bps: missing(),
        tasks: missing(),
    }
}

/// The type interface read besides `Unit`, for the four types whose columns need it.
fn type_iface(kind: UnitType) -> Option<&'static str> {
    match kind {
        UnitType::Service => Some("org.freedesktop.systemd1.Service"),
        UnitType::Socket => Some("org.freedesktop.systemd1.Socket"),
        UnitType::Mount => Some("org.freedesktop.systemd1.Mount"),
        UnitType::Timer => Some("org.freedesktop.systemd1.Timer"),
        _ => None,
    }
}

fn decode_error(e: impl std::fmt::Display) -> DbusError {
    DbusError::Call {
        name: "zbus".into(),
        message: e.to_string().into(),
    }
}

/// A D-Bus error reply about one unit (it vanished, or its name is bad). The unit is skipped;
/// any other error ends the listing.
fn is_unit_error(e: &DbusError) -> bool {
    matches!(e, DbusError::Call { name, .. } if &**name != "zbus")
}

async fn manager_call<B, T>(
    cx: &Cx,
    conn: &DbusConn,
    method: &str,
    body: &B,
) -> Result<T, DbusError>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    T: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
{
    let call = conn
        .raw()
        .call_method(Some(BUS_NAME), PATH, Some(MANAGER_IFACE), method, body);
    let reply = conn.read(cx, call).await?;
    reply.body().deserialize::<T>().map_err(decode_error)
}

async fn get_all(
    cx: &Cx,
    conn: &DbusConn,
    path: &OwnedObjectPath,
    iface: &str,
) -> Result<Props, DbusError> {
    let body = (iface,);
    let call = conn.raw().call_method(
        Some(BUS_NAME),
        path,
        Some(PROPERTIES_IFACE),
        "GetAll",
        &body,
    );
    let reply = conn.read(cx, call).await?;
    reply.body().deserialize::<Props>().map_err(decode_error)
}

/// One unit as a listing or a `GetAll` read it.
#[derive(Clone, Debug)]
struct Fetched {
    kind: UnitType,
    raw: RawUnit,
    path: OwnedObjectPath,
}

/// A full listing: the shown units sorted by name, and the installed unit files.
struct Listing {
    units: Vec<Fetched>,
    files: UnitFiles,
}

/// `ListUnitFilesByPatterns`: unit file name to its enablement state.
type UnitFiles = HashMap<String, String>;

async fn list_files(cx: &Cx, conn: &DbusConn) -> Result<UnitFiles, DbusError> {
    let patterns = (Vec::<&str>::new(), Vec::<&str>::new());
    let files: Vec<(String, String)> =
        manager_call(cx, conn, "ListUnitFilesByPatterns", &patterns).await?;
    Ok(files
        .into_iter()
        .filter_map(|(path, state)| path.rsplit('/').next().map(|name| (name.to_owned(), state)))
        .collect())
}

/// Reads one unit's `Unit` map and, for the kinds that need it, its type interface map.
/// `None` when the kind is not shown or the unit is gone; a bus failure is `Err`.
async fn fetch_unit(
    cx: &Cx,
    conn: &DbusConn,
    name: &str,
    path: &OwnedObjectPath,
) -> Result<Option<Fetched>, DbusError> {
    let Some(kind) = unit_kind(name) else {
        return Ok(None);
    };
    let unit = match get_all(cx, conn, path, UNIT_IFACE).await {
        Ok(props) => props,
        Err(e) if is_unit_error(&e) => {
            tracing::debug!(unit = %name, error = %e, "systemd: GetAll failed");
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let typed = match type_iface(kind) {
        Some(iface) => match get_all(cx, conn, path, iface).await {
            Ok(props) => props,
            Err(e) if is_unit_error(&e) => {
                tracing::debug!(unit = %name, error = %e, "systemd: GetAll failed");
                Props::new()
            }
            Err(e) => return Err(e),
        },
        None => Props::new(),
    };
    Ok(Some(Fetched {
        kind,
        raw: RawUnit::from_props(name, &unit, &typed),
        path: path.clone(),
    }))
}

/// Lists loaded units and installed unit files, loads the files that are not loaded and reads
/// every unit's properties. Hidden units are left out.
async fn list_units(cx: &Cx, conn: &DbusConn) -> Result<Listing, DbusError> {
    let patterns = (Vec::<&str>::new(), Vec::<&str>::new());
    let loaded: Vec<ListedUnit> = manager_call(cx, conn, "ListUnitsByPatterns", &patterns).await?;
    let files = list_files(cx, conn).await?;

    let mut targets: Vec<(String, OwnedObjectPath)> = Vec::with_capacity(loaded.len());
    let mut seen = HashSet::with_capacity(loaded.len());
    for (name, _, _, _, _, _, path, ..) in loaded {
        if name_shown(&name) && seen.insert(name.clone()) {
            targets.push((name, path));
        }
    }
    let mut unloaded: Vec<&String> = files
        .keys()
        .filter(|name| name_shown(name) && !seen.contains(*name))
        .collect();
    unloaded.sort();
    for name in unloaded {
        // `LoadUnit` loads the file to read it, as `systemctl show` does. It never starts it.
        match manager_call::<_, OwnedObjectPath>(cx, conn, "LoadUnit", &(name.as_str(),)).await {
            Ok(path) => targets.push((name.clone(), path)),
            Err(e) if is_unit_error(&e) => {
                tracing::debug!(unit = %name, error = %e, "systemd: LoadUnit failed");
            }
            Err(e) => return Err(e),
        }
    }

    let mut units: Vec<Fetched> = Vec::with_capacity(targets.len());
    for (name, path) in &targets {
        if let Some(unit) = fetch_unit(cx, conn, name, path).await? {
            let has_file = files.contains_key(name.as_str()) || !unit.raw.fragment_path.is_empty();
            if !is_hidden(name, &unit.raw.load_state, has_file) {
                units.push(unit);
            }
        }
    }
    units.sort_by(|a, b| a.raw.name.cmp(&b.raw.name));
    Ok(Listing { units, files })
}

/// The User column for each unit in `raws`, in order. The /proc and passwd reads run in one
/// blocking job.
async fn resolve_users<'a>(
    cx: &Cx,
    manager: Manager,
    sys_root: &SysRoot,
    raws: impl Iterator<Item = &'a RawUnit>,
) -> Result<Vec<Arc<str>>, DbusError> {
    let inputs: Vec<(u32, Arc<str>)> = raws
        .map(|raw| (raw.main_pid, Arc::clone(&raw.user)))
        .collect();
    let sys_root = sys_root.clone();
    let job = cx.spawn_blocking(move |_| {
        let base = sys_root.proc.parent().unwrap_or(std::path::Path::new("/"));
        let mut names = PasswdUserNames::load(&base.join("etc/passwd"));
        let current_uid = rustix::process::getuid().as_raw();
        let mut buf = Vec::new();
        let mut st = PidStatus::default();
        inputs
            .iter()
            .map(|(pid, user)| {
                let pid_uid = (*pid != 0)
                    .then(|| pid_real_uid(&sys_root, *pid, &mut buf, &mut st))
                    .flatten();
                resolve_user(manager, pid_uid, user, &mut names, current_uid)
            })
            .collect::<Vec<_>>()
    });
    let mut job = job.map_err(|e| decode_error(format!("spawn: {e}")))?;
    match job.join(cx).await {
        Ok(users) => Ok(users),
        Err(JoinError::Cancelled(_)) => Err(DbusError::Closed),
        Err(e) => Err(decode_error(e)),
    }
}

// Signals

/// One manager or unit signal, decoded.
#[derive(Clone, Debug)]
enum Signal {
    UnitNew {
        name: Arc<str>,
        path: OwnedObjectPath,
    },
    UnitRemoved {
        name: Arc<str>,
    },
    UnitFilesChanged,
    Reloading(bool),
    /// `PropertiesChanged` on a unit object.
    Properties {
        path: OwnedObjectPath,
        iface: String,
        changed: Props,
        invalidated: Vec<String>,
    },
    /// `JobNew`, `JobRemoved` (E11 tracks jobs) and anything that did not decode.
    Other,
}

impl Signal {
    fn parse(msg: &zbus::Message) -> Signal {
        let header = msg.header();
        let iface = header.interface().map(|i| i.as_str()).unwrap_or_default();
        let member = header.member().map(|m| m.as_str()).unwrap_or_default();
        let body = msg.body();
        let parsed = match (iface, member) {
            (MANAGER_IFACE, "UnitNew") => {
                body.deserialize::<(String, OwnedObjectPath)>()
                    .ok()
                    .map(|(name, path)| Signal::UnitNew {
                        name: name.into(),
                        path,
                    })
            }
            (MANAGER_IFACE, "UnitRemoved") => body
                .deserialize::<(String, OwnedObjectPath)>()
                .ok()
                .map(|(name, _)| Signal::UnitRemoved { name: name.into() }),
            (MANAGER_IFACE, "UnitFilesChanged") => Some(Signal::UnitFilesChanged),
            (MANAGER_IFACE, "Reloading") => body.deserialize::<bool>().ok().map(Signal::Reloading),
            (PROPERTIES_IFACE, "PropertiesChanged") => {
                let path = header.path().map(|p| OwnedObjectPath::from(p.to_owned()));
                let args = body.deserialize::<(String, Props, Vec<String>)>().ok();
                path.zip(args)
                    .map(|(path, (iface, changed, invalidated))| Signal::Properties {
                        path,
                        iface,
                        changed,
                        invalidated,
                    })
            }
            _ => None,
        };
        parsed.unwrap_or(Signal::Other)
    }
}

/// The signal stream. Tests replace the bus with a fake.
trait UnitSignals {
    /// The next signal, waiting at most `d`. `Ok(None)` on timeout; `Err` when the stream ends
    /// or `cx` is cancelled.
    async fn next(&mut self, cx: &Cx, d: Duration) -> Result<Option<Signal>, DbusError>;
    /// A signal that is already queued, without waiting. `Ok(None)` when none is.
    async fn ready(&mut self) -> Result<Option<Signal>, DbusError>;
}

/// The real subscription: manager signals on `/org/freedesktop/systemd1` and
/// `PropertiesChanged` on every unit object. The streams hold their own connection handle.
struct SystemdSignals<'a> {
    conn: &'a DbusConn,
    stream: Boxed<zbus::Result<zbus::Message>>,
}

impl<'a> SystemdSignals<'a> {
    /// Adds the match rules, then calls `Manager.Subscribe` so systemd sends unit signals.
    async fn open(cx: &Cx, conn: &'a DbusConn) -> Result<Self, DbusError> {
        let rules = || -> zbus::Result<_> {
            let manager = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(BUS_NAME)?
                .path(PATH)?
                .interface(MANAGER_IFACE)?
                .build();
            let units = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(BUS_NAME)?
                .path_namespace(UNIT_PATH_NAMESPACE)?
                .interface(PROPERTIES_IFACE)?
                .member("PropertiesChanged")?
                .build();
            Ok((manager.to_owned(), units.to_owned()))
        };
        let (manager_rule, unit_rule) = rules().map_err(decode_error)?;
        let raw = conn.raw();
        let queue = Some(SIGNAL_QUEUE);
        let manager = conn
            .read(
                cx,
                zbus::MessageStream::for_match_rule(manager_rule, raw, queue),
            )
            .await?;
        let units = conn
            .read(
                cx,
                zbus::MessageStream::for_match_rule(unit_rule, raw, queue),
            )
            .await?;
        let subscribe =
            raw.call_method(Some(BUS_NAME), PATH, Some(MANAGER_IFACE), "Subscribe", &());
        match conn.read(cx, subscribe).await {
            Ok(_) => {}
            Err(DbusError::Call { name, .. }) if &*name == ALREADY_SUBSCRIBED => {}
            Err(e) => return Err(e),
        }
        Ok(Self {
            conn,
            stream: manager.or(units).boxed(),
        })
    }
}

/// A stream item: a message, or a read error that only skips that message.
fn decode(item: zbus::Result<zbus::Message>) -> Signal {
    match item {
        Ok(msg) => Signal::parse(&msg),
        Err(e) => {
            tracing::debug!(error = %e, "systemd: bad signal message");
            Signal::Other
        }
    }
}

impl UnitSignals for SystemdSignals<'_> {
    async fn next(&mut self, cx: &Cx, d: Duration) -> Result<Option<Signal>, DbusError> {
        Ok(self
            .conn
            .next_signal(cx, &mut self.stream, d)
            .await?
            .map(decode))
    }

    async fn ready(&mut self) -> Result<Option<Signal>, DbusError> {
        match futures_lite::future::poll_once(self.stream.next()).await {
            None => Ok(None),
            Some(Some(item)) => Ok(Some(decode(item))),
            Some(None) => Err(DbusError::Closed),
        }
    }
}

/// ADR-008 schedule: a full listing once `Demand.services` has been true for a full
/// `RESYNC_PERIOD` since the last listing. While it is false the period starts over, so no
/// listing runs.
struct Schedule {
    since: Time,
}

impl Schedule {
    fn new(now: Time) -> Self {
        Self { since: now }
    }

    fn due(&mut self, now: Time, wanted: bool) -> bool {
        if !wanted {
            self.since = now;
            return false;
        }
        Duration::from_nanos(now.duration_since(self.since)) >= RESYNC_PERIOD
    }

    /// A full listing just ran.
    fn done(&mut self, now: Time) {
        self.since = now;
    }
}

/// What the signal loop does next.
enum Wake {
    /// `cx` was cancelled.
    Stop,
    /// The stream ended or failed.
    Lost(DbusError),
    /// Signals to apply, and whether a full listing is due (`RefreshNow` or the schedule).
    Batch { signals: Vec<Signal>, resync: bool },
}

/// Waits for signals, `RefreshNow` or the resync schedule. Each wait on the stream is at most
/// `SIGNAL_WAIT`; after it `cx` is checked. A batch is every signal already queued after the
/// first one, up to `MAX_BATCH`.
async fn wait_next<S: UnitSignals>(
    cx: &Cx,
    io: &WorkerIo,
    id: SourceId,
    signals: &mut S,
    schedule: &mut Schedule,
    last_seq: &mut u64,
) -> Wake {
    loop {
        let first = match signals.next(cx, SIGNAL_WAIT).await {
            Ok(first) => first,
            Err(_) if cx.is_cancel_requested() => return Wake::Stop,
            Err(e) => return Wake::Lost(e),
        };
        if cx.checkpoint().is_err() {
            return Wake::Stop;
        }
        let mut batch = Vec::new();
        if let Some(signal) = first {
            batch.push(signal);
            while batch.len() < MAX_BATCH {
                match signals.ready().await {
                    Ok(Some(signal)) => batch.push(signal),
                    Ok(None) => break,
                    Err(e) => return Wake::Lost(e),
                }
            }
        }
        let refresh = take_refresh(&io.control.borrow_and_clone(), id, last_seq);
        let wanted = io.demand.borrow_and_clone().services;
        let resync = schedule.due(cx.now(), wanted) || refresh;
        if resync || !batch.is_empty() {
            return Wake::Batch {
                signals: batch,
                resync,
            };
        }
    }
}

// State

/// The bus work a signal batch needs, which `ServicesState` cannot do itself.
#[derive(Debug, Default)]
struct Effects {
    /// Units to read with `GetAll`, each once.
    fetch: Vec<(Arc<str>, OwnedObjectPath)>,
    /// Run `ListUnitFilesByPatterns` again.
    list_files: bool,
    /// Run a full listing; the rest of the batch is then moot.
    resync: bool,
    /// Units whose `MainPID` or `User` changed, so the User column is resolved again.
    users: Vec<Arc<str>>,
    /// The record list changed.
    changed: bool,
}

impl Effects {
    fn fetch(&mut self, name: &Arc<str>, path: &OwnedObjectPath) {
        if !self.fetch.iter().any(|(n, _)| n == name) {
            self.fetch.push((Arc::clone(name), path.clone()));
        }
    }
}

/// One shown unit and the record built from it.
struct Entry {
    kind: UnitType,
    raw: RawUnit,
    path: OwnedObjectPath,
    user: Arc<str>,
    record: Arc<UnitRecord>,
}

/// The unit records of one manager, and the signal logic that keeps them current. Pure: the
/// bus reads a signal needs come back as `Effects`. A record whose data did not change keeps
/// its `Arc`.
struct ServicesState {
    manager: Manager,
    units: BTreeMap<Arc<str>, Entry>,
    paths: HashMap<OwnedObjectPath, Arc<str>>,
    /// The last `ListUnitFilesByPatterns` result.
    files: UnitFiles,
}

impl ServicesState {
    fn new(manager: Manager) -> Self {
        Self {
            manager,
            units: BTreeMap::new(),
            paths: HashMap::new(),
            files: UnitFiles::new(),
        }
    }

    /// The `Services` update with every record, sorted by name.
    fn update(&self, state: ManagerState) -> SourceUpdate {
        let units = self.units.values().map(|e| (*e.record).clone()).collect();
        services(self.manager, state, units)
    }

    fn raw(&self, name: &str) -> Option<&RawUnit> {
        self.units.get(name).map(|e| &e.raw)
    }

    fn entry(&self, unit: Fetched, user: Arc<str>) -> Entry {
        let record = Arc::new(record(
            self.manager,
            unit.kind,
            &unit.raw,
            Arc::clone(&user),
        ));
        Entry {
            kind: unit.kind,
            raw: unit.raw,
            path: unit.path,
            user,
            record,
        }
    }

    /// Replaces every record with a full listing. Units whose data did not change keep their
    /// `Arc`. True when anything changed.
    fn replace(&mut self, units: Vec<(Fetched, Arc<str>)>, files: UnitFiles) -> bool {
        let mut old = std::mem::take(&mut self.units);
        self.paths.clear();
        self.files = files;
        let mut changed = false;
        for (unit, user) in units {
            let name = Arc::clone(&unit.raw.name);
            let entry = match old.remove(&name) {
                Some(e) if e.kind == unit.kind && e.raw == unit.raw && e.user == user => e,
                _ => {
                    changed = true;
                    self.entry(unit, user)
                }
            };
            self.paths.insert(entry.path.clone(), Arc::clone(&name));
            self.units.insert(name, entry);
        }
        changed || !old.is_empty()
    }

    /// Adds or replaces one unit after a `GetAll`, unless it is hidden. True when the record
    /// list changed.
    fn upsert(&mut self, unit: Fetched, user: Arc<str>) -> bool {
        let name = Arc::clone(&unit.raw.name);
        let has_file = self.files.contains_key(&*name) || !unit.raw.fragment_path.is_empty();
        if is_hidden(&name, &unit.raw.load_state, has_file) {
            return self.remove(&name);
        }
        if let Some(e) = self.units.get(&name)
            && e.kind == unit.kind
            && e.raw == unit.raw
            && e.user == user
        {
            return false;
        }
        let entry = self.entry(unit, user);
        self.paths.insert(entry.path.clone(), Arc::clone(&name));
        self.units.insert(name, entry);
        true
    }

    fn remove(&mut self, name: &str) -> bool {
        match self.units.remove(name) {
            Some(e) => {
                self.paths.remove(&e.path);
                true
            }
            None => false,
        }
    }

    /// Replaces the raw data of `name` and rebuilds its record when it changed.
    fn set_raw(&mut self, name: &str, raw: RawUnit) -> bool {
        let manager = self.manager;
        let Some(e) = self.units.get_mut(name) else {
            return false;
        };
        if e.raw == raw {
            return false;
        }
        e.raw = raw;
        e.record = Arc::new(record(manager, e.kind, &e.raw, Arc::clone(&e.user)));
        true
    }

    fn set_user(&mut self, name: &str, user: Arc<str>) -> bool {
        let manager = self.manager;
        let Some(e) = self.units.get_mut(name) else {
            return false;
        };
        if e.user == user {
            return false;
        }
        e.user = user;
        e.record = Arc::new(record(manager, e.kind, &e.raw, Arc::clone(&e.user)));
        true
    }

    /// `UnitFilesChanged`: the new unit file list, and the startup type of every record that
    /// has a file in it.
    fn set_files(&mut self, files: UnitFiles) -> bool {
        let mut changed = false;
        let updates: Vec<(Arc<str>, RawUnit)> = self
            .units
            .iter()
            .filter_map(|(name, e)| {
                let state = files.get(&**name)?;
                (*e.raw.unit_file_state != **state).then(|| {
                    let mut raw = e.raw.clone();
                    raw.unit_file_state = Arc::from(state.as_str());
                    (Arc::clone(name), raw)
                })
            })
            .collect();
        for (name, raw) in updates {
            changed |= self.set_raw(&name, raw);
        }
        self.files = files;
        changed
    }

    /// Applies one signal. Bus reads it needs are added to `fx`.
    fn apply(&mut self, signal: &Signal, fx: &mut Effects) {
        match signal {
            Signal::UnitNew { name, path } => {
                if name_shown(name) {
                    fx.fetch(name, path);
                }
            }
            Signal::UnitRemoved { name } => {
                fx.fetch.retain(|(n, _)| n != name);
                if self.files.contains_key(&**name) {
                    let Some(mut raw) = self.raw(name).cloned() else {
                        return;
                    };
                    raw.active_state = Arc::from("inactive");
                    raw.sub_state = Arc::from("dead");
                    fx.changed |= self.set_raw(name, raw);
                } else {
                    fx.changed |= self.remove(name);
                }
            }
            Signal::UnitFilesChanged => fx.list_files = true,
            Signal::Reloading(active) => fx.resync |= !active,
            Signal::Properties {
                path,
                iface,
                changed,
                invalidated,
            } => {
                let Some(name) = self.paths.get(path).cloned() else {
                    return;
                };
                let Some(e) = self.units.get(&name) else {
                    return;
                };
                let typed = if iface == UNIT_IFACE {
                    false
                } else if type_iface(e.kind) == Some(iface.as_str()) {
                    true
                } else {
                    return;
                };
                if invalidated.iter().any(|p| is_tracked(p, typed)) {
                    fx.fetch(&name, &e.path);
                }
                let mut raw = e.raw.clone();
                raw.apply(changed, typed);
                let rewho = raw.main_pid != e.raw.main_pid || raw.user != e.raw.user;
                if self.set_raw(&name, raw) {
                    fx.changed = true;
                    if rewho && !fx.users.contains(&name) {
                        fx.users.push(name);
                    }
                }
            }
            Signal::Other => {}
        }
    }

    #[cfg(test)]
    fn record(&self, name: &str) -> Option<&Arc<UnitRecord>> {
        self.units.get(name).map(|e| &e.record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use asupersync::channel::watch;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::demand::Demand;
    use tl_model::settings::EngineSettings;
    use zbus::zvariant::{Array, Structure};

    use crate::source::QueuedCommand;

    fn worker_io() -> (
        WorkerIo,
        mpsc::Receiver<EngineMsg>,
        watch::Sender<WorkerControl>,
        watch::Sender<Demand>,
    ) {
        let (updates, rx) = mpsc::channel::<EngineMsg>(256);
        let (demand_tx, demand) = watch::channel(Demand::default());
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(
            updates,
            demand,
            settings,
            control,
            crate::dbus::SharedHandles::default(),
        );
        (io, rx, control_tx, demand_tx)
    }

    fn props(entries: Vec<(&str, Value<'static>)>) -> Props {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).expect("owned value")))
            .collect()
    }

    #[test]
    fn status_mapping_table() {
        let rows = [
            ("active", "running", ServiceStatus::Running),
            ("active", "exited", ServiceStatus::Active),
            ("activating", "start", ServiceStatus::Starting),
            ("deactivating", "stop", ServiceStatus::Stopping),
            ("reloading", "reload", ServiceStatus::Reloading),
            ("refreshing", "", ServiceStatus::Reloading),
            ("inactive", "dead", ServiceStatus::Stopped),
            ("failed", "failed", ServiceStatus::Failed),
            ("maintenance", "", ServiceStatus::Maintenance),
        ];
        for (active, sub, want) in rows {
            let raw = RawUnit {
                name: "a.service".into(),
                active_state: active.into(),
                sub_state: sub.into(),
                ..RawUnit::default()
            };
            let rec = record(Manager::System, UnitType::Service, &raw, "root".into());
            assert_eq!(rec.status, want, "{active}/{sub}");
            assert_eq!(&*rec.sub_state, sub);
        }
    }

    #[test]
    fn startup_type_mapping_table() {
        let rows = [
            ("enabled", StartupType::Enabled),
            ("enabled-runtime", StartupType::EnabledRuntime),
            ("disabled", StartupType::Disabled),
            ("static", StartupType::Static),
            ("indirect", StartupType::Indirect),
            ("masked", StartupType::Masked),
            ("masked-runtime", StartupType::Masked),
            ("generated", StartupType::Generated),
            ("transient", StartupType::Transient),
            ("alias", StartupType::Alias),
            ("linked", StartupType::Linked),
            ("linked-runtime", StartupType::Linked),
            ("", StartupType::NoUnitFile),
            ("bad", StartupType::NoUnitFile),
        ];
        for (state, want) in rows {
            let raw = RawUnit {
                name: "a.service".into(),
                unit_file_state: state.into(),
                ..RawUnit::default()
            };
            let rec = record(Manager::User, UnitType::Service, &raw, "u".into());
            assert_eq!(rec.startup, want, "{state:?}");
        }
    }

    #[test]
    fn hidden_units_filtered() {
        assert!(is_hidden("foo@.service", "loaded", true));
        assert!(is_hidden("sys-devices-x.device", "loaded", false));
        assert!(is_hidden("gone.service", "not-found", false));
        assert!(!is_hidden("foo@1.service", "loaded", true));
        // A not-found unit that still has a file is shown.
        assert!(!is_hidden("half.service", "not-found", true));
        assert!(!is_hidden("dev-sda.swap", "loaded", false));
        assert!(is_hidden("noext", "loaded", true));

        assert_eq!(unit_kind("a.service"), Some(UnitType::Service));
        assert_eq!(unit_kind("a.socket"), Some(UnitType::Socket));
        assert_eq!(unit_kind("a.timer"), Some(UnitType::Timer));
        assert_eq!(unit_kind("-.mount"), Some(UnitType::Mount));
        assert_eq!(unit_kind("a.automount"), Some(UnitType::Automount));
        assert_eq!(unit_kind("a.path"), Some(UnitType::Path));
        assert_eq!(unit_kind("a.target"), Some(UnitType::Target));
        assert_eq!(unit_kind("a.swap"), Some(UnitType::Swap));
        assert_eq!(unit_kind("a.slice"), Some(UnitType::Slice));
        assert_eq!(unit_kind("a.scope"), Some(UnitType::Scope));
        assert_eq!(unit_kind("a.device"), None);
    }

    #[test]
    fn user_column_fallbacks() {
        let mut names = PasswdUserNames::from_bytes(
            b"root:x:0:0::/root:/bin/sh\nalice:x:1000:1000::/home/alice:/bin/sh\nsvc:x:977:977::/:/bin/false\n",
        );
        // MainPID owner wins over User=.
        let u = resolve_user(Manager::System, Some(977), "nobody", &mut names, 1000);
        assert_eq!(&*u, "svc");
        let u = resolve_user(Manager::System, None, "nobody", &mut names, 1000);
        assert_eq!(&*u, "nobody");
        let u = resolve_user(Manager::System, None, "", &mut names, 1000);
        assert_eq!(&*u, "root");
        let u = resolve_user(Manager::User, None, "", &mut names, 1000);
        assert_eq!(&*u, "alice");

        // MainPID=0 never reads /proc, so the system manager shows root.
        let unit = props(vec![("Description", Value::from("d"))]);
        let typed = props(vec![("MainPID", Value::U32(0)), ("User", Value::from(""))]);
        let raw = RawUnit::from_props("a.service", &unit, &typed);
        assert_eq!(raw.main_pid, 0);
        let u = resolve_user(Manager::System, None, &raw.user, &mut names, 1000);
        assert_eq!(&*u, "root");
    }

    #[test]
    fn pid_uid_reads_real_uid() {
        // The fixture root lives under the workspace target dir, never /tmp.
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp/dbus_systemd_pid_uid");
        let pid_dir = base.join("proc/42");
        std::fs::create_dir_all(&pid_dir).expect("fixture dir");
        std::fs::write(
            pid_dir.join("status"),
            "Name:\tx\nUid:\t977\t0\t0\t0\nGid:\t977\t977\t977\t977\n",
        )
        .expect("fixture file");
        let root = SysRoot::at(&base);
        let mut buf = Vec::new();
        let mut st = PidStatus::default();
        assert_eq!(pid_real_uid(&root, 42, &mut buf, &mut st), Some(977));
        assert_eq!(pid_real_uid(&root, 43, &mut buf, &mut st), None);
    }

    #[test]
    fn path_column_per_kind() {
        let exec = Value::from(Array::from(vec![Value::from(Structure::from((
            "/usr/bin/foo",
            vec!["foo", "--x"],
            false,
            0u64,
            0u64,
            0u64,
            0u64,
            0u32,
            0i32,
            0i32,
        )))]));
        let typed = props(vec![("ExecStart", exec), ("MainPID", Value::U32(12))]);
        let raw = RawUnit::from_props("foo.service", &Props::new(), &typed);
        assert_eq!(&*path_of(UnitType::Service, &raw), "/usr/bin/foo");
        assert_eq!(raw.main_pid, 12);

        let listen = Value::from(Array::from(vec![Value::from(Structure::from((
            "Stream",
            "/run/foo.sock",
        )))]));
        let raw = RawUnit::from_props(
            "foo.socket",
            &Props::new(),
            &props(vec![("Listen", listen)]),
        );
        assert_eq!(&*path_of(UnitType::Socket, &raw), "/run/foo.sock");

        let typed = props(vec![("What", Value::from("/dev/sda1"))]);
        let raw = RawUnit::from_props("boot.mount", &Props::new(), &typed);
        assert_eq!(&*path_of(UnitType::Mount, &raw), "/dev/sda1");

        let typed = props(vec![("Unit", Value::from("foo.service"))]);
        let raw = RawUnit::from_props("foo.timer", &Props::new(), &typed);
        assert_eq!(&*path_of(UnitType::Timer, &raw), "foo.service");
        // MainPID is a services-only column.
        let typed = props(vec![("MainPID", Value::U32(5))]);
        assert_eq!(
            RawUnit::from_props("foo.timer", &Props::new(), &typed).main_pid,
            0
        );

        let raw = RawUnit::from_props("multi-user.target", &Props::new(), &Props::new());
        assert_eq!(&*path_of(UnitType::Target, &raw), "");
    }

    #[test]
    fn unit_props_to_record() {
        let unit = props(vec![
            ("Description", Value::from("Foo daemon")),
            ("LoadState", Value::from("loaded")),
            ("ActiveState", Value::from("active")),
            ("SubState", Value::from("running")),
            ("UnitFileState", Value::from("enabled")),
            (
                "FragmentPath",
                Value::from("/usr/lib/systemd/system/foo.service"),
            ),
            ("ActiveEnterTimestamp", Value::U64(1_700_000_000_000_000)),
            ("TriggeredBy", Value::from(vec!["foo.socket"])),
            ("CanReload", Value::Bool(true)),
        ]);
        let typed = props(vec![(
            "ControlGroup",
            Value::from("/system.slice/foo.service"),
        )]);
        let raw = RawUnit::from_props("foo.service", &unit, &typed);
        let rec = record(Manager::System, UnitType::Service, &raw, "root".into());
        assert_eq!(rec.unit.name.as_ref(), "foo.service");
        assert_eq!(rec.unit.manager, Manager::System);
        assert_eq!(&*rec.description, "Foo daemon");
        assert_eq!(rec.status, ServiceStatus::Running);
        assert_eq!(rec.startup, StartupType::Enabled);
        assert_eq!(&*rec.load_state, "loaded");
        assert_eq!(
            rec.unit_file.as_deref(),
            Some("/usr/lib/systemd/system/foo.service")
        );
        assert_eq!(rec.active_since_us, Some(1_700_000_000_000_000));
        assert_eq!(rec.triggered_by.len(), 1);
        assert_eq!(&*rec.triggered_by[0], "foo.socket");
        assert_eq!(rec.cpu_pct.validity, Validity::Missing);
        assert_eq!(
            rec.control_group.as_deref(),
            Some("/system.slice/foo.service")
        );
        assert!(rec.can_reload);

        let raw = RawUnit::from_props("bar.service", &Props::new(), &Props::new());
        let rec = record(Manager::User, UnitType::Service, &raw, "u".into());
        assert_eq!(rec.unit_file, None);
        assert_eq!(rec.active_since_us, None);
        assert_eq!(rec.control_group, None);
        assert!(!rec.can_reload);
    }

    #[test]
    fn refresh_now_for_this_source_only() {
        let control = |cmds: Vec<(u64, SourceId, WorkerCommand)>| WorkerControl {
            commands: cmds
                .into_iter()
                .map(|(seq, target, command)| QueuedCommand {
                    seq,
                    target,
                    command,
                })
                .collect(),
            ..WorkerControl::default()
        };
        let mut last = 0;
        let other = control(vec![(
            1,
            SourceId::SystemdSystem,
            WorkerCommand::RefreshNow,
        )]);
        assert!(!take_refresh(&other, SourceId::SystemdUser, &mut last));
        assert_eq!(last, 1);
        let mine = control(vec![
            (1, SourceId::SystemdSystem, WorkerCommand::RefreshNow),
            (2, SourceId::SystemdUser, WorkerCommand::RefreshNow),
        ]);
        assert!(take_refresh(&mine, SourceId::SystemdUser, &mut last));
        assert_eq!(last, 2);
        // Seen once only.
        assert!(!take_refresh(&mine, SourceId::SystemdUser, &mut last));
    }

    /// Always reports the bus as missing.
    struct NoBus;

    impl Opener for NoBus {
        async fn open(&self, _cx: &Cx, _manager: Manager) -> Result<DbusConn, OpenError> {
            Err(OpenError::Unavailable(DbusError::Connect(
                "no session bus".into(),
            )))
        }
    }

    fn advance(lab: &mut LabRuntime, d: Duration) {
        lab.advance_time(u64::try_from(d.as_nanos()).expect("fits in u64"));
        // `run_until_idle` stops when no task is ready; one step fires the due timers.
        lab.step_for_test();
        lab.run_until_idle();
    }

    fn unavailable_count(rx: &mut mpsc::Receiver<EngineMsg>) -> usize {
        let mut n = 0;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                EngineMsg::Source(SourceUpdate::Services(up)) => {
                    assert_eq!(up.manager, Manager::User);
                    assert_eq!(up.state, ManagerState::Unavailable);
                    assert!(up.units.is_empty());
                    n += 1;
                }
                other => panic!("unexpected message {other:?}"),
            }
        }
        n
    }

    #[test]
    fn no_session_bus_publishes_unavailable() {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
        let (io, mut rx, control_tx, _demand_tx) = worker_io();
        let cfg = Arc::new(EngineConfig::new(
            EngineMode::Full,
            std::path::PathBuf::from("/nonexistent"),
        ));
        type Ended = Arc<Mutex<Option<Outcome<(), WorkerError>>>>;
        let out = Ended::default();
        let sink = Arc::clone(&out);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let outcome = run(cx, io, cfg, Manager::User, &NoBus).await;
                *sink.lock().expect("slot lock") = Some(outcome);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        assert_eq!(unavailable_count(&mut rx), 1);

        // No retry before 60 s.
        for _ in 0..118 {
            advance(&mut lab, IDLE_RECHECK);
        }
        assert_eq!(unavailable_count(&mut rx), 0);
        advance(&mut lab, IDLE_RECHECK);
        advance(&mut lab, IDLE_RECHECK);
        assert_eq!(unavailable_count(&mut rx), 1, "retried after 60 s");

        // RefreshNow retries at the next check.
        control_tx
            .send(WorkerControl {
                commands: Arc::from([QueuedCommand {
                    seq: 1,
                    target: SourceId::SystemdUser,
                    command: WorkerCommand::RefreshNow,
                }]),
                ..WorkerControl::default()
            })
            .expect("control send");
        advance(&mut lab, IDLE_RECHECK);
        assert_eq!(unavailable_count(&mut rx), 1, "retried on RefreshNow");
        assert!(
            out.lock().expect("slot lock").is_none(),
            "worker still runs"
        );

        handle.abort();
        advance(&mut lab, IDLE_RECHECK);
        let outcome = out.lock().expect("slot lock").take().expect("worker ended");
        assert!(matches!(outcome, Outcome::Ok(())), "{outcome:?}");
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    struct FakeSettings;

    #[zbus::interface(name = "org.freedesktop.portal.Settings")]
    impl FakeSettings {
        fn read(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
            if namespace == "org.freedesktop.appearance" && key == "color-scheme" {
                Ok(OwnedValue::from(1u32))
            } else {
                Err(zbus::fdo::Error::Failed("not found".into()))
            }
        }
    }

    /// A peer-to-peer pair: the server side plays the portal settings at its usual path.
    fn fake_session() -> (zbus::Connection, zbus::Connection) {
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let server = zbus::connection::Builder::async_io_unix_stream(a)
            .server(zbus::Guid::generate())
            .expect("server builder")
            .p2p()
            .serve_at("/org/freedesktop/portal/desktop", FakeSettings)
            .expect("settings")
            .build();
        let client = zbus::connection::Builder::async_io_unix_stream(b)
            .p2p()
            .build();
        let (server, client) =
            futures_lite::future::block_on(futures_lite::future::zip(server, client));
        (
            server.expect("server connects"),
            client.expect("client connects"),
        )
    }

    /// The colour scheme messages `start_color_scheme` sends for `manager` within `wait`.
    fn schemes_for(manager: Manager, wait: Duration) -> Vec<u8> {
        let (server, client) = fake_session();
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async move {
            let cx = Cx::current().expect("task has a Cx");
            let (tx, mut rx) = mpsc::channel::<EngineMsg>(16);
            let guard = start_color_scheme(&cx, EngineMode::Full, manager, &client, &tx);
            assert_eq!(guard.is_some(), manager == Manager::User);
            let mut got = Vec::new();
            if let Ok(Ok(msg)) = time::timeout(cx.now(), wait, rx.recv(&cx)).await {
                match msg {
                    EngineMsg::Source(SourceUpdate::ColorScheme(v)) => got.push(v),
                    other => panic!("unexpected message {other:?}"),
                }
            }
            drop(guard);
            drop(server);
            got
        });
        rt.block_on(join).expect("task completes")
    }

    #[test]
    fn color_scheme_watch_starts_on_user_connect() {
        assert_eq!(schemes_for(Manager::User, Duration::from_secs(5)), [1]);
        assert!(schemes_for(Manager::System, Duration::from_millis(300)).is_empty());
    }

    #[test]
    fn specs_are_full_mode_only() {
        for (spec, id) in [
            (spec_system(), SourceId::SystemdSystem),
            (spec_user(), SourceId::SystemdUser),
        ] {
            assert_eq!(spec.id, id);
            assert_eq!(spec.modes, &[EngineMode::Full]);
        }
        let full = crate::workers::workers_for(EngineMode::Full);
        for id in [SourceId::SystemdSystem, SourceId::SystemdUser] {
            assert_eq!(full.iter().filter(|w| w.id == id).count(), 1);
        }
        let low = crate::workers::workers_for(EngineMode::LowMemory);
        assert!(!low.iter().any(|w| w.id == SourceId::SystemdSystem));
    }

    /// Needs the real system bus. Read-only: it lists units and reads properties.
    #[test]
    #[ignore = "needs a system bus with systemd (owner machine)"]
    fn real_system_listing() {
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async {
            let cx = Cx::current().expect("task has a Cx");
            let conn = BusOpener
                .open(&cx, Manager::System)
                .await
                .expect("system manager");
            list_units(&cx, &conn).await.expect("listing").units
        });
        let units = rt.block_on(join).expect("task completes");
        assert!(units.iter().any(|u| &*u.raw.name == "multi-user.target"));
        assert!(!units.iter().any(|u| u.raw.name.ends_with(".device")));
        assert!(!units.iter().any(|u| u.raw.name.contains("@.")));
    }

    /// Needs the real session bus with a systemd user manager. Read-only: it adds the match
    /// rules, calls `Subscribe` and waits for signals; it changes no unit.
    #[test]
    #[ignore = "needs a session bus with systemd (owner machine)"]
    fn real_user_subscribe() {
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async {
            let cx = Cx::current().expect("task has a Cx");
            let conn = BusOpener
                .open(&cx, Manager::User)
                .await
                .expect("user manager");
            let mut signals = SystemdSignals::open(&cx, &conn).await.expect("subscribe");
            // A quiet manager sends nothing; the wait must time out cleanly, not fail.
            signals
                .next(&cx, Duration::from_secs(1))
                .await
                .expect("stream stays open");
        });
        rt.block_on(join).expect("task completes");
    }

    // Signal state

    fn unit(name: &str, active: &str, sub: &str, file: &str) -> (Fetched, Arc<str>) {
        let fragment = if file.is_empty() {
            String::new()
        } else {
            format!("/usr/lib/systemd/user/{name}")
        };
        let props = props(vec![
            ("LoadState", Value::from("loaded")),
            ("ActiveState", Value::from(active.to_owned())),
            ("SubState", Value::from(sub.to_owned())),
            ("UnitFileState", Value::from(file.to_owned())),
            ("FragmentPath", Value::from(fragment)),
        ]);
        let raw = RawUnit::from_props(name, &props, &Props::new());
        let fetched = Fetched {
            kind: unit_kind(name).expect("shown kind"),
            raw,
            path: unit_path(name),
        };
        (fetched, Arc::from("alice"))
    }

    /// The object path systemd uses, with `.`, `-` and `@` escaped.
    fn unit_path(name: &str) -> OwnedObjectPath {
        let mut path = String::from(UNIT_PATH_NAMESPACE);
        path.push('/');
        for c in name.chars() {
            if c.is_ascii_alphanumeric() {
                path.push(c);
            } else {
                path.push_str(&format!("_{:02x}", c as u32));
            }
        }
        OwnedObjectPath::try_from(path).expect("valid path")
    }

    fn sample_state() -> ServicesState {
        let mut state = ServicesState::new(Manager::User);
        let files: UnitFiles = [("foo.service", "enabled"), ("bar.service", "disabled")]
            .into_iter()
            .map(|(n, s)| (n.to_owned(), s.to_owned()))
            .collect();
        let changed = state.replace(
            vec![
                unit("bar.service", "inactive", "dead", "disabled"),
                unit("foo.service", "active", "running", "enabled"),
                unit("run-u5.scope", "active", "running", ""),
            ],
            files,
        );
        assert!(changed);
        state
    }

    fn properties(name: &str, changed: Props, invalidated: &[&str]) -> Signal {
        Signal::Properties {
            path: unit_path(name),
            iface: UNIT_IFACE.to_owned(),
            changed,
            invalidated: invalidated.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn arcs(state: &ServicesState) -> Vec<Arc<UnitRecord>> {
        state
            .units
            .values()
            .map(|e| Arc::clone(&e.record))
            .collect()
    }

    #[test]
    fn apply_properties_changed_keeps_other_arcs() {
        let mut state = sample_state();
        let before = arcs(&state);
        let mut fx = Effects::default();
        let changed = props(vec![
            ("ActiveState", Value::from("inactive")),
            ("SubState", Value::from("dead")),
        ]);
        state.apply(&properties("foo.service", changed, &[]), &mut fx);
        assert!(fx.changed);
        assert!(fx.fetch.is_empty() && fx.users.is_empty() && !fx.resync);
        let foo = state.record("foo.service").expect("foo stays");
        assert_eq!(foo.status, ServiceStatus::Stopped);
        assert_eq!(&*foo.sub_state, "dead");
        for name in ["bar.service", "run-u5.scope"] {
            let old = before
                .iter()
                .find(|r| &*r.unit.name == name)
                .expect("was there");
            assert!(
                Arc::ptr_eq(old, state.record(name).expect("still there")),
                "{name}"
            );
        }

        // The same values again change nothing, and the record keeps its new Arc.
        let foo = Arc::clone(state.record("foo.service").expect("foo"));
        let mut fx = Effects::default();
        let same = props(vec![("SubState", Value::from("dead"))]);
        state.apply(&properties("foo.service", same, &[]), &mut fx);
        assert!(!fx.changed);
        assert!(Arc::ptr_eq(&foo, state.record("foo.service").expect("foo")));

        // A full listing with unchanged data keeps every Arc.
        let before = arcs(&state);
        let mut relisted = vec![
            unit("bar.service", "inactive", "dead", "disabled"),
            unit("foo.service", "inactive", "dead", "enabled"),
            unit("run-u5.scope", "active", "running", ""),
        ];
        relisted.sort_by(|a, b| a.0.raw.name.cmp(&b.0.raw.name));
        let files = state.files.clone();
        assert!(!state.replace(relisted, files));
        for (a, b) in before.iter().zip(arcs(&state)) {
            assert!(Arc::ptr_eq(a, &b));
        }
    }

    #[test]
    fn unit_removed_with_and_without_file() {
        let mut state = sample_state();
        let mut fx = Effects::default();
        state.apply(
            &Signal::UnitRemoved {
                name: "foo.service".into(),
            },
            &mut fx,
        );
        let foo = state
            .record("foo.service")
            .expect("a unit with a file stays");
        assert_eq!(foo.status, ServiceStatus::Stopped);
        assert_eq!(&*foo.sub_state, "dead");
        assert!(fx.changed);

        let mut fx = Effects::default();
        state.apply(
            &Signal::UnitRemoved {
                name: "run-u5.scope".into(),
            },
            &mut fx,
        );
        assert!(
            state.record("run-u5.scope").is_none(),
            "a transient scope goes"
        );
        assert!(fx.changed);
        // Its path is forgotten, so later signals for it are ignored.
        let mut fx = Effects::default();
        let late = props(vec![("ActiveState", Value::from("active"))]);
        state.apply(&properties("run-u5.scope", late, &["SubState"]), &mut fx);
        assert!(!fx.changed && fx.fetch.is_empty());
        assert_eq!(state.units.len(), 2);

        // UnitNew asks for a GetAll, UnitRemoved in the same batch cancels it.
        let mut fx = Effects::default();
        let path = unit_path("run-u6.scope");
        state.apply(
            &Signal::UnitNew {
                name: "run-u6.scope".into(),
                path,
            },
            &mut fx,
        );
        assert_eq!(fx.fetch.len(), 1);
        state.apply(
            &Signal::UnitRemoved {
                name: "run-u6.scope".into(),
            },
            &mut fx,
        );
        assert!(fx.fetch.is_empty());
        // A device never gets a GetAll.
        let path = unit_path("dev-sda.device");
        state.apply(
            &Signal::UnitNew {
                name: "dev-sda.device".into(),
                path,
            },
            &mut fx,
        );
        assert!(fx.fetch.is_empty());

        // The GetAll result is added, unless it is hidden.
        let (fetched, user) = unit("run-u7.scope", "active", "running", "");
        assert!(state.upsert(fetched.clone(), Arc::clone(&user)));
        assert!(!state.upsert(fetched, user), "same data, no change");
        let (mut gone, user) = unit("gone.service", "inactive", "dead", "");
        gone.raw.load_state = "not-found".into();
        assert!(!state.upsert(gone, user));
        assert!(state.record("gone.service").is_none());
    }

    #[test]
    fn invalidated_requests_getall() {
        let mut state = sample_state();
        let mut fx = Effects::default();
        state.apply(
            &properties("foo.service", Props::new(), &["SubState"]),
            &mut fx,
        );
        assert_eq!(fx.fetch.len(), 1);
        assert_eq!(&*fx.fetch[0].0, "foo.service");
        assert_eq!(fx.fetch[0].1, unit_path("foo.service"));
        // A second invalidation in the same batch asks for no second GetAll.
        state.apply(
            &properties("foo.service", Props::new(), &["ActiveState"]),
            &mut fx,
        );
        assert_eq!(fx.fetch.len(), 1);
        // A property the record does not keep asks for none.
        let mut fx = Effects::default();
        state.apply(
            &properties("bar.service", Props::new(), &["Conditions"]),
            &mut fx,
        );
        assert!(fx.fetch.is_empty());
        // Another interface on the unit object is ignored.
        state.apply(
            &Signal::Properties {
                path: unit_path("bar.service"),
                iface: "org.freedesktop.systemd1.Socket".to_owned(),
                changed: Props::new(),
                invalidated: vec!["Listen".to_owned()],
            },
            &mut fx,
        );
        assert!(fx.fetch.is_empty());
        // The service interface: MainPID changes ask for the User column again.
        state.apply(
            &Signal::Properties {
                path: unit_path("foo.service"),
                iface: "org.freedesktop.systemd1.Service".to_owned(),
                changed: props(vec![("MainPID", Value::U32(4242))]),
                invalidated: vec!["ExecStart".to_owned()],
            },
            &mut fx,
        );
        assert_eq!(fx.fetch.len(), 1);
        assert_eq!(fx.users.len(), 1);
        assert_eq!(state.record("foo.service").expect("foo").main_pid, 4242);
        assert!(state.set_user("foo.service", "svc".into()));
        assert_eq!(&*state.record("foo.service").expect("foo").user, "svc");
    }

    #[test]
    fn manager_signals_set_effects() {
        let mut state = sample_state();
        let mut fx = Effects::default();
        state.apply(&Signal::Reloading(true), &mut fx);
        assert!(!fx.resync);
        state.apply(&Signal::Reloading(false), &mut fx);
        assert!(fx.resync);
        let mut fx = Effects::default();
        state.apply(&Signal::Other, &mut fx);
        state.apply(&Signal::UnitFilesChanged, &mut fx);
        assert!(fx.list_files && !fx.changed);

        let bar = Arc::clone(state.record("bar.service").expect("bar"));
        let files: UnitFiles = [("foo.service", "masked"), ("bar.service", "disabled")]
            .into_iter()
            .map(|(n, s)| (n.to_owned(), s.to_owned()))
            .collect();
        assert!(state.set_files(files));
        assert_eq!(
            state.record("foo.service").expect("foo").startup,
            StartupType::Masked
        );
        assert!(Arc::ptr_eq(&bar, state.record("bar.service").expect("bar")));
        // The scope has no unit file; its startup type stays.
        assert_eq!(
            state.record("run-u5.scope").expect("scope").startup,
            StartupType::NoUnitFile
        );
    }

    /// Never yields a signal: each `next` waits `d`, or ends the stream when `end` is set.
    struct QuietSignals {
        end: bool,
    }

    impl UnitSignals for QuietSignals {
        async fn next(&mut self, cx: &Cx, d: Duration) -> Result<Option<Signal>, DbusError> {
            if self.end {
                return Err(DbusError::Closed);
            }
            let _ = time::timeout(cx.now(), d, std::future::pending::<()>()).await;
            if cx.checkpoint().is_err() {
                return Err(DbusError::Closed);
            }
            Ok(None)
        }

        async fn ready(&mut self) -> Result<Option<Signal>, DbusError> {
            Ok(None)
        }
    }

    #[test]
    fn resync_schedule_follows_demand() {
        let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(1_000_000));
        let (io, _rx, _control_tx, demand_tx) = worker_io();
        let resyncs = Arc::new(Mutex::new(0usize));
        let count = Arc::clone(&resyncs);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let mut signals = QuietSignals { end: false };
                let mut schedule = Schedule::new(cx.now());
                let mut last_seq = 0;
                let id = SourceId::SystemdUser;
                loop {
                    let wake =
                        wait_next(&cx, &io, id, &mut signals, &mut schedule, &mut last_seq).await;
                    match wake {
                        Wake::Stop => break,
                        Wake::Lost(e) => panic!("stream lost: {e}"),
                        Wake::Batch { signals, resync } => {
                            assert!(signals.is_empty());
                            assert!(resync);
                            *count.lock().expect("count lock") += 1;
                            schedule.done(cx.now());
                        }
                    }
                }
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let resyncs_now = || *resyncs.lock().expect("count lock");

        // Demand off: no resync in 180 virtual seconds.
        for _ in 0..360 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        assert_eq!(resyncs_now(), 0);

        // Demand on: exactly one resync once 60 s have passed.
        demand_tx
            .send(Demand {
                services: true,
                ..Demand::default()
            })
            .expect("demand send");
        for _ in 0..119 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        assert_eq!(resyncs_now(), 0, "not before 60 s");
        advance(&mut lab, SIGNAL_WAIT);
        assert_eq!(resyncs_now(), 1, "one at 60 s");
        for _ in 0..119 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        assert_eq!(resyncs_now(), 1, "still one at 119.5 s");
        advance(&mut lab, SIGNAL_WAIT);
        assert_eq!(resyncs_now(), 2, "the next one at 120 s");

        handle.abort();
        advance(&mut lab, SIGNAL_WAIT);
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn refresh_now_resyncs_at_once() {
        let mut lab = LabRuntime::new(LabConfig::new(43).max_steps(1_000_000));
        let (io, _rx, control_tx, _demand_tx) = worker_io();
        let woke = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&woke);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let mut signals = QuietSignals { end: false };
                let mut schedule = Schedule::new(cx.now());
                let mut last_seq = 0;
                let id = SourceId::SystemdSystem;
                let wake =
                    wait_next(&cx, &io, id, &mut signals, &mut schedule, &mut last_seq).await;
                let resync = matches!(wake, Wake::Batch { resync: true, .. });
                *sink.lock().expect("slot lock") = Some((resync, cx.now().as_nanos()));
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        advance(&mut lab, Duration::from_secs(5));
        assert!(woke.lock().expect("slot lock").is_none());
        control_tx
            .send(WorkerControl {
                commands: Arc::from([QueuedCommand {
                    seq: 1,
                    target: SourceId::SystemdSystem,
                    command: WorkerCommand::RefreshNow,
                }]),
                ..WorkerControl::default()
            })
            .expect("control send");
        advance(&mut lab, SIGNAL_WAIT);
        let (resync, at) = woke.lock().expect("slot lock").take().expect("woke");
        assert!(resync);
        assert!(at <= 5_500_000_000, "within one wait, at {at} ns");
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn stream_end_publishes_reconnecting() {
        let mut lab = LabRuntime::new(LabConfig::new(44).max_steps(1_000_000));
        let (io, mut rx, _control_tx, _demand_tx) = worker_io();
        type Ended = Arc<Mutex<Option<Outcome<(), WorkerError>>>>;
        let out = Ended::default();
        let sink = Arc::clone(&out);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let state = sample_state();
                let mut signals = QuietSignals { end: true };
                let mut schedule = Schedule::new(cx.now());
                let mut last_seq = 0;
                let id = SourceId::SystemdUser;
                let outcome =
                    match wait_next(&cx, &io, id, &mut signals, &mut schedule, &mut last_seq).await
                    {
                        Wake::Lost(e) => lost(&cx, &io, &state, e).await,
                        _ => panic!("the stream ended"),
                    };
                *sink.lock().expect("slot lock") = Some(outcome);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let outcome = out.lock().expect("slot lock").take().expect("ended");
        assert!(
            matches!(outcome, Outcome::Err(WorkerError::Dbus(_))),
            "{outcome:?}"
        );
        match rx.try_recv() {
            Ok(EngineMsg::Source(SourceUpdate::Services(up))) => {
                assert_eq!(up.state, ManagerState::Reconnecting);
                assert_eq!(up.validity, Validity::Stale);
                assert_eq!(up.units.len(), 3, "the last records are kept");
            }
            other => panic!("unexpected {other:?}"),
        }
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }
}
