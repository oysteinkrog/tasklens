//! The row canvas (UI-PRC-023, fallback step 3): one canvas per body row paints the row's plain
//! metric cells directly, so those cells add no element tree, no taffy node and no text
//! measuring to the frame.
//!
//! A plain cell is a text, path or numeric cell with validity Valid or Stale and no status dot,
//! usage bar or prefix ([`plain_cell`]). The table builds every other cell, and every cell of the
//! hovered row, as a `div` through [`value_cell`](crate::table::cells::value_cell), since only
//! those need tooltips, child elements or debug selectors.
//!
//! The canvas draws what the `div` path draws, pixel for pixel: the same text style (inherited
//! from the row, then the cell's `t_base` or `numeric_style`), the same colours, the suffix as a
//! highlight run, GPUI's end ellipsis on text cells, the clip on path and numeric cells, numeric
//! text at the right edge when it fits and at the left edge when it does not, the heat fill under
//! the text and the Stale opacity on text and fill. Positions repeat GPUI's layout rounding: each
//! absolute length rounds to whole device pixels, the text box centres in the row and its edges
//! round half toward zero, relative to the row's origin (the list lays out each row as a root).

use gpui_kit::{
    App, Bounds, ContentMask, HighlightStyle, Hsla, IntoElement, Pixels, Point, Refineable as _,
    SharedString, Styled, TextOverflow, TextRun, TextStyle, TextStyleRefinement, TruncateFrom,
    WhiteSpace, Window, canvas, fill, point, px, size,
};
use smallvec::SmallVec;
use std::rc::Rc;
use tl_model::validity::Validity;

use crate::table::cells::{CellCtx, CellOut, cell_background, cell_text_color};
use crate::table::columns::ColumnKind;
use tl_ui_base::text::style::{numeric_style, t_base};
use tl_ui_base::text::untrusted::debug_assert_clean;
use tl_ui_base::theme::UiMetrics;

/// The cells one row canvas paints, in column order. Ten fit without a heap allocation.
pub(crate) type CanvasCells = SmallVec<[CanvasCell; 10]>;

/// One plain metric cell, ready to paint.
pub(crate) struct CanvasCell {
    kind: ColumnKind,
    text: SharedString,
    /// The cell's left edge and width in the row, as the `div` path gets them.
    x: Pixels,
    width: Pixels,
    /// The text colour after the page overrides, before the Stale opacity.
    color: Hsla,
    /// The suffix's first byte and colour, when the suffix applies.
    suffix: Option<(usize, Hsla)>,
    /// The heat fill, before the Stale opacity.
    background: Option<Hsla>,
}

/// What every row canvas of one frame shares: the cell text styles and the row metrics.
pub(crate) struct CanvasStyles {
    /// `t_base`, one line, end ellipsis.
    text: TextStyleRefinement,
    /// `t_base`, one line.
    path: TextStyleRefinement,
    /// `numeric_style` at `text.base`, one line.
    numeric: TextStyleRefinement,
    pad: Pixels,
    row_h: Pixels,
}

impl CanvasStyles {
    pub(crate) fn new(m: &UiMetrics) -> Self {
        let nowrap = |mut r: TextStyleRefinement| {
            r.white_space = Some(WhiteSpace::Nowrap);
            r
        };
        let mut text = nowrap(t_base(m));
        // `Styled::text_ellipsis`.
        text.text_overflow = Some(TextOverflow::Truncate(SharedString::new_static("…")));
        Self {
            text,
            path: nowrap(t_base(m)),
            numeric: nowrap(numeric_style(m, m.text.base)),
            pad: m.sp[4],
            row_h: m.row_height,
        }
    }

    fn refinement(&self, kind: ColumnKind) -> &TextStyleRefinement {
        match kind {
            ColumnKind::Text => &self.text,
            ColumnKind::Path => &self.path,
            ColumnKind::Numeric => &self.numeric,
        }
    }
}

