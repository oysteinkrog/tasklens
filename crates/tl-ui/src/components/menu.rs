//! `TlMenu`: the builder every context menu, dropdown menu and menu bar menu uses (UI-FND-022,
//! R-112).
//!
//! Every menu gets the same width limits, scrolling, mark column and action context, so no view
//! repeats the menu look or keyboard rules. [`TlMenu::build`] returns an `Entity<TlMenuPopup>`,
//! the open menu, which owns the highlight and the open submenu.
//!
//! Built on unstyled GPUI elements and the kit's base actions, like `select.rs`, `tabs.rs` and
//! `slider.rs`. The styled kit `PopupMenu` is not used because it keeps the highlight and the
//! layout private, so it cannot meet the spec: no Home, End or type-ahead; Down and Right land
//! on header rows; Left out of a submenu leaves the submenu drawn; and its mark column,
//! separators, headers, row heights and submenu overlap differ from UI-FND-022.
//!
//! Keys (bound by [`bind_keys`] in the [`KEY_CONTEXT`] context): Up and Down move the highlight
//! (wrapping, skipping separators, headers and disabled items), Home and End jump to the first
//! and last item, a typed letter jumps to the next item starting with it, Right, Enter and Space
//! open a submenu and highlight its first item, Left and Esc close a submenu back to its parent,
//! Enter and Space run an item, and Esc on the top menu closes it. Hovering a row for 150 ms opens
//! its submenu or closes another one; a click on a submenu row opens it at once.
//!
//! Context menus opened from code (Shift+F10, the Menu key, a right-click the table handles
//! itself) go through [`open_context_menu_at`]. The window draws the open menu where it renders
//! [`context_menu_layer`]. The layer also covers the window, so a click outside the menu closes
//! it and reaches nothing below.

use std::time::Duration;

use gpui_kit::base::POPUP_PRIORITY;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::base::actions::{
    Cancel, Confirm, SelectDown, SelectFirst, SelectLast, SelectLeft, SelectRight, SelectUp,
};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::{
    Action, Anchor, Animation, AnimationExt as _, AnyElement, App, AppContext as _, AsKeystroke,
    Bounds, BoxShadow, Context, DismissEvent, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, Global, Hsla, InteractiveElement, IntoElement, KeyBinding, KeyContext, KeyDownEvent,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, Role, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Task, TextOverflow,
    TextRun, WeakEntity, Window, WindowId, anchored, canvas, deferred, div, font, point,
    prelude::FluentBuilder as _, px,
};

use super::icons::{TlIcon, icon};
use super::motion::{M_FAST, ease_out_cubic};
use super::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::i18n::tr;
use crate::text::fonts::UI_FAMILY;
use crate::text::style::{t_base, t_xs};
use crate::theme::{UiMetrics, current, metrics};

/// Item height at 100% scale.
pub const MENU_ITEM_H: f32 = 26.0;
/// Header row height at 100% scale.
pub const MENU_HEADER_H: f32 = 22.0;
/// Width of the check, radio and icon column at 100% scale.
pub const MENU_MARK_W: f32 = 20.0;
/// Default menu width at 100% scale. Owners raise it with [`TlMenu::min_w`].
pub const MENU_MIN_W: f32 = 180.0;
/// Widest menu at 100% scale. Longer labels end in an ellipsis.
pub const MENU_MAX_W: f32 = 360.0;
/// How far a submenu overlaps its parent menu, at 100% scale.
pub const SUBMENU_OVERLAP: f32 = 4.0;
/// Hovering a row this long opens its submenu, or closes the open submenu of another row.
pub const SUBMENU_HOVER_DELAY: Duration = Duration::from_millis(150);

/// The radio mark: a filled circle this wide at 100% scale.
const RADIO_DOT: f32 = 6.0;
/// The menu border width.
const BORDER: Pixels = px(1.);

/// Key context of every open menu, where [`bind_keys`] binds the menu keys.
pub const KEY_CONTEXT: &str = "TlMenu";

/// Binds the menu keys. Called from `bind_keys_components`.
pub fn bind_keys(cx: &mut App) {
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, ctx),
        KeyBinding::new("down", SelectDown, ctx),
        KeyBinding::new("left", SelectLeft, ctx),
        KeyBinding::new("right", SelectRight, ctx),
        KeyBinding::new("home", SelectFirst, ctx),
        KeyBinding::new("end", SelectLast, ctx),
        KeyBinding::new("enter", Confirm { secondary: false }, ctx),
        KeyBinding::new("space", Confirm { secondary: false }, ctx),
        KeyBinding::new("escape", Cancel, ctx),
    ]);
}

/// What an item shows in the mark column.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    None,
    Check(bool),
    Radio(bool),
}

enum Entry {
    Item {
        label: SharedString,
        action: Box<dyn Action>,
        mark: Mark,
        tooltip: Option<SharedString>,
    },
    Disabled {
        label: SharedString,
        tooltip: Option<SharedString>,
    },
    Painted {
        label: SharedString,
        bg: Hsla,
        fg: Hsla,
        suffix_icon: Option<TlIcon>,
        action: Box<dyn Action>,
    },
    Submenu {
        label: SharedString,
        menu: Entity<TlMenuPopup>,
        tooltip: Option<SharedString>,
    },
    Separator,
    Header(SharedString),
}

impl Entry {
    /// The arrow keys, Home, End, type-ahead and the mouse can highlight it.
    fn selectable(&self) -> bool {
        matches!(
            self,
            Entry::Item { .. } | Entry::Painted { .. } | Entry::Submenu { .. }
        )
    }

