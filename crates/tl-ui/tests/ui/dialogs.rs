//! Secondary window frame, confirmations and the window registry (E16-008, E16-007, E16-A2,
//! UI-FND-021, UI-SHL-090). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, Modifiers,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, TestAppContext,
    TestDispatcher, VisualTestContext, Window, WindowDecorations, WindowHandle, WindowKind, div,
    px, size,
};
use tl_model::client::EngineClient;
use tl_model::settings::{Decorations, Settings, UiState};
use tl_model::validity::Validity;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::dialog::{DialogButton, DialogResult, TlDialog, TlDialogView};
use tl_ui::components::toast::{self, ToastKind};
use tl_ui::components::window_frame::{
    APP_ID, PIN_TOOLTIP_OFF, PIN_TOOLTIP_ON, PIN_TOOLTIP_UNSUPPORTED, PinLook, TlSecondaryWindow,
    TlWindowKind, TlWindowSpec, ToggleAlwaysOnTop, client_frame_extra, window_options,
};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::registry::{CapClass, GlobalRegistry, OpenOutcome, WindowKey, WindowRegistry};
use tl_ui::windows::frame::{PwnWindow, spec as pwn_spec};

use super::common::{is_open, window_count};

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
        // Until the shell calls it from init.
        toast::init(cx);
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
    engine
}

/// Settings with client decorations, for tests of the kit title bar. The default is server
/// decorations (D-035), which draw no title bar.
fn client_settings() -> Settings {
    let mut settings = Settings::default();
    settings.appearance.decorations = Decorations::Client;
    settings
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

fn tool(title: &str) -> TlWindowSpec {
    TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        title.to_owned(),
        size(px(600.), px(400.)),
    )
}

