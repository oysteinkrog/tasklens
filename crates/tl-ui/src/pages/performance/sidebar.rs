//! The device sidebar: one card per visible device in sidebar order, a mini chart per card, the
//! roving selection and its keys, scroll into view, and the sidebar's demand part
//! (docs/ui/performance.md UI-PRF-010, UI-PRF-011, UI-PRF-012; PERF-001, PERF-007; ADR-008,
//! ADR-009).
//!
//! Each complete card is a persistent cached view. A card re-renders only when its own chart,
//! text or presentation changes. Card text is read from the snapshot on `SYSTEM`, `INVENTORY`,
//! `DEVICE_DETAILS` and `DEVICES` changes and kept when its text is unchanged
//! ([`update_card_lines`]). The card text reads no series, so `SERIES` alone formats nothing.
//! The card words are translated once per strings version ([`card_words`]).
//!
//! Hotplug (UI-PRF-012): a new card grows in at its place and a removed card collapses where it
//! was, over `m_base`. A removed card keeps rendering until then, but it is out of the
//! selection, the keys and the demand. Each change starts one timer that ends its animations;
//! nothing runs while no card moves. With animations off the change is at once. Edit mode
//! (E24-013) is a later bead.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, AppContext as _, AsyncApp,
    Context, DragMoveEvent, ElementId, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, Point, Render, ScrollHandle, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Subscription, WeakEntity, Window, canvas, div, px, white,
};
use rustc_hash::FxHashMap;
use smallvec::{SmallVec, smallvec};
use tl_model::demand::{CpuDemand, Demand};
use tl_model::ids::{MetricId, SeriesId};
use tl_model::settings::{DeviceVisibility, NetworkUnit};
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;

use super::actions::{
    PERFORMANCE_CONTEXT, PerfDisableAllPages, PerfEnableAllPages, PerfEnterEditMode,
    PerfExitEditMode, PerfMoveDevice, PerfMoveFocusedDevice, PerfOpenCardMenu,
    PerfResetPagesToDefault, PerfSelectDevice, PerfSelectFirstDevice, PerfSelectLastDevice,
    PerfSelectNextDevice, PerfSelectPrevDevice, PerfSelectVisibleDevice, PerfSetPageVisible,
};
use super::card::{
    CARD_CHART_BG, CARD_H, CardLines, CardTextInput, CardView, CardWords, DeviceCard, StaleTimer,
    card_chart_grid, card_chart_labels, card_words, update_card_lines,
};
use super::menu::card_menu;
use super::{
    DeviceKey, DeviceKind, DeviceRow, PerformanceSettingsView, fan_connected, initial_selection,
    is_visible, sidebar_order, tray_available, type_number, visible_rows,
};
use crate::chart::{ChartSeries, ChartSpec, ChartView, ValueFormat, YScale};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::motion::{M_BASE, M_FAST, ease_out_cubic};
use crate::components::switch::TlSwitch;
use crate::components::toast::{FIXED_KEY_BASE, ToastKey, ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::{Section, TlSettings, animations_on};
use crate::theme::derived::blend_over;
use crate::theme::palette::accent_color;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

const NS_PER_S: u64 = 1_000_000_000;
/// Fill alpha of a type that has one colour (UI-PRF-005 table note).
const SINGLE_FILL_ALPHA: f32 = 0.85;
/// The upload line is `net_line` mixed this far toward white.
const UPLOAD_TINT: f32 = 0.3;
/// Fan and battery line colours (UI-PRF-005: "Violet" and "Mint").
const FAN_COLOR: &str = "violet";
const BATTERY_COLOR: &str = "mint";

/// Removes disk children from a sidebar list. The engine keeps partitions and device-mapper
/// devices in the catalog for attribution, but only their whole parent disks get cards.
pub(super) fn retain_whole_disks(
    rows: &mut Vec<DeviceRow>,
    catalog: &tl_model::devices::DeviceCatalog,
) {
    rows.retain(|row| {
        row.kind != DeviceKind::Disk
            || catalog
                .entries
                .iter()
                .find(|entry| entry.idx == row.idx)
                .is_none_or(|entry| entry.parent.is_none())
    });
}

/// Edit-mode group. Overview shares Processor because it is derived from the CPU package and the
/// edit-mode specification has no separate Overview group.
fn edit_group(kind: DeviceKind) -> usize {
    match kind {
        DeviceKind::Overview | DeviceKind::Cpu => 0,
        DeviceKind::Memory => 1,
        DeviceKind::Disk => 2,
        DeviceKind::Gpu => 3,
        DeviceKind::Net => 4,
        DeviceKind::Fan => 5,
        DeviceKind::Battery => 6,
        DeviceKind::AggregateDisk | DeviceKind::AggregateGpu => 7,
    }
}

/// Opacity of an edit-mode row: the dragged row stays at 0.3 through the edit-mode fade
/// (UI-PRF-013), so `fade` scales it rather than replacing it.
fn edit_row_opacity(dragging: bool, fade: f32) -> f32 {
    if dragging { 0.3 * fade } else { fade }
}

/// Element id of the scrolling card list.
pub const LIST_ID: &str = "perf-sidebar-list";

/// The "At least one performance page must stay visible" toast, one at a time (R-232).
pub const LAST_PAGE_TOAST: ToastKey = ToastKey(FIXED_KEY_BASE | 0x0401);

struct EditWords {
    groups: [gpui_kit::SharedString; 8],
    empty: [gpui_kit::SharedString; 5],
    title: gpui_kit::SharedString,
    enable_all: gpui_kit::SharedString,
    disable_all: gpui_kit::SharedString,
    reset: gpui_kit::SharedString,
    done: gpui_kit::SharedString,
    tip: gpui_kit::SharedString,
}

fn edit_words(cx: &App) -> Rc<EditWords> {
    Rc::new(EditWords {
        groups: [
            tr(cx, "perf-group-processor"),
            tr(cx, "perf-group-memory"),
            tr(cx, "perf-group-disks"),
            tr(cx, "perf-group-gpus"),
            tr(cx, "perf-group-nets"),
            tr(cx, "perf-group-fans"),
            tr(cx, "perf-group-batteries"),
            tr(cx, "perf-group-aggregate"),
        ],
        empty: [
            tr(cx, "perf-empty-disk"),
            tr(cx, "perf-empty-gpu"),
            tr(cx, "perf-empty-net"),
            tr(cx, "perf-empty-fan"),
            tr(cx, "perf-empty-battery"),
        ],
        title: tr(cx, "perf-edit-title"),
        enable_all: tr(cx, "perf-edit-enable-all"),
        disable_all: tr(cx, "perf-edit-disable-all"),
        reset: tr(cx, "perf-edit-reset"),
        done: tr(cx, "perf-edit-done"),
        tip: tr(cx, "perf-tip-edit"),
    })
}

/// The snapshot components the card text reads.
const TEXT_MASK: ComponentMask = ComponentMask(
    ComponentMask::SYSTEM.0
        | ComponentMask::INVENTORY.0
        | ComponentMask::DEVICE_DETAILS.0
        | ComponentMask::DEVICES.0,
);

/// A card's hotplug animation (UI-PRF-012).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAnim {
    Steady,
    /// Height 0 to the card height and opacity 0 to 1.
    Inserting {
        started: Instant,
    },
    /// Opacity 1 to 0 and height from the card height to 0; the card goes when it ends.
    Removing {
        started: Instant,
    },
}

/// One card in the list: a visible device, or one that is collapsing out.
pub struct CardState {
    pub row: DeviceRow,
    pub chart: Entity<ChartView>,
    pub view: Entity<CardView>,
    pub lines: CardLines,
    /// The card height from its last render: at least `CARD_H`, more when its text wraps.
    pub height: Rc<Cell<Pixels>>,
    /// Index among the visible cards of its kind: "Disk 0".
    pub number: u16,
    pub anim: CardAnim,
    /// The sync that started `anim`; its timer ends every animation up to it.
    anim_seq: u64,
    /// `Removed` was emitted early: the same key came back while this card collapsed.
    page_released: bool,
    /// The snapshot value the chart's Y scale was built from ([`scale_input`]).
    scale: u64,
}

impl CardState {
    pub fn is_removing(&self) -> bool {
        matches!(self.anim, CardAnim::Removing { .. })
    }
}

/// Emitted to the `PerformancePage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarEvent {
    Selected(DeviceKey),
    /// The card left the list: the device went away, came back with a new generation, or was
    /// hidden.
    Removed(DeviceKey),
}

