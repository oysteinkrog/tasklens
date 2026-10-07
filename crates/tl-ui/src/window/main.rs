//! The main window: layout, the page match, the shell event bus and state, opening the window
//! and the first-frame hook (E20-008, UI-SHL-001, UI-SHL-002, UI-SHL-020, UI-SHL-100,
//! UI-SHL-110 steps 9 to 11, UI-SHL-111, UI-SHL-121, ADR-009, ADR-010, ADR-014, R-022, R-023,
//! R-107).
//!
//! Top to bottom: the title bar, the tab strip and the page area. The page area holds the active
//! page and the floating status chip at its bottom-left. Each part is a cached view, so a page
//! that redraws does not redraw the shell and the shell does not redraw the page.
//!
//! Pages are built on their first visit through [`page_factory`] and kept for the window's life
//! (ADR-009). A tab whose page has no arm in the match is not shown (R-023). Each page epic adds
//! its arm, its `impl ShellPage` and, when it has page buttons, its [`toolbar_for`] arm.
//!
//! The close decision, the window actions, reopening and size saving are E20-A1 (UI-SHL-002,
//! UI-SHL-005, UI-SHL-112, UI-SHL-113, APP-045). Every quit goes through
//! `request_exit(cx, UiExit::Quit)`; `tl_ui::run` sets `QuitMode::Explicit`, so removing the last
//! window does not quit (ADR-010).
//!
//! The process-view Actions `ToggleHideSystem`, `CycleViewMode` and `SetViewMode` are app-level
//! handlers here (E20-017, UI-SHL-012, UI-SHL-013, UI-SHL-035, PROC-015, PROC-016).

use std::mem;
use std::time::Duration;

use fluent_bundle::FluentValue;
use gpui_kit::base::Root;
use gpui_kit::component::{TitleBar, v_flex};
use gpui_kit::{
    AnyView, App, AppContext as _, AsyncApp, Bounds, Context, Entity, EntityId, EventEmitter,
    FocusHandle, Global, InteractiveElement, IntoElement, ModifiersChangedEvent, ParentElement,
    Pixels, Render, SharedString, StyleRefinement, Styled, Subscription, Task, WeakEntity, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle, WindowOptions, div,
    point, px, size,
};
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::settings::{Decorations, StartTab, WindowState};
use tl_model::table::TableMode;
use tl_model::time::now_boot_ns;
use tracing::{debug, error, info, warn};

use super::filter::{FilterBox, ReturnFocus, shown_on};
use super::registry::GlobalRegistry;
use super::status::{CHIP_H, CHIP_RESERVE, StatusAction, StatusChanged, StatusChip};
use super::tabstrip::TabStrip;
#[cfg(any(test, feature = "test-hooks"))]
use super::titlebar::TitleBarProbe;
use super::titlebar::{TitleBarView, bar_height_of, is_client, minimize_main, zoom};
use super::toolbar::{DefaultToolbar, ToolbarContent, ToolbarModel};
use crate::actions::{
    CancelWindowPick, ClearFilter, CloseWindow, CycleViewMode, EscapeChain, FocusFilter,
    GLOBAL_CONTEXT, MinimizeWindow, NextTab, PrevTab, Quit, RefreshCurrentTab, ScaleDown,
    ScaleReset, ScaleUp, SelectTab, SetViewMode, ShowMainWindow, ToggleHideSystem,
    ToggleMainWindow, ZoomWindow,
};
use crate::anim::AnimationDriver;
use crate::app::{OpenHooks, StartOptions, UiExit, request_exit};
use crate::components::menu::context_menu_layer;
use crate::components::toast::{ToastAction, ToastKey, ToastKind, ToastQueue};
use crate::demand::GlobalDemand;
use crate::focus::{EscOutcome, EscState, esc_outcome, focus_initial};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::{Strings, tr_args, tr_en};
use crate::pages::performance::details::Template;
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::toolbar::ProcessesToolbar;
use crate::settings_global::{Section, TlSettings};
use crate::theme::store::apply_metrics;
use crate::theme::{ThemeStore, UiMetrics, bump_style_version, current, metrics};

pub use super::tabstrip::TAB_STRIP_H;
pub use super::titlebar::TITLEBAR_H;

/// The window's smallest size in logical px (ADR-010).
pub const MIN_W: f32 = 800.0;
pub const MIN_H: f32 = 500.0;
/// The Wayland app id and X11 class; matches the desktop file.
pub const APP_ID: &str = "io.github.oysteinkrog.Tasklens";
/// The window title the KWin task switcher shows.
pub const WINDOW_TITLE: &str = "tasklens";
/// Gap between the tab strip and the toast stack's top edge, × ui_scale (UI-SHL-100).
const TOAST_GAP: f32 = 8.0;
/// The toast stack's top edge (UI-SHL-100): the bar height for the decoration kind, the tab
/// strip and an 8 px gap, × ui_scale. 72 under client decorations and 66 under server ones, at
/// 100 %.
fn toast_top(client: bool, s: f32) -> Pixels {
    bar_height_of(client, s) + px((TAB_STRIP_H + TOAST_GAP) * s)
}

/// The UI scale toast's key. Fixed, so repeated presses replace one toast (UI-SHL-130).
pub const UI_SCALE_TOAST: ToastKey = ToastKey(0x7569_7363_616c_6531);
/// "UI scale changed to { $pct }%".
const UI_SCALE_TEXT: &str = "shell-toast-ui-scale";
/// The UI scale keys' step and range, in percent (APP-040).
const HIDING_SYSTEM_TEXT: &str = "shell-toast-hiding-system";
const SHOWING_SYSTEM_TEXT: &str = "shell-toast-showing-system";
const VIEW_MODE_TEXT: &str = "shell-toast-view-mode";
const VIEW_MODE_TOOLTIP: &str = "shell-toolbar-view-mode";

const SCALE_STEP: u16 = 5;
const SCALE_MIN: u16 = 75;
const SCALE_MAX: u16 = 200;

/// Quiet time after the last bounds change before the size is written (UI-SHL-002).
const SIZE_SAVE_DELAY: Duration = Duration::from_millis(500);

/// What closing the main window does (UI-SHL-113).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseOutcome {
    Quit,
    /// Remove the window; the app and the engine keep running.
    RemoveWindow,
}

/// Implemented by every page entity (E22, E24, E27, E28). Inherent methods of the same name may
/// exist; the impl forwards to them.
pub trait ShellPage: Render + EventEmitter<StatusChanged> + Sized + 'static {
    /// UI-SHL-050. Default: no text.
    fn status_text(&self, _out: &mut String) {}
    /// Clickable chip parts (E20-006).
    fn status_actions(&self) -> SmallVec<[StatusAction; 2]> {
        SmallVec::new()
    }
    /// UI-SHL-121 initial focus.
    fn primary_focus(&self) -> FocusHandle;
    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        SmallVec::new()
    }
    /// Demand on or off (ADR-008).
    fn set_active(&mut self, _active: bool, _cx: &mut Context<Self>) {}
    /// F5, `RefreshCurrentTab`.
    fn refresh(&mut self, _cx: &mut Context<Self>) {}
}

