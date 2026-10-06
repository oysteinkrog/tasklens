//! `TlSelect` and `TlCombo`: dropdowns for enumerated settings (UI-FND-017, APP-023
//! `general.language`, `general.start_tab`, `units.network`, `performance.cpu_graph_mode`, the
//! PERF time range chooser, SYS-012, SYS-013).
//!
//! The view owns one `Entity<TlSelectState>` per dropdown. The state holds the items, the
//! committed selection, the open flag and the keyboard highlight. Call
//! [`TlSelectState::set_selected_value`] when the setting changes elsewhere; it fires nothing.
//! `on_change(value)` fires when the user confirms an item (Enter or a click), and never while
//! the highlight moves. It also fires when the confirmed item is the current one.
//!
//! `TlCombo` is a `TlSelect` with a `TlTextInput` at the top of the popup that filters the items
//! by label. The field shows only when the list has [`COMBO_FILTER_MIN_ITEMS`] or more items.
//!
//! Built on the kit's unstyled base `Select` and `Combobox` roots, which own the combobox role,
//! Enter and the arrow keys opening a closed control, Esc closing an open one, and moving focus
//! between the trigger and the popup. The styled kit `Select` over a kit `SelectState` is not
//! used because it cannot meet the spec:
//! - Its state keeps the list cursor private, so type-ahead cannot move the highlight.
//! - Its trigger has no hover border and its chevron does not turn when open.
//! - Its list rows have no fixed height and the popup pads them, so 10 rows do not fill 260 px.
//! - Its filter field is a kit `Input` inside the kit `List`, not a `TlTextInput`, and a plain
//!   `Vec` of items does not filter at all.
//!
//! The app calls [`bind_keys`] once: it binds Space and Alt+Down to open a closed control.

use std::f32::consts::PI;
use std::rc::Rc;

use gpui_kit::base::actions::{Confirm, SelectDown, SelectUp};
use gpui_kit::base::{Combobox, Select};
use gpui_kit::component::input::{Enter, Escape, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::{
    App, AppContext as _, Bounds, BoxShadow, Context, ElementId, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton, ParentElement, Pixels,
    RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Window, anchored, canvas, deferred, div, point, prelude::FluentBuilder as _, px, radians,
};

use super::icons::{TlIcon, icon};
use super::input::{TlInputSize, TlTextInput};
use crate::text::style::t_base;
use crate::theme::{current, metrics};

gpui_kit::actions!(tasklens, [OpenSelect]);

/// Key context around the trigger and the popup, where [`bind_keys`] binds Space and Alt+Down.
pub const KEY_CONTEXT: &str = "TlSelect";

/// A `TlCombo` list with at least this many items gets the filter field. UI-FND-017 says both
/// "more than 12" and "the language list has 12, so it gets one"; the example wins.
pub const COMBO_FILTER_MIN_ITEMS: usize = 12;

/// The text of the row shown when the filter matches no item.
pub const NO_MATCHES: &str = "No matches";

/// Sizes before the UI scale.
const TRIGGER_HEIGHT: f32 = 28.;
const TRIGGER_MIN_WIDTH: f32 = 160.;
const ITEM_HEIGHT: f32 = 26.;
/// Ten items, then the list scrolls.
const LIST_MAX_HEIGHT: f32 = 260.;
const EMPTY_ROW_HEIGHT: f32 = 40.;
/// Gap between the trigger and the popup.
const POPUP_GAP: f32 = 2.;
/// The popup keeps this far from the window edges.
const WINDOW_MARGIN: f32 = 8.;

/// Binds Space and Alt+Down to open a closed `TlSelect` or `TlCombo`. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("space", OpenSelect, ctx),
        KeyBinding::new("alt-down", OpenSelect, ctx),
    ]);
}

/// One choice: the stored setting value and the label the user sees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlSelectItem {
    pub value: SharedString,
    pub label: SharedString,
}

