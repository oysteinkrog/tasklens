//! The `UPower` worker (ADR-004, PERF-078): battery rows for the battery pages.
//!
//! It lists devices with `EnumerateDevices`, keeps every device except line power and the display device
//! and follows `DeviceAdded`, `DeviceRemoved` and each device's
//! `PropertiesChanged`. Without UPower it reads `/sys/class/power_supply` every `SYSFS_PERIOD`
//! and tries the bus again every 60 s. It runs while any surface is visible or recording is on.
//! A forwarded `BatteryHistory` command also wakes it and fills that battery's `history` from
//! `GetHistory` (E12-029).
//!
//! It also publishes whether the system runs on battery (wlf.32): UPower's `OnBattery`, read at
//! each resync and on the manager's `PropertiesChanged`; without UPower, a battery and no mains
//! or USB supply online. `None` without a battery. Sent only when it changes.
//!
//! The battery serial is shown on screen only. Never log it.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::sleep;
use asupersync::{Cx, Outcome};
use tl_collect::sysroot::read_trimmed;
use tl_model::command::DeviceCommand;
use tl_model::devices::DeviceIdentity;
use tl_model::health::{SourceId, SourceState};
use tl_model::snapshot::{BatteryDetails, InvField};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{Bus, DbusConn, DbusError, connect};
use crate::source::{CommandProgress, IDLE_RECHECK, WorkerCommand, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{
    DeviceDesc, DeviceDetailsUpdate, DeviceKinds, DeviceList, EngineMsg, SourceUpdate, WorkerError,
};
use crate::workers::WorkerSpec;

pub const BUS: &str = "org.freedesktop.UPower";
pub const PATH: &str = "/org/freedesktop/UPower";
pub const SYSFS_PERIOD: Duration = Duration::from_secs(30);
/// `GetHistory` covers the last 24 h (R-166).
pub const HISTORY_SPAN_S: u32 = 86_400;
/// `GetHistory` resolution: one point per 5 min over `HISTORY_SPAN_S`.
pub const HISTORY_POINTS: u32 = 288;

const MANAGER_IFACE: &str = "org.freedesktop.UPower";
const DEVICE_IFACE: &str = "org.freedesktop.UPower.Device";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
/// The resync period with UPower, and the bus retry period without it.
const RESYNC: Duration = Duration::from_secs(60);
/// UPower `Type` for a battery.
const TYPE_LINE_POWER: u32 = 1;
/// The health text while the worker reads sysfs.
const NOT_AVAILABLE: &str = "UPower not available, reading /sys/class/power_supply";
/// The `GetHistory` timeout.
const HISTORY_TIMEOUT: Duration = Duration::from_secs(5);

/// The properties of one `org.freedesktop.UPower.Device`, from `GetAll`.
pub type Props = HashMap<String, OwnedValue>;

pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::UPower,
        name: "upower",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// Each generation runs [`run`] with `/sys` from `cfg.sys_root`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    let sys = cfg.sys_root.sys.clone();
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| run(cx, io.clone(), sys.clone()))
}

/// The worker on the system bus, with the sysfs fallback under `sys`.
#[doc(hidden)]
pub async fn run(cx: Cx, io: WorkerIo, sys: PathBuf) -> Outcome<(), WorkerError> {
    run_with(cx, io, sys, SystemConnector).await
}

/// Where the worker goes next.
enum Next<B> {
    Idle,
    Bus(B, BTreeMap<String, BatteryDetails>),
    Sysfs,
    Stop(Outcome<(), WorkerError>),
}

/// The state machine: idle, UPower or sysfs. Returns `Outcome::Ok(())` on cancel and `Err` when
/// the bus fails or the reducer's queue is closed, so the supervisor restarts it.
async fn run_with<C: Connector>(
    cx: Cx,
    io: WorkerIo,
    sys: PathBuf,
    connector: C,
) -> Outcome<(), WorkerError> {
    let mut sent = Sent::default();
    let mut commands = Commands::resume(&io);
    let mut next = Next::Idle;
    loop {
        next = match next {
            Next::Idle => {
                if let Some(stop) = idle(&cx, &io, &commands).await {
                    return stop;
                }
                match start_bus(&cx, &connector).await {
                    Ok((bus, batteries)) => Next::Bus(bus, batteries),
                    Err(e) if unavailable(&e) => Next::Sysfs,
                    Err(e) => fail(&cx, &e),
                }
            }
            Next::Bus(bus, batteries) => {
                bus_loop(&cx, &io, bus, batteries, &mut sent, &mut commands).await
            }
            Next::Sysfs => sysfs_loop(&cx, &io, &sys, &connector, &mut sent, &mut commands).await,
            Next::Stop(outcome) => return outcome,
        };
    }
}

/// Step 1: waits until a surface is visible, recording is on or a command is queued. `Some` on
/// cancel.
async fn idle(cx: &Cx, io: &WorkerIo, commands: &Commands) -> Option<Outcome<(), WorkerError>> {
    while !active(io) && !commands.pending(io) {
        sleep(cx.now(), IDLE_RECHECK).await;
        if cx.checkpoint().is_err() {
            return Some(Outcome::Ok(()));
        }
    }
    None
}

fn active(io: &WorkerIo) -> bool {
    let demand = io.demand.borrow_and_clone();
    demand.any_surface_visible || demand.recording
}

/// Opens the bus and lists the batteries (steps 2 and 3).
async fn start_bus<C: Connector>(
    cx: &Cx,
    connector: &C,
) -> Result<(C::Bus, BTreeMap<String, BatteryDetails>), DbusError> {
    let mut bus = connector.open(cx).await?;
    let batteries = resync(cx, &mut bus).await?;
    Ok((bus, batteries))
}

/// Step 3: every battery by object path. A device whose `GetAll` fails is left out.
async fn resync<B: UpowerBus>(
    cx: &Cx,
    bus: &mut B,
) -> Result<BTreeMap<String, BatteryDetails>, DbusError> {
    let mut batteries = BTreeMap::new();
    for path in bus.devices(cx).await? {
        match bus.props(cx, &path).await {
            Ok(props) => {
                if is_display_device(&path) {
                    continue;
                }
                if let Some(battery) = battery_from_props(&props) {
                    batteries.insert(path, battery);
                }
            }
            Err(DbusError::Closed) => return Err(DbusError::Closed),
            Err(e) => tracing::debug!(error = dbus_name(&e), "UPower GetAll failed"),
        }
    }
    Ok(batteries)
}

/// Steps 4 to 6 with UPower.
async fn bus_loop<B: UpowerBus>(
    cx: &Cx,
    io: &WorkerIo,
    mut bus: B,
    mut batteries: BTreeMap<String, BatteryDetails>,
    sent: &mut Sent,
    commands: &mut Commands,
) -> Next<B> {
    if let Some(stop) = send(cx, io, health(None)).await {
        return Next::Stop(stop);
    }
    let mut flag = match on_battery_flag(cx, &mut bus).await {
        Ok(flag) => flag,
        Err(e) => return fail(cx, &e),
    };
    let on = |batteries: &BTreeMap<String, BatteryDetails>, flag| {
        on_battery(!batteries.is_empty(), flag)
    };
    if let Some(stop) = publish(cx, io, batteries.values().cloned().collect(), sent).await {
        return Next::Stop(stop);
    }
    if let Some(stop) = send_on_battery(cx, io, on(&batteries, flag), sent).await {
        return Next::Stop(stop);
    }
    let mut next_resync = cx.now().as_nanos() + nanos(RESYNC);
    loop {
        for (seq, command) in commands.take(io) {
            commands.progress.set_started(SourceId::UPower, seq);
            let stop = match command {
                WorkerCommand::Device(DeviceCommand::BatteryHistory { native_path }) => {
                    history(cx, io, &mut bus, &mut batteries, &native_path, sent).await
                }
                _ => None,
            };
            commands.progress.set_done(SourceId::UPower, seq);
            if let Some(stop) = stop {
                return Next::Stop(stop);
            }
        }
        if !active(io) {
            return Next::Idle;
        }
        let changed = match bus.next_event(cx, IDLE_RECHECK).await {
            Ok(Some(Event::ManagerChanged)) => {
                flag = match on_battery_flag(cx, &mut bus).await {
                    Ok(flag) => flag,
                    Err(e) => return fail(cx, &e),
                };
                false
            }
            Ok(Some(event)) => match apply_event(cx, &mut bus, &mut batteries, event).await {
                Ok(changed) => changed,
                Err(e) => return fail(cx, &e),
            },
            Ok(None) => {
                if cx.checkpoint().is_err() {
                    return Next::Stop(Outcome::Ok(()));
                }
                false
            }
            Err(e) => return fail(cx, &e),
        };
        let mut resynced = false;
        if cx.now().as_nanos() >= next_resync {
            next_resync = cx.now().as_nanos() + nanos(RESYNC);
            match resync(cx, &mut bus).await {
                Ok(mut fresh) => {
                    for (path, battery) in &mut fresh {
                        battery.history = batteries.get(path).and_then(|b| b.history.clone());
                    }
                    batteries = fresh;
                    resynced = true;
                }
                Err(e) if unavailable(&e) => return Next::Sysfs,
                Err(e) => return fail(cx, &e),
            }
            flag = match on_battery_flag(cx, &mut bus).await {
                Ok(flag) => flag,
                Err(e) if unavailable(&e) => return Next::Sysfs,
                Err(e) => return fail(cx, &e),
            };
        }
        if (changed || resynced)
            && let Some(stop) = publish(cx, io, batteries.values().cloned().collect(), sent).await
        {
            return Next::Stop(stop);
        }
        if let Some(stop) = send_on_battery(cx, io, on(&batteries, flag), sent).await {
            return Next::Stop(stop);
        }
    }
}

