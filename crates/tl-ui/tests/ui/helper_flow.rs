//! The helper session flow (tasklens-qfx.37, UI-FND-028, R-148): shield Actions become engine
//! commands, and each failed start toasts once, in the window that asked for the last start or
//! else in the main window. Run with `--features test-hooks`.

#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{AnyWindowHandle, Entity, TestAppContext, VisualTestContext};
use tl_model::client::EngineClient;
use tl_model::command::{Command, Tab};
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::ProcessSnapshot;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::shield::{EndHelperSession, StartHelperSession};
use tl_ui::components::toast::ToastQueue;
use tl_ui::helper_flow::GlobalHelperFlow;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::{main_window_view, open_main_window_on};
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::windows::actions::HandleSearchAll;
use tl_ui::windows::handle_search::{HandleSearchWindow, open_handle_search};
use tl_ui::windows::props::open_process_properties;

use super::common::{CANCELLED, FAILED, key, publish_helper, record, sent, starts_sent};

/// App globals and the main window on Processes, active.
fn setup(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let init = UiInit {
        settings: Settings::default(),
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        tl_ui::i18n::init_strings(cx, "en-US");
        cx.set_reduce_motion(true);
    });
    let handle = cx
        .update(|cx| open_main_window_on(Tab::Processes, cx))
        .expect("the main window opens");
    let mut main = VisualTestContext::from_window(handle.into(), cx);
    main.update(|window, _| window.activate_window());
    main.run_until_parked();
    (engine, main)
}

fn requester(cx: &mut TestAppContext) -> Option<AnyWindowHandle> {
    cx.update(|cx| cx.global::<GlobalHelperFlow>().0.read(cx).requester())
}

fn main_toasts(cx: &mut TestAppContext) -> Vec<String> {
    let main = cx.update(|cx| main_window_view(cx).expect("main window open"));
    cx.update(|cx| {
        let queue = main.read(cx).toasts().read(cx);
        queue
            .visible_texts()
            .into_iter()
            .chain(queue.waiting_texts())
            .map(|t| t.to_string())
            .collect()
    })
}

/// Opens the Process Properties window for `pid` and activates it.
fn open_props(cx: &mut TestAppContext, engine: &FakeEngine, pid: u32) -> VisualTestContext {
    engine.publish_with(|s| {
        let p = Arc::make_mut(&mut s.processes);
        *p = ProcessSnapshot {
            records: vec![record(pid)].into(),
            ..ProcessSnapshot::default()
        };
        p.header.version += 1;
    });
    cx.run_until_parked();
    cx.update(|cx| open_process_properties(key(pid), cx));
    cx.run_until_parked();
    let handle: AnyWindowHandle = cx
        .update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry.read(cx).get(&WindowKey::Properties(key(pid)))
        })
        .expect("properties opened");
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    vcx
}

/// Opens the Handle Search window, activates it and types a pattern.
fn open_search(cx: &mut TestAppContext) -> (VisualTestContext, Entity<HandleSearchWindow>) {
    cx.update(open_handle_search);
    cx.run_until_parked();
    let (handle, view) = cx
        .update(|cx| {
            cx.windows().into_iter().find_map(|w| {
                let h = w.downcast::<Root>()?;
                HandleSearchWindow::find(h, cx).map(|v| (h, v))
            })
        })
        .expect("the Handle Search window opens");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    let input = vcx.read(|cx| view.read(cx).pattern().clone());
    vcx.update(|window, cx| input.update(cx, |s, cx| s.set_value("log", window, cx)));
    vcx.run_until_parked();
    (vcx, view)
}

/// Clicks "Search all" while the helper is locked: Handle Search dispatches a start.
fn search_all(vcx: &mut VisualTestContext, view: &Entity<HandleSearchWindow>) {
    vcx.update(|window, cx| window.dispatch_action(Box::new(HandleSearchAll), cx));
    vcx.run_until_parked();
    assert!(vcx.read(|cx| view.read(cx).is_pending_all()));
}

fn window_toasts(vcx: &mut VisualTestContext) -> Vec<String> {
    vcx.update(|window, cx| {
        let queue = ToastQueue::of_window(window, cx).expect("toast queue");
        let queue = queue.read(cx);
        queue
            .visible_texts()
            .into_iter()
            .chain(queue.waiting_texts())
            .map(|t| t.to_string())
            .collect()
    })
}

