//! Performance tab: device keys, sidebar order and visibility rules, and the `PerformancePage`
//! entity (docs/ui/performance.md UI-PRF-010, UI-PRF-012; PERF-002, PERF-004, PERF-005,
//! PERF-008).

pub mod actions;
pub mod card;
pub mod cpu;
pub mod cpu_details;
pub mod cpu_grid;
pub mod details;
pub mod menu;
pub mod page;
pub mod sidebar;
pub mod summary;

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use gpui_kit::private::schemars::JsonSchema;
use gpui_kit::{AnyView, App, Context, FocusHandle, Focusable};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::settings::DeviceVisibility;

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
            DeviceIdentity::Aggregate(AggregateKind::Nets)
            | DeviceIdentity::CpuLogical { .. }
            | DeviceIdentity::Sensor { .. } => return None,
        };
        Some(DeviceKey(key.into()))
    }

    /// The kind from the key prefix. A key that matches no PERF-005 form (only possible from a
    /// hand-edited settings file) gives `Cpu`; such a key never matches a catalog row.
    pub fn kind(&self) -> DeviceKind {
        let k = &*self.0;
        match k {
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

/// The `[performance]` settings the sidebar rules read.
pub struct PerformanceSettingsView<'a> {
    /// `device_order`.
    pub order: &'a [DeviceKey],
    /// `device_overrides`.
    pub overrides: &'a HashMap<DeviceKey, DeviceVisibility>,
    pub show_disks: bool,
    pub show_gpus: bool,
    pub show_fans: bool,
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
}

/// Fills `out` with the present devices: keys in `settings.order` first, in that order, then
/// the rest by kind (Cpu, Memory, Gpu, Disk, Net, Fan, Battery, AggregateDisk, AggregateGpu)
/// and within a kind in catalog order. `lo` is never included. Runs on catalog changes only.
pub fn sidebar_order(
    catalog: &DeviceCatalog,
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
        out.push(DeviceRow {
            kind: key.kind(),
            key,
            idx: entry.idx,
            generation: entry.generation,
            net_class: entry.net_kind,
        });
    }
    // Stable sort, so rows of one kind keep catalog order.
    out.sort_by_key(|row| {
        let saved = settings.order.iter().position(|k| *k == row.key);
        (saved.unwrap_or(usize::MAX), row.kind as u8)
    });
}

/// A per-device override wins; otherwise the per-type setting decides. Cpu and Memory show by
/// default, aggregates are hidden.
pub fn is_visible(row: &DeviceRow, settings: &PerformanceSettingsView) -> bool {
    match settings.overrides.get(&row.key) {
        Some(DeviceVisibility::Show) => return true,
        Some(DeviceVisibility::Hide) => return false,
        None => {}
    }
    match row.kind {
        DeviceKind::Cpu | DeviceKind::Memory => true,
        DeviceKind::Gpu => settings.show_gpus,
        DeviceKind::Disk => settings.show_disks,
        DeviceKind::Fan => settings.show_fans,
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

/// The selection after the selected row at `removed_at` of `visible` (the rows before the
/// removal) goes away: the next row below, else the one above, else `None`.
pub fn selection_after_removal(visible: &[DeviceRow], removed_at: usize) -> Option<DeviceKey> {
    visible
        .get(removed_at + 1)
        .or_else(|| removed_at.checked_sub(1).and_then(|i| visible.get(i)))
        .map(|row| row.key.clone())
}

/// `saved` if it is visible, else the first visible row.
pub fn initial_selection(visible: &[DeviceRow], saved: Option<&DeviceKey>) -> Option<DeviceKey> {
    saved
        .and_then(|key| visible.iter().find(|row| row.key == *key))
        .or_else(|| visible.first())
        .map(|row| row.key.clone())
}

/// Index of `key` among the rows of its kind in `rows` (the visible rows), from 0: "Disk 0",
/// "GPU 0". 0 when `key` is not in `rows`.
pub fn type_number(rows: &[DeviceRow], key: &DeviceKey) -> u16 {
    let kind = key.kind();
    rows.iter()
        .filter(|row| row.kind == kind)
        .position(|row| row.key == *key)
        .map_or(0, |n| u16::try_from(n).unwrap_or(u16::MAX))
}

/// The Performance tab entity. Later E24 beads add the sidebar and summary fields (R-262).
pub struct PerformancePage {
    // E24-004: sidebar: Entity<sidebar::DeviceSidebar> (added by crw.6 as a chain edit, R-262)
    /// Device pages, created on first visit and kept (ADR-009).
    pub(crate) pages: FxHashMap<DeviceKey, AnyView>,
    pub(crate) selected: Option<DeviceKey>,
    // E24-015: summary: Option<Entity<summary::SummaryView>> (added by crw.17 as a chain edit, R-262)
    /// Not persisted (R-090).
    pub(crate) sidebar_visible: bool,
    /// Key context "Performance".
    pub(crate) focus: FocusHandle,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl PerformancePage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            pages: FxHashMap::default(),
            selected: None,
            sidebar_visible: true,
            focus: cx.focus_handle(),
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        }
    }
}

