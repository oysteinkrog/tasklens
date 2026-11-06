//! Compact view through the main window (tasklens-qdn1r): F11 and View > Compact view turn it
//! on, which hides the menu bar, the tab strip and the Performance sidebar and shows the
//! Performance tab. F11, the exit button and the page menu entry turn it off, and the sidebar
//! comes back as it was. `state.window.compact_view` keeps it across a restart.
//!
//! The thin status line of compact view (tasklens-i5cj4): on by default, with its own exit
//! button; `performance.compact_status_line` turns it off from Settings, the View menu and the
//! page menu, and the hover exit button then takes over.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement, Modifiers,
    MouseButton, MouseDownEvent, MouseExitEvent, MouseUpEvent, Render, TestAppContext,
    VisualTestContext, Window, div, point, px, size,
};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
use tl_ui::window::main::{
    self as shell, MainWindow, PageSlot, main_window_view, open_main_window, set_test_page,
};
use tl_ui::window::menubar::menu_bar_for;
use tl_ui::window::titlebar::TitleBarProbe;
use tl_ui_base::demand::{DemandAggregator, GlobalDemand};
use tl_ui_base::hub::{GlobalHub, SnapshotHub};
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::{SettingKey, TlSettings};
use tl_ui_kit::actions::{SelectTab, ToggleCompactView, ToggleSetting};
use tl_ui_kit::components::kv_grid;
use tl_ui_kit::components::menu::{TlMenuPopup, open_context_menu};
use tl_ui_perf::performance::overview::OVERVIEW_KEY;
use tl_ui_perf::performance::status_bar::{BAR_ID, THIN_EXIT_ID};
use tl_ui_perf::performance::{COMPACT_EXIT_ID, PAGE_PANEL_ID, PerformancePage};
use tl_ui_settings::settings::{
    AppearanceSettings, DeviceVisibility, Settings, ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("device"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
        hot_c: None,
    }
}

struct FakeProcesses(FocusHandle);

impl gpui_kit::EventEmitter<tl_ui::window::status::StatusChanged> for FakeProcesses {}

impl Render for FakeProcesses {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::{InteractiveElement as _, Styled as _};
        div().track_focus(&self.0).size_full()
    }
}

impl shell::ShellPage for FakeProcesses {
    fn primary_focus(&self) -> FocusHandle {
        self.0.clone()
    }
}

struct Rig<'a> {
    main: Entity<MainWindow>,
    /// `state.window.compact_view` of every persisted state.
    persisted: Rc<RefCell<Vec<bool>>>,
    /// `performance.compact_status_line` of every persisted settings file.
    persisted_line: Rc<RefCell<Vec<bool>>>,
    vcx: &'a mut VisualTestContext,
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

/// The main window at 1280 × 800 from `state`, under client decorations when `client`, with
/// reduced motion so the sidebar and the exit button change at once. The Processes tab is a
/// fake page and the Overview card is hidden (D-022), so the Performance tab opens on CPU.
/// Compact view's status line is on, as by default.
fn open(cx: &mut TestAppContext, state: UiState, client: bool) -> Rig<'_> {
    open_with(cx, state, client, true)
}

/// [`open`] with `performance.compact_status_line` set to `line`.
fn open_with(cx: &mut TestAppContext, state: UiState, client: bool, line: bool) -> Rig<'_> {
    let mut settings = Settings::default();
    settings.performance.compact_status_line = line;
    // Instant tab switches (E20-021).
    settings.motion.tab_switch_ms = 0;
    settings
        .performance
        .device_overrides
        .insert(OVERVIEW_KEY.to_owned(), DeviceVisibility::Hide);
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![
                entry(0, DeviceIdentity::CpuPackage),
                entry(1, DeviceIdentity::Memory),
            ]),
        });
    });
    let client_arc: Arc<dyn EngineClient> = engine;
    let persisted: Rc<RefCell<Vec<bool>>> = Rc::default();
    let persisted_line: Rc<RefCell<Vec<bool>>> = Rc::default();
    let seen = Rc::clone(&persisted);
    let seen_line = Rc::clone(&persisted_line);
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_reduce_motion(true);
        tl_ui_base::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client_arc)));
        cx.set_global(BootSettings {
            settings,
            state,
            persist: Some(Box::new(move |settings, state| {
                seen.borrow_mut().push(state.window.compact_view);
                seen_line
                    .borrow_mut()
                    .push(settings.performance.compact_status_line);
            })),
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client_arc), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client_arc, &hub, cx));
        cx.set_global(GlobalHub(hub));
        cx.set_global(GlobalDemand(demand));
        tl_ui_kit::components::toast::init(cx);
        tl_ui_kit::actions::bind_keys_global(cx);
        tl_ui_kit::actions::register_global_handlers(cx);
        tl_ui_kit::components::menu::bind_keys(cx);
        tl_ui_perf::performance::actions::bind_keys_performance(cx);
        cx.set_global(StartOptions::default());
        set_test_page(cx, Tab::Processes, |_, cx: &mut Context<MainWindow>| {
            let page = cx.new(|cx| FakeProcesses(cx.focus_handle().tab_stop(true)));
            PageSlot::new(page, cx)
        });
        cx.set_global(TitleBarProbe {
            client: Some(client),
            ..TitleBarProbe::default()
        });
        tl_ui::window::menubar::init(cx);
        shell::init(cx);
    });
    let handle = cx.update(open_main_window).expect("the main window opens");
    let vcx = VisualTestContext::from_window(handle.into(), cx).into_mut();
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1280.), px(800.)));
    vcx.update(|window, _| window.activate_window());
    draw(vcx);
    let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
    Rig {
        main,
        persisted,
        persisted_line,
        vcx,
    }
}

