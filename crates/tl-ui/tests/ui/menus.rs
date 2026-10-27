//! Menu bar, File menu and View menu tests (E21-A1, E21-001, E21-003, E21-044, E21-045, UI-SHL-030 to
//! UI-SHL-034, UI-SHL-039, ADR-013).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::AsKeystroke as _;
use gpui_kit::base::Root;
use gpui_kit::base::test_support::{ElementSnapshot, snapshots};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, ClipboardItem, Context, ElementId, Entity,
    FocusHandle, InteractiveElement, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, Role, Styled,
    TestAppContext, VisualTestContext, Window, WindowHandle, div, px, size,
};
use tl_model::actions::{ActionError, ActionEvent, ActionKind, ActionState, LaunchWhat};
use tl_model::client::EngineClient;
use tl_model::command::{Command, SetupPhase, SetupStep, Tab};
use tl_model::settings::{
    AppearanceSettings, ColumnResize, NetworkUnit, Settings, StartTab, ThemeSettings, UiState,
};
use tl_model::table::TableMode;
use tl_ui::actions::{
    MenuId, OpenAdditionalSetup, OpenCollectorHealth, OpenHandleSearch, OpenRunDialog,
    OpenSettings, OpenShortcuts, Quit, SetViewMode, ShowSystemInformation, bind_keys_global,
    register_global_handlers,
};
use tl_ui::app::{AppEnv, BootSettings, EngineGlobal};
use tl_ui::components::dialog::{DialogButton, TlDialogView};
use tl_ui::components::slider::TlSliderEvent;
use tl_ui::components::toast::ToastQueue;
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::settings_global::TlSettings;
use tl_ui::table::actions::{
    CollapseAll, EnableAllColumns, OpenColumnPicker, ResetColumns, ResizeColumnsToFit,
    TABLE_CONTEXT, bind_keys_table,
};
use tl_ui::table::columns::TableId;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::current;
use tl_ui::theme::user_store::{PersistTheme, ThemePersist};
use tl_ui::window::about::OpenAbout;
use tl_ui::window::confirm::ResetSettings;
use tl_ui::window::main::ShellState;
use tl_ui::window::menubar::{self, MenuBar, menu_bar_for};
use tl_ui::window::menus_file::SystemMonitor;
use tl_ui::window::menus_view::{CopyTheme, DeleteTheme, ImportTheme};
use tl_ui::window::titlebar::{TitleBarProbe, TitleBarView};
use tl_ui::window::{menus_debug, menus_file, menus_help, menus_settings, menus_view};

use super::common::{passwordless, publish_polkit};

const FILE_BUTTON: &str = "TlMenuBarButton-shell-menubar-file";
const VIEW_BUTTON: &str = "TlMenuBarButton-shell-menubar-view";
const SETTINGS_BUTTON: &str = "TlMenuBarButton-shell-menubar-settings";
const HELP_BUTTON: &str = "TlMenuBarButton-shell-menubar-help";
const DEBUG_BUTTON: &str = "TlMenuBarButton-shell-menubar-debug";

/// A main window stand-in: the title bar with the real menu bar, and a focusable table below.
struct Host {
    table: FocusHandle,
    bar: Entity<TitleBarView>,
    /// `CollapseAll` actions the table received.
    collapses: Rc<Cell<u32>>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let context_menu = tl_ui::components::menu::context_menu_layer(window, cx);
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
            .children(context_menu)
    }
}

struct Rig<'a> {
    cx: &'a mut VisualTestContext,
    table: FocusHandle,
    quits: Rc<Cell<u32>>,
    runs: Rc<Cell<u32>>,
    handle_searches: Rc<Cell<u32>>,
    /// The View > Columns table actions, by name and table.
    tables: Rc<RefCell<Vec<(&'static str, TableId)>>>,
    collapses: Rc<Cell<u32>>,
    modes: Rc<RefCell<Vec<TableMode>>>,
    engine: Arc<FakeEngine>,
    resets: Rc<Cell<u32>>,
    /// Calls of the settings persist callback.
    persists: Rc<Cell<u32>>,
    /// Requests to write themes.json.
    theme_persists: Rc<RefCell<Vec<PersistTheme>>>,
    /// The window's toast queue, in a rig made by `rig_with_toasts`.
    toasts: Option<Entity<ToastQueue>>,
}

fn rig(cx: &mut TestAppContext) -> Rig<'_> {
    rig_with(cx, false)
}

/// A rig whose window has a Root with a toast layer and queue, as the main window has.
fn rig_with_toasts(cx: &mut TestAppContext) -> Rig<'_> {
    rig_with(cx, true)
}

fn rig_with(cx: &mut TestAppContext, toasts: bool) -> Rig<'_> {
    let quits = Rc::new(Cell::new(0));
    let count = Rc::clone(&quits);
    let runs = Rc::new(Cell::new(0));
    let run_count = Rc::clone(&runs);
    let handle_searches = Rc::new(Cell::new(0));
    let handle_search_count = Rc::clone(&handle_searches);
    let tables = Rc::new(RefCell::new(Vec::new()));
    let (t1, t2, t3) = (tables.clone(), tables.clone(), tables.clone());
    let collapses = Rc::new(Cell::new(0));
    let modes = Rc::new(RefCell::new(Vec::new()));
    let mode_log = modes.clone();
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let resets = Rc::new(Cell::new(0));
    let reset_count = resets.clone();
    let persists = Rc::new(Cell::new(0));
    let persist_count = persists.clone();
    let theme_persists = Rc::new(RefCell::new(Vec::new()));
    let theme_persist_sink = theme_persists.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        if toasts {
            tl_ui::components::toast::init(cx);
        }
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        cx.set_global(ThemePersist(Box::new(move |request| {
            theme_persist_sink.borrow_mut().push(request);
        })));
        tl_ui::i18n::init_strings(cx, "en-US");
        cx.set_global(TitleBarProbe {
            client: Some(true),
            ..TitleBarProbe::default()
        });
        bind_keys_global(cx);
        menubar::init(cx);
        cx.on_action(move |_: &Quit, _| count.set(count.get() + 1));
        cx.on_action(move |_: &OpenRunDialog, _| run_count.set(run_count.get() + 1));
        cx.on_action(move |_: &OpenHandleSearch, _| {
            handle_search_count.set(handle_search_count.get() + 1)
        });
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
            persist: Some(Box::new(move |_, _| {
                persist_count.set(persist_count.get() + 1)
            })),
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        menus_view::init(cx);
        register_global_handlers(cx);
        cx.set_global(EngineGlobal(client));
        cx.on_action(move |_: &ResetSettings, _| reset_count.set(reset_count.get() + 1));
        cx.set_reduce_motion(true);
    });
    let host_collapses = collapses.clone();
    let (host, cx) = if toasts {
        let slot = Rc::new(RefCell::new(None));
        let out = slot.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            let host = cx.new(|cx| Host {
                table: cx.focus_handle(),
                bar: cx.new(|cx| TitleBarView::new(window, cx)),
                collapses: host_collapses,
            });
            *out.borrow_mut() = Some(host.clone());
            Root::new(host, window, cx)
        });
        let host = slot.borrow_mut().take().expect("host created");
        (host, cx)
    } else {
        cx.add_window_view(|window, cx| Host {
            table: cx.focus_handle(),
            bar: cx.new(|cx| TitleBarView::new(window, cx)),
            collapses: host_collapses,
        })
    };
    let toasts =
        toasts.then(|| cx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(40.)))));
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
        handle_searches,
        tables,
        collapses,
        modes,
        engine,
        resets,
        persists,
        theme_persists,
        toasts,
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
fn file_menu_p3_items(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    // File, View, Settings and Help, each followed by a separator one physical pixel wide, and
    // nothing for the absent Debug menu.
    let bar_w = bounds(r.cx, "TlMenuBar").size.width;
    let file = bounds(r.cx, FILE_BUTTON);
    let view = bounds(r.cx, VIEW_BUTTON);
    let settings = bounds(r.cx, SETTINGS_BUTTON);
    let help = bounds(r.cx, HELP_BUTTON);
    let scale = r.cx.update(|window, _| window.scale_factor());
    assert_eq!(
        bar_w,
        file.size.width + view.size.width + settings.size.width + help.size.width + px(4. / scale)
    );
    assert!(
        r.cx.debug_bounds(DEBUG_BUTTON).is_none(),
        "Debug has no button"
    );

    click_file(r.cx);
    let entries = entries(r.cx);
    assert_eq!(
        entries.len(),
        4,
        "Run..., Search File Handles..., separator, Exit: {entries:?}"
    );
    assert_eq!(entries[0].role(), Some(Role::MenuItem));
    assert_eq!(entries[0].label(), Some("Run..."));
    assert_eq!(entries[1].role(), Some(Role::MenuItem));
    assert_eq!(entries[1].label(), Some("Search File Handles..."));
    assert_eq!(entries[2].role(), None, "a separator");
    assert_eq!(entries[2].label(), None);
    assert!(entries[2].bounds().size.height < px(2.), "a one-pixel line");
    assert_eq!(entries[3].role(), Some(Role::MenuItem));
    assert_eq!(entries[3].label(), Some("Exit tasklens"));
    // The hints come from the live bindings.
    assert_eq!(hint_for(&mut r, &OpenRunDialog).as_deref(), Some("Ctrl+N"));
    assert_eq!(hint_for(&mut r, &Quit).as_deref(), Some("Ctrl+Q"));
    assert_eq!(hint_for(&mut r, &OpenHandleSearch), None, "no shortcut");
}

#[gpui_kit::test]
fn file_menu_handle_search_dispatch(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    keys(r.cx, "f10 down");
    assert_eq!(
        entries(r.cx)[1].selected(),
        Some(true),
        "Search File Handles... highlighted"
    );
    keys(r.cx, "enter");
    assert_eq!(r.handle_searches.get(), 1);
    assert_eq!(r.runs.get(), 0);
    assert_eq!(r.quits.get(), 0);
    assert_eq!(open_menu(r.cx), None);
    assert!(popup_bounds(r.cx).is_none());
    assert!(table_focused(&mut r));

    // A click on the item does the same.
    click_file(r.cx);
    let item = entries(r.cx)[1].bounds().center();
    click(r.cx, item);
    assert_eq!(r.handle_searches.get(), 2);
    assert_eq!(open_menu(r.cx), None);
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
    keys(r.cx, "f10 down down");
    assert_eq!(entries(r.cx)[3].label(), Some("Exit tasklens"));
    assert_eq!(entries(r.cx)[3].selected(), Some(true));
    keys(r.cx, "right");
    assert_eq!(open_menu(r.cx), Some(MenuId::View));
    assert_eq!(entries(r.cx)[0].selected(), Some(true));
    // In View, Left at the top level opens File.
    keys(r.cx, "left");
    assert_eq!(open_menu(r.cx), Some(MenuId::File));
    // In File, Left wraps to Help, the last present menu.
    keys(r.cx, "left");
    assert_eq!(open_menu(r.cx), Some(MenuId::Help));
    assert!(popup_bounds(r.cx).is_some());
}

