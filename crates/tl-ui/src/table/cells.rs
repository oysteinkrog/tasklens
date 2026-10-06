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
//! [`paint_name_cell`] and [`paint_group_header_cell`] draw the generic Name cell of a tree table
//! (UI-PRC-003) and the group header band (R-105). Page-specific painters (heat colouring rules,
//! dim zeros) live in the pages.
//! Pages pass their colours through [`CellOut::background`] and [`CellOut::text_color`].
//!
//! No painter formats text: texts arrive as `SharedString` from `CellTextCache`. No cell has an
//! element id; the tooltip state of a truncated cell is keyed by the id its owner passes to
//! [`attach_cell_tooltip`].

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{
    App, Bounds, ContentMask, Div, Element, ElementId, Font, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, InteractiveElement, IntoElement, LayoutId, ParentElement, Pixels, Point,
    SharedString, Style, Styled, StyledImage, StyledText, TextRun, TextStyleRefinement, Window,
    canvas, div, fill, font, img, point, px, relative, size, svg,
};
use smallvec::SmallVec;
use tl_model::validity::Validity;

use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::validity::{glyph, glyph_allows_heat};
use crate::table::tree_table::{
    CHEVRON, CHEVRON_HIT_W, ICON_GAP, INDENT, IconSlot, NAME_PAD, NameCell, NameIcon, RowFlags,
};
use crate::text::fonts::UI_FAMILY;
use crate::text::style::{TNUM, label_style, numeric_style, t_base};
use crate::text::untrusted::{debug_assert_clean, split_fffd_runs};
use crate::theme::{TlTheme, UiMetrics};

/// Filled by `TreeTableDelegate::cell`. The table calls `cell()` with `want_text = false`, looks
/// up `CellTextCache` with (row, col, bits, validity); only on a miss it calls `cell()` again
/// with `want_text = true` and the delegate writes `text`.
#[derive(Default)]
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

/// Gap between the primary and the secondary Name text (sp4).
const NAME_SECONDARY_GAP: f32 = 8.0;
/// The secondary Name text is hidden when less than this is left for it.
const NAME_SECONDARY_MIN: f32 = 40.0;
/// Gap before each state glyph (sp2).
const NAME_GLYPH_GAP: f32 = 4.0;
/// Inset of the trailing element from the cell's right edge (sp2).
const NAME_TRAILING_INSET: f32 = 4.0;
/// Group header row: chevron x and label x (R-105, UI-PRC-002).
const GROUP_CHEVRON_X: f32 = 8.0;
const GROUP_LABEL_X: f32 = 30.0;

/// Icon size of a Name cell slot, unscaled: 24 for `Large`, 16 for `Small`.
pub fn icon_px(slot: IconSlot) -> f32 {
    match slot {
        IconSlot::Large => 24.0,
        IconSlot::Small => 16.0,
    }
}

/// x of the chevron box's left edge at `depth`, × ui_scale: `20 + depth × 20 + 6` (R-190).
pub fn name_indent(depth: u8, ui_scale: f32) -> Pixels {
    px((NAME_PAD + f32::from(depth) * INDENT + ICON_GAP) * ui_scale)
}

/// x of the guide line of tree level `k`: the chevron centre of that level, `20 + k × 20 + 14`.
fn guide_x(k: u8, ui_scale: f32) -> f32 {
    (NAME_PAD + f32::from(k) * INDENT + ICON_GAP + CHEVRON / 2.0) * ui_scale
}

/// The connector quads of one Name cell (PROC-017, UI-PRC-003), in window coordinates for a
/// cell whose top-left corner is `origin`. Lines are one physical pixel wide and snapped to the
/// physical pixel grid (`device_scale` = `window.scale_factor()`).
///
/// For each ancestor level k below `depth` whose bit is set in `ancestor_continues`, a line over
/// the full row height at the chevron centre of level k. For the row's own level, at the chevron
/// centre of the parent level: a line from the row top to the centre (to the bottom unless
/// `LAST_SIBLING`) and a line from there to the chevron's left edge. Depth-0 rows have no parent
/// level and draw no elbow.
pub fn guide_quads(
    depth: u8,
    flags: RowFlags,
    origin: Point<Pixels>,
    row_h: Pixels,
    ui_scale: f32,
    device_scale: f32,
) -> SmallVec<[Bounds<Pixels>; 8]> {
    let line = px(1.0 / device_scale);
    let snap = |v: f32| px((v * device_scale).round() / device_scale);
    let x_at = |k: u8| snap(f32::from(origin.x) + guide_x(k, ui_scale));
    let top = origin.y;
    let mut quads = SmallVec::new();
    for k in 0..depth {
        if flags.ancestor_continues_at(k) {
            quads.push(Bounds::new(point(x_at(k), top), size(line, row_h)));
        }
    }
    if depth == 0 {
        return quads;
    }
    let x = x_at(depth - 1);
    let centre = snap(f32::from(top + row_h / 2.0));
    let bottom = if flags.has(RowFlags::LAST_SIBLING) {
        centre + line
    } else {
        top + row_h
    };
    quads.push(Bounds::new(point(x, top), size(line, bottom - top)));
    let chevron_left = snap(f32::from(origin.x + name_indent(depth, ui_scale)));
    if chevron_left > x {
        quads.push(Bounds::new(point(x, centre), size(chevron_left - x, line)));
    }
    quads
}