/// Object-safe forwarding of the [`ShellPage`] methods through the page's `Entity`.
trait PageHandle {
    fn status_text(&self, out: &mut String, cx: &App);
    fn status_actions(&self, cx: &App) -> SmallVec<[StatusAction; 2]>;
    fn primary_focus(&self, cx: &App) -> FocusHandle;
    #[allow(dead_code)]
    fn focus_order(&self, cx: &App) -> SmallVec<[FocusHandle; 4]>;
    fn set_active(&self, active: bool, cx: &mut App);
    fn refresh(&self, cx: &mut App);
}

impl<T: ShellPage> PageHandle for Entity<T> {
    fn status_text(&self, out: &mut String, cx: &App) {
        self.read(cx).status_text(out);
    }

    fn status_actions(&self, cx: &App) -> SmallVec<[StatusAction; 2]> {
        self.read(cx).status_actions()
    }

    fn primary_focus(&self, cx: &App) -> FocusHandle {
        self.read(cx).primary_focus()
    }

    fn focus_order(&self, cx: &App) -> SmallVec<[FocusHandle; 4]> {
        self.read(cx).focus_order()
    }

    fn set_active(&self, active: bool, cx: &mut App) {
        self.update(cx, |page, cx| page.set_active(active, cx));
    }

    fn refresh(&self, cx: &mut App) {
        self.update(cx, |page, cx| page.refresh(cx));
    }
}

/// One built page: its view, the forwarding handle and its `StatusChanged` subscription.
pub struct PageSlot {
    view: AnyView,
    handle: Box<dyn PageHandle>,
    _status: Subscription,
}

impl PageSlot {
    /// Wraps `page` and subscribes the main window to its `StatusChanged`.
    pub fn new<T: ShellPage>(page: Entity<T>, cx: &mut Context<MainWindow>) -> Self {
        let _status = cx.subscribe(&page, |this, page, _: &StatusChanged, cx| {
            this.on_page_status(page.entity_id(), cx);
        });
        Self {
            view: page.clone().into(),
            handle: Box::new(page),
            _status,
        }
    }

    fn entity_id(&self) -> EntityId {
        self.view.entity_id()
    }
}

/// Builds one page on its first visit.
pub type PageFactory = fn(&mut Window, &mut Context<MainWindow>) -> PageSlot;

/// The page match. A tab with no arm is not built and not shown (ADR-009, R-023).
fn page_factory(tab: Tab) -> Option<PageFactory> {
    match tab {
        // page arms (chain edits, one per page epic): Processes E22, Performance E24, Services E27,
        // Startup, Connections, Information E28. Form:
        // Tab::Services => Some(|window, cx| { let page = cx.new(|cx| ServicesPage::new(window, cx)); PageSlot::new(page, cx) }),
        Tab::Processes => Some(|window, cx| {
            let page = cx.new(|cx| ProcessesPage::new(window, cx));
            PageSlot::new(page, cx)
        }),
        _ => None,
    }
}

/// Toolbar registration: one arm per page epic with page buttons (E22 for Processes).
fn toolbar_for(tab: Tab) -> Box<dyn ToolbarContent> {
    match tab {
        // toolbar registrations (chain edits)
        Tab::Processes => Box::new(ProcessesToolbar),
        _ => Box::new(DefaultToolbar),
    }
}

/// Fake pages for shell tests. Checked before `page_factory`.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Default)]
struct TestPages([Option<PageFactory>; 6]);

#[cfg(any(test, feature = "test-hooks"))]
impl Global for TestPages {}

/// Builds `tab` with `factory` instead of its page arm.
#[cfg(any(test, feature = "test-hooks"))]
pub fn set_test_page(cx: &mut App, tab: Tab, factory: PageFactory) {
    cx.default_global::<TestPages>().0[tab as usize] = Some(factory);
}

/// The factory for `tab`: a test page if one is set, else the page match.
fn factory_for(tab: Tab, cx: &App) -> Option<PageFactory> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(factory) = cx
        .try_global::<TestPages>()
        .and_then(|pages| pages.0[tab as usize])
    {
        return Some(factory);
    }
    let _ = cx;
    page_factory(tab)
}

/// `Tab::ALL` filtered by the page match, in R-023 order.
fn built_tabs(cx: &App) -> SmallVec<[Tab; 6]> {
    Tab::ALL
        .into_iter()
        .filter(|tab| factory_for(*tab, cx).is_some())
        .collect()
}

/// The shell-wide event bus.
pub struct ShellEvents;

/// Emitted by E20-009 after its 100 ms debounce.
#[derive(Clone, Debug)]
pub struct FilterChanged {
    pub tab: Tab,
    pub text: SharedString,
}

/// Emitted by E20-012.
#[derive(Clone, Copy, Debug)]
pub struct PauseChanged(pub bool);

impl EventEmitter<FilterChanged> for ShellEvents {}
impl EventEmitter<PauseChanged> for ShellEvents {}

pub struct GlobalShellEvents(pub Entity<ShellEvents>);

impl Global for GlobalShellEvents {}

/// Shell state other parts of the UI read.
pub struct ShellState {
    /// `gpui_kit::open_window` roots every window in the kit `Root`, so the handle is typed on
    /// `Root`. `main_view` is the `MainWindow` inside it.
    pub main_window: Option<WindowHandle<Root>>,
    /// Read through [`main_window_view`]. Works inside the window's own updates, where the
    /// window's root cannot be read.
    pub main_view: Option<WeakEntity<MainWindow>>,
    pub active_tab: Tab,
    /// `Tab::ALL` filtered by `page_factory`, R-023 order.
    pub built_tabs: SmallVec<[Tab; 6]>,
    /// Mirror of `UiState.processes.history_mode`.
    pub history_mode: bool,
    /// Ctrl-pause (E20-012).
    pub paused: bool,
}

impl Global for ShellState {}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            main_window: None,
            main_view: None,
            active_tab: Tab::Processes,
            built_tabs: SmallVec::new(),
            history_mode: false,
            paused: false,
        }
    }
}

/// Set once the first frame's `first_frame_ms` is logged, so a reopened window does not log again.
struct FirstFrameLogged;

impl Global for FirstFrameLogged {}

