//! The device sidebar: one card per visible device in sidebar order, a mini chart per card, the
//! roving selection and its keys, scroll into view, and the sidebar's demand part
//! (docs/ui/performance.md UI-PRF-010, UI-PRF-011, UI-PRF-012; PERF-001, PERF-007; ADR-008,
//! ADR-009).
//!
//! The sidebar re-renders only when a card's text, the card list or the selection changes. The
//! charts are cached views that re-render for their own series. Card text is read from the
//! snapshot on `SYSTEM`, `INVENTORY`, `DEVICE_DETAILS` and `DEVICES` changes and kept when its
//! text is unchanged ([`update_card_lines`]). The card text reads no series, so `SERIES` alone
//! formats nothing. The card words are translated once per strings version ([`card_words`]).
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

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::v_flex;
use gpui_kit::{
    Animation, AnimationExt as _, AnyWindowHandle, App, AppContext as _, AsyncApp, Context, Entity,
    EntityId, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, ScrollHandle, StatefulInteractiveElement as _, Styled as _,
    Subscription, WeakEntity, Window, canvas, div, px, white,
};
use rustc_hash::FxHashMap;
use smallvec::{SmallVec, smallvec};
use tl_model::demand::{CpuDemand, Demand};
use tl_model::ids::{MetricId, SeriesId};
use tl_model::settings::{DeviceVisibility, NetworkUnit};
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;

use super::actions::{
    PERFORMANCE_CONTEXT, PerfSelectDevice, PerfSelectFirstDevice, PerfSelectLastDevice,
    PerfSelectNextDevice, PerfSelectPrevDevice, PerfSelectVisibleDevice,
};
use super::card::{
    CARD_CHART_BG, CARD_H, CardLines, CardTextInput, CardWords, DeviceCard, card_chart_grid,
    card_chart_labels, card_words, update_card_lines,
};
use super::{
    DeviceKey, DeviceKind, DeviceRow, PerformanceSettingsView, initial_selection,
    selection_after_removal, sidebar_order, type_number, visible_rows,
};
use crate::chart::{ChartSeries, ChartSpec, ChartView, ValueFormat, YScale};
use crate::components::motion::{M_BASE, ease_out_cubic};
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::Strings;
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

/// Element id of the scrolling card list.
pub const LIST_ID: &str = "perf-sidebar-list";

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
    /// Height 0 to `CARD_H` and opacity 0 to 1.
    Inserting {
        started: Instant,
    },
    /// Opacity 1 to 0 and height `CARD_H` to 0; the card goes when it ends.
    Removing {
        started: Instant,
    },
}

/// One card in the list: a visible device, or one that is collapsing out.
pub struct CardState {
    pub row: DeviceRow,
    pub chart: Entity<ChartView>,
    pub lines: CardLines,
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

/// The settings the sidebar draws from.
#[derive(Clone, Debug, PartialEq)]
struct SidebarSettings {
    order: Vec<DeviceKey>,
    overrides: HashMap<DeviceKey, DeviceVisibility>,
    show_disks: bool,
    show_gpus: bool,
    show_fans: bool,
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
        // P4 (E24-026) gives the aggregate cards their series.
        DeviceKind::AggregateDisk | DeviceKind::AggregateGpu => {
            (SmallVec::new(), percent, ValueFormat::Percent)
        }
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
    pub(crate) hub: Entity<SnapshotHub>,
    pub(crate) demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    /// The Performance tab is the active tab. While false the demand part is withdrawn.
    active: bool,
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
    _subs: SmallVec<[Subscription; 4]>,
    /// Focus in and out of the list redraw the focus ring. Set on the first render.
    focus_subs: Option<[Subscription; 2]>,
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
            hub,
            demand,
            id,
            window: None,
            active: true,
            settings: SidebarSettings::read(cx),
            settings_versions: section_versions(cx),
            style_version: m.style_version,
            catalog_version: snap.devices.version,
            card_width: Rc::new(Cell::new(card_width)),
            scratch: String::new(),
            words: card_words(cx),
            _subs: subs,
            focus_subs: None,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        // The first cards are there at once.
        this.sync_cards(&snap, false, cx);
        this.refresh_lines(&snap);
        this.selected = initial_selection(&this.visible(), Some(&saved_selection(cx)));
        if let Some(ix) = this.selected_index() {
            this.scroll.scroll_to_item(ix);
        }
        this
    }