#[gpui_kit::test]
fn exit_dispatches_quit(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    keys(r.cx, "f10");
    keys(r.cx, "down down");
    assert_eq!(
        entries(r.cx)[3].selected(),
        Some(true),
        "Down skips the separator"
    );
    keys(r.cx, "enter");
    assert_eq!(r.quits.get(), 1);
    assert_eq!(r.runs.get(), 0);
    assert_eq!(r.handle_searches.get(), 0);
    assert_eq!(open_menu(r.cx), None);
    assert!(popup_bounds(r.cx).is_none());
    assert!(table_focused(&mut r));

    // A click on the item does the same.
    click_file(r.cx);
    let item = entries(r.cx)[3].bounds().center();
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

/// The View root items in order.
const VIEW_ROOT: [&str; 6] = [
    "Columns",
    "Process View",
    "Window",
    "Appearance",
    "Theme",
    "Graphs",
];

/// The labels of the open submenus: every drawn label except the View root items.
fn sub_labels(cx: &mut VisualTestContext) -> Vec<String> {
    labels(cx)
        .into_iter()
        .filter(|l| !VIEW_ROOT.contains(&l.as_str()))
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
    assert_eq!(
        entries.len(),
        6,
        "Columns, Process View, Window, Appearance, Theme, Graphs: {entries:?}"
    );
    assert_eq!(entries[0].label(), Some("Columns"));
    assert_eq!(entries[0].selected(), Some(true), "Columns highlighted");
    assert_eq!(entries[1].label(), Some("Process View"));
    assert_eq!(entries[2].label(), Some("Window"));
    assert_eq!(entries[3].label(), Some("Appearance"));
    assert_eq!(entries[4].label(), Some("Theme"));
    assert_eq!(entries[5].label(), Some("Graphs"));
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
    assert_eq!(labels(r.cx), VIEW_ROOT);
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
    assert_eq!(drawn.len(), 6);
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
        .filter(|l| !VIEW_ROOT.contains(&l.as_str()))
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

// Settings menu (E21-004).

/// Every drawn entry of the open menus as "item:<label>", "header:<label>" or "separator", top
/// to bottom.
fn shapes(cx: &mut VisualTestContext) -> Vec<String> {
    entries(cx)
        .iter()
        .map(|e| match (e.role(), e.label()) {
            (Some(Role::MenuItem), Some(l)) => format!("item:{l}"),
            (_, Some(l)) => format!("header:{l}"),
            (_, None) => "separator".to_owned(),
        })
        .collect()
}

const SETTINGS_ROOT: [&str; 29] = [
    "header:Monitoring",
    "item:Refresh",
    "item:Network transfer unit",
    "item:Auto-scale network transfer rates",
    "item:Record history",
    "item:History storage",
    "item:Save history to disk",
    "item:Rendering GPU",
    "header:Motion & interaction",
    "item:Animations",
    "item:Smooth Graphs",
    "item:Smooth Scroll",
    "item:Tab Switch 250 ms",
    "item:Close with Esc",
    "header:Window & startup",
    "item:Close to tray",
    "item:Minimize to tray",
    "item:Always show tray icon",
    "item:Start at login",
    "item:Start minimized",
    "item:Default tab",
    "item:Unlock privileged actions at launch",
    "item:Skip password prompts (experimental)",
    "item:Auto-fetch Public IP on launch",
    "item:Open rescue window on critical memory pressure",
    "header:Language",
    "item:Language: System default",
    "separator",
    "item:All settings…",
];

/// The labels of an open Settings submenu: every drawn item label not in the root.
fn settings_sub(cx: &mut VisualTestContext) -> Vec<String> {
    shapes(cx)
        .into_iter()
        .filter(|s| !SETTINGS_ROOT.contains(&s.as_str()) && s != "item:Reset Settings to Defaults")
        .filter_map(|s| s.strip_prefix("item:").map(str::to_owned))
        .collect()
}

fn settings(cx: &mut VisualTestContext) -> Settings {
    cx.update(|_, cx| TlSettings::settings(cx).clone())
}

#[gpui_kit::test]
fn settings_menu_p1_rows(cx: &mut TestAppContext) {
    let r = rig(cx);
    keys(r.cx, "alt-s");
    assert_eq!(open_menu(r.cx), Some(MenuId::Settings));
    let mut expected: Vec<String> = SETTINGS_ROOT.iter().map(|s| s.to_string()).collect();
    expected.push("item:Reset Settings to Defaults".to_owned());
    assert_eq!(shapes(r.cx), expected);
    // The first enabled item is highlighted; headers are not selectable.
    let first = entries(r.cx)
        .into_iter()
        .find(|e| e.selected() == Some(true))
        .and_then(|e| e.label().map(str::to_owned));
    assert_eq!(first.as_deref(), Some("Refresh"));
}

#[gpui_kit::test]
fn settings_menu_refresh_labels(cx: &mut TestAppContext) {
    let r = rig(cx);
    keys(r.cx, "alt-s right");
    assert_eq!(
        settings_sub(r.cx),
        ["0.25s", "0.5s", "1.0s", "2.0s", "5.0s", "10.0s"]
    );
    // interval_ms = 1000: the mark is on "1.0s". A value off the list marks nothing.
    let marks = |cx: &mut VisualTestContext| cx.update(|_, cx| menus_settings::refresh_marks(cx));
    assert_eq!(marks(r.cx), [false, false, true, false, false, false]);
    r.cx.update(|_, cx| TlSettings::update(cx, |d| d.settings.processes.interval_ms = 1500));
    assert_eq!(marks(r.cx), [false; 6]);
}

#[gpui_kit::test]
fn settings_menu_refresh_radio(cx: &mut TestAppContext) {
    let r = rig(cx);
    keys(r.cx, "alt-s right down down down enter");
    assert_eq!(settings(r.cx).processes.interval_ms, 2000);
    assert_eq!(
        r.engine.settings().last().map(|s| s.interval_ms),
        Some(2000)
    );
}

#[gpui_kit::test]
fn settings_menu_network_and_checks(cx: &mut TestAppContext) {
    let r = rig(cx);
    assert_eq!(settings(r.cx).units.network, NetworkUnit::Bits);
    // Network transfer unit > Bytes per second.
    keys(r.cx, "alt-s down right");
    assert_eq!(settings_sub(r.cx), ["Bits per second", "Bytes per second"]);
    keys(r.cx, "down enter");
    assert_eq!(settings(r.cx).units.network, NetworkUnit::Bytes);
    // Animations: Network, Auto-scale, Record history, History storage, Rendering GPU,
    // Animations ("Save history to disk" is disabled and skipped).
    assert!(settings(r.cx).motion.animations);
    keys(r.cx, "alt-s down down down down down down enter");
    assert!(!settings(r.cx).motion.animations);
}

#[gpui_kit::test]
fn default_tab_lists_built_tabs(cx: &mut TestAppContext) {
    let r = rig(cx);
    r.cx.update(|_, cx| {
        cx.global_mut::<ShellState>().built_tabs =
            [Tab::Processes, Tab::Performance].into_iter().collect();
    });
    // Default tab is the 16th enabled item ("Save history to disk" and "Start minimized" are
    // disabled).
    keys(
        r.cx,
        "alt-s down down down down down down down down down down down down down down down right",
    );
    assert_eq!(
        settings_sub(r.cx),
        ["Processes", "Performance", "Last used"]
    );
    keys(r.cx, "up enter");
    assert_eq!(settings(r.cx).general.start_tab, StartTab::LastUsed);
}

#[gpui_kit::test]
fn language_submenu_p1(cx: &mut TestAppContext) {
    let r = rig(cx);
    // Up from the top wraps to Reset, then All settings, then Language.
    keys(r.cx, "alt-s up up up right");
    assert_eq!(
        settings_sub(r.cx),
        ["System default", "English (United States)"]
    );
    let current = settings(r.cx).general.language;
    let marks = r.cx.update(|_, cx| menus_settings::language_marks(cx));
    assert_eq!(
        marks,
        [current == "system", current == "en-US"],
        "the mark is on {current}"
    );
    assert_eq!(marks.iter().filter(|m| **m).count(), 1);
}

/// The Language row names the language in use (tasklens-bj6.58 d), so the chevron is not bare.
#[gpui_kit::test]
fn language_row_shows_current_value(cx: &mut TestAppContext) {
    let r = rig(cx);
    assert_eq!(settings(r.cx).general.language, "system");
    keys(r.cx, "alt-s");
    assert!(
        shapes(r.cx).contains(&"item:Language: System default".to_owned()),
        "{:?}",
        shapes(r.cx)
    );
}

#[gpui_kit::test]
fn reset_item_dispatches(cx: &mut TestAppContext) {
    let r = rig(cx);
    keys(r.cx, "alt-s up enter");
    assert_eq!(r.resets.get(), 1);
    assert_eq!(open_menu(r.cx), None);
}

// Settings menu P2 rows (E21-010).

/// The labels between `header` and the next header or separator.
fn group(cx: &mut VisualTestContext, header: &str) -> Vec<String> {
    shapes(cx)
        .into_iter()
        .skip_while(|s| *s != format!("header:{header}"))
        .skip(1)
        .take_while(|s| s.starts_with("item:"))
        .map(|s| s["item:".len()..].to_owned())
        .collect()
}

/// Clicks the drawn entry labelled `label`.
fn click_label(cx: &mut VisualTestContext, label: &str) {
    let at = entries(cx)
        .into_iter()
        .find(|e| e.label() == Some(label))
        .unwrap_or_else(|| panic!("{label} drawn"))
        .bounds()
        .center();
    click(cx, at);
}

#[gpui_kit::test]
fn settings_menu_p2_order(cx: &mut TestAppContext) {
    let r = rig(cx);
    keys(r.cx, "alt-s");
    assert_eq!(
        group(r.cx, "Motion & interaction"),
        [
            "Animations",
            "Smooth Graphs",
            "Smooth Scroll",
            "Tab Switch 250 ms",
            "Close with Esc"
        ]
    );
    assert_eq!(
        group(r.cx, "Window & startup"),
        [
            "Close to tray",
            "Minimize to tray",
            "Always show tray icon",
            "Start at login",
            "Start minimized",
            "Default tab",
            "Unlock privileged actions at launch",
            "Skip password prompts (experimental)",
            "Auto-fetch Public IP on launch",
            "Open rescue window on critical memory pressure"
        ]
    );
    let tail: Vec<String> = shapes(r.cx).into_iter().rev().take(3).collect();
    assert_eq!(
        tail,
        [
            "item:Reset Settings to Defaults",
            "item:All settings…",
            "separator"
        ]
    );
}

#[gpui_kit::test]
fn settings_menu_tab_switch_slider(cx: &mut TestAppContext) {
    let r = rig(cx);
    assert_eq!(settings(r.cx).motion.tab_switch_ms, 250);
    keys(r.cx, "alt-s");
    assert!(shapes(r.cx).contains(&"item:Tab Switch 250 ms".to_owned()));
    let state = slider_states(r.cx)[4].clone();
    let before = r.persists.get();
    // A drag moves the slider, then the slider emits.
    r.cx.update(|_, cx| tl_ui::components::slider::set_from_elsewhere(&state, 300., cx));
    emit(r.cx, &state, TlSliderEvent::Preview(300.));
    assert_eq!(
        settings(r.cx).motion.tab_switch_ms,
        300,
        "applied in memory"
    );
    assert_eq!(r.persists.get(), before, "a Change never persists");
    emit(r.cx, &state, TlSliderEvent::Commit(300.));
    assert_eq!(settings(r.cx).motion.tab_switch_ms, 300);
    assert_eq!(r.persists.get(), before + 1, "the release persists once");
    assert!(shapes(r.cx).contains(&"item:Tab Switch 300 ms".to_owned()));
    assert_eq!(open_menu(r.cx), Some(MenuId::Settings));

    // Refresh is highlighted; nine Downs reach Tab Switch, Right steps by 10.
    keys(r.cx, "down down down down down down down down down right");
    assert_eq!(settings(r.cx).motion.tab_switch_ms, 310);
    assert_eq!(r.persists.get(), before + 2);
}

fn set_notice_accepted(cx: &mut VisualTestContext, accepted: bool) {
    cx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            d.settings.network.public_ip_notice_accepted = accepted
        })
    });
}

#[gpui_kit::test]
fn settings_menu_public_ip_toggle(cx: &mut TestAppContext) {
    let r = rig(cx);
    tall_window(r.cx);
    set_notice_accepted(r.cx, true);
    assert!(!settings(r.cx).network.auto_fetch_public_ip);
    keys(r.cx, "alt-s");
    click_label(r.cx, "Auto-fetch Public IP on launch");
    assert!(settings(r.cx).network.auto_fetch_public_ip);
    assert_eq!(open_menu(r.cx), None);

    keys(r.cx, "alt-s");
    click_label(r.cx, "Auto-fetch Public IP on launch");
    assert!(!settings(r.cx).network.auto_fetch_public_ip, "off again");
}

#[gpui_kit::test]
fn settings_menu_public_ip_asks_first(cx: &mut TestAppContext) {
    let r = rig(cx);
    tall_window(r.cx);
    set_notice_accepted(r.cx, false);
    let windows = r.cx.update(|_, cx| cx.windows().len());
    keys(r.cx, "alt-s");
    click_label(r.cx, "Auto-fetch Public IP on launch");
    assert!(
        !settings(r.cx).network.auto_fetch_public_ip,
        "not before the notice is accepted"
    );
    assert_eq!(
        r.cx.update(|_, cx| cx.windows().len()),
        windows + 1,
        "the public IP notice opened"
    );
}

#[gpui_kit::test]
fn settings_menu_all_settings_dispatch(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    let opened = Rc::new(Cell::new(0));
    let count = opened.clone();
    r.cx.update(|_, cx| {
        cx.on_action(move |_: &OpenSettings, _| count.set(count.get() + 1));
    });
    assert_eq!(hint_for(&mut r, &OpenSettings).as_deref(), Some("Ctrl+,"));
    keys(r.cx, "alt-s up up");
    let row = entries(r.cx)
        .into_iter()
        .find(|e| e.selected() == Some(true))
        .and_then(|e| e.label().map(str::to_owned));
    assert_eq!(row.as_deref(), Some("All settings…"));
    keys(r.cx, "enter");
    assert_eq!(opened.get(), 1, "OpenSettings dispatched");
    assert_eq!(open_menu(r.cx), None);
}

// View > Window (E21-008).

/// Installs a hub on the rig's fake engine and publishes a windows snapshot with `validity`
/// (the KWin bridge state the caption pin reads).
fn set_kwin(r: &mut Rig<'_>, validity: tl_model::validity::Validity) {
    let client: Arc<dyn EngineClient> = r.engine.clone();
    r.cx.update(|_, cx| {
        if !cx.has_global::<tl_ui::hub::GlobalHub>() {
            let hub = cx.new(|cx| tl_ui::hub::SnapshotHub::new(client, cx));
            cx.set_global(tl_ui::hub::GlobalHub(hub));
        }
    });
    r.engine.publish_with(|s| {
        let w = Arc::make_mut(&mut s.windows);
        w.header.validity = validity;
        w.header.version += 1;
    });
    r.cx.run_until_parked();
}

fn set_on_top(cx: &mut VisualTestContext, on: bool) {
    cx.update(|_, cx| {
        TlSettings::set(
            cx,
            tl_ui::settings_global::SettingKey::WindowAlwaysOnTop,
            tl_ui::settings_global::SettingValue::Bool(on),
        )
    });
}

#[gpui_kit::test]
fn window_menu_always_on_top_dispatch(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    set_kwin(&mut r, tl_model::validity::Validity::Valid);
    let toggles = Rc::new(RefCell::new(Vec::new()));
    let sink = toggles.clone();
    r.cx.update(|_, cx| {
        cx.on_action(
            move |a: &tl_ui::components::window_frame::ToggleAlwaysOnTop, _| {
                sink.borrow_mut().push(a.clone())
            },
        );
    });
    assert_eq!(
        r.cx.update(|_, cx| menus_view::always_on_top_marks(cx)),
        (true, false),
        "enabled and unchecked"
    );
    keys(r.cx, "alt-v");
    assert!(
        labels(r.cx).contains(&"Window".to_owned()),
        "{:?}",
        labels(r.cx)
    );
    keys(r.cx, "down down right");
    assert_eq!(
        sub_labels(r.cx),
        ["Always On Top", "Performance mini window"]
    );
    keys(r.cx, "enter");
    let got = toggles.borrow().clone();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].window, tl_ui::window::registry::WindowKey::Main);
}

