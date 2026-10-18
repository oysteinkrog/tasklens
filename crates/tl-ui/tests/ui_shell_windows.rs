//! Shell secondary windows: Share Theme (E21-030, UI-SHL-082), Collector Health (E21-007,
//! UI-SHL-099) and Keyboard Shortcuts (E21-016, UI-SHL-095). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, Entity, Focusable as _, IntoElement, KeyDownEvent,
    KeyUpEvent, Keystroke, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualContext as _, VisualTestContext, Window, WindowHandle, div,
};
use tl_model::client::EngineClient;
use tl_model::health::{SourceHealth, SourceId, SourceState};
use tl_model::settings::{Settings, UiState};
use tl_model::time::{TimeNs, now_boot_ns};
use tl_ui::actions::{OpenCollectorHealth, OpenShortcuts};
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::toast::ToastQueue;
use tl_ui::components::tooltip::DELAY_CONTROL;
use tl_ui::components::window_frame::{
    CloseSecondaryWindow, TlSecondaryWindow, TlWindowKind, has_modal_child,
};
use tl_ui::i18n::{set_test_catalogue, tr_calls};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::share_code::{decode_tl1, encode_tl1};
use tl_ui::theme::user_store::serialize_user_themes;
use tl_ui::theme::{ThemeDef, ThemeStore, builtin, current, metrics};
use tl_ui::window::collector_health::{CollectorHealthWindow, StateLook};
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::share_theme::{ShareTheme, ShareThemeView};
use tl_ui::window::shortcuts::{
    Heading, LISTED_CONTEXTS, ShortcutsWindow, build_rows, first_context,
};
use tl_ui::window::theme_editor::{EditTheme, ThemeEditor, ThemeEditorDone};

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
    setup_themes(cx, &[mine()])
}

