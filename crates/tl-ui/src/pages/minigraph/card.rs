//! `MinigraphCard`: one 180 × 90 card over the Processes tab for a system source (E26-005,
//! PERF-080, PERF-024, UI-PRF-041, UI-PRF-008, R-085).
//!
//! The chart fills the card; the label (top left) and the value (top right) sit over its top
//! row. The card follows the hub for its value only and notifies only when the value text, the
//! label or the state changes. The chart notifies itself for its series. gpui marks the
//! ancestors of a notifying view dirty, so a new sample re-renders the card through its chart
//! even when the value is equal (crw.34); the card itself adds no render for it.

use gpui_kit::{
    App, AppContext as _, BoxShadow, Context, Entity, FocusHandle, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels, Point, Render,
    SharedString, Stateful, StatefulInteractiveElement as _, StyleRefinement, Styled as _,
    Subscription, WeakEntity, Window, div, point, px,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use std::hash::Hasher as _;
use tl_model::demand::{CpuDemand, Demand};
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceIdentity, DeviceKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::{ComponentMask, Snapshot, SystemSnapshot};

use super::GraphColor;
use super::menu::{CardMenuCtx, card_menu};
use super::sources::{MinigraphEntry, MinigraphId, MinigraphSource};
use crate::chart::view::ChangedRuns;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale, format_value,
};
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::tooltip::TlTooltip;
use crate::hub::{SnapshotChanged, SnapshotHub};
use crate::i18n::tr;
use crate::pages::performance::DeviceKey;
use crate::settings_global::TlSettings;
use crate::text::style::TNUM;
use crate::theme::palette::accent_color;
use crate::theme::{TlTheme, current, metrics};

pub const MINI_CARD_W: f32 = 180.;
pub const MINI_CARD_H: f32 = 90.;

/// Height of the label and value row over the chart, before `ui_scale`.
const TEXT_ROW_H: f32 = 20.;
/// Fill alpha of a named graph colour (R-263).
const NAMED_FILL_ALPHA: f32 = 0.85;
/// Hover border alpha of the accent.
const HOVER_BORDER_ALPHA: f32 = 0.5;
/// The value of a card whose series is absent or has no valid sample yet (WarmingUp).
const WARMING_UP_TEXT: &str = "\u{2026}";
/// Pointer travel in px before a press on a card becomes a drag (UI-PRF-043). GPUI starts its
/// own drag at 2 px, so the ghost and the panel check this themselves.
pub const DRAG_THRESHOLD: f64 = 4.;
/// Opacity of the card that follows the pointer (UI-PRF-043).
const GHOST_OPACITY: f32 = 0.85;

/// The payload of a card drag (UI-PRF-043). The panel takes the drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DraggedMinigraph {
    pub id: MinigraphId,
}

/// `to` is at least `DRAG_THRESHOLD` from `press`.
pub fn past_threshold(press: Point<Pixels>, to: Point<Pixels>) -> bool {
    (to - press).magnitude() >= DRAG_THRESHOLD
}

/// A card's data state. Process cards use the others (E26-012).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardState {
    Live,
    NoMatch,
    Ended,
    Restricted,
}

pub struct MinigraphCard {
    pub id: MinigraphId,
    pub entry: MinigraphEntry,
    chart: Entity<ChartView>,
    label: SharedString,
    value: SharedString,
    /// Hash of the inputs `value` was formatted from. The value re-formats only when it changes.
    value_bits: u64,
    /// CPU cards: the two tooltip lines, "── Total 88.6%" and "── Kernel 4.1%".
    tip: Option<[SharedString; 2]>,
    state: CardState,
    /// The series device, `None` when the catalog has no device for the source.
    device: Option<DeviceIdx>,
    hover: bool,
    /// The pointer is on the CPU card's label row, which blocks the card under it.
    row_hover: bool,
    /// The card is being dragged: it draws only the dashed outline of its slot.
    dragging: bool,
    /// Where the left button went down on the card, for the 4 px drag threshold.
    press: Option<Point<Pixels>>,
    /// GPUI runs a drag of this card. It starts at 2 px, before the 4 px threshold.
    gpui_drag: bool,
    hub: Entity<SnapshotHub>,
    _hub_sub: Subscription,
    /// Where the menu items go when nothing in the window has focus (cards take no focus).
    menu_focus: FocusHandle,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
    /// `cx.notify()` calls made by the card itself, not through its chart.
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: std::cell::Cell<u32>,
}

