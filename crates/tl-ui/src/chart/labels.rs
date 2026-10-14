//! Chart labels as cached shaped lines: heading, value, legend, axis max and range label
//! (UI-PRF-001 paint step 6, UI-PRF-003, ADR-009 rule 10, PERF-016, PERF-019, PERF-026).
//!
//! Each line keeps the text, size, weight and colour it was shaped from. A paint compares the
//! new text against the cached bytes and re-shapes only when something differs, so a chart whose
//! value text did not change paints its labels without shaping or allocating. Formatted text is
//! written into the caller's reused `scratch` buffer.

use core::fmt::Write;

use gpui_kit::assets::IconNamed;
use gpui_kit::{
    App, Bounds, Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, ShapedLine, SharedString,
    TextAlign, TextRun, TransformationMatrix, Window, fill, point, px, size,
};
use smallvec::SmallVec;
use tl_model::settings::NetworkUnit;
use tl_model::units::{self, NetUnit};
use tl_model::validity::Validity;

use super::{ChartLabels, ChartSpec, ChartState, LegendStyle, ValueAlign, ValueFormat, YScale};
use crate::components::icons::TlIcon;
use crate::text::fonts::UI_FAMILY;
use crate::text::style::TNUM;
use crate::theme::{TlTheme, UiMetrics};

const NS_PER_S: u64 = 1_000_000_000;

/// Label row height at 100% (`chart_label_row`).
pub(crate) const LABEL_ROW: f32 = 18.0;
/// Legend swatch size and spacing at 100%.
const SWATCH_W: f32 = 12.0;
const SWATCH_H: f32 = 2.0;
const DASH_W: f32 = 3.0;
const DASH_GAP: f32 = 2.0;
const LEGEND_ITEM_GAP: f32 = 12.0;
/// Below this box width the legend stacks one item per row.
pub(crate) const LEGEND_STACK_BELOW_W: f32 = 200.0;
/// The axis max, range label and legend need a plot at least this tall.
const MIN_PLOT_H: f32 = 100.0;
/// The compact scale label needs a plot at least this tall (TMOG).
const COMPACT_MIN_PLOT_H: f32 = 32.0;
/// The range label and legend need a box at least this wide.
const MIN_BOX_W: f32 = 160.0;

pub(crate) static WARMING: &str = "…";
pub(crate) static UNSUPPORTED: &str = "n/a";
pub(crate) static MISSING: &str = "-";
pub(crate) static FAILED: &str = "!";

/// One label and what it was shaped from.
#[derive(Debug)]
pub(crate) struct CachedLine {
    text: String,
    line: Option<ShapedLine>,
    font_size: Pixels,
    weight: FontWeight,
    color: Hsla,
}

impl Default for CachedLine {
    fn default() -> Self {
        Self {
            text: String::new(),
            line: None,
            font_size: px(0.),
            weight: FontWeight::NORMAL,
            color: Hsla::default(),
        }
    }
}

/// How one line is drawn.
#[derive(Clone, Copy)]
pub(crate) struct LineStyle {
    pub(crate) size: Pixels,
    pub(crate) weight: FontWeight,
    pub(crate) color: Hsla,
    pub(crate) tnum: bool,
}

impl CachedLine {
    /// Re-shapes when `text`, the size, the weight or the colour changed. Returns whether it
    /// shaped. The text compare is a byte compare; on a change the buffer is reused.
    pub(crate) fn sync(&mut self, window: &mut Window, text: &str, style: LineStyle) -> bool {
        if self.line.is_some()
            && self.text == text
            && self.font_size == style.size
            && self.weight == style.weight
            && self.color == style.color
        {
            return false;
        }
        self.text.clear();
        self.text.push_str(text);
        self.font_size = style.size;
        self.weight = style.weight;
        self.color = style.color;
        let run = TextRun {
            len: text.len(),
            font: Font {
                family: UI_FAMILY.into(),
                features: if style.tnum {
                    TNUM.clone()
                } else {
                    FontFeatures::default()
                },
                fallbacks: None,
                weight: style.weight,
                style: FontStyle::Normal,
            },
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        self.line = Some(window.text_system().shape_line(
            SharedString::new(text),
            style.size,
            &[run],
            None,
        ));
        true
    }

    pub(crate) fn width(&self) -> Pixels {
        self.line.as_ref().map_or(px(0.), |l| l.width)
    }

    /// The text this line was last shaped from.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn text(&self) -> Option<&str> {
        self.line.is_some().then_some(self.text.as_str())
    }
}