/// True when the row canvas can paint the cell in `out`: validity Valid or Stale, and no status
/// dot, usage bar or prefix.
pub(crate) fn plain_cell(out: &CellOut) -> bool {
    matches!(out.validity, Validity::Valid | Validity::Stale)
        && out.dot.is_none()
        && out.bar.is_none()
        && out.prefix.is_none()
}

/// The canvas cell for a plain cell (see [`plain_cell`]) at `x`, with the colours
/// [`value_cell`](crate::table::cells::value_cell), [`cell_text_color`] and, when `heat` is set,
/// [`cell_background`] give it.
pub(crate) fn canvas_cell(
    kind: ColumnKind,
    text: SharedString,
    out: &CellOut,
    ctx: &CellCtx,
    x: Pixels,
    heat: bool,
) -> CanvasCell {
    if kind != ColumnKind::Numeric {
        debug_assert_clean(&text);
    }
    let base = if kind == ColumnKind::Numeric && out.validity == Validity::Stale {
        ctx.secondary()
    } else {
        ctx.primary()
    };
    let color = cell_text_color(out, ctx).unwrap_or(base);
    // `CellText::with_suffix`: no suffix when its length is 0, too long or not on a char
    // boundary.
    let suffix = out.suffix.and_then(|s| {
        let start = text.len().checked_sub(s.len)?;
        (s.len > 0 && text.is_char_boundary(start)).then(|| {
            let color = if ctx.selected {
                s.selected_color
            } else {
                s.color
            };
            (start, color)
        })
    });
    CanvasCell {
        kind,
        text,
        x,
        width: ctx.width,
        color,
        suffix,
        background: if heat {
            cell_background(out, ctx)
        } else {
            None
        },
    }
}

