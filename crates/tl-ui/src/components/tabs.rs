//! `TlTabs`: the in-window tab strip for Properties, Service Properties, Additional Setup and the
//! per-device groups in Settings (UI-FND-018). The main window tab strip is the shell's, not this.
//!
//! Built on the kit's unstyled base `Tabs` and `Tab`, which own the tab list and tab roles, the
//! selected state and pointer activation. The styled kit `TabBar` is not used because it cannot
//! meet the spec:
//! - Its underline indicator moves on a spring, not over `m_fast` (120 ms).
//! - Underline tabs have a 12 px gap between them, and their own styles override the cell
//!   colours: no `window_bg` on the active cell, no `tab_hover` or `header` fill, and a disabled
//!   tab swaps colours instead of opacity 0.5.
//! - Its overflow button is a kit ghost button with a caret that shows whenever the menu is on,
//!   not a `TlButton::Icon` shown only when the cells do not fit.
//! - A tab has one label run, so the count cannot be a separate `tnum` run.
//!
//! The overflow menu is the kit `PopupMenu` the kit `TabBar::menu(true)` opens, through the kit
//! `DropdownMenu` trigger. The kit has no keyboard handling for tabs; this file adds it.
//!
//! The owner holds `selected` and the tab specs, and stores the index `on_change` gives it.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::ElementExt as _;
use gpui_kit::base::{Tab as BaseTab, Tabs as BaseTabs};
use gpui_kit::component::Selectable;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::{
    Anchor, Animation, AnimationExt as _, App, Bounds, Div, ElementId, Entity, FocusHandle,
    FontWeight, InteractiveElement, Interactivity, IntoElement, KeyBinding, KeyDownEvent,
    ParentElement, Pixels, RenderOnce, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _,
    px,
};

use super::button::{TlButton, TlButtonKind, TlButtonSize};
use super::focus::TlFocusRing;
use super::icons::TlIcon;
use super::motion::{M_FAST, ease_out_cubic};
use super::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::text::style::{label_style, numeric_style};
use crate::theme::{current, metrics};

gpui_kit::actions!(tasklens, [NextInWindowTab, PrevInWindowTab]);

/// Strip height at 100% scale, divider included.
const STRIP: f32 = 32.;
/// Smallest cell width at 100% scale.
const MIN_CELL: f32 = 80.;
/// Widest label at 100% scale. Longer labels end in "…"; the count is kept.
const MAX_LABEL: f32 = 160.;
/// Underline thickness.
const UNDERLINE: Pixels = px(2.);
/// The focus ring sits this far inside the focused cell.
const RING_INSET: Pixels = px(2.);

const WINDOW_CONTEXT: &str = "TlTabsWindow";

/// The key context an owner sets on the root of a window that holds a strip, so Ctrl+Tab and
/// Ctrl+Shift+Tab reach it from anywhere in the window. The owner handles [`NextInWindowTab`]
/// and [`PrevInWindowTab`] with [`TlTabs::next_index`] and [`TlTabs::prev_index`].
pub fn tabs_window_context() -> &'static str {
    WINDOW_CONTEXT
}

/// Ctrl+Tab and Ctrl+Shift+Tab inside a window with [`tabs_window_context`].
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-tab", NextInWindowTab, Some(WINDOW_CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PrevInWindowTab, Some(WINDOW_CONTEXT)),
    ]);
}

/// One tab. The owner builds the strings once and keeps them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TlTabSpec {
    pub label: SharedString,
    /// The count suffix, such as "[17]", formatted by the owner when the count changes.
    pub count: Option<SharedString>,
    /// `Some(reason)` disables the tab; the reason is its tooltip.
    pub disabled: Option<SharedString>,
}

impl TlTabSpec {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            ..Default::default()
        }
    }

    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    pub fn disabled(mut self, reason: impl Into<SharedString>) -> Self {
        self.disabled = Some(reason.into());
        self
    }

    fn enabled(&self) -> bool {
        self.disabled.is_none()
    }
}

type ChangeHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The in-window tab strip. Render it as a child with an id that is stable for the window.
#[derive(IntoElement)]
pub struct TlTabs {
    id: ElementId,
    tabs: Vec<TlTabSpec>,
    selected: usize,
    on_change: Option<ChangeHandler>,
    overflow_tooltip: Option<SharedString>,
}

