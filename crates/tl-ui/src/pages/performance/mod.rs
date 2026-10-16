//! Performance tab: device keys, sidebar order and visibility rules, and the `PerformancePage`
//! entity (docs/ui/performance.md UI-PRF-010, UI-PRF-012; PERF-002, PERF-004, PERF-005,
//! PERF-008).
//!
//! E25 device pages implement [`page::DevicePage`], add their kind to [`page::build_page`],
//! register their demand surface from `DevicePage::set_active`, and update every chart with
//! `ChartView::set_state` from `Snapshot.health`.

pub mod actions;
pub mod card;
pub mod cpu;
pub mod cpu_details;
pub mod cpu_grid;
pub mod details;
pub mod disk;
pub mod gpu;
pub mod memory;
pub mod memory_bar;
pub mod menu;
pub mod net;
pub mod overview;
pub mod page;
pub mod sidebar;
pub mod summary;

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use gpui_kit::component::{ResizableState, h_flex, h_resizable, resizable_panel};
use gpui_kit::private::schemars::JsonSchema;
use gpui_kit::{
    Animation, AnimationExt as _, AnyView, App, AppContext as _, AsyncApp, ClipboardItem, Context,
    Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, Point, Render, SharedString,
    Styled as _, Subscription, WeakEntity, Window, div, px,
};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::settings::DeviceVisibility;
use tl_model::system::{SensorKind, SensorReading};

use self::actions::{
    PERFORMANCE_CONTEXT, PerfCopyDeviceInfo, PerfCopyValue, PerfJumpToType, PerfSelectDevice,
    PerfSetGraphRange, PerfToggleSidebar, PerfToggleSummaryMode,
};
use self::details::ClickedValue;
use self::menu::{PageMenuCtx, VIEW_KINDS, page_menu};
use self::page::DevicePage;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::motion::{M_BASE, M_FAST, ease_out_cubic};
use crate::components::toast::{FIXED_KEY_BASE, ToastKey, ToastKind, ToastQueue};
use crate::demand::DemandAggregator;
use crate::hub::SnapshotHub;
use crate::i18n::tr;
use crate::settings_global::{TlSettings, animations_on};
use crate::theme::metrics;
use crate::util::clipboard_write;
use crate::window::main::{ShellPage, WINDOW_TITLE};
use crate::window::status::StatusChanged;

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
    pub const OVERVIEW: &'static str = overview::OVERVIEW_KEY;
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

/// The range of the sidebar width a drag or the state file can give, × ui_scale (UI-PRF-010).
const NAV_MIN_W: u16 = 180;
const NAV_MAX_W: u16 = 400;

/// Debug selectors of the two split panels' contents.
pub const SIDEBAR_PANEL_ID: &str = "perf-sidebar-panel";
pub const PAGE_PANEL_ID: &str = "perf-page-panel";

/// The sidebar hide or show animation in progress (`PerfToggleSidebar`).
#[derive(Clone, Copy)]
struct SidebarAnim {
    /// True while the sidebar opens, false while it closes.
    showing: bool,
    /// Bumped on each toggle, so a new toggle restarts the animation.
    generation: u32,
}

