//! Generic cell painters shared by every table (UI-PRC-001, UI-PRC-002, UI-FND-051,
//! UI-FND-052, UI-FND-053, UI-FND-026, ADR-009).
//!
//! Every cell is a fixed-width `div` the row positions at its column's x offset, with 8 px
//! padding left and right (× ui_scale), vertically centred, clipped and on one line. The text
//! inside is a [`CellText`] element: it draws the shaped text without an extra `div` and, in
//! `prepaint`, compares the text's width with the width the cell has for it and sets the shared
//! `truncated` flag. The owner reads that flag in the next render and attaches the cell tooltip
//! only when it is set ([`attach_cell_tooltip`]), so a cell gets its tooltip from its second
//! frame on.
//!
//! | Painter | Text | Overflow |
//! |---|---|---|
//! | [`text_cell`] | `t_base`, left | end ellipsis (U+2026), full text in the tooltip |
//! | [`path_cell`] | `t_base`, left | already middle-ellipsized by the caller; clips |
//! | [`numeric_cell`] | `tnum`, right | clips at the right edge, never ellipsized; full text in the tooltip |
//! | [`glyph_cell`] | `TlValidityGlyph`, right | glyphs are short; never heat-coloured |
//!
//! Page-specific painters (the Name cell, heat colouring rules, dim zeros) live in the pages.
//! Pages pass their colours through [`CellOut::background`] and [`CellOut::text_color`].
//!
//! No painter formats text: texts arrive as `SharedString` from `CellTextCache`. No cell has an
//! element id; the tooltip state of a truncated cell is keyed by the id its owner passes to
//! [`attach_cell_tooltip`].

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, Bounds, ContentMask, Div, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    InteractiveElement, IntoElement, LayoutId, ParentElement, Pixels, SharedString, Style, Styled,
    StyledText, TextStyleRefinement, Window, div, point, relative, size,
};
use tl_model::validity::Validity;

use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::validity::{glyph, glyph_allows_heat};
use crate::text::style::{numeric_style, t_base};
use crate::text::untrusted::debug_assert_clean;
use crate::theme::{TlTheme, UiMetrics};

/// Filled by `TreeTableDelegate::cell`. The table calls `cell()` with `want_text = false`, looks
/// up `CellTextCache` with (row, col, bits, validity); only on a miss it calls `cell()` again
/// with `want_text = true` and the delegate writes `text`.
pub struct CellOut {
    /// Value bits: `f32::to_bits as u64`, or the raw integer.
    pub bits: u64,
    pub validity: Validity,
    /// Text cells that hold an `Arc<str>` (names, users): no formatting, no copy.
    pub arc: Option<Arc<str>>,
    /// Scratch, reused across cells; written only when `want_text`.
    pub text: String,
    pub want_text: bool,
    /// Page override (heat colouring, UI-PRC-004). Drawn under the selection, never on glyphs.
    pub background: Option<Hsla>,
    /// Page override (dim zero values).
    pub text_color: Option<Hsla>,
    /// Optional explicit cell tooltip.
    pub tooltip: Option<SharedString>,
}

impl Default for CellOut {
    fn default() -> Self {
        Self {
            bits: 0,
            validity: Validity::default(),
            arc: None,
            text: String::new(),
            want_text: false,
            background: None,
            text_color: None,
            tooltip: None,
        }
    }
}

impl CellOut {
    /// Clears every field for the next cell. Keeps the capacity of `text`.
    pub fn reset(&mut self) {
        self.bits = 0;
        self.validity = Validity::default();
        self.arc = None;
        self.text.clear();
        self.want_text = false;
        self.background = None;
        self.text_color = None;
        self.tooltip = None;
    }
}

/// Per-row painting context. The row sets `width` to the column width before each cell.
pub struct CellCtx<'a> {
    pub m: &'a UiMetrics,
    pub theme: &'a TlTheme,
    pub selected: bool,
    pub width: Pixels,
}

impl CellCtx<'_> {
    /// `text_primary`, or `text_selected_primary` on a selected row.
    pub fn primary(&self) -> Hsla {
        if self.selected {
            self.theme.text_selected_primary
        } else {
            self.theme.text_primary
        }
    }

    /// `text_secondary`, or `text_selected_secondary` on a selected row.
    pub fn secondary(&self) -> Hsla {
        if self.selected {
            self.theme.text_selected_secondary
        } else {
            self.theme.text_secondary
        }
    }
}

/// A text cell with an end ellipsis. `truncated` is set in `prepaint` when the text does not fit;
/// pass it to [`attach_cell_tooltip`] in the next render.
///
/// In debug builds, panics when `text` holds a control character (UI-FND-053 rule 1).
pub fn text_cell(text: SharedString, ctx: &CellCtx, truncated: Rc<Cell<bool>>) -> Div {
    debug_assert_clean(&text);
    cell_box(t_base(ctx.m), ctx)
        .debug_selector(|| "TlCell-text".into())
        .text_color(ctx.primary())
        .text_ellipsis()
        .child(CellText::new(
            text,
            Align::Start,
            Fit::Ellipsis,
            Some(truncated),
        ))
}

