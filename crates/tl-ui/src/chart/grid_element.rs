//! `ChartGridView` and `ChartGridElement`: one element paints n chart cells, such as the CPU
//! per-core grid and the GPU engine grid (ADR-009 rule 7, UI-PRF-002, PERF-028, PERF-063).
//!
//! Every cell's geometry goes into one fill path, one total-line path and one overlay-line path,
//! so a frame clones 3 paths (plus up to 3 stale sets) however many cells there are. Cells share
//! one time map, so one translate serves them all, and their paths are clipped to the cell at
//! build time, so one content mask covers the element. Labels are cached shaped lines and
//! re-shape only when their text changes.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{
    App, AvailableSpace, BorderStyle, Bounds, ContentMask, Context, Edges, Element, ElementId,
    Entity, FillOptions, FontWeight, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    LayoutId, Length, Path, PathBuilder, PathStyle, Pixels, Point, Render, SharedString, Size,
    Style, Subscription, Window, point, px, quad, relative, size, transparent_black,
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
use super::labels::{self, CachedLine, LabelCache, LineStyle};
use super::view::find_series;
use super::{
    ChartCache, ChartSeries, DEFAULT_RANGE_NS, GeometryScalars, GridStyle, LegendStyle, PathSet,
    ValueFormat, YScale, scale,
};
use crate::anim::AnimationDriver;
use crate::hub::{SnapshotChanged, SnapshotHub};
use crate::settings_global::TlSettings;
use crate::theme::{TlTheme, UiMetrics};

/// Period used when no cell has data: 1 s.
const DEFAULT_PERIOD_NS: u64 = 1_000_000_000;

/// Alpha factor of the stale sets.
const STALE_ALPHA: f32 = 0.5;

/// Text of an offline cell. Not yet a Fluent key.
const OFFLINE: &str = "Offline";

/// Every cell draws grid lines, baseline and outline.
const CELL_GRID: GridStyle = GridStyle {
    lines: true,
    baseline: true,
    outline: true,
    rounded: false,
};

/// One cell of the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct GridCellSpec {
    pub heading: SharedString,
    pub fill_series: SeriesId,
    pub overlay_series: Option<SeriesId>,
    /// No plot; "Offline" in the middle and a `divider` outline. Owners set it for a cell whose
    /// series is Missing (CPU offline).
    pub offline: bool,
    /// P2 core marks.
    pub mark: Option<IconName>,
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
    series: Vec<(Option<Arc<SeriesView>>, Option<Arc<SeriesView>>)>,
    headings: Vec<CachedLine>,
    values: Vec<CachedLine>,
    offline: CachedLine,
    legend: LabelCache,
    scale_state: scale::ScaleState,
    axis_text: String,
    /// Reused value formatter buffer.
    value_text: String,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
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
    #[cfg(any(test, feature = "test-hooks"))]
    pub offline_shapes: std::cell::Cell<u32>,
}

