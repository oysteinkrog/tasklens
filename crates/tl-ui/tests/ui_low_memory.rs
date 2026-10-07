//! Low-memory rescue window: frame, texts, filter, eased state, keys, the minimal init, and the
//! process list with End Task (E32-009, E32-A2, UI-PWN-032, UI-PWN-033, R-243). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use gpui_kit::base::Root;
use gpui_kit::{
    Bounds, DevicePixels, Focusable as _, Font, FontId, FontMetrics, FontRun, GlyphId, LineLayout,
    Modifiers, Pixels, PlatformTextSystem, RenderGlyphParams, Size, TestAppContext, TestDispatcher,
    TextRenderingMode, VisualTestContext, WindowHandle,
};
use tl_model::actions::{
    ActionError, ActionEvent, ActionKind, ActionRequest, ActionState, ActionTarget, CgroupRef,
    LaunchWhat, RejectReason, Signal,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, KWinCommand};
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{
    LowMemGroup, LowMemoryInfo, ProcFlags, ProcState, ProcessMeta, ProcessRecord,
};
use tl_model::settings::{Settings, UiState};
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, Validity};
use tl_ui::app::{AppEnv, OpenHooks, UiExit, UiExitSlot, UiInit, start_for_test};
use tl_ui::components::toast::ToastQueue;
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::current;
use tl_ui::windows::low_memory::{LowMemoryWindow, MAX_ROWS, RowId, init_low_memory};
use tl_ui::{OpenMode, StartOptions, UiLaunch};

const SECOND: u64 = 1_000_000_000;

fn ui_init() -> UiInit {
    let (_tx, rx) = async_channel::bounded(4);
    UiInit {
        settings: Settings::default(),
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    }
}

/// `init_low_memory` against a new `FakeEngine`.
fn setup(cx: &mut TestAppContext) -> Arc<FakeEngine> {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        init_low_memory(cx, client, ui_init());
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    engine
}

fn group(id: u64, name: &str) -> LowMemGroup {
    LowMemGroup {
        group: GroupKey(id),
        name: Arc::from(name),
        process_count: 3,
        uid: 1000,
        cgroup: CgroupRef {
            rel_path: Arc::from(format!("user.slice/app-{name}.scope")),
            cgroup_id: id,
        },
        memory_current: 1 << 30,
    }
}

/// Publishes one low-memory sweep taken at `at_s` seconds.
fn sample(cx: &mut TestAppContext, engine: &FakeEngine, at_s: u64, critical: bool) {
    engine.publish_with(|s| {
        s.published_ns = at_s * SECOND;
        let p = Arc::make_mut(&mut s.processes);
        p.header.version += 1;
        p.low_memory = Some(Arc::new(LowMemoryInfo {
            groups: Arc::from([group(1, "Firefox"), group(2, "Konsole")]),
            mem_total: 16 << 30,
            mem_available: if critical { 100 << 20 } else { 8 << 30 },
            psi_full_pct: Some(if critical { 40. } else { 1. }),
            critical,
        }));
    });
    cx.run_until_parked();
}

fn open(
    cx: &mut TestAppContext,
    engine: &Arc<FakeEngine>,
) -> (
    WindowHandle<Root>,
    VisualTestContext,
    gpui_kit::Entity<LowMemoryWindow>,
) {
    let client: Arc<dyn EngineClient> = engine.clone();
    let handle = cx
        .update(|cx| LowMemoryWindow::open(cx, client))
        .expect("window opens");
    let view = cx
        .update(|cx| LowMemoryWindow::find(handle, cx))
        .expect("content view");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    (handle, vcx, view)
}

fn focus_list(vcx: &mut VisualTestContext, view: &gpui_kit::Entity<LowMemoryWindow>) {
    let list = vcx.read(|cx| view.read(cx).list_focus().clone());
    vcx.update(|window, cx| list.focus(window, cx));
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
    let id = handle.window_id();
    cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
}

fn header_texts(
    vcx: &mut VisualTestContext,
    view: &gpui_kit::Entity<LowMemoryWindow>,
) -> Vec<String> {
    vcx.update(|_, cx| {
        view.read(cx)
            .header(cx)
            .iter()
            .map(|l| l.text.to_string())
            .collect()
    })
}

const CRITICAL: &str = "Your device is running on critically low memory.";
const RECOMMEND: &str = "We recommend you close some of the processes below.";
const EASED: &str =
    "Memory pressure has eased. You can open the full tasklens or close this window.";

