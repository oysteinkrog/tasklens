//! The Overview boxes' drawing parts (UI-PRF-017, D-022): a meter bar, a per-core bar grid, the
//! box frame and the sparkline rows.
//!
//! [`Meter`] and [`CoreBars`] follow the custom `Element` pattern of `memory_bar.rs`
//! (UI-PRF-027): quads plus cached shaped text, no child elements. `TlProgress` is not allowed in
//! charts or cards (ADR-009). [`CoreBars`] re-formats and re-shapes a percent label only when its
//! 1% bucket changes (U-47).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Corners, Div, Element, ElementId, Entity,
    EntityId, FontWeight, GlobalElementId, Hsla, InspectorElementId, InteractiveElement as _,
    IntoElement, LayoutId, MouseButton, ParentElement as _, Pixels, SharedString, Size, Style,
    StyleRefinement, Styled as _, Window, canvas, div, fill, point, px, relative, size,
};
use smallvec::SmallVec;
use tl_model::settings::Settings;

use crate::chart::labels::{self, CachedLine, LABEL_ROW, LineStyle, WARMING};
use crate::chart::{ChartLabels, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat};
use crate::components::progress::bucket;
use crate::hub::SnapshotHub;
use crate::pages::performance::DeviceKey;
use crate::pages::performance::actions::PerfSelectDevice;
use crate::settings_global::{Section, TlSettings};
use crate::text::style::{numeric_style, t_md, t_sm, t_xs};
use crate::theme::{TlTheme, UiMetrics, current};

use super::layout::BoxKind;
use super::mem_box::{MeterRow, ROW_LABEL_W, ROW_VALUE_W, meter_row};

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
/// The two labels on the one time axis shared by every device box.
pub const TIME_AXIS_START: &str = "60 seconds ago";
pub const TIME_AXIS_END: &str = "Now";

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

    /// "C0\t12%" per CPU, one per line.
    pub fn copy_text(&self, out: &mut String) {
        for (name, pct) in self.names.iter().zip(&self.pct_text) {
            out.push_str(name);
            out.push('\t');
            out.push_str(pct);
            out.push('\n');
        }
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

/// `el` selects `target`'s page on a left click (crw.44). A static hitbox: no hover style, so
/// pointer moves notify nothing. `None` leaves `el` as it is.
pub fn opens_device(el: Div, target: Option<DeviceKey>) -> Div {
    match target {
        Some(key) => el.on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.dispatch_action(Box::new(PerfSelectDevice { key: key.clone() }), cx);
        }),
        None => el,
    }
}

/// The debug selector of a box's title row: the box id with `-title`.
pub fn title_row_id(box_id: &str) -> String {
    format!("{box_id}-title")
}

/// The debug selector of the title tab inset into a box's top border.
pub fn title_tab_id(box_id: &str) -> String {
    format!("{box_id}-title-tab")
}

/// The debug selector of the one time axis below a device box's charts.
pub fn time_axis_id(box_id: &str) -> String {
    format!("{box_id}-time-axis")
}

/// The debug selector of a device box's left or right time-axis label.
pub fn time_axis_label_id(box_id: &str, end: &str) -> String {
    format!("{box_id}-time-axis-{end}")
}

/// The one accent used by an Overview box frame and all of that device's data marks.
pub fn box_accent(box_id: &str, theme: &TlTheme) -> Hsla {
    let kind = BoxKind::from_frame_id(box_id)
        .unwrap_or_else(|| panic!("unknown Overview box frame id {box_id:?}"));
    theme.graph.device_accent(kind.accent())
}

/// One Overview box: a `list_bg` card with a thin rounded device-accent frame. Its 24 px title
/// line sits in the top border: an accent title tab, a one-line summary that ellipsizes and the
/// headline value at the right. `body` fills the rest. A click on the title line opens
/// `target`'s page.
#[allow(
    clippy::too_many_arguments,
    reason = "one call per box, all positional parts"
)]
pub fn box_frame(
    id: &'static str,
    target: Option<DeviceKey>,
    title: SharedString,
    value: SharedString,
    line: SharedString,
    body: AnyElement,
    m: &UiMetrics,
    theme: &TlTheme,
) -> Div {
    let accent = box_accent(id, theme);
    let draws_time_axis = BoxKind::from_frame_id(id) != Some(BoxKind::TopProcs);
    let mut title_el = div()
        .debug_selector(move || title_tab_id(id))
        .h_full()
        .flex()
        .items_center()
        .flex_none()
        .px(m.sp[4])
        .bg(theme.list_bg)
        .border_l_1()
        .border_r_1()
        .border_b_1()
        .border_color(accent)
        .rounded_bl(m.radius_small)
        .rounded_br(m.radius_small)
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
        .border_color(accent)
        .rounded(m.radius_card)
        .child(opens_device(
            div()
                .debug_selector(move || title_row_id(id))
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[4])
                .h(px(TITLE_ROW * m.ui_scale))
                .mx(m.sp[5])
                .child(title_el)
                .child(line_el)
                .child(value_el),
            target,
        ))
        .child({
            let mut body_el = div()
                .flex_1()
                .min_h_0()
                .w_full()
                .px(m.sp[5])
                .pt(m.sp[2])
                .pb(m.sp[5]);
            if draws_time_axis {
                let mut ago = div()
                    .debug_selector(move || time_axis_label_id(id, "start"))
                    .child(TIME_AXIS_START);
                *ago.text_style() = t_xs(m);
                let mut now = div()
                    .debug_selector(move || time_axis_label_id(id, "end"))
                    .child(TIME_AXIS_END);
                *now.text_style() = t_xs(m);
                body_el = body_el
                    .flex()
                    .flex_col()
                    .child(div().flex_1().min_h_0().w_full().child(body))
                    .child(
                        div()
                            .debug_selector(move || time_axis_id(id))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_between()
                            .h(px(LABEL_ROW * m.ui_scale))
                            .text_color(theme.text_secondary)
                            .child(ago)
                            .child(now),
                    );
            } else {
                body_el = body_el.child(body);
            }
            body_el
        })
}

