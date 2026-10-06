//! Low-memory rescue window: frame, texts, filter, eased state, keys and the minimal init
//! (E32-009, UI-PWN-032, R-243). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use gpui_kit::base::Root;
use gpui_kit::{
    Bounds, DevicePixels, Focusable as _, Font, FontId, FontMetrics, FontRun, GlyphId, LineLayout,
    Modifiers, Pixels, PlatformTextSystem, RenderGlyphParams, Size, TestAppContext, TestDispatcher,
    TextRenderingMode, VisualTestContext, WindowHandle,
};
use tl_model::actions::CgroupRef;
use tl_model::actions::{ActionKind, LaunchWhat};
use tl_model::client::EngineClient;
use tl_model::command::{Command, KWinCommand};
use tl_model::ids::GroupKey;
use tl_model::process::{LowMemGroup, LowMemoryInfo};
use tl_model::settings::{Settings, UiState};
use tl_ui::app::{AppEnv, OpenHooks, UiExit, UiExitSlot, UiInit};
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::current;
use tl_ui::windows::low_memory::{LowMemoryWindow, init_low_memory};

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
    assert_eq!(names, ["Firefox", "Konsole"]);

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
    assert_eq!(names, ["Firefox"]);

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
