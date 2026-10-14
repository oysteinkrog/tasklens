//! Startup Apps Enable, Disable and Run now flows (UI-SYS-045, UI-SYS-046, UI-SYS-007).
//!
//! Each command acts on the entries captured at menu open, or on the selection for a keyboard
//! command. The request carries only their `StartupRef`s; the engine validates them (ADR-006).

use gpui_kit::{Context, SharedString, Window};
use smallvec::SmallVec;
use tl_model::actions::{ActionKind, ActionTarget};
use tl_model::snapshot::StartupStatus;

use super::StartupPage;
use super::actions::{StartupDisable, StartupEnable, StartupRunNow};
use super::menu::StartupMenuTargets;
use crate::i18n::tr;
use crate::pages::sys_table::{ActionKindTag, ActionTexts};

/// The three flows differ only in their targets and texts.
#[derive(Clone, Copy)]
enum Flow {
    Enable,
    Disable,
    RunNow,
}

impl Flow {
    fn kind(self) -> (ActionKind, ActionKindTag) {
        match self {
            Flow::Enable => (ActionKind::StartupEnable, ActionKindTag::StartupEnable),
            Flow::Disable => (ActionKind::StartupDisable, ActionKindTag::StartupDisable),
            Flow::RunNow => (ActionKind::StartupRunNow, ActionKindTag::StartupRunNow),
        }
    }

    /// The captured entries this flow acts on. Entries whose state would not change are skipped.
    fn targets(self, captured: &StartupMenuTargets) -> SmallVec<[ActionTarget; 1]> {
        captured
            .entries
            .iter()
            .filter(|e| match self {
                Flow::Enable => e.status != StartupStatus::Enabled && e.can_toggle,
                Flow::Disable => e.status == StartupStatus::Enabled && e.can_toggle,
                Flow::RunNow => e.can_run,
            })
            .map(|e| ActionTarget::Startup(e.id.clone()))
            .collect()
    }

    /// No pending texts: the page shows "Updating…" in the row instead of a Progress toast.
    fn texts(self, cx: &gpui_kit::App) -> ActionTexts {
        let (done_one, done_many, partial_many, failed_prefix) = match self {
            Flow::Enable => (
                "startup-action-enabled",
                "startup-action-enabled-n",
                "startup-action-enabled-partial",
                "startup-action-update-failed",
            ),
            Flow::Disable => (
                "startup-action-disabled",
                "startup-action-disabled-n",
                "startup-action-disabled-partial",
                "startup-action-update-failed",
            ),
            Flow::RunNow => (
                "startup-action-launched",
                "startup-action-launched-n",
                "startup-action-launched-partial",
                "startup-action-launch-failed",
            ),
        };
        ActionTexts {
            pending_one: SharedString::default(),
            pending_many: "",
            done_one: tr(cx, done_one),
            done_many,
            partial_many,
            failed_prefix: tr(cx, failed_prefix),
            failed_suffix: true,
            unknown: tr(cx, "startup-action-unknown"),
            busy: tr(cx, "startup-action-busy"),
            failed_button: None,
        }
    }
}

impl StartupPage {
    pub(super) fn on_startup_enable(
        &mut self,
        _: &StartupEnable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_flow(Flow::Enable, window, cx);
    }

    pub(super) fn on_startup_disable(
        &mut self,
        _: &StartupDisable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_flow(Flow::Disable, window, cx);
    }

    pub(super) fn on_startup_run_now(
        &mut self,
        _: &StartupRunNow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_flow(Flow::RunNow, window, cx);
    }

    fn run_flow(&mut self, flow: Flow, window: &mut Window, cx: &mut Context<Self>) {
        let captured = self.take_targets(cx);
        let targets = flow.targets(&captured);
        if targets.is_empty() {
            return;
        }
        let (kind, tag) = flow.kind();
        let texts = flow.texts(cx);
        self.frame.actions.update(cx, |actions, cx| {
            actions.submit(kind, tag, targets, texts, window, cx);
        });
    }
}