/// Text runs of a Name cell's primary text: U+FFFD runs in `warning` when `highlight_fffd`, the
/// live count run with `tnum`. `None` when one plain run does (the common case).
fn primary_runs(
    text: &str,
    highlight_fffd: bool,
    count_run: Option<&Range<usize>>,
    normal: Hsla,
    warning: Hsla,
) -> Option<SmallVec<[TextRun; 4]>> {
    let fffd = highlight_fffd && text.contains(char::REPLACEMENT_CHARACTER);
    let count = count_run.filter(|r| r.start < r.end && r.end <= text.len());
    if !fffd && count.is_none() {
        return None;
    }
    let base = font(UI_FAMILY);
    let tnum = Font {
        features: TNUM.clone(),
        ..base.clone()
    };
    let mut runs = SmallVec::new();
    let pieces = if fffd {
        split_fffd_runs(text)
    } else {
        smallvec::smallvec![(0..text.len(), false)]
    };
    for (range, is_fffd) in pieces {
        let color = if is_fffd { warning } else { normal };
        // Cut each piece at the count run's edges.
        let mut start = range.start;
        let mut cuts: SmallVec<[usize; 3]> = SmallVec::new();
        if let Some(c) = count {
            cuts.extend(
                [c.start, c.end]
                    .into_iter()
                    .filter(|&b| b > start && b < range.end),
            );
        }
        cuts.push(range.end);
        for end in cuts {
            let in_count = count.is_some_and(|c| start >= c.start && end <= c.end);
            runs.push(TextRun {
                len: end - start,
                font: if in_count { tnum.clone() } else { base.clone() },
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            start = end;
        }
    }
    Some(runs)
}

/// The Name cell (UI-PRC-003, R-190): the 16 px chevron box (always reserved) at
/// `20 + depth × 20 + 6`, gap 6, the icon (16 or 24) and gap 6 when there is one, then the
/// primary text, the secondary text, the state glyphs and the trailing element. The connector
/// lines are drawn under them when `guide_lines`.
///
/// `window` measures the primary text, and only when there is a secondary text. `tips` is
/// `Some(key)` on the hovered row: the state glyphs then get their tooltips, keyed by `key`.
#[allow(clippy::too_many_arguments)]
pub fn paint_name_cell(
    cell: NameCell,
    depth: u8,
    expandable: Option<bool>,
    flags: RowFlags,
    guide_lines: bool,
    ctx: &CellCtx,
    tips: Option<u64>,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let m = ctx.m;
    let s = m.ui_scale;
    let indent = name_indent(depth, s);
    let mut root = div();
    *root.text_style() = t_base(m);
    root = root
        .debug_selector(|| "TlNameCell".into())
        .relative()
        .w(ctx.width)
        .h_full()
        .overflow_hidden()
        .whitespace_nowrap();

    if guide_lines && (depth > 0 || flags.ancestor_continues != 0) {
        let divider = ctx.theme.divider;
        let row_h = m.row_height;
        root = root.child(
            div()
                .debug_selector(|| "TlNameCell-guides".into())
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, (), window, _| {
                            let quads = guide_quads(
                                depth,
                                flags,
                                bounds.origin,
                                row_h,
                                s,
                                window.scale_factor(),
                            );
                            for q in quads {
                                window.paint_quad(fill(q, divider));
                            }
                        },
                    )
                    .size_full(),
                ),
        );
    }

    // The chevron's hover area is 20 wide, centred on its 16 px box.
    let hit_extra = px((CHEVRON_HIT_W - CHEVRON) / 2.0 * s);
    let mut chevron_slot = div()
        .group("tl-name-chevron")
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .h_full()
        .w(px(CHEVRON_HIT_W * s))
        .ml(-hit_extra)
        .mr(-hit_extra);
    if let Some(expanded) = expandable {
        let name = if expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        chevron_slot = chevron_slot.child(
            svg()
                .path(name.path())
                .flex_none()
                .size(px(CHEVRON * s))
                .text_color(ctx.secondary())
                .group_hover("tl-name-chevron", |st| st.text_color(ctx.primary())),
        );
    }

    let mut content = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .pl(indent)
        .pr(m.sp[4])
        .child(chevron_slot);

    let gap = px(ICON_GAP * s);
    let icon_w = px(icon_px(cell.icon_size) * s);
    let icon_color = ctx.secondary();
    let has_icon = !matches!(cell.icon, NameIcon::None);
    match cell.icon {
        NameIcon::File(path) => {
            let fallback_color = ctx.theme.text_secondary;
            content = content.child(
                img(std::path::Path::new(&*path))
                    .flex_none()
                    .ml(gap)
                    .size(icon_w)
                    .with_fallback(move || {
                        svg()
                            .path(IconName::Binary.path())
                            .size(icon_w)
                            .text_color(fallback_color)
                            .into_any_element()
                    }),
            );
        }
        NameIcon::Lucide(name) => {
            content = content.child(
                svg()
                    .path(name.path())
                    .flex_none()
                    .ml(gap)
                    .size(icon_w)
                    .text_color(icon_color),
            );
        }
        NameIcon::None => {}
    }

    let primary_color = ctx.primary();
    let text = match primary_runs(
        &cell.primary,
        cell.highlight_fffd,
        cell.count_run.as_ref(),
        primary_color,
        ctx.theme.status.warning,
    ) {
        Some(runs) => StyledText::new(cell.primary.clone()).with_runs(runs.into_vec()),
        None => StyledText::new(cell.primary.clone()),
    };
    content = content.child(
        div()
            .debug_selector(|| "TlNameCell-primary".into())
            .ml(gap)
            .min_w_0()
            .flex_shrink(1.)
            .overflow_hidden()
            .text_ellipsis()
            .text_color(primary_color)
            .child(text),
    );

    if let Some(secondary) = cell.secondary {
        let font_size = m.text.base;
        let run = TextRun {
            len: cell.primary.len(),
            font: font(UI_FAMILY),
            color: primary_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let primary_w = window
            .text_system()
            .shape_line(cell.primary.clone(), font_size, &[run], None)
            .width;
        let icon_room = if has_icon { icon_w + gap } else { px(0.) };
        let text_x = indent + px(CHEVRON * s) + gap + icon_room;
        let room = ctx.width - text_x - m.sp[4];
        if room >= primary_w + px((NAME_SECONDARY_GAP + NAME_SECONDARY_MIN) * s) {
            content = content.child(
                div()
                    .debug_selector(|| "TlNameCell-secondary".into())
                    .ml(px(NAME_SECONDARY_GAP * s))
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(ctx.secondary())
                    .child(secondary),
            );
        }
    }

    for (i, (name, tip)) in cell.glyphs.into_iter().enumerate() {
        let mut glyph = div().flex_none().ml(px(NAME_GLYPH_GAP * s)).child(
            svg()
                .path(name.path())
                .size(m.icon.xs)
                .text_color(ctx.secondary()),
        );
        if let Some(key) = tips {
            let id = ElementId::NamedInteger(
                SharedString::new_static("tl-name-glyph"),
                key.wrapping_add(i as u64),
            );
            let state = HoverTooltip::state(id, window, cx);
            glyph = HoverTooltip::attach(
                &state,
                glyph,
                DELAY_CONTROL,
                TlTooltip::text(tip).build(),
                window,
                cx,
            );
        }
        content = content.child(glyph);
    }

    root = root.child(content);
    if let Some(trailing) = cell.trailing {
        root = root.child(
            div()
                .absolute()
                .top_0()
                .right(px(NAME_TRAILING_INSET * s))
                .h_full()
                .flex()
                .items_center()
                .child(trailing),
        );
    }
    root
}

/// The Name cell of a group header row (R-105, UI-PRC-002): the chevron at x 8 in
/// `text_primary` and the label at x 30 in `t_md` 14 weight 600. `ctx.width` is the width the
/// label may use, normally the rest of the row.
pub fn paint_group_header_cell(label: SharedString, expanded: Option<bool>, ctx: &CellCtx) -> Div {
    let m = ctx.m;
    let s = m.ui_scale;
    let mut root = div()
        .debug_selector(|| "TlGroupHeaderCell".into())
        .relative()
        .w(ctx.width)
        .h_full()
        .overflow_hidden()
        .whitespace_nowrap();
    if let Some(expanded) = expanded {
        let name = if expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        root = root.child(
            div()
                .absolute()
                .top_0()
                .left(px(GROUP_CHEVRON_X * s))
                .h_full()
                .flex()
                .items_center()
                .child(
                    svg()
                        .path(name.path())
                        .size(px(CHEVRON * s))
                        .text_color(ctx.theme.text_primary),
                ),
        );
    }
    let mut text = div();
    *text.text_style() = label_style(m, m.text.md, FontWeight::SEMIBOLD);
    root.child(
        text.debug_selector(|| "TlGroupHeaderCell-label".into())
            .absolute()
            .top_0()
            .left(px(GROUP_LABEL_X * s))
            .right(m.sp[4])
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .text_ellipsis()
            .text_color(ctx.theme.text_primary)
            .child(label),
    )
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
