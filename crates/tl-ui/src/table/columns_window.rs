//! The column picker window "tasklens Columns" (UI-PRC-007, R-106, R-181, R-065, PROC-024).
//!
//! One table-generic tool window. The table that opens it passes itself as a [`ColumnHost`]; the
//! window lists the host's columns under their group headings, applies each toggle at once and
//! re-renders when the host's layout changes. There is one picker (`WindowKey::Columns`): opening
//! it again focuses the window and binds it to the table that asked.
//!
//! The host names columns by their `TableColumn::key()`, so the process tables (`ColumnId`) and
//! the system tables (`SysColumnId`) share the window, as they share the column actions.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, ElementId, Entity, FocusHandle,
    FontWeight, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, ScrollStrategy,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Task,
    UniformListScrollHandle, Window, div, prelude::FluentBuilder as _, px, size, uniform_list,
};

use crate::components::button::{TlButton, TlButtonKind};
use crate::components::checkbox::TlCheckbox;
use crate::components::icons::TlIcon;
use crate::components::input::{TlInputSize, TlTextInput};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{
    CloseSecondaryWindow, TlSecondaryWindow, TlWindowKind, TlWindowSpec,
};
use crate::i18n::{tr, tr_args};
use crate::table::columns::{Availability, ColumnError, TableId};
use crate::text::style::{label_style, t_base, t_sm};
use crate::theme::{TlTheme, UiMetrics, current, metrics};
use crate::window::registry::{GlobalRegistry, OpenOutcome, WindowKey};

/// Default window size, × ui_scale (R-106).
pub const PICKER_SIZE: (f32, f32) = (420., 560.);
/// Minimum window size (R-106).
pub const PICKER_MIN_SIZE: (f32, f32) = (320., 400.);
/// Height of every list item, headings included, × ui_scale.
const ITEM_H: f32 = 28.;
/// Checkbox and label x in a column row, × ui_scale.
const CHECK_X: f32 = 12.;
const LABEL_X: f32 = 36.;
/// The filter applies this long after the last change.
pub const FILTER_DEBOUNCE: Duration = Duration::from_millis(100);

/// Key context of the picker root.
pub const CONTEXT: &str = "ColumnPicker";
/// Key context of the column list.
pub const LIST_CONTEXT: &str = "ColumnPickerList";

gpui_kit::actions!(
    tasklens,
    [
        ColumnPickerPrev,
        ColumnPickerNext,
        ColumnPickerFirst,
        ColumnPickerLast,
        ColumnPickerToggle,
        /// Clears the filter when it has text, else closes the window.
        ColumnPickerEscape,
        ColumnPickerReset,
    ]
);

/// Binds the list keys and Esc (UI-PRC-007). `bind_keys_table` calls it.
pub fn bind_keys_column_picker(cx: &mut App) {
    let list = Some(LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", ColumnPickerPrev, list),
        KeyBinding::new("down", ColumnPickerNext, list),
        KeyBinding::new("home", ColumnPickerFirst, list),
        KeyBinding::new("end", ColumnPickerLast, list),
        KeyBinding::new("space", ColumnPickerToggle, list),
        KeyBinding::new("escape", ColumnPickerEscape, Some(CONTEXT)),
    ]);
}

/// One column as the picker shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerColumn {
    /// `TableColumn::key()`.
    pub key: &'static str,
    /// Header line 1, the alias, e.g. "Memory".
    pub label: SharedString,
    /// e.g. "Working set private". Equals `label` when there is no alias.
    pub full_name: SharedString,
    pub group: SharedString,
    pub availability: Availability,
    /// False for Name: always on.
    pub hideable: bool,
    pub visible: bool,
}

impl PickerColumn {
    /// The row text: `Working set private - "Memory"`, or the label alone without an alias.
    pub fn display_name(&self) -> SharedString {
        if self.full_name == self.label {
            self.label.clone()
        } else {
            format!("{} - \"{}\"", self.full_name, self.label).into()
        }
    }