#[gpui_kit::test]
fn main_window_start_sends_one_command(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);

    main.dispatch_action(StartHelperSession);
    main.run_until_parked();

    assert_eq!(starts_sent(&engine), 1);
    let main_id = main.update(|window, _| window.window_handle().window_id());
    assert_eq!(
        requester(cx).map(|w| w.window_id()),
        Some(main_id),
        "the main window asked for the start"
    );
}

#[gpui_kit::test]
fn unlocked_shield_sends_end(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Unlocked, None);

    main.dispatch_action(EndHelperSession);
    main.run_until_parked();

    assert_eq!(sent(&engine, |c| matches!(c, Command::EndHelperSession)), 1);
    assert_eq!(starts_sent(&engine), 0);
}

#[gpui_kit::test]
fn own_cancel_toasts_once(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    main.dispatch_action(StartHelperSession);
    main.run_until_parked();
    publish_helper(cx, &engine, HelperState::Starting, None);

    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(main_toasts(cx), [CANCELLED]);
    assert!(requester(cx).is_none(), "the outcome clears the record");

    // The same seq again, in a later health change, toasts nothing.
    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(main_toasts(cx), [CANCELLED]);
}

#[gpui_kit::test]
fn own_failure_toasts_start_failed(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    // The queue merges an equal text into the visible toast, so the second shows "(×2)".
    let expected = [FAILED.to_string(), format!("{FAILED} (\u{d7}2)")];
    for (seq, outcome, expected) in [
        (1, HelperStartOutcome::NotAuthorized, &expected[0]),
        (
            2,
            HelperStartOutcome::Failed("no answer from the helper".into()),
            &expected[1],
        ),
    ] {
        main.dispatch_action(StartHelperSession);
        main.run_until_parked();
        publish_helper(cx, &engine, HelperState::Locked, Some((seq, outcome)));
        assert_eq!(main_toasts(cx), [expected.as_str()], "seq {seq}");
    }
}

#[gpui_kit::test]
fn idle_exit_does_not_toast(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    main.dispatch_action(StartHelperSession);
    main.run_until_parked();
    publish_helper(cx, &engine, HelperState::Starting, None);
    publish_helper(cx, &engine, HelperState::Unlocked, None);
    assert!(requester(cx).is_none(), "a good start clears the record");

    // The helper's 300 s idle exit: back to Locked with no new seq.
    publish_helper(cx, &engine, HelperState::Locked, None);
    assert!(main_toasts(cx).is_empty());
    assert_eq!(starts_sent(&engine), 1, "no start without a user action");
}

#[gpui_kit::test]
fn props_cancel_toasts_in_props_only(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    let mut props = open_props(cx, &engine, 4321);

    props.dispatch_action(StartHelperSession);
    props.run_until_parked();
    assert_eq!(starts_sent(&engine), 1, "the module sends Props' start");
    let props_id = props.update(|window, _| window.window_handle().window_id());
    assert_eq!(requester(cx).map(|w| w.window_id()), Some(props_id));

    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(
        window_toasts(&mut props),
        [CANCELLED],
        "one toast, in Props"
    );
    assert!(main_toasts(cx).is_empty(), "no toast in the main window");
}

#[gpui_kit::test]
fn main_start_then_search_all_toasts_once_in_search(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    main.dispatch_action(StartHelperSession);
    main.run_until_parked();
    let (mut search, view) = open_search(cx);
    search_all(&mut search, &view);
    assert_eq!(starts_sent(&engine), 2);

    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(window_toasts(&mut search), [CANCELLED]);
    assert!(
        main_toasts(cx).is_empty(),
        "exactly one toast for one cancel"
    );
    assert!(!search.read(|cx| view.read(cx).is_pending_all()));
}

#[gpui_kit::test]
fn closed_window_cancel_toasts_in_main(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    let (mut search, view) = open_search(cx);
    search_all(&mut search, &view);
    search.update(|window, _| window.remove_window());
    search.run_until_parked();

    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(
        main_toasts(cx),
        [CANCELLED],
        "the requesting window is gone"
    );
}

#[gpui_kit::test]
fn cleared_search_still_toasts_once(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    let (mut search, view) = open_search(cx);
    search_all(&mut search, &view);
    search.update(|window, cx| view.update(cx, |v, cx| v.clear(window, cx)));
    search.run_until_parked();
    assert!(!search.read(|cx| view.read(cx).is_pending_all()));

    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    let toasts = window_toasts(&mut search).len() + main_toasts(cx).len();
    assert_eq!(toasts, 1, "exactly one toast");
    assert_eq!(window_toasts(&mut search), [CANCELLED]);
}