/// `setup` with `themes` in themes.json.
fn setup_themes(cx: &mut TestAppContext, themes: &[ThemeDef]) -> Arc<FakeEngine> {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
    let init = UiInit {
        settings: Settings::default(),
        state: UiState::default(),
        themes_json: Some(serialize_user_themes(themes).into()),
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
    assert_eq!(row.2, "Ctrl+Shift+/ or Ctrl+?");
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
    assert!(bound, "End Task is bound on the Processes page");
    let rows = shortcut_rows(&frame, &mut vcx);
    assert!(rows.iter().any(|r| r.1 == "End Task (SIGTERM)"));
    assert!(
        rows.iter().all(|r| r.1 != "Open Debug menu"),
        "the Debug menu row needs --debug"
    );
    let headings = vcx.read(|cx| frame.read(cx).content().read(cx).visible_headings());
    assert!(
        !headings.contains(&Heading::Connections),
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
fn shortcuts_second_esc_closes_from_search(cx: &mut TestAppContext) {
    setup(cx);
    let (frame, mut vcx) = open_shortcuts(cx, 1);
    drop(frame);
    vcx.simulate_input("filter");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(shortcuts_window(cx).is_some(), "the first Esc clears");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(
        shortcuts_window(cx).is_none(),
        "the second Esc closes the window with focus still in the search box"
    );
}

/// Hovers the centre of `selector`, coming from outside the window, and waits out the tooltip
/// delay. True when a tooltip shows.
fn hover_shows_tooltip(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    let at = vcx.debug_bounds(selector).expect("drawn");
    let none = gpui_kit::Modifiers::none();
    vcx.simulate_mouse_move(
        gpui_kit::point(gpui_kit::px(2.), gpui_kit::px(2.)),
        None,
        none,
    );
    vcx.executor().advance_clock(DELAY_CONTROL);
    vcx.run_until_parked();
    draw(vcx);
    vcx.simulate_mouse_move(at.center(), None, none);
    vcx.executor().advance_clock(DELAY_CONTROL);
    vcx.run_until_parked();
    draw(vcx);
    vcx.debug_bounds("TlTooltip").is_some()
}

#[gpui_kit::test]
fn shortcuts_keys_column_max_width_tooltip_when_clipped(cx: &mut TestAppContext) {
    setup(cx);
    let (frame, mut vcx) = open_shortcuts(cx, 1);
    // The first frame measures the keys; the next one attaches the tooltips.
    vcx.run_until_parked();
    draw(&mut vcx);
    let rows = shortcut_rows(&frame, &mut vcx);
    let long = rows
        .iter()
        .position(|r| r.0 == Heading::Global && r.2.starts_with("Ctrl+1 or Alt+1"))
        .expect("tab number row");
    let short = rows.iter().position(|r| r.2 == "Ctrl+N").expect("Run row");
    let selector =
        |kind: &str, ix: usize| -> &'static str { format!("Shortcuts-{kind}-{ix}").leak() };
    let scale = vcx.update(|_, cx| metrics(cx).ui_scale);

    let keys = vcx
        .debug_bounds(selector("keys", long))
        .expect("keys drawn");
    assert!(
        keys.size.width <= gpui_kit::px(220. * scale + 0.5),
        "the keys column is at most 220 px wide, got {:?}",
        keys.size.width
    );
    let desc = vcx
        .debug_bounds(selector("desc", long))
        .expect("description drawn");
    assert!(
        desc.size.width >= gpui_kit::px(100. * scale),
        "the description keeps its room next to a long key list, got {:?}",
        desc.size.width
    );
    assert!(
        hover_shows_tooltip(&mut vcx, selector("keys", long)),
        "clipped keys show the full key text in a tooltip"
    );
    assert!(
        !hover_shows_tooltip(&mut vcx, selector("keys", short)),
        "keys that fit get no tooltip"
    );
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
        let mut missing = Vec::new();
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
            if shown != 1 {
                missing.push(format!(
                    "{predicate} {stroke} {name}: shown {shown} times, need a SHORTCUT_DOCS entry"
                ));
                continue;
            }
            checked += 1;
        }
        assert!(missing.is_empty(), "{}", missing.join("\n"));
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
    let (frame, mut vcx) = open_about(cx, 1);
    let before = engine.commands().len();
    vcx.dispatch_action(OpenLogFolder);
    vcx.run_until_parked();
    draw(&mut vcx);
    let toasts = vcx.read(|cx| frame.read(cx).toasts().read(cx).visible_texts());
    assert!(toasts.is_empty(), "no toast while opening: {toasts:?}");
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

// ---- Reset settings confirmation (E21-005, tasklens-bj6.3) ----

mod reset_settings {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, Modifiers,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window,
        WindowHandle, div, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::settings::{Settings, UiState};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::dialog::{DialogButton, TlDialogView};
    use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::theme::ThemeStore;
    use tl_ui::window::confirm::ResetSettings;

    struct Body;

    impl Render for Body {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child("parent")
        }
    }

    type ParentFrame = TlSecondaryWindow<Body>;
    type Saves = Rc<RefCell<Vec<(Settings, UiState)>>>;

    struct Rig {
        engine: Arc<FakeEngine>,
        saves: Saves,
        parent: Entity<ParentFrame>,
        pcx: VisualTestContext,
    }

    /// Settings with scale 150, the light theme and a 2 s refresh; state with a saved column
    /// width and window size.
    fn changed() -> (Settings, UiState) {
        let mut settings = Settings::default();
        settings.appearance.ui_scale_pct = 150;
        settings.theme.active = "Default Light".to_owned();
        settings.processes.interval_ms = 2000;
        let mut state = UiState::default();
        state.window.width = 1234;
        state.window.height = 777;
        state
            .columns
            .processes
            .widths
            .insert("name".to_owned(), 321);
        (settings, state)
    }

    /// `TasklensUi::init` with `changed()` and a settings write that works when `write_ok`,
    /// then a parent window to open the confirmation from.
    fn rig(cx: &mut TestAppContext, write_ok: bool) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let (settings, state) = changed();
        let saves: Saves = Rc::default();
        let sink = saves.clone();
        let mut env = AppEnv::test();
        if !write_ok {
            env.write_settings_now = Arc::new(|_| Err("disk full".to_owned()));
        }
        let init = UiInit {
            settings,
            state,
            themes_json: None,
            persist: Box::new(move |s, st| sink.borrow_mut().push((s.clone(), st.clone()))),
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
        let spec = TlWindowSpec::new(
            TlWindowKind::Tool { resizable: true },
            "parent",
            size(px(600.), px(400.)),
        );
        let handle = cx
            .update(|cx| ParentFrame::open(cx, spec, |_, cx| cx.new(|_| Body)))
            .expect("parent opens");
        let parent = cx
            .update(|cx| ParentFrame::find(handle, cx))
            .expect("parent frame");
        let mut pcx = VisualTestContext::from_window(handle.into(), cx);
        pcx.update(|window, _| window.activate_window());
        pcx.run_until_parked();
        Rig {
            engine,
            saves,
            parent,
            pcx,
        }
    }

    /// Dispatches `ResetSettings` from the parent and returns the confirmation window.
    fn open_confirm(
        r: &mut Rig,
        cx: &mut TestAppContext,
    ) -> (WindowHandle<Root>, Entity<TlDialogView>, VisualTestContext) {
        let parent_id = r.pcx.update(|window, _| window.window_handle().window_id());
        r.pcx.dispatch_action(ResetSettings);
        r.pcx.run_until_parked();
        let handle = cx
            .update(|cx| {
                cx.windows()
                    .into_iter()
                    .find(|w| w.window_id() != parent_id)
            })
            .and_then(|w: AnyWindowHandle| w.downcast::<Root>())
            .expect("confirmation window");
        let view = cx
            .update(|cx| TlSecondaryWindow::<TlDialogView>::find(handle, cx))
            .map(|frame| cx.read(|cx| frame.read(cx).content().clone()))
            .expect("a TlDialog");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        (handle, view, vcx)
    }

    fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
        let id = handle.window_id();
        cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
    }

    fn settings(cx: &mut TestAppContext) -> Settings {
        cx.update(|cx| TlSettings::settings(cx).clone())
    }

    fn toasts(r: &mut Rig) -> Vec<String> {
        let parent = r.parent.clone();
        r.pcx.read(|cx| {
            parent
                .read(cx)
                .toasts()
                .read(cx)
                .visible_texts()
                .iter()
                .map(|t| t.to_string())
                .collect()
        })
    }

    /// Clicks Reset in the confirmation.
    fn click_reset(vcx: &mut VisualTestContext) {
        let at = vcx.debug_bounds("TlDialogAction").expect("Reset drawn");
        vcx.simulate_click(at.center(), Modifiers::none());
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn reset_confirm_focuses_cancel(cx: &mut TestAppContext) {
        let mut r = rig(cx, true);
        let before = settings(cx);
        let (handle, view, mut vcx) = open_confirm(&mut r, cx);
        let (title, text, destructive) = vcx.read(|cx| {
            let d = view.read(cx).dialog();
            (
                d.window_title().to_string(),
                d.text().to_string(),
                d.is_destructive(),
            )
        });
        assert_eq!(title, "tasklens");
        assert_eq!(
            text,
            "Reset all settings to their defaults? Themes and history are kept."
        );
        assert!(destructive, "Danger Reset");
        let cancel = vcx.read(|cx| view.read(cx).dialog().cancel_text().to_string());
        assert_eq!(cancel, "Cancel");
        let focused = vcx.update(|window, cx| view.read(cx).focused_button(window));
        assert_eq!(focused, Some(DialogButton::Cancel));

        drop(view);
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(!is_open(cx, handle), "Esc closes it");
        assert_eq!(settings(cx), before, "no setting changes");
        assert!(r.saves.borrow().is_empty());
    }

    /// The Cancel label comes from `shell-confirm-cancel`, not the dialog's English default.
    #[gpui_kit::test]
    fn reset_cancel_label_translated(cx: &mut TestAppContext) {
        let mut r = rig(cx, true);
        r.pcx.update(|_, cx| {
            tl_ui::i18n::set_test_catalogue(cx, "shell-confirm-cancel = <cancel>\n")
        });
        let (_, view, vcx) = open_confirm(&mut r, cx);
        let cancel = vcx.read(|cx| view.read(cx).dialog().cancel_text().to_string());
        assert_eq!(cancel, "<cancel>");
    }

    #[gpui_kit::test]
    fn reset_enter_on_cancel_does_nothing(cx: &mut TestAppContext) {
        let mut r = rig(cx, true);
        let before = settings(cx);
        let (handle, view, mut vcx) = open_confirm(&mut r, cx);
        drop(view);
        super::press_enter(&mut vcx);
        assert!(!is_open(cx, handle), "Enter on Cancel closes it");
        assert_eq!(settings(cx), before, "no setting changes");
        assert!(r.saves.borrow().is_empty());
        assert!(toasts(&mut r).is_empty());
    }

    #[gpui_kit::test]
    fn reset_settings_keeps_state(cx: &mut TestAppContext) {
        let mut r = rig(cx, true);
        let (_, state) = changed();
        let (handle, view, mut vcx) = open_confirm(&mut r, cx);
        drop(view);
        click_reset(&mut vcx);
        assert!(!is_open(cx, handle));
        let now = settings(cx);
        assert_eq!(now.appearance.ui_scale_pct, 100);
        assert_eq!(now, Settings::default());
        let kept = cx.update(|cx| TlSettings::state(cx).clone());
        assert_eq!(kept.window.width, state.window.width);
        assert_eq!(kept.window.height, state.window.height);
        assert_eq!(kept.columns, state.columns);
        let saves = r.saves.borrow().clone();
        let (saved, saved_state) = saves.last().expect("persisted");
        assert_eq!(*saved, Settings::default());
        assert_eq!(saved_state.columns, state.columns);
        assert_eq!(toasts(&mut r), ["Settings reset to defaults"]);
    }

    #[gpui_kit::test]
    fn reset_reapplies_theme(cx: &mut TestAppContext) {
        let mut r = rig(cx, true);
        let active = |cx: &mut TestAppContext| {
            cx.update(|cx| cx.global::<ThemeStore>().active.name.to_string())
        };
        assert_eq!(active(cx), "Default Light");
        let (_, view, mut vcx) = open_confirm(&mut r, cx);
        drop(view);
        click_reset(&mut vcx);
        assert_eq!(active(cx), "Default Dark");
    }

    #[gpui_kit::test]
    fn reset_sends_engine_settings(cx: &mut TestAppContext) {
        let mut r = rig(cx, true);
        let (_, view, mut vcx) = open_confirm(&mut r, cx);
        drop(view);
        click_reset(&mut vcx);
        let sent = r.engine.settings();
        assert_eq!(sent.last().map(|s| s.interval_ms), Some(1000));
    }

    #[gpui_kit::test]
    fn reset_write_failure_changes_nothing(cx: &mut TestAppContext) {
        let mut r = rig(cx, false);
        let before = settings(cx);
        let (_, view, mut vcx) = open_confirm(&mut r, cx);
        drop(view);
        click_reset(&mut vcx);
        assert_eq!(settings(cx), before);
        assert!(r.saves.borrow().is_empty());
        assert_eq!(toasts(&mut r), ["Failed to reset settings"]);
    }
}

// ---- Crash Report window (tasklens-bj6.8, UI-SHL-093) ----

mod crash_report {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::SystemTime;

    use gpui_kit::base::Root;
    use gpui_kit::{
        AnyWindowHandle, Context, Entity, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window,
        WindowHandle, WindowVisibility, div,
    };
    use tl_model::actions::ActionKind;
    use tl_model::client::EngineClient;
    use tl_model::command::Command;
    use tl_model::settings::{Settings, UiState};
    use tl_ui::app::{AppEnv, CrashReport, TasklensUi, UiInit};
    use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind};
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::crash_report::{
        CopyCrashReport, CrashReportWindow, OpenCrashLogFolder, show_pending,
    };
    use tl_ui::window::registry::{GlobalRegistry, WindowKey};

    struct Main;

    impl Render for Main {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child("main")
        }
    }

    type Frame = TlSecondaryWindow<CrashReportWindow>;

    struct Rig {
        engine: Arc<FakeEngine>,
        /// How many times `mark_crash_seen` ran.
        seen: Arc<AtomicU32>,
        main: VisualTestContext,
    }

    /// `TasklensUi::init` with a pending report "panic at x", a counting `mark_crash_seen`, a
    /// logs directory, and a main window.
    fn rig(cx: &mut TestAppContext) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let seen = Arc::new(AtomicU32::new(0));
        let count = seen.clone();
        let mut env = AppEnv::test();
        env.pending_crash = Some(CrashReport {
            path: "/home/u/.local/state/tasklens/crash/crash-1.txt".into(),
            text: "panic at x".into(),
            at: SystemTime::UNIX_EPOCH,
        });
        env.mark_crash_seen = Arc::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
        });
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
        let main: AnyWindowHandle = cx.add_window(|_, _| Main).into();
        let main = VisualTestContext::from_window(main, cx);
        Rig { engine, seen, main }
    }

    fn crash_window(cx: &mut TestAppContext) -> Option<WindowHandle<Root>> {
        cx.update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::CrashReport)
        })
        .and_then(|h| h.downcast::<Root>())
    }

    fn show(rig: &mut Rig) {
        rig.main.update(show_pending);
        rig.main.run_until_parked();
    }

    /// Shows the report and returns its frame and window context.
    fn opened(rig: &mut Rig, cx: &mut TestAppContext) -> (Entity<Frame>, VisualTestContext) {
        show(rig);
        let handle = crash_window(cx).expect("crash report window registered");
        let frame = cx
            .update(|cx| Frame::find(handle, cx))
            .expect("crash report frame");
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
    fn crash_report_shown_once(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let before = cx.update(|cx| cx.windows().len());
        let (frame, vcx) = opened(&mut rig, cx);
        assert_eq!(cx.update(|cx| cx.windows().len()), before + 1);
        assert_eq!(rig.seen.load(Ordering::Relaxed), 1, "marked seen once");
        let (title, kind, text) = vcx.read(|cx| {
            let f = frame.read(cx);
            (
                f.title().to_string(),
                f.kind(),
                f.content().read(cx).report_text().to_owned(),
            )
        });
        assert_eq!(title, "tasklens Crash Report");
        assert_eq!(kind, TlWindowKind::Tool { resizable: false });
        assert_eq!(text, "panic at x");

        show(&mut rig);
        assert_eq!(
            cx.update(|cx| cx.windows().len()),
            before + 1,
            "no second window"
        );
        assert_eq!(rig.seen.load(Ordering::Relaxed), 1);
    }

    #[gpui_kit::test]
    fn crash_report_deferred_while_hidden(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.main
            .simulate_visibility_change(WindowVisibility::Hidden);
        rig.main.run_until_parked();
        show(&mut rig);
        assert!(crash_window(cx).is_none(), "no window while hidden");
        assert_eq!(
            rig.seen.load(Ordering::Relaxed),
            0,
            "the report stays pending"
        );

        // Shown again from the tray: the report opens then.
        rig.main
            .simulate_visibility_change(WindowVisibility::Visible);
        rig.main.run_until_parked();
        assert!(
            crash_window(cx).is_some(),
            "opens once the main window shows"
        );
        assert_eq!(rig.seen.load(Ordering::Relaxed), 1);
    }

    #[gpui_kit::test]
    fn crash_report_copy(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let (frame, mut vcx) = opened(&mut rig, cx);
        vcx.dispatch_action(CopyCrashReport);
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let clip = vcx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
        assert_eq!(clip.as_deref(), Some("panic at x"));
        let toasts = vcx.read(|cx| frame.read(cx).toasts().read(cx).visible_texts());
        assert_eq!(toasts, ["Copied to clipboard"]);
    }

    #[gpui_kit::test]
    fn crash_report_open_logs(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let (_, mut vcx) = opened(&mut rig, cx);
        let before = rig.engine.commands().len();
        vcx.dispatch_action(OpenCrashLogFolder);
        vcx.run_until_parked();
        let uris: Vec<String> = rig.engine.commands()[before..]
            .iter()
            .filter_map(|c| match c {
                Command::Action(req) => match &req.kind {
                    ActionKind::OpenUri(uri) => {
                        assert!(req.targets.is_empty());
                        Some(uri.to_string())
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(uris.len(), 1, "{uris:?}");
        assert!(uris[0].ends_with("/tasklens/logs"), "{}", uris[0]);
    }

    #[gpui_kit::test]
    fn crash_report_enter_closes(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let (_, mut vcx) = opened(&mut rig, cx);
        let handle = crash_window(cx).expect("open");
        // One Enter as the platform sends it: the focused Close button clicks on key up.
        let keystroke = Keystroke::parse("enter").expect("keystroke");
        vcx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        if is_open(cx, handle) {
            vcx.simulate_event(KeyUpEvent { keystroke });
        }
        vcx.run_until_parked();
        assert!(!is_open(cx, handle), "Enter on Close closes the window");
    }
}

// ---- Public IP Notice (tasklens-bj6.20, UI-SHL-096) ----

mod public_ip {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, Modifiers,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualContext as _,
        VisualTestContext, Window, WindowHandle, div, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::settings::{Settings, UiState};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::dialog::{DialogButton, TlDialogView};
    use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::public_ip::request_public_ip_fetch;

    struct Body;

    impl Render for Body {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child("parent")
        }
    }

    type ParentFrame = TlSecondaryWindow<Body>;

    struct Rig {
        /// How many times the fetch ran. The test fetch only counts; nothing touches the network.
        fetches: Rc<Cell<u32>>,
        pcx: VisualTestContext,
        parent_id: gpui_kit::WindowId,
    }

    fn rig(cx: &mut TestAppContext, accepted: bool) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let mut settings = Settings::default();
        settings.network.public_ip_notice_accepted = accepted;
        let init = UiInit {
            settings,
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
            cx.set_reduce_motion(true);
        });
        cx.run_until_parked();
        let spec = TlWindowSpec::new(
            TlWindowKind::Tool { resizable: true },
            "parent",
            size(px(600.), px(400.)),
        );
        let handle = cx
            .update(|cx| ParentFrame::open(cx, spec, |_, cx| cx.new(|_| Body)))
            .expect("parent opens");
        let mut pcx = VisualTestContext::from_window(handle.into(), cx);
        pcx.update(|window, _| window.activate_window());
        pcx.run_until_parked();
        Rig {
            fetches: Rc::new(Cell::new(0)),
            pcx,
            parent_id: handle.window_id(),
        }
    }

    /// Calls `request_public_ip_fetch` from the parent with a counting fetch.
    fn request(r: &mut Rig) {
        let count = r.fetches.clone();
        r.pcx.update(|window, cx| {
            request_public_ip_fetch(window, cx, move |_, _| count.set(count.get() + 1))
        });
        r.pcx.run_until_parked();
    }

    fn windows(cx: &mut TestAppContext) -> usize {
        cx.update(|cx| cx.windows().len())
    }

    /// The notice window and its dialog view.
    fn notice(
        r: &Rig,
        cx: &mut TestAppContext,
    ) -> (WindowHandle<Root>, Entity<TlDialogView>, VisualTestContext) {
        let parent_id = r.parent_id;
        let handle = cx
            .update(|cx| {
                cx.windows()
                    .into_iter()
                    .find(|w| w.window_id() != parent_id)
            })
            .and_then(|w: AnyWindowHandle| w.downcast::<Root>())
            .expect("the notice window");
        let view = cx
            .update(|cx| TlSecondaryWindow::<TlDialogView>::find(handle, cx))
            .map(|frame| cx.read(|cx| frame.read(cx).content().clone()))
            .expect("a TlDialog");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        (handle, view, vcx)
    }

    fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
        let id = handle.window_id();
        cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
    }

    fn accepted(cx: &mut TestAppContext) -> bool {
        cx.update(|cx| TlSettings::settings(cx).network.public_ip_notice_accepted)
    }

    fn click(vcx: &mut VisualTestContext, selector: &'static str) {
        let at = vcx.debug_bounds(selector).expect("drawn");
        vcx.simulate_click(at.center(), Modifiers::none());
        vcx.run_until_parked();
        // Redraw for the next click, unless the click closed the notice.
        let id = vcx.window_handle().window_id();
        if vcx.read(|cx| cx.windows().iter().any(|w| w.window_id() == id)) {
            vcx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    #[gpui_kit::test]
    fn public_ip_notice_first_fetch(cx: &mut TestAppContext) {
        let mut r = rig(cx, false);
        let before = windows(cx);
        request(&mut r);
        assert_eq!(windows(cx), before + 1, "the notice opens");
        assert_eq!(r.fetches.get(), 0, "no fetch before Continue");
        let (_, view, mut vcx) = notice(&r, cx);
        let (title, text, focused) = vcx.update(|window, cx| {
            let v = view.read(cx);
            (
                v.dialog().window_title().to_string(),
                v.dialog().text().to_string(),
                v.focused_button(window),
            )
        });
        assert_eq!(title, "tasklens Public IP Notice");
        assert_eq!(
            text,
            "tasklens will fetch your public IP address from an online service."
        );
        assert_eq!(focused, Some(DialogButton::Action), "focus on Continue");
        assert!(
            !vcx.read(|cx| view.read(cx).checked()),
            "the box starts unchecked"
        );

        // A second request while open focuses the notice and drops its fetch.
        request(&mut r);
        assert_eq!(windows(cx), before + 1, "still one notice");
        let (_, _, mut vcx) = notice(&r, cx);
        click(&mut vcx, "TlDialogAction");
        assert_eq!(r.fetches.get(), 1, "only the first fetch runs");
    }

    #[gpui_kit::test]
    fn public_ip_continue_unchecked(cx: &mut TestAppContext) {
        let mut r = rig(cx, false);
        request(&mut r);
        let (handle, _, mut vcx) = notice(&r, cx);
        click(&mut vcx, "TlDialogAction");
        assert!(!is_open(cx, handle), "Continue closes the notice");
        assert_eq!(r.fetches.get(), 1);
        assert!(!accepted(cx), "the key stays false");
    }

    #[gpui_kit::test]
    fn public_ip_continue_checked_writes_key(cx: &mut TestAppContext) {
        let mut r = rig(cx, false);
        request(&mut r);
        let (_, view, mut vcx) = notice(&r, cx);
        click(&mut vcx, "TlCheckbox-label");
        assert!(vcx.read(|cx| view.read(cx).checked()), "the box is checked");
        click(&mut vcx, "TlDialogAction");
        assert!(accepted(cx), "the key becomes true");
        assert_eq!(r.fetches.get(), 1);
    }

    #[gpui_kit::test]
    fn public_ip_cancel_no_fetch(cx: &mut TestAppContext) {
        let mut r = rig(cx, false);
        request(&mut r);
        let (handle, _, mut vcx) = notice(&r, cx);
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(!is_open(cx, handle), "Esc closes the notice");
        assert_eq!(r.fetches.get(), 0, "no fetch");
        assert!(!accepted(cx));
    }

    #[gpui_kit::test]
    fn public_ip_accepted_skips_notice(cx: &mut TestAppContext) {
        let mut r = rig(cx, true);
        let before = windows(cx);
        request(&mut r);
        assert_eq!(windows(cx), before, "no window opens");
        assert_eq!(r.fetches.get(), 1, "the fetch runs at once");
    }
}

// ---- Theme Editor (E21-027, UI-SHL-081) ----

type EditorFrame = TlSecondaryWindow<ThemeEditor>;

struct EditorRig {
    parent: VisualTestContext,
    toasts: Entity<ToastQueue>,
}

/// Sets up the themes and a parent window with a Root and a toast queue, as the main window has.
fn editor_rig(cx: &mut TestAppContext, themes: &[ThemeDef]) -> EditorRig {
    setup_themes(cx, themes);
    cx.update(tl_ui::components::toast::init);
    let parent = cx.add_window(|window, cx| Root::new(cx.new(|_| Parent), window, cx));
    let mut parent = VisualTestContext::from_window(parent.into(), cx);
    let toasts =
        parent.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, gpui_kit::px(40.))));
    parent.update(|window, _| window.activate_window());
    draw(&mut parent);
    EditorRig { parent, toasts }
}