impl MinigraphCard {
    /// Builds the card and its chart for a system source. `sidebar_gpu_order` resolves a `None`
    /// GPU key to the first GPU in sidebar order (R-085).
    pub fn new(
        entry: MinigraphEntry,
        catalog: &DeviceCatalog,
        sidebar_gpu_order: &[DeviceKey],
        hub: &Entity<SnapshotHub>,
        cx: &mut Context<Self>,
    ) -> Self {
        let device = device_for(&entry.source, catalog, sidebar_gpu_order);
        let theme = current(cx).clone();
        let smooth = cx.has_global::<TlSettings>() && TlSettings::settings(cx).motion.smooth_graphs;
        let spec = {
            let hub = hub.read(cx);
            chart_spec_for(
                &entry.source,
                device.unwrap_or(DeviceIdx(u16::MAX)),
                &hub.current().system,
                entry.color,
                entry.range_s,
                smooth,
                &theme,
            )
        };
        let chart = cx.new(|cx| ChartView::new(spec, chart_labels(), hub, cx));
        let label = label_for(&entry.source, cx);
        let _hub_sub = cx.subscribe(hub, Self::on_snapshot);
        let mut this = Self {
            id: entry.id,
            entry,
            chart,
            label,
            value: SharedString::new_static(WARMING_UP_TEXT),
            value_bits: 0,
            tip: None,
            state: CardState::Live,
            device,
            hover: false,
            row_hover: false,
            dragging: false,
            press: None,
            gpui_drag: false,
            hub: hub.clone(),
            _hub_sub,
            menu_focus: cx.focus_handle(),
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: std::cell::Cell::new(0),
        };
        let snap = std::sync::Arc::clone(this.hub.read(cx).current());
        this.update_value(&snap, true, cx);
        this
    }

    pub fn chart(&self) -> &Entity<ChartView> {
        &self.chart
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn value(&self) -> &SharedString {
        &self.value
    }

    pub fn state(&self) -> CardState {
        self.state
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Where the last left press on the card was, until the drag ends.
    pub fn press(&self) -> Option<Point<Pixels>> {
        self.press
    }

    /// GPUI runs a drag of this card, past 2 px or not past 4 px.
    pub fn gpui_drag(&self) -> bool {
        self.gpui_drag
    }

    pub fn set_dragging(&mut self, dragging: bool, cx: &mut Context<Self>) {
        if self.dragging != dragging {
            self.dragging = dragging;
            cx.notify();
        }
    }

    /// Clears the press and the drag look.
    pub fn end_drag(&mut self, cx: &mut Context<Self>) {
        self.press = None;
        self.gpui_drag = false;
        self.set_dragging(false, cx);
    }

    /// The left press and the drag on `el` (the card, or the CPU label row that blocks it).
    fn draggable(
        &self,
        el: Stateful<gpui_kit::Div>,
        cx: &Context<Self>,
    ) -> Stateful<gpui_kit::Div> {
        let card = cx.weak_entity();
        el.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, ev: &MouseDownEvent, _, _| this.press = Some(ev.position)),
        )
        .on_drag(DraggedMinigraph { id: self.id }, move |_, _, _, cx| {
            if let Some(c) = card.upgrade() {
                c.update(cx, |c, _| c.gpui_drag = true);
            }
            MinigraphDragGhost::new(&card, cx)
        })
    }

    /// The CPU card's tooltip lines; `None` for every other source.
    pub fn tooltip_lines(&self) -> Option<&[SharedString; 2]> {
        self.tip.as_ref()
    }

    pub fn set_range(&mut self, secs: u16, cx: &mut Context<Self>) {
        if self.entry.range_s == secs {
            return;
        }
        self.entry.range_s = secs;
        self.chart.update(cx, |c, cx| {
            let mut spec = c.spec.clone();
            spec.range_ns = range_ns(secs);
            c.set_spec(spec, cx);
        });
    }

