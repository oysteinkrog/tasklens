//! The `Net` worker (ADR-004, ADR-008): wraps the tl-collect `NetSampler` and turns each
//! `NetReading` into a `SourceUpdate::Nets(NetSample)`.
//!
//! It samples `/proc/net/dev` at 1 Hz while network is demanded or history recording is on, and
//! not at all otherwise. The interfaces are enumerated again every 5 s; each enumeration also
//! sends a full `DeviceList` for `NETS` through `take_extra`, then the sysfs and udev
//! `NetDetails` when they differ from the last sent set. Loopback never appears (ADR-016, R-052,
//! PERF-054 to PERF-056). MAC addresses are never logged.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use tl_collect::SysRoot;
use tl_collect::counters::{Clock, Measured};
use tl_collect::parse::netif::{self, IfaceDetails, IfaceKind, NetIface, OperState};
use tl_collect::parse::udev::{UdevProps, net_entry, read_udev_props};
use tl_collect::sample::net::{IfaceId, NetReading, NetSampler};
use tl_model::demand::Demand;
use tl_model::devices::{DeviceIdentity, NetKind};
use tl_model::health::SourceId;
use tl_model::ids::DeviceIdx;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{InvField, NetDetails};
use tl_model::system::{LinkState, Metric, NetTotals};
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{DeviceDetailsUpdate, NetSample, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;
use crate::workers::enumerate::{self, EnumClock, GenTracker};

pub const PERIOD: Duration = Duration::from_secs(1);

/// The udev keys read for the adapter name (R-140 item 1).
const NET_UDEV_KEYS: [&str; 2] = ["ID_VENDOR_FROM_DATABASE", "ID_MODEL_FROM_DATABASE"];

/// The network rate table (ADR-008 row "Disks, network interfaces").
pub fn net_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    if d.nets || d.recording {
        Some(PERIOD)
    } else {
        None
    }
}

pub struct NetSource {
    root: SysRoot,
    sampler: NetSampler,
    /// Reused every sample.
    reading: NetReading,
    /// The last successful enumeration.
    ifaces: Vec<NetIface>,
    /// Identity and sampler id of every interface, in sampler order.
    ids: Vec<(DeviceIdentity, IfaceId)>,
    /// Link speed of every interface, in sampler order, kept until the next enumeration.
    speeds: Vec<Measured<u32>>,
    /// Keyed by (netns_ino, ifindex, name).
    gens: GenTracker<(u64, u32, Arc<str>)>,
    /// When the interfaces are enumerated again.
    clock: EnumClock,
    /// The `Devices` list and then the `DeviceDetails` waiting for `take_extra`.
    pending: VecDeque<SourceUpdate>,
    /// The last `NetDetails` sent.
    sent: Vec<(DeviceIdentity, NetDetails)>,
    /// Reused on each enumeration.
    buf: Vec<u8>,
    /// Reused for every udev database file.
    udev: UdevProps,
}

impl NetSource {
    pub fn new(root: SysRoot) -> Self {
        Self {
            sampler: NetSampler::new(root.clone()),
            root,
            reading: NetReading::default(),
            ifaces: Vec::new(),
            ids: Vec::new(),
            speeds: Vec::new(),
            gens: GenTracker::default(),
            clock: EnumClock::default(),
            pending: VecDeque::new(),
            sent: Vec::new(),
            buf: Vec::new(),
            udev: UdevProps::default(),
        }
    }

    /// The sampler ids of the last successful enumeration.
    pub fn iface_ids(&self) -> impl Iterator<Item = &IfaceId> + '_ {
        self.ids.iter().map(|(_, id)| id)
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        // `run` drains every extra after each sample, so anything left is from an older tick.
        self.pending.clear();
        if self.clock.due(clock.boot_ns) {
            self.enumerate(clock.boot_ns);
        }
        self.sampler.sample_at(clock, &mut self.reading);

