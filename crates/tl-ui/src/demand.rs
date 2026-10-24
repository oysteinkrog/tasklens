//! `DemandAggregator`: merges what every visible surface needs into one `Demand` and sends it to
//! the engine at most every 100 ms, only when it changed (ADR-005, ADR-008). While the hub
//! reports the main window hidden, parts from the main window are ignored (ADR-008 "Hidden
//! detection", ADR-010).
//!
//! Graph targets (PERF-085, PROC-093): process, name and app minigraph cards keep sampling while
//! they are off screen and while the window is hidden. Their surface sends them with
//! [`DemandAggregator::set_graph_targets`] as a separate part that is never left out. Only its
//! `graph_targets` count, so every bool still follows visibility.
//!
//! Save power on battery (wlf.32): while the system runs on battery and
//! `motion.save_power_on_battery` is on, the merge leaves `cpu_fast` off, so CPU updates drop
//! from 10 Hz to 1 Hz, and [`PowerSaving`] tells the charts to stop sliding. Both are applied
//! only when the battery state or the setting changes.
//!
//! Device history (ADR-017): `device_history` follows `history.device_metrics`, visible or not.
//! `device_history_range` comes from the first visible part that sets it.

use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AnyWindowHandle, App, AsyncApp, Context, Entity, EntityId, Global, WeakEntity};
use rustc_hash::{FxHashMap, FxHashSet};
use tl_model::client::EngineClient;
use tl_model::demand::{
    AFFINITY_TARGETS_MAX, CpuDemand, DETAIL_KEYS_MAX, DETAILS_MAX, Demand, GRAPH_TARGETS_MAX,
    UNIT_DETAILS_MAX,
};
use tl_model::ids::{ColumnId, GraphTarget};

use crate::hub::{ComponentMask, HiddenChanged, SnapshotChanged, SnapshotHub};
use crate::settings_global::TlSettings;

/// The shortest time between two `set_demand` calls.
pub const DEMAND_COALESCE: Duration = Duration::from_millis(100);

/// The one entity per app that owns the demand sent to the engine.
pub struct DemandAggregator {
    client: Arc<dyn EngineClient>,
    /// One contribution per surface. Window `None` means the surface is not in a window (tray).
    parts: FxHashMap<EntityId, (Option<AnyWindowHandle>, Demand)>,
    /// Always-on graph targets, one list per surface. Kept while no part is visible.
    graph_targets: FxHashMap<EntityId, Arc<[(GraphTarget, ColumnId)]>>,
    main_window: Option<AnyWindowHandle>,
    last_sent: Option<Demand>,
    flush_scheduled: bool,
    /// Mirrors `SnapshotHub::is_hidden` through `HiddenChanged`.
    hidden: bool,
    /// On battery with `motion.save_power_on_battery` on; mirrored in [`PowerSaving`].
    power_saving: bool,
    /// `history.device_metrics`, sent as `Demand.device_history`.
    device_metrics: bool,
}

/// Save power on battery is in effect: charts do not slide (wlf.32). Set by the
/// [`DemandAggregator`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerSaving(pub bool);

impl Global for PowerSaving {}

/// Whether save power on battery is in effect. False before the aggregator sets it.
pub fn power_saving(cx: &App) -> bool {
    cx.try_global::<PowerSaving>().is_some_and(|p| p.0)
}

/// `history.device_metrics`. It counts as on (its default) before settings load.
fn device_metrics_now(cx: &App) -> bool {
    !cx.has_global::<TlSettings>() || TlSettings::settings(cx).history.device_metrics
}

/// On battery with the setting on. The setting counts as on (its default) before settings load.
fn power_saving_now(hub: &Entity<SnapshotHub>, cx: &App) -> bool {
    let on_battery = hub.read(cx).current().device_details.on_battery == Some(true);
    let setting =
        !cx.has_global::<TlSettings>() || TlSettings::settings(cx).motion.save_power_on_battery;
    on_battery && setting
}

impl DemandAggregator {
    /// Subscribes to the hub's `HiddenChanged`, and to the battery state and the settings for
    /// save power on battery.
    pub fn new(
        client: Arc<dyn EngineClient>,
        hub: &Entity<SnapshotHub>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(hub, |this, _, ev: &HiddenChanged, cx| {
            if this.hidden != ev.hidden {
                this.hidden = ev.hidden;
                this.schedule_flush(cx);
            }
        })
        .detach();
        cx.subscribe(hub, |this, hub, ev: &SnapshotChanged, cx| {
            if ev.mask.contains(ComponentMask::DEVICE_DETAILS) {
                this.sync_power_saving(&hub, cx);
            }
        })
        .detach();
        let settings_hub = hub.clone();
        cx.observe_global::<TlSettings>(move |this, cx| {
            this.sync_power_saving(&settings_hub, cx);
            let device_metrics = device_metrics_now(cx);
            if device_metrics != this.device_metrics {
                this.device_metrics = device_metrics;
                this.schedule_flush(cx);
            }
        })
        .detach();
        let power_saving = power_saving_now(hub, cx);
        cx.set_global(PowerSaving(power_saving));
        Self {
            client,
            parts: FxHashMap::default(),
            graph_targets: FxHashMap::default(),
            main_window: None,
            last_sent: None,
            flush_scheduled: false,
            hidden: hub.read(cx).is_hidden(),
            power_saving,
            device_metrics: device_metrics_now(cx),
        }
    }

