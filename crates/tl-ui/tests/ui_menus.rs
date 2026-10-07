//! Menu bar, File menu and View menu tests (E21-A1, E21-001, E21-044, UI-SHL-030 to UI-SHL-034,
//! ADR-013).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::{Cell, RefCell};
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
use tl_model::command::Tab;
use tl_model::settings::{AppearanceSettings, ColumnResize, Settings, ThemeSettings, UiState};
use tl_model::table::TableMode;
use tl_ui::actions::{
    MenuId, OpenRunDialog, Quit, SetViewMode, bind_keys_global, register_global_handlers,
};
use tl_ui::app::BootSettings;
use tl_ui::settings_global::TlSettings;
use tl_ui::table::actions::{
    CollapseAll, OpenColumnPicker, ResetColumns, ResizeColumnsToFit, TABLE_CONTEXT, bind_keys_table,
};
use tl_ui::table::columns::TableId;
use tl_ui::theme::current;
use tl_ui::window::main::ShellState;
use tl_ui::window::menubar::{self, MenuBar, menu_bar_for};
use tl_ui::window::menus_view;
use tl_ui::window::titlebar::{TitleBarProbe, TitleBarView};

const FILE_BUTTON: &str = "TlMenuBarButton-shell-menubar-file";
const VIEW_BUTTON: &str = "TlMenuBarButton-shell-menubar-view";

/// A main window stand-in: the title bar with the real menu bar, and a focusable table below.
struct Host {
    table: FocusHandle,
    bar: Entity<TitleBarView>,
    /// `CollapseAll` actions the table received.
    collapses: Rc<Cell<u32>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Global")
            .flex()
            .flex_col()
            .size_full()
            .child(self.bar.clone())
            .child({
                let collapses = self.collapses.clone();
                div()
                    .id("table")
                    .key_context(TABLE_CONTEXT)
                    .track_focus(&self.table)
                    .on_action(move |_: &CollapseAll, _, _| collapses.set(collapses.get() + 1))
                    .size_full()
            })
    }
}

struct Rig<'a> {
    cx: &'a mut VisualTestContext,
    table: FocusHandle,
    quits: Rc<Cell<u32>>,
    runs: Rc<Cell<u32>>,
    /// The View > Columns table actions, by name and table.
    tables: Rc<RefCell<Vec<(&'static str, TableId)>>>,
    collapses: Rc<Cell<u32>>,
    modes: Rc<RefCell<Vec<TableMode>>>,
}

