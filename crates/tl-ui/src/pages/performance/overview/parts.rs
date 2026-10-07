//! The Overview boxes' drawing parts (UI-PRF-017, D-022): a meter bar, a per-core bar grid and the
//! box frame.
//!
//! [`Meter`] and [`CoreBars`] follow the custom `Element` pattern of `memory_bar.rs`
//! (UI-PRF-027): quads plus cached shaped text, no child elements. `TlProgress` is not allowed in
//! charts or cards (ADR-009). [`CoreBars`] re-formats and re-shapes a percent label only when its
//! 1% bucket changes (U-47).

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Bounds, Corners, Div, Element, ElementId, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, InteractiveElement as _, IntoElement, LayoutId, ParentElement as _, Pixels,
    SharedString, Style, Styled as _, Window, div, fill, point, px, relative, size,
};

use crate::chart::labels::{self, CachedLine, LABEL_ROW, LineStyle, WARMING};
use crate::components::progress::bucket;
use crate::text::style::{numeric_style, t_md, t_sm};
use crate::theme::{TlTheme, UiMetrics, current};

/// The meter bar height, logical px before `ui_scale`.
pub const METER_H: f32 = 6.;

/// The narrowest core cell, logical px before `ui_scale`.
const CELL_MIN_W: f32 = 140.;
/// The gap between core cells, logical px before `ui_scale`.
const CELL_GAP: f32 = 8.;
/// The core name column ("C12"), logical px before `ui_scale`.
const NAME_W: f32 = 28.;
/// The percent column ("100%"), logical px before `ui_scale`.
const PCT_W: f32 = 36.;
/// The gap between the bar and the percent, logical px before `ui_scale`.
const PCT_GAP: f32 = 4.;
/// The lowest row a squeezed core grid gets, logical px before `ui_scale`.
const MIN_ROW_H: f32 = 12.;
/// Rows lower than this hide the core labels, logical px before `ui_scale`.
const LABELS_MIN_ROW_H: f32 = 14.;
/// The box title row, logical px before `ui_scale`.
const TITLE_ROW: f32 = 24.;

/// `buckets` entry for a core with no valid sample.
const NO_BUCKET: u16 = u16::MAX;

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static METER_QUADS: RefCell<Vec<Bounds<Pixels>>> = const { RefCell::new(Vec::new()) };
}

