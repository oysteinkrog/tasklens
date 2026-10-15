//! Services Start, Stop, Restart, Reload and Reset failed flows (UI-SYS-018, UI-SYS-007, SYS-005,
//! SYS-006, R-114, R-117).
//!
//! None of the five asks for confirmation. Each sends one request for the captured unit targets
//! and the toast is replaced in place as the engine reports. Service jobs have no client-side
//! timeout, so the Progress toast stays until the final event (R-197). The Status cell overlay
//! while pending comes from `SysActions::pending_tag`, not from this module.

use gpui_kit::{Action, App, Entity, Global, Window, gpui};
use smallvec::SmallVec;
use tl_model::actions::{ActionKind, ActionTarget};
use tl_model::table::UnitRef;

use crate::i18n::tr;
use crate::pages::sys_table::{ActionKindTag, ActionTexts, SysActions};
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
