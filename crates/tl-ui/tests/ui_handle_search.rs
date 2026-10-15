//! The Search File Handles window (tasklens-gma.33, UI-PWN-030) and its result table and row
//! menu (tasklens-gma.34, UI-PWN-031). Run with `--features test-hooks`.

#![cfg(feature = "test-hooks")]

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::{
    Action, Entity, Keystroke, TestAppContext, VisualTestContext, WindowHandle, WindowId,
};
use tl_model::actions::{
    ActionEvent, ActionKind, ActionRequest, ActionState, ActionTarget, Signal,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, Tab};
use tl_model::details::{HandleHit, HandleKind, HandlePhase, HandleSearchState, SearchId};
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::ids::ProcessKey;
use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::ProcessSnapshot;
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::dialog::TlDialogView;
use tl_ui::components::menu::{MenuEntryKind, TlMenu};
use tl_ui::components::shield::StartHelperSession;
use tl_ui::components::toast::ToastQueue;
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::pages::processes::ProcessesPage;
use tl_ui::pages::processes::confirm::{ConfirmKind, texts};
use tl_ui::table::tree_table::TreeTableDelegate as _;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::{main_window_view, open_main_window_on};
use tl_ui::windows::actions::{HandleSearchEndProcess, HandleSearchEndTask};
use tl_ui::windows::handle_search::{
    HANDLE_HIT_CAP, HandleSearchOpenLocation, HandleSearchWindow, SearchStatus, open_handle_search,
};

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

// ---- The result table and its row menu (tasklens-gma.34) ----

fn proc_key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: 1_000 + u64::from(pid),
        boot_id: BootId(1),
        pid,
    }
}

/// A hit of process `pid` (named `proc{pid}`) on `path`.
fn hit_of(pid: u32, fd: i32, path: &str) -> HandleHit {
    HandleHit {
        key: proc_key(pid),
        name: format!("proc{pid}").into(),
        user: "me".into(),
        kind: HandleKind::Fd,
        fd: Some(fd),
        path: path.into(),
    }
}

/// An own, unprotected live process `pid`.
fn record(pid: u32, flags: ProcFlags) -> ProcessRecord {
    let a = |s: &str| Arc::<str>::from(s);
    let name = format!("proc{pid}");
    ProcessRecord {
        key: proc_key(pid),
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: a(&name),
            name: a(&name),
            exe: Some(a(&format!("/usr/bin/{name}"))),
            cmdline: None,
            user: a("me"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: a(&name),
        }),
        group: GroupKind::Background.key(),
        state: ProcState::Sleeping,
        flags,
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

/// Publishes own unprotected processes `pids`, sorted by key as the snapshot keeps them.
fn publish_procs(cx: &mut TestAppContext, engine: &FakeEngine, pids: &[u32]) {
    let mut records: Vec<ProcessRecord> = pids
        .iter()
        .map(|&p| record(p, ProcFlags::empty()))
        .collect();
    records.sort_by_key(|r| r.key);
    engine.publish_with(|s| {
        s.processes = Arc::new(ProcessSnapshot {
            records: records.into(),
            ..ProcessSnapshot::default()
        });
    });
    cx.run_until_parked();
}

/// A finished search with `hits`, published, and the table drawn.
fn show_hits(cx: &mut TestAppContext, rig: &mut Rig, hits: Vec<HandleHit>) {
    let id = begin(cx, rig);
    let chunk: Arc<[HandleHit]> = hits.into();
    let state = Arc::new(HandleSearchState {
        search: id,
        phase: HandlePhase::Done,
        progress_pct: 100,
        total_handles: 100,
        chunks: Arc::from([chunk]),
        searched: 2,
        total: 2,
        skipped: 0,
    });
    publish(cx, &rig.engine, state);
    draw(&mut rig.win);
}

/// Makes hit ids `ids` the selection, the first one focused.
fn select(rig: &mut Rig, ids: &[u32]) {
    let table = rig.win.read(|cx| rig.view.read(cx).table().clone());
    rig.win.update(|_, cx| {
        table.update(cx, |t, cx| {
            for (n, id) in ids.iter().enumerate() {
                if n == 0 {
                    t.selection.click(*id);
                } else {
                    t.selection.ctrl_click(*id);
                }
            }
            cx.notify();
        })
    });
    rig.win.run_until_parked();
}

fn window_ids(rig: &mut Rig) -> Vec<WindowId> {
    rig.win
        .update(|_, cx| cx.windows().iter().map(|w| w.window_id()).collect())
}

/// Focuses the table and presses `keys`. Returns the confirmation it opened.
fn press(rig: &mut Rig, keys: &str) -> Option<(VisualTestContext, Entity<TlDialogView>)> {
    let before = window_ids(rig);
    let table = rig.win.read(|cx| rig.view.read(cx).table().clone());
    rig.win.update(|window, cx| {
        let focus = table.read(cx).focus_handle().clone();
        focus.focus(window, cx);
    });
    draw(&mut rig.win);
    rig.win.simulate_keystrokes(keys);
    rig.win.run_until_parked();
    let id = window_ids(rig).into_iter().find(|w| !before.contains(w))?;
    let handle = WindowHandle::<Root>::new(id);
    let view = rig.win.update(|_, cx| {
        TlSecondaryWindow::<TlDialogView>::find(handle, cx).map(|f| f.read(cx).content().clone())
    })?;
    let any = rig
        .win
        .update(|_, cx| cx.windows())
        .into_iter()
        .find(|w| w.window_id() == id)?;
    let mut vcx = VisualTestContext::from_window(any, &rig.win.cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    Some((vcx, view))
}

/// Tab to the action button, then Enter (focus starts on Cancel).
fn confirm_dialog(mut vcx: VisualTestContext) {
    vcx.simulate_keystrokes("tab");
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
}

fn actions_sent(engine: &FakeEngine) -> Vec<ActionRequest> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::Action(request) => Some(request),
            _ => None,
        })
        .collect()
}

