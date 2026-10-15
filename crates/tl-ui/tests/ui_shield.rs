//! Shield flows end to end against the fake engine (tasklens-qfx.31, APP-003, PROC-058, PROC-076,
//! UI-FND-028, UI-SHL-011, UI-SHL-050, UI-PWN-018, UI-PWN-031, R-117, R-148): every entry point
//! that should start the helper sends the start, the shield follows `HealthSnapshot.helper`, a
//! failed start toasts once, and nothing escalates on its own. Run with `--features test-hooks`.

#![cfg(feature = "test-hooks")]

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, Context, Entity, FocusHandle, IntoElement, KeyDownEvent, KeyUpEvent,
    Keystroke, Modifiers, ParentElement as _, Pixels, Render, Styled as _, TestAppContext, TextRun,
    VisualTestContext, Window, div, font, point, px,
};
use tl_model::actions::{
    ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    Signal,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, Tab};
use tl_model::details::{
    DetailTab, DetailTarget, HandleHit, HandleKind, HandlePhase, HandleSearchState, SearchId,
    TargetDetail,
};
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::ids::ProcessKey;
use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::settings::{Settings, UiState};
use tl_model::snapshot::ProcessSnapshot;
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};
use tl_ui::action_tracker::{GlobalActionTracker, ToastTexts};
use tl_ui::actions::OpenSettings;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::shield::TlShield;
use tl_ui::components::toast::ToastQueue;
use tl_ui::components::tooltip::DELAY_CONTROL;
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::pages::processes::ProcessesPage;
use tl_ui::settings_global::{SettingKey, TlSettings};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::text::fonts::UI_FAMILY;
use tl_ui::text::style::t_sm;
use tl_ui::theme::{current, metrics};
use tl_ui::window::main::{HelperInstalled, MainWindow, main_window_view, open_main_window_on};
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::settings::{PageId, SettingsWindow, all_pages};
use tl_ui::windows::handle_search::{HandleSearchWindow, open_handle_search};
use tl_ui::windows::props::{AccessButton, PropsWindow, open_process_properties};

const CANCELLED: &str = "Authentication cancelled";
const FAILED: &str = "Could not start the privileged helper";
const TIP_LOCKED: &str = "Unlock privileged actions";
const TIP_UNLOCKED: &str = "Privileged actions unlocked (click to lock)";
const PERMISSION_DENIED: &str = "Permission denied";
const RETRY: &str = "Retry as administrator";
const NO_HELPER: &str = "Needs administrator helper (not installed)";
const PID: u32 = 4321;

// ---- Setup ----

/// App globals with `settings`, the helper installed, and the main window on Processes, active,
/// past its first frame (so the launch steps have run).
fn setup_with(cx: &mut TestAppContext, settings: Settings) -> (Arc<FakeEngine>, VisualTestContext) {
    setup_counting(cx, settings, Rc::new(Cell::new(0)))
}

/// `setup_with`, and `persists` counts the persist callback calls.
fn setup_counting(
    cx: &mut TestAppContext,
    settings: Settings,
    persists: Rc<Cell<u32>>,
) -> (Arc<FakeEngine>, VisualTestContext) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let init = UiInit {
        settings,
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(move |_, _| persists.set(persists.get() + 1)),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        tl_ui::i18n::init_strings(cx, "en-US");
        cx.set_global(HelperInstalled(true));
        cx.set_reduce_motion(true);
    });
    let handle = cx
        .update(|cx| open_main_window_on(Tab::Processes, cx))
        .expect("the main window opens");
    let mut main = VisualTestContext::from_window(handle.into(), cx);
    main.update(|window, _| window.activate_window());
    draw(&mut main);
    main.update(|window, cx| window.simulate_next_frame(cx));
    draw(&mut main);
    (engine, main)
}

fn setup(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
    setup_with(cx, Settings::default())
}

fn draw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
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