    /// Recomputes save power on battery; on a change, updates [`PowerSaving`], refreshes the
    /// windows so the charts read it, and schedules a flush.
    fn sync_power_saving(&mut self, hub: &Entity<SnapshotHub>, cx: &mut Context<Self>) {
        let on = power_saving_now(hub, cx);
        if on == self.power_saving {
            return;
        }
        self.power_saving = on;
        cx.set_global(PowerSaving(on));
        cx.refresh_windows();
        self.schedule_flush(cx);
    }

    /// Save power on battery is in effect.
    pub fn power_saving(&self) -> bool {
        self.power_saving
    }

    /// Replaces this surface's part and schedules a flush.
    pub fn set(
        &mut self,
        source: EntityId,
        window: Option<AnyWindowHandle>,
        part: Demand,
        cx: &mut Context<Self>,
    ) {
        self.parts.insert(source, (window, part));
        self.schedule_flush(cx);
    }

    /// Replaces this surface's always-on graph targets and schedules a flush. An empty list
    /// drops them. [`Self::remove`] leaves them in place.
    pub fn set_graph_targets(
        &mut self,
        source: EntityId,
        targets: Arc<[(GraphTarget, ColumnId)]>,
        cx: &mut Context<Self>,
    ) {
        let changed = if targets.is_empty() {
            self.graph_targets.remove(&source).is_some()
        } else {
            self.graph_targets.insert(source, Arc::clone(&targets)) != Some(targets)
        };
        if changed {
            self.schedule_flush(cx);
        }
    }

    /// Called once by `MainWindow::new`.
    pub fn set_main_window(&mut self, window: AnyWindowHandle) {
        self.main_window = Some(window);
    }

    /// The main window, for parts of main-window pages that have no window handle of their own.
    pub fn main_window(&self) -> Option<AnyWindowHandle> {
        self.main_window
    }

    /// Drops this surface's part (on surface drop or hide) and schedules a flush.
    pub fn remove(&mut self, source: EntityId, cx: &mut Context<Self>) {
        if self.parts.remove(&source).is_some() {
            self.schedule_flush(cx);
        }
    }

    /// The merge of every part that counts now. While hidden, parts from the main window are
    /// left out. The always-on graph targets follow the parts' own targets, capped at
    /// `GRAPH_TARGETS_MAX`, visible or not.
    pub fn merged(&self) -> Demand {
        let mut parts: Vec<(EntityId, &Demand)> = self
            .parts
            .iter()
            .filter(|(_, (window, _))| {
                !(self.hidden && window.is_some() && *window == self.main_window)
            })
            .map(|(id, (_, part))| (*id, part))
            .collect();
        // Map order is arbitrary. Sorting keeps list order stable, so an unchanged set of
        // parts gives an equal merge and nothing is resent.
        parts.sort_unstable_by_key(|(id, _)| *id);
        let mut merged = merge(parts.into_iter().map(|(_, part)| part));
        if !self.graph_targets.is_empty() {
            let mut always: Vec<_> = self.graph_targets.iter().collect();
            always.sort_unstable_by_key(|(id, _)| **id);
            merged.graph_targets = union(
                std::iter::once(&merged.graph_targets).chain(always.into_iter().map(|(_, t)| t)),
                GRAPH_TARGETS_MAX,
            );
        }
        if self.power_saving {
            merged.cpu_fast = false;
        }
        merged.device_history = self.device_metrics;
        merged
    }

    fn schedule_flush(&mut self, cx: &mut Context<Self>) {
        if self.flush_scheduled {
            return;
        }
        self.flush_scheduled = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(DEMAND_COALESCE).await;
            let _ = this.update(cx, |this, _| this.flush());
        })
        .detach();
    }

    fn flush(&mut self) {
        self.flush_scheduled = false;
        let merged = self.merged();
        if Some(&merged) != self.last_sent.as_ref() {
            self.client.set_demand(merged.clone());
            self.last_sent = Some(merged);
        }
    }
}

/// The app's aggregator, set once by `TasklensUi::init`.
pub struct GlobalDemand(pub Entity<DemandAggregator>);