    pub fn set_color(&mut self, color: GraphColor, cx: &mut Context<Self>) {
        if self.entry.color == color {
            return;
        }
        self.entry.color = color;
        let theme = current(cx).clone();
        let (fill, line) = resolve_colors(&self.entry.source, color, &theme);
        self.chart.update(cx, |c, cx| {
            let mut spec = c.spec.clone();
            if let Some(first) = spec.series.first_mut() {
                first.fill = fill;
                first.line = line;
            }
            c.set_spec(spec, cx);
        });
    }

    /// Opens the card menu at `at` (UI-PRF-042). The items go where focus is, so focus comes
    /// back there when the menu closes.
    pub fn open_menu(&self, at: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let ctx = CardMenuCtx {
            id: self.id,
            is_process: !self.entry.source.is_system(),
            range_s: self.entry.range_s,
            color: self.entry.color,
            detached: self.entry.detached,
            long_ranges: true,
            colors: false,
            detach: false,
        };
        let context = window
            .focused(cx)
            .unwrap_or_else(|| self.menu_focus.clone());
        let menu = TlMenu::build(window, cx, context, move |menu, window, cx| {
            card_menu(menu, &ctx, window, cx)
        });
        open_context_menu_at(at, window, cx, menu);
    }

    /// The chart's series.
    pub fn series_ids(&self, cx: &App) -> SmallVec<[SeriesId; 2]> {
        self.chart.read(cx).series_ids().collect()
    }

    /// The `Demand` bits this card needs; the panel sums the cards' parts (E26-004).
    pub fn demand_part(&self) -> Demand {
        let mut d = Demand {
            any_surface_visible: true,
            ..Demand::default()
        };
        match self.entry.source {
            MinigraphSource::Cpu => d.cpu = CpuDemand::Total,
            MinigraphSource::Memory => d.memory = true,
            MinigraphSource::Disk => d.disks = true,
            MinigraphSource::Network => d.nets = true,
            MinigraphSource::Gpu(_) | MinigraphSource::Vram(_) | MinigraphSource::GpuTemp(_) => {
                d.gpus = true;
            }
            MinigraphSource::Process { .. }
            | MinigraphSource::Name(_)
            | MinigraphSource::App(_) => {}
        }
        d
    }

    /// The series the value reads: the chart's plus the disk read and write rates.
    fn value_series(&self) -> SmallVec<[SeriesId; 2]> {
        let Some(device) = self.device else {
            return SmallVec::new();
        };
        let id = |metric| SeriesId { device, metric };
        match self.entry.source {
            MinigraphSource::Cpu => smallvec![id(MetricId::CPU_TOTAL), id(MetricId::CPU_KERNEL)],
            MinigraphSource::Memory => smallvec![id(MetricId::MEM_USED)],
            MinigraphSource::Disk => smallvec![id(MetricId::DISK_READ), id(MetricId::DISK_WRITE)],
            MinigraphSource::Network => smallvec![id(MetricId::NET_RX), id(MetricId::NET_TX)],
            MinigraphSource::Gpu(_) => smallvec![id(MetricId::GPU_UTIL)],
            MinigraphSource::Vram(_) => smallvec![id(MetricId::GPU_MEM)],
            MinigraphSource::GpuTemp(_) => smallvec![id(MetricId::GPU_TEMP)],
            MinigraphSource::Process { .. }
            | MinigraphSource::Name(_)
            | MinigraphSource::App(_) => SmallVec::new(),
        }
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let series = ev.mask.contains(ComponentMask::SERIES) && {
            let changed = ChangedRuns::new(&ev.changed_series);
            self.value_series().iter().any(|id| changed.contains(*id))
        };
        let totals = ev.mask.contains(ComponentMask::SYSTEM)
            && matches!(
                self.entry.source,
                MinigraphSource::Memory | MinigraphSource::Vram(_)
            );
        if series || totals {
            let snap = std::sync::Arc::clone(hub.read(cx).current());
            self.update_value(&snap, false, cx);
        }
    }