/// Installs `GlobalShellEvents` and `ShellState`, the app-level window actions and the main open
/// hook.
pub fn init(cx: &mut App) {
    let events = cx.new(|_| ShellEvents);
    cx.set_global(GlobalShellEvents(events));
    let history_mode = cx
        .try_global::<TlSettings>()
        .is_some_and(|_| TlSettings::state(cx).processes.history_mode);
    cx.set_global(ShellState {
        history_mode,
        ..ShellState::default()
    });
    cx.default_global::<OpenHooks>().main = Some(Box::new(|cx| {
        // On Linux a renderer failure comes back as an Err from `open_window` (E16-012).
        if let Err(err) = open_first_main(cx) {
            error!(target: "tasklens::ui", %err, "main window did not open");
            request_exit(cx, UiExit::RendererFailed(err));
        }
    }));
    cx.on_action(on_quit);
    // Deferred: from a key in the main window, that window is mid-update and cannot be reached.
    cx.on_action(|_: &ShowMainWindow, cx| cx.defer(show_main_window));
    cx.on_action(|_: &ToggleMainWindow, cx| cx.defer(toggle_main_window));
    cx.on_action(|_: &ScaleUp, cx| step_ui_scale(cx, |pct| (pct + SCALE_STEP).min(SCALE_MAX)));
    cx.on_action(|_: &ScaleDown, cx| {
        step_ui_scale(cx, |pct| pct.saturating_sub(SCALE_STEP).max(SCALE_MIN))
    });
    cx.on_action(|_: &ScaleReset, cx| step_ui_scale(cx, |_| 100));
    cx.on_action(on_toggle_hide_system);
    cx.on_action(|_: &CycleViewMode, cx| {
        let next = next_view_mode(TlSettings::settings(cx).processes.mode);
        set_view_mode(cx, next);
    });
    cx.on_action(|a: &SetViewMode, cx| set_view_mode(cx, a.0));
}

/// Hide system processes: flips `processes.hide_system` and says so in a toast. The engine
/// filters by it, so it also works in History mode.
fn on_toggle_hide_system(_: &ToggleHideSystem, cx: &mut App) {
    let hide = !TlSettings::settings(cx).processes.hide_system;
    TlSettings::update(cx, |d| d.settings.processes.hide_system = hide);
    let key = if hide {
        HIDING_SYSTEM_TEXT
    } else {
        SHOWING_SYSTEM_TEXT
    };
    let text = shell_text(cx, key, &[]);
    push_main_toast(cx, ToastKind::Info, text, None, None);
}

/// The view-mode cycler order: Tree View, Task Manager Grouping, List View, then Tree View.
fn next_view_mode(mode: TableMode) -> TableMode {
    match mode {
        TableMode::Tree => TableMode::Grouped,
        TableMode::Grouped => TableMode::Flat,
        TableMode::Flat => TableMode::Tree,
    }
}

/// Writes `processes.mode` and shows "View mode: <name>". Nothing happens in History mode, which
/// always shows a flat list, or when `mode` is already the current mode.
fn set_view_mode(cx: &mut App, mode: TableMode) {
    if TlSettings::state(cx).processes.history_mode
        || TlSettings::settings(cx).processes.mode == mode
    {
        return;
    }
    TlSettings::update(cx, |d| d.settings.processes.mode = mode);
    let name = shell_text(cx, view_mode_name(mode), &[]);
    let text = shell_text(
        cx,
        VIEW_MODE_TEXT,
        &[("name", FluentValue::from(name.as_ref()))],
    );
    push_main_toast(cx, ToastKind::Info, text, None, None);
}

/// The Fluent key of a view mode's name. The caller translates it.
pub fn view_mode_name(mode: TableMode) -> &'static str {
    match mode {
        TableMode::Tree => "shell-view-mode-tree",
        TableMode::Grouped => "shell-view-mode-grouped",
        TableMode::Flat => "shell-view-mode-flat",
    }
}

/// The view-mode cycler tooltip, "Toggle view mode (current: <name>)". In History mode the
/// cycler shows `shell-toolbar-view-mode-history` instead; the caller picks.
pub fn view_mode_tooltip(mode: TableMode, cx: &App) -> SharedString {
    let name = shell_text(cx, view_mode_name(mode), &[]);
    shell_text(
        cx,
        VIEW_MODE_TOOLTIP,
        &[("name", FluentValue::from(name.as_ref()))],
    )
}

