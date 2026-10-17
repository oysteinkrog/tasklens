//! Process-action confirmations (UI-PRC-014, UI-PWN-025, PROC-059, D-009, R-036).
//!
//! One helper for the Processes page, the Properties windows and Handle Search. Every
//! confirmation is a `TlDialog` window titled "tasklens", modal to the window that asked.
//! Destructive confirmations focus Cancel first, the others focus the action button
//! (UI-FND-021). The texts hold names and PIDs only, never command lines.

use std::cell::Cell;
use std::rc::Rc;

use fluent_bundle::FluentValue;
use gpui_kit::{AnyWindowHandle, App, SharedString};
use tl_model::units;

use crate::components::dialog::{DialogResult, TlDialog};
use crate::i18n::{tr, tr_args};

/// The action a protected-target confirmation names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectedAction {
    /// End Task, End Process Tree.
    End,
    /// End Process (SIGKILL).
    Kill,
    /// Freeze App.
    Freeze,
    /// Suspend.
    Suspend,
    /// Send Signal.
    Signal,
}

impl ProtectedAction {
    /// The Fluent key stem: `processes-confirm-protected-<stem>-title` and so on. End keeps
    /// the unprefixed keys.
    fn prefix(self) -> &'static str {
        match self {
            Self::End => "processes-confirm-protected",
            Self::Kill => "processes-confirm-protected-kill",
            Self::Freeze => "processes-confirm-protected-freeze",
            Self::Suspend => "processes-confirm-protected-suspend",
            Self::Signal => "processes-confirm-protected-signal",
        }
    }
}

/// What the confirmation is about. Names, counts and owners come from the targets captured
/// when the action started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    /// A signal, freeze or kill on a protected target. `action` picks the title, text and
    /// button, so Freeze says "Freeze" and not "End". `tree_count` is set for End Process
    /// Tree, and the protected text then keeps the tree size.
    Protected {
        action: ProtectedAction,
        name: SharedString,
        tree_count: Option<u32>,
    },
    /// End Process Tree with `count` processes.
    EndTree { count: u32 },
    /// Kill App on `count` processes of `app`.
    KillApp { app: SharedString, count: u32 },
    /// End Task (`kill: false`, SIGTERM) or End Process (`kill: true`, SIGKILL) on several
    /// targets.
    EndMany { count: u32, kill: bool },
    /// Any action on another user's process. `action` is the action label ("End task").
    /// `destructive` is set for End and Kill.
    OtherUser {
        action: SharedString,
        name: SharedString,
        pid: u32,
        user: SharedString,
        destructive: bool,
    },
    /// Set Priority > Realtime.
    Realtime,
    /// A signal to several targets. `signal_name` is the name without "SIG" ("TERM").
    /// `ends_process` is set when the signal's default action ends the process.
    SignalMany {
        count: u32,
        signal_name: SharedString,
        ends_process: bool,
    },
    /// Create dump file on a process above 2 GiB. `size` is the formatted VmRSS ("3.4 GiB").
    LargeDump {
        name: SharedString,
        size: SharedString,
    },
    /// The first Open in VirusTotal.
    VirusTotalFirstUse,
    /// A window-picker Ctrl+click when the modifier is unknown (PROC-055).
    Picked { name: SharedString, pid: u32 },
}

/// How a confirmation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmResult {
    /// The action button ("End tree", or "Select" for Picked).
    Confirmed,
    /// The second button. Only Picked has one: "Terminate".
    Second,
    /// Cancel, Esc or the window's close button.
    Cancelled,
}

/// The texts of one confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmTexts {
    /// The heading line.
    pub title: SharedString,
    pub description: SharedString,
    pub confirm_label: SharedString,
    /// The action button is Danger, and focus starts on Cancel.
    pub confirm_danger: bool,
    /// A second action button: label and whether it is Danger.
    pub second: Option<(SharedString, bool)>,
}

/// `n` in the table (d) count form.
fn count(n: u32) -> FluentValue<'static> {
    let mut out = String::new();
    units::count(&mut out, u64::from(n));
    FluentValue::from(out)
}

fn text(s: &SharedString) -> FluentValue<'static> {
    FluentValue::from(s.to_string())
}