/// UPower's `OnBattery`. `Ok(None)` when the call fails with anything but a closed bus.
async fn on_battery_flag<B: UpowerBus>(cx: &Cx, bus: &mut B) -> Result<Option<bool>, DbusError> {
    match bus.on_battery(cx).await {
        Ok(flag) => Ok(Some(flag)),
        Err(DbusError::Closed) => Err(DbusError::Closed),
        Err(e) => {
            tracing::debug!(error = dbus_name(&e), "UPower OnBattery failed");
            Ok(None)
        }
    }
}

/// The published state: `None` without a battery, else the source's flag.
fn on_battery(has_battery: bool, flag: Option<bool>) -> Option<bool> {
    flag.filter(|_| has_battery)
}

/// Sends `OnBattery` when it differs from the last one sent.
async fn send_on_battery(
    cx: &Cx,
    io: &WorkerIo,
    on: Option<bool>,
    sent: &mut Sent,
) -> Option<Outcome<(), WorkerError>> {
    if sent.on_battery == Some(on) {
        return None;
    }
    let update = SourceUpdate::DeviceDetails(DeviceDetailsUpdate::OnBattery(on));
    let stop = send(cx, io, update).await;
    if stop.is_none() {
        sent.on_battery = Some(on);
    }
    stop
}

/// Step 5 for one signal. `Ok(true)` when the battery set or a battery changed.
async fn apply_event<B: UpowerBus>(
    cx: &Cx,
    bus: &mut B,
    batteries: &mut BTreeMap<String, BatteryDetails>,
    event: Event,
) -> Result<bool, DbusError> {
    match event {
        Event::Removed(path) => Ok(batteries.remove(&path).is_some()),
        // `bus_loop` reads `OnBattery` itself.
        Event::ManagerChanged => Ok(false),
        // Changes of other devices (the display device, a mouse) are ignored; the resync
        // picks up a device that becomes a battery.
        Event::Changed(path) if !batteries.contains_key(&path) => Ok(false),
        Event::Added(path) if is_display_device(&path) => Ok(false),
        Event::Added(path) | Event::Changed(path) => {
            let props = match bus.props(cx, &path).await {
                Ok(props) => props,
                Err(DbusError::Closed) => return Err(DbusError::Closed),
                Err(e) => {
                    tracing::debug!(error = dbus_name(&e), "UPower GetAll failed");
                    return Ok(false);
                }
            };
            match battery_from_props(&props) {
                Some(mut battery) => {
                    // The cached history stays until the next `BatteryHistory`.
                    battery.history = batteries.get(&path).and_then(|b| b.history.clone());
                    batteries.insert(path, battery);
                    Ok(true)
                }
                None => Ok(batteries.remove(&path).is_some()),
            }
        }
    }
}

/// Answers one `BatteryHistory`: `GetHistory` on the device whose `NativePath` matches, then
/// `Batteries`. An unknown native path sends nothing. An error gives an empty history.
async fn history<B: UpowerBus>(
    cx: &Cx,
    io: &WorkerIo,
    bus: &mut B,
    batteries: &mut BTreeMap<String, BatteryDetails>,
    native_path: &str,
    sent: &mut Sent,
) -> Option<Outcome<(), WorkerError>> {
    let path = batteries
        .iter()
        .find(|(_, b)| &*b.native_path == native_path)
        .map(|(path, _)| path.clone())?;
    let points = match bus.history(cx, &path).await {
        Ok(raw) => history_points(&raw),
        Err(e) => {
            tracing::debug!(error = dbus_name(&e), "UPower GetHistory failed");
            Arc::from([])
        }
    };
    if let Some(battery) = batteries.get_mut(&path) {
        battery.history = Some(points);
    }
    publish(cx, io, batteries.values().cloned().collect(), sent).await
}

/// `GetHistory` points as `(unix time s, percent)`: entries with time 0 or a percent outside
/// 0 to 100 are dropped, the rest sorted by time. Of equal times the last one wins.
pub fn history_points(raw: &[(u32, f64, u32)]) -> Arc<[(i64, f32)]> {
    let mut kept: Vec<(u32, f64)> = raw
        .iter()
        .filter(|(time, pct, _)| *time != 0 && (0.0..=100.0).contains(pct))
        .map(|&(time, pct, _)| (time, pct))
        .collect();
    // A stable sort keeps the reply order among equal times.
    kept.sort_by_key(|&(time, _)| time);
    let mut points: Vec<(i64, f32)> = Vec::with_capacity(kept.len());
    for (time, pct) in kept {
        let time = i64::from(time);
        match points.last_mut() {
            Some(last) if last.0 == time => last.1 = pct as f32,
            _ => points.push((time, pct as f32)),
        }
    }
    points.into()
}

/// Step 2 without UPower: reads sysfs every `SYSFS_PERIOD` and tries the bus every 60 s.
async fn sysfs_loop<C: Connector>(
    cx: &Cx,
    io: &WorkerIo,
    sys: &Path,
    connector: &C,
    sent: &mut Sent,
    commands: &mut Commands,
) -> Next<C::Bus> {
    if let Some(stop) = send(cx, io, health(Some(NOT_AVAILABLE.into()))).await {
        return Next::Stop(stop);
    }
    let mut next_read = cx.now().as_nanos();
    let mut next_retry = next_read + nanos(RESYNC);
    loop {
        // Sysfs has no history: the page shows "No history".
        for (seq, _) in commands.take(io) {
            commands.progress.set_started(SourceId::UPower, seq);
            commands.progress.set_done(SourceId::UPower, seq);
        }
        if !active(io) {
            return Next::Idle;
        }
        let now = cx.now().as_nanos();
        if now >= next_read {
            next_read = now + nanos(SYSFS_PERIOD);
            let (batteries, supply_online) = match read_sysfs(cx, sys).await {
                Ok(read) => read,
                Err(stop) => return Next::Stop(stop),
            };
            let on = on_battery(!batteries.is_empty(), supply_online.map(|online| !online));
            if let Some(stop) = publish(cx, io, batteries, sent).await {
                return Next::Stop(stop);
            }
            if let Some(stop) = send_on_battery(cx, io, on, sent).await {
                return Next::Stop(stop);
            }
        }
        if cx.now().as_nanos() >= next_retry {
            next_retry = cx.now().as_nanos() + nanos(RESYNC);
            match start_bus(cx, connector).await {
                Ok((bus, batteries)) => return Next::Bus(bus, batteries),
                Err(DbusError::Closed) if cx.checkpoint().is_err() => {
                    return Next::Stop(Outcome::Ok(()));
                }
                Err(e) => tracing::debug!(error = dbus_name(&e), "UPower still not available"),
            }
        }
        sleep(cx.now(), IDLE_RECHECK).await;
        if cx.checkpoint().is_err() {
            return Next::Stop(Outcome::Ok(()));
        }
    }
}

/// The sysfs batteries and [`sysfs_supply_online`]: one listing in a blocking job.
type SysfsRead = (Vec<BatteryDetails>, Option<bool>);

/// One sysfs listing in a blocking job.
async fn read_sysfs(cx: &Cx, sys: &Path) -> Result<SysfsRead, Outcome<(), WorkerError>> {
    let dir = sys.join("class/power_supply");
    let mut job =
        match cx.spawn_blocking(move |_| (sysfs_batteries(&dir), sysfs_supply_online(&dir))) {
            Ok(job) => job,
            Err(e) => {
                return Err(Outcome::Err(WorkerError::Other(
                    format!("spawn: {e}").into(),
                )));
            }
        };
    match job.join(cx).await {
        Ok(batteries) => Ok(batteries),
        Err(JoinError::Cancelled(reason)) => {
            if cx.checkpoint().is_err() {
                return Err(Outcome::Ok(()));
            }
            Err(Outcome::Cancelled(reason))
        }
        Err(JoinError::Panicked(payload)) => Err(Outcome::Panicked(payload)),
        Err(JoinError::PolledAfterCompletion) => Err(Outcome::Err(WorkerError::Other(
            "job polled after completion".into(),
        ))),
    }
}

/// Every `type == Battery` entry of `dir`, peripheral ones (`scope == Device`) included, by name.
fn sysfs_batteries(dir: &Path) -> Vec<BatteryDetails> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|path| battery_from_sysfs(path))
        .collect()
}

