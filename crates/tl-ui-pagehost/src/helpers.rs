//! Small window and engine helpers that page tests in more than one crate used to copy.

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{TestAppContext, VisualTestContext, WindowHandle};
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_ui_kit::components::toast::ToastQueue;
use tl_ui_testkit::fake_engine::FakeEngine;

/// Publishes `health.helper` and `health.helper_start`, then runs until parked.
pub fn publish_helper(
    cx: &mut TestAppContext,
    engine: &FakeEngine,
    helper: HelperState,
    start: Option<(u64, HelperStartOutcome)>,
) {
    engine.publish_with(|s| {
        let health = Arc::make_mut(&mut s.health);
        health.helper = helper;
        health.helper_start = start;
        health.version += 1;
    });
    cx.run_until_parked();
}

/// Whether the window of `handle` is still open.
pub fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
    let id = handle.window_id();
    cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
}

/// How many windows are open.
pub fn window_count(cx: &mut TestAppContext) -> usize {
    cx.update(|cx| cx.windows().len())
}

/// The visible toast texts in the window of `vcx`.
pub fn toasts(vcx: &mut VisualTestContext) -> Vec<String> {
    vcx.update(|window, cx| {
        ToastQueue::of_window(window, cx)
            .expect("toast queue")
            .read(cx)
            .visible_texts()
            .iter()
            .map(|t| t.to_string())
            .collect()
    })
}
