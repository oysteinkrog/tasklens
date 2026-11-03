//! Helpers that several test modules used to copy word for word. The ones that need no shell
//! live in `tl_ui_testkit::fixtures` and `tl_ui_pagehost::helpers`; they are re-exported here
//! for the modules that still use the old paths.

use std::sync::Arc;

use gpui_kit::TestAppContext;
use tl_model::command::{SetupPhase, SetupRun, SetupState, SetupStep};
use tl_ui_base::settings_global::TlSettings;
use tl_ui_testkit::fake_engine::FakeEngine;

pub use tl_ui_pagehost::helpers::{is_open, publish_helper, toasts, window_count};
pub use tl_ui_testkit::fixtures::{
    CANCELLED, FAILED, actions_sent, key, record, searches, sent, starts_sent,
};

/// Publishes `health.setup.run` for the polkit rule step.
pub fn publish_polkit(engine: &FakeEngine, seq: u64, on: bool, phase: SetupPhase) {
    engine.publish_with(|s| {
        let mut health = (*s.health).clone();
        health.version += 1;
        health.setup = Arc::new(SetupState {
            status: None,
            run: Some(SetupRun {
                seq,
                step: SetupStep::PolkitRule { on },
                phase,
                output: Arc::from(""),
            }),
        });
        s.health = Arc::new(health);
    });
}

pub fn passwordless(cx: &mut TestAppContext) -> bool {
    cx.update(|cx| TlSettings::settings(cx).privilege.passwordless)
}