/// Whether any external supply in `dir` is online: `type` `Mains` or `USB` (a USB-C charger)
/// with `online` 1. `None` when there is no such supply.
fn sysfs_supply_online(dir: &Path) -> Option<bool> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut buf = Vec::new();
    let mut found = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let mut text = |name: &str| -> Option<String> {
            let bytes = read_trimmed(&path.join(name), &mut buf).ok()?;
            Some(std::str::from_utf8(bytes).ok()?.to_owned())
        };
        if !matches!(text("type").as_deref(), Some("Mains" | "USB")) {
            continue;
        }
        let online = text("online").as_deref() == Some("1");
        found = Some(found.unwrap_or(false) || online);
    }
    found
}

/// Commands the reducer forwards to this worker (R-276).
struct Commands {
    progress: Arc<CommandProgress>,
    /// The highest `seq` seen.
    last_seq: u64,
}

impl Commands {
    /// Resumes after the last command a generation started. One that was running when the last
    /// generation died is skipped without a message: the page can ask again.
    fn resume(io: &WorkerIo) -> Self {
        let progress = Arc::clone(&io.control.borrow_and_clone().progress);
        let last_seq = progress.started(SourceId::UPower);
        if last_seq > progress.done(SourceId::UPower) {
            progress.set_done(SourceId::UPower, last_seq);
        }
        Self { progress, last_seq }
    }

    /// Whether a command for this worker above `last_seq` is queued.
    fn pending(&self, io: &WorkerIo) -> bool {
        let control = io.control.borrow_and_clone();
        control
            .commands
            .iter()
            .any(|c| c.seq > self.last_seq && c.target == SourceId::UPower)
    }

    /// The queued commands for this worker above `last_seq`, in `seq` order. The caller marks
    /// each one started and done.
    fn take(&mut self, io: &WorkerIo) -> Vec<(u64, WorkerCommand)> {
        let control = io.control.borrow_and_clone();
        let taken = control
            .commands
            .iter()
            .filter(|c| c.seq > self.last_seq && c.target == SourceId::UPower)
            .map(|c| (c.seq, c.command.clone()))
            .collect();
        if let Some(newest) = control.commands.last() {
            self.last_seq = self.last_seq.max(newest.seq);
        }
        taken
    }
}

/// The last `Devices` list sent, as (native path, display name) pairs, and the last
/// `OnBattery`.
#[derive(Default)]
struct Sent {
    devices: Option<Vec<(Arc<str>, Arc<str>)>>,
    on_battery: Option<Option<bool>>,
}

/// Step 4: `Devices` when the set changed, then `Batteries`.
async fn publish(
    cx: &Cx,
    io: &WorkerIo,
    batteries: Vec<BatteryDetails>,
    sent: &mut Sent,
) -> Option<Outcome<(), WorkerError>> {
    let names: Vec<(Arc<str>, Arc<str>)> = batteries
        .iter()
        .enumerate()
        .map(|(n, b)| (Arc::clone(&b.native_path), display_name(b, n)))
        .collect();
    if sent.devices.as_ref() != Some(&names) {
        let entries = names
            .iter()
            .map(|(native_path, name)| DeviceDesc {
                identity: DeviceIdentity::Battery {
                    native_path: Arc::clone(native_path),
                },
                display_name: Arc::clone(name),
                parent: None,
                persistent_key: format!("bat:{native_path}").into(),
                net_kind: None,
            })
            .collect();
        let list = DeviceList {
            t_ns: now_boot_ns(),
            kinds: DeviceKinds::BATTERIES,
            entries,
        };
        if let Some(stop) = send(cx, io, SourceUpdate::Devices(list)).await {
            return Some(stop);
        }
        sent.devices = Some(names);
    }
    let entries = batteries
        .into_iter()
        .map(|b| {
            let identity = DeviceIdentity::Battery {
                native_path: Arc::clone(&b.native_path),
            };
            (identity, b)
        })
        .collect();
    let update = SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Batteries(entries));
    send(cx, io, update).await
}

/// "<Vendor> <Model>" trimmed, or "Battery <n + 1>" when both are empty.
fn display_name(b: &BatteryDetails, n: usize) -> Arc<str> {
    let part = |f: &InvField<Arc<str>>| {
        if f.validity == Validity::Valid {
            Arc::clone(&f.value)
        } else {
            Arc::from("")
        }
    };
    let name = format!("{} {}", part(&b.vendor), part(&b.model));
    let name = name.trim();
    if name.is_empty() {
        format!("Battery {}", n + 1).into()
    } else {
        name.into()
    }
}

fn health(last_error: Option<Arc<str>>) -> SourceUpdate {
    // `SourceState` has no degraded state: the fallback keeps `Running` with the text, as the
    // history recorder does for dropped events (R-249).
    SourceUpdate::Health {
        id: SourceId::UPower,
        state: SourceState::Running,
        last_error,
        t_ns: now_boot_ns(),
    }
}

/// Sends one update. `Some` holds the worker's result when it must stop.
async fn send(cx: &Cx, io: &WorkerIo, update: SourceUpdate) -> Option<Outcome<(), WorkerError>> {
    match io.updates.send(cx, EngineMsg::Source(update)).await {
        Ok(()) => None,
        Err(mpsc::SendError::Cancelled(_)) => Some(Outcome::Ok(())),
        Err(_) => Some(Outcome::Err(WorkerError::Closed)),
    }
}

/// `Ok` on cancel, else the error for the supervisor.
fn fail<B>(cx: &Cx, e: &DbusError) -> Next<B> {
    if cx.checkpoint().is_err() {
        return Next::Stop(Outcome::Ok(()));
    }
    tracing::debug!(error = dbus_name(e), "UPower bus failed");
    Next::Stop(Outcome::Err(WorkerError::Dbus(dbus_name(e).into())))
}

/// UPower is not on the bus: switch to sysfs.
fn unavailable(e: &DbusError) -> bool {
    matches!(e, DbusError::Call { name, .. }
        if &**name == "org.freedesktop.DBus.Error.ServiceUnknown"
            || &**name == "org.freedesktop.DBus.Error.NameHasNoOwner")
}

/// The D-Bus error name or the error kind. Messages are never logged.
fn dbus_name(e: &DbusError) -> &str {
    match e {
        DbusError::Connect(_) => "connect",
        DbusError::Timeout(_) => "timeout",
        DbusError::Call { name, .. } => name,
        DbusError::Closed => "closed",
    }
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

// ---- Building `BatteryDetails` ----

fn valid<T>(value: T) -> InvField<T> {
    InvField {
        value,
        validity: Validity::Valid,
    }
}

fn unsupported<T: Default>() -> InvField<T> {
    InvField {
        value: T::default(),
        validity: Validity::Unsupported,
    }
}

fn opt<T: Default>(value: Option<T>) -> InvField<T> {
    value.map_or_else(unsupported, valid)
}

/// A number property of any integer or double type.
fn number(p: &Props, key: &str) -> Option<f64> {
    match &**p.get(key)? {
        Value::U8(v) => Some(f64::from(*v)),
        Value::I16(v) => Some(f64::from(*v)),
        Value::U16(v) => Some(f64::from(*v)),
        Value::I32(v) => Some(f64::from(*v)),
        Value::U32(v) => Some(f64::from(*v)),
        Value::I64(v) => Some(*v as f64),
        Value::U64(v) => Some(*v as f64),
        Value::F64(v) => Some(*v),
        _ => None,
    }
}

fn flag(p: &Props, key: &str) -> Option<bool> {
    match &**p.get(key)? {
        Value::Bool(v) => Some(*v),
        _ => None,
    }
}

/// A non-empty string property.
fn string(p: &Props, key: &str) -> Option<Arc<str>> {
    match &**p.get(key)? {
        Value::Str(s) if !s.is_empty() => Some(Arc::from(s.as_str())),
        _ => None,
    }
}

/// A float field where 0 means not reported (energy and voltage fields).
fn nonzero(p: &Props, key: &str) -> InvField<f32> {
    opt(number(p, key).filter(|v| *v != 0.0).map(|v| v as f32))
}

/// A percent threshold, clamped to 0..=100.
fn percent(v: Option<f64>) -> u8 {
    v.map_or(0, |v| v.clamp(0.0, 100.0) as u8)
}

/// The UPower `State` text.
fn state_text(state: Option<f64>) -> &'static str {
    match state.map(|v| v as u32) {
        Some(1) => "Charging",
        Some(2) => "Discharging",
        Some(3) => "Empty",
        Some(4) => "Fully charged",
        Some(5) => "Pending charge",
        Some(6) => "Pending discharge",
        _ => "Unknown",
    }
}

/// The UPower `Technology` text.
fn technology_text(tech: Option<f64>) -> &'static str {
    match tech.map(|v| v as u32) {
        Some(1) => "Lithium ion",
        Some(2) => "Lithium polymer",
        Some(3) => "Lithium iron phosphate",
        Some(4) => "Lead acid",
        Some(5) => "Nickel cadmium",
        Some(6) => "Nickel metal hydride",
        _ => "Unknown",
    }
}

