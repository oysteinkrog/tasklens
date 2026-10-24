//! Graph summary mode: a tiled grid of every visible device's main graph that replaces the
//! window content (docs/ui/performance.md UI-PRF-016; PERF-010; ADR-008, ADR-009).
//!
//! The `SummaryView` is built when the mode opens and dropped when it closes. Each tile has its
//! own `ChartView` with the sidebar card's series and scale; the chart paints the tile's
//! heading and value itself, so a sample tick repaints only the charts and never this view.
//! The view renders on open, on a size change and on close.
//!
//! While shown, the view sends its own demand part: CPU at the fast rate, so the CPU tile
//! moves at 10 Hz (PERF-010).

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;

use gpui_kit::{
    Animation, AnimationExt as _, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId,
    EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, Point, Render, SharedString, Size,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, Window, canvas,
    div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::{CpuDemand, Demand};

use super::actions::{
    NoOp, PERFORMANCE_SUMMARY_CONTEXT, PerfCopyDeviceInfo, PerfJumpToType, PerfSetGraphRange,
    PerfShowMiniWindow, PerfToggleSummaryMode,
};
use super::set_graph_range;
use super::sidebar::DeviceSidebar;
use super::{DeviceKey, DeviceKind};
use crate::chart::{ChartBg, ChartLabels, ChartView, LegendStyle, ValueAlign, ValueFormat};
use crate::components::motion::{M_BASE, ease_out_cubic};
use crate::demand::DemandAggregator;
use crate::i18n::tr;
use crate::settings_global::{TlSettings, animations_on};
use crate::theme::{current, metrics};

/// Debug selector of the tile grid and the prefix of each tile's (`perf-summary-tile-<i>`).
pub const GRID_ID: &str = "perf-summary-grid";
/// Debug selector of the accent line on the tile selected on entry.
pub const MARKER_ID: &str = "perf-summary-marker";

/// Minimum tile size, × ui_scale.
const TILE_MIN_W: f32 = 200.;
const TILE_MIN_H: f32 = 120.;
/// From this tile width the heading adds ` · line 1`, × ui_scale.
const LINE1_MIN_W: f32 = 280.;
/// From this tile width the chart shows its legend, × ui_scale.
const LEGEND_MIN_W: f32 = 200.;
/// `sp6` and `sp4` at scale 1.
const SP6: f32 = 16.;
const SP4: f32 = 8.;
/// Height of the accent line under the label row.
const MARKER_H: f32 = 2.;

/// The tile grid for `n` tiles in a `w` × `h` area at scale 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileLayout {
    pub cols: usize,
    pub rows: usize,
    pub tile_w: Pixels,
    pub tile_h: Pixels,
    /// The tiles are taller than the area allows; the grid scrolls vertically.
    pub scrolls: bool,
}

/// `cols = ceil(sqrt(n))`, `rows = ceil(n / cols)`; each tile takes its share of the area less
/// the `sp6` padding and the `sp4` gaps. Columns drop while a tile is narrower than 200 px; a
/// tile shorter than 120 px is 120 px and the grid scrolls.
pub fn tile_grid(n: usize, w: Pixels, h: Pixels) -> TileLayout {
    let n = n.max(1);
    let span = |total: Pixels, count: usize| {
        let count_f = count as f32;
        (f32::from(total) - 2. * SP6 - (count_f - 1.) * SP4) / count_f
    };
    let mut cols = (n as f32).sqrt().ceil() as usize;
    while cols > 1 && span(w, cols) < TILE_MIN_W {
        cols -= 1;
    }
    let rows = n.div_ceil(cols);
    let tile_h = span(h, rows);
    let scrolls = tile_h < TILE_MIN_H;
    TileLayout {
        cols,
        rows,
        tile_w: px(span(w, cols).max(0.)),
        tile_h: px(if scrolls { TILE_MIN_H } else { tile_h }),
        scrolls,
    }
}

/// One tile: a visible device's main graph.
pub struct Tile {
    pub key: DeviceKey,
    pub kind: DeviceKind,
    pub chart: Entity<ChartView>,
    pub text: TileText,
}

/// What a tile's chart labels are made from.
pub struct TileText {
    /// The card heading.
    pub heading: SharedString,
    /// The card's line 1.
    pub line1: SharedString,
    pub value_fmt: ValueFormat,
    pub range_word: SharedString,
    pub legend: SmallVec<[(SharedString, LegendStyle); 2]>,
}

/// Emitted to the `PerformancePage`.
#[derive(Clone, Debug, PartialEq)]
pub enum SummaryEvent {
    /// Leave summary mode, then select `select` when set (a tile click).
    Exit { select: Option<DeviceKey> },
    /// Right-click on the tile of `key`: the page performs the page menu of that device.
    Menu { key: DeviceKey, at: Point<Pixels> },
    /// "Copy" in a tile menu: copy the text of the device the menu was opened for.
    Copy { key: DeviceKey },
    /// "View >" in a tile menu: leave summary mode and select the first visible device of `kind`.
    JumpToType { kind: DeviceKind },
}