    /// The string key of the right-aligned note: "always on" for Name, "n/a on this system" for
    /// a column this system lacks.
    pub fn note_key(&self) -> Option<&'static str> {
        if !self.hideable {
            Some("table-picker-always-on")
        } else if self.availability == Availability::NotOnThisSystem {
            Some("table-picker-na")
        } else if self.key == "remote_host" {
            Some("connections-remote-host-picker-note")
        } else {
            None
        }
    }

    /// Case-insensitive substring match over label, full name and group. `needle` is lowercase;
    /// an empty needle matches every column.
    fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || [&self.label, &self.full_name, &self.group]
                .iter()
                .any(|s| s.to_lowercase().contains(needle))
    }
}

/// What the picker needs from the table that opened it.
pub trait ColumnHost: 'static {
    /// The first list line, e.g. "Processes table".
    fn table_name(&self, cx: &App) -> SharedString;
    /// Every column of the table's registry in layout order, with its visibility. Empty once
    /// the table is gone.
    fn columns(&self, cx: &App) -> Vec<PickerColumn>;
    /// Shows or hides `key`; a re-shown column returns to its remembered slot and width. The
    /// table persists the change.
    fn set_visible(
        &self,
        key: &str,
        on: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), ColumnError>;
    /// The same as `ResetColumns` on the table.
    fn reset(&self, window: &mut Window, cx: &mut App);
    /// Notifies `picker` whenever the table changes. `None` once the table is gone.
    fn observe(&self, picker: &mut Context<ColumnPicker>) -> Option<Subscription>;
}

/// One list item. Every item is `ITEM_H` high.
#[derive(Clone, Debug, PartialEq)]
pub enum PickerItem {
    TableName(SharedString),
    /// The group name, uppercased.
    Heading(SharedString),
    Column(PickerColumn),
}

/// The list for `columns` under `needle` (lowercase): the table name, then each group in the
/// order of its first column, with its matching columns. Groups left empty are dropped.
pub fn picker_items(
    table_name: SharedString,
    columns: &[PickerColumn],
    needle: &str,
) -> Vec<PickerItem> {
    let mut groups: Vec<(&SharedString, Vec<&PickerColumn>)> = Vec::new();
    for col in columns {
        let slot = match groups.iter().position(|(g, _)| **g == col.group) {
            Some(i) => i,
            None => {
                groups.push((&col.group, Vec::new()));
                groups.len() - 1
            }
        };
        if col.matches(needle) {
            groups[slot].1.push(col);
        }
    }
    let mut items = vec![PickerItem::TableName(table_name)];
    for (group, cols) in groups.into_iter().filter(|(_, c)| !c.is_empty()) {
        items.push(PickerItem::Heading(group.to_uppercase().into()));
        items.extend(cols.into_iter().cloned().map(PickerItem::Column));
    }
    items
}

/// The window content.
pub struct ColumnPicker {
    host: Rc<dyn ColumnHost>,
    filter: Entity<InputState>,
    /// The lowercase filter text the last debounce applied.
    needle: String,
    debounce: Option<Task<()>>,
    list_focus: FocusHandle,
    /// The column row the list keys act on.
    cursor: Option<&'static str>,
    scroll: UniformListScrollHandle,
    /// This frame's items, read by the list's item builder.
    items: Rc<[PickerItem]>,
    _host: Option<Subscription>,
    _filter: Subscription,
}

