//! Charts of one line per item (tasklens-o55.57: the core clocks chart). A [`LineSet`] on a
//! `ChartView` adds four things to the usual chart:
//!
//! - a dashed horizontal line at a reference value (the base clock);
//! - a small ring on each line at its highest sample in the range;
//! - a highlighted line (`ChartView::set_highlight`): it is drawn again on top, 2 px wide and
//!   opaque, and the other lines dim;
//! - a hover readout that lists every line's value at the cursor time, high to low, with its
//!   peak in the range.
//!
//! The peaks and the highlighted line's geometry are built only when the chart's geometry is
//! built or the highlight changes, never per frame. The readout text is formatted only when the
//! hovered time, the series data or the peaks changed.
//!
//! The readout can list 64 lines, more than a short chart has room for, so the chart paints it
//! through a deferred overlay ([`ReadoutOverlay`]) with no clip: it may extend past the chart
//! and stays inside the window, in as many columns as the window height needs.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, BorderStyle, Bounds, Element, ElementId, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, Position, SharedString, Size, Style,
    Window, fill, point, px, quad, size,
};
use smallvec::SmallVec;
use tl_model::series::SeriesView;

use super::element::translated;
use super::geometry::{self, BuildInput, Scratch, TimeMap};
use super::hover::{nearest_index, offset_text};
use super::labels::{self, CachedLine, LABEL_ROW, LineStyle, NetFormat};
use super::{ChartCache, ChartSeries, ChartSpec, PathSet, ValueFormat};
use crate::theme::{TlTheme, UiMetrics};

/// Alpha factor of the other lines while one is highlighted.
pub const DIM: f32 = 0.3;
/// Width of the highlighted line, physical px at 100%.
const HIGHLIGHT_W: f32 = 2.0;
/// Radius of a peak ring at 100%.
const PEAK_R: f32 = 2.5;
/// Dash and gap of the reference line at 100%.
const DASH: f32 = 4.0;
/// Alpha of the reference line, on `text_secondary`.
const REFERENCE_ALPHA: f32 = 0.7;
/// Gap between the readout's columns at 100%.
const COL_GAP: f32 = 12.0;
/// Readout distance from the marker at 100%.
const BOX_GAP: f32 = 12.0;
/// The readout keeps this far from the window edges at 100%.
const MARGIN: f32 = 8.0;
/// Hover dot radius at 100%.
const DOT_R: f32 = 3.0;
/// Marker alpha factor.
const MARKER_ALPHA: f32 = 0.8;
/// Alpha factor of the stale sets.
const STALE_ALPHA: f32 = 0.5;

/// What the owner sets on a chart of one line per item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineSet {
    /// One name per spec series, for the readout rows ("Core 0").
    pub names: Arc<[SharedString]>,
    /// The heading of the readout's peak column ("Peak").
    pub peak_word: SharedString,
    /// A dashed line across the plot at this value, such as the base clock.
    pub reference: Option<f32>,
}

/// A line's peak: its point, chart-local physical px, and its value.
type Peak = (Point<Pixels>, f32);

/// The line set's state kept in the `ChartView` between frames.
#[derive(Default)]
pub struct LinesState {
    /// The peaks and the highlighted line need building again.
    pub(crate) dirty: bool,
    /// The highlighted series, by index in `spec.series`.
    pub(crate) highlight: Option<usize>,
    highlight_cache: ChartCache,
    scratch: Scratch,
    /// Each series' highest valid sample in the range at the last build: chart-local physical
    /// px (as `ChartCache::newest`) and the value.
    peaks: SmallVec<[Option<Peak>; 16]>,
    /// Each series' value at the hovered time.
    values: SmallVec<[Option<f32>; 16]>,
    /// The series in readout order: value high to low, the ones without a value last.
    order: SmallVec<[usize; 16]>,
    /// The hovered time, the series versions and the peaks the readout was formatted for.
    formatted: Option<(u64, u64, u64)>,
    pub(crate) readout: Rc<RefCell<Readout>>,
}