#[derive(Clone, Debug)]
struct DraggedCard {
    key: DeviceKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DragState {
    key: DeviceKey,
    group: DeviceKind,
    /// Drop boundary inside `group`, from zero through the group length.
    over: Option<usize>,
}

struct DragGhost {
    key: DeviceKey,
    chart: Entity<ChartView>,
    lines: CardLines,
    width: Pixels,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().opacity(0.8).child(DeviceCard {
            key: self.key.clone(),
            chart: self.chart.clone(),
            lines: self.lines.clone(),
            selected: false,
            focused: false,
            edit_mode: true,
            hidden_in_edit: false,
            pointer: false,
            width: self.width,
            prepared: None,
        })
    }
}

/// The settings the sidebar draws from.
#[derive(Clone, Debug, PartialEq)]
struct SidebarSettings {
    order: Vec<DeviceKey>,
    overrides: HashMap<DeviceKey, DeviceVisibility>,
    show_disks: bool,
    show_gpus: bool,
    show_fans: bool,
    show_disconnected_fans: bool,
    show_batteries: bool,
    show_network_wired: bool,
    show_network_wireless: bool,
    show_network_vpn: bool,
    show_network_virtual: bool,
    show_network_other: bool,
    range_ns: u64,
    smooth: bool,
    cpu_kernel: bool,
    network_dynamic_scale: bool,
    network_unit: NetworkUnit,
    network_auto_scale: bool,
    /// `[fans]` labels by fan key.
    fan_labels: FxHashMap<DeviceKey, Arc<str>>,
}

impl SidebarSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        let p = &s.performance;
        let key = |k: &str| DeviceKey(Arc::from(k));
        Self {
            order: p.device_order.iter().map(|k| key(k)).collect(),
            overrides: p
                .device_overrides
                .iter()
                .map(|(k, v)| (key(k), *v))
                .collect(),
            show_disks: p.show_disks,
            show_gpus: p.show_gpus,
            show_fans: p.show_fans,
            show_disconnected_fans: p.show_disconnected_fans,
            show_batteries: p.show_batteries,
            show_network_wired: p.show_network_wired,
            show_network_wireless: p.show_network_wireless,
            show_network_vpn: p.show_network_vpn,
            show_network_virtual: p.show_network_virtual,
            show_network_other: p.show_network_other,
            range_ns: u64::from(p.graph_range_s) * NS_PER_S,
            smooth: s.motion.smooth_graphs,
            cpu_kernel: p.cpu_kernel,
            network_dynamic_scale: p.network_dynamic_scale,
            network_unit: s.units.network,
            network_auto_scale: s.units.network_auto_scale,
            fan_labels: s
                .fans
                .iter()
                .filter_map(|(k, f)| Some((key(k), Arc::from(f.label.as_deref()?))))
                .collect(),
        }
    }

    fn view(&self) -> PerformanceSettingsView<'_> {
        PerformanceSettingsView {
            order: &self.order,
            overrides: &self.overrides,
            show_disks: self.show_disks,
            show_gpus: self.show_gpus,
            show_fans: self.show_fans,
            show_disconnected_fans: self.show_disconnected_fans,
            show_batteries: self.show_batteries,
            show_network_wired: self.show_network_wired,
            show_network_wireless: self.show_network_wireless,
            show_network_vpn: self.show_network_vpn,
            show_network_virtual: self.show_network_virtual,
            show_network_other: self.show_network_other,
        }
    }
}

/// The versions of the sections [`SidebarSettings`] reads.
fn section_versions(cx: &App) -> [u64; 4] {
    let tl = TlSettings::get(cx);
    [
        tl.version(Section::Performance),
        tl.version(Section::Motion),
        tl.version(Section::Units),
        tl.version(Section::Fans),
    ]
}

/// The snapshot value a card's Y scale and value format depend on: the memory total, the known
/// link speed when the network scale follows it, whether a battery powers the system. 0 for every
/// other kind.
fn scale_input(row: &DeviceRow, snap: &Snapshot, s: &SidebarSettings) -> u64 {
    match row.kind {
        DeviceKind::Memory => snap.system.memory.total,
        DeviceKind::Net if !s.network_dynamic_scale => snap
            .system
            .nets
            .iter()
            .find(|(i, _)| *i == row.idx)
            .map(|(_, n)| n.link_speed_mbps)
            .filter(|m| m.validity == Validity::Valid)
            .map_or(0, |m| u64::from(m.value)),
        DeviceKind::Battery => snap
            .device_details
            .batteries
            .iter()
            .find(|(i, _)| *i == row.idx)
            .map_or(0, |(_, b)| u64::from(b.power_supply)),
        _ => 0,
    }
}

/// The mini chart of a card and its value format. `scale` is [`scale_input`].
fn card_spec(
    row: &DeviceRow,
    scale: u64,
    s: &SidebarSettings,
    theme: &TlTheme,
) -> (ChartSpec, ValueFormat) {
    let g = &theme.graph;
    let id = |metric| SeriesId {
        device: row.idx,
        metric,
    };
    let filled = |metric, line: gpui_kit::Hsla, fill| ChartSeries {
        id: id(metric),
        fill: Some(fill),
        line,
        ..ChartSeries::default()
    };
    let single =
        |metric, line: gpui_kit::Hsla| filled(metric, line, line.opacity(SINGLE_FILL_ALPHA));
    let percent = YScale::Fixed {
        min: 0.0,
        max: 100.0,
    };
    let (series, y, fmt): (SmallVec<[ChartSeries; 4]>, _, _) = match row.kind {
        // The CPU total of the CPU package (`row.idx`), so no new history (D-022).
        DeviceKind::Overview => (
            smallvec![filled(MetricId::CPU_TOTAL, g.cpu_line, g.cpu_fill)],
            percent,
            ValueFormat::Percent,
        ),
        DeviceKind::Cpu => {
            let mut series = smallvec![filled(MetricId::CPU_TOTAL, g.cpu_line, g.cpu_fill)];
            if s.cpu_kernel {
                series.push(ChartSeries {
                    id: id(MetricId::CPU_KERNEL),
                    line: g.kernel_line,
                    ..ChartSeries::default()
                });
            }
            (series, percent, ValueFormat::Percent)
        }
        DeviceKind::Memory => (
            smallvec![
                filled(MetricId::MEM_USED, g.mem_line, g.mem_fill),
                ChartSeries {
                    id: id(MetricId::MEM_COMMIT),
                    line: g.mem_line,
                    dashed: true,
                    ..ChartSeries::default()
                },
            ],
            YScale::FixedTotal(scale as f32),
            ValueFormat::PercentOf { total: scale },
        ),
        DeviceKind::Gpu => (
            smallvec![filled(MetricId::GPU_UTIL, g.gpu_line, g.gpu_fill)],
            percent,
            ValueFormat::Percent,
        ),
        DeviceKind::Disk => (
            smallvec![filled(MetricId::DISK_ACTIVE, g.disk_line, g.disk_fill)],
            percent,
            ValueFormat::Percent,
        ),
        DeviceKind::Net => {
            let y = if scale > 0 {
                // Mbps to bits per second.
                YScale::LinkSpeed(scale as f32 * 1e6)
            } else {
                YScale::AutoRate {
                    iec: s.network_unit == NetworkUnit::Bytes,
                }
            };
            (
                smallvec![
                    filled(MetricId::NET_RX, g.net_line, g.net_fill),
                    ChartSeries {
                        id: id(MetricId::NET_TX),
                        line: blend_over(g.net_line, white().opacity(UPLOAD_TINT)),
                        dashed: true,
                        ..ChartSeries::default()
                    },
                ],
                y,
                ValueFormat::RateNetwork,
            )
        }
        DeviceKind::Fan => {
            let line = accent_color(FAN_COLOR).unwrap_or(g.gpu_line);
            (
                smallvec![single(MetricId::FAN_RPM, line)],
                // The snapshot carries no fan maximum, so the scale only grows.
                YScale::StickyUp { hw_max: None },
                ValueFormat::Rpm,
            )
        }
        DeviceKind::Battery => {
            let line = accent_color(BATTERY_COLOR).unwrap_or(g.gpu_line);
            if scale == 1 {
                // A battery that powers the system draws its charge and discharge power.
                (
                    smallvec![single(MetricId::BATTERY_POWER, line)],
                    YScale::Symmetric,
                    ValueFormat::Watts,
                )
            } else {
                (
                    smallvec![single(MetricId::BATTERY_PCT, line)],
                    percent,
                    ValueFormat::Percent,
                )
            }
        }
        // The engine publishes the aggregate's busiest-disk and GPU utilisation on its own idx.
        DeviceKind::AggregateDisk => (
            smallvec![filled(MetricId::DISK_ACTIVE, g.disk_line, g.disk_fill)],
            percent,
            ValueFormat::Percent,
        ),
        DeviceKind::AggregateGpu => (
            smallvec![filled(MetricId::GPU_UTIL, g.gpu_line, g.gpu_fill)],
            percent,
            ValueFormat::Percent,
        ),
    };
    let spec = ChartSpec {
        series,
        y,
        range_ns: s.range_ns,
        grid: card_chart_grid(),
        smooth: s.smooth,
        background: CARD_CHART_BG,
        ..ChartSpec::default()
    };
    (spec, fmt)
}