    /// Counts as an item for the "No items" row. Separators and headers do not.
    fn is_item(&self) -> bool {
        self.selectable() || matches!(self, Entry::Disabled { .. })
    }

    fn label(&self) -> Option<&SharedString> {
        match self {
            Entry::Item { label, .. }
            | Entry::Disabled { label, .. }
            | Entry::Painted { label, .. }
            | Entry::Submenu { label, .. }
            | Entry::Header(label) => Some(label),
            Entry::Separator => None,
        }
    }
}

/// Builds one menu. Use [`TlMenu::build`]; the closure adds the items.
pub struct TlMenu {
    entries: Vec<Entry>,
    /// Some item has a check or radio mark, so the menu reserves the mark column.
    has_mark_column: bool,
    min_w: Pixels,
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
    ) -> Entity<TlMenuPopup> {
        let m = *metrics(cx);
        let tl = TlMenu {
            entries: Vec::new(),
            has_mark_column: false,
            min_w: px(MENU_MIN_W * m.ui_scale),
            action_context: action_context.clone(),
            ui_scale: m.ui_scale,
        };
        let mut tl = f(tl, window, cx);
        if matches!(tl.entries.last(), Some(Entry::Separator)) {
            tl.entries.pop();
        }
        if !tl.entries.iter().any(Entry::is_item) {
            let label = tr(cx, "fnd-menu-no-items");
            tl = tl.item_disabled(label, None);
        }
        let max_h = window.viewport_size().height - m.sp[6];
        let menu = cx.new(|cx| TlMenuPopup {
            focus: cx.focus_handle(),
            entries: tl.entries,
            has_mark_column: tl.has_mark_column,
            min_w: tl.min_w,
            max_h,
            action_context,
            highlight: None,
            open_sub: None,
            parent: None,
            depth: 0,
            scroll: ScrollHandle::new(),
            bounds: Bounds::default(),
            hover_task: None,
        });
        let weak = menu.downgrade();
        for sub in menu.read(cx).submenus() {
            sub.update(cx, |sub, _| sub.parent = Some(weak.clone()));
            set_depth(&sub, 1, cx);
        }
        menu
    }

    /// Minimum width before the UI scale: 180 by default, 240 for the process row and header
    /// menus, 220 for the system table menus (R-112).
    pub fn min_w(mut self, w: Pixels) -> Self {
        self.min_w = w * self.ui_scale;
        self
    }

    /// An item that dispatches `action`. Its shortcut hint comes from the action context.
    pub fn item(mut self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.entries.push(Entry::Item {
            label: label.into(),
            action,
            mark: Mark::None,
            tooltip: None,
        });
        self
    }

    /// An item that cannot run: half opacity, skipped by the arrow keys. `tooltip` says why.
    pub fn item_disabled(
        mut self,
        label: impl Into<SharedString>,
        tooltip: Option<SharedString>,
    ) -> Self {
        self.entries.push(Entry::Disabled {
            label: label.into(),
            tooltip,
        });
        self
    }

    /// An item with a check mark in the mark column when `checked`.
    pub fn check_item(
        self,
        label: impl Into<SharedString>,
        checked: bool,
        action: Box<dyn Action>,
    ) -> Self {
        self.marked(label.into(), Mark::Check(checked), action)
    }

    /// An item with a radio dot in the mark column when `selected`.
    pub fn radio_item(
        self,
        label: impl Into<SharedString>,
        selected: bool,
        action: Box<dyn Action>,
    ) -> Self {
        self.marked(label.into(), Mark::Radio(selected), action)
    }

    /// The column stays reserved in a menu where nothing is checked yet.
    fn marked(mut self, label: SharedString, mark: Mark, action: Box<dyn Action>) -> Self {
        self.has_mark_column = true;
        self.entries.push(Entry::Item {
            label,
            action,
            mark,
            tooltip: None,
        });
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
        let menu = TlMenu::build(window, cx, self.action_context.clone(), f);
        self.entries.push(Entry::Submenu {
            label: label.into(),
            menu,
            tooltip: None,
        });
        self
    }

    /// A hover tooltip on the item or submenu added last, such as what an enabled item does.
    /// Disabled items take theirs in [`TlMenu::item_disabled`]; on other rows this does nothing.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        if let Some(Entry::Item { tooltip, .. } | Entry::Submenu { tooltip, .. }) =
            self.entries.last_mut()
        {
            *tooltip = Some(text.into());
        }
        self
    }

    /// A divider line. Leading, doubled and trailing separators are dropped.
    pub fn separator(mut self) -> Self {
        if !matches!(self.entries.last(), None | Some(Entry::Separator)) {
            self.entries.push(Entry::Separator);
        }
        self
    }

    /// A header row that cannot be selected.
    pub fn header(mut self, label: impl Into<SharedString>) -> Self {
        self.entries.push(Entry::Header(label.into()));
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
        self.entries.push(Entry::Painted {
            label: label.into(),
            bg,
            fg,
            suffix_icon,
            action,
        });
        self
    }
}

fn set_depth(menu: &Entity<TlMenuPopup>, depth: usize, cx: &mut App) {
    let subs = menu.update(cx, |menu, _| {
        menu.depth = depth;
        menu.submenus()
    });
    for sub in subs {
        set_depth(&sub, depth + 1, cx);
    }
}

