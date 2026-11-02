//! The shared base of the Performance tab (ADR-020 layer tl-ui-perf-core): device keys and
//! kinds, the sidebar order and visibility rules, the perf actions, the Details model, the core
//! clock charts, the page padding, and the helpers the Overview and the widgets share with the
//! device pages (GPU engines, CPU secondary chart, sensors, scales, line alphas). The Overview,
//! the widgets and the device pages build on it; it names none of them.

pub mod actions;
pub mod core_clocks;
pub mod cpu;
pub mod details;
pub mod gpu;
pub mod look;
pub mod scales;
pub mod sensors;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use gpui_kit::Hsla;
use gpui_kit::private::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::system::{SensorKind, SensorReading};
use tl_ui_settings::settings::DeviceVisibility;

use crate::components::icons::TlIcon;
use crate::theme::TlTheme;
use crate::theme::palette::{DeviceAccent, accent_color};

/// Page padding on every side, × ui_scale. `page_pad`.
pub const PAGE_PAD: f32 = 16.;
/// Height of the CPU Overall chart, × ui_scale.
pub const GRAPH_H_OVERALL: f32 = 300.;

/// PERF-054 type class of a network interface, as the engine reports it in the catalog.
pub use tl_model::devices::NetKind as NetClass;

/// Persistent device key used in settings and state, for example `cpu`, `gpu:GPU-<uuid>`,
/// `disk:<wwn>` (PERF-005).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "gpui_kit::private::schemars")]
pub struct DeviceKey(pub Arc<str>);

/// Device classes that have a sidebar card, in the default sidebar order (PERF-008).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "gpui_kit::private::schemars")]
pub enum DeviceKind {
    /// The Overview page (D-022). Not a catalog device; first by default.
    Overview,
    Cpu,
    Memory,
    Gpu,
    Disk,
    Net,
    Fan,
    Battery,
    AggregateDisk,
    AggregateGpu,
}

impl DeviceKey {
    pub const OVERVIEW: &'static str = "overview";
    pub const CPU: &'static str = "cpu";
    pub const MEMORY: &'static str = "memory";
    pub const AGGREGATE_DISK: &'static str = "aggregate:disk";
    pub const AGGREGATE_GPU: &'static str = "aggregate:gpu";

    /// The PERF-005 key of a device. `None` for identities that have no card: `CpuLogical`,
    /// `Sensor` and the network aggregate.
    pub fn from_identity(id: &DeviceIdentity) -> Option<DeviceKey> {
        let key: String = match id {
            DeviceIdentity::CpuPackage => Self::CPU.into(),
            DeviceIdentity::Memory => Self::MEMORY.into(),
            DeviceIdentity::Gpu { uuid, pci_bus_id } => {
                if *uuid == [0; 16] {
                    format!("gpu:pci:{pci_bus_id}")
                } else {
                    let mut s = String::with_capacity(4 + 4 + 36);
                    s.push_str("gpu:GPU-");
                    for (i, b) in uuid.iter().enumerate() {
                        if matches!(i, 4 | 6 | 8 | 10) {
                            s.push('-');
                        }
                        let _ = write!(s, "{b:02x}");
                    }
                    s
                }
            }
            DeviceIdentity::Disk {
                wwn_or_serial: Some(serial),
                ..
            } => format!("disk:{serial}"),
            DeviceIdentity::Disk {
                wwn_or_serial: None,
                name,
                ..
            } => format!("disk:{name}"),
            DeviceIdentity::Net { name, .. } => format!("net:{name}"),
            DeviceIdentity::Fan { chip, index } => format!("fan:{chip}:{index}"),
            DeviceIdentity::Battery { native_path } => format!("bat:{native_path}"),
            DeviceIdentity::Aggregate(AggregateKind::Disks) => Self::AGGREGATE_DISK.into(),
            DeviceIdentity::Aggregate(AggregateKind::Gpus) => Self::AGGREGATE_GPU.into(),
            DeviceIdentity::Aggregate(AggregateKind::Nets | AggregateKind::NetsOf(_))
            | DeviceIdentity::CpuLogical { .. }
            | DeviceIdentity::Sensor { .. }
            | DeviceIdentity::MemGroup { .. } => return None,
        };
        Some(DeviceKey(key.into()))
    }