#[gpui_kit::test]
fn tool_window_options(cx: &mut TestAppContext) {
    setup(cx, client_settings());
    let spec = tool("tasklens Settings").key(WindowKey::Settings);
    let options = cx.update(|cx| window_options(&spec, Decorations::Client, cx));
    assert_eq!(options.kind, WindowKind::Floating);
    let extra = client_frame_extra(Decorations::Client);
    assert_eq!(
        options.window_min_size,
        Some(size(px(480.) + extra, px(320.) + extra))
    );
    assert_eq!(options.window_decorations, Some(WindowDecorations::Client));
    assert!(options.is_resizable);
    assert_eq!(options.app_id.as_deref(), Some(APP_ID));
    let bounds = options.window_bounds.expect("bounds").get_bounds();
    assert_eq!(bounds.size, size(px(600.) + extra, px(400.) + extra));

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

/// Process, app, service, startup and connection Properties all use `TlWindowKind::Properties`.
/// They are `Normal` so the main window's menu popups stack above them (tasklens-fut.25, D-031).
/// Tool and Run windows stay `Floating`.
#[gpui_kit::test]
fn properties_windows_are_normal_not_floating(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let spec = TlWindowSpec::new(
        TlWindowKind::Properties,
        "Process Properties",
        size(px(900.), px(640.)),
    );
    let options = cx.update(|cx| window_options(&spec, Decorations::Client, cx));
    assert_eq!(options.kind, WindowKind::Normal);
    assert_eq!(TlWindowKind::Properties.window_kind(), WindowKind::Normal);
    // The process and app Properties windows take their kind from the UI-PWN-001 table.
    for window in [PwnWindow::ProcessProperties, PwnWindow::AppProperties] {
        assert_eq!(
            pwn_spec(window).kind.window_kind(),
            WindowKind::Normal,
            "{window:?}"
        );
    }
    assert_eq!(
        pwn_spec(PwnWindow::Run).kind.window_kind(),
        WindowKind::Floating
    );
    assert_eq!(
        pwn_spec(PwnWindow::HandleSearch).kind.window_kind(),
        WindowKind::Floating
    );
    assert_eq!(
        pwn_spec(PwnWindow::SetPriority).kind.window_kind(),
        WindowKind::Dialog
    );
    assert_eq!(TlWindowKind::Run.window_kind(), WindowKind::Floating);
    assert_eq!(
        TlWindowKind::Tool { resizable: true }.window_kind(),
        WindowKind::Floating
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
    let extra = client_frame_extra(Decorations::Client);
    assert_eq!(
        options.window_min_size,
        Some(size(px(360.) + extra, px(120.) + extra))
    );
    assert!(!options.is_resizable);
    let bounds = options.window_bounds.expect("bounds").get_bounds();
    assert_eq!(bounds.size, size(px(420.) + extra, px(160.) + extra));
}

/// The kit's window border draws a 20 px shadow and a 1 px border inside a client-decorated
/// window on Linux, so the window opens 42 px larger on each axis and the visible size is the
/// spec size. Server decorations add nothing (tasklens-fut.23).
#[gpui_kit::test]
fn client_frame_extra_grows_the_window_not_the_spec(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    assert_eq!(client_frame_extra(Decorations::Server), px(0.));
    #[cfg(target_os = "linux")]
    assert_eq!(client_frame_extra(Decorations::Client), px(42.));
    let spec = TlWindowSpec::new(
        TlWindowKind::Properties,
        "Connection Properties",
        size(px(560.), px(520.)),
    );
    for (decorations, extra) in [
        (Decorations::Server, px(0.)),
        (Decorations::Client, client_frame_extra(Decorations::Client)),
    ] {
        let options = cx.update(|cx| window_options(&spec, decorations, cx));
        let bounds = options.window_bounds.expect("bounds").get_bounds();
        assert_eq!(bounds.size, size(px(560.) + extra, px(520.) + extra));
    }
}

#[gpui_kit::test]
fn run_window_fixed_size(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let spec = TlWindowSpec::new(TlWindowKind::Run, "tasklens Run", size(px(600.), px(400.)))
        .key(WindowKey::Run);
    let options = cx.update(|cx| window_options(&spec, Decorations::Client, cx));
    assert!(!options.is_resizable);
    let extra = client_frame_extra(Decorations::Client);
    assert_eq!(
        options.window_min_size,
        Some(size(px(480.) + extra, px(248.) + extra))
    );

    let (_, mut vcx, _) = open(cx, spec);
    let extra = client_frame_extra(Settings::default().appearance.decorations);
    let bounds = vcx.update(|window, _| window.bounds());
    assert_eq!(bounds.size, size(px(480.) + extra, px(248.) + extra));
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
    let engine = setup(cx, client_settings());
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
    draw(&mut a);
    draw(&mut b);

    assert!(a.debug_bounds("TlToast-a1-frame").is_some());
    assert!(b.debug_bounds("TlToast-a1-frame").is_none());
}

#[test]
fn narrow_title_ends_with_ellipsis() {
    // The default `TestAppContext` text system gives every glyph zero width, so use the real one.
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
    let cx = &mut cx;
    setup(cx, client_settings());
    let long = "tasklens Properties of a process with a very long name that cannot fit into \
                the title bar of a narrow window at all";
    let spec = TlWindowSpec::new(TlWindowKind::Properties, long, size(px(320.), px(400.)));
    let (_, vcx, frame) = open(cx, spec);
    let drawn = vcx
        .read(|cx| frame.read(cx).drawn_title())
        .expect("title drawn");
    assert!(drawn.ends_with('…'), "drawn title {drawn:?}");
    assert!(drawn.starts_with("tasklens Prop"), "drawn title {drawn:?}");
}

#[gpui_kit::test]
fn init_binds_frame_keys_before_any_open(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    // `TasklensUi::init` binds the frame keys; no secondary window has opened yet.
    let close = tl_ui::components::window_frame::CloseSecondaryWindow;
    let bound = cx.update(|cx| {
        cx.key_bindings()
            .borrow()
            .bindings_for_action(&close)
            .count()
    });
    assert_eq!(bound, 2, "Esc and Ctrl+W are bound by init");
    let (handle, mut vcx, _) = open(cx, tool("tasklens Settings").key(WindowKey::Settings));
    vcx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!is_open(cx, handle), "escape did not close the window");
}

// TlDialog (E16-007, R-104).

type DialogFrame = TlSecondaryWindow<TlDialogView>;
type Results = Rc<RefCell<Vec<DialogResult>>>;

/// A parent window whose content counts left clicks.
struct ClickBody {
    clicks: Rc<RefCell<u32>>,
}

impl Render for ClickBody {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div()
            .id("click-body")
            .debug_selector(|| "ClickBody".into())
            .size_full()
            .on_click(move |_, _, _| *clicks.borrow_mut() += 1)
    }
}