/// The summary mode view.
pub struct SummaryView {
    pub tiles: Vec<Tile>,
    pub selected_on_entry: Option<DeviceKey>,
    /// Key context "PerformanceSummary".
    focus: FocusHandle,
    /// The device whose tile menu was opened last; "Copy" in that menu copies its page.
    menu_key: Option<DeviceKey>,
    /// The tile layout for `size`, in window pixels.
    layout: TileLayout,
    /// The area the layout was made for, from the last paint.
    size: Size<Pixels>,
    /// The tile width class the chart labels were made for: (line 1, legend).
    label_class: (bool, bool),
    /// The tiles fade in on entry unless animations are off.
    fade: bool,
    demand: Entity<DemandAggregator>,
    /// The tiles follow its visible cards.
    sidebar: Entity<DeviceSidebar>,
    id: EntityId,
    window: Option<AnyWindowHandle>,
    /// `performance.graph_range_s` the tile charts show, in ns.
    range_ns: u64,
    _subs: [Subscription; 2],
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl EventEmitter<SummaryEvent> for SummaryView {}

fn graph_range_ns(cx: &App) -> u64 {
    u64::from(TlSettings::settings(cx).performance.graph_range_s) * 1_000_000_000
}

/// One tile per visible (not collapsing) sidebar card, in sidebar order. The Overview card
/// gets no tile (D-022).
fn build_tiles(sidebar: &Entity<DeviceSidebar>, cx: &mut App) -> Vec<Tile> {
    let words = Words::new(cx);
    let s = sidebar.read(cx);
    let hub = s.hub.clone();
    let cards: Vec<_> = s
        .cards()
        .iter()
        .filter(|c| !c.is_removing() && c.row.kind != DeviceKind::Overview)
        .map(|c| {
            (
                c.row.key.clone(),
                c.row.kind,
                c.lines.clone(),
                c.chart.clone(),
            )
        })
        .collect();
    cards
        .into_iter()
        .map(|(key, kind, lines, card_chart)| {
            let card = card_chart.read(cx);
            let mut spec = card.spec.clone();
            spec.background = ChartBg::Window;
            let value_fmt = card.labels.value_fmt;
            let range_word = words.range_word(kind, value_fmt);
            let legend = words.legend(kind, spec.series.len());
            let text = TileText {
                heading: lines.heading,
                line1: lines.line1,
                value_fmt,
                range_word,
                legend,
            };
            let labels = tile_labels(&text, (false, false));
            let chart = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));
            Tile {
                key,
                kind,
                chart,
                text,
            }
        })
        .collect()
}

impl SummaryView {
    /// One tile per visible sidebar card, in sidebar order, laid out for `size` (the window
    /// content area).
    pub fn new(
        sidebar: &Entity<DeviceSidebar>,
        demand: Entity<DemandAggregator>,
        size: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected_on_entry = sidebar.read(cx).selected().cloned();
        let tiles = build_tiles(sidebar, cx);
        // A page hidden while the mode is on leaves the grid; a new range reaches the tiles.
        let subs = [
            cx.observe(sidebar, |this, _, cx| this.sync_tiles(cx)),
            cx.observe_global::<TlSettings>(Self::sync_range),
        ];

        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            tiles,
            selected_on_entry,
            focus: cx.focus_handle(),
            menu_key: None,
            layout: tile_grid(0, px(0.), px(0.)),
            size: Size::default(),
            label_class: (false, false),
            fade: animations_on(cx),
            demand,
            sidebar: sidebar.clone(),
            id,
            window: None,
            range_ns: graph_range_ns(cx),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        this.resize(size, cx);
        this
    }

    pub fn layout(&self) -> TileLayout {
        self.layout
    }

    /// The tiles fade in (animations on at entry).
    pub fn fades(&self) -> bool {
        self.fade
    }

