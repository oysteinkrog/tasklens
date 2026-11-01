//! The members of a group widget (ADR-019 "Group widgets", UI-PRF-061 phase 2).
//!
//! A pure function of the snapshot and the settings, so the rules are tested without a window:
//!
//! - `Cores`: every present logical CPU, by logical id (C0, C1, …). Each member is the source
//!   `{ device = "cpu:<n>", metric }`.
//! - `Disks`: the whole disks of the sidebar's visible disk cards, in sidebar order
//!   (`device_order`), as the Disk box lists them. Partitions never.
//! - `Adapters`: the present adapters the Network box counts (not `lo`, a class `show_network_*`
//!   shows), in sidebar order.
//! - `Engines`: the engines the GPU reports, in the GPU page's grid order (`grid_engines`).
//!   `performance.hidden_gpu_engines` leaves engines out only in a flow layout.
//!
//! The order is fixed; the user cannot reorder members. A group whose own device is gone (a GPU)
//! has no member list: it shows "Not connected". An unknown group kind shows "Needs a newer
//! tasklens".

use std::collections::HashMap;
use std::sync::Arc;

use tl_model::devices::DeviceIdentity;
use tl_model::layout::{
    DeviceSelector, GroupKind, GroupSelector, METRIC_ENGINE_ANY, MetricKey, SourceSelector,
};
use tl_model::layout_boxes::CPU_LOGICAL_PREFIX;
use tl_model::settings::PerformanceSettings;
use tl_model::snapshot::Snapshot;

use crate::pages::performance::gpu::{GpuVendor, gpu_totals, grid_engines};
use crate::pages::performance::sidebar::retain_whole_disks;
use crate::pages::performance::{
    DeviceKey, DeviceKind, DeviceRow, PerformanceSettingsView, sidebar_order, visible_rows,
};

/// The name of loopback, which the Network box never counts.
const LOOPBACK: &str = "lo";

/// The members of a group now.
#[derive(Clone, Debug, PartialEq)]
pub enum Members {
    /// The members in their fixed order, each the source of its metric. Empty: "None found".
    List(Vec<SourceSelector>),
    /// The group's own device is gone (a GPU, or a slot with no GPU): "Not connected".
    Gone,
    /// A group kind this build does not know: "Needs a newer tasklens".
    Unknown,
}

impl Members {
    /// The member sources; empty unless a list.
    pub fn list(&self) -> &[SourceSelector] {
        match self {
            Members::List(l) => l,
            Members::Gone | Members::Unknown => &[],
        }
    }
}

/// The metric each member of `group` shows: `engine:<key>` of each engine for an engine group
/// (whose metric is `engine:*`), else the group's metric.
fn member_metric(group: &GroupSelector, engine: Option<&str>) -> MetricKey {
    match engine {
        Some(key) if group.metric.as_str() == METRIC_ENGINE_ANY => {
            MetricKey::new(&format!("engine:{key}"))
        }
        _ => group.metric.clone(),
    }
}

/// The members of `group` on `snap`. `perf` is `[performance]` (`None` before settings load:
/// every disk and adapter class shows, no engine is hidden). `slot_key` is the device key of
/// the GPU in the group's slot (ignored for a `Key` device). `flow` says the group is in a flow
/// layout, where `hidden_gpu_engines` applies.
pub fn members(
    group: &GroupSelector,
    snap: &Snapshot,
    perf: Option<&PerformanceSettings>,
    slot_key: Option<&str>,
    flow: bool,
) -> Members {
    let one = |device: Arc<str>, engine: Option<&str>| SourceSelector {
        device: DeviceSelector::Key(device),
        metric: member_metric(group, engine),
    };
    match &group.members {
        GroupKind::Other(_) => Members::Unknown,
        GroupKind::Cores => {
            let mut ids: Vec<u16> = snap
                .devices
                .entries
                .iter()
                .filter(|e| e.present)
                .filter_map(|e| match e.identity {
                    DeviceIdentity::CpuLogical { id } => Some(id),
                    _ => None,
                })
                .collect();
            ids.sort_unstable();
            ids.dedup();
            Members::List(
                ids.into_iter()
                    .map(|n| one(Arc::from(format!("{CPU_LOGICAL_PREFIX}{n}")), None))
                    .collect(),
            )
        }
        GroupKind::Disks => Members::List(
            sidebar_rows(snap, perf, DeviceKind::Disk)
                .into_iter()
                .map(|row| one(row.key.0, None))
                .collect(),
        ),
        GroupKind::Adapters => {
            let shows = |row: &DeviceRow| {
                let entry = snap.devices.entries.iter().find(|e| e.idx == row.idx);
                let loopback = entry.is_some_and(
                    |e| matches!(&e.identity, DeviceIdentity::Net { name, .. } if &**name == LOOPBACK),
                );
                !loopback && perf.is_none_or(|p| p.shows_net_kind(entry.and_then(|e| e.net_kind)))
            };
            Members::List(
                all_rows(snap, perf)
                    .into_iter()
                    .filter(|row| row.kind == DeviceKind::Net && shows(row))
                    .map(|row| one(row.key.0, None))
                    .collect(),
            )
        }
        GroupKind::Engines => {
            let key: Option<&str> = match &group.device {
                DeviceSelector::Key(k) => Some(k),
                DeviceSelector::Slot(_) => slot_key,
            };
            let Some(key) = key else {
                return Members::Gone;
            };
            let Some(entry) = snap.devices.entries.iter().filter(|e| e.present).find(|e| {
                &*e.persistent_key == key
                    || DeviceKey::from_identity(&e.identity).is_some_and(|k| &*k.0 == key)
            }) else {
                return Members::Gone;
            };
            let vendor = GpuVendor::of(snap, entry.idx);
            let totals = gpu_totals(snap, entry.idx);
            let hidden: &[String] = match perf {
                Some(p) if flow => &p.hidden_gpu_engines,
                _ => &[],
            };
            Members::List(
                grid_engines(vendor, totals, hidden)
                    .map(|(engine, _)| SourceSelector {
                        // A slot group's members keep the slot, so they follow the box's GPUs.
                        device: group.device.clone(),
                        metric: member_metric(group, Some(engine.key())),
                    })
                    .collect(),
            )
        }
    }
}