/// Which label a test record is for.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelKind {
    Heading,
    Value,
    /// The lock icon in place of the value (Restricted).
    Lock,
    /// Legend item by index.
    Legend(usize),
    AxisMax,
    Range,
}

/// One label painted by the last paint (test hook).
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintedLabel {
    pub kind: LabelKind,
    pub origin: gpui_kit::Point<Pixels>,
    pub width: Pixels,
    pub color: Hsla,
}

/// Shaped label text kept between frames (ADR-009 rule 10).
#[derive(Default, Debug)]
pub struct LabelCache {
    heading: CachedLine,
    value: CachedLine,
    axis_max: CachedLine,
    range: CachedLine,
    legend: SmallVec<[CachedLine; 2]>,
    /// `shape_line` calls over every paint.
    #[cfg(any(test, feature = "test-hooks"))]
    pub shape_count: u32,
    /// Labels painted by the last paint, in paint order.
    #[cfg(any(test, feature = "test-hooks"))]
    pub painted: SmallVec<[PaintedLabel; 8]>,
}

impl LabelCache {
    /// The value label's text as last shaped.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn value_text(&self) -> Option<&str> {
        self.value.text()
    }

    fn shaped(&mut self, shaped: bool) {
        #[cfg(any(test, feature = "test-hooks"))]
        if shaped {
            self.shape_count += 1;
        }
        #[cfg(not(any(test, feature = "test-hooks")))]
        let _ = shaped;
    }

    #[cfg(any(test, feature = "test-hooks"))]
    fn record(
        &mut self,
        kind: LabelKind,
        origin: gpui_kit::Point<Pixels>,
        width: Pixels,
        color: Hsla,
    ) {
        self.painted.push(PaintedLabel {
            kind,
            origin,
            width,
            color,
        });
    }

    /// The text `kind` was last shaped from.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn text(&self, kind: LabelKind) -> Option<&str> {
        let line = match kind {
            LabelKind::Heading => &self.heading,
            LabelKind::Value => &self.value,
            LabelKind::Lock => return None,
            LabelKind::Legend(i) => self.legend.get(i)?,
            LabelKind::AxisMax => &self.axis_max,
            LabelKind::Range => &self.range,
        };
        line.line.is_some().then_some(line.text.as_str())
    }

    /// The last paint record for `kind`.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn painted(&self, kind: LabelKind) -> Option<PaintedLabel> {
        self.painted.iter().find(|p| p.kind == kind).copied()
    }
}

/// Inputs of one label paint.
pub struct LabelPaint<'a> {
    pub box_bounds: Bounds<Pixels>,
    pub plot: Bounds<Pixels>,
    pub labels: &'a ChartLabels,
    pub spec: &'a ChartSpec,
    pub state: &'a ChartState,
    /// Newest value of series 0 and its validity; `None` when the chart has no series.
    pub value: Option<(f32, Validity)>,
    /// Owner text shown in place of the formatted value while it is Valid or Stale.
    pub value_text: Option<&'a str>,
    /// Axis max text from `scale::compute`, `None` when the scale has none.
    pub axis_text: Option<&'a str>,
    /// Draw the scale ceiling small in the plot's top-left corner in place of the axis max
    /// (`ChartView::set_compact_scale`).
    pub compact_scale: bool,
    pub metrics: &'a UiMetrics,
    pub theme: &'a TlTheme,
    pub network_unit: NetworkUnit,
}

/// The owner's value text when set, else `v` formatted.
fn write_value(p: &LabelPaint, v: f32, out: &mut String) {
    match p.value_text {
        Some(text) => out.push_str(text),
        None => format_value(p.labels.value_fmt, v, p.network_unit, out),
    }
}

