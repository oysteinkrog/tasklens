//! The Overview boxes' drawing parts (UI-PRF-017, D-022): a meter bar, a per-core row grid, the
//! box frame and the sparkline rows.
//!
//! [`Meter`] and [`CoreBars`] follow the custom `Element` pattern of `memory_bar.rs`
//! (UI-PRF-027): quads plus cached shaped text, no child elements. `TlProgress` is not allowed in
//! charts or cards (ADR-009). [`CoreBars`] re-formats and re-shapes a percent label only when its
//! 1% bucket changes (U-47), and a clock label only when its [`clock_bucket`] changes. The core
//! sparklines are cached chart views in [`core_sparks`], a flex grid over the painted cells.
//! [`core_board`] is the whole board; the CPU page draws it too (crw.104).

use std::cell::{Cell, RefCell};
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, BoxShadow, Context, Corners, Div, Element, ElementId,
    Entity, EntityId, FontWeight, GlobalElementId, Hsla, InspectorElementId,
    InteractiveElement as _, IntoElement, LayoutId, MouseButton, ParentElement as _, Pixels,
    SharedString, Size, Style, StyleRefinement, Styled as _, Window, canvas, div, fill, point, px,
    relative, size,
};
use rustc_hash::FxHasher;
use smallvec::SmallVec;
use tl_model::settings::Settings;

use crate::chart::labels::{self, CachedLine, LABEL_ROW, LineStyle, WARMING};
use crate::chart::{ChartLabels, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat};
use crate::components::icons::icon;
use crate::components::progress::bucket;
use crate::hub::SnapshotHub;
use crate::pages::performance::actions::PerfSelectDevice;
use crate::pages::performance::{DeviceKey, DeviceKind};
use crate::settings_global::{Section, TlSettings};
use crate::text::style::{numeric_style, t_md, t_sm, t_xs};
use crate::theme::{TlTheme, UiMetrics, current};

use super::fit::fit_text;
use super::layout::{Arrange, BoxKind};
use super::mem_box::{MeterRow, ROW_LABEL_W, ROW_VALUE_W, meter_row};
use super::range::RangeText;
use super::timeline::{self, OverviewPause};

/// The meter bar height, logical px before `ui_scale`.
pub const METER_H: f32 = 6.;

/// The gap between core cells, logical px before `ui_scale`.
const CELL_GAP: f32 = 8.;
/// The most columns the core rows flow into before they scroll (D-029).
const MAX_CORE_COLS: usize = 4;
/// The width a scrolling core grid keeps for its scrollbar, logical px before `ui_scale`.
const CORE_ROWS_BAR_W: f32 = 16.;
/// The narrowest sparkline a core cell keeps; a narrower one is left out. Logical px before
/// `ui_scale`.
const CORE_SPARK_MIN_W: f32 = 12.;
/// The core name column ("C12"), logical px before `ui_scale`.
const NAME_W: f32 = 28.;
/// The sparkline in a core cell, logical px before `ui_scale`.
pub const CORE_SPARK_W: f32 = 48.;
/// The percent column ("100%"), logical px before `ui_scale`.
const PCT_W: f32 = 36.;
/// The clock column ("3.9 GHz", "800 MHz"), logical px before `ui_scale`.
const GHZ_W: f32 = 44.;
/// The room the BogoMIPS mark (" (?)") adds to the clock column, logical px before `ui_scale`.
const CLOCK_MARK_W: f32 = 20.;
/// Marks a clock read from BogoMIPS, as the CPU page's Speed row does (PERF-033).
const BOGOMIPS_MARK: &str = " (?)";
/// The gap between the parts of a core cell, logical px before `ui_scale`.
const PART_GAP: f32 = 4.;
/// The meter width a core cell keeps while its sparkline shrinks. Logical px before `ui_scale`.
const CORE_METER_MIN_W: f32 = 16.;
/// The space above and below a core sparkline in its row, logical px before `ui_scale`.
const CORE_SPARK_INSET: f32 = 2.;
/// The core row pitch, logical px before `ui_scale`: one label row (UI-PRF-059).
const CORE_ROW_H: f32 = LABEL_ROW;
/// The narrowest core cell: the name and the percent with their gap. Logical px before
/// `ui_scale`.
const CELL_MIN_W: f32 = NAME_W + PART_GAP + PCT_W;
/// The box title row, logical px before `ui_scale`.
const TITLE_ROW: f32 = 24.;
/// The space between a box's top border and its title row, logical px before `ui_scale`
/// (`sp4`, UI-PRF-059).
pub const TITLE_TOP: f32 = 8.;
/// The two labels on the one time axis shared by every device box. The start label follows the
/// range (UI-PRF-055, [`RangeText::axis_start`]); this one shows until the page sets it.
pub const TIME_AXIS_START: &str = "1 minute ago";
pub const TIME_AXIS_END: &str = "Now";
/// The min, avg and peak line under a device box's title (UI-PRF-055), logical px before
/// `ui_scale`: one label row.
pub const FIGURES_ROW: f32 = LABEL_ROW;

/// `buckets` entry for a core with no valid sample.
const NO_BUCKET: u16 = u16::MAX;

/// The shown clock of a clock in MHz, in MHz: whole MHz under 1 GHz, else whole 0.1 GHz.
/// 812.4 MHz is 812 ("812 MHz"), 3949 MHz is 3900 ("3.9 GHz"), 3950 MHz is 4000.
pub fn clock_bucket(mhz: f32) -> u16 {
    let mhz = mhz.max(0.);
    let shown = if mhz.round() < 1000. {
        mhz.round()
    } else {
        (mhz / 100.).round() * 100.
    };
    shown.min(f32::from(NO_BUCKET - 1)) as u16
}

/// "812 MHz" or "3.9 GHz" for a [`clock_bucket`], with " (?)" when `mark`.
fn clock_text(bucket: u16, mark: bool) -> SharedString {
    let mut out = if bucket < 1000 {
        format!("{bucket} MHz")
    } else {
        format!("{}.{} GHz", bucket / 1000, bucket / 100 % 10)
    };
    if mark {
        out.push_str(BOGOMIPS_MARK);
    }
    out.into()
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static METER_QUADS: RefCell<Vec<Bounds<Pixels>>> = const { RefCell::new(Vec::new()) };
}

/// The quads every [`Meter`] painted on this thread since the last call, track first.
#[cfg(any(test, feature = "test-hooks"))]
pub fn take_meter_quads() -> Vec<Bounds<Pixels>> {
    METER_QUADS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static CORE_LABELS: RefCell<Vec<CoreLabelPaint>> = const { RefCell::new(Vec::new()) };
}