impl TlTabs {
    /// `stable_id` must be the same on every frame of the window: the strip keys its focus,
    /// scroll and animation state on it.
    pub fn new(stable_id: impl Into<ElementId>) -> Self {
        Self {
            id: stable_id.into(),
            tabs: Vec::new(),
            selected: 0,
            on_change: None,
            overflow_tooltip: None,
        }
    }

    pub fn tabs(mut self, tabs: Vec<TlTabSpec>) -> Self {
        self.tabs = tabs;
        self
    }

    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    /// Called with the new index on a click, an arrow key, Home, End or an overflow menu pick.
    pub fn on_change(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }

    /// Tooltip and accessible name of the "»" overflow button, from the owner's strings.
    pub fn overflow_tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.overflow_tooltip = Some(text.into());
        self
    }

    /// The next enabled tab after `selected`, wrapping. `selected` when no other tab is enabled.
    pub fn next_index(selected: usize, tabs: &[TlTabSpec]) -> usize {
        step(selected, tabs, true)
    }

    /// The previous enabled tab before `selected`, wrapping.
    pub fn prev_index(selected: usize, tabs: &[TlTabSpec]) -> usize {
        step(selected, tabs, false)
    }

    /// The first enabled tab, for Home.
    pub fn first_index(selected: usize, tabs: &[TlTabSpec]) -> usize {
        tabs.iter().position(TlTabSpec::enabled).unwrap_or(selected)
    }

    /// The last enabled tab, for End.
    pub fn last_index(selected: usize, tabs: &[TlTabSpec]) -> usize {
        tabs.iter()
            .rposition(TlTabSpec::enabled)
            .unwrap_or(selected)
    }
}

fn step(selected: usize, tabs: &[TlTabSpec], forward: bool) -> usize {
    let n = tabs.len();
    if n == 0 {
        return selected;
    }
    let from = selected.min(n - 1);
    (1..n)
        .map(|k| {
            if forward {
                (from + k) % n
            } else {
                (from + n - k) % n
            }
        })
        .find(|&ix| tabs[ix].enabled())
        .unwrap_or(selected)
}

/// The index a key press inside the strip selects, or `None` for keys the strip ignores.
fn key_target(key: &str, selected: usize, tabs: &[TlTabSpec]) -> Option<usize> {
    match key {
        "left" => Some(TlTabs::prev_index(selected, tabs)),
        "right" => Some(TlTabs::next_index(selected, tabs)),
        "home" => Some(TlTabs::first_index(selected, tabs)),
        "end" => Some(TlTabs::last_index(selected, tabs)),
        _ => None,
    }
}

/// The overflow menu entries: every tab, the active one checked, disabled tabs disabled. Each
/// entry shows the label and the count as separate runs, like the cell.
fn overflow_items(tabs: &[TlTabSpec], selected: usize, cx: &App) -> Vec<PopupMenuItem> {
    let m = *metrics(cx);
    tabs.iter()
        .enumerate()
        .map(|(ix, tab)| {
            let label = tab.label.clone();
            let count = tab.count.clone();
            PopupMenuItem::element(move |_, _| {
                cell_text(&m, label.clone(), count.clone(), FontWeight::NORMAL)
                    .debug_selector(move || format!("TlTabs-menu-{ix}"))
            })
            .checked(ix == selected)
            .disabled(!tab.enabled())
        })
        .collect()
}

/// The label run (end ellipsis at 160 px) and the `tnum` count run.
fn cell_text(
    m: &crate::theme::UiMetrics,
    label: SharedString,
    count: Option<SharedString>,
    weight: FontWeight,
) -> Div {
    let mut label_el = div()
        .min_w_0()
        .max_w(px(MAX_LABEL * m.ui_scale))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis();
    *label_el.text_style() = label_style(m, m.text.sm, weight);
    let count_el = count.map(|count| {
        let mut el = div().flex_none().whitespace_nowrap();
        let mut style = numeric_style(m, m.text.sm);
        style.font_weight = Some(weight);
        *el.text_style() = style;
        el.child(count)
    });
    div()
        .flex()
        .flex_row()
        .items_center()
        .min_w_0()
        .gap(m.sp[2])
        .child(label_el.child(label))
        .children(count_el)
}