fn edit(r: &mut EditorRig, name: &str) {
    r.parent.dispatch_action(EditTheme {
        name: name.to_owned().into(),
    });
    r.parent.run_until_parked();
}

fn editor_handle(cx: &mut TestAppContext) -> Option<WindowHandle<Root>> {
    cx.update(|cx| {
        cx.global::<GlobalRegistry>()
            .0
            .read(cx)
            .get(&WindowKey::ThemeEditor)
    })
    .and_then(|h| h.downcast::<Root>())
}

fn editor_vcx(cx: &mut TestAppContext) -> (Entity<EditorFrame>, VisualTestContext) {
    let handle = editor_handle(cx).expect("editor registered");
    let frame = cx
        .update(|cx| EditorFrame::find(handle, cx))
        .expect("editor frame");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    (frame, vcx)
}

fn window_open(cx: &mut TestAppContext, id: gpui_kit::WindowId) -> bool {
    cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
}

fn toast_texts(r: &mut EditorRig) -> Vec<String> {
    let q = r.toasts.clone();
    r.parent.read(|cx| {
        q.read(cx)
            .visible_texts()
            .iter()
            .map(|t| t.to_string())
            .collect()
    })
}

#[gpui_kit::test]
fn editor_builtin_refused(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    edit(&mut r, "Default Dark");
    draw(&mut r.parent);
    assert_eq!(
        toast_texts(&mut r),
        ["Built-in themes cannot be edited. Copy it first."]
    );
    assert!(editor_handle(cx).is_none(), "no editor window");
    assert!(
        cx.update(|cx| ThemeStore::preview_name(cx)).is_none(),
        "no preview"
    );
}

