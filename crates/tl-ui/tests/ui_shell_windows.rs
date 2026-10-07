//! Shell secondary windows: Share Theme (E21-030, UI-SHL-082), Collector Health (E21-007,
//! UI-SHL-099) and Keyboard Shortcuts (E21-016, UI-SHL-095). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, Context, Entity, Focusable as _, IntoElement, KeyDownEvent, KeyUpEvent,
    Keystroke, ParentElement as _, Render, Styled as _, TestAppContext, VisualContext as _,
    VisualTestContext, Window, WindowHandle, div,
};
use tl_model::client::EngineClient;
use tl_model::health::{SourceHealth, SourceId, SourceState};
use tl_model::settings::{Settings, UiState};
use tl_model::time::{TimeNs, now_boot_ns};
use tl_ui::actions::{OpenCollectorHealth, OpenShortcuts};
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind, has_modal_child};
use tl_ui::i18n::{set_test_catalogue, tr_calls};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::share_code::{decode_tl1, encode_tl1};
use tl_ui::theme::user_store::serialize_user_themes;
use tl_ui::theme::{ThemeDef, ThemeStore, builtin, current};
use tl_ui::window::collector_health::{CollectorHealthWindow, StateLook};
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::share_theme::{ShareTheme, ShareThemeView};
use tl_ui::window::shortcuts::{
    Heading, LISTED_CONTEXTS, ShortcutsWindow, build_rows, first_context,
};

struct Parent;

impl Render for Parent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("parent")
    }
}

type Frame = TlSecondaryWindow<ShareThemeView>;

/// The user theme "Mine": the first built-in with another name and accent.
fn mine() -> ThemeDef {
    let mut def = builtin::load()[0].clone();
    def.name = "Mine".into();
    def.accent = builtin::parse_hex("#12AB34").expect("hex");
    def
}

/// `TasklensUi::init` with "Mine" in themes.json.
fn setup(cx: &mut TestAppContext) {
    setup_engine(cx);
}

/// `setup`, returning the fake engine.
fn setup_engine(cx: &mut TestAppContext) -> Arc<FakeEngine> {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let init = UiInit {
        settings: Settings::default(),
        state: UiState::default(),
        themes_json: Some(serialize_user_themes(&[mine()]).into()),
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    engine
}

struct Opened {
    parent: AnyWindowHandle,
    handle: WindowHandle<Root>,
    frame: Entity<Frame>,
    vcx: VisualTestContext,
}

/// Opens a parent window and runs `ShareTheme { name: "Mine" }` from it.
fn share_mine(cx: &mut TestAppContext) -> Opened {
    setup(cx);
    let parent = cx.add_window(|_, _| Parent);
    let parent: AnyWindowHandle = parent.into();
    let mut pcx = VisualTestContext::from_window(parent, cx);
    pcx.update(|window, _| window.activate_window());
    pcx.dispatch_action(ShareTheme {
        name: "Mine".into(),
    });
    pcx.run_until_parked();
    let handle = cx
        .update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::ShareTheme)
        })
        .and_then(|h| h.downcast::<Root>())
        .expect("share window registered");
    let frame = cx
        .update(|cx| Frame::find(handle, cx))
        .expect("share frame");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    Opened {
        parent,
        handle,
        frame,
        vcx,
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn code(o: &mut Opened) -> String {
    let frame = o.frame.clone();
    o.vcx
        .read(|cx| frame.read(cx).content().read(cx).code().to_string())
}

fn clipboard(o: &mut Opened) -> Option<String> {
    o.vcx
        .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
}

#[gpui_kit::test]
fn share_window_code_roundtrip(cx: &mut TestAppContext) {
    let mut o = share_mine(cx);
    let frame = o.frame.clone();
    let (title, kind) = o
        .vcx
        .read(|cx| (frame.read(cx).title().to_string(), frame.read(cx).kind()));
    assert_eq!(title, "tasklens Share Theme");
    assert_eq!(kind, TlWindowKind::Confirmation, "a modal dialog window");
    let parent = o.parent;
    assert!(
        o.vcx.read(|cx| has_modal_child(parent, cx)),
        "modal to its parent"
    );

    let code = code(&mut o);
    assert!(code.starts_with("TL1."), "{code}");
    let decoded = decode_tl1(&code).expect("decodes");
    assert_eq!(&*decoded.name, "Mine");
    let stored = o.vcx.read(|cx| {
        cx.global::<ThemeStore>()
            .resolve("Mine")
            .map(|(def, builtin)| (encode_tl1(def), builtin))
    });
    assert_eq!(stored, Some((code.clone(), false)), "the user theme's code");
    assert_eq!(
        encode_tl1(&decoded),
        code,
        "decode then encode gives the code"
    );
    assert_eq!(encode_tl1(&mine()), code);
}

#[gpui_kit::test]
fn share_copy_toast(cx: &mut TestAppContext) {
    let mut o = share_mine(cx);
    let frame = o.frame.clone();
    let copy_focused = o
        .vcx
        .update(|window, cx| frame.read(cx).content().read(cx).copy_focused(window));
    assert!(copy_focused, "focus starts on Copy");

    o.vcx.simulate_keystrokes("enter");
    o.vcx.run_until_parked();
    draw(&mut o.vcx);
    let code = code(&mut o);
    assert_eq!(clipboard(&mut o), Some(code));
    let toasts = o
        .vcx
        .read(|cx| frame.read(cx).toasts().read(cx).visible_texts());
    assert_eq!(toasts, ["Theme code copied"], "toast in this window");
}

#[gpui_kit::test]
fn share_code_preselected(cx: &mut TestAppContext) {
    let mut o = share_mine(cx);
    let frame = o.frame.clone();
    let selected = o
        .vcx
        .read(|cx| frame.read(cx).content().read(cx).selected_text(cx));
    let code = code(&mut o);
    assert_eq!(selected, code, "the whole code is selected");

    o.vcx.simulate_keystrokes("ctrl-c");
    o.vcx.run_until_parked();
    assert_eq!(clipboard(&mut o), Some(code));
}

#[gpui_kit::test]
fn share_esc_closes(cx: &mut TestAppContext) {
    let mut o = share_mine(cx);
    // The frame entity held here would keep the modal mark on the parent.
    drop(o.frame);
    o.vcx.simulate_keystrokes("escape");
    o.vcx.run_until_parked();
    let id = o.handle.window_id();
    assert!(
        !cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id)),
        "Esc closes the window"
    );
    let parent = o.parent;
    assert!(
        !cx.update(|cx| has_modal_child(parent, cx)),
        "parent unblocked"
    );
}