/// The Performance tab entity. Later E24 beads add the summary field (R-262).
pub struct PerformancePage {
    /// Rendered cached `size_full` in the split's first panel.
    pub(crate) sidebar: Entity<sidebar::DeviceSidebar>,
    /// The sidebar's focus handle: the tab's `primary_focus` (UI-SHL-121).
    sidebar_focus: FocusHandle,
    /// Device pages, created on first visit and kept (ADR-009).
    pub(crate) pages: FxHashMap<DeviceKey, AnyView>,
    pub(crate) selected: Option<DeviceKey>,
    /// The selected page's `focus_order`, read once when it is selected.
    page_focus: SmallVec<[FocusHandle; 4]>,
    /// Graph summary mode (E24-015), `Some` while it is on.
    pub(crate) summary: Option<Entity<summary::SummaryView>>,
    summary_sub: Option<Subscription>,
    /// The sidebar and page fade out before the summary opens; the generation of that fade.
    leaving: Option<u32>,
    leave_generation: u32,
    /// Not persisted (R-090).
    pub(crate) sidebar_visible: bool,
    anim: Option<SidebarAnim>,
    anim_generation: u32,
    /// `state.performance.nav_width`, clamped to 180..=400.
    nav_width: u16,
    /// The split's panel sizes. Made new each time the sidebar comes back, so a drag on the
    /// handle while the sidebar was hidden leaves nothing behind, and each time the split's
    /// width changes: the kit scales every panel with the container, and the sidebar keeps
    /// `nav_width` instead (UI-PRF-010).
    split: Entity<ResizableState>,
    /// The split width `split` was measured at; zero for a new state.
    split_width: Pixels,
    /// The tab is the active tab (`set_active`).
    active: bool,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    /// Key context "Performance".
    pub(crate) focus: FocusHandle,
    /// The Details value the open page menu was opened on, for "Copy value".
    clicked_value: Option<SharedString>,
    _subs: SmallVec<[Subscription; 1]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    /// The last title this page set; the test platform window does not report its title.
    #[cfg(any(test, feature = "test-hooks"))]
    pub window_title: String,
}

