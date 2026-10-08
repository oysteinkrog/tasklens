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
//!
//! `ToggleAlwaysOnTop` for the main window flips `window.always_on_top` and sends `KeepAbove`
//! through the KWin bridge; a window that opens with the setting on gets it once the bridge is up
//! (E20-022, UI-SHL-005, APP-008, APP-009, R-073, R-176).
//!
//! The requests a second `tasklens` launch forwards to this copy go to [`handle_app_request`]
//! (E20-019, UI-SHL-112, APP-011, APP-052, ADR-010).

use std::mem;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fluent_bundle::FluentValue;
use gpui_kit::base::Root;
use gpui_kit::component::{TitleBar, v_flex};
use gpui_kit::{
    AnyView, App, AppContext as _, AsyncApp, Bounds, Context, Entity, EntityId, EventEmitter,
    FocusHandle, Global, InteractiveElement, IntoElement, KeyDownEvent, ModifiersChangedEvent,
    ParentElement, Pixels, Render, SharedString, StyleRefinement, Styled, Subscription, Task,
    WeakEntity, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle,
    WindowOptions, div, point, prelude::FluentBuilder as _, px, size,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::command::{AppRequest, Command, CommandError, DeviceCommand, KWinCommand, Tab};
use tl_model::settings::{Decorations, StartTab, WindowState};
use tl_model::table::TableMode;
use tl_model::time::now_boot_ns;
use tracing::{debug, error, info, warn};

use super::filter::{FilterBox, ReturnFocus, shown_on};
use super::registry::{GlobalRegistry, WindowKey};
use super::status::{CHIP_H, CHIP_RESERVE, StatusAction, StatusChanged, StatusChip};
use super::tabstrip::{TabStrip, tween_progress};
#[cfg(any(test, feature = "test-hooks"))]
use super::titlebar::TitleBarProbe;
use super::titlebar::{
    TitleBarView, bar_height_of, is_client, keep_above_supported, minimize_main, zoom,
};
use super::toolbar::{DefaultToolbar, ToolbarContent, ToolbarModel};
use crate::actions::{
    CancelWindowPick, ClearFilter, CloseWindow, CycleViewMode, EscapeChain, FocusFilter,
    GLOBAL_CONTEXT, MinimizeWindow, NextTab, PrevTab, Quit, RefreshCurrentTab,
    SHELL_FILTER_CONTEXT, ScaleDown, ScaleReset, ScaleUp, SelectTab, SetViewMode, ShowMainWindow,
    ToggleHideSystem, ToggleHistoryMode, ToggleMainWindow, ZoomWindow,
};
use crate::anim::{AnimationDriver, FrameStats};
use crate::app::{
    AppRequestHandler, BootSettings, EngineGlobal, OpenHooks, StartOptions, ToastKindTag, UiExit,
    request_exit,
};
use crate::components::dialog::CONTEXT as DIALOG_CONTEXT;
use crate::components::menu::{KEY_CONTEXT as MENU_CONTEXT, context_menu_layer};
use crate::components::toast::{ToastAction, ToastKey, ToastKind, ToastQueue};
use crate::components::window_frame::ToggleAlwaysOnTop;
use crate::demand::GlobalDemand;
use crate::focus::{EscOutcome, EscState, esc_outcome, focus_initial};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr_args, tr_en};
use crate::pages::information::InformationPage;
use crate::pages::performance::details::Template;
use crate::pages::performance::{PerformancePage, SummaryToggled};
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::jump::JumpTexts;
use crate::pages::processes::toolbar::ProcessesToolbar;
use crate::settings_global::{Section, TlSettings, animations_on};
use crate::table::actions::TABLE_CONTEXT;
use crate::text::style::numeric_style;
use crate::theme::store::apply_metrics;
use crate::theme::{ThemeStore, UiMetrics, bump_style_version, current, metrics};

pub use super::tabstrip::TAB_STRIP_H;
pub use super::titlebar::TITLEBAR_H;

/// Key contexts that keep a typed key from going to the filter even below a "Table": the kit
/// and tl-ui names of inputs, the filter itself, menus and dialogs (E20-A2).
const NO_TYPE_TO_FILTER: [&str; 7] = [
    "Input",
    "TlInput",
    SHELL_FILTER_CONTEXT,
    "PopupMenu",
    MENU_CONTEXT,
    "Dialog",
    DIALOG_CONTEXT,
];

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

/// The one char of `text`, or `None` when it has none or more than one.
fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}

