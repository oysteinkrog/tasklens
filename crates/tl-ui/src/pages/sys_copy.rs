//! Copy commands shared by the system tables (UI-SYS-006, R-012, R-091, ADR-015).
//!
//! "Copy" copies the clicked cell, "Copy row" (Ctrl+C) one text block per selected row, and
//! "Copy rows as table" (Ctrl+Shift+C) the selected rows as TSV of the visible columns. Each page
//! implements [`SysCopySource`] with its own block format and toast texts; this file holds the
//! mechanics. Copying is a user export, so the copied text is never logged.

use fluent_bundle::FluentValue;
use gpui_kit::{App, SharedString, Window};
use smallvec::SmallVec;
use tl_model::ids::ColumnId;

use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::{Strings, tr_args, tr_en};
use crate::util::clipboard_write;

/// "Copied: { $value }".
const COPIED_KEY: &str = "sys-sys006-copied-value";
/// The toast shows at most this many chars of a copied cell (Unicode scalar values).
const TOAST_VALUE_MAX: usize = 60;
/// Starting size of the copy text per row.
const BYTES_PER_ROW: usize = 256;

/// Texts one table uses for its copy toasts.
pub struct CopyTexts {
    /// Services "Service details copied.", Startup "Copied to clipboard", Connections
    /// "Connection copied".
    pub row_ok: SharedString,
    /// Services "Could not copy service details.", the others "Failed to copy to clipboard".
    pub row_failed: SharedString,
    /// "Select a service row first." and the like.
    pub nothing_selected: SharedString,
}

/// What a page gives the copy commands. Rows are in view order.
pub trait SysCopySource {
    type RowId: Copy + Eq;
    /// Selected rows in view order, group rows already removed.
    fn copy_rows(&self) -> SmallVec<[Self::RowId; 8]>;
    /// Writes the "Copy row" block of one row (no trailing newline).
    fn write_block(&self, row: Self::RowId, out: &mut String);
    /// Visible columns in view order: (header label, id).
    fn visible_columns(&self) -> SmallVec<[(SharedString, ColumnId); 24]>;
    /// Cell text as shown; validity glyph cells write nothing.
    fn write_cell(&self, row: Self::RowId, col: ColumnId, out: &mut String);
    fn copy_texts(&self) -> &CopyTexts;
}

/// "Copy": the cell at `row` and `col`, both captured when the menu opened. The toast shows the
/// value cut to 60 chars; the clipboard gets all of it.
pub fn copy_cell<S: SysCopySource>(
    src: &S,
    row: S::RowId,
    col: ColumnId,
    window: &mut Window,
    cx: &mut App,
) {
    let mut text = String::new();
    src.write_cell(row, col, &mut text);
    let (kind, toast) = match clipboard_write(cx, text.clone()) {
        Ok(()) => (ToastKind::Success, copied_text(&text, cx)),
        Err(()) => (ToastKind::Error, src.copy_texts().row_failed.clone()),
    };
    push(kind, toast, window, cx);
}

/// "Copy row": the blocks of the selected rows, one blank line apart.
pub fn copy_row<S: SysCopySource>(src: &S, window: &mut Window, cx: &mut App) {
    let rows = src.copy_rows();
    finish(src, rows_blocks(src, &rows), window, cx);
}

/// "Copy rows as table": a header line of the visible column labels, then one line per selected
/// row, fields separated by tabs.
pub fn copy_rows_as_table<S: SysCopySource>(src: &S, window: &mut Window, cx: &mut App) {
    let rows = src.copy_rows();
    finish(src, rows_tsv(src, &rows), window, cx);
}

/// "Copied: <value>" with the value cut to 60 chars plus "…", in en-US.
pub fn copied_toast_text(value: &str) -> SharedString {
    tr_en(COPIED_KEY, &[("value", FluentValue::from(cut(value)))])
}

/// [`copied_toast_text`] in the UI language once the strings are installed.
fn copied_text(value: &str, cx: &App) -> SharedString {
    if cx.has_global::<Strings>() {
        tr_args(cx, COPIED_KEY, &[("value", FluentValue::from(cut(value)))])
    } else {
        copied_toast_text(value)
    }
}

/// The first 60 chars of `value`, plus "…" when it is longer.
fn cut(value: &str) -> String {
    match value.char_indices().nth(TOAST_VALUE_MAX) {
        Some((end, _)) => format!("{}\u{2026}", &value[..end]),
        None => value.to_owned(),
    }
}

/// The "Copy row" text, or `None` when no row is selected.
fn rows_blocks<S: SysCopySource>(src: &S, rows: &[S::RowId]) -> Option<String> {
    if rows.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(rows.len() * BYTES_PER_ROW);
    for (i, &row) in rows.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        src.write_block(row, &mut out);
    }
    Some(out)
}

/// The "Copy rows as table" text, or `None` when no row is selected.
fn rows_tsv<S: SysCopySource>(src: &S, rows: &[S::RowId]) -> Option<String> {
    if rows.is_empty() {
        return None;
    }
    let columns = src.visible_columns();
    let mut out = String::with_capacity(rows.len() * BYTES_PER_ROW);
    for (i, (label, _)) in columns.iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        push_field(&mut out, label);
    }
    let mut cell = String::new();
    for &row in rows {
        out.push('\n');
        for (i, &(_, col)) in columns.iter().enumerate() {
            if i > 0 {
                out.push('\t');
            }
            cell.clear();
            src.write_cell(row, col, &mut cell);
            push_field(&mut out, &cell);
        }
    }
    Some(out)
}

/// Appends one TSV field. A tab or a newline inside the value becomes one space each.
fn push_field(out: &mut String, value: &str) {
    out.extend(value.chars().map(|c| match c {
        '\t' | '\n' | '\r' => ' ',
        c => c,
    }));
}

/// Writes `text` and shows the row toast, or the "nothing selected" warning for `None`.
fn finish<S: SysCopySource>(src: &S, text: Option<String>, window: &mut Window, cx: &mut App) {
    let texts = src.copy_texts();
    let (kind, toast) = match text {
        None => (ToastKind::Warning, texts.nothing_selected.clone()),
        Some(text) => match clipboard_write(cx, text) {
            Ok(()) => (ToastKind::Success, texts.row_ok.clone()),
            Err(()) => (ToastKind::Error, texts.row_failed.clone()),
        },
    };
    push(kind, toast, window, cx);
}

/// Shows a toast on the window that ran the command.
fn push(kind: ToastKind, text: SharedString, window: &Window, cx: &mut App) {
    if let Some(queue) = ToastQueue::of_window(window, cx) {
        queue.update(cx, |queue, cx| queue.push(kind, text, None, None, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_keeps_60_chars() {
        let short = "a".repeat(60);
        assert_eq!(cut(&short), short);
        let long = "é".repeat(75);
        assert_eq!(cut(&long), format!("{}\u{2026}", "é".repeat(60)));
    }

    #[test]
    fn toast_text_cuts_the_value() {
        let value = "x".repeat(75);
        assert_eq!(
            copied_toast_text(&value).as_ref(),
            format!("Copied: {}\u{2026}", "x".repeat(60))
        );
    }

    #[test]
    fn field_replaces_tabs_and_newlines() {
        let mut out = String::new();
        push_field(&mut out, "a\tb\nc");
        assert_eq!(out, "a b c");
    }
}
