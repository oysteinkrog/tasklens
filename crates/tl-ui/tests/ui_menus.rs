//! Menu bar and File menu tests (E21-A1, E21-044, UI-SHL-030, UI-SHL-031, UI-SHL-032, ADR-013).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::AsKeystroke as _;
use gpui_kit::base::test_support::{ElementSnapshot, snapshots};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::{
    AppContext as _, Bounds, Context, ElementId, Entity, FocusHandle, InteractiveElement,
    IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, Render, Role, Styled, TestAppContext, VisualTestContext, Window,
    div, px, size,
};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_ui::actions::{MenuId, OpenRunDialog, Quit, bind_keys_global};
use tl_ui::theme::current;
use tl_ui::window::menubar::{self, MenuBar, menu_bar_for};
use tl_ui::window::titlebar::{TitleBarProbe, TitleBarView};

const FILE_BUTTON: &str = "TlMenuBarButton-shell-menubar-file";

/// A main window stand-in: the title bar with the real menu bar, and a focusable table below.
struct Host {
    table: FocusHandle,
    bar: Entity<TitleBarView>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Global")
            .flex()
            .flex_col()
            .size_full()
            .child(self.bar.clone())
            .child(div().id("table").track_focus(&self.table).size_full())
    }
}

struct Rig<'a> {
    cx: &'a mut VisualTestContext,
    table: FocusHandle,
    quits: Rc<Cell<u32>>,
    runs: Rc<Cell<u32>>,
}

fn rig(cx: &mut TestAppContext) -> Rig<'_> {
    let quits = Rc::new(Cell::new(0));
    let count = Rc::clone(&quits);
    let runs = Rc::new(Cell::new(0));
    let run_count = Rc::clone(&runs);
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::i18n::init_strings(cx, "en-US");
        cx.set_global(TitleBarProbe {
            client: Some(true),
            ..TitleBarProbe::default()
        });
        bind_keys_global(cx);
        menubar::init(cx);
        cx.on_action(move |_: &Quit, _| count.set(count.get() + 1));
        cx.on_action(move |_: &OpenRunDialog, _| run_count.set(run_count.get() + 1));
        cx.set_reduce_motion(true);
    });
    let (host, cx) = cx.add_window_view(|window, cx| Host {
        table: cx.focus_handle(),
        bar: cx.new(|cx| TitleBarView::new(window, cx)),
    });
    cx.simulate_resize(size(px(1000.), px(400.)));
    cx.update(|window, _| window.activate_window());
    let table = cx.update(|window, cx| {
        let table = host.read(cx).table.clone();
        table.focus(window, cx);
        table
    });
    draw(cx);
    Rig {
        cx,
        table,
        quits,
        runs,
    }
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

fn bar(cx: &mut VisualTestContext) -> Entity<MenuBar> {
    cx.update(|window, cx| menu_bar_for(window.window_handle().window_id(), cx))
        .expect("menu bar registered")
}

fn open_menu(cx: &mut VisualTestContext) -> Option<MenuId> {
    let bar = bar(cx);
    cx.update(|_, cx| bar.read(cx).open_menu_id())
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} drawn"))
}

