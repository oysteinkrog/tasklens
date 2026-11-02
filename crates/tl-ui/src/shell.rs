//! Shell types that pages, windows and other lower layers name (ADR-020, tasklens-zljr5.15).
//!
//! The shell (`window/`) owns the main window, the status chip, the toolbar and the title bar.
//! This module holds the parts of them that lower layers use as plain types: the page trait, the
//! shell events and the shell state, the status chip's event and action types, the toolbar's
//! content trait and model, and the title bar's keep-above check and test probe. The `window/`
//! modules re-export each item at its old path. [`ShellApi`] holds the shell services lower
//! layers call (tasklens-zljr5.16).

use gpui_kit::base::Root;
use gpui_kit::{
    Action, AnyElement, AnyWeakEntity, App, Context, Entity, EventEmitter, FocusHandle, Global,
    Render, SharedString, Window, WindowHandle,
};
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
use gpui_kit::{Pixels, Point};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::health::HelperState;
use tl_model::table::TableMode;
use tl_model::validity::Validity;
use tracing::debug;

pub use crate::app_env::APP_ID;
use crate::components::toast::{ToastAction, ToastKey, ToastKind};
use crate::host::HostContext;
use crate::registry::WindowKey;

/// The window title the KWin task switcher shows.
pub const WINDOW_TITLE: &str = "tasklens";

/// The window picker's "No window selected" text (E20-027).
pub(crate) const PICK_NONE_TEXT: &str = "shell-toast-pick-none";

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

/// Emitted when the user clears the Processes memory group chip with its x or with Esc in the
/// empty filter box (D-036, UI-PRF-060).
#[derive(Clone, Copy, Debug)]
pub struct MemGroupCleared;

impl EventEmitter<FilterChanged> for ShellEvents {}
impl EventEmitter<PauseChanged> for ShellEvents {}
impl EventEmitter<MemGroupCleared> for ShellEvents {}

/// The local host's shell event bus. Read only through [`crate::host::HostContext`].
pub struct GlobalShellEvents(pub Entity<ShellEvents>);

impl Global for GlobalShellEvents {}

/// The privileged helper is installed: `/usr/lib/tasklens/tl-helper` exists. Checked once per
/// process in `window::main::init`; a test may set it first.
pub struct HelperInstalled(pub bool);

impl Global for HelperInstalled {}

/// Shell state other parts of the UI read.
pub struct ShellState {
    /// `gpui_kit::open_window` roots every window in the kit `Root`, so the handle is typed on
    /// `Root`. `main_view` is the `MainWindow` inside it.
    pub main_window: Option<WindowHandle<Root>>,
    /// The `MainWindow` view, type-erased so this module does not name the shell. Read through
    /// `window::main::main_window_view`, which downcasts it. Works inside the window's own
    /// updates, where the window's root cannot be read.
    pub main_view: Option<AnyWeakEntity>,
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
    /// The host the main window shows, set when it opens and on every host switch. Read
    /// through [`HostContext::shown`].
    pub shown_host: Option<HostContext>,
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
            shown_host: None,
        }
    }
}

/// Chip height, × ui_scale. Tables add this as bottom padding (UI-SHL-050).
pub const CHIP_H: f32 = 26.0;

/// Emitted by a page entity when its `status_text` output changed (counts or selection size).
#[derive(Clone, Copy, Debug)]
pub struct StatusChanged;

/// How a [`StatusAction`] is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusActionKind {
    /// " | " and then the label, underlined on hover.
    Link,
    /// A `TlBadge` Info, one space after the text before it.
    Badge,
}

/// A clickable part a page adds after its text (for example E22's " | 42 restricted" link and its
/// "Administrator mode" badge, P3). Returned by `ShellPage::status_actions` (E20-008).
pub struct StatusAction {
    pub kind: StatusActionKind,
    pub label: SharedString,
    pub tooltip: SharedString,
    /// Dispatched on click, Enter or Space.
    pub action: Box<dyn Action>,
}

impl Clone for StatusAction {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            label: self.label.clone(),
            tooltip: self.tooltip.clone(),
            action: self.action.boxed_clone(),
        }
    }
}