/// The UPower device `Type` as a stable English word. The UI maps it to its own text.
fn kind_text(ty: u32) -> &'static str {
    match ty {
        2 => "Battery",
        3 => "UPS",
        4 => "Monitor",
        5 => "Mouse",
        6 => "Keyboard",
        7 => "PDA",
        8 => "Phone",
        9 => "Media player",
        10 => "Tablet",
        11 => "Computer",
        12 => "Gaming input",
        13 => "Pen",
        14 => "Touchpad",
        15 => "Modem",
        16 => "Network",
        17 => "Headset",
        18 => "Speakers",
        19 => "Headphones",
        20 => "Video",
        21 => "Other audio",
        22 => "Remote control",
        23 => "Printer",
        24 => "Scanner",
        25 => "Camera",
        26 => "Wearable",
        27 => "Toy",
        28 => "Bluetooth generic",
        _ => "Unknown",
    }
}

/// Whether `path` is the UPower composite `DisplayDevice`, which is not a device of its own.
fn is_display_device(path: &str) -> bool {
    path.ends_with("/DisplayDevice")
}

/// One device from UPower `Device` properties. `None` when `Type` is missing or 1 (line
/// power). Every other type is kept, with `kind` from the `Type` and `power_supply` from the
/// `PowerSupply` flag.
pub fn battery_from_props(p: &Props) -> Option<BatteryDetails> {
    let ty = number(p, "Type")? as u32;
    if ty == TYPE_LINE_POWER {
        return None;
    }
    let time = |key: &str| opt(number(p, key).filter(|v| *v != 0.0).map(|v| v as i64));
    let charge_threshold = (flag(p, "ChargeThresholdSupported") == Some(true)).then(|| {
        (
            flag(p, "ChargeThresholdEnabled").unwrap_or(false),
            percent(number(p, "ChargeStartThreshold")),
            percent(number(p, "ChargeEndThreshold")),
        )
    });
    Some(BatteryDetails {
        kind: kind_text(ty).into(),
        vendor: opt(string(p, "Vendor")),
        model: opt(string(p, "Model")),
        serial: opt(string(p, "Serial")),
        native_path: string(p, "NativePath").unwrap_or_else(|| Arc::from("")),
        percentage: opt(number(p, "Percentage").map(|v| v as f32)),
        energy_wh: nonzero(p, "Energy"),
        energy_rate_w: nonzero(p, "EnergyRate"),
        voltage_v: nonzero(p, "Voltage"),
        state: state_text(number(p, "State")).into(),
        time_to_full_s: time("TimeToFull"),
        time_to_empty_s: time("TimeToEmpty"),
        charge_cycles: opt(number(p, "ChargeCycles")
            .filter(|v| *v > 0.0)
            .map(|v| v as u32)),
        technology: technology_text(number(p, "Technology")).into(),
        capacity_pct: opt(number(p, "Capacity").map(|v| v as f32)),
        energy_empty_wh: nonzero(p, "EnergyEmpty"),
        energy_full_wh: nonzero(p, "EnergyFull"),
        energy_full_design_wh: nonzero(p, "EnergyFullDesign"),
        voltage_min_design_v: nonzero(p, "VoltageMinDesign"),
        voltage_max_design_v: nonzero(p, "VoltageMaxDesign"),
        power_supply: flag(p, "PowerSupply") == Some(true),
        charge_threshold,
        temperature_c: nonzero(p, "Temperature"),
        history: None,
    })
}

/// The kernel `technology` text as UPower names it. Other names pass through.
fn sysfs_technology(text: Option<&str>) -> Arc<str> {
    match text {
        Some("Li-ion") => "Lithium ion".into(),
        Some("Li-poly") => "Lithium polymer".into(),
        Some("LiFe") => "Lithium iron phosphate".into(),
        Some("NiCd") => "Nickel cadmium".into(),
        Some("NiMH") => "Nickel metal hydride".into(),
        Some(other) => other.into(),
        None => "Unknown".into(),
    }
}

/// One battery from a `/sys/class/power_supply/<name>` directory. `None` unless `type` is
/// `Battery`. `kind` is Battery and `power_supply` true for the system scope; `scope == Device`
/// (a peripheral battery) gives `Other` and false. Times are
/// `Unsupported`: the kernel does not report them.
pub fn battery_from_sysfs(dir: &Path) -> Option<BatteryDetails> {
    let mut buf = Vec::new();
    let mut text = |name: &str| -> Option<String> {
        let bytes = read_trimmed(&dir.join(name), &mut buf).ok()?;
        let s = std::str::from_utf8(bytes).ok()?;
        (!s.is_empty()).then(|| s.to_owned())
    };
    if text("type").as_deref() != Some("Battery") {
        return None;
    }
    let system = text("scope").as_deref() != Some("Device");
    let mut num = |name: &str| text(name)?.parse::<i64>().ok();
    // µV, µW and µWh to V, W and Wh.
    let micro = |v: Option<i64>| opt(v.map(|v| (v as f64 / 1e6) as f32));
    let vmin = num("voltage_min_design");
    // µAh × µV is 1e-12 Wh.
    let mut wh = |energy: &str, charge: &str| match num(energy) {
        Some(e) => valid((e as f64 / 1e6) as f32),
        None => match (num(charge), vmin) {
            (Some(c), Some(v)) => valid((c as f64 * v as f64 / 1e12) as f32),
            _ => unsupported(),
        },
    };
    let energy_wh = wh("energy_now", "charge_now");
    let energy_full_wh = wh("energy_full", "charge_full");
    let energy_full_design_wh = wh("energy_full_design", "charge_full_design");
    let percentage = opt(num("capacity").map(|v| v as f32));
    let energy_rate_w = micro(num("power_now"));
    let voltage_v = micro(num("voltage_now"));
    let voltage_max_design_v = micro(num("voltage_max_design"));
    let charge_cycles = opt(num("cycle_count")
        .filter(|v| *v > 0)
        .and_then(|v| u32::try_from(v).ok()));
    let start = num("charge_control_start_threshold");
    let end = num("charge_control_end_threshold");
    let charge_threshold = match (start, end) {
        (Some(start), Some(end)) => {
            Some((true, percent(Some(start as f64)), percent(Some(end as f64))))
        }
        _ => None,
    };
    let state: Arc<str> = text("status").unwrap_or_else(|| "Unknown".into()).into();
    let technology = sysfs_technology(text("technology").as_deref());
    let vendor = opt(text("manufacturer").map(Arc::from));
    let model = opt(text("model_name").map(Arc::from));
    let serial = opt(text("serial_number").map(Arc::from));
    let native_path: Arc<str> = dir.file_name()?.to_string_lossy().into();
    Some(BatteryDetails {
        kind: if system { "Battery" } else { "Other" }.into(),
        vendor,
        model,
        serial,
        native_path,
        percentage,
        energy_wh,
        energy_rate_w,
        voltage_v,
        state,
        time_to_full_s: unsupported(),
        time_to_empty_s: unsupported(),
        charge_cycles,
        technology,
        capacity_pct: unsupported(),
        energy_empty_wh: unsupported(),
        energy_full_wh,
        energy_full_design_wh,
        voltage_min_design_v: micro(vmin),
        voltage_max_design_v,
        power_supply: system,
        charge_threshold,
        temperature_c: unsupported(),
        history: None,
    })
}

// ---- The bus seam ----

/// A UPower signal, by device object path.
#[derive(Debug, Clone, PartialEq)]
enum Event {
    Added(String),
    Removed(String),
    Changed(String),
    /// `PropertiesChanged` on the manager (`OnBattery`).
    ManagerChanged,
}

/// Opens a UPower session. Tests replace the system bus with a fake.
trait Connector: Send + Sync + 'static {
    type Bus: UpowerBus + Send;
    fn open(&self, cx: &Cx) -> impl Future<Output = Result<Self::Bus, DbusError>> + Send;
}

/// The five UPower calls the worker makes.
trait UpowerBus {
    /// `EnumerateDevices`.
    fn devices(&mut self, cx: &Cx) -> impl Future<Output = Result<Vec<String>, DbusError>> + Send;
    /// `GetAll("org.freedesktop.UPower.Device")` on `path`.
    fn props(
        &mut self,
        cx: &Cx,
        path: &str,
    ) -> impl Future<Output = Result<Props, DbusError>> + Send;
    /// The next signal, or `Ok(None)` when none arrived within `d`.
    fn next_event(
        &mut self,
        cx: &Cx,
        d: Duration,
    ) -> impl Future<Output = Result<Option<Event>, DbusError>> + Send;
    /// `Get("org.freedesktop.UPower", "OnBattery")` on the manager.
    fn on_battery(&mut self, cx: &Cx) -> impl Future<Output = Result<bool, DbusError>> + Send;
    /// `GetHistory("charge", HISTORY_SPAN_S, HISTORY_POINTS)` on `path`: (unix time s, percent,
    /// state) entries.
    fn history(
        &mut self,
        cx: &Cx,
        path: &str,
    ) -> impl Future<Output = Result<Vec<(u32, f64, u32)>, DbusError>> + Send;
}