fn ends_sent(engine: &FakeEngine) -> usize {
    sent(engine, |c| matches!(c, Command::EndHelperSession))
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

fn main_window(cx: &mut TestAppContext) -> Entity<MainWindow> {
    cx.update(|cx| main_window_view(cx).expect("main window open"))
}

fn queue_texts(queue: &ToastQueue) -> Vec<String> {
    queue
        .visible_texts()
        .into_iter()
        .chain(queue.waiting_texts())
        .map(|t| t.to_string())
        .collect()
}

fn main_toasts(cx: &mut TestAppContext) -> Vec<String> {
    let main = main_window(cx);
    cx.update(|cx| queue_texts(main.read(cx).toasts().read(cx)))
}

fn window_toasts(vcx: &mut VisualTestContext) -> Vec<String> {
    vcx.update(|window, cx| {
        let queue = ToastQueue::of_window(window, cx).expect("toast queue");
        queue_texts(queue.read(cx))
    })
}

fn click(vcx: &mut VisualTestContext, selector: &str) {
    draw(vcx);
    let selector: &'static str = selector.to_owned().leak();
    let at = vcx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} drawn"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

/// Key down and key up: a focused button activates on the pair.
fn press(vcx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).expect("keystroke");
    vcx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    vcx.simulate_event(KeyUpEvent { keystroke });
    vcx.run_until_parked();
}

// ---- The toolbar shield ----

/// Hovers the shield until its tooltip shows. Returns the tooltip text width, `None` when no
/// tooltip shows.
fn shield_tooltip_width(main: &mut VisualTestContext) -> Option<Pixels> {
    main.simulate_mouse_move(point(px(1.), px(400.)), None, Modifiers::none());
    draw(main);
    let at = main
        .debug_bounds("TlShield")
        .expect("shield drawn")
        .center();
    main.simulate_mouse_move(at, None, Modifiers::none());
    main.executor().advance_clock(DELAY_CONTROL);
    main.run_until_parked();
    draw(main);
    draw(main);
    main.debug_bounds("TlTooltip-text").map(|b| b.size.width)
}

/// Shaped width of `text` in the tooltip style.
fn tooltip_text_width(vcx: &mut VisualTestContext, text: &'static str) -> Pixels {
    vcx.update(|window, cx| {
        let size = t_sm(metrics(cx)).font_size.expect("t_sm sets a size");
        let size = size.to_pixels(window.rem_size());
        let run = TextRun {
            len: text.len(),
            font: font(UI_FAMILY),
            color: current(cx).text_primary,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_line(text.into(), size, &[run], None)
            .width
    })
}

/// The shield shows a tooltip of `text`: its drawn width matches, and so does the accessible
/// name when the window builds an accessibility tree.
fn assert_shield_tooltip(main: &mut VisualTestContext, text: &'static str) {
    let drawn = shield_tooltip_width(main).expect("the shield shows a tooltip");
    let expected = tooltip_text_width(main, text);
    assert!(
        (f32::from(drawn) - f32::from(expected)).abs() <= 1.,
        "tooltip {drawn:?}, \"{text}\" is {expected:?}"
    );
    if let Some(tree) = main.update(|window, _| window.debug_a11y_tree_json()) {
        assert!(tree.contains(text), "the accessible name is \"{text}\"");
    }
}

/// A shield with keyboard focus in its own window, for Enter: toolbar buttons are not Tab stops
/// (UI-SHL-010), so the toolbar's shield never takes keyboard focus.
struct ShieldHost {
    state: HelperState,
    focus: FocusHandle,
}

impl Render for ShieldHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size(px(200.))
            .child(TlShield::new(self.state).track_focus(&self.focus))
    }
}

#[gpui_kit::test]
fn shield_click_sends_start(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);

    click(&mut main, "TlShield");
    assert_eq!(starts_sent(&engine), 1, "one StartHelperSession per click");
    assert_eq!(ends_sent(&engine), 0);

    // Enter on a focused shield goes the same way: the Action, then the command.
    let (host, keys) = cx.add_window_view(|_, cx| ShieldHost {
        state: HelperState::Locked,
        focus: cx.focus_handle(),
    });
    keys.update(|window, _| window.activate_window());
    let focus = keys.read(|cx| host.read(cx).focus.clone());
    keys.update(|window, cx| window.focus(&focus, cx));
    draw(keys);
    press(keys, "enter");
    assert_eq!(starts_sent(&engine), 2, "Enter sends StartHelperSession");
}