/// The present devices in sidebar order, as the sidebar lists them before visibility.
fn all_rows(snap: &Snapshot, perf: Option<&PerformanceSettings>) -> Vec<DeviceRow> {
    let (order, overrides) = view_parts(perf);
    let view = settings_view(perf, &order, &overrides);
    let mut rows = Vec::new();
    sidebar_order(&snap.devices, &snap.system.sensors, &view, &mut rows);
    rows
}

/// The visible sidebar cards of `kind`, in sidebar order. Disks are whole disks only.
fn sidebar_rows(
    snap: &Snapshot,
    perf: Option<&PerformanceSettings>,
    kind: DeviceKind,
) -> Vec<DeviceRow> {
    let (order, overrides) = view_parts(perf);
    let view = settings_view(perf, &order, &overrides);
    let mut rows = Vec::new();
    sidebar_order(&snap.devices, &snap.system.sensors, &view, &mut rows);
    retain_whole_disks(&mut rows, &snap.devices);
    visible_rows(&rows, &view)
        .filter(|row| row.kind == kind)
        .cloned()
        .collect()
}

type Overrides = HashMap<DeviceKey, tl_model::settings::DeviceVisibility>;

/// `device_order` and `device_overrides` as the sidebar reads them.
fn view_parts(perf: Option<&PerformanceSettings>) -> (Vec<DeviceKey>, Overrides) {
    let k = |s: &str| DeviceKey(Arc::from(s));
    match perf {
        Some(p) => (
            p.device_order.iter().map(|s| k(s)).collect(),
            p.device_overrides.iter().map(|(s, v)| (k(s), *v)).collect(),
        ),
        None => (Vec::new(), HashMap::new()),
    }
}

/// The sidebar's view of `perf`; every type shows before settings load.
fn settings_view<'a>(
    perf: Option<&PerformanceSettings>,
    order: &'a [DeviceKey],
    overrides: &'a Overrides,
) -> PerformanceSettingsView<'a> {
    let on = |f: fn(&PerformanceSettings) -> bool| perf.is_none_or(f);
    PerformanceSettingsView {
        order,
        overrides,
        show_disks: on(|p| p.show_disks),
        show_gpus: on(|p| p.show_gpus),
        show_fans: on(|p| p.show_fans),
        show_disconnected_fans: on(|p| p.show_disconnected_fans),
        show_batteries: on(|p| p.show_batteries),
        show_network_wired: on(|p| p.show_network_wired),
        show_network_wireless: on(|p| p.show_network_wireless),
        show_network_vpn: on(|p| p.show_network_vpn),
        show_network_virtual: on(|p| p.show_network_virtual),
        show_network_other: on(|p| p.show_network_other),
    }
}

