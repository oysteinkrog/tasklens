//! The `SystemdSystem` and `SystemdUser` workers (ADR-004, SYS-001 to SYS-005, SYS-016).
//!
//! One body serves both managers: `Manager::System` on the system bus, `Manager::User` on the
//! session bus. On connect it lists every loaded unit and every installed unit file, reads
//! their properties and sends one `SourceUpdate::Services` with the unit records. Then it
//! subscribes to the manager and keeps the records live from unit signals (`ServicesState`),
//! with one publish per signal batch. A full listing runs again on `RefreshNow`, on
//! `Reloading(false)` and every 60 s while the Services page wants data (ADR-008).
//!
//! Unit details (E14-004): for the first 10 units in `Demand.unit_details` that belong to this
//! manager, the worker reads `GetAll` on `Unit` (and `Service` for services) once when the unit
//! enters demand, then keeps the details current from `PropertiesChanged`. A unit that leaves
//! demand loses its details. A failed read leaves the unit out and is tried again on its next
//! `PropertiesChanged` or the next full listing. Details go out in the same update as the
//! records.
//!
//! Cgroup data (E14-003): each listing walks the unit cgroup trees once for `has_cgroup_procs`.
//! While `Demand.services` and `Demand.process_table` are both true, a 1 Hz tick lists each
//! unit's processes as `cgroup_pids`, from one walk that reads each `cgroup.procs` once. The root
//! slice `-.slice` gets no pids; while `Demand.service_cgroup_stats` is true, the same tick reads the
//! unit resource columns. All reads of one tick run in one blocking job. With neither demand set,
//! no tick runs and no cgroup file is read.
//!
//! A bus that cannot be opened, or a bus without `org.freedesktop.systemd1`, is an expected
//! condition: the worker publishes the manager as `Unavailable` and tries again on `RefreshNow`
//! or after 60 s. A call timeout or a connection error during the first listing ends the
//! generation with `Err` so the supervisor restarts it. Once connected, a lost bus publishes
//! the manager as `Reconnecting` with the last records, then ends with `Err`. A call that times
//! out on a connected bus is not a lost bus: a busy systemd can take over 2 s to answer
//! `ListUnitFilesByPatterns`. The worker keeps the last records and lists again after
//! `RESYNC_RETRY`.
//!
//! OpenRC (E14-008, SYS-018): at worker start, a host without `/run/systemd/system` but with
//! `rc-status` runs the OpenRC path. The `SystemdSystem` worker lists services with
//! `rc-status --all --format ini` on `RefreshNow` and every 5 s while `Demand.services` is true,
//! and publishes them with `backend = OpenRc`. The `SystemdUser` worker publishes its manager as
//! `Unavailable`. A host with neither runs the systemd path, which publishes `Unavailable`.
//!
//! Unit descriptions and paths are logged at `debug` level only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use asupersync::channel::mpsc;
use asupersync::runtime::{JoinError, TaskHandle};
use asupersync::supervision::ManagedGeneration;
use asupersync::time;
use asupersync::types::Time;
use asupersync::{Cx, Outcome};
use futures_lite::StreamExt;
use futures_lite::stream::Boxed;
use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use smallvec::SmallVec;
use tl_collect::counters::{Counter, Delta};
use tl_collect::parse::topology::parse_cpu_list;
use tl_collect::proc::cgroup::{CgroupRoot, CgroupStats, CgroupWant};
use tl_collect::proc::meta::{PasswdUserNames, UserNames};
use tl_collect::proc::pid_stat::read_start_ticks;
use tl_collect::proc::status::{PidStatus, STATUS_READ_MAX, parse_status};
use tl_collect::sysroot::read_file;
use tl_collect::{CollectError, SysRoot};
use tl_model::demand::{Demand, UNIT_DETAILS_MAX};
use tl_model::health::{ManagerState, SourceId};
use tl_model::ids::ProcessKey;
use tl_model::snapshot::{
    DepEntry, PidIds, RecoveryKeySet, RecoverySettings, Relation, ServiceBackend, ServiceStatus,
    StartupType, UnitDetails, UnitRecord,
};
use tl_model::table::{Manager, Total, UnitRef, UnitType};
use tl_model::time::BootId;
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
/// After a call timed out on a connected bus, the next full listing comes this much later.
pub const RESYNC_RETRY: Duration = Duration::from_secs(10);
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
/// ADR-008: unit cgroup pids and stats at 1 Hz while demanded.
pub const CGROUP_PERIOD: Duration = Duration::from_secs(1);

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
    let mut backend = ServiceBackend::Systemd;
    if let Init::OpenRc(bin) = detect_init(host_base(&cfg.sys_root)) {
        if manager == Manager::System {
            let lister = Arc::new(RcStatus { bin });
            return run_openrc(&cx, &io, lister, last_seq).await;
        }
        backend = ServiceBackend::OpenRc;
    }
    loop {
        let opened = match backend {
            ServiceBackend::Systemd => opener.open(&cx, manager).await,
            ServiceBackend::OpenRc => Err(OpenError::Unavailable(DbusError::Connect(
                "OpenRC has no user manager".into(),
            ))),
        };
        match opened {
            Ok(conn) => return connected(&cx, &io, &cfg, manager, conn, last_seq).await,
            Err(OpenError::Unavailable(reason)) => {
                if cx.checkpoint().is_err() {
                    return Outcome::Ok(());
                }
                tracing::debug!(?manager, %reason, "systemd: manager not available");
                let state = ManagerState::Unavailable;
                let update = services(manager, state, Vec::new(), Vec::new(), backend);
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

/// One connected generation: fill the slot, start the colour scheme watch, then `serve`.
async fn connected(
    cx: &Cx,
    io: &WorkerIo,
    cfg: &EngineConfig,
    manager: Manager,
    conn: DbusConn,
    last_seq: u64,
) -> Outcome<(), WorkerError> {
    let slot = slot_of(io, manager);
    slot.set(Arc::new(conn.raw().clone()));
    // Locals drop before parameters, so both guards go before `conn` stops its tick task.
    let _clear = ClearOnDrop(slot.clone());
    let _scheme = start_color_scheme(cx, cfg.mode, manager, conn.raw(), &io.updates);
    serve(cx, io, cfg, manager, &conn, last_seq).await
}

/// List, subscribe, then apply signals and resync on schedule until cancelled or the bus is
/// lost. A call that times out keeps the last records and moves the next listing to
/// `RESYNC_RETRY` from now.
async fn serve<B: SystemdBus>(
    cx: &Cx,
    io: &WorkerIo,
    cfg: &EngineConfig,
    manager: Manager,
    bus: &B,
    mut last_seq: u64,
) -> Outcome<(), WorkerError> {
    let id = source_id(manager);
    let mut state = ServicesState::new(manager);
    let fs = HostFs {
        root: cfg.sys_root.clone(),
    };
    let dropins = dropin_base(manager, &cfg.sys_root);
    let first = match resync(cx, bus, manager, &cfg.sys_root, &fs, &mut state).await {
        Ok(_) => {
            let wanted = io.demand.borrow_and_clone().unit_details;
            let host = DetailsHost {
                sys_root: &cfg.sys_root,
                dropins: dropins.as_deref(),
            };
            sync_details(cx, bus, host, &mut state, &wanted, true).await
        }
        Err(e) => Err(e),
    };
    if let Err(e) = first {
        // Nothing was published by this generation, so there is nothing to keep.
        if cx.checkpoint().is_err() {
            return Outcome::Ok(());
        }
        return Outcome::Err(e.into());
    }
    if let Some(stop) = send(cx, io, state.update(ManagerState::Connected)).await {
        return stop;
    }
    let mut signals = match bus.subscribe(cx).await {
        Ok(signals) => signals,
        Err(e) => return lost(cx, io, &state, e).await,
    };
    let mut schedule = Schedule::new(cx.now());
    let mut ticks = CgroupSchedule::default();
    loop {
        let wake = wait_next(
            cx,
            io,
            id,
            &mut signals,
            &mut schedule,
            &mut ticks,
            &mut last_seq,
        )
        .await;
        let (batch, mut full, tick) = match wake {
            Wake::Stop => return Outcome::Ok(()),
            Wake::Lost(e) => return lost(cx, io, &state, e).await,
            Wake::Batch {
                signals,
                resync,
                tick,
            } => (signals, resync, tick),
        };
        let mut changed = false;
        if !full {
            match apply_batch(cx, bus, manager, &cfg.sys_root, &mut state, &batch).await {
                Ok(BatchResult::Applied(c)) => changed = c,
                Ok(BatchResult::Resync) => full = true,
                Err(e @ DbusError::Timeout(_)) => {
                    // The signals already changed the state; the reads they needed come with
                    // the next listing.
                    changed = true;
                    timed_out(manager, &e, &mut schedule, cx.now());
                }
                Err(e) => return lost(cx, io, &state, e).await,
            }
        }
        if full {
            match resync(cx, bus, manager, &cfg.sys_root, &fs, &mut state).await {
                Ok(c) => {
                    changed = c;
                    schedule.done(cx.now());
                }
                Err(e @ DbusError::Timeout(_)) => {
                    full = false;
                    timed_out(manager, &e, &mut schedule, cx.now());
                }
                Err(e) => return lost(cx, io, &state, e).await,
            }
        }
        let wanted = io.demand.borrow_and_clone().unit_details;
        let host = DetailsHost {
            sys_root: &cfg.sys_root,
            dropins: dropins.as_deref(),
        };
        match sync_details(cx, bus, host, &mut state, &wanted, full).await {
            Ok(c) => changed |= c,
            Err(e) => return lost(cx, io, &state, e).await,
        }
        if let Some(want) = tick {
            changed |= cgroup_tick(cx, &fs, &mut state, want).await;
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

/// A call timed out on a connected bus. systemd is slow, the bus is not lost: the last records
/// stay and the next full listing is due `RESYNC_RETRY` from `now`.
fn timed_out(manager: Manager, e: &DbusError, schedule: &mut Schedule, now: Time) {
    tracing::warn!(
        ?manager,
        error = %e,
        "systemd: call timed out, keeping the last records"
    );
    schedule.retry(now);
}

/// A full listing into `state`, then the `has_cgroup_procs` walk. True when the record list
/// changed.
async fn resync<B: SystemdBus, F: CgroupFs>(
    cx: &Cx,
    bus: &B,
    manager: Manager,
    sys_root: &SysRoot,
    fs: &F,
    state: &mut ServicesState,
) -> Result<bool, DbusError> {
    let started = cx.now();
    let listing = bus.list(cx).await?;
    let users = resolve_users(cx, manager, sys_root, listing.units.iter().map(|f| &f.raw)).await?;
    let took = Duration::from_nanos(cx.now().duration_since(started));
    tracing::debug!(
        ?manager,
        units = listing.units.len(),
        ?took,
        "systemd: listing done"
    );
    let changed = state.replace(
        listing.units.into_iter().zip(users).collect(),
        listing.files,
    );
    Ok(cgroup_presence(cx, fs, state).await | changed)
}

enum BatchResult {
    /// The batch was applied; true when the record list changed.
    Applied(bool),
    /// The batch asked for a full listing.
    Resync,
}

/// Applies one signal batch: the state changes first, then the bus reads they ask for.
async fn apply_batch<B: SystemdBus>(
    cx: &Cx,
    bus: &B,
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
        changed |= state.set_files(bus.list_files(cx).await?);
    }
    let mut fetched = Vec::with_capacity(fx.fetch.len());
    for (name, path) in &fx.fetch {
        if let Some(unit) = bus.fetch_unit(cx, name, path).await? {
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

fn services(
    manager: Manager,
    state: ManagerState,
    units: Vec<UnitRecord>,
    details: Vec<(UnitRef, Arc<UnitDetails>)>,
    backend: ServiceBackend,
) -> SourceUpdate {
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
        details,
        backend,
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

// OpenRC

/// Exists only when systemd is the init system (`sd_booted`).
const SYSTEMD_MARKER: &str = "run/systemd/system";
/// Where `rc-status` lives on an OpenRC host, relative to the host root.
const RC_STATUS_PATHS: [&str; 2] = ["sbin/rc-status", "usr/bin/rc-status"];
/// SYS-018: an `rc-status` run that takes longer is killed.
pub const RC_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
/// R-167 item 16: the OpenRC listing runs every 5 s while `Demand.services` is true.
pub const OPENRC_PERIOD: Duration = Duration::from_secs(5);
/// Services in these runlevels start at boot, so their Startup type is Enabled.
const BOOT_RUNLEVELS: [&str; 3] = ["sysinit", "boot", "default"];
/// The most `rc-status` output kept. The rest is read and dropped.
const RC_STATUS_MAX: u64 = 1 << 20;
/// How often a running `rc-status` is checked for exit.
const RC_STATUS_POLL: Duration = Duration::from_millis(20);

/// The init system found at worker start.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Init {
    /// systemd, or no known init system: the bus path runs, and publishes `Unavailable` when
    /// systemd is not on the bus.
    Systemd,
    /// OpenRC, with the `rc-status` binary found.
    OpenRc(PathBuf),
}

/// The host root above `sys_root.proc`: `/` on the host, the fixture base in tests.
fn host_base(sys_root: &SysRoot) -> &Path {
    sys_root.proc.parent().unwrap_or(Path::new("/"))
}

/// systemd when `<root>/run/systemd/system` is a directory. Else OpenRC when
/// `<root>/sbin/rc-status` or `<root>/usr/bin/rc-status` is a file. Else systemd.
fn detect_init(root: &Path) -> Init {
    if root.join(SYSTEMD_MARKER).is_dir() {
        return Init::Systemd;
    }
    RC_STATUS_PATHS
        .iter()
        .map(|rel| root.join(rel))
        .find(|path| path.is_file())
        .map_or(Init::Systemd, Init::OpenRc)
}

/// One service from `rc-status`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RcService {
    name: Arc<str>,
    /// The state word, for example `started`.
    state: Arc<str>,
    /// Listed under `sysinit`, `boot` or `default`.
    enabled: bool,
}

/// Parses `rc-status --all --format ini`: `[<runlevel>]` headers, then `<service> = <state>`
/// lines. A service listed under more than one runlevel keeps its first state word and is
/// enabled when any of its runlevels starts at boot. Sorted by name.
fn parse_rc_status(text: &str) -> Vec<RcService> {
    let mut found: BTreeMap<&str, RcService> = BTreeMap::new();
    let mut runlevel = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            runlevel = Some(section.trim());
            continue;
        }
        let Some(runlevel) = runlevel else {
            continue;
        };
        let Some((name, state)) = line.split_once('=') else {
            continue;
        };
        let (name, state) = (name.trim(), state.trim());
        if name.is_empty() {
            continue;
        }
        let boot = BOOT_RUNLEVELS.contains(&runlevel);
        found
            .entry(name)
            .and_modify(|svc| svc.enabled |= boot)
            .or_insert_with(|| RcService {
                name: Arc::from(name),
                state: Arc::from(state),
                enabled: boot,
            });
    }
    found.into_values().collect()
}

/// R-167 item 16: the Status column from an OpenRC state word.
fn openrc_status(word: &str) -> ServiceStatus {
    match word {
        "started" => ServiceStatus::Running,
        "stopped" | "inactive" | "hotplugged" => ServiceStatus::Stopped,
        "crashed" => ServiceStatus::Failed,
        "starting" => ServiceStatus::Starting,
        "stopping" => ServiceStatus::Stopping,
        _ => ServiceStatus::Active,
    }
}

fn unsupported<T: Default>() -> Total<T> {
    Total {
        value: T::default(),
        validity: Validity::Unsupported,
    }
}

/// An OpenRC service as a record. OpenRC has no cgroup data here, so the resource fields are
/// `Unsupported`. The User column follows the SYS-003 fallback for the system manager.
fn openrc_record(svc: &RcService) -> UnitRecord {
    UnitRecord {
        unit: UnitRef {
            manager: Manager::System,
            name: Arc::clone(&svc.name),
        },
        kind: UnitType::Service,
        description: Arc::from(""),
        status: openrc_status(&svc.state),
        sub_state: Arc::clone(&svc.state),
        load_state: Arc::from(""),
        startup: if svc.enabled {
            StartupType::Enabled
        } else {
            StartupType::Disabled
        },
        main_pid: 0,
        user: Arc::from("root"),
        path: Arc::from(""),
        unit_file: None,
        active_since_us: None,
        triggered_by: Arc::from([]),
        control_group: None,
        can_reload: false,
        has_cgroup_procs: false,
        cgroup_pids: Arc::from([]),
        cpu_pct: unsupported(),
        memory_bytes: unsupported(),
        disk_bps: unsupported(),
        tasks: unsupported(),
    }
}

fn openrc_update(list: &[RcService]) -> SourceUpdate {
    let units = list.iter().map(openrc_record).collect();
    let state = ManagerState::Connected;
    services(
        Manager::System,
        state,
        units,
        Vec::new(),
        ServiceBackend::OpenRc,
    )
}

#[derive(Debug, thiserror::Error)]
enum RcError {
    #[error("rc-status did not finish in {0:?} and was killed")]
    Timeout(Duration),
    #[error("rc-status exited with {0}")]
    Exit(std::process::ExitStatus),
    #[error("rc-status: {0}")]
    Io(#[from] std::io::Error),
}

/// Lists OpenRC services. Blocking. Tests replace it with a fake.
trait RcLister: Send + Sync + 'static {
    fn list(&self) -> Result<Vec<RcService>, RcError>;
}

/// The real `rc-status`.
struct RcStatus {
    bin: PathBuf,
}

impl RcLister for RcStatus {
    fn list(&self) -> Result<Vec<RcService>, RcError> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(["--all", "--format", "ini"]);
        let out = run_capped(cmd, RC_STATUS_TIMEOUT)?;
        Ok(parse_rc_status(&String::from_utf8_lossy(&out)))
    }
}

/// Runs `cmd` with `LC_ALL=C`, stdin and stderr null, and returns at most [`RC_STATUS_MAX`]
/// bytes of stdout. After `timeout` the child is killed and waited for. Blocking.
fn run_capped(mut cmd: Command, timeout: Duration) -> Result<Vec<u8>, RcError> {
    cmd.env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    // A reader thread drains the pipe, so the child never blocks on a full pipe.
    let reader = child.stdout.take().map(|mut out| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = (&mut out).take(RC_STATUS_MAX).read_to_end(&mut buf);
            let _ = std::io::copy(&mut out, &mut std::io::sink());
            buf
        })
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() >= deadline => break Err(RcError::Timeout(timeout)),
            Ok(None) => std::thread::sleep(RC_STATUS_POLL),
            Err(e) => break Err(RcError::Io(e)),
        }
    };
    if status.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    // The child is gone, so its pipe closes and the reader ends.
    let out = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    match status? {
        status if status.success() => Ok(out),
        status => Err(RcError::Exit(status)),
    }
}

/// The OpenRC path of the `SystemdSystem` worker: a listing on `RefreshNow` and every
/// [`OPENRC_PERIOD`] while `Demand.services` is true, none while it is false. A changed list is
/// published. A failed listing logs a warning and keeps the last records.
async fn run_openrc<L: RcLister>(
    cx: &Cx,
    io: &WorkerIo,
    lister: Arc<L>,
    mut last_seq: u64,
) -> Outcome<(), WorkerError> {
    let id = SourceId::SystemdSystem;
    let mut listed_at: Option<Time> = None;
    let mut shown: Option<Vec<RcService>> = None;
    loop {
        let refresh = take_refresh(&io.control.borrow_and_clone(), id, &mut last_seq);
        let due = io.demand.borrow_and_clone().services
            && listed_at.is_none_or(|at| {
                Duration::from_nanos(cx.now().duration_since(at)) >= OPENRC_PERIOD
            });
        if refresh || due {
            listed_at = Some(cx.now());
            let lister = Arc::clone(&lister);
            let mut job = match cx.spawn_blocking(move |_| lister.list()) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let listed = match job.join(cx).await {
                Ok(listed) => listed,
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
            match listed {
                Ok(list) if shown.as_ref() != Some(&list) => {
                    tracing::debug!(services = list.len(), "openrc: listing changed");
                    if let Some(stop) = send(cx, io, openrc_update(&list)).await {
                        return stop;
                    }
                    shown = Some(list);
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "openrc: listing failed, keeping the last records");
                }
            }
        }
        time::sleep(cx.now(), IDLE_RECHECK).await;
        if cx.checkpoint().is_err() {
            return Outcome::Ok(());
        }
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

/// True when `Id` names another unit: `name` is an alias that systemd resolved to that unit.
/// The target is listed under its own name, so the alias would repeat its PID and state.
fn is_alias(name: &str, unit: &Props) -> bool {
    let id = prop_str(unit, "Id");
    !id.is_empty() && *id != *name
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
    /// From the type interface (services, sockets, mounts, timers, slices and scopes); empty
    /// for kinds that are not read.
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

/// The type interface read besides `Unit`: for the four types whose columns need it, and for
/// slices and scopes, whose `ControlGroup` gives their cgroup pids and resource columns.
fn type_iface(kind: UnitType) -> Option<&'static str> {
    match kind {
        UnitType::Service => Some("org.freedesktop.systemd1.Service"),
        UnitType::Socket => Some("org.freedesktop.systemd1.Socket"),
        UnitType::Mount => Some("org.freedesktop.systemd1.Mount"),
        UnitType::Timer => Some("org.freedesktop.systemd1.Timer"),
        UnitType::Slice => Some("org.freedesktop.systemd1.Slice"),
        UnitType::Scope => Some("org.freedesktop.systemd1.Scope"),
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

/// The manager reads a connected generation makes. Tests replace the bus with a fake.
trait SystemdBus {
    type Signals<'a>: UnitSignals
    where
        Self: 'a;
    /// The full listing (`list_units`).
    async fn list(&self, cx: &Cx) -> Result<Listing, DbusError>;
    /// `ListUnitFilesByPatterns`.
    async fn list_files(&self, cx: &Cx) -> Result<UnitFiles, DbusError>;
    /// One unit's properties (`fetch_unit`).
    async fn fetch_unit(
        &self,
        cx: &Cx,
        name: &str,
        path: &OwnedObjectPath,
    ) -> Result<Option<Fetched>, DbusError>;
    /// `LoadUnit`: the object path of `name`.
    async fn load_unit(&self, cx: &Cx, name: &str) -> Result<OwnedObjectPath, DbusError>;
    /// One unit's detail properties (`fetch_details`).
    async fn fetch_details(
        &self,
        cx: &Cx,
        path: &OwnedObjectPath,
        kind: UnitType,
    ) -> Result<DetailRaw, DbusError>;
    /// The unit signals, after `Subscribe`.
    async fn subscribe(&self, cx: &Cx) -> Result<Self::Signals<'_>, DbusError>;
}

impl SystemdBus for DbusConn {
    type Signals<'a> = SystemdSignals<'a>;

    async fn list(&self, cx: &Cx) -> Result<Listing, DbusError> {
        list_units(cx, self).await
    }

    async fn list_files(&self, cx: &Cx) -> Result<UnitFiles, DbusError> {
        list_files(cx, self).await
    }

    async fn fetch_unit(
        &self,
        cx: &Cx,
        name: &str,
        path: &OwnedObjectPath,
    ) -> Result<Option<Fetched>, DbusError> {
        fetch_unit(cx, self, name, path).await
    }

    async fn load_unit(&self, cx: &Cx, name: &str) -> Result<OwnedObjectPath, DbusError> {
        manager_call(cx, self, "LoadUnit", &(name,)).await
    }

    async fn fetch_details(
        &self,
        cx: &Cx,
        path: &OwnedObjectPath,
        kind: UnitType,
    ) -> Result<DetailRaw, DbusError> {
        fetch_details(cx, self, path, kind).await
    }

    async fn subscribe(&self, cx: &Cx) -> Result<SystemdSignals<'_>, DbusError> {
        SystemdSignals::open(cx, self).await
    }
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
    if is_alias(name, &unit) {
        tracing::debug!(unit = %name, "systemd: alias unit left out");
        return Ok(None);
    }
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
    /// `RESYNC_PERIOD`, or `RESYNC_RETRY` after a call timed out until the next listing ran.
    period: Duration,
    /// The last `Demand.unit_details` the loop saw.
    details: Arc<[UnitRef]>,
}

impl Schedule {
    fn new(now: Time) -> Self {
        Self {
            since: now,
            period: RESYNC_PERIOD,
            details: Arc::from([]),
        }
    }

    /// True when `Demand.unit_details` differs from the last call.
    fn details_moved(&mut self, wanted: &Arc<[UnitRef]>) -> bool {
        if *self.details == **wanted {
            return false;
        }
        self.details = Arc::clone(wanted);
        true
    }

    fn due(&mut self, now: Time, wanted: bool) -> bool {
        if !wanted {
            self.since = now;
            return false;
        }
        Duration::from_nanos(now.duration_since(self.since)) >= self.period
    }

    /// A full listing just ran.
    fn done(&mut self, now: Time) {
        self.since = now;
        self.period = RESYNC_PERIOD;
    }

    /// A call timed out: the next listing is due `RESYNC_RETRY` from `now`.
    fn retry(&mut self, now: Time) {
        self.since = now;
        self.period = RESYNC_RETRY;
    }
}

/// What the signal loop does next.
enum Wake {
    /// `cx` was cancelled.
    Stop,
    /// The stream ended or failed.
    Lost(DbusError),
    /// Signals to apply, whether a full listing is due (`RefreshNow` or the schedule), and the
    /// cgroup tick when one is due.
    Batch {
        signals: Vec<Signal>,
        resync: bool,
        tick: Option<TickWant>,
    },
}

/// Waits for signals, `RefreshNow`, the resync schedule, the cgroup tick or a change of
/// `Demand.unit_details`. Each wait on the
/// stream is at most `SIGNAL_WAIT`, and ends early when the tick is due; after it `cx` is
/// checked. A batch is every signal already queued after the first one, up to `MAX_BATCH`.
async fn wait_next<S: UnitSignals>(
    cx: &Cx,
    io: &WorkerIo,
    id: SourceId,
    signals: &mut S,
    schedule: &mut Schedule,
    ticks: &mut CgroupSchedule,
    last_seq: &mut u64,
) -> Wake {
    loop {
        let wait = ticks.wait(cx.now(), &io.demand.borrow_and_clone());
        let next = if wait.is_zero() {
            signals.ready().await
        } else {
            signals.next(cx, wait).await
        };
        let first = match next {
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
        let demand = io.demand.borrow_and_clone();
        let resync = schedule.due(cx.now(), demand.services) || refresh;
        let tick = ticks.due(cx.now(), &demand);
        let details = schedule.details_moved(&demand.unit_details);
        if resync || tick.is_some() || details || !batch.is_empty() {
            return Wake::Batch {
                signals: batch,
                resync,
                tick,
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
    cg: UnitCgroup,
    record: Arc<UnitRecord>,
}

/// The record of one unit, with its cgroup data.
fn build(
    manager: Manager,
    kind: UnitType,
    raw: &RawUnit,
    user: &Arc<str>,
    cg: &UnitCgroup,
) -> Arc<UnitRecord> {
    let mut rec = record(manager, kind, raw, Arc::clone(user));
    cg.fill(&mut rec);
    Arc::new(rec)
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
    /// Read by the first cgroup tick that lists pids.
    boot: Option<BootId>,
    /// Online CPUs, the divisor of `cpu_pct`; the last value read, 1 before the first.
    cpus: u32,
    /// The demanded units of this manager, in demand order, at most `UNIT_DETAILS_MAX`.
    details: Vec<DetailEntry>,
}

impl ServicesState {
    fn new(manager: Manager) -> Self {
        Self {
            manager,
            units: BTreeMap::new(),
            paths: HashMap::new(),
            files: UnitFiles::new(),
            boot: None,
            cpus: 1,
            details: Vec::new(),
        }
    }

    /// The `Services` update with every record, sorted by name, and the details of every
    /// demanded unit that was read.
    fn update(&self, state: ManagerState) -> SourceUpdate {
        let units = self.units.values().map(|e| (*e.record).clone()).collect();
        let details = self
            .details
            .iter()
            .filter_map(|d| {
                let built = d.built.as_ref()?;
                let unit = UnitRef {
                    manager: self.manager,
                    name: Arc::clone(&d.name),
                };
                Some((unit, Arc::clone(built)))
            })
            .collect();
        services(self.manager, state, units, details, ServiceBackend::Systemd)
    }

    fn raw(&self, name: &str) -> Option<&RawUnit> {
        self.units.get(name).map(|e| &e.raw)
    }

    /// A new entry. `cg` is the unit's earlier cgroup data, kept when its cgroup is the same.
    fn entry(&self, unit: Fetched, user: Arc<str>, cg: Option<UnitCgroup>) -> Entry {
        let cg = cg
            .filter(|cg| cg.rel == unit.raw.control_group)
            .unwrap_or_else(|| UnitCgroup::new(Arc::clone(&unit.raw.control_group)));
        let record = build(self.manager, unit.kind, &unit.raw, &user, &cg);
        Entry {
            kind: unit.kind,
            raw: unit.raw,
            path: unit.path,
            user,
            cg,
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
                prev => {
                    changed = true;
                    self.entry(unit, user, prev.map(|e| e.cg))
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
        let prev = self.units.get(&name).map(|e| e.cg.clone());
        let entry = self.entry(unit, user, prev);
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
        if e.cg.rel != e.raw.control_group {
            e.cg = UnitCgroup::new(Arc::clone(&e.raw.control_group));
        }
        e.record = build(manager, e.kind, &e.raw, &e.user, &e.cg);
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
        e.record = build(manager, e.kind, &e.raw, &e.user, &e.cg);
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
                if !typed || e.kind == UnitType::Service {
                    fx.changed |= self.apply_details(&name, changed, invalidated);
                }
                let Some(e) = self.units.get(&name) else {
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

    /// `(name, ControlGroup)` of every unit that has a cgroup.
    fn cgroup_units(&self) -> Vec<(Arc<str>, Arc<str>)> {
        self.units
            .iter()
            .filter(|(_, e)| !e.raw.control_group.is_empty())
            .map(|(name, e)| (Arc::clone(name), Arc::clone(&e.raw.control_group)))
            .collect()
    }

    /// Applies the reads of one cgroup pass taken at `t_ns`. A read for a cgroup the unit no
    /// longer has is dropped. True when any record changed.
    fn apply_tick(&mut self, read: TickRead, t_ns: u64, fresh: bool) -> bool {
        if read.boot.is_some() {
            self.boot = read.boot;
        }
        if let Some(cpus) = read.cpus {
            self.cpus = cpus;
        }
        let manager = self.manager;
        let cpus = self.cpus;
        if fresh {
            for e in self.units.values_mut() {
                e.cg.cpu.reset();
                e.cg.io.reset();
            }
        }
        let mut changed = false;
        for unit in read.units {
            let Some(e) = self.units.get_mut(&unit.name) else {
                continue;
            };
            if e.cg.rel != unit.rel {
                continue;
            }
            let before = e.cg.clone();
            if let Some(has_procs) = unit.has_procs {
                e.cg.has_procs = has_procs;
            }
            if let Some(pids) = unit.pids {
                e.cg.pids = pids;
            }
            if let Some(stats) = unit.stats {
                e.cg.sample(stats, t_ns, cpus);
            }
            if !e.cg.same_shown(&before) {
                e.record = build(manager, e.kind, &e.raw, &e.user, &e.cg);
                changed = true;
            }
        }
        changed
    }

    #[cfg(test)]
    fn record(&self, name: &str) -> Option<&Arc<UnitRecord>> {
        self.units.get(name).map(|e| &e.record)
    }
}

// Details

/// The drop-in file a P3 recovery edit writes (SYS-013).
const RECOVERY_DROPIN: &str = "50-tasklens-recovery.conf";

/// The `as` dependency properties, in the order of the Dependencies tab (SYS-014).
const RELATIONS: [(&str, Relation); 14] = [
    ("Requires", Relation::Requires),
    ("Requisite", Relation::Requisite),
    ("Wants", Relation::Wants),
    ("BindsTo", Relation::BindsTo),
    ("PartOf", Relation::PartOf),
    ("Upholds", Relation::Upholds),
    ("RequiredBy", Relation::RequiredBy),
    ("RequisiteOf", Relation::RequisiteOf),
    ("WantedBy", Relation::WantedBy),
    ("BoundBy", Relation::BoundBy),
    ("ConsistsOf", Relation::ConsistsOf),
    ("UpheldBy", Relation::UpheldBy),
    ("After", Relation::After),
    ("Before", Relation::Before),
];

const GRAPHICAL_SESSION: &str = "graphical-session.target";

/// The properties one `UnitDetails` is built from, read from the `Unit` and `Service` maps. A
/// property the unit does not have keeps its default: empty, `None`, or `u64::MAX` for the
/// resource values.
#[derive(Clone, Debug, PartialEq)]
struct DetailRaw {
    id: Arc<str>,
    description: Arc<str>,
    documentation: Arc<[Arc<str>]>,
    /// The argv of `ExecStart[0]`. Never logged (ADR-015).
    exec_start_argv: Arc<[Arc<str>]>,
    fragment_path: Arc<str>,
    drop_in_paths: Arc<[Arc<str>]>,
    unit_file_state: Arc<str>,
    active_state: Arc<str>,
    sub_state: Arc<str>,
    active_enter_us: u64,
    main_pid: u32,
    memory_current: u64,
    tasks_current: u64,
    cpu_usage_ns: u64,
    user: Arc<str>,
    group: Arc<str>,
    dynamic_user: bool,
    supplementary_groups: Arc<[Arc<str>]>,
    pam_name: Arc<str>,
    recovery: RecoverySettings,
    /// One unit list per `RELATIONS` entry.
    deps: [Arc<[Arc<str>]>; 14],
}

impl Default for DetailRaw {
    fn default() -> Self {
        Self {
            id: Arc::from(""),
            description: Arc::from(""),
            documentation: Arc::from([]),
            exec_start_argv: Arc::from([]),
            fragment_path: Arc::from(""),
            drop_in_paths: Arc::from([]),
            unit_file_state: Arc::from(""),
            active_state: Arc::from(""),
            sub_state: Arc::from(""),
            active_enter_us: 0,
            main_pid: 0,
            memory_current: u64::MAX,
            tasks_current: u64::MAX,
            cpu_usage_ns: u64::MAX,
            user: Arc::from(""),
            group: Arc::from(""),
            dynamic_user: false,
            supplementary_groups: Arc::from([]),
            pam_name: Arc::from(""),
            recovery: RecoverySettings::default(),
            deps: Default::default(),
        }
    }
}

/// Sets `slot` from `key` when `props` has it.
fn set<T>(props: &Props, key: &str, slot: &mut T, read: fn(&Props, &str) -> T) {
    if props.contains_key(key) {
        *slot = read(props, key);
    }
}

fn opt_str(props: &Props, key: &str) -> Option<Arc<str>> {
    props.get(key).and_then(|v| str_of(v)).map(Arc::from)
}

fn opt_u32(props: &Props, key: &str) -> Option<u32> {
    match props.get(key).map(|v| unwrap(v)) {
        Some(Value::U32(n)) => Some(*n),
        _ => None,
    }
}

fn opt_u64(props: &Props, key: &str) -> Option<u64> {
    match props.get(key).map(|v| unwrap(v)) {
        Some(Value::U64(n)) => Some(*n),
        _ => None,
    }
}

/// The argv (`as`, field 1) of the first row of the `a(sasbttttuii)` property `key`, or empty.
fn first_argv(props: &Props, key: &str) -> Arc<[Arc<str>]> {
    let Some(Value::Array(rows)) = props.get(key).map(|v| unwrap(v)) else {
        return Arc::from([]);
    };
    let Some(Value::Structure(row)) = rows.inner().first().map(unwrap) else {
        return Arc::from([]);
    };
    match row.fields().get(1).map(unwrap) {
        Some(Value::Array(argv)) => argv
            .inner()
            .iter()
            .filter_map(|v| str_of(v).map(Arc::from))
            .collect(),
        _ => Arc::from([]),
    }
}

impl DetailRaw {
    fn from_props(unit: &Props, service: &Props) -> Self {
        let mut raw = DetailRaw::default();
        raw.apply(unit);
        raw.apply(service);
        raw
    }

    /// Sets the fields whose properties are in `props`, a `Unit` or a `Service` map.
    fn apply(&mut self, props: &Props) {
        set(props, "Id", &mut self.id, prop_str);
        set(props, "Description", &mut self.description, prop_str);
        set(props, "Documentation", &mut self.documentation, prop_strv);
        set(props, "ExecStart", &mut self.exec_start_argv, first_argv);
        set(props, "FragmentPath", &mut self.fragment_path, prop_str);
        set(props, "DropInPaths", &mut self.drop_in_paths, prop_strv);
        set(props, "UnitFileState", &mut self.unit_file_state, prop_str);
        set(props, "ActiveState", &mut self.active_state, prop_str);
        set(props, "SubState", &mut self.sub_state, prop_str);
        set(
            props,
            "ActiveEnterTimestamp",
            &mut self.active_enter_us,
            prop_u64,
        );
        set(props, "MainPID", &mut self.main_pid, prop_u32);
        set(props, "MemoryCurrent", &mut self.memory_current, prop_u64);
        set(props, "TasksCurrent", &mut self.tasks_current, prop_u64);
        set(props, "CPUUsageNSec", &mut self.cpu_usage_ns, prop_u64);
        set(props, "User", &mut self.user, prop_str);
        set(props, "Group", &mut self.group, prop_str);
        set(props, "DynamicUser", &mut self.dynamic_user, prop_bool);
        set(
            props,
            "SupplementaryGroups",
            &mut self.supplementary_groups,
            prop_strv,
        );
        set(props, "PAMName", &mut self.pam_name, prop_str);
        let r = &mut self.recovery;
        set(props, "Restart", &mut r.restart, opt_str);
        set(props, "RestartUSec", &mut r.restart_usec, opt_u64);
        set(props, "RestartSteps", &mut r.restart_steps, opt_u32);
        set(
            props,
            "RestartMaxDelayUSec",
            &mut r.restart_max_delay_usec,
            opt_u64,
        );
        set(props, "StartLimitBurst", &mut r.start_limit_burst, opt_u32);
        set(
            props,
            "StartLimitIntervalUSec",
            &mut r.start_limit_interval_usec,
            opt_u64,
        );
        set(
            props,
            "StartLimitAction",
            &mut r.start_limit_action,
            opt_str,
        );
        set(props, "FailureAction", &mut r.failure_action, opt_str);
        set(props, "OnFailure", &mut r.on_failure, prop_strv);
        set(props, "NRestarts", &mut r.n_restarts, opt_u32);
        for ((key, _), list) in RELATIONS.iter().zip(self.deps.iter_mut()) {
            set(props, key, list, prop_strv);
        }
    }
}

/// `u64::MAX` means systemd has no value (accounting off or not running).
fn known(v: u64) -> Option<u64> {
    (v != u64::MAX).then_some(v)
}

fn non_empty(s: &Arc<str>) -> Option<Arc<str>> {
    (!s.is_empty()).then(|| Arc::clone(s))
}

/// The details of one unit. `ids` is the real uid and gid of MainPID, `dropin` whether the
/// recovery drop-in exists.
fn details_of(
    manager: Manager,
    raw: &DetailRaw,
    ids: Option<PidIds>,
    dropin: Option<RecoveryKeySet>,
) -> UnitDetails {
    let list = |r: Relation| {
        let i = RELATIONS.iter().position(|(_, x)| *x == r).unwrap_or(0);
        &raw.deps[i]
    };
    let graphical_session = [Relation::PartOf, Relation::BindsTo]
        .into_iter()
        .any(|r| list(r).iter().any(|u| &**u == GRAPHICAL_SESSION));
    let mut deps: Vec<(Arc<str>, usize)> = raw
        .deps
        .iter()
        .enumerate()
        .flat_map(|(i, units)| units.iter().map(move |u| (Arc::clone(u), i)))
        .collect();
    deps.sort();
    deps.dedup();
    let deps = deps
        .into_iter()
        .map(|(name, i)| DepEntry {
            unit: UnitRef { manager, name },
            relation: RELATIONS[i].1,
        })
        .collect();
    UnitDetails {
        id: Arc::clone(&raw.id),
        description: Arc::clone(&raw.description),
        documentation: Arc::clone(&raw.documentation),
        exec_start_argv: Arc::clone(&raw.exec_start_argv),
        fragment_path: non_empty(&raw.fragment_path),
        drop_in_paths: Arc::clone(&raw.drop_in_paths),
        startup: StartupType::from_unit_file_state(&raw.unit_file_state),
        active_state: Arc::clone(&raw.active_state),
        sub_state: Arc::clone(&raw.sub_state),
        active_enter_timestamp_us: (raw.active_enter_us != 0).then_some(raw.active_enter_us),
        main_pid: raw.main_pid,
        memory_current: known(raw.memory_current),
        tasks_current: known(raw.tasks_current),
        cpu_usage_nsec: known(raw.cpu_usage_ns),
        manager,
        user: non_empty(&raw.user),
        group: non_empty(&raw.group),
        dynamic_user: raw.dynamic_user,
        supplementary_groups: Arc::clone(&raw.supplementary_groups),
        pam_name: non_empty(&raw.pam_name),
        main_pid_ids: if raw.main_pid == 0 { None } else { ids },
        graphical_session,
        recovery: raw.recovery.clone(),
        recovery_dropin_exists: dropin.is_some(),
        recovery_dropin_keys: dropin.unwrap_or_default(),
        deps,
    }
}

/// One demanded unit.
#[derive(Debug)]
struct DetailEntry {
    name: Arc<str>,
    /// The object path from `LoadUnit`, for a unit that has no record (not loaded, or hidden
    /// from the table, as a dependency can be).
    path: Option<OwnedObjectPath>,
    /// `None` until the first `GetAll` pair succeeds.
    raw: Option<DetailRaw>,
    ids: Option<PidIds>,
    /// `None` without a drop-in, else the Recovery keys it sets.
    dropin: Option<RecoveryKeySet>,
    /// What the last update published; `None` while `raw` is.
    built: Option<Arc<UnitDetails>>,
    /// Read the unit with `GetAll` again: it is new, its last read failed, or a signal
    /// invalidated properties.
    refetch: bool,
    /// Read MainPID's ids and the drop-in again.
    reread_host: bool,
}

impl DetailEntry {
    fn new(name: Arc<str>) -> Self {
        Self {
            name,
            path: None,
            raw: None,
            ids: None,
            dropin: None,
            built: None,
            refetch: true,
            reread_host: false,
        }
    }

    fn rebuild(&mut self, manager: Manager) {
        self.built = self
            .raw
            .as_ref()
            .map(|raw| Arc::new(details_of(manager, raw, self.ids, self.dropin)));
    }
}

impl ServicesState {
    /// Sets the demanded units: the first `UNIT_DETAILS_MAX` of `wanted` that belong to this
    /// manager, in that order. Kept units keep their entry. True when published details were
    /// dropped.
    fn want_details(&mut self, wanted: &[UnitRef]) -> bool {
        let names: Vec<&Arc<str>> = wanted
            .iter()
            .take(UNIT_DETAILS_MAX)
            .filter(|u| u.manager == self.manager)
            .map(|u| &u.name)
            .collect();
        if names.len() == self.details.len()
            && names.iter().zip(&self.details).all(|(n, d)| **n == d.name)
        {
            return false;
        }
        let mut old = std::mem::take(&mut self.details);
        for name in names {
            if self.details.iter().any(|d| d.name == *name) {
                continue;
            }
            let entry = match old.iter().position(|d| d.name == *name) {
                Some(i) => old.swap_remove(i),
                None => DetailEntry::new(Arc::clone(name)),
            };
            self.details.push(entry);
        }
        old.iter().any(|d| d.built.is_some())
    }

    /// The demanded units to read with `GetAll`: those marked, or with `full` every one not
    /// read yet. A unit without a record uses the path from `LoadUnit`; without either it waits
    /// for one (see `details_to_resolve`).
    fn details_to_fetch(&self, full: bool) -> Vec<(Arc<str>, OwnedObjectPath, UnitType)> {
        self.details
            .iter()
            .filter(|d| d.refetch || (full && d.raw.is_none()))
            .filter_map(|d| {
                let name = Arc::clone(&d.name);
                match self.units.get(&d.name) {
                    Some(e) => Some((name, e.path.clone(), e.kind)),
                    None => Some((name, d.path.clone()?, unit_kind(&d.name)?)),
                }
            })
            .collect()
    }

    /// The demanded units that have neither a record nor a loaded path. They are loaded with
    /// `LoadUnit` to get one.
    fn details_to_resolve(&self) -> Vec<Arc<str>> {
        self.details
            .iter()
            .filter(|d| {
                d.path.is_none() && name_shown(&d.name) && !self.units.contains_key(&d.name)
            })
            .map(|d| Arc::clone(&d.name))
            .collect()
    }

    fn set_detail_path(&mut self, name: &str, path: OwnedObjectPath) {
        if let Some(d) = self.details.iter_mut().find(|d| *d.name == *name) {
            d.path = Some(path);
        }
    }

    fn set_detail_raw(&mut self, name: &str, raw: Option<DetailRaw>) -> bool {
        let manager = self.manager;
        let Some(d) = self.details.iter_mut().find(|d| *d.name == *name) else {
            return false;
        };
        let Some(raw) = raw else {
            d.refetch = true;
            return false;
        };
        d.refetch = false;
        if d.raw.as_ref() == Some(&raw) {
            return false;
        }
        if d.raw
            .as_ref()
            .is_none_or(|old| old.main_pid != raw.main_pid)
        {
            d.reread_host = true;
        }
        d.raw = Some(raw);
        d.rebuild(manager);
        true
    }

    /// `(name, MainPID)` of the read units whose host data is due: marked ones, or every one
    /// with `full`.
    fn details_host_due(&self, full: bool) -> Vec<(Arc<str>, u32)> {
        self.details
            .iter()
            .filter(|d| full || d.reread_host)
            .filter_map(|d| Some((Arc::clone(&d.name), d.raw.as_ref()?.main_pid)))
            .collect()
    }

    /// Applies host reads. True when published details changed.
    fn set_detail_host(
        &mut self,
        name: &str,
        ids: Option<PidIds>,
        dropin: Option<RecoveryKeySet>,
    ) -> bool {
        let manager = self.manager;
        let Some(d) = self.details.iter_mut().find(|d| *d.name == *name) else {
            return false;
        };
        d.reread_host = false;
        if d.ids == ids && d.dropin == dropin {
            return false;
        }
        d.ids = ids;
        d.dropin = dropin;
        d.rebuild(manager);
        d.built.is_some()
    }

    /// A `PropertiesChanged` on the `Unit` or `Service` interface of `name`. True when
    /// published details changed.
    fn apply_details(&mut self, name: &str, changed: &Props, invalidated: &[String]) -> bool {
        let manager = self.manager;
        let Some(d) = self.details.iter_mut().find(|d| *d.name == *name) else {
            return false;
        };
        if !invalidated.is_empty() {
            d.refetch = true;
        }
        let Some(raw) = &d.raw else {
            // The first read failed: this signal is the cue to try again.
            d.refetch = true;
            return false;
        };
        let mut next = raw.clone();
        next.apply(changed);
        if next == *raw {
            return false;
        }
        if next.main_pid != raw.main_pid {
            d.reread_host = true;
        }
        d.raw = Some(next);
        d.rebuild(manager);
        true
    }
}

/// Where host reads for details look.
#[derive(Clone, Copy)]
struct DetailsHost<'a> {
    sys_root: &'a SysRoot,
    /// The directory that holds `<unit>.d/` drop-ins for this manager.
    dropins: Option<&'a std::path::Path>,
}

/// `<etc>/systemd/system` for the system manager, the user config dir's `systemd/user` for
/// the user manager.
fn dropin_base(manager: Manager, sys_root: &SysRoot) -> Option<std::path::PathBuf> {
    match manager {
        Manager::System => {
            let base = sys_root.proc.parent().unwrap_or(std::path::Path::new("/"));
            Some(base.join("etc/systemd/system"))
        }
        Manager::User => user_dropin_base(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        ),
    }
}

/// `$XDG_CONFIG_HOME/systemd/user`, with `~/.config` when the variable is unset, empty or not
/// absolute (XDG Base Directory rules). `None` without a home.
fn user_dropin_base(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    let config = match xdg.map(std::path::PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        _ => std::path::PathBuf::from(home.filter(|h| !h.is_empty())?).join(".config"),
    };
    Some(config.join("systemd/user"))
}

/// Whether `<base>/<unit>.d/50-tasklens-recovery.conf` exists, checked with `stat`.
#[cfg(test)]
fn dropin_exists(base: Option<&std::path::Path>, unit: &str) -> bool {
    dropin_path(base, unit).is_some_and(|p| std::fs::metadata(p).is_ok())
}

/// `<base>/<unit>.d/50-tasklens-recovery.conf`, `None` without a base or for a unit name that
/// is not a plain file name.
fn dropin_path(base: Option<&std::path::Path>, unit: &str) -> Option<std::path::PathBuf> {
    let base = base?;
    if unit.is_empty() || unit.contains('/') || unit.starts_with('.') {
        return None;
    }
    Some(base.join(format!("{unit}.d")).join(RECOVERY_DROPIN))
}

/// The Recovery keys set under `[Unit]` and `[Service]` in drop-in text. Only `Key=value`
/// lines count; comments and unknown keys are ignored.
fn parse_dropin_keys(text: &str) -> RecoveryKeySet {
    let mut keys = RecoveryKeySet::default();
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            in_section = line == "[Unit]" || line == "[Service]";
        } else if in_section && let Some((key, _)) = line.split_once('=') {
            keys.set_key(key.trim_end());
        }
    }
    keys
}

/// The drop-in of `unit`: `None` when the file does not exist, else the Recovery keys in it.
/// A read error leaves the key set empty.
fn read_dropin(base: Option<&std::path::Path>, unit: &str) -> Option<RecoveryKeySet> {
    let path = dropin_path(base, unit)?;
    std::fs::metadata(&path).ok()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(parse_dropin_keys(&text)),
        Err(e) => {
            tracing::debug!(unit, error = %e, "systemd: recovery drop-in read failed");
            Some(RecoveryKeySet::default())
        }
    }
}

/// The real uid and gid in `<proc>/<pid>/status`, or `None` when the process is gone or
/// unreadable.
fn pid_ids(sys_root: &SysRoot, pid: u32, buf: &mut Vec<u8>, st: &mut PidStatus) -> Option<PidIds> {
    let path = sys_root.proc_path(&format!("{pid}/status"));
    read_file(&path, buf, STATUS_READ_MAX).ok()?;
    parse_status(buf, st).ok()?;
    Some(PidIds {
        uid: st.uid[0],
        gid: st.gid[0],
    })
}

/// The host reads of `units` (`(name, MainPID)`): MainPID's ids, `None` for pid 0, and whether
/// the recovery drop-in exists and which Recovery keys it sets.
fn read_details_host(
    sys_root: &SysRoot,
    dropins: Option<&std::path::Path>,
    units: &[(Arc<str>, u32)],
) -> Vec<(Option<PidIds>, Option<RecoveryKeySet>)> {
    let mut buf = Vec::new();
    let mut st = PidStatus::default();
    units
        .iter()
        .map(|(name, pid)| {
            let ids = (*pid != 0)
                .then(|| pid_ids(sys_root, *pid, &mut buf, &mut st))
                .flatten();
            (ids, read_dropin(dropins, name))
        })
        .collect()
}

/// Reads `Unit` and, for services, `Service` with `GetAll`. A unit error on `Service` leaves
/// those fields empty.
async fn fetch_details(
    cx: &Cx,
    conn: &DbusConn,
    path: &OwnedObjectPath,
    kind: UnitType,
) -> Result<DetailRaw, DbusError> {
    let unit = get_all(cx, conn, path, UNIT_IFACE).await?;
    let service = if kind == UnitType::Service {
        match get_all(cx, conn, path, "org.freedesktop.systemd1.Service").await {
            Ok(props) => props,
            Err(e) if is_unit_error(&e) => Props::new(),
            Err(e) => return Err(e),
        }
    } else {
        Props::new()
    };
    Ok(DetailRaw::from_props(&unit, &service))
}

/// Brings the details in line with `wanted`: reads new and marked units, then the host data of
/// those that need it. `full` retries every unit not read yet and checks every drop-in again.
/// A failed read of one unit is logged and leaves it out; only a closed bus is `Err`. True
/// when published details changed.
async fn sync_details<B: SystemdBus>(
    cx: &Cx,
    bus: &B,
    host: DetailsHost<'_>,
    state: &mut ServicesState,
    wanted: &[UnitRef],
    full: bool,
) -> Result<bool, DbusError> {
    let mut changed = state.want_details(wanted);
    for name in state.details_to_resolve() {
        match bus.load_unit(cx, &name).await {
            Ok(path) => state.set_detail_path(&name, path),
            Err(DbusError::Closed) => return Err(DbusError::Closed),
            Err(e) => tracing::debug!(unit = %name, error = %e, "systemd: LoadUnit failed"),
        }
    }
    for (name, path, kind) in state.details_to_fetch(full) {
        let raw = match bus.fetch_details(cx, &path, kind).await {
            Ok(raw) => Some(raw),
            Err(DbusError::Closed) => return Err(DbusError::Closed),
            Err(e) => {
                tracing::debug!(unit = %name, error = %e, "systemd: details GetAll failed");
                None
            }
        };
        changed |= state.set_detail_raw(&name, raw);
    }
    let due = state.details_host_due(full);
    if due.is_empty() {
        return Ok(changed);
    }
    let sys_root = host.sys_root.clone();
    let dropins = host.dropins.map(std::path::Path::to_path_buf);
    let units = due.clone();
    let reads = blocking(cx, move || {
        read_details_host(&sys_root, dropins.as_deref(), &units)
    })
    .await;
    if let Some(reads) = reads {
        for ((name, _), (ids, dropin)) in due.iter().zip(reads) {
            changed |= state.set_detail_host(name, ids, dropin);
        }
    }
    Ok(changed)
}

// Cgroups

/// The cgroup data of one unit. It outlives record rebuilds and starts over when the unit's
/// `ControlGroup` changes, so the next rate is `WarmingUp`.
#[derive(Clone, Debug)]
struct UnitCgroup {
    /// The `ControlGroup` these values belong to; empty for none.
    rel: Arc<str>,
    has_procs: bool,
    pids: Arc<[ProcessKey]>,
    cpu_pct: Total<f32>,
    memory_bytes: Total<u64>,
    disk_bps: Total<f64>,
    tasks: Total<u64>,
    /// `cpu.stat` `usage_usec`.
    cpu: Counter,
    /// `io.stat` `rbytes` + `wbytes`.
    io: Counter,
}

impl UnitCgroup {
    fn new(rel: Arc<str>) -> Self {
        Self {
            rel,
            has_procs: false,
            pids: Arc::from([]),
            cpu_pct: missing(),
            memory_bytes: missing(),
            disk_bps: missing(),
            tasks: missing(),
            cpu: Counter::default(),
            io: Counter::default(),
        }
    }

    /// Copies the values into `rec`. A unit without a cgroup keeps the `Missing` fields and no
    /// pids.
    fn fill(&self, rec: &mut UnitRecord) {
        if rec.control_group.is_none() {
            return;
        }
        rec.has_cgroup_procs = self.has_procs;
        rec.cgroup_pids = Arc::clone(&self.pids);
        rec.cpu_pct = self.cpu_pct;
        rec.memory_bytes = self.memory_bytes;
        rec.disk_bps = self.disk_bps;
        rec.tasks = self.tasks;
    }

    /// True when the record fields are equal; the counters are not compared.
    fn same_shown(&self, other: &Self) -> bool {
        self.has_procs == other.has_procs
            && self.pids == other.pids
            && self.cpu_pct == other.cpu_pct
            && self.memory_bytes == other.memory_bytes
            && self.disk_bps == other.disk_bps
            && self.tasks == other.tasks
    }

    /// One stat sample taken at `t_ns` (ADR-008 counter rules). A file that was not read gives
    /// `Restricted` when a file was denied, else `Unsupported`; a failed pass gives every field
    /// the error's validity.
    fn sample(&mut self, stats: Result<CgroupStats, Validity>, t_ns: u64, cpus: u32) {
        let s = match stats {
            Ok(s) => s,
            Err(v) => {
                self.cpu.reset();
                self.io.reset();
                self.cpu_pct = invalid(v);
                self.memory_bytes = invalid(v);
                self.disk_bps = invalid(v);
                self.tasks = invalid(v);
                return;
            }
        };
        let absent = if s.denied {
            Validity::Restricted
        } else {
            Validity::Unsupported
        };
        self.memory_bytes = s.anon.map_or_else(|| invalid(absent), valid);
        self.tasks = s.pids_current.map_or_else(|| invalid(absent), valid);
        self.cpu_pct = match s.usage_usec {
            Some(usec) => rate(&mut self.cpu, usec, t_ns, |delta, dt_ns| {
                cpu_percent(delta, dt_ns, cpus)
            }),
            None => {
                self.cpu.reset();
                invalid(absent)
            }
        };
        self.disk_bps = if s.io_valid {
            let bytes = s.rbytes.saturating_add(s.wbytes);
            rate(&mut self.io, bytes, t_ns, |delta, dt_ns| {
                delta as f64 * 1e9 / dt_ns as f64
            })
        } else {
            self.io.reset();
            invalid(absent)
        };
    }
}

fn valid<T>(value: T) -> Total<T> {
    Total {
        value,
        validity: Validity::Valid,
    }
}

fn invalid<T: Default>(validity: Validity) -> Total<T> {
    Total {
        value: T::default(),
        validity,
    }
}

/// One counter step: the rate from `per_sec`, or `WarmingUp` on the first sample, a reset or a
/// clock that did not move.
fn rate<T: Default>(
    counter: &mut Counter,
    cur: u64,
    t_ns: u64,
    per_sec: impl FnOnce(u64, u64) -> T,
) -> Total<T> {
    match counter.step(cur, t_ns, 0) {
        Delta::Ok { delta, dt_ns } => valid(per_sec(delta, dt_ns)),
        Delta::WarmingUp { .. } => invalid(Validity::WarmingUp),
    }
}

/// `usage_usec` delta as a share of all online CPUs, the scale of the process `cpu` column.
fn cpu_percent(delta_usec: u64, dt_ns: u64, cpus: u32) -> f32 {
    let busy_ns = delta_usec as f64 * 1000.0;
    (busy_ns / dt_ns as f64 / f64::from(cpus.max(1)) * 100.0) as f32
}

/// What one cgroup tick reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TickWant {
    /// `cgroup_pids`: `Demand.services` and `Demand.process_table`.
    pids: bool,
    /// The resource columns: `Demand.service_cgroup_stats`.
    stats: bool,
    /// The stats were not read on the tick before, so every rate starts over.
    fresh: bool,
}

impl TickWant {
    fn from_demand(d: &Demand) -> Self {
        Self {
            pids: d.services && d.process_table,
            stats: d.service_cgroup_stats,
            fresh: false,
        }
    }

    fn any(self) -> bool {
        self.pids || self.stats
    }
}

/// The 1 Hz cgroup tick. It runs only while `TickWant::any`; the first tick after the demand
/// comes on is due at once.
#[derive(Debug, Default)]
struct CgroupSchedule {
    last: Option<Time>,
    /// The last tick read the stats.
    stats_on: bool,
}

impl CgroupSchedule {
    /// How long the signal wait may last before the next tick: `SIGNAL_WAIT` while no tick is
    /// wanted.
    fn wait(&self, now: Time, d: &Demand) -> Duration {
        if !TickWant::from_demand(d).any() {
            return SIGNAL_WAIT;
        }
        let Some(last) = self.last else {
            return Duration::ZERO;
        };
        let since = Duration::from_nanos(now.duration_since(last));
        CGROUP_PERIOD.saturating_sub(since).min(SIGNAL_WAIT)
    }

    /// The tick to run now, if one is due. Marks it as run.
    fn due(&mut self, now: Time, d: &Demand) -> Option<TickWant> {
        let mut want = TickWant::from_demand(d);
        if !want.stats {
            self.stats_on = false;
        }
        if !want.any() {
            self.last = None;
            return None;
        }
        if let Some(last) = self.last
            && Duration::from_nanos(now.duration_since(last)) < CGROUP_PERIOD
        {
            return None;
        }
        self.last = Some(now);
        want.fresh = want.stats && !self.stats_on;
        self.stats_on = want.stats;
        Some(want)
    }
}

/// Where cgroup passes read: the host, a fixture directory, or a test fake.
trait CgroupFs: Clone + Send + 'static {
    type Pass: CgroupPass;
    /// Opens the handles of one pass. They close when the pass is dropped.
    fn open(&self) -> Self::Pass;
}

/// The reads of one pass. Cgroup paths are `ControlGroup` values, relative to
/// `/sys/fs/cgroup`.
trait CgroupPass {
    /// Appends the pids in `cgroup.procs` of cgroup `rel` to `out`, without its descendants.
    fn procs(&mut self, rel: &str, out: &mut Vec<u32>) -> Result<(), Validity>;
    /// Appends the names of the child cgroups of `rel` to `out`.
    fn children(&mut self, rel: &str, out: &mut Vec<Arc<str>>) -> Result<(), Validity>;
    /// `memory.stat`, `cpu.stat`, `io.stat` and `pids.current` of cgroup `rel`.
    fn stats(&mut self, rel: &str, out: &mut CgroupStats) -> Result<(), Validity>;
    /// `/proc/<pid>/stat` field 22. `None` when the process is gone or unreadable.
    fn start_ticks(&mut self, pid: u32) -> Option<u64>;
    fn boot_id(&mut self) -> Option<BootId>;
    fn online_cpus(&mut self) -> Option<u32>;
}

/// The stat files of a unit cgroup (R-137: `memory.stat` `anon`, not `memory.current`).
const UNIT_WANT: CgroupWant = CgroupWant {
    memory_stat: true,
    swap: false,
    memory_current: false,
    cpu_stat: true,
    io_stat: true,
    pids: true,
    events: false,
};

/// The host roots in `SysRoot`.
#[derive(Clone)]
struct HostFs {
    root: SysRoot,
}

impl CgroupFs for HostFs {
    type Pass = HostPass;

    fn open(&self) -> HostPass {
        HostPass {
            cg: CgroupRoot::open(&self.root.sys).map_err(|e| error_validity(&e)),
            proc: None,
            root: self.root.clone(),
            buf: Vec::new(),
        }
    }
}

/// A dirfd on `/sys/fs/cgroup` and, once a pid is read, one on `/proc`; every file is opened
/// relative to them and closed at once (R-137).
struct HostPass {
    cg: Result<CgroupRoot, Validity>,
    proc: Option<OwnedFd>,
    root: SysRoot,
    buf: Vec<u8>,
}

impl CgroupPass for HostPass {
    fn procs(&mut self, rel: &str, out: &mut Vec<u32>) -> Result<(), Validity> {
        let cg = self.cg.as_ref().map_err(|v| *v)?;
        cg.read_procs(rel, false, &mut self.buf, out)
            .map_err(|e| error_validity(&e))
    }

    fn children(&mut self, rel: &str, out: &mut Vec<Arc<str>>) -> Result<(), Validity> {
        let cg = self.cg.as_ref().map_err(|v| *v)?;
        cg.child_dirs(rel, out).map_err(|e| error_validity(&e))
    }

    fn stats(&mut self, rel: &str, out: &mut CgroupStats) -> Result<(), Validity> {
        let cg = self.cg.as_ref().map_err(|v| *v)?;
        cg.read_stats(rel, UNIT_WANT, &mut self.buf, out)
            .map_err(|e| error_validity(&e))
    }

    fn start_ticks(&mut self, pid: u32) -> Option<u64> {
        if self.proc.is_none() {
            self.proc = open_dir(&self.root.proc).ok();
        }
        let proc = self.proc.as_ref()?;
        let dir = rustix::fs::openat(
            proc,
            pid.to_string(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?;
        read_start_ticks(dir.as_fd(), &mut self.buf).ok()
    }

    fn boot_id(&mut self) -> Option<BootId> {
        let path = self.root.proc_path("sys/kernel/random/boot_id");
        BootId::from_uuid_text(&std::fs::read_to_string(path).ok()?)
    }

    fn online_cpus(&mut self) -> Option<u32> {
        let bytes = std::fs::read(self.root.sys_path("devices/system/cpu/online")).ok()?;
        let mut cpus: SmallVec<[u16; 64]> = SmallVec::new();
        parse_cpu_list(&bytes, &mut cpus).ok()?;
        (!cpus.is_empty()).then(|| u32::try_from(cpus.len()).unwrap_or(u32::MAX))
    }
}

fn open_dir(path: &std::path::Path) -> Result<OwnedFd, Errno> {
    rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
}

/// The validity of a failed cgroup read. The cgroup readers report an errno for the directory
/// and `cgroup.procs`, so a removed cgroup is `Missing` and a denied one `Restricted`.
fn error_validity(e: &CollectError) -> Validity {
    match e {
        CollectError::ProcRead { errno, .. } => {
            CollectError::from(Errno::from_raw_os_error(*errno)).validity()
        }
        other => other.validity(),
    }
}

/// The reads of one tick for one unit. `None` means not read, so the field keeps its value.
#[derive(Debug)]
struct UnitRead {
    name: Arc<str>,
    rel: Arc<str>,
    has_procs: Option<bool>,
    pids: Option<Arc<[ProcessKey]>>,
    stats: Option<Result<CgroupStats, Validity>>,
}

/// The reads of one tick.
#[derive(Debug, Default)]
struct TickRead {
    boot: Option<BootId>,
    cpus: Option<u32>,
    units: Vec<UnitRead>,
}

/// The parent of cgroup path `rel`. `None` for a top-level cgroup: the root is never a parent.
fn cgroup_parent(rel: &str) -> Option<&str> {
    match rel.rfind('/') {
        Some(0) | None => None,
        Some(i) => Some(&rel[..i]),
    }
}

/// One walk over the cgroup trees of `units` (`(name, ControlGroup)`): for each unit, in
/// `units` order, the pids of its cgroup and of every descendant. Each `cgroup.procs` is read
/// once, and its pids go to the unit that owns that cgroup and to every unit that owns an
/// ancestor. The root cgroup `/` (`-.slice`) is never read and gets no pid, so no walk covers
/// the whole system. A cgroup that cannot be read is skipped with its subtree.
fn walk_pids<P: CgroupPass>(pass: &mut P, units: &[(Arc<str>, Arc<str>)]) -> Vec<Vec<u32>> {
    let mut owners: HashMap<&str, SmallVec<[usize; 1]>> = HashMap::with_capacity(units.len());
    for (i, (_, rel)) in units.iter().enumerate() {
        let rel = rel.trim_end_matches('/');
        if !rel.is_empty() {
            owners.entry(rel).or_default().push(i);
        }
    }
    // The walk starts at each unit cgroup that has no unit cgroup above it.
    let mut stack: Vec<String> = owners
        .keys()
        .filter(|rel| {
            !std::iter::successors(cgroup_parent(rel), |p| cgroup_parent(p))
                .any(|p| owners.contains_key(p))
        })
        .map(|rel| (*rel).to_owned())
        .collect();
    let mut out = vec![Vec::new(); units.len()];
    let mut pids = Vec::new();
    let mut children = Vec::new();
    while let Some(rel) = stack.pop() {
        pids.clear();
        if pass.procs(&rel, &mut pids).is_err() {
            continue;
        }
        if !pids.is_empty() {
            for owner in std::iter::successors(Some(rel.as_str()), |p| cgroup_parent(p)) {
                for &i in owners.get(owner).into_iter().flatten() {
                    out[i].extend_from_slice(&pids);
                }
            }
        }
        children.clear();
        if pass.children(&rel, &mut children).is_ok() {
            stack.extend(children.drain(..).map(|name| format!("{rel}/{name}")));
        }
    }
    out
}

/// One tick's reads for `units` (`(name, ControlGroup)`). The pids walk runs when `want.pids`:
/// every pid of a unit's cgroup tree, flat, becomes a `ProcessKey`; a pid whose stat cannot be
/// read is skipped. Each pid's stat is read once, however many units hold it. Without a boot id
/// no keys are made and `cgroup_pids` keeps its value.
fn read_tick<P: CgroupPass>(
    pass: &mut P,
    units: &[(Arc<str>, Arc<str>)],
    want: TickWant,
    boot: Option<BootId>,
) -> TickRead {
    let mut read = TickRead::default();
    if want.pids && boot.is_none() {
        read.boot = pass.boot_id();
    }
    let boot = boot.or(read.boot);
    if want.stats {
        read.cpus = pass.online_cpus();
    }
    let walked = if want.pids {
        walk_pids(pass, units)
    } else {
        Vec::new()
    };
    let mut starts: HashMap<u32, Option<u64>> = HashMap::new();
    for (i, (name, rel)) in units.iter().enumerate() {
        let mut unit = UnitRead {
            name: Arc::clone(name),
            rel: Arc::clone(rel),
            has_procs: None,
            pids: None,
            stats: None,
        };
        if let Some(pids) = walked.get(i) {
            unit.has_procs = Some(!pids.is_empty());
            unit.pids = boot.map(|boot_id| {
                let mut keys: Vec<ProcessKey> = pids
                    .iter()
                    .filter_map(|&pid| {
                        let start = *starts.entry(pid).or_insert_with(|| pass.start_ticks(pid));
                        start.map(|start_ticks| ProcessKey {
                            start_ticks,
                            boot_id,
                            pid,
                        })
                    })
                    .collect();
                keys.sort_unstable();
                keys.dedup();
                Arc::from(keys)
            });
        }
        if want.stats {
            let mut stats = CgroupStats::default();
            unit.stats = Some(pass.stats(rel, &mut stats).map(|()| stats));
        }
        read.units.push(unit);
    }
    read
}

/// `has_cgroup_procs` for `units`: true when the cgroup tree holds any pid. One walk, as in
/// `walk_pids`.
fn read_presence<P: CgroupPass>(pass: &mut P, units: &[(Arc<str>, Arc<str>)]) -> TickRead {
    let walked = walk_pids(pass, units);
    let units = units
        .iter()
        .zip(walked)
        .map(|((name, rel), pids)| UnitRead {
            name: Arc::clone(name),
            rel: Arc::clone(rel),
            has_procs: Some(!pids.is_empty()),
            pids: None,
            stats: None,
        })
        .collect();
    TickRead {
        units,
        ..TickRead::default()
    }
}

/// Runs `f` as one blocking job. `None` when the job did not start or was cancelled; the
/// fields then keep their values.
async fn blocking<R: Send + 'static>(cx: &Cx, f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
    let mut job = match cx.spawn_blocking(move |_| f()) {
        Ok(job) => job,
        Err(e) => {
            tracing::debug!(error = %e, "systemd: cgroup job did not start");
            return None;
        }
    };
    match job.join(cx).await {
        Ok(out) => Some(out),
        Err(e) => {
            tracing::debug!(error = ?e, "systemd: cgroup job ended");
            None
        }
    }
}

/// The `has_cgroup_procs` walk after a listing. True when a record changed.
async fn cgroup_presence<F: CgroupFs>(cx: &Cx, fs: &F, state: &mut ServicesState) -> bool {
    let units = state.cgroup_units();
    if units.is_empty() {
        return false;
    }
    let fs = fs.clone();
    let t_ns = cx.now().as_nanos();
    match blocking(cx, move || read_presence(&mut fs.open(), &units)).await {
        Some(read) => state.apply_tick(read, t_ns, false),
        None => false,
    }
}

/// One cgroup tick: every read in one blocking job, then the records. True when a record
/// changed.
async fn cgroup_tick<F: CgroupFs>(
    cx: &Cx,
    fs: &F,
    state: &mut ServicesState,
    want: TickWant,
) -> bool {
    let units = state.cgroup_units();
    if units.is_empty() {
        return false;
    }
    let boot = state.boot;
    let fs = fs.clone();
    let t_ns = cx.now().as_nanos();
    match blocking(cx, move || read_tick(&mut fs.open(), &units, want, boot)).await {
        Some(read) => state.apply_tick(read, t_ns, want.fresh),
        None => false,
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
    fn alias_unit_is_detected_by_id() {
        let target = props(vec![("Id", Value::from("dbus-broker.service"))]);
        assert!(is_alias("dbus.service", &target));
        assert!(!is_alias("dbus-broker.service", &target));
        assert!(!is_alias("a.service", &Props::new()));
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
                let mut ticks = CgroupSchedule::default();
                let mut last_seq = 0;
                let id = SourceId::SystemdUser;
                loop {
                    let wake = wait_next(
                        &cx,
                        &io,
                        id,
                        &mut signals,
                        &mut schedule,
                        &mut ticks,
                        &mut last_seq,
                    )
                    .await;
                    match wake {
                        Wake::Stop => break,
                        Wake::Lost(e) => panic!("stream lost: {e}"),
                        Wake::Batch {
                            signals, resync, ..
                        } => {
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
                let mut ticks = CgroupSchedule::default();
                let mut last_seq = 0;
                let id = SourceId::SystemdSystem;
                let wake = wait_next(
                    &cx,
                    &io,
                    id,
                    &mut signals,
                    &mut schedule,
                    &mut ticks,
                    &mut last_seq,
                )
                .await;
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
                let mut ticks = CgroupSchedule::default();
                let mut last_seq = 0;
                let id = SourceId::SystemdUser;
                let outcome = match wait_next(
                    &cx,
                    &io,
                    id,
                    &mut signals,
                    &mut schedule,
                    &mut ticks,
                    &mut last_seq,
                )
                .await
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

    /// One queued answer of `SlowBus::list`.
    enum ListAnswer {
        Units(&'static [&'static str]),
        /// A busy systemd: no reply within `READ_TIMEOUT`.
        Slow,
        Closed,
    }

    /// A fake system manager for `serve`. Each `list` takes the next queued answer. Units have
    /// no unit file, no details and send no signals.
    struct SlowBus {
        answers: Arc<Mutex<std::collections::VecDeque<ListAnswer>>>,
        lists: Arc<Mutex<usize>>,
    }

    impl SystemdBus for SlowBus {
        type Signals<'a> = QuietSignals;

        async fn list(&self, cx: &Cx) -> Result<Listing, DbusError> {
            *self.lists.lock().expect("count lock") += 1;
            let answer = self.answers.lock().expect("queue lock").pop_front();
            match answer.expect("a queued answer") {
                ListAnswer::Units(names) => Ok(Listing {
                    units: names
                        .iter()
                        .map(|name| unit(name, "active", "running", "").0)
                        .collect(),
                    files: UnitFiles::new(),
                }),
                ListAnswer::Slow => {
                    time::sleep(cx.now(), crate::dbus::READ_TIMEOUT).await;
                    Err(DbusError::Timeout(crate::dbus::READ_TIMEOUT))
                }
                ListAnswer::Closed => Err(DbusError::Closed),
            }
        }

        async fn list_files(&self, _cx: &Cx) -> Result<UnitFiles, DbusError> {
            Ok(UnitFiles::new())
        }

        async fn fetch_unit(
            &self,
            _cx: &Cx,
            _name: &str,
            _path: &OwnedObjectPath,
        ) -> Result<Option<Fetched>, DbusError> {
            Ok(None)
        }

        async fn load_unit(&self, _cx: &Cx, name: &str) -> Result<OwnedObjectPath, DbusError> {
            Ok(unit_path(name))
        }

        async fn fetch_details(
            &self,
            _cx: &Cx,
            _path: &OwnedObjectPath,
            _kind: UnitType,
        ) -> Result<DetailRaw, DbusError> {
            Err(DbusError::Closed)
        }

        async fn subscribe(&self, _cx: &Cx) -> Result<QuietSignals, DbusError> {
            Ok(QuietSignals { end: false })
        }
    }

    /// The services updates in `rx` as (state, unit count).
    fn service_states(rx: &mut mpsc::Receiver<EngineMsg>) -> Vec<(ManagerState, usize)> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                EngineMsg::Source(SourceUpdate::Services(up)) => {
                    assert_eq!(up.manager, Manager::System);
                    out.push((up.state, up.units.len()));
                }
                other => panic!("unexpected message {other:?}"),
            }
        }
        out
    }

    /// Regression (y6k.33): a busy systemd let a resync call time out, and the worker took that
    /// for a lost bus. It published `Reconnecting` and the supervisor restarted it, 53 times in
    /// one session. A timeout now keeps the generation and the records, and lists again after
    /// `RESYNC_RETRY`. A closed bus still publishes `Reconnecting` and ends the generation.
    #[test]
    fn resync_timeout_keeps_the_worker() {
        assert_eq!(RESYNC_RETRY, Duration::from_secs(10));
        let mut lab = LabRuntime::new(LabConfig::new(45).max_steps(1_000_000));
        let (io, mut rx, control_tx, demand_tx) = worker_io();
        demand_tx
            .send(Demand {
                services: true,
                ..Demand::default()
            })
            .expect("demand send");
        let root = fixture_root("dbus_systemd_slow_bus");
        let cfg = EngineConfig::new(EngineMode::Full, root.join("state"))
            .with_sys_root(SysRoot::at(&root));
        let answers = Arc::new(Mutex::new(std::collections::VecDeque::from([
            ListAnswer::Units(&["a.service", "b.service"]),
            ListAnswer::Slow,
            ListAnswer::Units(&["a.service"]),
            ListAnswer::Closed,
        ])));
        let lists = Arc::new(Mutex::new(0usize));
        let bus = SlowBus {
            answers,
            lists: Arc::clone(&lists),
        };
        let out = Ended::default();
        let sink = Arc::clone(&out);
        let lab_root = lab.state.create_root_region(Budget::INFINITE);
        let (task, _handle) = lab
            .state
            .create_task(lab_root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let outcome = serve(&cx, &io, &cfg, Manager::System, &bus, 0).await;
                *sink.lock().expect("slot lock") = Some(outcome);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let lists_now = || *lists.lock().expect("count lock");
        assert_eq!(lists_now(), 1);
        assert_eq!(service_states(&mut rx), vec![(ManagerState::Connected, 2)]);

        // The resync at 60 s gets no reply; the read times out at 62 s.
        for _ in 0..124 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        assert_eq!(lists_now(), 2);
        for _ in 0..4 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        assert!(
            out.lock().expect("slot lock").is_none(),
            "the generation runs"
        );
        assert!(
            service_states(&mut rx).is_empty(),
            "no Reconnecting, the last records stay"
        );

        // The next listing comes 10 s after the timeout, not 60 s.
        for _ in 0..14 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        assert_eq!(lists_now(), 2, "not before 10 s");
        for _ in 0..4 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        assert_eq!(lists_now(), 3, "listed again at 10 s");
        assert_eq!(service_states(&mut rx), vec![(ManagerState::Connected, 1)]);
        assert!(
            out.lock().expect("slot lock").is_none(),
            "the generation runs"
        );

        // A closed bus is a lost bus.
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
        assert_eq!(lists_now(), 4);
        assert_eq!(
            service_states(&mut rx),
            vec![(ManagerState::Reconnecting, 1)]
        );
        let outcome = out.lock().expect("slot lock").take().expect("ended");
        assert!(
            matches!(outcome, Outcome::Err(WorkerError::Dbus(_))),
            "{outcome:?}"
        );
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    /// A stand-in for dbus-broker's per-user quota. It holds the bus side of other connections of
    /// the same user. Once more than `QUOTA` of them are open, it cuts every peer of the user,
    /// the systemd worker's connection among them, and the cut stays.
    #[derive(Default)]
    struct Broker {
        peers: Mutex<Vec<zbus::Connection>>,
        cut: std::sync::atomic::AtomicBool,
    }

    impl Broker {
        const QUOTA: usize = 2;

        fn open_peers(&self) -> usize {
            let peers = self.peers.lock().expect("peers lock");
            peers.iter().filter(|p| !p.is_closed()).count()
        }

        /// True once the broker has cut the user's peers.
        fn cut(&self) -> bool {
            use std::sync::atomic::Ordering;
            if self.open_peers() > Self::QUOTA {
                self.cut.store(true, Ordering::Release);
            }
            self.cut.load(Ordering::Acquire)
        }
    }

    /// The worker's signal stream on a bus behind `Broker`: quiet until the broker cuts it.
    struct BrokerSignals(Arc<Broker>);

    impl UnitSignals for BrokerSignals {
        async fn next(&mut self, cx: &Cx, d: Duration) -> Result<Option<Signal>, DbusError> {
            if self.0.cut() {
                return Err(DbusError::Closed);
            }
            QuietSignals { end: false }.next(cx, d).await
        }

        async fn ready(&mut self) -> Result<Option<Signal>, DbusError> {
            Ok(None)
        }
    }

    /// `SlowBus` behind a `Broker`.
    struct BrokerBus {
        inner: SlowBus,
        broker: Arc<Broker>,
    }

    impl SystemdBus for BrokerBus {
        type Signals<'a> = BrokerSignals;

        async fn list(&self, cx: &Cx) -> Result<Listing, DbusError> {
            self.inner.list(cx).await
        }

        async fn list_files(&self, cx: &Cx) -> Result<UnitFiles, DbusError> {
            self.inner.list_files(cx).await
        }

        async fn fetch_unit(
            &self,
            cx: &Cx,
            name: &str,
            path: &OwnedObjectPath,
        ) -> Result<Option<Fetched>, DbusError> {
            self.inner.fetch_unit(cx, name, path).await
        }

        async fn load_unit(&self, cx: &Cx, name: &str) -> Result<OwnedObjectPath, DbusError> {
            self.inner.load_unit(cx, name).await
        }

        async fn fetch_details(
            &self,
            cx: &Cx,
            path: &OwnedObjectPath,
            kind: UnitType,
        ) -> Result<DetailRaw, DbusError> {
            self.inner.fetch_details(cx, path, kind).await
        }

        async fn subscribe(&self, _cx: &Cx) -> Result<BrokerSignals, DbusError> {
            Ok(BrokerSignals(Arc::clone(&self.broker)))
        }
    }

    /// A peer-to-peer pair over a socketpair: (bus side, client side for `DbusConn::p2p`).
    fn broker_pair() -> (zbus::Connection, zbus::Connection) {
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let server = zbus::connection::Builder::async_io_unix_stream(a)
            .server(zbus::Guid::generate())
            .expect("server builder")
            .p2p()
            .build();
        let client = zbus::connection::Builder::async_io_unix_stream(b)
            .p2p()
            .internal_executor(false)
            .build();
        let (server, client) =
            futures_lite::future::block_on(futures_lite::future::zip(server, client));
        (
            server.expect("server connects"),
            client.expect("client connects"),
        )
    }

    /// Advances virtual time by `d` in 20 ms steps. The sockets and the socket close run on real
    /// threads, so each step also gives them 5 ms of real time.
    fn pump(lab: &mut LabRuntime, d: Duration) {
        let step = Duration::from_millis(20);
        for _ in 0..d.as_millis() / step.as_millis() {
            advance(lab, step);
            std::thread::sleep(Duration::from_millis(5));
            lab.run_until_idle();
        }
    }

    /// Regression (y6k.33): each Processes tab visit opened a system bus connection, dropped a
    /// match-rule stream and then the connection. The `RemoveMatch` task zbus queued kept the
    /// connection alive with its socket open. The open connections ran the user over its
    /// dbus-broker quota, the broker cut the user's peers, and the systemd worker lost its bus
    /// and restarted. Now each dropped connection closes its socket, the broker never cuts, and
    /// the worker stays in its first generation.
    #[test]
    fn leaked_connections_do_not_cut_the_worker() {
        const VISITS: usize = 4;
        let mut lab = LabRuntime::new(LabConfig::new(46).max_steps(10_000_000));
        let (io, mut rx, _control_tx, demand_tx) = worker_io();
        demand_tx
            .send(Demand {
                services: true,
                ..Demand::default()
            })
            .expect("demand send");
        let root = fixture_root("dbus_systemd_broker");
        let cfg = EngineConfig::new(EngineMode::Full, root.join("state"))
            .with_sys_root(SysRoot::at(&root));
        let broker = Arc::new(Broker::default());
        let mut clients = Vec::new();
        for _ in 0..VISITS {
            let (server, client) = broker_pair();
            broker.peers.lock().expect("peers lock").push(server);
            clients.push(client);
        }
        let bus = BrokerBus {
            inner: SlowBus {
                answers: Arc::new(Mutex::new(std::collections::VecDeque::from([
                    ListAnswer::Units(&["a.service"]),
                ]))),
                lists: Arc::new(Mutex::new(0usize)),
            },
            broker: Arc::clone(&broker),
        };
        let out = Ended::default();
        let sink = Arc::clone(&out);
        let lab_root = lab.state.create_root_region(Budget::INFINITE);
        let (worker, worker_handle) = lab
            .state
            .create_task(lab_root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let outcome = serve(&cx, &io, &cfg, Manager::System, &bus, 0).await;
                *sink.lock().expect("slot lock") = Some(outcome);
            })
            .expect("task is created");
        // Tab visits 2 s apart: open, drop a match-rule stream, drop the connection.
        let (visits, _visits_handle) = lab
            .state
            .create_task(lab_root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                for client in clients {
                    let conn = DbusConn::p2p(&cx, client).expect("p2p connection");
                    let rule = zbus::MatchRule::builder()
                        .msg_type(zbus::message::Type::Signal)
                        .build();
                    let stream = conn
                        .read(
                            &cx,
                            zbus::MessageStream::for_match_rule(rule, conn.raw(), Some(8)),
                        )
                        .await
                        .expect("signal stream");
                    drop(stream);
                    drop(conn);
                    time::sleep(cx.now(), Duration::from_secs(2)).await;
                }
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(worker, 0);
        lab.scheduler.lock().schedule(visits, 0);
        lab.run_until_idle();
        assert_eq!(service_states(&mut rx), vec![(ManagerState::Connected, 1)]);

        pump(&mut lab, Duration::from_secs(2 * VISITS as u64 + 2));
        assert_eq!(broker.open_peers(), 0, "every dropped connection closed");
        assert!(!broker.cut(), "the broker never cut the user's peers");
        assert!(
            out.lock().expect("slot lock").is_none(),
            "the first generation runs, no restart"
        );
        assert!(
            service_states(&mut rx).is_empty(),
            "no Reconnecting, the records stay"
        );

        worker_handle.abort();
        pump(&mut lab, SIGNAL_WAIT * 2);
        let outcome = out.lock().expect("slot lock").take().expect("ended");
        assert!(matches!(outcome, Outcome::Ok(())), "{outcome:?}");
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    // Cgroups

    const BOOT_TEXT: &str = "0123abcd-4567-89ab-cdef-0123456789ab\n";
    const FOO_CG: &str = "/system.slice/foo.service";

    /// An empty fixture root under the workspace target dir, never /tmp.
    fn fixture_root(name: &str) -> std::path::PathBuf {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(name);
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("fixture dir");
        base
    }

    fn put(base: &std::path::Path, rel: &str, text: &str) {
        let path = base.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture dir");
        std::fs::write(path, text).expect("fixture file");
    }

    /// A `/proc/<pid>/stat` line with `start` in field 22.
    fn stat_line(pid: u32, start: u64) -> String {
        let mut fields = vec![pid.to_string(), "(svc)".to_owned(), "S".to_owned()];
        for n in 4..=52 {
            fields.push(if n == 22 {
                start.to_string()
            } else {
                "0".to_owned()
            });
        }
        fields.join(" ") + "\n"
    }

    /// `foo.service` holds no pid itself, pid 100 in child `a` and pids 200 and 300 in `a/b`.
    /// Pid 300 has no stat file. `empty.service` holds none.
    fn cgroup_fixture(name: &str) -> std::path::PathBuf {
        let base = fixture_root(name);
        let cg = "sys/fs/cgroup/system.slice";
        put(&base, &format!("{cg}/foo.service/cgroup.procs"), "");
        put(&base, &format!("{cg}/foo.service/a/cgroup.procs"), "100\n");
        put(
            &base,
            &format!("{cg}/foo.service/a/b/cgroup.procs"),
            "200\n300\n",
        );
        put(
            &base,
            &format!("{cg}/foo.service/memory.stat"),
            "anon 4096\nfile 8192\n",
        );
        put(
            &base,
            &format!("{cg}/foo.service/cpu.stat"),
            "usage_usec 1000\n",
        );
        put(
            &base,
            &format!("{cg}/foo.service/io.stat"),
            "8:0 rbytes=10 wbytes=20 rios=1 wios=2\n",
        );
        put(&base, &format!("{cg}/foo.service/pids.current"), "3\n");
        put(&base, &format!("{cg}/empty.service/cgroup.procs"), "");
        put(&base, "proc/100/stat", &stat_line(100, 5000));
        put(&base, "proc/200/stat", &stat_line(200, 6000));
        put(&base, "proc/sys/kernel/random/boot_id", BOOT_TEXT);
        put(&base, "sys/devices/system/cpu/online", "0-7\n");
        base
    }

    fn boot() -> BootId {
        BootId::from_uuid_text(BOOT_TEXT).expect("boot id")
    }

    fn fixture_keys() -> Vec<ProcessKey> {
        vec![
            ProcessKey {
                start_ticks: 5000,
                boot_id: boot(),
                pid: 100,
            },
            ProcessKey {
                start_ticks: 6000,
                boot_id: boot(),
                pid: 200,
            },
        ]
    }

    fn cg_units(list: &[(&str, &str)]) -> Vec<(Arc<str>, Arc<str>)> {
        list.iter()
            .map(|(name, rel)| (Arc::from(*name), Arc::from(*rel)))
            .collect()
    }

    #[test]
    fn cgroup_walk_nested_pids() {
        let base = cgroup_fixture("dbus_systemd_cgroup_walk");
        let fs = HostFs {
            root: SysRoot::at(&base),
        };
        let walked = walk_pids(&mut fs.open(), &cg_units(&[("foo.service", FOO_CG)]));
        let mut pids = walked[0].clone();
        pids.sort_unstable();
        assert_eq!(pids, [100, 200, 300], "nested pids, flat");

        let units = cg_units(&[
            ("foo.service", FOO_CG),
            ("empty.service", "/system.slice/empty.service"),
            ("gone.service", "/system.slice/gone.service"),
        ]);
        let read = read_presence(&mut fs.open(), &units);
        let found: Vec<_> = read.units.iter().map(|u| u.has_procs).collect();
        assert_eq!(found, [Some(true), Some(false), Some(false)]);
        assert!(read.units.iter().all(|u| u.pids.is_none()));

        let want = TickWant {
            pids: true,
            stats: false,
            fresh: false,
        };
        let read = read_tick(&mut fs.open(), &units, want, None);
        assert_eq!(read.boot, Some(boot()));
        assert_eq!(read.cpus, None, "no stats wanted");
        let foo = &read.units[0];
        assert_eq!(foo.has_procs, Some(true));
        // Pid 300 has no stat file and is skipped.
        assert_eq!(foo.pids.as_deref(), Some(&fixture_keys()[..]));
        assert!(foo.stats.is_none());
        assert_eq!(read.units[2].pids.as_deref(), Some(&[][..]));
    }

    /// One pass that returns `stats` for every cgroup and has no pids.
    struct FakePass {
        stats: Result<CgroupStats, Validity>,
    }

    impl CgroupPass for FakePass {
        fn procs(&mut self, _rel: &str, _out: &mut Vec<u32>) -> Result<(), Validity> {
            Ok(())
        }

        fn children(&mut self, _rel: &str, _out: &mut Vec<Arc<str>>) -> Result<(), Validity> {
            Ok(())
        }

        fn stats(&mut self, _rel: &str, out: &mut CgroupStats) -> Result<(), Validity> {
            *out = self.stats?;
            Ok(())
        }

        fn start_ticks(&mut self, _pid: u32) -> Option<u64> {
            None
        }

        fn boot_id(&mut self) -> Option<BootId> {
            None
        }

        fn online_cpus(&mut self) -> Option<u32> {
            Some(8)
        }
    }

    fn usage(usec: u64, io: u64) -> Result<CgroupStats, Validity> {
        Ok(CgroupStats {
            anon: Some(1 << 20),
            usage_usec: Some(usec),
            rbytes: io,
            io_valid: true,
            pids_current: Some(4),
            ..CgroupStats::default()
        })
    }

    #[test]
    fn cgroup_stats_cpu_rate() {
        let mut cg = UnitCgroup::new(FOO_CG.into());
        cg.sample(usage(1_000_000, 0), 1_000_000_000, 8);
        assert_eq!(cg.cpu_pct.validity, Validity::WarmingUp, "first sample");
        assert_eq!(cg.disk_bps.validity, Validity::WarmingUp);
        assert_eq!(cg.memory_bytes, valid(1 << 20));
        assert_eq!(cg.tasks, valid(4));

        cg.sample(usage(1_500_000, 4096), 2_000_000_000, 8);
        assert_eq!(cg.cpu_pct, valid(6.25));
        assert_eq!(cg.disk_bps, valid(4096.0));

        // The rate uses the real elapsed time.
        cg.sample(usage(2_000_000, 4096), 4_000_000_000, 8);
        assert_eq!(cg.cpu_pct, valid(3.125));
        assert_eq!(cg.disk_bps, valid(0.0));
    }

    #[test]
    fn cgroup_stats_reset_warming_up() {
        let mut cg = UnitCgroup::new(FOO_CG.into());
        cg.sample(usage(1_000_000, 100), 1_000_000_000, 8);
        cg.sample(usage(1_500_000, 200), 2_000_000_000, 8);
        assert_eq!(cg.cpu_pct.validity, Validity::Valid);
        cg.sample(usage(400_000, 50), 3_000_000_000, 8);
        assert_eq!(
            cg.cpu_pct.validity,
            Validity::WarmingUp,
            "usage_usec went back"
        );
        assert_eq!(cg.cpu_pct.value, 0.0, "no rate");
        assert_eq!(cg.disk_bps.validity, Validity::WarmingUp);
        // The reset is the new baseline.
        cg.sample(usage(900_000, 50), 4_000_000_000, 8);
        assert_eq!(cg.cpu_pct, valid(6.25));

        // A demand that comes back on starts every rate over.
        let mut ticks = CgroupSchedule::default();
        let on = Demand {
            service_cgroup_stats: true,
            ..Demand::default()
        };
        let t = |s: u64| Time::from_nanos(s * 1_000_000_000);
        assert!(ticks.due(t(0), &on).expect("due at once").fresh);
        assert_eq!(ticks.due(t(0), &on), None, "not before 1 s");
        assert!(!ticks.due(t(1), &on).expect("due at 1 s").fresh);
        assert_eq!(ticks.due(t(2), &Demand::default()), None);
        assert!(ticks.due(t(2), &on).expect("due at once").fresh);
    }

    /// `foo.service` with a cgroup and `bar.service` without one.
    fn cgroup_state() -> ServicesState {
        let mut state = ServicesState::new(Manager::System);
        let (mut foo, user) = unit("foo.service", "active", "running", "enabled");
        foo.raw.control_group = FOO_CG.into();
        let bar = unit("bar.service", "active", "exited", "enabled");
        assert!(state.replace(vec![bar, (foo, user)], UnitFiles::new()));
        state
    }

    fn fake_tick(state: &mut ServicesState, stats: Result<CgroupStats, Validity>) -> bool {
        let want = TickWant {
            pids: false,
            stats: true,
            fresh: false,
        };
        let units = state.cgroup_units();
        let read = read_tick(&mut FakePass { stats }, &units, want, None);
        state.apply_tick(read, 1_000_000_000, false)
    }

    #[test]
    fn cgroup_stats_missing_without_cgroup() {
        let mut state = cgroup_state();
        assert_eq!(state.cgroup_units(), cg_units(&[("foo.service", FOO_CG)]));
        assert!(fake_tick(&mut state, usage(1, 1)));
        let foo = state.record("foo.service").expect("foo");
        assert_eq!(foo.memory_bytes, valid(1 << 20));
        assert_eq!(foo.tasks, valid(4));
        let bar = Arc::clone(state.record("bar.service").expect("bar"));
        assert_eq!(bar.control_group, None);
        assert_eq!(bar.cpu_pct.validity, Validity::Missing);
        assert_eq!(bar.memory_bytes.validity, Validity::Missing);
        assert_eq!(bar.disk_bps.validity, Validity::Missing);
        assert_eq!(bar.tasks.validity, Validity::Missing);
        assert!(!bar.has_cgroup_procs && bar.cgroup_pids.is_empty());

        // A record rebuilt from a signal keeps its cgroup values.
        let mut fx = Effects::default();
        let changed = props(vec![("SubState", Value::from("reload"))]);
        state.apply(&properties("foo.service", changed, &[]), &mut fx);
        let foo = state.record("foo.service").expect("foo");
        assert_eq!(&*foo.sub_state, "reload");
        assert_eq!(foo.memory_bytes, valid(1 << 20));
        // The same values at the same time change no record.
        let foo = Arc::clone(foo);
        assert!(!fake_tick(&mut state, usage(1, 1)));
        assert!(Arc::ptr_eq(&foo, state.record("foo.service").expect("foo")));
        assert!(Arc::ptr_eq(&bar, state.record("bar.service").expect("bar")));
    }

    /// The units of the fake bus and their `ControlGroup`, with the pids each cgroup holds itself.
    const TREE: [(&str, &str, &str); 7] = [
        ("-.slice", "/", ""),
        ("system.slice", "/system.slice", ""),
        ("a.service", "/system.slice/a.service", "10\n11\n"),
        ("user.slice", "/user.slice", ""),
        ("user-1000.slice", "/user.slice/user-1000.slice", ""),
        ("app.slice", "/user.slice/user-1000.slice/app.slice", ""),
        (
            "app-foo.scope",
            "/user.slice/user-1000.slice/app.slice/app-foo.scope",
            "20\n21\n22\n",
        ),
    ];

    /// The `TREE` cgroups. The root and `init.scope`, which no unit owns, hold pid 1.
    fn tree_fixture(name: &str) -> std::path::PathBuf {
        let base = fixture_root(name);
        put(&base, "sys/fs/cgroup/cgroup.procs", "1\n");
        put(&base, "sys/fs/cgroup/init.scope/cgroup.procs", "1\n");
        for (_, cg, pids) in TREE.iter().skip(1) {
            put(&base, &format!("sys/fs/cgroup{cg}/cgroup.procs"), pids);
        }
        for pid in [1, 10, 11, 20, 21, 22] {
            put(
                &base,
                &format!("proc/{pid}/stat"),
                &stat_line(pid, 1000 + u64::from(pid)),
            );
        }
        put(&base, "proc/sys/kernel/random/boot_id", BOOT_TEXT);
        base
    }

    /// The fake manager: `ListUnitsByPatterns` answers `TREE`, and there is no unit file.
    struct FakeListing;

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl FakeListing {
        fn list_units_by_patterns(
            &self,
            _states: Vec<String>,
            _patterns: Vec<String>,
        ) -> Vec<ListedUnit> {
            let none = OwnedObjectPath::try_from("/").expect("path");
            TREE.iter()
                .map(|(name, ..)| {
                    let (load, active, sub) = ("loaded", "active", "running");
                    (
                        (*name).to_owned(),
                        String::new(),
                        load.to_owned(),
                        active.to_owned(),
                        sub.to_owned(),
                        String::new(),
                        unit_path(name),
                        0,
                        String::new(),
                        none.clone(),
                    )
                })
                .collect()
        }

        fn list_unit_files_by_patterns(
            &self,
            _states: Vec<String>,
            _patterns: Vec<String>,
        ) -> Vec<(String, String)> {
            Vec::new()
        }
    }

    struct FakeUnitIface;

    #[zbus::interface(name = "org.freedesktop.systemd1.Unit")]
    impl FakeUnitIface {
        #[zbus(property)]
        fn load_state(&self) -> String {
            "loaded".to_owned()
        }

        #[zbus(property)]
        fn active_state(&self) -> String {
            "active".to_owned()
        }
    }

    /// The type interfaces: each carries only `ControlGroup`.
    macro_rules! fake_typed {
        ($ty:ident, $iface:literal) => {
            struct $ty(&'static str);

            #[zbus::interface(name = $iface)]
            impl $ty {
                #[zbus(property)]
                fn control_group(&self) -> String {
                    self.0.to_owned()
                }
            }
        };
    }
    fake_typed!(FakeSliceIface, "org.freedesktop.systemd1.Slice");
    fake_typed!(FakeScopeIface, "org.freedesktop.systemd1.Scope");
    fake_typed!(FakeServiceIface, "org.freedesktop.systemd1.Service");

    /// A peer-to-peer pair: the server side plays the system manager with the `TREE` units. The
    /// client has no internal executor, for `DbusConn::p2p`.
    fn fake_tree_bus() -> (zbus::Connection, zbus::Connection) {
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let mut server = zbus::connection::Builder::async_io_unix_stream(a)
            .server(zbus::Guid::generate())
            .expect("server builder")
            .p2p()
            .serve_at(PATH, FakeListing)
            .expect("manager");
        for (name, cg, _) in TREE {
            let path = unit_path(name);
            server = server.serve_at(path.clone(), FakeUnitIface).expect("unit");
            server = match unit_kind(name) {
                Some(UnitType::Slice) => server.serve_at(path, FakeSliceIface(cg)),
                Some(UnitType::Scope) => server.serve_at(path, FakeScopeIface(cg)),
                _ => server.serve_at(path, FakeServiceIface(cg)),
            }
            .expect("type interface");
        }
        let client = zbus::connection::Builder::async_io_unix_stream(b)
            .p2p()
            .internal_executor(false)
            .build();
        let (server, client) =
            futures_lite::future::block_on(futures_lite::future::zip(server.build(), client));
        (
            server.expect("server connects"),
            client.expect("client connects"),
        )
    }

    #[test]
    fn slice_and_scope_cgroup_pids() {
        assert_eq!(
            type_iface(UnitType::Scope),
            Some("org.freedesktop.systemd1.Scope")
        );
        assert_eq!(
            type_iface(UnitType::Slice),
            Some("org.freedesktop.systemd1.Slice")
        );
        let base = tree_fixture("dbus_systemd_tree_pids");
        let counts = Arc::new(Counts::default());
        let fs = Counting {
            inner: HostFs {
                root: SysRoot::at(&base),
            },
            counts: Arc::clone(&counts),
        };
        let (server, client) = fake_tree_bus();
        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let shared = Arc::clone(&counts);
        let join = rt.handle().spawn_checked(async move {
            let cx = Cx::current().expect("task has a Cx");
            let conn = DbusConn::p2p(&cx, client).expect("p2p connection");
            let sys_root = SysRoot::at(&base);
            let mut state = ServicesState::new(Manager::System);
            resync(&cx, &conn, Manager::System, &sys_root, &fs, &mut state)
                .await
                .expect("listing over the fake bus");
            let after_listing = std::mem::take(&mut *shared.procs_read.lock().expect("lock"));
            let want = TickWant {
                pids: true,
                stats: false,
                fresh: false,
            };
            assert!(cgroup_tick(&cx, &fs, &mut state, want).await);
            drop(conn);
            drop(server);
            let rows: Vec<(String, bool, Vec<u32>)> = TREE
                .iter()
                .map(|(name, ..)| {
                    let rec = state.record(name).expect("listed");
                    let pids = rec.cgroup_pids.iter().map(|k| k.pid).collect();
                    ((*name).to_owned(), rec.has_cgroup_procs, pids)
                })
                .collect();
            (after_listing, rows)
        });
        let (after_listing, rows) = rt.block_on(join).expect("task completes");

        let expected = [
            ("-.slice", vec![]),
            ("system.slice", vec![10, 11]),
            ("a.service", vec![10, 11]),
            ("user.slice", vec![20, 21, 22]),
            ("user-1000.slice", vec![20, 21, 22]),
            ("app.slice", vec![20, 21, 22]),
            ("app-foo.scope", vec![20, 21, 22]),
        ];
        for ((name, has_procs, pids), (want_name, want_pids)) in rows.iter().zip(expected) {
            assert_eq!(name, want_name);
            assert_eq!(pids, &want_pids, "{name}");
            assert_eq!(*has_procs, !want_pids.is_empty(), "{name}");
        }

        // Each walk reads every unit cgroup once, and never the root or a cgroup no unit owns.
        let mut once: Vec<String> = TREE
            .iter()
            .skip(1)
            .map(|(_, cg, _)| (*cg).to_owned())
            .collect();
        once.sort();
        let tick = counts.procs_read.lock().expect("lock").clone();
        for (walk, mut read) in [("listing", after_listing), ("tick", tick)] {
            read.sort();
            assert_eq!(read, once, "{walk}");
        }
        // Each pid's stat is read once, though up to four units hold it.
        assert_eq!(counts.get().2, 5);
    }

    #[test]
    fn cgroup_stats_restricted_on_eacces() {
        let mut state = cgroup_state();
        // memory.stat gave EACCES; the other files were read.
        let denied = Ok(CgroupStats {
            anon: None,
            usage_usec: Some(10),
            io_valid: true,
            pids_current: Some(2),
            denied: true,
            ..CgroupStats::default()
        });
        assert!(fake_tick(&mut state, denied));
        let foo = state.record("foo.service").expect("foo");
        assert_eq!(foo.memory_bytes.validity, Validity::Restricted);
        assert_eq!(foo.tasks, valid(2));
        assert_eq!(foo.cpu_pct.validity, Validity::WarmingUp);

        // A missing file that was not denied is Unsupported, never 0.
        let mut cg = UnitCgroup::new(FOO_CG.into());
        cg.sample(
            Ok(CgroupStats {
                pids_current: Some(1),
                ..CgroupStats::default()
            }),
            1,
            8,
        );
        assert_eq!(cg.memory_bytes.validity, Validity::Unsupported);
        assert_eq!(cg.disk_bps.validity, Validity::Unsupported);

        // The whole cgroup denied, or gone.
        assert!(fake_tick(&mut state, Err(Validity::Restricted)));
        let foo = state.record("foo.service").expect("foo");
        assert_eq!(foo.memory_bytes.validity, Validity::Restricted);
        assert_eq!(foo.cpu_pct.validity, Validity::Restricted);
        let eacces = CollectError::ProcRead {
            file: "cgroup",
            errno: Errno::ACCESS.raw_os_error(),
        };
        assert_eq!(error_validity(&eacces), Validity::Restricted);
        let enoent = CollectError::ProcRead {
            file: "cgroup",
            errno: Errno::NOENT.raw_os_error(),
        };
        assert_eq!(error_validity(&enoent), Validity::Missing);
    }

    #[derive(Default)]
    struct Counts {
        procs: std::sync::atomic::AtomicUsize,
        stats: std::sync::atomic::AtomicUsize,
        pid_stat: std::sync::atomic::AtomicUsize,
        /// The cgroup of each `cgroup.procs` read, in order.
        procs_read: Mutex<Vec<String>>,
    }

    impl Counts {
        fn get(&self) -> (usize, usize, usize) {
            use std::sync::atomic::Ordering::SeqCst;
            (
                self.procs.load(SeqCst),
                self.stats.load(SeqCst),
                self.pid_stat.load(SeqCst),
            )
        }
    }

    /// The fixture filesystem, counting every read.
    #[derive(Clone)]
    struct Counting {
        inner: HostFs,
        counts: Arc<Counts>,
    }

    struct CountingPass {
        inner: HostPass,
        counts: Arc<Counts>,
    }

    impl CgroupFs for Counting {
        type Pass = CountingPass;

        fn open(&self) -> CountingPass {
            CountingPass {
                inner: self.inner.open(),
                counts: Arc::clone(&self.counts),
            }
        }
    }

    impl CgroupPass for CountingPass {
        fn procs(&mut self, rel: &str, out: &mut Vec<u32>) -> Result<(), Validity> {
            self.counts
                .procs
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.counts
                .procs_read
                .lock()
                .expect("counts lock")
                .push(rel.to_owned());
            self.inner.procs(rel, out)
        }

        fn children(&mut self, rel: &str, out: &mut Vec<Arc<str>>) -> Result<(), Validity> {
            self.inner.children(rel, out)
        }

        fn stats(&mut self, rel: &str, out: &mut CgroupStats) -> Result<(), Validity> {
            self.counts
                .stats
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.stats(rel, out)
        }

        fn start_ticks(&mut self, pid: u32) -> Option<u64> {
            self.counts
                .pid_stat
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.start_ticks(pid)
        }

        fn boot_id(&mut self) -> Option<BootId> {
            self.inner.boot_id()
        }

        fn online_cpus(&mut self) -> Option<u32> {
            self.inner.online_cpus()
        }
    }

    type Seen = Arc<Mutex<Option<Arc<UnitRecord>>>>;

    /// Runs the signal wait and the cgroup tick as `connected` does, without a bus. After each
    /// wake the `foo.service` record goes to `seen`.
    fn tick_loop(lab: &mut LabRuntime, io: WorkerIo, fs: Counting, seen: Seen) -> TaskHandle<()> {
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let mut state = cgroup_state();
                let mut signals = QuietSignals { end: false };
                let mut schedule = Schedule::new(cx.now());
                let mut ticks = CgroupSchedule::default();
                let mut last_seq = 0;
                let id = SourceId::SystemdSystem;
                loop {
                    let wake = wait_next(
                        &cx,
                        &io,
                        id,
                        &mut signals,
                        &mut schedule,
                        &mut ticks,
                        &mut last_seq,
                    )
                    .await;
                    match wake {
                        Wake::Stop => break,
                        Wake::Lost(e) => panic!("stream lost: {e}"),
                        Wake::Batch { tick, .. } => {
                            if let Some(want) = tick {
                                cgroup_tick(&cx, &fs, &mut state, want).await;
                            }
                            *seen.lock().expect("seen lock") = state.record("foo.service").cloned();
                        }
                    }
                }
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        handle
    }

    fn counting_fixture(name: &str) -> (Counting, Arc<Counts>) {
        let base = cgroup_fixture(name);
        let counts = Arc::new(Counts::default());
        let fs = Counting {
            inner: HostFs {
                root: SysRoot::at(&base),
            },
            counts: Arc::clone(&counts),
        };
        (fs, counts)
    }

    fn seen_record(seen: &Seen) -> Arc<UnitRecord> {
        seen.lock()
            .expect("seen lock")
            .clone()
            .expect("the loop woke")
    }

    #[test]
    fn cgroup_pids_follow_process_table_demand() {
        let mut lab = LabRuntime::new(LabConfig::new(45).max_steps(1_000_000));
        let (io, _rx, _control_tx, demand_tx) = worker_io();
        let (fs, counts) = counting_fixture("dbus_systemd_pids_demand");
        let seen = Seen::default();
        let handle = tick_loop(&mut lab, io, fs, Arc::clone(&seen));

        // Services shown, no row expanded, a resource column shown: the stats tick runs, but no
        // pid is read.
        let stats_only = Demand {
            services: true,
            process_table: false,
            service_cgroup_stats: true,
            ..Demand::default()
        };
        demand_tx.send(stats_only.clone()).expect("demand send");
        for _ in 0..20 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        let (procs, stats, pid_stat) = counts.get();
        assert_eq!((procs, pid_stat), (0, 0));
        assert!((10..=11).contains(&stats), "1 Hz, got {stats}");
        let foo = seen_record(&seen);
        assert!(foo.cgroup_pids.is_empty());
        assert_eq!(foo.memory_bytes, valid(4096));

        // A row expanded: one tick lists both pids with their start times.
        let both = Demand {
            process_table: true,
            ..stats_only.clone()
        };
        demand_tx.send(both).expect("demand send");
        advance(&mut lab, SIGNAL_WAIT);
        advance(&mut lab, SIGNAL_WAIT);
        let foo = seen_record(&seen);
        assert!(foo.has_cgroup_procs);
        assert_eq!(&foo.cgroup_pids[..], &fixture_keys()[..]);
        let (_, _, pid_stat) = counts.get();
        assert!(pid_stat >= 3, "pids 100, 200 and 300");

        // The row collapsed: 10 s without a pid read, and the pids stay as they were.
        demand_tx.send(stats_only).expect("demand send");
        advance(&mut lab, SIGNAL_WAIT);
        let before = counts.get().2;
        let pids = Arc::clone(&seen_record(&seen).cgroup_pids);
        for _ in 0..20 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        assert_eq!(counts.get().2, before);
        assert!(Arc::ptr_eq(&pids, &seen_record(&seen).cgroup_pids));

        handle.abort();
        advance(&mut lab, SIGNAL_WAIT);
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn cgroup_stats_off_without_demand() {
        let mut lab = LabRuntime::new(LabConfig::new(46).max_steps(1_000_000));
        let (io, _rx, _control_tx, demand_tx) = worker_io();
        let (fs, counts) = counting_fixture("dbus_systemd_stats_off");
        let seen = Seen::default();
        let handle = tick_loop(&mut lab, io, fs, Arc::clone(&seen));

        demand_tx
            .send(Demand {
                services: true,
                ..Demand::default()
            })
            .expect("demand send");
        for _ in 0..120 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        assert_eq!(counts.get(), (0, 0, 0), "no cgroup read in 60 s");

        // Pids wanted, stats not: the tick lists pids and reads no stat file.
        demand_tx
            .send(Demand {
                services: true,
                process_table: true,
                ..Demand::default()
            })
            .expect("demand send");
        for _ in 0..4 {
            advance(&mut lab, SIGNAL_WAIT);
        }
        let (procs, stats, _) = counts.get();
        assert!(procs > 0);
        assert_eq!(stats, 0);
        assert_eq!(seen_record(&seen).cpu_pct.validity, Validity::Missing);

        handle.abort();
        advance(&mut lab, SIGNAL_WAIT);
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    // Details

    fn unit_ref(manager: Manager, name: &str) -> UnitRef {
        UnitRef {
            manager,
            name: Arc::from(name),
        }
    }

    fn published(state: &ServicesState) -> Vec<(UnitRef, Arc<UnitDetails>)> {
        match state.update(ManagerState::Connected) {
            SourceUpdate::Services(u) => u.details,
            other => panic!("not a services update: {other:?}"),
        }
    }

    fn detail_raw(entries: Vec<(&str, Value<'static>)>) -> DetailRaw {
        DetailRaw::from_props(&props(entries), &Props::new())
    }

    /// Reads every unit `details_to_fetch` names, as `sync_details` does, from `raw`.
    fn fetch_all(state: &mut ServicesState, raw: &DetailRaw) -> Vec<Arc<str>> {
        let fetch = state.details_to_fetch(false);
        for (name, _, _) in &fetch {
            state.set_detail_raw(name, Some(raw.clone()));
        }
        fetch.into_iter().map(|(name, _, _)| name).collect()
    }

    #[test]
    fn details_only_for_own_manager() {
        let wanted = [unit_ref(Manager::System, "sshd.service")];
        let raw = detail_raw(vec![("Id", Value::from("sshd.service"))]);

        let mut system = ServicesState::new(Manager::System);
        system.replace(
            vec![unit("sshd.service", "active", "running", "enabled")],
            UnitFiles::new(),
        );
        system.want_details(&wanted);
        assert_eq!(
            fetch_all(&mut system, &raw),
            vec![Arc::from("sshd.service")]
        );
        let details = published(&system);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].0, wanted[0]);
        assert_eq!(&*details[0].1.id, "sshd.service");
        assert_eq!(details[0].1.manager, Manager::System);

        let mut user = sample_state();
        assert!(!user.want_details(&wanted));
        assert!(fetch_all(&mut user, &raw).is_empty());
        assert!(published(&user).is_empty());
    }

    #[test]
    fn details_for_unit_without_record_load_a_path_first() {
        // A dependency can be a unit the table does not list: not loaded, or hidden.
        let mut state = sample_state();
        let wanted = [unit_ref(Manager::User, "dep.target")];
        state.want_details(&wanted);
        assert!(state.details_to_fetch(true).is_empty(), "no path yet");
        assert_eq!(state.details_to_resolve(), vec![Arc::from("dep.target")]);

        state.set_detail_path("dep.target", unit_path("dep.target"));
        assert!(state.details_to_resolve().is_empty());
        let raw = detail_raw(vec![("Id", Value::from("dep.target"))]);
        assert_eq!(fetch_all(&mut state, &raw), vec![Arc::from("dep.target")]);
        assert_eq!(published(&state).len(), 1);
    }

    #[test]
    fn details_dropped_when_undemanded() {
        let mut state = sample_state();
        let both = [
            unit_ref(Manager::User, "foo.service"),
            unit_ref(Manager::User, "bar.service"),
        ];
        state.want_details(&both);
        fetch_all(&mut state, &DetailRaw::default());
        assert_eq!(published(&state).len(), 2);
        let foo = Arc::clone(&published(&state)[0].1);

        assert!(state.want_details(&both[..1]));
        let details = published(&state);
        assert_eq!(details.len(), 1);
        assert_eq!(&*details[0].0.name, "foo.service");
        assert!(
            Arc::ptr_eq(&foo, &details[0].1),
            "a kept unit keeps its Arc"
        );
        assert!(state.details_to_fetch(true).is_empty(), "no new read");

        assert!(state.want_details(&[]));
        assert!(published(&state).is_empty());
    }

    #[test]
    fn details_u64_max_unsupported() {
        let raw = detail_raw(vec![
            ("MemoryCurrent", Value::U64(u64::MAX)),
            ("TasksCurrent", Value::U64(5)),
        ]);
        let d = details_of(Manager::System, &raw, None, None);
        assert_eq!(d.memory_current, None, "u64::MAX is not 0");
        assert_eq!(d.tasks_current, Some(5));
        assert_eq!(d.cpu_usage_nsec, None, "absent");

        // A timer has no `Service` properties: they stay empty.
        let timer = detail_raw(vec![("Id", Value::from("fstrim.timer"))]);
        let d = details_of(Manager::System, &timer, None, None);
        assert!(d.exec_start_argv.is_empty());
        assert_eq!(d.recovery.restart, None);
        assert_eq!(d.recovery.n_restarts, None);
        assert_eq!(d.user, None);
        assert_eq!(d.main_pid_ids, None);

        // ExecStart gives the argv of its first row.
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
        let raw = DetailRaw::from_props(
            &Props::new(),
            &props(vec![
                ("ExecStart", exec),
                ("Restart", Value::from("on-failure")),
            ]),
        );
        let d = details_of(Manager::System, &raw, None, None);
        let argv: Vec<&str> = d.exec_start_argv.iter().map(|a| &**a).collect();
        assert_eq!(argv, ["foo", "--x"]);
        assert_eq!(d.recovery.restart.as_deref(), Some("on-failure"));
    }

    #[test]
    fn details_graphical_session() {
        let part_of = detail_raw(vec![
            ("PartOf", Value::from(vec!["graphical-session.target"])),
            ("After", Value::from(vec!["b.target", "a.target"])),
            ("Wants", Value::from(vec!["b.target"])),
        ]);
        let d = details_of(Manager::User, &part_of, None, None);
        assert!(d.graphical_session);
        let deps: Vec<(&str, Relation)> =
            d.deps.iter().map(|e| (&*e.unit.name, e.relation)).collect();
        assert_eq!(
            deps,
            [
                ("a.target", Relation::After),
                ("b.target", Relation::Wants),
                ("b.target", Relation::After),
                ("graphical-session.target", Relation::PartOf),
            ]
        );
        assert!(d.deps.iter().all(|e| e.unit.manager == Manager::User));

        let binds = detail_raw(vec![(
            "BindsTo",
            Value::from(vec!["graphical-session.target"]),
        )]);
        assert!(details_of(Manager::User, &binds, None, None).graphical_session);
        let wants = detail_raw(vec![(
            "Wants",
            Value::from(vec!["graphical-session.target"]),
        )]);
        assert!(!details_of(Manager::User, &wants, None, None).graphical_session);
    }

    #[test]
    fn details_dropin_exists_paths() {
        let tmp_base =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&tmp_base).expect("tmp base");
        let tmp = tempfile::TempDir::new_in(&tmp_base).expect("temp dir");
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        let home = tmp.path().join("home");
        let home_os = Some(home.clone().into_os_string());
        let config = tmp.path().join("xdg");
        let config_os = Some(config.clone().into_os_string());

        assert_eq!(
            user_dropin_base(config_os.clone(), home_os.clone()),
            Some(config.join("systemd/user"))
        );
        assert_eq!(
            user_dropin_base(None, home_os.clone()),
            Some(home.join(".config/systemd/user"))
        );
        assert_eq!(
            user_dropin_base(os("relative"), home_os.clone()),
            Some(home.join(".config/systemd/user")),
            "a relative XDG_CONFIG_HOME is ignored"
        );
        assert_eq!(user_dropin_base(None, None), None);
        let root = SysRoot::at(tmp.path());
        assert_eq!(
            dropin_base(Manager::System, &root),
            Some(tmp.path().join("etc/systemd/system"))
        );

        let base = user_dropin_base(config_os, home_os).expect("base");
        let dir = base.join("foo.service.d");
        std::fs::create_dir_all(&dir).expect("drop-in dir");
        std::fs::write(dir.join(RECOVERY_DROPIN), "[Service]\nRestart=always\n").expect("file");
        assert!(dropin_exists(Some(&base), "foo.service"));
        assert!(!dropin_exists(Some(&base), "bar.service"));
        assert!(!dropin_exists(None, "foo.service"));
        assert!(!dropin_exists(Some(&base), "../foo.service"));

        // The host read gives the drop-in and MainPID's real uid and gid.
        let pid_dir = tmp.path().join("proc/42");
        std::fs::create_dir_all(&pid_dir).expect("pid dir");
        std::fs::write(
            pid_dir.join("status"),
            "Name:\tx\nUid:\t977\t0\t0\t0\nGid:\t978\t0\t0\t0\n",
        )
        .expect("status");
        let units = [
            (Arc::from("foo.service"), 42),
            (Arc::from("bar.service"), 0),
            (Arc::from("baz.service"), 43),
        ];
        let reads = read_details_host(&root, Some(&base), &units);
        assert_eq!(
            reads,
            [
                (
                    Some(PidIds { uid: 977, gid: 978 }),
                    Some(RecoveryKeySet {
                        restart: true,
                        ..Default::default()
                    })
                ),
                (None, None),
                (None, None),
            ]
        );

        // Through the state: the drop-in shows in the published details.
        let mut state = sample_state();
        state.want_details(&[unit_ref(Manager::User, "foo.service")]);
        let raw = detail_raw(vec![("MainPID", Value::U32(42))]);
        fetch_all(&mut state, &raw);
        let due = state.details_host_due(false);
        assert_eq!(due, [(Arc::from("foo.service"), 42)]);
        let (ids, dropin) = read_details_host(&root, Some(&base), &due)[0];
        assert!(state.set_detail_host("foo.service", ids, dropin));
        let d = &published(&state)[0].1;
        assert!(d.recovery_dropin_exists);
        assert!(d.recovery_dropin_keys.restart);
        assert_eq!(d.main_pid_ids, Some(PidIds { uid: 977, gid: 978 }));
        assert!(state.details_host_due(false).is_empty());
    }

    #[test]
    fn dropin_keys_parse_and_read() {
        let text = "# c\n[Unit]\nOnFailure=a.service\nBogus=1\n[Service]\nRestart=always\n; Restart\n#FailureAction=exit\nRestartSec\n[Install]\nStartLimitBurst=3\n";
        assert_eq!(
            parse_dropin_keys(text),
            RecoveryKeySet {
                restart: true,
                on_failure: true,
                ..Default::default()
            }
        );
        assert!(parse_dropin_keys("").is_empty());

        let tmp_base =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&tmp_base).expect("tmp base");
        let tmp = tempfile::TempDir::new_in(&tmp_base).expect("temp dir");
        let dir = tmp.path().join("foo.service.d");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join(RECOVERY_DROPIN),
            "[Service]\nRestart=on-failure\n[Unit]\nOnFailure=x.service\n",
        )
        .expect("file");
        let keys = read_dropin(Some(tmp.path()), "foo.service").expect("exists");
        assert_eq!(
            keys,
            RecoveryKeySet {
                restart: true,
                on_failure: true,
                ..Default::default()
            }
        );
        assert_eq!(read_dropin(Some(tmp.path()), "bar.service"), None);
    }

    #[test]
    fn details_cap_10() {
        let mut state = ServicesState::new(Manager::System);
        let names: Vec<String> = (0..12).map(|i| format!("u{i:02}.service")).collect();
        state.replace(
            names
                .iter()
                .map(|n| unit(n, "active", "running", "enabled"))
                .collect(),
            UnitFiles::new(),
        );
        let wanted: Vec<UnitRef> = names.iter().map(|n| unit_ref(Manager::System, n)).collect();
        state.want_details(&wanted);
        let fetched = fetch_all(&mut state, &DetailRaw::default());
        assert_eq!(fetched.len(), UNIT_DETAILS_MAX);
        let details = published(&state);
        let shown: Vec<&str> = details.iter().map(|(u, _)| &*u.name).collect();
        let first: Vec<&str> = names[..UNIT_DETAILS_MAX]
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(shown, first);
    }

    #[test]
    fn details_properties_changed_keeps_other_arcs() {
        let mut state = sample_state();
        state.want_details(&[
            unit_ref(Manager::User, "foo.service"),
            unit_ref(Manager::User, "bar.service"),
        ]);
        fetch_all(&mut state, &detail_raw(vec![("NRestarts", Value::U32(0))]));
        let before = published(&state);

        let mut fx = Effects::default();
        let signal = Signal::Properties {
            path: unit_path("foo.service"),
            iface: "org.freedesktop.systemd1.Service".to_owned(),
            changed: props(vec![("NRestarts", Value::U32(3))]),
            invalidated: Vec::new(),
        };
        state.apply(&signal, &mut fx);
        assert!(fx.changed);
        let after = published(&state);
        assert_eq!(after[0].1.recovery.n_restarts, Some(3));
        assert!(!Arc::ptr_eq(&before[0].1, &after[0].1), "foo is replaced");
        assert!(Arc::ptr_eq(&before[1].1, &after[1].1), "bar keeps its Arc");
        assert!(state.details_to_fetch(false).is_empty(), "no GetAll needed");

        // The same value again changes nothing.
        let mut fx = Effects::default();
        state.apply(&signal, &mut fx);
        assert!(!fx.changed);
        assert!(Arc::ptr_eq(&after[0].1, &published(&state)[0].1));

        // An invalidated property asks for a new GetAll of that unit only.
        let mut fx = Effects::default();
        state.apply(
            &properties("bar.service", Props::new(), &["ExecStart"]),
            &mut fx,
        );
        let fetch: Vec<Arc<str>> = state
            .details_to_fetch(false)
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(fetch, vec![Arc::from("bar.service")]);

        // A failed read leaves the unit out until a signal or a full listing retries it.
        assert!(!state.set_detail_raw("bar.service", None));
        assert_eq!(state.details_to_fetch(false).len(), 1);
    }

    // OpenRC (E14-008)

    fn rc_temp() -> tempfile::TempDir {
        let tmp_base =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&tmp_base).expect("tmp base");
        tempfile::TempDir::new_in(&tmp_base).expect("temp dir")
    }

    const RC_INI: &str = "[default]\nsshd = started\n[manual]\nfoo = stopped\n";

    #[test]
    fn openrc_ini_parse_runlevels() {
        let list = parse_rc_status(RC_INI);
        assert_eq!(list.len(), 2);
        let records: Vec<UnitRecord> = list.iter().map(openrc_record).collect();
        let foo = &records[0];
        assert_eq!(&*foo.unit.name, "foo");
        assert_eq!(foo.unit.manager, Manager::System);
        assert_eq!(foo.kind, UnitType::Service);
        assert_eq!(foo.startup, StartupType::Disabled);
        assert_eq!(foo.status, ServiceStatus::Stopped);
        let sshd = &records[1];
        assert_eq!(&*sshd.unit.name, "sshd");
        assert_eq!(sshd.startup, StartupType::Enabled);
        assert_eq!(sshd.status, ServiceStatus::Running);
        assert!(sshd.description.is_empty());
        assert_eq!(sshd.main_pid, 0);
        assert!(!sshd.has_cgroup_procs);
        assert_eq!(sshd.cpu_pct.validity, Validity::Unsupported);
        assert_eq!(sshd.memory_bytes.validity, Validity::Unsupported);
        assert_eq!(sshd.disk_bps.validity, Validity::Unsupported);
        assert_eq!(sshd.tasks.validity, Validity::Unsupported);

        // Lines before the first header, blank lines and lines without `=` are skipped.
        let noisy = "sshd = started\n\n[default]\n  cron =  started  \njunk\n = stopped\n";
        let list = parse_rc_status(noisy);
        assert_eq!(list.len(), 1);
        assert_eq!(&*list[0].name, "cron");
        assert_eq!(&*list[0].state, "started");
        assert!(parse_rc_status("").is_empty());
    }

    #[test]
    fn openrc_startup_from_runlevel() {
        let ini = "[sysinit]\nudev = started\n[boot]\nhostname = started\n\
                   [default]\nsshd = started\n[nonetwork]\nlocal = started\n\
                   [hotplugged]\nnet.eth0 = started\n[manual]\nfoo = stopped\n\
                   [needed/wanted]\nsshd = started\nnet.lo = started\n";
        let startup: Vec<(String, StartupType)> = parse_rc_status(ini)
            .iter()
            .map(|svc| (svc.name.to_string(), openrc_record(svc).startup))
            .collect();
        let want = |name: &str| {
            startup
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, s)| *s)
                .expect("service listed")
        };
        assert_eq!(want("udev"), StartupType::Enabled);
        assert_eq!(want("hostname"), StartupType::Enabled);
        assert_eq!(
            want("sshd"),
            StartupType::Enabled,
            "listed twice, once in default"
        );
        assert_eq!(want("local"), StartupType::Disabled);
        assert_eq!(want("net.eth0"), StartupType::Disabled);
        assert_eq!(want("foo"), StartupType::Disabled);
        assert_eq!(want("net.lo"), StartupType::Disabled);
        assert_eq!(startup.len(), 7, "sshd is one record");
    }

    #[test]
    fn openrc_detection_paths() {
        let tmp = rc_temp();
        let root = tmp.path();
        assert_eq!(detect_init(root), Init::Systemd, "neither: the bus path");

        put(root, "usr/bin/rc-status", "");
        assert_eq!(
            detect_init(root),
            Init::OpenRc(root.join("usr/bin/rc-status"))
        );
        put(root, "sbin/rc-status", "");
        assert_eq!(
            detect_init(root),
            Init::OpenRc(root.join("sbin/rc-status")),
            "/sbin first"
        );

        std::fs::create_dir_all(root.join("run/systemd/system")).expect("marker dir");
        assert_eq!(detect_init(root), Init::Systemd, "systemd wins");

        let sys_root = SysRoot::at(root);
        assert_eq!(host_base(&sys_root), root);
        assert_eq!(host_base(&SysRoot::host()), Path::new("/"));
    }

    #[test]
    fn openrc_state_word_mapping() {
        let table = [
            ("started", ServiceStatus::Running),
            ("stopped", ServiceStatus::Stopped),
            ("starting", ServiceStatus::Starting),
            ("stopping", ServiceStatus::Stopping),
            ("crashed", ServiceStatus::Failed),
            ("inactive", ServiceStatus::Stopped),
            ("hotplugged", ServiceStatus::Stopped),
            ("foo", ServiceStatus::Active),
        ];
        for (word, want) in table {
            assert_eq!(openrc_status(word), want, "{word}");
        }
    }

    /// Counts listings. Pops a queued result first, else lists [`RC_INI`].
    #[derive(Default)]
    struct FakeRc {
        calls: Mutex<usize>,
        queued: Mutex<std::collections::VecDeque<Result<Vec<RcService>, RcError>>>,
    }

    impl FakeRc {
        fn calls(&self) -> usize {
            *self.calls.lock().expect("calls lock")
        }
    }

    impl RcLister for FakeRc {
        fn list(&self) -> Result<Vec<RcService>, RcError> {
            *self.calls.lock().expect("calls lock") += 1;
            let next = self.queued.lock().expect("queue lock").pop_front();
            next.unwrap_or_else(|| Ok(parse_rc_status(RC_INI)))
        }
    }

    type Ended = Arc<Mutex<Option<Outcome<(), WorkerError>>>>;

    fn openrc_loop(lab: &mut LabRuntime, io: WorkerIo, rc: Arc<FakeRc>) -> (TaskHandle<()>, Ended) {
        let out = Ended::default();
        let sink = Arc::clone(&out);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let outcome = run_openrc(&cx, &io, rc, 0).await;
                *sink.lock().expect("slot lock") = Some(outcome);
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        (handle, out)
    }

    /// The OpenRC updates in `rx`, each checked for the system manager and the OpenRC backend.
    fn openrc_updates(rx: &mut mpsc::Receiver<EngineMsg>) -> Vec<ServicesUpdate> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                EngineMsg::Source(SourceUpdate::Services(up)) => {
                    assert_eq!(up.manager, Manager::System);
                    assert_eq!(up.backend, ServiceBackend::OpenRc);
                    assert_eq!(up.state, ManagerState::Connected);
                    out.push(up);
                }
                other => panic!("unexpected message {other:?}"),
            }
        }
        out
    }

    #[test]
    fn openrc_refresh_every_5s_while_visible() {
        let mut lab = LabRuntime::new(LabConfig::new(81).max_steps(1_000_000));
        let (io, mut rx, control_tx, demand_tx) = worker_io();
        demand_tx
            .send(Demand {
                services: true,
                ..Demand::default()
            })
            .expect("demand send");
        let rc = Arc::new(FakeRc::default());
        let (handle, out) = openrc_loop(&mut lab, io, Arc::clone(&rc));

        // Visible: one listing at 0 s and one at 5 s, none more before 10 s.
        assert_eq!(rc.calls(), 1, "listed at start");
        for _ in 0..19 {
            advance(&mut lab, IDLE_RECHECK);
        }
        assert_eq!(rc.calls(), 2, "two listings in the first 10 s");
        let ups = openrc_updates(&mut rx);
        assert_eq!(ups.len(), 1, "an unchanged list is not published again");
        assert_eq!(ups[0].units.len(), 2);
        assert_eq!(ups[0].validity, Validity::Valid);

        // Hidden for 10 s: no listing.
        demand_tx.send(Demand::default()).expect("demand send");
        for _ in 0..20 {
            advance(&mut lab, IDLE_RECHECK);
        }
        assert_eq!(rc.calls(), 2, "no listing while hidden");

        // RefreshNow lists once while hidden.
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
        advance(&mut lab, IDLE_RECHECK);
        assert_eq!(rc.calls(), 3, "listed on RefreshNow");
        for _ in 0..20 {
            advance(&mut lab, IDLE_RECHECK);
        }
        assert_eq!(rc.calls(), 3, "seen once only");
        assert!(out.lock().expect("slot lock").is_none(), "worker runs");

        handle.abort();
        advance(&mut lab, IDLE_RECHECK);
        let outcome = out.lock().expect("slot lock").take().expect("worker ended");
        assert!(matches!(outcome, Outcome::Ok(())), "{outcome:?}");
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn openrc_failed_listing_keeps_records() {
        let mut lab = LabRuntime::new(LabConfig::new(82).max_steps(1_000_000));
        let (io, mut rx, _control_tx, demand_tx) = worker_io();
        demand_tx
            .send(Demand {
                services: true,
                ..Demand::default()
            })
            .expect("demand send");
        let rc = Arc::new(FakeRc::default());
        {
            let mut queued = rc.queued.lock().expect("queue lock");
            queued.push_back(Ok(parse_rc_status(RC_INI)));
            queued.push_back(Err(RcError::Timeout(RC_STATUS_TIMEOUT)));
            queued.push_back(Ok(parse_rc_status("[default]\nsshd = crashed\n")));
        }
        let (handle, out) = openrc_loop(&mut lab, io, Arc::clone(&rc));
        assert_eq!(openrc_updates(&mut rx).len(), 1);

        // The timeout publishes nothing and the worker keeps running.
        for _ in 0..10 {
            advance(&mut lab, IDLE_RECHECK);
        }
        assert_eq!(rc.calls(), 2);
        assert!(openrc_updates(&mut rx).is_empty(), "last records kept");
        assert!(out.lock().expect("slot lock").is_none(), "worker runs");

        // The next listing publishes the new list.
        for _ in 0..10 {
            advance(&mut lab, IDLE_RECHECK);
        }
        assert_eq!(rc.calls(), 3);
        let ups = openrc_updates(&mut rx);
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].units.len(), 1);
        assert_eq!(ups[0].units[0].status, ServiceStatus::Failed);

        handle.abort();
        advance(&mut lab, IDLE_RECHECK);
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn openrc_hung_child_is_killed() {
        assert_eq!(RC_STATUS_TIMEOUT, Duration::from_secs(5));
        let tmp = rc_temp();
        let pid_file = tmp.path().join("pid");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("echo $$ > \"$1\"; exec sleep 30")
            .arg("sh")
            .arg(&pid_file);
        let started = Instant::now();
        let res = run_capped(cmd, Duration::from_millis(300));
        assert!(matches!(res, Err(RcError::Timeout(_))), "{res:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "killed at the limit"
        );
        let pid = std::fs::read_to_string(&pid_file).expect("pid file");
        let proc_dir = format!("/proc/{}", pid.trim());
        assert!(!Path::new(&proc_dir).exists(), "child killed and reaped");

        let mut ok = Command::new("/bin/sh");
        ok.arg("-c").arg("printf '[default]\\nsshd = started\\n'");
        let text = run_capped(ok, Duration::from_secs(5)).expect("listing");
        assert_eq!(parse_rc_status(&String::from_utf8_lossy(&text)).len(), 1);

        let mut fails = Command::new("/bin/sh");
        fails.arg("-c").arg("exit 3");
        let res = run_capped(fails, Duration::from_secs(5));
        assert!(matches!(res, Err(RcError::Exit(_))), "{res:?}");
    }

    /// Fails the test if the bus is opened.
    struct NeverBus;

    impl Opener for NeverBus {
        async fn open(&self, _cx: &Cx, _manager: Manager) -> Result<DbusConn, OpenError> {
            panic!("the user manager on OpenRC must not open a bus");
        }
    }

    /// The first update of a worker started on `root`, in its own lab runtime.
    fn first_update(seed: u64, root: &Path, manager: Manager) -> ServicesUpdate {
        let mut lab = LabRuntime::new(LabConfig::new(seed).max_steps(1_000_000));
        let (io, mut rx, _control_tx, _demand_tx) = worker_io();
        let cfg = Arc::new(
            EngineConfig::new(EngineMode::Full, root.join("state"))
                .with_sys_root(SysRoot::at(root)),
        );
        let lab_root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(lab_root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let _ = match manager {
                    Manager::System => run(cx, io, cfg, manager, &NoBus).await,
                    Manager::User => run(cx, io, cfg, manager, &NeverBus).await,
                };
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let msg = rx.try_recv().expect("one update");
        handle.abort();
        advance(&mut lab, IDLE_RECHECK);
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
        match msg {
            EngineMsg::Source(SourceUpdate::Services(up)) => up,
            other => panic!("unexpected message {other:?}"),
        }
    }

    #[test]
    fn openrc_start_without_init_or_user_manager() {
        let tmp = rc_temp();

        // No systemd marker and no rc-status: the bus path publishes Unavailable.
        let up = first_update(83, tmp.path(), Manager::System);
        assert_eq!(up.state, ManagerState::Unavailable);
        assert_eq!(up.backend, ServiceBackend::Systemd);
        assert!(up.units.is_empty());

        // OpenRC: the user manager is Unavailable without a bus.
        put(tmp.path(), "sbin/rc-status", "");
        let up = first_update(84, tmp.path(), Manager::User);
        assert_eq!(up.manager, Manager::User);
        assert_eq!(up.state, ManagerState::Unavailable);
        assert_eq!(up.backend, ServiceBackend::OpenRc);
        assert_eq!(up.validity, Validity::Missing);
    }
}