impl TlSelectItem {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// The items, the committed selection and the popup state of one dropdown.
pub struct TlSelectState {
    items: Vec<TlSelectItem>,
    selected: Option<usize>,
    open: bool,
    /// The keyboard and hover highlight while open, an index into `items`.
    highlight: Option<usize>,
    /// The filter applies only while a combo popup with the field is open.
    filtering: bool,
    trigger_focus: FocusHandle,
    popup_focus: FocusHandle,
    /// Created the first time a `TlCombo` with enough items renders.
    filter: Option<Entity<InputState>>,
    scroll: ScrollHandle,
    trigger_bounds: Bounds<Pixels>,
    _filter_events: Option<Subscription>,
}

impl TlSelectState {
    /// `selected` is the value of the current setting. A value not among the items selects
    /// nothing.
    pub fn new(items: Vec<TlSelectItem>, selected: Option<&str>, cx: &mut Context<Self>) -> Self {
        let selected = selected.and_then(|v| items.iter().position(|i| i.value == v));
        Self {
            items,
            selected,
            open: false,
            highlight: None,
            filtering: false,
            trigger_focus: cx.focus_handle(),
            popup_focus: cx.focus_handle(),
            filter: None,
            scroll: ScrollHandle::new(),
            trigger_bounds: Bounds::default(),
            _filter_events: None,
        }
    }

    pub fn items(&self) -> &[TlSelectItem] {
        &self.items
    }

    /// Replaces the items and keeps the selection when its value is still there.
    pub fn set_items(&mut self, items: Vec<TlSelectItem>, cx: &mut Context<Self>) {
        let value = self.selected_value().cloned();
        self.items = items;
        self.selected = value.and_then(|v| self.items.iter().position(|i| i.value == v));
        self.highlight = self.selected;
        cx.notify();
    }

    pub fn selected_value(&self) -> Option<&SharedString> {
        self.selected.map(|ix| &self.items[ix].value)
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// Shows `value` as selected, for a setting changed elsewhere. Fires no `on_change`. A value
    /// not among the items selects nothing.
    pub fn set_selected_value(&mut self, value: &str, cx: &mut Context<Self>) {
        let selected = self.items.iter().position(|i| i.value == value);
        if selected != self.selected {
            self.selected = selected;
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The highlighted item while open.
    pub fn highlighted_index(&self) -> Option<usize> {
        self.highlight
    }

    fn ensure_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx));
        self._filter_events = Some(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let visible = this.visible(cx);
                if !this.highlight.is_some_and(|h| visible.contains(&h)) {
                    this.highlight = visible.first().copied();
                }
                this.scroll_to_highlight(cx);
                cx.notify();
            }
        }));
        self.filter = Some(input);
    }

    /// The indexes of the items the popup lists, in order.
    fn visible(&self, cx: &App) -> Vec<usize> {
        let query = match (&self.filter, self.filtering) {
            (Some(filter), true) => filter.read(cx).text().to_string().to_lowercase(),
            _ => String::new(),
        };
        (0..self.items.len())
            .filter(|&ix| query.is_empty() || self.items[ix].label.to_lowercase().contains(&query))
            .collect()
    }

    fn scroll_to_highlight(&self, cx: &App) {
        if let Some(pos) = self
            .highlight
            .and_then(|h| self.visible(cx).iter().position(|&ix| ix == h))
        {
            self.scroll.scroll_to_item(pos);
        }
    }

    /// Opens the popup with the highlight on the selected item.
    fn open(&mut self, filterable: bool, cx: &mut Context<Self>) {
        if self.open {
            return;
        }
        self.open = true;
        self.filtering = filterable && self.filter.is_some();
        self.highlight = self.selected.or((!self.items.is_empty()).then_some(0));
        self.scroll_to_highlight(cx);
        cx.notify();
    }

    /// Closes the popup without a change and gives the trigger focus.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.filtering = false;
        if let Some(filter) = &self.filter
            && filter.read(cx).text().len() > 0
        {
            filter.update(cx, |f, cx| f.set_value("", window, cx));
        }
        self.trigger_focus.focus(window, cx);
        cx.notify();
    }

    /// The handle the root moves focus to when the popup opens: the filter field when the
    /// popup shows one, else the popup itself.
    fn content_focus(&self, filterable: bool, cx: &App) -> FocusHandle {
        match &self.filter {
            Some(filter) if filterable => filter.focus_handle(cx),
            _ => self.popup_focus.clone(),
        }
    }

    /// Moves the highlight one visible item up or down, wrapping.
    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let visible = self.visible(cx);
        let n = visible.len();
        if n == 0 {
            return;
        }
        let pos = match self
            .highlight
            .and_then(|h| visible.iter().position(|&ix| ix == h))
        {
            Some(pos) if forward => (pos + 1) % n,
            Some(pos) => (pos + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        self.highlight = Some(visible[pos]);
        self.scroll_to_highlight(cx);
        cx.notify();
    }

    /// Moves the highlight to the next visible item whose label starts with `ch`, wrapping.
    fn type_ahead(&mut self, ch: char, cx: &mut Context<Self>) {
        let visible = self.visible(cx);
        let n = visible.len();
        let start = self
            .highlight
            .and_then(|h| visible.iter().position(|&ix| ix == h));
        let ch = ch.to_lowercase().collect::<String>();
        for step in 1..=n {
            let pos = start.map_or(step - 1, |s| (s + step) % n);
            let ix = visible[pos];
            if self.items[ix].label.to_lowercase().starts_with(&ch) {
                self.highlight = Some(ix);
                self.scroll_to_highlight(cx);
                cx.notify();
                return;
            }
        }
    }

    /// Commits item `ix`, closes and returns its value.
    fn commit(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> SharedString {
        self.selected = Some(ix);
        self.close(window, cx);
        self.items[ix].value.clone()
    }

    /// Commits the highlighted item when it is listed. With no match there is nothing to commit.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<SharedString> {
        let ix = self.highlight.filter(|h| self.visible(cx).contains(h))?;
        Some(self.commit(ix, window, cx))
    }
}

