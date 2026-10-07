//! `ChartGridView` and `ChartGridElement`: one element paints n chart cells, such as the CPU
//! per-core grid and the GPU engine grid (ADR-009 rule 7, UI-PRF-002, PERF-028, PERF-063).
//!
//! Every cell's geometry goes into one fill path, one total-line path and one overlay-line path,
//! so a frame clones 3 paths (plus up to 3 stale sets) however many cells there are. Cells share
//! one time map, so one translate serves them all, and their paths are clipped to the cell at
//! build time, so one content mask covers the element. Labels are cached shaped lines and
//! re-shape only when their text changes.
//!
//! Dotted gaps (PERF-022): the build keeps each cell's gap spans, and paint draws them as quads
//! along the cell's plot bottom, as a single chart does, so they add no path paint.
//!
//! Heading tooltips: while the pointer is inside the element, prepaint inserts one hitbox per
//! heading; idle frames insert none (UI-PRF-002). The kit has no tooltip paint call for custom
//! elements, so the view records the hovered cell (`hovered_cell`, `hovered_tooltip`) and
//! notifies when it changes. The owning page shows the cell's tooltip text from there.
//!
//! Owner state: `set_state` with Failed, Restricted or Unsupported draws every cell like a single
//! chart in that state (UI-PRF-001): no paths, a dotted baseline across the plot, and `!` or
//! `n/a` as the value (Restricted shows `!` too, as the grid has no lock icon). The GPU page uses it while the probe restarts (PERF-062).
//!
//! Hover readout (UI-PRF-023): with `spec.hover`, prepaint also inserts one hitbox per cell plot
//! while the pointer is inside, offline cells excepted, so at most 2n hitboxes exist. The hovered
//! cell gets the `hover.rs` marker, dots and value box, with its fill series as series 0 and its
//! overlay as series 1. One `HoverState` serves the grid, since only one cell is hovered at a
//! time; a cell change resets its `last_index`, so the box lines are formatted again.

use std::cell::RefCell;
use std::mem;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{
    App, AvailableSpace, BorderStyle, Bounds, ContentMask, Context, Edges, Element, ElementId,
    Entity, FillOptions, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, Length, MouseDownEvent, MouseMoveEvent, Path,
    PathBuilder, PathStyle, Pixels, Point, Render, SharedString, Size, Style, Subscription,
    TransformationMatrix, Window, point, px, quad, relative, size, transparent_black,
};
use smallvec::SmallVec;
use tl_model::ids::SeriesId;
use tl_model::series::{SampleFlags, SeriesView};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use super::geometry::{self, BuildInput, TimeMap};
use super::grid::{self, GridPaint};
use super::hover::{self, HoverPaint, HoverState};
use super::labels::{self, CachedLine, LabelCache, LineStyle};
use super::view::{ChangedRuns, find_tiered};
use super::{
    ChartCache, ChartLabels, ChartSeries, ChartSpec, ChartState, DEFAULT_RANGE_NS, GeometryScalars,
    GridStyle, LegendStyle, PathSet, ValueAlign, ValueFormat, YScale, scale,
};
use crate::anim::AnimationDriver;
use crate::components::icons::AppIcon;
use crate::hub::{SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::TlSettings;
use crate::theme::{TlTheme, UiMetrics};

/// Period used when no cell has data: 1 s.
const DEFAULT_PERIOD_NS: u64 = 1_000_000_000;

/// Alpha factor of the stale sets.
const STALE_ALPHA: f32 = 0.5;

/// Fluent key of an offline cell's text (R-132).
const OFFLINE_KEY: &str = "chart-grid-offline";

/// Every cell draws grid lines, baseline and outline.
const CELL_GRID: GridStyle = GridStyle {
    lines: true,
    baseline: true,
    outline: true,
    rounded: false,
};

/// A cell's resolved (fill, overlay) series.
pub type CellSeries = (Option<Arc<SeriesView>>, Option<Arc<SeriesView>>);

/// One cell of the grid.
/// A core mark: a kit icon (the preferred-core star) or one of tasklens's own (the P-core and
/// E-core chips, as the affinity dialog shows them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellMark {
    Kit(IconName),
    App(AppIcon),
}

impl CellMark {
    pub fn path(self) -> SharedString {
        match self {
            Self::Kit(icon) => icon.path(),
            Self::App(icon) => SharedString::new_static(icon.path_str()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GridCellSpec {
    pub heading: SharedString,
    pub fill_series: SeriesId,
    pub overlay_series: Option<SeriesId>,
    /// No plot; the `chart-grid-offline` text in the middle and a `divider` outline. Owners set it for a cell whose
    /// series is Missing (CPU offline).
    pub offline: bool,
    /// The core mark after the heading (PERF-031).
    pub mark: Option<CellMark>,
    /// P2.
    pub tooltip: Option<SharedString>,
}

/// What a grid draws and how. Every cell shares the colours, scale and range.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartGridSpec {
    pub cells: Vec<GridCellSpec>,
    pub fill: Hsla,
    pub line: Hsla,
    pub overlay_line: Hsla,
    pub y: YScale,
    pub range_ns: u64,
    pub smooth: bool,
    /// 120.
    pub cell_min_w: Pixels,
    /// 112.
    pub cell_h: Pixels,
    /// `sp4`, 8.
    pub gap: Pixels,
    /// Drawn in cell 0 only.
    pub legend: SmallVec<[(SharedString, LegendStyle); 2]>,
    pub value_fmt: ValueFormat,
    /// P2: plot hitboxes and the hover readout per cell (UI-PRF-023).
    pub hover: bool,
}

impl Default for ChartGridSpec {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            fill: Hsla::default(),
            line: Hsla::default(),
            overlay_line: Hsla::default(),
            y: YScale::Fixed {
                min: 0.0,
                max: 100.0,
            },
            range_ns: DEFAULT_RANGE_NS,
            smooth: false,
            cell_min_w: px(120.),
            cell_h: px(112.),
            gap: px(8.),
            legend: SmallVec::new(),
            value_fmt: ValueFormat::Percent,
            hover: false,
        }
    }
}

/// Columns, rows and cell width of a grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLayout {
    pub cols: usize,
    pub rows: usize,
    pub cell_w: Pixels,
}

