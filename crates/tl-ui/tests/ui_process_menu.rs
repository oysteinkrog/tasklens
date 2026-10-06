//! Process-action confirmations (UI-PRC-014, UI-PWN-025). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _,
    Render, SharedString, Styled as _, TestAppContext, VisualTestContext, Window, WindowHandle,
    div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::settings::{Settings, UiState};
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui::components::dialog::{DialogButton, TlDialogView};
use tl_ui::components::toast;
use tl_ui::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use tl_ui::i18n::init_strings;
use tl_ui::pages::processes::confirm::{ConfirmKind, ConfirmResult, ConfirmTexts, confirm, texts};
use tl_ui::testing::fake_engine::FakeEngine;

struct Body;

impl Render for Body {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("content")
    }
}

type DialogFrame = TlSecondaryWindow<TlDialogView>;
type Results = Rc<RefCell<Vec<ConfirmResult>>>;

fn setup(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine;
    let (_tx, rx) = async_channel::bounded(4);
    let init = UiInit {
        settings: Settings::default(),
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
        toast::init(cx);
        init_strings(cx, "en-US");
        cx.set_reduce_motion(true);
    });
    cx.run_until_parked();
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn is_open(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> bool {
    let id = handle.window_id();
    cx.update(|cx| cx.windows().iter().any(|w| w.window_id() == id))
}

struct Opened {
    handle: WindowHandle<Root>,
    vcx: VisualTestContext,
    view: Entity<TlDialogView>,
    results: Results,
}

/// Opens a tool window as the parent, then the confirmation for `kind` modal to it.
fn open_confirm(cx: &mut TestAppContext, kind: ConfirmKind) -> Opened {
    let parent_handle = cx
        .update(|cx| {
            let spec = TlWindowSpec::new(
                TlWindowKind::Tool { resizable: true },
                "tasklens Properties",
                size(px(600.), px(400.)),
            );
            TlSecondaryWindow::open(cx, spec, |_, cx| cx.new(|_| Body))
        })
        .expect("parent opens");
    let parent: AnyWindowHandle = parent_handle.into();
    let mut pcx = VisualTestContext::from_window(parent, cx);
    pcx.run_until_parked();
    draw(&mut pcx);

    let before: Vec<_> = cx.update(|cx| cx.windows().iter().map(|w| w.window_id()).collect());
    let results: Results = Rc::default();
    let sink = results.clone();
    cx.update(|cx| {
        confirm(kind, parent, cx, move |result, _| {
            sink.borrow_mut().push(result)
        })
    });
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
        handle,
        vcx,
        view,
        results,
    }
}

fn focused(o: &mut Opened) -> Option<DialogButton> {
    let view = o.view.clone();
    o.vcx
        .update(|window, cx| view.read(cx).focused_button(window))
}

fn s(text: &str) -> SharedString {
    SharedString::from(text.to_owned())
}

fn expect(
    title: &str,
    description: &str,
    label: &str,
    danger: bool,
    second: Option<(&str, bool)>,
) -> ConfirmTexts {
    ConfirmTexts {
        title: s(title),
        description: s(description),
        confirm_label: s(label),
        confirm_danger: danger,
        second: second.map(|(l, d)| (s(l), d)),
    }
}

#[gpui_kit::test]
fn confirm_text_per_trigger(cx: &mut TestAppContext) {
    setup(cx);
    let cases = [
        (
            ConfirmKind::Protected {
                action: s("End"),
                name: s("plasmashell"),
                tree_count: None,
            },
            expect(
                "End plasmashell?",
                "Ending plasmashell will end your desktop session. End it anyway?",
                "End anyway",
                true,
                None,
            ),
        ),
        (
            ConfirmKind::EndTree { count: 7 },
            expect(
                "End process tree?",
                "End 7 processes in this tree?",
                "End tree",
                true,
                None,
            ),
        ),
        (
            ConfirmKind::KillApp {
                app: s("Firefox"),
                count: 12,
            },
            expect(
                "Kill Firefox?",
                "Kill all 12 processes of Firefox?",
                "Kill app",
                true,
                None,
            ),
        ),
        (
            ConfirmKind::EndMany {
                count: 3,
                kill: false,
            },
            expect(
                "End 3 processes?",
                "End 3 selected processes?",
                "End tasks",
                true,
                None,
            ),
        ),
        (
            ConfirmKind::EndMany {
                count: 3,
                kill: true,
            },
            expect(
                "End 3 processes?",
                "End 3 selected processes immediately?",
                "End processes",
                true,
                None,
            ),
        ),
        (
            ConfirmKind::OtherUser {
                action: s("End task"),
                name: s("sshd"),
                pid: 812,
                user: s("root"),
                destructive: true,
            },
            expect(
                "Act on another user's process?",
                "End task on sshd (PID 812), owned by root. Continue?",
                "End task",
                true,
                None,
            ),
        ),
        (
            ConfirmKind::OtherUser {
                action: s("Suspend"),
                name: s("sshd"),
                pid: 812,
                user: s("root"),
                destructive: false,
            },
            expect(
                "Act on another user's process?",
                "Suspend on sshd (PID 812), owned by root. Continue?",
                "Suspend",
                false,
                None,
            ),
        ),
        (
            ConfirmKind::Realtime,
            expect(
                "Set realtime priority?",
                "Realtime priority can freeze the desktop if the process spins. Set it anyway?",
                "Set realtime",
                true,
                None,
            ),
        ),
        (
            ConfirmKind::SignalMany {
                count: 4,
                signal_name: s("TERM"),
                ends_process: true,
            },
            expect(
                "Send signal to 4 processes?",
                "Send SIGTERM to 4 selected processes?",
                "Send signal",
                true,
                None,
            ),
        ),
        (
            ConfirmKind::SignalMany {
                count: 4,
                signal_name: s("STOP"),
                ends_process: false,
            },
            expect(
                "Send signal to 4 processes?",
                "Send SIGSTOP to 4 selected processes?",
                "Send signal",
                false,
                None,
            ),
        ),
        (
            ConfirmKind::LargeDump {
                name: s("firefox"),
                size: s("3.4 GiB"),
            },
            expect(
                "Create a large dump?",
                "firefox uses 3.4 GiB. The process pauses while the core is written and the \
                 file will be about that size. Continue?",
                "Create dump",
                false,
                None,
            ),
        ),
        (
            ConfirmKind::VirusTotalFirstUse,
            expect(
                "Open VirusTotal?",
                "This opens virustotal.com with the file's hash. The file is not uploaded.",
                "Open",
                false,
                None,
            ),
        ),
        (
            ConfirmKind::Picked {
                name: s("kate"),
                pid: 4242,
            },
            expect(
                "Picked kate (PID 4242)",
                "What do you want to do with this process?",
                "Select",
                false,
                Some(("Terminate", true)),
            ),
        ),
    ];
    for (kind, want) in cases {
        let got = cx.update(|cx| texts(&kind, cx));
        assert_eq!(got, want, "{kind:?}");
    }
}

