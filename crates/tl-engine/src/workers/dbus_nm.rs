//! The `NetworkManager` worker (ADR-004, PERF-054, PERF-058): what sysfs cannot tell about a
//! network interface.
//!
//! It reads the NM device type (WireGuard, VPN, Wi-Fi P2P), the active connection name shown under
//! the adapter name, and for Wi-Fi the SSID, signal strength, frequency, channel, bit rate and
//! security. It follows `DeviceAdded`, `DeviceRemoved` and `PropertiesChanged` under the NM path,
//! and while the network pages are shown it reads the active access point's `Strength` and the
//! device's `Bitrate` every `WIFI_PERIOD`. It runs while `Demand.nets` or `Demand.inventory` is
//! set. Without NetworkManager the `Net` worker's sysfs data stands alone.
//!
//! On the same `WIFI_PERIOD` tick it reads nl80211 (E12-025) for the signal in dBm and the RX and
//! TX bit rates, which NM does not give. Without NetworkManager the Wi-Fi rows come from nl80211
//! alone.
//!
//! SSIDs and connection names are shown on screen only. Never log them.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::sleep;
use asupersync::{Cx, Outcome};
use tl_collect::SysRoot;
use tl_collect::net::nl80211::{Nl80211, WifiLink};
use tl_collect::parse::netif::{self, NetIface};
use tl_collect::sanitize::sanitize_text;
use tl_model::devices::{DeviceIdentity, NetKind};
use tl_model::health::{SourceId, SourceState};
use tl_model::snapshot::{InvField, NetDetails, WifiInfo};
use tl_model::system::LinkState;
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{Bus, DbusConn, DbusError, connect};
use crate::source::{IDLE_RECHECK, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{DeviceDetailsUpdate, EngineMsg, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;
use crate::workers::{enumerate, net};

pub const BUS: &str = "org.freedesktop.NetworkManager";
pub const PATH: &str = "/org/freedesktop/NetworkManager";
/// PERF-058: the signal value updates at 1 Hz.
pub const WIFI_PERIOD: Duration = Duration::from_secs(1);

const MANAGER_IFACE: &str = "org.freedesktop.NetworkManager";
const DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACTIVE_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
/// The full resync period with NetworkManager, and the bus retry period without it.
const RESYNC: Duration = Duration::from_secs(60);
/// The health text while NetworkManager is missing.
const NOT_RUNNING: &str = "NetworkManager not running";
/// `NM_DEVICE_TYPE_WIFI`.
const TYPE_WIFI: u32 = 2;
/// The object path NM uses for "none".
const NO_PATH: &str = "/";
/// An SSID is at most 32 bytes (IEEE 802.11).
const SSID_MAX: usize = 32;
/// Shown for an access point with an empty SSID.
const HIDDEN_SSID: &str = "Hidden network";
/// nl80211 gives no security flags.
const UNKNOWN_SECURITY: &str = "Unknown";
/// The D-Bus error name for a missing service.
const SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";

/// NM 802-11 security flags (`NM80211ApFlags`, `NM80211ApSecurityFlags`).
const AP_FLAGS_PRIVACY: u32 = 0x1;
const KEY_MGMT_PSK: u32 = 0x100;
const KEY_MGMT_802_1X: u32 = 0x200;
const KEY_MGMT_SAE: u32 = 0x400;

/// The properties of one object interface, from `GetAll`.
pub type Props = HashMap<String, OwnedValue>;

/// The `org.freedesktop.NetworkManager.AccessPoint` properties the worker reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApProps {
    pub ssid: Vec<u8>,
    pub strength: u8,
    pub frequency: u32,
    pub flags: u32,
    pub wpa_flags: u32,
    pub rsn_flags: u32,
}

pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::NetworkManager,
        name: "networkmanager",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// Each generation runs [`run`] with the interface names from `cfg.sys_root`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    let root = cfg.sys_root.clone();
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| run(cx, io.clone(), root.clone()))
}

/// The worker on the system bus. `root` gives the current interfaces and their identities.
#[doc(hidden)]
pub async fn run(cx: Cx, io: WorkerIo, root: SysRoot) -> Outcome<(), WorkerError> {
    run_with(cx, io, root, SystemConnector, SystemWifi).await
}

/// NM `NMDeviceType` to the row kind and the PERF-054 "Type" text. `None` for loopback and the
/// types without a row of their own: the sysfs values stay.
pub fn kind_from_device_type(t: u32, active_type: Option<&str>) -> Option<(NetKind, &'static str)> {
    match t {
        1 => Some((NetKind::Wired, "Ethernet")),
        2 => Some((NetKind::Wireless, "Wi-Fi")),
        5 => Some((NetKind::Other, "Bluetooth")),
        8 => Some((NetKind::Wireless, "Cellular")),
        10 => Some((NetKind::Virtual, "Bond")),
        11 => Some((NetKind::Virtual, "VLAN")),
        13 => Some((NetKind::Virtual, "Bridge")),
        16 if active_type == Some("vpn") => Some((NetKind::VpnTunnel, "VPN")),
        16 => Some((NetKind::VpnTunnel, "Tunnel")),
        20 => Some((NetKind::Virtual, "Virtual")),
        29 => Some((NetKind::VpnTunnel, "WireGuard")),
        30 => Some((NetKind::Wireless, "Wi-Fi P2P")),
        // 32 is loopback, which is never shown.
        _ => None,
    }
}

/// The Wi-Fi rows from the active access point. NM's `Bitrate` is the current TX rate in kbit/s.
/// NM has no dBm or RX rate; nl80211 fills them (E12-025).
pub fn wifi_from_ap(ap: &ApProps, bitrate_kbps: u32) -> WifiInfo {
    let mut ssid = String::new();
    sanitize_text(&ap.ssid, SSID_MAX, &mut ssid);
    let ssid: Arc<str> = if ssid.is_empty() {
        HIDDEN_SSID.into()
    } else {
        ssid.into()
    };
    WifiInfo {
        ssid,
        signal_dbm: None,
        signal_pct: ap.strength,
        rx_mbps: None,
        tx_mbps: (bitrate_kbps > 0).then(|| bitrate_kbps as f32 / 1000.0),
        freq_mhz: ap.frequency,
        channel: channel_from_mhz(ap.frequency),
        security: security_text(ap.flags, ap.wpa_flags, ap.rsn_flags).into(),
    }
}

/// Adds what nl80211 knows to NM's Wi-Fi rows: the signal in dBm and the RX rate always, the TX
/// rate when NM has none. NM's `Strength` stays the percent.
pub fn merge_link(wifi: &mut WifiInfo, link: &WifiLink) {
    wifi.signal_dbm = link.signal_dbm;
    wifi.rx_mbps = link.rx_kbps.map(mbps);
    if wifi.tx_mbps.is_none() {
        wifi.tx_mbps = link.tx_kbps.map(mbps);
    }
}

/// The Wi-Fi rows from nl80211 alone, when NetworkManager is missing. `None` for an interface
/// that is not connected (no SSID).
pub fn wifi_from_link(link: &WifiLink) -> Option<WifiInfo> {
    let raw = link.ssid.as_ref()?;
    let mut ssid = String::new();
    sanitize_text(raw, SSID_MAX, &mut ssid);
    let ssid: Arc<str> = if ssid.is_empty() {
        HIDDEN_SSID.into()
    } else {
        ssid.into()
    };
    let freq_mhz = link.freq_mhz.unwrap_or(0);
    Some(WifiInfo {
        ssid,
        signal_dbm: link.signal_dbm,
        signal_pct: link.signal_dbm.map_or(0, signal_pct),
        rx_mbps: link.rx_kbps.map(mbps),
        tx_mbps: link.tx_kbps.map(mbps),
        freq_mhz,
        channel: channel_from_mhz(freq_mhz),
        security: UNKNOWN_SECURITY.into(),
    })
}