#[gpui_kit::test]
fn window_menu_disabled_without_keep_above(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    set_kwin(&mut r, tl_model::validity::Validity::Unsupported);
    assert!(
        !r.cx.update(|_, cx| menus_view::always_on_top_marks(cx)).0,
        "no keep-above mechanism"
    );
    keys(r.cx, "alt-v down down right");
    let item = entries(r.cx)
        .into_iter()
        .find(|e| e.label() == Some("Always On Top"))
        .expect("Always On Top drawn");
    assert_ne!(
        item.selected(),
        Some(true),
        "a disabled item is never highlighted"
    );
    r.cx.simulate_mouse_move(item.bounds().center(), None, Modifiers::none());
    r.cx.executor()
        .advance_clock(tl_ui::components::tooltip::DELAY_CONTROL * 2);
    draw(r.cx);
    assert!(r.cx.debug_bounds("TlTooltip").is_some(), "tooltip shown");
    let reason =
        r.cx.update(|_, cx| tl_ui::i18n::tr(cx, "fnd-pin-unsupported"));
    assert_eq!(reason.as_ref(), "Not supported by this desktop");
}

#[gpui_kit::test]
fn window_menu_check_follows_key(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    set_kwin(&mut r, tl_model::validity::Validity::Valid);
    set_on_top(r.cx, true);
    assert_eq!(
        r.cx.update(|_, cx| menus_view::always_on_top_marks(cx)),
        (true, true)
    );
    set_on_top(r.cx, false);
    assert_eq!(
        r.cx.update(|_, cx| menus_view::always_on_top_marks(cx)),
        (true, false)
    );
}

// View > Appearance (E21-009).

/// Opens View > Appearance from the keyboard; its first row, UI Scale, is highlighted.
fn open_appearance(cx: &mut VisualTestContext) {
    keys(cx, "alt-v down down down right");
    assert_eq!(sub_highlighted(cx), ["UI Scale 100%"]);
}

fn appearance(cx: &mut VisualTestContext) -> AppearanceSettings {
    cx.update(|_, cx| TlSettings::settings(cx).appearance.clone())
}

fn toast_texts(r: &mut Rig<'_>) -> Vec<String> {
    let queue = r.toasts.clone().expect("rig with toasts");
    r.cx.update(|_, cx| {
        queue
            .read(cx)
            .visible_texts()
            .iter()
            .map(|t| t.to_string())
            .collect()
    })
}

/// The menu bar's slider states, in `MenuSliders` field order.
fn slider_states(cx: &mut VisualTestContext) -> Vec<Entity<gpui_kit::base::slider::SliderState>> {
    let bar = bar(cx);
    cx.update(|_, cx| {
        let s = bar.read(cx).sliders();
        vec![
            s.ui_scale.clone(),
            s.row_height.clone(),
            s.heat_intensity.clone(),
            s.shadow_intensity.clone(),
            s.tab_switch.clone(),
        ]
    })
}

fn emit(
    cx: &mut VisualTestContext,
    state: &Entity<gpui_kit::base::slider::SliderState>,
    event: TlSliderEvent,
) {
    cx.update(|_, cx| state.update(cx, |_, cx| cx.emit(event)));
    draw(cx);
}

#[gpui_kit::test]
fn appearance_slider_persist_on_release(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    open_appearance(r.cx);
    let scale = slider_states(r.cx)[0].clone();
    let before = r.persists.get();
    for v in [105., 110., 115.] {
        emit(r.cx, &scale, TlSliderEvent::Preview(v));
        assert_eq!(appearance(r.cx).ui_scale_pct, v as u16, "applied in memory");
    }
    assert_eq!(r.persists.get(), before, "a Change never persists");
    assert!(toast_texts(&mut r).is_empty(), "no toast while dragging");
    emit(r.cx, &scale, TlSliderEvent::Commit(115.));
    assert_eq!(r.persists.get(), before + 1, "the release persists once");
    assert_eq!(appearance(r.cx).ui_scale_pct, 115);
    assert_eq!(toast_texts(&mut r), ["UI scale changed to 115%"]);
    assert_eq!(open_menu(r.cx), Some(MenuId::View), "the popup stays open");
}

#[gpui_kit::test]
fn appearance_slider_keys(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    open_appearance(r.cx);
    keys(r.cx, "down");
    assert_eq!(sub_highlighted(r.cx), ["Row Height 100%"]);
    let mut persists = r.persists.get();
    for (key, pct) in [("right", 105), ("left", 100), ("home", 80), ("end", 150)] {
        keys(r.cx, key);
        assert_eq!(appearance(r.cx).row_height_pct, pct, "{key}");
        persists += 1;
        assert_eq!(r.persists.get(), persists, "{key} persists once");
    }
    assert_eq!(
        sub_highlighted(r.cx),
        ["Row Height 150%"],
        "the value text follows"
    );
    assert!(
        toast_texts(&mut r).is_empty(),
        "only UI Scale shows a toast"
    );

    keys(r.cx, "escape escape");
    r.cx.update(|_, cx| TlSettings::update(cx, |d| d.settings.appearance.ui_scale_pct = 200));
    let persists = r.persists.get();
    open_appearance_at(r.cx, "UI Scale 200%");
    keys(r.cx, "end");
    assert_eq!(appearance(r.cx).ui_scale_pct, 200, "End at the maximum");
    assert_eq!(r.persists.get(), persists, "no change, no persist");
    assert!(toast_texts(&mut r).is_empty(), "no change, no toast");
    keys(r.cx, "left");
    assert_eq!(appearance(r.cx).ui_scale_pct, 195);
    assert_eq!(r.persists.get(), persists + 1);
    assert_eq!(toast_texts(&mut r), ["UI scale changed to 195%"]);
    assert_eq!(open_menu(r.cx), Some(MenuId::View));
}

/// Opens View > Appearance and checks the highlighted first row reads `first`.
fn open_appearance_at(cx: &mut VisualTestContext, first: &str) {
    keys(cx, "alt-v down down down right");
    assert_eq!(sub_highlighted(cx), [first]);
}

#[gpui_kit::test]
fn appearance_popup_stays_open(cx: &mut TestAppContext) {
    let r = rig(cx);
    let start = appearance(r.cx);
    open_appearance(r.cx);
    keys(r.cx, "down down");
    assert_eq!(sub_highlighted(r.cx), ["Rounded Corners"]);
    keys(r.cx, "enter");
    assert_eq!(appearance(r.cx).rounded_corners, !start.rounded_corners);
    assert_eq!(open_menu(r.cx), Some(MenuId::View), "open after the check");
    assert!(sub_labels(r.cx).contains(&"Rounded Corners".to_owned()));

    let heat = entries(r.cx)
        .into_iter()
        .find(|e| e.label() == Some("Value Color Coding"))
        .expect("Value Color Coding drawn")
        .bounds()
        .center();
    click(r.cx, heat);
    assert_eq!(appearance(r.cx).heat_coloring, !start.heat_coloring);
    assert_eq!(open_menu(r.cx), Some(MenuId::View), "open after a click");

    let shadow = slider_states(r.cx)[3].clone();
    emit(r.cx, &shadow, TlSliderEvent::Preview(30.));
    emit(r.cx, &shadow, TlSliderEvent::Commit(30.));
    assert_eq!(appearance(r.cx).shadow_intensity_pct, 30);
    assert_eq!(open_menu(r.cx), Some(MenuId::View), "open after a release");
}

#[gpui_kit::test]
fn slider_entities_reused(cx: &mut TestAppContext) {
    let r = rig(cx);
    let first = slider_states(r.cx);
    let drawn = |cx: &mut VisualTestContext| {
        let bar = bar(cx);
        cx.update(|window, cx| {
            let popup = bar.read(cx).popup().expect("menu open").clone();
            let root = popup.read(cx).entries_for_test(window);
            let sub = root[3].submenu.clone().expect("Appearance submenu");
            sub.read(cx)
                .entries_for_test(window)
                .into_iter()
                .filter_map(|e| e.slider)
                .collect::<Vec<_>>()
        })
    };
    open_appearance(r.cx);
    assert_eq!(drawn(r.cx), first[..4]);
    keys(r.cx, "escape escape");
    assert_eq!(open_menu(r.cx), None);
    open_appearance(r.cx);
    assert_eq!(slider_states(r.cx), first, "no new entity");
    assert_eq!(drawn(r.cx), first[..4]);
}

#[gpui_kit::test]
fn slider_row_layout(cx: &mut TestAppContext) {
    let r = rig(cx);
    open_appearance(r.cx);
    let row = entries(r.cx)
        .into_iter()
        .find(|e| e.label() == Some("UI Scale 100%"))
        .expect("UI Scale row");
    assert_eq!(row.bounds().size.height, px(28.), "row height at 100%");
    let label = bounds(r.cx, "TlSliderLabel");
    // The label column asks for 140. "Value Color Coding Intensity" holds the menu at its 360
    // maximum, where the fixed parts (border, padding, mark, gaps, track, value) leave 138.
    assert!(label.size.width >= px(138.), "label column {label:?}");
    let value = bounds(r.cx, "TlSliderValue");
    assert_eq!(value.size.width, px(40.), "value column");
    let menu = widest_menu(r.cx);
    assert!(
        menu <= px(360.),
        "the menu stays within its 360 maximum: {menu:?}"
    );
}

/// The widest drawn menu.
fn widest_menu(cx: &mut VisualTestContext) -> Pixels {
    cx.update(|window, _| snapshots(window))
        .into_iter()
        .filter(|s| s.role() == Some(Role::Menu))
        .map(|s| s.bounds().size.width)
        .fold(px(0.), |a, b| a.max(b))
}

// View > Graphs (E21-003).

/// Installs a hub on the rig's fake engine and publishes a device catalog with `gpus` present
/// GPUs.
fn set_gpus(r: &mut Rig<'_>, gpus: usize) {
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::DeviceIdx;
    let client: Arc<dyn EngineClient> = r.engine.clone();
    r.cx.update(|_, cx| {
        if !cx.has_global::<tl_ui::hub::GlobalHub>() {
            let hub = cx.new(|cx| tl_ui::hub::SnapshotHub::new(client, cx));
            cx.set_global(tl_ui::hub::GlobalHub(hub));
        }
    });
    let entries: Vec<DeviceEntry> = (0..gpus)
        .map(|i| DeviceEntry {
            idx: DeviceIdx(i as _),
            identity: DeviceIdentity::Gpu {
                uuid: [i as u8; 16],
                pci_bus_id: Arc::from(format!("0000:0{i}:00.0")),
            },
            generation: 0,
            display_name: Arc::from("GPU"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
            hot_c: None,
        })
        .collect();
    r.engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: s.devices.version + 1,
            entries: Arc::from(entries),
        });
    });
    r.cx.run_until_parked();
}

/// Opens View > Graphs from the keyboard (the last root item).
fn open_graphs(cx: &mut VisualTestContext) {
    keys(cx, "alt-v end right");
    assert_eq!(open_menu(cx), Some(MenuId::View));
}

/// The built entries of the open View > Graphs submenu.
fn graphs_entries(cx: &mut VisualTestContext) -> Vec<tl_ui::components::menu::MenuEntryInfo> {
    let bar = bar(cx);
    cx.update(|window, cx| {
        let popup = bar.read(cx).popup().expect("menu open").clone();
        let root = popup.read(cx).entries_for_test(window);
        assert_eq!(root[5].label.as_deref(), Some("Graphs"));
        let sub = root[5].submenu.clone().expect("Graphs submenu");
        sub.read(cx).entries_for_test(window)
    })
}

const GRAPH_LABELS: [&str; 7] = [
    "CPU Graph",
    "Memory Graph",
    "Disk Graph",
    "Network Graph",
    "GPU Graph",
    "VRAM Graph",
    "GPU Temperature Graph",
];

#[gpui_kit::test]
fn graphs_menu_p1_items(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    set_gpus(&mut r, 1);
    open_graphs(r.cx);
    let mut expected: Vec<String> = GRAPH_LABELS.iter().map(|l| format!("item:{l}")).collect();
    expected.push("separator".to_owned());
    expected.push("item:Keep detached graphs open when closing main window".to_owned());
    expected.push("item:Reset Minigraph Layout".to_owned());
    let drawn: Vec<String> = shapes(r.cx)
        .into_iter()
        .filter(|s| {
            !s.strip_prefix("item:")
                .is_some_and(|l| VIEW_ROOT.contains(&l))
        })
        .collect();
    assert_eq!(drawn, expected);
    assert_eq!(sub_highlighted(r.cx), ["CPU Graph"]);
}