impl Global for GlobalDemand {}

/// Field-wise merge: bools OR, `cpu` maximum, column sets union, lists deduplicated in
/// first-seen order and capped (ADR-008). With no visible part every periodic source is off and
/// only `recording` is kept.
fn merge<'a>(parts: impl Iterator<Item = &'a Demand> + Clone) -> Demand {
    let mut m = Demand::default();
    for p in parts.clone() {
        m.any_surface_visible |= p.any_surface_visible || p.tray.icons_shown;
        if cpu_rank(p.cpu) > cpu_rank(m.cpu) {
            m.cpu = p.cpu;
        }
        m.cpu_fast |= p.cpu_fast;
        m.memory |= p.memory;
        m.disks |= p.disks;
        m.nets |= p.nets;
        m.gpus |= p.gpus;
        m.sensors |= p.sensors;
        m.process_table |= p.process_table;
        m.process_top |= p.process_top;
        for (word, part) in m.process_columns.0.iter_mut().zip(p.process_columns.0) {
            *word |= part;
        }
        m.gpu_process |= p.gpu_process;
        m.connections |= p.connections;
        m.services |= p.services;
        m.startup |= p.startup;
        m.inventory |= p.inventory;
        m.service_cgroup_stats |= p.service_cgroup_stats;
        m.connections_all_netns |= p.connections_all_netns;
        m.tray.icons_shown |= p.tray.icons_shown;
        m.tray.cpu |= p.tray.cpu;
        m.tray.memory |= p.tray.memory;
        m.tray.disks |= p.tray.disks;
        m.tray.nets |= p.tray.nets;
        m.tray.gpus |= p.tray.gpus;
        m.recording |= p.recording;
        if m.device_history_range.is_none() {
            m.device_history_range = p.device_history_range;
        }
    }
    if !m.any_surface_visible {
        return Demand {
            recording: m.recording,
            ..Demand::default()
        };
    }
    m.detail_keys = union(parts.clone().map(|p| &p.detail_keys), DETAIL_KEYS_MAX);
    m.details = union(parts.clone().map(|p| &p.details), DETAILS_MAX);
    m.affinity_targets = union(
        parts.clone().map(|p| &p.affinity_targets),
        AFFINITY_TARGETS_MAX,
    );
    m.unit_details = union(parts.clone().map(|p| &p.unit_details), UNIT_DETAILS_MAX);
    m.remote_host_cookies = union(parts.clone().map(|p| &p.remote_host_cookies), usize::MAX);
    m.graph_targets = union(parts.map(|p| &p.graph_targets), GRAPH_TARGETS_MAX);
    m
}

/// `Off < Total < PerCore`.
fn cpu_rank(cpu: CpuDemand) -> u8 {
    match cpu {
        CpuDemand::Off => 0,
        CpuDemand::Total => 1,
        CpuDemand::PerCore => 2,
    }
}

