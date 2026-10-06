//! Shell secondary windows: Share Theme (E21-030, UI-SHL-082) and Collector Health (E21-007,
//! UI-SHL-099). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, Context, Entity, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke,
    ParentElement as _, Render, Styled as _, TestAppContext, VisualContext as _, VisualTestContext,
    Window, WindowHandle, div,
};
use tl_model::client::EngineClient;
use tl_model::health::{SourceHealth, SourceId, SourceState};
use tl_model::settings::{Settings, UiState};
use tl_model::time::{TimeNs, now_boot_ns};
use tl_ui::actions::OpenCollectorHealth;
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