/// The window resolves its words once per strings version: no tr call over 60 frames, and a new
/// catalogue reaches the drawn text (R-132).
#[gpui_kit::test]
fn share_words_follow_test_locale(cx: &mut TestAppContext) {
    const MARKERS: &str = "
shell-share-theme-text = <share-text>
shell-share-theme-copy = <copy>
shell-share-theme-close = <close>
shell-toast-theme-code-copied = <copied>
";
    let mut o = share_mine(cx);
    let frames = |cx: &mut VisualTestContext| {
        let before = tr_calls();
        for _ in 0..60 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        tr_calls() - before
    };
    assert_eq!(frames(&mut o.vcx), 0, "60 frames make no tr call");

    o.vcx.update(|_, cx| set_test_catalogue(cx, MARKERS));
    draw(&mut o.vcx);
    assert_eq!(
        frames(&mut o.vcx),
        0,
        "60 frames after the switch make no tr call"
    );
    o.vcx.simulate_keystrokes("enter");
    o.vcx.run_until_parked();
    let frame = o.frame.clone();
    let toasts = o
        .vcx
        .read(|cx| frame.read(cx).toasts().read(cx).visible_texts());
    assert_eq!(toasts, ["<copied>"]);
}

// ---- Collector Health (E21-007) ----

type HealthFrame = TlSecondaryWindow<CollectorHealthWindow>;

const SEC: TimeNs = 1_000_000_000;

/// Publishes a health snapshot with `sources` set and every other source Off.
fn publish_health(engine: &FakeEngine, sources: &[(SourceId, SourceHealth)]) {
    engine.publish_with(|s| {
        let mut health = (*s.health).clone();
        health.version += 1;
        for h in &mut health.sources {
            *h = SourceHealth::default();
        }
        for (id, h) in sources {
            health.sources[id.index()] = h.clone();
        }
        s.health = Arc::new(health);
    });
}

fn source(state: SourceState, age_s: Option<u64>, error: Option<&str>) -> SourceHealth {
    SourceHealth {
        state,
        last_ok_ns: age_s.map(|a| now_boot_ns() - a * SEC),
        last_error: error.map(Arc::from),
        restarts: 0,
    }
}

/// The registered Collector Health window, if open.
fn health_window(cx: &mut TestAppContext) -> Option<WindowHandle<Root>> {
    cx.update(|cx| {
        cx.global::<GlobalRegistry>()
            .0
            .read(cx)
            .get(&WindowKey::CollectorHealth)
    })
    .and_then(|h| h.downcast::<Root>())
}