/// Box widths from here up put the chart and its side part next to each other; narrower boxes
/// stack them. Logical px before `ui_scale`.
pub const SPLIT_W: f32 = 560.;

/// The chart's share of a wide box's body.
const CHART_SHARE: f32 = 0.55;

/// A canvas over its parent (which must be `relative`) that keeps the parent's width in `width`
/// and notifies `view` after the frame when the width crosses `threshold` the other way from
/// `wide`, the side the last render used. Resizes inside one side never notify.
pub fn width_probe(
    width: Rc<Cell<Pixels>>,
    wide: bool,
    threshold: Pixels,
    view: EntityId,
) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            width.set(bounds.size.width);
            if (bounds.size.width >= threshold) != wide {
                cx.defer(move |cx| cx.notify(view));
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A canvas over its parent (which must be `relative`) that keeps the parent's size in `size`
/// and notifies `view` after the frame when `classify(size)` differs from `class`, the layout
/// class the last render used (such as how many rows fit). Other resizes never notify.
pub fn class_probe(
    size: Rc<Cell<Size<Pixels>>>,
    class: u32,
    classify: impl Fn(Size<Pixels>) -> u32 + 'static,
    view: EntityId,
) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            size.set(bounds.size);
            if classify(bounds.size) != class {
                cx.defer(move |cx| cx.notify(view));
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The lowest a chart in a box with rows below it gets, logical px before `ui_scale` (TMOG).
pub const MIN_CHART_H: f32 = 48.;

/// Whether a box `width` wide (0 before the first paint) puts its chart beside its side part.
pub fn is_wide(width: Pixels, scale: f32) -> bool {
    width == px(0.) || width >= px(SPLIT_W * scale)
}

/// The body of a box: `chart` on the left (55%) and `side` on the right, or `chart` above
/// `side` when not `wide`.
pub fn split_body(chart: AnyElement, side: AnyElement, wide: bool, m: &UiMetrics) -> Div {
    split_body_with_side_height(chart, side, wide, None, m)
}

/// The body of a box whose stacked side has a fixed height. Wide boxes still use the same 55%
/// chart split as [`split_body`].
pub fn split_body_fixed_side(
    chart: AnyElement,
    side: AnyElement,
    wide: bool,
    side_height: Pixels,
    m: &UiMetrics,
) -> Div {
    split_body_with_side_height(chart, side, wide, Some(side_height), m)
}

fn split_body_with_side_height(
    chart: AnyElement,
    side: AnyElement,
    wide: bool,
    side_height: Option<Pixels>,
    m: &UiMetrics,
) -> Div {
    if wide {
        div()
            .flex()
            .flex_row()
            .size_full()
            .gap(m.sp[5])
            .child(
                div()
                    .flex_none()
                    .w(relative(CHART_SHARE))
                    .h_full()
                    .child(chart),
            )
            .child(div().flex_1().min_w_0().h_full().child(side))
    } else {
        let side = div().w_full().child(side);
        let side = match side_height {
            Some(height) => side.flex_none().h(height),
            None => side.flex_1().min_h_0(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap(m.sp[5])
            .child(div().flex_1().min_h_0().w_full().child(chart))
            .child(side)
    }
}

/// The settings a box chart is built from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChartSettings {
    pub kernel: bool,
    pub range_ns: u64,
    /// Always false: the Overview charts step at the sample rate (crw.46). A sliding chart
    /// notifies every animation frame, and GPUI then re-renders its box and the page too.
    pub smooth: bool,
    pub rounded: bool,
}

impl ChartSettings {
    pub fn read(cx: &App) -> Self {
        Self::from(TlSettings::settings(cx))
    }
}

impl From<&Settings> for ChartSettings {
    fn from(s: &Settings) -> Self {
        Self {
            kernel: s.performance.cpu_kernel,
            range_ns: u64::from(s.performance.graph_range_s) * 1_000_000_000,
            smooth: false,
            rounded: s.appearance.rounded_corners,
        }
    }
}

/// The versions of the sections [`ChartSettings`] reads.
pub fn chart_section_versions(cx: &App) -> [u64; 3] {
    let tl = TlSettings::get(cx);
    [
        tl.version(Section::Performance),
        tl.version(Section::Motion),
        tl.version(Section::Appearance),
    ]
}

/// A box chart: `ChartView::new` with the compact scale label on (TMOG), the ceiling small in
/// the plot's top-left corner.
pub fn box_chart<T: 'static>(
    spec: ChartSpec,
    mut labels: ChartLabels,
    hub: &Entity<SnapshotHub>,
    cx: &mut Context<T>,
) -> Entity<ChartView> {
    // The shared box frame draws one time axis. Individual charts must not repeat their range
    // caption, especially in the GPU and Network boxes that can contain several charts.
    labels.range_word = SharedString::default();
    cx.new(|cx| {
        let mut view = ChartView::new(spec, labels, hub, cx);
        view.set_compact_scale(true, cx);
        view
    })
}

/// The sparkline column at the right of a full spark row, logical px before `ui_scale`.
pub const SPARK_W: f32 = 96.;

/// The narrowest meter a full spark row keeps next to its sparkline, logical px before `ui_scale`.
const SPARK_MIN_METER_W: f32 = 32.;

/// The percent column of a narrow spark row ("100.0%"), logical px before `ui_scale`.
const SPARK_VALUE_W: f32 = 48.;

/// Whether a spark row `width` wide (0 before the first paint) has room for its meter: label, a
/// meter of at least `SPARK_MIN_METER_W`, value and the `SPARK_W` column, with their three gaps.
/// Narrower rows drop the meter and keep the sparkline (crw.47).
pub fn meter_fits(width: Pixels, m: &UiMetrics) -> bool {
    let fixed = (ROW_LABEL_W + SPARK_MIN_METER_W + ROW_VALUE_W + SPARK_W) * m.ui_scale;
    width == px(0.) || f32::from(width) >= fixed + 3. * f32::from(m.sp[4])
}

/// The grid of a sparkline: grid lines, no outline and no rounding.
pub const SPARK_GRID: GridStyle = GridStyle {
    lines: true,
    baseline: false,
    outline: false,
    rounded: false,
};

/// A sparkline chart for a spark row: no heading, no value, no range word, no scale label.
pub fn spark_chart<T: 'static>(
    spec: ChartSpec,
    value_fmt: ValueFormat,
    hub: &Entity<SnapshotHub>,
    cx: &mut Context<T>,
) -> Entity<ChartView> {
    let labels = ChartLabels {
        heading: None,
        value_fmt,
        value_align: ValueAlign::Hidden,
        range_word: SharedString::default(),
        legend: SmallVec::new(),
        show_axis_max: false,
    };
    cx.new(|cx| ChartView::new(spec, labels, hub, cx))
}

/// A meter row with a sparkline column at the right: label, meter, value, then `chart` in a
/// `SPARK_W` column. `None` leaves the column empty, so the meters of rows with and without a
/// sparkline line up. Without `meter` (see [`meter_fits`]) a row with a chart is label, sparkline
/// and percent, the sparkline taking the meter's place, and a row without a chart is a plain
/// meter row. The chart is a cached child view: the row builds no chart geometry.
pub fn spark_row(
    row: &MeterRow,
    chart: Option<&Entity<ChartView>>,
    meter: bool,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    let s = m.ui_scale;
    let cached =
        |chart: &Entity<ChartView>| chart.clone().cached(StyleRefinement::default().size_full());
    if !meter {
        let Some(chart) = chart else {
            return div()
                .flex_none()
                .w_full()
                .child(meter_row(row, m, theme))
                .into_any_element();
        };
        let mut label = div()
            .flex_none()
            .w(px(ROW_LABEL_W * s))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(theme.text_secondary)
            .child(row.label.clone());
        *label.text_style() = t_sm(m);
        let mut value = div()
            .flex_none()
            .w(px(SPARK_VALUE_W * s))
            .flex()
            .justify_end()
            .whitespace_nowrap()
            .text_color(theme.text_primary)
            .child(row.value.clone());
        *value.text_style() = numeric_style(m, m.text.sm);
        return div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(m.sp[4])
            .h(px(LABEL_ROW * s))
            .child(label)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h(px(LABEL_ROW * s))
                    .child(cached(chart)),
            )
            .child(value)
            .into_any_element();
    }
    let mut spark = div()
        .relative()
        .flex_none()
        .w(px(SPARK_W * s))
        .h(px(LABEL_ROW * s));
    if let Some(chart) = chart {
        spark = spark.child(cached(chart));
    }
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(m.sp[4])
        .h(px(LABEL_ROW * s))
        .child(div().flex_1().min_w_0().child(meter_row(row, m, theme)))
        .child(spark)
        .into_any_element()
}