struct Opened {
    parent: VisualTestContext,
    parent_handle: WindowHandle<Root>,
    handle: WindowHandle<Root>,
    vcx: VisualTestContext,
    view: Entity<TlDialogView>,
    results: Results,
    clicks: Rc<RefCell<u32>>,
}

/// Opens a tool window as the parent, then `dialog` modal to it.
fn open_dialog(cx: &mut TestAppContext, dialog: TlDialog) -> Opened {
    let clicks = Rc::new(RefCell::new(0));
    let parent_handle = cx
        .update(|cx| {
            let clicks = clicks.clone();
            TlSecondaryWindow::open(cx, tool("tasklens Settings"), move |_, cx| {
                cx.new(|_| ClickBody { clicks })
            })
        })
        .expect("parent opens");
    let mut parent = VisualTestContext::from_window(parent_handle.into(), cx);
    parent.run_until_parked();
    draw(&mut parent);

    let before: Vec<_> = cx.update(|cx| cx.windows().iter().map(|w| w.window_id()).collect());
    let results: Results = Rc::default();
    let sink = results.clone();
    parent
        .update(|window, cx| {
            dialog.open(window, cx, move |result, _, _| {
                sink.borrow_mut().push(result)
            })
        })
        .expect("dialog opens");
    let any = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(&w.window_id()))
        .expect("dialog window");
    let handle = WindowHandle::<Root>::new(any.window_id());
    let frame = cx
        .update(|cx| DialogFrame::find(handle, cx))
        .expect("dialog frame");
    let view = cx.update(|cx| frame.read(cx).content().clone());
    let mut vcx = VisualTestContext::from_window(any, cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    Opened {
        parent,
        parent_handle,
        handle,
        vcx,
        view,
        results,
        clicks,
    }
}

fn focused(o: &mut Opened) -> Option<DialogButton> {
    let view = o.view.clone();
    o.vcx
        .update(|window, cx| view.read(cx).focused_button(window))
}

fn reset_confirm() -> TlDialog {
    TlDialog::confirm(
        "Reset all settings to their defaults? Themes and history are kept.",
        "Reset",
    )
    .destructive()
}

fn skip_confirm() -> TlDialog {
    TlDialog::confirm(
        "tasklens actions that change other users' processes, system services and network \
         connections will run without asking for a password. Continue?",
        "Skip prompts",
    )
}

#[gpui_kit::test]
fn destructive_focuses_cancel(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let mut o = open_dialog(cx, reset_confirm());
    assert_eq!(focused(&mut o), Some(DialogButton::Cancel));
    let title = o.vcx.read(|cx| {
        DialogFrame::find(o.handle, cx)
            .map(|f| f.read(cx).title().to_string())
            .unwrap_or_default()
    });
    assert_eq!(title, "tasklens");
    assert!(o.vcx.debug_bounds("TlDialogCancel").is_some());
    let cancel = o.vcx.debug_bounds("TlDialogCancel").expect("cancel");
    let action = o.vcx.debug_bounds("TlDialogAction").expect("action");
    assert!(
        cancel.right() <= action.left(),
        "Cancel sits left of the action"
    );
    assert_eq!(cancel.size.height, px(28.));
    assert!(cancel.size.width >= px(72.));

    let mut o = open_dialog(cx, skip_confirm());
    assert_eq!(focused(&mut o), Some(DialogButton::Action));
}

#[gpui_kit::test]
fn confirm_enter_runs_focused_button_escape_cancels(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    for (dialog, expected) in [
        (reset_confirm(), DialogResult::Cancelled),
        (skip_confirm(), DialogResult::Confirmed { checkbox: false }),
    ] {
        let mut o = open_dialog(cx, dialog);
        o.vcx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(*o.results.borrow(), [expected]);
        assert!(!is_open(cx, o.handle));
    }
    for dialog in [reset_confirm(), skip_confirm()] {
        let mut o = open_dialog(cx, dialog);
        o.vcx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(*o.results.borrow(), [DialogResult::Cancelled]);
        assert!(!is_open(cx, o.handle), "Esc closes the window");
    }
}

