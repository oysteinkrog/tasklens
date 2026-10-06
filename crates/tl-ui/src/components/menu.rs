//! `TlMenu`: the builder every context menu, dropdown menu and menu bar menu uses (UI-FND-022,
//! R-112).
//!
//! A thin builder over the kit `PopupMenu`. Every menu gets the same width limits, scrolling,
//! check side and action context, so no view repeats the menu look or keyboard rules. The look
//! comes from the kit tokens mapped in `theme/map.rs` (`popover` = `menu_bg`, `accent` =
//! `menu_hover`, `border` = `divider`).
//!
//! The kit owns the arrow keys (wrap, skipping separators, headers and disabled items), Right
//! into a submenu, Left back to the parent, Enter, Esc and the shortcut hints. [`bind_keys`]
//! adds Space to run the highlighted item.
//!
//! Context menus opened from code (Shift+F10, the Menu key, a right-click the table handles
//! itself) go through [`open_context_menu_at`]. The window draws the open menu where it renders
//! [`context_menu_layer`]. The layer also covers the window, so a click outside the menu closes
//! it and reaches nothing below.

use gpui_kit::base::POPUP_PRIORITY;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::base::actions::Confirm;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{Icon, Side};
use gpui_kit::{
    Action, AnyElement, App, DismissEvent, ElementId, Entity, FocusHandle, Focusable as _, Global,
    Hsla, InteractiveElement, IntoElement, KeyBinding, MouseButton, ParentElement, Pixels, Point,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, TextOverflow, TextRun,
    Window, WindowId, anchored, deferred, div, font, point, px,
};

use super::icons::{TlIcon, icon};
use super::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::i18n::tr;
use crate::text::fonts::UI_FAMILY;
use crate::text::style::t_base;
use crate::theme::{current, metrics};

/// Item height at 100% scale.
pub const MENU_ITEM_H: f32 = 26.0;
/// Default menu width at 100% scale. Owners raise it with [`TlMenu::min_w`].
pub const MENU_MIN_W: f32 = 180.0;
/// Widest menu at 100% scale. Longer labels end in an ellipsis.
pub const MENU_MAX_W: f32 = 360.0;

/// The radio mark: a 6 px filled circle in a 12 px box, tinted like an icon.
const RADIO_DOT_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 12 12"><circle cx="6" cy="6" r="3" fill="currentColor"/></svg>"#;

/// Key context of every kit popup menu.
const KIT_CONTEXT: &str = "PopupMenu";

/// Binds Space to run the highlighted menu item. Called from `bind_keys_components`.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "space",
        Confirm { secondary: false },
        Some(KIT_CONTEXT),
    )]);
}

/// Builds one menu. Use [`TlMenu::build`]; the closure adds the items.
pub struct TlMenu {
    inner: PopupMenu,
    /// Some item has a check, radio or icon mark, so the kit reserves the mark column.
    has_mark_column: bool,
    /// Items, disabled items, submenus and painted items added. Separators and headers do not
    /// count.
    entries: usize,
    action_context: FocusHandle,
    ui_scale: f32,
}

impl TlMenu {
    /// Builds a menu. `action_context` is where the item actions are dispatched and where the
    /// shortcut hints are looked up: the row menu passes the table's focus handle, a menu bar
    /// menu the window's. Esc and a run item return focus there.
    pub fn build(
        window: &mut Window,
        cx: &mut App,
        action_context: FocusHandle,
        f: impl FnOnce(TlMenu, &mut Window, &mut App) -> TlMenu,
    ) -> Entity<PopupMenu> {
        let m = *metrics(cx);
        let max_h = window.viewport_size().height - m.sp[6];
        PopupMenu::build(window, cx, move |menu, window, cx| {
            let menu = menu
                .min_w(px(MENU_MIN_W * m.ui_scale))
                .max_w(px(MENU_MAX_W * m.ui_scale))
                .max_h(max_h)
                .scrollable(true)
                .check_side(Side::Left)
                .action_context(action_context.clone());
            let tl = TlMenu {
                inner: menu,
                has_mark_column: false,
                entries: 0,
                action_context,
                ui_scale: m.ui_scale,
            };
            let mut tl = f(tl, window, cx);
            if tl.entries == 0 {
                let label = tr(cx, "fnd-menu-no-items");
                tl = tl.item_disabled(label, None);
            }
            tl.inner
        })
    }