/// Paints the heading, value, legend, axis max and range label (UI-PRF-001 step 6). Lines are
/// re-shaped only when their text or style changed; `scratch` is the reused formatter buffer.
pub fn paint(
    window: &mut Window,
    cx: &mut App,
    cache: &mut LabelCache,
    p: &LabelPaint,
    scratch: &mut String,
) {
    #[cfg(any(test, feature = "test-hooks"))]
    cache.painted.clear();

    let m = p.metrics;
    let s = m.ui_scale;
    let pad = m.sp[3];
    let row = px(LABEL_ROW * s);
    let b = p.box_bounds;
    let row1 = b.top() + pad;
    let row2 = row1 + row;
    let tall = f32::from(p.plot.size.height) >= MIN_PLOT_H * s;
    let wide = f32::from(b.size.width) >= MIN_BOX_W * s;
    let secondary = LineStyle {
        size: m.text.xs,
        weight: FontWeight::NORMAL,
        color: p.theme.text_secondary,
        tnum: false,
    };

    // Heading.
    if let Some(heading) = &p.labels.heading {
        let style = LineStyle {
            size: m.text.sm,
            weight: FontWeight::MEDIUM,
            color: p.theme.text_primary,
            tnum: false,
        };
        let shaped = cache.heading.sync(window, heading, style);
        cache.shaped(shaped);
        let origin = point(b.left() + pad, row1);
        paint_line(window, cx, &cache.heading, origin, row);
        #[cfg(any(test, feature = "test-hooks"))]
        cache.record(
            LabelKind::Heading,
            origin,
            cache.heading.width(),
            style.color,
        );
    }

    // Value.
    if let Some((v, validity)) = p
        .value
        .filter(|_| p.labels.value_align != ValueAlign::Hidden)
    {
        let left = p.labels.value_align == ValueAlign::Left;
        let weight = if left {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        };
        if validity == Validity::Restricted {
            let icon = m.icon.xs;
            let x = if left {
                b.left() + pad
            } else {
                b.right() - pad - icon
            };
            let bounds = Bounds::new(point(x, row1 + (row - icon) / 2.0), size(icon, icon));
            let _ = window.paint_svg(
                bounds,
                TlIcon::Lock.path(),
                None,
                TransformationMatrix::unit(),
                p.theme.text_secondary,
                cx,
            );
            #[cfg(any(test, feature = "test-hooks"))]
            cache.record(LabelKind::Lock, bounds.origin, icon, p.theme.text_secondary);
        } else {
            scratch.clear();
            let color = match validity {
                Validity::Valid => {
                    write_value(p, v, scratch);
                    p.theme.text_primary
                }
                Validity::Stale => {
                    write_value(p, v, scratch);
                    p.theme.text_secondary
                }
                Validity::WarmingUp => {
                    scratch.push_str(WARMING);
                    p.theme.text_secondary
                }
                Validity::Unsupported => {
                    scratch.push_str(UNSUPPORTED);
                    p.theme.text_secondary
                }
                Validity::Missing => {
                    scratch.push_str(MISSING);
                    p.theme.text_secondary
                }
                Validity::Failed | Validity::Restricted => {
                    scratch.push_str(FAILED);
                    p.theme.status.warning
                }
            };
            let style = LineStyle {
                size: m.text.sm,
                weight,
                color,
                tnum: true,
            };
            let shaped = cache.value.sync(window, scratch, style);
            cache.shaped(shaped);
            let x = if left {
                b.left() + pad
            } else {
                b.right() - pad - cache.value.width()
            };
            let origin = point(x, row1);
            paint_line(window, cx, &cache.value, origin, row);
            #[cfg(any(test, feature = "test-hooks"))]
            cache.record(LabelKind::Value, origin, cache.value.width(), color);
        }
    }

    // Legend: row 2, horizontal, or one item per row when the box is narrow.
    if !p.labels.legend.is_empty() && tall && wide {
        let stack = f32::from(b.size.width) < LEGEND_STACK_BELOW_W * s;
        let colors = |i: usize| {
            p.spec
                .series
                .get(i)
                .map_or(p.theme.text_secondary, |s| s.line)
        };
        let origin = point(b.left() + pad, row2);
        paint_legend(
            window,
            cx,
            cache,
            &p.labels.legend,
            colors,
            origin,
            stack,
            m,
            p.theme,
        );
    }

    // Compact scale: the ceiling in the plot's top-left corner, on plots from 32 px. A fixed
    // scale has no axis text, so its ceiling is formatted here.
    let has_data = matches!(p.state, ChartState::Live | ChartState::Stale { .. });
    if p.compact_scale && f32::from(p.plot.size.height) >= COMPACT_MIN_PLOT_H * s {
        let fixed = match p.spec.y {
            YScale::Fixed { max, .. } => Some(max),
            YScale::FixedTotal(total) if total > 0.0 => Some(total),
            _ => None,
        };
        let text = match (p.axis_text.filter(|_| has_data), fixed) {
            (Some(text), _) => Some(text),
            (None, Some(max)) => {
                scratch.clear();
                format_value(p.labels.value_fmt, max, p.network_unit, scratch);
                Some(scratch.as_str())
            }
            (None, None) => None,
        };
        if let Some(text) = text {
            let style = LineStyle {
                tnum: true,
                ..secondary
            };
            let shaped = cache.axis_max.sync(window, text, style);
            cache.shaped(shaped);
            let origin = point(p.plot.left() + m.sp[2], p.plot.top() + m.sp[1]);
            paint_line(window, cx, &cache.axis_max, origin, row);
            #[cfg(any(test, feature = "test-hooks"))]
            cache.record(
                LabelKind::AxisMax,
                origin,
                cache.axis_max.width(),
                style.color,
            );
        }
    }

    // Axis max: row 2, right-aligned. Only with data on a tall plot.
    if let Some(text) = p
        .axis_text
        .filter(|_| p.labels.show_axis_max && !p.compact_scale && tall && has_data)
    {
        let shaped = cache.axis_max.sync(window, text, secondary);
        cache.shaped(shaped);
        let origin = point(b.right() - pad - cache.axis_max.width(), row2);
        paint_line(window, cx, &cache.axis_max, origin, row);
        #[cfg(any(test, feature = "test-hooks"))]
        cache.record(
            LabelKind::AxisMax,
            origin,
            cache.axis_max.width(),
            secondary.color,
        );
    }

    // Range label: bottom-left.
    if tall && wide && !p.labels.range_word.is_empty() {
        scratch.clear();
        range_label(&p.labels.range_word, p.spec.range_ns, scratch);
        let shaped = cache.range.sync(window, scratch, secondary);
        cache.shaped(shaped);
        let origin = point(b.left() + pad, b.bottom() - pad - row);
        paint_line(window, cx, &cache.range, origin, row);
        #[cfg(any(test, feature = "test-hooks"))]
        cache.record(
            LabelKind::Range,
            origin,
            cache.range.width(),
            secondary.color,
        );
    }
}

