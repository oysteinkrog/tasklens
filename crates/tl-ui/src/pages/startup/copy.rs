//! Startup Apps copy commands and Open file location (UI-SYS-006, UI-SYS-044, UI-SYS-048,
//! R-134, ADR-015).
//!
//! Every command works on the entries captured at menu open, or on the selection for a keyboard
//! command. The text is built from those entries, so a rescan cannot change what is copied.
//! Copied text is a user export and is never logged.

use std::fmt::Write as _;
use std::sync::Arc;

use gpui_kit::{Context, SharedString, Window};
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest};
use tl_model::ids::ColumnId;
use tl_model::snapshot::StartupEntry;
use tl_model::table::{StartupColumn, SysColumnId};

use super::StartupPage;
use super::actions::{StartupCopyCommand, StartupOpenFileLocation};
use super::delegate::{StartupWords, home_tilde};
use super::menu::StartupMenuTargets;
use crate::action_tracker::{GlobalActionTracker, ToastTexts};
use crate::components::toast::{ToastKey, ToastKind, ToastQueue};
use crate::i18n::tr;
use crate::pages::processes::actions::CopyDetails;
use crate::pages::sys_copy::{CopyTexts, SysCopySource, copy_cell, copy_row, copy_rows_as_table};
use crate::table::TreeTableDelegate as _;
use crate::table::actions::{CopyCell, CopyRow, CopyRowsAsTable};
use crate::util::clipboard_write;

/// What one copy command works on: the captured entries in view order and the words and columns
/// of the table at that moment.
#[derive(Default)]
pub(super) struct PreparedCopy {
    entries: Vec<StartupEntry>,
    columns: SmallVec<[(SharedString, ColumnId); 24]>,
    words: Option<StartupWords>,
    home: Arc<str>,
    not_found: SharedString,
}

impl SysCopySource for StartupPage {
    /// Index into the captured entries. `StartupRef` is not `Copy`, which the trait needs.
    type RowId = usize;

    fn copy_rows(&self) -> SmallVec<[Self::RowId; 8]> {
        (0..self.copy_prepared.entries.len()).collect()
    }

    fn write_block(&self, row: Self::RowId, out: &mut String) {
        let Some(entry) = self.copy_prepared.entries.get(row) else {
            return;
        };
        write_block(&self.copy_prepared, entry, out);
    }

    fn visible_columns(&self) -> SmallVec<[(SharedString, ColumnId); 24]> {
        self.copy_prepared.columns.clone()
    }

    fn write_cell(&self, row: Self::RowId, col: ColumnId, out: &mut String) {
        let Some(entry) = self.copy_prepared.entries.get(row) else {
            return;
        };
        if let Some(column) = alias_column(col) {
            write_cell(&self.copy_prepared, entry, column, out);
        }
    }

    fn copy_texts(&self) -> &CopyTexts {
        &self.copy_texts
    }
}

impl StartupPage {
    pub(super) fn copy_texts(cx: &gpui_kit::App) -> CopyTexts {
        CopyTexts {
            row_ok: tr(cx, "startup-copy-row-ok"),
            row_failed: tr(cx, "startup-copy-failed"),
            nothing_selected: tr(cx, "startup-copy-nothing-selected"),
        }
    }

    /// Captures the entries and the visible columns for one copy command.
    fn prepare_copy(&mut self, targets: StartupMenuTargets, cx: &gpui_kit::App) {
        let table = self.frame.table.read(cx);
        let delegate = &table.delegate;
        let columns = delegate
            .columns()
            .visible_columns()
            .iter()
            .filter_map(|&column| {
                let SysColumnId::Startup(startup) = column else {
                    return None;
                };
                Some((
                    delegate.column_spec(column).label.clone(),
                    column_alias(startup),
                ))
            })
            .collect();
        self.copy_prepared = PreparedCopy {
            entries: targets.entries.into_iter().collect(),
            columns,
            words: Some(delegate.words().clone()),
            home: delegate.home().clone(),
            not_found: tr(cx, "startup-copy-not-found"),
        };
    }