/// The UI scale toast's key. Fixed, so repeated presses replace one toast (UI-SHL-130).
pub const UI_SCALE_TOAST: ToastKey = ToastKey(0x7569_7363_616c_6531);
/// "UI scale changed to { $pct }%".
const UI_SCALE_TEXT: &str = "shell-toast-ui-scale";
/// The UI scale keys' step and range, in percent (APP-040).
const HIDING_SYSTEM_TEXT: &str = "shell-toast-hiding-system";
const SHOWING_SYSTEM_TEXT: &str = "shell-toast-showing-system";
const VIEW_MODE_TEXT: &str = "shell-toast-view-mode";
/// The cycler tooltip; the Processes toolbar reads the same key (jqk.35).
const VIEW_MODE_TOOLTIP: &str = "processes-toolbar-cycle-mode";

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
        Tab::Performance => Some(|_, cx| {
            let hub = cx.global::<GlobalHub>().0.clone();
            let demand = cx.global::<GlobalDemand>().0.clone();
            let page = cx.new(|cx| PerformancePage::new(hub, demand, cx));
            // Graph summary mode replaces the tab strip and the page (UI-PRF-016).
            cx.subscribe(&page, |this, page, _: &SummaryToggled, cx| {
                let hidden = page.read(cx).summary().is_some();
                this.titlebar
                    .update(cx, |bar, cx| bar.set_menus_hidden(hidden, cx));
                cx.notify();
            })
            .detach();
            PageSlot::new(page, cx)
        }),
        Tab::Information => Some(|window, cx| {
            let hub = cx.global::<GlobalHub>().0.clone();
            let demand = cx.global::<GlobalDemand>().0.clone();
            let page = cx.new(|cx| InformationPage::new(&hub, &demand, window, cx));
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
    /// Keep-above of the open secondary windows, in memory only (E20-029, R-073). A key is
    /// dropped when its window leaves the registry.
    pub secondary_pins: FxHashMap<WindowKey, bool>,
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
            secondary_pins: FxHashMap::default(),
        }
    }
}

/// Set once the first frame's `first_frame_ms` is logged, so a reopened window does not log again.
struct FirstFrameLogged;

impl Global for FirstFrameLogged {}

/// Set once the startup public IP fetch was considered, so a reopened window does not fetch
/// again (E20-024).
struct PublicIpAutoFetched;

impl Global for PublicIpAutoFetched {}

/// How long the startup public IP fetch waits before its one retry after `Busy`.
const PUBLIC_IP_RETRY: Duration = Duration::from_secs(1);

/// The startup public IP fetch (UI-SHL-110 step 11, APP-056, PERF-060): only with both
/// `network.auto_fetch_public_ip` and `network.public_ip_notice_accepted` on, so there is no
/// network contact before consent. `Busy` gets one retry 1 s later; no toast either way.
fn auto_fetch_public_ip(cx: &mut App) {
    let network = &TlSettings::settings(cx).network;
    if !(network.auto_fetch_public_ip && network.public_ip_notice_accepted) {
        return;
    }
    let Some(engine) = cx.try_global::<EngineGlobal>().map(|e| e.0.clone()) else {
        return;
    };
    let fetch = || Command::Device(DeviceCommand::FetchPublicIp);
    match engine.send(fetch()) {
        Ok(()) => {}
        Err(CommandError::Busy) => {
            cx.spawn(async move |cx: &mut AsyncApp| {
                cx.background_executor().timer(PUBLIC_IP_RETRY).await;
                if engine.send(fetch()).is_err() {
                    warn!(target: "tasklens::ui", "public IP auto-fetch not queued");
                }
            })
            .detach();
        }
        Err(_) => warn!(target: "tasklens::ui", "public IP auto-fetch not queued"),
    }
}

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
    cx.set_global(AppRequestHandler(Box::new(handle_app_request)));
    cx.on_action(on_toggle_always_on_top);
    // A closed secondary window forgets its pin (E20-029).
    if let Some(registry) = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        cx.observe(&registry, |registry, cx| {
            let open = registry.read(cx);
            let pins = &cx.global::<ShellState>().secondary_pins;
            let gone: SmallVec<[WindowKey; 4]> = pins
                .keys()
                .filter(|key| open.get(key).is_none())
                .cloned()
                .collect();
            if gone.is_empty() {
                return;
            }
            let pins = &mut cx.global_mut::<ShellState>().secondary_pins;
            for key in gone {
                pins.remove(&key);
            }
        })
        .detach();
    }
    cx.set_global(KeepAboveState::default());
    if let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) {
        cx.subscribe(&hub, |_, ev: &SnapshotChanged, cx| {
            if ev.mask.intersects(ComponentMask::WINDOWS) {
                on_windows_changed(cx);
            }
        })
        .detach();
    }
}

/// Keep-above work that waits for a `WINDOWS` change.
#[derive(Default)]
struct KeepAboveState {
    /// The main window opened and `window.always_on_top` was not applied to it yet.
    apply_pending: bool,
    /// A `KeepAbove` the engine refused as busy, sent once more on the next `WINDOWS` change.
    retry: Option<bool>,
}

impl Global for KeepAboveState {}