/// The legend items from `origin`, horizontal, or one item per row when `stack`. `color` gives
/// item `i`'s swatch colour.
#[allow(
    clippy::too_many_arguments,
    reason = "one paint call, every input is distinct"
)]
pub(crate) fn paint_legend(
    window: &mut Window,
    cx: &mut App,
    cache: &mut LabelCache,
    items: &[(SharedString, LegendStyle)],
    color: impl Fn(usize) -> Hsla,
    origin: gpui_kit::Point<Pixels>,
    stack: bool,
    m: &UiMetrics,
    theme: &TlTheme,
) {
    let s = m.ui_scale;
    let row = px(LABEL_ROW * s);
    let secondary = LineStyle {
        size: m.text.xs,
        weight: FontWeight::NORMAL,
        color: theme.text_secondary,
        tnum: false,
    };
    let n = items.len();
    if cache.legend.len() != n {
        cache.legend.resize_with(n, CachedLine::default);
    }
    let mut x = origin.x;
    let mut y = origin.y;
    for (i, (label, style)) in items.iter().enumerate() {
        let shaped = cache.legend[i].sync(window, label, secondary);
        cache.shaped(shaped);
        paint_swatch(
            window,
            point(x, y + (row - px(SWATCH_H * s)) / 2.0),
            s,
            *style,
            color(i),
        );
        let origin = point(x + px(SWATCH_W * s) + m.sp[2], y);
        paint_line(window, cx, &cache.legend[i], origin, row);
        let width = cache.legend[i].width();
        #[cfg(any(test, feature = "test-hooks"))]
        cache.record(LabelKind::Legend(i), origin, width, secondary.color);
        if stack {
            y += row;
        } else {
            x = origin.x + width + px(LEGEND_ITEM_GAP * s);
        }
    }
}