    /// "Copy": the cell captured at menu open.
    pub(super) fn on_copy_cell(
        &mut self,
        _: &CopyCell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_targets(cx);
        let Some((_, SysColumnId::Startup(column))) = targets.cell.clone() else {
            return;
        };
        if targets.entries.is_empty() {
            return;
        }
        self.prepare_copy(targets, cx);
        // The cell belongs to the first captured row.
        copy_cell(self, 0, column_alias(column), window, cx);
    }

    /// "Copy row" and Ctrl+C.
    pub(super) fn on_copy_details(
        &mut self,
        _: &CopyDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_targets(cx);
        self.prepare_copy(targets, cx);
        copy_row(self, window, cx);
    }

    /// The table's own Ctrl+C binding.
    pub(super) fn on_copy_row(&mut self, _: &CopyRow, window: &mut Window, cx: &mut Context<Self>) {
        self.on_copy_details(&CopyDetails, window, cx);
    }

    /// "Copy rows as table" and Ctrl+Shift+C.
    pub(super) fn on_copy_rows_as_table(
        &mut self,
        _: &CopyRowsAsTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_targets(cx);
        self.prepare_copy(targets, cx);
        copy_rows_as_table(self, window, cx);
    }

    /// Runs the page's own "Copy rows as table". On the keyboard the table handles
    /// Ctrl+Shift+C first, so tests reach this path directly.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn copy_rows_as_table_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.on_copy_rows_as_table(&CopyRowsAsTable, window, cx);
    }

    /// "Copy command": the Command value of the one captured entry.
    pub(super) fn on_startup_copy_command(
        &mut self,
        _: &StartupCopyCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_targets(cx);
        let [entry] = targets.entries.as_slice() else {
            return;
        };
        let (kind, text) = match clipboard_write(cx, entry.command.to_string()) {
            Ok(()) => (ToastKind::Success, tr(cx, "startup-copy-command-ok")),
            Err(()) => (ToastKind::Error, tr(cx, "startup-copy-failed")),
        };
        push_toast(window, cx, kind, text, None);
    }

    /// "Open file location": the entry's source file in the file manager. Nothing is sent when
    /// the file is gone.
    pub(super) fn on_startup_open_file_location(
        &mut self,
        _: &StartupOpenFileLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.take_targets(cx);
        let [entry] = targets.entries.as_slice() else {
            return;
        };
        if !entry.file_exists {
            let text = tr(cx, "startup-open-location-unavailable");
            push_toast(window, cx, ToastKind::Warning, text, None);
            return;
        }
        let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
            return;
        };
        let Some(queue) = ToastQueue::of_window(window, cx) else {
            return;
        };
        let request = ActionRequest {
            // The tracker gives id 0 its next ActionId.
            id: ActionId(0),
            kind: ActionKind::ShowInFileManager {
                path: entry.source_path.clone(),
                properties: false,
            },
            targets: SmallVec::new(),
            privileged: false,
        };
        // This page shows the toasts: Info now, Error when the request does not succeed.
        let texts = ToastTexts {
            owner_toasts: true,
            ..ToastTexts::new(SharedString::default(), SharedString::default())
        };
        let Some(id) = tracker.update(cx, |t, cx| {
            t.track_or_toast(request, queue.downgrade(), texts, cx)
        }) else {
            return;
        };
        self.open_requests.push(id);
        let text = tr(cx, "startup-open-location-opening");
        push_toast(window, cx, ToastKind::Info, text, Some(ToastKey(id.0)));
    }

    /// A request finished: when it was an Open file location that did not succeed, its toast
    /// becomes the Error.
    pub(super) fn on_open_location_finished(
        &mut self,
        id: ActionId,
        failed: u16,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.open_requests.iter().position(|open| *open == id) else {
            return;
        };
        self.open_requests.swap_remove(at);
        if failed == 0 {
            return;
        }
        let text = tr(cx, "startup-open-location-failed");
        let _ = self.window.update(cx, |_, window, cx| {
            push_toast(window, cx, ToastKind::Error, text, Some(ToastKey(id.0)));
        });
    }
}

