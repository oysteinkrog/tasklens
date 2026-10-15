//! The Search File Handles window (tasklens-gma.33, UI-PWN-030). Run with
//! `--features test-hooks`.

#![cfg(feature = "test-hooks")]

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, WindowHandle};
use tl_model::client::EngineClient;
use tl_model::command::{Command, Tab};
use tl_model::details::{HandleHit, HandleKind, HandlePhase, HandleSearchState, SearchId};
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::ids::ProcessKey;
use tl_model::settings::{Settings, UiState};
use tl_model::time::BootId;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::shield::StartHelperSession;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::open_main_window_on;
use tl_ui::windows::handle_search::{HandleSearchWindow, SearchStatus, open_handle_search};

struct Rig {
    engine: Arc<FakeEngine>,
    win: VisualTestContext,
    view: Entity<HandleSearchWindow>,
}

fn setup(cx: &mut TestAppContext) -> Rig {
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
    let _main = cx
        .update(|cx| open_main_window_on(Tab::Processes, cx))
        .expect("the main window opens");
    cx.run_until_parked();
    cx.update(open_handle_search);
    cx.run_until_parked();
    let (handle, view) = find(cx).expect("the Handle Search window opens");
    let mut win = VisualTestContext::from_window(handle.into(), cx);
    win.update(|window, _| window.activate_window());
    win.run_until_parked();
    Rig { engine, win, view }
}

fn find(cx: &mut TestAppContext) -> Option<(WindowHandle<Root>, Entity<HandleSearchWindow>)> {
    cx.update(|cx| {
        cx.windows().into_iter().find_map(|w| {
            let h = w.downcast::<Root>()?;
            HandleSearchWindow::find(h, cx).map(|v| (h, v))
        })
    })
}

fn draw(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
}

fn set_pattern(rig: &mut Rig, text: &str) {
    let input = rig.win.read(|cx| rig.view.read(cx).pattern().clone());
    let text = text.to_owned();
    rig.win
        .update(|window, cx| input.update(cx, |s, cx| s.set_value(text, window, cx)));
    rig.win.run_until_parked();
}

fn status(rig: &mut Rig) -> String {
    rig.win
        .read(|cx| rig.view.read(cx).status_text(cx).to_string())
}

fn searches(engine: &FakeEngine) -> Vec<(SearchId, bool)> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::HandleSearch {
                search, privileged, ..
            } => Some((search, privileged)),
            _ => None,
        })
        .collect()
}

fn hit() -> HandleHit {
    HandleHit {
        key: ProcessKey {
            start_ticks: 1,
            boot_id: BootId(1),
            pid: 10,
        },
        name: "a".into(),
        user: "u".into(),
        kind: HandleKind::Fd,
        fd: Some(3),
        path: "/tmp/x".into(),
    }
}

fn payload(
    search: SearchId,
    phase: HandlePhase,
    hits: usize,
    skipped: u32,
) -> Arc<HandleSearchState> {
    let chunk: Arc<[HandleHit]> = (0..hits).map(|_| hit()).collect();
    let chunks: Arc<[Arc<[HandleHit]>]> = if hits == 0 {
        Arc::from([])
    } else {
        Arc::from([chunk])
    };
    Arc::new(HandleSearchState {
        search,
        phase,
        progress_pct: 42,
        total_handles: 18234,
        chunks,
        searched: 412,
        total: 530,
        skipped,
    })
}