struct SystemConnector;

impl Connector for SystemConnector {
    type Bus = SystemUpower;

    async fn open(&self, cx: &Cx) -> Result<SystemUpower, DbusError> {
        let conn = connect(cx, Bus::System).await?;
        SystemUpower::on(cx, conn).await
    }
}

/// The real session. Fields drop in order, so the stream goes before the connection's tick
/// task stops.
struct SystemUpower {
    stream: zbus::MessageStream,
    conn: DbusConn,
}

impl SystemUpower {
    /// Subscribes to every UPower signal under `PATH`: `DeviceAdded` and `DeviceRemoved` on the
    /// manager and `PropertiesChanged` on each device.
    async fn on(cx: &Cx, conn: DbusConn) -> Result<Self, DbusError> {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(BUS)
            .and_then(|b| b.path_namespace(PATH))
            .map_err(|e| call_error(&e))?
            .build();
        let stream = conn
            .read(
                cx,
                zbus::MessageStream::for_match_rule(rule, conn.raw(), Some(64)),
            )
            .await?;
        Ok(Self { stream, conn })
    }
}

fn call_error(e: &impl std::fmt::Display) -> DbusError {
    DbusError::Call {
        name: "zbus".into(),
        message: e.to_string().into(),
    }
}

impl UpowerBus for SystemUpower {
    async fn devices(&mut self, cx: &Cx) -> Result<Vec<String>, DbusError> {
        let raw = self.conn.raw();
        let call = raw.call_method(
            Some(BUS),
            PATH,
            Some(MANAGER_IFACE),
            "EnumerateDevices",
            &(),
        );
        let msg = self.conn.read(cx, call).await?;
        let paths: Vec<OwnedObjectPath> = msg.body().deserialize().map_err(|e| call_error(&e))?;
        Ok(paths.into_iter().map(|p| p.as_str().to_owned()).collect())
    }

    async fn props(&mut self, cx: &Cx, path: &str) -> Result<Props, DbusError> {
        let raw = self.conn.raw();
        let call = raw.call_method(
            Some(BUS),
            path,
            Some(PROPS_IFACE),
            "GetAll",
            &(DEVICE_IFACE,),
        );
        let msg = self.conn.read(cx, call).await?;
        msg.body().deserialize().map_err(|e| call_error(&e))
    }

    async fn on_battery(&mut self, cx: &Cx) -> Result<bool, DbusError> {
        let raw = self.conn.raw();
        let call = raw.call_method(
            Some(BUS),
            PATH,
            Some(PROPS_IFACE),
            "Get",
            &(MANAGER_IFACE, "OnBattery"),
        );
        let msg = self.conn.read(cx, call).await?;
        let value: OwnedValue = msg.body().deserialize().map_err(|e| call_error(&e))?;
        bool::try_from(value).map_err(|e| call_error(&e))
    }

    async fn history(&mut self, cx: &Cx, path: &str) -> Result<Vec<(u32, f64, u32)>, DbusError> {
        let raw = self.conn.raw();
        let call = raw.call_method(
            Some(BUS),
            path,
            Some(DEVICE_IFACE),
            "GetHistory",
            &("charge", HISTORY_SPAN_S, HISTORY_POINTS),
        );
        let msg = self.conn.call(cx, HISTORY_TIMEOUT, call).await?;
        msg.body().deserialize().map_err(|e| call_error(&e))
    }