    /// Re-formats the value when its inputs changed and notifies only then.
    fn update_value(&mut self, snap: &Snapshot, first: bool, cx: &mut Context<Self>) {
        let ids = self.value_series();
        let newest: SmallVec<[Option<f32>; 2]> = ids.iter().map(|id| newest(snap, *id)).collect();
        let unit = if cx.has_global::<TlSettings>() {
            TlSettings::settings(cx).units.network
        } else {
            NetworkUnit::default()
        };
        let total = self.total(&snap.system);
        let mut h = FxHasher::default();
        for v in &newest {
            h.write_u32(v.map_or(u32::MAX, f32::to_bits));
            h.write_u8(u8::from(v.is_some()));
        }
        h.write_u64(total);
        h.write_u8(unit as u8);
        let bits = h.finish();
        if !first && bits == self.value_bits {
            return;
        }
        self.value_bits = bits;
        self.value = self.format(&newest, total, unit);
        if matches!(self.entry.source, MinigraphSource::Cpu) {
            let percent = |v: Option<f32>| {
                v.map_or(SharedString::new_static(WARMING_UP_TEXT), |v| {
                    let mut s = String::new();
                    format_value(ValueFormat::Percent, v, unit, &mut s);
                    s.into()
                })
            };
            let line = |key, v| {
                SharedString::from(format!("\u{2500}\u{2500} {} {}", tr(cx, key), percent(v)))
            };
            self.tip = Some([
                line("minigraph-tip-total", newest.first().copied().flatten()),
                line("minigraph-tip-kernel", newest.get(1).copied().flatten()),
            ]);
        }
        if !first {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// MemTotal for Memory, the GPU's memory total for VRAM, else 0.
    fn total(&self, sys: &SystemSnapshot) -> u64 {
        match self.entry.source {
            MinigraphSource::Memory => sys.memory.total,
            MinigraphSource::Vram(_) => self
                .device
                .and_then(|d| sys.gpus.iter().find(|(g, _)| *g == d))
                .map(|(_, g)| g)
                .filter(|g| g.mem_total.validity.is_valid())
                .map_or(0, |g| g.mem_total.value),
            _ => 0,
        }
    }

    fn format(&self, newest: &[Option<f32>], total: u64, unit: NetworkUnit) -> SharedString {
        let first = newest.first().copied().flatten();
        let sum = || -> Option<f32> { newest.iter().copied().sum() };
        let (fmt, value) = match self.entry.source {
            MinigraphSource::Cpu | MinigraphSource::Gpu(_) => (ValueFormat::Percent, first),
            MinigraphSource::Memory => (ValueFormat::PercentOf { total }, first),
            MinigraphSource::Disk => (ValueFormat::RateIec, sum()),
            MinigraphSource::Network => (ValueFormat::RateNetwork, sum()),
            MinigraphSource::Vram(_) => (ValueFormat::Bytes, first),
            MinigraphSource::GpuTemp(_) => (ValueFormat::Celsius, first),
            MinigraphSource::Process { .. }
            | MinigraphSource::Name(_)
            | MinigraphSource::App(_) => (ValueFormat::Percent, None),
        };
        match value {
            Some(v) if !(matches!(fmt, ValueFormat::PercentOf { .. }) && total == 0) => {
                let mut s = String::new();
                format_value(fmt, v, unit, &mut s);
                s.into()
            }
            _ => SharedString::new_static(WARMING_UP_TEXT),
        }
    }
}

impl Render for MinigraphCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = metrics(cx);
        let t = current(cx);
        let s = m.ui_scale;
        if self.dragging {
            // The home slot while the card follows the pointer: 1 px accent dashed outline.
            return div()
                .id(("MinigraphCard", u64::from(self.id.0)))
                .debug_selector(|| "MinigraphCard-home".into())
                .w(px(MINI_CARD_W * s))
                .h(px(MINI_CARD_H * s))
                .border_1()
                .border_dashed()
                .border_color(t.accent)
                .rounded(m.radius_card);
        }
        let border = if self.hover || self.row_hover {
            t.accent.opacity(HOVER_BORDER_ALPHA)
        } else {
            t.divider
        };
        let mut text_row = div()
            .id("MinigraphCard-text-row")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(TEXT_ROW_H * s))
            .px(m.sp[3])
            .flex()
            .items_center()
            .justify_between()
            .text_size(m.text.sm)
            .text_color(t.text_primary)
            .child(
                // A press on the label text starts no drag (UI-PRF-043).
                div()
                    .id("MinigraphCard-label")
                    .debug_selector(|| "MinigraphCard-label".into())
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.label.clone()),
            )
            .child(
                div()
                    .debug_selector(|| "MinigraphCard-value".into())
                    .font_features(TNUM.clone())
                    .child(self.value.clone()),
            );
        let card = div()
            .id(("MinigraphCard", u64::from(self.id.0)))
            .debug_selector(|| "MinigraphCard".into())
            .relative()
            .w(px(MINI_CARD_W * s))
            .h(px(MINI_CARD_H * s))
            .bg(t.list_bg)
            .border_1()
            .border_color(border)
            .rounded(m.radius_card)
            .overflow_hidden();
        let mut card = self
            .draggable(card, cx)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_menu(ev.position, window, cx);
                }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.hover != *hovered {
                    this.hover = *hovered;
                    cx.notify();
                }
            }))
            .child(
                div().absolute().inset_0().child(
                    self.chart
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                ),
            );
        if let Some(shadow) = card_shadow(t, s) {
            card.style().box_shadow = Some(vec![shadow]);
        }
        // The CPU tooltip belongs to the label row only; below it the chart's hover readout
        // takes over. The row blocks the mouse, so the chart under it shows no readout; it then
        // keeps the card's hover border, right-click and drag itself.
        if let Some(lines) = self.tip.clone() {
            text_row = self
                .draggable(text_row, cx)
                .occlude()
                .tooltip(TlTooltip::lines(lines.to_vec()).build())
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if this.row_hover != *hovered {
                        this.row_hover = *hovered;
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_menu(ev.position, window, cx);
                    }),
                );
        }
        card.child(text_row)
    }
}