/// A path cell. `text` is already cut by `text::ellipsis::middle_ellipsis` through
/// `CellTextCache::path`; a path that still does not fit is clipped. The caller attaches the full
/// path as the tooltip when the cached text differs from the source.
pub fn path_cell(text: SharedString, ctx: &CellCtx) -> Div {
    debug_assert_clean(&text);
    cell_box(t_base(ctx.m), ctx)
        .debug_selector(|| "TlCell-path".into())
        .text_color(ctx.primary())
        .child(CellText::new(text, Align::Start, Fit::Clip, None))
}

/// A numeric cell: right-aligned with tabular figures. A Valid or Stale value that does not fit
/// starts at the left padding edge and is clipped at the right one, never ellipsized
/// (UI-FND-051 rule 4); `clipped` is then set in `prepaint`. Every other validity draws its glyph
/// through [`glyph_cell`] and clears `clipped`.
pub fn numeric_cell(
    text: SharedString,
    validity: Validity,
    clipped: Rc<Cell<bool>>,
    ctx: &CellCtx,
) -> Div {
    let color = match validity {
        Validity::Valid => ctx.primary(),
        Validity::Stale => ctx.secondary(),
        _ => {
            clipped.set(false);
            return glyph_cell(validity, &text, ctx);
        }
    };
    cell_box(numeric_style(ctx.m, ctx.m.text.base), ctx)
        .debug_selector(|| "TlCell-numeric".into())
        .text_color(color)
        .child(CellText::new(text, Align::End, Fit::Clip, Some(clipped)))
}

/// A cell showing `TlValidityGlyph`: the glyph for any validity other than Valid, right-aligned
/// like a number so a change from `…` to a value moves nothing. Never heat-coloured
/// ([`cell_background`]).
pub fn glyph_cell(validity: Validity, value_text: &SharedString, ctx: &CellCtx) -> Div {
    cell_box(numeric_style(ctx.m, ctx.m.text.base), ctx)
        .debug_selector(|| "TlCell-glyph".into())
        .child(glyph(validity, value_text, ctx.selected, ctx.m, ctx.theme))
}

/// The tooltip for a cell whose text was cut: `TlTooltip::text(full)` when `truncated` is set,
/// else `None`.
pub fn cell_tooltip_if_truncated(full: SharedString, truncated: &Cell<bool>) -> Option<TlTooltip> {
    truncated.get().then(|| TlTooltip::text(full))
}

/// Attaches the 500 ms hover tooltip with `full` to `cell` when `truncated` was set by the
/// cell's last `prepaint`. `id` keys the tooltip state; it is not an element id on the cell.
/// A cell that fits gets nothing attached.
pub fn attach_cell_tooltip(
    cell: Div,
    id: impl Into<ElementId>,
    full: SharedString,
    truncated: &Cell<bool>,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let Some(tooltip) = cell_tooltip_if_truncated(full, truncated) else {
        return cell;
    };
    let state = HoverTooltip::state(id, window, cx);
    HoverTooltip::attach(&state, cell, DELAY_CONTROL, tooltip.build(), window, cx)
}

/// The page background for a cell: `out.background`, except on a selected row (the selected
/// fill wins) and on glyph cells (no heat on glyphs).
pub fn cell_background(out: &CellOut, ctx: &CellCtx) -> Option<Hsla> {
    if ctx.selected || !glyph_allows_heat(out.validity) {
        return None;
    }
    out.background
}

/// The page text colour for a cell: `out.text_color`, which becomes `text_selected_secondary`
/// on a selected row. Glyphs keep their own colours.
pub fn cell_text_color(out: &CellOut, ctx: &CellCtx) -> Option<Hsla> {
    out.text_color.map(|color| {
        if ctx.selected {
            ctx.theme.text_selected_secondary
        } else {
            color
        }
    })
}

/// Applies [`cell_background`] and [`cell_text_color`] to a cell built by a painter.
pub fn with_overrides(mut cell: Div, out: &CellOut, ctx: &CellCtx) -> Div {
    if let Some(background) = cell_background(out, ctx) {
        cell = cell.bg(background);
    }
    if let Some(color) = cell_text_color(out, ctx) {
        cell = cell.text_color(color);
    }
    cell
}

/// The box every cell shares: column width, 8 px padding, vertically centred, clipped, one line.
fn cell_box(text_style: TextStyleRefinement, ctx: &CellCtx) -> Div {
    let mut cell = div();
    *cell.text_style() = text_style;
    cell.w(ctx.width)
        .h_full()
        .flex()
        .items_center()
        .px(ctx.m.sp[4])
        .overflow_hidden()
        .whitespace_nowrap()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Start,
    End,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// GPUI's end ellipsis, from the cell's `text_ellipsis()`.
    Ellipsis,
    /// The whole text is shaped and clipped at the content edge.
    Clip,
}

