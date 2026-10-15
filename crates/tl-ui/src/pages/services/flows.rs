//! Services Start, Stop, Restart, Reload and Reset failed flows (UI-SYS-018, UI-SYS-007, SYS-005,
//! SYS-006, R-114, R-117).
//!
//! None of the five asks for confirmation. Each sends one request for the captured unit targets
//! and the toast is replaced in place as the engine reports. Service jobs have no client-side
//! timeout, so the Progress toast stays until the final event (R-197). The Status cell overlay
//! while pending comes from `SysActions::pending_tag`, not from this module.
//!
//! It also holds the startup type flow (UI-SYS-019), the three file openers (UI-SYS-017) and
//! Jump to process (UI-SYS-021). Paths and URIs go to the engine only and are never logged
//! (ADR-015).

use gpui_kit::{Action, App, Entity, Global, Window, gpui};
use smallvec::SmallVec;
use tl_model::actions::{ActionKind, ActionTarget};
use tl_model::command::Tab;
use tl_model::process::ProcFlags;
use tl_model::snapshot::{Snapshot, StartupType, UnitRecord};
use tl_model::table::{UnitRef, UnitType};
use tracing::warn;

use super::actions::StartupChoice;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::tr;
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::jump::JumpTexts;
use crate::pages::sys_table::{ActionKindTag, ActionTexts, SysActions};
use crate::window::main::main_window_view;
use crate::windows::service_properties::{ServicePropertiesWindow, ServicePropsTab};

/// The five service verbs that share one flow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnitVerb {
    Start,
    Stop,
    Restart,
    Reload,
    ResetFailed,
}

pub fn unit_action_kind(v: UnitVerb) -> ActionKind {
    match v {
        UnitVerb::Start => ActionKind::UnitStart,
        UnitVerb::Stop => ActionKind::UnitStop,
        UnitVerb::Restart => ActionKind::UnitRestart,
        UnitVerb::Reload => ActionKind::UnitReload,
        UnitVerb::ResetFailed => ActionKind::UnitResetFailed,
    }
}

pub fn unit_action_tag(v: UnitVerb) -> ActionKindTag {
    match v {
        UnitVerb::Start => ActionKindTag::Start,
        UnitVerb::Stop => ActionKindTag::Stop,
        UnitVerb::Restart => ActionKindTag::Restart,
        UnitVerb::Reload => ActionKindTag::Reload,
        UnitVerb::ResetFailed => ActionKindTag::ResetFailed,
    }
}

/// "View logs" on a failed toast: opens Service Properties on the Logs tab for the unit.
/// `ServicePropertiesWindow::open` is a function, so the toast button needs this action.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct ServiceOpenLogs(pub UnitRef);

/// Set once the `ServiceOpenLogs` handler is registered.
struct ViewLogsHandler;

impl Global for ViewLogsHandler {}

fn ensure_view_logs_handler(cx: &mut App) {
    if cx.has_global::<ViewLogsHandler>() {
        return;
    }
    cx.set_global(ViewLogsHandler);
    cx.on_action(|a: &ServiceOpenLogs, cx| {
        ServicePropertiesWindow::open(a.0.clone(), ServicePropsTab::Logs, cx);
    });
}

/// Keys: pending one, done one, failed prefix, then the three counted keys.
fn keys(
    v: UnitVerb,
) -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
) {
    match v {
        UnitVerb::Start => (
            "services-sys018-starting-service",
            "services-sys018-service-started",
            "services-sys018-failed-to-start-service",
            "services-sys018-starting-n-services",
            "services-sys018-n-services-started",
            "services-sys018-ok-of-n-services-started",
        ),
        UnitVerb::Stop => (
            "services-sys018-stopping-service",
            "services-sys018-service-stopped",
            "services-sys018-failed-to-stop-service",
            "services-sys018-stopping-n-services",
            "services-sys018-n-services-stopped",
            "services-sys018-ok-of-n-services-stopped",
        ),
        UnitVerb::Restart => (
            "services-sys018-restarting-service",
            "services-sys018-service-restarted",
            "services-sys018-failed-to-restart-service",
            "services-sys018-restarting-n-services",
            "services-sys018-n-services-restarted",
            "services-sys018-ok-of-n-services-restarted",
        ),
        UnitVerb::Reload => (
            "services-sys018-reloading-service",
            "services-sys018-service-reloaded",
            "services-sys018-failed-to-reload-service",
            "services-sys018-reloading-n-services",
            "services-sys018-n-services-reloaded",
            "services-sys018-ok-of-n-services-reloaded",
        ),
        UnitVerb::ResetFailed => (
            "services-sys018-clearing-failed-state",
            "services-sys018-failed-state-cleared",
            "services-sys018-failed-to-clear-failed-state",
            "services-sys018-resetting-n-services",
            "services-sys018-n-services-reset",
            "services-sys018-ok-of-n-services-reset",
        ),
    }
}