/// Dispatches `OpenCollectorHealth` from a parent window `times` times and returns the window.
fn open_health(cx: &mut TestAppContext, times: usize) -> (Entity<HealthFrame>, VisualTestContext) {
    let parent: AnyWindowHandle = cx.add_window(|_, _| Parent).into();
    let mut pcx = VisualTestContext::from_window(parent, cx);
    for _ in 0..times {
        pcx.update(|window, _| window.activate_window());
        pcx.dispatch_action(OpenCollectorHealth);
        pcx.run_until_parked();
    }
    let handle = health_window(cx).expect("health window registered");
    let frame = cx
        .update(|cx| HealthFrame::find(handle, cx))
        .expect("health frame");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    (frame, vcx)
}

/// (source, state word, age, restarts, error) of every shown row.
type RowText = (SourceId, String, String, String, Option<String>);

fn health_rows(frame: &Entity<HealthFrame>, vcx: &mut VisualTestContext) -> Vec<RowText> {
    let frame = frame.clone();
    vcx.read(|cx| {
        let view = frame.read(cx).content().read(cx);
        view.rows()
            .map(|r| {
                (
                    r.source,
                    view.state_word(r).to_string(),
                    r.age.to_string(),
                    r.restarts_text.to_string(),
                    r.error.as_ref().map(|e| e.to_string()),
                )
            })
            .collect()
    })
}

fn health_ages(frame: &Entity<HealthFrame>, vcx: &mut VisualTestContext) -> Vec<String> {
    health_rows(frame, vcx).into_iter().map(|r| r.2).collect()
}

#[gpui_kit::test]
fn collector_health_refocus(cx: &mut TestAppContext) {
    setup(cx);
    let before = cx.update(|cx| cx.windows().len());
    let (frame, vcx) = open_health(cx, 2);
    let title = vcx.read(|cx| frame.read(cx).title().to_string());
    assert_eq!(title, "tasklens Collector Health");
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        before + 2,
        "the parent and one health window"
    );
    let id = health_window(cx).expect("open").window_id();
    assert_eq!(
        cx.update(|cx| cx.active_window().map(|w| w.window_id())),
        Some(id),
        "the health window has focus"
    );
}

#[gpui_kit::test]
fn collector_health_rows_and_colours(cx: &mut TestAppContext) {
    const ERROR: &str = "probe version mismatch";
    let engine = setup_engine(cx);
    publish_health(
        &engine,
        &[
            (SourceId::Disk, source(SourceState::Stale, Some(12), None)),
            (
                SourceId::GpuNvidia,
                source(SourceState::Failed, None, Some(ERROR)),
            ),
            (SourceId::Cpu, source(SourceState::Running, Some(1), None)),
            (
                SourceId::Memory,
                SourceHealth {
                    restarts: 5,
                    ..source(SourceState::Disabled { until: 0 }, Some(2), None)
                },
            ),
        ],
    );
    cx.run_until_parked();
    let (frame, mut vcx) = open_health(cx, 1);
    let rows = health_rows(&frame, &mut vcx);
    let ids: Vec<SourceId> = rows.iter().map(|r| r.0).collect();
    assert_eq!(
        ids,
        [
            SourceId::Cpu,
            SourceId::Memory,
            SourceId::Disk,
            SourceId::GpuNvidia
        ],
        "SourceId order, Off sources hidden"
    );
    let words: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
    assert_eq!(words, ["Running", "Disabled", "Stale", "Failed"]);
    let frame2 = frame.clone();
    let names: Vec<String> = vcx.read(|cx| {
        let view = frame2.read(cx).content().read(cx);
        view.rows()
            .map(|r| view.source_name(r).to_string())
            .collect()
    });
    assert_eq!(names, ["CPU", "Memory", "Disks", "GPU (NVIDIA)"]);
    assert_eq!(rows[1].3, "5", "restart count");
    assert_eq!(rows[0].3, "0");
    assert_eq!(rows[3].2, "\u{2026}", "no data yet: the WarmingUp glyph");
    assert_eq!(
        rows[3].4.as_deref(),
        Some(ERROR),
        "the full error text, for the tooltip"
    );

    let theme = vcx.read(|cx| current(cx).clone());
    assert_eq!(StateLook::Running.color(&theme), theme.status.success);
    assert_eq!(StateLook::Stale.color(&theme), theme.status.warning);
    assert_eq!(StateLook::Disabled.color(&theme), theme.status.warning);
    assert_eq!(StateLook::Failed.color(&theme), theme.status.danger);
}

#[gpui_kit::test]
fn collector_health_age_ticks(cx: &mut TestAppContext) {
    let engine = setup_engine(cx);
    publish_health(
        &engine,
        &[(SourceId::Cpu, source(SourceState::Running, Some(3), None))],
    );
    cx.run_until_parked();
    let (frame, mut vcx) = open_health(cx, 1);
    assert_eq!(health_ages(&frame, &mut vcx), ["3s ago"]);

    vcx.executor().advance_clock(Duration::from_secs(1));
    vcx.run_until_parked();
    assert_eq!(
        health_ages(&frame, &mut vcx),
        ["4s ago"],
        "the age moves with no new snapshot"
    );
}