#[gpui_kit::test]
fn tab_then_enter_confirms_destructive(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let mut o = open_dialog(cx, reset_confirm());
    o.vcx.simulate_keystrokes("tab");
    o.vcx.run_until_parked();
    draw(&mut o.vcx);
    assert_eq!(focused(&mut o), Some(DialogButton::Action));
    o.vcx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        *o.results.borrow(),
        [DialogResult::Confirmed { checkbox: false }]
    );
    assert!(!is_open(cx, o.handle));
}

#[gpui_kit::test]
fn checkbox_comes_first_and_reports_its_value(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let mut o = open_dialog(cx, skip_confirm().checkbox("Do not ask again", true));
    // Tab order: checkbox, Cancel, action, then the pin.
    o.vcx.simulate_keystrokes("shift-tab shift-tab");
    o.vcx.run_until_parked();
    assert_eq!(focused(&mut o), None, "the checkbox has focus");
    o.vcx.simulate_keystrokes("tab");
    assert_eq!(focused(&mut o), Some(DialogButton::Cancel));
    o.vcx.simulate_keystrokes("tab");
    assert_eq!(focused(&mut o), Some(DialogButton::Action));
    o.vcx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        *o.results.borrow(),
        [DialogResult::Confirmed { checkbox: true }]
    );
}