/// Where [`CoreBars`] painted one core's labels.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CoreLabelPaint {
    pub core: usize,
    /// The top left of the name.
    pub name: gpui_kit::Point<Pixels>,
    /// The right edge of the percent.
    pub pct_right: Pixels,
    pub ghz: bool,
    /// The height of the core's row.
    pub row_h: Pixels,
}

/// The core labels every [`CoreBars`] painted on this thread since the last call.
#[cfg(any(test, feature = "test-hooks"))]
pub fn take_core_labels() -> Vec<CoreLabelPaint> {
    CORE_LABELS.with(|q| std::mem::take(&mut *q.borrow_mut()))
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

/// The per-core rows' data: one name, value, percent label and clock label per logical CPU, and
/// the order the rows show in.
#[derive(Default, Debug)]
pub struct CoreBarsModel {
    /// Percent per logical CPU id; `None` is no valid sample.
    pub values: Vec<Option<f32>>,
    /// Clock in MHz per logical CPU id; `None` is no valid sample.
    pub freqs: Vec<Option<f32>>,
    /// The 1% bucket of each value, or `NO_BUCKET`.
    buckets: Vec<u16>,
    /// The [`clock_bucket`] of each clock, or `NO_BUCKET`.
    ghz_buckets: Vec<u16>,
    /// The clocks come from BogoMIPS: every clock label ends in " (?)".
    clock_mark: bool,
    /// The rows by load, highest first, rather than by CPU id.
    by_load: bool,
    /// The CPU id each row shows, top of the first column first.
    order: Vec<usize>,
    names: Vec<SharedString>,
    /// "12%", or "…" for no valid sample.
    pct_text: Vec<SharedString>,
    /// "3.9 GHz", "800 MHz", or "…" for no valid sample.
    ghz_text: Vec<SharedString>,
    name_lines: Vec<CachedLine>,
    pct_lines: Vec<CachedLine>,
    ghz_lines: Vec<CachedLine>,
    /// Paints of the element.
    #[cfg(any(test, feature = "test-hooks"))]
    pub paint_count: Cell<u32>,
    /// Lines shaped while painting.
    #[cfg(any(test, feature = "test-hooks"))]
    pub shape_count: Cell<u32>,
}

impl CoreBarsModel {
    /// Sets the CPU names ("C0", "C1", …). Every value and clock goes back to no valid sample.
    pub fn set_cpus(&mut self, names: impl Iterator<Item = SharedString>) {
        self.names.clear();
        self.names.extend(names);
        let n = self.names.len();
        let warming = SharedString::new_static(WARMING);
        self.values.clear();
        self.values.resize(n, None);
        self.freqs.clear();
        self.freqs.resize(n, None);
        self.buckets.clear();
        self.buckets.resize(n, NO_BUCKET);
        self.ghz_buckets.clear();
        self.ghz_buckets.resize(n, NO_BUCKET);
        self.pct_text.clear();
        self.pct_text.resize(n, warming.clone());
        self.ghz_text.clear();
        self.ghz_text.resize(n, warming);
        self.name_lines.resize_with(n, CachedLine::default);
        self.pct_lines.resize_with(n, CachedLine::default);
        self.ghz_lines.resize_with(n, CachedLine::default);
        self.order.clear();
        self.order.extend(0..n);
    }

    /// The number of CPUs.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The CPU id each row shows, in row order.
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// Orders the rows by load, highest first, or by CPU id. True when the order changed.
    pub fn set_by_load(&mut self, by_load: bool) -> bool {
        if by_load == self.by_load {
            return false;
        }
        self.by_load = by_load;
        self.sort()
    }

    /// Sorts the rows: by CPU id, or by the shown percent, highest first, then by CPU id. A CPU
    /// without a valid sample goes last. True when the order changed.
    fn sort(&mut self) -> bool {
        let before = self.order.clone();
        let buckets = &self.buckets;
        if self.by_load {
            let load = |i: usize| match buckets[i] {
                NO_BUCKET => -1,
                b => i32::from(b),
            };
            self.order
                .sort_unstable_by(|&a, &b| load(b).cmp(&load(a)).then(a.cmp(&b)));
        } else {
            self.order.sort_unstable();
        }
        self.order != before
    }

    /// Whether the clocks come from BogoMIPS.
    pub fn clock_mark(&self) -> bool {
        self.clock_mark
    }

    /// Marks every clock " (?)" when it comes from BogoMIPS (PERF-033). True when that changed;
    /// the clock labels are formatted again.
    pub fn set_clock_mark(&mut self, mark: bool) -> bool {
        if mark == self.clock_mark {
            return false;
        }
        self.clock_mark = mark;
        for (b, text) in self.ghz_buckets.iter().zip(&mut self.ghz_text) {
            if *b != NO_BUCKET {
                *text = clock_text(*b, mark);
            }
        }
        true
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
        if changed && self.by_load {
            self.sort();
        }
        changed
    }

    /// Takes one clock in MHz per CPU; missing clocks count as no valid sample. True when a
    /// [`clock_bucket`] or a None/Some state changed. Re-formats only those labels ("3.9 GHz",
    /// "800 MHz").
    pub fn set_freqs(&mut self, mhz: impl Iterator<Item = Option<f32>>) -> bool {
        let mut mhz = mhz;
        let mut changed = false;
        for i in 0..self.names.len() {
            let f = mhz.next().flatten();
            self.freqs[i] = f;
            let b = f.map_or(NO_BUCKET, clock_bucket);
            if b == self.ghz_buckets[i] {
                continue;
            }
            self.ghz_buckets[i] = b;
            self.ghz_text[i] = if b == NO_BUCKET {
                SharedString::new_static(WARMING)
            } else {
                clock_text(b, self.clock_mark)
            };
            changed = true;
        }
        changed
    }

    /// CPU `i`'s name ("C3"), empty for an unknown CPU.
    pub fn name(&self, i: usize) -> &str {
        self.names.get(i).map_or("", |n| n)
    }

    /// CPU `i`'s percent label ("12%"), or "…" for no valid sample.
    pub fn pct_label(&self, i: usize) -> &str {
        self.pct_text.get(i).map_or(WARMING, |t| t)
    }

    /// The clock label of each CPU: "3.9 GHz", "800 MHz", or "…" for no valid sample.
    pub fn clock_labels(&self) -> &[SharedString] {
        &self.ghz_text
    }

    /// "C0\t12%" per CPU, one per line, with "\t3.9 GHz" when the clock is known.
    pub fn copy_text(&self, out: &mut String) {
        for i in 0..self.names.len() {
            out.push_str(&self.names[i]);
            out.push('\t');
            out.push_str(&self.pct_text[i]);
            if self.ghz_buckets[i] != NO_BUCKET {
                out.push('\t');
                out.push_str(&self.ghz_text[i]);
            }
            out.push('\n');
        }
    }
}

/// A grid of per-core rows, as large as its parent. Each cell is a name, room for the sparkline
/// ([`core_sparks`] draws it), a meter, a percent and, when there is room, a GHz label.
pub struct CoreBars {
    pub model: Rc<RefCell<CoreBarsModel>>,
}

/// How a core cell shares its width (D-029). The GHz label goes first, then the sparkline
/// shrinks, then the meter; the name and the percent always show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoreCellFit {
    /// Name, 48 px sparkline, meter, percent and GHz.
    Full,
    /// Name, 48 px sparkline, meter and percent.
    NoGhz,
    /// Name, a sparkline under 48 px, a 16 px meter and percent.
    SparkShrinks,
    /// Name, a 12 px sparkline, a meter under 16 px (or none) and percent.
    MeterShrinks,
    /// Name, a meter under 16 px (or none) and percent.
    NoSpark,
}