#[gpui_kit::test]
fn graphs_checks_follow_keys(cx: &mut TestAppContext) {
    use tl_ui::components::menu::MenuEntryKind;
    let mut r = rig(cx);
    set_gpus(&mut r, 1);
    let defaults = [true, true, true, true, true, true, false];
    assert_eq!(r.cx.update(|_, cx| menus_view::graphs_marks(cx)), defaults);
    open_graphs(r.cx);
    let checks: Vec<(String, bool)> = graphs_entries(r.cx)
        .into_iter()
        .filter(|e| e.kind == MenuEntryKind::Item)
        .take(7)
        .map(|e| (e.label.expect("label").to_string(), e.checked))
        .collect();
    let expected: Vec<(String, bool)> = GRAPH_LABELS
        .iter()
        .map(|l| l.to_string())
        .zip(defaults)
        .collect();
    assert_eq!(checks, expected);

    // CPU is highlighted; two Downs reach Disk Graph.
    keys(r.cx, "down down enter");
    let graphs =
        |cx: &mut VisualTestContext| cx.update(|_, cx| TlSettings::settings(cx).graphs.clone());
    assert!(!graphs(r.cx).minigraph_disk);
    assert_eq!(open_menu(r.cx), None);
    assert_eq!(
        r.cx.update(|_, cx| menus_view::graphs_marks(cx)),
        [true, true, false, true, true, true, false]
    );
    // GPU Temperature Graph turns on.
    open_graphs(r.cx);
    click_label(r.cx, "GPU Temperature Graph");
    assert!(graphs(r.cx).minigraph_gpu_temperature);
    assert!(!graphs(r.cx).minigraph_disk);
}

#[gpui_kit::test]
fn gpu_items_disabled_without_gpu(cx: &mut TestAppContext) {
    use tl_ui::components::menu::MenuEntryKind;
    let mut r = rig(cx);
    set_gpus(&mut r, 0);
    assert!(!r.cx.update(|_, cx| menus_view::has_gpu(cx)));
    open_graphs(r.cx);
    let built = graphs_entries(r.cx);
    let kinds: Vec<(Option<String>, MenuEntryKind, Option<String>)> = built
        .iter()
        .take(7)
        .map(|e| {
            (
                e.label.as_ref().map(|l| l.to_string()),
                e.kind,
                e.tooltip.as_ref().map(|t| t.to_string()),
            )
        })
        .collect();
    for (i, (label, kind, tooltip)) in kinds.iter().enumerate() {
        assert_eq!(label.as_deref(), Some(GRAPH_LABELS[i]));
        if i >= 4 {
            assert_eq!(*kind, MenuEntryKind::Disabled, "{label:?}");
            assert_eq!(tooltip.as_deref(), Some("No GPU found"), "{label:?}");
        } else {
            assert_eq!(*kind, MenuEntryKind::Item, "{label:?}");
        }
    }
    // Up from CPU wraps to Reset, then Keep detached, then the last enabled check.
    keys(r.cx, "up up up");
    assert_eq!(sub_highlighted(r.cx), ["Network Graph"]);

    // The tooltip opens on hover and gives the reason.
    let row = entries(r.cx)
        .into_iter()
        .find(|e| e.label() == Some("VRAM Graph"))
        .expect("VRAM Graph drawn")
        .bounds();
    r.cx.simulate_mouse_move(row.center(), None, Modifiers::none());
    r.cx.executor()
        .advance_clock(tl_ui::components::tooltip::DELAY_CONTROL * 2);
    draw(r.cx);
    assert!(r.cx.debug_bounds("TlTooltip").is_some(), "tooltip shown");

    // One GPU: all seven checks are enabled.
    keys(r.cx, "escape escape");
    set_gpus(&mut r, 1);
    assert!(r.cx.update(|_, cx| menus_view::has_gpu(cx)));
    open_graphs(r.cx);
    let enabled = graphs_entries(r.cx)
        .iter()
        .take(7)
        .filter(|e| e.kind == MenuEntryKind::Item)
        .count();
    assert_eq!(enabled, 7);
}

#[gpui_kit::test]
fn reset_minigraph_dispatch(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    set_gpus(&mut r, 1);
    let resets = Rc::new(Cell::new(0));
    let count = resets.clone();
    r.cx.update(|_, cx| {
        cx.on_action(
            move |_: &tl_ui::pages::minigraph::actions::ResetMinigraphLayout, _| {
                count.set(count.get() + 1)
            },
        );
    });
    open_graphs(r.cx);
    // Up from CPU wraps to Reset.
    keys(r.cx, "up");
    assert_eq!(sub_highlighted(r.cx), ["Reset Minigraph Layout"]);
    keys(r.cx, "enter");
    assert_eq!(resets.get(), 1, "ResetMinigraphLayout dispatched once");
    assert_eq!(open_menu(r.cx), None);
}

// Settings menu: Start at login and Start minimized (E21-011).

/// The calls of the fake autostart writer, `(enabled, minimized)`.
type AutostartCalls = Arc<std::sync::Mutex<Vec<(bool, bool)>>>;

/// Installs an `AppEnv` whose `set_autostart` records each call and fails when `fail` is set.
fn fake_autostart(cx: &mut VisualTestContext, fail: bool) -> AutostartCalls {
    let calls: AutostartCalls = Arc::default();
    let log = calls.clone();
    cx.update(|_, cx| {
        cx.set_global(tl_ui::app::AppEnv {
            set_autostart: Arc::new(move |enabled, minimized| {
                log.lock().expect("calls").push((enabled, minimized));
                if fail {
                    Err("read-only file system".to_owned())
                } else {
                    Ok(())
                }
            }),
            ..tl_ui::app::AppEnv::test()
        })
    });
    calls
}

fn set_start_at_login(cx: &mut VisualTestContext, on: bool) {
    cx.update(|_, cx| TlSettings::update(cx, |d| d.settings.startup.start_at_login = on));
}

#[gpui_kit::test]
fn start_minimized_disabled(cx: &mut TestAppContext) {
    let r = rig(cx);
    assert!(!settings(r.cx).startup.start_at_login);
    keys(r.cx, "alt-s");
    let row = entries(r.cx)
        .into_iter()
        .find(|e| e.label() == Some("Start minimized"))
        .expect("Start minimized drawn");
    let built = {
        let bar = bar(r.cx);
        r.cx.update(|window, cx| {
            let popup = bar.read(cx).popup().expect("menu open").clone();
            popup
                .read(cx)
                .entries_for_test(window)
                .into_iter()
                .find(|e| e.label.as_deref() == Some("Start minimized"))
                .map(|e| (e.kind, e.indent))
        })
    };
    assert_eq!(
        built,
        Some((tl_ui::components::menu::MenuEntryKind::Disabled, px(16.))),
        "disabled and indented by 16 at 100%"
    );
    assert_ne!(row.selected(), Some(true));
    // Fifteen Downs from Refresh pass "Start at login" and skip the disabled rows.
    keys(
        r.cx,
        "down down down down down down down down down down down down down down down",
    );
    assert_eq!(
        entries(r.cx)
            .into_iter()
            .find(|e| e.selected() == Some(true))
            .and_then(|e| e.label().map(str::to_owned))
            .as_deref(),
        Some("Default tab")
    );
}

#[gpui_kit::test]
fn start_at_login_success(cx: &mut TestAppContext) {
    let r = rig(cx);
    tall_window(r.cx);
    let calls = fake_autostart(r.cx, false);
    keys(r.cx, "alt-s");
    click_label(r.cx, "Start at login");
    assert_eq!(*calls.lock().expect("calls"), [(true, false)]);
    assert!(settings(r.cx).startup.start_at_login);
    assert_eq!(open_menu(r.cx), None);

    // Activated again: the entry is deleted and the key turns off.
    keys(r.cx, "alt-s");
    click_label(r.cx, "Start at login");
    assert_eq!(
        *calls.lock().expect("calls"),
        [(true, false), (false, false)]
    );
    assert!(!settings(r.cx).startup.start_at_login);
}

#[gpui_kit::test]
fn start_at_login_failure_reverts(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    tall_window(r.cx);
    let calls = fake_autostart(r.cx, true);
    keys(r.cx, "alt-s");
    click_label(r.cx, "Start at login");
    draw(r.cx);
    assert_eq!(*calls.lock().expect("calls"), [(true, false)]);
    assert!(
        !settings(r.cx).startup.start_at_login,
        "the key stays false"
    );
    assert_eq!(
        toast_texts(&mut r),
        ["Could not change the autostart entry"]
    );
}

#[gpui_kit::test]
fn start_minimized_rewrites_entry(cx: &mut TestAppContext) {
    let r = rig(cx);
    tall_window(r.cx);
    let calls = fake_autostart(r.cx, false);
    set_start_at_login(r.cx, true);
    keys(r.cx, "alt-s");
    click_label(r.cx, "Start minimized");
    assert!(settings(r.cx).startup.start_minimized);
    assert_eq!(
        *calls.lock().expect("calls"),
        [(true, true)],
        "the entry is rewritten"
    );

    // With Start at login off only the key changes.
    set_start_at_login(r.cx, false);
    r.cx.dispatch_action(tl_ui::window::menus_settings::ToggleStartMinimized);
    draw(r.cx);
    assert!(!settings(r.cx).startup.start_minimized);
    assert_eq!(calls.lock().expect("calls").len(), 1, "no writer call");
}

// Settings menu P3 rows (E21-033).

/// The Settings menu is taller than the 400 px rig window and scrolls; clicks need every row
/// on screen.
fn tall_window(cx: &mut VisualTestContext) {
    cx.simulate_resize(size(px(1000.), px(1200.)));
}

/// The built Settings popup entry labelled `label`: its kind and tooltip.
fn settings_entry(
    cx: &mut VisualTestContext,
    label: &str,
) -> (tl_ui::components::menu::MenuEntryKind, Option<String>) {
    let bar = bar(cx);
    cx.update(|window, cx| {
        let popup = bar.read(cx).popup().expect("menu open").clone();
        popup
            .read(cx)
            .entries_for_test(window)
            .into_iter()
            .find(|e| e.label.as_deref() == Some(label))
            .map(|e| (e.kind, e.tooltip.map(|t| t.to_string())))
            .unwrap_or_else(|| panic!("{label} built"))
    })
}

#[gpui_kit::test]
fn save_history_disabled_without_recording(cx: &mut TestAppContext) {
    use tl_ui::components::menu::MenuEntryKind;
    let r = rig(cx);
    assert!(!settings(r.cx).history.enabled);
    keys(r.cx, "alt-s");
    assert_eq!(
        settings_entry(r.cx, "Save history to disk"),
        (
            MenuEntryKind::Disabled,
            Some("Turn on Record history first".to_owned())
        )
    );
    keys(r.cx, "escape");
    r.cx.update(|_, cx| TlSettings::update(cx, |d| d.settings.history.enabled = true));
    keys(r.cx, "alt-s");
    let (kind, tooltip) = settings_entry(r.cx, "Save history to disk");
    assert_ne!(kind, MenuEntryKind::Disabled);
    assert_eq!(tooltip, None);
}

#[gpui_kit::test]
fn record_history_toast_and_engine_settings(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    tall_window(r.cx);
    keys(r.cx, "alt-s");
    click_label(r.cx, "Record history");
    draw(r.cx);
    assert!(settings(r.cx).history.enabled);
    assert_eq!(
        r.engine.settings().last().map(|s| s.history.enabled),
        Some(true)
    );
    assert_eq!(toast_texts(&mut r), ["History recording on"]);
}

#[gpui_kit::test]
fn history_storage_radio(cx: &mut TestAppContext) {
    let r = rig(cx);
    // Refresh, Network, Auto-scale, Record history, History storage.
    keys(r.cx, "alt-s down down down down right");
    assert_eq!(
        settings_sub(r.cx),
        [
            "1 hour", "2 hours", "3 hours", "4 hours", "6 hours", "8 hours", "12 hours"
        ]
    );
    keys(r.cx, "down down down down down down enter");
    assert_eq!(settings(r.cx).history.retention_h, 12);
}

#[gpui_kit::test]
fn tray_and_privilege_checks_toggle(cx: &mut TestAppContext) {
    let r = rig(cx);
    tall_window(r.cx);
    type Read = fn(&Settings) -> bool;
    let rows: [(&str, Read); 5] = [
        ("Close to tray", |s| s.tray.close_to_tray),
        ("Minimize to tray", |s| s.tray.minimize_to_tray),
        ("Always show tray icon", |s| s.tray.always_show),
        ("Unlock privileged actions at launch", |s| {
            s.privilege.unlock_at_launch
        }),
        ("Open rescue window on critical memory pressure", |s| {
            s.lowmem.enabled
        }),
    ];
    for (label, read) in rows {
        assert!(!read(&settings(r.cx)), "{label} starts off");
        keys(r.cx, "alt-s");
        click_label(r.cx, label);
        assert!(read(&settings(r.cx)), "{label} turns on");
    }
}

#[gpui_kit::test]
fn settings_menu_p3_order(cx: &mut TestAppContext) {
    let r = rig(cx);
    keys(r.cx, "alt-s");
    assert_eq!(
        group(r.cx, "Monitoring"),
        [
            "Refresh",
            "Network transfer unit",
            "Auto-scale network transfer rates",
            "Record history",
            "History storage",
            "Save history to disk",
            "Rendering GPU"
        ]
    );
    assert_eq!(
        group(r.cx, "Window & startup"),
        [
            "Close to tray",
            "Minimize to tray",
            "Always show tray icon",
            "Start at login",
            "Start minimized",
            "Default tab",
            "Unlock privileged actions at launch",
            "Skip password prompts (experimental)",
            "Auto-fetch Public IP on launch",
            "Open rescue window on critical memory pressure"
        ]
    );
}

// View > Theme (E21-012).

const P2_THEMES: [&str; 12] = [
    "Default Dark",
    "Vivid Nightfall",
    "Catppuccin Frappé",
    "Catppuccin Macchiato",
    "Catppuccin Mocha",
    "Dracula",
    "OLED Black",
    "Default Light",
    "Windows 7 Aero",
    "Golden Summer Fields",
    "Soft Pink Delight",
    "Catppuccin Latte",
];