/// UI-PRF-002: `cols = min(ceil(sqrt(2n)), floor((W + gap) / (cell_min_w + gap)))`, at least 1;
/// `rows = ceil(n / cols)`; `cell_w = (W − (cols − 1) × gap) / cols`.
pub fn grid_layout(n: usize, width: Pixels, cell_min_w: Pixels, gap: Pixels) -> GridLayout {
    let mut want = 0usize;
    while want * want < 2 * n {
        want += 1;
    }
    let step = f32::from(cell_min_w + gap);
    let fit = if step > 0.0 {
        (f32::from(width + gap) / step).floor().max(0.0) as usize
    } else {
        usize::MAX
    };
    let cols = want.min(fit).max(1);
    let gaps = gap * (cols - 1) as f32;
    GridLayout {
        cols,
        rows: n.div_ceil(cols),
        cell_w: ((width - gaps) / cols as f32).max(px(0.)),
    }
}

/// `rows × cell_h + (rows − 1) × gap`; 0 for no rows.
pub fn grid_height(layout: &GridLayout, cell_h: Pixels, gap: Pixels) -> Pixels {
    match layout.rows {
        0 => px(0.),
        rows => cell_h * rows as f32 + gap * (rows - 1) as f32,
    }
}

/// Cell `i`'s top-left corner relative to the grid, logical px.
fn cell_offset(layout: &GridLayout, i: usize, cell_h: Pixels, gap: Pixels) -> (f32, f32) {
    let (col, row) = (i % layout.cols, i / layout.cols);
    (
        col as f32 * f32::from(layout.cell_w + gap),
        row as f32 * f32::from(cell_h + gap),
    )
}

pub struct ChartGridView {
    pub spec: ChartGridSpec,
    /// Owner-set validity for every cell, as `ChartView::state`. Failed, Restricted and
    /// Unsupported win over the data; the rest is derived per cell.
    pub state: ChartState,
    hub: Entity<SnapshotHub>,
    _hub_sub: Subscription,
    /// All cells in one cache: `fills[0]`, `strokes[0]` (total), `strokes[1]` (overlay).
    cache: Rc<RefCell<ChartCache>>,
    scratch: geometry::Scratch,
    /// Right edge the cached geometry was built for (see `ChartView`).
    built_right_edge_ns: u64,
    /// Layout and scale factor the cached geometry was built for.
    built_layout: Option<(GridLayout, f32)>,
    /// Resolved (fill, overlay) per cell.
    series: Vec<CellSeries>,
    /// True when a cell series read its fast view in place of the 1 Hz view the range wants
    /// (see `ChartView::tier_fallback`).
    pub tier_fallback: bool,
    headings: Vec<CachedLine>,
    values: Vec<CachedLine>,
    offline: CachedLine,
    /// The offline text and the strings version it was read for (`None` without strings).
    offline_text: Option<(Option<u64>, SharedString)>,
    /// Dotted-gap spans of the cached geometry, (cell, chart-local physical px span), in cell
    /// order.
    gaps: Vec<(usize, Range<f32>)>,
    legend: LabelCache,
    scale_state: scale::ScaleState,
    axis_text: String,
    /// Reused value formatter buffer.
    value_text: String,
    /// Hitboxes of the last paint, none while the pointer is outside. The buffers move into the
    /// prepaint state and back, so hovering allocates nothing after the first time.
    hitboxes: GridHitboxes,
    /// The cell whose heading the pointer is over.
    hovered_cell: Option<usize>,
    /// Hover readout state of the hovered plot cell.
    hover: HoverState,
    /// The cell whose plot the pointer was over at the last paint.
    hover_cell: Option<usize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
    /// Hitboxes the last prepaint inserted.
    #[cfg(any(test, feature = "test-hooks"))]
    pub hitbox_count: std::cell::Cell<u32>,
    /// Geometry rebuilds.
    #[cfg(any(test, feature = "test-hooks"))]
    pub build_count: std::cell::Cell<u32>,
    /// `paint_path` calls, over every paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub path_paints: std::cell::Cell<u32>,
    /// Shapes of heading, value and "Offline" lines, over every paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub heading_shapes: std::cell::Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub value_shapes: std::cell::Cell<u32>,
    /// Core marks the last paint drew, (cell, icon, left edge).
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_marks: RefCell<Vec<(usize, CellMark, Pixels)>>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub offline_shapes: std::cell::Cell<u32>,
    /// Reads of the offline text from the strings.
    #[cfg(any(test, feature = "test-hooks"))]
    pub offline_reads: std::cell::Cell<u32>,
    /// Dotted-gap spans the last paint drew, (cell, chart-local physical px before the paint
    /// shift), as `ChartView::last_gap_spans`.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_gap_spans: RefCell<Vec<(usize, Range<f32>)>>,
}

