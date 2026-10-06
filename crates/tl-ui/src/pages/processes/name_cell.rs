//! The process table's Name cell (E22-005, UI-PRC-002, UI-PRC-003, PROC-006 to PROC-009,
//! PROC-063, D-011).
//!
//! This module decides what a process row shows in its Name cell: the icon and its size per row
//! kind, the primary text and the state glyph with its tooltip. The drawing, the R-190 geometry
//! (chevron box at `20 + depth × 20 + 6`, gap 6, icon, gap 6, text), the bidi isolation of the
//! count and the `binary` fallback for a missing icon file are `table/cells.rs`
//! ([`paint_name_cell`], [`paint_group_header_cell`]). The chevron's hit area, its tooltip and
//! the toggle that keeps the selection belong to `TlTreeTable`.

use std::ops::Range;

use gpui_kit::assets::IconName;
use gpui_kit::{AnyElement, App, ElementId, InteractiveElement, IntoElement, SharedString, Window};
use smallvec::SmallVec;

use crate::i18n::tr;
use crate::table::cells::{CellCtx, IconSourceCache, paint_group_header_cell, paint_name_cell};
pub use crate::table::tree_table::NameIcon;
use crate::table::tree_table::{IconSlot, NameCell, RowFlags, RowKind};

/// Fluent keys of the state glyph tooltips (R-132).
const SUSPENDED_KEY: &str = "processes-name-suspended";
const FROZEN_KEY: &str = "processes-name-frozen";

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
        secondary: None,
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
/// `node_key` is the row's `NodeId` hash. Inside `TlTreeTable` the table paints the cell itself.
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
        depth,
        expandable,
        flags,
        false,
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
    paint_group_header_cell(label, Some(expanded), ctx).into_any_element()
}