#[gpui_kit::test]
fn editor_cancel_restores(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    let before = cx.update(|cx| current(cx).clone());
    edit(&mut r, "Mine");
    assert!(
        cx.update(|cx| ThemeStore::preview_name(cx)).is_some(),
        "preview live"
    );
    let (frame, mut vcx) = editor_vcx(cx);
    let id = vcx.window_handle().window_id();
    // The preview token is private to the editor, so the edit is a rename.
    vcx.simulate_keystrokes("ctrl-a");
    vcx.simulate_input("Renamed");
    vcx.run_until_parked();
    cx.update(|cx| {
        assert!(
            !Arc::ptr_eq(&before, current(cx)),
            "the preview replaced current()"
        );
    });
    vcx.dispatch_action(CloseSecondaryWindow);
    vcx.run_until_parked();
    drop(frame);
    assert!(!window_open(cx, id), "window closed");
    let after = cx.update(|cx| current(cx).clone());
    assert!(
        Arc::ptr_eq(&before, &after),
        "current() is the theme active before"
    );
    assert!(cx.update(|cx| ThemeStore::preview_name(cx)).is_none());
}

#[gpui_kit::test]
fn editor_done_duplicate_name(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    edit(&mut r, "Mine");
    let (frame, mut vcx) = editor_vcx(cx);
    vcx.simulate_keystrokes("ctrl-a");
    vcx.simulate_input("default dark");
    vcx.run_until_parked();
    draw(&mut vcx);
    let f = frame.clone();
    assert!(
        !vcx.read(|cx| f.read(cx).content().read(cx).done_enabled()),
        "Done disabled"
    );
    assert!(
        vcx.debug_bounds("ThemeEditor-name-error").is_some(),
        "error text drawn"
    );
    vcx.dispatch_action(ThemeEditorDone);
    vcx.run_until_parked();
    let id = vcx.window_handle().window_id();
    assert!(window_open(cx, id), "Done does nothing");
    assert_eq!(cx.update(|cx| cx.global::<ThemeStore>().user.len()), 1);
    assert_eq!(
        cx.update(|cx| cx.global::<ThemeStore>().user[0].name.to_string()),
        "Mine"
    );
}