#[gpui_kit::test]
fn lowmem_texts_and_title(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sample(cx, &engine, 0, true);
    let (handle, mut vcx, view) = open(cx, &engine);

    let frame = cx
        .update(|cx| TlSecondaryWindow::<LowMemoryWindow>::find(handle, cx))
        .expect("frame");
    assert_eq!(
        vcx.read(|cx| frame.read(cx).title().to_string()),
        "tasklens - Low memory"
    );
    assert_eq!(
        vcx.read(|cx| frame.read(cx).drawn_title()).as_deref(),
        Some("tasklens - Low memory")
    );

    let (lines, danger, secondary) = vcx.update(|_, cx| {
        let theme = current(cx).clone();
        (
            view.read(cx).header(cx),
            theme.status.danger,
            theme.text_secondary,
        )
    });
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].text.as_ref(), CRITICAL);
    assert_eq!(lines[0].color, danger);
    assert!(lines[0].strong);
    assert_eq!(lines[1].text.as_ref(), RECOMMEND);
    assert_eq!(lines[1].color, secondary);
    assert!(vcx.debug_bounds("LowMemHeaderLine").is_some());
    assert!(vcx.debug_bounds("LowMemOpenFull").is_some());

    // Focus starts in the filter.
    assert!(vcx.update(|window, cx| {
        view.read(cx)
            .filter()
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }));
    let names: Vec<String> = vcx.read(|cx| {
        view.read(cx)
            .visible_names()
            .map(|n| n.to_string())
            .collect()
    });
    assert_eq!(names, ["Firefox (3)", "Konsole (3)"]);

    // A second frame makes no `tr` call.
    let before = tl_ui::i18n::tr_calls();
    draw(&mut vcx);
    assert_eq!(tl_ui::i18n::tr_calls(), before);
}

#[gpui_kit::test]
fn lowmem_starts_pinned(cx: &mut TestAppContext) {
    let engine = setup(cx);
    let (handle, vcx, _) = open(cx, &engine);
    let frame = cx
        .update(|cx| TlSecondaryWindow::<LowMemoryWindow>::find(handle, cx))
        .expect("frame");
    assert!(vcx.read(|cx| frame.read(cx).pinned()));
    assert!(vcx.read(|cx| frame.read(cx).pin_look(cx).on));

    let keep_above: Vec<_> = engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::KWin(KWinCommand::KeepAbove { pid, caption, on }) => Some((pid, caption, on)),
            _ => None,
        })
        .collect();
    assert_eq!(
        keep_above,
        [(
            std::process::id(),
            Some(Arc::from("tasklens - Low memory")),
            true
        )]
    );
}

#[gpui_kit::test]
fn lowmem_eased_after_10s(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sample(cx, &engine, 0, true);
    let (_, mut vcx, view) = open(cx, &engine);
    for t in [2, 4, 6, 8, 10] {
        sample(cx, &engine, t, false);
        assert!(!vcx.read(|cx| view.read(cx).eased()), "eased at {t} s");
    }
    sample(cx, &engine, 12, false);
    draw(&mut vcx);
    assert!(vcx.read(|cx| view.read(cx).eased()));
    let (lines, success) =
        vcx.update(|_, cx| (view.read(cx).header(cx), current(cx).status.success));
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text.as_ref(), EASED);
    assert_eq!(lines[0].color, success);
    // The list stays usable.
    assert_eq!(vcx.read(|cx| view.read(cx).visible_names().count()), 2);
}

#[gpui_kit::test]
fn lowmem_not_eased_after_8s(cx: &mut TestAppContext) {
    let engine = setup(cx);
    let (_, mut vcx, view) = open(cx, &engine);
    for t in [0, 2, 4, 6, 8] {
        sample(cx, &engine, t, false);
    }
    assert!(!vcx.read(|cx| view.read(cx).eased()));
    assert_eq!(header_texts(&mut vcx, &view), [CRITICAL, RECOMMEND]);
}

#[gpui_kit::test]
fn lowmem_eased_returns_on_pressure(cx: &mut TestAppContext) {
    let engine = setup(cx);
    let (_, mut vcx, view) = open(cx, &engine);
    for t in [0, 2, 4, 6, 8, 10] {
        sample(cx, &engine, t, false);
    }
    assert_eq!(header_texts(&mut vcx, &view), [EASED]);
    sample(cx, &engine, 12, true);
    draw(&mut vcx);
    assert_eq!(header_texts(&mut vcx, &view), [CRITICAL, RECOMMEND]);
}

#[gpui_kit::test]
fn lowmem_esc_clears_then_closes(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sample(cx, &engine, 0, true);
    let (handle, mut vcx, view) = open(cx, &engine);
    vcx.simulate_input("fire");
    vcx.run_until_parked();
    assert_eq!(
        vcx.read(|cx| view.read(cx).filter().read(cx).text().to_string()),
        "fire"
    );
    let names: Vec<String> = vcx.read(|cx| {
        view.read(cx)
            .visible_names()
            .map(|n| n.to_string())
            .collect()
    });
    assert_eq!(names, ["Firefox (3)"]);

    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(is_open(cx, handle));
    assert_eq!(
        vcx.read(|cx| view.read(cx).filter().read(cx).text().to_string()),
        ""
    );

    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!is_open(cx, handle));
    assert_eq!(
        cx.update(|cx| cx.try_global::<UiExitSlot>().and_then(|s| s.0.clone())),
        Some(UiExit::Quit)
    );
}

