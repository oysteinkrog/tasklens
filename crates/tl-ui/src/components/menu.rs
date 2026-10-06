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
use gpui_kit::base::actions::Confirm;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{Icon, Side};
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, DismissEvent, ElementId, Entity, FocusHandle,
    Focusable as _, Global, Hsla, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    ParentElement, Pixels, Point, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, TextOverflow, TextRun, Window, WindowId, anchored, deferred, div, font, point,
    px,
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
        mut self,
        label: impl Into<SharedString>,
        checked: bool,
        action: Box<dyn Action>,
    ) -> Self {
        let mark = checked.then(|| Icon::new(TlIcon::Check));
        self.marked(label.into(), checked, mark, action)
    }

    /// An item with a radio dot in the mark column when `selected`.
    pub fn radio_item(
        mut self,
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
