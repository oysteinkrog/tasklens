//! The process table's Name cell (E22-005, UI-PRC-002, UI-PRC-003, PROC-006 to PROC-009,
//! PROC-063, D-011).
//!
//! This module decides what a process row shows in its Name cell: the icon and its size per row
//! kind, the primary text and the state glyph with its tooltip. The drawing, the R-190 geometry
//! (chevron box at `20 + depth × 20 + 6`, gap 6, icon, gap 6, text), the bidi isolation of the
//! count and the `binary` fallback for a missing icon file are `table/cells.rs`
//! ([`paint_name_cell`], [`paint_group_header_cell`]). The chevron's hit area, its tooltip and
//! the toggle that keeps the selection belong to `TlTreeTable`.
//!
//! Tree connector lines (PROC-017, `processes.tree_guide_lines`) are quads that `TlTreeTable`
//! paints over the row from its depth and its `LAST_SIBLING`, `IN_GROUP` and
//! `ancestor_continues` flags ([`crate::table::cells::guide_quads`]). The delegate computes the
//! flags once per table version ([`super::delegate::sibling_pass`]).
//!
//! The window button (PROC-053) sits at the cell's right edge on a hovered or selected row that
//! owns a top-level window, and only while the KWin bridge is up. The delegate decides when to
//! draw it ([`super::delegate::ProcessDelegate`]); [`window_button`] builds it.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::{
    AnyElement, App, Bounds, ElementId, FontWeight, InteractiveElement, IntoElement,
    ParentElement as _, Pixels, Point, SharedString, Styled as _, Window, canvas, div, px,
};
use smallvec::SmallVec;
use tl_model::ids::PseudoKind;

use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::i18n::tr;
use crate::table::cells::{CellCtx, IconSourceCache, paint_group_header_cell, paint_name_cell};
pub use crate::table::tree_table::NameIcon;
use crate::table::tree_table::{IconSlot, NameCell, RowFlags, RowKind};
use crate::theme::metrics;

/// Fluent keys of the state glyph tooltips (R-132).
const SUSPENDED_KEY: &str = "processes-name-suspended";
const FROZEN_KEY: &str = "processes-name-frozen";

/// The name of the window button's element id, `("winbtn", node_key)` (UI-PRC-003).
pub const WINDOW_BUTTON_ID: &str = "winbtn";

/// A state glyph after the primary text (PROC-063). `Frozen` is P2 and drawn when the delegate
/// sets it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StateGlyph {
    Suspended,
    Frozen,
}

impl StateGlyph {
    /// Lucide `pause` or `snowflake`.
    pub fn icon(self) -> IconName {
        match self {
            StateGlyph::Suspended => IconName::Pause,
            StateGlyph::Frozen => IconName::Snowflake,
        }
    }
}

/// The tooltip texts of the state glyphs, translated once per locale change.
#[derive(Clone)]
pub struct NameWords {
    pub suspended: SharedString,
    pub frozen: SharedString,
}

impl NameWords {
    pub fn new(cx: &App) -> Self {
        Self::from_fn(|key| tr(cx, key))
    }