#[gpui_kit::test]
fn shield_unlocked_click_sends_end(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Unlocked, None);

    click(&mut main, "TlShield");
    assert_eq!(ends_sent(&engine), 1);
    assert_eq!(starts_sent(&engine), 0);
}

#[gpui_kit::test]
fn shield_states_follow_health(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);

    publish_helper(cx, &engine, HelperState::Missing, None);
    draw(&mut main);
    assert!(main.debug_bounds("TlShield").is_none(), "Missing: hidden");

    publish_helper(cx, &engine, HelperState::Locked, None);
    draw(&mut main);
    assert!(main.debug_bounds("TlShield-icon-shield").is_some());
    assert_shield_tooltip(&mut main, TIP_LOCKED);

    publish_helper(cx, &engine, HelperState::Starting, None);
    draw(&mut main);
    assert!(main.debug_bounds("TlShield-spinner").is_some());
    click(&mut main, "TlShield");
    assert_eq!(
        (starts_sent(&engine), ends_sent(&engine)),
        (0, 0),
        "Starting: the spinner is inert, a click sends nothing"
    );

    publish_helper(cx, &engine, HelperState::Unlocked, None);
    draw(&mut main);
    assert!(main.debug_bounds("TlShield-icon-unlocked").is_some());
    assert!(main.debug_bounds("TlShield-spinner").is_none());
    assert_shield_tooltip(&mut main, TIP_UNLOCKED);
}

// ---- Start outcome toasts ----

#[gpui_kit::test]
fn start_failure_toasts_once(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    click(&mut main, "TlShield");
    assert_eq!(starts_sent(&engine), 1);
    publish_helper(cx, &engine, HelperState::Starting, None);

    // The same seq in two snapshots: one toast.
    for _ in 0..2 {
        publish_helper(
            cx,
            &engine,
            HelperState::Locked,
            Some((1, HelperStartOutcome::Cancelled)),
        );
    }
    assert_eq!(main_toasts(cx), [CANCELLED]);

    click(&mut main, "TlShield");
    assert_eq!(starts_sent(&engine), 2);
    publish_helper(cx, &engine, HelperState::Starting, None);
    for _ in 0..2 {
        publish_helper(
            cx,
            &engine,
            HelperState::Locked,
            Some((
                2,
                HelperStartOutcome::Failed("pkexec exited with 127".into()),
            )),
        );
    }
    assert_eq!(main_toasts(cx), [CANCELLED, FAILED]);
}

#[gpui_kit::test]
fn idle_exit_no_toast(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    click(&mut main, "TlShield");
    publish_helper(cx, &engine, HelperState::Starting, None);
    publish_helper(cx, &engine, HelperState::Unlocked, None);
    // The helper's idle exit: back to Locked with `helper_start` unchanged.
    publish_helper(cx, &engine, HelperState::Locked, None);
    assert!(main_toasts(cx).is_empty());

    // An earlier outcome left in `helper_start` does not toast again on a later idle exit.
    click(&mut main, "TlShield");
    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(main_toasts(cx), [CANCELLED]);
    click(&mut main, "TlShield");
    let seq1 = Some((1, HelperStartOutcome::Cancelled));
    publish_helper(cx, &engine, HelperState::Starting, seq1.clone());
    publish_helper(cx, &engine, HelperState::Unlocked, seq1.clone());
    publish_helper(cx, &engine, HelperState::Locked, seq1);
    assert_eq!(main_toasts(cx), [CANCELLED], "no toast for the idle exit");
    assert_eq!(starts_sent(&engine), 3, "no start without a user action");
}

// ---- The Processes status chip ----

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