fn target_pids(r: &ActionRequest) -> Vec<u32> {
    r.targets
        .iter()
        .map(|t| match t {
            ActionTarget::Process(k) => k.pid,
            other => panic!("not a process target: {other:?}"),
        })
        .collect()
}

/// The toasts shown in the Handle Search window.
fn own_toasts(rig: &mut Rig) -> Vec<String> {
    rig.win.update(|window, cx| {
        let queue = ToastQueue::of_window(window, cx).expect("the window's toast stack");
        let texts = queue.read(cx).visible_texts();
        texts.iter().map(|t| t.to_string()).collect()
    })
}

#[gpui_kit::test]
fn handle_menu_labels_r102(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    show_hits(cx, &mut rig, vec![hit_of(300, 3, "/var/log/a.log")]);
    let table = rig.win.read(|cx| rig.view.read(cx).table().clone());
    rig.win.update(|window, cx| {
        let focus = table.read(cx).focus_handle().clone();
        focus.focus(window, cx);
    });
    draw(&mut rig.win);
    let menu = rig.win.update(|window, cx| {
        let focus = table.read(cx).focus_handle().clone();
        TlMenu::build(window, cx, focus, |menu, window, cx| {
            table.update(cx, |t, cx| {
                t.delegate.context_menu(&[0], None, menu, window, cx)
            })
        })
    });
    let entries = rig
        .win
        .update(|window, cx| menu.read(cx).entries_for_test(window));
    let shape: Vec<String> = entries
        .iter()
        .map(|e| match e.kind {
            MenuEntryKind::Separator => "-".to_owned(),
            _ => e.label.as_deref().unwrap_or_default().to_owned(),
        })
        .collect();
    assert_eq!(
        shape,
        [
            "End Task",
            "End Process",
            "-",
            "Go to process",
            "-",
            "Copy path",
            "Open file location",
        ]
    );
    assert!(
        entries.iter().all(|e| e.kind != MenuEntryKind::Disabled),
        "one row with a path enables every item"
    );
    let hint = |label: &str| {
        entries
            .iter()
            .find(|e| e.label.as_deref() == Some(label))
            .and_then(|e| e.hint.clone())
            .map(|h| h.to_string())
    };
    let format = |key: &str| Kbd::format(&Keystroke::parse(key).expect("a keystroke"));
    // Each item shows one of its bound keys.
    let shows_one_of = |label: &str, keys: &[&str]| {
        let shown = hint(label);
        assert!(
            keys.iter()
                .any(|k| shown.as_deref() == Some(format(k).as_str())),
            "{label}: {shown:?} is not one of {keys:?}"
        );
    };
    shows_one_of("End Task", &["delete", "ctrl-e"]);
    shows_one_of("End Process", &["shift-delete", "ctrl-x"]);
    shows_one_of("Go to process", &["enter"]);
    shows_one_of("Copy path", &["ctrl-c"]);
}