impl ChartGridView {
    /// Subscribes to `hub` and resolves the spec against its current snapshot.
    pub fn new(spec: ChartGridSpec, hub: &Entity<SnapshotHub>, cx: &mut Context<Self>) -> Self {
        let _hub_sub = cx.subscribe(hub, Self::on_snapshot);
        let mut this = Self {
            spec,
            hub: hub.clone(),
            _hub_sub,
            cache: Rc::default(),
            scratch: geometry::Scratch::default(),
            built_right_edge_ns: 0,
            built_layout: None,
            series: Vec::new(),
            headings: Vec::new(),
            values: Vec::new(),
            offline: CachedLine::default(),
            legend: LabelCache::default(),
            scale_state: scale::ScaleState::default(),
            axis_text: String::new(),
            value_text: String::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            build_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            path_paints: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            heading_shapes: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            value_shapes: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            offline_shapes: std::cell::Cell::new(0),
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
        let changed = |id: SeriesId| ev.changed_series.binary_search(&id).is_ok();
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

    /// Looks up every cell's series in `snap` by binary search. Reuses the list's capacity.
    pub fn resolve(&mut self, snap: &Snapshot) {
        let set = &*snap.series;
        self.series.clear();
        self.series.extend(self.spec.cells.iter().map(|c| {
            (
                find_series(set, c.fill_series).cloned(),
                c.overlay_series
                    .and_then(|id| find_series(set, id))
                    .cloned(),
            )
        }));
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

    /// Fill and stroke sets in the geometry cache, stale sets excluded.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cached_fill_stroke_counts(&self) -> (usize, usize) {
        let c = self.cache.borrow();
        (c.fills.len(), c.strokes.len())
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
    pub fn set_series_for_test(
        &mut self,
        series: Vec<(Option<Arc<SeriesView>>, Option<Arc<SeriesView>>)>,
        cx: &mut Context<Self>,
    ) {
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

impl Element for ChartGridElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

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

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let env = PaintEnv {
            scale_factor: window.scale_factor(),
            now: now_boot_ns(),
            metrics: *cx.global::<UiMetrics>(),
            theme: crate::theme::current(cx).clone(),
            reduce_motion: cx.reduce_motion(),
            network_unit: if cx.has_global::<TlSettings>() {
                TlSettings::settings(cx).units.network
            } else {
                NetworkUnit::default()
            },
        };
        let view_id = self.view.entity_id();
        let moving = self.view.update(cx, |view, cx| {
            paint_grid_view(view, bounds, &env, window, cx)
        });
        if moving {
            AnimationDriver::global(window, cx).update(cx, |d, cx| d.request(view_id, window, cx));
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
    network_unit: NetworkUnit,
}

/// Builds the geometry when its key changed, then paints every cell. Returns whether the grid
/// wants the next frame.
fn paint_grid_view(
    view: &mut ChartGridView,
    bounds: Bounds<Pixels>,
    env: &PaintEnv,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let sf = env.scale_factor;
    let now = env.now;
    let n = view.spec.cells.len();
    if n == 0 {
        return false;
    }
    let layout = grid_layout(n, bounds.size.width, view.spec.cell_min_w, view.spec.gap);
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
        legend,
        scale_state,
        axis_text,
        value_text,
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
    let sliding = spec.smooth && !env.reduce_motion;
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
    let has_data = newest_ns > 0;

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
                scratch,
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

    // Background, grid, baseline and outline per cell.
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
        grid::paint_grid(window, &gp);
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

    // Labels: heading left and value right in row 1, "Offline" in the middle, legend in cell 0.
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
    for (i, cell) in spec.cells.iter().enumerate() {
        let b = cell_bounds(bounds, &layout, i, cell_h, gap);
        let row1 = b.top() + pad;
        let _shaped = headings[i].sync(window, &cell.heading, heading_style);
        #[cfg(any(test, feature = "test-hooks"))]
        if _shaped {
            view.heading_shapes.set(view.heading_shapes.get() + 1);
        }
        labels::paint_line(window, cx, &headings[i], point(b.left() + pad, row1), row);

        if cell.offline {
            let style = LineStyle {
                size: m.text.sm,
                weight: FontWeight::NORMAL,
                color: theme.text_secondary,
                tnum: false,
            };
            let _shaped = offline.sync(window, OFFLINE, style);
            #[cfg(any(test, feature = "test-hooks"))]
            if _shaped {
                view.offline_shapes.set(view.offline_shapes.get() + 1);
            }
            let c = b.center();
            let origin = point(c.x - offline.width() / 2.0, c.y - row / 2.0);
            labels::paint_line(window, cx, offline, origin, row);
            continue;
        }

        let (v, validity) = cell_value(series.get(i).and_then(|(a, _)| a.as_deref()), now);
        value_text.clear();
        let color = match validity {
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

    let one_px_ns = range_ns / u64::from(pw.max(1));
    has_data
        && sliding
        && newest_ns
            .saturating_add(period_ns.saturating_mul(2))
            .saturating_add(one_px_ns)
            > now
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
/// overlay-line builder.
#[allow(
    clippy::too_many_arguments,
    reason = "one build call, every input is distinct"
)]
fn build_cells(
    spec: &ChartGridSpec,
    series: &[(Option<Arc<SeriesView>>, Option<Arc<SeriesView>>)],
    layout: &GridLayout,
    map: TimeMap,
    plot_height: f32,
    y_min: f32,
    y_max: f32,
    now: u64,
    sf: f32,
    cache: &mut ChartCache,
    scratch: &mut geometry::Scratch,
) {
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
fn visible_min_max(
    series: &[(Option<Arc<SeriesView>>, Option<Arc<SeriesView>>)],
    right_edge: u64,
    range_ns: u64,
) -> (f32, f32) {
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