#[gpui_kit::test]
fn collector_health_timer_stops_on_close(cx: &mut TestAppContext) {
    setup(cx);
    let (frame, mut vcx) = open_health(cx, 1);
    let view = vcx.read(|cx| frame.read(cx).content().downgrade());
    drop(frame);
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(health_window(cx).is_none(), "Esc closes the window");
    assert!(
        view.upgrade().is_none(),
        "the view, and the timer task it owns, are dropped"
    );
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
}

/// No tr call over 60 frames, and none after a catalogue switch (R-132).
#[gpui_kit::test]
fn collector_health_frames_make_no_tr_call(cx: &mut TestAppContext) {
    let engine = setup_engine(cx);
    publish_health(
        &engine,
        &[(SourceId::Cpu, source(SourceState::Running, Some(3), None))],
    );
    cx.run_until_parked();
    let (frame, mut vcx) = open_health(cx, 1);
    let frames = |cx: &mut VisualTestContext| {
        let before = tr_calls();
        for _ in 0..60 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        tr_calls() - before
    };
    assert_eq!(frames(&mut vcx), 0, "60 frames make no tr call");
    vcx.update(|_, cx| {
        set_test_catalogue(
            cx,
            "shell-health-state-running = <running>\n\
             shell-health-ago = <{ $age }>\n\
             shell-health-source-cpu = <cpu>\n",
        )
    });
    draw(&mut vcx);
    assert_eq!(
        frames(&mut vcx),
        0,
        "60 frames after the switch make no tr call"
    );
    let rows = health_rows(&frame, &mut vcx);
    assert_eq!(rows[0].1, "<running>");
    assert_eq!(rows[0].2, "<3s>", "the age goes in the template's place");
    let frame = frame.clone();
    let name = vcx.read(|cx| {
        let view = frame.read(cx).content().read(cx);
        view.rows().next().map(|r| view.source_name(r).to_string())
    });
    assert_eq!(name.as_deref(), Some("<cpu>"));
}

// ---- Share Theme: one Enter, one action (tasklens-ug9.35) ----

/// One Enter press as the platform sends it: key down, then key up while the window is open. A
/// focused button clicks on the key up, which `simulate_keystrokes` never sends.
fn press_enter(vcx: &mut VisualTestContext) {
    let keystroke = Keystroke::parse("enter").expect("keystroke");
    vcx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    let id = vcx.window_handle().window_id();
    if vcx.read(|cx| cx.windows().iter().any(|w| w.window_id() == id)) {
        vcx.simulate_event(KeyUpEvent { keystroke });
    }
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn share_enter_on_copy_copies_once(cx: &mut TestAppContext) {
    let mut o = share_mine(cx);
    press_enter(&mut o.vcx);
    draw(&mut o.vcx);
    let frame = o.frame.clone();
    let toasts = o
        .vcx
        .read(|cx| frame.read(cx).toasts().read(cx).visible_texts());
    // A second copy would show as "Theme code copied (×2)".
    assert_eq!(toasts, ["Theme code copied"], "one Enter, one copy");

    press_enter(&mut o.vcx);
    draw(&mut o.vcx);
    let toasts = o
        .vcx
        .read(|cx| frame.read(cx).toasts().read(cx).visible_texts());
    assert_eq!(
        toasts,
        ["Theme code copied (\u{d7}2)"],
        "a second Enter, a second copy"
    );
}

#[gpui_kit::test]
fn share_enter_on_close_closes_without_copy(cx: &mut TestAppContext) {
    let mut o = share_mine(cx);
    // Close is left of Copy in the footer.
    o.vcx.simulate_keystrokes("shift-tab");
    draw(&mut o.vcx);
    let frame = o.frame.clone();
    let copy_focused = o
        .vcx
        .update(|window, cx| frame.read(cx).content().read(cx).copy_focused(window));
    assert!(!copy_focused, "focus left Copy");
    drop(frame);
    drop(o.frame);
    press_enter(&mut o.vcx);
    let id = o.handle.window_id();
    assert!(
        !cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id)),
        "Enter on Close closes"
    );
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text())),
        None,
        "no copy"
    );
}