impl ChartGridView {
    /// Subscribes to `hub` and resolves the spec against its current snapshot.
    pub fn new(spec: ChartGridSpec, hub: &Entity<SnapshotHub>, cx: &mut Context<Self>) -> Self {
        let _hub_sub = cx.subscribe(hub, Self::on_snapshot);
        let mut this = Self {
            spec,
            state: ChartState::Live,
            hub: hub.clone(),
            _hub_sub,
            cache: Rc::default(),
            scratch: geometry::Scratch::default(),
            built_right_edge_ns: 0,
            built_layout: None,
            series: Vec::new(),
            tier_fallback: false,
            headings: Vec::new(),
            values: Vec::new(),
            offline: CachedLine::default(),
            offline_text: None,
            gaps: Vec::new(),
            legend: LabelCache::default(),
            scale_state: scale::ScaleState::default(),
            axis_text: String::new(),
            value_text: String::new(),
            hitboxes: GridHitboxes::default(),
            hovered_cell: None,
            hover: HoverState::default(),
            hover_cell: None,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            hitbox_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            build_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            path_paints: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            heading_shapes: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            value_shapes: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            last_marks: RefCell::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            offline_shapes: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            offline_reads: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            last_gap_spans: RefCell::default(),
        };
        this.resolve(hub.read(cx).current());
        this
    }

    /// Notifies only when a cell's series changed, or when a device change may have brought an
    /// absent series in (as `ChartView`).
    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let range_ns = self.spec.range_ns;
        let runs = ChangedRuns::new(&ev.changed_series);
        let changed = |id: SeriesId| runs.tier_changed(id, range_ns);
        let own_changed = ev.mask.contains(ComponentMask::SERIES)
            && self
                .spec
                .cells
                .iter()
                .any(|c| changed(c.fill_series) || c.overlay_series.is_some_and(changed));
        let device_appeared =
            ev.mask.contains(ComponentMask::DEVICES)
                && (self.series.len() != self.spec.cells.len()
                    || self.series.iter().zip(&self.spec.cells).any(|((f, o), c)| {
                        f.is_none() || (c.overlay_series.is_some() && o.is_none())
                    }));
        if own_changed || device_appeared {
            self.resolve(hub.read(cx).current());
            cx.notify();
        }
    }

    /// Looks up every cell's series in `snap` by binary search. Ranges over 60 s read the 1 Hz
    /// tier, as `ChartView::resolve`. Reuses the list's capacity.
    pub fn resolve(&mut self, snap: &Snapshot) {
        let set = &*snap.series;
        let range_ns = self.spec.range_ns;
        let mut fallback = false;
        let mut find = |id: SeriesId| {
            let (view, fell_back) = find_tiered(set, id, range_ns);
            fallback |= fell_back;
            view.cloned()
        };
        self.series.clear();
        self.series.extend(
            self.spec
                .cells
                .iter()
                .map(|c| (find(c.fill_series), c.overlay_series.and_then(&mut find))),
        );
        self.tier_fallback = fallback;
    }

    /// Replaces the spec. When it changed, resolves it against the hub's current snapshot and
    /// notifies.
    pub fn set_spec(&mut self, spec: ChartGridSpec, cx: &mut Context<Self>) {
        if self.spec != spec {
            self.spec = spec;
            let hub = self.hub.clone();
            self.resolve(hub.read(cx).current());
            cx.notify();
        }
    }

    /// Owner-supplied validity for every cell (Failed, Restricted, Unsupported). Notifies when
    /// it changed.
    pub fn set_state(&mut self, state: ChartState, cx: &mut Context<Self>) {
        if self.state != state {
            self.state = state;
            cx.notify();
        }
    }

    /// True when the owner state hides the data: Failed, Restricted or Unsupported.
    fn blocked(&self) -> bool {
        matches!(
            self.state,
            ChartState::Failed(_) | ChartState::Restricted | ChartState::Unsupported
        )
    }

    /// Cell `i`'s value text as last painted.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn value_text(&self, i: usize) -> Option<&str> {
        self.values.get(i)?.text()
    }

    /// The cell whose heading the pointer is over.
    pub fn hovered_cell(&self) -> Option<usize> {
        self.hovered_cell
    }

    /// The hovered cell's `tooltip` text, for the owner to show (PERF-028).
    pub fn hovered_tooltip(&self) -> Option<&SharedString> {
        self.spec.cells.get(self.hovered_cell?)?.tooltip.as_ref()
    }

    /// Hover pin, shaped lines and the last paint's box (E19-017).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn hover_state(&self) -> &HoverState {
        &self.hover
    }

    /// The cell whose plot the pointer was over at the last paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn hover_cell(&self) -> Option<usize> {
        self.hover_cell
    }

    /// Legend `shape_line` calls over every paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn legend_shapes(&self) -> u32 {
        self.legend.shape_count
    }

    /// Fill and stroke sets in the geometry cache, stale sets excluded.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cached_fill_stroke_counts(&self) -> (usize, usize) {
        let c = self.cache.borrow();
        (c.fills.len(), c.strokes.len())
    }

    /// The offline text as last read from the strings.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn offline_text(&self) -> Option<&SharedString> {
        self.offline_text.as_ref().map(|(_, text)| text)
    }

    /// Layout of the last geometry build.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn built_layout(&self) -> Option<GridLayout> {
        self.built_layout.map(|(l, _)| l)
    }

    /// Cached path vertices (all sets) inside cell `i`'s box, physical px relative to the
    /// element.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn vertices_in_cell(&self, i: usize) -> usize {
        let Some((layout, sf)) = self.built_layout else {
            return 0;
        };
        let (ox, oy) = cell_offset(&layout, i, self.spec.cell_h, self.spec.gap);
        let (x0, y0) = (ox * sf, oy * sf);
        let (x1, y1) = (
            x0 + f32::from(layout.cell_w) * sf,
            y0 + f32::from(self.spec.cell_h) * sf,
        );
        let c = self.cache.borrow();
        c.fills
            .iter()
            .chain(&c.strokes)
            .chain(&c.stale_fills)
            .chain(&c.stale_strokes)
            .flat_map(|s| s.path.vertices.iter())
            .filter(|v| {
                let (x, y) = (f32::from(v.xy_position.x), f32::from(v.xy_position.y));
                x > x0 && x < x1 && y > y0 && y < y1
            })
            .count()
    }

    /// Sets the resolved series directly, one pair per cell, bypassing the hub.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_series_for_test(&mut self, series: Vec<CellSeries>, cx: &mut Context<Self>) {
        self.series = series;
        cx.notify();
    }
}