/// The text for `key`, translated (R-132). en-US before the strings are installed. For text made
/// once per action, never per frame.
fn shell_text(cx: &App, key: &str, args: &[(&str, FluentValue<'_>)]) -> SharedString {
    if cx.has_global::<Strings>() {
        tr_args(cx, key, args)
    } else {
        tr_en(key, args)
    }
}

/// Ctrl+Plus, Ctrl+Minus and Ctrl+0: sets `appearance.ui_scale_pct` to `step(pct)` and shows
/// the new value in one keyed toast. Nothing happens when the value does not change. The main
/// window's settings observer applies the new metrics.
fn step_ui_scale(cx: &mut App, step: impl FnOnce(u16) -> u16) {
    let pct = TlSettings::settings(cx).appearance.ui_scale_pct;
    let new = step(pct);
    if new == pct {
        return;
    }
    TlSettings::update(cx, |d| d.settings.appearance.ui_scale_pct = new);
    let Some(view) = main_window_view(cx) else {
        return;
    };
    let mut text = String::new();
    let _ = view.read(cx).scale_text.write(&mut text, &[&new]);
    push_main_toast(cx, ToastKind::Info, text, Some(UI_SCALE_TOAST), None);
}

/// Makes the first main window open fail with this text, for the open-failure tests.
#[cfg(any(test, feature = "test-hooks"))]
pub struct FailMainOpen(pub String);

#[cfg(any(test, feature = "test-hooks"))]
impl Global for FailMainOpen {}

/// Opens the first main window. The error text on failure.
fn open_first_main(cx: &mut App) -> Result<(), String> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(fail) = cx.try_global::<FailMainOpen>() {
        return Err(fail.0.clone());
    }
    open_main_window(cx)
        .map(drop)
        .map_err(|err| err.to_string())
}

/// `Quit` always quits, whatever the tray settings (UI-SHL-113).
fn on_quit(_: &Quit, cx: &mut App) {
    if let Some(view) = main_window_view(cx) {
        view.update(cx, |this, cx| this.write_size(cx));
    }
    request_exit(cx, UiExit::Quit);
}

/// `ShowMainWindow`: activates the main window, or reopens it when it was removed.
fn show_main_window(cx: &mut App) {
    let shown = main_handle(cx).is_some_and(|handle| {
        handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    });
    if !shown {
        reopen_main_window(cx);
    }
}

/// `ToggleMainWindow`: an active main window is removed when the close decision says so and
/// minimized otherwise; an inactive one is activated; a removed one is reopened.
fn toggle_main_window(cx: &mut App) {
    let toggled = main_handle(cx).is_some_and(|handle| {
        handle
            .update(cx, |_, window, cx| {
                if !window.is_window_active() {
                    window.activate_window();
                    return;
                }
                let Some(view) = main_window_view(cx) else {
                    return;
                };
                match view.read(cx).close_decision(cx) {
                    CloseOutcome::RemoveWindow => view.update(cx, |this, cx| {
                        this.apply_close(CloseOutcome::RemoveWindow, window, cx)
                    }),
                    CloseOutcome::Quit => minimize_main(window, cx),
                }
            })
            .is_ok()
    });
    if !toggled {
        reopen_main_window(cx);
    }
}

/// The main window's handle. A handle whose window is gone is cleared.
fn main_handle(cx: &mut App) -> Option<WindowHandle<Root>> {
    let handle = cx.try_global::<ShellState>()?.main_window?;
    if cx
        .windows()
        .iter()
        .any(|w| w.window_id() == handle.window_id())
    {
        return Some(handle);
    }
    let state = cx.global_mut::<ShellState>();
    state.main_window = None;
    state.main_view = None;
    None
}

/// Opens a new main window on `UiState.tabs.last_tab` (UI-SHL-112), ignoring `--tab` and
/// `general.start_tab`, then activates it.
pub fn reopen_main_window(cx: &mut App) {
    let tab = TlSettings::state(cx).tabs.last_tab;
    let handle = match open_main_window_on(tab, cx) {
        Ok(handle) => handle,
        Err(err) => {
            error!(target: "tasklens::ui", %err, "could not reopen the main window");
            return;
        }
    };
    let _ = handle.update(cx, |_, window, _| window.activate_window());
    // after reopen (E31-009 chain edit): tell the tray bridge the main window is visible
}

/// `UiState.window` for `bounds`: logical px rounded to integers. Maximized and fullscreen save
/// the size they carry with `maximized` set.
fn size_state(bounds: WindowBounds) -> WindowState {
    let (b, maximized) = match bounds {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
    };
    WindowState {
        width: f32::from(b.size.width).round() as u32,
        height: f32::from(b.size.height).round() as u32,
        maximized,
    }
}

/// `window.window_bounds()`. The test window is never maximized, so tests set
/// `TitleBarProbe.maximized` instead.
fn window_bounds_of(window: &Window, cx: &App) -> WindowBounds {
    #[cfg(any(test, feature = "test-hooks"))]
    if cx.try_global::<TitleBarProbe>().and_then(|p| p.maximized) == Some(true) {
        return WindowBounds::Maximized(window.bounds());
    }
    let _ = cx;
    window.window_bounds()
}

/// The `MainWindow` view of the open main window.
pub fn main_window_view(cx: &App) -> Option<Entity<MainWindow>> {
    cx.try_global::<ShellState>()?.main_view.as_ref()?.upgrade()
}

/// The tab the window opens on (UI-SHL-002): `--tab`, else `general.start_tab`, where
/// `last_used` reads `UiState.tabs.last_tab`. A tab that is not built becomes the first built tab.
pub fn start_tab(cx: &App) -> Tab {
    let arg = cx.try_global::<StartOptions>().and_then(|s| s.tab);
    let wanted = arg.unwrap_or(TlSettings::settings(cx).general.start_tab);
    let tab = match wanted {
        StartTab::Processes => Tab::Processes,
        StartTab::Performance => Tab::Performance,
        StartTab::Services => Tab::Services,
        StartTab::Startup => Tab::Startup,
        StartTab::Connections => Tab::Connections,
        StartTab::Information => Tab::Information,
        StartTab::LastUsed => TlSettings::state(cx).tabs.last_tab,
    };
    let built = built_tabs(cx);
    if built.contains(&tab) {
        tab
    } else {
        built.first().copied().unwrap_or(tab)
    }
}

/// The restored window bounds: the saved size, at least 800 × 500, never a display or position
/// (zed#46378).
pub fn restored_bounds(state: &WindowState) -> WindowBounds {
    let bounds = Bounds {
        origin: point(px(0.), px(0.)),
        size: size(
            px(state.width.max(MIN_W as u32) as f32),
            px(state.height.max(MIN_H as u32) as f32),
        ),
    };
    if state.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    }
}

/// The tray is running and available (E31-009 replaces the body).
fn tray_available(cx: &App) -> bool {
    let _ = cx;
    false
}

/// The main window's `WindowOptions` (UI-SHL-002, ADR-010, APP-048).
pub fn main_window_options(cx: &App) -> WindowOptions {
    let settings = TlSettings::settings(cx);
    let tray = &settings.tray;
    let start_minimized = cx
        .try_global::<StartOptions>()
        .is_some_and(|s| s.start_minimized);
    let start_hidden = start_minimized
        && (tray.close_to_tray || tray.minimize_to_tray || tray.always_show)
        && tray_available(cx);
    let mut options = WindowOptions {
        window_decorations: Some(match settings.appearance.decorations {
            Decorations::Client => WindowDecorations::Client,
            Decorations::Server => WindowDecorations::Server,
        }),
        window_background: WindowBackgroundAppearance::Opaque,
        window_min_size: Some(size(px(MIN_W), px(MIN_H))),
        app_id: Some(APP_ID.into()),
        window_bounds: Some(restored_bounds(&TlSettings::state(cx).window)),
        focus: !start_hidden,
        show: !start_hidden,
        ..TitleBar::window_options()
    };
    if let Some(titlebar) = options.titlebar.as_mut() {
        titlebar.title = Some(WINDOW_TITLE.into());
    }
    options
}

/// Opens the main window on [`start_tab`] and stores its handle in `ShellState`. Returns the
/// GPUI error, which on Linux includes a renderer failure.
pub fn open_main_window(cx: &mut App) -> gpui_kit::Result<WindowHandle<Root>> {
    let start = start_tab(cx);
    open_main_window_on(start, cx)
}

/// [`open_main_window`] on `start`. E20-A1's reopen path passes `UiState.tabs.last_tab`.
pub fn open_main_window_on(start: Tab, cx: &mut App) -> gpui_kit::Result<WindowHandle<Root>> {
    let built = built_tabs(cx);
    let start = if built.contains(&start) {
        start
    } else {
        built.first().copied().unwrap_or(start)
    };
    cx.default_global::<ShellState>().built_tabs = built;
    let options = main_window_options(cx);
    let (handle, view) = gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| MainWindow::new(start, window, cx))
    })?;
    let handle = WindowHandle::<Root>::new(handle.window_id());
    let state = cx.default_global::<ShellState>();
    state.main_window = Some(handle);
    state.main_view = Some(view.downgrade());
    Ok(handle)
}