impl LinesState {
    /// The series in readout order, each with its value at the hovered time and its peak in
    /// the range, as last formatted.
    pub fn readout_rows(&self) -> impl Iterator<Item = (usize, Option<f32>, Option<f32>)> + '_ {
        self.order.iter().map(|&i| {
            let value = self.values.get(i).copied().flatten();
            let peak = self.peaks.get(i).copied().flatten().map(|(_, v)| v);
            (i, value, peak)
        })
    }

    /// The peak value of each series in the range at the last build.
    pub fn peak_values(&self) -> impl Iterator<Item = Option<f32>> + '_ {
        self.peaks.iter().map(|p| p.map(|(_, v)| v))
    }

    /// The readout panel of the last paint, `None` when none was painted.
    pub fn readout_panel(&self) -> Option<Bounds<Pixels>> {
        self.readout.borrow().panel
    }

    /// Stroke paths of the highlighted line from the last build.
    pub fn highlight_strokes(&self) -> usize {
        self.highlight_cache.strokes.len() + self.highlight_cache.stale_strokes.len()
    }
}

/// Inputs of a peak and highlight build: the map, scale and plot height the main geometry was
/// built with.
pub(crate) struct BuildCtx<'a> {
    pub spec: &'a ChartSpec,
    pub series: &'a [Option<Arc<SeriesView>>],
    pub map: TimeMap,
    pub plot_height: f32,
    pub y_min: f32,
    pub y_max: f32,
    pub now_boot_ns: u64,
    pub curved: bool,
}

/// Finds each series' peak in the range and builds the highlighted line.
pub(crate) fn rebuild(state: &mut LinesState, c: &BuildCtx) {
    state.dirty = false;
    state.peaks.clear();
    for view in c.series {
        let peak = view
            .as_deref()
            .and_then(|v| peak_in(v, &c.map))
            .map(|(t, v)| {
                let x = c.map.x_local(t);
                let y = geometry::y_of(v, c.y_min, c.y_max, c.plot_height);
                (point(px(x), px(y)), v)
            });
        state.peaks.push(peak);
    }
    let picked = state.highlight.and_then(|h| {
        let spec = c.spec.series.get(h)?;
        let view = c.series.get(h)?.as_deref()?;
        Some((spec, view))
    });
    let Some((spec, view)) = picked else {
        let cache = &mut state.highlight_cache;
        cache.fills.clear();
        cache.strokes.clear();
        cache.stale_fills.clear();
        cache.stale_strokes.clear();
        return;
    };
    let series = ChartSeries {
        fill: None,
        line: spec.line.alpha(1.0),
        line_width: px(HIGHLIGHT_W),
        ..spec.clone()
    };
    let pairs = [(series, Some(view))];
    let input = BuildInput {
        series: &pairs,
        map: c.map,
        plot_height: c.plot_height,
        y_min: c.y_min,
        y_max: c.y_max,
        now_boot_ns: c.now_boot_ns,
        stack: false,
        curved: c.curved,
    };
    geometry::build(&input, &mut state.highlight_cache, &mut state.scratch);
}

/// The time and value of the highest valid sample of `view` inside the map's range; the newest
/// of equals.
fn peak_in(view: &SeriesView, map: &TimeMap) -> Option<(u64, f32)> {
    let left = map.right_edge_ns.saturating_sub(map.range_ns);
    let from = geometry::window_start(&view.samples, map.right_edge_ns, map.range_ns, 0);
    let mut best: Option<(u64, f32)> = None;
    for s in &view.samples[from..] {
        if s.t_ns > map.right_edge_ns {
            break;
        }
        if s.t_ns < left || !s.is_valid() {
            continue;
        }
        if best.is_none_or(|(_, v)| s.value >= v) {
            best = Some((s.t_ns, s.value));
        }
    }
    best
}

/// The alpha factor of series `i`'s line: dimmed while another line is highlighted.
pub(crate) fn line_alpha(highlight: Option<usize>, i: usize) -> f32 {
    match highlight {
        Some(h) if h != i => DIM,
        _ => 1.0,
    }
}

/// The dashed reference line across `plot` at `value`, 1 physical px high. Nothing when the
/// value is outside the scale.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_reference(
    window: &mut Window,
    value: f32,
    plot: Bounds<Pixels>,
    y_min: f32,
    y_max: f32,
    sf: f32,
    m: &UiMetrics,
    theme: &TlTheme,
) {
    if !(y_min..=y_max).contains(&value) || y_max <= y_min {
        return;
    }
    let ph = f32::from(plot.size.height) * sf;
    let y = plot.top() + px(geometry::y_of(value, y_min, y_max, ph).round() / sf);
    let hairline = px(1.0 / sf);
    let dash = px(DASH * m.ui_scale);
    let color = theme
        .text_secondary
        .alpha(theme.text_secondary.a * REFERENCE_ALPHA);
    let mut x = plot.left();
    while x < plot.right() {
        let w = dash.min(plot.right() - x);
        window.paint_quad(fill(Bounds::new(point(x, y), size(w, hairline)), color));
        x += dash * 2.0;
    }
}