/// The card shadow: offset (0, 4), blur 12, the theme's derived shadow colour (black at 0.35 ×
/// `appearance.shadow_intensity_pct` / 100 in dark themes). `None` at intensity 0.
pub fn card_shadow(theme: &TlTheme, ui_scale: f32) -> Option<BoxShadow> {
    (theme.derived.shadow.a > 0.0).then(|| BoxShadow {
        color: theme.derived.shadow,
        offset: point(px(0.), px(4. * ui_scale)),
        blur_radius: px(12. * ui_scale),
        spread_radius: px(0.),
        inset: false,
    })
}

/// The card that follows the pointer during a drag (UI-PRF-043): the label and value on the card
/// background at opacity 0.85 with the card shadow. GPUI shows it from 2 px; it draws nothing
/// until the pointer is `DRAG_THRESHOLD` from the press, then stays until the drag ends.
pub struct MinigraphDragGhost {
    card: WeakEntity<MinigraphCard>,
    label: SharedString,
    value: SharedString,
    press: Option<Point<Pixels>>,
    /// The pointer has been `DRAG_THRESHOLD` from the press once.
    shown: bool,
}

impl MinigraphDragGhost {
    fn new(card: &WeakEntity<MinigraphCard>, cx: &mut App) -> Entity<Self> {
        let (label, value, press) = card.upgrade().map_or_else(Default::default, |c| {
            let c = c.read(cx);
            (c.label.clone(), c.value.clone(), c.press)
        });
        cx.new(|_| Self {
            card: card.clone(),
            label,
            value,
            press,
            shown: false,
        })
    }
}