#[gpui_kit::test]
fn lowmem_list_typing_starts_filter(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sample(cx, &engine, 0, true);
    let (handle, mut vcx, view) = open(cx, &engine);
    focus_list(&mut vcx, &view);
    vcx.simulate_keystrokes("k");
    vcx.run_until_parked();
    assert_eq!(
        vcx.read(|cx| view.read(cx).filter().read(cx).text().to_string()),
        "k"
    );
    assert!(vcx.update(|window, cx| {
        view.read(cx)
            .filter()
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }));

    // Esc in the list clears the filter first.
    focus_list(&mut vcx, &view);
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(is_open(cx, handle));
    assert_eq!(
        vcx.read(|cx| view.read(cx).filter().read(cx).text().to_string()),
        ""
    );

    vcx.simulate_keystrokes("ctrl-w");
    vcx.run_until_parked();
    assert!(!is_open(cx, handle));
}

#[gpui_kit::test]
fn lowmem_open_full_activates_or_spawns(cx: &mut TestAppContext) {
    let engine = setup(cx);
    let (handle, mut vcx, _) = open(cx, &engine);
    let button = vcx
        .debug_bounds("LowMemOpenFull")
        .expect("Open Full tasklens drawn");
    vcx.simulate_click(button.center(), Modifiers::none());
    vcx.run_until_parked();

    let launches: Vec<_> = engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::Action(req) => Some(req),
            _ => None,
        })
        .collect();
    assert_eq!(launches.len(), 1);
    match &launches[0].kind {
        ActionKind::Launch(spec) => {
            assert_eq!(spec.what, LaunchWhat::Command(Arc::from("tasklens")));
            assert!(!spec.as_admin);
            assert!(spec.cwd.is_none());
        }
        other => panic!("not a launch: {other:?}"),
    }
    assert!(launches[0].targets.is_empty());
    assert!(!launches[0].privileged);
    assert!(is_open(cx, handle));
}

#[gpui_kit::test]
fn lowmem_filter_no_match_text(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sample(cx, &engine, 0, true);
    let (_, mut vcx, view) = open(cx, &engine);
    assert!(vcx.update(|_, cx| view.read(cx).empty_text(cx)).is_none());
    vcx.simulate_input("zzz");
    vcx.run_until_parked();
    draw(&mut vcx);
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).empty_text(cx)).as_deref(),
        Some("No processes match the filter.")
    );
    assert!(vcx.debug_bounds("LowMemFilterEmpty").is_some());
}

/// Wraps a platform text system and records every face added to it.
struct RecordingText {
    inner: Arc<dyn PlatformTextSystem>,
    added: Mutex<Vec<Vec<u8>>>,
}

impl PlatformTextSystem for RecordingText {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> gpui_kit::Result<()> {
        self.added
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(fonts.iter().map(|f| f.to_vec()));
        self.inner.add_fonts(fonts)
    }
    fn all_font_names(&self) -> Vec<String> {
        self.inner.all_font_names()
    }
    fn font_id(&self, descriptor: &Font) -> gpui_kit::Result<FontId> {
        self.inner.font_id(descriptor)
    }
    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        self.inner.font_metrics(font_id)
    }
    fn typographic_bounds(
        &self,
        font_id: FontId,
        glyph_id: GlyphId,
    ) -> gpui_kit::Result<Bounds<f32>> {
        self.inner.typographic_bounds(font_id, glyph_id)
    }
    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> gpui_kit::Result<Size<f32>> {
        self.inner.advance(font_id, glyph_id)
    }
    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.inner.glyph_for_char(font_id, ch)
    }
    fn glyph_raster_bounds(
        &self,
        params: &RenderGlyphParams,
    ) -> gpui_kit::Result<Bounds<DevicePixels>> {
        self.inner.glyph_raster_bounds(params)
    }
    fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> gpui_kit::Result<(Size<DevicePixels>, Vec<u8>)> {
        self.inner.rasterize_glyph(params, raster_bounds)
    }
    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.inner.layout_line(text, font_size, runs)
    }
    fn recommended_rendering_mode(&self, font_id: FontId, font_size: Pixels) -> TextRenderingMode {
        self.inner.recommended_rendering_mode(font_id, font_size)
    }
}

const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Regular.ttf");
const MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Medium.ttf");
const SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-SemiBold.ttf");

/// A plain test: it needs its own text system. System fonts can hold a Noto Sans Medium, so the
/// test checks the faces `init_low_memory` adds, not the faces the text system resolves.
#[test]
fn lowmem_minimal_init_hook_and_faces() {
    let text = Arc::new(RecordingText {
        inner: gpui_kit::platform::current_platform(true).text_system(),
        added: Mutex::new(Vec::new()),
    });
    let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text.clone());
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine;
    cx.update(|cx| {
        init_low_memory(cx, client, ui_init());
        let hooks = cx.global::<OpenHooks>();
        assert!(hooks.low_memory.is_some());
        assert!(hooks.main.is_none());
        assert!(hooks.gallery.is_none());
    });

    let added = text.added.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(added.len(), 2);
    assert!(added.iter().any(|f| f.as_slice() == REGULAR));
    assert!(added.iter().any(|f| f.as_slice() == SEMIBOLD));
    assert!(!added.iter().any(|f| f.as_slice() == MEDIUM));
}