/// The canvas that paints `cells` over the whole row. `opacity` is the Stale row's metric cell
/// opacity, multiplied into every colour as the `div` path's element opacity would.
pub(crate) fn row_canvas(
    cells: CanvasCells,
    styles: Rc<CanvasStyles>,
    opacity: Option<f32>,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, cx| {
            let base = window.text_style();
            let s = window.scale_factor();
            for cell in &cells {
                paint_cell(cell, &base, &styles, opacity, bounds.origin, s, window, cx);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// GPUI's layout rounding of a logical length to whole device pixels (`round_to_device_pixel`).
fn dp(v: Pixels, s: f32) -> f32 {
    round_half_toward_zero(f32::from(v) * s)
}

fn round_half_toward_zero(v: f32) -> f32 {
    (v.abs() - 0.5).ceil().copysign(v)
}

/// The window bounds of a box whose corners lie at `min` and `max` device pixels from the row
/// origin, rounded as GPUI rounds layout bounds.
fn snapped(row: Point<Pixels>, min: Point<f32>, max: Point<f32>, s: f32) -> Bounds<Pixels> {
    let min = min.map(round_half_toward_zero);
    let max = max.map(round_half_toward_zero);
    let local = Bounds::new(
        point(px(min.x / s), px(min.y / s)),
        size(px((max.x - min.x) / s), px((max.y - min.y) / s)),
    );
    Bounds::new(row + local.origin, local.size)
}

#[allow(clippy::too_many_arguments)]
fn paint_cell(
    cell: &CanvasCell,
    base: &TextStyle,
    styles: &CanvasStyles,
    opacity: Option<f32>,
    row: Point<Pixels>,
    s: f32,
    window: &mut Window,
    cx: &mut App,
) {
    let fade = |c: Hsla| match opacity {
        Some(o) => c.opacity(o),
        None => c,
    };
    // The cell box: `left(x)`, `w(width)`, `h_full()` of the row, 8 px padding.
    let (left, width, pad) = (dp(cell.x, s), dp(cell.width, s), dp(styles.pad, s));
    let row_h = dp(styles.row_h, s);
    let cell_box = snapped(row, point(left, 0.), point(left + width, row_h), s);
    if let Some(bg) = cell.background {
        window.paint_quad(fill(cell_box, fade(bg)));
    }

    let mut style = base.clone();
    style.refine(styles.refinement(cell.kind));
    style.color = cell.color;
    let rem = window.rem_size();
    let font_size = style.font_size.to_pixels(rem);
    let line_height = window.pixel_snap(style.line_height.to_pixels(font_size.into(), rem));
    let mut runs: SmallVec<[TextRun; 2]> = SmallVec::new();
    match cell.suffix {
        Some((start, color)) => {
            if start > 0 {
                runs.push(style.clone().to_run(start));
            }
            let highlight = HighlightStyle {
                color: Some(color),
                ..HighlightStyle::default()
            };
            runs.push(
                style
                    .clone()
                    .highlight(highlight)
                    .to_run(cell.text.len() - start),
            );
        }
        None => runs.push(style.to_run(cell.text.len())),
    }
    // The `div` path's element opacity multiplies every glyph colour; the canvas has none.
    if let Some(o) = opacity {
        for run in &mut runs {
            run.color = run.color.opacity(o);
        }
    }

    // The text box: the content width of the cell, one line high, centred in the row.
    let content = (width - 2. * pad).max(0.);
    let available = px(content / s);
    let wrap = (style.white_space == WhiteSpace::Normal).then_some(available);
    let (text, runs) = fit_text(cell, &style, available, line_height, &runs, window, cx);
    // One line: the measured height rounds up to whole device pixels.
    let text_h = (f32::from(line_height) * s).ceil();
    let top = (row_h - text_h) / 2.;
    let text_box = snapped(
        row,
        point(left + pad, top),
        point(left + pad + content, top + text_h),
        s,
    );
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    crate::table::cells::log_painted_text(text_box, text.clone());
    let Some(lines) = window
        .text_system()
        .shape_text(text.clone(), font_size, &runs, wrap, style.line_clamp)
        .ok()
    else {
        return;
    };
    let Some(line) = lines.first() else { return };

    // `CellText::prepaint`: numeric text sits at the right edge when it fits.
    let shown = line.unwrapped_layout.width;
    let x = if cell.kind == ColumnKind::Numeric && shown <= text_box.size.width {
        text_box.right() - shown
    } else {
        text_box.left()
    };
    let text_bounds = Bounds::new(point(x, text_box.top()), size(shown, line_height));
    let clip = (cell.kind != ColumnKind::Text).then_some(ContentMask { bounds: text_box });
    let align = style.text_align;
    window.with_content_mask(Some(ContentMask { bounds: cell_box }), |window| {
        window.with_content_mask(clip, |window| {
            let _ = line.paint(
                text_bounds.origin,
                line_height,
                align,
                Some(text_bounds),
                window,
                cx,
            );
        });
    });
}

/// The text and runs GPUI's text layout shapes for a cell `available` wide: the whole text when
/// it fits or the style has no overflow, else the text cut by `LineWrapper::truncate_line`.
fn fit_text(
    cell: &CanvasCell,
    style: &TextStyle,
    available: Pixels,
    line_height: Pixels,
    runs: &[TextRun],
    window: &mut Window,
    cx: &mut App,
) -> (SharedString, Vec<TextRun>) {
    let whole = || (cell.text.clone(), runs.to_vec());
    let Some(overflow) = &style.text_overflow else {
        return whole();
    };
    let font_size = style.font_size.to_pixels(window.rem_size());
    let fits = window
        .text_system()
        .shape_text(cell.text.clone(), font_size, runs, None, None)
        .ok()
        .is_some_and(|lines| {
            lines
                .iter()
                .all(|line| line.size(line_height).width <= available)
        });
    if fits {
        return whole();
    }
    let (affix, from) = match overflow {
        TextOverflow::Truncate(s) => (s, TruncateFrom::End),
        TextOverflow::TruncateStart(s) => (s, TruncateFrom::Start),
        TextOverflow::TruncateMiddle(s) => (s, TruncateFrom::Middle),
    };
    let mut wrapper = cx.text_system().line_wrapper(style.font(), font_size);
    let (text, runs) = wrapper.truncate_line(cell.text.clone(), available, affix, runs, from);
    (text, runs.into_owned())
}