impl Render for MinigraphDragGhost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.shown {
            let mouse = window.mouse_position();
            let past = self.press.is_none_or(|p| past_threshold(p, mouse))
                || self.card.upgrade().is_some_and(|c| c.read(cx).dragging);
            if !past {
                return div();
            }
            self.shown = true;
        }
        let m = metrics(cx);
        let t = current(cx);
        let s = m.ui_scale;
        let mut ghost = div()
            .debug_selector(|| "MinigraphDragGhost".into())
            .w(px(MINI_CARD_W * s))
            .h(px(MINI_CARD_H * s))
            .opacity(GHOST_OPACITY)
            .bg(t.list_bg)
            .border_1()
            .border_color(t.accent.opacity(HOVER_BORDER_ALPHA))
            .rounded(m.radius_card)
            .child(
                div()
                    .h(px(TEXT_ROW_H * s))
                    .px(m.sp[3])
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(m.text.sm)
                    .text_color(t.text_primary)
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(self.label.clone()),
                    )
                    .child(div().font_features(TNUM.clone()).child(self.value.clone())),
            );
        if let Some(shadow) = card_shadow(t, s) {
            ghost.style().box_shadow = Some(vec![shadow]);
        }
        ghost
    }
}

/// (fill, line) of the card's first series. `Accent` uses the source's palette pair (VRAM and
/// GPU temp use the GPU pair); a named colour is that accent for the line and at alpha 0.85 for
/// the fill (R-263).
pub fn resolve_colors(
    source: &MinigraphSource,
    color: GraphColor,
    theme: &TlTheme,
) -> (Option<Hsla>, Hsla) {
    if color != GraphColor::Accent
        && let Some(named) = accent_color(color.as_str())
    {
        return (Some(named.opacity(NAMED_FILL_ALPHA)), named);
    }
    let g = &theme.graph;
    let (fill, line) = match source {
        MinigraphSource::Cpu => (g.cpu_fill, g.cpu_line),
        MinigraphSource::Memory => (g.mem_fill, g.mem_line),
        MinigraphSource::Disk => (g.disk_fill, g.disk_line),
        MinigraphSource::Network => (g.net_fill, g.net_line),
        _ => (g.gpu_fill, g.gpu_line),
    };
    (Some(fill), line)
}

/// The chart of a system card (UI-PRF-041 series table).
pub fn chart_spec_for(
    source: &MinigraphSource,
    idx: DeviceIdx,
    totals: &SystemSnapshot,
    color: GraphColor,
    range_s: u16,
    smooth: bool,
    theme: &TlTheme,
) -> ChartSpec {
    let (fill, line) = resolve_colors(source, color, theme);
    let id = |metric| SeriesId {
        device: idx,
        metric,
    };
    let main = |metric| ChartSeries {
        id: id(metric),
        fill,
        line,
        ..ChartSeries::default()
    };
    let percent = YScale::Fixed {
        min: 0.0,
        max: 100.0,
    };
    let (series, y): (SmallVec<[ChartSeries; 4]>, YScale) = match source {
        MinigraphSource::Cpu => (
            smallvec![
                main(MetricId::CPU_TOTAL),
                ChartSeries {
                    id: id(MetricId::CPU_KERNEL),
                    line: theme.graph.kernel_line,
                    ..ChartSeries::default()
                },
            ],
            percent,
        ),
        MinigraphSource::Memory => (
            smallvec![main(MetricId::MEM_USED)],
            YScale::FixedTotal(totals.memory.total as f32),
        ),
        MinigraphSource::Disk => (smallvec![main(MetricId::DISK_ACTIVE)], percent),
        // The aggregate has no link speed, so the dynamic rule of the net page applies.
        MinigraphSource::Network => (
            smallvec![
                main(MetricId::NET_RX),
                ChartSeries {
                    id: id(MetricId::NET_TX),
                    line,
                    dashed: true,
                    ..ChartSeries::default()
                },
            ],
            YScale::AutoRate { iec: false },
        ),
        MinigraphSource::Gpu(_) => (smallvec![main(MetricId::GPU_UTIL)], percent),
        MinigraphSource::Vram(_) => {
            let total = totals
                .gpus
                .iter()
                .find(|(g, _)| *g == idx)
                .map_or(0, |(_, g)| g.mem_total.value);
            (
                smallvec![main(MetricId::GPU_MEM)],
                YScale::FixedTotal(total as f32),
            )
        }
        MinigraphSource::GpuTemp(_) => (smallvec![main(MetricId::GPU_TEMP)], YScale::StickyTemp),
        MinigraphSource::Process { .. } | MinigraphSource::Name(_) | MinigraphSource::App(_) => {
            (SmallVec::new(), percent)
        }
    };
    ChartSpec {
        series,
        y,
        range_ns: range_ns(range_s),
        grid: GridStyle {
            lines: true,
            baseline: true,
            outline: true,
            rounded: true,
        },
        smooth,
        // The hover readout on every card from P2 (UI-PRF-041, PERF-023).
        hover: true,
        stack: false,
        background: ChartBg::List,
    }
}