impl Focusable for TlSelectState {
    /// The trigger's handle.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.trigger_focus.clone()
    }
}

type ChangeHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// Calls `on_change` with the value `confirm` returns, if any.
fn fire(
    value: Option<SharedString>,
    on_change: &Option<ChangeHandler>,
    w: &mut Window,
    cx: &mut App,
) {
    if let (Some(value), Some(f)) = (value, on_change) {
        f(&value, w, cx);
    }
}

/// A dropdown trigger that looks like a md `TlTextInput`, with a popup list of the items.
#[derive(IntoElement)]
pub struct TlSelect {
    id: ElementId,
    state: Entity<TlSelectState>,
    width: Option<Pixels>,
    disabled: bool,
    on_change: Option<ChangeHandler>,
    combo: bool,
}

impl TlSelect {
    pub fn new(id: impl Into<ElementId>, state: &Entity<TlSelectState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            width: None,
            disabled: false,
            on_change: None,
            combo: false,
        }
    }

    /// A fixed trigger width. Without it the trigger fits its label, at least 160 × ui_scale.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Opacity 0.5, not focusable, does not open.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the item's value when the user confirms an item.
    pub fn on_change(mut self, f: impl Fn(&SharedString, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

/// A `TlSelect` whose popup has a filter field once the list has [`COMBO_FILTER_MIN_ITEMS`]
/// items.
#[derive(IntoElement)]
pub struct TlCombo(TlSelect);

impl TlCombo {
    pub fn new(id: impl Into<ElementId>, state: &Entity<TlSelectState>) -> Self {
        Self(TlSelect {
            combo: true,
            ..TlSelect::new(id, state)
        })
    }

    pub fn width(self, width: Pixels) -> Self {
        Self(self.0.width(width))
    }

    pub fn disabled(self, disabled: bool) -> Self {
        Self(self.0.disabled(disabled))
    }

    pub fn on_change(self, f: impl Fn(&SharedString, &mut Window, &mut App) + 'static) -> Self {
        Self(self.0.on_change(f))
    }
}

impl RenderOnce for TlCombo {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.0.render(window, cx)
    }
}

impl RenderOnce for TlSelect {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = |v: f32| px(v * m.ui_scale);
        let state = self.state;
        let disabled = self.disabled;
        let on_change = self.on_change;

        let filterable = self.combo && state.read(cx).items.len() >= COMBO_FILTER_MIN_ITEMS;
        if filterable {
            state.update(cx, |st, cx| st.ensure_filter(window, cx));
        }

        let st = state.read(cx);
        let open = st.open && !disabled;
        let trigger_focus = st.trigger_focus.clone();
        let content_focus = st.content_focus(filterable, cx);
        let focused = !disabled && trigger_focus.is_focused(window);
        let label = st
            .selected
            .map(|ix| st.items[ix].label.clone())
            .unwrap_or_default();

        let border = if open || focused {
            theme.accent
        } else {
            theme.divider
        };
        let hover_border = theme.text_secondary;
        let chevron = icon(TlIcon::ChevronDown, m.icon.xs, theme.text_secondary)
            .when(open, |i| i.rotate(radians(PI)));

        let mut text = div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(label.clone());
        *text.text_style() = t_base(&m);

        let trigger = div()
            .id((self.id.clone(), "trigger"))
            .debug_selector({
                let id = self.id.clone();
                move || format!("TlSelect-{id}")
            })
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(m.sp[3])
            .h(s(TRIGGER_HEIGHT))
            .min_w(s(TRIGGER_MIN_WIDTH))
            .when_some(self.width, |d, w| d.w(w))
            .px(m.sp[4])
            .rounded(m.radius_small)
            .border_1()
            .border_color(border)
            .bg(if focused {
                theme.input_focus
            } else {
                theme.input
            })
            .text_color(theme.text_primary)
            .when(disabled, |d| d.opacity(0.5))
            .when(!disabled && !open && !focused, |d| {
                d.hover(move |s| s.border_color(hover_border))
            })
            .when(!disabled, |d| {
                let state = state.clone();
                d.on_click(move |_, window, cx| {
                    state.update(cx, |st, cx| {
                        if st.open {
                            st.close(window, cx);
                        } else {
                            st.open(filterable, cx);
                            st.content_focus(filterable, cx).focus(window, cx);
                        }
                    });
                })
            })
            .child(
                canvas(
                    {
                        let state = state.clone();
                        move |bounds, _, cx| {
                            state.update(cx, |st, _| st.trigger_bounds = bounds);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(text)
            .child(
                div()
                    .debug_selector(|| "TlSelect-chevron".into())
                    .flex_none()
                    .child(chevron),
            );

        let popup = open.then(|| popup(&self.id, &state, filterable, &on_change, window, cx));

        let on_open_change = {
            let state = state.clone();
            move |open: bool, window: &mut Window, cx: &mut App| {
                state.update(cx, |st, cx| {
                    if open {
                        st.open(filterable, cx);
                    } else if st.open {
                        st.close(window, cx);
                    }
                });
            }
        };
        let children = [
            Some(trigger.into_any_element()),
            popup.map(IntoElement::into_any_element),
        ];
        let root = if self.combo {
            Combobox::new(self.id.clone())
                .open(open)
                .disabled(disabled)
                .focus_handle(&trigger_focus)
                .content_focus_handle(&content_focus)
                .on_open_change(on_open_change)
                .children(children.into_iter().flatten())
                .into_any_element()
        } else {
            Select::new(self.id.clone())
                .open(open)
                .disabled(disabled)
                .focus_handle(&trigger_focus)
                .content_focus_handle(&content_focus)
                .accessibility_value(label)
                .on_open_change(on_open_change)
                .children(children.into_iter().flatten())
                .into_any_element()
        };

        div()
            .key_context(KEY_CONTEXT)
            .flex_none()
            .on_action(move |_: &OpenSelect, window, cx| {
                if disabled || state.read(cx).open {
                    cx.propagate();
                    return;
                }
                state.update(cx, |st, cx| {
                    st.open(filterable, cx);
                    st.content_focus(filterable, cx).focus(window, cx);
                });
            })
            .child(root)
    }
}

/// The popup: optional filter field, then the item list, 260 × ui_scale high at most.
fn popup(
    id: &ElementId,
    state: &Entity<TlSelectState>,
    filterable: bool,
    on_change: &Option<ChangeHandler>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let theme = current(cx).clone();
    let m = *metrics(cx);
    let s = |v: f32| px(v * m.ui_scale);
    let st = state.read(cx);
    let visible = st.visible(cx);
    let bounds = st.trigger_bounds;
    let filter = st.filter.clone().filter(|_| filterable && st.filtering);

    let rows: Vec<_> = visible
        .iter()
        .map(|&ix| {
            let item = &st.items[ix];
            let highlighted = st.highlight == Some(ix);
            let checked = st.selected == Some(ix);
            let mut label = div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(item.label.clone());
            *label.text_style() = t_base(&m);
            let hover_state = state.clone();
            let click_state = state.clone();
            let on_change = on_change.clone();
            div()
                .id(("item", ix))
                .flex()
                .flex_none()
                .items_center()
                .gap(m.sp[3])
                .h(s(ITEM_HEIGHT))
                .px(m.sp[4])
                .text_color(theme.text_primary)
                .when(highlighted, |d| d.bg(theme.menu_hover))
                .on_hover(move |hovered, _, cx| {
                    if *hovered {
                        hover_state.update(cx, |st, cx| {
                            if st.highlight != Some(ix) {
                                st.highlight = Some(ix);
                                cx.notify();
                            }
                        });
                    }
                })
                .on_click(move |_, window, cx| {
                    let value = click_state.update(cx, |st, cx| st.commit(ix, window, cx));
                    fire(Some(value), &on_change, window, cx);
                })
                .child(label)
                .child(
                    div()
                        .flex_none()
                        .when(!checked, |d| d.invisible())
                        .child(icon(TlIcon::Check, m.icon.xs, theme.text_primary)),
                )
                .into_any_element()
        })
        .collect();

    let list = div()
        .id((id.clone(), "list"))
        .debug_selector(|| "TlSelect-list".into())
        .flex()
        .flex_col()
        .max_h(s(LIST_MAX_HEIGHT))
        .overflow_y_scroll()
        .track_scroll(&st.scroll)
        .map(|d| {
            if rows.is_empty() {
                let mut empty = div()
                    .debug_selector(|| "TlSelect-no-matches".into())
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(s(EMPTY_ROW_HEIGHT))
                    .text_color(theme.text_secondary)
                    .child(NO_MATCHES);
                *empty.text_style() = t_base(&m);
                d.child(empty)
            } else {
                d.children(rows)
            }
        });

    let field = filter.as_ref().map(|input| {
        div()
            .debug_selector(|| "TlSelect-filter".into())
            .p(m.sp[2])
            .border_b_1()
            .border_color(theme.divider)
            .child(TlTextInput::new((id.clone(), "filter"), input).size(TlInputSize::Sm))
    });

    let close_state = state.clone();
    let mut panel = div()
        .id((id.clone(), "popup"))
        .debug_selector(|| "TlSelect-popup".into())
        .occlude()
        .flex()
        .flex_col()
        .min_w(bounds.size.width)
        .bg(theme.menu_bg)
        .border_1()
        .border_color(theme.divider)
        .rounded(m.radius_small)
        .overflow_hidden()
        .on_mouse_down_out(move |event, window, cx| {
            // A press on the trigger is left to its click, which closes.
            if !bounds.contains(&event.position) {
                close_state.update(cx, |st, cx| st.close(window, cx));
            }
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(field)
        .child(list);
    panel.style().box_shadow = Some(vec![BoxShadow {
        color: theme.derived.shadow,
        offset: point(px(0.), px(2.)),
        blur_radius: px(8.),
        spread_radius: px(0.),
        inset: false,
    }]);

    let panel = if filter.is_some() {
        // The filter field holds focus and binds the arrows, Enter and Esc itself, so the popup
        // takes those actions first.
        let (up, down, enter, escape) =
            (state.clone(), state.clone(), state.clone(), state.clone());
        let on_change = on_change.clone();
        panel
            .capture_action(move |_: &MoveUp, _, cx| {
                up.update(cx, |st, cx| st.step(false, cx));
                cx.stop_propagation();
            })
            .capture_action(move |_: &MoveDown, _, cx| {
                down.update(cx, |st, cx| st.step(true, cx));
                cx.stop_propagation();
            })
            .capture_action(move |_: &Enter, window, cx| {
                cx.stop_propagation();
                let value = enter.update(cx, |st, cx| st.confirm(window, cx));
                fire(value, &on_change, window, cx);
            })
            .capture_action(move |_: &Escape, window, cx| {
                cx.stop_propagation();
                escape.update(cx, |st, cx| st.close(window, cx));
            })
    } else {
        let (up, down, enter, typed) = (state.clone(), state.clone(), state.clone(), state.clone());
        let on_change = on_change.clone();
        panel
            .track_focus(&st.popup_focus)
            .on_action(move |_: &SelectUp, _, cx| up.update(cx, |st, cx| st.step(false, cx)))
            .on_action(move |_: &SelectDown, _, cx| down.update(cx, |st, cx| st.step(true, cx)))
            .on_action(move |_: &Confirm, window, cx| {
                let value = enter.update(cx, |st, cx| st.confirm(window, cx));
                fire(value, &on_change, window, cx);
            })
            .on_key_down(move |event: &KeyDownEvent, _, cx| {
                if let Some(ch) = typed_letter(event) {
                    typed.update(cx, |st, cx| st.type_ahead(ch, cx));
                    cx.stop_propagation();
                }
            })
    };

    deferred(
        anchored()
            .position(point(bounds.left(), bounds.bottom() + s(POPUP_GAP)))
            .snap_to_window_with_margin(px(WINDOW_MARGIN))
            .child(panel),
    )
    .with_priority(1)
}

/// The letter or digit a key press types, with no modifier other than Shift.
fn typed_letter(event: &KeyDownEvent) -> Option<char> {
    let ks = &event.keystroke;
    let mods = ks.modifiers;
    if mods.control || mods.alt || mods.platform || mods.function {
        return None;
    }
    let text = ks.key_char.as_deref().unwrap_or(&ks.key);
    let mut chars = text.chars();
    let ch = chars.next()?;
    (chars.next().is_none() && ch.is_alphanumeric()).then_some(ch)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui_kit::{Modifiers, Render, TestAppContext, VisualTestContext};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    type Changes = Rc<RefCell<Vec<SharedString>>>;

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        bind_keys(cx);
    }

    fn items(labels: &[&str]) -> Vec<TlSelectItem> {
        labels
            .iter()
            .map(|l| TlSelectItem::new(l.to_lowercase(), l.to_string()))
            .collect()
    }

    fn numbered(n: usize) -> Vec<TlSelectItem> {
        (1..=n)
            .map(|i| TlSelectItem::new(format!("v{i}"), format!("Item {i}")))
            .collect()
    }

    struct Host {
        state: Entity<TlSelectState>,
        combo: bool,
        changes: Changes,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            window.set_rem_size(px(13.));
            let changes = self.changes.clone();
            let on_change = move |v: &SharedString, _: &mut Window, _: &mut App| {
                changes.borrow_mut().push(v.clone());
            };
            let control = if self.combo {
                TlCombo::new("select", &self.state)
                    .on_change(on_change)
                    .into_any_element()
            } else {
                TlSelect::new("select", &self.state)
                    .on_change(on_change)
                    .into_any_element()
            };
            div().size(px(800.)).p(px(16.)).child(control)
        }
    }

    fn host(
        cx: &mut TestAppContext,
        items: Vec<TlSelectItem>,
        selected: Option<&'static str>,
        combo: bool,
    ) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(setup);
        let (view, cx) = cx.add_window_view(move |_, cx| Host {
            state: cx.new(|cx| TlSelectState::new(items, selected, cx)),
            combo,
            changes: Changes::default(),
        });
        draw(cx);
        (view, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn state(view: &Entity<Host>, cx: &mut VisualTestContext) -> Entity<TlSelectState> {
        cx.update(|_, cx| view.read(cx).state.clone())
    }

    fn focus_trigger(view: &Entity<Host>, cx: &mut VisualTestContext) {
        let state = state(view, cx);
        cx.update(|window, cx| state.focus_handle(cx).focus(window, cx));
        draw(cx);
    }

    fn keys(cx: &mut VisualTestContext, keys: &str) {
        for key in keys.split(' ') {
            cx.simulate_keystrokes(key);
            draw(cx);
        }
    }

    fn trigger_focused(view: &Entity<Host>, cx: &mut VisualTestContext) -> bool {
        let state = state(view, cx);
        cx.update(|window, cx| state.focus_handle(cx).is_focused(window))
    }

    fn changes(view: &Entity<Host>, cx: &mut VisualTestContext) -> Vec<SharedString> {
        cx.update(|_, cx| view.read(cx).changes.borrow().clone())
    }

    fn read<R>(
        view: &Entity<Host>,
        cx: &mut VisualTestContext,
        f: impl FnOnce(&TlSelectState) -> R,
    ) -> R {
        let state = state(view, cx);
        cx.update(|_, cx| f(state.read(cx)))
    }

    #[gpui_kit::test]
    fn keyboard_open_move_confirm(cx: &mut TestAppContext) {
        let five = items(&["One", "Two", "Three", "Four", "Five"]);
        let (view, cx) = host(cx, five, Some("one"), false);
        focus_trigger(&view, cx);

        keys(cx, "alt-down");
        assert!(read(&view, cx, |s| s.is_open()), "Alt+Down opens");
        assert_eq!(read(&view, cx, |s| s.highlighted_index()), Some(0));

        keys(cx, "down enter");
        assert_eq!(changes(&view, cx), vec![SharedString::from("two")]);
        assert!(!read(&view, cx, |s| s.is_open()));
        assert_eq!(read(&view, cx, |s| s.selected_index()), Some(1));
        assert!(trigger_focused(&view, cx), "focus is back on the trigger");
    }

    #[gpui_kit::test]
    fn other_keys_open_and_arrows_wrap(cx: &mut TestAppContext) {
        let three = items(&["One", "Two", "Three"]);
        let (view, cx) = host(cx, three, Some("one"), false);
        focus_trigger(&view, cx);

        for key in ["space", "enter"] {
            keys(cx, key);
            assert!(read(&view, cx, |s| s.is_open()), "{key} opens");
            keys(cx, "escape");
        }

        keys(cx, "alt-down up");
        assert_eq!(
            read(&view, cx, |s| s.highlighted_index()),
            Some(2),
            "Up wraps"
        );
        keys(cx, "down");
        assert_eq!(
            read(&view, cx, |s| s.highlighted_index()),
            Some(0),
            "Down wraps"
        );
        assert!(changes(&view, cx).is_empty(), "moving fires nothing");
    }

    #[gpui_kit::test]
    fn escape_restores_focus(cx: &mut TestAppContext) {
        let five = items(&["One", "Two", "Three", "Four", "Five"]);
        let (view, cx) = host(cx, five, Some("one"), false);
        focus_trigger(&view, cx);

        keys(cx, "alt-down down escape");
        assert!(!read(&view, cx, |s| s.is_open()), "Esc closes");
        assert!(changes(&view, cx).is_empty(), "no change fires");
        assert_eq!(read(&view, cx, |s| s.selected_index()), Some(0));
        assert!(trigger_focused(&view, cx));
    }

    #[gpui_kit::test]
    fn type_ahead(cx: &mut TestAppContext) {
        let units = items(&["Bits", "Bytes", "Binary"]);
        let (view, cx) = host(cx, units, Some("bits"), false);
        focus_trigger(&view, cx);

        keys(cx, "alt-down");
        assert_eq!(read(&view, cx, |s| s.highlighted_index()), Some(0));
        keys(cx, "b");
        assert_eq!(
            read(&view, cx, |s| s.highlighted_index()),
            Some(1),
            "b moves to Bytes"
        );
        keys(cx, "b");
        assert_eq!(read(&view, cx, |s| s.highlighted_index()), Some(2));
        keys(cx, "b");
        assert_eq!(read(&view, cx, |s| s.highlighted_index()), Some(0), "wraps");
        assert!(changes(&view, cx).is_empty());
    }

    #[gpui_kit::test]
    fn combo_filter_no_matches(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, numbered(COMBO_FILTER_MIN_ITEMS), Some("v1"), true);
        focus_trigger(&view, cx);

        keys(cx, "enter");
        assert!(read(&view, cx, |s| s.is_open()));
        assert!(
            cx.debug_bounds("TlSelect-filter").is_some(),
            "12 items get a filter field"
        );
        assert!(cx.debug_bounds("TlSelect-no-matches").is_none());

        cx.simulate_input("zz");
        draw(cx);
        assert!(
            cx.debug_bounds("TlSelect-no-matches").is_some(),
            "no item matches zz"
        );
        keys(cx, "enter");
        assert!(
            changes(&view, cx).is_empty(),
            "Enter with no match does nothing"
        );

        keys(cx, "escape");
        assert!(!read(&view, cx, |s| s.is_open()));
        assert!(trigger_focused(&view, cx));
    }

    #[gpui_kit::test]
    fn combo_filter_narrows_and_confirms(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, numbered(COMBO_FILTER_MIN_ITEMS), Some("v1"), true);
        focus_trigger(&view, cx);

        keys(cx, "space");
        cx.simulate_input("1");
        draw(cx);
        // "Item 1", "Item 10", "Item 11", "Item 12": the highlight stays on Item 1.
        keys(cx, "down enter");
        assert_eq!(changes(&view, cx), vec![SharedString::from("v10")]);
        assert!(trigger_focused(&view, cx));
    }

    #[gpui_kit::test]
    fn eleven_items_no_filter(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, numbered(COMBO_FILTER_MIN_ITEMS - 1), Some("v1"), true);
        focus_trigger(&view, cx);
        keys(cx, "enter");
        assert!(read(&view, cx, |s| s.is_open()));
        assert!(cx.debug_bounds("TlSelect-popup").is_some());
        assert!(
            cx.debug_bounds("TlSelect-filter").is_none(),
            "11 items get no filter field"
        );
    }

    #[gpui_kit::test]
    fn popup_max_height_scrolls(cx: &mut TestAppContext) {
        let (view, cx) = host(cx, numbered(20), Some("v1"), false);
        focus_trigger(&view, cx);
        keys(cx, "enter");

        let list = cx.debug_bounds("TlSelect-list").expect("the list is drawn");
        assert_eq!(list.size.height, px(LIST_MAX_HEIGHT), "10 items high");
        let trigger = cx.debug_bounds("TlSelect-select").expect("trigger drawn");
        let popup = cx.debug_bounds("TlSelect-popup").expect("popup drawn");
        assert!(popup.size.width >= trigger.size.width, "at least as wide");
        assert!(popup.top() > trigger.bottom(), "below the trigger");

        let scroll = read(&view, cx, |s| s.scroll.clone());
        assert!(scroll.max_offset().y > px(0.), "the list scrolls");

        // The highlight on the last item scrolls it into view.
        keys(cx, "up");
        assert_eq!(read(&view, cx, |s| s.highlighted_index()), Some(19));
        assert!(scroll.offset().y < px(0.));
    }

    #[gpui_kit::test]
    fn click_opens_and_item_click_confirms(cx: &mut TestAppContext) {
        let three = items(&["One", "Two", "Three"]);
        let (view, cx) = host(cx, three, Some("one"), false);
        let trigger = cx.debug_bounds("TlSelect-select").expect("trigger drawn");
        cx.simulate_click(trigger.center(), Modifiers::none());
        draw(cx);
        assert!(read(&view, cx, |s| s.is_open()));

        let list = cx.debug_bounds("TlSelect-list").expect("list drawn");
        let third = point(list.center().x, list.top() + px(ITEM_HEIGHT * 2.5));
        cx.simulate_click(third, Modifiers::none());
        draw(cx);
        assert_eq!(changes(&view, cx), vec![SharedString::from("three")]);
        assert!(!read(&view, cx, |s| s.is_open()));
    }

    #[gpui_kit::test]
    fn chevron_turns_and_border_when_open(cx: &mut TestAppContext) {
        let three = items(&["One", "Two", "Three"]);
        let (view, cx) = host(cx, three, None, false);
        let accent_borders = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                let accent = current(cx).accent;
                window
                    .painted_quads()
                    .iter()
                    .filter(|q| q.border_color == accent && q.border_widths.top.0 > 0.)
                    .count()
            })
        };
        cx.simulate_mouse_move(point(px(700.), px(700.)), None, Modifiers::none());
        assert_eq!(accent_borders(cx), 0, "rest border is divider");
        cx.update(|window, cx| {
            let state = view.read(cx).state.clone();
            state.update(cx, |st, cx| {
                st.open(false, cx);
                st.popup_focus.focus(window, cx);
            });
        });
        assert!(accent_borders(cx) > 0, "open border is accent");
    }

    #[gpui_kit::test]
    fn set_selected_value_fires_nothing(cx: &mut TestAppContext) {
        let three = items(&["One", "Two", "Three"]);
        let (view, cx) = host(cx, three, Some("one"), false);
        let state = state(&view, cx);
        cx.update(|_, cx| state.update(cx, |st, cx| st.set_selected_value("three", cx)));
        draw(cx);
        assert_eq!(
            read(&view, cx, |s| s.selected_value().cloned()),
            Some("three".into())
        );
        assert!(changes(&view, cx).is_empty());

        cx.update(|_, cx| state.update(cx, |st, cx| st.set_selected_value("none", cx)));
        assert_eq!(read(&view, cx, |s| s.selected_index()), None);
    }
}