/// `ToggleAlwaysOnTop` (UI-SHL-005): for the main window, while the KWin bridge is up, flips
/// `window.always_on_top` (saved at once, no toast) and sends `KeepAbove`. A secondary window
/// flips its in-memory pin and sends `KeepAbove` for its caption (E20-029).
fn on_toggle_always_on_top(a: &ToggleAlwaysOnTop, cx: &mut App) {
    if !keep_above_supported(cx) {
        return;
    }
    if a.window != WindowKey::Main {
        toggle_secondary_pin(a.window.clone(), &a.caption, cx);
        return;
    }
    let on = !TlSettings::settings(cx).window.always_on_top;
    TlSettings::update(cx, |d| d.settings.window.always_on_top = on);
    keep_above_main(on, cx);
}

/// Flips the pin of the secondary window `key`, found by KWin through the tasklens pid and
/// `caption`. Nothing is saved (R-073). A failed send is logged and the stored state kept.
fn toggle_secondary_pin(key: WindowKey, caption: &SharedString, cx: &mut App) {
    let pins = &mut cx.default_global::<ShellState>().secondary_pins;
    let on = !pins.get(&key).copied().unwrap_or(false);
    pins.insert(key, on);
    let Some(engine) = cx.try_global::<EngineGlobal>() else {
        return;
    };
    let cmd = Command::KWin(KWinCommand::KeepAbove {
        pid: std::process::id(),
        caption: Some(Arc::from(caption.as_ref())),
        on,
    });
    if engine.0.send(cmd).is_err() {
        warn!(target: "tasklens::ui", "keep above not sent");
    }
}

/// Sends `KeepAbove` for the main window. A busy engine gets one retry on the next `WINDOWS`
/// change.
fn keep_above_main(on: bool, cx: &mut App) {
    match send_keep_above(on, cx) {
        Ok(()) => {}
        Err(CommandError::Busy) => cx.default_global::<KeepAboveState>().retry = Some(on),
        Err(err) => warn!(target: "tasklens::ui", ?err, on, "keep-above not sent"),
    }
}

fn send_keep_above(on: bool, cx: &App) -> Result<(), CommandError> {
    let Some(engine) = cx.try_global::<EngineGlobal>() else {
        return Err(CommandError::EngineStopped);
    };
    engine.0.send(Command::KWin(KWinCommand::KeepAbove {
        pid: std::process::id(),
        caption: None,
        on,
    }))
}

/// A `WINDOWS` change with the KWin bridge up: the busy retry, then the setting for a main
/// window that opened since the last apply.
fn on_windows_changed(cx: &mut App) {
    if !keep_above_supported(cx) {
        return;
    }
    if let Some(on) = cx.default_global::<KeepAboveState>().retry.take()
        && let Err(err) = send_keep_above(on, cx)
    {
        warn!(target: "tasklens::ui", ?err, on, "keep-above retry failed");
    }
    if !cx.default_global::<KeepAboveState>().apply_pending || main_handle(cx).is_none() {
        return;
    }
    cx.global_mut::<KeepAboveState>().apply_pending = false;
    if TlSettings::settings(cx).window.always_on_top {
        keep_above_main(true, cx);
    }
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

/// The Fluent key of a view mode's name, shared with the Processes toolbar. The caller
/// translates it.
pub fn view_mode_name(mode: TableMode) -> &'static str {
    match mode {
        TableMode::Tree => "processes-toolbar-mode-tree",
        TableMode::Grouped => "processes-toolbar-mode-grouped",
        TableMode::Flat => "processes-toolbar-mode-flat",
    }
}