fn line_on(r: &mut Rig) -> bool {
    r.vcx
        .update(|_, cx| TlSettings::settings(cx).performance.compact_status_line)
}

fn compact(r: &mut Rig) -> bool {
    let main = r.main.clone();
    let shown = r.vcx.update(|_, cx| main.read(cx).is_compact());
    let saved = r
        .vcx
        .update(|_, cx| TlSettings::state(cx).window.compact_view);
    assert_eq!(shown, saved, "the window follows the saved value");
    shown
}

fn active(r: &mut Rig) -> Tab {
    let main = r.main.clone();
    r.vcx.update(|_, cx| main.read(cx).active_tab())
}

fn perf(r: &mut Rig) -> Entity<PerformancePage> {
    let main = r.main.clone();
    r.vcx
        .update(|_, cx| {
            main.read(cx)
                .page_entity::<PerformancePage>(Tab::Performance)
        })
        .expect("PerformancePage built")
}

fn sidebar_visible(r: &mut Rig) -> bool {
    let page = perf(r);
    r.vcx.update(|_, cx| page.read(cx).sidebar_visible())
}

fn drawn(r: &mut Rig, selector: &'static str) -> bool {
    r.vcx.debug_bounds(selector).is_some()
}

fn press(r: &mut Rig, keys: &str) {
    for key in keys.split(' ') {
        r.vcx.simulate_keystrokes(key);
        draw(r.vcx);
    }
}

/// Normal view: menu bar, tab strip and sidebar drawn.
fn assert_normal(r: &mut Rig) {
    assert!(!compact(r));
    assert!(drawn(r, "TitleBarMenu"), "menu bar");
    assert!(drawn(r, "TabStrip"), "tab strip");
    if active(r) == Tab::Performance {
        assert!(sidebar_visible(r), "sidebar");
    }
}

/// Compact view under client decorations: the title bar without menus, no tab strip, the
/// Performance tab without its sidebar, its page right of the expand strip and under the bar,
/// or under the 20 px status line while that is on.
fn assert_compact(r: &mut Rig) {
    assert!(compact(r));
    assert_eq!(active(r), Tab::Performance);
    assert!(drawn(r, "TitleBar"), "the title bar stays");
    assert!(!drawn(r, "TitleBarMenu"), "no menu bar");
    assert!(!drawn(r, "TabStrip"), "no tab strip");
    assert!(!sidebar_visible(r), "no sidebar");
    let line = if line_on(r) {
        let bar = r.vcx.debug_bounds(BAR_ID).expect("the status line");
        assert_eq!(bar.top(), px(34.), "the line right under the title bar");
        assert!(bar.size.height <= px(22.), "{:?}", bar.size);
        bar.size.height
    } else {
        assert!(!drawn(r, BAR_ID), "no status line");
        px(0.)
    };
    let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page panel");
    assert_eq!(
        page.top(),
        px(34.) + line,
        "right under the title bar or the line"
    );
    assert_eq!(page.bottom(), px(800.), "to the bottom");
    assert_eq!(page.left(), px(12.), "right of the expand strip");
    assert_eq!(page.right(), px(1280.));
}

/// F11 on another tab: compact view shows the Performance tab without the menu bar, the tab
/// strip and the sidebar, and saves `state.window.compact_view`.
#[gpui_kit::test]
fn f11_enters_compact_view(cx: &mut TestAppContext) {
    let mut r = open(cx, UiState::default(), true);
    assert_eq!(active(&mut r), Tab::Processes);
    assert_normal(&mut r);

    press(&mut r, "f11");
    assert_compact(&mut r);
    assert_eq!(
        r.persisted.borrow().last(),
        Some(&true),
        "state.toml written"
    );
    assert!(
        !r.vcx
            .update(|_, cx| TlSettings::state(cx).performance.sidebar_hidden),
        "the saved sidebar state is not touched"
    );
}