    /// Minimum width before the UI scale: 180 by default, 240 for the process row and header
    /// menus, 220 for the system table menus (R-112).
    pub fn min_w(mut self, w: Pixels) -> Self {
        self.inner = self.inner.min_w(w * self.ui_scale);
        self
    }

    /// An item that dispatches `action`. Its shortcut hint comes from the action context.
    pub fn item(mut self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.entries += 1;
        self.inner = self.inner.menu(label, action);
        self
    }

    /// An item that cannot run: half opacity, skipped by the arrow keys. `tooltip` says why.
    pub fn item_disabled(
        mut self,
        label: impl Into<SharedString>,
        tooltip: Option<SharedString>,
    ) -> Self {
        let label = label.into();
        self.entries += 1;
        self.inner = self.inner.item(
            PopupMenuItem::element(move |window, cx| {
                disabled_row(label.clone(), tooltip.clone(), window, cx)
            })
            .disabled(true),
        );
        self
    }

    /// An item with a check mark in the mark column when `checked`.
    pub fn check_item(
        self,
        label: impl Into<SharedString>,
        checked: bool,
        action: Box<dyn Action>,
    ) -> Self {
        let mark = checked.then(|| Icon::new(TlIcon::Check));
        self.marked(label.into(), checked, mark, action)
    }

    /// An item with a radio dot in the mark column when `selected`.
    pub fn radio_item(
        self,
        label: impl Into<SharedString>,
        selected: bool,
        action: Box<dyn Action>,
    ) -> Self {
        let mark = selected.then(|| Icon::default().data(RADIO_DOT_SVG));
        self.marked(label.into(), selected, mark, action)
    }

    /// The kit draws the item's icon in the mark column. An unmarked item gets an empty icon so
    /// the column stays reserved in a menu where nothing is checked yet.
    fn marked(
        mut self,
        label: SharedString,
        checked: bool,
        mark: Option<Icon>,
        action: Box<dyn Action>,
    ) -> Self {
        self.entries += 1;
        self.has_mark_column = true;
        let mark = mark.unwrap_or_else(Icon::empty);
        let item = PopupMenuItem::new(label)
            .checked(checked)
            .icon(mark)
            .action(action);
        self.inner = self.inner.item(item);
        self
    }

    /// A submenu. `f` builds its items when the parent menu is built. It shares the parent's
    /// action context.
    pub fn submenu(
        mut self,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut App,
        f: impl Fn(TlMenu, &mut Window, &mut App) -> TlMenu + 'static,
    ) -> Self {
        self.entries += 1;
        let menu = TlMenu::build(window, cx, self.action_context.clone(), f);
        self.inner = self.inner.item(PopupMenuItem::submenu(label, menu));
        self
    }

    /// A divider line. Leading, doubled and trailing separators are dropped.
    pub fn separator(mut self) -> Self {
        self.inner = self.inner.separator();
        self
    }

