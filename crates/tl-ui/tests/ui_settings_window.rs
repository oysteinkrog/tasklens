//! Settings window frame, page list and `PageBuilder` (E21-018, UI-SHL-060, APP-023). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, WindowHandle, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::settings::{Settings, UiState};
use tl_ui::actions::OpenSettings;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::slider::TlSliderEvent;
use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind};
use tl_ui::settings_global::{SettingKey, TlSettings};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::settings::{
    PageBuilder, PageId, SIDEBAR_W, SettingsWindow, TestPages, all_pages,
};

struct Parent;

impl Render for Parent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("parent")
    }
}

type Frame = TlSecondaryWindow<SettingsWindow>;

/// A Motion page with the Animations switch and a smooth scroll amount slider.
fn test_pages(_: &mut gpui_kit::App) -> Vec<PageBuilder> {
    let mut page = PageBuilder::new(PageId::Motion);
    page.switch("Motion", SettingKey::MotionAnimations, "Animations", None)
        .slider(
            "Motion",
            SettingKey::MotionSmoothScrollAmount,
            "Smooth scroll amount",
            1..=10,
            1,
            |v| format!("{v}").into(),
        );
    vec![page]
}

/// `TasklensUi::init` with a persist counter and the test page. Returns the engine and the
/// number of persist calls.
fn setup(cx: &mut TestAppContext) -> (Arc<FakeEngine>, Rc<Cell<u32>>) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let persists = Rc::new(Cell::new(0));
    let count = persists.clone();
    let init = UiInit {
        settings: Settings::default(),
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(move |_, _| count.set(count.get() + 1)),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        cx.set_global(TestPages(test_pages));
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    (engine, persists)
}

fn settings_window(cx: &mut TestAppContext) -> Option<WindowHandle<Root>> {
    cx.update(|cx| {
        cx.global::<GlobalRegistry>()
            .0
            .read(cx)
            .get(&WindowKey::Settings)
    })
    .and_then(|h| h.downcast::<Root>())
}

/// Dispatches `OpenSettings` from a parent window `times` times and returns the window.
fn open(cx: &mut TestAppContext, times: usize) -> (Entity<Frame>, VisualTestContext) {
    let parent: AnyWindowHandle = cx.add_window(|_, _| Parent).into();
    let mut pcx = VisualTestContext::from_window(parent, cx);
    for _ in 0..times {
        pcx.update(|window, _| window.activate_window());
        pcx.dispatch_action(OpenSettings);
        pcx.run_until_parked();
    }
    let handle = settings_window(cx).expect("settings window registered");
    let frame = cx
        .update(|cx| Frame::find(handle, cx))
        .expect("settings frame");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    (frame, vcx)
}

fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
    let id = handle.window_id();
    cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
}

#[gpui_kit::test]
fn settings_window_opens_and_refocuses(cx: &mut TestAppContext) {
    setup(cx);
    let before = cx.update(|cx| cx.windows().len());
    let (frame, vcx) = open(cx, 2);
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        before + 2,
        "the parent and one Settings window"
    );
    let id = settings_window(cx).expect("open").window_id();
    assert_eq!(
        cx.update(|cx| cx.active_window().map(|w| w.window_id())),
        Some(id),
        "the Settings window has focus"
    );
    let (title, kind) = vcx.read(|cx| (frame.read(cx).title().to_string(), frame.read(cx).kind()));
    assert_eq!(title, "tasklens Settings");
    assert_eq!(kind, TlWindowKind::Tool { resizable: true });
}

#[gpui_kit::test]
fn settings_window_size_and_sidebar(cx: &mut TestAppContext) {
    setup(cx);
    let (_, mut vcx) = open(cx, 1);
    let viewport = vcx.update(|window, _| window.viewport_size());
    assert_eq!(viewport, size(px(900.), px(640.)));
    assert_eq!(SIDEBAR_W, 220.);
    assert!(vcx.debug_bounds("SettingsWindowBody").is_some());
}

#[gpui_kit::test]
fn settings_esc_closes(cx: &mut TestAppContext) {
    setup(cx);
    let (frame, mut vcx) = open(cx, 1);
    drop(frame);
    let handle = settings_window(cx).expect("open");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!is_open(cx, handle), "Esc closes the window");
}

#[gpui_kit::test]
fn page_builder_records_keys(cx: &mut TestAppContext) {
    setup(cx);
    let (pages, before) =
        cx.update(|cx| (all_pages(cx), TlSettings::settings(cx).motion.animations));
    assert_eq!(pages.len(), 1);
    assert_eq!(
        pages[0].keys(),
        [
            SettingKey::MotionAnimations,
            SettingKey::MotionSmoothScrollAmount
        ]
    );
    // Toggling the switch writes through TlSettings.
    cx.update(|cx| pages[0].set_bool(SettingKey::MotionAnimations, !before, cx));
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).motion.animations),
        !before
    );
}

#[gpui_kit::test]
fn page_builder_slider_persists_on_release(cx: &mut TestAppContext) {
    let (_, persists) = setup(cx);
    let (frame, vcx) = open(cx, 1);
    let state = vcx
        .read(|cx| {
            frame
                .read(cx)
                .content()
                .read(cx)
                .slider_state(SettingKey::MotionSmoothScrollAmount)
                .cloned()
        })
        .expect("slider state created in new");
    let before = persists.get();
    cx.update(|cx| state.update(cx, |_, cx| cx.emit(TlSliderEvent::Preview(7.))));
    cx.run_until_parked();
    assert_eq!(persists.get(), before, "Change writes in memory only");
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).motion.smooth_scroll_amount),
        7
    );
    cx.update(|cx| state.update(cx, |_, cx| cx.emit(TlSliderEvent::Commit(8.))));
    cx.run_until_parked();
    assert_eq!(persists.get(), before + 1, "Release persists once");
    assert_eq!(
        cx.update(|cx| TlSettings::settings(cx).motion.smooth_scroll_amount),
        8
    );
}

#[gpui_kit::test]
fn settings_window_ignores_unrelated_snapshots(cx: &mut TestAppContext) {
    let (engine, _) = setup(cx);
    let (frame, mut vcx) = open(cx, 1);
    let content = vcx.read(|cx| frame.read(cx).content().clone());
    let notifies = Rc::new(Cell::new(0));
    let count = notifies.clone();
    let _watch = vcx.update(|_, cx| cx.observe(&content, move |_, _| count.set(count.get() + 1)));
    engine.publish_with(|s| {
        Arc::make_mut(&mut s.processes).header.version += 1;
    });
    cx.run_until_parked();
    assert_eq!(notifies.get(), 0, "a PROCESSES snapshot does not re-render");
    let start = vcx.read(|cx| content.read(cx).render_count());
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        vcx.read(|cx| content.read(cx).render_count()) > start,
        "the counter counts"
    );

    // The device catalog does re-render it.
    engine.publish_with(|s| Arc::make_mut(&mut s.devices).version += 1);
    cx.run_until_parked();
    assert!(notifies.get() > 0, "DEVICES re-renders");
}