impl Render for ChartGridView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        ChartGridElement::new(cx.entity())
    }
}

pub struct ChartGridElement {
    view: Entity<ChartGridView>,
}

impl ChartGridElement {
    pub fn new(view: Entity<ChartGridView>) -> Self {
        Self { view }
    }
}

impl IntoElement for ChartGridElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Heading hitboxes, one per cell, and plot hitboxes as (cell, hitbox), one per non-offline cell
/// when `spec.hover`. Both are empty while the pointer is outside the element.
#[derive(Default)]
pub struct GridHitboxes {
    headings: Vec<Hitbox>,
    plots: Vec<(usize, Hitbox)>,
}

impl Element for ChartGridElement {
    type RequestLayoutState = ();
    type PrepaintState = GridHitboxes;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    /// Full available width; the height is `grid_height` for that width, measured by the layout
    /// engine so the first frame already has the right height.
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let spec = &self.view.read(cx).spec;
        let (n, min_w, cell_h, gap) = (spec.cells.len(), spec.cell_min_w, spec.cell_h, spec.gap);
        let style = Style {
            size: Size {
                width: relative(1.).into(),
                height: Length::Auto,
            },
            ..Style::default()
        };
        let id = window.request_measured_layout(style, move |known, available, _, _| {
            let width = known.width.unwrap_or(match available.width {
                AvailableSpace::Definite(w) => w,
                _ => px(0.),
            });
            let layout = grid_layout(n, width, min_w, gap);
            size(width, grid_height(&layout, cell_h, gap))
        });
        (id, ())
    }

    /// One hitbox per heading while the pointer is inside the element, none otherwise. With
    /// `spec.hover`, also one per non-offline cell plot under the same rule.
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> GridHitboxes {
        let inside = bounds.contains(&window.mouse_position());
        let m = *cx.global::<UiMetrics>();
        self.view.update(cx, |view, _| {
            let mut hitboxes = mem::take(&mut view.hitboxes);
            hitboxes.headings.clear();
            hitboxes.plots.clear();
            let n = view.spec.cells.len();
            if inside && n > 0 {
                let spec = &view.spec;
                let layout = grid_layout(n, bounds.size.width, spec.cell_min_w, spec.gap);
                let pad = m.sp[3];
                let row = px(labels::LABEL_ROW * m.ui_scale);
                for i in 0..n {
                    let b = cell_bounds(bounds, &layout, i, spec.cell_h, spec.gap);
                    let width = view.headings.get(i).map_or(px(0.), CachedLine::width);
                    let heading =
                        Bounds::new(point(b.left() + pad, b.top() + pad), size(width, row));
                    hitboxes
                        .headings
                        .push(window.insert_hitbox(heading, HitboxBehavior::Normal));
                }
                if spec.hover {
                    for (i, cell) in spec.cells.iter().enumerate() {
                        if !cell.offline {
                            let b = cell_bounds(bounds, &layout, i, spec.cell_h, spec.gap);
                            let plot =
                                window.insert_hitbox(b.inset(px(1.)), HitboxBehavior::Normal);
                            hitboxes.plots.push((i, plot));
                        }
                    }
                }
            }
            #[cfg(any(test, feature = "test-hooks"))]
            view.hitbox_count
                .set((hitboxes.headings.len() + hitboxes.plots.len()) as u32);
            hitboxes
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitboxes: &mut GridHitboxes,
        window: &mut Window,
        cx: &mut App,
    ) {
        let env = PaintEnv {
            scale_factor: window.scale_factor(),
            now: now_boot_ns(),
            metrics: *cx.global::<UiMetrics>(),
            theme: crate::theme::current(cx).clone(),
            reduce_motion: cx.reduce_motion(),
            power_saving: crate::demand::power_saving(cx),
            network_unit: if cx.has_global::<TlSettings>() {
                TlSettings::settings(cx).units.network
            } else {
                NetworkUnit::default()
            },
        };
        let view_id = self.view.entity_id();
        let had_hitboxes = !hitboxes.headings.is_empty();
        // The readout shows only while the pointer is over a cell plot of an active window.
        let plot_cell = if window.is_window_active() {
            hitboxes
                .plots
                .iter()
                .find(|(_, h)| h.is_hovered(window))
                .map(|(i, _)| *i)
        } else {
            None
        };
        let (out, hover_changed) = self.view.update(cx, |view, cx| {
            let out = paint_grid_view(view, bounds, plot_cell, &env, window, cx);
            let hovered = hitboxes.headings.iter().position(|h| h.is_hovered(window));
            let changed = view.hovered_cell != hovered;
            view.hovered_cell = hovered;
            view.hitboxes = mem::take(hitboxes);
            (out, changed)
        });
        let moving = out.moving;
        if moving {
            AnimationDriver::global(window, cx).update(cx, |d, cx| d.request(view_id, window, cx));
        }
        // A notify during paint reaches no observer, so the owner hears of it after the frame.
        if hover_changed {
            cx.defer(move |cx| cx.notify(view_id));
        }
        // Pointer moves repaint the grid only when the pointer enters or leaves the element,
        // moves to another heading, or, with hover, changes the pinned time or the plot cell.
        // None of them rebuilds the geometry.
        let view = self.view.clone();
        let (map, grid, sf) = (out.map, out.grid, env.scale_factor);
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }
            let inside = bounds.contains(&e.position);
            view.update(cx, |view, cx| {
                if inside != had_hitboxes {
                    cx.notify();
                    return;
                }
                let mut notify = false;
                let hovered = view
                    .hitboxes
                    .headings
                    .iter()
                    .position(|h| h.is_hovered(window));
                if hovered != view.hovered_cell {
                    view.hovered_cell = hovered;
                    notify = true;
                }
                let plot = view
                    .hitboxes
                    .plots
                    .iter()
                    .find(|(_, h)| h.is_hovered(window));
                match plot {
                    // Same cell: move the pin. A new cell is pinned by the next paint.
                    Some((i, h)) if Some(*i) == view.hover_cell => {
                        let x = f32::from(e.position.x - h.bounds.left()) * sf;
                        let t = hover::pointer_time(&map, map.plot_width, x);
                        let t = Some(hover::snap_time(t, grid));
                        if view.hover.pinned_t_ns != t {
                            view.hover.pinned_t_ns = t;
                            notify = true;
                        }
                    }
                    Some(_) => notify = true,
                    None => notify |= view.hover_cell.is_some(),
                }
                if notify {
                    cx.notify();
                }
            });
        });
        if out.pinned {
            let view = self.view.clone();
            window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                if phase.bubble() {
                    view.update(cx, |v, cx| {
                        if v.hover.clear() {
                            cx.notify();
                        }
                    });
                }
            });
        }
    }
}

