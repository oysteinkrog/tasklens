//! `ToastQueue` rules with `TestAppContext` (E16-006, UI-FND-020, UI-SHL-100, R-195, R-197).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::theme::Theme as KitTheme;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
    SharedString, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
};
use tl_model::client::EngineClient;
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_ui::components::toast::{ToastAction, ToastKey, ToastKind, ToastQueue};
use tl_ui::hub::{GlobalHub, SnapshotHub};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::metrics::metrics;

gpui_kit::actions!(ui_toasts_test, [Retry]);

/// Main-window toast top before the UI scale: title bar 34 + tab strip 30 (UI-SHL-100).
const BARS: f32 = 34. + 30.;

struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("content")
    }
}

fn setup(cx: &mut TestAppContext, appearance: &AppearanceSettings) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        // Kit enter animations run on the real clock, not the test clock. Reduced motion draws
        // each toast at its final place at once.
        cx.set_reduce_motion(true);
        tl_ui::theme::store::init(cx, &ThemeSettings::default(), appearance, Vec::new());
    });
}

/// A window with a kit Root and its queue at the main-window top for the current scale.
fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<ToastQueue>) {
    let window = cx.add_window(|window, cx| {
        let host = cx.new(|_| Host);
        Root::new(host, window, cx)
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.update(|window, _| window.activate_window());
    let queue = vcx.update(|window, cx| {
        let m = *metrics(cx);
        let top = px(BARS * m.ui_scale) + m.sp[4];
        cx.new(|cx| ToastQueue::new(window, cx, top))
    });
    vcx.run_until_parked();
    (vcx, queue)
}

fn push(
    cx: &mut VisualTestContext,
    queue: &Entity<ToastQueue>,
    kind: ToastKind,
    text: &str,
    key: Option<u64>,
) {
    let text: SharedString = text.to_owned().into();
    queue.update(cx, |q, cx| q.push(kind, text, key.map(ToastKey), None, cx));
    cx.run_until_parked();
}

fn visible(cx: &mut VisualTestContext, queue: &Entity<ToastQueue>) -> Vec<String> {
    queue.read_with(cx, |q, _| {
        q.visible_texts().iter().map(|t| t.to_string()).collect()
    })
}

fn waiting(cx: &mut VisualTestContext, queue: &Entity<ToastQueue>) -> Vec<String> {
    queue.read_with(cx, |q, _| {
        q.waiting_texts().iter().map(|t| t.to_string()).collect()
    })
}

fn advance(cx: &mut VisualTestContext, d: Duration) {
    cx.executor().advance_clock(d);
    cx.run_until_parked();
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui_kit::test]
fn fifth_waits_then_shows(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let (mut cx, queue) = open(cx);
    for i in 1..=4 {
        push(
            &mut cx,
            &queue,
            ToastKind::Info,
            &format!("info {i}"),
            Some(i),
        );
    }
    push(&mut cx, &queue, ToastKind::Info, "fifth", None);
    assert_eq!(
        visible(&mut cx, &queue),
        ["info 1", "info 2", "info 3", "info 4"]
    );
    assert_eq!(waiting(&mut cx, &queue), ["fifth"]);
    // The kit holds the four shown toasts.
    assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 4);

    queue.update(&mut cx, |q, cx| q.remove(ToastKey(2), cx));
    cx.run_until_parked();
    assert_eq!(
        visible(&mut cx, &queue),
        ["info 1", "info 3", "info 4", "fifth"]
    );
    assert!(waiting(&mut cx, &queue).is_empty());
}

#[gpui_kit::test]
fn error_evicts_oldest_info(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let (mut cx, queue) = open(cx);
    for i in 1..=4 {
        push(&mut cx, &queue, ToastKind::Info, &format!("info {i}"), None);
    }
    push(&mut cx, &queue, ToastKind::Error, "failed", None);
    assert_eq!(
        visible(&mut cx, &queue),
        ["info 2", "info 3", "info 4", "failed"]
    );
    assert!(waiting(&mut cx, &queue).is_empty());

    // A Warning waits instead.
    push(&mut cx, &queue, ToastKind::Warning, "careful", None);
    assert_eq!(waiting(&mut cx, &queue), ["careful"]);
}

#[gpui_kit::test]
fn same_key_replaces_in_place(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let (mut cx, queue) = open(cx);
    push(
        &mut cx,
        &queue,
        ToastKind::Progress,
        "Ending process…",
        Some(7),
    );
    push(&mut cx, &queue, ToastKind::Info, "other", None);
    advance(&mut cx, Duration::from_secs(3));

    push(
        &mut cx,
        &queue,
        ToastKind::Warning,
        "Ended with a warning",
        Some(7),
    );
    assert_eq!(
        visible(&mut cx, &queue),
        ["Ended with a warning", "other"],
        "the content changes and the position is kept"
    );

    // "other" ends at 5 s. The replaced toast's 5 s started at 3 s.
    advance(&mut cx, Duration::from_secs(3));
    assert_eq!(visible(&mut cx, &queue), ["Ended with a warning"]);
    advance(&mut cx, Duration::from_millis(1900));
    assert_eq!(visible(&mut cx, &queue), ["Ended with a warning"]);
    advance(&mut cx, Duration::from_millis(100));
    assert!(visible(&mut cx, &queue).is_empty());
}

#[gpui_kit::test]
fn coalesce_count_suffix(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let (mut cx, queue) = open(cx);
    for _ in 0..5 {
        push(
            &mut cx,
            &queue,
            ToastKind::Success,
            "Copied to clipboard",
            None,
        );
        advance(&mut cx, Duration::from_millis(900));
    }
    assert_eq!(visible(&mut cx, &queue), ["Copied to clipboard (×5)"]);
    draw(&mut cx);
    assert!(
        cx.debug_bounds("TlToast-a1-text").is_some(),
        "the toast text is drawn"
    );

    // Each repeat restarted the timer: 5 s from the last push.
    advance(&mut cx, Duration::from_millis(4000));
    assert_eq!(visible(&mut cx, &queue), ["Copied to clipboard (×5)"]);
    advance(&mut cx, Duration::from_millis(200));
    assert!(visible(&mut cx, &queue).is_empty());

    // After more than 1 s the same text is a new toast.
    push(
        &mut cx,
        &queue,
        ToastKind::Success,
        "Copied to clipboard",
        None,
    );
    advance(&mut cx, Duration::from_millis(1100));
    push(
        &mut cx,
        &queue,
        ToastKind::Success,
        "Copied to clipboard",
        None,
    );
    assert_eq!(
        visible(&mut cx, &queue),
        ["Copied to clipboard", "Copied to clipboard"]
    );
}

#[gpui_kit::test]
fn error_lives_8s_info_5s(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let (mut cx, queue) = open(cx);
    push(&mut cx, &queue, ToastKind::Info, "info", None);
    push(&mut cx, &queue, ToastKind::Error, "error", None);
    advance(&mut cx, Duration::from_millis(4999));
    assert_eq!(visible(&mut cx, &queue), ["info", "error"]);
    advance(&mut cx, Duration::from_millis(1));
    assert_eq!(visible(&mut cx, &queue), ["error"]);
    advance(&mut cx, Duration::from_millis(2999));
    assert_eq!(visible(&mut cx, &queue), ["error"]);
    advance(&mut cx, Duration::from_millis(1));
    assert!(visible(&mut cx, &queue).is_empty());
    // The kit finishes the exit and unmounts both.
    advance(&mut cx, Duration::from_millis(500));
    assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 0);
}