/// NetworkManager's own dBm to percent formula: `clamp(2 × (dBm + 100), 0, 100)`.
pub fn signal_pct(dbm: i32) -> u8 {
    let pct = dbm.saturating_add(100).saturating_mul(2).clamp(0, 100);
    u8::try_from(pct).unwrap_or(0)
}

fn mbps(kbps: u32) -> f32 {
    kbps as f32 / 1000.0
}

/// The 802.11 channel number of a 2.4, 5 or 6 GHz frequency.
pub fn channel_from_mhz(mhz: u32) -> Option<u16> {
    let channel = match mhz {
        2412..=2472 => (mhz - 2407) / 5,
        2484 => 14,
        5160..=5885 => (mhz - 5000) / 5,
        5955..=7115 => (mhz - 5950) / 5,
        _ => return None,
    };
    u16::try_from(channel).ok()
}

/// The strongest key management the access point offers.
pub fn security_text(flags: u32, wpa_flags: u32, rsn_flags: u32) -> &'static str {
    if rsn_flags & KEY_MGMT_SAE != 0 {
        "WPA3 Personal"
    } else if rsn_flags & KEY_MGMT_802_1X != 0 {
        "WPA2 Enterprise"
    } else if rsn_flags & KEY_MGMT_PSK != 0 {
        "WPA2 Personal"
    } else if wpa_flags != 0 {
        "WPA"
    } else if flags & AP_FLAGS_PRIVACY != 0 {
        "WEP"
    } else {
        "Open"
    }
}

// ---- The worker loop ----