impl ColumnPicker {
    pub fn new(host: Rc<dyn ColumnHost>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx));
        let _filter = cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule_filter(cx);
            }
        });
        filter.update(cx, |s, cx| s.focus(window, cx));
        let _host = host.observe(cx);
        Self {
            host,
            filter,
            needle: String::new(),
            debounce: None,
            list_focus: cx.focus_handle().tab_stop(true),
            cursor: None,
            scroll: UniformListScrollHandle::new(),
            items: Rc::from([]),
            _host,
            _filter,
        }
    }

    /// Binds the picker to another table.
    pub fn set_host(&mut self, host: Rc<dyn ColumnHost>, cx: &mut Context<Self>) {
        self._host = host.observe(cx);
        self.host = host;
        self.cursor = None;
        cx.notify();
    }

    pub fn filter(&self) -> &Entity<InputState> {
        &self.filter
    }

    pub fn list_focus(&self) -> &FocusHandle {
        &self.list_focus
    }

    /// The column row the list keys act on.
    pub fn cursor(&self) -> Option<&'static str> {
        self.cursor
    }

    /// The list as it renders now.
    pub fn items(&self, cx: &App) -> Vec<PickerItem> {
        picker_items(
            self.host.table_name(cx),
            &self.host.columns(cx),
            &self.needle,
        )
    }

    /// The footer text, e.g. "9 of 96 columns shown".
    pub fn count_text(&self, cx: &App) -> SharedString {
        count_text(&self.host.columns(cx), cx)
    }

    fn schedule_filter(&mut self, cx: &mut Context<Self>) {
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FILTER_DEBOUNCE).await;
            this.update(cx, |this, cx| this.apply_filter(cx)).ok();
        }));
    }

    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        self.debounce = None;
        self.needle = self.filter.read(cx).text().to_string().to_lowercase();
        cx.notify();
    }

    /// Shows or hides `key` through the host. Hiding the last visible column shows the Warning
    /// toast in this window.
    fn set_visible(&mut self, key: &str, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.host.set_visible(key, on, window, cx) {
            Ok(()) => {}
            Err(ColumnError::LastVisible) => {
                let text = tr(cx, "table-toast-last-visible");
                if let Some(queue) = ToastQueue::of_window(window, cx) {
                    queue.update(cx, |q, cx| q.push(ToastKind::Warning, text, None, None, cx));
                }
            }
            Err(err) => {
                tracing::debug!(target: "tasklens::ui", ?err, key, "picker toggle refused");
            }
        }
        cx.notify();
    }

    /// Flips the column `key`. Name does not change.
    fn toggle(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(col) = self
            .host
            .columns(cx)
            .into_iter()
            .find(|c| c.key == key && c.hideable)
        else {
            return;
        };
        self.set_visible(key, !col.visible, window, cx);
    }

    /// The item indexes of the column rows.
    fn column_rows(&self) -> impl Iterator<Item = (usize, &'static str)> + '_ {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                PickerItem::Column(c) => Some((i, c.key)),
                _ => None,
            })
    }

    fn move_cursor(&mut self, pick: impl FnOnce(&[(usize, &'static str)], Option<usize>) -> usize) {
        let rows: Vec<_> = self.column_rows().collect();
        if rows.is_empty() {
            return;
        }
        let at = self
            .cursor
            .and_then(|key| rows.iter().position(|(_, k)| *k == key));
        let to = pick(&rows, at).min(rows.len() - 1);
        self.cursor = Some(rows[to].1);
        self.scroll
            .scroll_to_item(rows[to].0, ScrollStrategy::Nearest);
    }

    fn on_prev(&mut self, _: &ColumnPickerPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(|_, at| at.map_or(0, |i| i.saturating_sub(1)));
        cx.notify();
    }

    fn on_next(&mut self, _: &ColumnPickerNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(|_, at| at.map_or(0, |i| i + 1));
        cx.notify();
    }

    fn on_first(&mut self, _: &ColumnPickerFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(|_, _| 0);
        cx.notify();
    }

    fn on_last(&mut self, _: &ColumnPickerLast, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(|rows, _| rows.len() - 1);
        cx.notify();
    }

    fn on_toggle(&mut self, _: &ColumnPickerToggle, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.cursor {
            self.toggle(key, window, cx);
        }
    }

    fn on_escape(&mut self, _: &ColumnPickerEscape, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter.read(cx).text().len() > 0 {
            self.filter.update(cx, |s, cx| s.clean(window, cx));
        } else {
            window.remove_window();
        }
    }

    fn on_reset(&mut self, _: &ColumnPickerReset, window: &mut Window, cx: &mut Context<Self>) {
        self.host.reset(window, cx);
        cx.notify();
    }

    /// The list items in `range`.
    fn build_list_items(
        &mut self,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let scale = |v: f32| px(v * m.ui_scale);
        let list_focused = self.list_focus.is_focused(window);
        let items = self.items.clone();
        range
            .filter_map(|ix| items.get(ix).map(|item| (ix, item)))
            .map(|(ix, item)| {
                let row = div().w_full().h(scale(ITEM_H)).flex_none();
                match item {
                    PickerItem::TableName(name) => {
                        let mut row = row
                            .debug_selector(|| "ColumnPicker-table-name".into())
                            .flex()
                            .items_center()
                            .pl(scale(CHECK_X))
                            .text_color(theme.text_secondary);
                        *row.text_style() = t_sm(&m);
                        row.child(name.clone()).into_any_element()
                    }
                    PickerItem::Heading(group) => {
                        let mut row = row
                            .flex()
                            .items_end()
                            .pl(scale(CHECK_X))
                            .text_color(theme.text_secondary);
                        *row.text_style() = label_style(&m, m.text.xs, FontWeight::SEMIBOLD);
                        row.child(group.clone()).into_any_element()
                    }
                    PickerItem::Column(col) => {
                        self.column_row(ix, col, list_focused, &m, &theme, cx)
                    }
                }
            })
            .collect()
    }

    fn column_row(
        &self,
        ix: usize,
        col: &PickerColumn,
        list_focused: bool,
        m: &UiMetrics,
        theme: &TlTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scale = |v: f32| px(v * m.ui_scale);
        let key = col.key;
        let note = col.note_key().map(|k| tr(cx, k));
        let check_id = ElementId::Name(format!("column-picker-check-{key}").into());
        let mut label = div()
            .absolute()
            .left(scale(LABEL_X))
            .right(scale(CHECK_X))
            .top_0()
            .bottom_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(theme.text_primary);
        *label.text_style() = t_base(m);
        let note = note.map(|note| {
            let mut el = div()
                .debug_selector(move || format!("ColumnPicker-note-{key}"))
                .absolute()
                .right(scale(CHECK_X))
                .top_0()
                .bottom_0()
                .flex()
                .items_center()
                .text_color(theme.text_secondary);
            *el.text_style() = t_sm(m);
            el.child(note)
        });
        let focused = list_focused && self.cursor == Some(key);
        div()
            .id(("column-picker-row", ix))
            .debug_selector(move || format!("ColumnPicker-row-{key}"))
            .relative()
            .w_full()
            .h(scale(ITEM_H))
            .flex_none()
            .when(col.hideable, |d| {
                d.hover(|s| s.bg(theme.row_hover)).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.cursor = Some(key);
                        this.list_focus.focus(window, cx);
                        this.toggle(key, window, cx);
                    },
                ))
            })
            .when(focused, |d| d.border_1().border_color(theme.accent))
            .child(
                div()
                    .absolute()
                    .left(scale(CHECK_X))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(
                        TlCheckbox::new(check_id)
                            .checked(Some(col.visible || !col.hideable))
                            .disabled(!col.hideable)
                            .tab_stop(false),
                    ),
            )
            .child(label.child(col.display_name()))
            .children(note)
            .into_any_element()
    }
}