#[gpui_kit::test]
fn share_title_follows_strings_version(cx: &mut TestAppContext) {
    let mut o = share_mine(cx);
    o.vcx
        .update(|_, cx| set_test_catalogue(cx, "shell-share-theme-title = <share-title>\n"));
    o.vcx.run_until_parked();
    let frame = o.frame.clone();
    let title = o.vcx.read(|cx| frame.read(cx).title().to_string());
    assert_eq!(title, "<share-title>");
    let before = tr_calls();
    for _ in 0..10 {
        draw(&mut o.vcx);
    }
    assert_eq!(tr_calls() - before, 0, "frames make no tr call");
}

// ---- Keyboard Shortcuts (E21-016) ----

type ShortcutsFrame = TlSecondaryWindow<ShortcutsWindow>;

/// The registered Keyboard Shortcuts window, if open.
fn shortcuts_window(cx: &mut TestAppContext) -> Option<WindowHandle<Root>> {
    cx.update(|cx| {
        cx.global::<GlobalRegistry>()
            .0
            .read(cx)
            .get(&WindowKey::Shortcuts)
    })
    .and_then(|h| h.downcast::<Root>())
}

/// Dispatches `OpenShortcuts` from a parent window `times` times and returns the window.
fn open_shortcuts(
    cx: &mut TestAppContext,
    times: usize,
) -> (Entity<ShortcutsFrame>, VisualTestContext) {
    let parent: AnyWindowHandle = cx.add_window(|_, _| Parent).into();
    let mut pcx = VisualTestContext::from_window(parent, cx);
    for _ in 0..times {
        pcx.update(|window, _| window.activate_window());
        pcx.dispatch_action(OpenShortcuts);
        pcx.run_until_parked();
    }
    let handle = shortcuts_window(cx).expect("shortcuts window registered");
    let frame = cx
        .update(|cx| ShortcutsFrame::find(handle, cx))
        .expect("shortcuts frame");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    (frame, vcx)
}

/// (heading, description, key text) of every row that matches the search.
fn shortcut_rows(
    frame: &Entity<ShortcutsFrame>,
    vcx: &mut VisualTestContext,
) -> Vec<(Heading, String, String)> {
    let frame = frame.clone();
    vcx.read(|cx| {
        frame
            .read(cx)
            .content()
            .read(cx)
            .visible_rows()
            .map(|r| (r.heading, r.description.to_string(), r.key_text.to_string()))
            .collect()
    })
}

#[gpui_kit::test]
fn shortcuts_rows_from_keymap(cx: &mut TestAppContext) {
    setup(cx);
    let (frame, mut vcx) = open_shortcuts(cx, 1);
    let (title, kind) = vcx.read(|cx| (frame.read(cx).title().to_string(), frame.read(cx).kind()));
    assert_eq!(title, "tasklens Keyboard Shortcuts");
    assert_eq!(kind, TlWindowKind::Tool { resizable: true });
    let rows = shortcut_rows(&frame, &mut vcx);
    let row = rows
        .iter()
        .find(|r| r.1 == "Keyboard Shortcuts")
        .expect("Keyboard Shortcuts row");
    assert_eq!(row.0, Heading::Global);
    assert_eq!(row.2, "Ctrl+? or Ctrl+Shift+/");
    let next = rows
        .iter()
        .find(|r| r.1 == "Next / previous tab" && r.0 == Heading::Global)
        .expect("tab row");
    assert_eq!(next.2, "Ctrl+Tab or Ctrl+Shift+Tab");
    let pause = rows
        .iter()
        .find(|r| r.1 == "Pause display")
        .expect("static row");
    assert_eq!(pause.2, "Hold Ctrl");
    let headings = vcx.read(|cx| frame.read(cx).content().read(cx).visible_headings());
    let mut sorted = headings.clone();
    sorted.sort_by_key(|h| Heading::ALL.iter().position(|x| x == h));
    assert_eq!(headings, sorted, "headings in R-220 order");
    assert_eq!(headings[0], Heading::Global);
    assert_eq!(headings[1], Heading::Tables);
}

#[gpui_kit::test]
fn shortcuts_unbound_rows_absent(cx: &mut TestAppContext) {
    setup(cx);
    let (frame, mut vcx) = open_shortcuts(cx, 1);
    let bound = cx.update(|cx| {
        cx.key_bindings()
            .borrow()
            .bindings()
            .any(|b| b.action().name() == "tasklens::EndTask")
    });
    assert!(!bound, "End Task has no binding in this phase");
    let rows = shortcut_rows(&frame, &mut vcx);
    assert!(rows.iter().all(|r| r.1 != "End Task (SIGTERM)"));
    assert!(
        rows.iter().all(|r| r.1 != "Open Debug menu"),
        "the Debug menu row needs --debug"
    );
    let headings = vcx.read(|cx| frame.read(cx).content().read(cx).visible_headings());
    assert!(
        !headings.contains(&Heading::Services),
        "a heading with no bindings is not drawn"
    );
}