    /// This view's demand part: CPU at the fast rate (PERF-010).
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            cpu: CpuDemand::Total,
            cpu_fast: true,
            ..Demand::default()
        }
    }

    /// A new content size: the layout, and the chart labels when a tile crosses a width step.
    fn resize(&mut self, size: Size<Pixels>, cx: &mut Context<Self>) {
        if size == self.size {
            return;
        }
        self.size = size;
        let s = metrics(cx).ui_scale;
        let unit = tile_grid(self.tiles.len(), size.width / s, size.height / s);
        self.layout = TileLayout {
            tile_w: unit.tile_w * s,
            tile_h: unit.tile_h * s,
            ..unit
        };
        let w = f32::from(unit.tile_w);
        let class = (w >= LINE1_MIN_W, w >= LEGEND_MIN_W);
        if class != self.label_class {
            self.label_class = class;
            for tile in &self.tiles {
                let labels = tile_labels(&tile.text, class);
                tile.chart.update(cx, |v, cx| v.set_labels(labels, cx));
            }
        }
        cx.notify();
    }

    fn on_toggle(&mut self, _: &PerfToggleSummaryMode, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SummaryEvent::Exit { select: None });
    }

    /// Rebuilds the tiles when the sidebar's visible cards changed.
    fn sync_tiles(&mut self, cx: &mut Context<Self>) {
        let same = {
            let s = self.sidebar.read(cx);
            s.visible_keys()
                .filter(|k| k.kind() != DeviceKind::Overview)
                .eq(self.tiles.iter().map(|t| &t.key))
        };
        if same {
            return;
        }
        self.tiles = build_tiles(&self.sidebar, cx);
        let size = std::mem::take(&mut self.size);
        self.label_class = (false, false);
        self.resize(size, cx);
    }

    /// A new graph range: each tile chart takes it and builds its geometry once.
    fn sync_range(&mut self, cx: &mut Context<Self>) {
        let range_ns = graph_range_ns(cx);
        if range_ns == self.range_ns {
            return;
        }
        self.range_ns = range_ns;
        for tile in &self.tiles {
            tile.chart.update(cx, |v, cx| {
                let mut spec = v.spec.clone();
                spec.range_ns = range_ns;
                v.set_spec(spec, cx);
            });
        }
    }

    /// "Copy" in a tile menu: the page copies the device the menu was opened for.
    fn on_copy_device_info(
        &mut self,
        _: &PerfCopyDeviceInfo,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = self.menu_key.clone() {
            cx.emit(SummaryEvent::Copy { key });
        }
    }

    /// "View >" in a tile menu.
    fn on_jump_to_type(&mut self, a: &PerfJumpToType, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SummaryEvent::JumpToType { kind: a.kind });
    }

    /// The tile menu's Time range items run here: no `PerformancePage` is drawn above this view.
    fn on_set_graph_range(
        &mut self,
        a: &PerfSetGraphRange,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        set_graph_range(a.secs, cx);
    }
}

/// The chart labels of `tile`: heading (plus ` · line 1` when wide), value at the right, legend
/// when wide enough. `class` is (line 1, legend).
fn tile_labels(tile: &TileText, class: (bool, bool)) -> ChartLabels {
    let heading = if class.0 && !tile.line1.is_empty() {
        SharedString::from(format!("{} · {}", tile.heading, tile.line1))
    } else {
        tile.heading.clone()
    };
    ChartLabels {
        heading: Some(heading),
        value_fmt: tile.value_fmt,
        value_align: ValueAlign::Right,
        range_word: tile.range_word.clone(),
        legend: if class.1 {
            tile.legend.clone()
        } else {
            SmallVec::new()
        },
        show_axis_max: false,
    }
}

/// The range words and legends, translated once per summary.
struct Words {
    utilization: SharedString,
    memory: SharedString,
    active: SharedString,
    throughput: SharedString,
    fan: SharedString,
    power: SharedString,
    percentage: SharedString,
    total: SharedString,
    kernel: SharedString,
    used: SharedString,
    committed: SharedString,
    download: SharedString,
    upload: SharedString,
}

impl Words {
    fn new(cx: &App) -> Self {
        let t = |key| tr(cx, key);
        Self {
            utilization: t("perf-cpu-heading-utilization"),
            memory: t("perf-mem-range-memory"),
            active: t("perf-disk-heading-active"),
            throughput: t("perf-disk-range-throughput"),
            fan: t("perf-gpu-row-fan"),
            power: t("perf-gpu-row-power"),
            percentage: t("perf-summary-range-percentage"),
            total: t("perf-legend-total"),
            kernel: t("perf-legend-kernel"),
            used: t("perf-mem-legend-used"),
            committed: t("perf-mem-legend-committed"),
            download: t("perf-net-legend-download"),
            upload: t("perf-net-legend-upload"),
        }
    }

    fn range_word(&self, kind: DeviceKind, fmt: ValueFormat) -> SharedString {
        match kind {
            DeviceKind::Overview | DeviceKind::Cpu | DeviceKind::Gpu | DeviceKind::AggregateGpu => {
                &self.utilization
            }
            DeviceKind::Memory => &self.memory,
            DeviceKind::Disk | DeviceKind::AggregateDisk => &self.active,
            DeviceKind::Net => &self.throughput,
            DeviceKind::Fan => &self.fan,
            DeviceKind::Battery if fmt == ValueFormat::Watts => &self.power,
            DeviceKind::Battery => &self.percentage,
        }
        .clone()
    }

