//! The `Udisks` worker (ADR-004, ADR-016, PERF-049 to PERF-053): drive properties from UDisks2.
//!
//! While disks are demanded it lists every object with `GetManagedObjects`, then follows
//! `InterfacesAdded`, `InterfacesRemoved` and `PropertiesChanged` under `ROOT`, with a full
//! resync every 60 s. Each change sends the complete `Disks { from: Udisks }` list, and only when
//! it differs from the last one. The reducer takes `model`, `wwn`, `serial`, `rotation_rpm`,
//! `smart`, `smart_temp_c`, `ejectable` and `can_power_off` from these records (R-139 item 7).
//!
//! Read-only: the worker only calls `GetManagedObjects`. It never calls a drive or SMART method,
//! so it never wakes a sleeping disk. Serials and WWNs are shown on screen only. Never log them.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::sleep;
use asupersync::{Cx, Outcome};
use tl_collect::sanitize::sanitize_text;
use tl_model::devices::DeviceIdentity;
use tl_model::health::{SourceId, SourceState};
use tl_model::snapshot::{DiskDetails, InvField, SmartKind};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{Bus, DbusConn, DbusError, READ_TIMEOUT, connect};
use crate::source::{IDLE_RECHECK, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{DeviceDetailsUpdate, EngineMsg, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;

pub const BUS: &str = "org.freedesktop.UDisks2";
pub const ROOT: &str = "/org/freedesktop/UDisks2";

const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";
const BLOCK_IFACE: &str = "org.freedesktop.UDisks2.Block";
const PARTITION_IFACE: &str = "org.freedesktop.UDisks2.Partition";
const DRIVE_IFACE: &str = "org.freedesktop.UDisks2.Drive";
const ATA_IFACE: &str = "org.freedesktop.UDisks2.Drive.Ata";
const NVME_CONTROLLER_IFACE: &str = "org.freedesktop.UDisks2.NVMe.Controller";
/// The full resync period with UDisks2, and the retry period without it.
const RESYNC: Duration = Duration::from_secs(60);
/// How long one wait for a signal lasts before the worker re-reads demand.
const SIGNAL_WAIT: Duration = READ_TIMEOUT;
/// The health text when UDisks2 is not on the bus.
const NOT_AVAILABLE: &str = "UDisks2 not available";
/// Longest model, serial or WWN kept, in bytes.
const TEXT_MAX: usize = 256;
/// 0 °C in Kelvin.
const KELVIN_ZERO: f64 = 273.15;

/// The properties of one interface, by name.
pub type Props = BTreeMap<String, OwnedValue>;
/// The interfaces of one object, by name.
pub type Interfaces = BTreeMap<String, Props>;

/// `GetManagedObjects` data, keyed by object path. The key is the path as text because
/// `OwnedObjectPath` has no `Ord`.
#[derive(Clone, Debug, Default)]
pub struct ManagedObjects(pub BTreeMap<String, Interfaces>);

pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Udisks,
        name: "udisks",
        modes: &[EngineMode::Full],
        factory,
    }
}

pub fn factory(_cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| run(cx, io.clone()))
}

/// The worker on the system bus.
#[doc(hidden)]
pub async fn run(cx: Cx, io: WorkerIo) -> Outcome<(), WorkerError> {
    run_with(cx, io, SystemConnector).await
}

/// Where a bus session ends.
enum Next {
    Idle,
    Missing,
    Stop(Outcome<(), WorkerError>),
}

/// Idle, then UDisks2 or the missing-service wait. Returns `Outcome::Ok(())` on cancel and
/// `Err` when the bus fails or the reducer's queue is closed, so the supervisor restarts it.
async fn run_with<C: Connector>(cx: Cx, io: WorkerIo, connector: C) -> Outcome<(), WorkerError> {
    let mut last = None;
    loop {
        if let Some(stop) = idle(&cx, &io).await {
            return stop;
        }
        let next = match start_bus(&cx, &connector).await {
            Ok((bus, objs)) => bus_loop(&cx, &io, bus, objs, &mut last).await,
            Err(e) if unavailable(&e) => Next::Missing,
            Err(e) => fail(&cx, &e),
        };
        match next {
            Next::Idle => {}
            Next::Missing => {
                if let Some(stop) = missing(&cx, &io, &mut last).await {
                    return stop;
                }
            }
            Next::Stop(outcome) => return outcome,
        }
    }
}

/// Step 1: waits until disks are demanded or a command for this worker is queued. `Some` on
/// cancel.
async fn idle(cx: &Cx, io: &WorkerIo) -> Option<Outcome<(), WorkerError>> {
    while !active(io) {
        sleep(cx.now(), IDLE_RECHECK).await;
        if cx.checkpoint().is_err() {
            return Some(Outcome::Ok(()));
        }
    }
    None
}