/// A key down and up: a focused button activates on the key up.
fn press(vcx: &mut VisualTestContext, key: &str) {
    let keystroke = gpui_kit::Keystroke::parse(key).expect("keystroke");
    vcx.simulate_event(gpui_kit::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    vcx.simulate_event(gpui_kit::KeyUpEvent { keystroke });
    vcx.run_until_parked();
}

/// Whether a 2 px `accent` focus ring is painted over the centre of `selector`.
fn ring_on(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    let Some(bounds) = vcx.debug_bounds(selector) else {
        return false;
    };
    vcx.update(|window, cx| {
        let accent = current(cx).accent;
        let s = window.scale_factor();
        let center = bounds.center().scale(s);
        window.painted_quads().iter().any(|q| {
            q.border_color == accent
                && q.border_widths.top.0 == 2. * s
                && q.bounds.contains(&center)
        })
    })
}

/// Where focus is: "filter", "list", "open-full", "pin", "close", "other" or "none". The
/// buttons are told apart by their keyboard focus ring.
fn focus_at(vcx: &mut VisualTestContext, view: &gpui_kit::Entity<LowMemoryWindow>) -> &'static str {
    draw(vcx);
    let focused = vcx.update(|window, cx| {
        let view = view.read(cx);
        if view.filter().read(cx).focus_handle(cx).is_focused(window) {
            Some("filter")
        } else if view.list_focus().is_focused(window) {
            Some("list")
        } else if window.focused(cx).is_none() {
            Some("none")
        } else {
            None
        }
    });
    if let Some(name) = focused {
        return name;
    }
    [
        ("open-full", "LowMemOpenFull"),
        ("pin", "TlWindowPin"),
        ("close", "TlWindowCaption-close"),
    ]
    .into_iter()
    .find(|(_, selector)| ring_on(vcx, selector))
    .map_or("other", |(name, _)| name)
}

fn launches(engine: &FakeEngine) -> usize {
    engine
        .commands()
        .into_iter()
        .filter(|c| matches!(c, Command::Action(_)))
        .count()
}

#[gpui_kit::test]
fn lowmem_tab_visits_pin_and_close(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sample(cx, &engine, 0, true);
    let (handle, mut vcx, view) = open(cx, &engine);
    let frame = cx
        .update(|cx| TlSecondaryWindow::<LowMemoryWindow>::find(handle, cx))
        .expect("frame");
    assert_eq!(focus_at(&mut vcx, &view), "filter");

    vcx.simulate_keystrokes("tab");
    assert_eq!(focus_at(&mut vcx, &view), "list");

    // Open Full tasklens: Enter activates it.
    vcx.simulate_keystrokes("tab");
    assert_eq!(focus_at(&mut vcx, &view), "open-full");
    press(&mut vcx, "enter");
    assert_eq!(launches(&engine), 1, "Enter on Open Full tasklens");

    // The pin: Space flips it.
    vcx.simulate_keystrokes("tab");
    assert_eq!(focus_at(&mut vcx, &view), "pin");
    assert!(vcx.read(|cx| frame.read(cx).pinned()));
    press(&mut vcx, "space");
    assert!(!vcx.read(|cx| frame.read(cx).pinned()), "Space unpins");
    press(&mut vcx, "enter");
    assert!(vcx.read(|cx| frame.read(cx).pinned()), "Enter pins again");
    assert_eq!(launches(&engine), 1);

    // Close, then Tab wraps to the filter.
    vcx.simulate_keystrokes("tab");
    assert_eq!(focus_at(&mut vcx, &view), "close");
    assert!(vcx.read(|cx| frame.read(cx).pinned()), "Tab left the pin");
    vcx.simulate_keystrokes("tab");
    assert_eq!(focus_at(&mut vcx, &view), "filter", "Tab wraps");

    // Shift+Tab from the filter reaches close; Enter closes the window.
    vcx.simulate_keystrokes("shift-tab");
    assert_eq!(focus_at(&mut vcx, &view), "close");
    press(&mut vcx, "enter");
    assert!(!is_open(cx, handle), "Enter on close closes the window");
}