/// "{shown} of {total} columns shown".
fn count_text(columns: &[PickerColumn], cx: &App) -> SharedString {
    let shown = columns.iter().filter(|c| c.visible).count();
    tr_args(
        cx,
        "table-picker-count",
        &[
            ("shown", shown.to_string().into()),
            ("total", columns.len().to_string().into()),
        ],
    )
}

impl Render for ColumnPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let columns = self.host.columns(cx);
        let count = count_text(&columns, cx);
        let items: Rc<[PickerItem]> =
            picker_items(self.host.table_name(cx), &columns, &self.needle).into();
        let any_column = items.iter().any(|i| matches!(i, PickerItem::Column(_)));
        let len = items.len();
        self.items = items;

        let list = if any_column {
            uniform_list(
                "column-picker-list",
                len,
                cx.processor(Self::build_list_items),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        } else {
            let mut empty = div()
                .debug_selector(|| "ColumnPicker-no-match".into())
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.text_secondary);
            *empty.text_style() = t_base(&m);
            empty
                .child(tr(cx, "table-picker-no-match"))
                .into_any_element()
        };

        let mut count_el = div()
            .debug_selector(|| "ColumnPicker-count".into())
            .text_color(theme.text_secondary);
        *count_el.text_style() = t_sm(&m);

        div()
            .id("ColumnPicker")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(Self::on_reset))
            .size_full()
            .flex()
            .flex_col()
            .p(m.sp[5])
            .gap(m.sp[4])
            .child(
                TlTextInput::new("column-picker-filter", &self.filter)
                    .size(TlInputSize::Md)
                    .prefix_icon(TlIcon::Search)
                    .clear_button(true)
                    .clean_on_escape(true)
                    .placeholder(tr(cx, "table-picker-filter-placeholder")),
            )
            .child(
                div()
                    .id("column-picker-list-box")
                    .key_context(LIST_CONTEXT)
                    .track_focus(&self.list_focus)
                    .on_action(cx.listener(Self::on_prev))
                    .on_action(cx.listener(Self::on_next))
                    .on_action(cx.listener(Self::on_first))
                    .on_action(cx.listener(Self::on_last))
                    .on_action(cx.listener(Self::on_toggle))
                    .flex_1()
                    .min_h_0()
                    .child(list),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap(m.sp[4])
                    .child(count_el.child(count))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(m.sp[4])
                            .child(
                                TlButton::new("column-picker-reset", TlButtonKind::Ghost)
                                    .label(tr(cx, "table-picker-reset"))
                                    .action(Box::new(ColumnPickerReset)),
                            )
                            .child(
                                TlButton::new("column-picker-close", TlButtonKind::Primary)
                                    .label(tr(cx, "table-picker-close"))
                                    .action(Box::new(CloseSecondaryWindow)),
                            ),
                    ),
            )
    }
}