#[gpui_kit::test]
fn protected_wins_over_tree_text(cx: &mut TestAppContext) {
    setup(cx);
    let kind = ConfirmKind::Protected {
        action: s("End process tree"),
        name: s("plasmashell"),
        tree_count: Some(7),
    };
    let got = cx.update(|cx| texts(&kind, cx));
    assert_eq!(got.title.as_ref(), "End plasmashell?");
    assert_eq!(
        got.description.as_ref(),
        "Ending plasmashell will end your desktop session. End 7 processes in this tree anyway?"
    );
    assert_eq!(got.confirm_label.as_ref(), "End anyway");
}

#[gpui_kit::test]
fn confirm_focus_starts_on_cancel_enter_activates_focused(cx: &mut TestAppContext) {
    setup(cx);
    let mut o = open_confirm(cx, ConfirmKind::EndTree { count: 7 });
    let title = o.vcx.read(|cx| {
        DialogFrame::find(o.handle, cx)
            .map(|f| f.read(cx).title().to_string())
            .unwrap_or_default()
    });
    assert_eq!(title, "tasklens");
    assert_eq!(focused(&mut o), Some(DialogButton::Cancel));
    o.vcx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(*o.results.borrow(), [ConfirmResult::Cancelled]);
    assert!(!is_open(cx, o.handle));

    let mut o = open_confirm(cx, ConfirmKind::EndTree { count: 7 });
    o.vcx.simulate_keystrokes("tab");
    o.vcx.run_until_parked();
    draw(&mut o.vcx);
    assert_eq!(focused(&mut o), Some(DialogButton::Action));
    o.vcx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(*o.results.borrow(), [ConfirmResult::Confirmed]);
    assert!(!is_open(cx, o.handle));

    // Esc cancels every kind and closes the window.
    for kind in [
        ConfirmKind::EndTree { count: 7 },
        ConfirmKind::VirusTotalFirstUse,
        ConfirmKind::Picked {
            name: s("kate"),
            pid: 4242,
        },
    ] {
        let mut o = open_confirm(cx, kind);
        o.vcx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(*o.results.borrow(), [ConfirmResult::Cancelled]);
        assert!(!is_open(cx, o.handle), "Esc closes the window");
    }
}

#[gpui_kit::test]
fn non_destructive_focus_on_action(cx: &mut TestAppContext) {
    setup(cx);
    let mut o = open_confirm(cx, ConfirmKind::VirusTotalFirstUse);
    assert_eq!(focused(&mut o), Some(DialogButton::Action));
    o.vcx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(*o.results.borrow(), [ConfirmResult::Confirmed]);
}

#[gpui_kit::test]
fn picked_second_button_terminate(cx: &mut TestAppContext) {
    setup(cx);
    let mut o = open_confirm(
        cx,
        ConfirmKind::Picked {
            name: s("kate"),
            pid: 4242,
        },
    );
    // Select is the default; Terminate never has focus on open.
    assert_eq!(focused(&mut o), Some(DialogButton::Action));
    let terminate = o
        .vcx
        .debug_bounds("TlDialogSecond")
        .expect("Terminate button");
    let select = o.vcx.debug_bounds("TlDialogAction").expect("Select button");
    assert!(terminate.right() <= select.left(), "Select is rightmost");
    o.vcx.simulate_click(terminate.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(*o.results.borrow(), [ConfirmResult::Second]);
    assert!(!is_open(cx, o.handle));

    // Enter runs Select, the focused default.
    let mut o = open_confirm(
        cx,
        ConfirmKind::Picked {
            name: s("kate"),
            pid: 4242,
        },
    );
    o.vcx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(*o.results.borrow(), [ConfirmResult::Confirmed]);
}