#[gpui_kit::test]
fn shortcuts_search_filters(cx: &mut TestAppContext) {
    setup(cx);
    let (frame, mut vcx) = open_shortcuts(cx, 1);
    let all = shortcut_rows(&frame, &mut vcx).len();
    vcx.simulate_input("FILTER");
    vcx.run_until_parked();
    draw(&mut vcx);
    let rows = shortcut_rows(&frame, &mut vcx);
    assert!(!rows.is_empty() && rows.len() < all);
    for r in &rows {
        assert!(
            r.1.to_lowercase().contains("filter") || r.2.to_lowercase().contains("filter"),
            "{r:?} does not match"
        );
    }
    assert!(rows.iter().any(|r| r.1 == "Focus filter"));
    assert!(rows.iter().any(|r| r.1 == "Start filtering"));
    let headings = vcx.read(|cx| frame.read(cx).content().read(cx).visible_headings());
    let mut expected: Vec<Heading> = Vec::new();
    for r in &rows {
        if expected.last() != Some(&r.0) {
            expected.push(r.0);
        }
    }
    assert_eq!(headings, expected, "only headings with matching rows");
    assert!(!headings.contains(&Heading::Performance));

    // Key text matches too.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(
        shortcuts_window(cx).is_some(),
        "the first Esc clears the search"
    );
    vcx.simulate_input("shift+/");
    vcx.run_until_parked();
    let rows = shortcut_rows(&frame, &mut vcx);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "Keyboard Shortcuts");
}

#[gpui_kit::test]
fn shortcuts_refocus(cx: &mut TestAppContext) {
    setup(cx);
    let before = cx.update(|cx| cx.windows().len());
    let (_frame, _vcx) = open_shortcuts(cx, 2);
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        before + 2,
        "the parent and one shortcuts window"
    );
    let id = shortcuts_window(cx).expect("open").window_id();
    assert_eq!(
        cx.update(|cx| cx.active_window().map(|w| w.window_id())),
        Some(id),
        "the shortcuts window has focus"
    );
}

#[gpui_kit::test]
fn shortcuts_esc_and_ctrl_w_close(cx: &mut TestAppContext) {
    setup(cx);
    let (frame, mut vcx) = open_shortcuts(cx, 1);
    let focused = vcx.update(|window, cx| {
        let search = frame.read(cx).content().read(cx).search().clone();
        search.read(cx).focus_handle(cx).is_focused(window)
    });
    assert!(focused, "the search box has focus on open");
    drop(frame);
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(shortcuts_window(cx).is_none(), "Esc closes the window");

    let (frame, mut vcx) = open_shortcuts(cx, 1);
    drop(frame);
    vcx.simulate_keystrokes("ctrl-w");
    vcx.run_until_parked();
    assert!(shortcuts_window(cx).is_none(), "Ctrl+W closes the window");
}

/// Bindings in the listed contexts that canonical table (b) has no row for. They have no row
/// until table (b) gets one.
const NOT_IN_TABLE_B: &[(&str, &str)] = &[];

#[gpui_kit::test]
fn shortcuts_cover_all_bindings(cx: &mut TestAppContext) {
    setup(cx);
    cx.update(|cx| {
        let rows = build_rows(cx, true);
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let mut checked = 0;
        for (ix, b) in keymap.bindings().enumerate() {
            let Some(predicate) = b.predicate().map(|p| p.to_string()) else {
                continue;
            };
            let ctx = first_context(&predicate);
            if !LISTED_CONTEXTS.iter().any(|(c, _)| *c == ctx) {
                continue;
            }
            let name = b.action().name();
            let shown = rows
                .iter()
                .filter(|r| r.binding_indices().contains(&ix))
                .count();
            let stroke = b.keystrokes()[0].inner().unparse();
            if NOT_IN_TABLE_B.contains(&(predicate.as_str(), name)) {
                assert_eq!(shown, 0, "{predicate} {stroke} {name}");
                continue;
            }
            assert_eq!(
                shown, 1,
                "{predicate} {stroke} {name}: add a SHORTCUT_DOCS entry"
            );
            checked += 1;
        }
        assert!(checked > 60, "{checked} bindings checked");
    });
}