/// The text of the first list line for `table`.
pub fn table_name(table: TableId, cx: &App) -> SharedString {
    let key = match table {
        TableId::Processes => "table-picker-name-processes",
        TableId::History => "table-picker-name-history",
        TableId::Services => "table-picker-name-services",
        TableId::Startup => "table-picker-name-startup",
        TableId::Connections => "table-picker-name-connections",
    };
    tr(cx, key)
}

/// Opens the picker bound to `host`, or focuses the open one and binds it to `host` (R-106).
/// A new picker centres on `parent`, the table's window, or on the screen without one. Without a
/// window registry (tests) it always opens a window.
pub fn open_column_picker(host: Rc<dyn ColumnHost>, parent: Option<AnyWindowHandle>, cx: &mut App) {
    let ui_scale = metrics(cx).ui_scale;
    let (w, h) = PICKER_SIZE;
    let spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        tr(cx, "table-picker-title"),
        size(px(w * ui_scale), px(h * ui_scale)),
    )
    .min_size(size(px(PICKER_MIN_SIZE.0), px(PICKER_MIN_SIZE.1)))
    .key(WindowKey::Columns);
    let spec = match parent {
        Some(parent) => spec.parent(parent),
        None => spec,
    };
    let open_host = host.clone();
    let open = move |cx: &mut App| {
        TlSecondaryWindow::open(cx, spec, move |window, cx| {
            cx.new(|cx| ColumnPicker::new(open_host, window, cx))
        })
        .map(Into::into)
    };
    let Some(registry) = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) else {
        let _ = open(cx);
        return;
    };
    let outcome = registry.update(cx, |r, cx| r.open_or_focus(WindowKey::Columns, cx, open));
    if outcome != OpenOutcome::Focused {
        return;
    }
    let Some(handle) = registry
        .read(cx)
        .get(&WindowKey::Columns)
        .and_then(|h| h.downcast::<Root>())
    else {
        return;
    };
    if let Some(frame) = TlSecondaryWindow::<ColumnPicker>::find(handle, cx) {
        let picker = frame.read(cx).content().clone();
        picker.update(cx, |p, cx| p.set_host(host, cx));
    }
}