impl Focusable for PerformancePage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::devices::DeviceEntry;

    fn key(s: &str) -> DeviceKey {
        DeviceKey(Arc::from(s))
    }

    fn entry(idx: u16, identity: DeviceIdentity, present: bool) -> DeviceEntry {
        let net_kind = match &identity {
            DeviceIdentity::Net { name, .. } => Some(match &**name {
                "tailscale0" => NetClass::VpnTunnel,
                "docker0" => NetClass::Virtual,
                "lo" => NetClass::Other,
                _ => NetClass::Wired,
            }),
            _ => None,
        };
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("device"),
            present,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind,
        }
    }

    fn gpu(uuid: [u8; 16], bus: &str) -> DeviceIdentity {
        DeviceIdentity::Gpu {
            uuid,
            pci_bus_id: Arc::from(bus),
        }
    }

    fn disk(name: &str, serial: Option<&str>) -> DeviceIdentity {
        DeviceIdentity::Disk {
            major: 259,
            minor: 0,
            name: Arc::from(name),
            wwn_or_serial: serial.map(Arc::from),
        }
    }

    fn net(ifindex: u32, name: &str) -> DeviceIdentity {
        DeviceIdentity::Net {
            netns_ino: 1,
            ifindex,
            name: Arc::from(name),
        }
    }

    fn catalog(entries: Vec<DeviceEntry>) -> DeviceCatalog {
        DeviceCatalog {
            version: 1,
            entries: entries.into(),
        }
    }

    fn view<'a>(
        order: &'a [DeviceKey],
        overrides: &'a HashMap<DeviceKey, DeviceVisibility>,
    ) -> PerformanceSettingsView<'a> {
        PerformanceSettingsView {
            order,
            overrides,
            show_disks: true,
            show_gpus: true,
            show_fans: true,
            show_batteries: true,
            show_network_wired: true,
            show_network_wireless: true,
            show_network_vpn: true,
            show_network_virtual: false,
            show_network_other: true,
        }
    }

    fn row(k: &str) -> DeviceRow {
        let key = key(k);
        DeviceRow {
            kind: key.kind(),
            key,
            idx: DeviceIdx(0),
            generation: 0,
            net_class: None,
        }
    }

    const UUID: [u8; 16] = [
        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef,
    ];

    #[test]
    fn device_key_forms() {
        let cases: Vec<(DeviceIdentity, Option<&str>, Option<DeviceKind>)> = vec![
            (
                DeviceIdentity::CpuPackage,
                Some("cpu"),
                Some(DeviceKind::Cpu),
            ),
            (DeviceIdentity::CpuLogical { id: 3 }, None, None),
            (
                DeviceIdentity::Memory,
                Some("memory"),
                Some(DeviceKind::Memory),
            ),
            (
                gpu(UUID, "0000:2d:00.0"),
                Some("gpu:GPU-12345678-9abc-def0-0123-456789abcdef"),
                Some(DeviceKind::Gpu),
            ),
            (
                gpu([0; 16], "0000:2d:00.0"),
                Some("gpu:pci:0000:2d:00.0"),
                Some(DeviceKind::Gpu),
            ),
            (
                disk("nvme0n1", Some("eui.0025388b")),
                Some("disk:eui.0025388b"),
                Some(DeviceKind::Disk),
            ),
            (
                disk("nvme0n1", None),
                Some("disk:nvme0n1"),
                Some(DeviceKind::Disk),
            ),
            (
                net(2, "enp38s0"),
                Some("net:enp38s0"),
                Some(DeviceKind::Net),
            ),
            (
                DeviceIdentity::Sensor {
                    chip: Arc::from("k10temp"),
                    label: Arc::from("Tctl"),
                },
                None,
                None,
            ),
            (
                DeviceIdentity::Fan {
                    chip: Arc::from("nct6799"),
                    index: 2,
                },
                Some("fan:nct6799:2"),
                Some(DeviceKind::Fan),
            ),
            (
                DeviceIdentity::Battery {
                    native_path: Arc::from("BAT0"),
                },
                Some("bat:BAT0"),
                Some(DeviceKind::Battery),
            ),
            (
                DeviceIdentity::Aggregate(AggregateKind::Disks),
                Some("aggregate:disk"),
                Some(DeviceKind::AggregateDisk),
            ),
            (
                DeviceIdentity::Aggregate(AggregateKind::Gpus),
                Some("aggregate:gpu"),
                Some(DeviceKind::AggregateGpu),
            ),
            (DeviceIdentity::Aggregate(AggregateKind::Nets), None, None),
        ];
        for (identity, want, want_kind) in cases {
            let got = DeviceKey::from_identity(&identity);
            assert_eq!(got.as_ref().map(|k| &*k.0), want, "{identity:?}");
            assert_eq!(got.map(|k| k.kind()), want_kind, "{identity:?}");
        }
    }

    #[test]
    fn order_saved_then_type_default() {
        let cat = catalog(vec![
            entry(0, DeviceIdentity::CpuPackage, true),
            entry(1, DeviceIdentity::Memory, true),
            entry(2, net(1, "lo"), true),
            entry(3, net(2, "enp38s0"), true),
            entry(4, disk("X", None), true),
            entry(5, gpu([0; 16], "A"), true),
            entry(6, disk("Y", None), true),
            entry(7, net(3, "tailscale0"), true),
            entry(8, disk("gone", None), false),
        ]);
        let order = [key("net:enp38s0"), key("disk:Y"), key("disk:not-present")];
        let overrides = HashMap::new();
        let mut out = Vec::new();
        sidebar_order(&cat, &view(&order, &overrides), &mut out);
        let keys: Vec<&str> = out.iter().map(|r| &*r.key.0).collect();
        assert_eq!(
            keys,
            [
                "net:enp38s0",
                "disk:Y",
                "cpu",
                "memory",
                "gpu:pci:A",
                "disk:X",
                "net:tailscale0"
            ]
        );
        assert_eq!(out[0].idx, DeviceIdx(3));
        assert_eq!(out[0].net_class, Some(NetClass::Wired));
    }

    #[test]
    fn visibility_overrides_and_type_defaults() {
        let order = [];
        let mut overrides = HashMap::new();
        let docker = DeviceRow {
            net_class: Some(NetClass::Virtual),
            ..row("net:docker0")
        };
        let cpu = row("cpu");
        assert!(!is_visible(&docker, &view(&order, &overrides)));
        assert!(is_visible(&cpu, &view(&order, &overrides)));
        assert!(!is_visible(
            &row("aggregate:disk"),
            &view(&order, &overrides)
        ));

        overrides.insert(key("net:docker0"), DeviceVisibility::Show);
        overrides.insert(key("cpu"), DeviceVisibility::Hide);
        assert!(is_visible(&docker, &view(&order, &overrides)));
        assert!(!is_visible(&cpu, &view(&order, &overrides)));

        let no_disks = PerformanceSettingsView {
            show_disks: false,
            ..view(&order, &overrides)
        };
        assert!(!is_visible(&row("disk:X"), &no_disks));
        let rows = [row("cpu"), row("memory"), row("disk:X")];
        let shown: Vec<&str> = visible_rows(&rows, &no_disks).map(|r| &*r.key.0).collect();
        assert_eq!(shown, ["memory"]);
    }

    #[test]
    fn selected_device_removed_selects_next() {
        let rows = [row("disk:a"), row("disk:b"), row("disk:c")];
        assert_eq!(selection_after_removal(&rows, 1), Some(key("disk:c")));
        assert_eq!(selection_after_removal(&rows, 2), Some(key("disk:b")));
        assert_eq!(selection_after_removal(&rows[..1], 0), None);
    }

    #[test]
    fn initial_selection_falls_back_to_first() {
        let visible = [row("cpu"), row("disk:X")];
        assert_eq!(
            initial_selection(&visible, Some(&key("disk:Y"))),
            Some(key("cpu"))
        );
        assert_eq!(
            initial_selection(&visible, Some(&key("disk:X"))),
            Some(key("disk:X"))
        );
        assert_eq!(initial_selection(&visible, None), Some(key("cpu")));
        assert_eq!(initial_selection(&[], None), None);
    }

    #[test]
    fn type_number_counts_visible_of_kind() {
        let visible = [row("cpu"), row("disk:X"), row("gpu:pci:A"), row("disk:Y")];
        assert_eq!(type_number(&visible, &key("disk:Y")), 1);
        assert_eq!(type_number(&visible, &key("disk:X")), 0);
        assert_eq!(type_number(&visible, &key("gpu:pci:A")), 0);
    }
}