/// Per-paint inputs read from the window and the app before the view is borrowed.
struct PaintEnv {
    scale_factor: f32,
    now: u64,
    metrics: UiMetrics,
    theme: Arc<TlTheme>,
    reduce_motion: bool,
    /// Save power on battery is in effect: no sliding (wlf.32).
    power_saving: bool,
    network_unit: NetworkUnit,
}

struct PaintOut {
    /// The grid wants the next frame.
    moving: bool,
    /// Paint-time mapping, for the hover listener.
    map: TimeMap,
    /// A hover pin survived this paint.
    pinned: bool,
    /// The sample grid the hover pin snaps to.
    grid: hover::SampleGrid,
}

/// Builds the geometry when its key changed, then paints every cell and the readout of
/// `plot_cell`, the cell whose plot the pointer is over.
fn paint_grid_view(
    view: &mut ChartGridView,
    bounds: Bounds<Pixels>,
    plot_cell: Option<usize>,
    env: &PaintEnv,
    window: &mut Window,
    cx: &mut App,
) -> PaintOut {
    let sf = env.scale_factor;
    let now = env.now;
    let n = view.spec.cells.len();
    let plot_cell = plot_cell.filter(|_| view.spec.hover && !view.blocked());
    // A cell change drops the pin and the formatted lines; the new cell is pinned below.
    let entered = plot_cell != view.hover_cell;
    if entered {
        view.hover_cell = plot_cell;
        view.hover.last_index = None;
        view.hover.clear();
    }
    if n == 0 {
        return PaintOut {
            moving: false,
            map: TimeMap {
                anchor_ns: now,
                right_edge_ns: now,
                range_ns: view.spec.range_ns,
                plot_width: 0.0,
            },
            pinned: false,
            grid: hover::SampleGrid::default(),
        };
    }
    let layout = grid_layout(n, bounds.size.width, view.spec.cell_min_w, view.spec.gap);
    let blocked = view.blocked();
    let failed = matches!(view.state, ChartState::Failed(_) | ChartState::Restricted);
    let ChartGridView {
        spec,
        cache,
        scratch,
        built_right_edge_ns,
        built_layout,
        series,
        headings,
        values,
        offline,
        offline_text,
        gaps,
        legend,
        scale_state,
        axis_text,
        value_text,
        hover: hover_state,
        ..
    } = view;
    let (cell_h, gap) = (spec.cell_h, spec.gap);
    let pw = (f32::from(layout.cell_w - px(2.)) * sf).round().max(0.0) as u32;
    let ph = (f32::from(cell_h - px(2.)) * sf).round().max(0.0) as u32;

    let mut newest_ns = 0u64;
    let mut period_ns = u64::MAX;
    for s in series.iter().flat_map(|(a, b)| [a, b]).flatten() {
        if let Some(last) = s.samples.last() {
            newest_ns = newest_ns.max(last.t_ns);
        }
        period_ns = period_ns.min(s.period_ns);
    }
    if period_ns == u64::MAX {
        period_ns = DEFAULT_PERIOD_NS;
    }
    let sliding = spec.smooth && !env.reduce_motion && !env.power_saving;
    let stale = newest_ns > 0 && geometry::is_stale(newest_ns, now, period_ns);
    let right_edge = if newest_ns == 0 {
        now
    } else if stale {
        if sliding {
            now.saturating_sub(period_ns)
        } else {
            now
        }
    } else {
        geometry::right_edge_ns(now, newest_ns, period_ns, sliding)
    };
    let range_ns = spec.range_ns;
    // An owner state that hides the data builds and paints no paths.
    let has_data = newest_ns > 0 && !blocked;
    let sample_grid = hover::SampleGrid {
        anchor_ns: if newest_ns == 0 {
            right_edge
        } else {
            newest_ns
        },
        period_ns,
    };

    let mut cache = cache.borrow_mut();
    if has_data {
        let (vmin, vmax) = visible_min_max(series, right_edge, range_ns);
        let y = scale::compute(
            &spec.y,
            vmin,
            vmax,
            now,
            scale_state,
            spec.value_fmt,
            env.network_unit,
            axis_text,
        );
        // Sum of every cell's versions plus the cell count, and the column count. Offline cells
        // add their index above bit 40, so toggling one rebuilds.
        let versions = series
            .iter()
            .flat_map(|(a, b)| [a, b])
            .flatten()
            .fold(n as u64, |acc, s| acc.wrapping_add(s.version));
        let versions = spec
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.offline)
            .fold(versions, |acc, (i, _)| {
                acc.wrapping_add((i as u64 + 1) << 40)
            });
        let scalars = GeometryScalars {
            physical_width: pw,
            physical_height: ph,
            range_ns,
            scale_bits: spec.y.scale_bits(),
            y_min_bits: y.y_min.to_bits(),
            y_max_bits: y.y_max.to_bits(),
            style_version: env.metrics.style_version,
            curved: false,
        };
        let changed = cache
            .key
            .sync([versions, layout.cols as u64].into_iter(), scalars);
        if changed || (stale && right_edge != *built_right_edge_ns) {
            let map = TimeMap {
                anchor_ns: newest_ns,
                right_edge_ns: right_edge,
                range_ns,
                plot_width: pw as f32,
            };
            build_cells(
                spec, series, &layout, map, ph as f32, y.y_min, y.y_max, now, sf, &mut cache,
                scratch, gaps,
            );
            *built_right_edge_ns = right_edge;
            *built_layout = Some((layout, sf));
            #[cfg(any(test, feature = "test-hooks"))]
            view.build_count.set(view.build_count.get() + 1);
        }
    }

    let map = TimeMap {
        anchor_ns: if has_data {
            cache.anchor_ns
        } else {
            right_edge
        },
        right_edge_ns: right_edge,
        range_ns,
        plot_width: pw as f32,
    };
    let dx = map.dx(pw as f32);
    let theme = &*env.theme;
    let hairline = px(1.0 / sf);
    #[cfg(any(test, feature = "test-hooks"))]
    view.last_gap_spans.borrow_mut().clear();

    // Background, grid, dotted gaps, baseline and outline per cell. `gaps` is in cell order.
    let mut gap_at = 0;
    for (i, cell) in spec.cells.iter().enumerate() {
        let b = cell_bounds(bounds, &layout, i, cell_h, gap);
        grid::paint_background(window, b, theme.graph.chart_bg, px(0.));
        if cell.offline {
            window.paint_quad(quad(
                b,
                px(0.),
                transparent_black(),
                Edges::all(hairline),
                theme.divider,
                BorderStyle::Solid,
            ));
            continue;
        }
        let gp = GridPaint {
            plot: b.inset(px(1.)),
            box_bounds: b,
            map: &map,
            dx: px(dx),
            scale_factor: sf,
            theme,
            style: CELL_GRID,
            outline_color: spec.line,
            radius: px(0.),
        };
        if blocked {
            // As a single chart: outline and a dotted baseline across the plot, no grid.
            grid::paint_gaps(window, &gp, &[0.0..pw as f32]);
            grid::paint_outline(window, &gp);
            continue;
        }
        grid::paint_grid(window, &gp);
        while has_data && gap_at < gaps.len() && gaps[gap_at].0 <= i {
            let (cell_i, span) = &gaps[gap_at];
            gap_at += 1;
            if *cell_i == i {
                // `paint_gaps` clips to the plot width, so no content mask is needed.
                grid::paint_gaps(window, &gp, std::slice::from_ref(span));
                #[cfg(any(test, feature = "test-hooks"))]
                view.last_gap_spans.borrow_mut().push((i, span.clone()));
            }
        }
        grid::paint_baseline(window, &gp);
        grid::paint_outline(window, &gp);
    }

    // Every cell's paths in one mask and one translate. Vertices already carry the cell origin
    // and the 1 px plot inset.
    if has_data {
        let offset = point(bounds.left() + px(dx / sf), bounds.top());
        let painted = |window: &mut Window, sets: &[PathSet], alpha: f32| {
            for set in sets {
                let color = Hsla {
                    a: set.color.a * alpha,
                    ..set.color
                };
                window.paint_path(translated(&set.path, offset, sf), color);
                #[cfg(any(test, feature = "test-hooks"))]
                view.path_paints.set(view.path_paints.get() + 1);
            }
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            painted(window, &cache.fills, 1.0);
            painted(window, &cache.stale_fills, STALE_ALPHA);
            painted(window, &cache.strokes, 1.0);
            painted(window, &cache.stale_strokes, STALE_ALPHA);
        });
    }

    // Labels: heading left and value right in row 1, the offline text in the middle, legend in
    // cell 0.
    let m = &env.metrics;
    let pad = m.sp[3];
    let row = px(labels::LABEL_ROW * m.ui_scale);
    if headings.len() != n {
        headings.resize_with(n, CachedLine::default);
    }
    if values.len() != n {
        values.resize_with(n, CachedLine::default);
    }
    let heading_style = LineStyle {
        size: m.text.sm,
        weight: FontWeight::MEDIUM,
        color: theme.text_primary,
        tnum: false,
    };
    #[cfg(any(test, feature = "test-hooks"))]
    view.last_marks.borrow_mut().clear();
    for (i, cell) in spec.cells.iter().enumerate() {
        let b = cell_bounds(bounds, &layout, i, cell_h, gap);
        let row1 = b.top() + pad;
        let _shaped = headings[i].sync(window, &cell.heading, heading_style);
        #[cfg(any(test, feature = "test-hooks"))]
        if _shaped {
            view.heading_shapes.set(view.heading_shapes.get() + 1);
        }
        labels::paint_line(window, cx, &headings[i], point(b.left() + pad, row1), row);
        // The core mark (PERF-031): a 12 px icon after the heading in `text_secondary`.
        if let Some(icon) = cell.mark {
            let side = m.icon.xs;
            let x = b.left() + pad + headings[i].width() + m.sp[2];
            let mark = Bounds::new(point(x, row1 + (row - side) / 2.0), size(side, side));
            let _ = window.paint_svg(
                mark,
                icon.path(),
                None,
                TransformationMatrix::unit(),
                theme.text_secondary,
                cx,
            );
            #[cfg(any(test, feature = "test-hooks"))]
            view.last_marks.borrow_mut().push((i, icon, x));
        }

        if cell.offline {
            let style = LineStyle {
                size: m.text.sm,
                weight: FontWeight::NORMAL,
                color: theme.text_secondary,
                tnum: false,
            };
            let version = cx.try_global::<Strings>().map(|s| s.strings_version);
            if offline_text.as_ref().is_none_or(|(v, _)| *v != version) {
                let text = match version {
                    Some(_) => tr(cx, OFFLINE_KEY),
                    None => SharedString::new_static(OFFLINE_KEY),
                };
                *offline_text = Some((version, text));
                #[cfg(any(test, feature = "test-hooks"))]
                view.offline_reads.set(view.offline_reads.get() + 1);
            }
            let text = offline_text.as_ref().map_or("", |(_, t)| t.as_ref());
            let _shaped = offline.sync(window, text, style);
            #[cfg(any(test, feature = "test-hooks"))]
            if _shaped {
                view.offline_shapes.set(view.offline_shapes.get() + 1);
            }
            let c = b.center();
            let origin = point(c.x - offline.width() / 2.0, c.y - row / 2.0);
            labels::paint_line(window, cx, offline, origin, row);
            continue;
        }

        let (v, validity) = if blocked {
            let validity = if failed {
                Validity::Failed
            } else {
                Validity::Unsupported
            };
            (0.0, validity)
        } else {
            cell_value(series.get(i).and_then(|(a, _)| a.as_deref()), now)
        };
        value_text.clear();
        let color = match validity {
            Validity::Failed => {
                value_text.push_str(labels::FAILED);
                theme.status.warning
            }
            Validity::Unsupported => {
                value_text.push_str(labels::UNSUPPORTED);
                theme.text_secondary
            }
            Validity::Valid | Validity::Stale => {
                labels::format_value(spec.value_fmt, v, env.network_unit, value_text);
                if validity == Validity::Valid {
                    theme.text_primary
                } else {
                    theme.text_secondary
                }
            }
            Validity::Missing => {
                value_text.push_str(labels::MISSING);
                theme.text_secondary
            }
            _ => {
                value_text.push_str(labels::WARMING);
                theme.text_secondary
            }
        };
        let style = LineStyle {
            size: m.text.sm,
            weight: FontWeight::NORMAL,
            color,
            tnum: true,
        };
        let _shaped = values[i].sync(window, value_text, style);
        #[cfg(any(test, feature = "test-hooks"))]
        if _shaped {
            view.value_shapes.set(view.value_shapes.get() + 1);
        }
        let origin = point(b.right() - pad - values[i].width(), row1);
        labels::paint_line(window, cx, &values[i], origin, row);
    }

    if !spec.legend.is_empty() {
        let b = cell_bounds(bounds, &layout, 0, cell_h, gap);
        let stack = f32::from(layout.cell_w) < labels::LEGEND_STACK_BELOW_W * m.ui_scale;
        let colors = [spec.line, spec.overlay_line];
        let color = |i: usize| colors.get(i).copied().unwrap_or(theme.text_secondary);
        let origin = point(b.left() + pad, b.top() + pad + row);
        labels::paint_legend(
            window,
            cx,
            legend,
            &spec.legend,
            color,
            origin,
            stack,
            m,
            theme,
        );
    }

    if let Some(i) = plot_cell {
        let b = cell_bounds(bounds, &layout, i, cell_h, gap);
        let plot = b.inset(px(1.));
        // A new cell pins the time under the pointer; later moves are the listener's.
        if entered {
            let x = f32::from(window.mouse_position().x - plot.left()) * sf;
            let t = hover::pointer_time(&map, map.plot_width, x);
            hover_state.pinned_t_ns = Some(hover::snap_time(t, sample_grid));
        }
        let (fill_s, overlay_s) = series.get(i).cloned().unwrap_or_default();
        let has_overlay = spec.cells[i].overlay_series.is_some();
        let dashed = |k: usize| matches!(spec.legend.get(k), Some((_, LegendStyle::Dashed)));
        let mut chart = ChartSpec {
            range_ns,
            hover: true,
            ..ChartSpec::default()
        };
        chart.series.push(ChartSeries {
            line: spec.line,
            dashed: dashed(0),
            ..ChartSeries::default()
        });
        if has_overlay {
            chart.series.push(ChartSeries {
                line: spec.overlay_line,
                dashed: dashed(1),
                ..ChartSeries::default()
            });
        }
        let chart_labels = ChartLabels {
            heading: Some(spec.cells[i].heading.clone()),
            value_fmt: spec.value_fmt,
            value_align: ValueAlign::Right,
            range_word: SharedString::default(),
            legend: spec.legend.clone(),
            show_axis_max: false,
        };
        let cell_series = [fill_s, overlay_s];
        let hp = HoverPaint {
            box_bounds: bounds,
            plot,
            // Flip at the element's edge, so the box stays inside the element, not the cell.
            flip_right: bounds.right(),
            map,
            dx,
            scale_factor: sf,
            spec: &chart,
            labels: &chart_labels,
            series: &cell_series,
            y_min: cache.y_min,
            y_max: cache.y_max,
            metrics: m,
            theme,
            network_unit: env.network_unit,
        };
        hover::paint(window, cx, hover_state, &hp);
    } else {
        hover_state.index = None;
        #[cfg(any(test, feature = "test-hooks"))]
        {
            hover_state.last_box = None;
            hover_state.last_marker_x = None;
        }
    }

    let one_px_ns = range_ns / u64::from(pw.max(1));
    let moving = has_data
        && sliding
        && newest_ns
            .saturating_add(period_ns.saturating_mul(2))
            .saturating_add(one_px_ns)
            > now;
    PaintOut {
        moving,
        map,
        pinned: hover_state.pinned_t_ns.is_some(),
        grid: sample_grid,
    }
}