pub(crate) fn paint_line(
    window: &mut Window,
    cx: &mut App,
    line: &CachedLine,
    origin: gpui_kit::Point<Pixels>,
    line_height: Pixels,
) {
    if let Some(l) = &line.line {
        // A failed glyph raster leaves the label out for this frame; nothing to report.
        let _ = l.paint(origin, line_height, TextAlign::Left, None, window, cx);
    }
}

/// A 12 × 2 px line swatch at `origin`: one quad, or three 3 px dashes 2 px apart (the last one
/// clipped to the swatch width).
pub(crate) fn paint_swatch(
    window: &mut Window,
    origin: gpui_kit::Point<Pixels>,
    s: f32,
    style: LegendStyle,
    color: Hsla,
) {
    let h = px(SWATCH_H * s);
    match style {
        LegendStyle::Solid => {
            window.paint_quad(fill(Bounds::new(origin, size(px(SWATCH_W * s), h)), color));
        }
        LegendStyle::Dashed => {
            let mut x0 = 0.0;
            while x0 < SWATCH_W {
                let w = DASH_W.min(SWATCH_W - x0);
                let at = point(origin.x + px(x0 * s), origin.y);
                window.paint_quad(fill(Bounds::new(at, size(px(w * s), h)), color));
                x0 += DASH_W + DASH_GAP;
            }
        }
    }
}

/// Appends `"<word> over <range>"`: "30 seconds", "60 seconds", "5 minutes", "10 minutes"
/// (PERF-016). Whole minutes from 2 minutes on, seconds below.
pub fn range_label(range_word: &str, range_ns: u64, out: &mut String) {
    let secs = range_ns / NS_PER_S;
    out.push_str(range_word);
    out.push_str(" over ");
    if secs >= 120 && secs.is_multiple_of(60) {
        let _ = write!(out, "{} minutes", secs / 60);
    } else {
        let _ = write!(out, "{secs} seconds");
    }
}

/// Appends `value` in the chart value form of canonical table (d), through `tl_model::units`.
/// Byte and network rates are bytes per second; network rates auto-scale on graphs.
pub fn format_value(fmt: ValueFormat, value: f32, unit: NetworkUnit, out: &mut String) {
    // NaN and negative values clamp to 0 in the integer casts.
    let whole = |v: f32| v.max(0.0).round() as u64;
    match fmt {
        ValueFormat::Percent => units::percent(out, value),
        ValueFormat::PercentOf { total } => units::percent(out, percent_of(value, total)),
        ValueFormat::Bytes => units::bytes(out, whole(value)),
        ValueFormat::RateIec => units::rate_iec_auto(out, f64::from(value)),
        ValueFormat::RateNetwork => {
            let unit = match unit {
                NetworkUnit::Bits => NetUnit::Bits,
                NetworkUnit::Bytes => NetUnit::Bytes,
            };
            units::net_rate(out, f64::from(value), unit, true);
        }
        ValueFormat::Mhz => units::mhz(out, value),
        ValueFormat::Celsius => units::celsius(out, value),
        ValueFormat::Watts => units::watts(out, value),
        ValueFormat::Rpm => {
            let _ = write!(out, "{} RPM", whole(value));
        }
        ValueFormat::Count => units::count(out, whole(value)),
    }
}

/// `value` as a percent of `total`; 0 when `total` is 0.
pub(crate) fn percent_of(value: f32, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (f64::from(value) / total as f64 * 100.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: f32 = 1024.0 * 1024.0 * 1024.0;

    #[test]
    fn percent_of_total() {
        let f = |v: f32, total: u64| {
            let mut out = String::new();
            format_value(
                ValueFormat::PercentOf { total },
                v,
                NetworkUnit::Bits,
                &mut out,
            );
            out
        };
        assert_eq!(f(8.0 * GIB, 16 << 30), "50.0%");
        assert_eq!(f(34.68 * GIB, (63.16 * GIB) as u64), "54.9%");
        assert_eq!(f(0.0, 16 << 30), "0%");
        assert_eq!(f(8.0 * GIB, 0), "0%", "no total yet");
    }
}