/// One NM device as last read.
#[derive(Clone, Debug, PartialEq)]
struct NmDevice {
    /// `IpInterface`, else `Interface`.
    name: Arc<str>,
    kind: Option<(NetKind, &'static str)>,
    connection: Option<Arc<str>>,
    /// The active connection object, for `PropertiesChanged`.
    active: Option<String>,
    /// The active access point object and its properties (Wi-Fi only).
    ap: Option<(String, ApProps)>,
    bitrate_kbps: u32,
}

/// The fields the reducer takes from an NM record, per interface. Compared to send only changes.
type Record = (
    DeviceIdentity,
    NetKind,
    &'static str,
    Option<Arc<str>>,
    Option<WifiInfo>,
);

/// Where the worker goes next.
enum Next<B> {
    Idle,
    Bus(B, BTreeMap<String, NmDevice>),
    Missing,
    Stop(Outcome<(), WorkerError>),
}

/// The state machine: idle, NetworkManager or missing. Returns `Outcome::Ok(())` on cancel and
/// `Err` when the bus fails or the reducer's queue is closed, so the supervisor restarts it.
async fn run_with<C: Connector, W: WifiOpener>(
    cx: Cx,
    io: WorkerIo,
    root: SysRoot,
    connector: C,
    opener: W,
) -> Outcome<(), WorkerError> {
    let mut wifi = Wifi::new(opener);
    let mut next = Next::Idle;
    loop {
        next = match next {
            Next::Idle => {
                if let Some(stop) = idle(&cx, &io).await {
                    return stop;
                }
                match start_bus(&cx, &connector).await {
                    Ok((bus, devices)) => Next::Bus(bus, devices),
                    Err(e) if unavailable(&e) => Next::Missing,
                    Err(e) => fail(&cx, &e),
                }
            }
            Next::Bus(bus, devices) => bus_loop(&cx, &io, &root, bus, devices, &mut wifi).await,
            Next::Missing => missing_loop(&cx, &io, &root, &connector, &mut wifi).await,
            Next::Stop(outcome) => return outcome,
        };
    }
}

/// Step 1: waits until the network pages or the Information tab are shown. `Some` on cancel.
async fn idle(cx: &Cx, io: &WorkerIo) -> Option<Outcome<(), WorkerError>> {
    while !active(io) {
        sleep(cx.now(), IDLE_RECHECK).await;
        if cx.checkpoint().is_err() {
            return Some(Outcome::Ok(()));
        }
    }
    None
}

fn active(io: &WorkerIo) -> bool {
    let demand = io.demand.borrow_and_clone();
    demand.nets || demand.inventory
}

fn nets_demanded(io: &WorkerIo) -> bool {
    io.demand.borrow_and_clone().nets
}

/// Opens the bus and reads every device (steps 2 and 3).
async fn start_bus<C: Connector>(
    cx: &Cx,
    connector: &C,
) -> Result<(C::Bus, BTreeMap<String, NmDevice>), DbusError> {
    let mut bus = connector.open(cx).await?;
    let devices = resync(cx, &mut bus).await?;
    Ok((bus, devices))
}

/// Step 3: every device by object path. A device whose `GetAll` fails is left out.
async fn resync<B: NmBus>(cx: &Cx, bus: &mut B) -> Result<BTreeMap<String, NmDevice>, DbusError> {
    let mut devices = BTreeMap::new();
    for path in bus.devices(cx).await? {
        if let Some(device) = read_device(cx, bus, &path).await? {
            devices.insert(path, device);
        }
    }
    Ok(devices)
}

/// Step 3 for one device. `Ok(None)` when its `GetAll` fails or it has no interface name. A
/// failed read of the active connection or the access point leaves those fields `None`.
async fn read_device<B: NmBus>(
    cx: &Cx,
    bus: &mut B,
    path: &str,
) -> Result<Option<NmDevice>, DbusError> {
    let Some(props) = soft(bus.props(cx, path, DEVICE_IFACE).await)? else {
        return Ok(None);
    };
    let name = string(&props, "IpInterface")
        .filter(|s| !s.is_empty())
        .or_else(|| string(&props, "Interface").filter(|s| !s.is_empty()));
    let Some(name) = name else {
        return Ok(None);
    };
    let device_type = u32_of(&props, "DeviceType").unwrap_or(0);

    let active = object(&props, "ActiveConnection");
    let mut connection = None;
    let mut active_type = None;
    if let Some(ac) = &active
        && let Some(ac_props) = soft(bus.props(cx, ac, ACTIVE_IFACE).await)?
    {
        connection = string(&ac_props, "Id").filter(|s| !s.is_empty());
        active_type = string(&ac_props, "Type");
    }

    let mut ap = None;
    let mut bitrate_kbps = 0;
    if device_type == TYPE_WIFI
        && let Some(wireless) = soft(bus.props(cx, path, WIRELESS_IFACE).await)?
    {
        bitrate_kbps = u32_of(&wireless, "Bitrate").unwrap_or(0);
        if let Some(ap_path) = object(&wireless, "ActiveAccessPoint")
            && let Some(ap_props) = soft(bus.props(cx, &ap_path, AP_IFACE).await)?
        {
            ap = Some((ap_path, ap_from_props(&ap_props)));
        }
    }

    Ok(Some(NmDevice {
        name: name.into(),
        kind: kind_from_device_type(device_type, active_type.as_deref()),
        connection: connection.map(Arc::from),
        active,
        ap,
        bitrate_kbps,
    }))
}

/// `Ok(None)` for a failed read, logged by error name. `Closed` stays an error.
fn soft<T>(r: Result<T, DbusError>) -> Result<Option<T>, DbusError> {
    match r {
        Ok(value) => Ok(Some(value)),
        Err(DbusError::Closed) => Err(DbusError::Closed),
        Err(e) => {
            tracing::debug!(error = dbus_name(&e), "NetworkManager read failed");
            Ok(None)
        }
    }
}

fn ap_from_props(p: &Props) -> ApProps {
    ApProps {
        ssid: bytes(p, "Ssid").unwrap_or_default(),
        strength: u8_of(p, "Strength").unwrap_or(0),
        frequency: u32_of(p, "Frequency").unwrap_or(0),
        flags: u32_of(p, "Flags").unwrap_or(0),
        wpa_flags: u32_of(p, "WpaFlags").unwrap_or(0),
        rsn_flags: u32_of(p, "RsnFlags").unwrap_or(0),
    }
}

/// Steps 4 to 7 with NetworkManager.
async fn bus_loop<B: NmBus, W: WifiOpener>(
    cx: &Cx,
    io: &WorkerIo,
    root: &SysRoot,
    mut bus: B,
    mut devices: BTreeMap<String, NmDevice>,
    wifi: &mut Wifi<W>,
) -> Next<B> {
    if let Some(stop) = send(cx, io, health(wifi.error.clone())).await {
        return Next::Stop(stop);
    }
    let mut ifaces = match interfaces(cx, root).await {
        Ok(ifaces) => ifaces,
        Err(stop) => return Next::Stop(stop),
    };
    if nets_demanded(io)
        && devices.values().any(|d| d.ap.is_some())
        && let Err(stop) = wifi.poll(cx, io).await
    {
        return Next::Stop(stop);
    }
    let mut sent = None;
    if let Some(stop) = publish(cx, io, &devices, &ifaces, &wifi.links, &mut sent).await {
        return Next::Stop(stop);
    }
    let period = nanos(WIFI_PERIOD);
    let mut next_resync = cx.now().as_nanos() + nanos(RESYNC);
    let mut next_wifi = cx.now().as_nanos() + period;
    loop {
        // Step 7: dropping `bus` closes the connection.
        if !active(io) {
            return Next::Idle;
        }
        let polling = nets_demanded(io) && devices.values().any(|d| d.ap.is_some());
        let now = cx.now().as_nanos();
        let wait = if polling {
            Duration::from_nanos(next_wifi.saturating_sub(now)).min(IDLE_RECHECK)
        } else {
            IDLE_RECHECK
        };
        let mut changed = false;
        let mut renamed = false;
        if !wait.is_zero() {
            match bus.next_event(cx, wait).await {
                Ok(Some(event)) => {
                    renamed = matches!(event, Event::Added(_));
                    match apply_event(cx, &mut bus, &mut devices, event).await {
                        Ok(c) => changed = c,
                        Err(e) => return fail(cx, &e),
                    }
                }
                Ok(None) => {}
                Err(e) => return fail(cx, &e),
            }
        }
        if cx.checkpoint().is_err() {
            return Next::Stop(Outcome::Ok(()));
        }

        // Demand may have changed during the wait.
        if !active(io) {
            return Next::Idle;
        }
        // Step 6.
        let polling = nets_demanded(io) && devices.values().any(|d| d.ap.is_some());
        let now = cx.now().as_nanos();
        if !polling {
            next_wifi = now + period;
            changed |= wifi.clear();
        } else if now >= next_wifi {
            next_wifi += period;
            if next_wifi <= now {
                next_wifi = now + period;
            }
            match poll_wifi(cx, &mut bus, &mut devices).await {
                Ok(c) => changed |= c,
                Err(e) => return fail(cx, &e),
            }
            match wifi.poll(cx, io).await {
                Ok(c) => changed |= c,
                Err(stop) => return Next::Stop(stop),
            }
        }

        if now >= next_resync {
            next_resync = now + nanos(RESYNC);
            match resync(cx, &mut bus).await {
                Ok(fresh) => {
                    changed |= fresh != devices;
                    devices = fresh;
                    renamed = true;
                }
                Err(e) if unavailable(&e) => return Next::Missing,
                Err(e) => return fail(cx, &e),
            }
        }
        if renamed {
            ifaces = match interfaces(cx, root).await {
                Ok(ifaces) => ifaces,
                Err(stop) => return Next::Stop(stop),
            };
            changed = true;
        }
        if changed
            && let Some(stop) = publish(cx, io, &devices, &ifaces, &wifi.links, &mut sent).await
        {
            return Next::Stop(stop);
        }
    }
}

/// Step 5 for one signal. `Ok(true)` when the device set or a device changed.
async fn apply_event<B: NmBus>(
    cx: &Cx,
    bus: &mut B,
    devices: &mut BTreeMap<String, NmDevice>,
    event: Event,
) -> Result<bool, DbusError> {
    let path = match event {
        Event::Removed(path) => return Ok(devices.remove(&path).is_some()),
        Event::Added(path) => path,
        // A change of the device, its active connection or its access point re-reads the
        // device. Other objects (other access points, the manager) are ignored; the resync
        // picks up anything missed.
        Event::Changed(path) => {
            let owner = devices.iter().find(|(dev, d)| {
                **dev == path
                    || d.active.as_deref() == Some(path.as_str())
                    || d.ap.as_ref().is_some_and(|(ap, _)| *ap == path)
            });
            match owner {
                Some((dev, _)) => dev.clone(),
                None => return Ok(false),
            }
        }
    };
    match read_device(cx, bus, &path).await? {
        Some(device) => Ok(devices.insert(path, device.clone()).as_ref() != Some(&device)),
        None => Ok(devices.remove(&path).is_some()),
    }
}

/// Step 6: `Strength` of each active access point and `Bitrate` of its device. `Ok(true)` when
/// a value changed. A failed read keeps the old value.
async fn poll_wifi<B: NmBus>(
    cx: &Cx,
    bus: &mut B,
    devices: &mut BTreeMap<String, NmDevice>,
) -> Result<bool, DbusError> {
    let mut changed = false;
    for (path, device) in devices.iter_mut() {
        let Some((ap_path, ap)) = &mut device.ap else {
            continue;
        };
        if let Some(value) = soft(bus.prop(cx, ap_path, AP_IFACE, "Strength").await)?
            && let Value::U8(strength) = &*value
            && *strength != ap.strength
        {
            ap.strength = *strength;
            changed = true;
        }
        if let Some(value) = soft(bus.prop(cx, path, WIRELESS_IFACE, "Bitrate").await)?
            && let Value::U32(bitrate) = &*value
            && *bitrate != device.bitrate_kbps
        {
            device.bitrate_kbps = *bitrate;
            changed = true;
        }
    }
    Ok(changed)
}

/// Step 2 without NetworkManager: the health text, the Wi-Fi rows from nl80211 alone (read every
/// `WIFI_PERIOD` while the network pages are shown), and a bus retry every 60 s.
async fn missing_loop<C: Connector, W: WifiOpener>(
    cx: &Cx,
    io: &WorkerIo,
    root: &SysRoot,
    connector: &C,
    wifi: &mut Wifi<W>,
) -> Next<C::Bus> {
    if let Some(stop) = send(cx, io, health(Some(NOT_RUNNING.into()))).await {
        return Next::Stop(stop);
    }
    if nets_demanded(io)
        && let Err(stop) = wifi.poll(cx, io).await
    {
        return Next::Stop(stop);
    }
    let mut sent = None;
    if let Some(stop) = publish_links(cx, io, root, &wifi.links, &mut sent).await {
        return Next::Stop(stop);
    }
    let period = nanos(WIFI_PERIOD);
    let mut next_retry = cx.now().as_nanos() + nanos(RESYNC);
    let mut next_wifi = cx.now().as_nanos() + period;
    loop {
        if !active(io) {
            return Next::Idle;
        }
        if cx.now().as_nanos() >= next_retry {
            next_retry = cx.now().as_nanos() + nanos(RESYNC);
            match start_bus(cx, connector).await {
                Ok((bus, devices)) => return Next::Bus(bus, devices),
                Err(DbusError::Closed) if cx.checkpoint().is_err() => {
                    return Next::Stop(Outcome::Ok(()));
                }
                Err(e) => tracing::debug!(error = dbus_name(&e), "NetworkManager still missing"),
            }
        }
        let polling = nets_demanded(io);
        let now = cx.now().as_nanos();
        let mut changed = false;
        if !polling {
            next_wifi = now + period;
            changed = wifi.clear();
        } else if now >= next_wifi {
            next_wifi += period;
            if next_wifi <= now {
                next_wifi = now + period;
            }
            match wifi.poll(cx, io).await {
                Ok(c) => changed = c,
                Err(stop) => return Next::Stop(stop),
            }
        }
        if changed && let Some(stop) = publish_links(cx, io, root, &wifi.links, &mut sent).await {
            return Next::Stop(stop);
        }
        let wait = if polling {
            Duration::from_nanos(next_wifi.saturating_sub(now)).min(IDLE_RECHECK)
        } else {
            IDLE_RECHECK
        };
        sleep(cx.now(), wait).await;
        if cx.checkpoint().is_err() {
            return Next::Stop(Outcome::Ok(()));
        }
    }
}

/// The current interfaces, listed in a blocking job. A failed listing gives none.
async fn interfaces(cx: &Cx, root: &SysRoot) -> Result<Vec<NetIface>, Outcome<(), WorkerError>> {
    let root = root.clone();
    blocking(cx, move || {
        let mut out = Vec::new();
        if netif::enumerate(&root, &mut out).is_err() {
            out.clear();
        }
        out
    })
    .await
}

/// Runs `f` on the blocking pool and joins it. `Err` holds the worker's result when it must stop.
async fn blocking<T: Send + 'static>(
    cx: &Cx,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Outcome<(), WorkerError>> {
    let mut job = match cx.spawn_blocking(move |_| f()) {
        Ok(job) => job,
        Err(e) => {
            return Err(Outcome::Err(WorkerError::Other(
                format!("spawn: {e}").into(),
            )));
        }
    };
    match job.join(cx).await {
        Ok(value) => Ok(value),
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

/// Step 4: one record per device whose interface name is a current interface. A device NM
/// gives no kind for keeps the sysfs kind, so the merge leaves it as it is.
/// nl80211 values are merged into the Wi-Fi rows by ifindex.
fn records(
    devices: &BTreeMap<String, NmDevice>,
    ifaces: &[NetIface],
    links: &[WifiLink],
) -> Vec<Record> {
    devices
        .values()
        .filter_map(|d| {
            let iface = ifaces.iter().find(|i| i.name == d.name)?;
            let (kind, label) = d.kind.unwrap_or_else(|| net::net_kind(iface.kind));
            let mut wifi =
                d.ap.as_ref()
                    .map(|(_, ap)| wifi_from_ap(ap, d.bitrate_kbps));
            if let Some(wifi) = &mut wifi
                && let Some(link) = links.iter().find(|l| l.ifindex == iface.ifindex)
            {
                merge_link(wifi, link);
            }
            Some((
                enumerate::net_identity(iface),
                kind,
                label,
                d.connection.clone(),
                wifi,
            ))
        })
        .collect()
}

/// Sends `Nets { from: NetworkManager }` when the records differ from the last ones sent.
async fn publish(
    cx: &Cx,
    io: &WorkerIo,
    devices: &BTreeMap<String, NmDevice>,
    ifaces: &[NetIface],
    links: &[WifiLink],
    sent: &mut Option<Vec<Record>>,
) -> Option<Outcome<(), WorkerError>> {
    send_records(cx, io, records(devices, ifaces, links), sent).await
}

/// Without NetworkManager: one Wi-Fi record per nl80211 interface that is a current interface.
async fn publish_links(
    cx: &Cx,
    io: &WorkerIo,
    root: &SysRoot,
    links: &[WifiLink],
    sent: &mut Option<Vec<Record>>,
) -> Option<Outcome<(), WorkerError>> {
    let ifaces = if links.is_empty() {
        Vec::new()
    } else {
        match interfaces(cx, root).await {
            Ok(ifaces) => ifaces,
            Err(stop) => return Some(stop),
        }
    };
    let fresh = links
        .iter()
        .filter_map(|link| {
            let iface = ifaces.iter().find(|i| i.ifindex == link.ifindex)?;
            Some((
                enumerate::net_identity(iface),
                NetKind::Wireless,
                "Wi-Fi",
                None,
                wifi_from_link(link),
            ))
        })
        .collect();
    send_records(cx, io, fresh, sent).await
}

/// Sends `fresh` when it differs from the records sent last.
async fn send_records(
    cx: &Cx,
    io: &WorkerIo,
    fresh: Vec<Record>,
    sent: &mut Option<Vec<Record>>,
) -> Option<Outcome<(), WorkerError>> {
    if sent.as_ref() == Some(&fresh) {
        return None;
    }
    let entries = fresh.iter().cloned().map(details).collect();
    let stop = send(cx, io, nets(entries)).await;
    *sent = Some(fresh);
    stop
}

/// The reducer takes only `kind`, `type_label`, `connection` and `wifi` from an NM record. The
/// other fields belong to the `Net` worker and are left empty here.
fn details(record: Record) -> (DeviceIdentity, NetDetails) {
    let (identity, kind, label, connection, wifi) = record;
    let ifindex = match &identity {
        DeviceIdentity::Net { ifindex, .. } => *ifindex,
        _ => 0,
    };
    let none = || InvField {
        value: Arc::from(""),
        validity: Validity::Unsupported,
    };
    let details = NetDetails {
        type_label: label.into(),
        connection,
        adapter_name: none(),
        driver: none(),
        mac: none(),
        ifindex,
        link_speed_mbps: InvField {
            value: 0,
            validity: Validity::Unsupported,
        },
        state: LinkState::Unknown,
        addresses: Arc::from([]),
        kind,
        wifi,
    };
    (identity, details)
}

fn nets(entries: Vec<(DeviceIdentity, NetDetails)>) -> SourceUpdate {
    SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Nets {
        from: SourceId::NetworkManager,
        entries,
    })
}

fn health(last_error: Option<Arc<str>>) -> SourceUpdate {
    // `SourceState` has no degraded state: a missing NetworkManager keeps `Running` with the
    // text, as the UPower worker does for its sysfs fallback.
    SourceUpdate::Health {
        id: SourceId::NetworkManager,
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
    tracing::debug!(error = dbus_name(e), "NetworkManager bus failed");
    Next::Stop(Outcome::Err(WorkerError::Dbus(dbus_name(e).into())))
}

/// NetworkManager is not on the bus.
fn unavailable(e: &DbusError) -> bool {
    matches!(e, DbusError::Call { name, .. }
        if &**name == SERVICE_UNKNOWN
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

// ---- Property values ----

fn u32_of(p: &Props, key: &str) -> Option<u32> {
    match &**p.get(key)? {
        Value::U32(v) => Some(*v),
        _ => None,
    }
}

fn u8_of(p: &Props, key: &str) -> Option<u8> {
    match &**p.get(key)? {
        Value::U8(v) => Some(*v),
        _ => None,
    }
}

fn string(p: &Props, key: &str) -> Option<String> {
    match &**p.get(key)? {
        Value::Str(s) => Some(s.as_str().to_owned()),
        _ => None,
    }
}

/// An object path property. `None` for NM's "none" path `/`.
fn object(p: &Props, key: &str) -> Option<String> {
    match &**p.get(key)? {
        Value::ObjectPath(o) if o.as_str() != NO_PATH => Some(o.as_str().to_owned()),
        _ => None,
    }
}

/// An `ay` property.
fn bytes(p: &Props, key: &str) -> Option<Vec<u8>> {
    match &**p.get(key)? {
        Value::Array(a) => a
            .iter()
            .map(|v| match v {
                Value::U8(b) => Some(*b),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

// ---- nl80211 ----

/// The nl80211 reader of one worker generation: opened once, at the first Wi-Fi tick.
struct Wifi<W: WifiOpener> {
    opener: W,
    state: WifiState<W::Links>,
    /// The last read, empty when nl80211 is off or not read.
    links: Vec<WifiLink>,
    /// "nl80211: <error>" after a failure, kept in the health text for this generation.
    error: Option<Arc<str>>,
}

enum WifiState<L> {
    Unopened,
    Open(L),
    /// No Wi-Fi driver, or nl80211 failed in this generation.
    Off,
}

impl<W: WifiOpener> Wifi<W> {
    fn new(opener: W) -> Self {
        Self {
            opener,
            state: WifiState::Unopened,
            links: Vec::new(),
            error: None,
        }
    }

    /// One read on the blocking pool, opening first if needed. `Ok(true)` when the links changed.
    /// An `io::Error` turns nl80211 off and sends the health text once.
    async fn poll(&mut self, cx: &Cx, io: &WorkerIo) -> Result<bool, Outcome<(), WorkerError>> {
        let result = match std::mem::replace(&mut self.state, WifiState::Off) {
            WifiState::Off => return Ok(false),
            WifiState::Unopened => {
                let opener = self.opener.clone();
                let read = blocking(cx, move || -> io::Result<_> {
                    let mut source = opener.open()?;
                    let links = match &mut source {
                        Some(s) => s.links()?,
                        None => Vec::new(),
                    };
                    Ok((source, links))
                });
                read.await?
            }
            WifiState::Open(mut source) => {
                let read = blocking(cx, move || -> io::Result<_> {
                    let links = source.links()?;
                    Ok((Some(source), links))
                });
                read.await?
            }
        };
        match result {
            Ok((Some(source), links)) => {
                self.state = WifiState::Open(source);
                let changed = links != self.links;
                self.links = links;
                Ok(changed)
            }
            Ok((None, _)) => Ok(self.clear()),
            Err(e) => {
                tracing::debug!(error = %e, "nl80211 off for this generation");
                let text: Arc<str> = format!("nl80211: {e}").into();
                self.error = Some(Arc::clone(&text));
                if let Some(stop) = send(cx, io, health(Some(text))).await {
                    return Err(stop);
                }
                Ok(self.clear())
            }
        }
    }

    /// Drops the last read. `true` when there was one.
    fn clear(&mut self) -> bool {
        let had = !self.links.is_empty();
        self.links.clear();
        had
    }
}

/// Opens the nl80211 reader. Blocking; runs on the blocking pool. Tests replace the kernel.
trait WifiOpener: Clone + Send + Sync + 'static {
    type Links: WifiLinks;
    /// `Ok(None)` when there is no Wi-Fi driver.
    fn open(&self) -> io::Result<Option<Self::Links>>;
}

trait WifiLinks: Send + 'static {
    /// Blocking.
    fn links(&mut self) -> io::Result<Vec<WifiLink>>;
}

#[derive(Clone, Copy)]
struct SystemWifi;

impl WifiOpener for SystemWifi {
    type Links = Nl80211;

    fn open(&self) -> io::Result<Option<Nl80211>> {
        Nl80211::open()
    }
}

impl WifiLinks for Nl80211 {
    fn links(&mut self) -> io::Result<Vec<WifiLink>> {
        Nl80211::links(self)
    }
}

// ---- The bus seam ----

/// An NM signal, by object path.
#[derive(Debug, Clone, PartialEq)]
enum Event {
    Added(String),
    Removed(String),
    Changed(String),
}

/// Opens an NM session. Tests replace the system bus with a fake.
trait Connector: Send + Sync + 'static {
    type Bus: NmBus + Send;
    fn open(&self, cx: &Cx) -> impl Future<Output = Result<Self::Bus, DbusError>> + Send;
}

/// The NM calls the worker makes.
trait NmBus {
    /// `GetDevices` on the manager.
    fn devices(&mut self, cx: &Cx) -> impl Future<Output = Result<Vec<String>, DbusError>> + Send;
    /// `GetAll(iface)` on `path`.
    fn props(
        &mut self,
        cx: &Cx,
        path: &str,
        iface: &str,
    ) -> impl Future<Output = Result<Props, DbusError>> + Send;
    /// `Get(iface, name)` on `path`.
    fn prop(
        &mut self,
        cx: &Cx,
        path: &str,
        iface: &str,
        name: &str,
    ) -> impl Future<Output = Result<OwnedValue, DbusError>> + Send;
    /// The next signal, or `Ok(None)` when none arrived within `d`.
    fn next_event(
        &mut self,
        cx: &Cx,
        d: Duration,
    ) -> impl Future<Output = Result<Option<Event>, DbusError>> + Send;
}

struct SystemConnector;

impl Connector for SystemConnector {
    type Bus = SystemNm;

    async fn open(&self, cx: &Cx) -> Result<SystemNm, DbusError> {
        let conn = connect(cx, Bus::System).await?;
        SystemNm::on(cx, conn).await
    }
}

/// The real session. Fields drop in order, so the stream goes before the connection's tick
/// task stops.
struct SystemNm {
    stream: zbus::MessageStream,
    conn: DbusConn,
}

impl SystemNm {
    /// Checks that NetworkManager owns its name, so no call starts it through bus activation,
    /// then subscribes to every NM signal under `PATH`.
    async fn on(cx: &Cx, conn: DbusConn) -> Result<Self, DbusError> {
        let raw = conn.raw();
        let call = raw.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(BUS,),
        );
        let msg = conn.read(cx, call).await?;
        let owned: bool = msg.body().deserialize().map_err(|e| call_error(&e))?;
        if !owned {
            return Err(DbusError::Call {
                name: SERVICE_UNKNOWN.into(),
                message: "NetworkManager has no owner".into(),
            });
        }
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

impl NmBus for SystemNm {
    async fn devices(&mut self, cx: &Cx) -> Result<Vec<String>, DbusError> {
        let raw = self.conn.raw();
        let call = raw.call_method(Some(BUS), PATH, Some(MANAGER_IFACE), "GetDevices", &());
        let msg = self.conn.read(cx, call).await?;
        let paths: Vec<OwnedObjectPath> = msg.body().deserialize().map_err(|e| call_error(&e))?;
        Ok(paths.into_iter().map(|p| p.as_str().to_owned()).collect())
    }

    async fn props(&mut self, cx: &Cx, path: &str, iface: &str) -> Result<Props, DbusError> {
        let raw = self.conn.raw();
        let body = (iface,);
        let call = raw.call_method(Some(BUS), path, Some(PROPS_IFACE), "GetAll", &body);
        let msg = self.conn.read(cx, call).await?;
        msg.body().deserialize().map_err(|e| call_error(&e))
    }

    async fn prop(
        &mut self,
        cx: &Cx,
        path: &str,
        iface: &str,
        name: &str,
    ) -> Result<OwnedValue, DbusError> {
        let raw = self.conn.raw();
        let body = (iface, name);
        let call = raw.call_method(Some(BUS), path, Some(PROPS_IFACE), "Get", &body);
        let msg = self.conn.read(cx, call).await?;
        msg.body().deserialize().map_err(|e| call_error(&e))
    }

    async fn next_event(&mut self, cx: &Cx, d: Duration) -> Result<Option<Event>, DbusError> {
        let Some(item) = self.conn.next_signal(cx, &mut self.stream, d).await? else {
            return Ok(None);
        };
        let msg = item.map_err(|e| call_error(&e))?;
        let header = msg.header();
        let iface = header.interface().map(|i| i.as_str());
        let member = header.member().map(|m| m.as_str());
        let device = || -> Result<String, DbusError> {
            let path: OwnedObjectPath = msg.body().deserialize().map_err(|e| call_error(&e))?;
            Ok(path.as_str().to_owned())
        };
        match (iface, member) {
            (Some(MANAGER_IFACE), Some("DeviceAdded")) => Ok(Some(Event::Added(device()?))),
            (Some(MANAGER_IFACE), Some("DeviceRemoved")) => Ok(Some(Event::Removed(device()?))),
            (Some(PROPS_IFACE), Some("PropertiesChanged")) => {
                Ok(header.path().map(|p| Event::Changed(p.as_str().to_owned())))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::Mutex;

    use asupersync::channel::watch;
    use asupersync::time;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_model::demand::Demand;
    use tl_model::settings::EngineSettings;

    use crate::source::WorkerControl;

    const ETH_PATH: &str = "/org/freedesktop/NetworkManager/Devices/1";
    const WIFI_PATH: &str = "/org/freedesktop/NetworkManager/Devices/2";
    const ETH_AC: &str = "/org/freedesktop/NetworkManager/ActiveConnection/1";
    const WIFI_AC: &str = "/org/freedesktop/NetworkManager/ActiveConnection/2";
    const AP_PATH: &str = "/org/freedesktop/NetworkManager/AccessPoint/7";

    #[test]
    fn device_type_mapping() {
        assert_eq!(
            kind_from_device_type(16, Some("vpn")),
            Some((NetKind::VpnTunnel, "VPN"))
        );
        assert_eq!(
            kind_from_device_type(16, Some("tun")),
            Some((NetKind::VpnTunnel, "Tunnel"))
        );
        assert_eq!(
            kind_from_device_type(29, None),
            Some((NetKind::VpnTunnel, "WireGuard"))
        );
        assert_eq!(kind_from_device_type(32, None), None);
        assert_eq!(
            kind_from_device_type(1, None),
            Some((NetKind::Wired, "Ethernet"))
        );
        assert_eq!(
            kind_from_device_type(2, None),
            Some((NetKind::Wireless, "Wi-Fi"))
        );
        assert_eq!(
            kind_from_device_type(5, None),
            Some((NetKind::Other, "Bluetooth"))
        );
        assert_eq!(
            kind_from_device_type(8, None),
            Some((NetKind::Wireless, "Cellular"))
        );
        assert_eq!(
            kind_from_device_type(10, None),
            Some((NetKind::Virtual, "Bond"))
        );
        assert_eq!(
            kind_from_device_type(11, None),
            Some((NetKind::Virtual, "VLAN"))
        );
        assert_eq!(
            kind_from_device_type(13, None),
            Some((NetKind::Virtual, "Bridge"))
        );
        assert_eq!(
            kind_from_device_type(20, None),
            Some((NetKind::Virtual, "Virtual"))
        );
        assert_eq!(
            kind_from_device_type(30, None),
            Some((NetKind::Wireless, "Wi-Fi P2P"))
        );
        assert_eq!(kind_from_device_type(14, None), None);
    }

    #[test]
    fn channels() {
        assert_eq!(channel_from_mhz(2437), Some(6));
        assert_eq!(channel_from_mhz(2412), Some(1));
        assert_eq!(channel_from_mhz(2484), Some(14));
        assert_eq!(channel_from_mhz(5180), Some(36));
        assert_eq!(channel_from_mhz(5975), Some(5));
        assert_eq!(channel_from_mhz(900), None);
        assert_eq!(channel_from_mhz(2480), None);
    }

    #[test]
    fn security_texts() {
        assert_eq!(security_text(0x1, 0, 0x188), "WPA2 Personal");
        assert_eq!(security_text(0x1, 0, 0x588), "WPA3 Personal");
        assert_eq!(security_text(0x1, 0, 0x288), "WPA2 Enterprise");
        assert_eq!(security_text(0x1, 0x108, 0), "WPA");
        assert_eq!(security_text(0x1, 0, 0), "WEP");
        assert_eq!(security_text(0, 0, 0), "Open");
    }

    fn home_ap(strength: u8) -> ApProps {
        ApProps {
            ssid: b"Home".to_vec(),
            strength,
            frequency: 5180,
            flags: 0x1,
            wpa_flags: 0,
            rsn_flags: 0x188,
        }
    }

    #[test]
    fn wifi_fields() {
        let wifi = wifi_from_ap(&home_ap(72), 866_000);
        assert_eq!(&*wifi.ssid, "Home");
        assert_eq!(wifi.signal_pct, 72);
        assert_eq!(wifi.channel, Some(36));
        assert_eq!(wifi.freq_mhz, 5180);
        assert_eq!(wifi.tx_mbps, Some(866.0));
        assert_eq!(wifi.rx_mbps, None);
        assert_eq!(wifi.signal_dbm, None);
        assert_eq!(&*wifi.security, "WPA2 Personal");
        assert_eq!(wifi_from_ap(&home_ap(72), 0).tx_mbps, None);
    }

    #[test]
    fn hidden_ssid() {
        let ap = ApProps {
            ssid: Vec::new(),
            ..home_ap(40)
        };
        assert_eq!(&*wifi_from_ap(&ap, 0).ssid, "Hidden network");
        // Control bytes never reach the screen.
        let ap = ApProps {
            ssid: b"Cafe\n\x1b[31m".to_vec(),
            ..home_ap(40)
        };
        let ssid = wifi_from_ap(&ap, 0).ssid;
        assert!(!ssid.chars().any(char::is_control), "{ssid:?}");
    }

    // ---- The lab worker over a fake bus ----

    /// The fake NM: `None` when NetworkManager is missing.
    #[derive(Default)]
    struct FakeState {
        nm: Option<Fake>,
        events: VecDeque<Event>,
        opens: u32,
    }

    #[derive(Default)]
    struct Fake {
        devices: Vec<String>,
        objects: HashMap<(String, String), Props>,
    }

    #[derive(Clone, Default)]
    struct FakeConnector(Arc<Mutex<FakeState>>);

    struct FakeBus(Arc<Mutex<FakeState>>);

    fn service_unknown() -> DbusError {
        DbusError::Call {
            name: SERVICE_UNKNOWN.into(),
            message: "not activatable".into(),
        }
    }

    impl Connector for FakeConnector {
        type Bus = FakeBus;

        async fn open(&self, _cx: &Cx) -> Result<FakeBus, DbusError> {
            let mut state = self.0.lock().expect("fake lock");
            state.opens += 1;
            if state.nm.is_none() {
                return Err(service_unknown());
            }
            Ok(FakeBus(Arc::clone(&self.0)))
        }
    }

    impl NmBus for FakeBus {
        async fn devices(&mut self, _cx: &Cx) -> Result<Vec<String>, DbusError> {
            let state = self.0.lock().expect("fake lock");
            Ok(state
                .nm
                .as_ref()
                .ok_or_else(service_unknown)?
                .devices
                .clone())
        }

        async fn props(&mut self, _cx: &Cx, path: &str, iface: &str) -> Result<Props, DbusError> {
            let state = self.0.lock().expect("fake lock");
            let nm = state.nm.as_ref().ok_or_else(service_unknown)?;
            nm.objects
                .get(&(path.to_owned(), iface.to_owned()))
                .map(|p| {
                    p.iter()
                        .map(|(k, v)| (k.clone(), v.try_clone().expect("clone value")))
                        .collect()
                })
                .ok_or_else(|| DbusError::Call {
                    name: "org.freedesktop.DBus.Error.UnknownObject".into(),
                    message: "gone".into(),
                })
        }

        async fn prop(
            &mut self,
            cx: &Cx,
            path: &str,
            iface: &str,
            name: &str,
        ) -> Result<OwnedValue, DbusError> {
            let mut props = self.props(cx, path, iface).await?;
            props.remove(name).ok_or_else(|| DbusError::Call {
                name: "org.freedesktop.DBus.Error.InvalidArgs".into(),
                message: "no such property".into(),
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
    }

    /// The fake nl80211. `links: None` is a machine without a Wi-Fi driver.
    #[derive(Default)]
    struct FakeWifiState {
        links: Option<Vec<WifiLink>>,
        fail: bool,
        opens: u32,
    }

    #[derive(Clone, Default)]
    struct FakeWifi(Arc<Mutex<FakeWifiState>>);

    impl WifiOpener for FakeWifi {
        type Links = FakeWifi;

        fn open(&self) -> io::Result<Option<FakeWifi>> {
            let mut state = self.0.lock().expect("fake lock");
            state.opens += 1;
            Ok(state.links.is_some().then(|| self.clone()))
        }
    }

    impl WifiLinks for FakeWifi {
        fn links(&mut self) -> io::Result<Vec<WifiLink>> {
            let state = self.0.lock().expect("fake lock");
            if state.fail {
                return Err(io::Error::other("dump failed"));
            }
            Ok(state.links.clone().unwrap_or_default())
        }
    }

    fn wlan0_link(signal_dbm: i32) -> WifiLink {
        WifiLink {
            ifindex: 3,
            ssid: Some(b"Home".to_vec()),
            freq_mhz: Some(2437),
            signal_dbm: Some(signal_dbm),
            rx_kbps: Some(400_000),
            tx_kbps: Some(100_000),
        }
    }

    fn set_links(wifi: &FakeWifi, links: Vec<WifiLink>) {
        wifi.0.lock().expect("fake lock").links = Some(links);
    }

    fn props(entries: Vec<(&str, Value<'static>)>) -> Props {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).expect("owned value")))
            .collect()
    }

    fn path(p: &'static str) -> Value<'static> {
        Value::ObjectPath(zbus::zvariant::ObjectPath::from_static_str(p).expect("object path"))
    }

    /// `eth0` (Ethernet) and `wlan0` (Wi-Fi on `AP_PATH`), as in the `netif-kinds` fixture.
    fn eth_and_wifi(strength: u8) -> Fake {
        let mut objects = HashMap::new();
        let mut put = |p: &str, iface: &str, entries| {
            objects.insert((p.to_owned(), iface.to_owned()), props(entries));
        };
        put(
            ETH_PATH,
            DEVICE_IFACE,
            vec![
                ("Interface", Value::from("eth0")),
                ("IpInterface", Value::from("eth0")),
                ("DeviceType", Value::U32(1)),
                ("ActiveConnection", path(ETH_AC)),
            ],
        );
        put(
            ETH_AC,
            ACTIVE_IFACE,
            vec![
                ("Id", Value::from("Wired connection 1")),
                ("Type", Value::from("802-3-ethernet")),
            ],
        );
        put(
            WIFI_PATH,
            DEVICE_IFACE,
            vec![
                ("Interface", Value::from("wlan0")),
                ("IpInterface", Value::from("wlan0")),
                ("DeviceType", Value::U32(2)),
                ("ActiveConnection", path(WIFI_AC)),
            ],
        );
        put(
            WIFI_AC,
            ACTIVE_IFACE,
            vec![
                ("Id", Value::from("Home Wi-Fi")),
                ("Type", Value::from("802-11-wireless")),
            ],
        );
        put(
            WIFI_PATH,
            WIRELESS_IFACE,
            vec![
                ("ActiveAccessPoint", path(AP_PATH)),
                ("Bitrate", Value::U32(866_000)),
            ],
        );
        put(
            AP_PATH,
            AP_IFACE,
            vec![
                ("Ssid", Value::from(b"Home".to_vec())),
                ("Strength", Value::U8(strength)),
                ("Frequency", Value::U32(5180)),
                ("Flags", Value::U32(0x1)),
                ("WpaFlags", Value::U32(0)),
                ("RsnFlags", Value::U32(0x188)),
            ],
        );
        Fake {
            devices: vec![ETH_PATH.to_owned(), WIFI_PATH.to_owned()],
            objects,
        }
    }

    fn set_strength(fake: &Arc<Mutex<FakeState>>, strength: u8) {
        let mut state = fake.lock().expect("fake lock");
        let nm = state.nm.as_mut().expect("nm");
        nm.objects
            .get_mut(&(AP_PATH.to_owned(), AP_IFACE.to_owned()))
            .expect("ap")
            .insert("Strength".into(), OwnedValue::from(strength));
    }

    fn fixture_root() -> SysRoot {
        SysRoot::at(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../tl-collect/tests/fixtures/netif-kinds"),
        )
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
        fn start(connector: FakeConnector, demand: Demand) -> Self {
            Self::start_with(connector, FakeWifi::default(), demand)
        }

        fn start_with(connector: FakeConnector, wifi: FakeWifi, demand: Demand) -> Self {
            let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(1_000_000));
            let (updates, rx) = mpsc::channel::<EngineMsg>(256);
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
                    let _ = run_with(cx, io, fixture_root(), connector, wifi).await;
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
            self.advance(IDLE_RECHECK);
            let report = self.lab.run_until_quiescent_with_report();
            assert!(report.lab_test_passed(), "{report:?}");
        }
    }

    fn nets_of(update: &SourceUpdate) -> Option<&[(DeviceIdentity, NetDetails)]> {
        match update {
            SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Nets {
                from: SourceId::NetworkManager,
                entries,
            }) => Some(entries),
            _ => None,
        }
    }

    fn name_of(identity: &DeviceIdentity) -> &str {
        match identity {
            DeviceIdentity::Net { name, .. } => name,
            other => panic!("not a net identity: {other:?}"),
        }
    }

    fn wifi_strength(entries: &[(DeviceIdentity, NetDetails)]) -> Option<u8> {
        entries
            .iter()
            .find(|(id, _)| name_of(id) == "wlan0")
            .and_then(|(_, d)| d.wifi.as_ref())
            .map(|w| w.signal_pct)
    }

    #[test]
    fn strength_updates_1hz_only_when_nets() {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        fake.lock().expect("fake lock").nm = Some(eth_and_wifi(50));
        let mut rig = Rig::start(
            connector,
            Demand {
                nets: true,
                ..Demand::default()
            },
        );

        let first = rig.drain();
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(
            matches!(
                &first[0],
                SourceUpdate::Health {
                    id: SourceId::NetworkManager,
                    state: SourceState::Running,
                    last_error: None,
                    ..
                }
            ),
            "{:?}",
            first[0]
        );
        let entries = nets_of(&first[1]).expect("Nets");
        assert_eq!(entries.len(), 2, "{entries:?}");
        let (eth_id, eth) = &entries[0];
        assert_eq!(
            eth_id,
            &DeviceIdentity::Net {
                netns_ino: 4026531840,
                ifindex: 2,
                name: "eth0".into(),
            }
        );
        assert_eq!(eth.kind, NetKind::Wired);
        assert_eq!(&*eth.type_label, "Ethernet");
        assert_eq!(eth.connection.as_deref(), Some("Wired connection 1"));
        assert!(eth.wifi.is_none());
        let (wifi_id, wlan) = &entries[1];
        assert_eq!(name_of(wifi_id), "wlan0");
        assert_eq!(wlan.ifindex, 3);
        assert_eq!(wlan.kind, NetKind::Wireless);
        assert_eq!(wlan.connection.as_deref(), Some("Home Wi-Fi"));
        let wifi = wlan.wifi.as_ref().expect("wifi");
        assert_eq!(&*wifi.ssid, "Home");
        assert_eq!(wifi.channel, Some(36));
        assert_eq!(wifi.tx_mbps, Some(866.0));
        assert_eq!(wifi.signal_pct, 50);

        for second in 1..=5u8 {
            set_strength(&fake, 50 + second);
            rig.advance(IDLE_RECHECK);
            assert!(rig.drain().is_empty(), "second {second}: nothing at 0.5 s");
            rig.advance(IDLE_RECHECK);
            let got = rig.drain();
            assert_eq!(got.len(), 1, "second {second}: {got:?}");
            let entries = nets_of(&got[0]).expect("Nets");
            assert_eq!(wifi_strength(entries), Some(50 + second));
        }
        // No change, nothing sent.
        rig.advance(WIFI_PERIOD);
        assert!(rig.drain().is_empty());

        // Information tab only: no 1 s reads.
        rig.demand_tx
            .send_modify(|d| {
                d.nets = false;
                d.inventory = true;
            })
            .expect("demand send");
        for second in 1..=5u8 {
            set_strength(&fake, 10 + second);
            rig.advance(WIFI_PERIOD);
            assert!(rig.drain().is_empty(), "second {second}");
        }

        // A signal on the access point re-reads its device.
        fake.lock()
            .expect("fake lock")
            .events
            .push_back(Event::Changed(AP_PATH.to_owned()));
        rig.advance(IDLE_RECHECK);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(wifi_strength(nets_of(&got[0]).expect("Nets")), Some(15));

        // A removed device changes the list.
        fake.lock()
            .expect("fake lock")
            .events
            .push_back(Event::Removed(WIFI_PATH.to_owned()));
        rig.advance(IDLE_RECHECK);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "{got:?}");
        let entries = nets_of(&got[0]).expect("Nets");
        assert_eq!(entries.len(), 1);
        assert_eq!(name_of(&entries[0].0), "eth0");

        // Neither flag: the worker goes idle and sends nothing.
        rig.demand_tx
            .send_modify(|d| d.inventory = false)
            .expect("demand send");
        rig.advance(RESYNC);
        rig.advance(RESYNC);
        assert!(rig.drain().is_empty());
        assert_eq!(fake.lock().expect("fake lock").opens, 1);
        rig.finish();
    }

    #[test]
    fn nm_missing_degraded() {
        let connector = FakeConnector::default();
        let fake = Arc::clone(&connector.0);
        let mut rig = Rig::start(
            connector,
            Demand {
                inventory: true,
                ..Demand::default()
            },
        );
        let first = rig.drain();
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(
            matches!(&first[0], SourceUpdate::Health {
                id: SourceId::NetworkManager,
                state: SourceState::Running,
                last_error: Some(text),
                ..
            } if &**text == "NetworkManager not running"),
            "{:?}",
            first[0]
        );
        assert_eq!(nets_of(&first[1]).map(<[_]>::len), Some(0));

        // Retried every 60 s; once NM is up the full list goes out.
        rig.advance(Duration::from_secs(30));
        assert_eq!(fake.lock().expect("fake lock").opens, 1);
        fake.lock().expect("fake lock").nm = Some(eth_and_wifi(60));
        for _ in 0..60 {
            rig.advance(IDLE_RECHECK);
        }
        assert_eq!(fake.lock().expect("fake lock").opens, 2);
        let got = rig.drain();
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(
            matches!(
                &got[0],
                SourceUpdate::Health {
                    last_error: None,
                    ..
                }
            ),
            "{:?}",
            got[0]
        );
        assert_eq!(nets_of(&got[1]).map(<[_]>::len), Some(2));
        rig.finish();
    }

    fn wlan0_wifi(entries: &[(DeviceIdentity, NetDetails)]) -> Option<&WifiInfo> {
        entries
            .iter()
            .find(|(id, _)| name_of(id) == "wlan0")
            .and_then(|(_, d)| d.wifi.as_ref())
    }

    #[test]
    fn signal_percent_formula() {
        assert_eq!(signal_pct(-55), 90);
        assert_eq!(signal_pct(-100), 0);
        assert_eq!(signal_pct(-120), 0);
        assert_eq!(signal_pct(-50), 100);
        assert_eq!(signal_pct(-20), 100);
        assert_eq!(signal_pct(i32::MIN), 0);
        let mut link = wlan0_link(-55);
        link.ssid = Some(Vec::new());
        assert_eq!(
            wifi_from_link(&link).map(|w| w.ssid),
            Some("Hidden network".into())
        );
        link.ssid = None;
        assert_eq!(wifi_from_link(&link), None);
    }

    #[test]
    fn nl80211_only_wifi() {
        let wifi = FakeWifi::default();
        set_links(&wifi, vec![wlan0_link(-55)]);
        let mut rig = Rig::start_with(
            FakeConnector::default(),
            wifi.clone(),
            Demand {
                nets: true,
                ..Demand::default()
            },
        );
        let first = rig.drain();
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(
            matches!(&first[0], SourceUpdate::Health {
                state: SourceState::Running,
                last_error: Some(text),
                ..
            } if &**text == "NetworkManager not running"),
            "{:?}",
            first[0]
        );
        let entries = nets_of(&first[1]).expect("Nets");
        assert_eq!(entries.len(), 1, "{entries:?}");
        let (id, details) = &entries[0];
        assert_eq!(name_of(id), "wlan0");
        assert_eq!(details.ifindex, 3);
        assert_eq!(details.kind, NetKind::Wireless);
        assert_eq!(&*details.type_label, "Wi-Fi");
        let w = details.wifi.as_ref().expect("wifi");
        assert_eq!(&*w.ssid, "Home");
        assert_eq!(w.signal_pct, 90);
        assert_eq!(w.signal_dbm, Some(-55));
        assert_eq!(w.channel, Some(6));
        assert_eq!(w.freq_mhz, 2437);
        assert_eq!(&*w.security, "Unknown");
        assert_eq!(w.rx_mbps, Some(400.0));
        assert_eq!(w.tx_mbps, Some(100.0));

        // Each tick reads again; a change goes out.
        set_links(&wifi, vec![wlan0_link(-60)]);
        rig.advance(WIFI_PERIOD);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "{got:?}");
        let w = wlan0_wifi(nets_of(&got[0]).expect("Nets")).expect("wifi");
        assert_eq!(w.signal_pct, 80);
        rig.advance(WIFI_PERIOD);
        assert!(rig.drain().is_empty());
        assert_eq!(wifi.0.lock().expect("fake lock").opens, 1);

        // A failed read: the health text once, then nl80211 stays off with no more reads.
        wifi.0.lock().expect("fake lock").fail = true;
        rig.advance(WIFI_PERIOD);
        let got = rig.drain();
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(
            matches!(&got[0], SourceUpdate::Health {
                state: SourceState::Running,
                last_error: Some(text),
                ..
            } if &**text == "nl80211: dump failed"),
            "{:?}",
            got[0]
        );
        assert_eq!(nets_of(&got[1]).map(<[_]>::len), Some(0));
        for _ in 0..5 {
            rig.advance(WIFI_PERIOD);
        }
        assert!(rig.drain().is_empty());
        assert_eq!(wifi.0.lock().expect("fake lock").opens, 1);
        rig.finish();
    }

    #[test]
    fn nm_strength_wins_dbm_added() {
        let connector = FakeConnector::default();
        connector.0.lock().expect("fake lock").nm = Some(eth_and_wifi(72));
        let wifi = FakeWifi::default();
        set_links(&wifi, vec![wlan0_link(-64)]);
        let mut rig = Rig::start_with(
            connector,
            wifi.clone(),
            Demand {
                nets: true,
                ..Demand::default()
            },
        );
        let first = rig.drain();
        assert_eq!(first.len(), 2, "{first:?}");
        let w = wlan0_wifi(nets_of(&first[1]).expect("Nets")).expect("wifi");
        assert_eq!(w.signal_pct, 72);
        assert_eq!(w.signal_dbm, Some(-64));
        assert_eq!(w.rx_mbps, Some(400.0));
        // NM has a TX rate, so it stays.
        assert_eq!(w.tx_mbps, Some(866.0));
        // NM's SSID, channel and security stay.
        assert_eq!(w.channel, Some(36));
        assert_eq!(&*w.security, "WPA2 Personal");

        // A new dBm on the next tick goes out alone.
        set_links(&wifi, vec![wlan0_link(-70)]);
        rig.advance(WIFI_PERIOD);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "{got:?}");
        let w = wlan0_wifi(nets_of(&got[0]).expect("Nets")).expect("wifi");
        assert_eq!(w.signal_pct, 72);
        assert_eq!(w.signal_dbm, Some(-70));

        // Information tab only: no nl80211 reads, and the dBm is dropped.
        rig.demand_tx
            .send_modify(|d| {
                d.nets = false;
                d.inventory = true;
            })
            .expect("demand send");
        rig.advance(WIFI_PERIOD);
        let got = rig.drain();
        assert_eq!(got.len(), 1, "{got:?}");
        let w = wlan0_wifi(nets_of(&got[0]).expect("Nets")).expect("wifi");
        assert_eq!(w.signal_dbm, None);
        assert_eq!(w.rx_mbps, None);
        assert_eq!(wifi.0.lock().expect("fake lock").opens, 1);
        rig.finish();
    }

    #[test]
    fn spec_is_full_mode_only() {
        let spec = spec();
        assert_eq!(spec.id, SourceId::NetworkManager);
        assert_eq!(spec.name, "networkmanager");
        assert_eq!(spec.modes, &[EngineMode::Full]);
    }
}