/// What a test reads of one built entry ([`TlMenuPopup::entries_for_test`]).
#[cfg(any(test, feature = "test-hooks"))]
pub struct MenuEntryInfo {
    pub kind: MenuEntryKind,
    pub label: Option<SharedString>,
    pub tooltip: Option<SharedString>,
    /// A check or radio mark is drawn.
    pub checked: bool,
    /// The shortcut hint drawn at the right.
    pub hint: Option<SharedString>,
    pub action: Option<Box<dyn Action>>,
    pub submenu: Option<Entity<TlMenuPopup>>,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuEntryKind {
    Item,
    Disabled,
    Submenu,
    Separator,
    Header,
}

/// One open menu or submenu. Emits [`DismissEvent`] from the top menu when it closes.
pub struct TlMenuPopup {
    focus: FocusHandle,
    entries: Vec<Entry>,
    has_mark_column: bool,
    min_w: Pixels,
    max_h: Pixels,
    action_context: FocusHandle,
    /// The keyboard and hover highlight, always a selectable entry.
    highlight: Option<usize>,
    /// The submenu entry whose menu is drawn.
    open_sub: Option<usize>,
    parent: Option<WeakEntity<TlMenuPopup>>,
    /// 0 for the top menu, 1 for its submenus and so on.
    depth: usize,
    scroll: ScrollHandle,
    /// Border box in window coordinates, from the last paint.
    bounds: Bounds<Pixels>,
    /// A pending hover open or close of a submenu.
    hover_task: Option<Task<()>>,
}

impl EventEmitter<DismissEvent> for TlMenuPopup {}

impl Focusable for TlMenuPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TlMenuPopup {
    /// The highlighted entry index, if any. Separators and headers count as entries.
    pub fn highlighted_index(&self) -> Option<usize> {
        self.highlight
    }

    /// Highlights the first enabled item, as a menu opened from the keyboard shows it.
    pub fn highlight_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.jump(false, window, cx);
    }