fn push_toast(
    window: &Window,
    cx: &mut gpui_kit::App,
    kind: ToastKind,
    text: SharedString,
    key: Option<ToastKey>,
) {
    if let Some(queue) = ToastQueue::of_window(window, cx) {
        queue.update(cx, |q, cx| q.push(kind, text, key, None, cx));
    }
}

/// The eight "Copy row" lines. Location is the full path, so the text works in a shell.
fn write_block(prepared: &PreparedCopy, entry: &StartupEntry, out: &mut String) {
    let words = prepared.words.as_ref();
    let _ = writeln!(out, "Name: {}", entry.name);
    let _ = writeln!(out, "Publisher: {}", publisher(words, entry));
    let _ = writeln!(out, "Status: {}", status(words, entry));
    let _ = writeln!(out, "Startup impact: {}", impact(words, entry));
    let _ = writeln!(out, "Type: {}", kind(words, entry));
    let _ = writeln!(out, "Location: {}", entry.location);
    let _ = writeln!(out, "Command: {}", entry.command);
    let _ = write!(
        out,
        "Resolved path: {}",
        entry
            .resolved_path
            .as_deref()
            .unwrap_or(prepared.not_found.as_ref())
    );
}

/// The cell text as the table shows it (Location with `~`).
fn write_cell(
    prepared: &PreparedCopy,
    entry: &StartupEntry,
    column: StartupColumn,
    out: &mut String,
) {
    let words = prepared.words.as_ref();
    match column {
        StartupColumn::Name => out.push_str(&entry.name),
        StartupColumn::Publisher => out.push_str(publisher(words, entry)),
        StartupColumn::Status => out.push_str(status(words, entry)),
        StartupColumn::Impact => out.push_str(impact(words, entry)),
        StartupColumn::Type => out.push_str(kind(words, entry)),
        StartupColumn::Location => match home_tilde(&entry.location, &prepared.home) {
            Some(shown) => out.push_str(&shown),
            None => out.push_str(&entry.location),
        },
        StartupColumn::Command => out.push_str(&entry.command),
    }
}

fn publisher<'a>(words: Option<&'a StartupWords>, entry: &'a StartupEntry) -> &'a str {
    match (&entry.publisher, words) {
        (Some(p), Some(w)) if &**p == "User" => &w.user,
        (Some(p), _) => p,
        (None, _) => "",
    }
}

fn status<'a>(words: Option<&'a StartupWords>, entry: &StartupEntry) -> &'a str {
    words.map_or("", |w| w.status(entry.status))
}

fn impact<'a>(words: Option<&'a StartupWords>, entry: &StartupEntry) -> &'a str {
    words.map_or("", |w| w.impact(entry.impact))
}

fn kind<'a>(words: Option<&'a StartupWords>, entry: &StartupEntry) -> &'a str {
    words.map_or("", |w| w.kind(entry.kind))
}

// `SysCopySource` predates system-table column ids. These aliases are private transport values;
// clipboard labels and text always come from the Startup columns.
fn column_alias(column: StartupColumn) -> ColumnId {
    match column {
        StartupColumn::Name => ColumnId::Name,
        StartupColumn::Publisher => ColumnId::Company,
        StartupColumn::Status => ColumnId::State,
        StartupColumn::Impact => ColumnId::Priority,
        StartupColumn::Type => ColumnId::Type,
        StartupColumn::Location => ColumnId::ImagePath,
        StartupColumn::Command => ColumnId::Cmdline,
    }
}

fn alias_column(alias: ColumnId) -> Option<StartupColumn> {
    Some(match alias {
        ColumnId::Name => StartupColumn::Name,
        ColumnId::Company => StartupColumn::Publisher,
        ColumnId::State => StartupColumn::Status,
        ColumnId::Priority => StartupColumn::Impact,
        ColumnId::Type => StartupColumn::Type,
        ColumnId::ImagePath => StartupColumn::Location,
        ColumnId::Cmdline => StartupColumn::Command,
        _ => return None,
    })
}