pub fn unit_action_texts(v: UnitVerb, cx: &App) -> ActionTexts {
    let (pending_one, done_one, failed, pending_many, done_many, partial_many) = keys(v);
    let failed_button = (v != UnitVerb::ResetFailed).then(|| {
        let make: crate::pages::sys_table::FailedButtonAction = Box::new(|t| match t {
            ActionTarget::Unit(u) => Box::new(ServiceOpenLogs(u.clone())),
            _ => Box::new(gpui::NoAction),
        });
        (tr(cx, "services-sys018-view-logs"), make)
    });
    ActionTexts {
        pending_one: tr(cx, pending_one),
        pending_many,
        done_one: tr(cx, done_one),
        done_many,
        partial_many,
        failed_prefix: tr(cx, failed),
        failed_suffix: true,
        unknown: tr(cx, "services-sys018-service-action-finished"),
        busy: tr(cx, "services-sys018-a-service-action-is-already"),
        failed_button,
    }
}

/// Submits one request for the captured unit targets (group rows and child rows already removed).
pub fn run_unit_verb(
    v: UnitVerb,
    units: &[UnitRef],
    actions: &Entity<SysActions>,
    window: &mut Window,
    cx: &mut App,
) {
    if units.is_empty() {
        return;
    }
    ensure_view_logs_handler(cx);
    let targets: SmallVec<[ActionTarget; 1]> =
        units.iter().cloned().map(ActionTarget::Unit).collect();
    let texts = unit_action_texts(v, cx);
    let (kind, tag) = (unit_action_kind(v), unit_action_tag(v));
    actions.update(cx, |a, cx| {
        a.submit(kind, tag, targets, texts, window, cx);
    });
}

/// Request kind for a startup change, or None when the choice equals the current value.
/// "Enabled (runtime)" counts as Enabled. Leaving Masked sends `UnitEnable` or `UnitDisable`;
/// the engine unmasks first as part of that request.
pub fn startup_change_kind(current: StartupType, choice: StartupChoice) -> Option<ActionKind> {
    let same = matches!(
        (current, choice),
        (
            StartupType::Enabled | StartupType::EnabledRuntime,
            StartupChoice::Enabled
        ) | (StartupType::Disabled, StartupChoice::Disabled)
            | (StartupType::Masked, StartupChoice::Masked)
    );
    if same {
        return None;
    }
    Some(match choice {
        StartupChoice::Enabled => ActionKind::UnitEnable,
        StartupChoice::Disabled => ActionKind::UnitDisable,
        StartupChoice::Masked => ActionKind::UnitMask,
    })
}

fn startup_texts(cx: &App) -> ActionTexts {
    ActionTexts {
        pending_one: tr(cx, "services-sys019-changing-startup-type"),
        pending_many: "services-sys019-changing-n-services",
        done_one: tr(cx, "services-sys019-startup-type-changed"),
        done_many: "services-sys019-n-services-changed",
        partial_many: "services-sys019-ok-of-n-services-changed",
        failed_prefix: tr(cx, "services-sys019-failed-to-change-startup-type"),
        failed_suffix: true,
        unknown: tr(cx, "services-sys018-service-action-finished"),
        busy: tr(cx, "services-sys018-a-service-action-is-already"),
        failed_button: None,
    }
}

/// Sends one request for the units whose startup type differs from `choice`. The kind depends
/// only on the choice, so there is at most one request.
pub fn run_set_startup(
    choice: StartupChoice,
    units: &[(UnitRef, StartupType)],
    actions: &Entity<SysActions>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut kind = None;
    let mut targets: SmallVec<[ActionTarget; 1]> = SmallVec::new();
    for (unit, current) in units {
        if let Some(k) = startup_change_kind(*current, choice) {
            kind = Some(k);
            targets.push(ActionTarget::Unit(unit.clone()));
        }
    }
    let Some(kind) = kind else {
        return;
    };
    let texts = startup_texts(cx);
    actions.update(cx, |a, cx| {
        a.submit(kind, ActionKindTag::SetStartup, targets, texts, window, cx);
    });
}

/// `file://` plus `path` with every byte outside `A-Z a-z 0-9 - . _ ~ /` written as `%XX`.
pub fn file_uri(path: &str) -> String {
    let mut uri = String::with_capacity(7 + path.len());
    uri.push_str("file://");
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// A toast in `window`'s own stack.
fn toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: gpui_kit::SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack for a Services toast"),
    }
}