/// Disks are demanded, or a `Device` command for this worker (E12-021, E12-022) is queued
/// after the last one a generation started.
fn active(io: &WorkerIo) -> bool {
    if io.demand.borrow_and_clone().disks {
        return true;
    }
    let control = io.control.borrow_and_clone();
    let started = control.progress.started(SourceId::Udisks);
    control
        .commands
        .iter()
        .any(|c| c.target == SourceId::Udisks && c.seq > started)
}

/// Opens the bus, subscribes and lists every object (steps 2 and 3). The subscription comes
/// first, so no change between the listing and the first signal is lost.
async fn start_bus<C: Connector>(
    cx: &Cx,
    connector: &C,
) -> Result<(C::Bus, ManagedObjects), DbusError> {
    let mut bus = connector.open(cx).await?;
    let objs = bus.managed_objects(cx).await?;
    Ok((bus, objs))
}

/// Step 2 without UDisks2: `Failed` health and an empty list, then a 60 s wait. `Some` on
/// cancel or a closed queue. Returns early when demand ends.
async fn missing(
    cx: &Cx,
    io: &WorkerIo,
    last: &mut Option<Vec<(DeviceIdentity, DiskDetails)>>,
) -> Option<Outcome<(), WorkerError>> {
    let failed = health(SourceState::Failed, Some(NOT_AVAILABLE.into()));
    if let Some(stop) = send(cx, io, failed).await {
        return Some(stop);
    }
    if let Some(stop) = send(cx, io, disks(Vec::new())).await {
        return Some(stop);
    }
    *last = Some(Vec::new());
    let retry = cx.now().as_nanos() + nanos(RESYNC);
    while cx.now().as_nanos() < retry {
        sleep(cx.now(), IDLE_RECHECK).await;
        if cx.checkpoint().is_err() {
            return Some(Outcome::Ok(()));
        }
        if !active(io) {
            return None;
        }
    }
    None
}

/// Steps 3 and 4 with UDisks2.
async fn bus_loop<B: UdisksBus>(
    cx: &Cx,
    io: &WorkerIo,
    mut bus: B,
    mut objs: ManagedObjects,
    last: &mut Option<Vec<(DeviceIdentity, DiskDetails)>>,
) -> Next {
    if let Some(stop) = send(cx, io, health(SourceState::Running, None)).await {
        return Next::Stop(stop);
    }
    // A new session always sends its first listing.
    *last = None;
    if let Some(stop) = publish(cx, io, &objs, last).await {
        return Next::Stop(stop);
    }
    let mut next_resync = cx.now().as_nanos() + nanos(RESYNC);
    loop {
        if !active(io) {
            return Next::Idle;
        }
        let mut resync_now = match bus.next_event(cx, SIGNAL_WAIT).await {
            Ok(Some(event)) => apply_event(&mut objs, event),
            Ok(None) => {
                if cx.checkpoint().is_err() {
                    return Next::Stop(Outcome::Ok(()));
                }
                false
            }
            Err(e) => return fail(cx, &e),
        };
        if cx.now().as_nanos() >= next_resync {
            resync_now = true;
        }
        if resync_now {
            next_resync = cx.now().as_nanos() + nanos(RESYNC);
            match bus.managed_objects(cx).await {
                Ok(fresh) => objs = fresh,
                Err(e) if unavailable(&e) => return Next::Missing,
                Err(e) => return fail(cx, &e),
            }
        }
        if let Some(stop) = publish(cx, io, &objs, last).await {
            return Next::Stop(stop);
        }
    }
}

/// Applies one signal to the cached objects. Returns true when a full resync is needed: the
/// stream lost a message, or a cached property was invalidated without a value.
fn apply_event(objs: &mut ManagedObjects, event: Event) -> bool {
    match event {
        Event::Added { path, interfaces } => {
            objs.0.entry(path).or_default().extend(interfaces);
            false
        }
        Event::Removed { path, interfaces } => {
            if let Some(obj) = objs.0.get_mut(&path) {
                for name in &interfaces {
                    obj.remove(name);
                }
                if obj.is_empty() {
                    objs.0.remove(&path);
                }
            }
            false
        }
        Event::Changed {
            path,
            interface,
            changed,
            invalidated,
        } => {
            // A change of an object or interface not in the cache waits for the next resync.
            let Some(props) = objs.0.get_mut(&path).and_then(|o| o.get_mut(&interface)) else {
                return false;
            };
            props.extend(changed);
            !invalidated.is_empty()
        }
        Event::Lost => true,
    }
}

/// Sends the drive list when it differs from `last`.
async fn publish(
    cx: &Cx,
    io: &WorkerIo,
    objs: &ManagedObjects,
    last: &mut Option<Vec<(DeviceIdentity, DiskDetails)>>,
) -> Option<Outcome<(), WorkerError>> {
    let entries = drives_from_objects(objs);
    if last
        .as_deref()
        .is_some_and(|prev| same_list(prev, &entries))
    {
        return None;
    }
    *last = Some(entries.clone());
    send(cx, io, disks(entries)).await
}