/// Security regression (R-036): two processes ask before anything is sent.
#[gpui_kit::test]
fn handle_end_task_multi_confirms(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    publish_procs(cx, &rig.engine, &[300, 301]);
    show_hits(
        cx,
        &mut rig,
        vec![
            hit_of(300, 3, "/var/log/a.log"),
            hit_of(301, 4, "/var/log/b.log"),
        ],
    );
    select(&mut rig, &[0, 1]);
    let (vcx, view) = press(&mut rig, "delete").expect("the End many confirmation");
    let text = rig
        .win
        .update(|_, cx| view.read(cx).dialog().text().to_string());
    assert_eq!(text, "End 2 selected processes?");
    let title = rig.win.update(|_, cx| {
        texts(
            &ConfirmKind::EndMany {
                count: 2,
                kill: false,
            },
            cx,
        )
        .title
        .to_string()
    });
    assert_eq!(title, "End 2 processes?");
    assert!(
        actions_sent(&rig.engine).is_empty(),
        "nothing is sent before the confirmation"
    );

    confirm_dialog(vcx);
    let sent = actions_sent(&rig.engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, ActionKind::Signal(Signal::Term));
    assert_eq!(target_pids(&sent[0]), [300, 301]);
    assert!(!sent[0].privileged);
}

/// D-009: one own unprotected process ends at once.
#[gpui_kit::test]
fn handle_single_end_process_no_confirm(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    publish_procs(cx, &rig.engine, &[300]);
    show_hits(cx, &mut rig, vec![hit_of(300, 3, "/var/log/a.log")]);
    select(&mut rig, &[0]);
    assert!(
        press(&mut rig, "shift-delete").is_none(),
        "no dialog (D-009)"
    );
    let sent = actions_sent(&rig.engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, ActionKind::Signal(Signal::Kill));
    assert_eq!(target_pids(&sent[0]), [300]);

    rig.engine.emit(ActionEvent {
        id: sent[0].id,
        target: None,
        state: ActionState::Started,
    });
    rig.win.run_until_parked();
    assert!(
        own_toasts(&mut rig)
            .iter()
            .any(|t| t == "End process requested."),
        "the toast shows in the Handle Search window"
    );
}

#[gpui_kit::test]
fn handle_targets_are_distinct_keys(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    publish_procs(cx, &rig.engine, &[300]);
    show_hits(
        cx,
        &mut rig,
        vec![
            hit_of(300, 3, "/var/log/a.log"),
            hit_of(300, 4, "/var/log/b.log"),
        ],
    );
    select(&mut rig, &[0, 1]);
    rig.win
        .update(|window, cx| window.dispatch_action(HandleSearchEndTask.boxed_clone(), cx));
    rig.win.run_until_parked();
    // One own process: no confirmation, one target.
    let sent = actions_sent(&rig.engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, ActionKind::Signal(Signal::Term));
    assert_eq!(target_pids(&sent[0]), [300]);

    // A process that exited after the search is still sent, for the engine's identity check.
    publish_procs(cx, &rig.engine, &[]);
    rig.win
        .update(|window, cx| window.dispatch_action(HandleSearchEndProcess.boxed_clone(), cx));
    rig.win.run_until_parked();
    let sent = actions_sent(&rig.engine);
    assert_eq!(sent.len(), 2);
    assert_eq!(target_pids(&sent[1]), [300]);
}