#[gpui_kit::test]
fn result_called_once_on_window_close(cx: &mut TestAppContext) {
    setup(cx, client_settings());
    let mut o = open_dialog(cx, skip_confirm());
    let close = o
        .vcx
        .debug_bounds("TlWindowCaption-close")
        .expect("close button");
    o.vcx.simulate_click(close.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(!is_open(cx, o.handle));
    assert_eq!(*o.results.borrow(), [DialogResult::Cancelled]);

    // A click on a footer button closes the window too; the release must not report again.
    let mut o = open_dialog(cx, skip_confirm());
    let action = o.vcx.debug_bounds("TlDialogAction").expect("action");
    o.vcx.simulate_click(action.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(!is_open(cx, o.handle));
    assert_eq!(
        *o.results.borrow(),
        [DialogResult::Confirmed { checkbox: false }]
    );
}

#[gpui_kit::test]
fn parent_blocked_while_open(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let mut o = open_dialog(cx, reset_confirm());
    draw(&mut o.parent);
    assert!(o.parent.debug_bounds("TlWindowModalOverlay").is_some());
    let body = o.parent.debug_bounds("ClickBody").expect("parent content");
    o.parent.simulate_click(body.center(), Modifiers::none());
    o.parent.run_until_parked();
    assert_eq!(*o.clicks.borrow(), 0, "the parent ignores the click");

    o.vcx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(is_open(cx, o.parent_handle));
    draw(&mut o.parent);
    assert!(o.parent.debug_bounds("TlWindowModalOverlay").is_none());
    o.parent.simulate_click(body.center(), Modifiers::none());
    o.parent.run_until_parked();
    assert_eq!(*o.clicks.borrow(), 1);
}

#[test]
fn heading_and_long_text_fit() {
    // The default `TestAppContext` text system gives every glyph zero width, so use the real one.
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
    let cx = &mut cx;
    setup(cx, Settings::default());
    let long = "Ending this process will end every process under it as well. ".repeat(20);
    let mut o = open_dialog(
        cx,
        TlDialog::confirm(long, "End tree").heading("End process tree?"),
    );
    o.vcx.run_until_parked();
    draw(&mut o.vcx);

    let heading = o.vcx.debug_bounds("TlDialogHeading").expect("heading");
    let text = o.vcx.debug_bounds("TlDialogText").expect("text");
    // t_md: 14 px at line height 1.3, one line.
    assert!(
        (f32::from(heading.size.height) - 18.2).abs() < 0.5,
        "{heading:?}"
    );
    assert!(heading.bottom() <= text.top());
    // t_base: 13 px at line height 1.3; at most 6 lines.
    let lines = f32::from(text.size.height) / (13. * 1.3);
    assert!(lines > 5.5 && lines < 6.5, "{lines} lines");
    let height = o.vcx.update(|window, _| window.bounds().size.height);
    assert!(height >= px(140.) && height <= px(320.), "{height:?}");
    assert_eq!(
        o.vcx.update(|window, _| window.bounds().size.width),
        px(420.) + client_frame_extra(Settings::default().appearance.decorations)
    );
}

#[gpui_kit::test]
fn short_dialog_keeps_min_height(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let mut o = open_dialog(
        cx,
        TlDialog::confirm("Delete this custom theme?", "Delete").destructive(),
    );
    o.vcx.run_until_parked();
    draw(&mut o.vcx);
    let height = o.vcx.update(|window, _| window.bounds().size.height);
    assert!(height >= px(140.) && height <= px(320.), "{height:?}");
    assert!(o.vcx.debug_bounds("TlDialogIcon").is_some());
    assert!(o.vcx.debug_bounds("TlDialogHeading").is_none());
}

// WindowRegistry (E16-A2, UI-FND-021, UI-SHL-090, APP-045, R-218).

fn registry(cx: &mut TestAppContext) -> Entity<WindowRegistry> {
    cx.update(|cx| cx.global::<GlobalRegistry>().0.clone())
}

/// `open_or_focus(key)` with a frame window titled after the key. Counts calls to the open
/// closure in `opens`.
fn open_key(cx: &mut TestAppContext, key: WindowKey, opens: &Rc<RefCell<u32>>) -> OpenOutcome {
    let reg = registry(cx);
    let opens = opens.clone();
    let spec = tool(&format!("{key:?}")).key(key.clone());
    let outcome = cx.update(|cx| {
        reg.update(cx, |r, cx| {
            r.open_or_focus(key, cx, move |cx| {
                *opens.borrow_mut() += 1;
                Frame::open(cx, spec, |_, cx| cx.new(|_| Body)).map(Into::into)
            })
        })
    });
    cx.run_until_parked();
    outcome
}

fn process_key(pid: u32) -> tl_model::ids::ProcessKey {
    tl_model::ids::ProcessKey {
        start_ticks: 100,
        boot_id: tl_model::time::BootId(1),
        pid,
    }
}

fn unit(name: &str) -> tl_model::table::UnitRef {
    tl_model::table::UnitRef {
        manager: tl_model::table::Manager::System,
        name: name.into(),
    }
}

#[gpui_kit::test]
fn secondary_window_refocus(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let opens = Rc::new(RefCell::new(0));
    assert_eq!(open_key(cx, WindowKey::About, &opens), OpenOutcome::Opened);
    let windows = window_count(cx);
    assert_eq!(open_key(cx, WindowKey::About, &opens), OpenOutcome::Focused);
    assert_eq!(*opens.borrow(), 1, "the open closure ran again");
    assert_eq!(window_count(cx), windows, "a second About window opened");
}

#[gpui_kit::test]
fn ninth_properties_refused(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let opens = Rc::new(RefCell::new(0));
    for pid in 1..=8 {
        let key = WindowKey::Properties(process_key(pid));
        assert_eq!(open_key(cx, key, &opens), OpenOutcome::Opened);
    }
    let windows = window_count(cx);
    let ninth = WindowKey::Properties(process_key(9));
    assert_eq!(
        open_key(cx, ninth, &opens),
        OpenOutcome::CapReached(CapClass::ProcessProps)
    );
    assert_eq!(*opens.borrow(), 8);
    assert_eq!(window_count(cx), windows);
    let reg = registry(cx);
    assert_eq!(cx.read(|cx| reg.read(cx).count(CapClass::ProcessProps)), 8);
}

#[gpui_kit::test]
fn service_cap_is_10(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let opens = Rc::new(RefCell::new(0));
    for i in 0..10 {
        let key = WindowKey::Service(unit(&format!("u{i}.service")));
        assert_eq!(open_key(cx, key, &opens), OpenOutcome::Opened);
    }
    let eleventh = WindowKey::Service(unit("u10.service"));
    assert_eq!(
        open_key(cx, eleventh, &opens),
        OpenOutcome::CapReached(CapClass::ServiceProps)
    );
    assert_eq!(*opens.borrow(), 10);
}

#[gpui_kit::test]
fn closed_window_deregisters(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let opens = Rc::new(RefCell::new(0));
    let first = WindowKey::Properties(process_key(1));
    open_key(cx, first.clone(), &opens);
    open_key(cx, WindowKey::Properties(process_key(2)), &opens);
    let reg = registry(cx);
    assert_eq!(cx.read(|cx| reg.read(cx).count(CapClass::ProcessProps)), 2);

    // The user presses Esc in the first window.
    let handle = cx.read(|cx| reg.read(cx).get(&first)).expect("registered");
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    vcx.simulate_keystrokes("escape");
    cx.run_until_parked();

    assert!(cx.read(|cx| reg.read(cx).get(&first)).is_none());
    assert_eq!(cx.read(|cx| reg.read(cx).count(CapClass::ProcessProps)), 1);
}

#[gpui_kit::test]
fn main_close_closes_children(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let reg = registry(cx);
    let opens = Rc::new(RefCell::new(0));
    for keep_detached in [true, false] {
        let props = WindowKey::Properties(process_key(1));
        let graph = WindowKey::DetachedGraph(0);
        open_key(cx, WindowKey::Settings, &opens);
        open_key(cx, props.clone(), &opens);
        open_key(cx, graph.clone(), &opens);
        let handles: Vec<_> = [WindowKey::Settings, props.clone(), graph.clone()]
            .iter()
            .map(|k| cx.read(|cx| reg.read(cx).get(k)).expect("registered"))
            .collect();
        cx.update(|cx| reg.update(cx, |r, cx| r.close_children_of_main(keep_detached, cx)));
        cx.run_until_parked();

        let open_ids: Vec<_> = cx.update(|cx| cx.windows().iter().map(|w| w.window_id()).collect());
        assert!(
            !open_ids.contains(&handles[0].window_id()),
            "Settings stayed open"
        );
        assert!(
            !open_ids.contains(&handles[1].window_id()),
            "Properties stayed open"
        );
        assert_eq!(
            open_ids.contains(&handles[2].window_id()),
            keep_detached,
            "detached graph with keep_detached = {keep_detached}"
        );
        assert_eq!(
            cx.read(|cx| reg.read(cx).get(&graph).is_some()),
            keep_detached
        );
        assert!(!cx.read(|cx| reg.read(cx).has_key(|k| *k != graph)));
        if keep_detached {
            cx.update(|cx| reg.update(cx, |r, cx| r.close(&graph, cx)));
            cx.run_until_parked();
        }
    }
}

#[gpui_kit::test]
fn registry_has_key_and_count_where(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let opens = Rc::new(RefCell::new(0));
    open_key(cx, WindowKey::DetachedGraph(0), &opens);
    open_key(cx, WindowKey::About, &opens);
    let reg = registry(cx);
    cx.read(|cx| {
        let r = reg.read(cx);
        assert!(r.has_key(|k| matches!(k, WindowKey::DetachedGraph(_))));
        assert_eq!(r.count_where(|_| true), 2);
        assert!(!r.has_key(|k| matches!(k, WindowKey::Settings)));
    });
}

#[gpui_kit::test]
fn registry_notifies_on_insert_and_remove(cx: &mut TestAppContext) {
    setup(cx, Settings::default());
    let reg = registry(cx);
    let notes = Rc::new(RefCell::new(0));
    let n = notes.clone();
    let _sub = cx.update(|cx| cx.observe(&reg, move |_, _| *n.borrow_mut() += 1));
    let opens = Rc::new(RefCell::new(0));
    open_key(cx, WindowKey::About, &opens);
    assert_eq!(*notes.borrow(), 1, "after open");

    // Closed by the user, not through the registry.
    let handle = cx
        .read(|cx| reg.read(cx).get(&WindowKey::About))
        .expect("registered");
    cx.update(|cx| handle.update(cx, |_, window, _| window.remove_window()))
        .expect("window open");
    cx.run_until_parked();
    assert_eq!(*notes.borrow(), 2, "after close");
    assert!(cx.read(|cx| reg.read(cx).get(&WindowKey::About)).is_none());
}