#[cfg(test)]
mod tests {
    use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, NetKind};
    use tl_model::ids::DeviceIdx;
    use tl_model::layout::{GroupItems, GroupOptions};
    use tl_model::layout_boxes::{
        DEVICE_CPU, DEVICE_DISKS, DEVICE_NETS, METRIC_DISK_IO, METRIC_LOAD, METRIC_NET_TRAFFIC,
    };
    use tl_model::time::BootId;

    use super::*;

    fn entry(idx: u16, identity: DeviceIdentity, present: bool) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("x"),
            present,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: None,
        }
    }

    fn disk(idx: u16, name: &str) -> DeviceEntry {
        entry(
            idx,
            DeviceIdentity::Disk {
                major: 8,
                minor: u32::from(idx),
                name: Arc::from(name),
                wwn_or_serial: None,
            },
            true,
        )
    }

    fn net(idx: u16, name: &str, kind: NetKind) -> DeviceEntry {
        let mut e = entry(
            idx,
            DeviceIdentity::Net {
                netns_ino: 0,
                ifindex: u32::from(idx),
                name: Arc::from(name),
            },
            true,
        );
        e.net_kind = Some(kind);
        e
    }

    fn snap(entries: Vec<DeviceEntry>) -> Snapshot {
        let mut s = Snapshot::empty(BootId(0), 0);
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(entries),
        });
        s
    }

    fn group(device: &str, members: GroupKind, metric: &str) -> GroupSelector {
        GroupSelector {
            device: DeviceSelector::Key(Arc::from(device)),
            members,
            metric: MetricKey::new(metric),
            options: GroupOptions {
                limit: None,
                items: GroupItems::Rows,
                arrange: None,
            },
        }
    }

    fn keys(m: &Members) -> Vec<String> {
        m.list()
            .iter()
            .map(|s| match &s.device {
                DeviceSelector::Key(k) => format!("{k}/{}", s.metric.as_str()),
                DeviceSelector::Slot(n) => format!("slot{n}/{}", s.metric.as_str()),
            })
            .collect()
    }

    #[test]
    fn cores_by_logical_id_present_only() {
        let s = snap(vec![
            entry(0, DeviceIdentity::CpuPackage, true),
            entry(3, DeviceIdentity::CpuLogical { id: 2 }, true),
            entry(1, DeviceIdentity::CpuLogical { id: 0 }, true),
            entry(2, DeviceIdentity::CpuLogical { id: 1 }, false),
        ]);
        let g = group(DEVICE_CPU, GroupKind::Cores, METRIC_LOAD);
        assert_eq!(
            keys(&members(&g, &s, None, None, false)),
            ["cpu:0/load", "cpu:2/load"]
        );
    }

    #[test]
    fn disks_are_visible_whole_disks_in_sidebar_order() {
        let mut part = disk(5, "sda1");
        part.parent = Some(DeviceIdx(3));
        let s = snap(vec![
            entry(1, DeviceIdentity::Aggregate(AggregateKind::Disks), true),
            disk(3, "sda"),
            disk(4, "nvme0n1"),
            part,
        ]);
        let g = group(DEVICE_DISKS, GroupKind::Disks, METRIC_DISK_IO);
        assert_eq!(
            keys(&members(&g, &s, None, None, false)),
            ["disk:sda/io", "disk:nvme0n1/io"]
        );
        let mut p = PerformanceSettings::default();
        p.device_order = vec!["disk:nvme0n1".into()];
        assert_eq!(
            keys(&members(&g, &s, Some(&p), None, false)),
            ["disk:nvme0n1/io", "disk:sda/io"]
        );
        p.show_disks = false;
        assert_eq!(
            members(&g, &s, Some(&p), None, false),
            Members::List(vec![])
        );
    }

    #[test]
    fn adapters_leave_out_loopback_and_hidden_classes() {
        let s = snap(vec![
            net(2, "lo", NetKind::Virtual),
            net(3, "eth0", NetKind::Wired),
            net(4, "wg0", NetKind::Vpn),
        ]);
        let g = group(DEVICE_NETS, GroupKind::Adapters, METRIC_NET_TRAFFIC);
        let mut p = PerformanceSettings::default();
        p.show_network_vpn = true;
        p.show_network_virtual = true;
        assert_eq!(
            keys(&members(&g, &s, Some(&p), None, false)),
            ["net:eth0/traffic", "net:wg0/traffic"]
        );
        p.show_network_vpn = false;
        assert_eq!(
            keys(&members(&g, &s, Some(&p), None, false)),
            ["net:eth0/traffic"]
        );
    }

    #[test]
    fn engines_of_a_gone_gpu_and_unknown_groups() {
        let s = snap(vec![]);
        let g = group("gpu:GPU-x", GroupKind::Engines, METRIC_ENGINE_ANY);
        assert_eq!(members(&g, &s, None, None, false), Members::Gone);
        let mut slot = g.clone();
        slot.device = DeviceSelector::Slot(0);
        assert_eq!(members(&slot, &s, None, None, true), Members::Gone);
        let other = group(DEVICE_CPU, GroupKind::Other(Arc::from("fans")), "x");
        assert_eq!(members(&other, &s, None, None, false), Members::Unknown);
    }
}