    async fn next_event(&mut self, cx: &Cx, d: Duration) -> Result<Option<Event>, DbusError> {
        let Some(item) = self.conn.next_signal(cx, &mut self.stream, d).await? else {
            return Ok(None);
        };
        let msg = item.map_err(|e| call_error(&e))?;
        let header = msg.header();
        let member = header.member().map(|m| m.as_str());
        let device = || -> Result<String, DbusError> {
            let path: OwnedObjectPath = msg.body().deserialize().map_err(|e| call_error(&e))?;
            Ok(path.as_str().to_owned())
        };
        match member {
            Some("DeviceAdded") => Ok(Some(Event::Added(device()?))),
            Some("DeviceRemoved") => Ok(Some(Event::Removed(device()?))),
            Some("PropertiesChanged") => {
                let iface: String = match msg.body().deserialize::<(String, Props, Vec<String>)>() {
                    Ok((iface, _, _)) => iface,
                    Err(e) => return Err(call_error(&e)),
                };
                if iface == MANAGER_IFACE {
                    return Ok(Some(Event::ManagerChanged));
                }
                Ok((iface == DEVICE_IFACE)
                    .then(|| header.path().map(|p| Event::Changed(p.as_str().to_owned())))
                    .flatten())
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use asupersync::channel::watch;
    use asupersync::time;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::demand::Demand;
    use tl_model::settings::EngineSettings;

    use crate::source::{QueuedCommand, WorkerControl};

    const BAT_PATH: &str = "/org/freedesktop/UPower/devices/battery_BAT0";
    const MOUSE_PATH: &str = "/org/freedesktop/UPower/devices/mouse_hidpp_battery_0";

    fn fixture_sys() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/power-supply-bat0/sys")
    }

    fn props(entries: Vec<(&str, Value<'static>)>) -> Props {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).expect("owned value")))
            .collect()
    }

    fn discharging() -> Props {
        props(vec![
            ("Type", Value::U32(2)),
            ("PowerSupply", Value::Bool(true)),
            ("NativePath", Value::from("BAT0")),
            ("Vendor", Value::from("SMP")),
            ("Model", Value::from("5B10W51867")),
            ("Percentage", Value::F64(81.0)),
            ("Energy", Value::F64(40.5)),
            ("EnergyFull", Value::F64(50.0)),
            ("EnergyFullDesign", Value::F64(57.0)),
            ("EnergyRate", Value::F64(7.2)),
            ("State", Value::U32(2)),
            ("TimeToEmpty", Value::I64(20250)),
            ("ChargeCycles", Value::I32(312)),
            ("Technology", Value::U32(2)),
            ("Capacity", Value::F64(87.7)),
        ])
    }

    #[test]
    fn props_discharging() {
        let b = battery_from_props(&discharging()).expect("a battery");
        assert_eq!(&*b.state, "Discharging");
        assert_eq!(&*b.technology, "Lithium polymer");
        assert_eq!(b.time_to_empty_s, valid(20250));
        assert_eq!(b.time_to_full_s.validity, Validity::Unsupported);
        assert_eq!(b.charge_cycles, valid(312));
        assert_eq!(&*b.native_path, "BAT0");
        assert_eq!(b.percentage, valid(81.0));
        assert_eq!(b.energy_wh, valid(40.5));
        assert_eq!(b.capacity_pct, valid(87.7));
        // Missing on UPower before 1.90.
        assert_eq!(b.voltage_min_design_v.validity, Validity::Unsupported);
        assert_eq!(b.voltage_v.validity, Validity::Unsupported);
        assert_eq!(b.temperature_c.validity, Validity::Unsupported);
        assert!(b.charge_threshold.is_none());
        assert!(b.power_supply);
        assert_eq!(&*display_name(&b, 0), "SMP 5B10W51867");

        let mut zero = discharging();
        for (k, v) in [
            ("Energy", Value::F64(0.0)),
            ("Voltage", Value::F64(0.0)),
            ("ChargeCycles", Value::I32(-1)),
        ] {
            zero.insert(k.to_owned(), OwnedValue::try_from(v).expect("owned"));
        }
        let b = battery_from_props(&zero).expect("a battery");
        assert_eq!(b.energy_wh.validity, Validity::Unsupported);
        assert_eq!(b.voltage_v.validity, Validity::Unsupported);
        assert_eq!(b.charge_cycles.validity, Validity::Unsupported);
    }

    #[test]
    fn keep_ups_and_peripherals() {
        let mut ups = discharging();
        ups.insert("Type".into(), OwnedValue::from(3u32));
        let ups = battery_from_props(&ups).expect("a UPS");
        assert_eq!(&*ups.kind, "UPS");
        assert!(ups.power_supply);
        let mut mouse = discharging();
        mouse.insert("Type".into(), OwnedValue::from(5u32));
        mouse.insert("PowerSupply".into(), OwnedValue::from(false));
        let mouse = battery_from_props(&mouse).expect("a mouse");
        assert_eq!(&*mouse.kind, "Mouse");
        assert!(!mouse.power_supply);
        let mut line = discharging();
        line.insert("Type".into(), OwnedValue::from(1u32));
        assert!(battery_from_props(&line).is_none());
        assert!(battery_from_props(&Props::new()).is_none());
        let mut odd = discharging();
        odd.insert("Type".into(), OwnedValue::from(99u32));
        assert_eq!(&*battery_from_props(&odd).expect("kept").kind, "Unknown");
        assert_eq!(
            &*battery_from_props(&discharging()).expect("kept").kind,
            "Battery"
        );
        assert!(is_display_device(
            "/org/freedesktop/UPower/devices/DisplayDevice"
        ));
        assert!(!is_display_device(
            "/org/freedesktop/UPower/devices/battery_BAT0"
        ));
    }

    #[test]
    fn charge_threshold() {
        let mut p = discharging();
        for (k, v) in [
            ("ChargeThresholdSupported", Value::Bool(true)),
            ("ChargeThresholdEnabled", Value::Bool(true)),
            ("ChargeStartThreshold", Value::U32(40)),
            ("ChargeEndThreshold", Value::U32(80)),
        ] {
            p.insert(k.to_owned(), OwnedValue::try_from(v).expect("owned"));
        }
        let b = battery_from_props(&p).expect("a battery");
        assert_eq!(b.charge_threshold, Some((true, 40, 80)));

        p.insert("ChargeThresholdSupported".into(), OwnedValue::from(false));
        let b = battery_from_props(&p).expect("a battery");
        assert_eq!(b.charge_threshold, None);
    }

    #[test]
    fn history_points_cleanup() {
        let raw = [
            (1_759_500_300, 80.0, 2),
            (1_759_500_000, 81.0, 2),
            (0, 50.0, 0),
            (1_759_500_300, 79.5, 2),
            (1_759_500_600, 140.0, 2),
        ];
        let want: &[(i64, f32)] = &[(1_759_500_000, 81.0), (1_759_500_300, 79.5)];
        assert_eq!(&*history_points(&raw), want);
        assert!(history_points(&[(5, f64::NAN, 0), (6, -1.0, 0)]).is_empty());
        assert!(history_points(&[]).is_empty());
    }

    #[test]
    fn sysfs_energy_files() {
        let dir = fixture_sys().join("class/power_supply");
        let b = battery_from_sysfs(&dir.join("BAT0")).expect("a battery");
        assert_eq!(b.percentage, valid(64.0));
        assert_eq!(b.energy_wh, valid(32.0));
        assert_eq!(b.energy_full_wh, valid(50.0));
        assert_eq!(b.energy_rate_w, valid(6.5));
        assert_eq!(&*b.technology, "Lithium ion");
        assert_eq!(&*b.state, "Discharging");
        assert_eq!(&*b.native_path, "BAT0");
        assert_eq!(b.charge_cycles, valid(312));
        assert_eq!(b.charge_threshold, Some((true, 40, 80)));
        assert_eq!(b.time_to_empty_s.validity, Validity::Unsupported);
        assert_eq!(b.time_to_full_s.validity, Validity::Unsupported);
        assert!(b.power_supply);
        assert!(battery_from_sysfs(&dir.join("AC")).is_none());

        let all = sysfs_batteries(&dir);
        assert_eq!(all.len(), 1);
        assert_eq!(&*all[0].native_path, "BAT0");
    }

    /// A fake `power_supply` directory under `target/tmp`, removed on drop.
    fn fake_supply(files: &[(&str, &str, &str)]) -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&base).expect("create target/tmp");
        let dir = tempfile::Builder::new()
            .prefix("tl-engine-upower-")
            .tempdir_in(base)
            .expect("tempdir");
        for (sub, name, value) in files {
            let d = dir.path().join(sub);
            std::fs::create_dir_all(&d).expect("create dir");
            std::fs::write(d.join(name), format!("{value}\n")).expect("write");
        }
        dir
    }

    #[test]
    fn sysfs_charge_files() {
        let dir = fake_supply(&[
            ("BAT1", "type", "Battery"),
            ("BAT1", "charge_now", "3000000"),
            ("BAT1", "charge_full", "4000000"),
            ("BAT1", "charge_full_design", "4400000"),
            ("BAT1", "voltage_min_design", "11400000"),
            ("BAT1", "technology", "Li-poly"),
            ("hidpp_battery_0", "type", "Battery"),
            ("hidpp_battery_0", "scope", "Device"),
            ("hidpp_battery_0", "capacity", "50"),
        ]);
        let b = battery_from_sysfs(&dir.path().join("BAT1")).expect("a battery");
        let close = |f: InvField<f32>, want: f32| {
            assert_eq!(f.validity, Validity::Valid);
            assert!((f.value - want).abs() < 1e-3, "{} != {want}", f.value);
        };
        close(b.energy_wh, 34.2);
        close(b.energy_full_wh, 45.6);
        close(b.energy_full_design_wh, 50.16);
        close(b.voltage_min_design_v, 11.4);
        assert_eq!(&*b.technology, "Lithium polymer");
        assert_eq!(&*b.state, "Unknown");
        assert_eq!(b.percentage.validity, Validity::Unsupported);
        assert_eq!(b.energy_rate_w.validity, Validity::Unsupported);
        assert_eq!(b.charge_cycles.validity, Validity::Unsupported);
        assert_eq!(b.charge_threshold, None);
        assert_eq!(&*display_name(&b, 1), "Battery 2");

        let mouse = battery_from_sysfs(&dir.path().join("hidpp_battery_0")).expect("read");
        assert!(!mouse.power_supply);
        assert_eq!(&*mouse.kind, "Other");
        let all = sysfs_batteries(dir.path());
        assert_eq!(all.len(), 2, "the peripheral battery is kept");
        assert_eq!(&*all[0].native_path, "BAT1");
        assert_eq!(&*all[0].kind, "Battery");
        assert_eq!(&*all[1].native_path, "hidpp_battery_0");
        assert_eq!(&*all[1].kind, "Other");
    }

    // ---- The worker loop over a fake bus ----

    #[derive(Default)]
    struct FakeState {
        /// `None`: UPower is not on the bus.
        devices: Option<BTreeMap<String, Props>>,
        events: VecDeque<Event>,
        opens: u32,
        /// The `GetHistory` reply. `None`: the call fails.
        history: Option<Vec<(u32, f64, u32)>>,
        /// The device path of each `GetHistory` call.
        history_calls: Vec<String>,
        /// The manager's `OnBattery`. `None`: the call fails.
        on_battery: Option<bool>,
    }

    #[derive(Clone, Default)]
    struct FakeConnector(Arc<Mutex<FakeState>>);

    struct FakeBus(Arc<Mutex<FakeState>>);

    fn service_unknown() -> DbusError {
        DbusError::Call {
            name: "org.freedesktop.DBus.Error.ServiceUnknown".into(),
            message: "not activatable".into(),
        }
    }

    impl Connector for FakeConnector {
        type Bus = FakeBus;

        async fn open(&self, _cx: &Cx) -> Result<FakeBus, DbusError> {
            let mut state = self.0.lock().expect("fake lock");
            state.opens += 1;
            if state.devices.is_none() {
                return Err(service_unknown());
            }
            Ok(FakeBus(Arc::clone(&self.0)))
        }
    }

    impl UpowerBus for FakeBus {
        async fn devices(&mut self, _cx: &Cx) -> Result<Vec<String>, DbusError> {
            let state = self.0.lock().expect("fake lock");
            let devices = state.devices.as_ref().ok_or_else(service_unknown)?;
            Ok(devices.keys().cloned().collect())
        }

        async fn props(&mut self, _cx: &Cx, path: &str) -> Result<Props, DbusError> {
            let state = self.0.lock().expect("fake lock");
            let devices = state.devices.as_ref().ok_or_else(service_unknown)?;
            devices.get(path).cloned().ok_or_else(|| DbusError::Call {
                name: "org.freedesktop.DBus.Error.UnknownObject".into(),
                message: "gone".into(),
            })
        }

        async fn next_event(&mut self, cx: &Cx, d: Duration) -> Result<Option<Event>, DbusError> {
            let event = self.0.lock().expect("fake lock").events.pop_front();
            if event.is_some() {
                return Ok(event);
            }
            let _ = time::timeout(cx.now(), d, std::future::pending::<()>()).await;
            // Same as `DbusConn::next_signal`: a cancelled `cx` ends the stream.
            if cx.checkpoint().is_err() {
                return Err(DbusError::Closed);
            }
            Ok(None)
        }

        async fn on_battery(&mut self, _cx: &Cx) -> Result<bool, DbusError> {
            let state = self.0.lock().expect("fake lock");
            state.on_battery.ok_or_else(|| DbusError::Call {
                name: "org.freedesktop.DBus.Error.UnknownProperty".into(),
                message: "no OnBattery".into(),
            })
        }

        async fn history(
            &mut self,
            _cx: &Cx,
            path: &str,
        ) -> Result<Vec<(u32, f64, u32)>, DbusError> {
            let mut state = self.0.lock().expect("fake lock");
            state.history_calls.push(path.to_owned());
            state.history.clone().ok_or_else(|| DbusError::Call {
                name: "org.freedesktop.DBus.Error.Failed".into(),
                message: "no history".into(),
            })
        }
    }

    struct Rig {
        lab: LabRuntime,
        rx: mpsc::Receiver<EngineMsg>,
        _demand_tx: watch::Sender<Demand>,
        _settings_tx: watch::Sender<EngineSettings>,
        control_tx: watch::Sender<WorkerControl>,
        handle: asupersync::runtime::TaskHandle<()>,
    }

    impl Rig {
        /// Runs `run_with` over `connector` with a visible surface and the sysfs fixture.
        fn start(connector: FakeConnector) -> Self {
            let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
            let (updates, rx) = mpsc::channel::<EngineMsg>(256);
            let demand = Demand {
                any_surface_visible: true,
                ..Demand::default()
            };
            let (demand_tx, demand) = watch::channel(demand);
            let (settings_tx, settings) = watch::channel(EngineSettings::default());
            let (control_tx, control) = watch::channel(WorkerControl::default());
            let io = WorkerIo::new(
                updates,
                demand,
                settings,
                control,
                crate::dbus::SharedHandles::default(),
            );
            let root = lab.state.create_root_region(Budget::INFINITE);
            let (task, handle) = lab
                .state
                .create_task(root, Budget::INFINITE, async move {
                    let cx = Cx::current().expect("task has a Cx");
                    let _ = run_with(cx, io, fixture_sys(), connector).await;
                })
                .expect("task is created");
            lab.scheduler.lock().schedule(task, 0);
            lab.run_until_idle();
            Rig {
                lab,
                rx,
                _demand_tx: demand_tx,
                _settings_tx: settings_tx,
                control_tx,
                handle,
            }
        }

        fn advance(&mut self, d: Duration) {
            self.lab.advance_time(nanos(d));
            // `run_until_idle` stops when no task is ready; one step fires the due timers.
            self.lab.step_for_test();
            self.lab.run_until_idle();
        }

        /// Queues `BatteryHistory { native_path }` as the reducer does. Returns its `seq`.
        fn ask_history(&self, native_path: &str) -> u64 {
            let mut seq = 0;
            self.control_tx
                .send_modify(|c| {
                    seq = c.commands.last().map_or(0, |q| q.seq) + 1;
                    let mut next = c.commands.to_vec();
                    next.push(QueuedCommand {
                        seq,
                        target: SourceId::UPower,
                        command: WorkerCommand::Device(DeviceCommand::BatteryHistory {
                            native_path: native_path.into(),
                        }),
                    });
                    c.commands = next.into();
                })
                .expect("control send");
            seq
        }

        fn progress_done(&self) -> u64 {
            self.control_tx.borrow().progress.done(SourceId::UPower)
        }

        fn drain(&mut self) -> Vec<SourceUpdate> {
            let mut out = Vec::new();
            while let Ok(EngineMsg::Source(update)) = self.rx.try_recv() {
                out.push(update);
            }
            out
        }

        fn finish(mut self) {
            self.handle.abort();
            self.advance(IDLE_RECHECK);
            let report = self.lab.run_until_quiescent_with_report();
            assert!(report.lab_test_passed(), "{report:?}");
        }
    }

    fn on_battery_of(update: &SourceUpdate) -> Option<Option<bool>> {
        match update {
            SourceUpdate::DeviceDetails(DeviceDetailsUpdate::OnBattery(on)) => Some(*on),
            _ => None,
        }
    }

    #[test]
    fn on_battery_follows_upower() {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        {
            let mut state = fake.lock().expect("fake lock");
            state.devices = Some(BTreeMap::from([(BAT_PATH.to_owned(), discharging())]));
            state.on_battery = Some(true);
        }
        let mut rig = Rig::start(connector);
        let first = rig.drain();
        let ons: Vec<_> = first.iter().filter_map(on_battery_of).collect();
        assert_eq!(ons, [Some(true)], "{first:?}");

        // The charger is plugged in: the manager's PropertiesChanged.
        {
            let mut state = fake.lock().expect("fake lock");
            state.on_battery = Some(false);
            state.events.push_back(Event::ManagerChanged);
        }
        rig.advance(IDLE_RECHECK);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "only OnBattery: {got:?}");
        assert_eq!(on_battery_of(&got[0]), Some(Some(false)));

        // An unchanged flag sends nothing.
        fake.lock()
            .expect("fake lock")
            .events
            .push_back(Event::ManagerChanged);
        rig.advance(IDLE_RECHECK);
        assert!(rig.drain().is_empty());
        rig.finish();
    }