/// Compares the fields this worker sets. `DiskDetails` has no `PartialEq`.
fn same_list(a: &[(DeviceIdentity, DiskDetails)], b: &[(DeviceIdentity, DiskDetails)]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((ia, da), (ib, db))| {
            ia == ib
                && da.model == db.model
                && da.wwn == db.wwn
                && da.serial == db.serial
                && da.rotation_rpm == db.rotation_rpm
                && da.smart == db.smart
                && da.smart_temp_c == db.smart_temp_c
                && da.ejectable == db.ejectable
                && da.can_power_off == db.can_power_off
        })
}

fn disks(entries: Vec<(DeviceIdentity, DiskDetails)>) -> SourceUpdate {
    SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks {
        from: SourceId::Udisks,
        entries,
    })
}

fn health(state: SourceState, last_error: Option<Arc<str>>) -> SourceUpdate {
    SourceUpdate::Health {
        id: SourceId::Udisks,
        state,
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
fn fail(cx: &Cx, e: &DbusError) -> Next {
    if cx.checkpoint().is_err() {
        return Next::Stop(Outcome::Ok(()));
    }
    tracing::debug!(error = dbus_name(e), "UDisks2 bus failed");
    Next::Stop(Outcome::Err(WorkerError::Dbus(dbus_name(e).into())))
}

/// UDisks2 is not on the bus.
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

// ---- Building `DiskDetails` ----

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

fn flag(p: Option<&Props>, key: &str) -> Option<bool> {
    match &**p?.get(key)? {
        Value::Bool(v) => Some(*v),
        _ => None,
    }
}

/// A non-empty string property, sanitized and cut at `TEXT_MAX`.
fn text(p: Option<&Props>, key: &str) -> InvField<Arc<str>> {
    let Some(Value::Str(s)) = p.and_then(|p| p.get(key)).map(|v| &**v) else {
        return unsupported();
    };
    let mut clean = String::new();
    sanitize_text(s.as_bytes(), TEXT_MAX, &mut clean);
    opt((!clean.is_empty()).then(|| Arc::from(clean)))
}

fn object_path<'a>(p: &'a Props, key: &str) -> Option<&'a str> {
    match &**p.get(key)? {
        Value::ObjectPath(path) => Some(path.as_str()),
        _ => None,
    }
}

/// The basename of the `Device` byte string, without its trailing NUL.
fn device_name(p: &Props) -> Option<Arc<str>> {
    let Value::Array(array) = &**p.get("Device")? else {
        return None;
    };
    let mut bytes = Vec::with_capacity(array.len());
    for item in array.iter() {
        match item {
            Value::U8(b) => bytes.push(*b),
            _ => return None,
        }
    }
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    let path = std::str::from_utf8(&bytes).ok()?;
    let name = path.rsplit('/').next()?;
    (!name.is_empty()).then(|| Arc::from(name))
}

/// glibc `gnu_dev_major` and `gnu_dev_minor`.
fn split_dev(dev: u64) -> (u32, u32) {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    (major as u32, minor as u32)
}

/// Every whole-disk block device that has a drive, in object path order. A block is a whole
/// disk when it has no `Partition` interface. Missing drive properties leave fields `None`.
pub fn drives_from_objects(objs: &ManagedObjects) -> Vec<(DeviceIdentity, DiskDetails)> {
    let mut out = Vec::new();
    for ifaces in objs.0.values() {
        let Some(block) = ifaces.get(BLOCK_IFACE) else {
            continue;
        };
        if ifaces.contains_key(PARTITION_IFACE) {
            continue;
        }
        // "/" means the block has no drive (loop, dm and zram devices).
        let Some(drive_path) = object_path(block, "Drive").filter(|p| *p != "/") else {
            continue;
        };
        let (Some(name), Some(Value::U64(dev))) =
            (device_name(block), block.get("DeviceNumber").map(|v| &**v))
        else {
            continue;
        };
        let (major, minor) = split_dev(*dev);
        let drive_obj = objs.0.get(drive_path);
        let drive = drive_obj.and_then(|o| o.get(DRIVE_IFACE));
        let ata = drive_obj.and_then(|o| o.get(ATA_IFACE));
        let nvme = drive_obj.is_some_and(|o| o.contains_key(NVME_CONTROLLER_IFACE));
        let smart = if flag(ata, "SmartSupported") == Some(true) {
            SmartKind::Ata
        } else if nvme {
            SmartKind::Nvme
        } else if name.starts_with("nvme") {
            SmartKind::NeedsFallback
        } else {
            SmartKind::None
        };
        let rotation_rpm = match drive.and_then(|d| d.get("RotationRate")).map(|v| &**v) {
            Some(Value::I32(rpm)) if *rpm > 0 => u32::try_from(*rpm).ok(),
            _ => None,
        };
        let smart_temp_c = match ata.and_then(|a| a.get("SmartTemperature")).map(|v| &**v) {
            Some(Value::F64(k)) if *k > 0.0 => Some((*k - KELVIN_ZERO) as f32),
            _ => None,
        };
        let identity = DeviceIdentity::Disk {
            major,
            minor,
            name,
            wwn_or_serial: None,
        };
        let details = DiskDetails {
            model: text(drive, "Model"),
            wwn: text(drive, "WWN"),
            serial: text(drive, "Serial"),
            rotation_rpm,
            smart,
            smart_temp_c,
            ejectable: flag(drive, "Ejectable").unwrap_or(false),
            can_power_off: flag(drive, "CanPowerOff").unwrap_or(false),
            ..DiskDetails::default()
        };
        out.push((identity, details));
    }
    out
}

