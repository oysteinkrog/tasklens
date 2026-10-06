//! Secondary window frame, confirmations and the window registry (E16-008, E16-007, E16-A2,
//! UI-FND-021, UI-SHL-090). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
    Styled as _, TestAppContext, TestDispatcher, VisualTestContext, Window, WindowDecorations,
    WindowHandle, WindowKind, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::settings::{Decorations, Settings, UiState};
use tl_model::validity::Validity;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::dialog::{DialogButton, DialogResult, TlDialog, TlDialogView};
use tl_ui::components::toast::{self, ToastKind};
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
        // Until the shell calls it from init.
        toast::init(cx);
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
    setup(cx, Settings::default());
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
    setup(cx, Settings::default());
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
        px(420.)
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
