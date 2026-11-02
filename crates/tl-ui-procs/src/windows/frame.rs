//! Frame values for the process windows: kind, title, sizes and the Esc order (UI-PWN-001,
//! UI-FND-021, R-073, R-127, APP-045, D-006).
//!
//! Each window opens through `TlSecondaryWindow`; this file only fixes what the foundation leaves
//! to the owner. Sizes are logical px at `ui_scale_pct = 100` and scale with `UiMetrics` when the
//! window opens. Window position is never saved (ADR-010).

use gpui_kit::base::Root;
use gpui_kit::{AnyWindowHandle, App, Entity, Pixels, Render, SharedString, Window, WindowHandle};
use gpui_kit::{px, size};

use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::theme::metrics::metrics;

/// Window title of the Set Affinity dialog.
pub const TITLE_SET_AFFINITY: &str = "tasklens Set Affinity";
/// Window title of the custom Set Priority dialog.
pub const TITLE_SET_PRIORITY: &str = "tasklens Set Priority";
/// Window title of the real-time signal dialog.
pub const TITLE_SEND_SIGNAL: &str = "tasklens Send Signal";
/// Window title of the Run window.
pub const TITLE_RUN: &str = "tasklens Run";
/// Window title of the Handle Search window.
pub const TITLE_HANDLE_SEARCH: &str = "tasklens Handle Search";

/// The process windows of UI-PWN-001.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PwnWindow {
    ProcessProperties,
    AppProperties,
    SetAffinity,
    SetPriority,
    RealtimeSignal,
    Run,
    HandleSearch,
}

impl PwnWindow {
    pub const ALL: [Self; 7] = [
        Self::ProcessProperties,
        Self::AppProperties,
        Self::SetAffinity,
        Self::SetPriority,
        Self::RealtimeSignal,
        Self::Run,
        Self::HandleSearch,
    ];
}

/// The frame values of one process window, in logical px at `ui_scale_pct = 100`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PwnFrameSpec {
    pub kind: TlWindowKind,
    pub default_size: gpui_kit::Size<Pixels>,
    pub min_size: gpui_kit::Size<Pixels>,
    pub resizable: bool,
    /// Modal to the window that opened it.
    pub modal: bool,
}

const fn frame(
    kind: TlWindowKind,
    default: (f32, f32),
    min: (f32, f32),
    resizable: bool,
    modal: bool,
) -> PwnFrameSpec {
    PwnFrameSpec {
        kind,
        default_size: size(px(default.0), px(default.1)),
        min_size: size(px(min.0), px(min.1)),
        resizable,
        modal,
    }
}

/// Height a content-sized dialog opens at before its view fits the window to the content: the
/// Confirmation kind minimum (UI-FND-021). Set Affinity uses the bottom of its 360 to 640 range.
const DIALOG_START_HEIGHT: f32 = 120.;
/// Set Affinity height range; the dialog fits its content inside it (UI-PWN-021).
pub const AFFINITY_HEIGHT: (f32, f32) = (360., 640.);

/// The UI-PWN-001 table. "Content" heights start at the values above; the dialog view resizes
/// the window to its content and keeps that height as the minimum.
pub const fn spec(w: PwnWindow) -> PwnFrameSpec {
    use TlWindowKind::{Confirmation, Properties, Run, Tool};
    match w {
        PwnWindow::ProcessProperties => frame(Properties, (900., 640.), (640., 400.), true, false),
        PwnWindow::AppProperties => frame(Properties, (720., 560.), (640., 400.), true, false),
        PwnWindow::SetAffinity => {
            let wh = (640., AFFINITY_HEIGHT.0);
            frame(Confirmation, wh, wh, false, true)
        }
        PwnWindow::SetPriority => {
            let wh = (440., DIALOG_START_HEIGHT);
            frame(Confirmation, wh, wh, false, true)
        }
        PwnWindow::RealtimeSignal => {
            let wh = (360., DIALOG_START_HEIGHT);
            frame(Confirmation, wh, wh, false, true)
        }
        PwnWindow::Run => frame(Run, (480., 248.), (480., 248.), false, false),
        PwnWindow::HandleSearch => frame(
            Tool { resizable: true },
            (860., 560.),
            (560., 360.),
            true,
            false,
        ),
    }
}