#[gpui_kit::test]
fn lowmem_title_follows_language(cx: &mut TestAppContext) {
    let engine = setup(cx);
    let (handle, mut vcx, _) = open(cx, &engine);
    let frame = cx
        .update(|cx| TlSecondaryWindow::<LowMemoryWindow>::find(handle, cx))
        .expect("frame");
    assert_eq!(
        vcx.read(|cx| frame.read(cx).title().to_string()),
        "tasklens - Low memory"
    );

    vcx.update(|_, cx| {
        tl_ui::i18n::set_test_catalogue(cx, "lowmem-window-title = [Low memory]\n");
    });
    vcx.run_until_parked();
    draw(&mut vcx);
    assert_eq!(
        vcx.read(|cx| frame.read(cx).title().to_string()),
        "[Low memory]"
    );
    assert_eq!(
        vcx.read(|cx| frame.read(cx).drawn_title()).as_deref(),
        Some("[Low memory]")
    );

    // Back to English.
    vcx.update(|_, cx| tl_ui::i18n::set_language(cx, "en-US"));
    vcx.run_until_parked();
    assert_eq!(
        vcx.read(|cx| frame.read(cx).title().to_string()),
        "tasklens - Low memory"
    );

    // A second frame makes no `tr` call.
    draw(&mut vcx);
    let before = tl_ui::i18n::tr_calls();
    draw(&mut vcx);
    assert_eq!(tl_ui::i18n::tr_calls(), before);
}

/// A test context whose text system records every added face.
fn recording_cx() -> (TestAppContext, Arc<RecordingText>) {
    let text = Arc::new(RecordingText {
        inner: gpui_kit::platform::current_platform(true).text_system(),
        added: Mutex::new(Vec::new()),
    });
    let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text.clone());
    (cx, text)
}

/// The `run` closure body (`start`) for `open` against a new `FakeEngine`.
fn start_run(cx: &mut TestAppContext, open: OpenMode) -> Option<UiExit> {
    let client: Arc<dyn EngineClient> = FakeEngine::new();
    let launch = UiLaunch {
        client,
        init: ui_init(),
        open,
        start: StartOptions::default(),
    };
    let exit = cx.update(|cx| start_for_test(cx, launch));
    cx.run_until_parked();
    exit.take()
}

fn added_faces(text: &RecordingText) -> Vec<Vec<u8>> {
    text.added
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// A plain test like `lowmem_minimal_init_hook_and_faces`: the full init must not run.
#[test]
fn run_low_memory_uses_minimal_init() {
    let (mut cx, text) = recording_cx();
    assert_eq!(start_run(&mut cx, OpenMode::LowMemory), None);

    // `open_first` took the low-memory hook; the full init would have set `main`.
    cx.update(|cx| {
        let hooks = cx.global::<OpenHooks>();
        assert!(hooks.main.is_none());
        assert!(hooks.low_memory.is_none());
    });
    assert!(!added_faces(&text).iter().any(|f| f.as_slice() == MEDIUM));

    let windows = cx.update(|cx| cx.windows());
    assert_eq!(windows.len(), 1);
    let handle = windows[0].downcast::<Root>().expect("root window");
    let frame = cx
        .update(|cx| TlSecondaryWindow::<LowMemoryWindow>::find(handle, cx))
        .expect("rescue window frame");
    assert_eq!(
        cx.read(|cx| frame.read(cx).title().to_string()),
        "tasklens - Low memory"
    );
}

#[test]
fn run_main_uses_full_init() {
    let (mut cx, text) = recording_cx();
    assert_eq!(start_run(&mut cx, OpenMode::Main), None);
    cx.update(|cx| assert!(cx.global::<OpenHooks>().low_memory.is_none()));
    assert!(added_faces(&text).iter().any(|f| f.as_slice() == MEDIUM));
}

#[test]
fn low_memory_loads_two_faces() {
    let (mut cx, text) = recording_cx();
    start_run(&mut cx, OpenMode::LowMemory);
    let added = added_faces(&text);
    assert_eq!(added.len(), 2);
    assert!(added.iter().any(|f| f.as_slice() == REGULAR));
    assert!(added.iter().any(|f| f.as_slice() == SEMIBOLD));
}

// E32-A2: the process list and End Task.

const MIB: u64 = 1 << 20;
/// MemTotal of the list tests: 1000 MiB, so 120 MiB is 12.0%.
const TOTAL: u64 = 1000 * MIB;

fn key(pid: u32, start_ticks: u64) -> ProcessKey {
    ProcessKey {
        start_ticks,
        boot_id: BootId(0),
        pid,
    }
}

/// A process of this user outside any app group, with `mib` MiB of RssAnon.
fn proc(pid: u32, name: &str, mib: u64) -> ProcessRecord {
    let name: Arc<str> = Arc::from(name);
    let lower: Arc<str> = Arc::from(name.to_lowercase());
    ProcessRecord {
        key: key(pid, u64::from(pid) * 10),
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: name.clone(),
            name,
            exe: None,
            cmdline: None,
            user: Arc::from("user"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: lower,
        }),
        group: GroupKey(0),
        state: ProcState::Sleeping,
        flags: ProcFlags::empty(),
        nice: 0,
        threads: 1,
        cpu_pct: 0.,
        rss_anon_bytes: mib * MIB,
        rss_bytes: mib * MIB,
        pss_bytes: 0,
        uss_bytes: 0,
        swap_bytes: 0,
        io_read_bps: 0.,
        io_write_bps: 0.,
        net_rx_bps: 0.,
        net_tx_bps: 0.,
        gpu_util_pct: 0.,
        gpu_mem_bytes: 0,
        euid: None,
        net_shared: false,
        cancelled_write_bps: 0.,
        validity: FieldValidity::all(Validity::Valid),
    }
}