/// `state.performance.selected_device`.
fn saved_selection(cx: &App) -> DeviceKey {
    DeviceKey(Arc::from(
        TlSettings::state(cx).performance.selected_device.as_str(),
    ))
}

/// The device sidebar entity.
pub struct DeviceSidebar {
    /// The cards in sidebar order, collapsing ones included.
    pub(crate) cards: Vec<CardState>,
    /// Position in `cards` by key, for cards that are not collapsing.
    index: FxHashMap<DeviceKey, usize>,
    /// Bumped by each card list change that animates.
    anim_seq: u64,
    /// Every present device in sidebar order, hidden ones included.
    pub(crate) rows: Vec<DeviceRow>,
    pub(crate) selected: Option<DeviceKey>,
    /// Key context "Performance", the list's one Tab stop.
    pub(crate) focus: FocusHandle,
    pub(crate) scroll: ScrollHandle,
    /// E24-013.
    pub(crate) edit_mode: bool,
    /// The row that owns keyboard focus in edit mode. E24-014 uses it for reordering.
    pub(crate) edit_focus_row: Option<DeviceKey>,
    drag: Option<DragState>,
    /// Every row gets a card while edit mode is mounted, including hidden rows.
    edit_cards: Vec<CardState>,
    /// Keeps the edit surface mounted while its leave animation runs.
    edit_leaving: bool,
    edit_generation: u64,
    /// Edit cards are not cached [`CardView`]s, so the sidebar draws them again when the first
    /// Valid value among them turns Stale.
    edit_stale: StaleTimer,
    pub(crate) hub: Entity<SnapshotHub>,
    pub(crate) demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    /// The Performance tab is the active tab. While false the demand part is withdrawn.
    active: bool,
    /// A selection change is saved as `selected_device`. Off for the mini window's private
    /// sidebar, whose selection is never shown.
    saves_selection: bool,
    settings: SidebarSettings,
    settings_versions: [u64; 4],
    /// `UiMetrics.style_version` the chart colours were built for.
    style_version: u64,
    catalog_version: u64,
    /// Card width from the last paint: the sidebar width less its padding.
    card_width: Rc<Cell<Pixels>>,
    /// Reused by [`update_card_lines`].
    scratch: String,
    /// The card words for the current strings version.
    words: Rc<CardWords>,
    edit_words: Rc<EditWords>,
    _subs: SmallVec<[Subscription; 4]>,
    /// Focus in and out of the list redraw the focus ring. Set on the first render.
    focus_subs: Option<[Subscription; 2]>,
    cards_focused: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl EventEmitter<SidebarEvent> for DeviceSidebar {}

impl DeviceSidebar {
    /// Builds the cards from the installed snapshot and selects the saved device, or the first
    /// visible one. Subscribes to the hub and observes the settings.
    pub fn new(
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let snap = Arc::clone(hub.read(cx).current());
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<UiMetrics>(Self::sync_style));
        subs.push(cx.observe_global::<Strings>(Self::sync_words));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let m = *metrics(cx);
        let nav = TlSettings::state(cx).performance.nav_width.clamp(180, 400);
        let card_width = px(f32::from(nav) * m.ui_scale) - m.sp[4] * 2.0;
        let mut this = Self {
            cards: Vec::new(),
            index: FxHashMap::default(),
            anim_seq: 0,
            rows: Vec::new(),
            selected: None,
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            edit_mode: false,
            edit_focus_row: None,
            drag: None,
            edit_cards: Vec::new(),
            edit_leaving: false,
            edit_generation: 0,
            edit_stale: StaleTimer::default(),
            hub,
            demand,
            id,
            window: None,
            active: true,
            saves_selection: true,
            settings: SidebarSettings::read(cx),
            settings_versions: section_versions(cx),
            style_version: m.style_version,
            catalog_version: snap.devices.version,
            card_width: Rc::new(Cell::new(card_width)),
            scratch: String::new(),
            words: card_words(cx),
            edit_words: edit_words(cx),
            _subs: subs,
            focus_subs: None,
            cards_focused: false,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        // The first cards are there at once.
        this.sync_cards(&snap, false, cx);
        this.refresh_lines(&snap, cx);
        this.selected = initial_selection(&this.visible(), Some(&saved_selection(cx)));
        this.sync_card_selection(cx);
        if let Some(ix) = this.selected_index() {
            this.scroll.scroll_to_item(ix);
        }
        this
    }

    /// Selects the card of `key`: scrolls it into view, saves it as `selected_device`, emits
    /// `Selected` and redraws. Nothing happens for a key without a visible card or for the
    /// selected key. In edit mode the key becomes the Alt+Up and Alt+Down row first, hidden
    /// rows included, since edit mode lists them too.
    pub fn select(&mut self, key: DeviceKey, cx: &mut Context<Self>) {
        if self.edit_mode && self.rows.iter().any(|row| row.key == key) {
            self.edit_focus_row = Some(key.clone());
        }
        let Some(&ix) = self.index.get(&key) else {
            return;
        };
        if self.selected.as_ref() == Some(&key) {
            return;
        }
        self.scroll.scroll_to_item(ix);
        self.selected = Some(key.clone());
        if self.saves_selection && *TlSettings::state(cx).performance.selected_device != *key.0 {
            let saved = key.0.to_string();
            TlSettings::update(cx, |d| d.state.performance.selected_device = saved);
        }
        self.sync_card_selection(cx);
        cx.emit(SidebarEvent::Selected(key));
    }

    /// The keys of the visible cards, in sidebar order. Collapsing cards are not visible.
    pub fn visible_keys(&self) -> impl Iterator<Item = &DeviceKey> {
        self.live().map(|c| &c.row.key)
    }

    /// The cards that are not collapsing, in sidebar order.
    fn live(&self) -> impl Iterator<Item = &CardState> {
        self.cards.iter().filter(|c| !c.is_removing())
    }

    /// The text of the card of `key`; page headers reuse its heading.
    pub fn heading_for(&self, key: &DeviceKey) -> Option<&CardLines> {
        self.card(key).map(|c| &c.lines)
    }

    pub fn card(&self, key: &DeviceKey) -> Option<&CardState> {
        self.index.get(key).map(|&i| &self.cards[i])
    }

    /// Every card in the list, in sidebar order, collapsing ones included.
    pub fn cards(&self) -> &[CardState] {
        &self.cards
    }

    /// Every present device in sidebar order, hidden ones included.
    pub fn rows(&self) -> &[DeviceRow] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&DeviceKey> {
        self.selected.as_ref()
    }