/// The highlighted line's paths at `offset`, on top of the others.
pub(crate) fn paint_highlight(
    window: &mut Window,
    state: &LinesState,
    offset: Point<Pixels>,
    sf: f32,
) {
    let painted = |window: &mut Window, sets: &[PathSet], alpha: f32| {
        for set in sets {
            let color = set.color.alpha(set.color.a * alpha);
            window.paint_path(translated(&set.path, offset, sf), color);
        }
    };
    painted(window, &state.highlight_cache.strokes, 1.0);
    painted(window, &state.highlight_cache.stale_strokes, STALE_ALPHA);
}

/// A ring on each line at its peak, in the line's colour, dimmed as the line is.
pub(crate) fn paint_peaks(
    window: &mut Window,
    state: &LinesState,
    spec: &ChartSpec,
    offset: Point<Pixels>,
    sf: f32,
    m: &UiMetrics,
    bg: Hsla,
) {
    let r = px(PEAK_R * m.ui_scale);
    let border = px(m.ui_scale);
    for (i, peak) in state.peaks.iter().enumerate() {
        let (Some((p, _)), Some(series)) = (peak, spec.series.get(i)) else {
            continue;
        };
        let alpha = if state.highlight == Some(i) {
            1.0
        } else {
            series.line.a * line_alpha(state.highlight, i)
        };
        let c = *p * (1.0 / sf) + offset;
        let ring = Bounds::new(point(c.x - r, c.y - r), size(r * 2.0, r * 2.0));
        window.paint_quad(quad(
            ring,
            r,
            bg,
            border,
            series.line.alpha(alpha),
            BorderStyle::Solid,
        ));
    }
}

/// Inputs of one hover paint.
pub(crate) struct HoverInput<'a> {
    pub plot: Bounds<Pixels>,
    /// Paint-time mapping; `dx` is its shift in physical px.
    pub map: TimeMap,
    pub dx: f32,
    pub sf: f32,
    pub spec: &'a ChartSpec,
    pub value_fmt: ValueFormat,
    pub set: &'a LineSet,
    pub series: &'a [Option<Arc<SeriesView>>],
    pub y_min: f32,
    pub y_max: f32,
    pub metrics: &'a UiMetrics,
    pub theme: &'a TlTheme,
    pub network_unit: NetFormat,
    /// The window's size; the readout stays inside it.
    pub viewport: Size<Pixels>,
}

/// The marker and a dot per line at the hovered time and, for the chart's own pin, the readout
/// for the overlay to paint. `follow_t_ns` is another linked chart's pin: marker and dots only.
pub(crate) fn paint_hover(
    window: &mut Window,
    state: &mut LinesState,
    pinned_t_ns: Option<u64>,
    follow_t_ns: Option<u64>,
    p: &HoverInput,
) {
    state.readout.borrow_mut().panel = None;
    let (pinned, own) = match (pinned_t_ns, follow_t_ns) {
        (Some(t), _) => (t, true),
        (None, Some(t)) => (t, false),
        (None, None) => return,
    };
    // The hovered sample: the nearest valid one of the first series that has one.
    let Some(t_ns) = p
        .series
        .iter()
        .flatten()
        .find_map(|v| nearest_index(&v.samples, pinned).map(|j| v.samples[j].t_ns))
    else {
        return;
    };
    let left_ns = p.map.right_edge_ns.saturating_sub(p.map.range_ns);
    if t_ns < left_ns || t_ns > p.map.right_edge_ns {
        return;
    }
    let versions = p
        .series
        .iter()
        .flatten()
        .fold(0u64, |acc, s| acc.wrapping_add(s.version));
    let peaks = state.peaks.iter().flatten().fold(0u64, |acc, (_, v)| {
        acc.rotate_left(5) ^ u64::from(v.to_bits())
    });
    let key = (t_ns, versions, peaks);
    if state.formatted != Some(key) {
        read_values(state, p, t_ns);
        if own {
            format_readout(window, state, p, t_ns);
            state.formatted = Some(key);
        } else {
            state.formatted = None;
        }
    }

    let m = p.metrics;
    let s = m.ui_scale;
    let sf = p.sf;
    let one_px = px(1.0 / sf);
    let x_phys = (p.map.x_local(t_ns) + p.dx).round();
    let x = p.plot.left() + px(x_phys / sf);
    let ph = f32::from(p.plot.size.height) * sf;
    let marker = Bounds::new(point(x, p.plot.top()), size(one_px, p.plot.size.height));
    let color = p.theme.text_secondary;
    window.paint_quad(fill(marker, color.alpha(color.a * MARKER_ALPHA)));
    let r = px(DOT_R * s);
    for (i, series) in p.spec.series.iter().enumerate() {
        let Some(v) = state.values.get(i).copied().flatten() else {
            continue;
        };
        let y = p.plot.top() + px(geometry::y_of(v, p.y_min, p.y_max, ph) / sf);
        let dot = Bounds::new(point(x - r, y - r), size(r * 2.0, r * 2.0));
        let line = series.line.alpha(if state.highlight == Some(i) {
            1.0
        } else {
            series.line.a * line_alpha(state.highlight, i)
        });
        window.paint_quad(quad(
            dot,
            r,
            line,
            one_px,
            p.theme.window_bg,
            BorderStyle::Solid,
        ));
    }
    if own {
        place_readout(state, p, x);
    }
}