/// The title, description and button labels of `kind` (UI-PRC-014 table, UI-PWN-025).
pub fn texts(kind: &ConfirmKind, cx: &App) -> ConfirmTexts {
    let danger = |title, description, label: &str| ConfirmTexts {
        title,
        description,
        confirm_label: tr(cx, label),
        confirm_danger: true,
        second: None,
    };
    let primary = |title, description, label: &str| ConfirmTexts {
        confirm_danger: false,
        ..danger(title, description, label)
    };
    match kind {
        ConfirmKind::Protected {
            action,
            name,
            tree_count,
        } => {
            let prefix = action.prefix();
            let title = tr_args(cx, &format!("{prefix}-title"), &[("name", text(name))]);
            let description = match tree_count {
                Some(n) if *action == ProtectedAction::End => tr_args(
                    cx,
                    "processes-confirm-protected-tree-text",
                    &[("name", text(name)), ("count", count(*n))],
                ),
                _ => tr_args(cx, &format!("{prefix}-text"), &[("name", text(name))]),
            };
            danger(title, description, &format!("{prefix}-button"))
        }
        ConfirmKind::EndTree { count: n } => danger(
            tr(cx, "processes-confirm-end-tree-title"),
            tr_args(
                cx,
                "processes-confirm-end-tree-text",
                &[("count", count(*n))],
            ),
            "processes-confirm-end-tree-button",
        ),
        ConfirmKind::KillApp { app, count: n } => danger(
            tr_args(
                cx,
                "processes-confirm-kill-app-title",
                &[("app", text(app))],
            ),
            tr_args(
                cx,
                "processes-confirm-kill-app-text",
                &[("count", count(*n)), ("app", text(app))],
            ),
            "processes-confirm-kill-app-button",
        ),
        ConfirmKind::EndMany { count: n, kill } => {
            let (description, label) = if *kill {
                (
                    "processes-confirm-end-many-kill-text",
                    "processes-confirm-end-many-kill-button",
                )
            } else {
                (
                    "processes-confirm-end-many-text",
                    "processes-confirm-end-many-button",
                )
            };
            danger(
                tr_args(
                    cx,
                    "processes-confirm-end-many-title",
                    &[("count", count(*n))],
                ),
                tr_args(cx, description, &[("count", count(*n))]),
                label,
            )
        }
        ConfirmKind::OtherUser {
            action,
            name,
            pid,
            user,
            destructive,
        } => ConfirmTexts {
            title: tr(cx, "processes-confirm-other-user-title"),
            description: tr_args(
                cx,
                "processes-confirm-other-user-text",
                &[
                    ("action", text(action)),
                    ("name", text(name)),
                    ("pid", FluentValue::from(pid.to_string())),
                    ("user", text(user)),
                ],
            ),
            confirm_label: action.clone(),
            confirm_danger: *destructive,
            second: None,
        },
        ConfirmKind::Realtime => danger(
            tr(cx, "processes-confirm-realtime-title"),
            tr(cx, "processes-confirm-realtime-text"),
            "processes-confirm-realtime-button",
        ),
        ConfirmKind::SignalMany {
            count: n,
            signal_name,
            ends_process,
        } => ConfirmTexts {
            confirm_danger: *ends_process,
            ..danger(
                tr_args(
                    cx,
                    "processes-confirm-signal-many-title",
                    &[("count", count(*n))],
                ),
                tr_args(
                    cx,
                    "processes-confirm-signal-many-text",
                    &[("signal", text(signal_name)), ("count", count(*n))],
                ),
                "processes-confirm-signal-many-button",
            )
        },
        ConfirmKind::LargeDump { name, size } => primary(
            tr(cx, "processes-confirm-large-dump-title"),
            tr_args(
                cx,
                "processes-confirm-large-dump-text",
                &[("name", text(name)), ("size", text(size))],
            ),
            "processes-confirm-large-dump-button",
        ),
        ConfirmKind::VirusTotalFirstUse => primary(
            tr(cx, "processes-confirm-virustotal-title"),
            tr(cx, "processes-confirm-virustotal-text"),
            "processes-confirm-virustotal-button",
        ),
        ConfirmKind::Picked { name, pid } => ConfirmTexts {
            second: Some((tr(cx, "processes-confirm-picked-terminate"), true)),
            ..primary(
                tr_args(
                    cx,
                    "processes-confirm-picked-title",
                    &[
                        ("name", text(name)),
                        ("pid", FluentValue::from(pid.to_string())),
                    ],
                ),
                tr(cx, "processes-confirm-picked-text"),
                "processes-confirm-picked-button",
            )
        },
    }
}

/// The `TlDialog` for `texts`.
pub fn dialog(texts: ConfirmTexts) -> TlDialog {
    let mut dialog = TlDialog::confirm(texts.description, texts.confirm_label).heading(texts.title);
    if texts.confirm_danger {
        dialog = dialog.destructive();
    }
    if let Some((label, danger)) = texts.second {
        dialog = dialog.second_action(label, danger);
    }
    dialog
}

type Done = Box<dyn FnOnce(ConfirmResult, &mut App)>;

/// Opens the confirmation for `kind` modal to `parent`. `done` runs once, in the parent window.
/// If the parent is gone or the window cannot open, `done` gets `Cancelled` at once.
pub fn confirm(
    kind: ConfirmKind,
    parent: AnyWindowHandle,
    cx: &mut App,
    done: impl FnOnce(ConfirmResult, &mut App) + 'static,
) {
    let dialog = dialog(texts(&kind, cx));
    let done: Rc<Cell<Option<Done>>> = Rc::new(Cell::new(Some(Box::new(done))));
    let on_result = {
        let done = done.clone();
        move |result: DialogResult, _: &mut gpui_kit::Window, cx: &mut App| {
            let result = match result {
                DialogResult::Confirmed { .. } => ConfirmResult::Confirmed,
                DialogResult::Second => ConfirmResult::Second,
                DialogResult::Cancelled => ConfirmResult::Cancelled,
            };
            if let Some(done) = done.take() {
                done(result, cx);
            }
        }
    };
    let opened = parent.update(cx, |_, window, cx| dialog.open(window, cx, on_result));
    if !matches!(opened, Ok(Ok(()))) {
        tracing::debug!(target: "tasklens::ui", "process confirmation did not open");
        if let Some(done) = done.take() {
            done(ConfirmResult::Cancelled, cx);
        }
    }
}