/// F11 again: normal view with the sidebar shown as before; with a sidebar hidden before,
/// hidden again.
#[gpui_kit::test]
fn f11_exits_with_the_previous_sidebar_state(cx: &mut TestAppContext) {
    let mut r = open(cx, UiState::default(), true);
    r.vcx.dispatch_action(SelectTab(Tab::Performance as usize));
    draw(r.vcx);
    press(&mut r, "f11");
    assert_compact(&mut r);
    press(&mut r, "f11");
    assert_normal(&mut r);
    assert_eq!(active(&mut r), Tab::Performance, "the tab stays");
    assert_eq!(r.persisted.borrow().last(), Some(&false));

    // The sidebar hidden before compact view stays hidden after it.
    press(&mut r, "f9");
    assert!(!sidebar_visible(&mut r));
    press(&mut r, "f11");
    assert_compact(&mut r);
    press(&mut r, "f11");
    assert!(!compact(&mut r));
    assert!(!sidebar_visible(&mut r), "hidden, as before");
}

/// Without the status line, the exit button in the top-right corner: none until the mouse
/// moves, and a click on it ends compact view.
#[gpui_kit::test]
fn exit_button_ends_compact_view(cx: &mut TestAppContext) {
    let mut r = open_with(cx, UiState::default(), true, false);
    press(&mut r, "f11");
    assert_compact(&mut r);
    assert!(!drawn(&mut r, COMPACT_EXIT_ID), "no button before a move");
    r.vcx
        .simulate_mouse_move(point(px(600.), px(400.)), None, Modifiers::none());
    draw(r.vcx);
    let b = r.vcx.debug_bounds(COMPACT_EXIT_ID).expect("exit button");
    assert_eq!((b.top(), b.right()), (px(34. + 8.), px(1280. - 8.)));
    r.vcx.simulate_click(b.center(), Modifiers::none());
    draw(r.vcx);
    assert_normal(&mut r);
    assert!(!drawn(&mut r, COMPACT_EXIT_ID));
}

/// Without the status line, the exit button follows the mouse over the whole window: a move
/// over the client title bar, which compact view keeps, shows it, and leaving the window from
/// there hides it.
#[gpui_kit::test]
fn exit_button_shows_on_a_title_bar_move(cx: &mut TestAppContext) {
    let mut r = open_with(cx, UiState::default(), true, false);
    press(&mut r, "f11");
    assert_compact(&mut r);
    assert!(!drawn(&mut r, COMPACT_EXIT_ID), "no button before a move");
    // The middle of the 34 px client title bar, clear of the caption buttons.
    let on_bar = point(px(600.), px(17.));
    r.vcx.simulate_mouse_move(on_bar, None, Modifiers::none());
    draw(r.vcx);
    assert!(drawn(&mut r, COMPACT_EXIT_ID), "a title bar move shows it");
    r.vcx.simulate_event(MouseExitEvent {
        position: on_bar,
        pressed_button: None,
        modifiers: Modifiers::none(),
    });
    draw(r.vcx);
    assert!(
        !drawn(&mut r, COMPACT_EXIT_ID),
        "leaving the window from the title bar hides it"
    );
}

