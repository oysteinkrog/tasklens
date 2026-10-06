//! `TlTreeTableHeader`: the column header above a `TlTreeTable` body (UI-PRC-001, UI-PRC-005).
//!
//! This is the one-line mode (E18-006): one label per visible column at the same x offsets as
//! the body, and a 1 px `divider` line at every column boundary. In `HeaderLines::Two` the header
//! is 44 px tall and its second line stays empty until E18-009 adds the totals. Drag, resize and
//! the sort arrow come with E18-009 to E18-011, which fill [`HeaderState`].
//!
//! The header owns no scroll state: the table passes its shared `x_offset` and the labels are
//! translated by `-x_offset`, so header and body always move together.

use gpui_kit::{
    Div, FontWeight, InteractiveElement, ParentElement, Pixels, Styled, div, prelude::FluentBuilder,
};

use crate::table::columns::{ColumnAlign, ColumnLayout, ColumnSpec, TableColumn};
use crate::table::tree_table::{HEADER_H_ONE, HEADER_H_TWO, HeaderLines};
use crate::text::style::label_style;
use crate::theme::{TlTheme, UiMetrics};

/// Header interaction state kept by the table across frames: column drag and resize. Empty
/// until E18-009 to E18-011.
#[derive(Default, Debug)]
pub struct HeaderState {}

/// Builds the header for one frame. Holds borrows only; nothing is formatted or copied.
pub struct TlTreeTableHeader<'a, C: TableColumn> {
    pub layout: &'a ColumnLayout<C>,
    pub lines: HeaderLines,
    /// Shared with the body: how far the content is scrolled to the left.
    pub x_offset: Pixels,
    pub m: &'a UiMetrics,
    pub theme: &'a TlTheme,
    /// One physical pixel (`UiMetrics::hairline`).
    pub hairline: Pixels,
}

impl<C: TableColumn> TlTreeTableHeader<'_, C> {
    /// Header height: 28 for `One`, 44 for `Two`, × ui_scale.
    pub fn height(lines: HeaderLines, m: &UiMetrics) -> Pixels {
        let h = match lines {
            HeaderLines::One => HEADER_H_ONE,
            HeaderLines::Two => HEADER_H_TWO,
        };
        gpui_kit::px(h * m.ui_scale)
    }

    /// The header element. `spec` returns the spec of a visible column.
    pub fn render<'s>(&self, spec: impl Fn(C) -> &'s ColumnSpec<C>) -> Div
    where
        C: 's,
    {
        let m = self.m;
        let theme = self.theme;
        let height = Self::height(self.lines, m);
        // Line 1 is the top 28 px in both modes.
        let line1_h = gpui_kit::px(HEADER_H_ONE * m.ui_scale);
        let columns = self.layout.visible_columns();
        let offsets = self.layout.offsets();
        let total = self.layout.total_width();

        let mut content = div()
            .absolute()
            .top_0()
            .left(-self.x_offset)
            .w(total)
            .h_full();
        for (pos, &id) in columns.iter().enumerate() {
            let x = offsets[pos];
            let w = self.layout.width(id);
            let spec = spec(id);
            let mut label = div();
            *label.text_style() = label_style(m, m.text.base, FontWeight::MEDIUM);
            let label = label
                .debug_selector(|| format!("TlTreeTableHeader-label-{}", id.key()))
                .absolute()
                .top_0()
                .left(x)
                .w(w)
                .h(line1_h)
                .px(m.sp[4])
                .flex()
                .items_center()
                .when(spec.align == ColumnAlign::Right, |d| d.justify_end())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(theme.text_primary)
                .child(spec.label.clone());
            // The boundary line sits on the column's right edge, inside the column.
            let line = div()
                .absolute()
                .top_0()
                .left(x + w - self.hairline)
                .w(self.hairline)
                .h_full()
                .bg(theme.divider);
            content = content.child(label).child(line);
        }

        div()
            .debug_selector(|| "TlTreeTableHeader".into())
            .relative()
            .flex_none()
            .w_full()
            .h(height)
            .overflow_hidden()
            .bg(theme.header)
            .child(content)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .bottom_0()
                    .w_full()
                    .h(self.hairline)
                    .bg(theme.divider),
            )
    }
}