    /// The kind from the key prefix. A key that matches no PERF-005 form (only possible from a
    /// hand-edited settings file) gives `Cpu`; such a key never matches a catalog row.
    pub fn kind(&self) -> DeviceKind {
        let k = &*self.0;
        match k {
            Self::OVERVIEW => DeviceKind::Overview,
            Self::CPU => DeviceKind::Cpu,
            Self::MEMORY => DeviceKind::Memory,
            Self::AGGREGATE_DISK => DeviceKind::AggregateDisk,
            Self::AGGREGATE_GPU => DeviceKind::AggregateGpu,
            _ if k.starts_with("gpu:") => DeviceKind::Gpu,
            _ if k.starts_with("disk:") => DeviceKind::Disk,
            _ if k.starts_with("net:") => DeviceKind::Net,
            _ if k.starts_with("fan:") => DeviceKind::Fan,
            _ if k.starts_with("bat:") => DeviceKind::Battery,
            _ => DeviceKind::Cpu,
        }
    }
}

impl DeviceKind {
    /// The device icon of UI-PRF-058 for box titles and page headers. Sidebar cards draw none
    /// (D-033). The Overview has none.
    pub const fn icon(self) -> Option<TlIcon> {
        match self {
            Self::Overview => None,
            Self::Cpu => Some(TlIcon::Cpu),
            Self::Memory => Some(TlIcon::MemoryStick),
            Self::Gpu | Self::AggregateGpu => Some(TlIcon::Gpu),
            Self::Disk | Self::AggregateDisk => Some(TlIcon::HardDrive),
            Self::Net => Some(TlIcon::Network),
            Self::Fan => Some(TlIcon::Fan),
            Self::Battery => Some(TlIcon::Battery),
        }
    }

    /// The tint of the device icon: the device graph colour. Fan and Battery use the Violet and
    /// Mint of their graphs (UI-PRF-005). The Overview has no icon; it gets the theme accent.
    pub fn icon_color(self, theme: &TlTheme) -> Hsla {
        let family = match self {
            Self::Cpu => DeviceAccent::Cpu,
            Self::Memory => DeviceAccent::Memory,
            Self::Gpu | Self::AggregateGpu => DeviceAccent::Gpu,
            Self::Disk | Self::AggregateDisk => DeviceAccent::Disk,
            Self::Net => DeviceAccent::Net,
            Self::Fan => return accent_color("violet").unwrap_or(theme.graph.gpu_line),
            Self::Battery => return accent_color("mint").unwrap_or(theme.graph.gpu_line),
            Self::Overview => return theme.accent,
        };
        theme.graph.device_accent(family)
    }
}

/// The `[performance]` settings the sidebar rules read.
pub struct PerformanceSettingsView<'a> {
    /// `device_order`.
    pub order: &'a [DeviceKey],
    /// `device_overrides`.
    pub overrides: &'a HashMap<DeviceKey, DeviceVisibility>,
    pub show_disks: bool,
    pub show_gpus: bool,
    pub show_fans: bool,
    /// Show a fan that never spun this session and has no label (PERF-076).
    pub show_disconnected_fans: bool,
    pub show_batteries: bool,
    pub show_network_wired: bool,
    pub show_network_wireless: bool,
    pub show_network_vpn: bool,
    pub show_network_virtual: bool,
    pub show_network_other: bool,
}

/// One present device in sidebar order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeviceRow {
    pub key: DeviceKey,
    pub kind: DeviceKind,
    pub idx: DeviceIdx,
    /// A device that comes back gets `generation + 1` and so a new row (PERF-005).
    pub generation: u32,
    /// `Some` for `Net` rows.
    pub net_class: Option<NetClass>,
    /// `Some` for `Fan` rows: the fan record's `connected` flag (PERF-076).
    pub fan_connected: Option<bool>,
}

/// The `fan_connected` flag of the fan reading at `idx`; false when there is none.
pub fn fan_connected(sensors: &[(DeviceIdx, SensorReading)], idx: DeviceIdx) -> bool {
    sensors
        .iter()
        .any(|(i, r)| *i == idx && r.kind == SensorKind::Fan && r.fan_connected)
}

