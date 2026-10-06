//! Secondary window frame, confirmations and the window registry (E16-008, E16-007, E16-A2,
//! UI-FND-021, UI-SHL-090). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, WindowDecorations, WindowHandle,
    WindowKind, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::settings::{Decorations, Settings, UiState};
use tl_model::validity::Validity;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::toast::ToastKind;
use tl_ui::components::window_frame::{
    APP_ID, PIN_TOOLTIP_OFF, PIN_TOOLTIP_ON, PIN_TOOLTIP_UNSUPPORTED, PinLook, TlSecondaryWindow,
    TlWindowKind, TlWindowSpec, ToggleAlwaysOnTop, window_options,
};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::registry::WindowKey;

struct Body;

impl Render for Body {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("content")
    }
}

type Frame = TlSecondaryWindow<Body>;

/// `TasklensUi::init` with `settings` against a new `FakeEngine`.
fn setup(cx: &mut TestAppContext, settings: Settings) -> Arc<FakeEngine> {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    let (_tx, rx) = async_channel::bounded(4);
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
    engine
}

fn set_windows_validity(cx: &mut TestAppContext, engine: &FakeEngine, validity: Validity) {
    engine.publish_with(|s| {
        let windows = Arc::make_mut(&mut s.windows);
        windows.header.validity = validity;
        windows.header.version += 1;
    });
    cx.run_until_parked();
}

fn open(
    cx: &mut TestAppContext,
    spec: TlWindowSpec,
) -> (WindowHandle<Root>, VisualTestContext, Entity<Frame>) {
    let handle = cx
        .update(|cx| Frame::open(cx, spec, |_, cx| cx.new(|_| Body)))
        .expect("window opens");
    let frame = cx.update(|cx| Frame::find(handle, cx)).expect("frame view");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    (handle, vcx, frame)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
    let id = handle.window_id();
    cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
}

fn tool(title: &str) -> TlWindowSpec {
    TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        title.to_owned(),
        size(px(600.), px(400.)),
    )
}

#[gpui_kit::test]
fn tool_window_options(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let spec = tool("tasklens Settings").key(WindowKey::Settings);
    let options = cx.update(|cx| window_options(&spec, Decorations::Client, cx));
    assert_eq!(options.kind, WindowKind::Floating);
    assert_eq!(options.window_min_size, Some(size(px(480.), px(320.))));
    assert_eq!(options.window_decorations, Some(WindowDecorations::Client));
    assert!(options.is_resizable);
    assert_eq!(options.app_id.as_deref(), Some(APP_ID));
    let bounds = options.window_bounds.expect("bounds").get_bounds();
    assert_eq!(bounds.size, size(px(600.), px(400.)));

    let (_, mut vcx, frame) = open(cx, spec);
    assert!(vcx.read(|cx| frame.read(cx).has_title_bar()));
    let bar = vcx
        .debug_bounds("TlWindowTitleBar")
        .expect("title bar drawn");
    assert_eq!(bar.size.height, px(34.));
    let title = vcx.debug_bounds("TlWindowTitle").expect("title drawn");
    assert!(title.size.width > px(0.));
    assert_eq!(
        vcx.read(|cx| frame.read(cx).drawn_title()).as_deref(),
        Some("tasklens Settings")
    );
}

#[gpui_kit::test]
fn confirmation_is_a_modal_dialog(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let spec = TlWindowSpec::new(
        TlWindowKind::Confirmation,
        "tasklens",
        size(px(420.), px(160.)),
    );
    let options = cx.update(|cx| window_options(&spec, Decorations::Client, cx));
    assert_eq!(options.kind, WindowKind::Dialog);
    assert_eq!(options.window_min_size, Some(size(px(360.), px(120.))));
    assert!(!options.is_resizable);
}

#[gpui_kit::test]
fn run_window_fixed_size(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let spec = TlWindowSpec::new(TlWindowKind::Run, "tasklens Run", size(px(600.), px(400.)))
        .key(WindowKey::Run);
    let options = cx.update(|cx| window_options(&spec, Decorations::Client, cx));
    assert!(!options.is_resizable);
    assert_eq!(options.window_min_size, Some(size(px(480.), px(248.))));

    let (_, mut vcx, _) = open(cx, spec);
    let bounds = vcx.update(|window, _| window.bounds());
    assert_eq!(bounds.size, size(px(480.), px(248.)));
    assert!(!vcx.update(|window, _| window.is_resizable()));
}

#[gpui_kit::test]
fn pin_disabled_without_bridge(cx: &mut TestAppContext) {
    let engine = setup(cx, Settings::default());
    set_windows_validity(cx, &engine, Validity::Unsupported);
    let dispatched = record_toggles(cx);
    let (_, mut vcx, frame) = open(cx, tool("tasklens Settings").key(WindowKey::Settings));

    let look = vcx.read(|cx| frame.read(cx).pin_look(cx));
    assert_eq!(
        look,
        PinLook {
            on: false,
            enabled: false,
            tooltip: PIN_TOOLTIP_UNSUPPORTED,
        }
    );
    assert_eq!(PIN_TOOLTIP_UNSUPPORTED, "Not supported by this desktop");

    let pin = vcx.debug_bounds("TlWindowPin").expect("pin drawn");
    vcx.simulate_click(pin.center(), Modifiers::none());
    vcx.run_until_parked();
    assert!(!vcx.read(|cx| frame.read(cx).pinned()));
    assert!(dispatched.borrow().is_empty());
}