    /// A header row that cannot be selected.
    pub fn header(mut self, label: impl Into<SharedString>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// An item painted in another theme's colours (APP-036), with `suffix_icon` (`copy`) at the
    /// right. A label too long for the menu shows in full in a tooltip.
    pub fn painted_item(
        mut self,
        label: impl Into<SharedString>,
        bg: Hsla,
        fg: Hsla,
        suffix_icon: Option<TlIcon>,
        action: Box<dyn Action>,
    ) -> Self {
        let label = label.into();
        self.entries += 1;
        self.inner = self.inner.menu_element(action, move |window, cx| {
            painted_row(label.clone(), bg, fg, suffix_icon, window, cx)
        });
        self
    }
}

/// The width a label may take before it ends in an ellipsis: the widest menu less the menu
/// padding (4 a side), the item padding (8 a side) and the painted row's own padding and icon.
fn painted_label_max(m: &crate::theme::UiMetrics) -> Pixels {
    px(MENU_MAX_W * m.ui_scale) - m.sp[2] * 2. - m.sp[4] * 4. - m.icon.sm - m.sp[2]
}

fn label_width(label: &SharedString, size: Pixels, window: &Window) -> Pixels {
    let run = TextRun {
        len: label.len(),
        font: font(UI_FAMILY),
        color: gpui_kit::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(label.clone(), size, &[run], None)
        .width
}

fn disabled_row(
    label: SharedString,
    tooltip: Option<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = current(cx).clone();
    let m = *metrics(cx);
    let row = div()
        .id("tl-menu-disabled")
        .test_support()
        .aria_label(label.clone())
        .flex_1()
        .min_w_0()
        .opacity(0.5)
        .text_color(t.text_primary)
        .whitespace_nowrap()
        .overflow_hidden()
        .text_overflow(TextOverflow::Truncate("\u{2026}".into()))
        .child(label.clone());
    let mut row = row;
    *row.text_style() = t_base(&m);
    if let Some(reason) = tooltip {
        let state = HoverTooltip::state(
            ElementId::Name(format!("tl-menu-disabled-tip-{label}").into()),
            window,
            cx,
        );
        row = HoverTooltip::attach(
            &state,
            row,
            DELAY_CONTROL,
            TlTooltip::text(reason).build(),
            window,
            cx,
        );
    }
    row.into_any_element()
}

fn painted_row(
    label: SharedString,
    bg: Hsla,
    fg: Hsla,
    suffix_icon: Option<TlIcon>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let m = *metrics(cx);
    let truncated = label_width(&label, m.text.base, window) > painted_label_max(&m);
    let mut text = div()
        .flex_1()
        .min_w_0()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_overflow(TextOverflow::Truncate("\u{2026}".into()))
        .child(label.clone());
    *text.text_style() = t_base(&m);
    let mut row = div()
        .id("tl-menu-painted")
        .test_support()
        .aria_label(label.clone())
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(m.sp[2])
        .h(px(MENU_ITEM_H * m.ui_scale))
        .px(m.sp[4])
        .rounded(m.radius_small)
        .bg(bg)
        .text_color(fg)
        .child(text)
        .children(suffix_icon.map(|name| icon(name, m.icon.sm, fg)));
    if truncated {
        let state = HoverTooltip::state(
            ElementId::Name(format!("tl-menu-painted-tip-{label}").into()),
            window,
            cx,
        );
        row = HoverTooltip::attach(
            &state,
            row,
            DELAY_CONTROL,
            TlTooltip::text(label).build(),
            window,
            cx,
        );
    }
    row.into_any_element()
}

/// The context menu open in one window.
struct OpenMenu {
    window: WindowId,
    menu: Entity<PopupMenu>,
    position: Point<Pixels>,
    _dismiss: Subscription,
}

/// At most one open context menu per window.
#[derive(Default)]
struct OpenMenus(Vec<OpenMenu>);

impl Global for OpenMenus {}

/// Opens `menu` with its top left corner at `position`, kept inside the window, and focuses it.
/// Right-click passes the pointer position. Shift+F10 and the Menu key pass the focused row's
/// left edge plus `sp4` and the row bottom. A menu already open in this window closes.
pub fn open_context_menu_at(
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
    menu: Entity<PopupMenu>,
) {
    let window_id = window.window_handle().window_id();
    let menu_id = menu.entity_id();
    let dismiss = window.subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
        if let Some(open) = cx.try_global::<OpenMenus>()
            && open.0.iter().any(|o| o.menu.entity_id() == menu_id)
        {
            cx.global_mut::<OpenMenus>()
                .0
                .retain(|o| o.menu.entity_id() != menu_id);
            window.refresh();
        }
    });
    let open = cx.default_global::<OpenMenus>();
    open.0.retain(|o| o.window != window_id);
    open.0.push(OpenMenu {
        window: window_id,
        menu: menu.clone(),
        position,
        _dismiss: dismiss,
    });
    menu.focus_handle(cx).focus(window, cx);
    window.refresh();
}

