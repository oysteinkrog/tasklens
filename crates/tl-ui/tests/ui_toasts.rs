//! `ToastQueue` rules and the stack's look with `TestAppContext` (E16-006, UI-FND-020, UI-SHL-100,
//! R-195, R-197). Every rule test runs twice: with motion and with reduced motion.
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
    SharedString, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
};
use tl_model::client::EngineClient;
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_ui::app::BootSettings;
use tl_ui::components::toast::{self, ToastAction, ToastKey, ToastKind, ToastQueue};
use tl_ui::hub::{GlobalHub, SnapshotHub};
use tl_ui::settings_global::{TlSettings, animations_on};
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

/// Runs each named test fn, which takes `(cx, reduce_motion)`, once with motion and once with
/// reduced motion.
macro_rules! both_motions {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[gpui_kit::test]
            fn motion(cx: &mut gpui_kit::TestAppContext) {
                super::$name(cx, false)
            }

            #[gpui_kit::test]
            fn reduced_motion(cx: &mut gpui_kit::TestAppContext) {
                super::$name(cx, true)
            }
        }
    )*};
}

both_motions!(
    fifth_waits_then_shows,
    error_evicts_oldest_info,
    same_key_replaces_in_place,
    coalesce_count_suffix,
    error_lives_8s_info_5s,
    action_toast_lives_15s,
    progress_never_auto_dismisses,
    progress_does_not_take_a_slot,
    hover_pauses_all,
    wait_list_drops_oldest_info_only,
    hidden_window_drops_or_routes_to_system,
    toast_top_follows_scale,
    stack_grows_down_with_gaps,
    one_close_button_per_toast,
    close_icon_is_i_sm,
);

/// Enter and exit animations run on the real clock, so the tests measure each toast's frame,
/// which the slide does not move.
fn setup(cx: &mut TestAppContext, appearance: &AppearanceSettings, reduce_motion: bool) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        toast::init(cx);
        cx.set_reduce_motion(reduce_motion);
        tl_ui::theme::store::init(cx, &ThemeSettings::default(), appearance, Vec::new());
        tl_ui::i18n::init_strings(cx, "en-US");
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

fn mounted(cx: &mut VisualTestContext, queue: &Entity<ToastQueue>) -> usize {
    queue.read_with(cx, |q, _| q.mounted_len())
}

fn texts(list: Vec<SharedString>) -> Vec<String> {
    list.iter().map(|t| t.to_string()).collect()
}

fn fifth_waits_then_shows(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
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
    assert_eq!(mounted(&mut cx, &queue), 4);

    queue.update(&mut cx, |q, cx| q.remove(ToastKey(2), cx));
    cx.run_until_parked();
    assert_eq!(
        visible(&mut cx, &queue),
        ["info 1", "info 3", "info 4", "fifth"]
    );
    assert!(waiting(&mut cx, &queue).is_empty());
}

