//! `MinigraphCard`: one 180 × 90 card over the Processes tab for a system source (E26-005,
//! PERF-080, PERF-024, UI-PRF-041, UI-PRF-008, R-085) or a process, name or app source
//! (E26-011, PERF-085, PERF-087, R-154).
//!
//! The chart fills the card; the label (top left) and the value (top right) sit over its top
//! row. The card follows the hub for its value only and notifies only when the value text, the
//! label or the state changes. The chart notifies itself for its series. gpui marks the
//! ancestors of a notifying view dirty, so a new sample re-renders the card through its chart
//! even when the value is equal (crw.34); the card itself adds no render for it.

use gpui_kit::{
    App, AppContext as _, BorrowAppContext as _, BoxShadow, Context, Entity, EventEmitter,
    FocusHandle, Hsla, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, Point, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, WeakEntity,
    Window, div, point, px,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use std::hash::Hasher as _;
use tl_model::demand::{CpuDemand, Demand};
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceIdentity, DeviceKind};
use tl_model::ids::{ColumnId, GraphTarget};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::{ComponentMask, Snapshot, SystemSnapshot};
use tl_model::validity::Validity;
use unicode_segmentation::UnicodeSegmentation as _;

use super::GraphColor;
use super::menu::{CardMenuCtx, card_menu};
use super::panel::MinigraphPanel;
use super::sources::{
    MinigraphEntry, MinigraphId, MinigraphSource, ProcessLabel, ProcessSourceView,
    resolve_process_source,
};
use crate::chart::view::ChangedRuns;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale, format_value,
};
use crate::components::icons::{TlIcon, icon};
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::tooltip::TlTooltip;
use crate::components::validity::{GlyphContent, glyph_parts};
use crate::hub::{SnapshotChanged, SnapshotHub};
use crate::i18n::tr;
use crate::pages::performance::DeviceKey;
use crate::settings_global::TlSettings;
use crate::text::ellipsis::ELLIPSIS;
use crate::text::metrics::TextMetricsCache;
use crate::text::style::TNUM;
use crate::theme::metrics::UiMetrics;
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

/// A card's data state. Process cards use the others (E26-012, PERF-086, UI-PRF-041).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardState {
    Live,
    /// A name or app card that matches no process: `No match`, and the engine's samples are gaps
    /// until a process matches again.
    NoMatch,
    /// A process card whose process exited, or was saved in another boot. A latch: the chart is
    /// frozen at its last samples, the target leaves demand, and the card never binds again.
    Ended,
    /// A matching process belongs to another user and the column's field is not readable: the
    /// lock glyph, and a gap in the graph.
    Restricted,
}