    /// CPU Total and Kernel (when the kernel line is on), Memory Used and Committed, Network
    /// download and upload; the one-series kinds have none.
    fn legend(
        &self,
        kind: DeviceKind,
        series: usize,
    ) -> SmallVec<[(SharedString, LegendStyle); 2]> {
        match kind {
            DeviceKind::Cpu if series > 1 => smallvec![
                (self.total.clone(), LegendStyle::Solid),
                (self.kernel.clone(), LegendStyle::Solid)
            ],
            DeviceKind::Memory => smallvec![
                (self.used.clone(), LegendStyle::Solid),
                (self.committed.clone(), LegendStyle::Dashed)
            ],
            DeviceKind::Net => smallvec![
                (self.download.clone(), LegendStyle::Solid),
                (self.upload.clone(), LegendStyle::Dashed)
            ],
            _ => SmallVec::new(),
        }
    }
}

impl Focusable for SummaryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SummaryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // The demand part carries the window, so it drops out while the window is hidden.
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            let (id, part) = (self.id, self.demand_part());
            self.demand
                .update(cx, |d, cx| d.set(id, Some(handle), part, cx));
        }
        let m = *metrics(cx);
        let theme = current(cx);
        let s = m.ui_scale;
        let l = self.layout;
        let marker_top = m.sp[3] + px(crate::chart::labels::LABEL_ROW * s);
        let selected = self.selected_on_entry.clone();

        let tiles = self.tiles.iter().enumerate().map(|(i, tile)| {
            let key = tile.key.clone();
            let marker = (selected.as_ref() == Some(&tile.key)).then(|| {
                div()
                    .debug_selector(|| MARKER_ID.into())
                    .absolute()
                    .top(marker_top)
                    .left(m.sp[3])
                    .right(m.sp[3])
                    .h(px(MARKER_H * s))
                    .bg(theme.accent)
            });
            div()
                .id(("perf-summary-tile", i))
                .debug_selector(move || format!("perf-summary-tile-{i}"))
                .relative()
                .flex_none()
                .w(l.tile_w)
                .h(l.tile_h)
                .bg(theme.window_bg)
                .child(
                    tile.chart
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                )
                .children(marker)
                // The page menu of the tile's device (UI-PRF-016), opened by the page.
                .on_mouse_down(MouseButton::Right, {
                    let key = tile.key.clone();
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.menu_key = Some(key.clone());
                        cx.emit(SummaryEvent::Menu {
                            key: key.clone(),
                            at: ev.position,
                        });
                    })
                })
                .on_click(
                    cx.listener(move |_, ev: &gpui_kit::ClickEvent, window, cx| {
                        if ev.click_count() == 2 {
                            window.dispatch_action(Box::new(PerfShowMiniWindow), cx);
                        } else {
                            cx.emit(SummaryEvent::Exit {
                                select: Some(key.clone()),
                            });
                        }
                    }),
                )
        });

        // The grid measures its box each paint and lays the tiles out again when it moved.
        let this = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let _ = this.update(cx, |v, cx| v.resize(bounds.size, cx));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();

        let grid = div()
            .id(GRID_ID)
            .debug_selector(|| GRID_ID.into())
            .size_full()
            .p(m.sp[6])
            .flex()
            .flex_row()
            .flex_wrap()
            .content_start()
            .gap(m.sp[4])
            .when_scrolls(l.scrolls)
            .children(tiles);
        let grid = if self.fade {
            grid.with_animation(
                ("perf-summary-fade", self.id.as_u64() as usize),
                Animation::new(M_BASE).with_easing(ease_out_cubic),
                |el, t| el.opacity(t),
            )
            .into_any_element()
        } else {
            grid.into_any_element()
        };

        div()
            .key_context(PERFORMANCE_SUMMARY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_toggle))
            .on_action(cx.listener(Self::on_set_graph_range))
            .on_action(cx.listener(Self::on_copy_device_info))
            .on_action(cx.listener(Self::on_jump_to_type))
            // A binding whose action has no handler falls through to the next binding for the
            // same keys, so the swallowed tab keys need this empty handler.
            .on_action(|_: &NoOp, _, _| {})
            .relative()
            .size_full()
            .bg(theme.window_bg)
            .child(measure)
            .child(grid)
    }
}

/// `overflow_y_scroll` only when the tiles do not fit.
trait WhenScrolls: Sized {
    fn when_scrolls(self, scrolls: bool) -> Self;
}

impl WhenScrolls for gpui_kit::Stateful<gpui_kit::Div> {
    fn when_scrolls(self, scrolls: bool) -> Self {
        if scrolls {
            self.overflow_y_scroll()
        } else {
            self
        }
    }
}