    pub fn edit_mode(&self) -> bool {
        self.edit_mode
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_drag_for_test(&mut self, key: DeviceKey, over: Option<usize>) {
        self.drag = Some(DragState {
            group: key.kind(),
            key,
            over,
        });
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn dragging_for_test(&self) -> bool {
        self.drag.is_some()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn drag_over_for_test(&self) -> Option<Option<usize>> {
        self.drag.as_ref().map(|drag| drag.over)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn edit_rows_for_test(&self) -> Vec<(DeviceKey, DeviceKind, bool)> {
        self.edit_cards
            .iter()
            .map(|card| {
                (
                    card.row.key.clone(),
                    card.row.kind,
                    is_visible(&card.row, &self.settings.view()),
                )
            })
            .collect()
    }

    /// The stale deadline the edit cards armed, if any.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn edit_stale_at_for_test(&self) -> Option<u64> {
        self.edit_stale.at()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn edit_card_opacity_for_test(&self, key: &DeviceKey) -> Option<f32> {
        let card = self.edit_cards.iter().find(|card| card.row.key == *key)?;
        let visible = is_visible(&card.row, &self.settings.view());
        Some(
            DeviceCard {
                key: card.row.key.clone(),
                chart: card.chart.clone(),
                lines: card.lines.clone(),
                selected: self.selected.as_ref() == Some(key),
                focused: false,
                edit_mode: true,
                hidden_in_edit: !visible,
                pointer: false,
                width: px(0.),
                prepared: None,
            }
            .opacity(),
        )
    }

    /// This surface's demand part: CPU total and memory always, and each device class that has
    /// a visible card. `cpu_fast` is the CPU page's. Sensors are also wanted while a fan row
    /// exists and fans are enabled: a fan is shown only after it has spun, and that is only
    /// known from sensor readings, so a hidden fan must keep sampling on. The engine lists the
    /// fans without any demand, so no fan row means no sensor demand.
    ///
    /// Disks and network interfaces are the other way round: the engine lists them only while
    /// they are demanded (ARCHITECTURE "Device enumeration ... while any of those is
    /// demanded"). So while a class is shown and has no row yet, the sidebar asks for it, or
    /// a start on a page other than Overview never gets a Disk or network card.
    pub fn demand_part(&self) -> Demand {
        let has = |kind| self.live().any(|c| c.row.kind == kind);
        let view = self.settings.view();
        let fans_pending = view.show_fans && self.rows.iter().any(|r| r.kind == DeviceKind::Fan);
        let unlisted = |kind, shown: bool| shown && !self.rows.iter().any(|r| r.kind == kind);
        let nets_shown = view.show_network_wired
            || view.show_network_wireless
            || view.show_network_vpn
            || view.show_network_virtual
            || view.show_network_other;
        Demand {
            any_surface_visible: true,
            cpu: CpuDemand::Total,
            memory: true,
            disks: has(DeviceKind::Disk) || unlisted(DeviceKind::Disk, view.show_disks),
            nets: has(DeviceKind::Net) || unlisted(DeviceKind::Net, nets_shown),
            gpus: has(DeviceKind::Gpu),
            sensors: has(DeviceKind::Fan) || has(DeviceKind::Battery) || fans_pending,
            ..Demand::default()
        }
    }

    /// Called by the page on tab activation. Inactive withdraws the demand part, active sends
    /// it again. An unchanged value does nothing, so staying on the tab sends nothing.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.send_demand(cx);
        } else {
            let id = self.id;
            self.demand.update(cx, |d, cx| d.remove(id, cx));
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Off stops [`Self::select`] from saving the selection as `selected_device`. The mini
    /// window's private sidebar turns it off, so its own fallback selection never replaces the
    /// main window's.
    pub fn set_saves_selection(&mut self, saves: bool) {
        self.saves_selection = saves;
    }

    /// Sends the demand part while the tab is active.
    fn send_demand(&self, cx: &mut App) {
        if !self.active {
            return;
        }
        let (id, window, part) = (self.id, self.window, self.demand_part());
        self.demand.update(cx, |d, cx| d.set(id, window, part, cx));
    }

    fn visible(&self) -> Vec<DeviceRow> {
        self.live().map(|c| c.row.clone()).collect()
    }

    fn new_card(
        &self,
        row: &DeviceRow,
        number: u16,
        anim: CardAnim,
        snap: &Snapshot,
        cx: &mut App,
    ) -> CardState {
        let scale = scale_input(row, snap, &self.settings);
        let (spec, fmt) = card_spec(row, scale, &self.settings, current(cx));
        let hub = self.hub.clone();
        let chart = cx.new(|cx| ChartView::new(spec, card_chart_labels(fmt), &hub, cx));
        let height = Rc::new(Cell::new(px(CARD_H * metrics(cx).ui_scale)));
        let view = cx.new(|_| {
            CardView::new(
                row.key.clone(),
                chart.clone(),
                Rc::clone(&self.card_width),
                Rc::clone(&height),
                self.id,
            )
        });
        CardState {
            row: row.clone(),
            chart,
            view,
            lines: CardLines::default(),
            height,
            number,
            anim,
            anim_seq: self.anim_seq,
            page_released: false,
            scale,
        }
    }

    /// Recomputes `rows` and the visible cards: keeps the state of a card whose row is
    /// unchanged and makes a card for a new row. A card whose row went collapses in its place
    /// and is dropped when that ends, and the sidebar emits `Removed` then. With animations off,
    /// or `animate` false (the first build), new cards are steady and gone ones are dropped at
    /// once. Sends the demand part. Returns
    /// the visible cards before when they changed.
    fn sync_cards(
        &mut self,
        snap: &Snapshot,
        animate: bool,
        cx: &mut Context<Self>,
    ) -> Option<Vec<DeviceRow>> {
        sidebar_order(
            &snap.devices,
            &snap.system.sensors,
            &self.settings.view(),
            &mut self.rows,
        );
        retain_whole_disks(&mut self.rows, &snap.devices);
        let visible: Vec<DeviceRow> = visible_rows(&self.rows, &self.settings.view())
            .cloned()
            .collect();
        // A fan whose `connected` flag flipped while it is shown keeps its card: only the
        // stored row changes, so the card does not collapse and re-insert.
        for card in &mut self.cards {
            if card.is_removing() {
                continue;
            }
            if let Some(row) = visible.iter().find(|r| {
                r.key == card.row.key
                    && r.idx == card.row.idx
                    && r.generation == card.row.generation
            }) {
                card.row = row.clone();
            }
        }
        let before: Vec<DeviceRow> = self.visible();
        if visible == before {
            return None;
        }

        let animate = animate && animations_on(cx);
        let now = Instant::now();
        self.anim_seq += 1;
        let seq = self.anim_seq;
        let leaves = |c: &CardState| c.is_removing() || !visible.contains(&c.row);
        let mut old: Vec<Option<CardState>> = std::mem::take(&mut self.cards)
            .into_iter()
            .map(Some)
            .collect();
        let mut cards = Vec::with_capacity(old.len().max(visible.len()));
        // Old cards before `next` are placed; leaving ones keep their place among the rest.
        let mut next = 0;
        let mut animating = false;
        for row in &visible {
            let number = type_number(&visible, &row.key);
            let found = old.iter().position(|c| {
                c.as_ref()
                    .is_some_and(|c| !c.is_removing() && c.row == *row)
            });
            let card = match found {
                Some(p) => {
                    if p >= next {
                        for slot in &mut old[next..p] {
                            if let Some(c) = slot.take_if(|c| leaves(c)) {
                                cards.push(c);
                            }
                        }
                        next = p + 1;
                    }
                    let mut card = old[p].take().expect("found above");
                    card.number = number;
                    card
                }
                None if animate => {
                    animating = true;
                    self.new_card(row, number, CardAnim::Inserting { started: now }, snap, cx)
                }
                None => self.new_card(row, number, CardAnim::Steady, snap, cx),
            };
            cards.push(card);
        }
        cards.extend(old.into_iter().flatten());

        // Cards that just left start collapsing, or go at once.
        let mut kept = Vec::with_capacity(cards.len());
        for mut card in cards {
            let gone = !card.is_removing() && !visible.contains(&card.row);
            if gone && !animate {
                cx.emit(SidebarEvent::Removed(card.row.key));
                continue;
            }
            if gone {
                card.anim = CardAnim::Removing { started: now };
                card.anim_seq = seq;
                animating = true;
            }
            kept.push(card);
        }
        self.cards = kept;
        self.index.clear();
        for (i, card) in self.cards.iter().enumerate() {
            if !card.is_removing() {
                self.index.insert(card.row.key.clone(), i);
            }
        }
        // A key that came back while its old card collapses gets a new page: drop the old one.
        for card in &mut self.cards {
            if card.is_removing() && !card.page_released && self.index.contains_key(&card.row.key) {
                card.page_released = true;
                cx.emit(SidebarEvent::Removed(card.row.key.clone()));
            }
        }
        if animating {
            self.end_anims_after(seq, cx);
        }

        self.send_demand(cx);
        Some(before)
    }

    /// After `m_base`, makes the cards inserted up to `seq` steady and drops the ones removed up
    /// to it, emitting `Removed` for each.
    fn end_anims_after(&self, seq: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(M_BASE).await;
            let _ = this.update(cx, |this, cx| this.end_anims(seq, cx));
        })
        .detach();
    }

    fn end_anims(&mut self, seq: u64, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut kept = Vec::with_capacity(self.cards.len());
        for mut card in std::mem::take(&mut self.cards) {
            if card.anim_seq > seq {
                kept.push(card);
                continue;
            }
            match card.anim {
                CardAnim::Steady => kept.push(card),
                CardAnim::Inserting { .. } => {
                    card.anim = CardAnim::Steady;
                    changed = true;
                    kept.push(card);
                }
                CardAnim::Removing { .. } => {
                    changed = true;
                    if !card.page_released {
                        cx.emit(SidebarEvent::Removed(card.row.key));
                    }
                }
            }
        }
        self.cards = kept;
        if changed {
            self.index.clear();
            for (i, card) in self.cards.iter().enumerate() {
                if !card.is_removing() {
                    self.index.insert(card.row.key.clone(), i);
                }
            }
            cx.notify();
        }
    }

    /// [`Self::sync_cards`], then moves the selection off a card that went: to the next card
    /// below it, else the one above (UI-PRF-012). With no selection, selects the saved device
    /// or the first card. True when the card list changed.
    fn sync_rows(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let Some(before) = self.sync_cards(snap, true, cx) else {
            return false;
        };
        let next = match self.selected.clone() {
            Some(selected) if self.index.contains_key(&selected) => None,
            Some(selected) => {
                self.selected = None;
                before
                    .iter()
                    .position(|r| r.key == selected)
                    .and_then(|at| {
                        before[at + 1..]
                            .iter()
                            .chain(before[..at].iter().rev())
                            .find(|row| self.index.contains_key(&row.key))
                            .map(|row| row.key.clone())
                    })
                    .or_else(|| self.live().next().map(|c| c.row.key.clone()))
            }
            None => initial_selection(&self.visible(), Some(&saved_selection(cx))),
        };
        if let Some(next) = next {
            self.select(next, cx);
        }
        self.sync_card_selection(cx);
        true
    }

    /// Re-formats every card's text. True when any line changed.
    fn refresh_lines(&mut self, snap: &Snapshot, cx: &mut App) -> bool {
        let mut changed = false;
        for card in &mut self.cards {
            let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == card.row.idx) else {
                continue;
            };
            let input = CardTextInput {
                words: &self.words,
                kind: card.row.kind,
                number: card.number,
                entry,
                snap,
                network_unit: self.settings.network_unit,
                auto_scale: self.settings.network_auto_scale,
                fan_label: self.settings.fan_labels.get(&card.row.key).map(|l| &**l),
                // P3 (PERF-077) pairs a temperature sensor with a fan.
                fan_temp: None,
            };
            if update_card_lines(&mut card.lines, &input, &mut self.scratch) {
                changed = true;
                let lines = card.lines.clone();
                card.view.update(cx, |view, cx| view.set_lines(lines, cx));
            }
        }
        changed
    }

    /// Pushes selection into the persistent card leaves.
    fn sync_card_selection(&mut self, cx: &mut App) {
        for card in &mut self.cards {
            let selected = !card.is_removing() && self.selected.as_ref() == Some(&card.row.key);
            card.view
                .update(cx, |view, cx| view.set_selected(selected, cx));
        }
    }

    /// Pushes list focus into every card because it changes only their focus rings.
    fn sync_card_focus(&mut self, focused: bool, cx: &mut App) {
        for card in &self.cards {
            card.view
                .update(cx, |view, cx| view.set_focused(focused, cx));
        }
    }

    /// Rebuilds the chart of each card whose scale input moved, or of every card with `all`.
    fn sync_charts(&mut self, snap: &Snapshot, all: bool, cx: &mut App) {
        let theme = current(cx).clone();
        for card in &mut self.cards {
            let scale = scale_input(&card.row, snap, &self.settings);
            if !all && scale == card.scale {
                continue;
            }
            card.scale = scale;
            let (spec, fmt) = card_spec(&card.row, scale, &self.settings, &theme);
            card.chart.update(cx, |v, cx| {
                v.set_spec(spec, cx);
                v.set_labels(card_chart_labels(fmt), cx);
            });
        }
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        if !ev.mask.intersects(TEXT_MASK) {
            return;
        }
        let snap = Arc::clone(hub.read(cx).current());
        let mut changed = false;
        if ev.mask.contains(ComponentMask::DEVICES) && snap.devices.version != self.catalog_version
        {
            self.catalog_version = snap.devices.version;
            changed |= self.sync_rows(&snap, cx);
            if self.edit_mode || self.edit_leaving {
                self.rebuild_edit_cards(&snap, cx);
            }
        } else if ev.mask.contains(ComponentMask::SYSTEM)
            && self.rows.iter().any(|r| {
                r.fan_connected
                    .is_some_and(|c| c != fan_connected(&snap.system.sensors, r.idx))
            })
        {
            changed |= self.sync_rows(&snap, cx);
            if self.edit_mode || self.edit_leaving {
                self.rebuild_edit_cards(&snap, cx);
            }
        }
        self.sync_charts(&snap, false, cx);
        self.refresh_lines(&snap, cx);
        self.refresh_edit_lines(&snap);
        if changed {
            cx.notify();
        }
    }

    /// Applies the settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = SidebarSettings::read(cx);
        if new == self.settings {
            return;
        }
        self.settings = new;
        let snap = Arc::clone(self.hub.read(cx).current());
        let changed = self.sync_rows(&snap, cx);
        self.sync_charts(&snap, true, cx);
        self.refresh_lines(&snap, cx);
        if self.edit_mode || self.edit_leaving {
            self.rebuild_edit_cards(&snap, cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// A new theme or UI scale: the charts take the new colours.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_charts(&snap, true, cx);
        let theme = current(cx).clone();
        for card in &mut self.edit_cards {
            let scale = scale_input(&card.row, &snap, &self.settings);
            card.scale = scale;
            let (spec, fmt) = card_spec(&card.row, scale, &self.settings, &theme);
            card.chart.update(cx, |v, cx| {
                v.set_spec(spec, cx);
                v.set_labels(card_chart_labels(fmt), cx);
            });
        }
        for card in &self.cards {
            card.view.update(cx, CardView::invalidate_style);
        }
        cx.notify();
    }

    /// New strings: the cards take the new words.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        let words = card_words(cx);
        if Rc::ptr_eq(&words, &self.words) {
            return;
        }
        self.words = words;
        self.edit_words = edit_words(cx);
        let snap = Arc::clone(self.hub.read(cx).current());
        self.refresh_lines(&snap, cx);
        self.refresh_edit_lines(&snap);
    }

    /// Whether `key`'s page shows when no override is set: its type's setting (PERF-002).
    fn type_default(&self, row: &DeviceRow) -> bool {
        let none = HashMap::new();
        let view = PerformanceSettingsView {
            overrides: &none,
            ..self.settings.view()
        };
        is_visible(row, &view)
    }

    /// Rebuilds the short-lived edit cards from every present row. Edit mode has at most 15
    /// rows, so rebuilding on a device or visibility setting change is cheaper and clearer than
    /// maintaining a second hotplug state machine.
    fn rebuild_edit_cards(&mut self, snap: &Snapshot, cx: &mut App) {
        let rows = self.rows.clone();
        self.edit_cards = rows
            .iter()
            .map(|row| {
                self.new_card(
                    row,
                    type_number(&rows, &row.key),
                    CardAnim::Steady,
                    snap,
                    cx,
                )
            })
            .collect();
        self.refresh_edit_lines(snap);
    }

    fn refresh_edit_lines(&mut self, snap: &Snapshot) {
        for card in &mut self.edit_cards {
            let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == card.row.idx) else {
                continue;
            };
            let input = CardTextInput {
                words: &self.words,
                kind: card.row.kind,
                number: card.number,
                entry,
                snap,
                network_unit: self.settings.network_unit,
                auto_scale: self.settings.network_auto_scale,
                fan_label: self.settings.fan_labels.get(&card.row.key).map(|l| &**l),
                fan_temp: None,
            };
            update_card_lines(&mut card.lines, &input, &mut self.scratch);
        }
    }