/// The view-mode cycler tooltip, "Toggle view mode (current: <name>)". In History mode the
/// cycler shows `processes-toolbar-history-flat` instead; the caller picks.
pub fn view_mode_tooltip(mode: TableMode, cx: &App) -> SharedString {
    let name = shell_text(cx, view_mode_name(mode), &[]);
    shell_text(
        cx,
        VIEW_MODE_TOOLTIP,
        &[("mode", FluentValue::from(name.as_ref()))],
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

/// A request a second launch forwarded (E20-019). tl-app sends `Activate` before `SelectTab` and
/// `SelectPid`, so the main window exists for those; without one they do nothing.
/// `SetLowMemoryEnabled` never reaches here: `tl_ui::app` applies it.
fn handle_app_request(req: AppRequest, cx: &mut App) {
    match req {
        // GPUI 0.3.7 has no way to pass an XDG activation token to `activate_window`, so the
        // token is not used. It is never logged.
        AppRequest::Activate { token: _ } => activate_main(cx),
        // Called directly rather than dispatched: a window reopened by the `Activate` just before
        // has no rendered frame yet, and a dispatched action would find no listener.
        AppRequest::SelectTab(tab) => {
            with_main_view(cx, |this, window, cx| this.select_tab(tab, window, cx));
        }
        AppRequest::SelectPid(pid) => {
            with_main_view(cx, |this, window, cx| this.run_select_pid(pid, window, cx));
        }
        AppRequest::Quit => cx.dispatch_action(&Quit),
        AppRequest::SetLowMemoryEnabled(_) => {}
    }
}

/// `Activate`: brings the main window forward, or reopens it on `UiState.tabs.last_tab` when it
/// was removed. On Wayland KWin may only flash the taskbar entry. With the KWin bridge up, an
/// existing window is also activated through KWin.
fn activate_main(cx: &mut App) {
    let shown = main_handle(cx).is_some_and(|handle| {
        handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    });
    if !shown {
        reopen_main_window(cx);
        return;
    }
    if keep_above_supported(cx) {
        kwin_activate_main(cx);
    }
}

/// Sends `KWin(Activate)` for this process's window titled [`WINDOW_TITLE`]. Nothing when the
/// windows snapshot has no such window.
fn kwin_activate_main(cx: &mut App) {
    let Some(internal_id) = cx.try_global::<GlobalHub>().and_then(|hub| {
        let windows = &hub.0.read(cx).current().windows;
        windows
            .for_pid(std::process::id())
            .iter()
            .find(|w| &*w.caption == WINDOW_TITLE)
            .map(|w| Arc::clone(&w.internal_id))
    }) else {
        return;
    };
    let Some(engine) = cx.try_global::<EngineGlobal>().map(|e| e.0.clone()) else {
        return;
    };
    if let Err(err) = engine.send(Command::KWin(KWinCommand::Activate { internal_id })) {
        warn!(target: "tasklens::ui", ?err, "KWin activate not sent");
    }
}

/// Runs `f` on the main window's view inside its window. Nothing without a main window.
fn with_main_view(
    cx: &mut App,
    f: impl FnOnce(&mut MainWindow, &mut Window, &mut Context<MainWindow>),
) {
    let (Some(handle), Some(view)) = (main_handle(cx), main_window_view(cx)) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        view.update(cx, |this, cx| f(this, window, cx))
    });
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

/// `--start-minimized` with a tray to hide in: the main window opens hidden (APP-048).
fn start_hidden(cx: &App) -> bool {
    let tray = &TlSettings::settings(cx).tray;
    let start_minimized = cx
        .try_global::<StartOptions>()
        .is_some_and(|s| s.start_minimized);
    start_minimized
        && (tray.close_to_tray || tray.minimize_to_tray || tray.always_show)
        && tray_available(cx)
}

/// The once-per-process step 11 items that already ran (E20-018): a reopened window skips them.
#[derive(Default)]
struct StartupItemsDone {
    toasts: bool,
    select_pid: bool,
    start_minimized: bool,
}

impl Global for StartupItemsDone {}

/// How long `--select-pid` waits for the process to appear (E20-018).
pub const SELECT_PID_WAIT: Duration = Duration::from_secs(3);

/// A `--select-pid` waiting for its process: the hub subscription and the 3 s timeout.
struct PendingPid {
    pid: u32,
    _sub: Subscription,
    _timeout: Task<()>,
}

/// The main window's `WindowOptions` (UI-SHL-002, ADR-010, APP-048).
pub fn main_window_options(cx: &App) -> WindowOptions {
    let settings = TlSettings::settings(cx);
    let start_hidden = start_hidden(cx);
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
    // A new window is not kept above until `KeepAbove` reaches it.
    cx.default_global::<KeepAboveState>().apply_pending = true;
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
    /// The `debug.show_fps` overlay (E20-020). Rendered only while shown.
    fps: Entity<FpsOverlay>,
    /// The `debug` section version the overlay was last shown or hidden for.
    debug_version: u64,
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
    /// A next-frame callback that tells the hub this window drew is queued.
    frame_note_queued: bool,
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
    /// `--select-pid` waiting for its process (E20-018).
    pending_pid: Option<PendingPid>,
    /// The page fade-in after a tab switch: start and duration (E20-021).
    fade: Option<(Instant, Duration)>,
    /// The page container's opacity at the last render.
    #[cfg(any(test, feature = "test-hooks"))]
    page_opacity: f32,
    /// Animation frames the fade asked for.
    #[cfg(any(test, feature = "test-hooks"))]
    fade_frames: u32,
    #[cfg(any(test, feature = "test-hooks"))]
    first_frames: u32,
    /// `select_by_pid` calls made by `run_select_pid`.
    #[cfg(any(test, feature = "test-hooks"))]
    select_pid_jumps: u32,
    /// `minimize_window` calls made for `--start-minimized`.
    #[cfg(any(test, feature = "test-hooks"))]
    start_minimizes: u32,
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

        let fps = cx.new(FpsOverlay::new);

        let mut subs = SmallVec::new();
        // `debug.show_fps` shows or hides the FPS overlay (E20-020).
        subs.push(cx.observe_global::<TlSettings>(|this: &mut Self, cx| {
            let version = TlSettings::get(cx).version(Section::Debug);
            if version != this.debug_version {
                this.debug_version = version;
                this.sync_fps(cx);
                cx.notify();
            }
        }));
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
        // Health items in the status chip (E20-016), and the frame note for hidden detection.
        if let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) {
            subs.push(cx.subscribe_in(
                &hub,
                window,
                |this: &mut Self, hub, ev: &SnapshotChanged, window, cx| {
                    if ev.mask.contains(ComponentMask::HEALTH) {
                        this.sync_health(cx);
                    }
                    this.note_next_frame(hub.clone(), window, cx);
                },
            ));
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
            fps,
            debug_version: TlSettings::get(cx).version(Section::Debug),
            pages: Default::default(),
            active: start,
            picking: false,
            status_buf: String::new(),
            settings_versions: settings_versions(cx),
            style_version: metrics(cx).style_version,
            client,
            bounds: window_bounds_of(window, cx),
            save_task: None,
            frame_note_queued: false,
            applied: metrics_key(cx),
            scale_factor: window.scale_factor(),
            scale_text: scale_text(cx),
            strings_version: strings_version(cx),
            health_ticking: false,
            _health_tick: None,
            _subs: subs,
            pending_pid: None,
            fade: None,
            #[cfg(any(test, feature = "test-hooks"))]
            page_opacity: 1.0,
            #[cfg(any(test, feature = "test-hooks"))]
            fade_frames: 0,
            #[cfg(any(test, feature = "test-hooks"))]
            first_frames: 0,
            #[cfg(any(test, feature = "test-hooks"))]
            select_pid_jumps: 0,
            #[cfg(any(test, feature = "test-hooks"))]
            start_minimizes: 0,
        };
        this.sync_fps(cx);
        this.switch_to(start, false, window, cx);
        cx.on_next_frame(window, |this, window, cx| {
            this.after_first_frame(window, cx)
        });
        this
    }

    /// The FPS overlay, shown while `debug.show_fps` is on.
    pub fn fps_overlay(&self) -> &Entity<FpsOverlay> {
        &self.fps
    }

    /// Starts or stops the FPS overlay to match `debug.show_fps`.
    fn sync_fps(&mut self, cx: &mut Context<Self>) {
        let show = TlSettings::settings(cx).debug.show_fps;
        let driver = self._anim.clone();
        self.fps
            .update(cx, |fps, cx| fps.set_shown(show, &driver, cx));
    }

    /// Selects `tab`. Nothing happens for the active tab or a tab that is not built.
    pub fn select_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.active || factory_for(tab, cx).is_none() {
            return;
        }
        self.begin_switch_animation(cx);
        self.switch_to(tab, true, window, cx);
    }

    /// The tab switch animation (E20-021, UI-SHL-021): the outline slides from where it is now
    /// and the new page fades in, over `motion.tab_switch_ms`. Nothing with animations off,
    /// reduced motion or 0 ms.
    fn begin_switch_animation(&mut self, cx: &mut Context<Self>) {
        let ms = TlSettings::settings(cx).motion.tab_switch_ms;
        if !animations_on(cx) || ms == 0 {
            self.fade = None;
            return;
        }
        let duration = Duration::from_millis(u64::from(ms));
        if let Some(from) = self.tabstrip.read(cx).outline_bounds() {
            self.tabstrip
                .update(cx, |strip, cx| strip.begin_switch(from, duration, cx));
        }
        self.fade = Some((cx.background_executor().now(), duration));
    }

    /// The page container's opacity at the last render.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn page_opacity(&self) -> f32 {
        self.page_opacity
    }

    /// Animation frames the page fade asked for.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn fade_frames(&self) -> u32 {
        self.fade_frames
    }

    /// The tab strip, for the animation tests.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn tabstrip_view(&self) -> &Entity<TabStrip> {
        &self.tabstrip
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

    /// Type-to-filter (E20-A2, UI-SHL-124, PROC-044): a printable key that no binding and no
    /// focused element used, typed while a table has focus, moves focus to the filter box and
    /// goes in there. Space is left to the table (D-008). The cheap checks come first; only a
    /// printable key reads the context stack.
    fn route_type_to_filter(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !shown_on(self.active) {
            return;
        }
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        let Some(ch) = ks.key_char.as_deref().and_then(single_char) else {
            return;
        };
        if ch.is_control() || ch == ' ' {
            return;
        }
        let stack = window.context_stack();
        let table = stack.iter().any(|c| c.contains(TABLE_CONTEXT));
        let blocked = stack
            .iter()
            .any(|c| NO_TYPE_TO_FILTER.iter().any(|name| c.contains(name)));
        if !table || blocked {
            return;
        }
        self.filter
            .update(cx, |f, cx| f.begin_typing(ch, window, cx));
        cx.stop_propagation();
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

    /// The title bar view, for the pin tests.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn titlebar_view(&self) -> &Entity<TitleBarView> {
        &self.titlebar
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

    /// `select_by_pid` calls made by `run_select_pid`.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn select_pid_jumps(&self) -> u32 {
        self.select_pid_jumps
    }

    /// `minimize_window` calls made for `--start-minimized`.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn start_minimizes(&self) -> u32 {
        self.start_minimizes
    }

    /// Selects the Processes tab and jumps to `pid` as soon as a live record with that pid
    /// exists (PROC-071). After [`SELECT_PID_WAIT`] without one: the Info toast "Process <pid>
    /// not found", and the pid is dropped. A new call replaces a waiting one. Used at startup
    /// and by E20-019.
    pub fn run_select_pid(&mut self, pid: u32, window: &mut Window, cx: &mut Context<Self>) {
        if factory_for(Tab::Processes, cx).is_none() {
            return;
        }
        self.pending_pid = None;
        self.select_tab(Tab::Processes, window, cx);
        if self.try_select_pid(pid, window, cx) {
            return;
        }
        let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) else {
            return;
        };
        let sub = cx.subscribe_in(
            &hub,
            window,
            move |this: &mut Self, _, ev: &SnapshotChanged, window, cx| {
                if ev.mask.contains(ComponentMask::PROCESSES)
                    && this.pending_pid.as_ref().is_some_and(|p| p.pid == pid)
                    && this.try_select_pid(pid, window, cx)
                {
                    this.pending_pid = None;
                }
            },
        );
        let timeout = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(SELECT_PID_WAIT).await;
            let _ = this.update(cx, |this, cx| {
                if this.pending_pid.as_ref().is_some_and(|p| p.pid == pid) {
                    this.pending_pid = None;
                    let text = tr_args(
                        cx,
                        "shell-toast-pid-not-found",
                        &[("pid", FluentValue::from(pid))],
                    );
                    this.toasts
                        .update(cx, |q, cx| q.push(ToastKind::Info, text, None, None, cx));
                }
            });
        });
        self.pending_pid = Some(PendingPid {
            pid,
            _sub: sub,
            _timeout: timeout,
        });
    }

    /// Jumps to `pid` when the hub's current snapshot has a non-exited record with it.
    fn try_select_pid(&mut self, pid: u32, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let live = cx.try_global::<GlobalHub>().is_some_and(|hub| {
            hub.0.read(cx).current().processes.records.iter().any(|r| {
                r.key.pid == pid && !r.flags.contains(tl_model::process::ProcFlags::EXITED)
            })
        });
        let page = self.page_entity::<ProcessesPage>(Tab::Processes);
        let (true, Some(page)) = (live, page) else {
            return false;
        };
        page.update(cx, |p, cx| {
            p.select_by_pid(pid, JumpTexts::default(), window, cx)
        });
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.select_pid_jumps += 1;
        }
        true
    }

    /// Ctrl-pause (E20-012, UI-SHL-123, UI-PRC-017): starts when Ctrl goes down with no other
    /// modifier while the window is active, and ends when Ctrl goes up. A modifier added during
    /// the pause keeps it, so Ctrl+Shift+C and Ctrl+click work while paused.
    fn on_modifiers(
        &mut self,
        ev: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = ev.modifiers;
        let paused = cx.try_global::<ShellState>().is_some_and(|s| s.paused);
        let want = if paused {
            m.control
        } else {
            m.control
                && !m.shift
                && !m.alt
                && !m.platform
                && !m.function
                && window.is_window_active()
        };
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

    /// Tells the hub on the next frame of this window that the window drew (ADR-008 hidden
    /// detection). A snapshot change usually redraws only cached views such as the process
    /// table, so `render` here does not run, and without this note the hub counts a shown
    /// window as hidden after 1 s and the demand drops. A suspended Wayland window gets no
    /// frame, so the note stays pending there. At most one callback is queued.
    fn note_next_frame(
        &mut self,
        hub: Entity<SnapshotHub>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.frame_note_queued {
            return;
        }
        self.frame_note_queued = true;
        let this = cx.weak_entity();
        window.on_next_frame(move |_, cx| {
            let _ = this.update(cx, |this, _| this.frame_note_queued = false);
            hub.update(cx, |hub, cx| hub.note_render(cx));
        });
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

    /// `ToggleHistoryMode` (E20-025, UI-SHL-023, PROC-088): selects Processes when another tab
    /// is active, then flips `UiState.processes.history_mode`. The tab label and the Processes
    /// page follow the state. The only handler of this Action.
    fn on_toggle_history(
        &mut self,
        _: &ToggleHistoryMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active != Tab::Processes {
            self.select_tab(Tab::Processes, window, cx);
        }
        TlSettings::update(cx, |d| {
            d.state.processes.history_mode = !d.state.processes.history_mode
        });
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
        let done = cx.default_global::<StartupItemsDone>();
        let first_toasts = !mem::replace(&mut done.toasts, true);
        let first_select = !mem::replace(&mut done.select_pid, true);
        let first_minimize = !mem::replace(&mut done.start_minimized, true);
        // step 11: queued toasts (E20-018), once per process, in order
        if first_toasts && cx.has_global::<BootSettings>() {
            let queued = mem::take(&mut cx.global_mut::<BootSettings>().startup_toasts);
            for (tag, text) in queued {
                let kind = match tag {
                    ToastKindTag::Info => ToastKind::Info,
                    ToastKindTag::Success => ToastKind::Success,
                    ToastKindTag::Warning => ToastKind::Warning,
                    ToastKindTag::Error => ToastKind::Error,
                };
                self.toasts
                    .update(cx, |q, cx| q.push(kind, text, None, None, cx));
            }
        }
        // step 11: crash report (E20-018), on every main window's first frame
        super::crash_report::show_pending(window, cx);
        // step 11: --select-pid (E20-018), once per process
        let options = cx.try_global::<StartOptions>().cloned().unwrap_or_default();
        if first_select && let Some(pid) = options.select_pid {
            self.run_select_pid(pid, window, cx);
        }
        // step 11: --start-minimized without tray (E20-018), once per process
        if first_minimize && options.start_minimized && !start_hidden(cx) {
            // The GPUI test window panics on minimize_window; TitleBarProbe stands in for it.
            #[cfg(any(test, feature = "test-hooks"))]
            let probe = cx.has_global::<super::titlebar::TitleBarProbe>();
            #[cfg(not(any(test, feature = "test-hooks")))]
            let probe = false;
            if !probe {
                window.minimize_window();
            }
            #[cfg(any(test, feature = "test-hooks"))]
            {
                self.start_minimizes += 1;
            }
        }
        // step 11: helper unlock at launch (E20-026)
        // step 11: public IP auto-fetch (E20-024), once per process
        if !cx.has_global::<PublicIpAutoFetched>() {
            cx.set_global(PublicIpAutoFetched);
            auto_fetch_public_ip(cx);
        }
    }
}

// ---- FPS overlay (E20-020, UI-SHL-140) ----

/// How often the FPS overlay reads the frame statistics.
pub const FPS_TICK: Duration = Duration::from_millis(500);
/// Overlay offset from the page area's top right corner, × ui_scale.
const FPS_INSET: f32 = 8.;
/// Overlay padding, × ui_scale.
const FPS_PAD_Y: f32 = 4.;
const FPS_PAD_X: f32 = 8.;

/// The developer FPS overlay. While shown it turns on the driver's sampling and reads the
/// statistics every 500 ms; it never requests frames, so it does not change what it measures.
/// Hidden, it has no timer and the driver does not sample.
pub struct FpsOverlay {
    text: SharedString,
    /// `stats.frames` at the last tick.
    last_frames: u64,
    last_tick: Option<Instant>,
    driver: Option<WeakEntity<AnimationDriver>>,
    /// The CPU times of one tick, reused so a tick allocates only the text.
    cpu_ns: Vec<u64>,
    _tick: Option<Task<()>>,
}

impl FpsOverlay {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            text: SharedString::default(),
            last_frames: 0,
            last_tick: None,
            driver: None,
            cpu_ns: Vec::new(),
            _tick: None,
        }
    }

    pub fn is_shown(&self) -> bool {
        self._tick.is_some()
    }

    pub fn text(&self) -> &SharedString {
        &self.text
    }

    /// Shows the overlay for `driver`'s window, or hides it and drops the timer.
    pub fn set_shown(
        &mut self,
        shown: bool,
        driver: &Entity<AnimationDriver>,
        cx: &mut Context<Self>,
    ) {
        if shown == self.is_shown() {
            return;
        }
        if !shown {
            self._tick = None;
            self.driver = None;
            driver.update(cx, |d, _| {
                d.stats.sampling = false;
                d.stats.samples.clear();
            });
            cx.notify();
            return;
        }
        let (frames, stalls) = driver.update(cx, |d, _| {
            d.stats.sampling = true;
            d.stats.samples.clear();
            (d.stats.frames, d.stats.stalls)
        });
        self.last_frames = frames;
        self.last_tick = Some(cx.background_executor().now());
        self.driver = Some(driver.downgrade());
        self.text = fps_text(0, FPS_TICK, &mut [], stalls).into();
        self._tick = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(FPS_TICK).await;
                    if this.update(cx, |this, cx| this.on_timer(cx)).is_err() {
                        break;
                    }
                }
            }),
        );
        cx.notify();
    }

    fn on_timer(&mut self, cx: &mut Context<Self>) {
        let Some(driver) = self.driver.as_ref().and_then(WeakEntity::upgrade) else {
            return;
        };
        let now = cx.background_executor().now();
        let elapsed = self
            .last_tick
            .map_or(FPS_TICK, |last| now.saturating_duration_since(last));
        self.last_tick = Some(now);
        // Out of the driver for the tick, so the tick can notify this view; nothing else runs
        // in between.
        let mut stats = driver.update(cx, |d, _| mem::take(&mut d.stats));
        self.tick(&mut stats, elapsed, cx);
        driver.update(cx, |d, _| d.stats = stats);
    }

    /// Builds the text from the frames and samples since the last tick, then clears the
    /// samples.
    fn tick(&mut self, stats: &mut FrameStats, elapsed: Duration, cx: &mut Context<Self>) {
        let frames = stats.frames.saturating_sub(self.last_frames);
        self.last_frames = stats.frames;
        self.cpu_ns.clear();
        self.cpu_ns
            .extend(stats.samples.iter().map(|s| s.thread_cpu_ns));
        stats.samples.clear();
        self.text = fps_text(frames, elapsed, &mut self.cpu_ns, stats.stalls).into();
        cx.notify();
    }
}