/// The card draws the label and value itself.
fn chart_labels() -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Hidden,
        range_word: SharedString::default(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

fn range_ns(secs: u16) -> u64 {
    u64::from(secs) * 1_000_000_000
}

fn label_for(source: &MinigraphSource, cx: &App) -> SharedString {
    let key = match source {
        MinigraphSource::Cpu => "minigraph-label-cpu",
        MinigraphSource::Memory => "minigraph-label-memory",
        MinigraphSource::Disk => "minigraph-label-disk",
        MinigraphSource::Network => "minigraph-label-network",
        MinigraphSource::Gpu(_) => "minigraph-label-gpu",
        MinigraphSource::Vram(_) => "minigraph-label-vram",
        MinigraphSource::GpuTemp(_) => "minigraph-label-gpu-temp",
        // Process cards are E26-011.
        MinigraphSource::Process { .. } | MinigraphSource::Name(_) | MinigraphSource::App(_) => {
            return SharedString::default();
        }
    };
    tr(cx, key)
}

/// The series device of `source`: the CPU package, memory, the aggregate disks or nets device,
/// or the GPU of the key (`None`: the first GPU in sidebar order, else the first present GPU).
pub fn device_for(
    source: &MinigraphSource,
    catalog: &DeviceCatalog,
    sidebar_gpu_order: &[DeviceKey],
) -> Option<DeviceIdx> {
    let fixed = |identity: DeviceIdentity| catalog.find(&identity).map(|e| e.idx);
    let gpu = |key: &Option<DeviceKey>| {
        let key = key.as_ref().or_else(|| sidebar_gpu_order.first());
        match key {
            Some(key) => catalog
                .entries
                .iter()
                .find(|e| DeviceKey::from_identity(&e.identity).as_ref() == Some(key))
                .map(|e| e.idx),
            None => catalog.present_of(DeviceKind::Gpu).next().map(|e| e.idx),
        }
    };
    match source {
        MinigraphSource::Cpu => fixed(DeviceIdentity::CpuPackage),
        MinigraphSource::Memory => fixed(DeviceIdentity::Memory),
        MinigraphSource::Disk => fixed(DeviceIdentity::Aggregate(AggregateKind::Disks)),
        MinigraphSource::Network => fixed(DeviceIdentity::Aggregate(AggregateKind::Nets)),
        MinigraphSource::Gpu(key) | MinigraphSource::Vram(key) | MinigraphSource::GpuTemp(key) => {
            gpu(key)
        }
        MinigraphSource::Process { .. } | MinigraphSource::Name(_) | MinigraphSource::App(_) => {
            None
        }
    }
}

/// The newest valid sample of `id`.
fn newest(snap: &Snapshot, id: SeriesId) -> Option<f32> {
    snap.series
        .get(id)?
        .samples
        .iter()
        .rev()
        .find(|s| s.is_valid())
        .map(|s| s.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_color_named_overrides() {
        let theme = crate::theme::tests::test_theme(crate::theme::ThemeMode::Dark);
        let coral = accent_color("coral").expect("coral");
        assert_eq!(
            resolve_colors(&MinigraphSource::Memory, GraphColor::Coral, &theme),
            (Some(coral.opacity(0.85)), coral)
        );
        assert_eq!(
            resolve_colors(&MinigraphSource::Memory, GraphColor::Accent, &theme),
            (Some(theme.graph.mem_fill), theme.graph.mem_line)
        );
        assert_eq!(
            resolve_colors(&MinigraphSource::GpuTemp(None), GraphColor::Accent, &theme),
            (Some(theme.graph.gpu_fill), theme.graph.gpu_line)
        );
    }
}