#[gpui_kit::test]
fn shortcuts_dialogs_and_windows_rows(cx: &mut TestAppContext) {
    setup(cx);
    cx.update(|cx| {
        let rows = build_rows(cx, false);
        let keys = |text: &str| {
            let row = rows
                .iter()
                .find(|r| r.description.as_ref() == text)
                .unwrap_or_else(|| panic!("{text} row"));
            (row.heading, row.key_text.to_string())
        };
        assert_eq!(
            keys("Move device up / down"),
            (Heading::Performance, "Alt+Up or Alt+Down".into())
        );
        for (text, key_text) in [
            ("Close the Properties window", "Ctrl+W or Esc"),
            ("Open the field menu", "Shift+F10 or Menu"),
            ("Toggle the CPU group", "Shift+Space"),
            ("Apply affinity (OK)", "Enter"),
            ("Close the Column Picker", "Esc"),
        ] {
            assert_eq!(
                keys(text),
                (Heading::DialogsAndWindows, key_text.into()),
                "{text}"
            );
        }
        let last = rows.last().expect("rows");
        assert_eq!(last.heading, Heading::DialogsAndWindows, "the last heading");
    });
}

// ---- About (E21-015) ----

use tl_model::actions::ActionKind;
use tl_model::command::Command;
use tl_ui::app::BuildInfo;
use tl_ui::window::about::{
    AboutBack, AboutView, AboutWindow, CopyVersionInfo, OpenAbout, OpenLogFolder, ShowLicenses,
    file_uri, parse_licenses,
};

type AboutFrame = TlSecondaryWindow<AboutWindow>;

const LICENSES_JSON: &str = r#"{
  "overview": [],
  "licenses": [
    { "name": "MIT License", "id": "MIT", "text": "...",
      "used_by": [
        { "crate": { "name": "smallvec", "version": "1.15.1", "license": "MIT OR Apache-2.0" } },
        { "crate": { "name": "anyhow", "version": "1.0.98", "license": null } }
      ] },
    { "name": "Apache License 2.0", "id": "Apache-2.0", "text": "...",
      "used_by": [
        { "crate": { "name": "smallvec", "version": "1.15.1", "license": "MIT OR Apache-2.0" } }
      ] }
  ]
}"#;

/// `setup_engine` with build info 0.1.0, a1b2c3d, 2026-10-03 and a logs directory.
fn setup_about(cx: &mut TestAppContext) -> Arc<FakeEngine> {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let mut env = AppEnv::test();
    env.build = BuildInfo {
        version: "0.1.0",
        git_hash: "a1b2c3d",
        build_date: "2026-10-03",
        gpui_rev: "0123456789ab",
        licenses_json: LICENSES_JSON,
    };
    env.logs_dir = "/home/u/.local/state/tasklens/logs".into();
    let init = UiInit {
        settings: Settings::default(),
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env,
    };
    cx.update(|cx| {
        TasklensUi::init(cx, client, init);
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    engine
}

/// The registered About window, if open.
fn about_window(cx: &mut TestAppContext) -> Option<WindowHandle<Root>> {
    cx.update(|cx| {
        cx.global::<GlobalRegistry>()
            .0
            .read(cx)
            .get(&WindowKey::About)
    })
    .and_then(|h| h.downcast::<Root>())
}

/// Dispatches `OpenAbout` from a parent window `times` times and returns the window.
fn open_about(cx: &mut TestAppContext, times: usize) -> (Entity<AboutFrame>, VisualTestContext) {
    let parent: AnyWindowHandle = cx.add_window(|_, _| Parent).into();
    let mut pcx = VisualTestContext::from_window(parent, cx);
    for _ in 0..times {
        pcx.update(|window, _| window.activate_window());
        pcx.dispatch_action(OpenAbout);
        pcx.run_until_parked();
    }
    let handle = about_window(cx).expect("about window registered");
    let frame = cx
        .update(|cx| AboutFrame::find(handle, cx))
        .expect("about frame");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    (frame, vcx)
}

#[gpui_kit::test]
fn secondary_window_refocus(cx: &mut TestAppContext) {
    setup_about(cx);
    let before = cx.update(|cx| cx.windows().len());
    let (frame, vcx) = open_about(cx, 2);
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        before + 2,
        "the parent and one About window"
    );
    let id = about_window(cx).expect("open").window_id();
    assert_eq!(
        cx.update(|cx| cx.active_window().map(|w| w.window_id())),
        Some(id),
        "the About window has focus"
    );
    let (title, kind) = vcx.read(|cx| (frame.read(cx).title().to_string(), frame.read(cx).kind()));
    assert_eq!(title, "tasklens About");
    assert_eq!(kind, TlWindowKind::Tool { resizable: false });
}

#[gpui_kit::test]
fn about_version_line(cx: &mut TestAppContext) {
    setup_about(cx);
    let (frame, vcx) = open_about(cx, 1);
    let line = vcx.read(|cx| frame.read(cx).content().read(cx).version_line().to_string());
    assert_eq!(line, "Version: 0.1.0 (a1b2c3d, 2026-10-03)");
}

#[gpui_kit::test]
fn about_copy_version_info(cx: &mut TestAppContext) {
    setup_about(cx);
    let (frame, mut vcx) = open_about(cx, 1);
    vcx.dispatch_action(CopyVersionInfo);
    vcx.run_until_parked();
    draw(&mut vcx);
    let text = vcx
        .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
        .expect("clipboard text");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 9, "{text}");
    assert_eq!(lines[0], "Version: 0.1.0");
    assert_eq!(lines[1], "Git hash: a1b2c3d");
    assert_eq!(lines[2], "Build date: 2026-10-03");
    assert_eq!(lines[3], "GPUI revision: 0123456789ab");
    let labels: Vec<&str> = lines
        .iter()
        .map(|l| l.split_once(": ").expect("Label: value").0)
        .collect();
    assert_eq!(
        labels,
        [
            "Version",
            "Git hash",
            "Build date",
            "GPUI revision",
            "Kernel",
            "NVIDIA driver",
            "Compositor",
            "Session type",
            "Scale",
        ]
    );
    assert_eq!(
        lines[5], "NVIDIA driver: Unknown",
        "no graphics rows in the snapshot"
    );
    assert!(lines[8].ends_with('%'), "{}", lines[8]);
    assert!(!text.contains("/home/"), "no paths in the block");
    let toasts = vcx.read(|cx| frame.read(cx).toasts().read(cx).visible_texts());
    assert_eq!(toasts, ["Copied to clipboard"]);
}