    #[test]
    fn on_battery_none_without_battery() {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        {
            let mut state = fake.lock().expect("fake lock");
            state.devices = Some(BTreeMap::new());
            state.on_battery = Some(true);
        }
        let mut rig = Rig::start(connector);
        let first = rig.drain();
        let ons: Vec<_> = first.iter().filter_map(on_battery_of).collect();
        assert_eq!(ons, [None], "no battery: {first:?}");
        rig.finish();
    }

    #[test]
    fn sysfs_supply_online_mains_and_usb() {
        let ac_off = fake_supply(&[("AC", "type", "Mains"), ("AC", "online", "0")]);
        assert_eq!(sysfs_supply_online(ac_off.path()), Some(false));
        let usb_on = fake_supply(&[
            ("AC", "type", "Mains"),
            ("AC", "online", "0"),
            ("ucsi-source-psy-USBC000:001", "type", "USB"),
            ("ucsi-source-psy-USBC000:001", "online", "1"),
        ]);
        assert_eq!(
            sysfs_supply_online(usb_on.path()),
            Some(true),
            "a USB-C charger"
        );
        let none = fake_supply(&[("BAT0", "type", "Battery")]);
        assert_eq!(sysfs_supply_online(none.path()), None);
        assert_eq!(on_battery(true, Some(false).map(|o: bool| !o)), Some(true));
        assert_eq!(on_battery(false, Some(true)), None, "no battery");
    }