fn error_evicts_oldest_info(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
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

fn same_key_replaces_in_place(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
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

fn coalesce_count_suffix(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
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

fn error_lives_8s_info_5s(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
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
    // The exit ends and both unmount.
    advance(&mut cx, Duration::from_millis(500));
    assert_eq!(mounted(&mut cx, &queue), 0);
}

fn action_toast_lives_15s(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
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

fn progress_never_auto_dismisses(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
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

fn progress_does_not_take_a_slot(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
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
    assert_eq!(mounted(&mut cx, &queue), 5);
}

fn hover_pauses_all(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
    let (mut cx, queue) = open(cx);
    push(&mut cx, &queue, ToastKind::Info, "one", None);
    push(&mut cx, &queue, ToastKind::Error, "two", None);
    advance(&mut cx, Duration::from_millis(500));
    draw(&mut cx);
    let toast = cx.debug_bounds("TlToast-a2-frame").expect("toast drawn");

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

fn wait_list_drops_oldest_info_only(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
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

fn hidden_window_drops_or_routes_to_system(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
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
    assert_eq!(mounted(&mut cx, &queue), 0);
    let shown = app.shown_system_notifications();
    assert_eq!(shown.len(), 1, "only the Error reaches the desktop");
    assert_eq!(shown[0].title, "Service failed to start");

    // Nothing is replayed on restore.
    hub.update(&mut cx, |h, cx| h.note_render(cx));
    cx.run_until_parked();
    assert!(visible(&mut cx, &queue).is_empty());
}

fn toast_top_follows_scale(cx: &mut TestAppContext, reduce_motion: bool) {
    let appearance = AppearanceSettings {
        ui_scale_pct: 150,
        ..AppearanceSettings::default()
    };
    setup(cx, &appearance, reduce_motion);
    let (mut cx, queue) = open(cx);
    let top = cx.update(|_, cx| {
        let m = *metrics(cx);
        px(BARS * m.ui_scale) + m.sp[4]
    });
    assert_eq!(top, px(108.));
    queue.update(&mut cx, |q, cx| q.set_top(top, cx));
    assert_eq!(queue.read_with(&cx, |q, _| q.top()), px(108.));

    push(
        &mut cx,
        &queue,
        ToastKind::Info,
        "UI scale changed to 150%",
        None,
    );
    draw(&mut cx);
    let toast = cx.debug_bounds("TlToast-a1-frame").expect("toast drawn");
    assert_eq!(toast.top(), px(108.));
}

fn stack_grows_down_with_gaps(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
    let (mut cx, queue) = open(cx);
    push(&mut cx, &queue, ToastKind::Info, "one", None);
    push(&mut cx, &queue, ToastKind::Warning, "two", None);
    push(&mut cx, &queue, ToastKind::Error, "three", None);
    draw(&mut cx);
    let top = queue.read_with(&cx, |q, _| q.top());
    let frames: Vec<_> = ["TlToast-a1-frame", "TlToast-a2-frame", "TlToast-a3-frame"]
        .into_iter()
        .map(|s| cx.debug_bounds(s).unwrap_or_else(|| panic!("{s} drawn")))
        .collect();
    assert_eq!(frames[0].top(), top, "the oldest is at the top edge");
    for pair in frames.windows(2) {
        // Newest at the bottom, sp3 = 6 px apart, no overlap without hover.
        assert_eq!(pair[1].top() - pair[0].bottom(), px(6.), "{frames:?}");
        assert!(pair[0].size.height >= px(40.));
    }
    // Centred in the window.
    let window_w = cx.update(|window, _| window.viewport_size().width);
    assert_eq!(frames[0].size.width, px(382.));
    assert_eq!(frames[0].left(), (window_w - px(382.)) / 2.);
}

fn one_close_button_per_toast(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
    let (mut cx, queue) = open(cx);
    push(&mut cx, &queue, ToastKind::Info, "one", None);
    push(&mut cx, &queue, ToastKind::Error, "two", None);
    draw(&mut cx);
    let toast = cx.debug_bounds("TlToast-a2-frame").expect("toast drawn");
    cx.simulate_mouse_move(toast.center(), None, Modifiers::none());
    cx.run_until_parked();
    draw(&mut cx);
    // The kit notification list holds nothing, so it draws no toast and no close button of its
    // own. Each toast draws its one X inside its frame.
    assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 0);
    for (frame, close) in [
        ("TlToast-a1-frame", "TlButton-TlToast-a1-close"),
        ("TlToast-a2-frame", "TlButton-TlToast-a2-close"),
    ] {
        let frame = cx.debug_bounds(frame).expect("frame drawn");
        let close = cx.debug_bounds(close).expect("close drawn");
        assert!(frame.contains(&close.center()), "{close:?} in {frame:?}");
    }
}

fn close_icon_is_i_sm(cx: &mut TestAppContext, reduce_motion: bool) {
    setup(cx, &AppearanceSettings::default(), reduce_motion);
    let (mut cx, queue) = open(cx);
    push(&mut cx, &queue, ToastKind::Info, "one", None);
    draw(&mut cx);
    let i_sm = cx.update(|_, cx| metrics(cx).icon.sm);
    assert_eq!(i_sm, px(14.));
    let x = cx
        .debug_bounds("TlButton-TlToast-a1-close-icon")
        .expect("X drawn");
    assert_eq!((x.size.width, x.size.height), (i_sm, i_sm));
}

/// Installs `TlSettings` with `motion.animations = animations`, then clears
/// `App::reduce_motion`, so only the setting decides.
fn install_settings(cx: &mut TestAppContext, animations: bool) {
    let mut settings = Settings::default();
    settings.motion.animations = animations;
    cx.update(|cx| {
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: Some(Box::new(|_, _| {})),
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        cx.set_reduce_motion(false);
        assert_eq!(animations_on(cx), animations);
    });
}

#[gpui_kit::test]
fn animations_off_no_enter_or_exit(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default(), false);
    install_settings(cx, false);
    let (mut cx, queue) = open(cx);
    push(&mut cx, &queue, ToastKind::Info, "one", Some(1));
    draw(&mut cx);
    let entered = queue.read_with(&cx, |q, _| texts(q.enter_animated_texts()));
    assert!(entered.is_empty(), "no enter animation: {entered:?}");

    queue.update(&mut cx, |q, cx| q.remove(ToastKey(1), cx));
    cx.run_until_parked();
    assert!(queue.read_with(&cx, |q, _| q.exiting_texts()).is_empty());
    assert_eq!(mounted(&mut cx, &queue), 0, "unmounted at once, no exit");
}

#[gpui_kit::test]
fn animations_on_enter_and_exit(cx: &mut TestAppContext) {
    setup(cx, &AppearanceSettings::default(), false);
    install_settings(cx, true);
    let (mut cx, queue) = open(cx);
    push(&mut cx, &queue, ToastKind::Info, "one", Some(1));
    draw(&mut cx);
    let entered = queue.read_with(&cx, |q, _| texts(q.enter_animated_texts()));
    assert_eq!(entered, ["one"]);

    queue.update(&mut cx, |q, cx| q.remove(ToastKey(1), cx));
    cx.run_until_parked();
    assert!(visible(&mut cx, &queue).is_empty());
    let exiting = queue.read_with(&cx, |q, _| texts(q.exiting_texts()));
    assert_eq!(exiting, ["one"], "the toast fades out in place");
    draw(&mut cx);
    advance(&mut cx, Duration::from_millis(199));
    assert_eq!(mounted(&mut cx, &queue), 1);
    advance(&mut cx, Duration::from_millis(1));
    assert_eq!(mounted(&mut cx, &queue), 0, "unmounted after m_base");
}