    fn enter_edit_mode(&mut self, cx: &mut Context<Self>) {
        if self.edit_mode {
            return;
        }
        self.edit_generation += 1;
        self.edit_mode = true;
        self.edit_leaving = false;
        self.edit_focus_row = self.selected.clone();
        let snap = Arc::clone(self.hub.read(cx).current());
        self.rebuild_edit_cards(&snap, cx);
        cx.notify();
    }

    fn leave_edit_mode(&mut self, cx: &mut Context<Self>) {
        if !self.edit_mode {
            return;
        }
        self.edit_generation += 1;
        self.edit_mode = false;
        self.edit_focus_row = None;
        self.drag = None;
        if !animations_on(cx) {
            self.edit_leaving = false;
            self.edit_cards.clear();
            cx.notify();
            return;
        }
        self.edit_leaving = true;
        let generation = self.edit_generation;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(M_BASE).await;
            let _ = this.update(cx, |this, cx| {
                if this.edit_leaving && this.edit_generation == generation {
                    this.edit_leaving = false;
                    this.edit_cards.clear();
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn on_enter_edit(&mut self, _: &PerfEnterEditMode, _: &mut Window, cx: &mut Context<Self>) {
        self.enter_edit_mode(cx);
    }

    fn on_exit_edit(&mut self, _: &PerfExitEditMode, _: &mut Window, cx: &mut Context<Self>) {
        self.leave_edit_mode(cx);
    }

    fn on_enable_all(&mut self, _: &PerfEnableAllPages, _: &mut Window, cx: &mut Context<Self>) {
        let values: Vec<_> = self
            .rows
            .iter()
            .map(|row| (row.key.0.to_string(), self.type_default(row)))
            .collect();
        TlSettings::update(cx, |d| {
            let overrides = &mut d.settings.performance.device_overrides;
            overrides.retain(|_, value| *value != DeviceVisibility::Hide);
            for (key, default_on) in values {
                if default_on {
                    overrides.remove(&key);
                } else {
                    overrides.insert(key, DeviceVisibility::Show);
                }
            }
        });
    }

    fn on_disable_all(
        &mut self,
        _: &PerfDisableAllPages,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.selected.clone();
        let values: Vec<_> = self
            .rows
            .iter()
            .map(|row| {
                (
                    row.key.clone(),
                    row.key.0.to_string(),
                    self.type_default(row),
                )
            })
            .collect();
        TlSettings::update(cx, |d| {
            let overrides = &mut d.settings.performance.device_overrides;
            for (key, name, default_on) in values {
                if Some(&key) == selected.as_ref() {
                    if default_on {
                        overrides.remove(&name);
                    } else {
                        overrides.insert(name, DeviceVisibility::Show);
                    }
                } else {
                    overrides.insert(name, DeviceVisibility::Hide);
                }
            }
        });
        let text = tr(cx, "perf-toast-last-page");
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| {
                q.push(ToastKind::Info, text, Some(LAST_PAGE_TOAST), None, cx)
            });
        }
    }

    fn on_reset_pages(
        &mut self,
        _: &PerfResetPagesToDefault,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        TlSettings::update(cx, |d| {
            d.settings.performance.device_order.clear();
            d.settings.performance.device_overrides.clear();
        });
    }

    /// Writes a complete order of every present row after moving `key` before `before`, or to
    /// the end of its device group. Cross-group moves are ignored.
    fn move_device(&mut self, key: &DeviceKey, before: Option<&DeviceKey>, cx: &mut Context<Self>) {
        let Some(moved) = self.rows.iter().find(|row| row.key == *key) else {
            return;
        };
        if before == Some(key) {
            return;
        }
        let kind = moved.kind;
        if before.is_some_and(|before| {
            !self
                .rows
                .iter()
                .any(|row| row.key == *before && row.kind == kind)
        }) {
            return;
        }

        let mut order: Vec<DeviceKey> = self.rows.iter().map(|row| row.key.clone()).collect();
        let Some(from) = order.iter().position(|candidate| candidate == key) else {
            return;
        };
        let moved = order.remove(from);
        let insert_at = match before {
            Some(before) => order
                .iter()
                .position(|candidate| candidate == before)
                .expect("validated present key"),
            None => order
                .iter()
                .rposition(|candidate| candidate.kind() == kind)
                .map_or(from, |at| at + 1),
        };
        order.insert(insert_at, moved);
        let saved: Vec<String> = order.iter().map(|key| key.0.to_string()).collect();
        if saved == TlSettings::settings(cx).performance.device_order {
            return;
        }
        TlSettings::update(cx, |d| d.settings.performance.device_order = saved);
    }

    fn on_move_device(&mut self, action: &PerfMoveDevice, _: &mut Window, cx: &mut Context<Self>) {
        self.move_device(&action.key, action.before.as_ref(), cx);
    }

    fn on_move_focused_device(
        &mut self,
        action: &PerfMoveFocusedDevice,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.edit_mode || action.delta == 0 {
            return;
        }
        let Some(key) = self.edit_focus_row.clone() else {
            return;
        };
        let kind = key.kind();
        let group: Vec<DeviceKey> = self
            .rows
            .iter()
            .filter(|row| row.kind == kind)
            .map(|row| row.key.clone())
            .collect();
        let Some(at) = group.iter().position(|candidate| *candidate == key) else {
            return;
        };
        let before = if action.delta < 0 {
            let Some(previous) = at.checked_sub(1) else {
                return;
            };
            Some(group[previous].clone())
        } else {
            if at + 1 >= group.len() {
                return;
            }
            group.get(at + 2).cloned()
        };
        self.move_device(&key, before.as_ref(), cx);
    }

    fn drag_over_row(
        &mut self,
        target: &DeviceKey,
        group_index: usize,
        event: &DragMoveEvent<DraggedCard>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }
        let dragged = event.drag(cx);
        let group = dragged.key.kind();
        let over = (target.kind() == group)
            .then(|| group_index + usize::from(event.event.position.y >= event.bounds.center().y));
        let next = DragState {
            key: dragged.key.clone(),
            group,
            over,
        };
        if self.drag.as_ref() != Some(&next) {
            self.drag = Some(next);
            cx.notify();
        }
    }

    /// The pointer is over a group header or an empty-group hint: no drop target.
    fn drag_over_none(&mut self, event: &DragMoveEvent<DraggedCard>, cx: &mut Context<Self>) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }
        let key = event.drag(cx).key.clone();
        let next = DragState {
            group: key.kind(),
            key,
            over: None,
        };
        if self.drag.as_ref() != Some(&next) {
            self.drag = Some(next);
            cx.notify();
        }
    }

