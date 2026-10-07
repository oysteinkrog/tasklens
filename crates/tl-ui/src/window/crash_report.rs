//! "tasklens Crash Report": shown once after a crash, with "Open log folder", "Copy report" and
//! "Close" (UI-SHL-093, APP-055, R-049, ADR-015, UI-FND-021).
//!
//! tl-app's panic hook writes the report files and decides which one is pending: the newest
//! `crash-<t>.txt` without a `crash-<t>.seen` marker. It passes it as `AppEnv.pending_crash`, with
//! `AppEnv.mark_crash_seen` to write the marker (R-171). tl-ui reads no file (ADR-011).
//!
//! [`show_pending`] runs after the main window's first frame. It opens the window when a report
//! is pending and the main window is visible, then marks the report seen, once per run. While
//! the main window is hidden (started to the tray) it waits and runs again when the window is
//! shown. A Tool window keyed `WindowKey::CrashReport`, 480 × 220 × ui_scale, not resizable.
//! Focus starts on "Close", so Enter closes; Esc and Ctrl+W close through the frame.
//!
//! The report text is shown and copied only on request; nothing from it is logged.

use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, FocusHandle, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px, size,
};
use tracing::warn;

use super::about::{file_uri, send_open_uri};
use super::registry::{GlobalRegistry, WindowKey};
use crate::action_tracker::ToastTexts;
use crate::app::AppEnv;
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::icons::{TlIcon, icon};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{
    CloseSecondaryWindow, TlSecondaryWindow, TlWindowKind, TlWindowSpec,
};
use crate::i18n::tr;
use crate::text::style::{t_base, t_sm};
use crate::theme::{current, metrics};
use crate::util::clipboard_write;

gpui_kit::actions!(
    tasklens,
    [
        /// Copies the crash report and shows "Copied to clipboard".
        CopyCrashReport,
        /// Opens the log folder.
        OpenCrashLogFolder
    ]
);

/// Window size, × ui_scale. Not resizable, so it is also the minimum size.
const SIZE: (f32, f32) = (480., 220.);
/// Warning icon edge, × ui_scale.
const ICON: f32 = 24.;

const TITLE: &str = "shell-crash-title";
const LINE_1: &str = "shell-crash-line-1";
const LINE_2: &str = "shell-crash-line-2";
const OPEN_LOGS: &str = "shell-crash-open-logs";
const COPY: &str = "shell-crash-copy";
const CLOSE: &str = "shell-crash-close";
const COPIED: &str = "shell-toast-copied";
/// The shared "Failed to open file location" text (APP-044).
const LOGS_FAILED: &str = "shell-about-logs-failed";

/// What this run did with the pending report.
#[derive(Default)]
struct CrashShown {
    /// The window was shown this run.
    shown: bool,
    /// Waits for the hidden main window to be shown.
    wait: Option<Subscription>,
}

impl Global for CrashShown {}

/// Shows the pending crash report once, if there is one. `window` is the main window: while it
/// is not visible this waits and runs again when it is shown.
pub fn show_pending(window: &mut Window, cx: &mut App) {
    if cx.try_global::<CrashShown>().is_some_and(|c| c.shown || c.wait.is_some()) {
        return;
    }
    if !cx.try_global::<AppEnv>().is_some_and(|e| e.pending_crash.is_some()) {
        return;
    }
    if window.is_visible() {
        open_pending(cx);
        return;
    }
    let wait = window.observe_window_visibility(cx, |visibility, _, cx| {
        if visibility.is_visible() {
            cx.defer(|cx| {
                cx.default_global::<CrashShown>().wait = None;
                open_pending(cx);
            });
        }
    });
    cx.default_global::<CrashShown>().wait = Some(wait);
}