/// Each series' value at `t_ns`: its nearest valid sample, missing when that is over one
/// period away.
fn read_values(state: &mut LinesState, p: &HoverInput, t_ns: u64) {
    state.values.clear();
    state.values.extend(p.series.iter().map(|v| {
        let v = v.as_deref()?;
        let j = nearest_index(&v.samples, t_ns)?;
        let s = v.samples[j];
        (s.t_ns.abs_diff(t_ns) <= v.period_ns.max(1)).then_some(s.value)
    }));
    state.values.resize(p.spec.series.len(), None);
}

/// Orders the rows and formats and shapes every readout line: line 0 the time offset, line 1
/// the peak heading, then per series its name, value and peak.
fn format_readout(window: &mut Window, state: &mut LinesState, p: &HoverInput, t_ns: u64) {
    let n = p.spec.series.len();
    state.order.clear();
    state.order.extend(0..n);
    let values = &state.values;
    state.order.sort_by(|&a, &b| {
        let (va, vb) = (values[a], values[b]);
        match (va, vb) {
            (Some(a), Some(b)) => b.total_cmp(&a),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    let m = p.metrics;
    let primary = LineStyle {
        size: m.text.xs,
        weight: FontWeight::NORMAL,
        color: p.theme.text_primary,
        tnum: true,
    };
    let secondary = LineStyle {
        color: p.theme.text_secondary,
        ..primary
    };
    let mut readout = state.readout.borrow_mut();
    let lines = &mut readout.lines;
    lines.resize_with(2 + 3 * n, CachedLine::default);
    let mut text = String::new();
    let newest = p
        .series
        .iter()
        .flatten()
        .filter_map(|v| v.samples.last())
        .map(|s| s.t_ns)
        .max()
        .unwrap_or(t_ns);
    offset_text(newest.saturating_sub(t_ns), p.spec.range_ns, &mut text);
    lines[0].sync(
        window,
        &text,
        LineStyle {
            weight: FontWeight::MEDIUM,
            tnum: false,
            ..primary
        },
    );
    lines[1].sync(
        window,
        &p.set.peak_word,
        LineStyle {
            tnum: false,
            ..secondary
        },
    );
    for i in 0..n {
        let name = p.set.names.get(i).map_or("", |s| s.as_ref());
        lines[2 + 3 * i].sync(
            window,
            name,
            LineStyle {
                tnum: false,
                ..secondary
            },
        );
        let mut value = |v: Option<f32>, style, line: &mut CachedLine| {
            text.clear();
            match v {
                Some(v) => labels::format_value(p.value_fmt, v, p.network_unit, &mut text),
                None => text.push_str(labels::MISSING),
            }
            line.sync(window, &text, style);
        };
        value(state.values[i], primary, &mut lines[3 + 3 * i]);
        let peak = state.peaks.get(i).copied().flatten().map(|(_, v)| v);
        value(peak, secondary, &mut lines[4 + 3 * i]);
    }
}

/// Lays the readout out beside the marker at `x`: a header row, then the rows in `order`, in as
/// many columns as the window height needs. It sits right of the marker, or left of it where
/// it would pass the window's right edge, and stays inside the window.
fn place_readout(state: &mut LinesState, p: &HoverInput, x: Pixels) {
    let n = p.spec.series.len();
    let mut readout = state.readout.borrow_mut();
    let readout = &mut *readout;
    if readout.lines.len() != 2 + 3 * n || n == 0 {
        return;
    }
    let m = p.metrics;
    let s = m.ui_scale;
    let pad = m.sp[3];
    let row = px(LABEL_ROW * s);
    let gap = px(COL_GAP * s);
    let margin = px(MARGIN * s);
    let lines = &readout.lines;
    let width = |k: usize| lines[k].width();
    let max_of = |part: usize| {
        (0..n)
            .map(|i| width(2 + 3 * i + part))
            .fold(px(0.), |a, b| a.max(b))
    };
    let name_w = max_of(0);
    let value_w = max_of(1);
    let peak_w = max_of(2).max(width(1));
    let col_w = (name_w + gap + value_w + gap + peak_w).max(width(0) + gap + peak_w);

    // Rows per column: as many as the window height holds under the header row.
    let room = p.viewport.height - margin * 2.0 - pad * 2.0 - row;
    let fit = (f32::from(room) / f32::from(row)).floor().max(1.0) as usize;
    let cols = n.div_ceil(fit).max(1);
    let per_col = n.div_ceil(cols);
    let box_w = pad * 2.0 + col_w * cols as f32 + gap * 2.0 * (cols - 1) as f32;
    let box_h = pad * 2.0 + row * (per_col + 1) as f32;
    let box_gap = px(BOX_GAP * s);
    let mut left = x + box_gap;
    if left + box_w > p.viewport.width - margin {
        left = x - box_gap - box_w;
    }
    let left = left.min(p.viewport.width - margin - box_w).max(margin);
    let top = p
        .plot
        .top()
        .min(p.viewport.height - margin - box_h)
        .max(margin);
    let panel = Bounds::new(point(left, top), size(box_w, box_h));

    readout.at.clear();
    for c in 0..cols {
        let x0 = left + pad + (col_w + gap * 2.0) * c as f32;
        let y0 = top + pad;
        if c == 0 {
            readout.at.push((0, point(x0, y0)));
        }
        readout.at.push((1, point(x0 + col_w - width(1), y0)));
        for (k, &i) in state
            .order
            .iter()
            .skip(c * per_col)
            .take(per_col)
            .enumerate()
        {
            let y = y0 + row * (k + 1) as f32;
            readout.at.push((2 + 3 * i, point(x0, y)));
            let value = 3 + 3 * i;
            readout
                .at
                .push((value, point(x0 + name_w + gap + value_w - width(value), y)));
            let peak = 4 + 3 * i;
            readout.at.push((peak, point(x0 + col_w - width(peak), y)));
        }
    }
    readout.panel = Some(panel);
    readout.row_h = row;
    readout.bg = p.theme.menu_bg;
    readout.border = p.theme.divider;
    readout.radius = m.radius_small;
    readout.border_w = px(1.0 / p.sf);
}

/// The readout as the chart's last paint laid it out, for [`ReadoutOverlay`] to paint.
#[derive(Default)]
pub struct Readout {
    /// `None` when there is nothing to paint.
    panel: Option<Bounds<Pixels>>,
    /// Line 0 the time offset, line 1 the peak heading, then per series name, value and peak.
    lines: Vec<CachedLine>,
    /// Which line goes where, window px.
    at: Vec<(usize, Point<Pixels>)>,
    row_h: Pixels,
    bg: Hsla,
    border: Hsla,
    radius: Pixels,
    border_w: Pixels,
}

/// Paints the readout of a [`Readout`] after the rest of the window, unclipped. The chart adds
/// it through `deferred` while it has a hover pin; its own layout is empty.
pub struct ReadoutOverlay(pub(crate) Rc<RefCell<Readout>>);

impl IntoElement for ReadoutOverlay {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ReadoutOverlay {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
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
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let r = self.0.borrow();
        let Some(panel) = r.panel else {
            return;
        };
        window.paint_quad(quad(
            panel,
            r.radius,
            r.bg,
            r.border_w,
            r.border,
            BorderStyle::Solid,
        ));
        for &(k, at) in &r.at {
            if let Some(line) = r.lines.get(k) {
                labels::paint_line(window, cx, line, at, r.row_h);
            }
        }
    }
}