/// One line of cell text that fills the cell's content width. It positions the shaped text
/// itself (no wrapper `div`), clips it to the content box and reports whether it was cut.
struct CellText {
    text: SharedString,
    inner: StyledText,
    align: Align,
    fit: Fit,
    cut: Option<Rc<Cell<bool>>>,
}

impl CellText {
    fn new(text: SharedString, align: Align, fit: Fit, cut: Option<Rc<Cell<bool>>>) -> Self {
        Self {
            inner: StyledText::new(text.clone()),
            text,
            align,
            fit,
            cut,
        }
    }

    /// Whether GPUI replaced the end of the text with the ellipsis. `shown_len` and
    /// `last_glyph` describe the shaped line; `available` is the content width.
    fn ellipsized(
        &self,
        shown_len: usize,
        last_glyph: Option<usize>,
        available: Pixels,
        window: &Window,
    ) -> bool {
        let len = self.text.len();
        if shown_len != len {
            return true;
        }
        // Same length: either the text is whole, or exactly 3 bytes were replaced by the 3-byte
        // "…". A cut line ends with the "…" glyph at byte len - 3. A whole line ends there only
        // when its last cluster is 3 bytes (a CJK letter, "…", an "ffi" ligature); only then is
        // the full text measured, which hits the layout cache GPUI filled in this frame.
        if len < 3 || last_glyph != Some(len - 3) {
            return false;
        }
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let run = style.to_run(len);
        window
            .text_system()
            .shape_text(self.text.clone(), font_size, &[run], None, None)
            .ok()
            .and_then(|lines| lines.first().map(|line| line.width()))
            .is_some_and(|full| full > available)
    }
}

impl IntoElement for CellText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for CellText {
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
        let (text_id, ()) = self.inner.request_layout(None, None, window, cx);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        (window.request_layout(style, [text_id], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.inner.layout().clone();
        let lines = layout.line_layouts();
        let (shown_width, shown_len, last_glyph) = match lines.first() {
            Some(line) => {
                let line = &line.unwrapped_layout;
                let last = line
                    .runs
                    .iter()
                    .flat_map(|run| run.glyphs.iter().map(|g| g.index))
                    .max();
                (line.width, line.len, last)
            }
            None => (Pixels::ZERO, 0, None),
        };
        let available = bounds.size.width;
        let x = match self.align {
            Align::End if shown_width <= available => bounds.right() - shown_width,
            _ => bounds.left(),
        };
        let text_bounds = Bounds::new(
            point(x, bounds.top()),
            size(shown_width, layout.line_height()),
        );
        self.inner
            .prepaint(None, None, text_bounds, &mut (), window, cx);

        let Some(cut) = &self.cut else { return };
        let was_cut = match self.fit {
            Fit::Clip => {
                debug_assert_eq!(
                    shown_len,
                    self.text.len(),
                    "a clipped cell inherited text_ellipsis"
                );
                shown_width > available
            }
            // GPUI may also leave a text whole that is a fraction of a pixel too wide.
            Fit::Ellipsis => {
                shown_width > available || self.ellipsized(shown_len, last_glyph, available, window)
            }
        };
        cut.set(was_cut);
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
        let mask = (self.fit == Fit::Clip).then_some(ContentMask { bounds });
        window.with_content_mask(mask, |window| {
            self.inner
                .paint(None, None, bounds, &mut (), &mut (), window, cx);
        });
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::px;
    use tl_model::settings::AppearanceSettings;

    use super::*;
    use crate::theme::ThemeMode;
    use crate::theme::metrics::compute;
    use crate::theme::tests::test_theme;

    #[test]
    fn reset_clears_every_field() {
        let mut out = CellOut {
            bits: 7,
            validity: Validity::Valid,
            arc: Some("bash".into()),
            text: String::with_capacity(64),
            want_text: true,
            background: Some(gpui_kit::red()),
            text_color: Some(gpui_kit::red()),
            tooltip: Some("tip".into()),
        };
        out.text.push_str("12.5%");
        out.reset();
        assert_eq!(out.bits, 0);
        assert_eq!(out.validity, Validity::WarmingUp);
        assert!(out.arc.is_none() && out.background.is_none() && out.text_color.is_none());
        assert!(out.tooltip.is_none() && !out.want_text);
        assert!(out.text.is_empty());
        assert!(out.text.capacity() >= 64, "the scratch keeps its capacity");
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "U+0007")]
    fn control_char_debug_assert() {
        let m = compute(&AppearanceSettings::default(), 0);
        let theme = test_theme(ThemeMode::Dark);
        let ctx = CellCtx {
            m: &m,
            theme: &theme,
            selected: false,
            width: px(120.),
        };
        let _ = text_cell("bell\u{7}".into(), &ctx, Rc::default());
    }
}