#[gpui_kit::test]
fn editor_done_saves(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    edit(&mut r, "Mine");
    let (frame, mut vcx) = editor_vcx(cx);
    let id = vcx.window_handle().window_id();
    vcx.simulate_keystrokes("ctrl-a");
    vcx.simulate_input("Mine 2");
    vcx.run_until_parked();
    draw(&mut vcx);
    vcx.dispatch_action(ThemeEditorDone);
    vcx.run_until_parked();
    drop(frame);
    assert!(!window_open(cx, id), "window closed");
    let (names, active, setting) = cx.update(|cx| {
        let store = cx.global::<ThemeStore>();
        (
            store
                .user
                .iter()
                .map(|d| d.name.to_string())
                .collect::<Vec<_>>(),
            store.active.name.to_string(),
            tl_ui::settings_global::TlSettings::settings(cx)
                .theme
                .active
                .clone(),
        )
    });
    assert_eq!(names, ["Mine 2"]);
    assert_eq!(active, "Mine 2");
    assert_eq!(setting, "Mine 2");
    draw(&mut r.parent);
    assert_eq!(toast_texts(&mut r), ["Theme saved"]);
}

#[gpui_kit::test]
fn editor_second_theme_replaces_first(cx: &mut TestAppContext) {
    let mut other = mine();
    other.name = "Other".into();
    let mut r = editor_rig(cx, &[mine(), other]);
    edit(&mut r, "Mine");
    let first = editor_handle(cx).expect("first editor").window_id();
    edit(&mut r, "Other");
    let second = editor_handle(cx).expect("second editor").window_id();
    assert_ne!(first, second, "a new window opened");
    assert!(!window_open(cx, first), "the first editor closed");
    assert_eq!(
        cx.update(|cx| ThemeStore::preview_name(cx)).as_deref(),
        Some("Other")
    );
    let editors = cx.update(|cx| {
        cx.windows()
            .iter()
            .filter(|w| {
                w.downcast::<Root>()
                    .is_some_and(|h| EditorFrame::find(h, cx).is_some())
            })
            .count()
    });
    assert_eq!(editors, 1, "one editor window");
}