/// The built-in names in menu order. The embedded file holds the P1 pair until the P2 themes
/// land; then this is all twelve.
fn builtin_names(cx: &mut VisualTestContext) -> Vec<String> {
    let present: Vec<String> = cx.update(|_, cx| {
        cx.global::<tl_ui::theme::ThemeStore>()
            .builtin
            .iter()
            .map(|d| d.name.to_string())
            .collect()
    });
    P2_THEMES
        .iter()
        .filter(|n| present.iter().any(|p| p == *n))
        .map(|n| n.to_string())
        .collect()
}

/// Opens View > Theme from the keyboard (the item above Graphs).
fn open_theme(cx: &mut VisualTestContext) {
    keys(cx, "alt-v end up right");
    assert_eq!(open_menu(cx), Some(MenuId::View));
}

/// The built entries of the open View > Theme submenu.
fn theme_entries(cx: &mut VisualTestContext) -> Vec<tl_ui::components::menu::MenuEntryInfo> {
    let bar = bar(cx);
    cx.update(|window, cx| {
        let popup = bar.read(cx).popup().expect("menu open").clone();
        let root = popup.read(cx).entries_for_test(window);
        assert_eq!(root[4].label.as_deref(), Some("Theme"));
        let sub = root[4].submenu.clone().expect("Theme submenu");
        sub.read(cx).entries_for_test(window)
    })
}

fn theme(cx: &mut VisualTestContext) -> ThemeSettings {
    cx.update(|_, cx| TlSettings::settings(cx).theme.clone())
}

fn active_theme(cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| tl_ui::theme::current(cx).name.to_string())
}

fn add_custom_theme(cx: &mut VisualTestContext, name: &str, base: &str) {
    let name = name.to_owned();
    let base = base.to_owned();
    cx.update(move |_, cx| {
        let mut def = cx
            .global::<tl_ui::theme::ThemeStore>()
            .builtin
            .iter()
            .find(|d| d.name.as_ref() == base)
            .expect("base theme")
            .clone();
        def.name = name.clone().into();
        cx.global_mut::<tl_ui::theme::ThemeStore>().user.push(def);
    });
}

fn user_theme_names(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, cx| {
        cx.global::<tl_ui::theme::ThemeStore>()
            .user
            .iter()
            .map(|d| d.name.to_string())
            .collect()
    })
}

fn window_count(cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| cx.windows().len())
}

fn delete_dialog_handle(cx: &mut VisualTestContext) -> WindowHandle<Root> {
    let parent = cx.update(|window, _| window.window_handle().window_id());
    cx.update(|_, cx| cx.windows().into_iter().find(|w| w.window_id() != parent))
        .and_then(|w: AnyWindowHandle| w.downcast::<Root>())
        .expect("delete confirmation window")
}

fn delete_dialog_view(
    cx: &mut VisualTestContext,
    handle: WindowHandle<Root>,
) -> Entity<TlDialogView> {
    cx.update(|_, cx| TlSecondaryWindow::<TlDialogView>::find(handle, cx))
        .map(|frame| cx.read(|cx| frame.read(cx).content().clone()))
        .expect("delete dialog view")
}

fn set_follow(cx: &mut VisualTestContext, on: bool) {
    cx.update(|_, cx| TlSettings::update(cx, |d| d.settings.theme.follow_system = on));
}

fn activate(cx: &mut VisualTestContext, name: &str) {
    cx.dispatch_action(tl_ui::theme::ActivateTheme {
        name: name.to_owned().into(),
    });
    draw(cx);
}

#[gpui_kit::test]
fn theme_rows_painted(cx: &mut TestAppContext) {
    let r = rig(cx);
    let builtin = builtin_names(r.cx);
    let n = builtin.len();
    assert!(n == 2 || n == 12, "P1 or P2 built-ins: {builtin:?}");
    open_theme(r.cx);
    let rows = theme_entries(r.cx);
    let labels: Vec<String> = rows
        .iter()
        .take(n)
        .map(|e| e.label.as_ref().map(|l| l.to_string()).unwrap_or_default())
        .collect();
    assert_eq!(labels, builtin);
    assert!(rows[0].checked, "the active theme carries the mark");
    assert!(rows[1..n].iter().all(|e| !e.checked));
    // Dracula in a P2 build, Default Light in a P1 build: a theme that is not active.
    let other = if n == 12 { "Dracula" } else { "Default Light" };
    let ix = builtin.iter().position(|b| b == other).expect("listed");
    let painted = rows[ix].painted.expect("painted row");
    keys(r.cx, "escape escape");
    // That theme's own colours, as built when it is active.
    r.cx.update(|_, cx| {
        tl_ui::theme::store::activate(other, tl_ui::theme::ActivateSource::User, cx)
            .expect("theme exists");
    });
    let (menu_bg, text) = r.cx.update(|_, cx| {
        let t = tl_ui::theme::current(cx);
        (t.menu_bg, t.text_primary)
    });
    assert_eq!(painted, (menu_bg, text));
    // Then sync, and the two preferred-theme submenus.
    open_theme(r.cx);
    let tail: Vec<String> = theme_entries(r.cx)
        .iter()
        .skip(n)
        .map(|e| e.label.as_ref().map(|l| l.to_string()).unwrap_or_default())
        .collect();
    assert_eq!(
        tail,
        [
            "",
            "Create copy",
            "Edit colors",
            "Share theme",
            "Delete theme",
            "",
            "Import theme",
            "Sync theme with system",
            "Preferred Light Theme",
            "Preferred Dark Theme"
        ]
    );
}

#[gpui_kit::test]
fn copy_theme_names_increment(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    let source = if builtin_names(r.cx).iter().any(|n| n == "Dracula") {
        "Dracula"
    } else {
        "Default Light"
    };
    r.cx.dispatch_action(CopyTheme {
        name: source.into(),
    });
    draw(r.cx);
    r.cx.dispatch_action(CopyTheme {
        name: source.into(),
    });
    draw(r.cx);
    assert_eq!(
        user_theme_names(r.cx),
        [format!("{source} copy"), format!("{source} copy 2")]
    );
    assert_eq!(active_theme(r.cx), format!("{source} copy 2"));
    assert_eq!(theme(r.cx).active, format!("{source} copy 2"));
    assert_eq!(toast_texts(&mut r), ["Theme copied (×2)"]);
    assert_eq!(r.theme_persists.borrow().len(), 2);
}

#[gpui_kit::test]
fn copy_icon_copies_row_theme(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    let source = if builtin_names(r.cx).iter().any(|n| n == "Dracula") {
        "Dracula"
    } else {
        "Default Light"
    };
    open_theme(r.cx);
    let row = entries(r.cx)
        .into_iter()
        .find(|e| e.role() == Some(Role::MenuItem) && e.label() == Some(source))
        .expect("source theme row");
    let suffix =
        r.cx.update(|window, _| snapshots(window))
            .into_iter()
            .find(|s| {
                s.role() == Some(Role::Button)
                    && s.label() == Some("Create copy")
                    && row.bounds().contains(&s.bounds().center())
            })
            .expect("theme copy suffix");
    click(r.cx, suffix.bounds().center());
    assert_eq!(user_theme_names(r.cx), [format!("{source} copy")]);
    assert_eq!(active_theme(r.cx), format!("{source} copy"));
    assert_eq!(open_menu(r.cx), Some(MenuId::View), "the menu stays open");
    assert_eq!(toast_texts(&mut r), ["Theme copied"]);
}

fn import_clipboard(r: &mut Rig<'_>, text: String) {
    r.cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(text)));
    r.cx.dispatch_action(ImportTheme);
    draw(r.cx);
}

fn builtin_def(cx: &mut VisualTestContext, name: &str) -> tl_ui::theme::ThemeDef {
    cx.update(|_, cx| {
        cx.global::<tl_ui::theme::ThemeStore>()
            .builtin
            .iter()
            .find(|d| d.name.as_ref() == name)
            .expect("built-in theme")
            .clone()
    })
}

fn user_defs(cx: &mut VisualTestContext) -> Vec<tl_ui::theme::ThemeDef> {
    cx.update(|_, cx| cx.global::<tl_ui::theme::ThemeStore>().user.clone())
}

#[gpui_kit::test]
fn theme_menu_p3_items(cx: &mut TestAppContext) {
    let r = rig_with_toasts(cx);
    let n = builtin_names(r.cx).len();
    open_theme(r.cx);
    let tail: Vec<String> = theme_entries(r.cx)
        .iter()
        .skip(n)
        .map(|e| e.label.as_ref().map(|l| l.to_string()).unwrap_or_default())
        .collect();
    assert_eq!(
        tail,
        [
            "",
            "Create copy",
            "Edit colors",
            "Share theme",
            "Delete theme",
            "",
            "Import theme",
            "Sync theme with system",
            "Preferred Light Theme",
            "Preferred Dark Theme"
        ]
    );
}

#[gpui_kit::test]
fn import_tl1_roundtrip(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    add_custom_theme(r.cx, "Mine", "Default Dark");
    let def = user_defs(r.cx).remove(0);
    import_clipboard(&mut r, tl_ui::theme::share_code::encode_tl1(&def));
    assert_eq!(user_theme_names(r.cx), ["Mine", "Mine 2"]);
    let defs = user_defs(r.cx);
    assert_eq!(defs[1].slots, defs[0].slots);
    assert_eq!(defs[1].accent, defs[0].accent);
    assert_eq!(active_theme(r.cx), "Mine 2");
    assert_eq!(theme(r.cx).active, "Mine 2");
    assert_eq!(toast_texts(&mut r), ["Theme imported"]);
    assert_eq!(r.theme_persists.borrow().len(), 1);
}

#[gpui_kit::test]
fn import_raw_json_multiple(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    let mut a = builtin_def(r.cx, "Default Dark");
    a.name = "A".into();
    let mut b = builtin_def(r.cx, "Default Light");
    b.name = "B".into();
    import_clipboard(
        &mut r,
        tl_ui::theme::user_store::serialize_user_themes(&[a, b]),
    );
    assert_eq!(user_theme_names(r.cx), ["A", "B"]);
    assert_eq!(active_theme(r.cx), "A");
    assert_eq!(toast_texts(&mut r), ["Imported 2 themes"]);
}

#[gpui_kit::test]
fn import_missing_slot_fill(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    import_clipboard(
        &mut r,
        r##"{"themes":[{"name":"Half","WINDOW_BG":"#101010"}]}"##.to_owned(),
    );
    let dark = builtin_def(r.cx, "Default Dark");
    let defs = user_defs(r.cx);
    assert_eq!(defs[0].name.as_ref(), "Half");
    let i = tl_ui::theme::ThemeSlot::Titlebar as usize;
    assert_eq!(defs[0].slots[i], dark.slots[i]);
}

#[gpui_kit::test]
fn import_garbage_toast(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    let active = active_theme(r.cx);
    import_clipboard(&mut r, "hello".to_owned());
    assert_eq!(
        toast_texts(&mut r),
        ["Clipboard does not contain a theme code"]
    );
    assert!(user_theme_names(r.cx).is_empty());
    assert_eq!(active_theme(r.cx), active);
    assert!(r.theme_persists.borrow().is_empty());
}

#[gpui_kit::test]
fn delete_builtin_toast(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    let before = window_count(r.cx);
    r.cx.dispatch_action(DeleteTheme {
        name: "Default Dark".into(),
    });
    draw(r.cx);
    assert_eq!(window_count(r.cx), before, "no confirmation opens");
    assert_eq!(toast_texts(&mut r), ["Built-in themes cannot be deleted"]);
    assert!(r.theme_persists.borrow().is_empty());
}

#[gpui_kit::test]
fn delete_active_falls_back_to_preferred(cx: &mut TestAppContext) {
    let r = rig_with_toasts(cx);
    add_custom_theme(r.cx, "Mine", "Default Dark");
    activate(r.cx, "Mine");
    r.cx.update(|_, cx| TlSettings::update(cx, |d| d.settings.theme.dark = "Mine".to_owned()));
    r.cx.dispatch_action(DeleteTheme {
        name: "Mine".into(),
    });
    r.cx.run_until_parked();
    let handle = delete_dialog_handle(r.cx);
    let persists = r.theme_persists.clone();
    let queue = r.toasts.clone().expect("toast queue");
    drop(r);

    let mut dialog = VisualTestContext::from_window(handle.into(), cx);
    dialog.run_until_parked();
    dialog.update(|window, cx| window.draw(cx).clear(cx));
    let at = dialog.debug_bounds("TlDialogAction").expect("Delete drawn");
    dialog.simulate_click(at.center(), Modifiers::none());
    dialog.run_until_parked();
    drop(dialog);

    let (names, active, settings) = cx.update(|cx| {
        let names = cx
            .global::<tl_ui::theme::ThemeStore>()
            .user
            .iter()
            .map(|d| d.name.to_string())
            .collect::<Vec<_>>();
        let active = current(cx).name.to_string();
        (names, active, TlSettings::settings(cx).theme.clone())
    });
    assert!(names.is_empty());
    assert_eq!(active, "Default Dark");
    assert_eq!(settings.active, "Default Dark");
    assert_eq!(settings.dark, "Default Dark");
    assert_eq!(persists.borrow().len(), 1);
    assert_eq!(
        cx.read(|cx| queue.read(cx).visible_texts()[0].to_string()),
        "Theme deleted"
    );
}