fn mouse_down(cx: &mut VisualTestContext, at: Point<Pixels>, button: MouseButton) {
    cx.simulate_event(MouseDownEvent {
        position: at,
        button,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
}

fn click(cx: &mut VisualTestContext, at: Point<Pixels>) {
    mouse_down(cx, at, MouseButton::Left);
    cx.simulate_event(MouseUpEvent {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    draw(cx);
}

fn click_file(cx: &mut VisualTestContext) {
    let at = bounds(cx, FILE_BUTTON).center();
    click(cx, at);
}

fn in_menu(s: &ElementSnapshot) -> bool {
    s.path()
        .iter()
        .any(|id| *id == ElementId::Name("tl-menu".into()))
}

/// The popup's entries (items, separators and headers), top to bottom.
fn entries(cx: &mut VisualTestContext) -> Vec<ElementSnapshot> {
    let mut entries: Vec<_> = cx
        .update(|window, _| snapshots(window))
        .into_iter()
        .filter(|s| in_menu(s) && matches!(s.path().last(), Some(ElementId::Integer(_))))
        .collect();
    entries.sort_by_key(|s| s.bounds().origin.y);
    entries
}

fn popup_bounds(cx: &mut VisualTestContext) -> Option<Bounds<Pixels>> {
    cx.update(|window, _| snapshots(window))
        .into_iter()
        .find(|s| s.role() == Some(Role::Menu))
        .map(|s| s.bounds())
}

fn table_focused(r: &mut Rig<'_>) -> bool {
    let table = r.table.clone();
    r.cx.update(|window, _| table.is_focused(window))
}

/// The shortcut hint the menu draws for `action`: its first binding as seen from the menu's action
/// context, the focused table.
fn hint_for(r: &mut Rig<'_>, action: &dyn gpui_kit::Action) -> Option<String> {
    let table = r.table.clone();
    r.cx.update(|window, _| {
        let binding = window.highest_precedence_binding_for_action_in(action, &table)?;
        let key = binding.keystrokes().first()?;
        Some(Kbd::format(key.as_keystroke()))
    })
}

#[gpui_kit::test]
fn file_menu_p2_items(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    // One button, one separator one physical pixel wide, and nothing for the absent menus.
    let bar_w = bounds(r.cx, "TlMenuBar").size.width;
    let file = bounds(r.cx, FILE_BUTTON);
    let scale = r.cx.update(|window, _| window.scale_factor());
    assert_eq!(bar_w, file.size.width + px(1. / scale));
    for absent in [
        "TlMenuBarButton-shell-menubar-view",
        "TlMenuBarButton-shell-menubar-settings",
        "TlMenuBarButton-shell-menubar-help",
        "TlMenuBarButton-shell-menubar-debug",
    ] {
        assert!(
            r.cx.debug_bounds(absent).is_none(),
            "{absent} has no button"
        );
    }

    click_file(r.cx);
    let entries = entries(r.cx);
    assert_eq!(entries.len(), 3, "Run..., separator, Exit: {entries:?}");
    assert_eq!(entries[0].role(), Some(Role::MenuItem));
    assert_eq!(entries[0].label(), Some("Run..."));
    assert_eq!(entries[1].role(), None, "a separator");
    assert_eq!(entries[1].label(), None);
    assert!(entries[1].bounds().size.height < px(2.), "a one-pixel line");
    assert_eq!(entries[2].role(), Some(Role::MenuItem));
    assert_eq!(entries[2].label(), Some("Exit tasklens"));
    // The hints come from the live bindings.
    assert_eq!(hint_for(&mut r, &OpenRunDialog).as_deref(), Some("Ctrl+N"));
    assert_eq!(hint_for(&mut r, &Quit).as_deref(), Some("Ctrl+Q"));
}

#[gpui_kit::test]
fn file_menu_run_dispatch(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    keys(r.cx, "f10");
    assert_eq!(
        entries(r.cx)[0].selected(),
        Some(true),
        "Run... highlighted"
    );
    keys(r.cx, "enter");
    assert_eq!(r.runs.get(), 1);
    assert_eq!(r.quits.get(), 0);
    assert_eq!(open_menu(r.cx), None);
    assert!(popup_bounds(r.cx).is_none());
    assert!(table_focused(&mut r));

    // A click on the item does the same.
    click_file(r.cx);
    let item = entries(r.cx)[0].bounds().center();
    click(r.cx, item);
    assert_eq!(r.runs.get(), 2);
    assert_eq!(r.quits.get(), 0);
    assert_eq!(open_menu(r.cx), None);
}

#[gpui_kit::test]
fn menubar_click_toggle(cx: &mut TestAppContext) {
    let r = rig(cx);
    click_file(r.cx);
    assert_eq!(open_menu(r.cx), Some(MenuId::File));
    // The popup's top edge is the title bar's bottom edge; its left edge is the button's.
    let popup = popup_bounds(r.cx).expect("popup drawn");
    let file = bounds(r.cx, FILE_BUTTON);
    assert_eq!(popup.origin.y, bounds(r.cx, "TitleBar").bottom());
    assert_eq!(popup.origin.x, file.origin.x);

    click_file(r.cx);
    assert_eq!(open_menu(r.cx), None);
    assert!(popup_bounds(r.cx).is_none());
}

#[gpui_kit::test]
fn menubar_f10_opens_file(cx: &mut TestAppContext) {
    let r = rig(cx);
    keys(r.cx, "f10");
    assert_eq!(open_menu(r.cx), Some(MenuId::File));
    let entries = entries(r.cx);
    assert_eq!(entries[0].selected(), Some(true), "Run... highlighted");
}

#[gpui_kit::test]
fn menu_esc_returns_focus(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    keys(r.cx, "f10");
    assert!(!table_focused(&mut r), "the menu has the keys");
    keys(r.cx, "escape");
    assert_eq!(open_menu(r.cx), None);
    assert!(popup_bounds(r.cx).is_none());
    assert!(table_focused(&mut r));
}

#[gpui_kit::test]
fn menubar_wrap_single_menu(cx: &mut TestAppContext) {
    let r = rig(cx);
    keys(r.cx, "f10");
    keys(r.cx, "left");
    assert_eq!(open_menu(r.cx), Some(MenuId::File));
    assert!(popup_bounds(r.cx).is_some());
    keys(r.cx, "right");
    assert_eq!(open_menu(r.cx), Some(MenuId::File));
    assert!(popup_bounds(r.cx).is_some());
    assert_eq!(entries(r.cx)[0].selected(), Some(true));
}

#[gpui_kit::test]
fn exit_dispatches_quit(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    keys(r.cx, "f10");
    keys(r.cx, "down");
    assert_eq!(
        entries(r.cx)[2].selected(),
        Some(true),
        "Down skips the separator"
    );
    keys(r.cx, "enter");
    assert_eq!(r.quits.get(), 1);
    assert_eq!(r.runs.get(), 0);
    assert_eq!(open_menu(r.cx), None);
    assert!(popup_bounds(r.cx).is_none());
    assert!(table_focused(&mut r));

    // A click on the item does the same.
    click_file(r.cx);
    let item = entries(r.cx)[2].bounds().center();
    click(r.cx, item);
    assert_eq!(r.quits.get(), 2);
    assert_eq!(open_menu(r.cx), None);
}

#[gpui_kit::test]
fn alt_alone_does_nothing(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    r.cx.simulate_modifiers_change(Modifiers::alt());
    r.cx.simulate_modifiers_change(Modifiers::none());
    draw(r.cx);
    assert_eq!(open_menu(r.cx), None);
    assert!(popup_bounds(r.cx).is_none());
    assert!(table_focused(&mut r));
}

#[gpui_kit::test]
fn menu_button_no_drag(cx: &mut TestAppContext) {
    let r = rig(cx);
    let at = bounds(r.cx, FILE_BUTTON).center();
    mouse_down(r.cx, at, MouseButton::Left);
    r.cx.simulate_event(MouseMoveEvent {
        position: at + Point::new(px(20.), px(0.)),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::none(),
    });
    let moves = r.cx.update(|_, cx| cx.global::<TitleBarProbe>().moves);
    assert_eq!(moves, 0);

    // The probe does see a press on the drag area.
    let drag = bounds(r.cx, "TitleBarDrag").center();
    mouse_down(r.cx, drag, MouseButton::Left);
    r.cx.simulate_event(MouseMoveEvent {
        position: drag + Point::new(px(20.), px(0.)),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::none(),
    });
    let moves = r.cx.update(|_, cx| cx.global::<TitleBarProbe>().moves);
    assert_eq!(moves, 1);
}

#[gpui_kit::test]
fn menubar_inactive_labels(cx: &mut TestAppContext) {
    let r = rig(cx);
    let bar = bar(r.cx);
    let (primary, secondary) = r.cx.update(|_, cx| {
        let t = current(cx);
        (t.text_primary, t.text_secondary)
    });
    let color = |cx: &mut VisualTestContext| cx.update(|_, cx| bar.read(cx).drawn_label_color());
    assert_eq!(color(r.cx), Some(primary));

    r.cx.deactivate_window();
    draw(r.cx);
    assert_eq!(color(r.cx), Some(secondary));
}