    /// The pointer is inside the section of edit group `group`. In another type's section,
    /// including its gaps and padding, nothing is a drop target. In the dragged card's own
    /// section the rows decide, and a gap between rows keeps the last indicator.
    fn drag_over_group(
        &mut self,
        group: usize,
        event: &DragMoveEvent<DraggedCard>,
        cx: &mut Context<Self>,
    ) {
        if edit_group(event.drag(cx).key.kind()) != group {
            self.drag_over_none(event, cx);
        }
    }

    /// A release outside the card list ends the drag without moving anything.
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }

    /// A release anywhere in the card list drops the card where the indicator shows. With no
    /// indicator it only ends the drag.
    fn drop_dragged_card(&mut self, dragged: &DraggedCard, cx: &mut Context<Self>) {
        let state = self.drag.take();
        let Some(DragState {
            key,
            group,
            over: Some(over),
        }) = state
        else {
            cx.notify();
            return;
        };
        if key != dragged.key || key.kind() != group {
            cx.notify();
            return;
        }
        let group_keys: Vec<DeviceKey> = self
            .rows
            .iter()
            .filter(|row| row.kind == group)
            .map(|row| row.key.clone())
            .collect();
        let before = group_keys.get(over).cloned();
        self.move_device(&key, before.as_ref(), cx);
        cx.notify();
    }

    /// `PerfSetPageVisible` (PERF-002). Hiding the only visible page is refused with an Info
    /// toast in `window`. Hiding writes the `hide` override. Showing writes `show` when the type
    /// is off by default and removes the override when it is on. Hiding the selected page
    /// selects the next card ([`Self::sync_rows`]).
    pub fn set_page_visible(
        &mut self,
        key: &DeviceKey,
        visible: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.rows.iter().find(|r| r.key == *key).cloned() else {
            return;
        };
        if !visible && self.index.contains_key(key) && self.live().count() <= 1 {
            let text = tr(cx, "perf-toast-last-page");
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| {
                    q.push(ToastKind::Info, text, Some(LAST_PAGE_TOAST), None, cx)
                });
            }
            return;
        }
        let value = match (visible, self.type_default(&row)) {
            (false, _) => Some(DeviceVisibility::Hide),
            (true, false) => Some(DeviceVisibility::Show),
            (true, true) => None,
        };
        let name = key.0.to_string();
        TlSettings::update(cx, |d| {
            let overrides = &mut d.settings.performance.device_overrides;
            match value {
                Some(v) => {
                    overrides.insert(name, v);
                }
                None => {
                    overrides.remove(&name);
                }
            }
        });
    }

    /// The card menu of `key` at `at`. Its items run on the sidebar.
    pub fn open_card_menu(
        &self,
        key: &DeviceKey,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let menu = self.card_menu_for(key, self.focus.clone(), window, cx);
        open_context_menu_at(at, window, cx, menu);
    }

    /// The card menu of `key`, its actions dispatched at `action_context`.
    pub fn card_menu_for(
        &self,
        key: &DeviceKey,
        action_context: FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<crate::components::menu::TlMenuPopup> {
        let (key, count) = (key.clone(), self.live().count());
        let hidden = !self.index.contains_key(&key);
        let edit_mode = self.edit_mode;
        TlMenu::build(window, cx, action_context, move |menu, _, cx| {
            let tray = tray_available(cx);
            card_menu(menu, &key, count, hidden, edit_mode, tray, cx)
        })
    }

    fn on_set_page_visible(
        &mut self,
        a: &PerfSetPageVisible,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_page_visible(&a.key, a.visible, window, cx);
    }

    /// Shift+F10 and Menu: the selected card's menu at its left edge plus `sp4`, below it.
    fn on_open_card_menu(
        &mut self,
        _: &PerfOpenCardMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        let Some(bounds) = self
            .index
            .get(&key)
            .and_then(|&i| self.scroll.bounds_for_item(i))
        else {
            return;
        };
        let at = Point::new(bounds.left() + metrics(cx).sp[4], bounds.bottom());
        self.open_card_menu(&key, at, window, cx);
    }

    /// The selected card's index among the visible cards (collapsing ones are skipped).
    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.live().position(|c| c.row.key == *selected)
    }

    /// Selects the visible card at `ix`; collapsing cards do not count.
    fn select_at(&mut self, ix: usize, cx: &mut Context<Self>) {
        let key = self.live().nth(ix).map(|c| c.row.key.clone());
        if let Some(key) = key {
            self.select(key, cx);
        }
    }

    fn on_select(&mut self, a: &PerfSelectDevice, _: &mut Window, cx: &mut Context<Self>) {
        self.select(a.key.clone(), cx);
    }

    fn on_select_visible(
        &mut self,
        a: &PerfSelectVisibleDevice,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_at(usize::from(a.index), cx);
    }

    fn on_prev(&mut self, _: &PerfSelectPrevDevice, _: &mut Window, cx: &mut Context<Self>) {
        match self.selected_index() {
            Some(ix) if ix > 0 => self.select_at(ix - 1, cx),
            Some(_) => {}
            None => self.select_at(0, cx),
        }
    }

    fn on_next(&mut self, _: &PerfSelectNextDevice, _: &mut Window, cx: &mut Context<Self>) {
        let ix = self.selected_index().map_or(0, |ix| ix + 1);
        self.select_at(ix, cx);
    }

    fn on_first(&mut self, _: &PerfSelectFirstDevice, _: &mut Window, cx: &mut Context<Self>) {
        self.select_at(0, cx);
    }

    fn on_last(&mut self, _: &PerfSelectLastDevice, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.live().count().checked_sub(1) {
            self.select_at(ix, cx);
        }
    }
}