#[gpui_kit::test]
fn restricted_segment_starts_helper(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    let page = main_window(cx)
        .read_with(cx, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
        .expect("Processes built");
    let generation = page.read_with(cx, |p, cx| p.table().read(cx).delegate.query().generation());
    // A view for the current query with 42 rows the engine could not read.
    engine.publish_with(|s| {
        let t = Arc::make_mut(&mut s.table);
        t.query_generation = generation;
        t.total = 296;
        t.shown = 296;
        t.restricted = 42;
        t.header.version += 1;
    });
    draw(&mut main);
    let labels: Vec<String> = page.read_with(cx, |p, _| {
        p.status_actions()
            .iter()
            .map(|a| a.label.to_string())
            .collect()
    });
    assert_eq!(labels, ["42 restricted"]);

    click(&mut main, "StatusChip-action-0");
    assert_eq!(
        starts_sent(&engine),
        1,
        "the segment sends StartHelperSession"
    );
}

// ---- Retry as administrator ----

/// Sends End task for pid 101 through the app's tracker, with its toasts in the main window, and
/// fails it with `PermissionDenied`. Returns the request id.
fn end_task_denied(cx: &mut TestAppContext, engine: &FakeEngine) -> (ActionId, ActionRequest) {
    let target = ActionTarget::Process(key(101));
    let request = ActionRequest {
        id: ActionId(0),
        kind: ActionKind::Signal(Signal::Term),
        targets: vec![target].into_iter().collect(),
        privileged: false,
    };
    let texts = ToastTexts {
        retry_as_admin: true,
        ..ToastTexts::new("Process terminated", "Could not end task.")
    };
    let main = main_window(cx);
    let id = cx
        .update(|cx| {
            let queue = main.read(cx).toasts().downgrade();
            let tracker = cx.global::<GlobalActionTracker>().0.clone();
            tracker.update(cx, |t, cx| t.track(request, queue, texts, cx))
        })
        .expect("sent");
    cx.run_until_parked();
    let eperm = ActionState::Failed(ActionError::PermissionDenied);
    for (target, state) in [
        (None, ActionState::Started),
        (Some(0), eperm.clone()),
        (None, eperm),
    ] {
        engine.emit(ActionEvent { id, target, state });
    }
    cx.run_until_parked();
    let sent = actions_sent(engine);
    let first = sent.last().expect("the request was sent").clone();
    (id, first)
}

fn retry_button(id: ActionId) -> String {
    format!("TlButton-TlToast-k{}-action", id.0)
}

#[gpui_kit::test]
fn retry_as_administrator_sends_privileged(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
    let (id, first) = end_task_denied(cx, &engine);
    assert!(!first.privileged);
    assert_eq!(main_toasts(cx), [PERMISSION_DENIED]);
    draw(&mut main);
    let button = retry_button(id);
    assert!(
        main.debug_bounds(button.clone().leak()).is_some(),
        "the toast has \"{RETRY}\""
    );
    let pushed = cx.update(|cx| {
        let tracker = cx.global::<GlobalActionTracker>().0.clone();
        tracker.read(cx).pushed().last().cloned().expect("a toast")
    });
    assert_eq!(pushed.button.as_deref(), Some(RETRY));

    // Security: no privileged request without the click, not after a wait, not when the helper
    // unlocks for another reason. 10 s keeps the toast up: a toast with a button stays 15 s.
    main.executor()
        .advance_clock(std::time::Duration::from_secs(10));
    main.run_until_parked();
    publish_helper(cx, &engine, HelperState::Unlocked, None);
    publish_helper(cx, &engine, HelperState::Locked, None);
    assert!(
        actions_sent(&engine).iter().all(|r| !r.privileged),
        "nothing escalates without the click"
    );
    assert_eq!(starts_sent(&engine), 0, "no helper start without the click");

    click(&mut main, &button);
    let sent = actions_sent(&engine);
    assert_eq!(sent.len(), 2);
    let retry = &sent[1];
    assert!(retry.privileged);
    assert_ne!(retry.id, first.id, "a new ActionId");
    assert_eq!(retry.kind, first.kind);
    assert_eq!(retry.targets, first.targets);
    assert_eq!(
        actions_sent(&engine)
            .iter()
            .filter(|r| r.privileged)
            .count(),
        1,
        "one privileged request, for the one click"
    );
}

#[gpui_kit::test]
fn retry_hidden_without_helper(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Missing, None);
    let (id, _) = end_task_denied(cx, &engine);
    let toasts = main_toasts(cx);
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert!(toasts[0].contains(PERMISSION_DENIED), "{toasts:?}");
    let pushed = cx.update(|cx| {
        let tracker = cx.global::<GlobalActionTracker>().0.clone();
        tracker.read(cx).pushed().last().cloned().expect("a toast")
    });
    assert_eq!(pushed.button, None, "no Retry button");
    draw(&mut main);
    assert!(main.debug_bounds(retry_button(id).leak()).is_none());
    assert!(actions_sent(&engine).iter().all(|r| !r.privileged));
}