impl PerformancePage {
    /// Builds the sidebar and the page of the device the sidebar selected first.
    pub fn new(
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let sidebar = cx.new(|cx| sidebar::DeviceSidebar::new(hub.clone(), demand.clone(), cx));
        let sidebar_focus = sidebar.read(cx).focus.clone();
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&sidebar, Self::on_sidebar));
        let nav_width = TlSettings::state(cx).performance.nav_width;
        let first = sidebar.read(cx).selected().cloned();
        let mut this = Self {
            sidebar,
            sidebar_focus,
            pages: FxHashMap::default(),
            selected: None,
            page_focus: SmallVec::new(),
            sidebar_visible: true,
            anim: None,
            anim_generation: 0,
            nav_width: nav_width.clamp(NAV_MIN_W, NAV_MAX_W),
            split: cx.new(|_| ResizableState::default()),
            split_width: Pixels::ZERO,
            active: true,
            hub,
            demand,
            focus: cx.focus_handle(),
            clicked_value: None,
            summary: None,
            summary_sub: None,
            leaving: None,
            leave_generation: 0,
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            window_title: String::new(),
        };
        if let Some(key) = first {
            this.show(key, cx);
        }
        this
    }

    /// Tab activation: the sidebar and the shown page withdraw their demand parts while another
    /// tab is shown and send them again when this tab comes back.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if !active && self.leaving.take().is_some() {
            cx.notify();
        }
        self.sidebar.update(cx, |s, cx| s.set_active(active, cx));
        let page_on = self.page_on();
        if let Some(view) = self.selected_view() {
            update_page(&view, cx, |p, cx| p.set_active(page_on, cx));
        }
    }

    /// The shown device page sends demand: the tab is active and summary mode is off.
    fn page_on(&self) -> bool {
        self.active && self.summary.is_none()
    }

    /// The summary mode view while it is on. `MainWindow` renders it in place of the tab strip
    /// and the page (UI-SHL-001).
    pub fn summary_view(&self) -> Option<AnyView> {
        self.summary.clone().map(AnyView::from)
    }

    pub fn summary(&self) -> Option<&Entity<summary::SummaryView>> {
        self.summary.as_ref()
    }

    /// `PerfToggleSummaryMode` (Ctrl+M) on the tab.
    fn on_toggle_summary(
        &mut self,
        _: &PerfToggleSummaryMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.summary.is_some() {
            self.exit_summary(None, window, cx);
        } else if self.leaving.take().is_some() {
            // Ctrl+M again during the fade: stay on the page.
            cx.notify();
        } else if animations_on(cx) {
            self.leave_generation = self.leave_generation.wrapping_add(1);
            let generation = self.leave_generation;
            self.leaving = Some(generation);
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(M_FAST).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.leaving == Some(generation) {
                        this.leaving = None;
                        this.enter_summary(window, cx);
                    }
                });
            })
            .detach();
            cx.notify();
        } else {
            self.enter_summary(window, cx);
        }
    }

    /// The sidebar and page are fading out before summary mode opens.
    pub fn summary_pending(&self) -> bool {
        self.leaving.is_some()
    }

    /// Opens summary mode: one tile per visible card. The shown page withdraws its demand; the
    /// sidebar keeps its part and the summary sends its own.
    fn enter_summary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let size = window.viewport_size();
        let (sidebar, demand) = (self.sidebar.clone(), self.demand.clone());
        let view = cx.new(|cx| summary::SummaryView::new(&sidebar, demand, size, cx));
        self.summary_sub = Some(cx.subscribe_in(
            &view,
            window,
            |this, _, ev: &summary::SummaryEvent, window, cx| match ev {
                summary::SummaryEvent::Exit { select } => {
                    this.exit_summary(select.clone(), window, cx);
                }
            },
        ));
        self.summary = Some(view.clone());
        if let Some(page) = self.selected_view() {
            update_page(&page, cx, |p, cx| p.set_active(false, cx));
        }
        let suffix = tr(cx, "perf-summary-title-suffix");
        let title = format!("{WINDOW_TITLE}{suffix}");
        window.set_window_title(&title);
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.window_title = title;
        }
        let focus = view.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        cx.emit(SummaryToggled);
        cx.notify();
    }

    /// Closes summary mode, then selects `select` when set (a tile click).
    fn exit_summary(
        &mut self,
        select: Option<DeviceKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.summary.take().is_none() {
            return;
        }
        self.summary_sub = None;
        let page_on = self.page_on();
        if let Some(page) = self.selected_view() {
            update_page(&page, cx, |p, cx| p.set_active(page_on, cx));
        }
        window.set_window_title(WINDOW_TITLE);
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.window_title = WINDOW_TITLE.into();
        }
        self.sidebar_focus.focus(window, cx);
        if let Some(key) = select {
            self.sidebar.update(cx, |s, cx| s.select(key, cx));
        }
        cx.emit(SummaryToggled);
        cx.notify();
    }

    /// UI-SHL-121 initial focus: the sidebar list.
    pub fn primary_focus(&self) -> FocusHandle {
        self.sidebar_focus.clone()
    }

    /// The sidebar, then the shown page's header toggles and block controls (UI-SHL-121).
    pub fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        let mut order = SmallVec::new();
        order.push(self.sidebar_focus.clone());
        order.extend(self.page_focus.iter().cloned());
        order
    }

    pub fn sidebar(&self) -> &Entity<sidebar::DeviceSidebar> {
        &self.sidebar
    }

    pub fn selected(&self) -> Option<&DeviceKey> {
        self.selected.as_ref()
    }

    /// The page view of `key`, if it was built.
    pub fn page(&self, key: &DeviceKey) -> Option<&AnyView> {
        self.pages.get(key)
    }

    /// The number of pages built so far.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    /// The sidebar width in logical pixels before ui_scale.
    pub fn nav_width(&self) -> u16 {
        self.nav_width
    }

    fn selected_view(&self) -> Option<AnyView> {
        self.selected
            .as_ref()
            .and_then(|k| self.pages.get(k))
            .cloned()
    }

    fn on_sidebar(
        &mut self,
        _: Entity<sidebar::DeviceSidebar>,
        ev: &sidebar::SidebarEvent,
        cx: &mut Context<Self>,
    ) {
        match ev {
            sidebar::SidebarEvent::Selected(key) => {
                if self.show(key.clone(), cx) {
                    cx.notify();
                }
            }
            // A device that went away or came back with a new generation gets a new page on
            // its next visit. Dropping the view drops its demand part.
            sidebar::SidebarEvent::Removed(key) => {
                if self.pages.remove(key).is_some() && self.selected.as_ref() == Some(key) {
                    self.selected = None;
                    self.page_focus.clear();
                    cx.notify();
                }
            }
        }
    }

    /// Shows the page of `key`: builds it on the first visit, deactivates the old page and
    /// activates the new one. False when nothing changed.
    fn show(&mut self, key: DeviceKey, cx: &mut App) -> bool {
        if self.selected.as_ref() == Some(&key) {
            return false;
        }
        let view = match self.pages.get(&key) {
            Some(view) => view.clone(),
            None => {
                let Some(row) = self
                    .sidebar
                    .read(cx)
                    .rows()
                    .iter()
                    .find(|r| r.key == key)
                    .cloned()
                else {
                    return false;
                };
                // The Overview page reads the sidebar's cards, so it is built here.
                let view = if row.kind == DeviceKind::Overview {
                    let (hub, demand) = (self.hub.clone(), self.demand.clone());
                    let sidebar = &self.sidebar;
                    cx.new(|cx| overview::OverviewPage::new(key.clone(), hub, demand, sidebar, cx))
                        .into()
                } else {
                    page::build_page(&key, &row, &self.hub, &self.demand, cx)
                };
                self.pages.insert(key.clone(), view.clone());
                view
            }
        };
        if let Some(old) = self.selected_view() {
            update_page(&old, cx, |p, cx| p.set_active(false, cx));
        }
        if self.page_on() {
            update_page(&view, cx, |p, cx| p.set_active(true, cx));
        }
        self.page_focus = page_focus_order(&view, cx);
        self.selected = Some(key);
        true
    }

    /// `PerfToggleSidebar` (F9, Ctrl+T): hides or shows the sidebar. The width animates over
    /// `m_base` unless animations are off. Not persisted (R-090).
    fn on_toggle_sidebar(
        &mut self,
        _: &PerfToggleSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_visible = !self.sidebar_visible;
        // Focus leaves the list before it goes, so F9 still reaches this page, and comes back
        // to it with the list.
        if !self.sidebar_visible && self.sidebar_focus.contains_focused(window, cx) {
            self.focus.focus(window, cx);
        } else if self.sidebar_visible && self.focus.is_focused(window) {
            self.sidebar_focus.focus(window, cx);
        }
        self.anim_generation = self.anim_generation.wrapping_add(1);
        if !animations_on(cx) {
            self.finish_toggle(cx);
            return;
        }
        let generation = self.anim_generation;
        self.anim = Some(SidebarAnim {
            showing: self.sidebar_visible,
            generation,
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(M_BASE).await;
            let _ = this.update(cx, |this, cx| {
                if this.anim_generation == generation {
                    this.finish_toggle(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// The end state of a toggle: the split panel shows or hides at once.
    fn finish_toggle(&mut self, cx: &mut Context<Self>) {
        self.anim = None;
        if self.sidebar_visible {
            self.new_split(cx);
        }
        cx.notify();
    }

    fn new_split(&mut self, cx: &mut App) {
        self.split = cx.new(|_| ResizableState::default());
        self.split_width = Pixels::ZERO;
    }

    /// `PerfSetGraphRange`: the sidebar, the pages and the summary tiles follow the setting.
    fn on_set_graph_range(
        &mut self,
        a: &PerfSetGraphRange,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        set_graph_range(a.secs, cx);
    }

    /// The page menu of the shown page at `at` (UI-PRF-015). Its items run on the page.
    fn open_page_menu(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.selected_view() else {
            return;
        };
        let focus = page_focus(&view, cx).unwrap_or_else(|| self.focus.clone());
        let summary_on = self.summary.is_some();
        let sidebar = self.sidebar.clone();
        // Taken, so a later menu opened another way does not see an old value.
        self.clicked_value = cx
            .has_global::<ClickedValue>()
            .then(|| cx.remove_global::<ClickedValue>().0)
            .flatten();
        let clicked_value = self.clicked_value.is_some();
        let menu = TlMenu::build(window, cx, focus, move |menu, window, cx| {
            let types = view_types(sidebar.read(cx));
            let ctx = PageMenuCtx {
                view_types: Some(&types),
                clicked_value,
                ..page_menu_ctx(summary_on, cx)
            };
            let mut slot = Some(menu);
            update_page(&view, cx, |p, cx| {
                let menu = slot.take().expect("one page");
                slot = Some(page_menu(menu, p, &ctx, window, cx));
            });
            slot.expect("the menu")
        });
        open_context_menu_at(at, window, cx, menu);
    }

    /// `PerfCopyDeviceInfo` (Ctrl+C, menu "Copy"): copies the shown page as text and pushes the
    /// "Copied metrics to clipboard" toast (PERF-011, UI-PRF-022). A selected Details value is
    /// copied by the grid before the action gets here.
    fn on_copy_device_info(
        &mut self,
        _: &PerfCopyDeviceInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.selected_view() else {
            return;
        };
        let mut text = String::new();
        match view.clone().downcast::<overview::OverviewPage>() {
            Ok(page) => page.read(cx).copy_all(&mut text, cx),
            Err(view) => update_page(&view, cx, |p, _| p.copy_text(&mut text)),
        }
        if text.is_empty() {
            return;
        }
        let _ = clipboard_write(cx, text);
        let toast = tr(cx, "perf-toast-copied");
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| {
                q.push(ToastKind::Info, toast, Some(PERF_COPY_TOAST), None, cx)
            });
        }
    }

    /// `PerfCopyValue` ("Copy value" in the page menu): copies the value the menu was opened on,
    /// with no toast (UI-PRF-020).
    fn on_copy_value(&mut self, _: &PerfCopyValue, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = &self.clicked_value {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        }
    }

    /// `PerfJumpToType`: selects the first visible card of `kind`.
    fn on_jump_to_type(&mut self, a: &PerfJumpToType, _: &mut Window, cx: &mut Context<Self>) {
        let sidebar = self.sidebar.read(cx);
        let key = sidebar
            .visible_keys()
            .find(|k| sidebar.card(k).is_some_and(|c| c.row.kind == a.kind))
            .cloned();
        if let Some(key) = key {
            self.sidebar.update(cx, |s, cx| s.select(key, cx));
        }
    }

    /// `PerfSelectDevice` from outside the sidebar, for example a menu.
    fn on_select_device(&mut self, a: &PerfSelectDevice, _: &mut Window, cx: &mut Context<Self>) {
        let key = a.key.clone();
        self.sidebar.update(cx, |s, cx| s.select(key, cx));
    }

    /// The split handle was let go: saves the sidebar width (ADR-011; tl-app debounces the
    /// write by 500 ms).
    fn on_split_resized(&mut self, width: Pixels, cx: &mut Context<Self>) {
        if !self.sidebar_visible || self.anim.is_some() {
            return;
        }
        let s = metrics(cx).ui_scale;
        let w = (f32::from(width) / s)
            .round()
            .clamp(f32::from(NAV_MIN_W), f32::from(NAV_MAX_W));
        // In 180..=400, so the cast is exact.
        let w = w as u16;
        if w == self.nav_width {
            return;
        }
        self.nav_width = w;
        TlSettings::update(cx, |d| d.state.performance.nav_width = w);
    }
}

/// Runs `f` on the device page behind `view`. Each page type `page::build_page` creates has a
/// line here (chain edit with its `build_page` arm, E25-010).
/// Writes `performance.graph_range_s` (PERF-016). 300 and 600 read the 1 Hz tier (E19-013).
pub fn set_graph_range(secs: u16, cx: &mut App) {
    if !matches!(secs, 30 | 60 | 300 | 600) {
        return;
    }
    TlSettings::update(cx, |d| d.settings.performance.graph_range_s = secs);
}

/// The page menu's state from the settings.
pub fn page_menu_ctx(summary_on: bool, cx: &App) -> PageMenuCtx<'static> {
    PageMenuCtx {
        range_s: TlSettings::settings(cx).performance.graph_range_s,
        summary_on,
        ranges_long_enabled: true,
        view_types: None,
        clicked_value: false,
        tray_available: tray_available(cx),
    }
}

/// Whether the tray can take a graph: `None` leaves the item out, `Some(false)` disables it
/// (PERF-088).
pub fn tray_available(cx: &App) -> Option<bool> {
    // TODO(E31-A2): Some(cx.try_global::<TrayStatus>().is_some_and(|s| s.available && s.running))
    let _ = cx;
    Some(false)
}

/// For each [`VIEW_KINDS`] kind, whether the sidebar shows a card of it.
fn view_types(sidebar: &sidebar::DeviceSidebar) -> [(DeviceKind, bool); 7] {
    VIEW_KINDS.map(|(kind, _)| {
        let visible = sidebar
            .visible_keys()
            .any(|k| sidebar.card(k).is_some_and(|c| c.row.kind == kind));
        (kind, visible)
    })
}

/// The page's own focus handle; its actions are handled on the page's root.
fn page_focus(view: &AnyView, cx: &App) -> Option<FocusHandle> {
    fn of<T: Focusable>(view: &AnyView, cx: &App) -> Option<FocusHandle> {
        view.clone()
            .downcast::<T>()
            .ok()
            .map(|e| e.read(cx).focus_handle(cx))
    }
    of::<cpu::CpuPage>(view, cx)
        .or_else(|| of::<memory::MemoryPage>(view, cx))
        .or_else(|| of::<disk::DiskPage>(view, cx))
        .or_else(|| of::<net::NetPage>(view, cx))
        .or_else(|| of::<gpu::GpuPage>(view, cx))
        .or_else(|| of::<overview::OverviewPage>(view, cx))
}

/// The "Copied metrics to clipboard" toast, one at a time.
pub const PERF_COPY_TOAST: ToastKey = ToastKey(FIXED_KEY_BASE | 0x0402);

fn update_page(view: &AnyView, cx: &mut App, f: impl FnOnce(&mut dyn DevicePage, &mut App)) {
    let view = match view.clone().downcast::<cpu::CpuPage>() {
        Ok(e) => return e.update(cx, |p, cx| f(p, cx)),
        Err(view) => view,
    };
    let view = match view.downcast::<memory::MemoryPage>() {
        Ok(e) => return e.update(cx, |p, cx| f(p, cx)),
        Err(view) => view,
    };
    let view = match view.downcast::<disk::DiskPage>() {
        Ok(e) => return e.update(cx, |p, cx| f(p, cx)),
        Err(view) => view,
    };
    let view = match view.downcast::<net::NetPage>() {
        Ok(e) => return e.update(cx, |p, cx| f(p, cx)),
        Err(view) => view,
    };
    let view = match view.downcast::<gpu::GpuPage>() {
        Ok(e) => return e.update(cx, |p, cx| f(p, cx)),
        Err(view) => view,
    };
    let view = match view.downcast::<overview::OverviewPage>() {
        Ok(e) => return e.update(cx, |p, cx| f(p, cx)),
        Err(view) => view,
    };
    if let Ok(e) = view.downcast::<page::PlaceholderPage>() {
        e.update(cx, |p, cx| f(p, cx));
    }
}

/// The `focus_order` of the device page behind `view`. Same type list as [`update_page`].
fn page_focus_order(view: &AnyView, cx: &App) -> SmallVec<[FocusHandle; 4]> {
    let view = match view.clone().downcast::<cpu::CpuPage>() {
        Ok(e) => return e.read(cx).focus_order(),
        Err(view) => view,
    };
    let view = match view.downcast::<memory::MemoryPage>() {
        Ok(e) => return e.read(cx).focus_order(),
        Err(view) => view,
    };
    let view = match view.downcast::<disk::DiskPage>() {
        Ok(e) => return e.read(cx).focus_order(),
        Err(view) => view,
    };
    let view = match view.downcast::<net::NetPage>() {
        Ok(e) => return e.read(cx).focus_order(),
        Err(view) => view,
    };
    let view = match view.downcast::<gpu::GpuPage>() {
        Ok(e) => return e.read(cx).focus_order(),
        Err(view) => view,
    };
    let view = match view.downcast::<overview::OverviewPage>() {
        Ok(e) => return e.read(cx).focus_order(),
        Err(view) => view,
    };
    match view.downcast::<page::PlaceholderPage>() {
        Ok(e) => e.read(cx).focus_order(),
        Err(_) => SmallVec::new(),
    }
}

impl Focusable for PerformancePage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<StatusChanged> for PerformancePage {}

/// Summary mode turned on or off; the main window renders again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SummaryToggled;

impl EventEmitter<SummaryToggled> for PerformancePage {}

impl ShellPage for PerformancePage {
    fn primary_focus(&self) -> FocusHandle {
        PerformancePage::primary_focus(self)
    }

    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        PerformancePage::focus_order(self)
    }

    fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        PerformancePage::set_active(self, active, cx);
    }
}

impl Render for PerformancePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let s = metrics(cx).ui_scale;
        let nav = px(f32::from(self.nav_width) * s);
        let width = self.split.read(cx).container_size();
        if width != self.split_width {
            if self.split_width == Pixels::ZERO {
                self.split_width = width;
            } else {
                self.new_split(cx);
            }
        }

        let sidebar = div()
            .debug_selector(|| SIDEBAR_PANEL_ID.into())
            .size_full()
            .child(self.sidebar.clone());
        let page = div()
            .debug_selector(|| PAGE_PANEL_ID.into())
            .size_full()
            // Right-click anywhere on the device page: the page menu (UI-PRF-015). The press is
            // seen here before the Details grid takes it, and the menu opens once the grid has
            // named the value under the pointer in `ClickedValue`. The grid stops a press on a
            // value, so it opens no menu of its own and this one opens once.
            .capture_any_mouse_down(cx.listener(|_, ev: &MouseDownEvent, window, cx| {
                if ev.button != MouseButton::Right {
                    return;
                }
                cx.set_global(ClickedValue(None));
                let at = ev.position;
                cx.defer_in(window, move |this, window, cx| {
                    this.open_page_menu(at, window, cx)
                });
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .children(self.selected_view());

        // While the width animates, the sidebar sits in a clipping box before the split and the
        // split's own sidebar panel is hidden. The page panel keeps its place in the tree.
        let (in_split, animated) = match self.anim {
            None => (self.sidebar_visible.then_some(sidebar), None),
            Some(SidebarAnim {
                showing,
                generation,
            }) => {
                let el = div()
                    .flex_none()
                    .h_full()
                    .overflow_hidden()
                    .child(div().h_full().w(nav).child(sidebar))
                    .with_animation(
                        ("perf-sidebar-anim", generation as usize),
                        Animation::new(M_BASE).with_easing(ease_out_cubic),
                        move |el, t| el.w(nav * if showing { t } else { 1. - t }),
                    );
                (None, Some(el))
            }
        };

        let this = cx.weak_entity();
        let split = h_resizable("perf-split")
            .with_state(&self.split)
            .on_resize(move |state, _, cx| {
                let Some(&width) = state.read(cx).sizes().first() else {
                    return;
                };
                let _ = this.update(cx, |p, cx| p.on_split_resized(width, cx));
            })
            .child(
                resizable_panel()
                    .flex_none()
                    .size(nav)
                    .size_range(px(f32::from(NAV_MIN_W) * s)..px(f32::from(NAV_MAX_W) * s))
                    .visible(in_split.is_some())
                    .children(in_split),
            )
            .child(
                resizable_panel()
                    .size_range(px(page::PAGE_MIN_W * s)..Pixels::MAX)
                    .child(page),
            );

        let root = h_flex()
            .key_context(PERFORMANCE_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_jump_to_type))
            .on_action(cx.listener(Self::on_select_device))
            .on_action(cx.listener(Self::on_toggle_summary))
            .on_action(cx.listener(Self::on_set_graph_range))
            .on_action(cx.listener(Self::on_copy_value))
            .on_action(cx.listener(Self::on_copy_device_info))
            .size_full()
            .children(animated)
            .child(split);
        match self.leaving {
            Some(generation) => root
                .with_animation(
                    ("perf-summary-leave", generation as usize),
                    Animation::new(M_FAST).with_easing(ease_out_cubic),
                    |el, t| el.opacity(1. - t),
                )
                .into_any_element(),
            None => root.into_any_element(),
        }
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
            show_disconnected_fans: false,
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
            fan_connected: None,
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
        sidebar_order(&cat, &[], &view(&order, &overrides), &mut out);
        let keys: Vec<&str> = out.iter().map(|r| &*r.key.0).collect();
        assert_eq!(
            keys,
            [
                "net:enp38s0",
                "disk:Y",
                "overview",
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
    fn overview_row_follows_device_order() {
        let cat = catalog(vec![
            entry(0, DeviceIdentity::CpuPackage, true),
            entry(1, DeviceIdentity::Memory, true),
        ]);
        let overrides = HashMap::new();
        let mut out = Vec::new();
        sidebar_order(&cat, &[], &view(&[], &overrides), &mut out);
        let keys: Vec<&str> = out.iter().map(|r| &*r.key.0).collect();
        assert_eq!(keys, ["overview", "cpu", "memory"]);
        assert_eq!(out[0].kind, DeviceKind::Overview);
        assert_eq!(out[0].idx, DeviceIdx(0));

        let order = [key("cpu"), key("overview")];
        sidebar_order(&cat, &[], &view(&order, &overrides), &mut out);
        let keys: Vec<&str> = out.iter().map(|r| &*r.key.0).collect();
        assert_eq!(keys, ["cpu", "overview", "memory"]);
        assert!(is_visible(&out[1], &view(&order, &overrides)));

        let no_cpu = catalog(vec![entry(1, DeviceIdentity::Memory, true)]);
        sidebar_order(&no_cpu, &[], &view(&[], &overrides), &mut out);
        assert!(out.iter().all(|r| r.kind != DeviceKind::Overview));
    }

    fn fan_entries() -> DeviceCatalog {
        catalog(
            (1..=7)
                .map(|i| {
                    entry(
                        i,
                        DeviceIdentity::Fan {
                            chip: Arc::from("nct6797"),
                            index: i,
                        },
                        true,
                    )
                })
                .collect(),
        )
    }

    fn fan_reading(idx: u16, connected: bool) -> (DeviceIdx, SensorReading) {
        use tl_model::system::Metric;
        use tl_model::validity::Validity;
        (
            DeviceIdx(idx),
            SensorReading {
                device: DeviceIdx(idx),
                kind: SensorKind::Fan,
                value: Metric::with(Validity::Unsupported),
                pwm_pct: Metric::with(Validity::Unsupported),
                fan_control: None,
                fan_connected: connected,
            },
        )
    }

    fn visible_fans(
        sensors: &[(DeviceIdx, SensorReading)],
        settings: &PerformanceSettingsView,
    ) -> usize {
        let mut out = Vec::new();
        sidebar_order(&fan_entries(), sensors, settings, &mut out);
        visible_rows(&out, settings)
            .filter(|r| r.kind == DeviceKind::Fan)
            .count()
    }

    #[test]
    fn disconnected_fans_hidden_by_default() {
        let order = [];
        let mut overrides = HashMap::new();
        // Fans 1 to 4 spun; 5 to 7 never did.
        let sensors: Vec<_> = (1..=7).map(|i| fan_reading(i, i <= 4)).collect();
        assert_eq!(visible_fans(&sensors, &view(&order, &overrides)), 4);
        let all = PerformanceSettingsView {
            show_disconnected_fans: true,
            ..view(&order, &overrides)
        };
        assert_eq!(visible_fans(&sensors, &all), 7);
        overrides.insert(key("fan:nct6797:6"), DeviceVisibility::Show);
        assert_eq!(visible_fans(&sensors, &view(&order, &overrides)), 5);
    }

    #[test]
    fn disconnected_fan_with_label_visible() {
        // A label makes the engine set `fan_connected` even at 0 RPM.
        let order = [];
        let overrides = HashMap::new();
        let sensors = [fan_reading(1, true)];
        assert_eq!(visible_fans(&sensors, &view(&order, &overrides)), 1);
        // No reading at all counts as not connected.
        assert_eq!(visible_fans(&[], &view(&order, &overrides)), 0);
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
