//! The helper session flow (tasklens-qfx.37, UI-FND-028, R-148): shield Actions become engine
//! commands, and a failed start toasts once in the main window unless the window that started
//! it toasts its own cancel. Run with `--features test-hooks`.

#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{AnyWindowHandle, Entity, TestAppContext, VisualTestContext};
use tl_model::client::EngineClient;
use tl_model::command::{Command, Tab};
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::ids::ProcessKey;
use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::ProcessSnapshot;
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};
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

const CANCELLED: &str = "Authentication cancelled";
const FAILED: &str = "Could not start the privileged helper";

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

fn publish_helper(
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

fn sent(engine: &FakeEngine, f: fn(&Command) -> bool) -> usize {
    engine.commands().iter().filter(|c| f(c)).count()
}

fn starts_sent(engine: &FakeEngine) -> usize {
    sent(engine, |c| matches!(c, Command::StartHelperSession))
}

fn pending(cx: &mut TestAppContext) -> Vec<bool> {
    cx.update(|cx| cx.global::<GlobalHelperFlow>().0.read(cx).pending())
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

fn record(pid: u32) -> ProcessRecord {
    let a = |s: &str| Arc::<str>::from(s);
    ProcessRecord {
        key: key(pid),
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: a("demo"),
            name: a("demo"),
            exe: None,
            cmdline: None,
            user: a("u"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: a("demo"),
        }),
        group: GroupKind::System.key(),
        state: ProcState::Sleeping,
        flags: ProcFlags::empty(),
        nice: 0,
        threads: 1,
        cpu_pct: 0.0,
        rss_anon_bytes: 0,
        rss_bytes: 0,
        pss_bytes: 0,
        uss_bytes: 0,
        swap_bytes: 0,
        io_read_bps: 0.0,
        io_write_bps: 0.0,
        net_rx_bps: 0.0,
        net_tx_bps: 0.0,
        gpu_util_pct: 0.0,
        gpu_mem_bytes: 0,
        euid: None,
        net_shared: false,
        cancelled_write_bps: 0.0,
        validity: FieldValidity::all(Validity::Valid),
    }
}

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 100,
        boot_id: BootId(0),
        pid,
    }
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
    assert_eq!(
        pending(cx),
        [true],
        "the main window's start is the module's own"
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
    assert!(pending(cx).is_empty());

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
fn props_start_does_not_toast_from_module(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    let mut props = open_props(cx, &engine, 4321);

    props.dispatch_action(StartHelperSession);
    props.run_until_parked();
    assert_eq!(starts_sent(&engine), 1, "the module sends Props' start");
    assert_eq!(pending(cx), [false], "Props toasts its own cancel");

    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert!(main_toasts(cx).is_empty(), "no toast from this module");
    assert_eq!(
        window_toasts(&mut props),
        [CANCELLED],
        "one toast, Props' own"
    );
}

#[gpui_kit::test]
fn idle_exit_does_not_toast(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    main.dispatch_action(StartHelperSession);
    main.run_until_parked();
    publish_helper(cx, &engine, HelperState::Starting, None);
    publish_helper(cx, &engine, HelperState::Unlocked, None);
    assert!(pending(cx).is_empty(), "a good start answers the start");

    // The helper's 300 s idle exit: back to Locked with no new seq.
    publish_helper(cx, &engine, HelperState::Locked, None);
    assert!(main_toasts(cx).is_empty());
    assert_eq!(starts_sent(&engine), 1, "no start without a user action");
}

#[gpui_kit::test]
fn outcome_without_a_sent_start_does_not_toast(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert!(main_toasts(cx).is_empty());
}

#[gpui_kit::test]
fn overlapping_starts_toast_each_cancel_once(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    main.dispatch_action(StartHelperSession);
    main.run_until_parked();
    let (mut search, view) = open_search(cx);
    search_all(&mut search, &view);
    assert_eq!(starts_sent(&engine), 2);
    assert_eq!(pending(cx), [true, false]);

    // The first cancel answers the main window's start, but Handle Search toasts it.
    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(window_toasts(&mut search), [CANCELLED]);
    assert!(main_toasts(cx).is_empty(), "one toast for the first cancel");

    // The second cancel answers Handle Search's start, which toasts no more.
    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((2, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(
        main_toasts(cx),
        [CANCELLED],
        "one toast for the second cancel"
    );
    assert_eq!(window_toasts(&mut search), [CANCELLED]);
    assert!(pending(cx).is_empty());
}

#[gpui_kit::test]
fn overlapping_cancels_in_one_snapshot_toast_twice(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    main.dispatch_action(StartHelperSession);
    main.run_until_parked();
    let (mut search, view) = open_search(cx);
    search_all(&mut search, &view);

    // Both cancels reach the UI in one snapshot.
    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((2, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(window_toasts(&mut search), [CANCELLED]);
    assert_eq!(main_toasts(cx), [CANCELLED]);
}

#[gpui_kit::test]
fn window_start_first_then_main_start_toasts_each_cancel_once(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    let (mut search, view) = open_search(cx);
    search_all(&mut search, &view);
    main.update(|window, _| window.activate_window());
    main.dispatch_action(StartHelperSession);
    main.run_until_parked();
    assert_eq!(pending(cx), [false, true]);

    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(window_toasts(&mut search), [CANCELLED]);
    assert!(main_toasts(cx).is_empty());

    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((2, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(main_toasts(cx), [CANCELLED]);
    assert_eq!(window_toasts(&mut search), [CANCELLED]);
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
        "no window is left to toast it"
    );
}