fn rig(cx: &mut TestAppContext) -> Rig<'_> {
    let quits = Rc::new(Cell::new(0));
    let count = Rc::clone(&quits);
    let runs = Rc::new(Cell::new(0));
    let run_count = Rc::clone(&runs);
    let tables = Rc::new(RefCell::new(Vec::new()));
    let (t1, t2, t3) = (tables.clone(), tables.clone(), tables.clone());
    let collapses = Rc::new(Cell::new(0));
    let modes = Rc::new(RefCell::new(Vec::new()));
    let mode_log = modes.clone();
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
        cx.on_action(move |a: &OpenColumnPicker, _| {
            t1.borrow_mut().push(("OpenColumnPicker", a.table))
        });
        cx.on_action(move |a: &ResizeColumnsToFit, _| {
            t2.borrow_mut().push(("ResizeColumnsToFit", a.table))
        });
        cx.on_action(move |a: &ResetColumns, _| t3.borrow_mut().push(("ResetColumns", a.table)));
        cx.on_action(move |a: &SetViewMode, _| mode_log.borrow_mut().push(a.0));
        bind_keys_table(cx);
        cx.set_global(ShellState::default());
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        register_global_handlers(cx);
        cx.set_reduce_motion(true);
    });
    let host_collapses = collapses.clone();
    let (host, cx) = cx.add_window_view(|window, cx| Host {
        table: cx.focus_handle(),
        bar: cx.new(|cx| TitleBarView::new(window, cx)),
        collapses: host_collapses,
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
        tables,
        collapses,
        modes,
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
    // File and View, each followed by a separator one physical pixel wide, and nothing for the
    // absent menus.
    let bar_w = bounds(r.cx, "TlMenuBar").size.width;
    let file = bounds(r.cx, FILE_BUTTON);
    let view = bounds(r.cx, VIEW_BUTTON);
    let scale = r.cx.update(|window, _| window.scale_factor());
    assert_eq!(bar_w, file.size.width + view.size.width + px(2. / scale));
    for absent in [
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
fn menubar_left_right_wrap(cx: &mut TestAppContext) {
    let r = rig(cx);
    // In File, Right on "Exit tasklens" opens View.
    keys(r.cx, "f10 down");
    assert_eq!(entries(r.cx)[2].label(), Some("Exit tasklens"));
    assert_eq!(entries(r.cx)[2].selected(), Some(true));
    keys(r.cx, "right");
    assert_eq!(open_menu(r.cx), Some(MenuId::View));
    assert_eq!(entries(r.cx)[0].selected(), Some(true));
    // In View, Left at the top level opens File.
    keys(r.cx, "left");
    assert_eq!(open_menu(r.cx), Some(MenuId::File));
    // In File, Left wraps to View, the last present menu.
    keys(r.cx, "left");
    assert_eq!(open_menu(r.cx), Some(MenuId::View));
    assert!(popup_bounds(r.cx).is_some());
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

// View menu (E21-001).

fn set_tab(cx: &mut VisualTestContext, tab: Tab, history: bool) {
    cx.update(|_, cx| {
        let shell = cx.global_mut::<ShellState>();
        shell.active_tab = tab;
        shell.history_mode = history;
    });
}

/// The labels of the open submenus: every drawn label except the View root items.
fn sub_labels(cx: &mut VisualTestContext) -> Vec<String> {
    labels(cx)
        .into_iter()
        .filter(|l| l != "Columns" && l != "Process View")
        .collect()
}

/// The labels of the drawn menu entries whose label is set, top to bottom.
fn labels(cx: &mut VisualTestContext) -> Vec<String> {
    entries(cx)
        .iter()
        .filter_map(|e| e.label().map(str::to_owned))
        .collect()
}

#[gpui_kit::test]
fn menubar_alt_keys(cx: &mut TestAppContext) {
    let r = rig(cx);
    keys(r.cx, "alt-v");
    assert_eq!(open_menu(r.cx), Some(MenuId::View));
    let entries = entries(r.cx);
    assert_eq!(entries.len(), 2, "Columns, Process View: {entries:?}");
    assert_eq!(entries[0].label(), Some("Columns"));
    assert_eq!(entries[0].selected(), Some(true), "Columns highlighted");
    assert_eq!(entries[1].label(), Some("Process View"));
}

#[gpui_kit::test]
fn menubar_hover_switch(cx: &mut TestAppContext) {
    let r = rig(cx);
    click_file(r.cx);
    assert_eq!(open_menu(r.cx), Some(MenuId::File));
    let view = bounds(r.cx, VIEW_BUTTON).center();
    r.cx.simulate_mouse_move(view, None, Modifiers::none());
    draw(r.cx);
    assert_eq!(open_menu(r.cx), Some(MenuId::View));
    // One popup, View's, below the View button.
    assert_eq!(labels(r.cx), ["Columns", "Process View"]);
    let popup = popup_bounds(r.cx).expect("popup drawn");
    assert_eq!(popup.origin.x, bounds(r.cx, VIEW_BUTTON).origin.x);
}

#[gpui_kit::test]
fn columns_menu_targets_active_table(cx: &mut TestAppContext) {
    let r = rig(cx);
    set_tab(r.cx, Tab::Services, false);
    keys(r.cx, "alt-v right");
    let submenu = sub_labels(r.cx);
    assert_eq!(
        submenu,
        [
            "Select Columns…",
            "Resize columns to fit content",
            "Resize Behavior",
            "Reset to default",
        ]
    );
    // Down skips the separator above "Reset to default".
    keys(r.cx, "down down down enter");
    assert_eq!(*r.tables.borrow(), [("ResetColumns", TableId::Services)]);
    assert_eq!(open_menu(r.cx), None);

    keys(r.cx, "alt-v right down enter");
    assert_eq!(
        r.tables.borrow().last(),
        Some(&("ResizeColumnsToFit", TableId::Services))
    );
}

#[gpui_kit::test]
fn columns_menu_history_table(cx: &mut TestAppContext) {
    let r = rig(cx);
    set_tab(r.cx, Tab::Processes, true);
    keys(r.cx, "alt-v right enter");
    assert_eq!(*r.tables.borrow(), [("OpenColumnPicker", TableId::History)]);

    set_tab(r.cx, Tab::Processes, false);
    keys(r.cx, "alt-v right enter");
    assert_eq!(
        r.tables.borrow().last(),
        Some(&("OpenColumnPicker", TableId::Processes))
    );
    for (tab, table) in [
        (Tab::Startup, Some(TableId::Startup)),
        (Tab::Connections, Some(TableId::Connections)),
        (Tab::Information, None),
    ] {
        set_tab(r.cx, tab, false);
        assert_eq!(r.cx.update(|_, cx| menus_view::active_table(cx)), table);
    }
}

#[gpui_kit::test]
fn columns_disabled_on_performance(cx: &mut TestAppContext) {
    let r = rig(cx);
    set_tab(r.cx, Tab::Performance, false);
    keys(r.cx, "alt-v");
    assert_eq!(open_menu(r.cx), Some(MenuId::View));
    let drawn = entries(r.cx);
    assert_eq!(drawn.len(), 2);
    assert_eq!(drawn[0].label(), Some("Columns"));
    assert_ne!(
        drawn[0].selected(),
        Some(true),
        "a disabled item is never highlighted"
    );
    assert_eq!(drawn[1].selected(), Some(true), "Process View highlighted");
    assert!(r.tables.borrow().is_empty());

    // The tooltip opens on hover after the control delay and gives the reason.
    let row = entries(r.cx)[0].bounds();
    r.cx.simulate_mouse_move(row.center(), None, Modifiers::none());
    r.cx.executor()
        .advance_clock(tl_ui::components::tooltip::DELAY_CONTROL * 2);
    draw(r.cx);
    assert!(r.cx.debug_bounds("TlTooltip").is_some(), "tooltip shown");
    let reason =
        r.cx.update(|_, cx| tl_ui::i18n::tr(cx, "shell-menu-view-no-table"));
    assert_eq!(reason.as_ref(), "This tab has no table");
}

#[gpui_kit::test]
fn resize_behavior_radio(cx: &mut TestAppContext) {
    let r = rig(cx);
    let resize = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| TlSettings::settings(cx).tables.column_resize)
    };
    assert_eq!(resize(r.cx), ColumnResize::Proportional);
    // View > Columns > Resize Behavior: the mark sits on "Proportional".
    keys(r.cx, "alt-v right down down right");
    let marks = r.cx.update(|_, cx| menus_view::resize_marks(cx));
    assert_eq!(marks, [true, false]);
    let drawn = labels(r.cx);
    assert!(
        ["Proportional", "Last Column"]
            .iter()
            .all(|l| drawn.iter().any(|d| d == l)),
        "{drawn:?}"
    );
    // Choosing "Last Column" writes the key.
    keys(r.cx, "down enter");
    assert_eq!(resize(r.cx), ColumnResize::LastColumn);
    assert_eq!(open_menu(r.cx), None);
    let marks = r.cx.update(|_, cx| menus_view::resize_marks(cx));
    assert_eq!(marks, [false, true]);
}

// View > Process View (E21-002).

/// The labels of the highlighted rows in the open submenus.
fn sub_highlighted(cx: &mut VisualTestContext) -> Vec<String> {
    entries(cx)
        .iter()
        .filter(|e| e.selected() == Some(true))
        .filter_map(|e| e.label().map(str::to_owned))
        .filter(|l| l != "Columns" && l != "Process View")
        .collect()
}

fn set_processes(
    cx: &mut VisualTestContext,
    f: impl FnOnce(&mut tl_model::settings::ProcessesSettings),
) {
    cx.update(|_, cx| TlSettings::update(cx, |d| f(&mut d.settings.processes)));
}

#[gpui_kit::test]
fn menu_action_reaches_table(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    // View > Process View; Tree View is highlighted, then five Downs past the two separators.
    keys(r.cx, "alt-v down right");
    assert_eq!(sub_highlighted(r.cx), ["Tree View"]);
    keys(r.cx, "down down down down down");
    assert_eq!(sub_highlighted(r.cx), ["Collapse all"]);
    // The hint comes from the Table context binding.
    assert_eq!(hint_for(&mut r, &CollapseAll).as_deref(), Some("Ctrl+L"));
    keys(r.cx, "enter");
    assert_eq!(r.collapses.get(), 1, "the table got CollapseAll");
    assert_eq!(open_menu(r.cx), None);
    assert!(table_focused(&mut r));
}

#[gpui_kit::test]
fn process_view_radio_follows_mode(cx: &mut TestAppContext) {
    let r = rig(cx);
    for (mode, marks) in [
        (TableMode::Tree, [true, false, false]),
        (TableMode::Grouped, [false, true, false]),
        (TableMode::Flat, [false, false, true]),
    ] {
        set_processes(r.cx, |p| p.mode = mode);
        assert_eq!(r.cx.update(|_, cx| menus_view::view_mode_marks(cx)), marks);
    }
    keys(r.cx, "alt-v down right");
    assert_eq!(
        sub_labels(r.cx),
        [
            "Tree View",
            "Task Manager Grouping",
            "List View",
            "Tree Connector Lines",
            "Highlight New and Exited Processes",
            "Collapse all",
        ]
    );
    // "List View" dispatches SetViewMode(Flat) once.
    keys(r.cx, "down down enter");
    assert_eq!(*r.modes.borrow(), [TableMode::Flat]);
}

#[gpui_kit::test]
fn collapse_all_disabled_off_processes(cx: &mut TestAppContext) {
    let r = rig(cx);
    set_tab(r.cx, Tab::Services, false);
    keys(r.cx, "alt-v down right");
    assert_eq!(sub_highlighted(r.cx), ["Tree View"]);
    // Up wraps to the last enabled item: Highlight, not Collapse all.
    keys(r.cx, "up");
    assert_eq!(
        sub_highlighted(r.cx),
        ["Highlight New and Exited Processes"]
    );
    // Items 1 to 5 are reachable.
    keys(r.cx, "down");
    assert_eq!(sub_highlighted(r.cx), ["Tree View"]);
    keys(r.cx, "down down down");
    assert_eq!(sub_highlighted(r.cx), ["Tree Connector Lines"]);
    keys(r.cx, "enter");
    assert_eq!(r.collapses.get(), 0);
}

#[gpui_kit::test]
fn process_view_history_disables_modes(cx: &mut TestAppContext) {
    let r = rig(cx);
    set_tab(r.cx, Tab::Processes, true);
    keys(r.cx, "alt-v down right");
    // The three modes are skipped: the first enabled item is the first check.
    assert_eq!(sub_highlighted(r.cx), ["Tree Connector Lines"]);
    keys(r.cx, "up");
    assert_eq!(sub_highlighted(r.cx), ["Collapse all"]);
    let tree = entries(r.cx)
        .into_iter()
        .find(|e| e.label() == Some("Tree View"))
        .expect("Tree View drawn")
        .bounds();
    r.cx.simulate_mouse_move(tree.center(), None, Modifiers::none());
    r.cx.executor()
        .advance_clock(tl_ui::components::tooltip::DELAY_CONTROL * 2);
    draw(r.cx);
    assert!(r.cx.debug_bounds("TlTooltip").is_some(), "tooltip shown");
    let reason =
        r.cx.update(|_, cx| tl_ui::i18n::tr(cx, "shell-menu-pv-history-flat"));
    assert_eq!(reason.as_ref(), "History shows a flat list");
    assert!(r.modes.borrow().is_empty());
}

#[gpui_kit::test]
fn process_view_checks_toggle_keys(cx: &mut TestAppContext) {
    let r = rig(cx);
    set_processes(r.cx, |p| {
        p.tree_guide_lines = false;
        p.highlight_new_exited = false;
    });
    let processes =
        |cx: &mut VisualTestContext| cx.update(|_, cx| TlSettings::settings(cx).processes.clone());
    keys(r.cx, "alt-v down right down down down enter");
    assert!(processes(r.cx).tree_guide_lines);
    assert!(!processes(r.cx).highlight_new_exited);
    keys(r.cx, "alt-v down right down down down down enter");
    assert!(processes(r.cx).highlight_new_exited);
    assert!(processes(r.cx).tree_guide_lines);
}