/// Where the core cells go.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CoreBarsLayout {
    pub cols: usize,
    pub rows: usize,
    pub cell_w: Pixels,
    pub row_h: Pixels,
    /// The clock column, wider when the clocks carry the BogoMIPS mark.
    pub clock_w: Pixels,
    /// True when 18 px rows do not fit in 4 columns: the rows scroll under a scrollbar.
    pub scrolls: bool,
    pub fit: CoreCellFit,
}

impl CoreBarsLayout {
    /// The layout class for `class_probe`: what the sparkline grid depends on. Cell width and row
    /// height inside one class follow the flex layout.
    pub fn class(&self) -> u32 {
        (self.cols.min(0x3ff) as u32)
            | ((self.rows.min(0xfff) as u32) << 10)
            | (u32::from(self.scrolls) << 23)
            | ((self.fit as u32) << 24)
    }

    /// The column and row of core `i`: the rows fill the first column top to bottom, then the
    /// next (C0 to C15, then C16 to C31 for 32 CPUs in 2 columns).
    pub fn cell_of(&self, i: usize) -> (usize, usize) {
        let rows = self.rows.max(1);
        (i / rows, i % rows)
    }

    /// The height of all rows: what a scrolling grid scrolls through.
    pub fn content_h(&self) -> Pixels {
        self.row_h * self.rows as f32
    }
}

/// The width a core cell needs for every part: name, sparkline, a 16 px meter, percent and GHz
/// with their 4 gaps, logical px before `ui_scale`.
const CORE_FULL_W: f32 = NAME_W + CORE_SPARK_W + CORE_METER_MIN_W + PCT_W + GHZ_W + 4. * PART_GAP;
/// The same without the GHz label and its gap.
const CORE_NO_GHZ_W: f32 = CORE_FULL_W - GHZ_W - PART_GAP;
/// The cell width right of a shrinking sparkline: a gap, the 16 px meter, a gap and the percent.
const CORE_SPARK_TAIL_W: f32 = PART_GAP + CORE_METER_MIN_W + PART_GAP + PCT_W;
/// The narrowest cell that keeps a sparkline: name, the 12 px sparkline, no meter and the
/// percent with their 3 gaps.
const CORE_SPARK_MIN_CELL_W: f32 = NAME_W + CORE_SPARK_MIN_W + PCT_W + 3. * PART_GAP;

/// The grid for `n` cores in `w × h` (D-029, UI-PRF-059): one column of 18 px rows, or the
/// fewest equal columns (at most 4) that fit them. When 18 px rows do not fit in 4 columns the
/// rows stay 18 px and scroll, with room for the scrollbar. A cell never gets narrower than its
/// name and percent, so a narrow part takes fewer columns and scrolls sooner.
pub fn core_bars_layout(n: usize, w: Pixels, h: Pixels, scale: f32) -> CoreBarsLayout {
    core_bars_layout_marked(n, w, h, scale, false)
}

/// [`core_bars_layout`] with a clock column that has room for the BogoMIPS mark when `mark`.
pub fn core_bars_layout_marked(
    n: usize,
    w: Pixels,
    h: Pixels,
    scale: f32,
    mark: bool,
) -> CoreBarsLayout {
    let n = n.max(1);
    let mark_w = if mark { CLOCK_MARK_W } else { 0. };
    let gap = CELL_GAP * scale;
    let (w, h) = (f32::from(w).max(0.), f32::from(h).max(0.));
    let row_h = CORE_ROW_H * scale;
    let max_cols = |w: f32| {
        (((w + gap) / (CELL_MIN_W * scale + gap)).floor() as usize).clamp(1, MAX_CORE_COLS)
    };
    let fits = |cols: usize| n.div_ceil(cols) as f32 * row_h <= h + 1e-3;
    let (w, cols, scrolls) = match (1..=max_cols(w)).find(|&c| fits(c)) {
        Some(cols) => (w, cols, false),
        None => {
            let w = (w - CORE_ROWS_BAR_W * scale).max(0.);
            (w, max_cols(w), true)
        }
    };
    let rows = n.div_ceil(cols);
    // The fewest columns for those rows, so no column is left empty.
    let cols = n.div_ceil(rows);
    let cell_w = ((w - gap * (cols - 1) as f32) / cols as f32).max(0.);
    let fit = if cell_w >= (CORE_FULL_W + mark_w) * scale {
        CoreCellFit::Full
    } else if cell_w >= CORE_NO_GHZ_W * scale {
        CoreCellFit::NoGhz
    } else if cell_w >= (NAME_W + PART_GAP + CORE_SPARK_MIN_W + CORE_SPARK_TAIL_W) * scale {
        CoreCellFit::SparkShrinks
    } else if cell_w >= CORE_SPARK_MIN_CELL_W * scale {
        CoreCellFit::MeterShrinks
    } else {
        CoreCellFit::NoSpark
    };
    CoreBarsLayout {
        cols,
        rows,
        cell_w: px(cell_w),
        row_h: px(row_h),
        clock_w: px((GHZ_W + mark_w) * scale),
        scrolls,
        fit,
    }
}

/// The x offsets of a core cell's parts from the cell's left edge.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CoreCell {
    pub spark_x: Pixels,
    /// 0 when the cell has no sparkline.
    pub spark_w: Pixels,
    pub meter_x: Pixels,
    /// 0 when the cell has no room for a meter.
    pub meter_w: Pixels,
    /// The right edge of the percent label.
    pub pct_right: Pixels,
    /// Whether the GHz label shows; it ends at the cell's right edge.
    pub ghz: bool,
}