// ---- The bus seam ----

/// A UDisks2 signal, by object path.
#[derive(Debug, Clone)]
enum Event {
    Added {
        path: String,
        interfaces: Interfaces,
    },
    Removed {
        path: String,
        interfaces: Vec<String>,
    },
    Changed {
        path: String,
        interface: String,
        changed: Props,
        invalidated: Vec<String>,
    },
    /// A message the worker could not read. The worker resyncs.
    Lost,
}

/// Opens a UDisks2 session. Tests replace the system bus with a fake.
trait Connector: Send + Sync + 'static {
    type Bus: UdisksBus + Send;
    fn open(&self, cx: &Cx) -> impl Future<Output = Result<Self::Bus, DbusError>> + Send;
}

/// The one call and the signal stream the worker uses.
trait UdisksBus {
    /// `GetManagedObjects` on `ROOT`.
    fn managed_objects(
        &mut self,
        cx: &Cx,
    ) -> impl Future<Output = Result<ManagedObjects, DbusError>> + Send;
    /// The next signal, or `Ok(None)` when none arrived within `d`.
    fn next_event(
        &mut self,
        cx: &Cx,
        d: Duration,
    ) -> impl Future<Output = Result<Option<Event>, DbusError>> + Send;
}

struct SystemConnector;

impl Connector for SystemConnector {
    type Bus = SystemUdisks;

    async fn open(&self, cx: &Cx) -> Result<SystemUdisks, DbusError> {
        let conn = connect(cx, Bus::System).await?;
        SystemUdisks::on(cx, conn).await
    }
}

/// The real session. Fields drop in order, so the stream goes before the connection's tick
/// task stops.
struct SystemUdisks {
    stream: zbus::MessageStream,
    conn: DbusConn,
}