/// Cell `i`'s box in window coordinates, logical px.
fn cell_bounds(
    bounds: Bounds<Pixels>,
    layout: &GridLayout,
    i: usize,
    cell_h: Pixels,
    gap: Pixels,
) -> Bounds<Pixels> {
    let (ox, oy) = cell_offset(layout, i, cell_h, gap);
    Bounds::new(
        point(bounds.left() + px(ox), bounds.top() + px(oy)),
        size(layout.cell_w, cell_h),
    )
}

/// One geometry build for every non-offline cell into one fill, one total-line and one
/// overlay-line builder. Replaces `gaps` with each cell's dotted-gap spans.
#[allow(
    clippy::too_many_arguments,
    reason = "one build call, every input is distinct"
)]
fn build_cells(
    spec: &ChartGridSpec,
    series: &[CellSeries],
    layout: &GridLayout,
    map: TimeMap,
    plot_height: f32,
    y_min: f32,
    y_max: f32,
    now: u64,
    sf: f32,
    cache: &mut ChartCache,
    scratch: &mut geometry::Scratch,
    gaps: &mut Vec<(usize, Range<f32>)>,
) {
    gaps.clear();
    let mut fill = PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::non_zero()));
    let mut stroke = PathBuilder::stroke(px(1.0));
    let mut overlay = spec
        .cells
        .iter()
        .any(|c| c.overlay_series.is_some())
        .then(|| PathBuilder::stroke(px(1.0)));
    let total = ChartSeries {
        fill: Some(spec.fill),
        line: spec.line,
        ..ChartSeries::default()
    };
    let over = ChartSeries {
        line: spec.overlay_line,
        ..ChartSeries::default()
    };
    let base = BuildInput {
        series: &[],
        map,
        plot_height,
        y_min,
        y_max,
        now_boot_ns: now,
        stack: false,
        curved: false,
    };
    for (i, cell) in spec.cells.iter().enumerate() {
        if cell.offline {
            continue;
        }
        let Some((a, b)) = series.get(i) else {
            continue;
        };
        let pairs = [(total.clone(), a.as_deref()), (over.clone(), b.as_deref())];
        let (ox, oy) = cell_offset(layout, i, spec.cell_h, spec.gap);
        let origin = ((ox + 1.0) * sf, (oy + 1.0) * sf);
        let input = BuildInput {
            series: &pairs,
            ..base
        };
        geometry::build_cell(
            &input,
            origin,
            map.plot_width,
            &mut fill,
            &mut stroke,
            overlay.as_mut(),
            scratch,
        );
        gaps.extend(scratch.cell_gaps().iter().map(|g| (i, g.clone())));
    }
    geometry::finish_grid(
        &base,
        (spec.fill, fill),
        (spec.line, stroke),
        overlay.map(|b| (spec.overlay_line, b)),
        cache,
        scratch,
    );
}