/// The ExecStart binary: only services have one.
fn binary_path(unit: &UnitRecord) -> Option<&str> {
    (unit.kind == UnitType::Service && !unit.path.is_empty()).then_some(&*unit.path)
}

fn unit_file_path(unit: &UnitRecord) -> Option<&str> {
    unit.unit_file.as_deref().filter(|p| !p.is_empty())
}

fn opener_texts(done: &str, failed: &str, cx: &App) -> ActionTexts {
    let text = |key: &str| {
        if key.is_empty() {
            Default::default()
        } else {
            tr(cx, key)
        }
    };
    ActionTexts {
        pending_one: Default::default(),
        pending_many: "",
        done_one: text(done),
        done_many: "",
        partial_many: "",
        failed_prefix: tr(cx, failed),
        failed_suffix: false,
        unknown: tr(cx, "services-sys018-service-action-finished"),
        busy: tr(cx, "services-sys018-a-service-action-is-already"),
        failed_button: None,
    }
}

/// Opener requests have no targets.
fn submit_targetless(
    kind: ActionKind,
    texts: ActionTexts,
    actions: &Entity<SysActions>,
    window: &mut Window,
    cx: &mut App,
) {
    actions.update(cx, |a, cx| {
        a.submit(
            kind,
            ActionKindTag::Other,
            SmallVec::new(),
            texts,
            window,
            cx,
        );
    });
}

fn image_unavailable(window: &mut Window, cx: &mut App) {
    let text = tr(cx, "services-sys017-image-unavailable");
    toast(window, cx, ToastKind::Warning, text);
}

/// File properties: the ExecStart binary, else the unit file.
pub fn run_file_properties(
    unit: &UnitRecord,
    actions: &Entity<SysActions>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(path) = binary_path(unit).or_else(|| unit_file_path(unit)) else {
        image_unavailable(window, cx);
        return;
    };
    let kind = ActionKind::ShowInFileManager {
        path: path.into(),
        properties: true,
    };
    let texts = opener_texts(
        "services-sys017-opened-file-properties",
        "services-sys017-file-properties-failed",
        cx,
    );
    submit_targetless(kind, texts, actions, window, cx);
}

/// Open file location: the ExecStart binary in the file manager.
pub fn run_open_file_location(
    unit: &UnitRecord,
    actions: &Entity<SysActions>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(path) = binary_path(unit) else {
        image_unavailable(window, cx);
        return;
    };
    let kind = ActionKind::ShowInFileManager {
        path: path.into(),
        properties: false,
    };
    let texts = opener_texts(
        "services-sys017-opening-file-location",
        "services-sys017-file-location-failed",
        cx,
    );
    submit_targetless(kind, texts, actions, window, cx);
}

/// Open unit file: the unit file as a `file://` URI. No success toast.
pub fn run_open_unit_file(
    unit: &UnitRecord,
    actions: &Entity<SysActions>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(path) = unit_file_path(unit) else {
        return;
    };
    let kind = ActionKind::OpenUri(file_uri(path).into());
    let texts = opener_texts("", "services-sys017-unit-file-failed", cx);
    submit_targetless(kind, texts, actions, window, cx);
}

/// Jump to process: selects the service's main process on the Processes tab (UI-SYS-021).
pub fn run_jump_to_process(unit: &UnitRecord, snap: &Snapshot, window: &mut Window, cx: &mut App) {
    if unit.main_pid == 0 {
        let text = tr(cx, "services-sys017-not-running");
        toast(window, cx, ToastKind::Info, text);
        return;
    }
    let key = snap
        .processes
        .records
        .iter()
        .find(|r| r.key.pid == unit.main_pid && !r.flags.contains(ProcFlags::EXITED))
        .map(|r| r.key);
    let Some(key) = key else {
        let text = tr(cx, "processes-jump-not-found");
        toast(window, cx, ToastKind::Warning, text);
        return;
    };
    let Some(view) = main_window_view(cx) else {
        return;
    };
    view.update(cx, |v, cx| v.select_tab(Tab::Processes, window, cx));
    let Some(page) = view.read(cx).page_entity::<ProcessesPage>(Tab::Processes) else {
        warn!(target: "tasklens::ui", "no Processes page for Jump to process");
        return;
    };
    let texts = JumpTexts {
        switched_tab: None,
        success: Some(tr(cx, "services-sys021-switched-to-process")),
        not_found: Default::default(),
        hidden: Some(tr(cx, "services-sys021-process-not-visible")),
    };
    page.update(cx, |p, cx| p.select_by_key(key, texts, window, cx));
}