/// The quads every [`Meter`] painted on this thread since the last call, track first.
#[cfg(any(test, feature = "test-hooks"))]
pub fn take_meter_quads() -> Vec<Bounds<Pixels>> {
    METER_QUADS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// A horizontal meter as wide as its parent and `METER_H` high: a track quad (`theme.divider`)
/// plus a fill quad (`color`), corner radius `radius_small`. `None` draws the track only.
pub struct Meter {
    pub fraction: Option<f32>,
    pub color: Hsla,
}

/// Fill width: `fraction` clamped to `0..=1`, rounded to whole physical px. A fraction above 0
/// gets at least 1 physical px; 1 and above gets the whole width.
pub fn meter_fill_w(fraction: f32, width: Pixels, scale_factor: f32) -> Pixels {
    // NaN and anything at or below 0 draw no fill.
    if fraction.is_nan() || fraction <= 0. {
        return px(0.);
    }
    if fraction >= 1. {
        return width;
    }
    let sf = scale_factor.max(f32::MIN_POSITIVE);
    let phys = (fraction * f32::from(width) * sf).round().max(1.);
    px(phys / sf).min(width)
}

/// Track and fill of one meter in `b`.
fn paint_meter(
    window: &mut Window,
    b: Bounds<Pixels>,
    fraction: Option<f32>,
    color: Hsla,
    track: Hsla,
    radius: Pixels,
    sf: f32,
) {
    let corners = Corners::all(radius);
    window.paint_quad(fill(b, track).corner_radii(corners));
    #[cfg(any(test, feature = "test-hooks"))]
    METER_QUADS.with(|q| q.borrow_mut().push(b));
    let Some(f) = fraction else {
        return;
    };
    let w = meter_fill_w(f, b.size.width, sf);
    if w <= px(0.) {
        return;
    }
    let fb = Bounds::new(b.origin, size(w, b.size.height));
    window.paint_quad(fill(fb, color).corner_radii(corners));
    #[cfg(any(test, feature = "test-hooks"))]
    METER_QUADS.with(|q| q.borrow_mut().push(fb));
}

impl IntoElement for Meter {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Meter {
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
        let m = cx.global::<UiMetrics>();
        let style = Style {
            size: size(relative(1.).into(), px(METER_H * m.ui_scale).into()),
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
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let radius = cx.global::<UiMetrics>().radius_small;
        let track = current(cx).divider;
        let sf = window.scale_factor();
        paint_meter(window, bounds, self.fraction, self.color, track, radius, sf);
    }
}

/// The per-core bars' data: one name, value and percent label per logical CPU.
#[derive(Default, Debug)]
pub struct CoreBarsModel {
    /// Percent per logical CPU id; `None` is no valid sample.
    pub values: Vec<Option<f32>>,
    /// The 1% bucket of each value, or `NO_BUCKET`.
    buckets: Vec<u16>,
    names: Vec<SharedString>,
    /// "12%", or "…" for no valid sample.
    pct_text: Vec<SharedString>,
    name_lines: Vec<CachedLine>,
    pct_lines: Vec<CachedLine>,
    /// Paints of the element.
    #[cfg(any(test, feature = "test-hooks"))]
    pub paint_count: Cell<u32>,
    /// Lines shaped while painting.
    #[cfg(any(test, feature = "test-hooks"))]
    pub shape_count: Cell<u32>,
}

impl CoreBarsModel {
    /// Sets the CPU names ("C0", "C1", …). Every value goes back to no valid sample.
    pub fn set_cpus(&mut self, names: impl Iterator<Item = SharedString>) {
        self.names.clear();
        self.names.extend(names);
        let n = self.names.len();
        self.values.clear();
        self.values.resize(n, None);
        self.buckets.clear();
        self.buckets.resize(n, NO_BUCKET);
        self.pct_text.clear();
        self.pct_text.resize(n, SharedString::new_static(WARMING));
        self.name_lines.resize_with(n, CachedLine::default);
        self.pct_lines.resize_with(n, CachedLine::default);
    }

    /// Takes one value per CPU; missing values count as no valid sample. True when a 1% bucket
    /// (`components::progress::bucket`) or a None/Some state changed. Re-formats only those
    /// labels ("12%").
    pub fn set_values(&mut self, values: impl Iterator<Item = Option<f32>>) -> bool {
        let mut values = values;
        let mut changed = false;
        for i in 0..self.names.len() {
            let v = values.next().flatten();
            self.values[i] = v;
            let b = v.map_or(NO_BUCKET, |v| bucket(v) as u16);
            if b == self.buckets[i] {
                continue;
            }
            self.buckets[i] = b;
            self.pct_text[i] = if b == NO_BUCKET {
                SharedString::new_static(WARMING)
            } else {
                format!("{b}%").into()
            };
            changed = true;
        }
        changed
    }
}

/// A grid of per-core bars, as large as its parent. Each cell is a name, a meter and a percent.
pub struct CoreBars {
    pub model: Rc<RefCell<CoreBarsModel>>,
}

/// Where the core cells go.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CoreBarsLayout {
    pub cols: usize,
    pub rows: usize,
    pub cell_w: Pixels,
    pub row_h: Pixels,
    /// False when the rows are too low for the name and percent.
    pub labels: bool,
}

/// The grid for `n` cores in `w × h`: as many 140 px cells per row as fit, 18 px rows, squeezed
/// down to 12 px when the rows do not fit. Labels hide below 14 px.
pub fn core_bars_layout(n: usize, w: Pixels, h: Pixels, scale: f32) -> CoreBarsLayout {
    let n = n.max(1);
    let gap = CELL_GAP * scale;
    let w = f32::from(w).max(0.);
    let fit = ((w + gap) / (CELL_MIN_W * scale + gap)).floor() as usize;
    let cols = fit.clamp(1, n);
    let rows = n.div_ceil(cols);
    let cell_w = ((w - gap * (cols - 1) as f32) / cols as f32).max(0.);
    let mut row_h = LABEL_ROW * scale;
    let h = f32::from(h);
    if rows as f32 * row_h > h {
        row_h = (MIN_ROW_H * scale).max(h / rows as f32);
    }
    CoreBarsLayout {
        cols,
        rows,
        cell_w: px(cell_w),
        row_h: px(row_h),
        labels: row_h >= LABELS_MIN_ROW_H * scale,
    }
}

impl IntoElement for CoreBars {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for CoreBars {
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
            size: size(relative(1.).into(), relative(1.).into()),
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
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let m = *cx.global::<UiMetrics>();
        let theme = current(cx).clone();
        let sf = window.scale_factor();
        let s = m.ui_scale;
        let mut model = self.model.borrow_mut();
        let model = &mut *model;
        #[cfg(any(test, feature = "test-hooks"))]
        model.paint_count.set(model.paint_count.get() + 1);

        let n = model.names.len();
        if n == 0 {
            return;
        }
        let l = core_bars_layout(n, bounds.size.width, bounds.size.height, s);
        let gap = px(CELL_GAP * s);
        let bar_h = px(METER_H * s);
        let (name_w, pct_w) = if l.labels {
            (px(NAME_W * s), px((PCT_W + PCT_GAP) * s))
        } else {
            (px(0.), px(0.))
        };
        let name_style = LineStyle {
            size: m.text.xs,
            weight: FontWeight::NORMAL,
            color: theme.text_secondary,
            tnum: false,
        };
        let pct_style = LineStyle {
            tnum: true,
            ..name_style
        };
        for i in 0..n {
            let (col, row) = (i % l.cols, i / l.cols);
            let x = bounds.left() + (l.cell_w + gap) * col as f32;
            let y = bounds.top() + l.row_h * row as f32;
            let bar_w = (l.cell_w - name_w - pct_w).max(px(0.));
            let bar = Bounds::new(
                point(x + name_w, y + (l.row_h - bar_h) / 2.),
                size(bar_w, bar_h),
            );
            let fraction = model.values[i].map(|v| v / 100.);
            paint_meter(
                window,
                bar,
                fraction,
                theme.graph.cpu_line,
                theme.divider,
                m.radius_small,
                sf,
            );
            if !l.labels {
                continue;
            }
            let name = &mut model.name_lines[i];
            let shaped = name.sync(window, &model.names[i], name_style);
            labels::paint_line(window, cx, name, point(x, y), l.row_h);
            let pct = &mut model.pct_lines[i];
            let shaped =
                u32::from(shaped) + u32::from(pct.sync(window, &model.pct_text[i], pct_style));
            let pct_x = x + l.cell_w - pct.width();
            labels::paint_line(window, cx, pct, point(pct_x, y), l.row_h);
            #[cfg(any(test, feature = "test-hooks"))]
            model.shape_count.set(model.shape_count.get() + shaped);
            #[cfg(not(any(test, feature = "test-hooks")))]
            let _ = shaped;
        }
    }
}

/// One Overview box: a `list_bg` card with a 24 px title row (title, a one-line summary that
/// ellipsizes, and the headline value at the right) above `body`, which fills the rest.
pub fn box_frame(
    id: &'static str,
    title: SharedString,
    value: SharedString,
    line: SharedString,
    body: AnyElement,
    m: &UiMetrics,
    theme: &TlTheme,
) -> Div {
    let mut title_el = div()
        .flex_none()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(title);
    *title_el.text_style() = t_md(m);
    let mut line_el = div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(line);
    *line_el.text_style() = t_sm(m);
    let mut value_el = div()
        .flex_none()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(value);
    *value_el.text_style() = numeric_style(m, m.text.md);
    div()
        .debug_selector(move || id.into())
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(theme.list_bg)
        .border_1()
        .border_color(theme.divider)
        .rounded(m.radius_card)
        .p(m.sp[5])
        .child(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[4])
                .h(px(TITLE_ROW * m.ui_scale))
                .child(title_el)
                .child(line_el)
                .child(value_el),
        )
        .child(div().flex_1().min_h_0().w_full().child(body))
}