/// The parts of a cell of `l`, in the order name, sparkline, meter, percent, GHz.
pub fn core_cell(l: &CoreBarsLayout, scale: f32) -> CoreCell {
    let cell = l.cell_w;
    let gap = px(PART_GAP * scale);
    let spark_x = px(NAME_W * scale) + gap;
    let ghz = l.fit == CoreCellFit::Full;
    let pct_right = if ghz { cell - l.clock_w - gap } else { cell };
    // The meter ends one gap before the percent column.
    let meter_end = pct_right - px(PCT_W * scale) - gap;
    let (spark_w, meter_x) = match l.fit {
        CoreCellFit::Full | CoreCellFit::NoGhz => {
            let spark_w = px(CORE_SPARK_W * scale);
            (spark_w, spark_x + spark_w + gap)
        }
        CoreCellFit::SparkShrinks => {
            let meter_x = meter_end - px(CORE_METER_MIN_W * scale);
            ((meter_x - gap - spark_x).max(px(0.)), meter_x)
        }
        CoreCellFit::MeterShrinks => {
            let spark_w = px(CORE_SPARK_MIN_W * scale);
            (spark_w, spark_x + spark_w + gap)
        }
        CoreCellFit::NoSpark => (px(0.), spark_x),
    };
    CoreCell {
        spark_x,
        spark_w,
        meter_x,
        meter_w: (meter_end - meter_x).max(px(0.)),
        pct_right,
        ghz,
    }
}

/// The debug selector of core `i`'s sparkline.
pub fn core_spark_id(i: usize) -> String {
    format!("perf-overview-core-spark-{i}")
}

/// The debug selector of the core rows' scrolling viewport.
pub const CORE_ROWS_SCROLL_ID: &str = "perf-overview-core-rows-scroll";

/// The sparklines over a [`CoreBars`] grid laid out as `l`, one cached chart per core in
/// `charts`, row `k` showing core `order[k]`. A flex grid of the same columns, rows and gaps,
/// with the rows in the same column-first order, so each sparkline sits in its painted cell at
/// any size inside `l`'s class. Empty when the cells have no room for a sparkline.
pub fn core_sparks(
    charts: &[Entity<ChartView>],
    order: &[usize],
    l: &CoreBarsLayout,
    m: &UiMetrics,
) -> Div {
    let s = m.ui_scale;
    let mut grid = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .flex_col();
    if l.fit == CoreCellFit::NoSpark || charts.is_empty() {
        return grid;
    }
    let gap = px(PART_GAP * s);
    let left = px(NAME_W * s) + gap;
    let inset = px(CORE_SPARK_INSET * s);
    let tail = px(CORE_SPARK_TAIL_W * s);
    for r in 0..l.rows {
        let mut row = div()
            .flex()
            .flex_row()
            .w_full()
            .gap(px(CELL_GAP * s))
            .flex_none()
            .h(l.row_h);
        for c in 0..l.cols {
            let slot = c * l.rows + r;
            let mut cell = div().relative().flex_1().min_w_0().h_full();
            let core = order.get(slot).and_then(|&i| Some((i, charts.get(i)?)));
            if let Some((i, chart)) = core {
                let spark = div()
                    .debug_selector(move || core_spark_id(i))
                    .absolute()
                    .top(inset)
                    .bottom(inset)
                    .left(left);
                let spark = match l.fit {
                    CoreCellFit::SparkShrinks => spark.right(tail),
                    CoreCellFit::MeterShrinks => spark.w(px(CORE_SPARK_MIN_W * s)),
                    _ => spark.w(px(CORE_SPARK_W * s)),
                };
                cell = cell.child(
                    spark.child(chart.clone().cached(StyleRefinement::default().size_full())),
                );
            }
            row = row.child(cell);
        }
        grid = grid.child(row);
    }
    grid
}

/// The core rows laid out as `l`: [`CoreBars`] with its sparklines over it, filling the part.
/// When the rows scroll they are as high as all rows, in a viewport with a vertical scrollbar.
pub fn core_rows(
    model: &Rc<RefCell<CoreBarsModel>>,
    charts: &[Entity<ChartView>],
    l: &CoreBarsLayout,
    m: &UiMetrics,
) -> AnyElement {
    let sparks = core_sparks(charts, model.borrow().order(), l, m);
    let rows = div()
        .relative()
        .child(CoreBars {
            model: Rc::clone(model),
        })
        .child(sparks);
    if !l.scrolls {
        return rows.size_full().into_any_element();
    }
    // The rows leave the scrollbar's width free at the right, as `core_bars_layout` counts it.
    let content = div()
        .flex()
        .flex_row()
        .flex_none()
        .w_full()
        .h(l.content_h())
        .child(rows.flex_1().min_w_0().h_full())
        .child(div().flex_none().w(px(CORE_ROWS_BAR_W * m.ui_scale)));
    // The selector sits on a plain wrapper: the scrollable's own element is its content.
    div()
        .size_full()
        .debug_selector(|| CORE_ROWS_SCROLL_ID.into())
        .child(
            div()
                .size_full()
                .child(content)
                .overflow_y_scrollbar()
                .id(CORE_ROWS_SCROLL_ID),
        )
        .into_any_element()
}