#[gpui_kit::test]
fn handle_go_to_process_dispatches_entry_point(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    publish_procs(cx, &rig.engine, &[300, 301]);
    show_hits(
        cx,
        &mut rig,
        vec![
            hit_of(301, 3, "/var/log/a.log"),
            hit_of(300, 4, "/var/log/b.log"),
        ],
    );
    let page = rig.win.update(|_, cx| {
        main_window_view(cx)
            .and_then(|v| v.read(cx).page_entity::<ProcessesPage>(Tab::Processes))
            .expect("the Processes page is built")
    });
    let generation = |rig: &mut Rig| {
        rig.win
            .update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation())
    };
    let before = generation(&mut rig);
    // Default sort: proc300 is the first row.
    let first = rig.win.read(|cx| {
        rig.view
            .read(cx)
            .table()
            .read(cx)
            .delegate
            .view_ids()
            .next()
    });
    assert_eq!(first, Some(1));
    select(&mut rig, &[1]);
    assert!(press(&mut rig, "enter").is_none());
    assert!(
        generation(&mut rig) > before,
        "the jump asks the Processes page for the row's key"
    );
    assert!(
        searches(&rig.engine).len() == 1,
        "Enter in the table does not start a search"
    );
}

#[gpui_kit::test]
fn handle_hit_cap_status(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    let id = begin(cx, &mut rig);
    publish(
        cx,
        &rig.engine,
        payload(id, HandlePhase::Done, HANDLE_HIT_CAP, 0),
    );
    assert_eq!(
        status(&mut rig),
        "Showing the first 100\u{2009}000 matches."
    );
    let rows = rig
        .win
        .read(|cx| rig.view.read(cx).table().read(cx).delegate.row_count());
    assert_eq!(rows, HANDLE_HIT_CAP);

    // A search that reaches the cap is stopped.
    let id = begin(cx, &mut rig);
    publish(
        cx,
        &rig.engine,
        payload(id, HandlePhase::Searching, HANDLE_HIT_CAP, 0),
    );
    assert!(matches!(
        rig.engine.commands().last(),
        Some(Command::CancelHandleSearch(s)) if *s == id
    ));
    assert_eq!(
        rig.win.read(|cx| rig.view.read(cx).state()),
        SearchStatus::Done
    );
    assert_eq!(
        status(&mut rig),
        "Showing the first 100\u{2009}000 matches."
    );
    draw(&mut rig.win);
}

/// Security regression: more than 256 distinct processes send nothing and name the limit,
/// rather than ending only the first 256.
#[gpui_kit::test]
fn handle_end_over_target_limit_sends_nothing(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    let hits: Vec<HandleHit> = (0..257)
        .map(|n| hit_of(300 + n, 3, "/var/log/a.log"))
        .collect();
    show_hits(cx, &mut rig, hits);
    let ids: Vec<u32> = (0..257).collect();
    select(&mut rig, &ids);
    let before = window_ids(&mut rig);
    rig.win
        .update(|window, cx| window.dispatch_action(HandleSearchEndProcess.boxed_clone(), cx));
    rig.win.run_until_parked();
    assert!(actions_sent(&rig.engine).is_empty(), "nothing is sent");
    assert_eq!(window_ids(&mut rig), before, "no confirmation opens");
    assert!(
        own_toasts(&mut rig)
            .iter()
            .any(|t| t == "Too many processes selected. The limit is 256."),
        "the limit is reported in the Handle Search window"
    );
}

/// The item acts on the selected row even when a Ctrl-click left the focus on a deselected one.
#[gpui_kit::test]
fn handle_open_location_uses_selected_row(cx: &mut TestAppContext) {
    let mut rig = setup(cx);
    show_hits(
        cx,
        &mut rig,
        vec![
            hit_of(300, 3, "/var/log/a.log"),
            hit_of(301, 4, "/srv/b.log"),
        ],
    );
    // Select A and B, then Ctrl-click B again: only A stays selected, the focus is on B.
    select(&mut rig, &[0, 1, 1]);
    let (focus, selected) = rig.win.read(|cx| {
        let t = rig.view.read(cx).table().read(cx);
        (t.selection.focus(), t.selection.ids().collect::<Vec<_>>())
    });
    assert_eq!((focus, selected), (Some(1), vec![0]));
    rig.win
        .update(|window, cx| window.dispatch_action(HandleSearchOpenLocation.boxed_clone(), cx));
    rig.win.run_until_parked();
    let sent = actions_sent(&rig.engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].kind,
        ActionKind::ShowInFileManager {
            path: "/var/log/a.log".into(),
            properties: false,
        }
    );
    assert!(sent[0].targets.is_empty());
}