/// Newest valid value of `series` and its validity at `now`: Missing when absent, WarmingUp
/// without a valid newest sample, Stale per R-051, else Valid.
fn cell_value(series: Option<&SeriesView>, now: u64) -> (f32, Validity) {
    let Some(s) = series else {
        return (0.0, Validity::Missing);
    };
    let value = s
        .samples
        .iter()
        .rev()
        .find(|s| s.is_valid())
        .map_or(0.0, |s| s.value);
    let validity = match s.samples.last() {
        None => Validity::WarmingUp,
        Some(last) if geometry::is_stale(last.t_ns, now, s.period_ns) => Validity::Stale,
        Some(last) if last.is_valid() => Validity::Valid,
        Some(_) => Validity::WarmingUp,
    };
    (value, validity)
}

/// Min and max of the valid samples inside `[right_edge − range, right_edge]` over every cell,
/// `(0, 0)` when there are none.
fn visible_min_max(series: &[CellSeries], right_edge: u64, range_ns: u64) -> (f32, f32) {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for s in series.iter().flat_map(|(a, b)| [a, b]).flatten() {
        let from = geometry::window_start(&s.samples, right_edge, range_ns, 0);
        for sample in &s.samples[from..] {
            if sample.t_ns > right_edge {
                break;
            }
            if sample.flags.contains(SampleFlags::VALID) {
                lo = lo.min(sample.value);
                hi = hi.max(sample.value);
            }
        }
    }
    if lo > hi { (0.0, 0.0) } else { (lo, hi) }
}

/// `path` (physical px) as logical px moved by `offset`, as `element.rs` does it: vertices and
/// bounds divided by `scale_factor` once, then shifted.
fn translated(path: &Path<Pixels>, offset: Point<Pixels>, scale_factor: f32) -> Path<Pixels> {
    let inv = 1.0 / scale_factor;
    let mut path = path.clone();
    path.bounds.origin = path.bounds.origin * inv + offset;
    path.bounds.size = size(path.bounds.size.width * inv, path.bounds.size.height * inv);
    for vertex in &mut path.vertices {
        vertex.xy_position = vertex.xy_position * inv + offset;
    }
    path
}
