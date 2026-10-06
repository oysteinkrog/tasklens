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
//! The close decision, the window actions, reopening and size saving are E20-A1.

use std::mem;

use gpui_kit::base::Root;
use gpui_kit::component::{TitleBar, v_flex};
use gpui_kit::{
    AnyView, App, AppContext as _, Bounds, Context, Entity, EntityId, EventEmitter, FocusHandle,
    Global, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StyleRefinement,
    Styled, Subscription, WeakEntity, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowHandle, WindowOptions, div, point, px, size,
};
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::settings::{Decorations, StartTab, WindowState};
use tracing::{debug, error, info, warn};

use super::status::{CHIP_H, CHIP_RESERVE, StatusAction, StatusChanged, StatusChip};
use super::tabstrip::TabStrip;
use super::titlebar::TitleBarView;
use super::toolbar::{DefaultToolbar, ToolbarContent, ToolbarModel};
use crate::actions::{GLOBAL_CONTEXT, NextTab, PrevTab, RefreshCurrentTab, SelectTab};
use crate::anim::AnimationDriver;
use crate::app::{OpenHooks, StartOptions, UiExit, request_exit};
use crate::components::menu::context_menu_layer;
use crate::components::toast::{ToastAction, ToastKey, ToastKind, ToastQueue};
use crate::demand::GlobalDemand;
use crate::focus::focus_initial;
use crate::hub::GlobalHub;
use crate::settings_global::{Section, TlSettings};
use crate::theme::{UiMetrics, current, metrics};

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
// The match stays so page epics can add their arms; the lint goes with the first arm.
#[allow(clippy::match_single_binding)]
fn page_factory(tab: Tab) -> Option<PageFactory> {
    match tab {
        // page arms (chain edits, one per page epic): Processes E22, Performance E24, Services E27,
        // Startup, Connections, Information E28. Form:
        // Tab::Services => Some(|window, cx| { let page = cx.new(|cx| ServicesPage::new(window, cx)); PageSlot::new(page, cx) }),
        _ => None,
    }
}

/// Toolbar registration: one arm per page epic with page buttons (E22 for Processes).
// The match stays so page epics can add their arms; the lint goes with the first arm.
#[allow(clippy::match_single_binding)]
fn toolbar_for(tab: Tab) -> Box<dyn ToolbarContent> {
    match tab {
        // toolbar registrations (chain edits)
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

/// Installs `GlobalShellEvents` and `ShellState` and sets the main open hook.
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
        if let Err(err) = open_main_window(cx) {
            error!(target: "tasklens::ui", %err, "main window did not open");
            request_exit(cx, UiExit::RendererFailed(err.to_string()));
        }
    }));
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
    status: Entity<StatusChip>,
    toasts: Entity<ToastQueue>,
    /// Kept here so the driver lives as long as the window (anim.rs "Storage").
    _anim: Entity<AnimationDriver>,
    /// Index = `Tab::ALL` position. Created on first visit, kept.
    pages: [Option<PageSlot>; 6],
    active: Tab,
    status_buf: String,
    /// `(state_version, processes version)` the toolbar model was built from.
    settings_versions: (u64, u64),
    style_version: u64,
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
        let s = metrics(cx).ui_scale;
        let root_focus = cx.focus_handle();
        let built = cx
            .try_global::<ShellState>()
            .map_or_else(|| built_tabs(cx), |state| state.built_tabs.clone());
        let titlebar = cx.new(|cx| TitleBarView::new(window, cx));
        let tabstrip = cx.new(|cx| TabStrip::new(built, start, window, cx));
        let status = cx.new(StatusChip::new);
        let top = px((TITLEBAR_H + TAB_STRIP_H + TOAST_GAP) * s);
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
        // The layout and the toast top scale with ui_scale (UI-SHL-130).
        subs.push(cx.observe_global::<UiMetrics>(|this: &mut Self, cx| {
            let m = metrics(cx);
            if m.style_version == this.style_version {
                return;
            }
            this.style_version = m.style_version;
            let top = px((TITLEBAR_H + TAB_STRIP_H + TOAST_GAP) * m.ui_scale);
            this.toasts.update(cx, |queue, cx| queue.set_top(top, cx));
            cx.notify();
        }));
        // The title bar keeps the toolbar model's `window_active` itself (E20-004).

        let mut this = Self {
            root_focus,
            titlebar,
            tabstrip,
            status,
            toasts,
            _anim: anim,
            pages: Default::default(),
            active: start,
            status_buf: String::new(),
            settings_versions: settings_versions(cx),
            style_version: metrics(cx).style_version,
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

    /// The status chip. E20-012 sets its paused tag through it.
    pub fn status_chip(&self) -> &Entity<StatusChip> {
        &self.status
    }

    /// `after_first_frame` runs.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn first_frames(&self) -> u32 {
        self.first_frames
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
        let model = self.toolbar_model(cx);
        self.titlebar
            .update(cx, |bar, cx| bar.set_toolbar(model, toolbar_for(tab), cx));
        self.sync_status(cx);
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
            // window-level on_action handlers (chain edits)
            .child(
                AnyView::from(self.titlebar.clone())
                    .cached(StyleRefinement::default().w_full().h(px(TITLEBAR_H * s))),
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