/// An app group of this user with `count` processes and `mib` MiB of `memory.current`.
fn app(id: u64, name: &str, count: u32, mib: u64) -> LowMemGroup {
    LowMemGroup {
        process_count: count,
        memory_current: mib * MIB,
        ..group(id, name)
    }
}

/// Publishes one sweep with these groups and records, under pressure.
fn sweep(
    cx: &mut TestAppContext,
    engine: &FakeEngine,
    groups: Vec<LowMemGroup>,
    mut records: Vec<ProcessRecord>,
) {
    records.sort_by_key(|r| r.key);
    engine.publish_with(|s| {
        s.published_ns += SECOND;
        let p = Arc::make_mut(&mut s.processes);
        p.header.version += 1;
        p.records = Arc::from(records);
        p.low_memory = Some(Arc::new(LowMemoryInfo {
            groups: Arc::from(groups),
            mem_total: TOTAL,
            mem_available: 10 * MIB,
            psi_full_pct: Some(40.),
            critical: true,
        }));
    });
    cx.run_until_parked();
}

fn names(vcx: &mut VisualTestContext, view: &gpui_kit::Entity<LowMemoryWindow>) -> Vec<String> {
    vcx.read(|cx| {
        view.read(cx)
            .visible_names()
            .map(|n| n.to_string())
            .collect()
    })
}

/// Every `ActionRequest` the window sent, in order.
fn actions(engine: &FakeEngine) -> Vec<ActionRequest> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::Action(req) => Some(req),
            _ => None,
        })
        .collect()
}

fn click(vcx: &mut VisualTestContext, selector: &'static str) {
    draw(vcx);
    let bounds = vcx.debug_bounds(selector).expect("drawn");
    vcx.simulate_click(bounds.center(), Modifiers::none());
    vcx.run_until_parked();
}

fn toasts(vcx: &mut VisualTestContext) -> Vec<String> {
    vcx.update(|window, cx| {
        ToastQueue::of_window(window, cx)
            .expect("toast queue")
            .read(cx)
            .visible_texts()
            .iter()
            .map(|t| t.to_string())
            .collect()
    })
}