/// Unique items of `lists` in first-seen order, at most `cap`.
fn union<'a, T: Clone + Eq + Hash + 'a>(
    lists: impl Iterator<Item = &'a Arc<[T]>>,
    cap: usize,
) -> Arc<[T]> {
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    for item in lists.flat_map(|l| l.iter()) {
        if out.len() == cap {
            break;
        }
        if seen.insert(item) {
            out.push(item.clone());
        }
    }
    out.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::demand::{
        ColumnSet, DetailDemand, DeviceHistoryRequest, DeviceRange, TrayDemand,
    };
    use tl_model::details::{DetailTab, DetailTarget};
    use tl_model::ids::{ColumnId, GraphTarget, ProcessKey};
    use tl_model::table::{Manager, UnitRef};
    use tl_model::time::BootId;

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 1000 + pid as u64,
            boot_id: BootId(42),
            pid,
        }
    }

    fn visible() -> Demand {
        Demand {
            any_surface_visible: true,
            ..Demand::default()
        }
    }

    fn cols(list: &[ColumnId]) -> ColumnSet {
        let mut set = ColumnSet::EMPTY;
        for c in list {
            set.insert(*c);
        }
        set
    }

    #[test]
    fn merge_rules() {
        let a = Demand {
            memory: true,
            cpu: CpuDemand::Total,
            process_table: true,
            process_columns: cols(&[ColumnId::Name, ColumnId::Cpu]),
            detail_keys: vec![key(1), key(2)].into(),
            tray: TrayDemand {
                cpu: true,
                ..TrayDemand::default()
            },
            ..visible()
        };
        let b = Demand {
            nets: true,
            cpu: CpuDemand::PerCore,
            process_columns: cols(&[ColumnId::Cpu, ColumnId::Container]),
            detail_keys: vec![key(2), key(3), key(2)].into(),
            recording: true,
            ..visible()
        };
        let c = Demand {
            cpu: CpuDemand::Off,
            ..visible()
        };
        let m = merge([&a, &b, &c].into_iter());
        assert!(m.any_surface_visible);
        assert!(m.memory && m.nets && m.process_table && m.recording && m.tray.cpu);
        assert!(!m.disks && !m.gpus && !m.tray.memory);
        assert_eq!(m.cpu, CpuDemand::PerCore);
        assert_eq!(
            m.process_columns,
            cols(&[ColumnId::Name, ColumnId::Cpu, ColumnId::Container])
        );
        assert_eq!(&m.detail_keys[..], &[key(1), key(2), key(3)]);

        // Off < Total < PerCore regardless of order.
        let total = Demand {
            cpu: CpuDemand::Total,
            ..visible()
        };
        assert_eq!(merge([&c, &total].into_iter()).cpu, CpuDemand::Total);
        assert_eq!(merge([&b, &total].into_iter()).cpu, CpuDemand::PerCore);

        // A shown tray icon counts as a visible surface.
        let tray = Demand {
            tray: TrayDemand {
                icons_shown: true,
                memory: true,
                ..TrayDemand::default()
            },
            ..Demand::default()
        };
        let m = merge([&tray].into_iter());
        assert!(m.any_surface_visible && m.tray.icons_shown && m.tray.memory);

        // No visible part: every periodic source off, recording kept.
        let hidden = Demand {
            any_surface_visible: false,
            ..b.clone()
        };
        assert_eq!(
            merge([&hidden].into_iter()),
            Demand {
                recording: true,
                ..Demand::default()
            }
        );
        assert_eq!(merge(std::iter::empty()), Demand::default());
    }

    #[test]
    fn device_history_range_first_visible_part() {
        let day = DeviceHistoryRequest {
            range: DeviceRange::H24,
            end_realtime_ns: None,
        };
        let week = DeviceHistoryRequest {
            range: DeviceRange::D7,
            end_realtime_ns: Some(5),
        };
        let a = visible();
        let b = Demand {
            device_history_range: Some(day),
            ..visible()
        };
        let c = Demand {
            device_history_range: Some(week),
            ..visible()
        };
        let m = merge([&a, &b, &c].into_iter());
        assert_eq!(m.device_history_range, Some(day));
        // A hidden Overview asks for no range.
        let hidden = Demand {
            any_surface_visible: false,
            ..b
        };
        assert_eq!(merge([&hidden].into_iter()).device_history_range, None);
    }

    #[test]
    fn caps_applied() {
        let details = |range: std::ops::Range<u32>| -> Arc<[DetailDemand]> {
            range
                .map(|i| DetailDemand {
                    target: DetailTarget::Process(key(i)),
                    tab: DetailTab::General,
                })
                .collect()
        };
        let units = |range: std::ops::Range<u32>| -> Arc<[UnitRef]> {
            range
                .map(|i| UnitRef {
                    manager: Manager::System,
                    name: format!("u{i}.service").into(),
                })
                .collect()
        };
        let a = Demand {
            detail_keys: (0..400).map(key).collect(),
            details: details(0..12),
            affinity_targets: (0..200).map(key).collect(),
            unit_details: units(0..8),
            remote_host_cookies: (0..3000).collect(),
            graph_targets: (0..12)
                .map(|i| (GraphTarget::Process(key(i)), ColumnId::Cpu))
                .collect(),
            ..visible()
        };
        // 100 of each list shared with `a` where the list is that long.
        let b = Demand {
            detail_keys: (300..700).map(key).collect(),
            details: details(6..18),
            affinity_targets: (100..300).map(key).collect(),
            unit_details: units(4..12),
            remote_host_cookies: (2000..5000).collect(),
            graph_targets: (6..18)
                .map(|i| (GraphTarget::Process(key(i)), ColumnId::Cpu))
                .collect(),
            ..visible()
        };
        let m = merge([&a, &b].into_iter());

        let expected: Vec<ProcessKey> = (0..512).map(key).collect();
        assert_eq!(&m.detail_keys[..], &expected[..]);
        assert_eq!(m.detail_keys.len(), DETAIL_KEYS_MAX);
        assert_eq!(&m.details[..], &details(0..16)[..]);
        assert_eq!(m.affinity_targets.len(), AFFINITY_TARGETS_MAX);
        assert_eq!(
            &m.affinity_targets[..],
            &(0..256).map(key).collect::<Vec<_>>()[..]
        );
        assert_eq!(&m.unit_details[..], &units(0..10)[..]);
        assert_eq!(m.graph_targets.len(), GRAPH_TARGETS_MAX);
        // Not capped: 5000 unique cookies.
        assert_eq!(
            &m.remote_host_cookies[..],
            &(0..5000).collect::<Vec<u64>>()[..]
        );
    }
}