fn publish(cx: &mut TestAppContext, engine: &FakeEngine, state: Arc<HandleSearchState>) {
    engine.publish_with(|s| {
        let d = Arc::make_mut(&mut s.details);
        d.handle_search = Some(state);
        d.header.version += 1;
    });
    cx.run_until_parked();
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

/// Types a pattern and presses Enter. Returns the id of the search.
fn begin(cx: &mut TestAppContext, rig: &mut Rig) -> SearchId {
    set_pattern(rig, "log");
    rig.win.simulate_keystrokes("enter");
    cx.run_until_parked();
    let sent = searches(&rig.engine);
    sent.last().expect("a search was sent").0
}

#[gpui_kit::test]
fn handle_search_status_sequence(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    // Empty pattern: Search is disabled and Enter sends nothing.
    assert!(!rig.win.read(|cx| rig.view.read(cx).search_enabled(cx)));
    rig.win.simulate_keystrokes("enter");
    assert!(searches(&rig.engine).is_empty());
    assert_eq!(status(&mut rig), "");

    let id = begin(cx, &mut rig);
    assert!(rig.win.read(|cx| rig.view.read(cx).is_searching()));
    let mut seen = vec![status(&mut rig)];

    publish(
        cx,
        &rig.engine,
        payload(id, HandlePhase::Initializing, 0, 0),
    );
    seen.push(status(&mut rig));
    publish(cx, &rig.engine, payload(id, HandlePhase::Retrieving, 0, 0));
    seen.push(status(&mut rig));
    publish(cx, &rig.engine, payload(id, HandlePhase::Searching, 0, 0));
    seen.push(status(&mut rig));
    // A payload of an older search is ignored.
    publish(
        cx,
        &rig.engine,
        payload(SearchId(id.0 - 1), HandlePhase::Done, 3, 0),
    );
    assert_eq!(status(&mut rig), "Searching...");
    publish(cx, &rig.engine, payload(id, HandlePhase::Done, 12, 0));
    seen.push(status(&mut rig));

    assert_eq!(
        seen,
        [
            "Initializing... please wait.",
            "Initializing... please wait.",
            "Retrieving file handles... 42% (18\u{2009}234 total handles)",
            "Searching...",
            "Found 12 matches",
        ]
    );
    assert_eq!(
        rig.win.read(|cx| rig.view.read(cx).state()),
        SearchStatus::Done
    );
    draw(&mut rig.win);
}

#[gpui_kit::test]
fn handle_search_singular_match_text(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    let id = begin(cx, &mut rig);
    publish(cx, &rig.engine, payload(id, HandlePhase::Done, 1, 0));
    assert_eq!(status(&mut rig), "Found 1 match");

    // A second search with no hits.
    let id = begin(cx, &mut rig);
    assert_ne!(id.0, 0);
    publish(cx, &rig.engine, payload(id, HandlePhase::Done, 0, 0));
    assert_eq!(status(&mut rig), "No results found.");
}

#[gpui_kit::test]
fn handle_search_stop_keeps_results(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    let id = begin(cx, &mut rig);
    publish(cx, &rig.engine, payload(id, HandlePhase::Searching, 5, 0));
    rig.win.update(|window, cx| {
        window.dispatch_action(Box::new(tl_ui::windows::actions::StopHandleSearch), cx);
    });
    rig.win.run_until_parked();
    assert!(matches!(
        rig.engine.commands().last(),
        Some(Command::CancelHandleSearch(s)) if *s == id
    ));
    assert_eq!(rig.win.read(|cx| rig.view.read(cx).hit_count()), 5);
    assert_eq!(status(&mut rig), "Search stopped. Found 5 matches");
    // A later payload of the same search adds results and keeps the text.
    publish(cx, &rig.engine, payload(id, HandlePhase::Searching, 6, 0));
    assert_eq!(status(&mut rig), "Search stopped. Found 6 matches");
}

#[gpui_kit::test]
fn handle_search_footer_only_when_skipped(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    let id = begin(cx, &mut rig);
    publish(cx, &rig.engine, payload(id, HandlePhase::Done, 2, 0));
    assert_eq!(rig.win.read(|cx| rig.view.read(cx).footer_note()), None);
    assert!(!rig.win.read(|cx| rig.view.read(cx).search_all_shown()));

    let id = begin(cx, &mut rig);
    publish(cx, &rig.engine, payload(id, HandlePhase::Done, 2, 118));
    assert_eq!(
        rig.win
            .read(|cx| rig.view.read(cx).footer_note().map(|s| s.to_string())),
        Some("Searched 412 of 530 processes. 118 need administrator rights.".to_owned())
    );
    assert!(rig.win.read(|cx| rig.view.read(cx).search_all_shown()));
    draw(&mut rig.win);
}

#[gpui_kit::test]
fn handle_search_esc_stops_then_closes(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    let id = begin(cx, &mut rig);
    rig.win.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(matches!(
        rig.engine.commands().last(),
        Some(Command::CancelHandleSearch(s)) if *s == id
    ));
    assert!(find(cx).is_some(), "the window stays open");
    rig.win.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(find(cx).is_none(), "the window closed");
}

#[gpui_kit::test]
fn handle_search_open_twice_is_one_window(cx: &mut TestAppContext) {
    let _rig = setup(cx);
    cx.update(open_handle_search);
    cx.run_until_parked();
    let count = cx.update(|cx| {
        cx.windows()
            .into_iter()
            .filter_map(|w| w.downcast::<Root>())
            .filter(|h| HandleSearchWindow::find(*h, cx).is_some())
            .count()
    });
    assert_eq!(count, 1);
}

#[gpui_kit::test]
fn handle_search_all_privileged_after_session(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    let starts = Rc::new(Cell::new(0));
    let count = starts.clone();
    cx.update(|cx| {
        cx.on_action(move |_: &StartHelperSession, _| count.set(count.get() + 1));
    });
    publish_helper(cx, &rig.engine, HelperState::Locked, None);
    let id = begin(cx, &mut rig);
    publish(cx, &rig.engine, payload(id, HandlePhase::Done, 2, 118));

    rig.win.update(|window, cx| {
        window.dispatch_action(Box::new(tl_ui::windows::actions::HandleSearchAll), cx);
    });
    rig.win.run_until_parked();
    assert_eq!(starts.get(), 1);
    assert!(rig.win.read(|cx| rig.view.read(cx).is_pending_all()));
    assert_eq!(searches(&rig.engine).len(), 1, "waits for the session");

    publish_helper(cx, &rig.engine, HelperState::Unlocked, None);
    let sent = searches(&rig.engine);
    assert_eq!(sent.len(), 2);
    assert!(sent[1].1, "privileged");
    assert!(!rig.win.read(|cx| rig.view.read(cx).is_pending_all()));
}