/// State kept across frames for one strip.
struct TabsState {
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// Cell bounds from the last prepaint, by index.
    cells: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    /// The cells were wider than the strip at the last prepaint.
    overflow: bool,
    /// The selection drawn last frame.
    selected: usize,
    /// Where the underline starts sliding from, relative to the selected cell: the left and right
    /// edge offsets of the cell it left. `None` draws it at the cell.
    slide_from: Option<(Pixels, Pixels)>,
    /// Bumps on every selection change; keys the slide animation.
    epoch: usize,
    /// Disabled-tab tooltips, by index.
    tips: Vec<Entity<HoverTooltip>>,
}

impl TabsState {
    /// Records a selection change: the underline slides from the old cell unless motion is
    /// reduced or the old cell was never laid out, and an off-screen cell scrolls into view.
    fn select(&mut self, selected: usize, reduced: bool) {
        if selected == self.selected {
            return;
        }
        let cells = self.cells.borrow();
        self.slide_from = match (cells.get(self.selected), cells.get(selected)) {
            (Some(from), Some(to)) if !reduced && to.size.width > px(0.) => {
                Some((from.left() - to.left(), to.right() - from.right()))
            }
            _ => None,
        };
        drop(cells);
        self.epoch += 1;
        self.selected = selected;
        if self.overflow {
            self.scroll.scroll_to_item(selected);
        }
    }
}

impl RenderOnce for TlTabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let n = self.tabs.len();
        let selected = self.selected.min(n.saturating_sub(1));
        let reduced = cx.reduce_motion();

        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| TabsState {
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            cells: Rc::new(RefCell::new(Vec::new())),
            overflow: false,
            selected,
            slide_from: None,
            epoch: 0,
            tips: Vec::new(),
        });
        let (focus, scroll, cells, overflow, slide_from, epoch, tips) =
            state.update(cx, |s, cx| {
                s.select(selected, reduced);
                if reduced {
                    s.slide_from = None;
                }
                s.cells.borrow_mut().resize(n, Bounds::default());
                while s.tips.len() < n {
                    s.tips.push(cx.new(|_| HoverTooltip::default()));
                }
                (
                    s.focus.clone(),
                    s.scroll.clone(),
                    s.cells.clone(),
                    s.overflow,
                    s.slide_from,
                    s.epoch,
                    s.tips.clone(),
                )
            });
        let show_ring = TlFocusRing::show(focus.is_focused(window), window);
        let a11y = window.is_a11y_active();

        let mut cell_els = Vec::with_capacity(n);
        for (ix, tab) in self.tabs.iter().enumerate() {
            let active = ix == selected;
            let enabled = tab.enabled();
            let weight = if active {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            };
            let name = match (&tab.count, a11y) {
                (Some(count), true) => SharedString::from(format!("{} {count}", tab.label)),
                _ => tab.label.clone(),
            };
            let cells_writer = cells.clone();
            let mut cell = BaseTab::new(("tl-tab", ix))
                .selected(active)
                .disabled(!enabled)
                .set_position(ix + 1, n)
                .accessibility_label(name)
                .debug_selector(move || format!("TlTabs-cell-{ix}"))
                .relative()
                .flex_none()
                .h_full()
                .min_w(px(MIN_CELL * m.ui_scale))
                .px(m.sp[5])
                .text_color(if active {
                    theme.text_primary
                } else {
                    theme.text_secondary
                })
                .when(active, |c| c.bg(theme.window_bg))
                .when(!enabled, |c| c.opacity(0.5))
                .when(enabled && !active, |c| {
                    let (hover, text, pressed) =
                        (theme.tab_hover, theme.text_primary, theme.header);
                    c.hover(move |s| s.bg(hover).text_color(text))
                        .active(move |s| s.bg(pressed))
                })
                .child(cell_text(&m, tab.label.clone(), tab.count.clone(), weight))
                .on_prepaint(move |bounds, _, _| {
                    if let Some(slot) = cells_writer.borrow_mut().get_mut(ix) {
                        *slot = bounds;
                    }
                });

            if enabled && !active {
                let on_change = self.on_change.clone();
                let focus = focus.clone();
                cell = cell.on_click(move |_, window, cx| {
                    focus.focus(window, cx);
                    if let Some(f) = &on_change {
                        f(ix, window, cx);
                    }
                });
            }
            if active {
                cell = cell.child(underline(theme.accent, slide_from, epoch));
                if show_ring {
                    cell = cell.child(
                        div()
                            .absolute()
                            .top(RING_INSET)
                            .left(RING_INSET)
                            .right(RING_INSET)
                            .bottom(RING_INSET)
                            .border_2()
                            .border_color(theme.accent)
                            .rounded(m.radius_small),
                    );
                }
            }
            if let Some(reason) = &tab.disabled {
                cell = HoverTooltip::attach(
                    &tips[ix],
                    cell,
                    DELAY_CONTROL,
                    TlTooltip::text(reason.clone()).build(),
                    window,
                    cx,
                );
            }
            cell_els.push(cell);
        }

        let more = overflow.then(|| {
            let tabs = Rc::new(self.tabs.clone());
            let on_change = self.on_change.clone();
            let tooltip = self
                .overflow_tooltip
                .clone()
                .unwrap_or_else(|| SharedString::new_static("»"));
            let id = ElementId::NamedChild(Arc::new(self.id.clone()), "more".into());
            MoreTrigger {
                base: div().id(id.clone()),
                open: false,
                button: TlButton::new(
                    ElementId::NamedChild(Arc::new(id), "button".into()),
                    TlButtonKind::Icon,
                )
                .icon(TlIcon::ChevronRight)
                .size(TlButtonSize::Sm)
                .tooltip(tooltip)
                .tab_stop(false),
            }
            .debug_selector(|| "TlTabs-more".into())
            .flex_none()
            .px(m.sp[1])
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, cx| {
                let mut menu = menu.scrollable(true);
                for (ix, item) in overflow_items(&tabs, selected, cx).into_iter().enumerate() {
                    let on_change = on_change.clone();
                    menu = menu.item(item.on_click(move |_, window, cx| {
                        if let Some(f) = &on_change {
                            f(ix, window, cx);
                        }
                    }));
                }
                menu
            })
        });

        let tabs = self.tabs;
        let on_key = self.on_change;
        let overflow_state = state.clone();
        let overflow_cells = cells;
        BaseTabs::new(self.id)
            .track_focus(&focus)
            .key_context("TlTabs")
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.modified() {
                    return;
                }
                let Some(target) = key_target(&event.keystroke.key, selected, &tabs) else {
                    return;
                };
                cx.stop_propagation();
                if target != selected
                    && let Some(f) = &on_key
                {
                    f(target, window, cx);
                }
            })
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(STRIP * m.ui_scale))
            .overflow_hidden()
            .bg(theme.tab_bg)
            .border_b_1()
            .border_color(theme.divider)
            .child(
                div()
                    .id("tl-tabs-scroll")
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&scroll)
                    .children(cell_els),
            )
            .children(more)
            .on_prepaint(move |strip, _, cx| {
                let total: Pixels = overflow_cells.borrow().iter().map(|b| b.size.width).sum();
                let fits = total <= strip.size.width;
                overflow_state.update(cx, |s, cx| {
                    if s.overflow == fits {
                        s.overflow = !fits;
                        cx.notify();
                    }
                });
            })
    }
}