#[gpui_kit::test]
fn action_toast_lives_15s(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let (mut cx, queue) = open(cx);
    queue.update(&mut cx, |q, cx| {
        q.push(
            ToastKind::Error,
            "Permission denied",
            None,
            Some(ToastAction {
                label: "Retry as administrator".into(),
                action: Box::new(Retry),
            }),
            cx,
        )
    });
    cx.run_until_parked();
    draw(&mut cx);
    assert!(
        cx.debug_bounds("TlButton-TlToast-a1-action").is_some(),
        "the action button is drawn"
    );
    assert!(
        cx.debug_bounds("TlButton-TlToast-a1-close").is_some(),
        "the close button is drawn"
    );
    advance(&mut cx, Duration::from_secs(14));
    assert_eq!(visible(&mut cx, &queue), ["Permission denied"]);
    advance(&mut cx, Duration::from_secs(1));
    assert!(visible(&mut cx, &queue).is_empty());
}

#[gpui_kit::test]
fn progress_never_auto_dismisses(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let (mut cx, queue) = open(cx);
    push(
        &mut cx,
        &queue,
        ToastKind::Progress,
        "Writing core…",
        Some(3),
    );
    advance(&mut cx, Duration::from_secs(600));
    assert_eq!(visible(&mut cx, &queue), ["Writing core…"]);
    queue.update(&mut cx, |q, cx| q.remove(ToastKey(3), cx));
    cx.run_until_parked();
    assert!(visible(&mut cx, &queue).is_empty());
}

#[gpui_kit::test]
fn progress_does_not_take_a_slot(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let (mut cx, queue) = open(cx);
    for i in 1..=4 {
        push(&mut cx, &queue, ToastKind::Info, &format!("info {i}"), None);
    }
    push(
        &mut cx,
        &queue,
        ToastKind::Progress,
        "Starting 6 services...",
        Some(9),
    );
    assert_eq!(
        visible(&mut cx, &queue),
        [
            "info 1",
            "info 2",
            "info 3",
            "info 4",
            "Starting 6 services..."
        ]
    );
    assert!(waiting(&mut cx, &queue).is_empty());
    assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 5);
}