    /// Every entry as built, top to bottom, with the shortcut hint the menu draws (tests).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn entries_for_test(&self, window: &Window) -> Vec<MenuEntryInfo> {
        self.entries
            .iter()
            .map(|e| {
                let (kind, tooltip, checked, action, submenu) = match e {
                    Entry::Item {
                        action,
                        mark,
                        tooltip,
                        ..
                    } => (
                        MenuEntryKind::Item,
                        tooltip.clone(),
                        matches!(mark, Mark::Check(true) | Mark::Radio(true)),
                        Some(action.boxed_clone()),
                        None,
                    ),
                    Entry::Disabled { tooltip, .. } => {
                        (MenuEntryKind::Disabled, tooltip.clone(), false, None, None)
                    }
                    Entry::Painted { action, .. } => (
                        MenuEntryKind::Item,
                        None,
                        false,
                        Some(action.boxed_clone()),
                        None,
                    ),
                    Entry::Submenu { menu, tooltip, .. } => (
                        MenuEntryKind::Submenu,
                        tooltip.clone(),
                        false,
                        None,
                        Some(menu.clone()),
                    ),
                    Entry::Separator => (MenuEntryKind::Separator, None, false, None, None),
                    Entry::Header(_) => (MenuEntryKind::Header, None, false, None, None),
                };
                let hint = action
                    .as_deref()
                    .and_then(|a| shortcut_hint(a, &self.action_context, window));
                MenuEntryInfo {
                    kind,
                    label: e.label().cloned(),
                    tooltip,
                    checked,
                    hint,
                    action,
                    submenu,
                }
            })
            .collect()
    }

    fn submenus(&self) -> Vec<Entity<TlMenuPopup>> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Submenu { menu, .. } => Some(menu.clone()),
                _ => None,
            })
            .collect()
    }

    fn submenu_at(&self, ix: usize) -> Option<Entity<TlMenuPopup>> {
        match self.entries.get(ix) {
            Some(Entry::Submenu { menu, .. }) => Some(menu.clone()),
            _ => None,
        }
    }

    fn parent(&self) -> Option<Entity<TlMenuPopup>> {
        self.parent.as_ref().and_then(WeakEntity::upgrade)
    }

    fn selectable(&self) -> Vec<usize> {
        (0..self.entries.len())
            .filter(|&ix| self.entries[ix].selectable())
            .collect()
    }

    /// Moves the highlight from the keyboard. A submenu the mouse opened on another row closes.
    fn set_highlight(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.hover_task = None;
        if self.open_sub.is_some_and(|open| open != ix) {
            self.close_sub(window, cx);
        }
        self.highlight = Some(ix);
        self.scroll.scroll_to_item(ix);
        cx.notify();
    }

    /// Moves the highlight one item up or down, wrapping.
    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.selectable();
        let n = sel.len();
        if n == 0 {
            return;
        }
        let pos = match self
            .highlight
            .and_then(|h| sel.iter().position(|&ix| ix == h))
        {
            Some(pos) if forward => (pos + 1) % n,
            Some(pos) => (pos + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        self.set_highlight(sel[pos], window, cx);
    }

    /// Highlights the first or the last item.
    fn jump(&mut self, last: bool, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.selectable();
        let ix = if last { sel.last() } else { sel.first() };
        if let Some(&ix) = ix {
            self.set_highlight(ix, window, cx);
        }
    }

    /// Highlights the next item whose label starts with `ch`, wrapping.
    fn type_ahead(&mut self, ch: char, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.selectable();
        let n = sel.len();
        let start = self
            .highlight
            .and_then(|h| sel.iter().position(|&ix| ix == h));
        let ch = ch.to_lowercase().collect::<String>();
        for step in 1..=n {
            let pos = start.map_or(step - 1, |s| (s + step) % n);
            let ix = sel[pos];
            let matches = self.entries[ix]
                .label()
                .is_some_and(|l| l.to_lowercase().starts_with(&ch));
            if matches {
                self.set_highlight(ix, window, cx);
                return;
            }
        }
    }

    /// Opens the submenu at `ix`. With `focus_first` the submenu takes the keys and highlights
    /// its first item.
    fn open_submenu(
        &mut self,
        ix: usize,
        focus_first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(sub) = self.submenu_at(ix) else {
            return false;
        };
        self.hover_task = None;
        if self.open_sub.is_some_and(|open| open != ix) {
            self.close_sub(window, cx);
        }
        self.open_sub = Some(ix);
        self.highlight = Some(ix);
        if focus_first {
            sub.update(cx, |sub, cx| {
                sub.highlight = None;
                sub.jump(false, window, cx);
                sub.focus.focus(window, cx);
            });
        }
        cx.notify();
        true
    }

    /// Closes the open submenu, and any below it. The keys come back here if they were there.
    fn close_sub(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sub) = self.open_sub.take().and_then(|ix| self.submenu_at(ix)) else {
            return;
        };
        let had_focus = sub.read(cx).focus.contains_focused(window, cx);
        sub.update(cx, |sub, cx| {
            sub.close_sub(window, cx);
            sub.highlight = None;
            sub.hover_task = None;
            cx.notify();
        });
        if had_focus {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Left or Esc in a submenu: closes it and gives the keys back to the parent, whose
    /// highlight stays on the submenu row.
    fn back_to_parent(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(parent) = self.parent() else {
            return false;
        };
        self.close_sub(window, cx);
        self.highlight = None;
        self.hover_task = None;
        parent.update(cx, |parent, cx| {
            parent.open_sub = None;
            parent.hover_task = None;
            parent.focus.focus(window, cx);
            cx.notify();
        });
        cx.notify();
        true
    }

    /// Runs the item at `ix`: focus goes to the action context, the action is dispatched there
    /// and the whole menu closes.
    fn run(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let action = match self.entries.get(ix) {
            Some(Entry::Item { action, .. } | Entry::Painted { action, .. }) => {
                action.boxed_clone()
            }
            _ => return,
        };
        self.action_context.focus(window, cx);
        window.dispatch_action(action, cx);
        self.dismiss(window, cx);
    }

    /// Closes this menu and every menu above it. The top menu emits [`DismissEvent`] and gives
    /// focus back to the action context, unless an item moved focus elsewhere.
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hover_task = None;
        self.highlight = None;
        self.open_sub = None;
        cx.notify();
        if let Some(parent) = self.parent() {
            parent.update(cx, |parent, cx| parent.dismiss(window, cx));
            return;
        }
        let moved_away = window.focused(cx).is_some() && !self.focus.contains_focused(window, cx);
        if !moved_away {
            self.action_context.focus(window, cx);
        }
        cx.emit(DismissEvent);
    }

    /// Whether this menu or one of its open submenus has the keys.
    fn chain_focused(&self, window: &Window, cx: &App) -> bool {
        if self.focus.is_focused(window) {
            return true;
        }
        self.open_sub
            .and_then(|ix| self.submenu_at(ix))
            .is_some_and(|sub| sub.read(cx).chain_focused(window, cx))
    }

    /// Whether `position` is inside this menu or one of its open submenus.
    fn chain_contains(&self, position: Point<Pixels>, cx: &App) -> bool {
        if self.bounds.contains(&position) {
            return true;
        }
        self.open_sub
            .and_then(|ix| self.submenu_at(ix))
            .is_some_and(|sub| sub.read(cx).chain_contains(position, cx))
    }

    /// After [`SUBMENU_HOVER_DELAY`], opens the submenu at `target`, or closes the open one when
    /// `target` is `None`.
    fn hover_later(&mut self, target: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_sub == target {
            self.hover_task = None;
            return;
        }
        self.hover_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SUBMENU_HOVER_DELAY).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.hover_task = None;
                match target {
                    Some(ix) => {
                        this.open_submenu(ix, false, window, cx);
                    }
                    None => this.close_sub(window, cx),
                }
            });
        }));
    }

    fn on_row_hover(
        &mut self,
        ix: usize,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selectable = self.entries[ix].selectable();
        if !hovered {
            if self.highlight == Some(ix) && self.open_sub != Some(ix) {
                self.highlight = None;
                cx.notify();
            }
            return;
        }
        // The pointer is in this submenu, so its parent row stays highlighted and open.
        if let Some(parent) = self.parent() {
            parent.update(cx, |parent, cx| {
                parent.hover_task = None;
                parent.highlight = parent.open_sub;
                cx.notify();
            });
        }
        if selectable {
            self.highlight = Some(ix);
            cx.notify();
        }
        let target = self.submenu_at(ix).map(|_| ix);
        if target.is_some() || self.open_sub.is_some() {
            self.hover_later(target, window, cx);
        }
    }

    fn on_row_click(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if !self.open_submenu(ix, false, window, cx) {
            self.run(ix, window, cx);
        }
    }

    fn select_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        self.step(false, window, cx);
    }

    fn select_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        self.step(true, window, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, window: &mut Window, cx: &mut Context<Self>) {
        self.jump(false, window, cx);
    }

    fn select_last(&mut self, _: &SelectLast, window: &mut Window, cx: &mut Context<Self>) {
        self.jump(true, window, cx);
    }

    /// In a top menu Left goes on to the owner, so a menu bar can move to the previous menu.
    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        if !self.back_to_parent(window, cx) {
            cx.propagate();
        }
    }

    /// Right on a row that is not a submenu goes on to the owner, so a menu bar can move to
    /// the next menu.
    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let opened = self
            .highlight
            .is_some_and(|ix| self.open_submenu(ix, true, window, cx));
        if !opened {
            cx.propagate();
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.highlight else {
            return;
        };
        if !self.open_submenu(ix, true, window, cx) {
            self.run(ix, window, cx);
        }
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if !self.back_to_parent(window, cx) {
            self.dismiss(window, cx);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        if let Some(ch) = typed_letter(event) {
            self.type_ahead(ch, window, cx);
            cx.stop_propagation();
        }
    }

    fn on_mouse_down_out(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.chain_contains(event.position, cx) {
            self.dismiss(window, cx);
        }
    }

    fn render_row(
        &self,
        ix: usize,
        m: &UiMetrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = current(cx).clone();
        let s = |v: f32| px(v * m.ui_scale);
        let highlighted = self.highlight == Some(ix);
        let mark_column = |mark: Mark| {
            self.has_mark_column.then(|| {
                let content = match mark {
                    Mark::Check(true) => {
                        Some(icon(TlIcon::Check, m.icon.xs, t.text_primary).into_any_element())
                    }
                    Mark::Radio(true) => Some(
                        div()
                            .size(s(RADIO_DOT))
                            .rounded_full()
                            .bg(t.text_primary)
                            .into_any_element(),
                    ),
                    _ => None,
                };
                div()
                    .id("mark")
                    .test_support()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(s(MENU_MARK_W))
                    .children(content)
            })
        };
        let label_text = |label: &SharedString| {
            let mut text = div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_overflow(TextOverflow::Truncate("\u{2026}".into()))
                .child(label.clone());
            *text.text_style() = t_base(m);
            text
        };
        let row = |label: Option<&SharedString>| {
            div()
                .id(ix)
                .test_support()
                .role(Role::MenuItem)
                .when_some(label.cloned(), |d, l| d.aria_label(l))
                .aria_selected(highlighted)
                .flex()
                .flex_none()
                .items_center()
                .h(s(MENU_ITEM_H))
                .rounded(m.radius_small)
                .text_color(t.text_primary)
                .when(highlighted, |d| d.bg(t.menu_hover))
                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    this.on_row_hover(ix, *hovered, window, cx)
                }))
        };
        match &self.entries[ix] {
            Entry::Item {
                label,
                action,
                mark,
                tooltip,
            } => {
                let hint =
                    shortcut_hint(action.as_ref(), &self.action_context, window).map(|hint| {
                        let color = if highlighted {
                            t.text_primary
                        } else {
                            t.text_secondary
                        };
                        let mut hint = div()
                            .flex_none()
                            .ml(m.sp[6])
                            .whitespace_nowrap()
                            .text_color(color)
                            .child(hint);
                        *hint.text_style() = t_xs(m);
                        hint
                    });
                let item = row(Some(label));
                let text = match tooltip {
                    Some(tip) => menu_label(label.clone(), Some(tip.clone()), false, window, cx),
                    None => label_text(label).into_any_element(),
                };
                item.px(m.sp[4])
                    .children(mark_column(*mark))
                    .child(text)
                    .children(hint)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.on_row_click(ix, window, cx)),
                    )
                    .into_any_element()
            }
            Entry::Disabled { label, tooltip } => row(Some(label))
                .px(m.sp[4])
                .children(mark_column(Mark::None))
                .child(menu_label(label.clone(), tooltip.clone(), true, window, cx))
                .into_any_element(),
            Entry::Painted {
                label,
                bg,
                fg,
                suffix_icon,
                ..
            } => row(Some(label))
                .child(painted_row(
                    label.clone(),
                    *bg,
                    *fg,
                    *suffix_icon,
                    highlighted.then_some(t.accent),
                    window,
                    cx,
                ))
                .on_click(cx.listener(move |this, _, window, cx| this.on_row_click(ix, window, cx)))
                .into_any_element(),
            Entry::Submenu {
                label,
                menu,
                tooltip,
            } => {
                let open = self.open_sub == Some(ix);
                let item = row(Some(label));
                let text = match tooltip {
                    Some(tip) => menu_label(label.clone(), Some(tip.clone()), false, window, cx),
                    None => label_text(label).into_any_element(),
                };
                item.relative()
                    .px(m.sp[4])
                    .aria_expanded(open)
                    .children(mark_column(Mark::None))
                    .child(text)
                    .child(div().flex_none().ml(m.sp[6]).child(icon(
                        TlIcon::ChevronRight,
                        m.icon.xs,
                        t.text_secondary,
                    )))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.on_row_click(ix, window, cx)),
                    )
                    .when(open, |d| d.child(self.render_submenu(menu, m, window, cx)))
                    .into_any_element()
            }
            Entry::Separator => div()
                .id(ix)
                .test_support()
                .flex_none()
                .h(px(1.))
                .my(m.sp[2])
                .mx(m.sp[4])
                .bg(t.divider)
                .into_any_element(),
            Entry::Header(label) => {
                let mut header = div()
                    .id(ix)
                    .test_support()
                    .aria_label(label.clone())
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(s(MENU_HEADER_H))
                    .px(m.sp[4])
                    .text_color(t.text_secondary)
                    .children(mark_column(Mark::None))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_overflow(TextOverflow::Truncate("\u{2026}".into()))
                            .child(label.clone()),
                    );
                *header.text_style() = t_xs(m);
                header.into_any_element()
            }
        }
    }

    /// The open submenu, drawn above this menu. It opens to the right with a 4 px overlap, or
    /// to the left when it does not fit, and its first item lines up with the submenu row.
    fn render_submenu(
        &self,
        menu: &Entity<TlMenuPopup>,
        m: &UiMetrics,
        window: &Window,
        cx: &App,
    ) -> AnyElement {
        let overlap = px(SUBMENU_OVERLAP * m.ui_scale);
        // The row's left edge, from this menu's left edge.
        let row_x = BORDER + m.sp[2];
        let sub = menu.read(cx);
        let sub_w = sub.bounds.size.width.max(sub.min_w);
        let margin = m.sp[4];
        let fits_right =
            self.bounds.right() - overlap + sub_w <= window.viewport_size().width - margin;
        let (anchor, dx) = if fits_right {
            (Anchor::TopLeft, self.bounds.size.width - overlap - row_x)
        } else {
            (Anchor::TopRight, overlap - row_x)
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .child(
                deferred(
                    anchored()
                        .anchor(anchor)
                        .offset(point(dx, -(BORDER + m.sp[2])))
                        .snap_to_window_with_margin(margin)
                        .child(
                            div()
                                .id("submenu")
                                .test_support()
                                .occlude()
                                .child(menu.clone()),
                        ),
                )
                .with_priority(POPUP_PRIORITY + self.depth + 1),
            )
            .into_any_element()
    }
}