/// The 2 px accent line along the bottom of the selected cell. It slides in from the cell the
/// selection left over `m_fast`, or sits at the cell at once.
fn underline(
    color: gpui_kit::Hsla,
    slide_from: Option<(Pixels, Pixels)>,
    epoch: usize,
) -> gpui_kit::AnyElement {
    let line = div()
        .debug_selector(|| "TlTabs-underline".into())
        .absolute()
        .bottom_0()
        .h(UNDERLINE)
        .bg(color);
    match slide_from {
        None => line.left_0().right_0().into_any_element(),
        Some((left, right)) => line
            .with_animation(
                ("tl-tabs-underline", epoch),
                Animation::new(M_FAST).with_easing(ease_out_cubic),
                move |line, t| line.left(left * (1. - t)).right(right * (1. - t)),
            )
            .into_any_element(),
    }
}

/// The "»" overflow button as a kit dropdown trigger.
#[derive(IntoElement)]
struct MoreTrigger {
    base: Stateful<Div>,
    open: bool,
    button: TlButton,
}

impl Styled for MoreTrigger {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for MoreTrigger {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for MoreTrigger {}

impl Selectable for MoreTrigger {
    fn selected(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl DropdownMenu for MoreTrigger {}

impl RenderOnce for MoreTrigger {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.base
            .flex()
            .items_center()
            .child(self.button.pressed(self.open))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui_kit::{
        Context, KeyUpEvent, Keystroke, Modifiers, Render, TestAppContext, VisualTestContext,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    struct Host {
        tabs: Vec<TlTabSpec>,
        selected: usize,
        width: Pixels,
        content: FocusHandle,
        changes: Rc<RefCell<Vec<usize>>>,
    }

    impl Host {
        fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
            self.changes.borrow_mut().push(ix);
            self.selected = ix;
            cx.notify();
        }
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let view = cx.entity();
            div()
                .id("root")
                .key_context(tabs_window_context())
                .on_action(cx.listener(|h, _: &NextInWindowTab, _, cx| {
                    let ix = TlTabs::next_index(h.selected, &h.tabs);
                    h.select(ix, cx);
                }))
                .on_action(cx.listener(|h, _: &PrevInWindowTab, _, cx| {
                    let ix = TlTabs::prev_index(h.selected, &h.tabs);
                    h.select(ix, cx);
                }))
                .w(self.width)
                .h(px(300.))
                .flex()
                .flex_col()
                .child(
                    TlTabs::new("props-tabs")
                        .tabs(self.tabs.clone())
                        .selected(self.selected)
                        .overflow_tooltip("More tabs")
                        .on_change(move |ix, _, cx| view.update(cx, |h, cx| h.select(ix, cx))),
                )
                .child(div().id("content").track_focus(&self.content).flex_1())
        }
    }

    fn specs(n: usize) -> Vec<TlTabSpec> {
        (0..n)
            .map(|ix| TlTabSpec::new(format!("Tab {ix}")))
            .collect()
    }

    fn open(
        cx: &mut TestAppContext,
        tabs: Vec<TlTabSpec>,
        selected: usize,
        width: f32,
    ) -> (
        Entity<Host>,
        &mut VisualTestContext,
        Rc<RefCell<Vec<usize>>>,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            bind_keys(cx);
        });
        let changes = Rc::new(RefCell::new(Vec::new()));
        let (view, cx) = cx.add_window_view({
            let changes = changes.clone();
            move |_, cx| Host {
                tabs,
                selected,
                width: px(width),
                content: cx.focus_handle(),
                changes,
            }
        });
        draw(cx);
        (view, cx, changes)
    }

