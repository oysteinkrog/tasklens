//! The `SystemdSystem` and `SystemdUser` workers (ADR-004, SYS-001 to SYS-003).
//!
//! One body serves both managers: `Manager::System` on the system bus, `Manager::User` on the
//! session bus. On connect it lists every loaded unit and every installed unit file, reads
//! their properties and sends one `SourceUpdate::Services` with the unit records. It lists
//! again on `RefreshNow`. Signals, cgroup data and unit details come in E14-002 to E14-004.
//!
//! A bus that cannot be opened, or a bus without `org.freedesktop.systemd1`, is an expected
//! condition: the worker publishes the manager as `Unavailable` and tries again on `RefreshNow`
//! or after 60 s. A call timeout or a connection error during a listing ends the generation
//! with `Err` so the supervisor restarts it.
//!
//! Unit descriptions and paths are logged at `debug` level only.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::runtime::{JoinError, TaskHandle};
use asupersync::supervision::ManagedGeneration;
use asupersync::time;
use asupersync::{Cx, Outcome};
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

/// How long an unavailable manager waits before the next try when no `RefreshNow` comes.
pub const RETRY_UNAVAILABLE: Duration = Duration::from_secs(60);

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

/// One connected generation: fill the slot, start the colour scheme watch, list, then list
/// again on each `RefreshNow`.
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
    loop {
        let started = cx.now();
        let units = match list_units(cx, &conn, manager, &cfg.sys_root).await {
            Ok(units) => units,
            Err(e) => {
                if cx.checkpoint().is_err() {
                    return Outcome::Ok(());
                }
                return Outcome::Err(e.into());
            }
        };
        let took = Duration::from_nanos(cx.now().duration_since(started));
        tracing::debug!(?manager, units = units.len(), ?took, "systemd: listing done");
        if let Some(stop) = send(cx, io, services(manager, ManagerState::Connected, units)).await {
            return stop;
        }
        if !wait_refresh(cx, io, id, None, &mut last_seq).await {
            return Outcome::Ok(());
        }
    }
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
    let found = control.commands.iter().any(|c| {
        c.seq > *last_seq && c.target == id && c.command == WorkerCommand::RefreshNow
    });
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
    let validity = match state {
        ManagerState::Connected => Validity::Valid,
        _ => Validity::Missing,
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
}

