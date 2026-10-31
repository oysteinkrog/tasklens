//! The File menu of the menu bar (E21-A1, UI-SHL-031, UI-SHL-032, APP-012, APP-016).
//!
//! From P3 the menu reads "Run...", "Search File Handles...", a separator, "Exit tasklens".
//! "Run..." dispatches `OpenRunDialog`; E23 handles it by opening or focusing "tasklens Run".
//! "Search File Handles..." has no shortcut and dispatches `OpenHandleSearch`; E23 handles it by
//! opening or focusing "tasklens Handle Search". "Exit tasklens"
//! dispatches `Quit`, which quits even when the tray settings are on (UI-SHL-113); E20 owns its
//! handler. Shortcut hints ("Ctrl+N", "Ctrl+Q") come from the live bindings in the menu's action
//! context. From P4 "Open System Monitor" goes after "Search File Handles...". It shows only
//! when `plasma-systemmonitor`, `gnome-system-monitor` or `missioncenter` is on `PATH`; `init`
//! probes `PATH` once at startup on the background executor. The separator above "Exit
//! tasklens" is dropped when nothing is above it.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt as _;

use gpui_kit::{App, AppContext as _, AsyncApp, Entity, FocusHandle, Global, Window};
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest, LaunchSpec, LaunchWhat};

use crate::action_tracker::ToastTexts;
use crate::actions::{OpenHandleSearch, OpenRunDialog, Quit};
use crate::components::menu::{TlMenu, TlMenuPopup};
use crate::components::toast::ToastQueue;
use crate::host::HostContext;
use crate::i18n::tr;

gpui_kit::actions!(
    tasklens,
    [
        /// File > Open System Monitor: starts the desktop's own system monitor.
        OpenSystemMonitor
    ]
);

/// The desktop system monitors, in the order they are tried.
const SYSTEM_MONITORS: [&str; 3] = [
    "plasma-systemmonitor",
    "gnome-system-monitor",
    "missioncenter",
];

/// The system monitor found on `PATH`, or `None` while the probe runs or when none is installed.
/// A test sets it before `init` to skip the probe.
pub struct SystemMonitor(pub Option<&'static str>);

impl Global for SystemMonitor {}

/// The first of `SYSTEM_MONITORS` that is an executable file in a `path` directory.
fn find_system_monitor(path: Option<&OsStr>) -> Option<&'static str> {
    let dirs: Vec<_> = std::env::split_paths(path?).collect();
    SYSTEM_MONITORS.into_iter().find(|name| {
        dirs.iter().any(|dir| {
            std::fs::metadata(dir.join(name))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

/// Registers the File menu actions and starts the `PATH` probe unless a result is already set.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenSystemMonitor, cx| open_system_monitor(cx));
    if cx.has_global::<SystemMonitor>() {
        return;
    }
    cx.set_global(SystemMonitor(None));
    let path = std::env::var_os("PATH");
    let probe = cx.background_spawn(async move { find_system_monitor(path.as_deref()) });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let found = probe.await;
        cx.update(|cx| cx.set_global(SystemMonitor(found)));
    })
    .detach();
}

/// Launches the found system monitor. A failure shows a toast in the active window.
fn open_system_monitor(cx: &mut App) {
    let Some(name) = cx.try_global::<SystemMonitor>().and_then(|g| g.0) else {
        return;
    };
    let Some(handle) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, cx| {
            // The monitor was found on this PC's `PATH` and opens on this desktop: the local host.
            let Some(tracker) = HostContext::local(cx).tracker().cloned() else {
                return;
            };
            let Some(queue) = ToastQueue::of_window(window, cx) else {
                return;
            };
            let request = ActionRequest {
                id: ActionId(0),
                kind: ActionKind::Launch(LaunchSpec {
                    what: LaunchWhat::Command(name.into()),
                    as_admin: false,
                    cwd: None,
                }),
                targets: SmallVec::new(),
                privileged: false,
            };
            let texts = ToastTexts::new("", tr(cx, "shell-toast-system-monitor-failed"));
            tracker.update(cx, |t, cx| {
                t.track_or_toast(request, queue.downgrade(), texts, cx);
            });
        });
    });
}

/// Builds the File menu. Item actions go to `return_focus`.
pub fn build(window: &mut Window, cx: &mut App, return_focus: &FocusHandle) -> Entity<TlMenuPopup> {
    TlMenu::build(window, cx, return_focus.clone(), |menu, _, cx| {
        let menu = menu
            .item(tr(cx, "shell-menu-file-run"), Box::new(OpenRunDialog))
            .item(
                tr(cx, "shell-menu-file-handle-search"),
                Box::new(OpenHandleSearch),
            );
        let menu = if cx.try_global::<SystemMonitor>().and_then(|g| g.0).is_some() {
            menu.item(
                tr(cx, "shell-menu-file-system-monitor"),
                Box::new(OpenSystemMonitor),
            )
        } else {
            menu
        };
        menu.separator()
            .item(tr(cx, "shell-menu-file-exit"), Box::new(Quit))
    })
}