impl Render for TlMenuPopup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = current(cx).clone();
        let m = *metrics(cx);
        let rows: Vec<_> = (0..self.entries.len())
            .map(|ix| self.render_row(ix, &m, window, cx))
            .collect();
        let view = cx.entity();
        let mut panel = div()
            .id("tl-menu")
            .test_support()
            .role(Role::Menu)
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(self.parent.is_none(), |d| {
                d.on_mouse_down_out(cx.listener(Self::on_mouse_down_out))
            })
            .occlude()
            .relative()
            .flex()
            .flex_col()
            .min_w(self.min_w)
            .max_w(px(MENU_MAX_W * m.ui_scale))
            .bg(t.menu_bg)
            .border(BORDER)
            .border_color(t.divider)
            .rounded(m.radius_small)
            .text_color(t.text_primary)
            .child(
                div()
                    .id("items")
                    .flex()
                    .flex_col()
                    .p(m.sp[2])
                    .max_h(self.max_h)
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows),
            )
            .child(
                // Inside the border, so the stored bounds add it back.
                canvas(
                    move |bounds, _, cx| {
                        view.update(cx, |this, _| this.bounds = bounds.dilate(BORDER));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        panel.style().box_shadow = Some(vec![BoxShadow {
            color: t.derived.shadow,
            offset: point(px(0.), px(2.)),
            blur_radius: px(8.),
            spread_radius: px(0.),
            inset: false,
        }]);
        // A new menu entity fades in once; GPUI draws the end state under reduced motion.
        panel.with_animation(
            ("tl-menu-fade", cx.entity_id()),
            Animation::new(M_FAST).with_easing(ease_out_cubic),
            |panel, delta| panel.opacity(delta),
        )
    }
}

/// The text of the first key binding for `action` as seen from `context`, for example
/// "Shift+Delete". Falls back to a binding with no key context.
fn shortcut_hint(
    action: &dyn Action,
    context: &FocusHandle,
    window: &Window,
) -> Option<SharedString> {
    let binding = window
        .highest_precedence_binding_for_action_in(action, context)
        .or_else(|| {
            window.highest_precedence_binding_for_action_in_context(action, KeyContext::default())
        })?;
    let key = binding.keystrokes().first()?;
    Some(Kbd::format(key.as_keystroke()).into())
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

/// The width a label may take before it ends in an ellipsis: the widest menu less the menu
/// padding (4 a side), the item padding (8 a side) and the painted row's own padding and icon.
fn painted_label_max(m: &UiMetrics) -> Pixels {
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

/// A row label with a hover tooltip. `disabled`: half opacity, as a disabled item.
fn menu_label(
    label: SharedString,
    tooltip: Option<SharedString>,
    disabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = current(cx).clone();
    let m = *metrics(cx);
    let (id, tip_id) = if disabled {
        ("tl-menu-disabled", "tl-menu-disabled-tip")
    } else {
        ("tl-menu-label", "tl-menu-tip")
    };
    let mut row = div()
        .id(id)
        .test_support()
        .aria_label(label.clone())
        .flex_1()
        .min_w_0()
        .when(disabled, |d| d.opacity(0.5))
        .text_color(t.text_primary)
        .whitespace_nowrap()
        .overflow_hidden()
        .text_overflow(TextOverflow::Truncate("\u{2026}".into()))
        .child(label.clone());
    *row.text_style() = t_base(&m);
    if let Some(reason) = tooltip {
        let state = HoverTooltip::state(
            ElementId::Name(format!("{tip_id}-{label}").into()),
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

/// A theme row filling its menu row. `ring` outlines it while highlighted, since its own
/// colours hide the hover fill.
fn painted_row(
    label: SharedString,
    bg: Hsla,
    fg: Hsla,
    suffix_icon: Option<TlIcon>,
    ring: Option<Hsla>,
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
        .h_full()
        .px(m.sp[4])
        .rounded(m.radius_small)
        .bg(bg)
        .text_color(fg)
        .when_some(ring, |d, ring| d.border_1().border_color(ring))
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
    menu: Entity<TlMenuPopup>,
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
    menu: Entity<TlMenuPopup>,
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
    // Keep the keys on the menu while it is open. A submenu that just took focus is not in the
    // last frame yet, so `contains_focused` alone would pull the keys back to the top menu.
    let focus = menu.focus_handle(cx);
    if !focus.contains_focused(window, cx) && !menu.read(cx).chain_focused(window, cx) {
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
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    gpui_kit::actions!(tl_menu_test, [RunA, RunB, RunC, RunX, RunY]);

    fn setup_scaled(cx: &mut App, ui_scale_pct: u16) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings {
                ui_scale_pct,
                ..AppearanceSettings::default()
            },
            Vec::new(),
        );
        crate::i18n::init_strings(cx, "en-US");
        bind_keys(cx);
        cx.set_reduce_motion(true);
    }

    fn setup(cx: &mut App) {
        setup_scaled(cx, 100);
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

    fn host_with(
        cx: &mut TestAppContext,
        init: impl FnOnce(&mut App),
    ) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(init);
        let (view, cx) = cx.add_window_view(|_, cx| Host {
            focus: cx.focus_handle(),
            row_pressed: Rc::default(),
            ran: Rc::default(),
        });
        draw(cx);
        (view, cx)
    }

    fn host(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
        host_with(cx, setup)
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
    ) -> Entity<TlMenuPopup> {
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
        menu_bounds(cx, false)
    }

    fn menu_bounds(cx: &mut VisualTestContext, submenu: bool) -> Vec<Bounds<Pixels>> {
        let mut out: Vec<_> = all(cx)
            .into_iter()
            .filter(|s| s.role() == Some(Role::Menu) && in_submenu(s) == submenu)
            .map(|s| s.bounds())
            .collect();
        out.sort_by(|a, b| a.origin.x.partial_cmp(&b.origin.x).unwrap());
        out
    }

    /// The row with entry index `ix`, in the top menu or the submenu.
    fn row(cx: &mut VisualTestContext, submenu: bool, ix: usize) -> ElementSnapshot {
        all(cx)
            .into_iter()
            .find(|s| {
                in_submenu(s) == submenu
                    && s.path().last() == Some(&ElementId::Integer(ix as u64))
                    && !s
                        .path()
                        .iter()
                        .any(|id| *id == ElementId::Name("mark".into()))
            })
            .unwrap_or_else(|| panic!("no row {ix}"))
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
        const LONG: &str = "Show processes from every user";
        let (view, cx) = host(cx);
        open(&view, cx, point(px(10.), px(40.)), |menu, _, _| {
            let menu = menu.item(LONG, Box::new(RunA));
            assert!(!menu.has_mark_column);
            menu
        });
        let plain = menus(cx)[0];
        cx.update(close_context_menu);
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
        cx.update(close_context_menu);
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

    #[gpui_kit::test]
    fn home_and_end_jump_to_first_and_last_enabled(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(100.), px(100.)), |menu, _, _| {
            menu.header("Group")
                .item_disabled("Off", None)
                .item("A", Box::new(RunA))
                .separator()
                .item("C", Box::new(RunC))
                .item_disabled("Off too", None)
        });

        keys(cx, "end");
        assert_eq!(highlighted(cx), vec![(false, 4)], "C, not the disabled row");
        keys(cx, "home");
        assert_eq!(highlighted(cx), vec![(false, 2)], "A, not the header");
    }

    #[gpui_kit::test]
    fn typed_letter_jumps_to_next_match(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(100.), px(100.)), |menu, _, _| {
            menu.item("Apple", Box::new(RunA))
                .item("Banana", Box::new(RunB))
                .item("avocado", Box::new(RunC))
                .item_disabled("Bad", None)
                .item("Blue", Box::new(RunX))
        });

        keys(cx, "b");
        assert_eq!(highlighted(cx), vec![(false, 1)]);
        keys(cx, "b");
        assert_eq!(highlighted(cx), vec![(false, 4)], "skips the disabled row");
        keys(cx, "b");
        assert_eq!(highlighted(cx), vec![(false, 1)], "wraps");
        keys(cx, "a");
        assert_eq!(highlighted(cx), vec![(false, 2)], "ignores case");
        keys(cx, "z");
        assert_eq!(highlighted(cx), vec![(false, 2)], "no match keeps it");
    }

    #[gpui_kit::test]
    fn down_and_right_skip_headers_and_disabled(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(100.), px(100.)), |menu, window, cx| {
            menu.header("Group")
                .item_disabled("Off", None)
                .item("A", Box::new(RunA))
                .submenu("More", window, cx, |menu, _, _| {
                    menu.header("Inner")
                        .item_disabled("Off", None)
                        .item("X", Box::new(RunX))
                })
        });

        keys(cx, "down");
        assert_eq!(
            highlighted(cx),
            vec![(false, 2)],
            "skips the header and Off"
        );
        keys(cx, "down");
        assert_eq!(highlighted(cx), vec![(false, 3)]);
        assert!(
            menu_bounds(cx, true).is_empty(),
            "highlight alone opens nothing"
        );
        keys(cx, "right");
        assert_eq!(
            highlighted(cx),
            vec![(false, 3), (true, 2)],
            "X, not the submenu header"
        );
        keys(cx, "down");
        assert_eq!(highlighted(cx), vec![(false, 3), (true, 2)], "wraps onto X");
    }

    #[gpui_kit::test]
    fn left_and_escape_close_submenu(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        let menu = open(&view, cx, point(px(100.), px(100.)), |menu, window, cx| {
            menu.item("A", Box::new(RunA))
                .submenu("More", window, cx, |menu, _, _| {
                    menu.item("X", Box::new(RunX)).item("Y", Box::new(RunY))
                })
        });

        for close in ["left", "escape"] {
            keys(cx, "end right down");
            assert_eq!(highlighted(cx), vec![(false, 1), (true, 1)]);
            keys(cx, close);
            assert_eq!(highlighted(cx), vec![(false, 1)], "{close}");
            assert!(
                menu_bounds(cx, true).is_empty(),
                "{close}: submenu not drawn"
            );
            assert!(
                cx.update(|window, cx| menu.focus_handle(cx).is_focused(window)),
                "{close}: the parent has the keys"
            );
            assert!(cx.update(|window, cx| context_menu_open(window, cx)));
        }
        keys(cx, "down");
        assert_eq!(highlighted(cx), vec![(false, 0)], "the parent moves on");
    }

    #[gpui_kit::test]
    fn submenu_runs_item_and_closes_all(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(100.), px(100.)), |menu, window, cx| {
            menu.item("X", Box::new(RunX))
                .submenu("More", window, cx, |menu, _, _| {
                    menu.item("A", Box::new(RunA))
                })
        });
        keys(cx, "end enter enter");
        assert_eq!(*view.read_with(cx, |h, _| h.ran.clone()).borrow(), ["A"]);
        assert!(!cx.update(|window, cx| context_menu_open(window, cx)));
        let focus = view.read_with(cx, |h, _| h.focus.clone());
        assert!(cx.update(|window, _| focus.is_focused(window)));
    }

    #[gpui_kit::test]
    fn geometry_matches_spec(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(100.), px(100.)), |menu, window, cx| {
            menu.header("Group")
                .check_item("A", true, Box::new(RunA))
                .separator()
                .submenu("More", window, cx, |menu, _, _| {
                    menu.item("X", Box::new(RunX))
                })
        });

        let header = row(cx, false, 0);
        assert_eq!(header.label(), Some("Group"));
        assert_eq!(header.bounds().size.height, px(MENU_HEADER_H));
        let separator = row(cx, false, 2);
        assert_eq!(separator.bounds().size.height, px(1.));

        let mark = all(cx)
            .into_iter()
            .find(|s| s.path().ends_with(&[ElementId::Integer(1), "mark".into()]))
            .expect("mark column");
        assert_eq!(mark.bounds().size.width, px(MENU_MARK_W));

        keys(cx, "end right");
        let parent = menus(cx)[0];
        let sub = menu_bounds(cx, true);
        assert_eq!(sub.len(), 1);
        assert_eq!(
            sub[0].left(),
            parent.right() - px(SUBMENU_OVERLAP),
            "4 px overlap"
        );
        assert_eq!(
            row(cx, true, 0).bounds().top(),
            row(cx, false, 3).bounds().top(),
            "the first item lines up with the submenu row"
        );
    }

    #[gpui_kit::test]
    fn submenu_flips_left_at_the_right_edge(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        let width = cx.update(|window, _| window.viewport_size().width);
        open(
            &view,
            cx,
            point(width - px(200.), px(100.)),
            |menu, window, cx| {
                menu.submenu("More", window, cx, |menu, _, _| {
                    menu.item("X", Box::new(RunX))
                })
            },
        );
        keys(cx, "down right");
        // The first frame may use the submenu's minimum width; the next one its real width.
        draw(cx);
        let parent = menus(cx)[0];
        let sub = menu_bounds(cx, true)[0];
        assert_eq!(sub.right(), parent.left() + px(SUBMENU_OVERLAP));
    }

    #[gpui_kit::test]
    fn row_height_follows_ui_scale(cx: &mut TestAppContext) {
        let (view, cx) = host_with(cx, |cx| setup_scaled(cx, 110));
        open(&view, cx, point(px(100.), px(100.)), |menu, _, _| {
            menu.header("Group")
                .check_item("A", true, Box::new(RunA))
                .item("B", Box::new(RunB))
        });
        // Layout snaps to device pixels, half a logical pixel in the test window.
        let close = |a: Pixels, b: f32| (f32::from(a) - b).abs() <= 0.25;
        let item = row(cx, false, 1).bounds().size.height;
        assert!(close(item, MENU_ITEM_H * 1.1), "{item:?}");
        let header = row(cx, false, 0).bounds().size.height;
        assert!(close(header, MENU_HEADER_H * 1.1), "{header:?}");
        let width = menus(cx)[0].size.width;
        assert!(f32::from(width) >= MENU_MIN_W * 1.1 - 0.01, "{width:?}");
    }

    #[gpui_kit::test]
    fn hover_opens_submenu_after_delay(cx: &mut TestAppContext) {
        let (view, cx) = host(cx);
        open(&view, cx, point(px(100.), px(100.)), |menu, window, cx| {
            menu.item("A", Box::new(RunA))
                .submenu("More", window, cx, |menu, _, _| {
                    menu.item("X", Box::new(RunX))
                })
        });
        let more = row(cx, false, 1).bounds().center();
        cx.simulate_mouse_move(more, None, Modifiers::none());
        draw(cx);
        assert_eq!(highlighted(cx), vec![(false, 1)]);
        assert!(menu_bounds(cx, true).is_empty(), "not before 150 ms");
        cx.executor().advance_clock(SUBMENU_HOVER_DELAY);
        draw(cx);
        assert_eq!(menu_bounds(cx, true).len(), 1, "open after 150 ms");

        let a = row(cx, false, 0).bounds().center();
        cx.simulate_mouse_move(a, None, Modifiers::none());
        draw(cx);
        cx.executor().advance_clock(SUBMENU_HOVER_DELAY);
        draw(cx);
        assert!(menu_bounds(cx, true).is_empty(), "closed by hovering A");
        assert_eq!(highlighted(cx), vec![(false, 0)]);
    }
}