/// `144 fps · frame p50 0.50 ms · p99 0.70 ms · stalls 0`, or `idle · stalls 2` without
/// frames. p50 and p99 are nearest-rank percentiles of `cpu_ns`, which is sorted in place.
pub fn fps_text(frames: u64, elapsed: Duration, cpu_ns: &mut [u64], stalls: u64) -> String {
    if frames == 0 {
        return format!("idle · stalls {stalls}");
    }
    let secs = elapsed.as_secs_f64();
    let fps = if secs > 0. {
        (frames as f64 / secs).round() as u64
    } else {
        0
    };
    cpu_ns.sort_unstable();
    let ms = |q: f64| -> f64 {
        if cpu_ns.is_empty() {
            return 0.;
        }
        let rank = ((q * cpu_ns.len() as f64).ceil() as usize).clamp(1, cpu_ns.len());
        cpu_ns[rank - 1] as f64 / 1e6
    };
    format!(
        "{fps} fps · frame p50 {:.2} ms · p99 {:.2} ms · stalls {stalls}",
        ms(0.5),
        ms(0.99)
    )
}

impl Render for FpsOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx);
        let mut el = div();
        *el.text_style() = numeric_style(&m, m.text.xs);
        el.debug_selector(|| "FpsOverlay".into())
            .bg(theme.menu_bg.opacity(0.9))
            .rounded(m.radius_small)
            .py(px(FPS_PAD_Y * m.ui_scale))
            .px(px(FPS_PAD_X * m.ui_scale))
            .whitespace_nowrap()
            .text_color(theme.text_primary)
            .child(self.text.clone())
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

        // The fade-in after a tab switch. The page is drawn uncached while it runs, so the
        // container's opacity reaches every frame of it.
        let opacity = self.fade.and_then(|(start, duration)| {
            let t = tween_progress(start, duration, cx.background_executor().now());
            if t.is_none() {
                self.fade = None;
            }
            t
        });
        if opacity.is_some() {
            window.request_animation_frame();
            #[cfg(any(test, feature = "test-hooks"))]
            {
                self.fade_frames += 1;
            }
        }
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.page_opacity = opacity.unwrap_or(1.0);
        }
        let page = self.pages[self.active as usize].as_ref().map(|slot| {
            if opacity.is_some() {
                slot.view.clone().into_any_element()
            } else {
                slot.view
                    .clone()
                    .cached(StyleRefinement::default().size_full())
                    .into_any_element()
            }
        });
        // Graph summary mode: the summary view takes the place of the tab strip and the page.
        let summary = (self.active == Tab::Performance)
            .then(|| self.page_entity::<PerformancePage>(Tab::Performance))
            .flatten()
            .and_then(|p| p.read(cx).summary_view());
        let shell = summary.is_none();
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
            .on_action(cx.listener(Self::on_toggle_history))
            .on_action(cx.listener(Self::on_focus_filter))
            .on_action(cx.listener(Self::on_clear_filter))
            .on_action(cx.listener(Self::on_escape_chain))
            // Bubble phase: sees only keys that no binding and no focused element used.
            .on_key_down(cx.listener(Self::route_type_to_filter))
            .child(
                AnyView::from(self.titlebar.clone()).cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(bar_height_of(client, s)),
                ),
            )
            .children(summary.map(|view| {
                div()
                    .debug_selector(|| "MainWindowSummary".into())
                    .flex_1()
                    .min_h_0()
                    .child(view.cached(StyleRefinement::default().size_full()))
            }))
            .when(shell, |el| {
                el.child(
                    AnyView::from(self.tabstrip.clone())
                        .cached(StyleRefinement::default().w_full().h(px(TAB_STRIP_H * s))),
                )
            })
            .when(shell, |el| {
                el.child(
                    div()
                        .debug_selector(|| "MainWindowPage".into())
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .when_some(opacity, |el, t| el.opacity(t))
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
                        )
                        // Above the page; not a Tab stop.
                        .children(self.fps.read(cx).is_shown().then(|| {
                            div()
                                .absolute()
                                .top(px(FPS_INSET * s))
                                .right(px(FPS_INSET * s))
                                .child(self.fps.clone())
                        })),
                )
            })
            // Last: an open context menu covers the window (E16-A3).
            .children(menu)
    }
}