    fn batteries(update: &SourceUpdate) -> Option<&[(DeviceIdentity, BatteryDetails)]> {
        match update {
            SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Batteries(b)) => Some(b),
            _ => None,
        }
    }

    #[test]
    fn fallback_every_30s() {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        let mut rig = Rig::start(connector);

        let first = rig.drain();
        assert_eq!(first.len(), 4, "{first:?}");
        assert_eq!(
            on_battery_of(&first[3]),
            Some(Some(true)),
            "the fixture's AC is offline"
        );
        assert!(
            matches!(&first[0], SourceUpdate::Health {
                id: SourceId::UPower,
                state: SourceState::Running,
                last_error: Some(text),
                ..
            } if &**text == NOT_AVAILABLE),
            "{:?}",
            first[0]
        );
        match &first[1] {
            SourceUpdate::Devices(list) => {
                assert_eq!(list.kinds, DeviceKinds::BATTERIES);
                assert_eq!(list.entries.len(), 1);
                let entry = &list.entries[0];
                assert_eq!(&*entry.display_name, "SMP 5B10W51867");
                assert_eq!(&*entry.persistent_key, "bat:BAT0");
            }
            other => panic!("expected Devices, got {other:?}"),
        }
        let got = batteries(&first[2]).expect("Batteries");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1.percentage, valid(64.0));

        let opens = |fake: &Arc<Mutex<FakeState>>| fake.lock().expect("fake lock").opens;
        assert_eq!(opens(&fake), 1);
        for round in 1..=4 {
            for _ in 0..59 {
                rig.advance(IDLE_RECHECK);
                assert!(rig.drain().is_empty(), "round {round}: nothing before 30 s");
            }
            rig.advance(IDLE_RECHECK);
            let got = rig.drain();
            assert_eq!(got.len(), 1, "round {round}: {got:?}");
            assert!(batteries(&got[0]).is_some(), "round {round}: {got:?}");
        }
        // The bus is retried every 60 s.
        assert_eq!(opens(&fake), 3);
        rig.finish();
    }

    #[test]
    fn properties_changed_resends() {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        let mut mouse = discharging();
        mouse.insert("PowerSupply".into(), OwnedValue::from(false));
        fake.lock().expect("fake lock").devices = Some(BTreeMap::from([
            (BAT_PATH.to_owned(), discharging()),
            (MOUSE_PATH.to_owned(), mouse),
        ]));
        let mut rig = Rig::start(connector);

        let first = rig.drain();
        assert_eq!(first.len(), 4, "{first:?}");
        assert_eq!(
            on_battery_of(&first[3]),
            Some(None),
            "no OnBattery reply: unknown"
        );
        assert!(
            matches!(
                &first[0],
                SourceUpdate::Health {
                    state: SourceState::Running,
                    last_error: None,
                    ..
                }
            ),
            "{:?}",
            first[0]
        );
        assert!(
            matches!(&first[1], SourceUpdate::Devices(list) if list.entries.len() == 1),
            "{:?}",
            first[1]
        );
        assert_eq!(
            batteries(&first[2]).expect("Batteries")[0].1.percentage,
            valid(81.0)
        );

        {
            let mut state = fake.lock().expect("fake lock");
            let devices = state.devices.as_mut().expect("devices");
            let bat = devices.get_mut(BAT_PATH).expect("battery");
            bat.insert("Percentage".into(), OwnedValue::from(80.0f64));
            state
                .events
                .push_back(Event::Changed(MOUSE_PATH.to_owned()));
            state.events.push_back(Event::Changed(BAT_PATH.to_owned()));
        }
        rig.advance(IDLE_RECHECK);
        rig.advance(IDLE_RECHECK);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "one Batteries and no Devices: {got:?}");
        let got = batteries(&got[0]).expect("Batteries");
        assert_eq!(got[0].1.percentage, valid(80.0));

        // A removed battery changes the set.
        fake.lock()
            .expect("fake lock")
            .events
            .push_back(Event::Removed(BAT_PATH.to_owned()));
        rig.advance(IDLE_RECHECK);
        let got = rig.drain();
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(
            matches!(&got[0], SourceUpdate::Devices(list) if list.entries.is_empty()),
            "{got:?}"
        );
        assert_eq!(batteries(&got[1]).map(<[_]>::len), Some(0));
        rig.finish();
    }

    /// A rig over one UPower battery at `BAT_PATH`, with the first updates drained.
    fn history_rig(history: Option<Vec<(u32, f64, u32)>>) -> (Rig, Arc<Mutex<FakeState>>) {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        {
            let mut state = fake.lock().expect("fake lock");
            state.devices = Some(BTreeMap::from([(BAT_PATH.to_owned(), discharging())]));
            state.history = history;
        }
        let mut rig = Rig::start(connector);
        assert_eq!(
            rig.drain().len(),
            4,
            "Health, Devices, Batteries, OnBattery"
        );
        (rig, fake)
    }

    #[test]
    fn history_command_fills() {
        let (mut rig, fake) = history_rig(Some(vec![
            (1_759_500_000, 81.0, 2),
            (1_759_500_300, 80.0, 2),
            (1_759_500_600, 79.0, 2),
        ]));
        let seq = rig.ask_history("BAT0");
        rig.advance(IDLE_RECHECK);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "one Batteries: {got:?}");
        let got = batteries(&got[0]).expect("Batteries");
        let history = got[0].1.history.as_deref().expect("history");
        assert_eq!(history.len(), 3);
        assert_eq!(history[0], (1_759_500_000, 81.0));
        assert_eq!(
            fake.lock().expect("fake lock").history_calls,
            vec![BAT_PATH.to_owned()]
        );
        assert_eq!(rig.progress_done(), seq);

        // A property change keeps the cached history.
        {
            let mut state = fake.lock().expect("fake lock");
            let devices = state.devices.as_mut().expect("devices");
            let bat = devices.get_mut(BAT_PATH).expect("battery");
            bat.insert("Percentage".into(), OwnedValue::from(78.0f64));
            state.events.push_back(Event::Changed(BAT_PATH.to_owned()));
        }
        rig.advance(IDLE_RECHECK);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "{got:?}");
        let got = batteries(&got[0]).expect("Batteries");
        assert_eq!(got[0].1.percentage, valid(78.0));
        assert_eq!(got[0].1.history.as_ref().map(|h| h.len()), Some(3));
        assert_eq!(fake.lock().expect("fake lock").history_calls.len(), 1);
        rig.finish();
    }

    #[test]
    fn history_error_empty() {
        let (mut rig, _fake) = history_rig(None);
        rig.ask_history("BAT0");
        rig.advance(IDLE_RECHECK);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "one Batteries and no Health: {got:?}");
        let got = batteries(&got[0]).expect("Batteries");
        assert_eq!(got[0].1.history.as_deref(), Some(&[][..]));
        rig.finish();
    }

    #[test]
    fn history_unknown_path_ignored() {
        let (mut rig, fake) = history_rig(Some(vec![(1_759_500_000, 81.0, 2)]));
        let seq = rig.ask_history("BAT9");
        rig.advance(IDLE_RECHECK);
        rig.advance(IDLE_RECHECK);
        assert!(rig.drain().is_empty(), "no update for an unknown battery");
        assert!(fake.lock().expect("fake lock").history_calls.is_empty());
        assert_eq!(rig.progress_done(), seq);
        rig.finish();
    }

    #[test]
    fn idle_until_demand() {
        let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
        let (updates, mut rx) = mpsc::channel::<EngineMsg>(256);
        let (demand_tx, demand) = watch::channel(Demand::default());
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (_control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(
            updates,
            demand,
            settings,
            control,
            crate::dbus::SharedHandles::default(),
        );
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let _ = run_with(cx, io, fixture_sys(), connector).await;
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();
        let advance = |lab: &mut LabRuntime| {
            lab.advance_time(nanos(IDLE_RECHECK));
            lab.step_for_test();
            lab.run_until_idle();
        };
        for _ in 0..10 {
            advance(&mut lab);
        }
        assert!(rx.try_recv().is_err(), "idle sends nothing");
        assert_eq!(fake.lock().expect("fake lock").opens, 0);

        let recording = Demand {
            recording: true,
            ..Demand::default()
        };
        demand_tx.send(recording).expect("demand send");
        advance(&mut lab);
        assert!(rx.try_recv().is_ok(), "recording wakes the worker");
        assert_eq!(fake.lock().expect("fake lock").opens, 1);

        handle.abort();
        advance(&mut lab);
        let report = lab.run_until_quiescent_with_report();
        assert!(report.lab_test_passed(), "{report:?}");
    }

    #[test]
    fn spec_is_full_mode_only() {
        let s = spec();
        assert_eq!(s.id, SourceId::UPower);
        assert_eq!(s.name, "upower");
        assert_eq!(s.modes, &[EngineMode::Full]);
    }

    // ---- The zbus session over a peer-to-peer fake ----

    struct FakeManager;

    #[zbus::interface(name = "org.freedesktop.UPower")]
    impl FakeManager {
        fn enumerate_devices(&self) -> Vec<OwnedObjectPath> {
            vec![OwnedObjectPath::try_from(BAT_PATH).expect("object path")]
        }
    }

    struct FakeDevice {
        pct: f64,
    }

    #[zbus::interface(name = "org.freedesktop.UPower.Device")]
    impl FakeDevice {
        #[zbus(property, name = "Type")]
        fn kind(&self) -> u32 {
            2
        }

        #[zbus(property)]
        fn power_supply(&self) -> bool {
            true
        }

        #[zbus(property)]
        fn native_path(&self) -> String {
            "BAT0".to_owned()
        }

        #[zbus(property)]
        fn percentage(&self) -> f64 {
            self.pct
        }

        fn get_history(
            &self,
            kind: String,
            timespan: u32,
            resolution: u32,
        ) -> Vec<(u32, f64, u32)> {
            if (kind.as_str(), timespan, resolution) == ("charge", HISTORY_SPAN_S, HISTORY_POINTS) {
                vec![(1_759_500_000, self.pct, 2)]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn upower_bus_over_p2p() {
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let server = zbus::connection::Builder::async_io_unix_stream(a)
            .server(zbus::Guid::generate())
            .expect("server builder")
            .p2p()
            .serve_at(PATH, FakeManager)
            .expect("manager")
            .serve_at(BAT_PATH, FakeDevice { pct: 81.0 })
            .expect("device")
            .build();
        let client = zbus::connection::Builder::async_io_unix_stream(b)
            .p2p()
            .internal_executor(false)
            .build();
        let (server, client) =
            futures_lite::future::block_on(futures_lite::future::zip(server, client));
        let server = server.expect("server connects");
        let client = client.expect("client connects");

        let rt = crate::runtime::build_runtime().expect("runtime builds");
        let join = rt.handle().spawn_checked(async move {
            let cx = Cx::current().expect("task has a Cx");
            let conn = DbusConn::p2p(&cx, client).expect("p2p connection");
            let mut bus = SystemUpower::on(&cx, conn).await.expect("subscribed");
            let found = resync(&cx, &mut bus).await.expect("resync");

            let iface = server
                .object_server()
                .interface::<_, FakeDevice>(BAT_PATH)
                .await
                .expect("device interface");
            iface.get_mut().await.pct = 80.0;
            iface
                .get()
                .await
                .percentage_changed(iface.signal_emitter())
                .await
                .expect("signal sent");
            let mut event = None;
            for _ in 0..10 {
                event = bus.next_event(&cx, IDLE_RECHECK).await.expect("signal");
                if event.is_some() {
                    break;
                }
            }
            let reread = bus.props(&cx, BAT_PATH).await.expect("GetAll");
            let history = bus.history(&cx, BAT_PATH).await.expect("GetHistory");
            drop(bus);
            drop(server);
            (found, event, battery_from_props(&reread), history)
        });
        let (found, event, reread, history) = rt.block_on(join).expect("task completes");
        assert_eq!(history, vec![(1_759_500_000, 80.0, 2)]);

        assert_eq!(found.len(), 1);
        let bat = found.get(BAT_PATH).expect("battery");
        assert_eq!(&*bat.native_path, "BAT0");
        assert_eq!(bat.percentage, valid(81.0));
        assert_eq!(event, Some(Event::Changed(BAT_PATH.to_owned())));
        assert_eq!(reread.expect("battery").percentage, valid(80.0));
    }
}