impl Focusable for DeviceSidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DeviceSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // The demand part carries the window, so it drops out while the window is hidden.
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            self.send_demand(cx);
        }
        if self.focus_subs.is_none() {
            self.focus_subs = Some([
                cx.on_focus(&self.focus, window, |this, _, cx| {
                    this.sync_card_focus(true, cx)
                }),
                cx.on_blur(&self.focus, window, |this, _, cx| {
                    this.sync_card_focus(false, cx)
                }),
            ]);
        }
        let focused = self.focus.is_focused(window);
        if focused != self.cards_focused {
            self.cards_focused = focused;
            self.sync_card_focus(focused, cx);
        }

        let m = *metrics(cx);
        let theme = current(cx).clone();
        let width = self.card_width.get();
        let normal_cards: Vec<AnyElement> = self
            .cards
            .iter()
            .map(|c| {
                // The height the card measured on its last render; it notifies on a change.
                let card_h = c.height.get();
                let menu_key = c.row.key.clone();
                // Right-click opens the card menu and leaves the selection alone (UI-PRF-014).
                let on_right = cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_card_menu(&menu_key, ev.position, window, cx);
                });
                let card = div()
                    .flex_none()
                    .on_mouse_down(MouseButton::Right, on_right)
                    .child(
                        c.view
                            .clone()
                            .cached(StyleRefinement::default().w(width).h(card_h)),
                    );
                let showing = match c.anim {
                    CardAnim::Steady => return card.into_any_element(),
                    CardAnim::Inserting { .. } => true,
                    CardAnim::Removing { .. } => false,
                };
                // Cards below move with the flex layout as the height changes.
                div()
                    .flex_none()
                    .w_full()
                    .overflow_hidden()
                    .child(card)
                    .with_animation(
                        ("perf-card-anim", c.anim_seq as usize),
                        Animation::new(M_BASE).with_easing(ease_out_cubic),
                        move |el, t| {
                            let t = if showing { t } else { 1. - t };
                            el.h(card_h * t).opacity(t)
                        },
                    )
                    .into_any_element()
            })
            .collect();

        let edit_present = self.edit_mode || self.edit_leaving;
        // The earliest stale deadline of a Valid edit card value.
        let mut edit_stale_at: Option<u64> = None;
        let content: AnyElement = if edit_present {
            let entering = self.edit_mode;
            let animate_edit = animations_on(cx);
            let edit_generation = self.edit_generation as usize;
            let edit_card_w = (width - px(52. * m.ui_scale) - m.sp[4] * 2.).max(px(0.));
            let mut edit_groups: Vec<AnyElement> = Vec::new();
            let specs = [
                None,
                None,
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                None,
            ];
            for (group, empty_index) in specs.into_iter().enumerate() {
                let members: Vec<_> = self
                    .edit_cards
                    .iter()
                    .filter(|card| edit_group(card.row.kind) == group)
                    .collect();
                if group == 7 && members.is_empty() {
                    continue;
                }
                let label = div()
                    .debug_selector(move || format!("perf-edit-group-{group}"))
                    .mt(m.sp[4])
                    .text_size(m.text.xs)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_secondary)
                    .on_drag_move(
                        cx.listener(|this, event: &DragMoveEvent<DraggedCard>, _, cx| {
                            this.drag_over_none(event, cx);
                        }),
                    )
                    .child(self.edit_words.groups[group].to_uppercase());
                // gpui-pre 0.3.7 does not expose letter spacing on div text.
                let mut section = v_flex()
                    .w_full()
                    .px(m.sp[4])
                    .gap(m.sp[4])
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<DraggedCard>, _, cx| {
                            this.drag_over_group(group, event, cx);
                        },
                    ))
                    .child(label);
                if members.is_empty() {
                    if let Some(empty_index) = empty_index {
                        section = section.child(
                            div()
                                .debug_selector(move || format!("perf-edit-empty-{group}"))
                                .pl(m.sp[4])
                                .text_size(m.text.sm)
                                .text_color(theme.text_secondary)
                                .child(self.edit_words.empty[empty_index].clone()),
                        );
                    }
                } else {
                    let member_count = members.len();
                    for (group_index, card) in members.into_iter().enumerate() {
                        let key = card.row.key.clone();
                        let menu_key = key.clone();
                        let checked = is_visible(&card.row, &self.settings.view());
                        let dragging = self
                            .drag
                            .as_ref()
                            .is_some_and(|drag| drag.key == card.row.key);
                        let indicator_before = self.drag.as_ref().is_some_and(|drag| {
                            drag.group == card.row.kind && drag.over == Some(group_index)
                        });
                        let indicator_after = group_index + 1 == member_count
                            && self.drag.as_ref().is_some_and(|drag| {
                                drag.group == card.row.kind && drag.over == Some(member_count)
                            });
                        let on_right = cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_card_menu(&menu_key, ev.position, window, cx);
                        });
                        let action_key = key.clone();
                        let switch = TlSwitch::new(ElementId::Name(
                            format!("perf-edit-switch-{}", key.0).into(),
                        ))
                        .checked(checked)
                        .on_change(move |visible, window, cx| {
                            window.dispatch_action(
                                Box::new(PerfSetPageVisible {
                                    key: action_key.clone(),
                                    visible,
                                }),
                                cx,
                            );
                        });
                        let ghost_key = key.clone();
                        let ghost_chart = card.chart.clone();
                        let ghost_lines = card.lines.clone();
                        let sidebar = cx.weak_entity();
                        let tip = HoverTooltip::state(
                            ElementId::Name(format!("perf-drag-tip-{}", key.0).into()),
                            window,
                            cx,
                        );
                        let handle = div()
                            .id(ElementId::Name(
                                format!("perf-drag-handle-{}", key.0).into(),
                            ))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .w(px(24. * m.ui_scale))
                            .cursor_grab()
                            .on_drag(
                                DraggedCard { key: key.clone() },
                                move |dragged, _, _, cx| {
                                    let state = DragState {
                                        key: dragged.key.clone(),
                                        group: dragged.key.kind(),
                                        over: None,
                                    };
                                    let _ = sidebar.update(cx, |this, cx| {
                                        this.drag = Some(state);
                                        cx.notify();
                                    });
                                    cx.new(|_| DragGhost {
                                        key: ghost_key.clone(),
                                        chart: ghost_chart.clone(),
                                        lines: ghost_lines.clone(),
                                        width: edit_card_w,
                                    })
                                },
                            )
                            .child(
                                Icon::new(IconName::GripVertical)
                                    .size(px(16. * m.ui_scale))
                                    .text_color(theme.text_secondary),
                            );
                        let handle = HoverTooltip::attach(
                            &tip,
                            handle,
                            DELAY_CONTROL,
                            TlTooltip::text("Drag to reorder").build(),
                            window,
                            cx,
                        );
                        let mut device_card = DeviceCard {
                            key,
                            chart: card.chart.clone(),
                            lines: card.lines.clone(),
                            selected: self.selected.as_ref() == Some(&card.row.key),
                            focused: self.focus.is_focused(window),
                            edit_mode: true,
                            hidden_in_edit: !checked && !dragging,
                            pointer: false,
                            width: edit_card_w,
                            prepared: None,
                        };
                        let stale_at = device_card
                            .prepare(window, cx)
                            .stale_at(card.chart.read(cx));
                        edit_stale_at = match (edit_stale_at, stale_at) {
                            (Some(a), Some(b)) => Some(a.min(b)),
                            (a, b) => a.or(b),
                        };
                        let key = device_card.key.clone();
                        let target_key = key.clone();
                        let mut row = div()
                            .debug_selector({
                                let key = key.clone();
                                move || format!("perf-edit-row-{}", key.0)
                            })
                            .relative()
                            .flex()
                            .items_center()
                            .gap(m.sp[4])
                            .w_full()
                            .opacity(edit_row_opacity(dragging, 1.))
                            .on_drag_move(cx.listener(
                                move |this, event: &DragMoveEvent<DraggedCard>, _, cx| {
                                    this.drag_over_row(&target_key, group_index, event, cx);
                                },
                            ))
                            .child(handle)
                            .child(
                                div()
                                    .flex_none()
                                    .on_mouse_down(MouseButton::Right, on_right)
                                    .child(device_card),
                            )
                            .child(div().flex_none().w(px(28. * m.ui_scale)).child(switch));
                        if indicator_before || indicator_after {
                            let line = div()
                                .debug_selector(|| "perf-drop-indicator".into())
                                .absolute()
                                .left(-m.sp[4])
                                .right(-m.sp[4])
                                .h(px(3.))
                                .bg(theme.accent);
                            row = row.child(if indicator_before {
                                line.top(px(-1.5))
                            } else {
                                line.bottom(px(-1.5))
                            });
                        }
                        let row: AnyElement = if animate_edit {
                            row.with_animation(
                                ("perf-edit-row", edit_generation),
                                Animation::new(M_FAST).with_easing(ease_out_cubic),
                                move |row, t| {
                                    let fade = if entering { t } else { 1. - t };
                                    row.opacity(edit_row_opacity(dragging, fade))
                                },
                            )
                            .into_any_element()
                        } else {
                            row.into_any_element()
                        };
                        section = section.child(row);
                    }
                }
                edit_groups.push(section.into_any_element());
            }

            let header = v_flex()
                .debug_selector(|| "perf-edit-header".into())
                .on_drag_move(
                    cx.listener(|this, event: &DragMoveEvent<DraggedCard>, _, cx| {
                        this.drag_over_none(event, cx);
                    }),
                )
                .flex_none()
                .w_full()
                .px(m.sp[4])
                .border_b(UiMetrics::hairline(window))
                .border_color(theme.divider)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(32. * m.ui_scale))
                        .child(
                            div()
                                .text_size(m.text.md)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_primary)
                                .child(self.edit_words.title.clone()),
                        )
                        .child(
                            TlButton::new("perf-edit-done", TlButtonKind::Primary)
                                .size(TlButtonSize::Sm)
                                .label(self.edit_words.done.clone())
                                .action(Box::new(PerfExitEditMode)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(m.sp[2])
                        .h(px(32. * m.ui_scale))
                        .child(
                            TlButton::new("perf-edit-enable-all", TlButtonKind::Ghost)
                                .size(TlButtonSize::Sm)
                                .label(self.edit_words.enable_all.clone())
                                .action(Box::new(PerfEnableAllPages)),
                        )
                        .child(
                            TlButton::new("perf-edit-disable-all", TlButtonKind::Ghost)
                                .size(TlButtonSize::Sm)
                                .label(self.edit_words.disable_all.clone())
                                .action(Box::new(PerfDisableAllPages)),
                        ),
                )
                .child(
                    div().flex().items_center().h(px(32. * m.ui_scale)).child(
                        TlButton::new("perf-edit-reset", TlButtonKind::Ghost)
                            .size(TlButtonSize::Sm)
                            .label(self.edit_words.reset.clone())
                            .action(Box::new(PerfResetPagesToDefault)),
                    ),
                );
            let header: AnyElement = if edit_present && animate_edit {
                header
                    .with_animation(
                        ("perf-edit-header", edit_generation),
                        Animation::new(M_BASE).with_easing(ease_out_cubic),
                        move |header, t| {
                            let t = if entering { t } else { 1. - t };
                            header.h(px(96. * m.ui_scale * t)).opacity(t)
                        },
                    )
                    .into_any_element()
            } else {
                header.h(px(96. * m.ui_scale)).into_any_element()
            };

            v_flex()
                .size_full()
                .child(header)
                .child(
                    v_flex()
                        .id(LIST_ID)
                        .flex_1()
                        .min_h_0()
                        .py(m.sp[4])
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .on_drop(cx.listener(|this, dragged: &DraggedCard, _, cx| {
                            this.drop_dragged_card(dragged, cx);
                        }))
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.end_drag(cx)),
                        )
                        .children(edit_groups),
                )
                .into_any_element()
        } else {
            v_flex()
                .id(LIST_ID)
                .size_full()
                .p(m.sp[4])
                .gap(m.sp[4])
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .children(normal_cards)
                .into_any_element()
        };
        self.edit_stale
            .arm(edit_stale_at, |this: &mut Self| &mut this.edit_stale, cx);

        // The cards have a fixed width: measure the sidebar each paint and redraw when it moved.
        let measured = Rc::clone(&self.card_width);
        let pad = m.sp[4] * 2.0;
        let id = self.id;
        let measure = canvas(
            move |bounds, _, cx| {
                let w = (bounds.size.width - pad).max(px(0.));
                if measured.get() != w {
                    measured.set(w);
                    cx.notify(id);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();

        div()
            .id("perf-sidebar")
            .debug_selector(|| "perf-sidebar".into())
            .key_context(if edit_present {
                "Performance PerformanceEdit"
            } else {
                PERFORMANCE_CONTEXT
            })
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_select))
            .on_action(cx.listener(Self::on_select_visible))
            .on_action(cx.listener(Self::on_prev))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_first))
            .on_action(cx.listener(Self::on_last))
            .on_action(cx.listener(Self::on_set_page_visible))
            .on_action(cx.listener(Self::on_open_card_menu))
            .on_action(cx.listener(Self::on_enter_edit))
            .on_action(cx.listener(Self::on_exit_edit))
            .on_action(cx.listener(Self::on_enable_all))
            .on_action(cx.listener(Self::on_disable_all))
            .on_action(cx.listener(Self::on_reset_pages))
            .on_action(cx.listener(Self::on_move_device))
            .on_action(cx.listener(Self::on_move_focused_device))
            .relative()
            .size_full()
            .bg(theme.window_bg)
            .child(measure)
            .child(div().size_full().child(content))
            .when(!edit_present, |root| {
                root.child(
                    div().absolute().top(m.sp[4]).right(m.sp[4]).child(
                        TlButton::new("perf-edit-open", TlButtonKind::Icon)
                            .size(TlButtonSize::Sm)
                            .icon(TlIcon::Pencil)
                            .tooltip(self.edit_words.tip.clone())
                            .action(Box::new(PerfEnterEditMode)),
                    ),
                )
            })
            .vertical_scrollbar(&self.scroll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::DeviceIdx;

    fn disk(
        idx: u16,
        major: u32,
        minor: u32,
        name: &str,
        parent: Option<DeviceIdx>,
    ) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity: DeviceIdentity::Disk {
                major,
                minor,
                name: Arc::from(name),
                wwn_or_serial: None,
            },
            generation: 0,
            display_name: Arc::from(name),
            present: true,
            parent,
            persistent_key: Arc::from(format!("disk:{name}")),
            net_kind: None,
        }
    }

    #[test]
    fn dragged_row_keeps_its_opacity_through_the_fade() {
        assert_eq!(edit_row_opacity(true, 1.), 0.3);
        assert_eq!(edit_row_opacity(true, 0.5), 0.15);
        assert_eq!(edit_row_opacity(false, 1.), 1.);
        assert_eq!(edit_row_opacity(false, 0.5), 0.5);
    }

    #[test]
    fn disk_partitions_dm_fixture_has_only_whole_disk_rows() {
        // This is the exact block topology of tl-collect's disk-partitions-dm fixture.
        let catalog = DeviceCatalog {
            version: 1,
            entries: Arc::from([
                disk(0, 179, 0, "mmcblk0", None),
                disk(1, 259, 0, "nvme0n1", None),
                disk(2, 8, 0, "sda", None),
                disk(3, 8, 1, "sda1", Some(DeviceIdx(2))),
                disk(4, 8, 2, "sda2", Some(DeviceIdx(2))),
                disk(5, 254, 0, "dm-0", Some(DeviceIdx(2))),
                disk(6, 8, 16, "sdb", None),
                disk(7, 11, 0, "sr0", None),
                disk(8, 253, 0, "vda", None),
            ]),
        };
        let mut rows: Vec<_> = catalog
            .entries
            .iter()
            .map(|entry| DeviceRow {
                key: DeviceKey::from_identity(&entry.identity).expect("disk key"),
                kind: DeviceKind::Disk,
                idx: entry.idx,
                generation: 0,
                net_class: None,
                fan_connected: None,
            })
            .collect();

        retain_whole_disks(&mut rows, &catalog);

        let names: Vec<&str> = rows.iter().map(|row| &*row.key.0).collect();
        assert_eq!(
            names,
            [
                "disk:mmcblk0",
                "disk:nvme0n1",
                "disk:sda",
                "disk:sdb",
                "disk:sr0",
                "disk:vda"
            ]
        );
    }
}