/// `Process Properties - {name} - PID {pid}`, plus ` (not running)` once the process exited.
pub fn props_title(name: &str, pid: u32, exited: bool) -> SharedString {
    let suffix = if exited { " (not running)" } else { "" };
    format!("Process Properties - {name} - PID {pid}{suffix}").into()
}

/// `App Properties - {name}`, plus ` (not running)` once the app has no processes.
pub fn app_props_title(name: &str, exited: bool) -> SharedString {
    let suffix = if exited { " (not running)" } else { "" };
    format!("App Properties - {name}{suffix}").into()
}

/// The `TlWindowSpec` `open_pwn` opens, with sizes × `ui_scale`. Only a modal dialog keeps
/// `parent`; every other window centres on the display (`Bounds::centered(None, ..)`). A caller
/// that registers the window adds its `WindowKey` with `.key(..)` and opens it itself.
pub fn pwn_window_spec(
    cx: &App,
    w: PwnWindow,
    title: SharedString,
    parent: Option<AnyWindowHandle>,
) -> TlWindowSpec {
    let s = metrics(cx).ui_scale;
    let f = spec(w);
    let scaled = |v: gpui_kit::Size<Pixels>| size(v.width * s, v.height * s);
    let mut out =
        TlWindowSpec::new(f.kind, title, scaled(f.default_size)).min_size(scaled(f.min_size));
    if let Some(parent) = parent.filter(|_| f.modal) {
        out = out.parent(parent);
    }
    out
}

/// Opens process window `w` through `TlSecondaryWindow::open`. Returns the open error and opens
/// nothing on failure.
pub fn open_pwn<V: Render>(
    cx: &mut App,
    w: PwnWindow,
    title: SharedString,
    parent: Option<AnyWindowHandle>,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> gpui_kit::Result<WindowHandle<Root>> {
    let spec = pwn_window_spec(cx, w, title, parent);
    TlSecondaryWindow::open(cx, spec, build)
}

/// What one Esc press does in a process window (UI-PWN-001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscStep {
    /// Close the topmost popup or menu.
    ClosePopup,
    /// Cancel the running scan or search (Strings, Handle Search).
    CancelScan,
    /// Clear the focused, non-empty filter field.
    ClearFilter,
    /// Close the window; a modal dialog runs its Cancel.
    CloseWindow,
}

/// The first step that applies, in the UI-PWN-001 order.
pub fn esc_step(popup_open: bool, scan_running: bool, filter_focused_non_empty: bool) -> EscStep {
    if popup_open {
        EscStep::ClosePopup
    } else if scan_running {
        EscStep::CancelScan
    } else if filter_focused_non_empty {
        EscStep::ClearFilter
    } else {
        EscStep::CloseWindow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pwn_esc_order_scan_then_filter_then_close() {
        assert_eq!(esc_step(true, true, true), EscStep::ClosePopup);
        assert_eq!(esc_step(false, true, true), EscStep::CancelScan);
        assert_eq!(esc_step(false, false, true), EscStep::ClearFilter);
        assert_eq!(esc_step(false, false, false), EscStep::CloseWindow);
    }

    #[test]
    fn titles_format() {
        assert_eq!(
            props_title("firefox", 4321, true).as_ref(),
            "Process Properties - firefox - PID 4321 (not running)"
        );
        assert_eq!(
            props_title("firefox", 4321, false).as_ref(),
            "Process Properties - firefox - PID 4321"
        );
        assert_eq!(
            app_props_title("Firefox", false).as_ref(),
            "App Properties - Firefox"
        );
        assert_eq!(
            app_props_title("Firefox", true).as_ref(),
            "App Properties - Firefox (not running)"
        );
    }

    #[test]
    fn run_spec_is_fixed() {
        let run = spec(PwnWindow::Run);
        assert_eq!(run.default_size, size(px(480.), px(248.)));
        assert_eq!(run.min_size, size(px(480.), px(248.)));
        assert!(!run.resizable);
        assert!(!run.modal);
    }

    #[test]
    fn spec_resizable_matches_kind() {
        for w in PwnWindow::ALL {
            let f = spec(w);
            assert_eq!(f.resizable, f.kind.resizable(), "{w:?}");
            assert_eq!(f.modal, f.kind.modal(), "{w:?}");
        }
    }
}