#[gpui_kit::test]
fn editor_slot_list_keys(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    edit(&mut r, "Mine");
    let (frame, mut vcx) = editor_vcx(cx);
    let at = vcx.debug_bounds("ThemeEditor-slot-0").expect("row 0 drawn");
    vcx.simulate_click(at.center(), gpui_kit::Modifiers::none());
    draw(&mut vcx);
    let f = frame.clone();
    assert_eq!(
        vcx.read(|cx| f.read(cx).content().read(cx).selected_slot()),
        tl_ui::theme::ThemeSlot::WindowBg
    );
    vcx.simulate_keystrokes("down");
    vcx.run_until_parked();
    let f = frame.clone();
    assert_eq!(
        vcx.read(|cx| f.read(cx).content().read(cx).selected_slot()),
        tl_ui::theme::ThemeSlot::Titlebar
    );
}

// ---- Theme Editor picker (E21-028, UI-SHL-081) ----

fn editor_content(frame: &Entity<EditorFrame>, vcx: &mut VisualTestContext) -> Entity<ThemeEditor> {
    let f = frame.clone();
    vcx.read(|cx| f.read(cx).content().clone())
}

/// Opens "Mine" and selects "Title bar" (#27292B).
fn editor_on_titlebar(
    cx: &mut TestAppContext,
) -> (EditorRig, Entity<EditorFrame>, VisualTestContext) {
    let mut r = editor_rig(cx, &[mine()]);
    edit(&mut r, "Mine");
    let (frame, mut vcx) = editor_vcx(cx);
    let at = vcx.debug_bounds("ThemeEditor-slot-1").expect("row 1 drawn");
    vcx.simulate_click(at.center(), gpui_kit::Modifiers::none());
    draw(&mut vcx);
    (r, frame, vcx)
}