    /// Draws twice: the second frame sees the bounds and overflow the first one measured.
    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn press(cx: &mut VisualTestContext, key: &str) {
        let keystroke = Keystroke::parse(key).unwrap();
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
        cx.run_until_parked();
    }

    fn focus_strip(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.focus_next(cx));
        draw(cx);
    }

    #[test]
    fn next_and_prev_skip_disabled_and_wrap() {
        let tabs = vec![
            TlTabSpec::new("General"),
            TlTabSpec::new("Strings").disabled("Loading"),
            TlTabSpec::new("Threads").count("[17]"),
            TlTabSpec::new("Token"),
        ];
        assert_eq!(TlTabs::next_index(0, &tabs), 2);
        assert_eq!(TlTabs::next_index(3, &tabs), 0);
        assert_eq!(TlTabs::prev_index(0, &tabs), 3);
        assert_eq!(TlTabs::prev_index(2, &tabs), 0);
        let only = vec![TlTabSpec::new("A"), TlTabSpec::new("B").disabled("No")];
        assert_eq!(TlTabs::next_index(0, &only), 0, "no other enabled tab");
        let ends = vec![
            TlTabSpec::new("A").disabled("No"),
            TlTabSpec::new("B"),
            TlTabSpec::new("C"),
            TlTabSpec::new("D").disabled("No"),
        ];
        assert_eq!(TlTabs::first_index(2, &ends), 1);
        assert_eq!(TlTabs::last_index(1, &ends), 2);
    }

    #[gpui_kit::test]
    fn arrow_keys_wrap(cx: &mut TestAppContext) {
        let (view, cx, changes) = open(cx, specs(4), 3, 600.);
        focus_strip(cx);
        press(cx, "right");
        assert_eq!(*changes.borrow(), vec![0], "Right on the last tab wraps");
        press(cx, "left");
        assert_eq!(*changes.borrow(), vec![0, 3], "Left on the first tab wraps");
        press(cx, "home");
        press(cx, "end");
        assert_eq!(*changes.borrow(), vec![0, 3, 0, 3]);
        cx.update(|_, cx| assert_eq!(view.read(cx).selected, 3));
    }