        let nets = self
            .ids
            .iter()
            .zip(&self.reading.ifaces)
            .zip(&self.speeds)
            .map(|(((identity, _), r), speed)| {
                let totals = NetTotals {
                    // The worker cannot know the catalog index; the reducer sets it.
                    device: DeviceIdx(0),
                    rx_bps: metric(r.rx_bps),
                    tx_bps: metric(r.tx_bps),
                    rx_total: r.total_rx_bytes.value,
                    tx_total: r.total_tx_bytes.value,
                    link_speed_mbps: metric(*speed),
                    state: link_state(r.state),
                };
                (identity.clone(), totals)
            })
            .collect();
        let sample = NetSample {
            t_ns: self.reading.t_ns,
            nets,
        };
        (self, SourceUpdate::Nets(sample))
    }

    /// Rebuilds `ids`, `speeds`, the sampler's interface list, the pending `Devices` list and,
    /// when they changed, the pending `NetDetails`. A failed enumeration keeps the previous
    /// interfaces and sends nothing.
    fn enumerate(&mut self, t_ns: u64) {
        let mut found = Vec::new();
        if netif::enumerate(&self.root, &mut found).is_err() {
            return;
        }
        self.ifaces = found;

        let keys: Vec<(u64, u32, Arc<str>)> = self
            .ifaces
            .iter()
            .map(|i| (i.netns_ino, i.ifindex, i.name.clone()))
            .collect();
        let generations = self.gens.update(&keys);
        self.ids = self
            .ifaces
            .iter()
            .zip(generations)
            .map(|(iface, generation)| {
                let id = IfaceId {
                    name: iface.name.clone(),
                    generation,
                };
                (enumerate::net_identity(iface), id)
            })
            .collect();
        let sampler_ids: Vec<IfaceId> = self.ids.iter().map(|(_, id)| id.clone()).collect();
        self.sampler.set_interfaces(&sampler_ids);

        let mut list = enumerate::net_list(t_ns, &self.ifaces, &self.root.sys);
        for (desc, iface) in list.entries.iter_mut().zip(&self.ifaces) {
            desc.net_kind = Some(net_kind(iface.kind).0);
        }
        self.pending.push_back(SourceUpdate::Devices(list));

        let mut speeds = Vec::with_capacity(self.ifaces.len());
        let mut entries = Vec::with_capacity(self.ifaces.len());
        for ((identity, _), iface) in self.ids.iter().zip(&self.ifaces) {
            let details = netif::read_details(&self.root, &iface.name, &mut self.buf);
            let state = netif::read_oper_state(&self.root, &iface.name, &mut self.buf);
            let path = self
                .root
                .run_path(&format!("udev/data/{}", net_entry(iface.ifindex)));
            let props = read_udev_props(&path, &NET_UDEV_KEYS, &mut self.udev);
            let adapter_name = adapter_name(props.map(|()| &self.udev));
            speeds.push(details.speed_mbps);
            entries.push((
                identity.clone(),
                net_details(iface.kind, details, link_state(state), adapter_name),
            ));
        }
        self.speeds = speeds;
        let changed = entries.len() != self.sent.len()
            || entries
                .iter()
                .zip(&self.sent)
                .any(|((ia, a), (ib, b))| ia != ib || !same_details(a, b));
        if changed {
            self.sent = entries.clone();
            self.pending
                .push_back(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Nets {
                    from: SourceId::Net,
                    entries,
                }));
        }
    }
}

impl Source for NetSource {
    fn id(&self) -> SourceId {
        SourceId::Net
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        net_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    fn reset_baselines(&mut self) {
        self.sampler.reset_baselines();
        self.clock.force();
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.pending.pop_front()
    }
}

/// In the full engine only.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Net,
        name: "net",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// A new `NetSource` per generation, built on the blocking pool, then `source::run`.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let root = cfg.sys_root.clone();
        async move {
            let mut job = match cx.spawn_blocking(move |_| NetSource::new(root)) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let src = match job.join(&cx).await {
                Ok(src) => src,
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
            source::run(cx, io, src).await
        }
    })
}

/// The sidebar class and the PERF-054 "Type" label of an interface kind (R-162 OI-9). tl-collect
/// has no VPN kind, so VPN interfaces arrive as `Tunnel`; the NetworkManager worker refines them.
pub fn net_kind(k: IfaceKind) -> (NetKind, &'static str) {
    match k {
        IfaceKind::Ethernet => (NetKind::Wired, "Ethernet"),
        IfaceKind::UsbNetwork => (NetKind::Wired, "USB network"),
        IfaceKind::WiFi => (NetKind::Wireless, "Wi-Fi"),
        IfaceKind::Cellular => (NetKind::Wireless, "Cellular"),
        IfaceKind::Tunnel => (NetKind::VpnTunnel, "Tunnel"),
        IfaceKind::Virtual => (NetKind::Virtual, "Virtual"),
        IfaceKind::Bridge => (NetKind::Virtual, "Bridge"),
        IfaceKind::Other => (NetKind::Other, "Other"),
    }
}