fn style_version(cx: &mut TestAppContext) -> u64 {
    cx.update(|cx| tl_ui::theme::metrics(cx).style_version)
}

fn type_hex(vcx: &mut VisualTestContext, text: &str) {
    let at = vcx.debug_bounds("ThemeEditor-hex").expect("hex drawn");
    vcx.simulate_click(at.center(), gpui_kit::Modifiers::none());
    vcx.simulate_keystrokes("ctrl-a");
    vcx.simulate_input(text);
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    draw(vcx);
}

#[gpui_kit::test]
fn editor_readouts_for_slot(cx: &mut TestAppContext) {
    let (_r, frame, mut vcx) = editor_on_titlebar(cx);
    let ed = editor_content(&frame, &mut vcx);
    vcx.read(|cx| {
        let e = ed.read(cx);
        assert_eq!(e.selected_slot(), tl_ui::theme::ThemeSlot::Titlebar);
        assert_eq!(e.readouts(), ("R 39 G 41 B 43", "H 210 S 009 V 017"));
    });
    let hex = vcx.read(|cx| ed.read(cx).hex_text(cx));
    assert_eq!(hex, "#27292B");
}

#[gpui_kit::test]
fn editor_hex_invalid(cx: &mut TestAppContext) {
    let (_r, frame, mut vcx) = editor_on_titlebar(cx);
    let ed = editor_content(&frame, &mut vcx);
    let before = cx.update(|cx| current(cx).titlebar);
    let n = style_version(cx);
    type_hex(&mut vcx, "12345");
    assert_eq!(cx.update(|cx| current(cx).titlebar), before);
    assert_eq!(style_version(cx), n);
    assert!(vcx.read(|cx| ed.read(cx).hex_error()));
    assert!(vcx.debug_bounds("ThemeEditor-hex-error").is_some());
}

#[gpui_kit::test]
fn editor_hex_without_hash_applies(cx: &mut TestAppContext) {
    let (_r, _frame, mut vcx) = editor_on_titlebar(cx);
    type_hex(&mut vcx, "1E1F22");
    vcx.run_until_parked();
    assert_eq!(
        cx.update(|cx| current(cx).titlebar),
        builtin::parse_hex("#1E1F22").expect("hex")
    );
}

#[gpui_kit::test]
fn editor_plane_keys(cx: &mut TestAppContext) {
    let (_r, frame, mut vcx) = editor_on_titlebar(cx);
    let ed = editor_content(&frame, &mut vcx);
    let plane = vcx.read(|cx| ed.read(cx).plane().cloned().expect("plane"));
    let s0 = vcx.read(|cx| plane.read(cx).s());
    let n = style_version(cx);
    let handle = vcx.read(|cx| plane.focus_handle(cx));
    vcx.update(|window, cx| handle.focus(window, cx));
    vcx.simulate_keystrokes("right");
    vcx.run_until_parked();
    let s1 = vcx.read(|cx| plane.read(cx).s());
    assert!((s1 - s0 - 0.01).abs() < 1e-4, "{s0} -> {s1}");
    assert_eq!(style_version(cx), n + 1);
}

#[gpui_kit::test]
fn editor_hue_shift_step(cx: &mut TestAppContext) {
    let (_r, frame, mut vcx) = editor_on_titlebar(cx);
    let ed = editor_content(&frame, &mut vcx);
    let strip = vcx.read(|cx| ed.read(cx).hue_strip().cloned().expect("strip"));
    assert!((vcx.read(|cx| strip.read(cx).hue()) - 210.0).abs() < 0.5);
    let handle = vcx.read(|cx| strip.focus_handle(cx));
    vcx.update(|window, cx| handle.focus(window, cx));
    vcx.simulate_keystrokes("shift-up");
    vcx.run_until_parked();
    assert!((vcx.read(|cx| strip.read(cx).hue()) - 200.0).abs() < 0.5);
}

#[gpui_kit::test]
fn editor_drag_coalesced(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    let _driver = r.parent.update(|window, cx| {
        let driver = cx.new(tl_ui::anim::AnimationDriver::new);
        tl_ui::anim::AnimationDriver::register(&driver, window, cx);
        tl_ui::theme::store::set_anim_window(window.window_handle(), cx);
        driver
    });
    edit(&mut r, "Mine");
    let (_frame, mut vcx) = editor_vcx(cx);
    let at = vcx.debug_bounds("ThemeEditor-plane").expect("plane drawn");
    vcx.simulate_mouse_down(
        at.center(),
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::none(),
    );
    let n = style_version(cx);
    for i in 0..30 {
        let x = at.left() + at.size.width * (i as f32 / 30.0);
        vcx.simulate_mouse_move(
            gpui_kit::point(x, at.center().y),
            Some(gpui_kit::MouseButton::Left),
            gpui_kit::Modifiers::none(),
        );
    }
    assert_eq!(style_version(cx), n, "nothing applies before the frame");
    r.parent.run_until_parked();
    r.parent.update(|window, cx| window.simulate_next_frame(cx));
    assert_eq!(style_version(cx), n + 1);
}