/// Emitted once when a process card latches `Ended`, so its owner resends demand without the
/// card's graph target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardEnded;

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
    /// Process, name and app cards: the label in parts, for the name cut.
    process_label: Option<ProcessLabel>,
    /// Process cards: the label as drawn, cut to fit, with the label and the width in px bits it
    /// was cut for.
    fitted: Option<(SharedString, u32, SharedString)>,
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
        let snap = std::sync::Arc::clone(hub.read(cx).current());
        let process =
            resolve_process_source(&entry.source, entry.column, entry.color, &snap, &theme, cx);
        let spec = match &process {
            Some(view) => process_chart_spec(view, entry.range_s, smooth),
            None => chart_spec_for(
                &entry.source,
                device.unwrap_or(DeviceIdx(u16::MAX)),
                &snap.system,
                entry.color,
                entry.range_s,
                smooth,
                &theme,
            ),
        };
        let chart = cx.new(|cx| ChartView::new(spec, chart_labels(), hub, cx));
        let (label, process_label) = match &process {
            Some(view) => (view.label.clone(), Some(view.label_parts.clone())),
            None => (label_for(&entry.source, cx), None),
        };
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
            process_label,
            fitted: None,
            _hub_sub,
            menu_focus: cx.focus_handle(),
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: std::cell::Cell::new(0),
        };
        match process {
            Some(view) => {
                this.value = view.value;
                this.state = view.state;
                // A key from another boot names a process that cannot exist any more (R-100).
                // Checked once, at load.
                let other_boot = matches!(
                    &this.entry.source,
                    MinigraphSource::Process { key } if key.boot_id != snap.boot_id
                );
                if other_boot || this.gone(view.state) {
                    this.latch_ended(cx);
                }
            }
            None => this.update_value(&snap, true, cx),
        }
        this
    }

    /// A process card in `state` (from the resolver) has lost its process: the key is missing
    /// from a `Valid` process list.
    fn gone(&self, state: CardState) -> bool {
        state == CardState::NoMatch && matches!(self.entry.source, MinigraphSource::Process { .. })
    }

    /// Ends the card for good: `Ended`, the chart frozen at its last samples. The label keeps
    /// the last known name. The caller emits `CardEnded` and notifies.
    fn latch_ended(&mut self, cx: &mut Context<Self>) {
        self.state = CardState::Ended;
        self.value = tr(cx, "minigraph-ended");
        self.chart.update(cx, |c, cx| c.freeze(cx));
    }

    pub fn chart(&self) -> &Entity<ChartView> {
        &self.chart
    }

    /// The whole label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The label as last drawn: a process, name or app label cut in the middle of its name part
    /// to fit beside the value (UI-PRF-041).
    pub fn shown_label(&self) -> &SharedString {
        match &self.fitted {
            Some((_, _, shown)) => shown,
            None => &self.label,
        }
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
        .on_drag(DraggedMinigraph { id: self.id }, move |d, _, window, cx| {
            if let Some(c) = card.upgrade() {
                c.update(cx, |c, _| c.gpui_drag = true);
            }
            // GPUI starts the drag after the panel's drag-move listener ran for this move, so
            // the move that starts it would show no preview until the next one.
            let (id, pos) = (d.id, window.mouse_position());
            cx.defer(move |cx| {
                if let Some(panel) = MinigraphPanel::global(cx) {
                    panel.update(cx, |p, cx| p.drag_to(id, pos, cx));
                }
            });
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
            source: self.entry.source.to_state_string().into(),
            column: self.entry.column,
            range_s: self.entry.range_s,
            color: self.entry.color,
            detached: self.entry.detached,
            long_ranges: true,
            colors: true,
            detach: true,
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

    /// The `Demand` bits this card needs; the panel sums the cards' parts (E26-004). A process,
    /// name or app card lists its identity and column in `graph_targets` (PERF-085, R-154),
    /// unless the engine does not sample that column.
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
            | MinigraphSource::App(_) => {
                if let Some(target) = self.graph_target() {
                    d.graph_targets = std::sync::Arc::from([target]);
                }
            }
        }
        d
    }

    /// The `graph_targets` entry of a process, name or app card; `None` for a system card, for
    /// a column the engine does not sample and for an ended card.
    pub fn graph_target(&self) -> Option<(GraphTarget, ColumnId)> {
        let column = self.entry.column;
        if !column.engine_sampled() || self.state == CardState::Ended {
            return None;
        }
        Some((self.entry.source.graph_target()?, column.column_id()))
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
        if self.process_label.is_some() {
            if ev
                .mask
                .intersects(ComponentMask::PROCESSES | ComponentMask::PROCESS_SERIES)
            {
                let snap = std::sync::Arc::clone(hub.read(cx).current());
                self.update_process(&snap, cx);
            }
            return;
        }
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

    /// Process, name and app cards: takes the label, value and state from the resolver and
    /// notifies only when one of them changed. The label changes only with the name. A process
    /// card whose key left a `Valid` process list latches `Ended`; an ended card reads nothing
    /// more, so a new process with the same pid never rebinds it.
    fn update_process(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        if self.state == CardState::Ended {
            return;
        }
        let Some(view) = resolve_process_source(
            &self.entry.source,
            self.entry.column,
            self.entry.color,
            snap,
            current(cx),
            cx,
        ) else {
            return;
        };
        if self.gone(view.state) {
            self.latch_ended(cx);
            cx.emit(CardEnded);
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
            return;
        }
        let mut changed = false;
        if self.process_label.as_ref() != Some(&view.label_parts) {
            self.label = view.label;
            self.process_label = Some(view.label_parts);
            changed = true;
        }
        if self.value != view.value || self.state != view.state {
            self.value = view.value;
            self.state = view.state;
            changed = true;
        }
        if changed {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// The label cut in the middle of its name part to fit `max` beside the value, keeping
    /// ` · <col>` (UI-PRF-041). Cut again only when the label or `max` changed.
    fn fit_label(&mut self, max: Pixels, window: &Window, cx: &mut App) -> SharedString {
        let Some(parts) = self.process_label.clone() else {
            return self.label.clone();
        };
        let bits = f32::from(max).to_bits();
        if let Some((label, at, shown)) = &self.fitted
            && *label == self.label
            && *at == bits
        {
            return shown.clone();
        }
        let m = *metrics(cx);
        let (size, version) = (m.text.sm, m.style_version);
        // The label without its name: what stays whatever the cut.
        let rest = parts.with_name("", cx);
        if !cx.has_global::<TextMetricsCache>() {
            cx.set_global(TextMetricsCache::default());
        }
        let label = self.label.clone();
        let cut = cx.update_global::<TextMetricsCache, _>(|cache, cx| {
            cache.clear_if_stale(version);
            let mut width_of = |s: &str| {
                s.chars().fold(px(0.), |sum, ch| {
                    sum + cache.char_advance(ch, size, window, cx)
                })
            };
            if width_of(&label) <= max {
                return None;
            }
            let budget = (max - width_of(&rest)).max(px(0.));
            Some(cut_middle(&parts.name, budget, &mut width_of))
        });
        let shown = match cut {
            Some(name) if name != parts.name => parts.with_name(&name, cx),
            _ => label.clone(),
        };
        self.fitted = Some((label, bits, shown.clone()));
        shown
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

impl MinigraphCard {
    /// The value at the top right: the number in `text_primary`; `No match` and `Ended` in
    /// `text_secondary`; the `TlValidityGlyph` lock when Restricted.
    fn value_element(&self, m: &UiMetrics, t: &TlTheme) -> gpui_kit::Div {
        let el = div()
            .debug_selector(|| "MinigraphCard-value".into())
            .flex_none()
            .whitespace_nowrap();
        match self.state {
            CardState::Live => el.font_features(TNUM.clone()).child(self.value.clone()),
            CardState::NoMatch | CardState::Ended => {
                el.text_color(t.text_secondary).child(self.value.clone())
            }
            CardState::Restricted => {
                match glyph_parts(Validity::Restricted, &self.value, false, t) {
                    (GlyphContent::Lock, color) => el.child(
                        div()
                            .debug_selector(|| "TlValidityGlyph-lock".into())
                            .flex()
                            .items_center()
                            .child(icon(TlIcon::Lock, m.icon.xs, color)),
                    ),
                    (GlyphContent::Text(text), color) => el.text_color(color).child(text),
                }
            }
        }
    }
}

impl EventEmitter<CardEnded> for MinigraphCard {}

impl Render for MinigraphCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let s = m.ui_scale;
        let label = if self.process_label.is_some() && !self.dragging {
            // A Restricted card draws the lock icon in place of the value text.
            let value_w = if self.state == CardState::Restricted {
                m.icon.xs
            } else {
                value_width(&self.value, &m, window, cx)
            };
            // The card's width less its padding, the value and a gap between them.
            let max = px(MINI_CARD_W * s) - m.sp[3] * 2. - value_w - m.sp[2];
            self.fit_label(max, window, cx)
        } else {
            self.label.clone()
        };
        let t = current(cx);
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
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(label),
            )
            .child(self.value_element(&m, t));
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

/// (fill, line) of the card's first series. `Accent` uses the source's palette pair on a system
/// card (VRAM and GPU temp use the GPU pair) and `TlTheme.accent` on a process, name or app
/// card (UI-PRF-041); a named colour is that accent for the line and at alpha 0.85 for the fill
/// (R-263), and `TlTheme.accent` is drawn the same way.
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
    if !source.is_system() {
        return (Some(theme.accent.opacity(NAMED_FILL_ALPHA)), theme.accent);
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
            vertical_at: None,
        },
        smooth,
        // The hover readout on every card from P2 (UI-PRF-041, PERF-023).
        hover: true,
        stack: false,
        background: ChartBg::List,
    }
}

/// The chart of a process, name or app card: its one engine-sampled series on the column's
/// scale (UI-PRF-041).
pub fn process_chart_spec(view: &ProcessSourceView, range_s: u16, smooth: bool) -> ChartSpec {
    ChartSpec {
        series: smallvec![view.series.clone()],
        y: view.y,
        range_ns: range_ns(range_s),
        grid: GridStyle {
            lines: true,
            baseline: true,
            outline: true,
            rounded: true,
            vertical_at: None,
        },
        smooth,
        hover: true,
        stack: false,
        background: ChartBg::List,
    }
}

/// Width of the value text in the label font at `text.sm`.
fn value_width(value: &str, m: &UiMetrics, window: &Window, cx: &mut App) -> Pixels {
    if !cx.has_global::<TextMetricsCache>() {
        cx.set_global(TextMetricsCache::default());
    }
    let (size, version) = (m.text.sm, m.style_version);
    cx.update_global::<TextMetricsCache, _>(|cache, cx| {
        cache.clear_if_stale(version);
        value.chars().fold(px(0.), |sum, ch| {
            sum + cache.char_advance(ch, size, window, cx)
        })
    })
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
        // Process, name and app labels come from `resolve_process_source`.
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

/// `text` cut in the middle to fit `max`: graphemes from the start and the end are kept in
/// turn while they fit beside the ellipsis, so both ends of a process name stay readable.
/// `text` whole when it fits.
pub fn cut_middle(
    text: &str,
    max: Pixels,
    measure: &mut impl FnMut(&str) -> Pixels,
) -> SharedString {
    if measure(text) <= max {
        return SharedString::new(text);
    }
    let graphemes: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    let mut buf = [0u8; 4];
    let mut budget = max - measure(ELLIPSIS.encode_utf8(&mut buf));
    // Graphemes `..head` and `tail..` are kept.
    let (mut head, mut tail) = (0, graphemes.len());
    let mut front = true;
    while head < tail {
        let w = measure(graphemes[if front { head } else { tail - 1 }].1);
        if w > budget {
            break;
        }
        budget -= w;
        if front {
            head += 1;
        } else {
            tail -= 1;
        }
        front = !front;
    }
    let at = |i: usize| graphemes.get(i).map_or(text.len(), |(byte, _)| *byte);
    let (head, tail) = (at(head), at(tail));
    let mut out = String::with_capacity(head + ELLIPSIS.len_utf8() + text.len() - tail);
    out.push_str(&text[..head]);
    out.push(ELLIPSIS);
    out.push_str(&text[tail..]);
    out.into()
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
    fn cut_middle_keeps_both_ends() {
        let mut mono = |s: &str| px(s.chars().count() as f32);
        assert_eq!(cut_middle("firefox", px(7.), &mut mono).as_ref(), "firefox");
        assert_eq!(
            cut_middle("abcdefghij", px(5.), &mut mono).as_ref(),
            "ab\u{2026}ij"
        );
        assert_eq!(
            cut_middle("abcdefghij", px(0.), &mut mono).as_ref(),
            "\u{2026}"
        );
    }

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