#[gpui_kit::test]
fn delete_resets_preferred_key(cx: &mut TestAppContext) {
    let r = rig(cx);
    add_custom_theme(r.cx, "Mine", "Default Dark");
    r.cx.update(|_, cx| TlSettings::update(cx, |d| d.settings.theme.dark = "Mine".to_owned()));
    r.cx.dispatch_action(DeleteTheme {
        name: "Mine".into(),
    });
    r.cx.run_until_parked();
    let handle = delete_dialog_handle(r.cx);
    drop(r);

    let mut dialog = VisualTestContext::from_window(handle.into(), cx);
    dialog.run_until_parked();
    dialog.update(|window, cx| window.draw(cx).clear(cx));
    let at = dialog.debug_bounds("TlDialogAction").expect("Delete drawn");
    dialog.simulate_click(at.center(), Modifiers::none());
    dialog.run_until_parked();
    drop(dialog);
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).theme.dark.clone()),
        "Default Dark"
    );
}

#[gpui_kit::test]
fn delete_confirm_focuses_cancel(cx: &mut TestAppContext) {
    let r = rig(cx);
    add_custom_theme(r.cx, "Mine", "Default Dark");
    r.cx.dispatch_action(DeleteTheme {
        name: "Mine".into(),
    });
    r.cx.run_until_parked();
    let handle = delete_dialog_handle(r.cx);
    let view = delete_dialog_view(r.cx, handle);
    drop(r);

    let mut dialog = VisualTestContext::from_window(handle.into(), cx);
    dialog.update(|window, _| window.activate_window());
    dialog.run_until_parked();
    let (title, text, destructive) = dialog.read(|cx| {
        let dialog = view.read(cx).dialog();
        (
            dialog.window_title().to_string(),
            dialog.text().to_string(),
            dialog.is_destructive(),
        )
    });
    assert_eq!(title, "tasklens");
    assert_eq!(text, "Delete this custom theme?");
    assert!(destructive);
    assert_eq!(
        dialog.update(|window, cx| view.read(cx).focused_button(window)),
        Some(DialogButton::Cancel)
    );
    dialog.simulate_keystrokes("escape");
    dialog.run_until_parked();
    drop(dialog);
    assert_eq!(
        cx.update(|cx| {
            cx.global::<tl_ui::theme::ThemeStore>()
                .user
                .iter()
                .map(|d| d.name.to_string())
                .collect::<Vec<_>>()
        }),
        ["Mine"]
    );
}

#[gpui_kit::test]
fn set_theme_turns_off_sync(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    set_follow(r.cx, true);
    activate(r.cx, "Default Light");
    assert!(
        !theme(r.cx).follow_system,
        "an explicit choice turns sync off"
    );
    assert_eq!(theme(r.cx).active, "Default Light");
    assert_eq!(active_theme(r.cx), "Default Light");
    assert_eq!(toast_texts(&mut r), ["Theme sync turned off"]);
}

#[gpui_kit::test]
fn set_theme_no_toast_without_sync(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    assert!(!theme(r.cx).follow_system);
    // From the menu: the second row.
    let second = builtin_names(r.cx)[1].clone();
    open_theme(r.cx);
    keys(r.cx, "down enter");
    assert_eq!(theme(r.cx).active, second);
    assert_eq!(active_theme(r.cx), second);
    assert!(toast_texts(&mut r).is_empty());
}

#[gpui_kit::test]
fn theme_custom_suffix(cx: &mut TestAppContext) {
    let r = rig(cx);
    // A user theme named like a built-in one (Dracula in the bead; any built-in name works).
    let builtin = builtin_names(r.cx);
    let name = builtin.last().expect("built-ins").clone();
    let shadow = name.clone();
    r.cx.update(move |_, cx| {
        let def = cx
            .global::<tl_ui::theme::ThemeStore>()
            .builtin
            .iter()
            .find(|d| *d.name == *shadow)
            .expect("built-in")
            .clone();
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            vec![def],
        );
    });
    let rows: Vec<String> =
        r.cx.update(|_, cx| menus_view::theme_rows(cx))
            .into_iter()
            .map(|row| row.label.to_string())
            .collect();
    let custom = format!("{name} (custom)");
    assert_eq!(rows.len(), builtin.len() + 1);
    assert_eq!(rows[builtin.len() - 1], name, "both rows exist");
    assert_eq!(rows[builtin.len()], custom);
    open_theme(r.cx);
    let drawn = labels(r.cx);
    assert!(drawn.contains(&custom), "{drawn:?}");
}

#[gpui_kit::test]
fn activate_refused_while_editing(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    r.cx.update(|_, cx| {
        let store = cx.global_mut::<tl_ui::theme::ThemeStore>();
        store.preview = Some(store.active.clone());
    });
    activate(r.cx, "Default Light");
    assert_eq!(active_theme(r.cx), "Default Dark", "unchanged");
    assert_eq!(theme(r.cx).active, "Default Dark");
    assert_eq!(
        toast_texts(&mut r),
        ["Finish or cancel the theme editor first"]
    );
    let rows = r.cx.update(|_, cx| menus_view::theme_rows(cx));
    let marked: Vec<String> = rows
        .iter()
        .filter(|row| row.selected)
        .map(|row| row.label.to_string())
        .collect();
    assert_eq!(marked, ["Default Dark (editing)"]);
}

#[gpui_kit::test]
fn sync_on_applies_at_once(cx: &mut TestAppContext) {
    let r = rig(cx);
    r.cx.update(|_, cx| cx.global_mut::<tl_ui::theme::ThemeStore>().system_scheme = 2);
    assert!(!theme(r.cx).follow_system);
    open_theme(r.cx);
    click_label(r.cx, "Sync theme with system");
    assert!(theme(r.cx).follow_system);
    assert_eq!(active_theme(r.cx), theme(r.cx).light, "theme.light at once");
    assert_eq!(active_theme(r.cx), "Default Light");
}

#[gpui_kit::test]
fn preferred_theme_radios(cx: &mut TestAppContext) {
    let r = rig(cx);
    open_theme(r.cx);
    // End reaches Preferred Dark Theme, the last row.
    keys(r.cx, "end right");
    assert!(
        sub_highlighted(r.cx).contains(&"Default Dark".to_owned()),
        "the first radio is highlighted: {:?}",
        sub_highlighted(r.cx)
    );
    // "OLED Black" in a P2 build; the last built-in here.
    let builtin = builtin_names(r.cx);
    let target = if builtin.len() == 12 {
        "OLED Black".to_owned()
    } else {
        builtin.last().expect("built-ins").clone()
    };
    let downs = builtin.iter().position(|b| *b == target).expect("listed");
    keys(r.cx, &vec!["down"; downs].join(" "));
    keys(r.cx, "enter");
    assert_eq!(theme(r.cx).dark, target);
    assert_eq!(theme(r.cx).light, "Default Light", "the other key is kept");
    assert_eq!(
        active_theme(r.cx),
        "Default Dark",
        "no switch with sync off"
    );
}

// Graph Color submenu (E21-032).

use tl_ui::pages::minigraph::GraphColor;
use tl_ui::pages::minigraph::actions::MinigraphSetColor;
use tl_ui::pages::minigraph::sources::MinigraphId;

fn set_color(color: GraphColor) -> Box<dyn gpui_kit::Action> {
    Box::new(MinigraphSetColor {
        id: MinigraphId(7),
        color,
    })
}

/// Opens a context menu filled by `graph_color_menu` with the mark on `current`, and returns
/// its built entries.
fn open_graph_colors(
    r: &mut Rig<'_>,
    current: GraphColor,
) -> Vec<tl_ui::components::menu::MenuEntryInfo> {
    let table = r.table.clone();
    let popup = r.cx.update(|window, cx| {
        let popup =
            tl_ui::components::menu::TlMenu::build(window, cx, table, move |menu, _, cx| {
                menubar::graph_color_menu(menu, current, set_color, cx)
            });
        tl_ui::components::menu::open_context_menu_at(
            gpui_kit::point(px(100.), px(40.)),
            window,
            cx,
            popup.clone(),
        );
        popup
    });
    draw(r.cx);
    r.cx.update(|window, cx| popup.read(cx).entries_for_test(window))
}

const COLOR_LABELS: [&str; 18] = [
    "Accent (theme default)",
    "Coral",
    "Orange",
    "Amber",
    "Yellow",
    "Lime",
    "Green",
    "Mint",
    "Teal",
    "Cyan",
    "Blue",
    "Indigo",
    "Violet",
    "Lavender",
    "Fuchsia",
    "Pink",
    "Rose",
    "Slate",
];

#[gpui_kit::test]
fn graph_color_menu_rows(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    let rows = open_graph_colors(&mut r, GraphColor::Teal);
    let names: Vec<String> = rows
        .iter()
        .map(|e| e.label.as_ref().map(|l| l.to_string()).unwrap_or_default())
        .collect();
    assert_eq!(names, COLOR_LABELS);
    let marked: Vec<&str> = rows
        .iter()
        .zip(COLOR_LABELS)
        .filter(|(e, _)| e.checked)
        .map(|(_, l)| l)
        .collect();
    assert_eq!(marked, ["Teal"]);
    assert!(
        rows.iter().all(|e| e.swatch.is_some()),
        "every row has a swatch"
    );
    let teal: gpui_kit::Hsla = gpui_kit::rgb(0x14A098).into();
    assert_eq!(rows[8].swatch, Some(teal));
    // The rows are drawn, top to bottom.
    let drawn = labels(r.cx);
    assert_eq!(drawn, COLOR_LABELS);
}

#[gpui_kit::test]
fn graph_color_accent_swatch(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    let rows = open_graph_colors(&mut r, GraphColor::Accent);
    let accent = r.cx.update(|_, cx| tl_ui::theme::current(cx).accent);
    let default_dark: gpui_kit::Hsla = gpui_kit::rgb(0x0277A6).into();
    assert_eq!(accent, default_dark, "Default Dark is active");
    assert_eq!(rows[0].swatch, Some(default_dark));
    assert!(rows[0].checked);
}

#[gpui_kit::test]
fn graph_color_dispatch(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    let picked = Rc::new(RefCell::new(Vec::new()));
    let sink = picked.clone();
    r.cx.update(|_, cx| {
        cx.on_action(move |a: &MinigraphSetColor, _| sink.borrow_mut().push(a.clone()));
    });
    open_graph_colors(&mut r, GraphColor::Accent);
    // Accent is highlighted; End reaches Slate, Up reaches Rose.
    keys(r.cx, "end up enter");
    assert_eq!(
        *picked.borrow(),
        [MinigraphSetColor {
            id: MinigraphId(7),
            color: GraphColor::Rose
        }]
    );
}

// Settings > Rendering GPU (E21-040, UI-SHL-040a, R-237).

/// Two adapters: a discrete card with a PCI bus id and a software one without.
fn two_adapters() -> Vec<tl_ui::app::AdapterInfo> {
    vec![
        tl_ui::app::AdapterInfo {
            name: "NVIDIA GeForce RTX 4070".to_owned(),
            backend: "Vulkan".to_owned(),
            device_type: "DiscreteGpu".to_owned(),
            pci_bus_id: Some("0000:01:00.0".to_owned()),
        },
        tl_ui::app::AdapterInfo {
            name: "llvmpipe".to_owned(),
            backend: "Vulkan".to_owned(),
            device_type: "Cpu".to_owned(),
            pci_bus_id: None,
        },
    ]
}

/// The sender of the last probe the fake `list_adapters` started.
type ProbeSender =
    Arc<std::sync::Mutex<Option<async_channel::Sender<Vec<tl_ui::app::AdapterInfo>>>>>;

/// Installs an `AppEnv` whose `list_adapters` keeps each probe's sender for the test, or answers
/// at once with `answer` when it is set.
fn fake_probe(
    cx: &mut VisualTestContext,
    answer: Option<Vec<tl_ui::app::AdapterInfo>>,
) -> ProbeSender {
    let sender: ProbeSender = Arc::default();
    let slot = sender.clone();
    cx.update(|_, cx| {
        cx.set_global(tl_ui::app::AppEnv {
            list_adapters: Arc::new(move || {
                let (tx, rx) = async_channel::bounded(1);
                if let Some(adapters) = &answer {
                    tx.try_send(adapters.clone()).expect("room for one answer");
                }
                *slot.lock().expect("probe sender") = Some(tx);
                rx
            }),
            ..tl_ui::app::AppEnv::test()
        })
    });
    sender
}

/// Opens Settings > Rendering GPU from the keyboard: Refresh, Network, Auto-scale, Record
/// history, History storage, Rendering GPU ("Save history to disk" is disabled and skipped).
fn open_rendering_gpu(cx: &mut VisualTestContext) {
    keys(cx, "alt-s down down down down down right");
}

/// The highlighted rows of the open Rendering GPU submenu.
fn gpu_highlighted(cx: &mut VisualTestContext) -> Vec<String> {
    entries(cx)
        .iter()
        .filter(|e| e.selected() == Some(true))
        .filter_map(|e| e.label().map(str::to_owned))
        .filter(|l| l != "Rendering GPU")
        .collect()
}

/// The built Rendering GPU rows as (label, kind, marked).
fn gpu_rows(
    cx: &mut VisualTestContext,
) -> Vec<(String, tl_ui::components::menu::MenuEntryKind, bool)> {
    let bar = bar(cx);
    cx.update(|window, cx| {
        let popup = bar.read(cx).popup().expect("menu open").clone();
        let sub = popup
            .read(cx)
            .entries_for_test(window)
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Rendering GPU"))
            .and_then(|e| e.submenu)
            .expect("Rendering GPU submenu");
        sub.read(cx)
            .entries_for_test(window)
            .into_iter()
            .map(|e| {
                let label = e.label.map(|l| l.to_string()).unwrap_or_default();
                (label, e.kind, e.checked)
            })
            .collect()
    })
}