    #[gpui_kit::test]
    fn ctrl_tab_from_content(cx: &mut TestAppContext) {
        let (view, cx, changes) = open(cx, specs(4), 0, 600.);
        cx.update(|window, cx| {
            let content = view.read(cx).content.clone();
            content.focus(window, cx);
        });
        draw(cx);
        press(cx, "ctrl-tab");
        assert_eq!(*changes.borrow(), vec![1]);
        press(cx, "ctrl-shift-tab");
        press(cx, "ctrl-shift-tab");
        assert_eq!(
            *changes.borrow(),
            vec![1, 0, 3],
            "Ctrl+Shift+Tab goes back and wraps"
        );
        cx.update(|window, cx| {
            assert!(
                view.read(cx).content.is_focused(window),
                "focus stays in the content"
            );
        });
    }

    #[gpui_kit::test]
    fn disabled_tab_skipped(cx: &mut TestAppContext) {
        let tabs = vec![
            TlTabSpec::new("General"),
            TlTabSpec::new("Strings").disabled("Strings load after the first sample"),
            TlTabSpec::new("Threads").count("[17]"),
            TlTabSpec::new("Token"),
        ];
        let (_, cx, changes) = open(cx, tabs, 0, 600.);
        focus_strip(cx);
        press(cx, "right");
        assert_eq!(*changes.borrow(), vec![2]);

        let disabled = cx
            .debug_bounds("TlTabs-cell-1")
            .expect("disabled cell drawn");
        cx.simulate_click(disabled.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            *changes.borrow(),
            vec![2],
            "a click on a disabled tab does nothing"
        );
    }

    #[gpui_kit::test]
    fn overflow_menu_lists_all(cx: &mut TestAppContext) {
        let (_, cx, changes) = open(cx, specs(10), 3, 400.);
        let more = cx.debug_bounds("TlTabs-more").expect("the » button shows");
        assert!(more.right() <= px(400.), "the button sits inside the strip");

        cx.simulate_click(more.center(), Modifiers::none());
        draw(cx);
        for ix in 0..10 {
            let selector: &'static str = format!("TlTabs-menu-{ix}").leak();
            assert!(cx.debug_bounds(selector).is_some(), "menu lists tab {ix}");
        }
        cx.update(|_, cx| {
            let checked: Vec<bool> = overflow_items(&specs(10), 3, cx)
                .iter()
                .map(|item| matches!(item, PopupMenuItem::ElementItem { checked: true, .. }))
                .collect();
            assert_eq!(checked.len(), 10);
            assert_eq!(checked.iter().filter(|c| **c).count(), 1);
            assert!(checked[3], "the active tab is checked");
        });

        let item = cx.debug_bounds("TlTabs-menu-7").unwrap();
        cx.simulate_click(item.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(*changes.borrow(), vec![7], "a menu pick selects the tab");
    }

    #[gpui_kit::test]
    fn no_overflow_button_when_cells_fit(cx: &mut TestAppContext) {
        let (_, cx, _) = open(cx, specs(3), 0, 400.);
        assert!(cx.debug_bounds("TlTabs-more").is_none());
        let cell = cx.debug_bounds("TlTabs-cell-0").unwrap();
        assert_eq!(
            cell.size.height,
            px(31.),
            "32 px strip less the 1 px divider"
        );
        assert!(cell.size.width >= px(80.), "min cell width 80");
    }

    #[gpui_kit::test]
    fn reduced_motion_underline_same_frame(cx: &mut TestAppContext) {
        let (view, cx, _) = open(cx, specs(4), 0, 600.);
        cx.update(|_, cx| cx.set_reduce_motion(true));
        view.update(cx, |h, cx| {
            h.selected = 2;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let cell = cx.debug_bounds("TlTabs-cell-2").unwrap();
        let line = cx.debug_bounds("TlTabs-underline").unwrap();
        assert_eq!(line.left(), cell.left());
        assert_eq!(line.right(), cell.right());
        assert_eq!(line.bottom(), cell.bottom());
        assert_eq!(line.size.height, px(2.));
    }

    #[gpui_kit::test]
    fn underline_slides_without_reduced_motion(cx: &mut TestAppContext) {
        let (view, cx, _) = open(cx, specs(4), 0, 600.);
        cx.update(|_, cx| cx.set_reduce_motion(false));
        view.update(cx, |h, cx| {
            h.selected = 2;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let cell = cx.debug_bounds("TlTabs-cell-2").unwrap();
        let line = cx.debug_bounds("TlTabs-underline").unwrap();
        assert!(line.left() < cell.left(), "the line starts at the old cell");
    }
}