// ---- Grant access in Process Properties ----

/// Opens Process Properties for `PID` on the File descriptors tab with a `Restricted` payload.
fn restricted_fds(
    cx: &mut TestAppContext,
    engine: &FakeEngine,
) -> (Entity<PropsWindow>, VisualTestContext) {
    engine.publish_with(|s| {
        let p = Arc::make_mut(&mut s.processes);
        *p = ProcessSnapshot {
            records: vec![record(PID)].into(),
            ..ProcessSnapshot::default()
        };
        p.header.version += 1;
    });
    cx.run_until_parked();
    cx.update(|cx| open_process_properties(key(PID), cx));
    cx.run_until_parked();
    let handle: AnyWindowHandle = cx
        .update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry.read(cx).get(&WindowKey::Properties(key(PID)))
        })
        .expect("properties opened");
    let view = cx
        .update(|cx| PropsWindow::find(handle, cx))
        .expect("props window");
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.update(|window, _| window.activate_window());
    vcx.update(|window, cx| view.update(cx, |p, cx| p.select_tab(DetailTab::Fds, window, cx)));
    let target = DetailTarget::Process(key(PID));
    engine.publish_with(|s| {
        let d = Arc::make_mut(&mut s.details);
        let mut detail = TargetDetail::default();
        detail.tabs[DetailTab::Fds.index()].validity = Validity::Restricted;
        d.targets.insert(target, Arc::new(detail));
        d.header.version += 1;
    });
    draw(&mut vcx);
    (view, vcx)
}

fn focus_access_button(vcx: &mut VisualTestContext, view: &Entity<PropsWindow>) {
    let focus = vcx.read(|cx| view.read(cx).access_focus().clone());
    vcx.update(|window, cx| window.focus(&focus, cx));
    draw(vcx);
}

#[gpui_kit::test]
fn grant_access_starts_helper(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);

    // Without the helper the button is disabled and says why.
    publish_helper(cx, &engine, HelperState::Missing, None);
    let (view, mut props) = restricted_fds(cx, &engine);
    let b = props
        .update(|_, cx| view.read(cx).access_button())
        .expect("the Grant access button");
    assert_eq!(b.kind, AccessButton::GrantAccess);
    assert!(b.disabled);
    assert_eq!(b.tooltip.as_deref(), Some(NO_HELPER));
    focus_access_button(&mut props, &view);
    press(&mut props, "enter");
    assert_eq!(starts_sent(&engine), 0, "a disabled button sends nothing");

    // With the helper locked, Enter on the button starts the session.
    publish_helper(cx, &engine, HelperState::Locked, None);
    draw(&mut props);
    let b = props
        .update(|_, cx| view.read(cx).access_button())
        .expect("the Grant access button");
    assert_eq!(b.label.as_ref(), "Grant access");
    assert!(!b.disabled);
    focus_access_button(&mut props, &view);
    press(&mut props, "enter");
    assert_eq!(
        starts_sent(&engine),
        1,
        "Grant access sends StartHelperSession"
    );

    // A cancel of the start Props asked for toasts in Props only.
    publish_helper(cx, &engine, HelperState::Starting, None);
    publish_helper(
        cx,
        &engine,
        HelperState::Locked,
        Some((1, HelperStartOutcome::Cancelled)),
    );
    assert_eq!(window_toasts(&mut props), [CANCELLED]);
    assert!(main_toasts(cx).is_empty(), "no toast in the main window");
}

// ---- Handle Search "Search all" ----

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