/// The core board (crw.104): [`core_rows`] filling the part, and the probe that renders `view`
/// again when the part's size moves the grid into another class. `grid_size` is the part's size
/// at the last paint. Returns the board and the layout it used.
pub fn core_board(
    model: &Rc<RefCell<CoreBarsModel>>,
    charts: &[Entity<ChartView>],
    grid_size: &Rc<Cell<Size<Pixels>>>,
    m: &UiMetrics,
    view: EntityId,
) -> (AnyElement, CoreBarsLayout) {
    let (n, mark) = {
        let model = model.borrow();
        (model.len(), model.clock_mark())
    };
    let s = m.ui_scale;
    let size = grid_size.get();
    let l = core_bars_layout_marked(n, size.width, size.height, s, mark);
    let board = div()
        .relative()
        .size_full()
        .child(core_rows(model, charts, &l, m))
        .child(class_probe(
            Rc::clone(grid_size),
            l.class(),
            move |size| core_bars_layout_marked(n, size.width, size.height, s, mark).class(),
            view,
        ))
        .into_any_element();
    (board, l)
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
        let l = core_bars_layout_marked(
            n,
            bounds.size.width,
            bounds.size.height,
            s,
            model.clock_mark,
        );
        let parts = core_cell(&l, s);
        let gap = px(CELL_GAP * s);
        let bar_h = px(METER_H * s);
        let name_style = LineStyle {
            size: m.text.xs,
            weight: FontWeight::NORMAL,
            color: theme.text_secondary,
            tnum: false,
        };
        let num_style = LineStyle {
            tnum: true,
            ..name_style
        };
        for slot in 0..n {
            let i = model.order.get(slot).copied().unwrap_or(slot);
            let (col, row) = l.cell_of(slot);
            let x = bounds.left() + (l.cell_w + gap) * col as f32;
            let y = bounds.top() + l.row_h * row as f32;
            if parts.meter_w > px(0.) {
                let bar = Bounds::new(
                    point(x + parts.meter_x, y + (l.row_h - bar_h) / 2.),
                    size(parts.meter_w, bar_h),
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
            }
            let name = &mut model.name_lines[i];
            let mut shaped = u32::from(name.sync(window, &model.names[i], name_style));
            labels::paint_line(window, cx, name, point(x, y), l.row_h);
            let pct = &mut model.pct_lines[i];
            shaped += u32::from(pct.sync(window, &model.pct_text[i], num_style));
            let pct_x = x + parts.pct_right - pct.width();
            labels::paint_line(window, cx, pct, point(pct_x, y), l.row_h);
            if parts.ghz {
                let ghz = &mut model.ghz_lines[i];
                shaped += u32::from(ghz.sync(window, &model.ghz_text[i], num_style));
                let ghz_x = x + l.cell_w - ghz.width();
                labels::paint_line(window, cx, ghz, point(ghz_x, y), l.row_h);
            }
            #[cfg(any(test, feature = "test-hooks"))]
            {
                model.shape_count.set(model.shape_count.get() + shaped);
                CORE_LABELS.with(|q| {
                    q.borrow_mut().push(CoreLabelPaint {
                        core: i,
                        name: point(x, y),
                        pct_right: x + parts.pct_right,
                        ghz: parts.ghz,
                        row_h: l.row_h,
                    });
                });
            }
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

/// Whether Edit Layout mode is on, so every box frame leaves the right end of its title line to
/// the gear and the hide button (D-029). Set by the page when the mode turns on or off.
#[derive(Clone, Copy, Default)]
pub struct EditChrome(pub bool);

impl gpui_kit::Global for EditChrome {}

/// The right end of a title line that Edit Layout keeps free for the gear and the hide button,
/// logical px before `ui_scale`: the 16 px hide button, a 4 px gap, the gear button (24 px) and a
/// 4 px gap.
pub const EDIT_TITLE_RESERVE: f32 = 48.;

/// The left end of a title line that Edit Layout keeps free for the 16 px grip, logical px before
/// `ui_scale`, measured from the box's left edge (UI-PRF-018).
pub const EDIT_TITLE_LEAD: f32 = 20.;

/// Whether the box frames reserve the edit controls' space now.
pub fn edit_chrome(cx: &App) -> bool {
    cx.try_global::<EditChrome>().is_some_and(|e| e.0)
}

/// The debug selector of a box's title row: the box id with `-title`.
pub fn title_row_id(box_id: &str) -> String {
    format!("{box_id}-title")
}

/// The debug selector of a box's headline value in the title row: the box id with `-title-value`.
pub fn title_value_id(box_id: &str) -> String {
    format!("{box_id}-title-value")
}

/// The debug selector of a box's one-line summary in its title line: the box id with
/// `-title-line`.
pub fn title_line_id(box_id: &str) -> String {
    format!("{box_id}-title-line")
}

/// The debug selector of a box's device icon and title at the left of its title line.
pub fn title_name_id(box_id: &str) -> String {
    format!("{box_id}-title-name")
}

/// The debug selector of the one time axis below a device box's charts.
pub fn time_axis_id(box_id: &str) -> String {
    format!("{box_id}-time-axis")
}

/// The debug selector of a device box's left or right time-axis label.
pub fn time_axis_label_id(box_id: &str, end: &str) -> String {
    format!("{box_id}-time-axis-{end}")
}

/// The debug selector of a device box's min, avg and peak line under its title: the box id with
/// `-figures`.
pub fn figures_id(box_id: &str) -> String {
    format!("{box_id}-figures")
}

/// The debug selector of a box's device icon in its title line: the box id with `-title-icon`.
pub fn title_icon_id(box_id: &str) -> String {
    format!("{box_id}-title-icon")
}

/// The title value style (UI-PRF-058): `t_lg` (16 px), weight 600, tabular figures.
pub fn title_value_style(m: &UiMetrics) -> gpui_kit::TextStyleRefinement {
    gpui_kit::TextStyleRefinement {
        font_weight: Some(gpui_kit::FontWeight::SEMIBOLD),
        ..numeric_style(m, m.text.lg)
    }
}

/// The 20 px device icon in a box's title line, tinted in the device colour (UI-PRF-058).
fn box_icon(id: &'static str, m: &UiMetrics, theme: &TlTheme) -> Option<impl IntoElement> {
    let kind = BoxKind::from_frame_id(id)?;
    let device = match kind {
        BoxKind::Cpu | BoxKind::TopProcs => DeviceKind::Cpu,
        BoxKind::Memory => DeviceKind::Memory,
        BoxKind::Gpu => DeviceKind::Gpu,
        BoxKind::Disk => DeviceKind::Disk,
        BoxKind::Net => DeviceKind::Net,
        // The timeline shows every device: no device icon.
        BoxKind::Timeline => return None,
    };
    let name = device.icon()?;
    Some(
        div()
            .debug_selector(move || title_icon_id(id))
            .flex_none()
            .child(icon(name, m.icon.lg, device.icon_color(theme))),
    )
}

/// The one accent used by an Overview box frame and all of that device's data marks.
pub fn box_accent(box_id: &str, theme: &TlTheme) -> Hsla {
    let kind = BoxKind::from_frame_id(box_id)
        .unwrap_or_else(|| panic!("unknown Overview box frame id {box_id:?}"));
    theme.graph.device_accent(kind.accent())
}

/// The box shadow (UI-PRF-058): the menu-level shadow of UI-FND-003, offset (0, 2), blur 8,
/// colour `derived.shadow`, which already scales with `appearance.shadow_intensity_pct`. `None`
/// at intensity 0.
pub fn box_shadow(theme: &TlTheme, ui_scale: f32) -> Option<BoxShadow> {
    (theme.derived.shadow.a > 0.0).then(|| BoxShadow {
        color: theme.derived.shadow,
        offset: point(px(0.), px(2. * ui_scale)),
        blur_radius: px(8. * ui_scale),
        spread_radius: px(0.),
        inset: false,
    })
}

/// The 1 px inner highlight of a box: a ring just inside the accent frame, white at 6%. It is
/// laid over the box and takes no pointer events.
fn box_highlight(m: &UiMetrics) -> Div {
    div()
        .absolute()
        .inset_0()
        .border_1()
        .border_color(gpui_kit::white().alpha(HIGHLIGHT_ALPHA))
        .rounded((m.radius_card - px(1.)).max(px(0.)))
        .debug_selector(|| "BoxHighlight".into())
}

/// Alpha of the inner highlight ring.
const HIGHLIGHT_ALPHA: f32 = 0.06;

/// One Overview box: a `list_bg` card with a thin rounded device-accent frame. Its 24 px title
/// line sits inside the border, `TITLE_TOP` below it (UI-PRF-059): the icon and title, a one-line
/// summary that ellipsizes and the headline value at the right. Title and body are 12 px in
/// from the left and right borders, and the body ends 12 px above the bottom border. A device box has the range figures under it and the time axis
/// below `body`, both from `range` (UI-PRF-055). `body` fills the rest. A click on the title
/// line opens `target`'s page.
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
    range: &RangeText,
    body: AnyElement,
    edit: bool,
    m: &UiMetrics,
    theme: &TlTheme,
) -> Div {
    let accent = box_accent(id, theme);
    // The title value and summary end before the edit controls (D-029).
    let title_pad_right = if edit {
        (px(EDIT_TITLE_RESERVE * m.ui_scale) - m.sp[5]).max(px(0.))
    } else {
        px(0.)
    };
    let title_pad_left = if edit {
        (px(EDIT_TITLE_LEAD * m.ui_scale) - m.sp[5]).max(px(0.))
    } else {
        px(0.)
    };
    let draws_time_axis = !matches!(
        BoxKind::from_frame_id(id),
        Some(BoxKind::TopProcs | BoxKind::Timeline)
    );
    // Each text style goes in first: it replaces the whole text refinement, so the nowrap,
    // ellipsis and colour set after it would otherwise be lost.
    let mut title_el = div();
    *title_el.text_style() = t_md(m);
    let title_el = title_el
        .debug_selector(move || title_name_id(id))
        .h_full()
        .flex()
        .items_center()
        .flex_none()
        .whitespace_nowrap()
        .gap(m.sp[3])
        .text_color(theme.text_primary)
        .children(box_icon(id, m, theme))
        .child(title);
    let mut line_el = div();
    *line_el.text_style() = t_sm(m);
    let line_el = line_el
        .debug_selector(move || title_line_id(id))
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(line);
    let mut value_el = div();
    *value_el.text_style() = title_value_style(m);
    let value_el = value_el
        .debug_selector(move || title_value_id(id))
        .flex()
        .min_w_0()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(fit_text(value));
    let mut frame = div()
        .debug_selector(move || id.into())
        .relative()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(theme.list_bg)
        .border_1()
        .border_color(accent)
        .rounded(m.radius_card);
    if let Some(shadow) = box_shadow(theme, m.ui_scale) {
        frame.style().box_shadow = Some(vec![shadow]);
    }
    frame
        .child(opens_device(
            div()
                .debug_selector(move || title_row_id(id))
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[4])
                .h(px(TITLE_ROW * m.ui_scale))
                .mt(px(TITLE_TOP * m.ui_scale))
                .mx(m.sp[5])
                .pl(title_pad_left)
                .pr(title_pad_right)
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
                let start = if range.axis_start.is_empty() {
                    SharedString::from(TIME_AXIS_START)
                } else {
                    range.axis_start.clone()
                };
                let mut ago = div()
                    .debug_selector(move || time_axis_label_id(id, "start"))
                    .child(start);
                *ago.text_style() = t_xs(m);
                let mut now = div()
                    .debug_selector(move || time_axis_label_id(id, "end"))
                    .child(TIME_AXIS_END);
                *now.text_style() = t_xs(m);
                let mut figures = div();
                *figures.text_style() = t_xs(m);
                let figures = figures
                    .debug_selector(move || figures_id(id))
                    .flex_none()
                    .h(px(FIGURES_ROW * m.ui_scale))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(theme.text_secondary)
                    .child(range.figures.clone());
                body_el = body_el
                    .flex()
                    .flex_col()
                    .child(figures)
                    // Clipped, so nothing draws in the time-axis band (UI-PRF-017).
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .overflow_hidden()
                            .child(body),
                    )
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
        // Last, so the ring paints over the title row and the body.
        .child(box_highlight(m))
}

/// Box widths from here up put the chart and its side part next to each other; narrower boxes
/// stack them. Logical px before `ui_scale`.
pub const SPLIT_W: f32 = 560.;

/// The chart's share of a wide box's body.
const CHART_SHARE: f32 = 0.55;

/// The CPU box chart's share of a wide body (crw.53), so 2 columns of core rows fit beside it.
pub const CPU_CHART_SHARE: f32 = 0.40;

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

/// A grid of equal cells that fills its area (UI-PRF-019, D-026), from [`fill_grid`] or
/// [`arrange_grid`].
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct GridFit {
    pub cols: usize,
    pub rows: usize,
    /// Every cell's size, in whole px.
    pub cell: Size<Pixels>,
    /// The cells do not fit at their minimum height, so the grid scrolls vertically.
    pub scrolls: bool,
    /// The cells of one row do not fit at their minimum width, so the row scrolls sideways
    /// (D-027 "Side by side").
    pub scrolls_x: bool,
}

impl GridFit {
    /// The height of every row with `gap` between them.
    pub fn content_h(&self, gap: Pixels) -> Pixels {
        if self.rows == 0 {
            return px(0.);
        }
        self.cell.height * self.rows as f32 + gap * (self.rows - 1) as f32
    }

    /// The width of every column with `gap` between them.
    pub fn content_w(&self, gap: Pixels) -> Pixels {
        if self.cols == 0 {
            return px(0.);
        }
        self.cell.width * self.cols as f32 + gap * (self.cols - 1) as f32
    }

    /// A layout class for [`class_probe`]: it changes when the columns, the rows, the scrolling
    /// or the cell size change. Cell sizes are whole px, so a sub-pixel resize keeps the class.
    pub fn class(&self) -> u32 {
        let mut h = FxHasher::default();
        (self.cols, self.rows, self.scrolls, self.scrolls_x).hash(&mut h);
        (f32::from(self.cell.width) as u32).hash(&mut h);
        (f32::from(self.cell.height) as u32).hash(&mut h);
        h.finish() as u32
    }
}

/// The columns of `n` cells of at least `min_w` that fit a row `width` wide with `gap` between
/// them: at least 1 and at most `n`.
fn grid_max_cols(n: usize, width: f32, min_w: f32, gap: f32) -> usize {
    if min_w + gap <= 0. {
        return n.max(1);
    }
    (((width + gap) / (min_w + gap)).floor().max(1.) as usize).min(n.max(1))
}

/// The lowest grid of `n` cells of at least `min` in a row `width` wide, `gap` apart: as many
/// columns as fit, rows at the minimum height.
pub fn grid_min_h(n: usize, width: Pixels, min: Size<Pixels>, gap: Pixels) -> Pixels {
    if n == 0 {
        return px(0.);
    }
    let cols = grid_max_cols(n, f32::from(width), f32::from(min.width), f32::from(gap));
    let rows = n.div_ceil(cols);
    min.height * rows as f32 + gap * (rows - 1) as f32
}

/// The lowest `n` cells of at least `min` need in a row `width` wide when placed as `arrange`
/// says: [`grid_min_h`] for `Auto`, `n` cells for `Vertical`, one cell for `Horizontal`.
pub fn arrange_min_h(
    arrange: Arrange,
    n: usize,
    width: Pixels,
    min: Size<Pixels>,
    gap: Pixels,
) -> Pixels {
    match arrange {
        Arrange::Auto => grid_min_h(n, width, min, gap),
        Arrange::Vertical if n > 0 => min.height * n as f32 + gap * (n - 1) as f32,
        Arrange::Vertical | Arrange::Horizontal => {
            if n == 0 {
                px(0.)
            } else {
                min.height
            }
        }
    }
}

/// Lays out `n` equal cells in `area`, `gap` apart (UI-PRF-019, D-026). Of the column counts
/// whose cells are at least `min`, it takes the one with the largest cells. When no column
/// count fits the minimum height, the grid scrolls: it keeps `bar` px at the right for the
/// scrollbar, takes as many columns as fit the width, and gives the cells the minimum height.
/// A row narrower than one minimum cell has one column as wide as the row. Sizes are floored to
/// whole px, so every cell is the same size.
pub fn fill_grid(
    n: usize,
    area: Size<Pixels>,
    min: Size<Pixels>,
    gap: Pixels,
    bar: Pixels,
) -> GridFit {
    if n == 0 {
        return GridFit::default();
    }
    let (w, h, g) = (
        f32::from(area.width),
        f32::from(area.height),
        f32::from(gap),
    );
    let (min_w, min_h) = (f32::from(min.width), f32::from(min.height));
    let cell_w = |w: f32, cols: usize| {
        ((w - g * (cols - 1) as f32) / cols as f32 + 1e-3)
            .floor()
            .max(0.)
    };
    let mut best: Option<(f32, GridFit)> = None;
    for cols in 1..=grid_max_cols(n, w, min_w, g) {
        let rows = n.div_ceil(cols);
        let raw = (h - g * (rows - 1) as f32) / rows as f32;
        if raw + 1e-3 < min_h {
            continue;
        }
        let ch = (raw + 1e-3).floor();
        let cw = cell_w(w, cols);
        let size_now = cw * ch;
        if best.is_none_or(|(a, _)| size_now > a) {
            let fit = GridFit {
                cols,
                rows,
                cell: size(px(cw), px(ch)),
                scrolls: false,
                scrolls_x: false,
            };
            best = Some((size_now, fit));
        }
    }
    if let Some((_, fit)) = best {
        return fit;
    }
    let w = (w - f32::from(bar)).max(0.);
    let cols = grid_max_cols(n, w, min_w, g);
    GridFit {
        cols,
        rows: n.div_ceil(cols),
        cell: size(px(cell_w(w, cols)), px(min_h.floor())),
        scrolls: true,
        scrolls_x: false,
    }
}

/// `n` equal cells in one line `along` px long and `across` px wide, `gap` apart: each cell's
/// length and width in whole px, and whether the line scrolls. A cell gets an equal share of
/// the length, at least `min`; when the cells do not fit at `min` the line scrolls and keeps
/// `bar` px of its width for the scrollbar.
fn line_fit(n: usize, along: f32, across: f32, min: f32, gap: f32, bar: f32) -> (f32, f32, bool) {
    let raw = (along - gap * (n - 1) as f32) / n as f32;
    if raw + 1e-3 >= min {
        ((raw + 1e-3).floor(), across.floor().max(0.), false)
    } else {
        (min.floor(), (across - bar).floor().max(0.), true)
    }
}

/// Lays out `n` equal cells in `area` as `arrange` says (UI-PRF-019 "Arrange", D-027).
/// `Auto` is the mode's grid, [`fill_grid`]. `Vertical` is one column as wide as the area: each
/// cell an equal share of the height and at least `min.height`; when they do not fit, the
/// column scrolls vertically and keeps `bar` px at the right for the scrollbar. `Horizontal` is
/// one row as high as the area, each cell at least `min.width`; when they do not fit, the row
/// scrolls sideways and keeps `bar` px at the bottom.
pub fn arrange_grid(
    arrange: Arrange,
    n: usize,
    area: Size<Pixels>,
    min: Size<Pixels>,
    gap: Pixels,
    bar: Pixels,
) -> GridFit {
    if n == 0 {
        return GridFit::default();
    }
    let (w, h, g, b) = (
        f32::from(area.width),
        f32::from(area.height),
        f32::from(gap),
        f32::from(bar),
    );
    match arrange {
        Arrange::Auto => fill_grid(n, area, min, gap, bar),
        Arrange::Vertical => {
            let (ch, cw, scrolls) = line_fit(n, h, w, f32::from(min.height), g, b);
            GridFit {
                cols: 1,
                rows: n,
                cell: size(px(cw), px(ch)),
                scrolls,
                scrolls_x: false,
            }
        }
        Arrange::Horizontal => {
            let (cw, ch, scrolls_x) = line_fit(n, w, h, f32::from(min.width), g, b);
            GridFit {
                cols: n,
                rows: 1,
                cell: size(px(cw), px(ch)),
                scrolls: false,
                scrolls_x,
            }
        }
    }
}

/// The cells of a box list with a "+n more" line (UI-PRF-059): the grid and how many cells it
/// shows.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct ListFit {
    pub grid: GridFit,
    pub shown: usize,
}

impl ListFit {
    /// A layout class for [`class_probe`]: the grid's class and the number of cells shown.
    pub fn class(&self) -> u32 {
        let mut h = FxHasher::default();
        (self.grid.class(), self.shown).hash(&mut h);
        h.finish() as u32
    }
}

/// Lays out `n` cells of a list of `total` items in `body`, as `arrange` says, showing only
/// whole cells (UI-PRF-059). When `total > n`, or when some cells do not fit at `min`, the
/// "+n more" line takes `more_h` from the body first. Cells that do not fit whole are left out
/// and counted in "+n more", so the grid never scrolls. Before the first paint the body has no
/// size at all and every cell shows; a painted body 0 px high shows none.
pub fn list_fit(
    arrange: Arrange,
    n: usize,
    total: usize,
    body: Size<Pixels>,
    min: Size<Pixels>,
    gap: Pixels,
    more_h: Pixels,
) -> ListFit {
    let less = |by: Pixels| size(body.width, (body.height - by).max(px(0.)));
    let more = if total > n { more_h } else { px(0.) };
    let grid = arrange_grid(arrange, n, less(more), min, gap, px(0.));
    let unpainted = body.width == px(0.) && body.height == px(0.);
    if unpainted || (!grid.scrolls && !grid.scrolls_x) {
        return ListFit { grid, shown: n };
    }
    let area = less(more_h);
    let (w, h, g) = (
        f32::from(area.width),
        f32::from(area.height),
        f32::from(gap),
    );
    let (min_w, min_h) = (f32::from(min.width), f32::from(min.height));
    let fits = |len: f32, min: f32| ((len + g) / (min + g) + 1e-3).floor().max(0.) as usize;
    let cap = match arrange {
        Arrange::Auto => grid_max_cols(n, w, min_w, g) * fits(h, min_h),
        Arrange::Vertical => fits(h, min_h),
        Arrange::Horizontal => fits(w, min_w),
    };
    let shown = cap.min(n);
    ListFit {
        grid: arrange_grid(arrange, shown, area, min, gap, px(0.)),
        shown,
    }
}

/// `cells` in rows of `fit.cols`, `gap` apart. A grid that scrolls sits in a viewport with the
/// scrollbar of its direction; `id` keeps the viewport's scroll position.
pub fn grid_cells(
    fit: &GridFit,
    cells: impl IntoIterator<Item = AnyElement>,
    gap: Pixels,
    id: impl Into<SharedString>,
) -> AnyElement {
    let id = id.into();
    let mut cells = cells.into_iter();
    let mut grid = div().flex().flex_col().flex_none().gap(gap);
    for _ in 0..fit.rows {
        grid = grid.child(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .gap(gap)
                .children(cells.by_ref().take(fit.cols.max(1))),
        );
    }
    if fit.scrolls_x {
        // The row is wider than the viewport, so its width is set: a stretched row would
        // report the viewport's width and never scroll.
        div()
            .size_full()
            .child(grid.w(fit.content_w(gap)).h(fit.content_h(gap)))
            .overflow_x_scrollbar()
            .id(id)
            .into_any_element()
    } else if fit.scrolls {
        div()
            .size_full()
            .child(grid.w_full())
            .overflow_y_scrollbar()
            .id(id)
            .into_any_element()
    } else {
        grid.w_full().into_any_element()
    }
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
    split_body_with_side_height(chart, side, wide, None, CHART_SHARE, m)
}

/// [`split_body`] with the chart taking `share` of a wide body.
pub fn split_body_share(
    chart: AnyElement,
    side: AnyElement,
    wide: bool,
    share: f32,
    m: &UiMetrics,
) -> Div {
    split_body_with_side_height(chart, side, wide, None, share, m)
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
    split_body_with_side_height(chart, side, wide, Some(side_height), CHART_SHARE, m)
}

fn split_body_with_side_height(
    chart: AnyElement,
    side: AnyElement,
    wide: bool,
    side_height: Option<Pixels>,
    share: f32,
    m: &UiMetrics,
) -> Div {
    if wide {
        div()
            .flex()
            .flex_row()
            .size_full()
            .gap(m.sp[5])
            .child(div().flex_none().w(relative(share)).h_full().child(chart))
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
            // The Overview ignores `graph_range_s` (UI-PRF-055).
            range_ns: s.performance.overview_range.span_ns(),
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

/// Whether the Overview is paused or previewing: its charts wait for the history's answer
/// (`ChartView::with_hold`) and never draw live samples.
fn overview_held(cx: &App) -> bool {
    timeline::paused(cx).is_some()
}

/// A box chart: `ChartView::new` with the compact scale label on (TMOG), the ceiling small in
/// the plot's top-left corner. It reads the device metric history at 1 h, 24 h and 7 d, and
/// while the Overview is paused only the history's answer at the picked time.
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
        let snap = std::sync::Arc::clone(hub.read(cx).current());
        let mut view = ChartView::new(spec, labels, hub, cx)
            .with_newest_marks()
            .with_device_history(&snap)
            .with_hold::<OverviewPause>(overview_held, cx);
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
    vertical_at: None,
};

/// A sparkline's labels: no heading, no value, no range word, no scale label.
pub fn spark_labels(value_fmt: ValueFormat) -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt,
        value_align: ValueAlign::Hidden,
        range_word: SharedString::default(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// A sparkline chart for a spark row: no heading, no value, no range word, no scale label. The
/// newest-point marks are on, and a sparkline under `grid::MARKS_MIN_HEIGHT` tall draws none.
/// While the Overview is paused it waits for the history, as [`box_chart`] does.
pub fn spark_chart<T: 'static>(
    spec: ChartSpec,
    value_fmt: ValueFormat,
    hub: &Entity<SnapshotHub>,
    cx: &mut Context<T>,
) -> Entity<ChartView> {
    let labels = spark_labels(value_fmt);
    cx.new(|cx| {
        let snap = std::sync::Arc::clone(hub.read(cx).current());
        ChartView::new(spec, labels, hub, cx)
            .with_newest_marks()
            .with_device_history(&snap)
            .with_hold::<OverviewPause>(overview_held, cx)
    })
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
        let mut label = div();
        *label.text_style() = t_sm(m);
        let label = label
            .flex_none()
            .w(px(ROW_LABEL_W * s))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(theme.text_secondary)
            .child(row.label.clone());
        let mut value = div();
        *value.text_style() = numeric_style(m, m.text.sm);
        let value = value
            .flex_none()
            .w(px(SPARK_VALUE_W * s))
            .flex()
            .justify_end()
            .whitespace_nowrap()
            .text_color(theme.text_primary)
            .child(fit_text(row.value.clone()));
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

#[cfg(test)]
mod polish_tests {
    use super::*;
    use crate::theme::ThemeMode;
    use crate::theme::tests::test_theme;

    #[test]
    fn polish_box_shadow_follows_derived_shadow() {
        let mut t = test_theme(ThemeMode::Dark);
        t.derived.shadow = Hsla {
            h: 0.,
            s: 0.,
            l: 0.,
            a: 0.35,
        };
        let s = box_shadow(&t, 1.0).expect("shadow at intensity 100");
        assert_eq!(s.color, t.derived.shadow);
        assert_eq!(s.offset, point(px(0.), px(2.)));
        assert_eq!(s.blur_radius, px(8.));
        assert_eq!(s.spread_radius, px(0.));
        assert!(!s.inset);
        assert_eq!(box_shadow(&t, 2.0).unwrap().blur_radius, px(16.));

        // Intensity 0 gives a clear shadow colour: nothing is drawn.
        t.derived.shadow.a = 0.0;
        assert!(box_shadow(&t, 1.0).is_none());
    }
}