impl PartialEq for StatusAction {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.label == other.label
            && self.tooltip == other.tooltip
            && self.action.name() == other.action.name()
            && self.action.partial_eq(other.action.as_ref())
    }
}

/// What the toolbar needs to know to draw its buttons. The title bar keeps it current.
#[derive(Clone, PartialEq, Debug)]
pub struct ToolbarModel {
    /// The open tab.
    pub tab: Tab,
    /// `UiState.processes.history_mode`.
    pub history_mode: bool,
    /// `settings.processes.hide_system`.
    pub hide_system: bool,
    /// `settings.processes.mode`.
    pub mode: TableMode,
    /// `HealthSnapshot.helper`. `Missing` before P3.
    pub helper: HelperState,
    /// `/usr/lib/tasklens/tl-helper` exists. Checked once at startup by E20-026; false until then.
    pub helper_installed: bool,
    /// `Snapshot.windows.header.validity != Unsupported`.
    pub picker_available: bool,
    /// Window picking mode is on.
    pub picking: bool,
    /// The main window is the active window.
    pub window_active: bool,
}

/// The page buttons of one tab. One per page epic that has page buttons. Registered in
/// `window/main.rs` (`toolbar_for`).
pub trait ToolbarContent: 'static {
    /// Page buttons for slots 2 and 3, left to right, at most 2. Build them with
    /// `toolbar_toggle` and `toolbar_icon` (`components/toolbar_button.rs`).
    fn buttons(
        &self,
        model: &ToolbarModel,
        window: &mut Window,
        cx: &mut App,
    ) -> SmallVec<[AnyElement; 2]>;
}

/// True when the pin can work: `Snapshot.windows.header.validity` is Valid or Stale (KWin bridge
/// up). False without a hub. The pin acts on this PC's own windows, so the local host's hub.
pub fn keep_above_supported(cx: &App) -> bool {
    HostContext::local_hub(cx).is_some_and(|hub| {
        matches!(
            hub.read(cx).current().windows.header.validity,
            Validity::Valid | Validity::Stale
        )
    })
}

/// The shell's toast service: kind, text, optional owner key and optional action button. The
/// shell installs the function that pushes to the main window's toast queue.
pub type PushToastFn = fn(&mut App, ToastKind, SharedString, Option<ToastKey>, Option<ToastAction>);

/// Shell services that lower layers call without naming the shell (ADR-020 "ShellApi").
/// `window::main::init` installs it. CS-C4 adds more fields.
pub struct ShellApi {
    /// Pushes a toast to the main window's queue.
    pub toast: PushToastFn,
}

impl Global for ShellApi {}

impl ShellApi {
    /// Pushes a toast to the main window's queue. Does nothing when no shell is installed (a
    /// page-host test) or when the main window is closed.
    pub fn push_toast(
        cx: &mut App,
        kind: ToastKind,
        text: impl Into<SharedString>,
        key: Option<ToastKey>,
        action: Option<ToastAction>,
    ) {
        let Some(toast) = cx.try_global::<ShellApi>().map(|api| api.toast) else {
            debug!(target: "tasklens::ui", ?kind, "no shell, toast dropped");
            return;
        };
        toast(cx, kind, text.into(), key, action);
    }
}

/// Test stand-in for the platform window. The GPUI test window panics on `start_window_move`,
/// `zoom_window`, `minimize_window` and `show_window_menu`, always reports server decorations
/// and is never maximized. When this Global is installed the shell records those four calls here
/// instead of making them, and reads the overrides below.
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
#[derive(Default)]
pub struct TitleBarProbe {
    /// Overrides `window.window_decorations()`: true for client decorations.
    pub client: Option<bool>,
    /// Overrides `window.is_maximized()`.
    pub maximized: Option<bool>,
    /// Overrides `window.window_controls()`.
    pub controls: Option<gpui_kit::WindowControls>,
    /// `start_window_move` calls.
    pub moves: u32,
    /// `zoom_window` calls.
    pub zooms: u32,
    /// `minimize_window` calls.
    pub minimizes: u32,
    /// `show_window_menu` positions.
    pub window_menus: Vec<Point<Pixels>>,
}

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
impl Global for TitleBarProbe {}