fn strings_version(cx: &App) -> u64 {
    cx.try_global::<Strings>().map_or(0, |s| s.strings_version)
}

/// The UI scale toast text, translated (R-132). en-US before the strings are installed.
fn scale_text(cx: &App) -> Template {
    if cx.has_global::<Strings>() {
        Template::new(cx, UI_SCALE_TEXT, &["pct"])
    } else {
        Template::en(UI_SCALE_TEXT, &["pct"])
    }
}

/// The `[appearance]` values that change the metrics: scale, row height, rounded corners and
/// shadow intensity.
type MetricsKey = (u16, u16, bool, u8);

fn metrics_key(cx: &App) -> MetricsKey {
    let a = &TlSettings::settings(cx).appearance;
    (
        a.ui_scale_pct,
        a.row_height_pct,
        a.rounded_corners,
        a.shadow_intensity_pct,
    )
}

/// Pushes a toast to the main window's queue. Without a main window the toast is dropped.
pub fn push_main_toast(
    cx: &mut App,
    kind: ToastKind,
    text: impl Into<SharedString>,
    key: Option<ToastKey>,
    action: Option<ToastAction>,
) {
    let Some(view) = main_window_view(cx) else {
        debug!(target: "tasklens::ui", ?kind, "no main window, toast dropped");
        return;
    };
    let toasts = view.read(cx).toasts.clone();
    toasts.update(cx, |queue, cx| queue.push(kind, text, key, action, cx));
}

/// `first_frame_ms` from `/proc/self/stat` and `/proc/uptime` text (ADR-014): uptime minus the
/// process start time, both in ms. `None` on any parse failure.
pub fn first_frame_ms_from(stat: &str, uptime: &str) -> Option<u64> {
    // The command name may contain spaces and parentheses; the fields after it start at field 3,
    // so field 22 (`starttime`, clock ticks since boot) is the 20th of them.
    let (_, rest) = stat.rsplit_once(')')?;
    let start_ticks: u64 = rest.split_whitespace().nth(19)?.parse().ok()?;
    // USER_HZ is 100 on Linux (kernel ABI).
    let start_ms = start_ticks.checked_mul(10)?;

    let up = uptime.split_whitespace().next()?;
    let (secs, frac) = up.split_once('.').unwrap_or((up, "0"));
    let secs: u64 = secs.parse().ok()?;
    if frac.is_empty() || frac.len() > 2 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hundredths: u64 = frac.parse::<u64>().ok()? * if frac.len() == 1 { 10 } else { 1 };
    let now_ms = secs.checked_mul(1000)?.checked_add(hundredths * 10)?;
    Some(now_ms.saturating_sub(start_ms))
}

/// The main window's root view.
pub struct MainWindow {
    /// Key context "Global".
    root_focus: FocusHandle,
    titlebar: Entity<TitleBarView>,
    tabstrip: Entity<TabStrip>,
    /// The shell filter box (E20-009); in the tab strip's slot on every tab but Performance.
    filter: Entity<FilterBox>,
    status: Entity<StatusChip>,
    toasts: Entity<ToastQueue>,
    /// Kept here so the driver lives as long as the window (anim.rs "Storage").
    _anim: Entity<AnimationDriver>,
    /// Index = `Tab::ALL` position. Created on first visit, kept.
    pages: [Option<PageSlot>; 6],
    active: Tab,
    /// Window picking is active (E20-027 sets it). Esc cancels it first.
    picking: bool,
    status_buf: String,
    /// `(state_version, processes version)` the toolbar model was built from.
    settings_versions: (u64, u64),
    style_version: u64,
    /// The decoration kind the window reported at the last render: true for client. The title
    /// bar's height and the toast top follow it.
    client: bool,
    /// The latest window bounds, written to `UiState.window` after the debounce or on close.
    bounds: WindowBounds,
    /// The pending size write. A new bounds change replaces it.
    save_task: Option<Task<()>>,
    /// The `[appearance]` values the metrics were last applied from.
    applied: MetricsKey,
    /// The window's scale factor at the last check.
    scale_factor: f32,
    /// "UI scale changed to { $pct }%", per strings version.
    scale_text: Template,
    strings_version: u64,
    /// The 1 s health refresh runs while the chip shows a Stale item (E20-016).
    health_ticking: bool,
    _health_tick: Option<Task<()>>,
    _subs: SmallVec<[Subscription; 8]>,
    #[cfg(any(test, feature = "test-hooks"))]
    first_frames: u32,
}

/// The `TlSettings` versions the toolbar model depends on.
fn settings_versions(cx: &App) -> (u64, u64) {
    let tl = TlSettings::get(cx);
    (tl.state_version(), tl.version(Section::Processes))
}

impl MainWindow {
    pub fn new(start: Tab, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A window reopened after an appearance change made while it was gone applies it here.
        // Unchanged values do nothing.
        if cx.has_global::<ThemeStore>() {
            let appearance = TlSettings::settings(cx).appearance.clone();
            apply_metrics(&appearance, cx);
        }
        let s = metrics(cx).ui_scale;
        let root_focus = cx.focus_handle();
        let built = cx
            .try_global::<ShellState>()
            .map_or_else(|| built_tabs(cx), |state| state.built_tabs.clone());
        let titlebar = cx.new(|cx| TitleBarView::new(window, cx));
        let tabstrip = cx.new(|cx| TabStrip::new(built, start, window, cx));
        let filter = cx.new(|cx| FilterBox::new(start, window, cx));
        let status = cx.new(StatusChip::new);
        let client = is_client(window, cx);
        let top = toast_top(client, s);
        let toasts = cx.new(|cx| ToastQueue::new(window, cx, top));
        let anim = cx.new(AnimationDriver::new);
        AnimationDriver::register(&anim, window, cx);
        crate::theme::store::set_anim_window(window.window_handle(), cx);
        if let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) {
            let handle = window.window_handle();
            demand.update(cx, |demand, _| demand.set_main_window(handle));
        }