    /// Selects the card of `key`: scrolls it into view, saves it as `selected_device`, emits
    /// `Selected` and redraws. Nothing happens for a key without a visible card or for the
    /// selected key.
    pub fn select(&mut self, key: DeviceKey, cx: &mut Context<Self>) {
        let Some(&ix) = self.index.get(&key) else {
            return;
        };
        if self.selected.as_ref() == Some(&key) {
            return;
        }
        self.scroll.scroll_to_item(ix);
        self.selected = Some(key.clone());
        if *TlSettings::state(cx).performance.selected_device != *key.0 {
            let saved = key.0.to_string();
            TlSettings::update(cx, |d| d.state.performance.selected_device = saved);
        }
        cx.emit(SidebarEvent::Selected(key));
        cx.notify();
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

    /// This surface's demand part: CPU total and memory always, and each device class that has
    /// a visible card. `cpu_fast` is the CPU page's.
    pub fn demand_part(&self) -> Demand {
        let has = |kind| self.live().any(|c| c.row.kind == kind);
        Demand {
            any_surface_visible: true,
            cpu: CpuDemand::Total,
            memory: true,
            disks: has(DeviceKind::Disk),
            nets: has(DeviceKind::Net),
            gpus: has(DeviceKind::Gpu),
            sensors: has(DeviceKind::Fan) || has(DeviceKind::Battery),
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
        CardState {
            row: row.clone(),
            chart,
            lines: CardLines::default(),
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
        sidebar_order(&snap.devices, &self.settings.view(), &mut self.rows);
        let visible: Vec<DeviceRow> = visible_rows(&self.rows, &self.settings.view())
            .cloned()
            .collect();
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
                    .and_then(|at| selection_after_removal(&before, at))
                    .filter(|k| self.index.contains_key(k))
                    .or_else(|| self.live().next().map(|c| c.row.key.clone()))
            }
            None => initial_selection(&self.visible(), Some(&saved_selection(cx))),
        };
        if let Some(next) = next {
            self.select(next, cx);
        }
        true
    }

    /// Re-formats every card's text. True when any line changed.
    fn refresh_lines(&mut self, snap: &Snapshot) -> bool {
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
            changed |= update_card_lines(&mut card.lines, &input, &mut self.scratch);
        }
        changed
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
        }
        self.sync_charts(&snap, false, cx);
        changed |= self.refresh_lines(&snap);
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
        let mut changed = self.sync_rows(&snap, cx);
        self.sync_charts(&snap, true, cx);
        changed |= self.refresh_lines(&snap);
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
        cx.notify();
    }

    /// New strings: the cards take the new words.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        let words = card_words(cx);
        if Rc::ptr_eq(&words, &self.words) {
            return;
        }
        self.words = words;
        let snap = Arc::clone(self.hub.read(cx).current());
        if self.refresh_lines(&snap) {
            cx.notify();
        }
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
                cx.on_focus(&self.focus, window, |_, _, cx| cx.notify()),
                cx.on_blur(&self.focus, window, |_, _, cx| cx.notify()),
            ]);
        }

        let m = *metrics(cx);
        let theme = current(cx);
        let focused = self.focus.is_focused(window);
        let width = self.card_width.get();
        let card_h = CARD_H * m.ui_scale;
        let cards = self.cards.iter().map(|c| {
            let card = DeviceCard {
                key: c.row.key.clone(),
                chart: c.chart.clone(),
                lines: c.lines.clone(),
                selected: !c.is_removing() && self.selected.as_ref() == Some(&c.row.key),
                focused,
                edit_mode: self.edit_mode,
                hidden_in_edit: false,
                width,
            };
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
                        el.h(px(card_h * t)).opacity(t)
                    },
                )
                .into_any_element()
        });

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
            .key_context(PERFORMANCE_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_select))
            .on_action(cx.listener(Self::on_select_visible))
            .on_action(cx.listener(Self::on_prev))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_first))
            .on_action(cx.listener(Self::on_last))
            .relative()
            .size_full()
            .bg(theme.window_bg)
            .child(measure)
            .child(
                v_flex()
                    .id(LIST_ID)
                    .size_full()
                    .p(m.sp[4])
                    .gap(m.sp[4])
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(cards),
            )
            .vertical_scrollbar(&self.scroll)
    }
}