#[gpui_kit::test]
fn rendering_gpu_detecting_then_rebuild(cx: &mut TestAppContext) {
    use tl_ui::components::menu::MenuEntryKind;
    let r = rig(cx);
    let sender = fake_probe(r.cx, None);
    open_rendering_gpu(r.cx);
    assert_eq!(
        settings_sub(r.cx),
        ["Auto", "Detecting adapters…", "Software renderer"]
    );
    assert_eq!(
        gpu_rows(r.cx)[1],
        (
            "Detecting adapters…".to_owned(),
            MenuEntryKind::Disabled,
            false
        )
    );
    // The placeholder is skipped by the arrows.
    assert_eq!(gpu_highlighted(r.cx), ["Auto"]);
    keys(r.cx, "down");
    assert_eq!(gpu_highlighted(r.cx), ["Software renderer"]);

    // The probe answers well within 2 s.
    let tx = sender
        .lock()
        .expect("probe sender")
        .take()
        .expect("the open started a probe");
    tx.try_send(two_adapters()).expect("probe answer");
    draw(r.cx);
    assert_eq!(
        settings_sub(r.cx),
        [
            "Auto",
            "NVIDIA GeForce RTX 4070",
            "llvmpipe",
            "Software renderer"
        ]
    );
    assert_eq!(
        gpu_highlighted(r.cx),
        ["Software renderer"],
        "the highlight stays on its row"
    );
    // rendering.gpu = "auto": the mark is on Auto.
    let marks: Vec<bool> = gpu_rows(r.cx).into_iter().map(|(_, _, on)| on).collect();
    assert_eq!(marks, [true, false, false, false]);
}

#[gpui_kit::test]
fn rendering_gpu_probe_timeout(cx: &mut TestAppContext) {
    let r = rig(cx);
    // The fake keeps the sender and never answers.
    let sender = fake_probe(r.cx, None);
    open_rendering_gpu(r.cx);
    assert!(
        sender.lock().expect("probe sender").is_some(),
        "the open started a probe"
    );
    r.cx.executor().advance_clock(
        menus_settings::ADAPTER_PROBE_TIMEOUT - std::time::Duration::from_millis(100),
    );
    draw(r.cx);
    assert_eq!(
        settings_sub(r.cx),
        ["Auto", "Detecting adapters…", "Software renderer"],
        "still waiting at 1.9 s"
    );
    r.cx.executor()
        .advance_clock(std::time::Duration::from_millis(100));
    draw(r.cx);
    assert_eq!(
        settings_sub(r.cx),
        ["Auto", "No adapters found", "Software renderer"]
    );
    assert_eq!(gpu_highlighted(r.cx), ["Auto"]);
    keys(r.cx, "down");
    assert_eq!(gpu_highlighted(r.cx), ["Software renderer"], "skipped");
}

#[gpui_kit::test]
fn rendering_gpu_restart_toast(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    fake_probe(r.cx, Some(two_adapters()));
    let gpu = |r: &mut Rig<'_>| {
        r.cx.update(|_, cx| TlSettings::settings(cx).rendering.gpu.clone())
    };
    assert_eq!(gpu(&mut r), "auto");

    // Auto is already stored: choosing it again writes nothing new and shows no toast.
    open_rendering_gpu(r.cx);
    keys(r.cx, "enter");
    assert_eq!(gpu(&mut r), "auto");
    assert!(toast_texts(&mut r).is_empty(), "no toast without a change");

    // The adapter with a PCI bus id stores pci:<bus id>.
    open_rendering_gpu(r.cx);
    assert_eq!(
        settings_sub(r.cx),
        [
            "Auto",
            "NVIDIA GeForce RTX 4070",
            "llvmpipe",
            "Software renderer"
        ]
    );
    keys(r.cx, "down enter");
    assert_eq!(gpu(&mut r), "pci:0000:01:00.0");
    assert_eq!(toast_texts(&mut r), ["Restart tasklens to use the new GPU"]);

    // An adapter without one stores name:<adapter name> (R-237).
    open_rendering_gpu(r.cx);
    keys(r.cx, "down down enter");
    assert_eq!(gpu(&mut r), "name:llvmpipe");
}

// Settings > Skip password prompts (E21-042, UI-SHL-040, UI-SHL-091, R-222).

const PASSWORDLESS: &str = "Skip password prompts (experimental)";

/// A rig with toasts, a window tall enough for every Settings row, and a hub on the fake engine
/// (the flow reads the setup result from it).
fn passwordless_rig(cx: &mut TestAppContext) -> Rig<'_> {
    let r = rig_with_toasts(cx);
    tall_window(r.cx);
    let client: Arc<dyn EngineClient> = r.engine.clone();
    r.cx.update(|_, cx| {
        let hub = cx.new(|cx| tl_ui::hub::SnapshotHub::new(client, cx));
        cx.set_global(tl_ui::hub::GlobalHub(hub));
    });
    draw(r.cx);
    r
}

/// The `on` values of every `RunSetup { PolkitRule }` the engine received, in order.
fn polkit_steps(engine: &FakeEngine) -> Vec<bool> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::RunSetup {
                step: SetupStep::PolkitRule { on },
            } => Some(on),
            _ => None,
        })
        .collect()
}

fn queue_texts(cx: &mut TestAppContext, queue: &Entity<ToastQueue>) -> Vec<String> {
    cx.update(|cx| {
        queue
            .read(cx)
            .visible_texts()
            .iter()
            .map(|t| t.to_string())
            .collect()
    })
}

/// Clicks the row with the key off. Returns the engine, the main window's toast queue and a
/// context on the confirmation, which is the active window.
fn open_passwordless_confirm(
    cx: &mut TestAppContext,
) -> (
    Arc<FakeEngine>,
    Entity<ToastQueue>,
    Entity<TlDialogView>,
    VisualTestContext,
) {
    let r = passwordless_rig(cx);
    assert!(!settings(r.cx).privilege.passwordless);
    keys(r.cx, "alt-s");
    click_label(r.cx, PASSWORDLESS);
    r.cx.run_until_parked();
    let handle = delete_dialog_handle(r.cx);
    let view = delete_dialog_view(r.cx, handle);
    let engine = r.engine.clone();
    let queue = r.toasts.clone().expect("rig with toasts");
    drop(r);
    let mut dialog = VisualTestContext::from_window(handle.into(), cx);
    dialog.update(|window, _| window.activate_window());
    dialog.run_until_parked();
    (engine, queue, view, dialog)
}

#[gpui_kit::test]
fn passwordless_confirm_first(cx: &mut TestAppContext) {
    let (engine, _, view, mut dialog) = open_passwordless_confirm(cx);
    let (title, text, destructive) = dialog.read(|cx| {
        let dialog = view.read(cx).dialog();
        (
            dialog.window_title().to_string(),
            dialog.text().to_string(),
            dialog.is_destructive(),
        )
    });
    assert_eq!(title, "tasklens");
    assert_eq!(
        text,
        "tasklens actions that change other users' processes, system services and network \
         connections will run without asking for a password. Continue?"
    );
    draw(&mut dialog);
    let buttons: Vec<String> = dialog
        .update(|window, _| snapshots(window))
        .into_iter()
        .filter(|s| s.role() == Some(Role::Button))
        .filter_map(|s| s.label().map(str::to_owned))
        .collect();
    assert!(
        buttons.iter().any(|b| b == "Skip prompts") && buttons.iter().any(|b| b == "Cancel"),
        "buttons: {buttons:?}"
    );
    assert!(!destructive);
    assert_eq!(
        dialog.update(|window, cx| view.read(cx).focused_button(window)),
        Some(DialogButton::Action)
    );
    assert!(
        polkit_steps(&engine).is_empty(),
        "nothing sent before the answer"
    );
}

#[gpui_kit::test]
fn passwordless_success_writes_key(cx: &mut TestAppContext) {
    let (engine, _, _, mut dialog) = open_passwordless_confirm(cx);
    dialog.simulate_keystrokes("enter");
    dialog.run_until_parked();
    drop(dialog);
    assert_eq!(polkit_steps(&engine), [true]);
    assert!(!passwordless(cx), "not before the step ends");

    publish_polkit(&engine, 1, true, SetupPhase::Running);
    cx.run_until_parked();
    assert!(!passwordless(cx), "not while the step runs");

    publish_polkit(&engine, 1, true, SetupPhase::Succeeded);
    cx.run_until_parked();
    assert!(passwordless(cx));
}

#[gpui_kit::test]
fn passwordless_cancel_no_command(cx: &mut TestAppContext) {
    let (engine, queue, _, mut dialog) = open_passwordless_confirm(cx);
    dialog.simulate_keystrokes("escape");
    dialog.run_until_parked();
    drop(dialog);
    cx.run_until_parked();
    assert!(polkit_steps(&engine).is_empty(), "no setup step sent");
    assert!(!passwordless(cx));
    assert!(queue_texts(cx, &queue).is_empty(), "no toast");
}

#[gpui_kit::test]
fn passwordless_dismissed_prompt_toast(cx: &mut TestAppContext) {
    let (engine, queue, _, mut dialog) = open_passwordless_confirm(cx);
    dialog.simulate_keystrokes("enter");
    dialog.run_until_parked();
    drop(dialog);
    assert_eq!(polkit_steps(&engine), [true]);

    publish_polkit(&engine, 1, true, SetupPhase::Cancelled);
    cx.run_until_parked();
    assert_eq!(queue_texts(cx, &queue), ["Authentication cancelled"]);
    assert!(!passwordless(cx));

    // The answered run does not count again.
    publish_polkit(&engine, 1, true, SetupPhase::Succeeded);
    cx.run_until_parked();
    assert!(!passwordless(cx));
}

#[gpui_kit::test]
fn passwordless_off_no_confirm(cx: &mut TestAppContext) {
    let r = passwordless_rig(cx);
    r.cx.update(|_, cx| {
        TlSettings::set(
            cx,
            tl_ui::settings_global::SettingKey::PrivilegePasswordless,
            tl_ui::settings_global::SettingValue::Bool(true),
        )
    });
    let windows = window_count(r.cx);
    keys(r.cx, "alt-s");
    click_label(r.cx, PASSWORDLESS);
    r.cx.run_until_parked();
    assert_eq!(window_count(r.cx), windows, "no confirmation opened");
    assert_eq!(polkit_steps(&r.engine), [false]);
    assert!(
        settings(r.cx).privilege.passwordless,
        "not before the step ends"
    );

    publish_polkit(&r.engine, 1, false, SetupPhase::Succeeded);
    r.cx.run_until_parked();
    assert!(!settings(r.cx).privilege.passwordless);
}

// ---- Debug menu (tasklens-bj6.42) ----

fn set_debug(cx: &mut VisualTestContext, debug: bool) {
    cx.update(|_, cx| {
        cx.set_global(AppEnv {
            debug,
            ..AppEnv::test()
        })
    });
    draw(cx);
}

#[gpui_kit::test]
fn debug_menu_hidden_without_flag(cx: &mut TestAppContext) {
    let r = rig(cx);
    assert!(r.cx.debug_bounds(DEBUG_BUTTON).is_none(), "no AppEnv");
    keys(r.cx, "alt-d");
    assert_eq!(open_menu(r.cx), None);
    set_debug(r.cx, false);
    assert!(r.cx.debug_bounds(DEBUG_BUTTON).is_none(), "debug off");
    keys(r.cx, "alt-d");
    assert_eq!(open_menu(r.cx), None);
}

#[gpui_kit::test]
fn debug_menu_alt_d(cx: &mut TestAppContext) {
    let r = rig(cx);
    set_debug(r.cx, true);
    let settings_right = bounds(r.cx, SETTINGS_BUTTON).right();
    assert!(bounds(r.cx, DEBUG_BUTTON).left() >= settings_right);
    keys(r.cx, "alt-d");
    assert_eq!(open_menu(r.cx), Some(MenuId::Debug));
    assert_eq!(
        labels(r.cx),
        [
            "Show FPS",
            "Continuous rendering",
            "Enable all columns",
            "Add random process minigraphs",
            "Collector health",
        ]
    );
    assert_eq!(entries(r.cx).len(), 6, "five rows and a separator");
    assert_eq!(entries(r.cx)[0].selected(), Some(true), "Show FPS");
}

#[gpui_kit::test]
fn continuous_rendering_requests_frames(cx: &mut TestAppContext) {
    let r = rig(cx);
    set_debug(r.cx, true);
    let requested =
        |cx: &mut VisualTestContext| cx.update(|_, cx| menus_debug::continuous_requested(cx));
    let frames = |cx: &mut VisualTestContext| {
        for _ in 0..10 {
            cx.update(|window, cx| window.simulate_next_frame(cx));
        }
    };
    keys(r.cx, "alt-d down enter");
    let start = requested(r.cx);
    assert!(start >= 1, "the first request starts the chain");
    frames(r.cx);
    assert_eq!(requested(r.cx) - start, 10);
    assert!(
        r.cx.update(|_, cx| menus_debug::continuous_on(cx)),
        "flag on"
    );
    keys(r.cx, "alt-d down enter");
    let stopped = requested(r.cx);
    frames(r.cx);
    assert_eq!(requested(r.cx) - stopped, 0);
}

#[gpui_kit::test]
fn enable_all_columns_dispatch_and_toast(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    r.cx.update(|_, cx| {
        cx.on_action(move |a: &EnableAllColumns, _| sink.borrow_mut().push(a.table));
    });
    set_debug(r.cx, true);
    set_tab(r.cx, Tab::Processes, false);
    keys(r.cx, "alt-d down down enter");
    r.cx.run_until_parked();
    assert_eq!(*seen.borrow(), [TableId::Processes]);
    assert!(toast_texts(&mut r).contains(&"All columns enabled".to_owned()));
}