/// A right-click on the page in compact view offers "Exit compact view", which ends it.
#[gpui_kit::test]
fn page_menu_entry_ends_compact_view(cx: &mut TestAppContext) {
    let mut r = open(cx, UiState::default(), true);
    press(&mut r, "f11");
    let at = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page").center();
    r.vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    r.vcx.simulate_event(MouseUpEvent {
        button: MouseButton::Right,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    draw(r.vcx);
    let menu: Entity<TlMenuPopup> = r
        .vcx
        .update(|window, cx| open_context_menu(window, cx))
        .expect("the page menu is open");
    let last = r.vcx.update(|window, cx| {
        menu.read(cx)
            .entries_for_test(window)
            .pop()
            .expect("entries")
    });
    assert_eq!(last.label.as_deref(), Some("Exit compact view"));
    let action = last.action.expect("an action");
    assert!(action.partial_eq(&ToggleCompactView));
    // The keyboard path through the open menu: Up wraps to the last item, Enter runs it.
    press(&mut r, "up enter");
    assert_normal(&mut r);
}

/// Compact view saved: the next start opens the Performance tab in compact view.
#[gpui_kit::test]
fn compact_view_restored_at_start(cx: &mut TestAppContext) {
    let mut state = UiState::default();
    state.window.compact_view = true;
    state.tabs.last_tab = Tab::Processes;
    let mut r = open(cx, state, true);
    assert_compact(&mut r);
    press(&mut r, "f11");
    assert_normal(&mut r);
}

/// Under server decorations compact view leaves out the whole menu row: the status line is at
/// the top of the window, at most 22 px high, with the page under it.
#[gpui_kit::test]
fn server_decorations_drop_the_menu_row(cx: &mut TestAppContext) {
    let mut r = open(cx, UiState::default(), false);
    assert!(drawn(&mut r, "TitleBar"), "the menu row");
    press(&mut r, "f11");
    assert!(compact(&mut r));
    assert!(!drawn(&mut r, "TitleBar"), "no menu row");
    assert!(!drawn(&mut r, "TabStrip"));
    let bar = r.vcx.debug_bounds(BAR_ID).expect("the status line");
    assert_eq!(bar.top(), px(0.));
    assert!(bar.size.height <= px(22.), "{:?}", bar.size);
    let exit = r.vcx.debug_bounds(THIN_EXIT_ID).expect("its exit button");
    assert!(
        exit.right() <= px(1280.) && exit.right() >= px(1272.),
        "{exit:?}"
    );
    let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page panel");
    assert_eq!(page.top(), bar.bottom());
    press(&mut r, "f11");
    assert!(drawn(&mut r, "TitleBar"), "back");
}

/// With the status line off, compact view under server decorations draws no line and the page
/// starts at the top edge of the window (tasklens-i5cj4).
#[gpui_kit::test]
fn status_line_off_page_at_the_top_edge(cx: &mut TestAppContext) {
    let mut r = open_with(cx, UiState::default(), false, false);
    press(&mut r, "f11");
    assert!(compact(&mut r));
    assert!(!drawn(&mut r, BAR_ID), "no status line");
    assert!(!drawn(&mut r, THIN_EXIT_ID));
    let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page panel");
    assert_eq!(page.top(), px(0.));
}

/// The line's exit button is drawn without a mouse move, and a click on it ends compact view.
#[gpui_kit::test]
fn status_line_exit_button_ends_compact_view(cx: &mut TestAppContext) {
    let mut r = open(cx, UiState::default(), true);
    press(&mut r, "f11");
    assert_compact(&mut r);
    let b = r
        .vcx
        .debug_bounds(THIN_EXIT_ID)
        .expect("the line's exit button");
    assert!(!drawn(&mut r, COMPACT_EXIT_ID), "no hover button");
    r.vcx.simulate_click(b.center(), Modifiers::none());
    draw(r.vcx);
    assert_normal(&mut r);
    assert!(!drawn(&mut r, THIN_EXIT_ID));
}

/// View > "Show status line in compact view" in compact view: Alt+V shows the menu row with
/// the View menu open under its button. The item is a check from
/// `performance.compact_status_line` that runs `ToggleSetting` for it. Off, the line goes and
/// the setting is saved; the row hides again when the menu closes (tasklens-i5cj4).
#[gpui_kit::test]
fn view_menu_status_line_toggle_is_saved(cx: &mut TestAppContext) {
    let mut r = open(cx, UiState::default(), true);
    press(&mut r, "f11");
    assert_compact(&mut r);
    let label = "Show status line in compact view";
    let item = view_item(&mut r, label).expect("in the View menu");
    assert_view_menu_shown(&mut r);
    assert!(item.checked, "on by default");
    assert!(item.action.as_ref().is_some_and(|a| a.partial_eq(&ToggleSetting(
        SettingKey::PerformanceCompactStatusLine
    ))));
    // Run it from the open menu: "Show sidebar" is last, this item is above it.
    press(&mut r, "end up enter");
    assert!(!line_on(&mut r), "off");
    assert_eq!(
        r.persisted_line.borrow().last(),
        Some(&false),
        "settings.toml written"
    );
    assert_compact(&mut r);
    assert!(!drawn(&mut r, BAR_ID), "no line in compact view");
    let item = view_item(&mut r, label).expect("still listed");
    assert_view_menu_shown(&mut r);
    assert!(!item.checked, "unchecked while off");
    press(&mut r, "end up enter");
    assert!(line_on(&mut r), "on again");
    assert_compact(&mut r);
}

/// Compact view under server decorations has no menu row. Alt+V brings it back with the View
/// menu open, and the row goes again when the menu closes (tasklens-i5cj4).
#[gpui_kit::test]
fn view_menu_opens_in_compact_view_under_server_decorations(cx: &mut TestAppContext) {
    let mut r = open(cx, UiState::default(), false);
    press(&mut r, "f11");
    assert!(compact(&mut r));
    assert!(!drawn(&mut r, "TitleBar"), "no menu row");
    let item = view_item(&mut r, "Show status line in compact view").expect("listed");
    assert!(item.checked);
    assert_view_menu_shown(&mut r);
    press(&mut r, "end up enter");
    assert!(!line_on(&mut r), "off");
    assert!(compact(&mut r));
    assert!(!drawn(&mut r, "TitleBar"), "the row goes with the menu");
    assert!(!drawn(&mut r, BAR_ID), "no line");
    let page = r.vcx.debug_bounds(PAGE_PANEL_ID).expect("page panel");
    assert_eq!(page.top(), px(0.));
}

/// The View menu is open and drawn: its menu button shows and its popup holds focus, which a
/// popup that is not drawn loses.
fn assert_view_menu_shown(r: &mut Rig) {
    assert!(
        drawn(r, "TlMenuBarButton-shell-menubar-view"),
        "the menu row shows while the menu is open"
    );
    let bar = r
        .vcx
        .update(|window, cx| menu_bar_for(window.window_handle().window_id(), cx))
        .expect("menu bar");
    let focused = r.vcx.update(|window, cx| {
        let popup = bar.read(cx).popup().expect("View open").clone();
        popup.focus_handle(cx).is_focused(window)
    });
    assert!(focused, "the popup is drawn and focused");
}

/// The saved choice at start: compact view without its line comes back without it.
#[gpui_kit::test]
fn status_line_off_restored_at_start(cx: &mut TestAppContext) {
    let mut state = UiState::default();
    state.window.compact_view = true;
    let mut r = open_with(cx, state, true, false);
    assert_compact(&mut r);
    assert!(!drawn(&mut r, BAR_ID));
    r.vcx
        .dispatch_action(ToggleSetting(SettingKey::PerformanceCompactStatusLine));
    draw(r.vcx);
    assert_compact(&mut r);
    assert!(drawn(&mut r, BAR_ID), "turned on again");
}

/// Opens the View menu with Alt+V and returns the item labelled `label`, if any. The menu
/// stays open.
fn view_item(r: &mut Rig, label: &str) -> Option<tl_ui_kit::components::menu::MenuEntryInfo> {
    press(r, "alt-v");
    let bar = r
        .vcx
        .update(|window, cx| menu_bar_for(window.window_handle().window_id(), cx))
        .expect("menu bar");
    r.vcx.update(|window, cx| {
        let popup = bar.read(cx).popup().expect("View open").clone();
        popup
            .read(cx)
            .entries_for_test(window)
            .into_iter()
            .find(|e| e.label.as_deref() == Some(label))
    })
}

/// A jump or a key that shows another tab ends compact view, so that tab has its tab strip.
#[gpui_kit::test]
fn another_tab_ends_compact_view(cx: &mut TestAppContext) {
    let mut r = open(cx, UiState::default(), true);
    press(&mut r, "f11");
    assert_compact(&mut r);
    r.vcx.dispatch_action(SelectTab(Tab::Processes as usize));
    draw(r.vcx);
    assert_eq!(active(&mut r), Tab::Processes);
    assert_normal(&mut r);
}

/// View > Compact view: a check from `state.window.compact_view` that runs `ToggleCompactView`.
#[gpui_kit::test]
fn view_menu_compact_view_check(cx: &mut TestAppContext) {
    let mut r = open(cx, UiState::default(), true);
    press(&mut r, "alt-v");
    let bar = r
        .vcx
        .update(|window, cx| menu_bar_for(window.window_handle().window_id(), cx))
        .expect("menu bar");
    let item = r.vcx.update(|window, cx| {
        let popup = bar.read(cx).popup().expect("View open").clone();
        popup
            .read(cx)
            .entries_for_test(window)
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Compact view"))
            .expect("Compact view item")
    });
    assert!(!item.checked, "unchecked in normal view");
    assert_eq!(item.hint.as_deref(), Some("F11"), "the key hint");
    assert!(
        item.action
            .as_ref()
            .is_some_and(|a| a.partial_eq(&ToggleCompactView))
    );
    // Third from the end, above "Show status line in compact view" and "Show sidebar".
    press(&mut r, "end up up enter");
    assert_compact(&mut r);
    // Left alone, nothing changes after a while.
    r.vcx.executor().advance_clock(Duration::from_secs(3));
    draw(r.vcx);
    assert!(compact(&mut r));
}