/// Every `ToggleAlwaysOnTop` that reaches the app.
fn record_toggles(cx: &mut TestAppContext) -> Rc<RefCell<Vec<ToggleAlwaysOnTop>>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    cx.update(|cx| {
        cx.on_action(move |a: &ToggleAlwaysOnTop, _| sink.borrow_mut().push(a.clone()));
    });
    seen
}

#[gpui_kit::test]
fn pin_dispatches_toggle(cx: &mut TestAppContext) {
    let engine = setup(cx, Settings::default());
    set_windows_validity(cx, &engine, Validity::Valid);
    let dispatched = record_toggles(cx);
    let (_, mut vcx, frame) = open(cx, tool("Settings").key(WindowKey::Settings));
    assert_eq!(
        vcx.read(|cx| frame.read(cx).pin_look(cx)).tooltip,
        PIN_TOOLTIP_OFF
    );

    let pin = vcx.debug_bounds("TlWindowPin").expect("pin drawn");
    assert_eq!(pin.size, size(px(34.), px(34.)));
    vcx.simulate_click(pin.center(), Modifiers::none());
    vcx.run_until_parked();

    assert_eq!(
        *dispatched.borrow(),
        [ToggleAlwaysOnTop {
            window: WindowKey::Settings,
            caption: "Settings".into(),
        }]
    );
    let look = vcx.read(|cx| frame.read(cx).pin_look(cx));
    assert_eq!(
        look,
        PinLook {
            on: true,
            enabled: true,
            tooltip: PIN_TOOLTIP_ON,
        }
    );
    assert_eq!(PIN_TOOLTIP_ON, "Always on top (on)");
}

#[gpui_kit::test]
fn escape_and_ctrl_w_close_tool_window(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    for key in ["escape", "ctrl-w"] {
        let (handle, mut vcx, _) = open(cx, tool("tasklens Settings").key(WindowKey::Settings));
        assert!(is_open(cx, handle));
        vcx.simulate_keystrokes(key);
        cx.run_until_parked();
        assert!(!is_open(cx, handle), "{key} did not close the window");
    }
}

#[gpui_kit::test]
fn server_decorations_pin_fallback(cx: &mut TestAppContext) {
    let mut settings = Settings::default();
    settings.appearance.decorations = Decorations::Server;
    setup(cx, settings);
    let spec = tool("tasklens Settings").key(WindowKey::Settings);
    let options = cx.update(|cx| window_options(&spec, Decorations::Server, cx));
    assert_eq!(options.window_decorations, Some(WindowDecorations::Server));

    let (_, mut vcx, frame) = open(cx, spec);
    assert!(!vcx.read(|cx| frame.read(cx).has_title_bar()));
    assert!(vcx.debug_bounds("TlWindowTitleBar").is_none());
    let content = vcx.debug_bounds("TlWindowContent").expect("content drawn");
    let pin = vcx.debug_bounds("TlWindowPin").expect("pin drawn");
    assert_eq!(pin.size, size(px(24.), px(24.)));
    // Inset sp4 (8 px at 100%) from the content's top-right corner.
    assert_eq!(pin.top(), content.top() + px(8.));
    assert_eq!(pin.right(), content.right() - px(8.));
}

#[gpui_kit::test]
fn secondary_window_has_own_toasts(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let (_, mut a, frame_a) = open(cx, tool("tasklens Settings").key(WindowKey::Settings));
    let (_, mut b, _) = open(cx, tool("tasklens About").key(WindowKey::About));

    let queue = a.read(|cx| frame_a.read(cx).toasts().clone());
    queue.update(&mut a, |q, cx| {
        q.push(ToastKind::Info, "Copied", None, None, cx)
    });
    a.run_until_parked();
    b.run_until_parked();

    assert_eq!(a.update(|window, cx| window.notifications(cx).len()), 1);
    assert_eq!(b.update(|window, cx| window.notifications(cx).len()), 0);
}

#[gpui_kit::test]
fn narrow_title_ends_with_ellipsis(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let long = "tasklens Properties of a process with a very long name that cannot fit into \
                the title bar of a narrow window at all";
    let spec = TlWindowSpec::new(TlWindowKind::Properties, long, size(px(320.), px(400.)));
    let (_, mut vcx, frame) = open(cx, spec);
    let drawn = vcx
        .read(|cx| frame.read(cx).drawn_title())
        .expect("title drawn");
    assert!(drawn.ends_with('…'), "drawn title {drawn:?}");
    assert!(drawn.starts_with("tasklens Prop"), "drawn title {drawn:?}");
}