/// Fills `out` with the present devices: keys in `settings.order` first, in that order, then
/// the rest by kind (Overview, Cpu, Memory, Gpu, Disk, Net, Fan, Battery, AggregateDisk,
/// AggregateGpu) and within a kind in catalog order. `lo` is never included. The Overview row
/// is added when the CPU package is present and carries its index (D-022). Runs on catalog
/// changes only.
pub fn sidebar_order(
    catalog: &DeviceCatalog,
    sensors: &[(DeviceIdx, SensorReading)],
    settings: &PerformanceSettingsView,
    out: &mut Vec<DeviceRow>,
) {
    out.clear();
    for entry in catalog.entries.iter().filter(|e| e.present) {
        if matches!(&entry.identity, DeviceIdentity::Net { name, .. } if &**name == "lo") {
            continue;
        }
        let Some(key) = DeviceKey::from_identity(&entry.identity) else {
            continue;
        };
        let kind = key.kind();
        // An aggregate card needs two or more devices of its type (PERF-009).
        let too_few = match kind {
            DeviceKind::AggregateDisk => count_present(catalog, is_whole_disk) < 2,
            DeviceKind::AggregateGpu => count_present(catalog, is_gpu) < 2,
            _ => false,
        };
        if too_few {
            continue;
        }
        out.push(DeviceRow {
            kind,
            key,
            idx: entry.idx,
            generation: entry.generation,
            net_class: entry.net_kind,
            fan_connected: (kind == DeviceKind::Fan).then(|| fan_connected(sensors, entry.idx)),
        });
    }
    if let Some(cpu) = catalog
        .entries
        .iter()
        .find(|e| e.present && matches!(e.identity, DeviceIdentity::CpuPackage))
    {
        out.push(DeviceRow {
            key: DeviceKey(DeviceKey::OVERVIEW.into()),
            kind: DeviceKind::Overview,
            idx: cpu.idx,
            generation: 0,
            net_class: None,
            fan_connected: None,
        });
    }
    // Stable sort, so rows of one kind keep catalog order.
    out.sort_by_key(|row| {
        let saved = settings.order.iter().position(|k| *k == row.key);
        (saved.unwrap_or(usize::MAX), row.kind as u8)
    });
}

/// Removes disk children from a sidebar list. The engine keeps partitions and device-mapper
/// devices in the catalog for attribution, but only their whole parent disks get cards.
pub(crate) fn retain_whole_disks(rows: &mut Vec<DeviceRow>, catalog: &DeviceCatalog) {
    rows.retain(|row| {
        row.kind != DeviceKind::Disk
            || catalog
                .entries
                .iter()
                .find(|entry| entry.idx == row.idx)
                .is_none_or(|entry| entry.parent.is_none())
    });
}

fn is_whole_disk(e: &DeviceEntry) -> bool {
    e.parent.is_none() && matches!(e.identity, DeviceIdentity::Disk { .. })
}

fn is_gpu(e: &DeviceEntry) -> bool {
    matches!(e.identity, DeviceIdentity::Gpu { .. })
}

/// The present entries of `catalog` that satisfy `pred`.
fn count_present(catalog: &DeviceCatalog, pred: fn(&DeviceEntry) -> bool) -> usize {
    catalog
        .entries
        .iter()
        .filter(|e| e.present && pred(e))
        .count()
}

/// A per-device override wins; otherwise the per-type setting decides. Overview, Cpu and Memory
/// show by default, aggregates are hidden.
pub fn is_visible(row: &DeviceRow, settings: &PerformanceSettingsView) -> bool {
    match settings.overrides.get(&row.key) {
        Some(DeviceVisibility::Show) => return true,
        Some(DeviceVisibility::Hide) => return false,
        None => {}
    }
    match row.kind {
        DeviceKind::Cpu | DeviceKind::Memory | DeviceKind::Overview => true,
        DeviceKind::Gpu => settings.show_gpus,
        DeviceKind::Disk => settings.show_disks,
        DeviceKind::Fan => {
            settings.show_fans
                && (row.fan_connected != Some(false) || settings.show_disconnected_fans)
        }
        DeviceKind::Battery => settings.show_batteries,
        DeviceKind::Net => match row.net_class {
            Some(NetClass::Wired) => settings.show_network_wired,
            Some(NetClass::Wireless) => settings.show_network_wireless,
            Some(NetClass::VpnTunnel) => settings.show_network_vpn,
            Some(NetClass::Virtual) => settings.show_network_virtual,
            Some(NetClass::Other) | None => settings.show_network_other,
        },
        DeviceKind::AggregateDisk | DeviceKind::AggregateGpu => false,
    }
}

pub fn visible_rows<'a>(
    rows: &'a [DeviceRow],
    settings: &PerformanceSettingsView,
) -> impl Iterator<Item = &'a DeviceRow> {
    rows.iter().filter(move |row| is_visible(row, settings))
}

/// `DeviceIdx` per logical CPU id from the catalog. An id the catalog skips gets
/// `DeviceIdx(u16::MAX)`, which matches no series.
pub(crate) fn logical_devices(catalog: &DeviceCatalog, out: &mut Vec<DeviceIdx>) {
    out.clear();
    for e in catalog.entries.iter() {
        if let DeviceIdentity::CpuLogical { id } = e.identity {
            let i = usize::from(id);
            if out.len() <= i {
                out.resize(i + 1, DeviceIdx(u16::MAX));
            }
            out[i] = e.idx;
        }
    }
}