impl SystemUdisks {
    /// Subscribes to every UDisks2 signal under `ROOT`: `InterfacesAdded` and
    /// `InterfacesRemoved` on the root and `PropertiesChanged` on each object.
    async fn on(cx: &Cx, conn: DbusConn) -> Result<Self, DbusError> {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(BUS)
            .and_then(|b| b.path_namespace(ROOT))
            .map_err(|e| call_error(&e))?
            .build();
        let stream = conn
            .read(
                cx,
                zbus::MessageStream::for_match_rule(rule, conn.raw(), Some(256)),
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

type RawProps = HashMap<String, OwnedValue>;
type RawInterfaces = HashMap<String, RawProps>;

fn interfaces(raw: RawInterfaces) -> Interfaces {
    raw.into_iter()
        .map(|(name, props)| (name, props.into_iter().collect()))
        .collect()
}

/// One signal message as an `Event`. `Ok(None)` for other signals.
fn event_from(msg: &zbus::Message) -> Result<Option<Event>, zbus::Error> {
    let header = msg.header();
    let member = header.member().map(|m| m.as_str());
    let interface = header.interface().map(|i| i.as_str());
    let body = msg.body();
    match (interface, member) {
        (Some(OBJECT_MANAGER_IFACE), Some("InterfacesAdded")) => {
            let (path, raw): (OwnedObjectPath, RawInterfaces) = body.deserialize()?;
            Ok(Some(Event::Added {
                path: path.as_str().to_owned(),
                interfaces: interfaces(raw),
            }))
        }
        (Some(OBJECT_MANAGER_IFACE), Some("InterfacesRemoved")) => {
            let (path, interfaces): (OwnedObjectPath, Vec<String>) = body.deserialize()?;
            Ok(Some(Event::Removed {
                path: path.as_str().to_owned(),
                interfaces,
            }))
        }
        (_, Some("PropertiesChanged")) => {
            let (interface, changed, invalidated): (String, RawProps, Vec<String>) =
                body.deserialize()?;
            Ok(header.path().map(|p| Event::Changed {
                path: p.as_str().to_owned(),
                interface,
                changed: changed.into_iter().collect(),
                invalidated,
            }))
        }
        _ => Ok(None),
    }
}

impl UdisksBus for SystemUdisks {
    async fn managed_objects(&mut self, cx: &Cx) -> Result<ManagedObjects, DbusError> {
        let raw = self.conn.raw();
        let call = raw.call_method(
            Some(BUS),
            ROOT,
            Some(OBJECT_MANAGER_IFACE),
            "GetManagedObjects",
            &(),
        );
        let msg = self.conn.read(cx, call).await?;
        let objects: HashMap<OwnedObjectPath, RawInterfaces> =
            msg.body().deserialize().map_err(|e| call_error(&e))?;
        Ok(ManagedObjects(
            objects
                .into_iter()
                .map(|(path, raw)| (path.as_str().to_owned(), interfaces(raw)))
                .collect(),
        ))
    }

    async fn next_event(&mut self, cx: &Cx, d: Duration) -> Result<Option<Event>, DbusError> {
        let Some(item) = self.conn.next_signal(cx, &mut self.stream, d).await? else {
            return Ok(None);
        };
        let Ok(msg) = item else {
            return Ok(Some(Event::Lost));
        };
        Ok(event_from(&msg).unwrap_or(Some(Event::Lost)))
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
    use zbus::zvariant::ObjectPath;

    use crate::source::WorkerControl;

    const SDA: &str = "/org/freedesktop/UDisks2/block_devices/sda";
    const SDA1: &str = "/org/freedesktop/UDisks2/block_devices/sda1";
    const SDA_DRIVE: &str = "/org/freedesktop/UDisks2/drives/WDC_WD10EZEX_WD_1";
    const NVME: &str = "/org/freedesktop/UDisks2/block_devices/nvme0n1";
    const NVME_DRIVE: &str = "/org/freedesktop/UDisks2/drives/Samsung_SSD_980_S1";
    const SDB: &str = "/org/freedesktop/UDisks2/block_devices/sdb";
    const STICK_DRIVE: &str = "/org/freedesktop/UDisks2/drives/SanDisk_Cruzer_1";

    fn makedev(major: u64, minor: u64) -> u64 {
        ((major & 0xfff) << 8) | ((major & !0xfff) << 32) | (minor & 0xff) | ((minor & !0xff) << 12)
    }

    fn props(entries: Vec<(&str, Value<'static>)>) -> Props {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).expect("owned value")))
            .collect()
    }

    fn path(p: &'static str) -> Value<'static> {
        Value::from(ObjectPath::try_from(p).expect("object path"))
    }

    fn block(dev: u64, device: &str, drive: &'static str) -> Props {
        let mut bytes = device.as_bytes().to_vec();
        bytes.push(0);
        props(vec![
            ("DeviceNumber", Value::U64(dev)),
            ("Device", Value::from(bytes)),
            ("Drive", path(drive)),
        ])
    }

    fn object(ifaces: Vec<(&str, Props)>) -> Interfaces {
        ifaces
            .into_iter()
            .map(|(name, p)| (name.to_owned(), p))
            .collect()
    }

    fn sata_objects() -> ManagedObjects {
        let drive = props(vec![
            ("Model", Value::from("WDC WD10EZEX")),
            ("Serial", Value::from("WD-1")),
            ("WWN", Value::from("0x5001")),
            ("RotationRate", Value::I32(7200)),
            ("Ejectable", Value::Bool(false)),
            ("CanPowerOff", Value::Bool(false)),
        ]);
        let ata = props(vec![
            ("SmartSupported", Value::Bool(true)),
            ("SmartTemperature", Value::F64(308.15)),
        ]);
        ManagedObjects(BTreeMap::from([
            (
                SDA.to_owned(),
                object(vec![(
                    BLOCK_IFACE,
                    block(makedev(8, 0), "/dev/sda", SDA_DRIVE),
                )]),
            ),
            (
                SDA_DRIVE.to_owned(),
                object(vec![(DRIVE_IFACE, drive), (ATA_IFACE, ata)]),
            ),
        ]))
    }

    fn stick_objects() -> ManagedObjects {
        let drive = props(vec![
            ("Model", Value::from("Cruzer Blade")),
            ("Serial", Value::from("")),
            ("RotationRate", Value::I32(-1)),
            ("Ejectable", Value::Bool(true)),
            ("CanPowerOff", Value::Bool(true)),
        ]);
        ManagedObjects(BTreeMap::from([
            (
                SDB.to_owned(),
                object(vec![(
                    BLOCK_IFACE,
                    block(makedev(8, 16), "/dev/sdb", STICK_DRIVE),
                )]),
            ),
            (STICK_DRIVE.to_owned(), object(vec![(DRIVE_IFACE, drive)])),
        ]))
    }

    #[test]
    fn sata_drive_fields() {
        let got = drives_from_objects(&sata_objects());
        assert_eq!(got.len(), 1);
        let (identity, d) = &got[0];
        assert_eq!(
            identity,
            &DeviceIdentity::Disk {
                major: 8,
                minor: 0,
                name: "sda".into(),
                wwn_or_serial: None,
            }
        );
        assert_eq!(d.rotation_rpm, Some(7200));
        assert_eq!(d.smart, SmartKind::Ata);
        assert_eq!(d.smart_temp_c, Some(35.0));
        assert_eq!(d.model, valid(Arc::from("WDC WD10EZEX")));
        assert_eq!(d.serial, valid(Arc::from("WD-1")));
        assert_eq!(d.wwn, valid(Arc::from("0x5001")));
        assert!(!d.ejectable);
        assert!(!d.can_power_off);
    }

    #[test]
    fn split_dev_matches_glibc() {
        assert_eq!(split_dev(makedev(8, 0)), (8, 0));
        assert_eq!(split_dev(makedev(259, 3)), (259, 3));
        assert_eq!(split_dev(makedev(4097, 300)), (4097, 300));
    }

    #[test]
    fn nvme_without_controller_needs_fallback() {
        let drive = props(vec![("Model", Value::from("Samsung SSD 980"))]);
        let mut objs = ManagedObjects(BTreeMap::from([
            (
                NVME.to_owned(),
                object(vec![(
                    BLOCK_IFACE,
                    block(makedev(259, 0), "/dev/nvme0n1", NVME_DRIVE),
                )]),
            ),
            (NVME_DRIVE.to_owned(), object(vec![(DRIVE_IFACE, drive)])),
        ]));
        let got = drives_from_objects(&objs);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1.smart, SmartKind::NeedsFallback);
        assert_eq!(got[0].1.rotation_rpm, None);
        assert_eq!(got[0].1.smart_temp_c, None);
        assert_eq!(got[0].1.serial.validity, Validity::Unsupported);

        objs.0
            .get_mut(NVME_DRIVE)
            .expect("drive")
            .insert(NVME_CONTROLLER_IFACE.to_owned(), Props::new());
        assert_eq!(drives_from_objects(&objs)[0].1.smart, SmartKind::Nvme);
    }

    #[test]
    fn partitions_not_listed() {
        let mut objs = sata_objects();
        objs.0.insert(
            SDA1.to_owned(),
            object(vec![
                (BLOCK_IFACE, block(makedev(8, 1), "/dev/sda1", SDA_DRIVE)),
                (PARTITION_IFACE, Props::new()),
            ]),
        );
        // A loop device has no drive.
        objs.0.insert(
            "/org/freedesktop/UDisks2/block_devices/loop0".to_owned(),
            object(vec![(BLOCK_IFACE, block(makedev(7, 0), "/dev/loop0", "/"))]),
        );
        let got = drives_from_objects(&objs);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(matches!(&got[0].0, DeviceIdentity::Disk { name, .. } if &**name == "sda"));
    }

    #[test]
    fn usb_stick_eject_flags() {
        let got = drives_from_objects(&stick_objects());
        assert_eq!(got.len(), 1);
        let d = &got[0].1;
        assert!(d.ejectable);
        assert!(d.can_power_off);
        assert_eq!(d.rotation_rpm, None);
        assert_eq!(d.smart, SmartKind::None);
        assert_eq!(d.serial.validity, Validity::Unsupported, "empty serial");
    }

    #[test]
    fn sanitized_text() {
        let mut objs = sata_objects();
        let drive = objs
            .0
            .get_mut(SDA_DRIVE)
            .and_then(|o| o.get_mut(DRIVE_IFACE))
            .expect("drive");
        drive.insert(
            "Model".into(),
            OwnedValue::try_from(Value::from("WDC\nWD\u{202e}10")).expect("owned"),
        );
        let got = drives_from_objects(&objs);
        assert_eq!(got[0].1.model, valid(Arc::from("WDC WD10")));
    }

    // ---- The worker loop over a fake bus ----

    #[derive(Default)]
    struct FakeState {
        /// `None`: UDisks2 is not on the bus.
        objects: Option<ManagedObjects>,
        events: VecDeque<Event>,
        opens: u32,
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
            self.0.lock().expect("fake lock").opens += 1;
            Ok(FakeBus(Arc::clone(&self.0)))
        }
    }

    impl UdisksBus for FakeBus {
        async fn managed_objects(&mut self, _cx: &Cx) -> Result<ManagedObjects, DbusError> {
            let state = self.0.lock().expect("fake lock");
            state.objects.clone().ok_or_else(service_unknown)
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
    }

    struct Rig {
        lab: LabRuntime,
        rx: mpsc::Receiver<EngineMsg>,
        demand_tx: watch::Sender<Demand>,
        _settings_tx: watch::Sender<EngineSettings>,
        _control_tx: watch::Sender<WorkerControl>,
        handle: asupersync::runtime::TaskHandle<()>,
    }

    impl Rig {
        fn start(connector: FakeConnector, disks: bool) -> Self {
            let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
            let (updates, rx) = mpsc::channel::<EngineMsg>(256);
            let demand = Demand {
                disks,
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
                    let _ = run_with(cx, io, connector).await;
                })
                .expect("task is created");
            lab.scheduler.lock().schedule(task, 0);
            lab.run_until_idle();
            Rig {
                lab,
                rx,
                demand_tx,
                _settings_tx: settings_tx,
                _control_tx: control_tx,
                handle,
            }
        }

        fn advance(&mut self, d: Duration) {
            self.lab.advance_time(nanos(d));
            // `run_until_idle` stops when no task is ready; one step fires the due timers.
            self.lab.step_for_test();
            self.lab.run_until_idle();
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
            self.advance(SIGNAL_WAIT);
            let report = self.lab.run_until_quiescent_with_report();
            assert!(report.lab_test_passed(), "{report:?}");
        }
    }

    fn entries(update: &SourceUpdate) -> Option<&[(DeviceIdentity, DiskDetails)]> {
        match update {
            SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks {
                from: SourceId::Udisks,
                entries,
            }) => Some(entries),
            _ => None,
        }
    }

    fn opens(fake: &Arc<Mutex<FakeState>>) -> u32 {
        fake.lock().expect("fake lock").opens
    }

    #[test]
    fn no_connect_when_not_demanded() {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        fake.lock().expect("fake lock").objects = Some(sata_objects());
        let mut rig = Rig::start(connector, false);
        for _ in 0..120 {
            rig.advance(IDLE_RECHECK);
        }
        assert!(rig.drain().is_empty(), "idle sends nothing");
        assert_eq!(opens(&fake), 0);

        let demand = Demand {
            disks: true,
            ..Demand::default()
        };
        rig.demand_tx.send(demand).expect("demand send");
        rig.advance(IDLE_RECHECK);
        assert_eq!(opens(&fake), 1, "demand wakes the worker");
        assert_eq!(rig.drain().len(), 2);

        // Demand ends: the session closes and nothing more is sent.
        rig.demand_tx.send(Demand::default()).expect("demand send");
        for _ in 0..240 {
            rig.advance(IDLE_RECHECK);
        }
        assert!(rig.drain().is_empty());
        assert_eq!(opens(&fake), 1);
        rig.finish();
    }

    #[test]
    fn properties_changed_resends_only_on_change() {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        fake.lock().expect("fake lock").objects = Some(stick_objects());
        let mut rig = Rig::start(connector, true);

        let first = rig.drain();
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(
            matches!(
                &first[0],
                SourceUpdate::Health {
                    id: SourceId::Udisks,
                    state: SourceState::Running,
                    last_error: None,
                    ..
                }
            ),
            "{:?}",
            first[0]
        );
        assert!(entries(&first[1]).expect("Disks")[0].1.ejectable);

        let change = |key: &str, value: Value<'static>| Event::Changed {
            path: STICK_DRIVE.to_owned(),
            interface: DRIVE_IFACE.to_owned(),
            changed: props(vec![(key, value)]),
            invalidated: Vec::new(),
        };
        fake.lock()
            .expect("fake lock")
            .events
            .push_back(change("Ejectable", Value::Bool(false)));
        rig.advance(SIGNAL_WAIT);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "{got:?}");
        let list = entries(&got[0]).expect("Disks");
        assert_eq!(list.len(), 1);
        assert!(!list[0].1.ejectable);
        assert!(list[0].1.can_power_off);

        fake.lock()
            .expect("fake lock")
            .events
            .push_back(change("TimeMediaDetected", Value::U64(1_700_000_000)));
        rig.advance(SIGNAL_WAIT);
        assert!(rig.drain().is_empty(), "an unrelated change sends nothing");

        // A removed block interface drops the disk from the list.
        fake.lock()
            .expect("fake lock")
            .events
            .push_back(Event::Removed {
                path: SDB.to_owned(),
                interfaces: vec![BLOCK_IFACE.to_owned()],
            });
        rig.advance(SIGNAL_WAIT);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(entries(&got[0]).map(<[_]>::len), Some(0));
        rig.finish();
    }

    #[test]
    fn missing_service_reports_failed() {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        let mut rig = Rig::start(connector, true);

        let first = rig.drain();
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(
            matches!(&first[0], SourceUpdate::Health {
                id: SourceId::Udisks,
                state: SourceState::Failed,
                last_error: Some(text),
                ..
            } if &**text == NOT_AVAILABLE),
            "{:?}",
            first[0]
        );
        assert_eq!(entries(&first[1]).map(<[_]>::len), Some(0));
        assert_eq!(opens(&fake), 1);

        // UDisks2 starts. The retry after 60 s finds it.
        fake.lock().expect("fake lock").objects = Some(sata_objects());
        for _ in 0..119 {
            rig.advance(IDLE_RECHECK);
        }
        assert!(rig.drain().is_empty(), "nothing before 60 s");
        assert_eq!(opens(&fake), 1);
        rig.advance(IDLE_RECHECK);
        let got = rig.drain();
        assert_eq!(opens(&fake), 2);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(matches!(
            &got[0],
            SourceUpdate::Health {
                state: SourceState::Running,
                ..
            }
        ));
        assert_eq!(entries(&got[1]).map(<[_]>::len), Some(1));
        rig.finish();
    }

    #[test]
    fn periodic_resync_sends_only_on_change() {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        fake.lock().expect("fake lock").objects = Some(sata_objects());
        let mut rig = Rig::start(connector, true);
        assert_eq!(rig.drain().len(), 2);
        for _ in 0..30 {
            rig.advance(SIGNAL_WAIT);
        }
        assert!(rig.drain().is_empty(), "an equal resync sends nothing");

        fake.lock().expect("fake lock").objects = Some(stick_objects());
        for _ in 0..30 {
            rig.advance(SIGNAL_WAIT);
        }
        let got = rig.drain();
        assert_eq!(got.len(), 1, "{got:?}");
        let list = entries(&got[0]).expect("Disks");
        assert!(matches!(&list[0].0, DeviceIdentity::Disk { name, .. } if &**name == "sdb"));
        rig.finish();
    }

    #[test]
    fn spec_is_full_mode_only() {
        let s = spec();
        assert_eq!(s.id, SourceId::Udisks);
        assert_eq!(s.name, "udisks");
        assert_eq!(s.modes, &[EngineMode::Full]);
    }

    // ---- The zbus session over a peer-to-peer fake ----

    struct FakeBlock;

    #[zbus::interface(name = "org.freedesktop.UDisks2.Block")]
    impl FakeBlock {
        #[zbus(property)]
        fn device(&self) -> Vec<u8> {
            b"/dev/sdb\0".to_vec()
        }

        #[zbus(property)]
        fn device_number(&self) -> u64 {
            makedev(8, 16)
        }

        #[zbus(property)]
        fn drive(&self) -> OwnedObjectPath {
            OwnedObjectPath::try_from(STICK_DRIVE).expect("object path")
        }
    }

    struct FakeDrive {
        ejectable: bool,
    }

    #[zbus::interface(name = "org.freedesktop.UDisks2.Drive")]
    impl FakeDrive {
        #[zbus(property)]
        fn model(&self) -> String {
            "Cruzer Blade".to_owned()
        }

        #[zbus(property, name = "WWN")]
        fn wwn(&self) -> String {
            String::new()
        }

        #[zbus(property)]
        fn rotation_rate(&self) -> i32 {
            -1
        }

        #[zbus(property)]
        fn ejectable(&self) -> bool {
            self.ejectable
        }

        #[zbus(property)]
        fn can_power_off(&self) -> bool {
            true
        }
    }

    #[test]
    fn udisks_bus_over_p2p() {
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let server = zbus::connection::Builder::async_io_unix_stream(a)
            .server(zbus::Guid::generate())
            .expect("server builder")
            .p2p()
            .serve_at(ROOT, zbus::fdo::ObjectManager)
            .expect("object manager")
            .serve_at(STICK_DRIVE, FakeDrive { ejectable: true })
            .expect("drive")
            .serve_at(SDB, FakeBlock)
            .expect("block")
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
            let mut bus = SystemUdisks::on(&cx, conn).await.expect("subscribed");
            let mut objs = bus.managed_objects(&cx).await.expect("GetManagedObjects");
            let before = drives_from_objects(&objs);

            let iface = server
                .object_server()
                .interface::<_, FakeDrive>(STICK_DRIVE)
                .await
                .expect("drive interface");
            iface.get_mut().await.ejectable = false;
            iface
                .get()
                .await
                .ejectable_changed(iface.signal_emitter())
                .await
                .expect("signal sent");
            let mut event = None;
            for _ in 0..10 {
                event = bus.next_event(&cx, IDLE_RECHECK).await.expect("signal");
                if matches!(event, Some(Event::Changed { .. })) {
                    break;
                }
            }
            let event = event.expect("an event");
            let resync = apply_event(&mut objs, event.clone());
            let after = drives_from_objects(&objs);
            drop(bus);
            drop(server);
            (before, event, resync, after)
        });
        let (before, event, resync, after) = rt.block_on(join).expect("task completes");

        assert_eq!(before.len(), 1, "{before:?}");
        let (identity, d) = &before[0];
        assert_eq!(
            identity,
            &DeviceIdentity::Disk {
                major: 8,
                minor: 16,
                name: "sdb".into(),
                wwn_or_serial: None,
            }
        );
        assert!(d.ejectable);
        assert!(d.can_power_off);
        assert_eq!(d.model, valid(Arc::from("Cruzer Blade")));
        assert_eq!(d.wwn.validity, Validity::Unsupported);
        assert!(
            matches!(&event, Event::Changed { path, interface, .. }
                if path == STICK_DRIVE && interface == DRIVE_IFACE),
            "{event:?}"
        );
        assert!(!resync);
        assert!(!after[0].1.ejectable);
    }
}
