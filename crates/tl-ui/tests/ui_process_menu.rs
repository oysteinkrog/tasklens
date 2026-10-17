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
use tl_ui::pages::processes::confirm::{
    ConfirmKind, ConfirmResult, ConfirmTexts, ProtectedAction, confirm, texts,
};
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
                action: ProtectedAction::End,
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
                skipped: 0,
                groups: 1,
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
        action: ProtectedAction::End,
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
    use tl_model::health::HelperState;
    use tl_model::ids::{NodeId, ProcessKey};
    use tl_model::process::ProcFlags;
    use tl_model::settings::{Settings, UiState};
    use tl_model::system::CpuSet;
    use tl_model::time::BootId;
    use tl_ui::action_tracker::{GlobalActionTracker, PushedToast, RetryActionAsAdmin};
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
            stopped: false,
            ppid: 1,
            nice: Some(0),
            io_priority: None,
            exe_deleted: false,
            unit: None,
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
    pub(super) fn open(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
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

    pub(super) fn emit(
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

    pub(super) fn visible(vcx: &mut VisualTestContext) -> Vec<String> {
        vcx.update(|_, cx| {
            let main = main_window_view(cx).expect("main window");
            let texts = main.read(cx).toasts().read(cx).visible_texts();
            texts.iter().map(|t| t.to_string()).collect()
        })
    }

    pub(super) fn actions_sent(engine: &FakeEngine) -> Vec<ActionRequest> {
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

    /// tl-helper is installed, so a privilege failure may be retried.
    fn publish_helper(engine: &FakeEngine, vcx: &mut VisualTestContext) {
        engine.publish_with(|s| {
            let health = Arc::make_mut(&mut s.health);
            health.helper = HelperState::Locked;
            health.version += 1;
        });
        vcx.run_until_parked();
    }

    /// One state per target, then the summary.
    fn finish(
        engine: &FakeEngine,
        vcx: &mut VisualTestContext,
        id: ActionId,
        states: &[ActionState],
    ) {
        for (i, state) in states.iter().enumerate() {
            emit(engine, vcx, id, Some(i as u16), state.clone());
        }
        emit(engine, vcx, id, None, ActionState::summarize(states));
    }

    fn last_button(vcx: &mut VisualTestContext) -> Option<String> {
        let last = pushed(vcx).pop().expect("a toast");
        last.button.map(|b| b.to_string())
    }

    fn five(bad: ActionState) -> [ActionState; 5] {
        let ok = ActionState::Succeeded;
        [ok.clone(), ok.clone(), bad, ok.clone(), ok]
    }

    #[gpui_kit::test]
    fn retry_button_only_on_eperm(cx: &mut TestAppContext) {
        let (engine, mut vcx) = open(cx);
        publish_helper(&engine, &mut vcx);

        let id = send(&mut vcx, ActionKind::Signal(Signal::Term), 5);
        finish(&engine, &mut vcx, id, &five(EPERM));
        let last = pushed(&mut vcx).pop().expect("a toast");
        assert_eq!(last.text, "Ended 4 of 5 processes (1 permission denied)");
        assert_eq!(last.button.as_deref(), Some("Retry as administrator"));

        let gone = ActionState::Failed(ActionError::NoSuchProcess);
        let id = send(&mut vcx, ActionKind::Signal(Signal::Term), 5);
        finish(&engine, &mut vcx, id, &five(gone.clone()));
        assert_eq!(last_button(&mut vcx).as_deref(), Some("Details"));

        let id = send(&mut vcx, ActionKind::Signal(Signal::Term), 1);
        finish(&engine, &mut vcx, id, &[gone]);
        assert_eq!(last_button(&mut vcx), None);
    }

    #[gpui_kit::test]
    fn retry_resends_failed_targets_privileged(cx: &mut TestAppContext) {
        let (engine, mut vcx) = open(cx);
        publish_helper(&engine, &mut vcx);
        let id = send(&mut vcx, ActionKind::Signal(Signal::Term), 5);
        finish(&engine, &mut vcx, id, &five(EPERM));
        assert_eq!(actions_sent(&engine).len(), 1);

        vcx.dispatch_action(RetryActionAsAdmin(id.0));
        vcx.run_until_parked();
        let sent = actions_sent(&engine);
        assert_eq!(sent.len(), 2);
        let retry = &sent[1];
        assert!(retry.privileged);
        assert_ne!(retry.id, id);
        assert_eq!(retry.kind, ActionKind::Signal(Signal::Term));
        assert_eq!(retry.targets.as_slice(), [ActionTarget::Process(key(102))]);

        // 17 more finished requests push the first one out of the ring of 16.
        for _ in 0..17 {
            let next = send(&mut vcx, ActionKind::Signal(Signal::Term), 1);
            finish(&engine, &mut vcx, next, &[EPERM]);
        }
        let before = actions_sent(&engine).len();
        vcx.dispatch_action(RetryActionAsAdmin(id.0));
        vcx.run_until_parked();
        assert_eq!(actions_sent(&engine).len(), before, "too old to retry");
    }

    #[gpui_kit::test]
    fn no_automatic_retry(cx: &mut TestAppContext) {
        let (engine, mut vcx) = open(cx);
        publish_helper(&engine, &mut vcx);
        let one = send(&mut vcx, ActionKind::Signal(Signal::Term), 1);
        finish(&engine, &mut vcx, one, &[EPERM]);
        let many = send(&mut vcx, ActionKind::Signal(Signal::Term), 5);
        finish(&engine, &mut vcx, many, &five(EPERM));
        vcx.run_until_parked();
        let sent = actions_sent(&engine);
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|r| !r.privileged));
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

/// End Task, End Process and End Process Tree handlers (tasklens-pdu.20, UI-PRC-013, UI-PRC-014,
/// PROC-059, D-009, R-192, R-198).
mod end {
    use std::sync::Arc;

    use gpui_kit::component::kbd::Kbd;
    use gpui_kit::{
        Action, Entity, Modifiers, TestAppContext, VisualTestContext, WindowHandle, WindowId,
    };
    use gpui_kit::{Keystroke, point, px};
    use tl_model::actions::ActionError;
    use tl_model::actions::CgroupRef;
    use tl_model::actions::IoPriority;
    use tl_model::actions::SchedPolicy;
    use tl_model::actions::{ActionKind, ActionState, ActionTarget, Signal};
    use tl_model::command::KWinCommand;
    use tl_model::command::{Command, Tab};
    use tl_model::health::{EngineCaps, HelperState};
    use tl_model::ids::ColumnId;
    use tl_model::ids::DeviceIdx;
    use tl_model::ids::PseudoKind;
    use tl_model::ids::{GroupKey, NodeId, ProcessKey};
    use tl_model::process::{
        AppCgroupSample, GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState, ProcessMeta,
        ProcessRecord,
    };
    use tl_model::process::{ProcessExtras, ProcessExtrasP2};
    use tl_model::settings::Settings;
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, WindowEntry, WindowsSnapshot};
    use tl_model::snapshot::{DeviceDetailsSnapshot, DiskDetails};
    use tl_model::snapshot::{ServiceStatus, ServicesSnapshot, StartupType, UnitRecord};
    use tl_model::system::CpuSet;
    use tl_model::table::{Manager, Total, UnitRef, UnitType};
    use tl_model::table::{ProcessTableView, TableRow};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::action_tracker::message;
    use tl_ui::components::dialog::{DialogButton, TlDialogView};
    use tl_ui::components::menu::{
        MenuEntryInfo, MenuEntryKind, TlMenu, TlMenuPopup, open_context_menu, open_context_menu_at,
    };
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::i18n::tr_en;
    use tl_ui::pages::processes::actions::{
        ActivateWindow, CopyAllMetrics, CopyDetails, CreateDump, EndProcess, EndProcessTree,
        EndTask, FileProperties, FreezeApp, GoToParent, IoClass, KillApp, OpenFileLocation,
        OpenVirusTotal, PriorityClass, ProcessesShowHidden, RestartUnit, ResumeProcess,
        SetIoPriorityClass, SetPriority, ShowWindow, SuspendProcess, ThawApp,
    };
    use tl_ui::pages::processes::cells::{
        CellCtx, CellWords, GroupMembers, RowRef, copy_all_metrics, copy_cell, copy_details,
        process_column,
    };
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::pages::processes::menu::window_popup;
    use tl_ui::pages::processes::{ProcessesPage, SuspendLabel, suspend_or_resume};
    use tl_ui::pages::services::actions::{GoToService, JumpOrigin, ManagerArg};
    use tl_ui::table::actions::{CopyCell, CopyRowsAsTable};
    use tl_ui::table::tree_table::{RowKind, TreeTableDelegate as _};
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::main_window_view;
    use tl_ui::windows::actions::{OpenCustomPriority, SetAffinity};
    use tl_ui::windows::affinity::AffinityDialog;
    use tl_ui::windows::priority::PriorityDialog;

    use super::toasts::{actions_sent, emit, open, visible};

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 1_000 + u64::from(pid),
            boot_id: BootId(1),
            pid,
        }
    }

    fn record(pid: u32, ppid: u32, name: &str, flags: ProcFlags) -> ProcessRecord {
        let a = |s: &str| Arc::<str>::from(s);
        ProcessRecord {
            key: key(pid),
            ppid,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: a(name),
                name: a(name),
                exe: Some(a(&format!("/usr/bin/{name}"))),
                cmdline: None,
                user: a("me"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a(name),
            }),
            group: GroupKind::Background.key(),
            state: ProcState::Sleeping,
            flags,
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn app_key() -> GroupKey {
        GroupKind::App {
            app_id: Arc::from("firefox"),
        }
        .key()
    }

    fn app_group(members: u32) -> GroupRow {
        GroupRow {
            node: NodeId::Group(app_key()),
            key: app_key(),
            kind: GroupRowKind::App,
            class: GroupKind::App {
                app_id: Arc::from("firefox"),
            },
            name: Arc::from("Firefox"),
            icon: None,
            members,
            pids: members,
            note: None,
            memory_bytes: 0,
            disk_bps: 0.0,
            net_bps: 0.0,
            gpu_pct: 0.0,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            threads: 0,
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            commit_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            handles: None,
            desktop_path: None,
            description: None,
            user: None,
            frozen: false,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn row(node: NodeId, record: Option<u32>, group: Option<u32>) -> TableRow {
        TableRow {
            node,
            depth: 0,
            expanded: false,
            has_children: false,
            record,
            group,
            descendants: 0,
            subtree: None,
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
        version: u64,
        /// The app.slice samples the next publish carries (`ProcessSnapshot::apps`).
        apps: Vec<AppCgroupSample>,
    }

    impl Rig {
        fn new(cx: &mut TestAppContext) -> Self {
            let (engine, mut vcx) = open(cx);
            let page = vcx.update(|_, cx| {
                main_window_view(cx)
                    .expect("main window")
                    .read(cx)
                    .page_entity::<ProcessesPage>(Tab::Processes)
                    .expect("the Processes page is built")
            });
            Self {
                engine,
                vcx,
                page,
                version: 1,
                apps: Vec::new(),
            }
        }

        /// Publishes `records` (sorted by key) with one process row per record in `order`
        /// (PIDs) and the given group rows after them.
        fn publish(&mut self, records: Vec<ProcessRecord>, order: &[u32], groups: Vec<GroupRow>) {
            let mut rows: Vec<TableRow> = order
                .iter()
                .map(|&pid| {
                    let i = records
                        .iter()
                        .position(|r| r.key.pid == pid)
                        .expect("a record for every row");
                    row(NodeId::Process(key(pid)), Some(i as u32), None)
                })
                .collect();
            rows.extend(
                groups
                    .iter()
                    .enumerate()
                    .map(|(g, group)| row(group.node.clone(), None, Some(g as u32))),
            );
            self.publish_rows(records, Vec::new(), rows, groups);
        }

        /// Publishes `records` with their `extras` (empty, or one per record) and exactly
        /// `rows`.
        fn publish_rows(
            &mut self,
            records: Vec<ProcessRecord>,
            extras: Vec<ProcessExtras>,
            rows: Vec<TableRow>,
            groups: Vec<GroupRow>,
        ) {
            self.version += 1;
            let version = self.version;
            let apps = self.apps.clone();
            let generation = self.vcx.update(|_, cx| {
                self.page
                    .read(cx)
                    .table()
                    .read(cx)
                    .delegate
                    .query()
                    .generation()
            });
            self.engine.publish_with(|s| {
                s.table = Arc::new(ProcessTableView {
                    header: ComponentHeader {
                        version,
                        validity: Validity::Valid,
                        ..ComponentHeader::default()
                    },
                    query_generation: generation,
                    rows: rows.into(),
                    groups: groups.into(),
                    ..ProcessTableView::default()
                });
                s.processes = Arc::new(ProcessSnapshot {
                    records: records.into(),
                    extras: extras.into(),
                    apps: apps.into(),
                    ..ProcessSnapshot::default()
                });
            });
            self.vcx.run_until_parked();
        }

        /// Makes `ids` the selection, the first one clicked and the rest Ctrl+clicked.
        fn select(&mut self, ids: &[ProcRowId]) {
            let table = self.vcx.update(|_, cx| self.page.read(cx).table().clone());
            self.vcx.update(|_, cx| {
                table.update(cx, |t, cx| {
                    for (n, id) in ids.iter().enumerate() {
                        if n == 0 {
                            t.selection.click(*id);
                        } else {
                            t.selection.ctrl_click(*id);
                        }
                    }
                    cx.notify();
                })
            });
            self.vcx.run_until_parked();
        }

        fn select_pids(&mut self, pids: &[u32]) {
            let ids: Vec<ProcRowId> = pids.iter().map(|&p| ProcRowId::Process(key(p))).collect();
            self.select(&ids);
        }

        fn windows(&mut self) -> Vec<WindowId> {
            self.vcx
                .update(|_, cx| cx.windows().iter().map(|w| w.window_id()).collect())
        }

        /// Focuses the table, dispatches `action` and returns the confirmation it opened.
        fn dispatch(&mut self, action: impl Action) -> Option<Dialog> {
            let before = self.windows();
            let page = self.page.clone();
            self.vcx
                .update(|window, cx| page.update(cx, |p, cx| p.focus_table(window, cx)));
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
            self.vcx.dispatch_action(action);
            self.vcx.run_until_parked();
            let id = self.windows().into_iter().find(|w| !before.contains(w))?;
            let handle = WindowHandle::<gpui_kit::base::Root>::new(id);
            let view = self.vcx.update(|_, cx| {
                TlSecondaryWindow::<TlDialogView>::find(handle, cx)
                    .map(|f| f.read(cx).content().clone())
            })?;
            let any = self
                .vcx
                .update(|_, cx| cx.windows())
                .into_iter()
                .find(|w| w.window_id() == id)?;
            let mut vcx = VisualTestContext::from_window(any, &self.vcx.cx);
            vcx.update(|window, _| window.activate_window());
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            Some(Dialog { vcx, view })
        }

        /// Every request sent, with its targets as they are.
        fn sent_targets(&self) -> Vec<(ActionKind, Vec<ActionTarget>)> {
            actions_sent(&self.engine)
                .into_iter()
                .map(|r| (r.kind, r.targets.to_vec()))
                .collect()
        }

        fn sent(&self) -> Vec<(ActionKind, Vec<u32>)> {
            actions_sent(&self.engine)
                .into_iter()
                .map(|r| {
                    let pids = r
                        .targets
                        .iter()
                        .map(|t| match t {
                            ActionTarget::Process(k) => k.pid,
                            other => panic!("not a process target: {other:?}"),
                        })
                        .collect();
                    (r.kind, pids)
                })
                .collect()
        }
    }

    struct Dialog {
        vcx: VisualTestContext,
        view: Entity<TlDialogView>,
    }

    impl Dialog {
        fn text(&mut self) -> String {
            let view = self.view.clone();
            self.vcx
                .update(|_, cx| view.read(cx).dialog().text().to_string())
        }

        /// Tab to the action button, then Enter (focus starts on Cancel).
        fn confirm(mut self) {
            self.vcx.simulate_keystrokes("tab");
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
            self.vcx.simulate_keystrokes("enter");
            self.vcx.run_until_parked();
        }

        fn cancel(mut self) {
            self.vcx.simulate_keystrokes("escape");
            self.vcx.run_until_parked();
        }

        /// The action button is Danger.
        fn is_destructive(&mut self) -> bool {
            let view = self.view.clone();
            self.vcx
                .update(|_, cx| view.read(cx).dialog().is_destructive())
        }

        fn focused(&mut self) -> Option<DialogButton> {
            let view = self.view.clone();
            self.vcx
                .update(|window, cx| view.read(cx).focused_button(window))
        }

        /// Enter on the button that has focus.
        fn enter(mut self) {
            self.vcx.simulate_keystrokes("enter");
            self.vcx.run_until_parked();
        }
    }

    fn own(pids: &[u32]) -> Vec<ProcessRecord> {
        pids.iter()
            .map(|&p| record(p, 1, "proc", ProcFlags::empty()))
            .collect()
    }

    #[gpui_kit::test]
    fn single_end_process_own_unprotected_has_no_dialog(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300, 301]), &[300, 301], Vec::new());
        rig.select_pids(&[301]);
        assert!(rig.dispatch(EndProcess).is_none(), "no dialog (D-009)");
        assert_eq!(rig.sent(), [(ActionKind::Signal(Signal::Kill), vec![301])]);
        let id = actions_sent(&rig.engine).pop().expect("a request").id;
        emit(&rig.engine, &mut rig.vcx, id, None, ActionState::Started);
        assert_eq!(visible(&mut rig.vcx), ["End process requested."]);
    }

    #[gpui_kit::test]
    fn protected_target_confirms(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(
            vec![record(900, 1, "plasmashell", ProcFlags::PROTECTED)],
            &[900],
            Vec::new(),
        );
        rig.select_pids(&[900]);
        let mut dialog = rig.dispatch(EndTask).expect("the Protected dialog");
        assert_eq!(
            dialog.text(),
            "Ending plasmashell will end your desktop session. End it anyway?"
        );
        dialog.cancel();
        assert!(rig.sent().is_empty(), "Cancel sends nothing");

        let dialog = rig.dispatch(EndTask).expect("the Protected dialog");
        dialog.confirm();
        assert_eq!(rig.sent(), [(ActionKind::Signal(Signal::Term), vec![900])]);
    }

    #[gpui_kit::test]
    fn multi_end_confirms_with_count(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(
            own(&[300, 301, 302, 303]),
            &[303, 300, 302, 301],
            Vec::new(),
        );
        rig.select_pids(&[300, 301, 302]);
        let mut dialog = rig.dispatch(EndTask).expect("the EndMany dialog");
        assert_eq!(dialog.text(), "End 3 selected processes?");
        dialog.confirm();
        assert_eq!(
            rig.sent(),
            [(ActionKind::Signal(Signal::Term), vec![300, 302, 301])],
            "one request, targets in view order"
        );

        let mut dialog = rig.dispatch(EndProcess).expect("the EndMany dialog");
        assert_eq!(dialog.text(), "End 3 selected processes immediately?");
        dialog.cancel();
        assert_eq!(rig.sent().len(), 1);
    }

    #[gpui_kit::test]
    fn other_user_target_confirms(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let mut sshd = record(812, 1, "sshd", ProcFlags::OTHER_USER);
        sshd.uid = 0;
        Arc::make_mut(&mut sshd.meta).user = Arc::from("root");
        rig.publish(vec![sshd], &[812], Vec::new());
        rig.select_pids(&[812]);
        let mut dialog = rig.dispatch(EndTask).expect("the OtherUser dialog");
        assert_eq!(
            dialog.text(),
            "End task on sshd (PID 812), owned by root. Continue?"
        );
        dialog.confirm();
        assert_eq!(rig.sent(), [(ActionKind::Signal(Signal::Term), vec![812])]);
    }

    #[gpui_kit::test]
    fn refusals_self_pid1_kthread(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(
            vec![
                record(1, 0, "systemd", ProcFlags::PROTECTED),
                record(2, 0, "kthreadd", ProcFlags::KERNEL_THREAD),
                record(3, 2, "kworker/0:1", ProcFlags::KERNEL_THREAD),
                record(
                    4242,
                    1,
                    "tasklens",
                    ProcFlags::IS_SELF | ProcFlags::PROTECTED,
                ),
                record(4300, 1, "gone", ProcFlags::EXITED),
            ],
            &[4242, 1, 2, 3, 4300],
            Vec::new(),
        );
        rig.select_pids(&[4242]);
        assert!(rig.dispatch(EndTask).is_none());
        assert_eq!(visible(&mut rig.vcx), ["Refusing to end tasklens itself."]);
        assert!(rig.sent().is_empty(), "nothing is sent");

        // One toast per distinct reason; the exited row is dropped first.
        rig.select_pids(&[1, 2, 3, 4300]);
        assert!(rig.dispatch(EndProcess).is_none());
        assert!(rig.sent().is_empty());
        let shown = visible(&mut rig.vcx);
        let count = |text: &str| shown.iter().filter(|t| *t == text).count();
        assert_eq!(count("PID 1 (systemd) cannot be ended."), 1);
        assert_eq!(count("Kernel threads cannot be signalled."), 1);
        assert_eq!(count("Process exited"), 0);

        // Only exited targets: "Process exited".
        rig.select_pids(&[4300]);
        assert!(rig.dispatch(EndTask).is_none());
        assert!(visible(&mut rig.vcx).iter().any(|t| t == "Process exited"));
        assert!(rig.sent().is_empty());
    }

    #[gpui_kit::test]
    fn end_tree_collects_descendants_capped_256(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        // 100 has 7 descendants over four levels; 150 is an unrelated process and 160 an
        // exited child that the walk skips.
        let records = vec![
            record(100, 1, "root", ProcFlags::empty()),
            record(101, 100, "a", ProcFlags::empty()),
            record(102, 100, "b", ProcFlags::empty()),
            record(103, 101, "c", ProcFlags::empty()),
            record(104, 101, "d", ProcFlags::empty()),
            record(105, 102, "e", ProcFlags::empty()),
            record(106, 105, "f", ProcFlags::empty()),
            record(107, 106, "g", ProcFlags::empty()),
            record(150, 1, "other", ProcFlags::empty()),
            record(160, 100, "gone", ProcFlags::EXITED),
        ];
        rig.publish(records, &[150, 100], Vec::new());
        rig.select_pids(&[100]);
        let mut dialog = rig.dispatch(EndProcessTree).expect("the EndTree dialog");
        assert_eq!(dialog.text(), "End 8 processes in this tree?");
        dialog.confirm();
        assert_eq!(
            rig.sent(),
            [(
                ActionKind::KillTree,
                vec![100, 101, 102, 103, 104, 105, 106, 107]
            )],
            "breadth first, lowest PID first"
        );

        // 300 children: the request stops at 256 keys.
        let mut records = vec![record(1000, 1, "root", ProcFlags::empty())];
        records.extend((1001..1301).map(|p| record(p, 1000, "child", ProcFlags::empty())));
        rig.publish(records, &[1000], Vec::new());
        rig.select_pids(&[1000]);
        let mut dialog = rig.dispatch(EndProcessTree).expect("the EndTree dialog");
        assert_eq!(dialog.text(), "End 256 processes in this tree?");
        dialog.confirm();
        let (kind, pids) = rig.sent().pop().expect("a request");
        assert_eq!(kind, ActionKind::KillTree);
        assert_eq!(pids.len(), 256);
        assert_eq!(pids[..3], [1000, 1001, 1002]);
    }

    #[gpui_kit::test]
    fn end_tree_refuses_subtree_with_self_or_pid1(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        // tasklens (4242) runs under a shell (410) in a terminal (400). Records are in PID
        // order, as in a snapshot.
        let records = vec![
            record(400, 1, "konsole", ProcFlags::empty()),
            record(410, 400, "fish", ProcFlags::empty()),
            record(420, 400, "vim", ProcFlags::empty()),
            record(
                4242,
                410,
                "tasklens",
                ProcFlags::IS_SELF | ProcFlags::PROTECTED,
            ),
        ];
        rig.publish(records, &[400, 420], Vec::new());
        rig.select_pids(&[400]);
        assert!(rig.dispatch(EndProcessTree).is_none(), "no dialog");
        assert_eq!(visible(&mut rig.vcx), ["Refusing to end tasklens itself."]);
        assert!(rig.sent().is_empty(), "nothing is sent");

        // PID 1 under the selected process, as in a container's PID namespace view.
        let records = vec![
            record(1, 500, "systemd", ProcFlags::PROTECTED),
            record(500, 0, "conmon", ProcFlags::empty()),
            record(510, 1, "sshd", ProcFlags::empty()),
        ];
        rig.publish(records, &[500], Vec::new());
        rig.select_pids(&[500]);
        assert!(rig.dispatch(EndProcessTree).is_none(), "no dialog");
        let shown = visible(&mut rig.vcx);
        assert_eq!(
            shown
                .iter()
                .filter(|t| *t == "PID 1 (systemd) cannot be ended.")
                .count(),
            1
        );
        assert!(rig.sent().is_empty(), "nothing is sent");

        // A sibling subtree without them still asks first.
        let records = vec![
            record(400, 1, "konsole", ProcFlags::empty()),
            record(410, 400, "fish", ProcFlags::empty()),
            record(420, 400, "vim", ProcFlags::empty()),
            record(421, 420, "rust-analyzer", ProcFlags::empty()),
            record(
                4242,
                410,
                "tasklens",
                ProcFlags::IS_SELF | ProcFlags::PROTECTED,
            ),
        ];
        rig.publish(records, &[400, 420], Vec::new());
        rig.select_pids(&[420]);
        let mut dialog = rig.dispatch(EndProcessTree).expect("the EndTree dialog");
        assert_eq!(dialog.text(), "End 2 processes in this tree?");
        dialog.confirm();
        assert_eq!(rig.sent(), [(ActionKind::KillTree, vec![420, 421])]);
    }

    #[gpui_kit::test]
    fn end_app_unwraps_bwrap(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let member = |pid, ppid, name| ProcessRecord {
            group: app_key(),
            ..record(pid, ppid, name, ProcFlags::empty())
        };
        let records = vec![
            member(200, 1, "bwrap"),
            member(201, 200, "bwrap"),
            member(202, 201, "firefox"),
            member(203, 202, "firefox"),
        ];
        rig.publish(records, &[], vec![app_group(4)]);
        rig.select(&[ProcRowId::Group(app_key())]);
        assert!(
            rig.dispatch(EndTask).is_none(),
            "one own app row: no dialog"
        );
        assert_eq!(
            rig.sent(),
            [(ActionKind::Signal(Signal::Term), vec![202])],
            "the request targets firefox, not bwrap"
        );

        // End Process skips app rows until Kill App.
        assert!(rig.dispatch(EndProcess).is_none());
        assert_eq!(rig.sent().len(), 1);
    }

    #[gpui_kit::test]
    fn row_menu_targets_captured_on_open(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300, 301, 302]), &[300, 301, 302], Vec::new());
        rig.select_pids(&[300]);
        // The menu opens on row X (PID 300, row 0) and captures it.
        let menu = build_menu(&mut rig, &[0], ColumnId::Name);
        assert_eq!(shape(&mut rig, &menu)[0], "End Task");
        // The rows move and the selection changes before the item runs.
        rig.publish(own(&[300, 301, 302]), &[302, 301, 300], Vec::new());
        rig.select_pids(&[302]);
        assert!(rig.dispatch(EndTask).is_none());
        assert_eq!(
            rig.sent(),
            [(ActionKind::Signal(Signal::Term), vec![300])],
            "the captured target X"
        );

        // The capture is used once; the next action reads the selection.
        assert!(rig.dispatch(EndTask).is_none());
        assert_eq!(rig.sent()[1], (ActionKind::Signal(Signal::Term), vec![302]));
    }

    /// UI-PRC-017: an action started during Ctrl-pause targets the paused snapshot's identity,
    /// and the engine revalidates it.
    #[gpui_kit::test]
    fn action_while_paused_revalidates_identity(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300, 301]), &[300, 301], Vec::new());
        rig.select_pids(&[300]);
        let ctrl = |rig: &mut Rig, control: bool| {
            rig.vcx.simulate_modifiers_change(gpui_kit::Modifiers {
                control,
                ..gpui_kit::Modifiers::default()
            });
            rig.vcx.run_until_parked();
        };
        let page = rig.page.clone();
        ctrl(&mut rig, true);
        assert!(rig.vcx.update(|_, cx| page.read(cx).is_paused()));
        // PID 300 restarts: the new snapshot has another identity for it.
        let mut restarted = own(&[300, 301]);
        restarted[0].key.start_ticks += 500;
        rig.publish(restarted, &[300, 301], Vec::new());
        let menu = build_menu(&mut rig, &[0], ColumnId::Name);
        assert_eq!(shape(&mut rig, &menu)[0], "End Task");
        assert!(rig.dispatch(EndTask).is_none());
        let request = actions_sent(&rig.engine).pop().expect("a request");
        assert_eq!(request.kind, ActionKind::Signal(Signal::Term));
        assert_eq!(
            request.targets.as_slice(),
            [ActionTarget::Process(key(300))],
            "the paused snapshot's identity, not the restarted one"
        );
        ctrl(&mut rig, false);
        assert!(!rig.vcx.update(|_, cx| page.read(cx).is_paused()));
    }

    /// Builds the menu the table opens for the rows at `rows`, right-clicked in `col`, the
    /// way `TlTreeTable::open_menu` builds it.
    fn build_menu(rig: &mut Rig, rows: &[usize], col: ColumnId) -> Entity<TlMenuPopup> {
        build_menu_from(rig, rows, Some(col))
    }

    /// As [`build_menu`], with `None` for the keyboard path.
    fn build_menu_from(
        rig: &mut Rig,
        rows: &[usize],
        col: Option<ColumnId>,
    ) -> Entity<TlMenuPopup> {
        let table = rig.vcx.update(|_, cx| rig.page.read(cx).table().clone());
        rig.vcx.update(|window, cx| window.draw(cx).clear(cx));
        rig.vcx.update(|window, cx| {
            let focus = table.read(cx).focus_handle().clone();
            let ids: Vec<ProcRowId> = rows
                .iter()
                .map(|&ix| table.read(cx).delegate.row_id(ix))
                .collect();
            TlMenu::build(window, cx, focus, |menu, window, cx| {
                table.update(cx, |t, cx| {
                    t.delegate.context_menu(&ids, col, menu, window, cx)
                })
            })
        })
    }

    fn entries(rig: &mut Rig, menu: &Entity<TlMenuPopup>) -> Vec<MenuEntryInfo> {
        rig.vcx
            .update(|window, cx| menu.read(cx).entries_for_test(window))
    }

    /// Each entry as text: an item by its label ("* " first when checked), a disabled item as
    /// "(label: tooltip)", a submenu as "label >", a tooltip as " [tooltip]" after the label, a
    /// separator as "-" and a header as "# label".
    fn shape(rig: &mut Rig, menu: &Entity<TlMenuPopup>) -> Vec<String> {
        entries(rig, menu)
            .into_iter()
            .map(|e| {
                let label = e.label.as_deref().unwrap_or_default();
                let tip = e
                    .tooltip
                    .as_deref()
                    .map(|t| format!(" [{t}]"))
                    .unwrap_or_default();
                match e.kind {
                    MenuEntryKind::Item if e.checked => format!("* {label}{tip}"),
                    MenuEntryKind::Item => format!("{label}{tip}"),
                    MenuEntryKind::Disabled => {
                        format!(
                            "({label}: {})",
                            e.tooltip.as_deref().unwrap_or("NO TOOLTIP")
                        )
                    }
                    MenuEntryKind::Submenu => format!("{label} >{tip}"),
                    MenuEntryKind::Slider => format!("{label} ~"),
                    MenuEntryKind::Separator => "-".to_owned(),
                    MenuEntryKind::Header => format!("# {label}"),
                }
            })
            .collect()
    }

    /// The shapes of every submenu of `menu`, one after the other.
    fn submenu_shapes(rig: &mut Rig, menu: &Entity<TlMenuPopup>) -> Vec<String> {
        let subs: Vec<_> = entries(rig, menu)
            .into_iter()
            .filter_map(|e| e.submenu)
            .collect();
        subs.iter().flat_map(|s| shape(rig, s)).collect()
    }

    fn unit_ref() -> UnitRef {
        UnitRef {
            manager: Manager::System,
            name: Arc::from("foo.service"),
        }
    }

    fn header_row(node: NodeId) -> TableRow {
        TableRow {
            expanded: true,
            has_children: true,
            ..row(node, None, None)
        }
    }

    /// Every row kind, by index: 0 an expanded group header, 1 to 3 own processes, 4 an exited
    /// process, 5 a kernel thread, 6 a process whose executable was deleted, 7 an app row, 8 a
    /// unit row, 9 a pseudo row, 10 PID 1 (parent PID 0, protected).
    fn publish_kinds(rig: &mut Rig) {
        let mut records = own(&[300, 301, 302]);
        records.push(record(4300, 1, "gone", ProcFlags::EXITED));
        records.push(record(3, 2, "kworker/0:1", ProcFlags::KERNEL_THREAD));
        records.push(record(500, 1, "updated", ProcFlags::empty()));
        records.push(ProcessRecord {
            group: app_key(),
            ..record(200, 1, "firefox", ProcFlags::empty())
        });
        records.push(record(600, 1, "food", ProcFlags::empty()));
        records.push(record(1, 0, "systemd", ProcFlags::PROTECTED));
        let extras = records
            .iter()
            .map(|r| ProcessExtras {
                exe_deleted: r.key.pid == 500,
                unit: (r.key.pid == 600).then(unit_ref),
                ..firefox_extras()
            })
            .collect();
        let at = |pid: u32| {
            let i = records
                .iter()
                .position(|r| r.key.pid == pid)
                .expect("a record");
            row(NodeId::Process(key(pid)), Some(i as u32), None)
        };
        let unit = GroupRow {
            node: NodeId::Unit(unit_ref()),
            kind: GroupRowKind::Unit,
            name: Arc::from("foo.service"),
            ..app_group(1)
        };
        let rows = vec![
            header_row(NodeId::Group(GroupKind::Background.key())),
            at(300),
            at(301),
            at(302),
            at(4300),
            at(3),
            at(500),
            row(NodeId::Group(app_key()), None, Some(0)),
            row(NodeId::Unit(unit_ref()), None, Some(1)),
            row(NodeId::Pseudo(PseudoKind::Interrupts), None, None),
            at(1),
        ];
        rig.apps = vec![firefox_sample(false)];
        rig.publish_rows(records, extras, rows, vec![app_group(1), unit]);
    }

    fn app_cgroup() -> CgroupRef {
        CgroupRef {
            rel_path: Arc::from(
                "user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox-1.scope",
            ),
            cgroup_id: 77,
        }
    }

    /// The app.slice sample of the firefox app row.
    fn firefox_sample(frozen: bool) -> AppCgroupSample {
        AppCgroupSample {
            group: app_key(),
            anon_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            rios: 0,
            wios: 0,
            pids: 1,
            frozen,
            cgroup: app_cgroup(),
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    const COPIES: [&str; 4] = [
        "Copy",
        "Copy details",
        "Copy all metrics",
        "Copy rows as table",
    ];

    /// Item 18 with no tray installed (wox.13, PERF-088).
    const TRAY_ITEM: &str = "(Add tray icon graph: No system tray available)";

    /// The menu of one process row, with item 21 as `open_file` and item 24 as `parent`.
    fn process_shape(open_file: &str, parent: Option<&str>) -> Vec<String> {
        let mut out = vec![
            "End Task",
            "End Process [Ends the process immediately (SIGKILL)]",
            "End Process Tree",
            "-",
            "Suspend Process",
            "Send Signal >",
            "Set Priority >",
            "Set Affinity…",
            "Set I/O Priority >",
            "-",
        ];
        out.extend(COPIES);
        out.extend([
            "-",
            "Add Minigraph >",
            "Add Process Name Graph >",
            TRAY_ITEM,
            "-",
            "Properties",
            open_file,
            "Open in VirusTotal",
            "(Create dump file: Install gdb to create dumps)",
        ]);
        if let Some(parent) = parent {
            out.extend(["-", parent]);
        }
        out.into_iter().map(String::from).collect()
    }

    #[gpui_kit::test]
    fn row_menu_items_per_row_kind(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        let menu = |rig: &mut Rig, rows: &[usize]| {
            let m = build_menu(rig, rows, ColumnId::Cpu);
            shape(rig, &m)
        };
        let process = process_shape("Open file location", Some("Go to parent process"));
        assert_eq!(menu(&mut rig, &[1]), process, "one running own process");
        let mut kernel = process.clone();
        let signal = kernel.iter().position(|l| l == "Send Signal >").unwrap();
        kernel[signal] = "(Send Signal: Kernel threads cannot be signalled.)".to_owned();
        assert_eq!(
            menu(&mut rig, &[5]),
            kernel,
            "a kernel thread as a process, Send Signal disabled"
        );
        assert_eq!(
            menu(&mut rig, &[6]),
            process_shape(
                "(Open file location: Executable deleted)",
                Some("Go to parent process")
            )
        );
        assert_eq!(
            menu(&mut rig, &[10]),
            process_shape("Open file location", None),
            "parent PID 0: no Go to parent; protected keeps the labels"
        );

        let mut multi = vec![
            "End Selected Tasks",
            "End Selected Processes [Ends the process immediately (SIGKILL)]",
            "-",
            "Suspend Selected Tasks",
            "Send Signal >",
            "Set Priority >",
            "Set Affinity…",
            "Set I/O Priority >",
            "-",
        ];
        multi.extend(COPIES);
        multi.extend(["-", "Properties"]);
        assert_eq!(menu(&mut rig, &[1, 2, 3]), multi, "3 selected rows");

        let mut exited = vec![
            "(End Task: Process exited)",
            "(End Process: Process exited)",
            "(End Process Tree: Process exited)",
            "-",
            "(Suspend Process: Process exited)",
            "(Send Signal: Process exited)",
            "(Set Priority: Process exited)",
            "(Set Affinity…: Process exited)",
            "(Set I/O Priority: Process exited)",
            "-",
        ];
        exited.extend(COPIES);
        exited.extend([
            "-",
            "(Add Minigraph: Process exited)",
            "(Add Process Name Graph: Process exited)",
            "(Add tray icon graph: Process exited)",
            "-",
            "(Properties: Process exited)",
            "(Open file location: Process exited)",
            "(Open in VirusTotal: Process exited)",
            "(Create dump file: Process exited)",
            "-",
            "(Go to parent process: Process exited)",
        ]);
        assert_eq!(menu(&mut rig, &[4]), exited, "an exited row");

        let mut app = vec![
            "End App",
            "Kill App (all processes)",
            "-",
            "Freeze App",
            "Send Signal >",
            "Set Priority > [Applies to the app's current processes]",
            "Set Affinity…",
            "Set I/O Priority >",
            "-",
        ];
        app.extend(COPIES);
        app.extend([
            "-",
            "Add Minigraph >",
            "Add App Name Graph >",
            TRAY_ITEM,
            "-",
            "Properties",
        ]);
        assert_eq!(menu(&mut rig, &[7]), app, "an app row");
        let mut unit = app.clone();
        unit.splice(9..9, ["Restart unit \"foo.service\"", "-"]);
        unit.extend(["-", "Go to service"]);
        assert_eq!(menu(&mut rig, &[8]), unit, "a unit row");

        assert_eq!(
            menu(&mut rig, &[9]),
            ["Copy", "Copy details", "Copy all metrics"],
            "a pseudo row"
        );
        assert_eq!(
            menu(&mut rig, &[0]),
            ["Collapse"],
            "an expanded group header"
        );
    }

    /// Publishes `rows` and `groups` with one live firefox member, so an app row is not exited.
    fn publish_app_rows(rig: &mut Rig, rows: Vec<TableRow>, groups: Vec<GroupRow>) {
        let records = vec![ProcessRecord {
            group: app_key(),
            ..record(200, 1, "firefox", ProcFlags::empty())
        }];
        rig.publish_rows(records, vec![firefox_extras()], rows, groups);
    }

    /// The menu of row `ix` as text.
    fn menu_of(rig: &mut Rig, ix: usize) -> Vec<String> {
        let m = build_menu(rig, &[ix], ColumnId::Cpu);
        shape(rig, &m)
    }

    /// Selects the row at `ix`, whatever its kind.
    fn select_row(rig: &mut Rig, ix: usize) {
        let table = rig.vcx.update(|_, cx| rig.page.read(cx).table().clone());
        let id = rig.vcx.update(|_, cx| table.read(cx).delegate.row_id(ix));
        rig.select(&[id]);
    }

    /// Row 7 is the firefox app row and row 8 the unit row of `publish_kinds`.
    const APP_ROW: usize = 7;
    const UNIT_ROW: usize = 8;

    #[gpui_kit::test]
    fn app_row_menu_p2_items(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        let items = |shape: Vec<String>| shape[..5].to_vec();
        assert_eq!(
            items(menu_of(&mut rig, APP_ROW)),
            [
                "End App",
                "Kill App (all processes)",
                "-",
                "Freeze App",
                "Send Signal >"
            ],
            "an app row under app.slice"
        );
        assert_eq!(
            items(menu_of(&mut rig, UNIT_ROW))[3],
            "Freeze App",
            "a unit row"
        );

        // A frozen row reads Thaw App.
        let mut records_frozen = rig.apps.clone();
        records_frozen[0] = firefox_sample(true);
        rig.apps = records_frozen;
        let mut groups = vec![GroupRow {
            frozen: true,
            ..app_group(1)
        }];
        groups.push(GroupRow {
            node: NodeId::Unit(unit_ref()),
            kind: GroupRowKind::Unit,
            name: Arc::from("foo.service"),
            ..app_group(1)
        });
        let rows = vec![
            row(NodeId::Group(app_key()), None, Some(0)),
            row(NodeId::Unit(unit_ref()), None, Some(1)),
        ];
        publish_app_rows(&mut rig, rows, groups);
        assert_eq!(menu_of(&mut rig, 0)[3], "Thaw App", "a frozen app row");

        // No app.slice sample: both items are disabled, never a unit fallback.
        rig.apps = Vec::new();
        let rows = vec![row(NodeId::Group(app_key()), None, Some(0))];
        publish_app_rows(&mut rig, rows, vec![app_group(1)]);
        let tip = "This app has no app scope of its own";
        let shape = menu_of(&mut rig, 0);
        assert_eq!(shape[1], format!("(Kill App (all processes): {tip})"));
        assert_eq!(shape[3], format!("(Freeze App: {tip})"));
    }

    #[gpui_kit::test]
    fn freeze_targets_cgroup_or_unit_by_location(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        select_row(&mut rig, APP_ROW);
        assert!(rig.dispatch(FreezeApp).is_none(), "no dialog");
        select_row(&mut rig, UNIT_ROW);
        assert!(rig.dispatch(FreezeApp).is_none(), "no dialog");
        select_row(&mut rig, APP_ROW);
        assert!(rig.dispatch(ThawApp).is_none(), "no dialog");
        assert_eq!(
            rig.sent_targets(),
            [
                (
                    ActionKind::FreezeGroup,
                    vec![ActionTarget::Cgroup(app_cgroup())]
                ),
                (
                    ActionKind::FreezeGroup,
                    vec![ActionTarget::Unit(unit_ref())]
                ),
                (
                    ActionKind::ThawGroup,
                    vec![ActionTarget::Cgroup(app_cgroup())]
                ),
            ]
        );
        let id = actions_sent(&rig.engine)[0].id;
        emit(
            &rig.engine,
            &mut rig.vcx,
            id,
            Some(0),
            ActionState::Succeeded,
        );
        emit(&rig.engine, &mut rig.vcx, id, None, ActionState::Succeeded);
        assert_eq!(visible(&mut rig.vcx), ["App frozen"]);
    }

    #[gpui_kit::test]
    fn freeze_without_app_sample_sends_nothing(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        rig.apps = Vec::new();
        let rows = vec![row(NodeId::Group(app_key()), None, Some(0))];
        publish_app_rows(&mut rig, rows, vec![app_group(1)]);
        select_row(&mut rig, 0);
        assert!(rig.dispatch(FreezeApp).is_none());
        assert!(rig.dispatch(KillApp).is_none());
        assert!(rig.sent_targets().is_empty());
    }

    #[gpui_kit::test]
    fn freeze_protected_member_confirms(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        let protected = record(900, 1, "plasmashell", ProcFlags::PROTECTED);
        let records = vec![ProcessRecord {
            group: app_key(),
            ..protected
        }];
        let extras = vec![firefox_extras()];
        let rows = vec![
            row(NodeId::Group(app_key()), None, Some(0)),
            row(NodeId::Process(key(900)), Some(0), None),
        ];
        rig.publish_rows(records, extras, rows, vec![app_group(1)]);
        select_row(&mut rig, 0);
        let mut dialog = rig.dispatch(FreezeApp).expect("the Protected dialog");
        assert_eq!(
            dialog.text(),
            "Freezing Firefox will freeze your desktop session. Freeze it anyway?"
        );
        dialog.cancel();
        assert!(rig.sent_targets().is_empty(), "Cancel sends nothing");
    }

    #[gpui_kit::test]
    fn kill_app_confirms_with_count(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        let rows = vec![row(NodeId::Group(app_key()), None, Some(0))];
        publish_app_rows(&mut rig, rows, vec![app_group(14)]);
        select_row(&mut rig, 0);
        let mut dialog = rig.dispatch(KillApp).expect("the Kill App dialog");
        assert_eq!(dialog.text(), "Kill all 14 processes of Firefox?");
        dialog.cancel();
        assert!(rig.sent_targets().is_empty());

        let dialog = rig.dispatch(KillApp).expect("the Kill App dialog");
        dialog.confirm();
        assert_eq!(
            rig.sent_targets(),
            [(
                ActionKind::KillGroup,
                vec![ActionTarget::Cgroup(app_cgroup())]
            )]
        );
    }

    /// A firefox row with two app.slice samples: its own scope, and a terminal scope that
    /// also holds a process of another program.
    fn publish_shared_scope(rig: &mut Rig, keep_own_scope: bool) {
        let shared_path = "user.slice/user-1000.slice/user@1000.service/app.slice/app-term-1.scope";
        let in_cgroup = |r: ProcessRecord, path: &str| ProcessRecord {
            meta: Arc::new(ProcessMeta {
                cgroup: Some(Arc::from(path)),
                ..(*r.meta).clone()
            }),
            ..r
        };
        let own = in_cgroup(
            ProcessRecord {
                group: app_key(),
                ..record(200, 1, "firefox", ProcFlags::empty())
            },
            &app_cgroup().rel_path,
        );
        let helper = in_cgroup(
            ProcessRecord {
                group: app_key(),
                ..record(201, 1, "firefox-helper", ProcFlags::empty())
            },
            shared_path,
        );
        let terminal = in_cgroup(record(700, 1, "konsole", ProcFlags::empty()), shared_path);
        rig.apps = vec![AppCgroupSample {
            cgroup: CgroupRef {
                rel_path: Arc::from(shared_path),
                cgroup_id: 78,
            },
            ..firefox_sample(false)
        }];
        if keep_own_scope {
            rig.apps.insert(0, firefox_sample(false));
        }
        let rows = vec![row(NodeId::Group(app_key()), None, Some(0))];
        rig.publish_rows(
            vec![own, helper, terminal],
            vec![firefox_extras(); 3],
            rows,
            vec![app_group(2)],
        );
    }

    #[gpui_kit::test]
    fn kill_app_skips_scope_that_holds_other_programs(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        publish_shared_scope(&mut rig, true);
        select_row(&mut rig, 0);
        let mut dialog = rig.dispatch(KillApp).expect("the Kill App dialog");
        assert_eq!(
            dialog.text(),
            "Kill all 2 processes of Firefox? 1 of 2 groups skipped: it holds other programs."
        );
        dialog.confirm();
        assert_eq!(
            rig.sent_targets(),
            [(
                ActionKind::KillGroup,
                vec![ActionTarget::Cgroup(app_cgroup())]
            )]
        );
    }

    #[gpui_kit::test]
    fn freeze_skips_scope_that_holds_other_programs(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        publish_shared_scope(&mut rig, true);
        select_row(&mut rig, 0);
        assert!(rig.dispatch(FreezeApp).is_none(), "no dialog");
        assert_eq!(
            rig.sent_targets(),
            [(
                ActionKind::FreezeGroup,
                vec![ActionTarget::Cgroup(app_cgroup())]
            )]
        );
    }

    #[gpui_kit::test]
    fn app_row_with_only_shared_scopes_is_disabled(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        publish_shared_scope(&mut rig, false);
        let tip = "This app has no app scope of its own";
        let shape = menu_of(&mut rig, 0);
        assert_eq!(shape[1], format!("(Kill App (all processes): {tip})"));
        assert_eq!(shape[3], format!("(Freeze App: {tip})"));
        select_row(&mut rig, 0);
        assert!(rig.dispatch(KillApp).is_none());
        assert!(rig.sent_targets().is_empty());
    }

    #[gpui_kit::test]
    fn shift_delete_on_app_row_is_kill_app(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        let rows = vec![row(NodeId::Group(app_key()), None, Some(0))];
        publish_app_rows(&mut rig, rows, vec![app_group(14)]);
        select_row(&mut rig, 0);
        let page = rig.page.clone();
        rig.vcx
            .update(|window, cx| page.update(cx, |p, cx| p.focus_table(window, cx)));
        rig.vcx.update(|window, cx| window.draw(cx).clear(cx));
        let before = rig.windows();
        rig.vcx.simulate_keystrokes("shift-delete");
        rig.vcx.run_until_parked();
        assert!(
            rig.windows().iter().any(|w| !before.contains(w)),
            "the Kill App dialog opened"
        );
        assert!(rig.sent_targets().is_empty(), "nothing sent before confirm");
    }

    /// Shift+F10 on the focus row opens the menu without Copy: the keyboard names no cell.
    #[gpui_kit::test]
    fn keyboard_row_menu_has_no_copy_cell(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300, 301]), &[300, 301], Vec::new());
        rig.select_pids(&[301]);
        let page = rig.page.clone();
        rig.vcx
            .update(|window, cx| page.update(cx, |p, cx| p.focus_table(window, cx)));
        rig.vcx.update(|window, cx| window.draw(cx).clear(cx));
        rig.vcx.simulate_keystrokes("shift-f10");
        rig.vcx.run_until_parked();
        let menu = rig
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("the keyboard opened the menu");
        let mut expected = process_shape("Open file location", Some("Go to parent process"));
        expected.retain(|e| e != "Copy");
        assert_eq!(shape(&mut rig, &menu), expected);
        let keyboard = build_menu_from(&mut rig, &[1], None);
        assert_eq!(
            shape(&mut rig, &keyboard),
            expected,
            "None from the keyboard"
        );
    }

    /// A dismissed menu leaves no targets: a later key command acts on the new selection.
    #[gpui_kit::test]
    fn dismissed_menu_targets_do_not_leak_to_keyboard_commands(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300, 301]), &[300, 301], Vec::new());
        rig.select_pids(&[300]);
        let page = rig.page.clone();
        rig.vcx
            .update(|window, cx| page.update(cx, |p, cx| p.focus_table(window, cx)));
        rig.vcx.update(|window, cx| window.draw(cx).clear(cx));
        rig.vcx.simulate_keystrokes("shift-f10");
        rig.vcx.run_until_parked();
        assert!(
            rig.vcx
                .update(|window, cx| open_context_menu(window, cx))
                .is_some()
        );
        rig.vcx.simulate_keystrokes("escape");
        rig.vcx.run_until_parked();
        assert!(
            rig.vcx
                .update(|window, cx| open_context_menu(window, cx))
                .is_none()
        );

        rig.select_pids(&[301]);
        assert!(rig.dispatch(EndTask).is_none());
        assert_eq!(
            rig.sent(),
            [(ActionKind::Signal(Signal::Term), vec![301])],
            "the selected row, not the dismissed menu's row"
        );
    }

    /// A right-click on a CPU cell opens the menu with Copy, which copies that cell.
    #[gpui_kit::test]
    fn right_click_cpu_cell_copies_that_cell(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let records = (300..302)
            .map(|pid| ProcessRecord {
                cpu_pct: 1.5,
                ..record(pid, 1, "proc", ProcFlags::empty())
            })
            .collect();
        rig.publish(records, &[300, 301], Vec::new());
        rig.vcx.update(|window, cx| window.draw(cx).clear(cx));
        let table = rig.vcx.update(|_, cx| rig.page.read(cx).table().clone());
        let body = rig
            .vcx
            .debug_bounds("TlTreeTable-body")
            .expect("the table body");
        let (x, row_h) = rig.vcx.update(|_, cx| {
            let layout = table.read(cx).columns().clone();
            let pos = layout
                .visible_columns()
                .iter()
                .position(|c| *c == ColumnId::Cpu)
                .expect("CPU is shown");
            (
                layout.offsets()[pos] + px(4.),
                tl_ui::theme::metrics(cx).row_height,
            )
        });
        let at = point(body.origin.x + x, body.origin.y + row_h * 1.5);
        rig.vcx.simulate_event(gpui_kit::MouseDownEvent {
            position: at,
            modifiers: gpui_kit::Modifiers::none(),
            button: gpui_kit::MouseButton::Right,
            click_count: 1,
            first_mouse: false,
        });
        rig.vcx.run_until_parked();
        rig.vcx.update(|window, cx| window.draw(cx).clear(cx));
        let menu = rig
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("the right-click opened the menu");
        let copy = entries(&mut rig, &menu)
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Copy"))
            .and_then(|e| e.action)
            .expect("a Copy item");
        assert!(copy.partial_eq(&CopyCell { col: ColumnId::Cpu }));
        // Type-ahead "c" highlights Copy, the first item starting with C; Enter runs it in
        // the table's context.
        rig.vcx.simulate_keystrokes("c enter");
        rig.vcx.run_until_parked();
        assert_eq!(clipboard(&mut rig), "1.5%", "the CPU cell of row 301");
        let selected = rig
            .vcx
            .update(|_, cx| table.read(cx).selection.ids().collect::<Vec<_>>());
        assert_eq!(selected, [ProcRowId::Process(key(301))]);
    }

    fn service_unit(manager: Manager, name: &str, main_pid: u32) -> UnitRecord {
        let a = |s: &str| -> Arc<str> { Arc::from(s) };
        fn missing<T: Default>() -> Total<T> {
            Total {
                value: T::default(),
                validity: Validity::Missing,
            }
        }
        UnitRecord {
            unit: UnitRef {
                manager,
                name: a(name),
            },
            kind: UnitType::Service,
            description: a(name),
            status: ServiceStatus::Running,
            sub_state: a("running"),
            load_state: a("loaded"),
            startup: StartupType::Enabled,
            main_pid,
            user: a("root"),
            path: a(""),
            unit_file: None,
            active_since_us: None,
            triggered_by: Arc::from([]),
            control_group: None,
            can_reload: false,
            has_cgroup_procs: false,
            cgroup_pids: Arc::from([]),
            cpu_pct: missing(),
            memory_bytes: missing(),
            disk_bps: missing(),
            tasks: missing(),
        }
    }

    /// Publishes `units` with the service data first, then the rows of [`publish_kinds`]. The
    /// page only takes a new snapshot when a component it draws changes, so the rows publish
    /// carries the services in.
    fn publish_units(rig: &mut Rig, units: Vec<UnitRecord>) {
        rig.engine.publish_with(|s| {
            s.services = Arc::new(ServicesSnapshot {
                units: units.into(),
                ..ServicesSnapshot::default()
            });
        });
        publish_kinds(rig);
    }

    fn pid_units() -> Vec<UnitRecord> {
        vec![
            service_unit(Manager::User, "pipewire.service", 301),
            service_unit(Manager::System, "old.service", 4300),
        ]
    }

    /// The "Go to service" entry of the menu on `rows`, if any.
    fn go_to_service(rig: &mut Rig, rows: &[usize]) -> Option<MenuEntryInfo> {
        let menu = build_menu(rig, rows, ColumnId::Name);
        entries(rig, &menu)
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Go to service"))
    }

    #[gpui_kit::test]
    fn process_menu_go_to_service_on_unit_row(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        let menu = build_menu(&mut rig, &[8], ColumnId::Name);
        let last = entries(&mut rig, &menu).pop().expect("an entry");
        assert_eq!(last.label.as_deref(), Some("Go to service"));
        let action = last.action.expect("an action");
        assert!(action.partial_eq(&GoToService {
            manager: ManagerArg::System,
            name: "foo.service".into(),
            origin: JumpOrigin::Processes,
        }));
    }

    #[gpui_kit::test]
    fn process_menu_go_to_service_on_main_pid_row(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_units(&mut rig, pid_units());
        let menu = build_menu(&mut rig, &[2], ColumnId::Name);
        let last = entries(&mut rig, &menu).pop().expect("an entry");
        assert_eq!(last.label.as_deref(), Some("Go to service"));
        let action = last.action.expect("an action");
        assert!(action.partial_eq(&GoToService {
            manager: ManagerArg::User,
            name: "pipewire.service".into(),
            origin: JumpOrigin::Processes,
        }));
        let menu = build_menu(&mut rig, &[4], ColumnId::Name);
        let last = entries(&mut rig, &menu).pop().expect("an entry");
        assert_eq!(last.label.as_deref(), Some("Go to service"));
        assert!(matches!(last.kind, MenuEntryKind::Disabled));
        assert_eq!(last.tooltip.as_deref(), Some("Process exited"));
    }

    #[gpui_kit::test]
    fn process_menu_go_to_service_hidden_otherwise(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_units(&mut rig, pid_units());
        for rows in [&[1][..], &[7], &[9], &[0], &[1, 2]] {
            assert!(go_to_service(&mut rig, rows).is_none(), "rows {rows:?}");
        }
        assert!(go_to_service(&mut rig, &[2]).is_some());
        publish_units(&mut rig, Vec::new());
        assert!(go_to_service(&mut rig, &[2]).is_none(), "no services data");
    }

    #[gpui_kit::test]
    fn row_menu_hides_unshipped_phases(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        let unshipped = ["Show Window", "File properties"];
        for rows in [&[1][..], &[1, 2], &[4], &[5], &[7], &[8], &[9]] {
            let menu = build_menu(&mut rig, rows, ColumnId::Name);
            let mut labels = shape(&mut rig, &menu);
            assert!(labels.len() <= 30, "at most 30 items");
            labels.extend(submenu_shapes(&mut rig, &menu));
            for label in &labels {
                for word in unshipped {
                    assert!(!label.contains(word), "{label:?} on rows {rows:?}");
                }
            }
        }
    }

    /// Publishes `windows` (a Valid windows snapshot, the KWin bridge up) and `caps` together.
    fn publish_windows_caps(rig: &mut Rig, windows: Vec<WindowEntry>, caps: EngineCaps) {
        rig.engine.publish_with(|s| {
            Arc::make_mut(&mut s.health).caps = caps;
            s.windows = Arc::new(WindowsSnapshot {
                header: ComponentHeader {
                    version: 2,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                windows: windows.into(),
                pick: None,
            });
        });
        rig.vcx.run_until_parked();
    }

    fn menu_labels(rig: &mut Rig, ix: usize) -> Vec<String> {
        let m = build_menu(rig, &[ix], ColumnId::Name);
        shape(rig, &m)
    }

    #[gpui_kit::test]
    fn show_window_hidden_without_bridge_disabled_without_window(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        assert!(
            !menu_labels(&mut rig, 1)
                .iter()
                .any(|l| l.contains("Show Window")),
            "no bridge"
        );
        let windows = vec![
            kwin_window("{late}", 300, 1, "Back"),
            kwin_window("{top}", 300, 0, "Front"),
        ];
        publish_windows(&mut rig, windows);
        let with = menu_labels(&mut rig, 1);
        assert!(with.contains(&"Show Window".to_owned()), "{with:?}");
        let without = menu_labels(&mut rig, 2);
        assert!(
            without.contains(&"(Show Window: No window)".to_owned()),
            "{without:?}"
        );
        let exited = menu_labels(&mut rig, 4);
        assert!(
            !exited.iter().any(|l| l.contains("Show Window"))
                || exited.contains(&"(Show Window: Process exited)".to_owned())
        );

        // The built menu holds the captured targets the handler acts on.
        menu_labels(&mut rig, 1);
        assert!(rig.dispatch(ShowWindow).is_none());
        let sent = kwin_sent(&rig);
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            &sent[0],
            KWinCommand::Activate { internal_id } if &**internal_id == "{top}"
        ));
        assert_eq!(visible(&mut rig.vcx), ["Process window shown."]);
    }

    #[gpui_kit::test]
    fn restart_plasma_shell_toasts_and_in_progress(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(
            vec![record(700, 1, "plasmashell", ProcFlags::empty())],
            &[700],
            Vec::new(),
        );
        let labels = menu_labels(&mut rig, 0);
        assert!(
            labels.contains(&"Restart Plasma Shell".to_owned()),
            "{labels:?}"
        );
        let unit = UnitRef {
            manager: Manager::User,
            name: Arc::from("plasma-plasmashell.service"),
        };
        let restarts = |rig: &Rig| {
            actions_sent(&rig.engine)
                .into_iter()
                .filter(|r| r.kind == ActionKind::UnitRestart)
                .collect::<Vec<_>>()
        };
        rig.select_pids(&[700]);
        assert!(rig.dispatch(RestartUnit { unit: unit.clone() }).is_none());
        let sent = restarts(&rig);
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].targets.as_slice(),
            [ActionTarget::Unit(unit.clone())]
        );
        let id = sent[0].id;

        rig.select_pids(&[700]);
        assert!(rig.dispatch(RestartUnit { unit: unit.clone() }).is_none());
        assert_eq!(restarts(&rig).len(), 1, "nothing sent while pending");
        assert_eq!(
            visible(&mut rig.vcx),
            ["Plasma Shell restart is already in progress."]
        );

        emit(&rig.engine, &mut rig.vcx, id, None, ActionState::Started);
        assert!(visible(&mut rig.vcx).contains(&"Restarting Plasma Shell...".to_owned()));
        emit(
            &rig.engine,
            &mut rig.vcx,
            id,
            Some(0),
            ActionState::Succeeded,
        );
        emit(&rig.engine, &mut rig.vcx, id, None, ActionState::Succeeded);
        assert!(visible(&mut rig.vcx).contains(&"Plasma Shell restarted".to_owned()));

        rig.select_pids(&[700]);
        assert!(rig.dispatch(RestartUnit { unit }).is_none());
        assert_eq!(restarts(&rig).len(), 2, "sends again once done");
    }

    #[gpui_kit::test]
    fn restart_unit_label_for_mainpid(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_units(&mut rig, pid_units());
        let has = |labels: &[String], want: &str| labels.iter().any(|l| l == want);
        assert!(has(
            &menu_labels(&mut rig, 2),
            "Restart unit \"pipewire.service\""
        ));
        assert!(has(
            &menu_labels(&mut rig, 8),
            "Restart unit \"foo.service\""
        ));
        assert!(
            !menu_labels(&mut rig, 7)
                .iter()
                .any(|l| l.contains("Restart"))
        );
        assert!(
            !menu_labels(&mut rig, 1)
                .iter()
                .any(|l| l.contains("Restart"))
        );
        publish_units(
            &mut rig,
            vec![service_unit(
                Manager::System,
                "plasma-kwin_wayland.service",
                301,
            )],
        );
        assert!(
            !menu_labels(&mut rig, 2)
                .iter()
                .any(|l| l.contains("Restart"))
        );
    }

    #[gpui_kit::test]
    fn file_properties_hidden_without_filemanager1(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        assert!(
            !menu_labels(&mut rig, 1)
                .iter()
                .any(|l| l.contains("File properties")),
            "no FileManager1 owner"
        );
        publish_windows_caps(&mut rig, Vec::new(), EngineCaps::FILE_MANAGER1);
        let labels = menu_labels(&mut rig, 1);
        let at = |l: &[String], want: &str| l.iter().position(|x| x == want);
        assert_eq!(
            at(&labels, "File properties"),
            at(&labels, "Properties").map(|i| i + 1)
        );
        assert!(
            menu_labels(&mut rig, 6).contains(&"(File properties: Executable deleted)".to_owned())
        );
        assert!(
            !menu_labels(&mut rig, 7)
                .iter()
                .any(|l| l.contains("File properties"))
        );

        menu_labels(&mut rig, 1);
        assert!(rig.dispatch(FileProperties).is_none());
        let sent = actions_sent(&rig.engine);
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].kind,
            ActionKind::ShowInFileManager {
                path: Arc::from("/usr/bin/proc"),
                properties: true,
            }
        );
    }

    #[gpui_kit::test]
    fn priority_check_only_when_all_share(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let nice = |pid, nice| ProcessRecord {
            nice,
            ..record(pid, 1, "proc", ProcFlags::empty())
        };
        // Sorted by key, as the engine sends them. Rows 0 to 7 are pids 300 to 307, row 8 the
        // app. 304 and 306 are SCHED_IDLE, 305 SCHED_RR, 307 SCHED_DEADLINE, all at nice 0.
        let mut records = vec![ProcessRecord {
            group: app_key(),
            ..nice(200, 0)
        }];
        records.extend([(300, 0), (301, 5), (302, 5), (303, 3)].map(|(p, n)| nice(p, n)));
        records.extend((304..=307).map(|p| nice(p, 0)));
        let policy = |pid: u32| match pid {
            304 | 306 => SchedPolicy::Idle,
            305 => SchedPolicy::Rr,
            307 => SchedPolicy::Deadline,
            _ => SchedPolicy::Other,
        };
        let extras: Vec<ProcessExtras> = records
            .iter()
            .map(|r| ProcessExtras {
                policy: policy(r.key.pid),
                rt_prio: u8::from(r.key.pid == 305),
                ..firefox_extras()
            })
            .collect();
        let mut rows: Vec<TableRow> = (300..=307)
            .map(|pid| row(NodeId::Process(key(pid)), Some(pid - 299), None))
            .collect();
        rows.push(row(NodeId::Group(app_key()), None, Some(0)));
        rig.publish_rows(records.clone(), extras, rows.clone(), vec![app_group(1)]);
        let priority = |rig: &mut Rig, rows: &[usize]| {
            let menu = build_menu(rig, rows, ColumnId::Name);
            let sub = entries(rig, &menu)
                .into_iter()
                .find(|e| e.label.as_deref() == Some("Set Priority"))
                .and_then(|e| e.submenu)
                .expect("Set Priority submenu");
            shape(rig, &sub)
        };
        let classes = |checked: Option<&str>| -> Vec<String> {
            let note = "# With autogroup on, nice only orders processes within the same session.";
            let mut out = vec![note.to_owned()];
            let all = [
                "Realtime",
                "High",
                "Above normal",
                "Normal",
                "Below normal",
                "Low",
                "Idle",
            ];
            for class in all {
                out.push(if Some(class) == checked {
                    format!("* {class}")
                } else {
                    class.to_owned()
                });
            }
            out.push("-".to_owned());
            out.push("Custom…".to_owned());
            out
        };
        assert_eq!(priority(&mut rig, &[0, 1]), classes(None), "nice 0 and 5");
        assert_eq!(priority(&mut rig, &[1, 2]), classes(Some("Below normal")));
        assert_eq!(priority(&mut rig, &[0]), classes(Some("Normal")));
        assert_eq!(
            priority(&mut rig, &[3]),
            classes(None),
            "nice 3 is no class"
        );
        assert_eq!(priority(&mut rig, &[8]), classes(None), "an app row");
        assert_eq!(
            priority(&mut rig, &[4]),
            classes(Some("Idle")),
            "SCHED_IDLE at nice 0 is Idle, not Normal"
        );
        assert_eq!(
            priority(&mut rig, &[5]),
            classes(Some("Realtime")),
            "SCHED_RR at nice 0 is Realtime, not Normal"
        );
        assert_eq!(priority(&mut rig, &[4, 6]), classes(Some("Idle")));
        assert_eq!(
            priority(&mut rig, &[0, 4]),
            classes(None),
            "Normal and Idle"
        );
        assert_eq!(priority(&mut rig, &[7]), classes(None), "SCHED_DEADLINE");

        rig.publish_rows(records, Vec::new(), rows, vec![app_group(1)]);
        assert_eq!(
            priority(&mut rig, &[0]),
            classes(None),
            "no extras, so the policy is not known"
        );
    }

    #[gpui_kit::test]
    fn row_menu_key_hints_from_process_table_context(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.vcx
            .update(|_, cx| tl_ui::pages::processes::bind_keys_processes(cx));
        rig.publish(own(&[300]), &[300], Vec::new());
        let menu = build_menu(&mut rig, &[0], ColumnId::Name);
        let all = entries(&mut rig, &menu);
        let hint = |label: &str| {
            all.iter()
                .find(|e| e.label.as_deref().is_some_and(|l| l.starts_with(label)))
                .and_then(|e| e.hint.clone())
                .map(|h| h.to_string())
        };
        let format = |key: &str| Kbd::format(&Keystroke::parse(key).expect("a keystroke"));
        assert_eq!(hint("End Task"), Some(format("delete")));
        assert_eq!(hint("End Process"), Some(format("shift-delete")));
        assert_eq!(hint("Copy details"), Some(format("ctrl-c")));
    }

    #[gpui_kit::test]
    fn group_header_menu_toggles_its_header_not_the_focus_row(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let header = NodeId::Group(GroupKind::Background.key());
        let rows = vec![
            header_row(header.clone()),
            row(NodeId::Process(key(300)), Some(0), None),
            row(NodeId::Process(key(301)), Some(1), None),
        ];
        rig.publish_rows(own(&[300, 301]), Vec::new(), rows, Vec::new());
        rig.select_pids(&[301]);
        let table = rig.vcx.update(|_, cx| rig.page.read(cx).table().clone());
        let expanded = |rig: &mut Rig| {
            rig.vcx
                .update(|_, cx| table.read(cx).delegate.query().is_expanded(&header))
        };
        let before = expanded(&mut rig);

        let menu = build_menu(&mut rig, &[0], ColumnId::Name);
        assert_eq!(shape(&mut rig, &menu), ["Collapse"]);
        rig.vcx
            .update(|window, cx| open_context_menu_at(point(px(20.), px(20.)), window, cx, menu));
        rig.vcx.run_until_parked();
        rig.vcx.update(|window, cx| window.draw(cx).clear(cx));
        rig.vcx.simulate_keystrokes("down enter");
        rig.vcx.run_until_parked();

        assert_eq!(expanded(&mut rig), !before, "the header toggled");
        let (selected, focus) = rig.vcx.update(|_, cx| {
            let t = table.read(cx);
            (t.selection.ids().collect::<Vec<_>>(), t.selection.focus())
        });
        let row_301 = ProcRowId::Process(key(301));
        assert_eq!(selected, [row_301], "the selection is unchanged");
        assert_eq!(focus, Some(row_301), "the focus row is unchanged");
        assert!(rig.sent().is_empty(), "no action request");
    }

    /// PID 300 (parent PID 1) and its child 400.
    fn parent_and_child() -> Vec<ProcessRecord> {
        vec![
            record(300, 1, "parent", ProcFlags::empty()),
            record(400, 300, "child", ProcFlags::empty()),
        ]
    }

    fn query_generation(rig: &mut Rig) -> u64 {
        rig.vcx.update(|_, cx| {
            rig.page
                .read(cx)
                .table()
                .read(cx)
                .delegate
                .query()
                .generation()
        })
    }

    fn selected(rig: &mut Rig) -> Vec<ProcRowId> {
        rig.vcx
            .update(|_, cx| rig.page.read(cx).table().read(cx).selection.ids().collect())
    }

    #[gpui_kit::test]
    fn go_to_parent_selects_parent(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(parent_and_child(), &[300, 400], Vec::new());
        rig.select_pids(&[400]);
        let before = query_generation(&mut rig);
        assert!(rig.dispatch(GoToParent).is_none());
        assert!(
            query_generation(&mut rig) > before,
            "the jump expands the path"
        );
        // The view of the jump's query holds the parent.
        rig.publish(parent_and_child(), &[300, 400], Vec::new());
        assert_eq!(selected(&mut rig), [ProcRowId::Process(key(300))]);
        assert_eq!(visible(&mut rig.vcx), ["Switched to parent process"]);
        assert!(rig.sent().is_empty(), "no action request");
    }

    #[gpui_kit::test]
    fn go_to_parent_hidden_offers_show(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(parent_and_child(), &[300, 400], Vec::new());
        rig.select_pids(&[400]);
        let table = rig.vcx.update(|_, cx| rig.page.read(cx).table().clone());
        rig.vcx.update(|_, cx| {
            table.update(cx, |t, _| t.delegate.query_mut().set_filter("child".into()))
        });
        assert!(rig.dispatch(GoToParent).is_none());
        // The filtered view lacks the parent.
        rig.publish(parent_and_child(), &[400], Vec::new());
        assert_eq!(
            visible(&mut rig.vcx),
            ["Parent process not found in current view"]
        );
        assert_eq!(selected(&mut rig), [ProcRowId::Process(key(400))]);

        // The toast's Show button dispatches this: it clears the filter and asks again.
        let generation = query_generation(&mut rig);
        rig.vcx.dispatch_action(ProcessesShowHidden);
        rig.vcx.run_until_parked();
        let filter = rig
            .vcx
            .update(|_, cx| table.read(cx).delegate.query().filter().clone());
        assert_eq!(filter.as_ref(), "", "Show clears the filter");
        assert!(
            query_generation(&mut rig) > generation,
            "Show expands again"
        );
        rig.publish(parent_and_child(), &[300, 400], Vec::new());
        assert_eq!(selected(&mut rig), [ProcRowId::Process(key(300))]);
    }

    #[gpui_kit::test]
    fn go_to_parent_ppid_zero_does_nothing(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(
            vec![record(1, 0, "systemd", ProcFlags::PROTECTED)],
            &[1],
            Vec::new(),
        );
        rig.select_pids(&[1]);
        let before = query_generation(&mut rig);
        assert!(rig.dispatch(GoToParent).is_none());
        assert_eq!(query_generation(&mut rig), before, "no jump");
        assert!(visible(&mut rig.vcx).is_empty(), "no toast");

        // A parent PID with no live record toasts not found.
        rig.publish(own(&[300]), &[300], Vec::new());
        rig.select_pids(&[300]);
        assert!(rig.dispatch(GoToParent).is_none());
        assert_eq!(
            visible(&mut rig.vcx),
            ["Parent process not found in current view"]
        );
    }

    #[gpui_kit::test]
    fn open_file_location_sends_show_in_file_manager(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300]), &[300], Vec::new());
        rig.select_pids(&[300]);
        assert!(rig.dispatch(OpenFileLocation).is_none());
        let sent = actions_sent(&rig.engine);
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].kind,
            ActionKind::ShowInFileManager {
                path: Arc::from("/usr/bin/proc"),
                properties: false,
            }
        );
        assert!(sent[0].targets.is_empty(), "no targets");
        let id = sent[0].id;
        emit(&rig.engine, &mut rig.vcx, id, None, ActionState::Started);
        assert_eq!(visible(&mut rig.vcx), ["Opening file location"]);

        // Success keeps the Info toast; a failure shows the reason alone.
        emit(&rig.engine, &mut rig.vcx, id, None, ActionState::Succeeded);
        assert_eq!(visible(&mut rig.vcx), ["Opening file location"]);
        assert!(rig.dispatch(OpenFileLocation).is_none());
        let id = actions_sent(&rig.engine)[1].id;
        let err = ActionError::Unsupported;
        emit(
            &rig.engine,
            &mut rig.vcx,
            id,
            None,
            ActionState::Failed(err.clone()),
        );
        let toasts = visible(&mut rig.vcx);
        assert_eq!(
            toasts.last().map(String::as_str),
            Some(message(&err).as_ref())
        );
    }

    #[gpui_kit::test]
    fn row_menu_virustotal_calls_notice_flow(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300]), &[300], Vec::new());
        rig.select_pids(&[300]);
        let mut notice = rig.dispatch(OpenVirusTotal).expect("the notice opens");
        assert_eq!(
            notice.text(),
            "This opens virustotal.com with the file's hash. The file is not uploaded."
        );
        let hashes = |rig: &Rig| {
            rig.engine
                .commands()
                .iter()
                .filter(|c| matches!(c, Command::ComputeHashes(k) if *k == key(300)))
                .count()
        };
        assert_eq!(
            hashes(&rig),
            0,
            "no ComputeHashes before the notice is answered"
        );
        let at = notice
            .vcx
            .debug_bounds("TlDialogAction")
            .expect("Open button drawn");
        notice.vcx.simulate_click(at.center(), Modifiers::none());
        notice.vcx.run_until_parked();
        assert_eq!(hashes(&rig), 1, "ComputeHashes with the captured key");
    }

    #[gpui_kit::test]
    fn row_menu_dump_calls_start_dump(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300]), &[300], Vec::new());
        rig.select_pids(&[300]);
        assert!(
            rig.dispatch(CreateDump).is_none(),
            "no confirmation under 2 GiB"
        );
        let dumps: Vec<_> = actions_sent(&rig.engine)
            .into_iter()
            .filter(|r| matches!(r.kind, ActionKind::Dump { .. }))
            .collect();
        assert_eq!(dumps.len(), 1, "{dumps:?}");
        assert!(matches!(
            dumps[0].targets.as_slice(),
            [ActionTarget::Process(k)] if *k == key(300)
        ));
    }

    #[gpui_kit::test]
    fn open_file_location_restricted_toasts(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let mut restricted = record(300, 1, "proc", ProcFlags::empty());
        Arc::make_mut(&mut restricted.meta).exe = None;
        let mut deleted = record(301, 1, "proc", ProcFlags::empty());
        Arc::make_mut(&mut deleted.meta).exe = Some(Arc::from("/usr/bin/proc (deleted)"));
        rig.publish(vec![restricted, deleted], &[300, 301], Vec::new());
        rig.select_pids(&[300]);
        assert!(rig.dispatch(OpenFileLocation).is_none());
        assert_eq!(visible(&mut rig.vcx), ["Process image unavailable"]);
        assert!(actions_sent(&rig.engine).is_empty(), "nothing is sent");

        // A deleted executable: the menu item is disabled and the handler does nothing.
        let menu = build_menu(&mut rig, &[1], ColumnId::Name);
        assert!(
            shape(&mut rig, &menu).contains(&"(Open file location: Executable deleted)".into())
        );
        rig.select_pids(&[301]);
        assert!(rig.dispatch(OpenFileLocation).is_none());
        assert!(actions_sent(&rig.engine).is_empty());
        assert_eq!(visible(&mut rig.vcx), ["Process image unavailable"]);
    }

    #[gpui_kit::test]
    fn set_affinity_item_position_and_states(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        publish_kinds(&mut rig);
        let at = |shape: &[String]| shape.iter().position(|e| e.starts_with("Set Affinity"));
        let one = {
            let m = build_menu(&mut rig, &[1], ColumnId::Name);
            shape(&mut rig, &m)
        };
        let priority = one
            .iter()
            .position(|e| e == "Set Priority >")
            .expect("item 6");
        assert_eq!(at(&one), Some(priority + 1), "item 7 follows Set Priority");
        let menu = build_menu(&mut rig, &[1], ColumnId::Name);
        assert!(
            entries(&mut rig, &menu)
                .iter()
                .find(|e| e.label.as_deref() == Some("Set Affinity…"))
                .is_some_and(|e| e.hint.is_none()),
            "no key hint"
        );
        let shape_of = |rig: &mut Rig, rows: &[usize]| {
            let m = build_menu(rig, rows, ColumnId::Name);
            shape(rig, &m)
        };
        assert!(
            at(&shape_of(&mut rig, &[1, 2])).is_some(),
            "built on two rows, same label (P2, E23-005)"
        );
        assert_eq!(at(&shape_of(&mut rig, &[9])), None, "not on a pseudo row");
        let exited = shape_of(&mut rig, &[4]);
        assert!(exited.contains(&"(Set Affinity…: Process exited)".to_owned()));
        let app = shape_of(&mut rig, &[7]);
        assert!(
            app.contains(&"Set Affinity…".to_owned()),
            "same label on an app row"
        );
    }

    #[gpui_kit::test]
    fn set_affinity_action_opens_pwn021_with_captured_targets(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300, 301]), &[300, 301], Vec::new());
        rig.select_pids(&[300]);
        // The menu opens on row A (PID 300), then the rows re-sort and the selection moves.
        let _menu = build_menu(&mut rig, &[0], ColumnId::Name);
        rig.publish(own(&[300, 301]), &[301, 300], Vec::new());
        rig.select_pids(&[301]);
        let before = rig.windows();
        assert!(rig.dispatch(SetAffinity).is_none());
        let id = rig
            .windows()
            .into_iter()
            .find(|w| !before.contains(w))
            .expect("the Set Affinity dialog opened");
        let handle = WindowHandle::<gpui_kit::base::Root>::new(id);
        let (targets, subtitle) = rig.vcx.update(|_, cx| {
            let dialog = AffinityDialog::find(handle, cx).expect("an affinity dialog");
            let d = dialog.read(cx);
            (d.targets().to_vec(), d.subtitle(cx).to_string())
        });
        assert_eq!(targets, [ActionTarget::Process(key(300))], "row A's key");
        assert!(subtitle.starts_with("proc (PID 300)"), "{subtitle}");
        assert!(rig.sent().is_empty(), "the dialog sends on OK only");
    }

    #[gpui_kit::test]
    fn set_affinity_on_two_rows_opens_with_both(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300, 301]), &[300, 301], Vec::new());
        rig.select_pids(&[300, 301]);
        let before = rig.windows();
        assert!(rig.dispatch(SetAffinity).is_none());
        let id = rig
            .windows()
            .into_iter()
            .find(|w| !before.contains(w))
            .expect("the Set Affinity dialog opened");
        let handle = WindowHandle::<gpui_kit::base::Root>::new(id);
        let (targets, subtitle) = rig.vcx.update(|_, cx| {
            let dialog = AffinityDialog::find(handle, cx).expect("an affinity dialog");
            let d = dialog.read(cx);
            (d.targets().to_vec(), d.subtitle(cx).to_string())
        });
        assert_eq!(
            targets,
            [
                ActionTarget::Process(key(300)),
                ActionTarget::Process(key(301))
            ]
        );
        assert!(subtitle.starts_with("2 processes"), "{subtitle}");
    }

    /// The Custom priority dialog `rig.dispatch(OpenCustomPriority)` opened: its targets,
    /// subtitle and initial values.
    fn open_custom_priority(rig: &mut Rig) -> (Vec<ActionTarget>, String, (SchedPolicy, i8, u8)) {
        let before = rig.windows();
        rig.dispatch(OpenCustomPriority);
        let id = rig
            .windows()
            .into_iter()
            .find(|w| !before.contains(w))
            .expect("the Custom priority dialog opened");
        let handle = WindowHandle::<gpui_kit::base::Root>::new(id);
        rig.vcx.update(|_, cx| {
            let dialog = PriorityDialog::find(handle, cx).expect("a priority dialog");
            let d = dialog.read(cx);
            (d.targets().to_vec(), d.subtitle().to_string(), d.initial())
        })
    }

    #[gpui_kit::test]
    fn custom_priority_opens_pwn023(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let records = vec![
            ProcessRecord {
                nice: 5,
                ..record(300, 1, "proc", ProcFlags::empty())
            },
            record(301, 1, "proc", ProcFlags::empty()),
        ];
        let extras = vec![
            ProcessExtras {
                policy: SchedPolicy::Batch,
                rt_prio: 0,
                ..firefox_extras()
            },
            firefox_extras(),
        ];
        let rows = vec![
            row(NodeId::Process(key(300)), Some(0), None),
            row(NodeId::Process(key(301)), Some(1), None),
        ];
        rig.publish_rows(records.clone(), extras.clone(), rows, Vec::new());
        let menu = build_menu(&mut rig, &[0], ColumnId::Name);
        let priority_entry = entries(&mut rig, &menu)
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Set Priority"))
            .and_then(|e| e.submenu)
            .expect("the Set Priority submenu");
        let priority = shape(&mut rig, &priority_entry);
        assert_eq!(
            &priority[priority.len() - 2..],
            ["-", "Custom…"],
            "Custom… is last, after a separator"
        );
        // The menu opened on row A (PID 300); then the rows re-sort and the selection moves.
        let rows = vec![
            row(NodeId::Process(key(301)), Some(1), None),
            row(NodeId::Process(key(300)), Some(0), None),
        ];
        rig.publish_rows(records, extras, rows, Vec::new());
        rig.select_pids(&[301]);
        let (targets, subtitle, initial) = open_custom_priority(&mut rig);
        assert_eq!(targets, [ActionTarget::Process(key(300))], "row A's key");
        assert_eq!(subtitle, "proc (PID 300)");
        assert_eq!(
            initial,
            (SchedPolicy::Batch, 5, 0),
            "row A's Priority cell values"
        );
        assert!(
            rig.sent().is_empty(),
            "the dialog sends on Set priority only"
        );
    }

    #[gpui_kit::test]
    fn custom_priority_app_row_targets(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let records: Vec<ProcessRecord> = (0..34)
            .map(|n| ProcessRecord {
                group: app_key(),
                ..record(200 + n, 1, "firefox", ProcFlags::empty())
            })
            .collect();
        rig.publish(records, &[], vec![app_group(34)]);
        rig.select(&[ProcRowId::Group(app_key())]);
        let (targets, subtitle, _) = open_custom_priority(&mut rig);
        assert_eq!(targets.len(), 34);
        assert_eq!(
            targets,
            (200..234)
                .map(|pid| ActionTarget::Process(key(pid)))
                .collect::<Vec<_>>()
        );
        assert_eq!(subtitle, "Firefox (34 processes)");
    }

    fn stopped(pid: u32) -> ProcessRecord {
        ProcessRecord {
            state: ProcState::Stopped,
            ..record(pid, 1, "proc", ProcFlags::empty())
        }
    }

    fn other_user_sshd(pid: u32) -> ProcessRecord {
        let mut sshd = record(pid, 1, "sshd", ProcFlags::OTHER_USER);
        sshd.uid = 0;
        Arc::make_mut(&mut sshd.meta).user = Arc::from("root");
        sshd
    }

    /// Emits `state` for the last request sent and returns the visible toasts.
    fn result(rig: &mut Rig, state: ActionState) -> Vec<String> {
        let id = actions_sent(&rig.engine).pop().expect("a request").id;
        emit(&rig.engine, &mut rig.vcx, id, None, state);
        visible(&mut rig.vcx)
    }

    #[gpui_kit::test]
    fn suspend_resume_requests_and_toasts(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let mut records = own(&[300, 301]);
        records.push(other_user_sshd(812));
        records.push(record(900, 1, "plasmashell", ProcFlags::PROTECTED));
        rig.publish(records, &[300, 301, 812, 900], Vec::new());

        // A running own process: no dialog, one Suspend request.
        rig.select_pids(&[300]);
        assert!(rig.dispatch(SuspendProcess).is_none(), "no dialog");
        assert_eq!(rig.sent(), [(ActionKind::Suspend, vec![300])]);
        assert_eq!(
            result(&mut rig, ActionState::Succeeded),
            ["Process suspended."]
        );

        rig.select_pids(&[300]);
        assert!(rig.dispatch(ResumeProcess).is_none(), "no dialog");
        assert_eq!(rig.sent()[1], (ActionKind::Resume, vec![300]));
        assert!(result(&mut rig, ActionState::Succeeded).contains(&"Process resumed.".into()));

        // Several own processes still do not ask.
        rig.select_pids(&[300, 301]);
        assert!(rig.dispatch(SuspendProcess).is_none(), "no dialog");
        assert_eq!(rig.sent()[2], (ActionKind::Suspend, vec![300, 301]));

        // Another user's process: a non-destructive OtherUser dialog.
        rig.select_pids(&[812]);
        let mut dialog = rig.dispatch(ResumeProcess).expect("the OtherUser dialog");
        assert_eq!(
            dialog.text(),
            "Resume on sshd (PID 812), owned by root. Continue?"
        );
        assert!(!dialog.is_destructive());
        dialog.enter();
        assert_eq!(rig.sent()[3], (ActionKind::Resume, vec![812]));

        // A protected target: Suspend asks, Resume does not.
        rig.select_pids(&[900]);
        let dialog = rig.dispatch(SuspendProcess).expect("the Protected dialog");
        dialog.cancel();
        assert_eq!(rig.sent().len(), 4, "Cancel sends nothing");
        rig.select_pids(&[900]);
        assert!(rig.dispatch(ResumeProcess).is_none(), "no dialog");
        assert_eq!(rig.sent()[4], (ActionKind::Resume, vec![900]));
    }

    #[gpui_kit::test]
    fn suspend_or_resume_label_rule(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let records = vec![
            stopped(300),
            stopped(301),
            record(302, 1, "proc", ProcFlags::empty()),
        ];
        rig.publish(records, &[300, 301, 302], Vec::new());
        let label = |rig: &mut Rig, pids: &[u32]| {
            rig.select_pids(pids);
            let page = rig.page.clone();
            rig.vcx
                .update(|_, cx| suspend_or_resume(&page.read(cx).capture_targets(None, cx)))
        };
        assert_eq!(label(&mut rig, &[300, 301, 302]), SuspendLabel::Suspend);
        assert_eq!(label(&mut rig, &[300, 301]), SuspendLabel::Resume);
        assert_eq!(label(&mut rig, &[300]), SuspendLabel::Resume);
        assert_eq!(label(&mut rig, &[302]), SuspendLabel::Suspend);
    }

    #[gpui_kit::test]
    fn priority_classes_map_to_nice(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300, 301]), &[300, 301], Vec::new());
        let classes = [
            (PriorityClass::High, -10),
            (PriorityClass::AboveNormal, -5),
            (PriorityClass::Normal, 0),
            (PriorityClass::BelowNormal, 5),
            (PriorityClass::Low, 19),
        ];
        for (class, nice) in classes {
            rig.select_pids(&[300]);
            assert!(rig.dispatch(SetPriority { class }).is_none(), "no dialog");
            assert_eq!(
                rig.sent().pop(),
                Some((ActionKind::SetNice(nice), vec![300])),
                "{class:?}"
            );
        }
        assert_eq!(
            result(&mut rig, ActionState::Succeeded)
                .last()
                .map(String::as_str),
            Some("Priority updated")
        );

        // Several targets: one request, the multi texts.
        rig.select_pids(&[300, 301]);
        assert!(
            rig.dispatch(SetPriority {
                class: PriorityClass::Low
            })
            .is_none()
        );
        assert_eq!(
            rig.sent().pop(),
            Some((ActionKind::SetNice(19), vec![300, 301]))
        );

        // Another user's process: OtherUser with the primary "Set priority" button.
        rig.publish(vec![other_user_sshd(812)], &[812], Vec::new());
        rig.select_pids(&[812]);
        let mut dialog = rig
            .dispatch(SetPriority {
                class: PriorityClass::Low,
            })
            .expect("the OtherUser dialog");
        assert_eq!(
            dialog.text(),
            "Set priority on sshd (PID 812), owned by root. Continue?"
        );
        assert!(!dialog.is_destructive(), "primary, not danger");
        assert_eq!(dialog.focused(), Some(DialogButton::Action));
        dialog.enter();
        assert_eq!(rig.sent().pop(), Some((ActionKind::SetNice(19), vec![812])));
    }

    #[gpui_kit::test]
    fn realtime_confirms_then_sets_rr1(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300]), &[300], Vec::new());
        let realtime = SetPriority {
            class: PriorityClass::Realtime,
        };
        rig.select_pids(&[300]);
        let mut dialog = rig
            .dispatch(realtime.clone())
            .expect("the Realtime dialog, even for an own process");
        assert_eq!(
            dialog.text(),
            "Realtime priority can freeze the desktop if the process spins. Set it anyway?"
        );
        assert!(dialog.is_destructive());
        assert_eq!(dialog.focused(), Some(DialogButton::Cancel));
        dialog.cancel();
        assert!(rig.sent().is_empty(), "Cancel sends nothing");

        rig.select_pids(&[300]);
        rig.dispatch(realtime.clone())
            .expect("the Realtime dialog")
            .confirm();
        let rr1 = ActionKind::SetSched {
            policy: SchedPolicy::Rr,
            rt_prio: 1,
        };
        assert_eq!(rig.sent(), [(rr1.clone(), vec![300])]);
        assert_eq!(
            result(&mut rig, ActionState::Succeeded)
                .last()
                .map(String::as_str),
            Some("Priority updated")
        );

        // Another user's process still gets the Realtime dialog, not OtherUser.
        rig.publish(vec![other_user_sshd(812)], &[812], Vec::new());
        rig.select_pids(&[812]);
        let mut dialog = rig.dispatch(realtime).expect("the Realtime dialog");
        assert_eq!(
            dialog.text(),
            "Realtime priority can freeze the desktop if the process spins. Set it anyway?"
        );
        dialog.confirm();
        assert_eq!(rig.sent().pop(), Some((rr1, vec![812])));
    }

    #[gpui_kit::test]
    fn idle_sets_sched_idle(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let member = |pid| ProcessRecord {
            group: app_key(),
            ..record(pid, 1, "firefox", ProcFlags::empty())
        };
        let mut records = own(&[300, 301]);
        records.extend([member(200), member(201)]);
        rig.publish(records, &[300, 301], vec![app_group(2)]);
        let idle = SetPriority {
            class: PriorityClass::Idle,
        };
        let sched_idle = ActionKind::SetSched {
            policy: SchedPolicy::Idle,
            rt_prio: 0,
        };
        rig.select_pids(&[300]);
        assert!(rig.dispatch(idle.clone()).is_none(), "no dialog");
        assert_eq!(rig.sent(), [(sched_idle.clone(), vec![300])]);

        rig.select_pids(&[300, 301]);
        assert!(rig.dispatch(idle.clone()).is_none());
        assert_eq!(rig.sent().pop(), Some((sched_idle.clone(), vec![300, 301])));
        assert_eq!(
            result(&mut rig, ActionState::Succeeded)
                .last()
                .map(String::as_str),
            Some("Selected process priority updated.")
        );

        rig.select(&[ProcRowId::Group(app_key())]);
        assert!(rig.dispatch(idle).is_none());
        assert_eq!(
            rig.sent().pop(),
            Some((sched_idle, vec![200, 201])),
            "every current member"
        );
    }

    /// Publishes the helper state and one disk per (system disk, scheduler) pair.
    fn publish_ioprio_inputs(rig: &mut Rig, helper: HelperState, disks: &[(bool, &str)]) {
        fn valid<T>(value: T) -> Total<T> {
            Total {
                value,
                validity: Validity::Valid,
            }
        }
        let disks: Vec<(DeviceIdx, DiskDetails)> = disks
            .iter()
            .enumerate()
            .map(|(i, (system, sched))| {
                let details = DiskDetails {
                    system_disk: valid(*system),
                    scheduler: valid(Arc::from(*sched)),
                    ..DiskDetails::default()
                };
                (DeviceIdx(i as u16), details)
            })
            .collect();
        rig.engine.publish_with(|s| {
            let health = Arc::make_mut(&mut s.health);
            health.helper = helper;
            health.version += 1;
            s.device_details = Arc::new(DeviceDetailsSnapshot {
                disks: disks.into(),
                ..DeviceDetailsSnapshot::default()
            });
        });
        rig.vcx.run_until_parked();
    }

    /// The Set I/O Priority submenu of the rows at `rows`.
    fn ioprio_submenu(rig: &mut Rig, rows: &[usize]) -> Entity<TlMenuPopup> {
        let menu = build_menu(rig, rows, ColumnId::Name);
        entries(rig, &menu)
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Set I/O Priority"))
            .and_then(|e| e.submenu)
            .expect("Set I/O Priority submenu")
    }

    /// Publishes, for each record, the I/O priority `prio` gives its pid, as the engine fills
    /// `extras_p2` for visible rows. A pid `prio` gives `None` for has no entry key. The table
    /// version goes up, as with every sweep, so the page takes the new snapshot.
    fn publish_io_priority(rig: &mut Rig, prio: impl Fn(u32) -> Option<Option<IoPriority>>) {
        rig.version += 1;
        let version = rig.version;
        rig.engine.publish_with(|s| {
            Arc::make_mut(&mut s.table).header.version = version;
            let p = Arc::make_mut(&mut s.processes);
            p.extras_p2 = p
                .records
                .iter()
                .map(|r| match prio(r.key.pid) {
                    Some(io_priority) => ProcessExtrasP2 {
                        key: Some(r.key),
                        io_priority,
                        ..ProcessExtrasP2::default()
                    },
                    None => ProcessExtrasP2::default(),
                })
                .collect();
        });
        rig.vcx.run_until_parked();
    }

    /// The submenu shape with `realtime` as its Realtime entry and `scheduler` as its note.
    fn ioprio_shape(scheduler: Option<&str>, realtime: &str) -> Vec<String> {
        let mut out: Vec<String> = scheduler
            .map(|s| format!("# Scheduler: {s}"))
            .into_iter()
            .collect();
        out.push(realtime.to_owned());
        out.extend(
            [
                "High",
                "Normal",
                "Low",
                "Idle",
                "-",
                "Default (follows nice)",
            ]
            .map(String::from),
        );
        out
    }

    #[gpui_kit::test]
    fn ioprio_realtime_disabled_without_helper(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300, 301]), &[300, 301], Vec::new());
        let no_session = "(Realtime: Needs administrator access.)";

        publish_ioprio_inputs(
            &mut rig,
            HelperState::Locked,
            &[(true, "bfq"), (false, "none")],
        );
        let sub = ioprio_submenu(&mut rig, &[0]);
        assert_eq!(shape(&mut rig, &sub), ioprio_shape(Some("bfq"), no_session));
        let sub = ioprio_submenu(&mut rig, &[0, 1]);
        assert_eq!(
            shape(&mut rig, &sub),
            ioprio_shape(Some("bfq"), no_session),
            "nothing is checked on several rows"
        );

        publish_ioprio_inputs(&mut rig, HelperState::Starting, &[(true, "bfq")]);
        let sub = ioprio_submenu(&mut rig, &[0]);
        assert_eq!(shape(&mut rig, &sub), ioprio_shape(Some("bfq"), no_session));

        publish_ioprio_inputs(&mut rig, HelperState::Missing, &[(true, "bfq")]);
        let sub = ioprio_submenu(&mut rig, &[0]);
        assert_eq!(
            shape(&mut rig, &sub),
            ioprio_shape(
                Some("bfq"),
                "(Realtime: Needs administrator helper (not installed))"
            )
        );

        publish_ioprio_inputs(&mut rig, HelperState::Unlocked, &[(true, "bfq")]);
        let sub = ioprio_submenu(&mut rig, &[0]);
        assert_eq!(shape(&mut rig, &sub), ioprio_shape(Some("bfq"), "Realtime"));

        // Two system disks (`/` and `/boot`): the line only when they agree.
        let two = [(true, "mq-deadline"), (true, "mq-deadline")];
        publish_ioprio_inputs(&mut rig, HelperState::Unlocked, &two);
        let sub = ioprio_submenu(&mut rig, &[0]);
        assert_eq!(
            shape(&mut rig, &sub),
            ioprio_shape(Some("mq-deadline"), "Realtime")
        );
        let two = [(true, "bfq"), (true, "mq-deadline")];
        publish_ioprio_inputs(&mut rig, HelperState::Unlocked, &two);
        let sub = ioprio_submenu(&mut rig, &[0]);
        assert_eq!(shape(&mut rig, &sub), ioprio_shape(None, "Realtime"));
        publish_ioprio_inputs(&mut rig, HelperState::Unlocked, &[(false, "bfq")]);
        let sub = ioprio_submenu(&mut rig, &[0]);
        assert_eq!(
            shape(&mut rig, &sub),
            ioprio_shape(None, "Realtime"),
            "no system disk"
        );
    }

    #[gpui_kit::test]
    fn ioprio_classes_map_and_check(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let member = |pid| ProcessRecord {
            group: app_key(),
            ..record(pid, 1, "firefox", ProcFlags::empty())
        };
        let mut records = own(&[300, 301]);
        records.extend([member(200), member(201)]);
        rig.publish(records, &[300, 301], vec![app_group(2)]);
        publish_ioprio_inputs(&mut rig, HelperState::Unlocked, &[(true, "bfq")]);

        // Each menu item carries its class.
        let sub = ioprio_submenu(&mut rig, &[0]);
        let classes: Vec<IoClass> = entries(&mut rig, &sub)
            .iter()
            .filter_map(|e| e.action.as_ref())
            .filter_map(|a| a.as_any().downcast_ref::<SetIoPriorityClass>())
            .map(|a| a.class)
            .collect();
        assert_eq!(
            classes,
            [
                IoClass::Realtime,
                IoClass::High,
                IoClass::Normal,
                IoClass::Low,
                IoClass::Idle,
                IoClass::Default
            ]
        );

        // The current class has a check mark; with several targets only when all share it
        // (PROC-068, PROC-069). Rows 0 and 1 are pids 300 and 301, row 2 the app.
        let checked = |rig: &mut Rig, rows: &[usize]| {
            let sub = ioprio_submenu(rig, rows);
            shape(rig, &sub)
        };
        let marked = |class: Option<&str>| -> Vec<String> {
            ioprio_shape(Some("bfq"), "Realtime")
                .into_iter()
                .map(|e| match class {
                    Some(c) if c == e => format!("* {e}"),
                    _ => e,
                })
                .collect()
        };
        let both = |a: IoPriority, b: IoPriority| {
            move |pid: u32| match pid {
                300 => Some(Some(a)),
                301 => Some(Some(b)),
                _ => Some(Some(IoPriority::BestEffort(4))),
            }
        };
        let one_class = [
            (IoPriority::Realtime(0), "Realtime"),
            (IoPriority::Realtime(3), "Realtime"),
            (IoPriority::BestEffort(0), "High"),
            (IoPriority::BestEffort(4), "Normal"),
            (IoPriority::BestEffort(7), "Low"),
            (IoPriority::Idle, "Idle"),
            (IoPriority::None, "Default (follows nice)"),
        ];
        for (prio, label) in one_class {
            publish_io_priority(&mut rig, both(prio, prio));
            assert_eq!(checked(&mut rig, &[0]), marked(Some(label)), "{prio:?}");
            assert_eq!(checked(&mut rig, &[0, 1]), marked(Some(label)), "{prio:?}");
        }
        for level in [1, 2, 3, 5, 6] {
            publish_io_priority(
                &mut rig,
                both(IoPriority::BestEffort(level), IoPriority::Idle),
            );
            assert_eq!(checked(&mut rig, &[0]), marked(None), "best effort {level}");
        }
        publish_io_priority(&mut rig, both(IoPriority::Idle, IoPriority::BestEffort(4)));
        assert_eq!(checked(&mut rig, &[1]), marked(Some("Normal")));
        assert_eq!(checked(&mut rig, &[0, 1]), marked(None), "classes differ");
        publish_io_priority(&mut rig, both(IoPriority::None, IoPriority::BestEffort(4)));
        assert_eq!(
            checked(&mut rig, &[0, 1]),
            marked(None),
            "Default and Normal differ"
        );
        assert_eq!(checked(&mut rig, &[2]), marked(None), "an app row");
        publish_io_priority(&mut rig, |pid| {
            (pid != 301).then_some(Some(IoPriority::Idle))
        });
        assert_eq!(checked(&mut rig, &[0]), marked(Some("Idle")));
        assert_eq!(checked(&mut rig, &[1]), marked(None), "not read");
        assert_eq!(checked(&mut rig, &[0, 1]), marked(None), "one not read");
        publish_io_priority(&mut rig, |_| Some(None));
        assert_eq!(checked(&mut rig, &[0]), marked(None), "read failed");

        // A disabled Realtime item still shows the check mark.
        publish_io_priority(&mut rig, both(IoPriority::Realtime(0), IoPriority::Idle));
        publish_ioprio_inputs(&mut rig, HelperState::Locked, &[(true, "bfq")]);
        let sub = ioprio_submenu(&mut rig, &[0]);
        let realtime = entries(&mut rig, &sub)
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Realtime"))
            .expect("Realtime entry");
        assert_eq!(realtime.kind, MenuEntryKind::Disabled);
        assert!(realtime.checked);
        publish_ioprio_inputs(&mut rig, HelperState::Unlocked, &[(true, "bfq")]);

        let map = [
            (IoClass::Realtime, IoPriority::Realtime(0)),
            (IoClass::High, IoPriority::BestEffort(0)),
            (IoClass::Normal, IoPriority::BestEffort(4)),
            (IoClass::Low, IoPriority::BestEffort(7)),
            (IoClass::Idle, IoPriority::Idle),
            (IoClass::Default, IoPriority::None),
        ];
        for (class, model) in map {
            rig.select_pids(&[300]);
            assert!(
                rig.dispatch(SetIoPriorityClass { class }).is_none(),
                "no dialog"
            );
            assert_eq!(
                rig.sent().pop(),
                Some((ActionKind::SetIoPriority(model), vec![300])),
                "{class:?}"
            );
        }

        rig.select_pids(&[300]);
        let normal = SetIoPriorityClass {
            class: IoClass::Normal,
        };
        assert!(rig.dispatch(normal.clone()).is_none());
        assert_eq!(
            rig.sent().pop(),
            Some((
                ActionKind::SetIoPriority(IoPriority::BestEffort(4)),
                vec![300]
            ))
        );
        assert_eq!(
            result(&mut rig, ActionState::Succeeded)
                .last()
                .map(String::as_str),
            Some("I/O priority updated")
        );

        // Several targets and an app row: one request, every current member.
        rig.select_pids(&[300, 301]);
        assert!(rig.dispatch(normal.clone()).is_none());
        assert_eq!(
            rig.sent().pop(),
            Some((
                ActionKind::SetIoPriority(IoPriority::BestEffort(4)),
                vec![300, 301]
            ))
        );
        assert_eq!(
            result(&mut rig, ActionState::Succeeded)
                .last()
                .map(String::as_str),
            Some("Selected process I/O priority updated.")
        );
        rig.select(&[ProcRowId::Group(app_key())]);
        assert!(rig.dispatch(normal.clone()).is_none());
        assert_eq!(
            rig.sent().pop(),
            Some((
                ActionKind::SetIoPriority(IoPriority::BestEffort(4)),
                vec![200, 201]
            ))
        );

        // Another user's process asks first, with the primary "Set I/O priority" button.
        rig.publish(vec![other_user_sshd(812)], &[812], Vec::new());
        rig.select_pids(&[812]);
        let mut dialog = rig.dispatch(normal).expect("the OtherUser dialog");
        assert_eq!(
            dialog.text(),
            "Set I/O priority on sshd (PID 812), owned by root. Continue?"
        );
        assert!(!dialog.is_destructive());
        dialog.enter();
        assert_eq!(
            rig.sent().pop(),
            Some((
                ActionKind::SetIoPriority(IoPriority::BestEffort(4)),
                vec![812]
            ))
        );
    }

    #[gpui_kit::test]
    fn priority_app_row_targets_members(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let member = |pid, ppid| ProcessRecord {
            group: app_key(),
            ..record(pid, ppid, "firefox", ProcFlags::empty())
        };
        let records = vec![member(200, 1), member(201, 200), member(202, 200)];
        rig.publish(records, &[], vec![app_group(3)]);
        rig.select(&[ProcRowId::Group(app_key())]);
        assert!(
            rig.dispatch(SetPriority {
                class: PriorityClass::High
            })
            .is_none(),
            "own app: no dialog"
        );
        assert_eq!(
            rig.sent(),
            [(ActionKind::SetNice(-10), vec![200, 201, 202])],
            "every current member"
        );

        // Suspend skips app rows (Freeze App is E22-035).
        rig.select(&[ProcRowId::Group(app_key())]);
        assert!(rig.dispatch(SuspendProcess).is_none());
        assert_eq!(rig.sent().len(), 1);
    }

    #[gpui_kit::test]
    fn kthread_suspend_refused(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(
            vec![
                record(1, 0, "systemd", ProcFlags::PROTECTED),
                record(3, 2, "kworker/0:1", ProcFlags::KERNEL_THREAD),
                record(
                    4242,
                    1,
                    "tasklens",
                    ProcFlags::IS_SELF | ProcFlags::PROTECTED,
                ),
            ],
            &[1, 3, 4242],
            Vec::new(),
        );
        rig.select_pids(&[3]);
        assert!(rig.dispatch(SuspendProcess).is_none());
        assert_eq!(
            visible(&mut rig.vcx),
            ["Kernel threads cannot be signalled."]
        );
        assert!(rig.sent().is_empty(), "nothing is sent");

        // Resume refuses kernel threads too, but not tasklens or PID 1.
        rig.select_pids(&[3]);
        assert!(rig.dispatch(ResumeProcess).is_none());
        assert!(rig.sent().is_empty());
        rig.select_pids(&[4242]);
        assert!(rig.dispatch(SuspendProcess).is_none());
        assert!(visible(&mut rig.vcx).contains(&"Refusing to end tasklens itself.".into()));
        assert!(rig.sent().is_empty());
        rig.select_pids(&[1]);
        assert!(rig.dispatch(SuspendProcess).is_none());
        assert!(rig.sent().is_empty());
        rig.select_pids(&[1, 4242]);
        let dialog = rig.dispatch(ResumeProcess);
        assert!(
            dialog.is_none(),
            "own protected targets resume without a dialog"
        );
        assert_eq!(rig.sent(), [(ActionKind::Resume, vec![1, 4242])]);
    }

    const MIB: u64 = 1_048_576;

    /// The Copy details example of PROC-070: firefox, PID 4321.
    fn firefox(pid: u32) -> ProcessRecord {
        let mut r = ProcessRecord {
            cpu_pct: 3.27,
            rss_anon_bytes: 812 * MIB,
            rss_bytes: 1104 * MIB,
            threads: 97,
            ..record(pid, 2010, "firefox", ProcFlags::empty())
        };
        let meta = Arc::make_mut(&mut r.meta);
        meta.cmdline = Some(Arc::from("/usr/lib/firefox/firefox -P default"));
        r
    }

    fn firefox_extras() -> ProcessExtras {
        ProcessExtras {
            cpu_kernel_pct: 0.5,
            cpu_user_pct: 2.77,
            cpu_relative_pct: 26.16,
            utime_ticks: 4_200,
            stime_ticks: 1_300,
            start_boot_ns: 3_600_000_000_000,
            rss_peak_bytes: 1_300 * MIB,
            rss_shared_bytes: 292 * MIB,
            commit_bytes: 1490 * MIB,
            vsize_bytes: 3_000 * MIB,
            vsize_peak_bytes: 3_200 * MIB,
            page_faults: 120_000,
            page_faults_delta: 40,
            major_faults: 12,
            policy: SchedPolicy::Other,
            rt_prio: 0,
            affinity: CpuSet::default(),
            handles: Some(412),
            exe_name: Arc::from("firefox"),
            exe_path: Arc::from("/usr/lib/firefox/firefox"),
            exe_deleted: false,
            unit: None,
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
        }
    }

    /// Records, their extras and one process row each, with the cell context over them.
    struct CopyFixture {
        view: ProcessTableView,
        rows: Vec<TableRow>,
        records: Vec<ProcessRecord>,
        extras: Vec<ProcessExtras>,
        settings: Settings,
        words: CellWords,
        online: CpuSet,
        members: GroupMembers,
    }

    impl CopyFixture {
        fn new(records: Vec<ProcessRecord>, extras: Vec<ProcessExtras>) -> Self {
            let rows = records
                .iter()
                .enumerate()
                .map(|(i, r)| row(NodeId::Process(r.key), Some(i as u32), None))
                .collect();
            let mut f = Self {
                view: ProcessTableView::default(),
                rows,
                records,
                extras,
                settings: Settings::default(),
                words: CellWords::from_fn(|key| Arc::from(tr_en(key, &[]).as_ref())),
                online: CpuSet::default(),
                members: GroupMembers::default(),
            };
            f.members.refresh(&f.view, &f.records, &f.extras);
            f
        }

        fn ctx(&self) -> CellCtx<'_> {
            CellCtx {
                view: &self.view,
                records: &self.records,
                extras: &self.extras,
                extras_p2: &[],
                settings: &self.settings,
                words: &self.words,
                members: &self.members,
                ncpu: 8,
                online: &self.online,
                mem_total: 64 << 30,
                vram_total: 0,
                boot_time_s: 1_700_000_000,
                clk_tck: 100,
                windows: {
                    static NONE: std::sync::OnceLock<tl_model::snapshot::WindowsSnapshot> =
                        std::sync::OnceLock::new();
                    NONE.get_or_init(Default::default)
                },
            }
        }

        fn row_refs(&self) -> Vec<RowRef<'_>> {
            self.records
                .iter()
                .zip(&self.rows)
                .map(|(r, t)| RowRef::Record(r, t, RowKind::Process))
                .collect()
        }
    }

    /// Compares `actual` with `tests/golden/<name>`. `TL_UPDATE_GOLDEN=1` rewrites the file.
    fn golden(name: &str, actual: &str) {
        let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
        if std::env::var("TL_UPDATE_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(&path, actual).expect("write the golden");
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(actual, expected, "{name}");
    }

    #[test]
    fn copy_details_matches_golden() {
        let f = CopyFixture::new(vec![firefox(4321)], vec![firefox_extras()]);
        golden("copy_details.txt", &copy_details(&f.row_refs(), &f.ctx()));

        // Two rows: one blank line between the blocks.
        let f = CopyFixture::new(
            vec![firefox(4321), firefox(4400)],
            vec![firefox_extras(), firefox_extras()],
        );
        let text = copy_details(&f.row_refs(), &f.ctx());
        let blocks: Vec<&str> = text.split("\n\n").collect();
        assert_eq!(blocks.len(), 2);
        assert!(
            blocks[1].starts_with("Name: firefox\nPID: 4400\n"),
            "{text}"
        );
        assert!(!text.contains("\n\n\n"), "exactly one blank line");
        assert!(!text.ends_with('\n'));
    }

    #[test]
    fn copy_restricted_prints_word_not_zero() {
        let mut x = firefox_extras();
        x.handles = None;
        let mut r = firefox(4321);
        r.validity.set(
            tl_model::validity::ProcessField::Threads,
            Validity::Unsupported,
        );
        let f = CopyFixture::new(vec![r], vec![x]);
        let text = copy_details(&f.row_refs(), &f.ctx());
        assert!(text.contains("\nHandles: Restricted\n"), "{text}");
        assert!(text.contains("\nThreads: n/a\n"), "{text}");
        let handles = process_column(ColumnId::Handles);
        assert_eq!(copy_cell(&f.row_refs(), handles, &f.ctx()), "Restricted");

        // No extras yet: Private bytes and Handles are warming up, never 0.
        let f = CopyFixture::new(vec![firefox(4321)], Vec::new());
        let text = copy_details(&f.row_refs(), &f.ctx());
        assert!(text.contains("\nPrivate bytes: …\n"), "{text}");
        assert!(text.contains("\nHandles: …\n"), "{text}");
    }

    #[test]
    fn copy_all_metrics_matches_golden_and_ends_with_cmdline() {
        let f = CopyFixture::new(vec![firefox(4321)], vec![firefox_extras()]);
        let text = copy_all_metrics(&f.row_refs(), &f.ctx());
        assert_eq!(
            text.lines().last(),
            Some("Command line: /usr/lib/firefox/firefox -P default")
        );
        // Start time is in the local time zone, so the golden leaves it out.
        assert_eq!(
            text.lines()
                .filter(|l| l.starts_with("Start time: "))
                .count(),
            1
        );
        let stable: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with("Start time: "))
            .collect();
        golden("copy_all_metrics.txt", &stable.join("\n"));
        assert!(
            !text.to_lowercase().contains("environ"),
            "no environment data"
        );

        // Two rows: one blank line between them, each ending with its command line.
        let f = CopyFixture::new(
            vec![firefox(4321), firefox(4400)],
            vec![firefox_extras(), firefox_extras()],
        );
        let text = copy_all_metrics(&f.row_refs(), &f.ctx());
        let blocks: Vec<&str> = text.split("\n\n").collect();
        assert_eq!(blocks.len(), 2);
        for b in blocks {
            assert!(b.lines().last().unwrap().starts_with("Command line: "));
        }
    }

    #[gpui_kit::test]
    fn copy_rows_as_table_matches_golden(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let mut restricted = firefox(4400);
        restricted.validity.set(
            tl_model::validity::ProcessField::RssAnon,
            Validity::Restricted,
        );
        rig.publish(vec![firefox(4321), restricted], &[4321, 4400], Vec::new());
        // The table's own TSV (ADR-015 copy words); the page adds the toast.
        assert_eq!(
            last_toast(&mut rig, &[4321, 4400], CopyRowsAsTable),
            "Copied to clipboard"
        );
        let text = clipboard(&mut rig);
        assert!(text.starts_with("Name\tUser\tPID\tCPU\t"), "{text}");
        assert_eq!(text.lines().count(), 3, "{text}");
        golden("copy_rows.tsv", &text);
    }

    /// Selects `pids`, dispatches `action` and returns the newest visible toast.
    fn last_toast(rig: &mut Rig, pids: &[u32], action: impl Action) -> String {
        rig.select_pids(pids);
        assert!(rig.dispatch(action).is_none());
        visible(&mut rig.vcx).last().cloned().unwrap_or_default()
    }

    fn clipboard(rig: &mut Rig) -> String {
        rig.vcx
            .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
            .unwrap_or_default()
    }

    #[gpui_kit::test]
    fn copy_cell_multi_rows_one_per_line(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let records = (300..303)
            .map(|pid| ProcessRecord {
                cpu_pct: 1.5,
                ..record(pid, 1, "proc", ProcFlags::empty())
            })
            .collect();
        rig.publish(records, &[300, 301, 302], Vec::new());
        rig.select_pids(&[300, 301, 302]);
        assert!(rig.dispatch(CopyCell { col: ColumnId::Cpu }).is_none());
        let text = clipboard(&mut rig);
        assert_eq!(text.lines().count(), 3, "{text}");
        assert_eq!(visible(&mut rig.vcx), ["Selected process values copied."]);
        assert!(rig.sent().is_empty(), "copying sends nothing");
    }

    #[gpui_kit::test]
    fn copy_toasts_single_and_multi(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300, 301]), &[300, 301], Vec::new());
        let cell = CopyCell { col: ColumnId::Pid };
        assert_eq!(last_toast(&mut rig, &[300], cell), "Copied to clipboard");
        assert_eq!(clipboard(&mut rig), "300");
        assert_eq!(
            last_toast(&mut rig, &[300], CopyDetails),
            "Process details copied."
        );
        assert_eq!(
            last_toast(&mut rig, &[300], CopyAllMetrics),
            "Copied metrics to clipboard"
        );
        assert_eq!(
            last_toast(&mut rig, &[300, 301], CopyAllMetrics),
            "Selected process metrics copied."
        );
    }

    #[gpui_kit::test]
    fn copy_details_exited_row(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let mut records = own(&[300]);
        records.push(record(4300, 1, "gone", ProcFlags::EXITED));
        rig.publish(records, &[300, 4300], Vec::new());
        assert_eq!(
            last_toast(&mut rig, &[4300], CopyDetails),
            "Process details copied."
        );
        assert!(clipboard(&mut rig).starts_with("Name: gone\nPID: 4300\n"));
    }

    fn kwin_window(id: &str, pid: u32, stack_index: u32, caption: &str) -> WindowEntry {
        WindowEntry {
            internal_id: Arc::from(id),
            pid,
            caption: Arc::from(caption),
            resource_class: Arc::from("mail"),
            desktop_file_name: Arc::from("mail"),
            minimized: false,
            maximized: false,
            keep_above: false,
            frame_geometry: Default::default(),
            window_type: 0,
            stack_index,
        }
    }

    fn publish_windows(rig: &mut Rig, windows: Vec<WindowEntry>) {
        rig.engine.publish_with(|s| {
            s.windows = Arc::new(WindowsSnapshot {
                header: ComponentHeader {
                    version: 1,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                windows: windows.into(),
                pick: None,
            });
        });
        rig.vcx.run_until_parked();
    }

    /// Builds the window popup over `windows` in the table's context, as the window button does.
    fn window_menu(rig: &mut Rig, windows: Vec<WindowEntry>) -> Entity<TlMenuPopup> {
        let table = rig.vcx.update(|_, cx| rig.page.read(cx).table().clone());
        rig.vcx.update(|window, cx| window.draw(cx).clear(cx));
        rig.vcx.update(|window, cx| {
            let focus = table.read(cx).focus_handle().clone();
            TlMenu::build(window, cx, focus, move |menu, _, cx| {
                window_popup(menu, &windows, cx)
            })
        })
    }

    fn kwin_sent(rig: &Rig) -> Vec<KWinCommand> {
        rig.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::KWin(k) => Some(k),
                _ => None,
            })
            .collect()
    }

    /// PROC-053, UI-PRC-003: one item per window in the given order, labelled with the caption,
    /// an empty caption as "Untitled window", no icon, each dispatching `ActivateWindow`.
    #[gpui_kit::test]
    fn window_popup_items_and_untitled(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        let windows = vec![
            kwin_window("{a}", 300, 0, "Inbox - Mail"),
            kwin_window("{b}", 300, 1, ""),
        ];
        let menu = window_menu(&mut rig, windows);
        let entries = entries(&mut rig, &menu);
        let shown: Vec<(MenuEntryKind, Option<String>, bool)> = entries
            .iter()
            .map(|e| {
                (
                    e.kind,
                    e.label.as_ref().map(|l| l.to_string()),
                    e.icon.is_some(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (MenuEntryKind::Item, Some("Inbox - Mail".to_owned()), false),
                (
                    MenuEntryKind::Item,
                    Some("Untitled window".to_owned()),
                    false
                ),
            ]
        );
        let ids: Vec<Option<String>> = entries
            .iter()
            .map(|e| {
                e.action
                    .as_ref()
                    .and_then(|a| a.as_any().downcast_ref::<ActivateWindow>())
                    .map(|a| a.internal_id.to_string())
            })
            .collect();
        assert_eq!(ids, [Some("{a}".to_owned()), Some("{b}".to_owned())]);
    }

    /// PROC-053, PROC-054: Enter on a popup item sends one `KWin(Activate)` for its window and
    /// shows "Process window shown." A window gone from the snapshot sends nothing and shows
    /// "No window found". Esc closes the popup and gives the focus back to the table.
    #[gpui_kit::test]
    fn activate_window_sends_kwin_command(cx: &mut TestAppContext) {
        let mut rig = Rig::new(cx);
        rig.publish(own(&[300]), &[300], Vec::new());
        let windows = vec![kwin_window("{a}", 300, 0, "Inbox")];
        publish_windows(&mut rig, windows.clone());

        let menu = window_menu(&mut rig, windows.clone());
        rig.vcx.update(|window, cx| {
            open_context_menu_at(point(px(20.), px(80.)), window, cx, menu.clone());
            menu.update(cx, |m, cx| m.highlight_first(window, cx));
        });
        rig.vcx.run_until_parked();
        rig.vcx.update(|window, cx| window.draw(cx).clear(cx));
        rig.vcx.simulate_keystrokes("enter");
        rig.vcx.run_until_parked();
        assert_eq!(
            kwin_sent(&rig),
            [KWinCommand::Activate {
                internal_id: Arc::from("{a}"),
            }]
        );
        assert_eq!(
            visible(&mut rig.vcx).last().map(String::as_str),
            Some("Process window shown.")
        );

        // The window closed before the click.
        publish_windows(&mut rig, Vec::new());
        assert!(
            rig.dispatch(ActivateWindow {
                internal_id: "{a}".into(),
            })
            .is_none()
        );
        assert_eq!(
            kwin_sent(&rig).len(),
            1,
            "nothing sent for a vanished window"
        );
        assert_eq!(
            visible(&mut rig.vcx).last().map(String::as_str),
            Some("No window found")
        );

        // Esc closes the popup and returns focus to the table.
        let table = rig.vcx.update(|_, cx| rig.page.read(cx).table().clone());
        let menu = window_menu(&mut rig, windows);
        rig.vcx.update(|window, cx| {
            open_context_menu_at(point(px(20.), px(80.)), window, cx, menu.clone());
        });
        rig.vcx.run_until_parked();
        rig.vcx.update(|window, cx| window.draw(cx).clear(cx));
        rig.vcx.simulate_keystrokes("escape");
        rig.vcx.run_until_parked();
        let (open, focused) = rig.vcx.update(|window, cx| {
            (
                open_context_menu(window, cx).is_some(),
                table.read(cx).focus_handle().is_focused(window),
            )
        });
        assert!(!open, "Esc closes the popup");
        assert!(focused, "focus is back on the table");
        assert_eq!(kwin_sent(&rig).len(), 1);
    }
}