// ---- Theme Editor preview, Random, Share and Import (E21-029) ----

fn click_editor_button(vcx: &mut VisualTestContext, id: &str) {
    let selector = match id {
        "random" => "TlButton-ThemeEditor-random",
        "share" => "TlButton-ThemeEditor-share",
        _ => "TlButton-ThemeEditor-import",
    };
    let at = vcx.debug_bounds(selector).expect("button drawn");
    vcx.simulate_click(at.center(), gpui_kit::Modifiers::none());
    vcx.run_until_parked();
    draw(vcx);
}

fn current_slots(cx: &mut TestAppContext) -> Vec<gpui_kit::Hsla> {
    cx.update(|cx| {
        let t = current(cx);
        tl_ui::theme::ThemeSlot::ALL
            .iter()
            .map(|s| s.get(t))
            .collect()
    })
}

fn wcag_luminance(c: gpui_kit::Hsla) -> f32 {
    let c: gpui_kit::Rgba = c.into();
    let lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

fn wcag_contrast(a: gpui_kit::Hsla, b: gpui_kit::Hsla) -> f32 {
    let (la, lb) = (wcag_luminance(a), wcag_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn set_clipboard(vcx: &mut VisualTestContext, text: &str) {
    vcx.update(|_, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.to_owned())));
}

#[gpui_kit::test]
fn editor_preview_follows_slot(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    edit(&mut r, "Mine");
    let (frame, mut vcx) = editor_vcx(cx);
    let at = vcx.debug_bounds("ThemeEditor-slot-9").expect("row 9 drawn");
    vcx.simulate_click(at.center(), gpui_kit::Modifiers::none());
    draw(&mut vcx);
    type_hex(&mut vcx, "#FF0000");
    vcx.run_until_parked();
    draw(&mut vcx);
    let ed = editor_content(&frame, &mut vcx);
    let rows = vcx.read(|cx| ed.read(cx).preview_row_bgs());
    assert_eq!(rows[3], builtin::parse_hex("#FF0000").expect("hex"));
}

#[gpui_kit::test]
fn editor_random_contrast_and_toast(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    edit(&mut r, "Mine");
    let (frame, mut vcx) = editor_vcx(cx);
    let before = current_slots(cx);
    click_editor_button(&mut vcx, "random");
    vcx.run_until_parked();
    let after = current_slots(cx);
    assert_eq!(before.len(), 23);
    for (i, (a, b)) in before.iter().zip(&after).enumerate() {
        assert_ne!(a, b, "slot {i} changed");
    }
    let (text, list) = cx.update(|cx| {
        let t = current(cx);
        (t.text_primary, t.list_bg)
    });
    assert!(wcag_contrast(text, list) >= 4.5);
    let toasts = vcx.read(|cx| frame.read(cx).toasts().read(cx).visible_texts());
    assert_eq!(toasts, ["Random theme generated."]);
}

#[gpui_kit::test]
fn editor_share_encodes_preview(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    edit(&mut r, "Mine");
    let (_frame, mut vcx) = editor_vcx(cx);
    let at = vcx.debug_bounds("ThemeEditor-slot-1").expect("row 1 drawn");
    vcx.simulate_click(at.center(), gpui_kit::Modifiers::none());
    draw(&mut vcx);
    type_hex(&mut vcx, "#123456");
    click_editor_button(&mut vcx, "share");
    vcx.run_until_parked();
    let handle = cx
        .update(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::ShareTheme)
        })
        .and_then(|h| h.downcast::<Root>())
        .expect("share window registered");
    let share = cx
        .update(|cx| Frame::find(handle, cx))
        .expect("share frame");
    let code = cx.update(|cx| share.read(cx).content().read(cx).code().to_string());
    let def = decode_tl1(&code).expect("decodes");
    assert_eq!(&*def.name, "Mine");
    assert_eq!(
        def.slots[tl_ui::theme::ThemeSlot::Titlebar as usize],
        builtin::parse_hex("#123456").expect("hex")
    );
}

#[gpui_kit::test]
fn editor_import_loads_preview(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    edit(&mut r, "Mine");
    let (frame, mut vcx) = editor_vcx(cx);
    let mut def = builtin::load()[0].clone();
    def.name = "Imported".into();
    def.slots[tl_ui::theme::ThemeSlot::RowSelected as usize] =
        builtin::parse_hex("#FF0000").expect("hex");
    set_clipboard(&mut vcx, &encode_tl1(&def));
    click_editor_button(&mut vcx, "import");
    vcx.run_until_parked();
    cx.update(|cx| {
        let t = current(cx);
        assert_eq!(&*t.name, "Imported");
        assert_eq!(t.row_selected, builtin::parse_hex("#FF0000").expect("hex"));
    });
    let ed = editor_content(&frame, &mut vcx);
    let name = vcx.read(|cx| ed.read(cx).name_text(cx));
    assert_eq!(name, "Imported");
    let users: Vec<String> = cx.update(|cx| {
        cx.global::<ThemeStore>()
            .user
            .iter()
            .map(|d| d.name.to_string())
            .collect()
    });
    assert_eq!(users, ["Mine"], "no theme is added");
}

#[gpui_kit::test]
fn editor_import_garbage_toast(cx: &mut TestAppContext) {
    let mut r = editor_rig(cx, &[mine()]);
    edit(&mut r, "Mine");
    let (frame, mut vcx) = editor_vcx(cx);
    let before = current_slots(cx);
    set_clipboard(&mut vcx, "hello");
    click_editor_button(&mut vcx, "import");
    vcx.run_until_parked();
    let toasts = vcx.read(|cx| frame.read(cx).toasts().read(cx).visible_texts());
    assert_eq!(toasts, ["Clipboard does not contain a theme code"]);
    assert_eq!(current_slots(cx), before);
}