#[gpui_kit::test]
fn search_all_starts_helper_and_resends(cx: &mut TestAppContext) {
    let (engine, _main) = setup(cx);
    publish_helper(cx, &engine, HelperState::Locked, None);
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
    let mut search = VisualTestContext::from_window(handle.into(), cx);
    search.update(|window, _| window.activate_window());
    let input = search.read(|cx| view.read(cx).pattern().clone());
    search.update(|window, cx| input.update(cx, |s, cx| s.set_value("log", window, cx)));
    search.run_until_parked();
    search.simulate_keystrokes("enter");
    cx.run_until_parked();
    let id = searches(&engine).last().expect("a search was sent").0;
    assert!(
        !searches(&engine)[0].1,
        "the first search is not privileged"
    );

    // The engine skipped 118 processes it may not read: the footer offers "Search all".
    let hit = HandleHit {
        key: key(10),
        name: "a".into(),
        user: "u".into(),
        kind: HandleKind::Fd,
        fd: Some(3),
        path: "/var/log/a.log".into(),
    };
    let chunk: Arc<[HandleHit]> = Arc::from(vec![hit]);
    engine.publish_with(|s| {
        let d = Arc::make_mut(&mut s.details);
        d.handle_search = Some(Arc::new(HandleSearchState {
            search: id,
            phase: HandlePhase::Done,
            progress_pct: 100,
            total_handles: 18234,
            chunks: Arc::from([chunk]),
            searched: 412,
            total: 530,
            skipped: 118,
        }));
        d.header.version += 1;
    });
    draw(&mut search);
    assert!(search.read(|cx| view.read(cx).search_all_shown()));

    click(&mut search, "TlButton-HandleSearchAll");
    assert_eq!(
        starts_sent(&engine),
        1,
        "Search all sends StartHelperSession"
    );
    assert_eq!(searches(&engine).len(), 1, "waits for the session");

    publish_helper(cx, &engine, HelperState::Starting, None);
    publish_helper(cx, &engine, HelperState::Unlocked, None);
    let sent = searches(&engine);
    assert_eq!(sent.len(), 2);
    assert!(sent[1].1, "the second HandleSearch is privileged");
}

// ---- Unlock at launch ----

#[gpui_kit::test]
fn unlock_at_launch_starts_once(cx: &mut TestAppContext) {
    let mut settings = Settings::default();
    settings.privilege.unlock_at_launch = true;
    let (engine, mut main) = setup_with(cx, settings);
    assert_eq!(starts_sent(&engine), 1, "one start after the first frame");

    // Later frames and a reopened main window send no second start.
    main.update(|window, cx| window.simulate_next_frame(cx));
    draw(&mut main);
    main.update(|window, _| window.remove_window());
    main.run_until_parked();
    let handle = cx
        .update(|cx| open_main_window_on(Tab::Processes, cx))
        .expect("the main window opens again");
    let mut again = VisualTestContext::from_window(handle.into(), cx);
    draw(&mut again);
    again.update(|window, cx| window.simulate_next_frame(cx));
    draw(&mut again);
    assert_eq!(starts_sent(&engine), 1, "exactly one StartHelperSession");
}

/// Setup leaves the window active at the clock's start; moves the clock, then sends `mods`.
fn shift_after(main: &mut VisualTestContext, ms: u64, mods: Modifiers) {
    main.executor().advance_clock(Duration::from_millis(ms));
    main.simulate_modifiers_change(mods);
    draw(main);
}

#[gpui_kit::test]
fn shift_within_500ms_starts_once(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    assert_eq!(starts_sent(&engine), 0);
    shift_after(&mut main, 100, Modifiers::shift());
    assert_eq!(starts_sent(&engine), 1);
}

#[gpui_kit::test]
fn shift_after_500ms_ignored(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    shift_after(&mut main, 600, Modifiers::shift());
    assert_eq!(starts_sent(&engine), 0);
}

#[gpui_kit::test]
fn shift_twice_starts_once(cx: &mut TestAppContext) {
    let (engine, mut main) = setup(cx);
    shift_after(&mut main, 100, Modifiers::shift());
    shift_after(&mut main, 100, Modifiers::none());
    shift_after(&mut main, 100, Modifiers::shift());
    assert_eq!(starts_sent(&engine), 1);
}