#[gpui_kit::test]
fn enable_all_columns_disabled_on_performance(cx: &mut TestAppContext) {
    let r = rig(cx);
    set_debug(r.cx, true);
    set_tab(r.cx, Tab::Performance, false);
    keys(r.cx, "alt-d down down");
    let drawn = entries(r.cx);
    assert_eq!(drawn[2].label(), Some("Enable all columns"));
    assert_ne!(drawn[2].selected(), Some(true), "never highlighted");
    let row = drawn[2].bounds();
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
fn debug_collector_health_dispatch(cx: &mut TestAppContext) {
    let r = rig(cx);
    let count = Rc::new(Cell::new(0));
    let sink = count.clone();
    r.cx.update(|_, cx| {
        cx.on_action(move |_: &OpenCollectorHealth, _| sink.set(sink.get() + 1));
    });
    set_debug(r.cx, true);
    keys(r.cx, "alt-d end enter");
    assert_eq!(count.get(), 1);
}

#[gpui_kit::test]
fn help_button_present_p2(cx: &mut TestAppContext) {
    let r = rig(cx);
    let settings = bounds(r.cx, SETTINGS_BUTTON);
    let help = bounds(r.cx, HELP_BUTTON);
    assert!(
        help.origin.x >= settings.origin.x + settings.size.width,
        "Help comes after Settings"
    );
}

#[gpui_kit::test]
fn help_menu_p2_items(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    keys(r.cx, "alt-h");
    assert_eq!(open_menu(r.cx), Some(MenuId::Help));
    let entries = entries(r.cx);
    assert_eq!(
        entries.len(),
        6,
        "Keyboard Shortcuts, System Information, Additional setup, Documentation, separator, About: {entries:?}"
    );
    assert_eq!(entries[0].role(), Some(Role::MenuItem));
    assert_eq!(entries[0].label(), Some("Keyboard Shortcuts"));
    assert_eq!(entries[0].selected(), Some(true));
    assert_eq!(entries[1].role(), Some(Role::MenuItem));
    assert_eq!(entries[1].label(), Some("System Information…"));
    assert_eq!(entries[2].role(), Some(Role::MenuItem));
    assert_eq!(entries[2].label(), Some("Additional setup…"));
    assert_eq!(entries[3].role(), Some(Role::MenuItem));
    assert_eq!(entries[3].label(), Some("Documentation"));
    assert_eq!(entries[4].role(), None, "a separator");
    assert!(entries[4].bounds().size.height < px(2.), "a one-pixel line");
    assert_eq!(entries[5].role(), Some(Role::MenuItem));
    assert_eq!(entries[5].label(), Some("About tasklens…"));
    assert_eq!(
        hint_for(&mut r, &OpenShortcuts).as_deref(),
        Some("Ctrl+?"),
        "the live hint"
    );
    assert_eq!(hint_for(&mut r, &ShowSystemInformation), None);
    assert_eq!(hint_for(&mut r, &OpenAbout), None);
    assert_eq!(
        hint_for(&mut r, &OpenAdditionalSetup { focus_row: None }),
        None
    );
}

#[gpui_kit::test]
fn help_sysinfo_dispatch(cx: &mut TestAppContext) {
    let r = rig(cx);
    let count = Rc::new(Cell::new(0));
    let seen = Rc::clone(&count);
    r.cx.update(|_, cx| {
        cx.on_action(move |_: &ShowSystemInformation, _| seen.set(seen.get() + 1));
    });
    keys(r.cx, "alt-h down enter");
    assert_eq!(count.get(), 1);
    assert_eq!(open_menu(r.cx), None);
}

#[gpui_kit::test]
fn help_about_dispatch(cx: &mut TestAppContext) {
    let r = rig(cx);
    let count = Rc::new(Cell::new(0));
    let seen = Rc::clone(&count);
    r.cx.update(|_, cx| {
        cx.on_action(move |_: &OpenAbout, _| seen.set(seen.get() + 1));
    });
    keys(r.cx, "alt-h end enter");
    assert_eq!(count.get(), 1);
    assert_eq!(open_menu(r.cx), None);
}

#[gpui_kit::test]
fn help_additional_setup_position_and_dispatch(cx: &mut TestAppContext) {
    let r = rig(cx);
    let count = Rc::new(Cell::new(0));
    let seen = Rc::clone(&count);
    r.cx.update(|_, cx| {
        cx.on_action(move |a: &OpenAdditionalSetup, _| {
            assert_eq!(a.focus_row, None);
            seen.set(seen.get() + 1)
        });
    });
    keys(r.cx, "alt-h");
    let entries = entries(r.cx);
    assert_eq!(entries[1].label(), Some("System Information…"));
    assert_eq!(entries[2].label(), Some("Additional setup…"));
    assert_eq!(entries[3].label(), Some("Documentation"));
    assert_eq!(entries[4].role(), None, "a separator follows");
    keys(r.cx, "down down enter");
    assert_eq!(count.get(), 1);
    assert_eq!(open_menu(r.cx), None);
}

// View > Window: Performance mini window and View > Graphs: keep detached (E21-034).

/// The built entries of the open View > Window submenu (root index 2).
fn window_entries(cx: &mut VisualTestContext) -> Vec<tl_ui::components::menu::MenuEntryInfo> {
    let bar = bar(cx);
    cx.update(|window, cx| {
        let popup = bar.read(cx).popup().expect("menu open").clone();
        let root = popup.read(cx).entries_for_test(window);
        assert_eq!(root[2].label.as_deref(), Some("Window"));
        let sub = root[2].submenu.clone().expect("Window submenu");
        sub.read(cx).entries_for_test(window)
    })
}

struct MiniStub;

impl Render for MiniStub {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[gpui_kit::test]
fn mini_window_item_check_follows_registry(cx: &mut TestAppContext) {
    use tl_ui::window::registry::{GlobalRegistry, WindowKey};
    let mut r = rig(cx);
    set_kwin(&mut r, tl_model::validity::Validity::Valid);
    assert!(
        !r.cx.update(|_, cx| menus_view::mini_window_open(cx)),
        "no registry means closed"
    );
    r.cx.update(|_, cx| GlobalRegistry::init(cx));
    assert!(!r.cx.update(|_, cx| menus_view::mini_window_open(cx)));
    keys(r.cx, "alt-v down down right");
    let item = |cx: &mut VisualTestContext| {
        window_entries(cx)
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Performance mini window"))
            .expect("mini window item")
    };
    let e = item(r.cx);
    assert!(!e.checked, "unchecked while closed");
    assert_eq!(e.hint.as_deref(), Some("Ctrl+Shift+M"));
    keys(r.cx, "escape escape");
    assert_eq!(open_menu(r.cx), None);

    r.cx.update(|_, cx| {
        let reg = cx.global::<GlobalRegistry>().0.clone();
        reg.update(cx, |reg, cx| {
            reg.open_or_focus(WindowKey::Mini, cx, |cx| {
                cx.open_window(Default::default(), |_, cx| cx.new(|_| MiniStub))
                    .map(Into::into)
            })
        });
    });
    r.cx.run_until_parked();
    assert!(r.cx.update(|_, cx| menus_view::mini_window_open(cx)));
    keys(r.cx, "alt-v down down right");
    assert!(item(r.cx).checked, "checked while open");
    keys(r.cx, "escape escape");

    r.cx.update(|_, cx| {
        let reg = cx.global::<GlobalRegistry>().0.clone();
        reg.update(cx, |reg, cx| reg.close(&WindowKey::Mini, cx));
    });
    r.cx.run_until_parked();
    keys(r.cx, "alt-v down down right");
    assert!(!item(r.cx).checked, "unchecked after the window closes");
}

#[gpui_kit::test]
fn mini_window_item_dispatch(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    set_kwin(&mut r, tl_model::validity::Validity::Valid);
    let count = Rc::new(Cell::new(0));
    let seen = Rc::clone(&count);
    r.cx.update(|_, cx| {
        cx.on_action(move |_: &tl_ui::actions::ToggleMiniWindow, _| seen.set(seen.get() + 1));
    });
    keys(r.cx, "alt-v down down right down enter");
    assert_eq!(count.get(), 1);
    assert_eq!(open_menu(r.cx), None);
}

#[gpui_kit::test]
fn graphs_menu_is_wide_and_separator_is_a_visible_hairline(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    set_gpus(&mut r, 1);
    open_graphs(r.cx);
    let scale = r.cx.update(|window, _| window.scale_factor());
    let seps: Vec<_> = entries(r.cx)
        .into_iter()
        .filter(|e| e.role().is_none() && e.label().is_none())
        .collect();
    assert!(!seps.is_empty(), "the Graphs menu has a separator");
    for sep in &seps {
        let b = sep.bounds();
        assert!(b.size.width > px(50.), "a separator spans the menu: {b:?}");
        assert!(
            (b.size.height - px(1. / scale)).abs() < px(0.01),
            "one physical pixel high: {b:?}"
        );
    }
    let widest =
        r.cx.update(|window, _| snapshots(window))
            .into_iter()
            .filter(|s| s.role() == Some(Role::Menu))
            .map(|s| s.bounds().size.width)
            .fold(px(0.), |a, b| a.max(b));
    assert!(widest > px(400.), "the Graphs submenu widens: {widest:?}");
}

#[gpui_kit::test]
fn keep_detached_toggle(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    set_gpus(&mut r, 1);
    let keep = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| TlSettings::settings(cx).graphs.keep_detached_on_close)
    };
    assert!(!keep(r.cx), "off by default");
    open_graphs(r.cx);
    keys(r.cx, "up up");
    assert_eq!(
        sub_highlighted(r.cx),
        ["Keep detached graphs open when closing main window"]
    );
    keys(r.cx, "enter");
    assert!(keep(r.cx), "the setting flipped");
}

// File > Open System Monitor and Help > Documentation (E21-039).

/// Sets the probe result, registers the handlers and installs the action tracker.
fn set_up_launchers(r: &mut Rig<'_>, monitor: Option<&'static str>) {
    let engine: Arc<dyn EngineClient> = r.engine.clone();
    r.cx.update(|_, cx| {
        cx.set_global(SystemMonitor(monitor));
        menus_file::init(cx);
        menus_help::init(cx);
        tl_ui::action_tracker::install(engine, cx);
    });
}

/// The `Action` requests of the given kind that the fake engine received.
fn action_requests(r: &Rig<'_>) -> Vec<tl_model::actions::ActionRequest> {
    r.engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::Action(req) => Some(req),
            _ => None,
        })
        .collect()
}

fn fail_action(r: &mut Rig<'_>, id: tl_model::actions::ActionId) {
    r.engine.emit(ActionEvent {
        id,
        target: None,
        state: ActionState::Failed(ActionError::Os(2)),
    });
    r.cx.run_until_parked();
}

#[gpui_kit::test]
fn system_monitor_hidden_when_absent(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    set_up_launchers(&mut r, None);
    click_file(r.cx);
    assert_eq!(entries(r.cx).len(), 4, "no Open System Monitor item");
    keys(r.cx, "escape");

    set_up_launchers(&mut r, Some("plasma-systemmonitor"));
    click_file(r.cx);
    let entries = entries(r.cx);
    assert_eq!(entries.len(), 5, "{entries:?}");
    assert_eq!(entries[1].label(), Some("Search File Handles..."));
    assert_eq!(entries[2].role(), Some(Role::MenuItem));
    assert_eq!(entries[2].label(), Some("Open System Monitor"));
    assert_eq!(entries[3].role(), None, "a separator");
    assert_eq!(entries[4].role(), Some(Role::MenuItem));
}

#[gpui_kit::test]
fn system_monitor_launch(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    set_up_launchers(&mut r, Some("plasma-systemmonitor"));
    keys(r.cx, "f10 down down enter");
    r.cx.run_until_parked();
    let sent = action_requests(&r);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let ActionKind::Launch(spec) = &sent[0].kind else {
        panic!("not a Launch: {:?}", sent[0].kind)
    };
    assert!(matches!(&spec.what, LaunchWhat::Command(c) if &**c == "plasma-systemmonitor"));
    assert!(!spec.as_admin);
    assert!(sent[0].targets.is_empty());
}

#[gpui_kit::test]
fn system_monitor_failure_toast(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    set_up_launchers(&mut r, Some("plasma-systemmonitor"));
    keys(r.cx, "f10 down down enter");
    r.cx.run_until_parked();
    let sent = action_requests(&r);
    assert_eq!(sent.len(), 1);
    fail_action(&mut r, sent[0].id);
    let toasts = toast_texts(&mut r);
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert!(
        toasts[0].starts_with("Failed to launch System Monitor"),
        "{toasts:?}"
    );
}

#[gpui_kit::test]
fn documentation_open_uri_and_failure(cx: &mut TestAppContext) {
    let mut r = rig_with_toasts(cx);
    set_up_launchers(&mut r, None);
    keys(r.cx, "alt-h down down down enter");
    r.cx.run_until_parked();
    let sent = action_requests(&r);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        matches!(&sent[0].kind, ActionKind::OpenUri(u) if &**u == "file:///usr/share/doc/tasklens/README.md"),
        "{:?}",
        sent[0].kind
    );
    assert!(sent[0].targets.is_empty());
    fail_action(&mut r, sent[0].id);
    let toasts = toast_texts(&mut r);
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert!(
        toasts[0].starts_with("Could not open the documentation"),
        "{toasts:?}"
    );
}