/// Closes the context menu open in this window, if any, without running an item.
pub fn close_context_menu(window: &mut Window, cx: &mut App) {
    let window_id = window.window_handle().window_id();
    if let Some(open) = cx.try_global::<OpenMenus>()
        && open.0.iter().any(|o| o.window == window_id)
    {
        cx.global_mut::<OpenMenus>()
            .0
            .retain(|o| o.window != window_id);
        window.refresh();
    }
}

/// Whether a context menu from [`open_context_menu_at`] is open in this window.
pub fn context_menu_open(window: &Window, cx: &App) -> bool {
    let window_id = window.window_handle().window_id();
    cx.try_global::<OpenMenus>()
        .is_some_and(|open| open.0.iter().any(|o| o.window == window_id))
}

/// The open context menu of this window, drawn above everything. Render it once per window, as
/// a child anywhere in the window's tree; it draws nothing while no menu is open.
pub fn context_menu_layer(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let window_id = window.window_handle().window_id();
    let (menu, position) = cx
        .try_global::<OpenMenus>()?
        .0
        .iter()
        .find(|o| o.window == window_id)
        .map(|o| (o.menu.clone(), o.position))?;
    // Keep the keys on the menu while it is open, as the kit's own context menu does.
    let focus = menu.focus_handle(cx);
    if !focus.contains_focused(window, cx) {
        focus.focus(window, cx);
    }
    let viewport = window.viewport_size();
    let margin = metrics(cx).sp[4];
    let layer = anchored().position(point(px(0.), px(0.))).child(
        div()
            .id("tl-context-menu-layer")
            .w(viewport.width)
            .h(viewport.height)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                anchored()
                    .position(position)
                    .snap_to_window_with_margin(margin)
                    .child(menu),
            ),
    );
    Some(
        deferred(layer)
            .with_priority(POPUP_PRIORITY)
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gpui_kit::base::test_support::{ElementSnapshot, snapshots};
    use gpui_kit::{Bounds, Context, Modifiers, Render, Role, TestAppContext, VisualTestContext};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    gpui_kit::actions!(tl_menu_test, [RunA, RunB, RunC, RunX, RunY]);

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        crate::i18n::init_strings(cx, "en-US");
        bind_keys(cx);
    }

    /// A window with a table-like row at the top and the context menu layer.
    struct Host {
        focus: FocusHandle,
        row_pressed: Rc<Cell<bool>>,
        ran: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            window.set_rem_size(px(13.));
            let pressed = self.row_pressed.clone();
            let ran = self.ran.clone();
            let ran_c = self.ran.clone();
            div()
                .size(px(800.))
                .key_context("MenuHost")
                .track_focus(&self.focus)
                .on_action(move |_: &RunA, _, _| ran.borrow_mut().push("A"))
                .on_action(move |_: &RunC, _, _| ran_c.borrow_mut().push("C"))
                .child(
                    div()
                        .id("row")
                        .h(px(28.))
                        .w_full()
                        .on_mouse_down(MouseButton::Left, move |_, _, _| pressed.set(true)),
                )
                .children(context_menu_layer(window, cx))
        }
    }

    fn host(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(setup);
        let (view, cx) = cx.add_window_view(|_, cx| Host {
            focus: cx.focus_handle(),
            row_pressed: Rc::default(),
            ran: Rc::default(),
        });
        draw(cx);
        (view, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn keys(cx: &mut VisualTestContext, keys: &str) {
        for key in keys.split(' ') {
            cx.simulate_keystrokes(key);
            draw(cx);
        }
    }

    fn open(
        view: &Entity<Host>,
        cx: &mut VisualTestContext,
        at: Point<Pixels>,
        f: impl FnOnce(TlMenu, &mut Window, &mut App) -> TlMenu + 'static,
    ) -> Entity<PopupMenu> {
        let focus = cx.update(|_, cx| view.read(cx).focus.clone());
        let menu = cx.update(|window, cx| {
            focus.focus(window, cx);
            let menu = TlMenu::build(window, cx, focus, f);
            open_context_menu_at(at, window, cx, menu.clone());
            menu
        });
        draw(cx);
        menu
    }

    fn all(cx: &mut VisualTestContext) -> Vec<ElementSnapshot> {
        cx.update(|window, _| snapshots(window))
    }

    fn in_submenu(s: &ElementSnapshot) -> bool {
        s.path()
            .iter()
            .any(|id| *id == ElementId::Name("submenu".into()))
    }

    fn index(s: &ElementSnapshot) -> usize {
        match s.path().last() {
            Some(ElementId::Integer(ix)) => *ix as usize,
            other => panic!("menu item id {other:?}"),
        }
    }

    fn items(cx: &mut VisualTestContext) -> Vec<ElementSnapshot> {
        all(cx)
            .into_iter()
            .filter(|s| s.role() == Some(Role::MenuItem))
            .collect()
    }

    /// The highlighted items as (in a submenu, index).
    fn highlighted(cx: &mut VisualTestContext) -> Vec<(bool, usize)> {
        let mut out: Vec<_> = items(cx)
            .iter()
            .filter(|s| s.selected() == Some(true))
            .map(|s| (in_submenu(s), index(s)))
            .collect();
        out.sort();
        out
    }

    /// Bounds of the top-level menus, sorted by x.
    fn menus(cx: &mut VisualTestContext) -> Vec<Bounds<Pixels>> {
        let mut out: Vec<_> = all(cx)
            .into_iter()
            .filter(|s| s.role() == Some(Role::Menu) && !in_submenu(s))
            .map(|s| s.bounds())
            .collect();
        out.sort_by(|a, b| a.origin.x.partial_cmp(&b.origin.x).unwrap());
        out
    }

    fn a_sep_b_c(menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        menu.item("A", Box::new(RunA))
            .separator()
            .item_disabled("B", Some("Not now".into()))
            .item("C", Box::new(RunC))
    }

    #[gpui_kit::test]
    fn arrow_keys_skip_separators_and_disabled(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(100.), px(100.)), a_sep_b_c);

        keys(cx, "down");
        assert_eq!(highlighted(cx), vec![(false, 0)], "A");
        keys(cx, "down");
        assert_eq!(
            highlighted(cx),
            vec![(false, 3)],
            "skips the separator and B"
        );
        keys(cx, "down");
        assert_eq!(highlighted(cx), vec![(false, 0)], "wraps to A");
        keys(cx, "up");
        assert_eq!(highlighted(cx), vec![(false, 3)], "wraps back to C");

        keys(cx, "space");
        assert_eq!(*view.read_with(cx, |h, _| h.ran.clone()).borrow(), ["C"]);
        assert!(
            !cx.update(|window, cx| context_menu_open(window, cx)),
            "closed"
        );
        let focus = view.read_with(cx, |h, _| h.focus.clone());
        assert!(
            cx.update(|window, _| focus.is_focused(window)),
            "focus is back on the opener"
        );
    }

    #[gpui_kit::test]
    fn right_opens_submenu_left_closes(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        let menu = open(&view, cx, point(px(100.), px(100.)), |menu, window, cx| {
            menu.item("A", Box::new(RunA))
                .submenu("More", window, cx, |menu, _, _| {
                    menu.item("X", Box::new(RunX)).item("Y", Box::new(RunY))
                })
        });

        keys(cx, "down down");
        assert_eq!(highlighted(cx), vec![(false, 1)]);
        keys(cx, "right");
        assert_eq!(
            highlighted(cx),
            vec![(false, 1), (true, 0)],
            "the parent stays highlighted and X is highlighted"
        );
        let parent_focused = cx.update(|window, cx| menu.focus_handle(cx).is_focused(window));
        assert!(!parent_focused, "the submenu has the keys");

        keys(cx, "left");
        assert_eq!(highlighted(cx), vec![(false, 1)], "back on the parent item");
        let parent_focused = cx.update(|window, cx| menu.focus_handle(cx).is_focused(window));
        assert!(parent_focused, "the parent has the keys");
    }

    #[gpui_kit::test]
    fn shift_f10_opens_at_row(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        // The caller computes the point: the focused row's left edge plus sp4, the row bottom.
        let m = cx.update(|_, cx| *metrics(cx));
        let at = point(px(0.) + m.sp[4], px(28.));
        open(&view, cx, at, a_sep_b_c);
        let menus = menus(cx);
        assert_eq!(menus.len(), 1);
        assert_eq!(menus[0].origin, at);
    }

    #[gpui_kit::test]
    fn item_height_is_26_at_100pct(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(100.), px(100.)), |menu, window, cx| {
            menu.item("A", Box::new(RunA))
                .check_item("Checked", true, Box::new(RunB))
                .radio_item("Radio", true, Box::new(RunC))
                .item_disabled("Disabled", None)
                .painted_item(
                    "Theme",
                    gpui_kit::black(),
                    gpui_kit::white(),
                    Some(TlIcon::Copy),
                    Box::new(RunX),
                )
                .submenu("More", window, cx, |menu, _, _| {
                    menu.item("X", Box::new(RunX))
                })
        });
        let rows = items(cx);
        assert_eq!(rows.len(), 6);
        for row in rows {
            assert_eq!(row.bounds().size.height, px(MENU_ITEM_H), "{row:?}");
        }
    }

    #[gpui_kit::test]
    fn outside_click_consumed(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(300.), px(300.)), a_sep_b_c);
        assert!(cx.update(|window, cx| context_menu_open(window, cx)));

        cx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
        draw(cx);
        assert!(
            !cx.update(|window, cx| context_menu_open(window, cx)),
            "closed"
        );
        assert!(menus(cx).is_empty());
        assert!(
            !view.read_with(cx, |h, _| h.row_pressed.get()),
            "the row under the pointer got no click"
        );
        assert!(view.read_with(cx, |h, _| h.ran.borrow().is_empty()));

        cx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
        assert!(
            view.read_with(cx, |h, _| h.row_pressed.get()),
            "the next click reaches the row"
        );
    }

    #[gpui_kit::test]
    fn mark_column_collapses(cx: &mut TestAppContext) {
        const LONG: &str = "Show processes from every user and every session";
        let (view, cx) = host(cx);
        open(&view, cx, point(px(10.), px(40.)), |menu, _, _| {
            let menu = menu.item(LONG, Box::new(RunA));
            assert!(!menu.has_mark_column);
            menu
        });
        let plain = menus(cx)[0];
        cx.update(|window, cx| close_context_menu(window, cx));
        draw(cx);

        open(&view, cx, point(px(10.), px(40.)), |menu, _, _| {
            let menu = menu.check_item(LONG, false, Box::new(RunA));
            assert!(menu.has_mark_column, "reserved with nothing checked");
            menu
        });
        let marked = menus(cx)[0];
        assert!(
            plain.size.width > px(MENU_MIN_W),
            "the label sets the width"
        );
        assert!(
            marked.size.width > plain.size.width,
            "{marked:?} has the column, {plain:?} has none"
        );
    }

    #[gpui_kit::test]
    fn min_width_is_raised(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(10.), px(40.)), |menu, _, _| {
            menu.item("A", Box::new(RunA))
        });
        let width = menus(cx)[0].size.width;
        assert!(width >= px(MENU_MIN_W) && width < px(240.), "{width:?}");
        cx.update(|window, cx| close_context_menu(window, cx));
        draw(cx);

        open(&view, cx, point(px(10.), px(40.)), |menu, _, _| {
            menu.min_w(px(240.)).item("A", Box::new(RunA))
        });
        assert!(menus(cx)[0].size.width >= px(240.));
    }

    #[gpui_kit::test]
    fn empty_menu_shows_no_items(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(10.), px(40.)), |menu, _, _| {
            menu.header("Columns")
        });
        let labels: Vec<_> = all(cx)
            .iter()
            .filter_map(|s| s.label().map(str::to_owned))
            .collect();
        assert!(labels.iter().any(|l| l == "No items"), "{labels:?}");

        keys(cx, "down down");
        assert!(
            highlighted(cx).iter().all(|&(_, ix)| ix != 1),
            "the row cannot be highlighted"
        );
    }
}