#[gpui_kit::test]
fn shift_and_setting_start_once(cx: &mut TestAppContext) {
    let mut settings = Settings::default();
    settings.privilege.unlock_at_launch = true;
    let (engine, mut main) = setup_with(cx, settings);
    assert_eq!(starts_sent(&engine), 1);
    shift_after(&mut main, 100, Modifiers::shift());
    assert_eq!(starts_sent(&engine), 1);
}

// ---- Settings Privileges page ----

type SettingsFrame = TlSecondaryWindow<SettingsWindow>;

/// Opens the Settings window from the main window and returns its frame and a context on it.
fn open_settings(
    cx: &mut TestAppContext,
    main: &mut VisualTestContext,
) -> (Entity<SettingsFrame>, VisualTestContext) {
    main.update(|window, _| window.activate_window());
    main.dispatch_action(OpenSettings);
    main.run_until_parked();
    let handle = cx
        .update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::Settings)
        })
        .and_then(|h| h.downcast::<Root>())
        .expect("settings window registered");
    let frame = cx
        .update(|cx| SettingsFrame::find(handle, cx))
        .expect("settings frame");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    draw(&mut vcx);
    (frame, vcx)
}

fn privileges_page(cx: &mut TestAppContext) -> tl_ui::window::settings::PageBuilder {
    cx.update(all_pages)
        .into_iter()
        .find(|p| p.id() == PageId::Privileges)
        .expect("Privileges page listed")
}

#[gpui_kit::test]
fn privileges_unlock_at_launch_persists(cx: &mut TestAppContext) {
    let persists = Rc::new(Cell::new(0));
    setup_counting(cx, Settings::default(), persists.clone());
    let page = privileges_page(cx);
    assert!(!cx.update(|cx| TlSettings::settings(cx).privilege.unlock_at_launch));
    let before = persists.get();
    cx.update(|cx| page.set_bool(SettingKey::PrivilegeUnlockAtLaunch, true, cx));
    cx.run_until_parked();
    assert!(cx.update(|cx| TlSettings::settings(cx).privilege.unlock_at_launch));
    assert_eq!(persists.get(), before + 1, "persisted once");
}

#[gpui_kit::test]
fn privileges_additional_setup_opens_window(cx: &mut TestAppContext) {
    let (_engine, mut main) = setup(cx);
    let (_frame, mut vcx) = open_settings(cx, &mut main);
    let page = privileges_page(cx);
    vcx.update(|window, cx| page.press("Additional setup…", window, cx));
    vcx.run_until_parked();
    let open = cx.update(|cx| {
        cx.global::<GlobalRegistry>()
            .0
            .read(cx)
            .get(&WindowKey::AdditionalSetup)
            .is_some()
    });
    assert!(open, "the Additional Setup window is registered");
}

#[gpui_kit::test]
fn privileges_helper_status_line(cx: &mut TestAppContext) {
    const NOT_INSTALLED: &str = "Administrator helper: Not installed";
    const INSTALLED: &str = "Administrator helper: Installed";
    let (engine, mut main) = setup(cx);
    let (frame, mut vcx) = open_settings(cx, &mut main);
    publish_helper(cx, &engine, HelperState::Missing, None);
    let page = privileges_page(cx);
    assert!(vcx.update(|window, cx| page.render_custom(NOT_INSTALLED, window, cx).is_some()));
    assert!(vcx.update(|window, cx| page.render_custom(INSTALLED, window, cx).is_none()));

    let content = vcx.read(|cx| frame.read(cx).content().clone());
    let renders = vcx.read(|cx| content.read(cx).render_count());
    publish_helper(cx, &engine, HelperState::Locked, None);
    draw(&mut vcx);
    assert!(
        vcx.read(|cx| content.read(cx).render_count()) > renders,
        "a HEALTH snapshot re-renders the window"
    );
    let page = privileges_page(cx);
    assert!(vcx.update(|window, cx| page.render_custom(INSTALLED, window, cx).is_some()));
    assert!(vcx.update(|window, cx| page.render_custom(NOT_INSTALLED, window, cx).is_none()));
}