#[gpui_kit::test]
fn about_licenses_back(cx: &mut TestAppContext) {
    setup_about(cx);
    let (frame, mut vcx) = open_about(cx, 1);
    let view =
        |vcx: &mut VisualTestContext| vcx.read(|cx| frame.read(cx).content().read(cx).view());
    assert_eq!(view(&mut vcx), AboutView::Main);
    assert!(
        vcx.read(|cx| frame.read(cx).content().read(cx).license_rows().is_none()),
        "the list is built on first open"
    );
    vcx.dispatch_action(ShowLicenses);
    vcx.run_until_parked();
    draw(&mut vcx);
    assert_eq!(view(&mut vcx), AboutView::Licenses);
    let rows = vcx.read(|cx| {
        frame
            .read(cx)
            .content()
            .read(cx)
            .license_rows()
            .expect("rows")
            .iter()
            .map(|r| {
                (
                    r.name.to_string(),
                    r.version.to_string(),
                    r.license.to_string(),
                )
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        rows,
        [
            ("anyhow".into(), "1.0.98".into(), "MIT".into()),
            (
                "smallvec".into(),
                "1.15.1".into(),
                "MIT OR Apache-2.0".into()
            ),
        ],
        "one row per crate, sorted by name"
    );
    vcx.dispatch_action(AboutBack);
    vcx.run_until_parked();
    draw(&mut vcx);
    assert_eq!(view(&mut vcx), AboutView::Main);
    assert!(parse_licenses("not json").is_empty());
}

#[gpui_kit::test]
fn about_open_log_folder(cx: &mut TestAppContext) {
    let engine = setup_about(cx);
    let (_frame, mut vcx) = open_about(cx, 1);
    let before = engine.commands().len();
    vcx.dispatch_action(OpenLogFolder);
    vcx.run_until_parked();
    let commands = engine.commands();
    let uris: Vec<String> = commands[before..]
        .iter()
        .filter_map(|c| match c {
            Command::Action(req) => match &req.kind {
                ActionKind::OpenUri(uri) => Some(uri.to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(uris.len(), 1, "{uris:?}");
    assert_eq!(uris[0], "file:///home/u/.local/state/tasklens/logs");
    assert!(uris[0].ends_with("/tasklens/logs"));
    assert_eq!(
        file_uri(std::path::Path::new("/a b/ø")),
        "file:///a%20b/%C3%B8"
    );
}

#[gpui_kit::test]
fn about_no_foreign_branding(cx: &mut TestAppContext) {
    setup_about(cx);
    let (frame, mut vcx) = open_about(cx, 1);
    let check = |vcx: &mut VisualTestContext| {
        let (title, texts) = vcx.read(|cx| {
            let f = frame.read(cx);
            (f.title().to_string(), f.content().read(cx).drawn_text())
        });
        assert!(texts.len() > 1);
        for t in std::iter::once(title.as_str()).chain(texts.iter().map(|t| t.as_ref())) {
            assert!(
                !t.contains("TaskSlinger") && !t.contains("Mission Center"),
                "{t}"
            );
        }
    };
    check(&mut vcx);
    vcx.dispatch_action(ShowLicenses);
    vcx.run_until_parked();
    draw(&mut vcx);
    check(&mut vcx);
}