/// Sends the engine's summary for request `req`.
fn answer(cx: &mut TestAppContext, engine: &FakeEngine, req: &ActionRequest, state: ActionState) {
    engine.emit(ActionEvent {
        id: req.id,
        target: None,
        state,
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn lowmem_list_caps_30_sorted(cx: &mut TestAppContext) {
    let engine = setup(cx);
    // 40 processes with 1..=40 MiB, in a mixed order.
    let records = (1..=40u32)
        .map(|i| {
            let mib = u64::from((i * 17) % 40 + 1);
            proc(1000 + i, &format!("p{mib}"), mib)
        })
        .collect();
    sweep(cx, &engine, Vec::new(), records);
    let (_, mut vcx, view) = open(cx, &engine);
    draw(&mut vcx);

    let rows: Vec<u64> = vcx.read(|cx| {
        view.read(cx)
            .rows()
            .iter()
            .map(|r| r.memory_bytes)
            .collect()
    });
    assert_eq!(rows.len(), MAX_ROWS);
    let want: Vec<u64> = (11..=40).rev().map(|mib| mib * MIB).collect();
    assert_eq!(rows, want);
    assert_eq!(names(&mut vcx, &view)[0], "p40");
    assert!(vcx.debug_bounds("LowMemRow-29").is_some());
    assert!(vcx.debug_bounds("LowMemRow-30").is_none());
}

#[gpui_kit::test]
fn lowmem_app_row_name_and_percent(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sweep(
        cx,
        &engine,
        vec![app(7, "Firefox", 34, 120)],
        vec![proc(4000, "konsole", 50)],
    );
    let (_, mut vcx, view) = open(cx, &engine);
    draw(&mut vcx);
    assert_eq!(names(&mut vcx, &view), ["Firefox (34)", "konsole"]);
    let (memory, a11y) = vcx.read(|cx| {
        let row = &view.read(cx).rows()[0];
        (row.memory_text.to_string(), row.a11y.to_string())
    });
    assert_eq!(memory, "Memory 12.0%");
    assert_eq!(a11y, "End Task: Firefox (34)");
    assert_eq!(
        vcx.read(|cx| view.read(cx).rows()[1].memory_text.to_string()),
        "Memory 5.0%"
    );
    assert!(vcx.debug_bounds("LowMemMemory-0").is_some());
    // The End Task button's accessible name carries the row name, when the tree is built.
    draw(&mut vcx);
    if let Some(tree) = vcx.update(|window, _| window.debug_a11y_tree_json()) {
        assert!(
            tree.contains("End Task: Firefox (34)"),
            "the a11y tree names it"
        );
    }

    // A process inside an app group is not listed on its own.
    let mut member = proc(4001, "firefox", 900);
    member.group = GroupKey(7);
    sweep(
        cx,
        &engine,
        vec![app(7, "Firefox", 34, 120)],
        vec![proc(4000, "konsole", 50), member],
    );
    assert_eq!(names(&mut vcx, &view), ["Firefox (34)", "konsole"]);

    // A frame formats nothing.
    draw(&mut vcx);
    let before = tl_ui::i18n::tr_calls();
    draw(&mut vcx);
    assert_eq!(tl_ui::i18n::tr_calls(), before);
}

#[gpui_kit::test]
fn lowmem_order_held_while_hovered(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sweep(
        cx,
        &engine,
        Vec::new(),
        vec![proc(10, "alpha", 200), proc(11, "beta", 100)],
    );
    let (_, mut vcx, view) = open(cx, &engine);
    draw(&mut vcx);
    assert_eq!(names(&mut vcx, &view), ["alpha", "beta"]);

    let list = vcx.debug_bounds("LowMemList").expect("list drawn");
    vcx.simulate_mouse_move(list.center(), None, Modifiers::none());
    vcx.run_until_parked();

    sweep(
        cx,
        &engine,
        Vec::new(),
        vec![proc(10, "alpha", 50), proc(11, "beta", 300)],
    );
    draw(&mut vcx);
    assert_eq!(names(&mut vcx, &view), ["alpha", "beta"], "order held");
    let values: Vec<String> = vcx.read(|cx| {
        view.read(cx)
            .rows()
            .iter()
            .map(|r| r.memory_text.to_string())
            .collect()
    });
    assert_eq!(values, ["Memory 5.0%", "Memory 30.0%"], "values update");

    // The pointer leaves and nothing in the list has focus: the list re-sorts.
    let header = vcx.debug_bounds("LowMemHeaderLine").expect("header drawn");
    vcx.simulate_mouse_move(header.center(), None, Modifiers::none());
    vcx.run_until_parked();
    assert_eq!(names(&mut vcx, &view), ["beta", "alpha"]);

    // Focus in the list holds the order too, until it moves out.
    focus_list(&mut vcx, &view);
    sweep(
        cx,
        &engine,
        Vec::new(),
        vec![proc(10, "alpha", 400), proc(11, "beta", 300)],
    );
    assert_eq!(names(&mut vcx, &view), ["beta", "alpha"], "held by focus");
    let filter = vcx.read(|cx| view.read(cx).filter().read(cx).focus_handle(cx));
    vcx.update(|window, cx| filter.focus(window, cx));
    vcx.run_until_parked();
    assert_eq!(names(&mut vcx, &view), ["alpha", "beta"]);
}

#[gpui_kit::test]
fn lowmem_other_user_disabled_tooltip(cx: &mut TestAppContext) {
    let engine = setup(cx);
    let mut root = proc(20, "postgres", 300);
    root.uid = 0;
    root.flags = ProcFlags::OTHER_USER;
    sweep(cx, &engine, Vec::new(), vec![root, proc(21, "kate", 100)]);
    let (_, mut vcx, view) = open(cx, &engine);
    draw(&mut vcx);
    assert_eq!(names(&mut vcx, &view), ["postgres", "kate"]);
    let (other, tip0, tip1) = vcx.update(|_, cx| {
        let v = view.read(cx);
        (
            v.rows()[0].other_user,
            v.end_task_tooltip(0, cx).map(|t| t.to_string()),
            v.end_task_tooltip(1, cx).map(|t| t.to_string()),
        )
    });
    assert!(other);
    assert_eq!(tip0.as_deref(), Some("Needs administrator rights"));
    assert_eq!(
        tip1.as_deref(),
        Some("Ends immediately, unsaved work is lost")
    );

    // The disabled button sends nothing.
    click(&mut vcx, "LowMemEndTask-0");
    assert!(actions(&engine).is_empty());

    // Its tooltip still shows on hover.
    let button = vcx.debug_bounds("LowMemEndTask-0").expect("button drawn");
    vcx.simulate_mouse_move(button.center(), None, Modifiers::none());
    vcx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    vcx.run_until_parked();
    draw(&mut vcx);
    assert!(vcx.debug_bounds("TlTooltip").is_some(), "tooltip shows");
}

#[gpui_kit::test]
fn lowmem_end_task_no_confirmation(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sweep(cx, &engine, Vec::new(), vec![proc(30, "leaky", 700)]);
    let (_, mut vcx, view) = open(cx, &engine);
    click(&mut vcx, "LowMemEndTask-0");

    let sent = actions(&engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, ActionKind::Signal(Signal::Kill));
    assert_eq!(
        sent[0].targets.as_slice(),
        [ActionTarget::Process(key(30, 300))]
    );
    assert!(!sent[0].privileged);
    // No dialog: the rescue window is the only window and nothing else drew over it.
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    draw(&mut vcx);
    assert!(vcx.debug_bounds("TlDialog").is_none());
    // The button shows loading while the request runs.
    assert!(vcx.read(|cx| view.read(cx).is_ending(RowId::Process(key(30, 300)))));
    assert!(vcx.debug_bounds("TlButton-spinner").is_some());

    // A second click while in flight sends nothing more.
    click(&mut vcx, "LowMemEndTask-0");
    assert_eq!(actions(&engine).len(), 1);

    answer(cx, &engine, &sent[0], ActionState::Succeeded);
    assert!(!vcx.read(|cx| view.read(cx).is_ending(RowId::Process(key(30, 300)))));
    assert_eq!(toasts(&mut vcx), ["Task ended"]);
    // The row goes with the next sweep.
    sweep(cx, &engine, Vec::new(), Vec::new());
    assert!(names(&mut vcx, &view).is_empty());
}

#[gpui_kit::test]
fn lowmem_end_task_app_uses_kill_group(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sweep(cx, &engine, vec![app(5, "Firefox", 12, 400)], Vec::new());
    let (_, mut vcx, _) = open(cx, &engine);
    click(&mut vcx, "LowMemEndTask-0");

    let sent = actions(&engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, ActionKind::KillGroup);
    assert_eq!(
        sent[0].targets.as_slice(),
        [ActionTarget::Cgroup(CgroupRef {
            rel_path: Arc::from("user.slice/app-Firefox.scope"),
            cgroup_id: 5,
        })]
    );
    assert!(!sent[0].privileged);
}

#[gpui_kit::test]
fn lowmem_end_task_failure_toast(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sweep(cx, &engine, Vec::new(), vec![proc(40, "stuck", 300)]);
    let (_, mut vcx, view) = open(cx, &engine);
    click(&mut vcx, "LowMemEndTask-0");
    let sent = actions(&engine);
    answer(
        cx,
        &engine,
        &sent[0],
        ActionState::Failed(ActionError::Os(5)),
    );
    assert_eq!(
        toasts(&mut vcx),
        ["End Task Failed. The system call failed (error 5)."]
    );
    assert!(!vcx.read(|cx| view.read(cx).is_ending(RowId::Process(key(40, 400)))));
}

#[gpui_kit::test]
fn lowmem_keyboard_delete_ends_highlighted(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sweep(
        cx,
        &engine,
        Vec::new(),
        vec![proc(50, "a", 300), proc(51, "b", 200), proc(52, "c", 100)],
    );
    let (_, mut vcx, view) = open(cx, &engine);
    focus_list(&mut vcx, &view);
    vcx.simulate_keystrokes("down down");
    vcx.run_until_parked();
    assert_eq!(vcx.read(|cx| view.read(cx).highlight()), Some(1));
    vcx.simulate_keystrokes("down up");
    assert_eq!(vcx.read(|cx| view.read(cx).highlight()), Some(1));

    vcx.simulate_keystrokes("delete");
    vcx.run_until_parked();
    let sent = actions(&engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].targets.as_slice(),
        [ActionTarget::Process(key(51, 510))]
    );

    // Enter on another row ends that one.
    vcx.simulate_keystrokes("up enter");
    vcx.run_until_parked();
    let sent = actions(&engine);
    assert_eq!(sent.len(), 2);
    assert_eq!(
        sent[1].targets.as_slice(),
        [ActionTarget::Process(key(50, 500))]
    );
}

/// Security regression (PROC-001): the request carries the key captured from the sweep, never a
/// pid looked up again, and an identity change kills nothing.
#[gpui_kit::test]
fn lowmem_identity_changed_not_killed(cx: &mut TestAppContext) {
    let engine = setup(cx);
    sweep(cx, &engine, Vec::new(), vec![proc(60, "editor", 300)]);
    let (_, mut vcx, view) = open(cx, &engine);
    click(&mut vcx, "LowMemEndTask-0");

    // The pid is reused by a new process before the engine checks the request.
    let mut reused = proc(60, "editor", 300);
    reused.key = key(60, 999);
    sweep(cx, &engine, Vec::new(), vec![reused]);
    let sent = actions(&engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].targets.as_slice(),
        [ActionTarget::Process(key(60, 600))]
    );
    answer(
        cx,
        &engine,
        &sent[0],
        ActionState::Rejected(RejectReason::IdentityChanged),
    );

    assert_eq!(actions(&engine).len(), 1, "nothing else sent");
    assert_eq!(
        toasts(&mut vcx),
        ["The selected process changed; wait for the list to update and try again."]
    );
    // The pointer still holds the order, so the gone process left and the new one waits. Once
    // the pointer leaves, the new process gets its own row.
    assert!(names(&mut vcx, &view).is_empty());
    draw(&mut vcx);
    let header = vcx.debug_bounds("LowMemHeaderLine").expect("header drawn");
    vcx.simulate_mouse_move(header.center(), None, Modifiers::none());
    vcx.run_until_parked();
    let ids: Vec<RowId> = vcx.read(|cx| view.read(cx).rows().iter().map(|r| r.id).collect());
    assert_eq!(ids, [RowId::Process(key(60, 999))]);
}