#[gpui_kit::test]
fn hover_pauses_all(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let (mut cx, queue) = open(cx);
    push(&mut cx, &queue, ToastKind::Info, "one", None);
    push(&mut cx, &queue, ToastKind::Error, "two", None);
    advance(&mut cx, Duration::from_millis(500));
    draw(&mut cx);
    let toast = cx.debug_bounds("TlToast-a2").expect("toast drawn");

    cx.simulate_mouse_move(toast.center(), None, Modifiers::none());
    cx.run_until_parked();
    assert!(queue.read_with(&cx, |q, _| q.is_paused()));
    advance(&mut cx, Duration::from_secs(10));
    assert_eq!(visible(&mut cx, &queue), ["one", "two"]);

    // Leaving the stack resumes each timer with the time it had left: 4.5 s and 7.5 s.
    cx.simulate_mouse_move(point(px(5.), px(500.)), None, Modifiers::none());
    cx.run_until_parked();
    assert!(!queue.read_with(&cx, |q, _| q.is_paused()));
    advance(&mut cx, Duration::from_millis(4500));
    assert_eq!(visible(&mut cx, &queue), ["two"]);
    advance(&mut cx, Duration::from_millis(3000));
    assert!(visible(&mut cx, &queue).is_empty());
}

#[gpui_kit::test]
fn wait_list_drops_oldest_info_only(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let (mut cx, queue) = open(cx);
    for i in 1..=4 {
        push(
            &mut cx,
            &queue,
            ToastKind::Warning,
            &format!("shown {i}"),
            None,
        );
    }
    // 16 waiting: a Warning, an Info, a Success, then 13 Warnings.
    push(&mut cx, &queue, ToastKind::Warning, "w0", None);
    push(&mut cx, &queue, ToastKind::Info, "i1", None);
    push(&mut cx, &queue, ToastKind::Success, "s2", None);
    for i in 3..16 {
        push(&mut cx, &queue, ToastKind::Warning, &format!("w{i}"), None);
    }
    assert_eq!(waiting(&mut cx, &queue).len(), 16);

    push(&mut cx, &queue, ToastKind::Info, "i16", None);
    let list = waiting(&mut cx, &queue);
    assert_eq!(list.len(), 16);
    assert_eq!(&list[..3], ["w0", "s2", "w3"], "the oldest Info went");
    assert_eq!(list.last().map(String::as_str), Some("i16"));

    push(&mut cx, &queue, ToastKind::Warning, "w17", None);
    let list = waiting(&mut cx, &queue);
    assert_eq!(&list[..2], ["w0", "w3"], "then the oldest Success");
    assert_eq!(list.len(), 16);
    assert!(list.iter().filter(|t| t.starts_with('w')).count() == 15);
}

#[gpui_kit::test]
fn hidden_window_drops_or_routes_to_system(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default());
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let hub = cx.new(|cx| SnapshotHub::new(client, cx));
    cx.update(|cx| {
        cx.set_global(GlobalHub(hub.clone()));
        // The test platform posts system notifications only for an app with an identity.
        cx.set_app_identity("io.tasklens.test", "tasklens");
    });
    let app = cx.clone();
    let (mut cx, queue) = open(cx);

    // A change waits more than 1 s for a render: the hub reports the window hidden.
    engine.publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
    advance(&mut cx, Duration::from_millis(1500));
    assert!(hub.read_with(&cx, |h, _| h.is_hidden()));

    push(
        &mut cx,
        &queue,
        ToastKind::Error,
        "Service failed to start",
        None,
    );
    push(
        &mut cx,
        &queue,
        ToastKind::Info,
        "Copied to clipboard",
        None,
    );
    assert!(visible(&mut cx, &queue).is_empty());
    assert!(waiting(&mut cx, &queue).is_empty());
    assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 0);
    let shown = app.shown_system_notifications();
    assert_eq!(shown.len(), 1, "only the Error reaches the desktop");
    assert_eq!(shown[0].title, "Service failed to start");

    // Nothing is replayed on restore.
    hub.update(&mut cx, |h, cx| h.note_render(cx));
    cx.run_until_parked();
    assert!(visible(&mut cx, &queue).is_empty());
}

#[gpui_kit::test]
fn toast_top_follows_scale(cx: &mut TestAppContext) {
    let appearance = AppearanceSettings {
        ui_scale_pct: 150,
        ..AppearanceSettings::default()
    };
    setup(cx, &appearance);
    let (mut cx, queue) = open(cx);
    let top = cx.update(|_, cx| {
        let m = *metrics(cx);
        px(BARS * m.ui_scale) + m.sp[4]
    });
    assert_eq!(top, px(108.));
    queue.update(&mut cx, |q, cx| q.set_top(top, cx));
    assert_eq!(queue.read_with(&cx, |q, _| q.top()), px(108.));
    assert_eq!(
        cx.update(|_, cx| KitTheme::global(cx).notification.margins.top),
        px(108.)
    );

    push(
        &mut cx,
        &queue,
        ToastKind::Info,
        "UI scale changed to 150%",
        None,
    );
    // The kit stack measures each toast in the first frame and places it in the second.
    draw(&mut cx);
    draw(&mut cx);
    let toast = cx.debug_bounds("TlToast-a1").expect("toast drawn");
    // The content sits inside the toast's 1 px border and 12 px padding.
    assert!(
        toast.top() >= px(108.) && toast.top() <= px(108. + 14.),
        "toast content top {:?}",
        toast.top()
    );
}