/// `Unknown` when the reading is not `Valid`.
fn link_state(m: Measured<OperState>) -> LinkState {
    if m.validity != Validity::Valid {
        return LinkState::Unknown;
    }
    match m.value {
        OperState::Connected => LinkState::Up,
        OperState::Disconnected => LinkState::Down,
        OperState::Testing => LinkState::Testing,
        OperState::Dormant => LinkState::Dormant,
        OperState::NotPresent => LinkState::NotPresent,
        OperState::LowerLayerDown => LinkState::LowerLayerDown,
        OperState::Unknown => LinkState::Unknown,
    }
}

/// "<vendor> <model>", either alone when the other is missing. Neither gives `Unsupported`
/// ("Unknown adapter"); a file that cannot be read gives its error's validity.
fn adapter_name(props: Result<&UdevProps, tl_collect::CollectError>) -> InvField<Arc<str>> {
    let props = match props {
        Ok(props) => props,
        Err(err) => return text_field(None, err.validity()),
    };
    let part = |key| props.get(key).map(str::trim).filter(|v| !v.is_empty());
    let name = match (
        part("ID_VENDOR_FROM_DATABASE"),
        part("ID_MODEL_FROM_DATABASE"),
    ) {
        (Some(vendor), Some(model)) => Some(format!("{vendor} {model}")),
        (Some(one), None) | (None, Some(one)) => Some(one.to_owned()),
        (None, None) => None,
    };
    text_field(name.map(Arc::from), Validity::Unsupported)
}

/// `Valid(value)`, or an empty value with `validity` when there is none.
fn text_field(value: Option<Arc<str>>, validity: Validity) -> InvField<Arc<str>> {
    match value {
        Some(value) => InvField {
            value,
            validity: Validity::Valid,
        },
        None => InvField {
            value: Arc::from(""),
            validity,
        },
    }
}

fn net_details(
    kind: IfaceKind,
    details: IfaceDetails,
    state: LinkState,
    adapter_name: InvField<Arc<str>>,
) -> NetDetails {
    let (kind, type_label) = net_kind(kind);
    let mac = match details.mac.validity {
        Validity::Valid => text_field(details.mac.value, Validity::Unsupported),
        validity => text_field(None, validity),
    };
    NetDetails {
        type_label: Arc::from(type_label),
        connection: None,
        adapter_name,
        driver: text_field(details.driver, Validity::Unsupported),
        mac,
        ifindex: details.ifindex,
        link_speed_mbps: InvField {
            value: details.speed_mbps.value,
            validity: details.speed_mbps.validity,
        },
        state,
        // E12-015 fills the addresses.
        addresses: Arc::from([]),
        kind,
        wifi: None,
    }
}

/// `NetDetails` has no `PartialEq`; this compares every field the worker sets.
fn same_details(a: &NetDetails, b: &NetDetails) -> bool {
    a.type_label == b.type_label
        && a.connection == b.connection
        && a.adapter_name == b.adapter_name
        && a.driver == b.driver
        && a.mac == b.mac
        && a.ifindex == b.ifindex
        && a.link_speed_mbps == b.link_speed_mbps
        && a.state == b.state
        && a.addresses == b.addresses
        && a.kind == b.kind
        && a.wifi == b.wifi
}

fn metric<T: Copy + Default>(m: Measured<T>) -> Metric<T> {
    Metric {
        value: m.value,
        validity: m.validity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_table() {
        let s = EngineSettings::default();
        assert_eq!(net_period(&Demand::default(), &s), None);

        let nets = Demand {
            nets: true,
            ..Demand::default()
        };
        assert_eq!(net_period(&nets, &s), Some(Duration::from_secs(1)));

        let recording = Demand {
            recording: true,
            ..Demand::default()
        };
        assert_eq!(net_period(&recording, &s), Some(Duration::from_secs(1)));
    }
}