impl RawUnit {
    fn from_props(name: &str, unit: &Props, typed: &Props) -> Self {
        let first_field = |key: &str, index: usize| -> Arc<str> {
            let Some(Value::Array(rows)) = typed.get(key).map(|v| unwrap(v)) else {
                return Arc::from("");
            };
            match rows.inner().first() {
                Some(Value::Structure(row)) => row.fields().get(index).map_or_else(
                    || Arc::from(""),
                    |v| Arc::from(str_of(v).unwrap_or_default()),
                ),
                _ => Arc::from(""),
            }
        };
        let kind = unit_kind(name);
        RawUnit {
            name: Arc::from(name),
            description: prop_str(unit, "Description"),
            load_state: prop_str(unit, "LoadState"),
            active_state: prop_str(unit, "ActiveState"),
            sub_state: prop_str(unit, "SubState"),
            unit_file_state: prop_str(unit, "UnitFileState"),
            fragment_path: prop_str(unit, "FragmentPath"),
            active_enter_us: prop_u64(unit, "ActiveEnterTimestamp"),
            triggered_by: prop_strv(unit, "TriggeredBy"),
            main_pid: if kind == Some(UnitType::Service) {
                prop_u32(typed, "MainPID")
            } else {
                0
            },
            user: prop_str(typed, "User"),
            exec_start: first_field("ExecStart", 0),
            listen: first_field("Listen", 1),
            what: prop_str(typed, "What"),
            timer_unit: prop_str(typed, "Unit"),
        }
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
fn pid_real_uid(sys_root: &SysRoot, pid: u32, buf: &mut Vec<u8>, st: &mut PidStatus) -> Option<u32> {
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

async fn manager_call<B, T>(cx: &Cx, conn: &DbusConn, method: &str, body: &B) -> Result<T, DbusError>
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
    let call = conn
        .raw()
        .call_method(Some(BUS_NAME), path, Some(PROPERTIES_IFACE), "GetAll", &(iface,));
    let reply = conn.read(cx, call).await?;
    reply.body().deserialize::<Props>().map_err(decode_error)
}

/// Lists loaded units and installed unit files, loads the files that are not loaded, reads
/// every unit's properties and builds the records, sorted by name.
async fn list_units(
    cx: &Cx,
    conn: &DbusConn,
    manager: Manager,
    sys_root: &SysRoot,
) -> Result<Vec<UnitRecord>, DbusError> {
    let patterns = (Vec::<&str>::new(), Vec::<&str>::new());
    let loaded: Vec<ListedUnit> = manager_call(cx, conn, "ListUnitsByPatterns", &patterns).await?;
    let files: Vec<(String, String)> =
        manager_call(cx, conn, "ListUnitFilesByPatterns", &patterns).await?;

    let file_names: HashSet<String> = files
        .iter()
        .filter_map(|(path, _)| path.rsplit('/').next().map(str::to_owned))
        .collect();
    let mut targets: Vec<(String, OwnedObjectPath)> = Vec::with_capacity(loaded.len());
    let mut seen = HashSet::with_capacity(loaded.len());
    for (name, _, _, _, _, _, path, ..) in loaded {
        if name_shown(&name) && seen.insert(name.clone()) {
            targets.push((name, path));
        }
    }
    let mut unloaded: Vec<&String> = file_names
        .iter()
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

    let empty = Props::new();
    let mut raws: Vec<(UnitType, RawUnit)> = Vec::with_capacity(targets.len());
    for (name, path) in &targets {
        let Some(kind) = unit_kind(name) else {
            continue;
        };
        let unit = match get_all(cx, conn, path, UNIT_IFACE).await {
            Ok(props) => props,
            Err(e) if is_unit_error(&e) => {
                tracing::debug!(unit = %name, error = %e, "systemd: GetAll failed");
                continue;
            }
            Err(e) => return Err(e),
        };
        let typed = match type_iface(kind) {
            Some(iface) => match get_all(cx, conn, path, iface).await {
                Ok(props) => Some(props),
                Err(e) if is_unit_error(&e) => {
                    tracing::debug!(unit = %name, error = %e, "systemd: GetAll failed");
                    None
                }
                Err(e) => return Err(e),
            },
            None => None,
        };
        let raw = RawUnit::from_props(name, &unit, typed.as_ref().unwrap_or(&empty));
        let has_file = file_names.contains(name.as_str()) || !raw.fragment_path.is_empty();
        if !is_hidden(name, &raw.load_state, has_file) {
            raws.push((kind, raw));
        }
    }
    raws.sort_by(|a, b| a.1.name.cmp(&b.1.name));

    let users = resolve_users(cx, manager, sys_root, &raws).await?;
    Ok(raws
        .iter()
        .zip(users)
        .map(|((kind, raw), user)| record(manager, *kind, raw, user))
        .collect())
}

/// The User column for each unit in `raws`, in order. The /proc and passwd reads run in one
/// blocking job.
async fn resolve_users(
    cx: &Cx,
    manager: Manager,
    sys_root: &SysRoot,
    raws: &[(UnitType, RawUnit)],
) -> Result<Vec<Arc<str>>, DbusError> {
    let inputs: Vec<(u32, Arc<str>)> = raws
        .iter()
        .map(|(_, raw)| (raw.main_pid, Arc::clone(&raw.user)))
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
    ) {
        let (updates, rx) = mpsc::channel::<EngineMsg>(256);
        let (_demand_tx, demand) = watch::channel(Demand::default());
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(
            updates,
            demand,
            settings,
            control,
            crate::dbus::SharedHandles::default(),
        );
        (io, rx, control_tx)
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
        let raw = RawUnit::from_props("foo.socket", &Props::new(), &props(vec![("Listen", listen)]));
        assert_eq!(&*path_of(UnitType::Socket, &raw), "/run/foo.sock");

        let typed = props(vec![("What", Value::from("/dev/sda1"))]);
        let raw = RawUnit::from_props("boot.mount", &Props::new(), &typed);
        assert_eq!(&*path_of(UnitType::Mount, &raw), "/dev/sda1");

        let typed = props(vec![("Unit", Value::from("foo.service"))]);
        let raw = RawUnit::from_props("foo.timer", &Props::new(), &typed);
        assert_eq!(&*path_of(UnitType::Timer, &raw), "foo.service");
        // MainPID is a services-only column.
        let typed = props(vec![("MainPID", Value::U32(5))]);
        assert_eq!(RawUnit::from_props("foo.timer", &Props::new(), &typed).main_pid, 0);

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
            ("FragmentPath", Value::from("/usr/lib/systemd/system/foo.service")),
            ("ActiveEnterTimestamp", Value::U64(1_700_000_000_000_000)),
            ("TriggeredBy", Value::from(vec!["foo.socket"])),
        ]);
        let raw = RawUnit::from_props("foo.service", &unit, &Props::new());
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

        let raw = RawUnit::from_props("bar.service", &Props::new(), &Props::new());
        let rec = record(Manager::User, UnitType::Service, &raw, "u".into());
        assert_eq!(rec.unit_file, None);
        assert_eq!(rec.active_since_us, None);
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
        let other = control(vec![(1, SourceId::SystemdSystem, WorkerCommand::RefreshNow)]);
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
            Err(OpenError::Unavailable(DbusError::Connect("no session bus".into())))
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
        let (io, mut rx, control_tx) = worker_io();
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
        assert!(out.lock().expect("slot lock").is_none(), "worker still runs");

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
            list_units(&cx, &conn, Manager::System, &SysRoot::host())
                .await
                .expect("listing")
        });
        let units = rt.block_on(join).expect("task completes");
        assert!(units.iter().any(|u| &*u.unit.name == "multi-user.target"));
        assert!(!units.iter().any(|u| u.unit.name.ends_with(".device")));
        assert!(!units.iter().any(|u| u.unit.name.contains("@.")));
    }
}