    /// Builds the words from `text(key)`, for tests and for callers with their own lookup.
    pub fn from_fn(mut text: impl FnMut(&'static str) -> SharedString) -> Self {
        Self {
            suspended: text(SUSPENDED_KEY),
            frozen: text(FROZEN_KEY),
        }
    }

    fn tooltip(&self, glyph: StateGlyph) -> SharedString {
        match glyph {
            StateGlyph::Suspended => self.suspended.clone(),
            StateGlyph::Frozen => self.frozen.clone(),
        }
    }
}

/// What the delegate knows about one row's Name cell.
pub struct NameCellData {
    pub kind: RowKind,
    pub depth: u8,
    /// `None`: no children, no chevron (its width is still reserved).
    pub expandable: Option<bool>,
    /// The row's own icon. Unit and kernel thread rows ignore it (see [`row_icon`]).
    pub icon: NameIcon,
    /// Name, count and qualifiers in one string, `%s (%s) (%s)`, built by the delegate.
    pub primary: SharedString,
    /// Byte range of the live count " (N)" inside `primary`, drawn with tnum.
    pub count_run: Option<Range<usize>>,
    pub state_glyph: Option<StateGlyph>,
    pub exited: bool,
    /// Dimmed text after the primary text: an app row's window caption (PROC-008). Display
    /// only, never logged.
    pub secondary: Option<SharedString>,
}

/// 24 px for app rows, 16 px for every other row (PROC-006).
pub fn icon_slot(kind: RowKind) -> IconSlot {
    match kind {
        RowKind::App => IconSlot::Large,
        _ => IconSlot::Small,
    }
}

/// The icon a row of `kind` draws: Lucide `cog` for unit rows, Lucide `cpu` for the
/// "Kernel threads (N)" row and kernel thread rows (the delegate gives both
/// `RowKind::KernelThread`), else `icon`.
pub fn row_icon(kind: RowKind, icon: NameIcon) -> NameIcon {
    match kind {
        RowKind::Unit => NameIcon::Lucide(IconName::Cog),
        RowKind::KernelThread => NameIcon::Lucide(IconName::Cpu),
        _ => icon,
    }
}

/// The 16 px Lucide icon of a pseudo row (UI-PRC-002): `activity` for "Interrupts", `archive`
/// for "Memory compression (zram)".
pub fn pseudo_icon(kind: PseudoKind) -> NameIcon {
    NameIcon::Lucide(match kind {
        PseudoKind::Interrupts => IconName::Activity,
        PseudoKind::MemoryCompression => IconName::Archive,
    })
}

/// The `NameCell` the table paints for `d`.
pub fn name_cell(d: NameCellData, words: &NameWords) -> NameCell {
    let mut glyphs = SmallVec::new();
    if let Some(glyph) = d.state_glyph {
        glyphs.push((glyph.icon(), words.tooltip(glyph)));
    }
    NameCell {
        icon_size: icon_slot(d.kind),
        icon: row_icon(d.kind, d.icon),
        primary: d.primary,
        count_run: d.count_run,
        secondary: d.secondary,
        glyphs,
        highlight_fffd: true,
        trailing: None,
    }
}

/// The row flags the Name cell reads from `d`: `EXITED` when the process is gone.
pub fn name_flags(d: &NameCellData) -> RowFlags {
    RowFlags {
        bits: if d.exited { RowFlags::EXITED } else { 0 },
        ancestor_continues: 0,
    }
}

/// One Name cell outside a table, with the element id `("name", node_key)` (UI-PRC-023 rule 3).
/// `node_key` is the row's `NodeId` hash. Inside `TlTreeTable` the table paints the cell itself,
/// and its connector lines.
#[allow(clippy::too_many_arguments)]
pub fn render_name_cell(
    d: NameCellData,
    node_key: u64,
    ctx: &CellCtx,
    words: &NameWords,
    tips: bool,
    icons: &mut IconSourceCache,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let flags = name_flags(&d);
    let (depth, expandable) = (d.depth, d.expandable);
    paint_name_cell(
        name_cell(d, words),
        FontWeight::NORMAL,
        depth,
        expandable,
        false,
        flags,
        ctx,
        tips.then_some(node_key),
        icons,
        window,
        cx,
    )
    .id(ElementId::NamedInteger(
        SharedString::new_static("name"),
        node_key,
    ))
    .into_any_element()
}

/// The Name cell of a group header row (D-011): chevron at x 8, label such as "Apps (11)" at
/// x 30 in `t_md` 14 weight 600. The row draws the `header` band and the top `divider` line.
pub fn render_group_header(label: SharedString, expanded: bool, ctx: &CellCtx) -> AnyElement {
    let cell = NameCell {
        icon: NameIcon::None,
        icon_size: IconSlot::Small,
        primary: label,
        count_run: None,
        secondary: None,
        glyphs: SmallVec::new(),
        highlight_fffd: false,
        trailing: None,
    };
    paint_group_header_cell(cell, Some(expanded), ctx).into_any_element()
}

/// The Name-cell window button (PROC-053, UI-PRC-003): an icon button with Lucide `app-window`
/// at 14 px and the tooltip "Windows (N)", id `("winbtn", node_key)`. A click or Enter calls
/// `on_open` with the button's bottom left corner, where the window popup opens. `tab_stop`:
/// the button is on the focused row; on any other row Tab passes it by.
pub fn window_button(
    node_key: u64,
    tooltip: SharedString,
    tab_stop: bool,
    on_open: impl Fn(Point<Pixels>, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let icon_size = metrics(cx).icon.sm;
    let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::default();
    let at = Rc::clone(&bounds);
    let id = ElementId::NamedInteger(SharedString::new_static(WINDOW_BUTTON_ID), node_key);
    let button = TlButton::new(id, TlButtonKind::Icon)
        .size(TlButtonSize::Xs)
        .icon(TlIcon::AppWindow)
        .icon_size(icon_size)
        .tooltip(tooltip)
        .tab_stop(tab_stop)
        .on_click(move |ev, window, cx| {
            let b = at.get();
            let point = if b.size.width > px(0.) {
                b.bottom_left()
            } else {
                ev.position()
            };
            on_open(point, window, cx);
        });
    let measure = canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
        .absolute()
        .inset_0();
    div()
        .relative()
        .child(button)
        .child(measure)
        .into_any_element()
}
