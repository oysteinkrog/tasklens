//! The Debug menu of the menu bar (E21-038, UI-SHL-042, UI-SHL-030, UI-SHL-140, APP-026,
//! APP-052, R-071).
//!
//! Shown only in debug mode (`AppEnv.debug`: `--debug` or `TASKLENS_DEBUG=1`). Rows: "Show FPS",
//! "Continuous rendering", "Enable all columns", "Add random process minigraphs", a separator
//! and "Collector health". "Continuous rendering" is a session flag, never persisted. While it
//! is on, a frame callback chain on the active window asks for the next frame every frame. This
//! is the one place allowed to bypass the `AnimationDriver` rule (APP-026, debug only).
//! "Enable all columns" dispatches `DebugEnableAllColumns`, whose handler sends the table's
//! `EnableAllColumns` and shows the toast, because the table pushes none (R-180).

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, App, Entity, FocusHandle, Global, SharedString, Window, gpui};
use serde::Deserialize;

use super::menus_view::active_table;
use crate::actions::{OpenCollectorHealth, ToggleSetting};
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::tr;
use crate::pages::minigraph::actions::AddRandomMinigraphs;
use crate::settings_global::{SettingKey, TlSettings};
use crate::table::actions::EnableAllColumns;
use crate::table::columns::TableId;

gpui_kit::actions!(
    tasklens,
    [
        /// Turns continuous rendering on or off (Debug menu, APP-026).
        ToggleContinuousRendering
    ]
);

/// Enables every column of `table` and shows "All columns enabled". The Debug menu row sends it.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct DebugEnableAllColumns {
    pub table: TableId,
}

/// Session state of continuous rendering.
#[derive(Default)]
struct ContinuousRendering {
    on: bool,
    /// A frame callback is queued. Stops a second chain when the item is toggled twice before
    /// the next frame.
    armed: bool,
    /// Frames requested so far.
    requested: u64,
}

impl Global for ContinuousRendering {}

/// Whether the debug flag is set.
pub fn debug_enabled(cx: &App) -> bool {
    cx.try_global::<crate::app::AppEnv>()
        .is_some_and(|e| e.debug)
}

/// Whether continuous rendering is on.
pub fn continuous_on(cx: &App) -> bool {
    cx.try_global::<ContinuousRendering>().is_some_and(|c| c.on)
}

/// Frames requested by continuous rendering since the session began.
#[cfg(any(test, feature = "test-hooks"))]
pub fn continuous_requested(cx: &App) -> u64 {
    cx.try_global::<ContinuousRendering>()
        .map_or(0, |c| c.requested)
}

/// Requests one frame and queues the next link of the chain. Ends when the flag is off.
fn arm(window: &mut Window, cx: &mut App) {
    let state = cx.default_global::<ContinuousRendering>();
    if !state.on || state.armed {
        return;
    }
    state.armed = true;
    state.requested += 1;
    window.refresh();
    window.on_next_frame(|window, cx| {
        cx.default_global::<ContinuousRendering>().armed = false;
        arm(window, cx);
    });
}

/// Registers the two action handlers. `menubar::init` calls it.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &ToggleContinuousRendering, cx| {
        let state = cx.default_global::<ContinuousRendering>();
        state.on = !state.on;
        if !state.on {
            return;
        }
        let Some(handle) = cx.active_window() else {
            return;
        };
        // The window is busy dispatching this action, so start the chain right after.
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| arm(window, cx));
        });
    });
    cx.on_action(|action: &DebugEnableAllColumns, cx| {
        let table = action.table;
        let Some(handle) = cx.active_window() else {
            return;
        };
        let text = tr(cx, "shell-toast-all-columns");
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(EnableAllColumns { table }), cx);
                if let Some(queue) = ToastQueue::of_window(window, cx) {
                    queue.update(cx, |q, cx| q.push(ToastKind::Info, text, None, None, cx));
                }
            });
        });
    });
}

/// Builds the Debug menu, or `None` when debug mode is off. Item actions go to `return_focus`.
pub fn build(
    window: &mut Window,
    cx: &mut App,
    return_focus: &FocusHandle,
) -> Option<Entity<TlMenuPopup>> {
    if !debug_enabled(cx) {
        return None;
    }
    let show_fps = cx
        .try_global::<TlSettings>()
        .is_some_and(|_| TlSettings::settings(cx).debug.show_fps);
    let continuous = continuous_on(cx);
    let table = active_table(cx);
    Some(TlMenu::build(
        window,
        cx,
        return_focus.clone(),
        move |menu, _, cx| {
            let all_columns: SharedString = tr(cx, "shell-menu-debug-all-columns");
            let menu = menu
                .check_item(
                    tr(cx, "shell-menu-debug-show-fps"),
                    show_fps,
                    Box::new(ToggleSetting(SettingKey::DebugShowFps)),
                )
                .check_item(
                    tr(cx, "shell-menu-debug-continuous"),
                    continuous,
                    Box::new(ToggleContinuousRendering),
                );
            let menu = match table {
                Some(table) => menu.item(all_columns, Box::new(DebugEnableAllColumns { table })),
                None => menu.item_disabled(all_columns, Some(tr(cx, "shell-menu-view-no-table"))),
            };
            menu.item(
                tr(cx, "shell-menu-debug-random-minigraphs"),
                Box::new(AddRandomMinigraphs),
            )
            .separator()
            .item(
                tr(cx, "shell-menu-debug-collector-health"),
                Box::new(OpenCollectorHealth),
            )
        },
    ))
}