        let mut subs = SmallVec::new();
        subs.push(cx.observe_global::<TlSettings>(|this: &mut Self, cx| {
            let versions = settings_versions(cx);
            if versions == this.settings_versions {
                return;
            }
            this.settings_versions = versions;
            let history_mode = TlSettings::state(cx).processes.history_mode;
            cx.default_global::<ShellState>().history_mode = history_mode;
            let model = this.toolbar_model(cx);
            this.titlebar.update(cx, |bar, cx| bar.set_model(model, cx));
        }));
        // Scale, row height, corners and shadow changes from the keys, the menu sliders, the
        // Settings window or a reset (UI-SHL-130).
        subs.push(cx.observe_global::<TlSettings>(|this: &mut Self, cx| this.apply_appearance(cx)));
        subs.push(cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = strings_version(cx);
            if version != this.strings_version {
                this.strings_version = version;
                this.scale_text = scale_text(cx);
            }
        }));
        // The layout and the toast top scale with ui_scale (UI-SHL-130).
        subs.push(cx.observe_global::<UiMetrics>(|this: &mut Self, cx| {
            let m = metrics(cx);
            if m.style_version == this.style_version {
                return;
            }
            this.style_version = m.style_version;
            let top = toast_top(this.client, m.ui_scale);
            this.toasts.update(cx, |queue, cx| queue.set_top(top, cx));
            cx.notify();
        }));
        // The title bar keeps the toolbar model's `window_active` itself (E20-004). Losing
        // activation ends Ctrl-pause (UI-SHL-007, E20-012).
        subs.push(
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if !window.is_window_active() {
                    this.set_paused(false, cx);
                }
            }),
        );
        // Size saving (UI-SHL-002): the last bounds change wins, written 500 ms later.
        subs.push(
            cx.observe_window_bounds(window, |this: &mut Self, window, cx| {
                this.on_scale_factor(window.scale_factor(), cx);
                this.bounds = window_bounds_of(window, cx);
                this.save_task = Some(cx.spawn(
                    async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                        cx.background_executor().timer(SIZE_SAVE_DELAY).await;
                        let _ = this.update(cx, |this, cx| this.write_size(cx));
                    },
                ));
            }),
        );
        // Esc, Enter and `ClearFilter` in the filter box hand focus back to the page.
        subs.push(cx.subscribe_in(
            &filter,
            window,
            |this: &mut Self, _, _: &ReturnFocus, window, cx| this.focus_page(window, cx),
        ));
        // Health items in the status chip (E20-016).
        if let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) {
            subs.push(
                cx.subscribe(&hub, |this: &mut Self, _, ev: &SnapshotChanged, cx| {
                    if ev.mask.contains(ComponentMask::HEALTH) {
                        this.sync_health(cx);
                    }
                }),
            );
        }
        // The compositor's close request follows the same decision; tasklens then removes the
        // window or quits itself, so the platform never closes it.
        let view = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            view.update(cx, |this, cx| {
                let outcome = this.close_decision(cx);
                this.apply_close(outcome, window, cx);
            })
            .is_err()
        });

        let mut this = Self {
            root_focus,
            titlebar,
            tabstrip,
            filter,
            status,
            toasts,
            _anim: anim,
            pages: Default::default(),
            active: start,
            picking: false,
            status_buf: String::new(),
            settings_versions: settings_versions(cx),
            style_version: metrics(cx).style_version,
            client,
            bounds: window_bounds_of(window, cx),
            save_task: None,
            applied: metrics_key(cx),
            scale_factor: window.scale_factor(),
            scale_text: scale_text(cx),
            strings_version: strings_version(cx),
            health_ticking: false,
            _health_tick: None,
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            first_frames: 0,
        };
        this.switch_to(start, false, window, cx);
        cx.on_next_frame(window, |this, window, cx| {
            this.after_first_frame(window, cx)
        });
        this
    }

    /// Selects `tab`. Nothing happens for the active tab or a tab that is not built.
    pub fn select_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.active || factory_for(tab, cx).is_none() {
            return;
        }
        self.switch_to(tab, true, window, cx);
    }

    pub fn active_tab(&self) -> Tab {
        self.active
    }

    /// The page entity of `tab`, if it was built and is a `T`.
    pub fn page_entity<T: 'static>(&self, tab: Tab) -> Option<Entity<T>> {
        self.pages[tab as usize]
            .as_ref()
            .and_then(|slot| slot.view.clone().downcast::<T>().ok())
    }

    pub fn toasts(&self) -> &Entity<ToastQueue> {
        &self.toasts
    }

    /// The shell filter box. E20-A2 routes type-to-filter through it.
    pub fn filter_box(&self) -> &Entity<FilterBox> {
        &self.filter
    }

    /// Focuses the active page's `primary_focus`.
    fn focus_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(page) = &self.pages[self.active as usize] {
            page.handle.primary_focus(cx).focus(window, cx);
        }
    }

    /// Ctrl+F: nothing on Performance, where the box is not shown.
    fn on_focus_filter(&mut self, _: &FocusFilter, window: &mut Window, cx: &mut Context<Self>) {
        if shown_on(self.active) {
            self.filter
                .update(cx, |f, cx| f.focus_and_select_all(window, cx));
        }
    }

    fn on_clear_filter(&mut self, _: &ClearFilter, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |f, cx| f.clear(window, cx));
    }

    /// The Esc chain (UI-SHL-122). The kit has already closed any open menu, popover or
    /// in-window dialog: those are deeper in the focus path.
    fn on_escape_chain(&mut self, _: &EscapeChain, window: &mut Window, cx: &mut Context<Self>) {
        let filter = self.filter.read(cx);
        let state = EscState {
            picking: self.picking,
            filter_focused: filter.is_focused(window, cx),
            filter_has_text: !filter.text(self.active).is_empty(),
            close_with_esc: TlSettings::settings(cx).window.close_with_esc,
        };
        match esc_outcome(state) {
            EscOutcome::CancelPick => window.dispatch_action(Box::new(CancelWindowPick), cx),
            // `FilterEscape` in "ShellFilter" handled this Esc already.
            EscOutcome::FilterRule | EscOutcome::Nothing => {}
            EscOutcome::ClearFilterAndFocusTable => {
                self.filter.update(cx, |f, cx| f.clear(window, cx));
                self.focus_page(window, cx);
            }
            EscOutcome::CloseWindow => window.dispatch_action(Box::new(CloseWindow), cx),
        }
    }

    /// Sets window picking for tests until E20-027 owns it.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_picking_for_test(&mut self, picking: bool) {
        self.picking = picking;
    }

    /// The status chip. E20-012 sets its paused tag through it.
    pub fn status_chip(&self) -> &Entity<StatusChip> {
        &self.status
    }

    /// `after_first_frame` runs.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn first_frames(&self) -> u32 {
        self.first_frames
    }

    /// Ctrl-pause (E20-012, UI-SHL-123): paused while Ctrl is held and the window is active. A
    /// Ctrl that is part of a shortcut pauses too, for as long as it is held.
    fn on_modifiers(
        &mut self,
        ev: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let want = ev.modifiers.control && window.is_window_active();
        self.set_paused(want, cx);
    }

    /// Sets `ShellState.paused` and the chip's paused tag, and emits `PauseChanged`. Nothing
    /// happens when the state does not change.
    fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        let state = cx.default_global::<ShellState>();
        if state.paused == paused {
            return;
        }
        state.paused = paused;
        self.status
            .update(cx, |chip, cx| chip.set_paused(paused, cx));
        if let Some(events) = cx.try_global::<GlobalShellEvents>().map(|e| e.0.clone()) {
            events.update(cx, |_, cx| cx.emit(PauseChanged(paused)));
        }
    }

    /// The close decision (UI-SHL-113). Rows are checked in order; first match wins.
    fn close_decision(&self, cx: &App) -> CloseOutcome {
        let _ = cx;
        // close decision: row 1, close to tray (E31-009 chain edit)
        // close decision: row 2, keep detached graphs (E20-028)
        CloseOutcome::Quit
    }

    /// Saves the size, then quits or removes the window. Removing closes the registered child
    /// windows (APP-045); the app and the engine keep running.
    fn apply_close(&mut self, outcome: CloseOutcome, window: &mut Window, cx: &mut Context<Self>) {
        self.save_size_now(window, cx);
        match outcome {
            CloseOutcome::Quit => request_exit(cx, UiExit::Quit),
            CloseOutcome::RemoveWindow => {
                let keep_detached = TlSettings::settings(cx).graphs.keep_detached_on_close;
                if let Some(registry) = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
                    registry.update(cx, |registry, cx| {
                        registry.close_children_of_main(keep_detached, cx)
                    });
                }
                // A removed window gets no later deactivation, so Ctrl-pause ends here.
                self.set_paused(false, cx);
                let state = cx.default_global::<ShellState>();
                state.main_window = None;
                state.main_view = None;
                window.remove_window();
            }
        }
    }

    /// `apply_close` for tests that need a removed main window.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn apply_close_for_test(
        &mut self,
        outcome: CloseOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_close(outcome, window, cx);
    }

    /// Drops the pending size write and writes the current bounds at once.
    fn save_size_now(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.bounds = window_bounds_of(window, cx);
        self.write_size(cx);
    }

    /// Writes the recorded bounds to `UiState.window` and drops the pending write. Position is
    /// never saved.
    fn write_size(&mut self, cx: &mut Context<Self>) {
        self.save_task = None;
        let state = size_state(self.bounds);
        TlSettings::update(cx, |d| d.state.window = state);
    }

    fn on_close_window(&mut self, _: &CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = self.close_decision(cx);
        self.apply_close(outcome, window, cx);
    }

    fn on_minimize(&mut self, _: &MinimizeWindow, window: &mut Window, cx: &mut Context<Self>) {
        minimize_main(window, cx);
    }

    fn on_zoom(&mut self, _: &ZoomWindow, window: &mut Window, cx: &mut Context<Self>) {
        zoom(window, cx);
    }

    /// Re-applies the metrics when the scale, row height, corners or shadow changed
    /// (UI-SHL-130 steps 1 to 5). A `TlSettings::preview` reaches it too, so a slider drag
    /// previews live.
    fn apply_appearance(&mut self, cx: &mut Context<Self>) {
        let key = metrics_key(cx);
        if key == self.applied {
            return;
        }
        self.applied = key;
        let appearance = TlSettings::settings(cx).appearance.clone();
        self.keep_first_row(cx, |cx| apply_metrics(&appearance, cx));
        self.tabstrip
            .update(cx, |strip, cx| strip.invalidate_layout_cache(cx));
        let top = toast_top(self.client, metrics(cx).ui_scale);
        self.toasts.update(cx, |queue, cx| queue.set_top(top, cx));
        cx.refresh_windows();
    }

    /// Runs `apply` with the active page's first visible row kept at the top (UI-SHL-130 step
    /// 5, R-187). Each page with a table adds its arm.
    fn keep_first_row(&self, cx: &mut Context<Self>, apply: impl FnOnce(&mut Context<Self>)) {
        let table = match self.active {
            Tab::Processes => self
                .page_entity::<ProcessesPage>(Tab::Processes)
                .map(|page| page.read(cx).table().clone()),
            _ => None,
        };
        let anchor = table
            .as_ref()
            .and_then(|table| table.read(cx).first_visible_row());
        apply(cx);
        if let (Some(table), Some(anchor)) = (table, anchor) {
            table.update(cx, |t, cx| t.restore_first_visible_row(anchor, cx));
        }
    }

    /// A compositor scale change. GPUI applies the factor itself, so `ui_scale` stays; the
    /// style version bumps so cached text and paths are rebuilt (UI-SHL-130).
    fn on_scale_factor(&mut self, factor: f32, cx: &mut Context<Self>) {
        if factor == self.scale_factor {
            return;
        }
        self.scale_factor = factor;
        bump_style_version(cx);
        self.tabstrip
            .update(cx, |strip, cx| strip.invalidate_layout_cache(cx));
        cx.refresh_windows();
    }

    /// Reports a window scale factor change; the test window's factor is fixed.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn scale_factor_changed_for_test(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.on_scale_factor(factor, cx);
    }

    /// The tab switch. `write_last` is false only for the start tab.
    fn switch_to(
        &mut self,
        tab: Tab,
        write_last: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if tab != self.active
            && let Some(old) = &self.pages[self.active as usize]
        {
            old.handle.set_active(false, cx);
        }
        if self.pages[tab as usize].is_none()
            && let Some(factory) = factory_for(tab, cx)
        {
            self.pages[tab as usize] = Some(factory(window, cx));
        }
        self.active = tab;
        if let Some(page) = &self.pages[tab as usize] {
            page.handle.set_active(true, cx);
        }
        cx.default_global::<ShellState>().active_tab = tab;
        self.tabstrip
            .update(cx, |strip, cx| strip.set_active(tab, cx));
        self.filter.update(cx, |f, cx| f.set_tab(tab, window, cx));
        let slot = shown_on(tab).then(|| AnyView::from(self.filter.clone()));
        self.tabstrip
            .update(cx, |strip, cx| strip.set_filter_slot(slot, cx));
        let model = self.toolbar_model(cx);
        self.titlebar
            .update(cx, |bar, cx| bar.set_toolbar(model, toolbar_for(tab), cx));
        self.sync_status(cx);
        self.sync_health(cx);
        if write_last {
            TlSettings::update(cx, |d| d.state.tabs.last_tab = tab);
        }
        let primary = self.pages[tab as usize]
            .as_ref()
            .map(|page| page.handle.primary_focus(cx));
        focus_initial(primary.as_ref(), &self.root_focus, window, cx);
        cx.notify();
    }

    /// The toolbar model for the active tab. Fields other beads own (helper, picker) are kept.
    fn toolbar_model(&self, cx: &App) -> ToolbarModel {
        let mut model = self.titlebar.read(cx).model().clone();
        let settings = TlSettings::settings(cx);
        model.tab = self.active;
        model.history_mode = TlSettings::state(cx).processes.history_mode;
        model.hide_system = settings.processes.hide_system;
        model.mode = settings.processes.mode;
        model
    }

    /// Passes the active page's status text and actions to the chip.
    fn sync_status(&mut self, cx: &mut Context<Self>) {
        let mut buf = mem::take(&mut self.status_buf);
        buf.clear();
        let actions = match &self.pages[self.active as usize] {
            Some(page) => {
                page.handle.status_text(&mut buf, cx);
                page.handle.status_actions(cx)
            }
            None => SmallVec::new(),
        };
        self.status.update(cx, |chip, cx| {
            chip.set_page_text(&buf, cx);
            chip.set_page_actions(actions, cx);
        });
        self.status_buf = buf;
    }

    /// Passes the hub's health snapshot and the active tab to the chip. While the chip shows a
    /// Stale item, a 1 s timer calls this again so the tooltip age moves.
    fn sync_health(&mut self, cx: &mut Context<Self>) {
        let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) else {
            return;
        };
        let health = hub.read(cx).current().health.clone();
        let tab = self.active;
        let stale = self.status.update(cx, |chip, cx| {
            chip.set_health(&health, tab, now_boot_ns(), cx);
            chip.has_stale()
        });
        if stale && !self.health_ticking {
            self.health_ticking = true;
            self._health_tick = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let ticking = this.update(cx, |this, cx| {
                        this.sync_health(cx);
                        let stale = this.status.read(cx).has_stale();
                        this.health_ticking = stale;
                        stale
                    });
                    if !matches!(ticking, Ok(true)) {
                        break;
                    }
                }
            }));
        }
    }

    /// A page emitted `StatusChanged`. Only the active page reaches the chip.
    fn on_page_status(&mut self, page: EntityId, cx: &mut Context<Self>) {
        let active = self.pages[self.active as usize]
            .as_ref()
            .is_some_and(|slot| slot.entity_id() == page);
        if active {
            self.sync_status(cx);
        }
    }

    /// The next (`step` 1) or previous (`step` -1) built tab, wrapping around.
    fn neighbour(&self, step: isize, cx: &App) -> Option<Tab> {
        let built = built_tabs(cx);
        let n = built.len() as isize;
        if n == 0 {
            return None;
        }
        let at = built.iter().position(|t| *t == self.active).unwrap_or(0) as isize;
        Some(built[(at + step).rem_euclid(n) as usize])
    }

    fn on_select_tab(&mut self, a: &SelectTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&tab) = Tab::ALL.get(a.0) {
            self.select_tab(tab, window, cx);
        }
    }

    fn on_next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.neighbour(1, cx) {
            self.select_tab(tab, window, cx);
        }
    }

    fn on_prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.neighbour(-1, cx) {
            self.select_tab(tab, window, cx);
        }
    }

    fn on_refresh(&mut self, _: &RefreshCurrentTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(page) = &self.pages[self.active as usize] {
            page.handle.refresh(cx);
        }
    }

    /// UI-SHL-110 step 11. Runs once per window, after its first frame.
    fn after_first_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.first_frames += 1;
        }
        // step 11: first_frame_ms, once per process (ADR-014)
        if !cx.has_global::<FirstFrameLogged>() {
            cx.set_global(FirstFrameLogged);
            crate::app::perf_step("first_frame");
            let ms = std::fs::read_to_string("/proc/self/stat")
                .ok()
                .zip(std::fs::read_to_string("/proc/uptime").ok())
                .and_then(|(stat, uptime)| first_frame_ms_from(&stat, &uptime));
            match ms {
                Some(ms) => info!(target: "tasklens::perf", first_frame_ms = ms, "first frame"),
                None => warn!(target: "tasklens::perf", "first_frame_ms unavailable"),
            }
        }
        // step 11: queued toasts (E20-018)
        // step 11: crash report (E20-018)
        // step 11: --select-pid (E20-018)
        // step 11: --start-minimized without tray (E20-018)
        // step 11: helper unlock at launch (E20-026)
        // step 11: public IP auto-fetch (E20-024)
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) {
            hub.update(cx, |hub, cx| hub.note_render(cx));
        }
        let bg = current(cx).window_bg;
        let s = metrics(cx).ui_scale;
        // A configure event can switch the decoration kind (the compositor refused or granted
        // server decorations). The kind is part of the title bar's cache key: its height
        // changes, the bar view is marked dirty and the toast top moves.
        // `appearance.decorations` itself is read once at open (UI-SHL-006).
        let client = is_client(window, cx);
        if client != self.client {
            self.client = client;
            self.titlebar.update(cx, |_, cx| cx.notify());
            // Deferred: the toast layer may already be drawn in this frame.
            cx.defer_in(window, move |this, _, cx| {
                let top = toast_top(client, metrics(cx).ui_scale);
                this.toasts.update(cx, |queue, cx| queue.set_top(top, cx));
            });
        }
        let chip_w = (window.viewport_size().width - px(CHIP_RESERVE * s)).max(px(0.));

        let page = self.pages[self.active as usize].as_ref().map(|slot| {
            slot.view
                .clone()
                .cached(StyleRefinement::default().size_full())
        });
        // A cached view is laid out at its style size, so the chip gets the slot's full width;
        // the chip itself sizes to its content inside it (max_w_full).
        let chip = AnyView::from(self.status.clone())
            .cached(StyleRefinement::default().w(chip_w).h(px(CHIP_H * s)));
        let menu = context_menu_layer(window, cx);

        v_flex()
            .debug_selector(|| "MainWindow".into())
            .size_full()
            .bg(bg)
            .key_context(GLOBAL_CONTEXT)
            .track_focus(&self.root_focus)
            .on_action(cx.listener(Self::on_select_tab))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_close_window))
            .on_action(cx.listener(Self::on_minimize))
            .on_action(cx.listener(Self::on_zoom))
            // Only the main window pauses; secondary windows have no such listener.
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            // window-level on_action handlers (chain edits)
            .on_action(cx.listener(Self::on_focus_filter))
            .on_action(cx.listener(Self::on_clear_filter))
            .on_action(cx.listener(Self::on_escape_chain))
            .child(
                AnyView::from(self.titlebar.clone()).cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(bar_height_of(client, s)),
                ),
            )
            .child(
                AnyView::from(self.tabstrip.clone())
                    .cached(StyleRefinement::default().w_full().h(px(TAB_STRIP_H * s))),
            )
            .child(
                div()
                    .debug_selector(|| "MainWindowPage".into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .children(page)
                    // Painted after the page, so the chip's actions come last in Tab order
                    // (UI-SHL-121).
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .bottom_0()
                            .max_w(chip_w)
                            .child(chip),
                    ),
            )
            // Last: an open context menu covers the window (E16-A3).
            .children(menu)
    }
}
