//! Shell secondary windows: Share Theme (E21-030, UI-SHL-082). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, WindowHandle, div,
};
use tl_model::client::EngineClient;
use tl_model::settings::{Settings, UiState};
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind, has_modal_child};
use tl_ui::i18n::{set_test_catalogue, tr_calls};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::share_code::{decode_tl1, encode_tl1};
use tl_ui::theme::user_store::serialize_user_themes;
use tl_ui::theme::{ThemeDef, ThemeStore, builtin};
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
    let client: Arc<dyn EngineClient> = FakeEngine::new();
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