/// Opens the window for the pending report and marks it seen, once per run.
fn open_pending(cx: &mut App) {
    if cx.try_global::<CrashShown>().is_some_and(|c| c.shown) {
        return;
    }
    let Some((text, mark_seen)) = cx.try_global::<AppEnv>().and_then(|e| {
        let report = e.pending_crash.as_ref()?;
        Some((report.text.clone(), e.mark_crash_seen.clone()))
    }) else {
        return;
    };
    cx.default_global::<CrashShown>().shown = true;
    let s = metrics(cx).ui_scale;
    let window_size = size(px(SIZE.0 * s), px(SIZE.1 * s));
    let spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: false },
        tr(cx, TITLE),
        window_size,
    )
    .min_size(window_size)
    .key(WindowKey::CrashReport);
    let open = move |cx: &mut App| {
        let handle = TlSecondaryWindow::open(cx, spec, |window, cx| {
            cx.new(|cx| CrashReportWindow::new(text, window, cx))
        })?;
        Ok(handle.into())
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            registry.update(cx, |r, cx| r.open_or_focus(WindowKey::CrashReport, cx, open));
        }
        None => {
            if let Err(e) = open(cx) {
                warn!(target: "tasklens::ui", ?e, "the crash report window did not open");
            }
        }
    }
    mark_seen();
}

/// The content of the Crash Report window.
pub struct CrashReportWindow {
    report_text: Arc<str>,
    close_focus: FocusHandle,
}

impl CrashReportWindow {
    fn new(report_text: Arc<str>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let close_focus = cx.focus_handle();
        close_focus.focus(window, cx);
        Self {
            report_text,
            close_focus,
        }
    }

    /// The report text the window copies.
    pub fn report_text(&self) -> &str {
        &self.report_text
    }

    fn on_copy(&mut self, _: &CopyCrashReport, window: &mut Window, cx: &mut Context<Self>) {
        if clipboard_write(cx, self.report_text.to_string()).is_err() {
            return;
        }
        let copied = tr(cx, COPIED);
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| {
                q.push(ToastKind::Success, copied, None, None, cx)
            });
        }
    }

    fn on_open_logs(&mut self, _: &OpenCrashLogFolder, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = cx.try_global::<AppEnv>().map(|e| e.logs_dir.clone()) else {
            warn!(target: "tasklens::ui", "no AppEnv; the log folder was not opened");
            return;
        };
        let texts = ToastTexts::new("", tr(cx, LOGS_FAILED));
        send_open_uri(file_uri(&dir), texts, window, cx);
    }
}

impl Render for CrashReportWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let mut line_1 = div()
            .text_color(theme.text_primary)
            .child(tr(cx, LINE_1));
        *line_1.text_style() = t_base(&m);
        let mut line_2 = div()
            .text_color(theme.text_secondary)
            .child(tr(cx, LINE_2));
        *line_2.text_style() = t_sm(&m);
        let label = |key: &str| -> SharedString { tr(cx, key) };
        let footer = div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(m.sp[4])
            .child(
                TlButton::new("CrashReport-open-logs", TlButtonKind::Secondary)
                    .label(label(OPEN_LOGS))
                    .action(Box::new(OpenCrashLogFolder)),
            )
            .child(
                TlButton::new("CrashReport-copy", TlButtonKind::Secondary)
                    .label(label(COPY))
                    .action(Box::new(CopyCrashReport)),
            )
            .child(
                TlButton::new("CrashReport-close", TlButtonKind::Primary)
                    .label(label(CLOSE))
                    .focus_handle(self.close_focus.clone())
                    .action(Box::new(CloseSecondaryWindow)),
            );
        div()
            .id("CrashReport")
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_open_logs))
            .size_full()
            .flex()
            .flex_col()
            .justify_between()
            .p(px(16. * m.ui_scale))
            .bg(theme.list_bg)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(m.sp[4])
                    .child(
                        div().flex_none().child(icon(
                            TlIcon::TriangleAlert,
                            px(ICON * m.ui_scale),
                            theme.status.warning,
                        )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(m.sp[2])
                            .child(line_1)
                            .child(line_2),
                    ),
            )
            .child(footer)
    }
}
