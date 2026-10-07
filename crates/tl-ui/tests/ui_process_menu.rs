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

/// Process action toasts (tasklens-pdu.19, UI-PRC-015, PROC-059).
mod toasts {
    use std::sync::Arc;

    use gpui_kit::{SharedString, TestAppContext, VisualTestContext};
    use tl_model::actions::{
        ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
        IoPriority, RejectReason, Signal,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::ids::{NodeId, ProcessKey};
    use tl_model::process::ProcFlags;
    use tl_model::settings::{Settings, UiState};
    use tl_model::system::CpuSet;
    use tl_model::time::BootId;
    use tl_ui::action_tracker::{GlobalActionTracker, PushedToast};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::toast::{ToastKey, ToastKind};
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::actions::{CapturedTarget, CapturedTargets};
    use tl_ui::pages::processes::toasts::{
        ActionToastTexts, exited_toast, refusal_reason, refusal_text, refuse, submit, texts_for,
    };
    use tl_ui::table::tree_table::RowKind;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window_on};

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 1_000 + u64::from(pid),
            boot_id: BootId(1),
            pid,
        }
    }

    fn target(pid: u32, flags: ProcFlags) -> CapturedTarget {
        CapturedTarget {
            node: NodeId::Process(key(pid)),
            kind: RowKind::Process,
            name: "proc".into(),
            pid: Some(pid),
            uid: 1000,
            user: "me".into(),
            flags,
            member_count: 1,
        }
    }

    fn targets(n: u32) -> CapturedTargets {
        CapturedTargets::new((0..n).map(|i| target(100 + i, ProcFlags::empty())), None)
    }

    fn texts(kind: ActionKind, n: u32) -> ActionToastTexts {
        texts_for(&kind, &targets(n)).expect("process kinds have texts")
    }

    fn pair(kind: ToastKind, text: &str) -> (ToastKind, SharedString) {
        (kind, SharedString::from(text.to_owned()))
    }

    fn partial(
        t: &ActionToastTexts,
        done: u32,
        total: u32,
        failed: &[(u32, &str)],
    ) -> (ToastKind, SharedString) {
        let failed: Vec<(u32, SharedString)> = failed
            .iter()
            .map(|(n, r)| (*n, SharedString::from((*r).to_owned())))
            .collect();
        (t.partial.as_ref().expect("a partial text"))(done, total, &failed)
    }

    const EPERM: ActionState = ActionState::Failed(ActionError::PermissionDenied);

    #[test]
    fn toast_texts_match_spec() {
        use ToastKind::*;
        let one = |kind: ActionKind| {
            let t = texts(kind, 1);
            (t.requested, t.success, t.failure, t.partial.is_some())
        };
        let cases = [
            (
                ActionKind::Signal(Signal::Term),
                Some(pair(Info, "End task requested.")),
                pair(Success, "Process terminated"),
                pair(Error, "Could not end task."),
            ),
            (
                ActionKind::Signal(Signal::Kill),
                Some(pair(Info, "End process requested.")),
                pair(Success, "Process terminated"),
                pair(Error, "Could not end process."),
            ),
            (
                ActionKind::KillTree,
                Some(pair(Info, "End process tree requested.")),
                pair(Success, "Process tree terminated"),
                pair(Error, "Could not end the process tree."),
            ),
            (
                ActionKind::Suspend,
                None,
                pair(Success, "Process suspended."),
                pair(Error, "Could not suspend the process."),
            ),
            (
                ActionKind::Resume,
                None,
                pair(Success, "Process resumed."),
                pair(Error, "Could not resume the process."),
            ),
            (
                ActionKind::FreezeGroup,
                None,
                pair(Success, "App frozen"),
                pair(Error, "Could not freeze the app."),
            ),
            (
                ActionKind::ThawGroup,
                None,
                pair(Success, "App thawed"),
                pair(Error, "Could not thaw the app."),
            ),
            (
                ActionKind::KillGroup,
                None,
                pair(Success, "App killed"),
                pair(Error, "Could not kill the app."),
            ),
            (
                ActionKind::SetNice(5),
                None,
                pair(Success, "Priority updated"),
                pair(Error, "Could not update selected process priority."),
            ),
            (
                ActionKind::SetAffinity(CpuSet::default()),
                None,
                pair(Success, "Affinity updated"),
                pair(Error, "Could not update selected process affinity."),
            ),
            (
                ActionKind::SetIoPriority(IoPriority::Idle),
                None,
                pair(Success, "I/O priority updated"),
                pair(Error, "Could not update selected process I/O priority."),
            ),
        ];
        for (kind, requested, success, failure) in cases {
            let tree = kind == ActionKind::KillTree;
            assert_eq!(
                one(kind.clone()),
                (requested, success, failure, tree),
                "{kind:?}"
            );
        }

        // Several targets.
        let end = texts(ActionKind::Signal(Signal::Term), 5);
        assert_eq!(
            end.requested,
            Some(pair(Info, "End task requested for selected processes."))
        );
        assert_eq!(end.success, pair(Success, "Ended 5 of 5 processes"));
        assert_eq!(
            partial(&end, 4, 5, &[(1, "permission denied")]),
            pair(Warning, "Ended 4 of 5 processes (1 permission denied)")
        );
        assert_eq!(
            partial(
                &end,
                2,
                5,
                &[
                    (2, "permission denied"),
                    (1, "the process has already exited")
                ]
            ),
            pair(
                Warning,
                "Ended 2 of 5 processes (2 permission denied, 1 the process has already exited)"
            )
        );
        let kill = texts(ActionKind::Signal(Signal::Kill), 2);
        assert_eq!(
            kill.requested,
            Some(pair(Info, "End task requested for selected processes."))
        );
        let tree = texts(ActionKind::KillTree, 1);
        assert_eq!(
            partial(&tree, 4, 5, &[(1, "permission denied")]),
            pair(Warning, "Ended 4 of 5 processes (1 permission denied)")
        );

        let settings = [
            (
                ActionKind::SetNice(5),
                "Selected process priority updated.",
                "Could not update selected process priority.",
                "Priority updated for 3 of 4 processes. 1 failed: permission denied",
            ),
            (
                ActionKind::SetAffinity(CpuSet::default()),
                "Selected process affinity updated.",
                "Could not update selected process affinity.",
                "Affinity updated for 3 of 4 processes. 1 failed: permission denied",
            ),
            (
                ActionKind::SetIoPriority(IoPriority::Idle),
                "Selected process I/O priority updated.",
                "Could not update selected process I/O priority.",
                "I/O priority updated for 3 of 4 processes. 1 failed: permission denied",
            ),
        ];
        for (kind, all_ok, all_failed, mixed) in settings {
            let t = texts(kind.clone(), 4);
            assert_eq!(t.requested, None, "{kind:?}");
            assert_eq!(t.success, pair(Success, all_ok), "{kind:?}");
            assert_eq!(
                partial(&t, 0, 4, &[(4, "permission denied")]),
                pair(Error, all_failed),
                "{kind:?}"
            );
            assert_eq!(
                partial(&t, 3, 4, &[(1, "permission denied")]),
                pair(Warning, mixed),
                "{kind:?}"
            );
        }
        let t = texts(ActionKind::SetNice(5), 4);
        assert_eq!(
            partial(
                &t,
                1,
                4,
                &[
                    (2, "permission denied"),
                    (1, "the process has already exited")
                ]
            )
            .1,
            "Priority updated for 1 of 4 processes. 3 failed: permission denied, the process has \
             already exited"
        );

        // Refusals and the exited row.
        let me = target(4242, ProcFlags::IS_SELF | ProcFlags::PROTECTED);
        let init = target(1, ProcFlags::PROTECTED);
        let kthread = CapturedTarget {
            kind: RowKind::KernelThread,
            ..target(2, ProcFlags::KERNEL_THREAD)
        };
        let plasma = target(900, ProcFlags::PROTECTED);
        let refused = |t: &CapturedTarget| {
            refusal_reason(t)
                .and_then(|r| refusal_text(r, t))
                .map(|s| s.to_string())
        };
        assert_eq!(
            refused(&me).as_deref(),
            Some("Refusing to end tasklens itself.")
        );
        assert_eq!(
            refused(&init).as_deref(),
            Some("PID 1 (systemd) cannot be ended.")
        );
        assert_eq!(
            refused(&kthread).as_deref(),
            Some("Kernel threads cannot be signalled.")
        );
        assert_eq!(
            refused(&plasma),
            None,
            "other protected targets get a dialog"
        );
        assert_eq!(refusal_text(RejectReason::Busy, &me), None);
        assert_eq!(exited_toast(), pair(Warning, "Process exited"));

        assert!(texts_for(&ActionKind::UnitStart, &targets(1)).is_none());
    }

    /// The real main window on the Processes tab, with the app's tracker.
    fn open(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
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
            init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Processes, cx))
            .expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        (engine, vcx)
    }

    fn request(kind: ActionKind, n: u32) -> ActionRequest {
        ActionRequest {
            id: ActionId(0),
            kind,
            targets: (0..n)
                .map(|i| ActionTarget::Process(key(100 + i)))
                .collect(),
            privileged: false,
        }
    }

    fn send(vcx: &mut VisualTestContext, kind: ActionKind, n: u32) -> ActionId {
        let texts = texts(kind.clone(), n);
        let req = request(kind, n);
        let id = vcx.update(|_, cx| {
            let main = main_window_view(cx).expect("main window");
            let page = main
                .read(cx)
                .page_entity::<ProcessesPage>(Tab::Processes)
                .expect("the Processes page is built");
            page.update(cx, |p, cx| submit(p, req, texts, cx))
        });
        vcx.run_until_parked();
        id.expect("sent")
    }

    fn emit(
        engine: &FakeEngine,
        vcx: &mut VisualTestContext,
        id: ActionId,
        target: Option<u16>,
        state: ActionState,
    ) {
        engine.emit(ActionEvent { id, target, state });
        vcx.run_until_parked();
    }

    fn pushed(vcx: &mut VisualTestContext) -> Vec<PushedToast> {
        vcx.update(|_, cx| {
            cx.global::<GlobalActionTracker>()
                .0
                .read(cx)
                .pushed()
                .to_vec()
        })
    }

    fn visible(vcx: &mut VisualTestContext) -> Vec<String> {
        vcx.update(|_, cx| {
            let main = main_window_view(cx).expect("main window");
            let texts = main.read(cx).toasts().read(cx).visible_texts();
            texts.iter().map(|t| t.to_string()).collect()
        })
    }

    fn actions_sent(engine: &FakeEngine) -> Vec<ActionRequest> {
        engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    #[gpui_kit::test]
    fn action_toast_replaces_in_place_per_request(cx: &mut TestAppContext) {
        let (engine, mut vcx) = open(cx);
        let a = send(&mut vcx, ActionKind::Signal(Signal::Term), 1);
        let b = send(&mut vcx, ActionKind::Signal(Signal::Kill), 1);
        assert_ne!(a, b);
        assert_eq!(actions_sent(&engine).len(), 2);

        emit(&engine, &mut vcx, a, None, ActionState::Started);
        emit(&engine, &mut vcx, b, None, ActionState::Started);
        assert_eq!(
            visible(&mut vcx),
            ["End task requested.", "End process requested."]
        );

        emit(&engine, &mut vcx, b, Some(0), ActionState::Succeeded);
        emit(
            &engine,
            &mut vcx,
            a,
            Some(0),
            ActionState::Rejected(RejectReason::IdentityChanged),
        );
        emit(&engine, &mut vcx, b, None, ActionState::Succeeded);
        emit(
            &engine,
            &mut vcx,
            a,
            None,
            ActionState::Rejected(RejectReason::IdentityChanged),
        );

        let keys: Vec<_> = pushed(&mut vcx)
            .iter()
            .map(|t| (t.key, t.kind, t.text.to_string()))
            .collect();
        assert_eq!(
            keys,
            [
                (
                    Some(ToastKey(a.0)),
                    ToastKind::Info,
                    "End task requested.".into()
                ),
                (
                    Some(ToastKey(b.0)),
                    ToastKind::Info,
                    "End process requested.".into()
                ),
                (
                    Some(ToastKey(b.0)),
                    ToastKind::Success,
                    "Process terminated".into()
                ),
                (
                    Some(ToastKey(a.0)),
                    ToastKind::Warning,
                    "The selected process changed; wait for the list to update and try again."
                        .into()
                ),
            ]
        );
        assert_eq!(
            visible(&mut vcx),
            [
                "The selected process changed; wait for the list to update and try again.",
                "Process terminated",
            ],
            "each request kept its own toast"
        );
    }

    #[gpui_kit::test]
    fn multi_target_result_counts_and_reasons(cx: &mut TestAppContext) {
        let (engine, mut vcx) = open(cx);
        let id = send(&mut vcx, ActionKind::Signal(Signal::Term), 5);
        emit(&engine, &mut vcx, id, None, ActionState::Started);
        assert_eq!(
            visible(&mut vcx),
            ["End task requested for selected processes."]
        );
        let states = [
            ActionState::Succeeded,
            ActionState::Succeeded,
            EPERM,
            ActionState::Succeeded,
            ActionState::Succeeded,
        ];
        for (i, state) in states.iter().enumerate() {
            emit(&engine, &mut vcx, id, Some(i as u16), state.clone());
        }
        emit(&engine, &mut vcx, id, None, ActionState::summarize(&states));
        let last = pushed(&mut vcx).pop().expect("a toast");
        assert_eq!(
            (last.key, last.kind, last.text.as_ref()),
            (
                Some(ToastKey(id.0)),
                ToastKind::Warning,
                "Ended 4 of 5 processes (1 permission denied)"
            )
        );
        assert_eq!(
            visible(&mut vcx),
            ["Ended 4 of 5 processes (1 permission denied)"],
            "the result replaced the requested toast in place"
        );

        // Priority on 4 targets: 3 ok, 1 EPERM; then all failed; then all ok.
        let run = |vcx: &mut VisualTestContext, states: &[ActionState]| {
            let id = send(vcx, ActionKind::SetNice(5), states.len() as u32);
            emit(&engine, vcx, id, None, ActionState::Started);
            for (i, state) in states.iter().enumerate() {
                emit(&engine, vcx, id, Some(i as u16), state.clone());
            }
            emit(&engine, vcx, id, None, ActionState::summarize(states));
            let last = pushed(vcx).pop().expect("a toast");
            (last.kind, last.text.to_string())
        };
        let ok = ActionState::Succeeded;
        assert_eq!(
            run(&mut vcx, &[ok.clone(), EPERM, ok.clone(), ok.clone()]),
            (
                ToastKind::Warning,
                "Priority updated for 3 of 4 processes. 1 failed: permission denied".into()
            )
        );
        assert_eq!(
            run(&mut vcx, &[EPERM, EPERM, EPERM, EPERM]),
            (
                ToastKind::Error,
                "Could not update selected process priority.".into()
            )
        );
        assert_eq!(
            run(&mut vcx, &[ok.clone(), ok.clone(), ok.clone(), ok]),
            (
                ToastKind::Success,
                "Selected process priority updated.".into()
            )
        );
    }

    #[gpui_kit::test]
    fn refusals_send_no_request(cx: &mut TestAppContext) {
        let (engine, mut vcx) = open(cx);
        let before = actions_sent(&engine).len();
        let me = CapturedTargets::new(
            [
                target(300, ProcFlags::empty()),
                target(4242, ProcFlags::IS_SELF | ProcFlags::PROTECTED),
            ],
            None,
        );
        assert!(vcx.update(|_, cx| refuse(&me, cx)));
        vcx.run_until_parked();
        assert_eq!(visible(&mut vcx), ["Refusing to end tasklens itself."]);
        assert!(
            !vcx.update(|_, cx| refuse(&targets(2), cx)),
            "nothing to refuse"
        );
        assert_eq!(actions_sent(&engine).len(), before, "no request was sent");
    }
}
