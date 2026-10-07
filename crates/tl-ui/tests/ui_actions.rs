//! `ActionTracker`: action events to toasts, deadlines, retry and Details (E16-A3, ADR-005,
//! ADR-006, R-114, R-196, R-197, UI-PRC-015). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, TestAppContext, VisualTestContext, WeakEntity, Window, WindowHandle, div, px,
};
use tl_model::actions::{
    ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    DumpMode, RejectReason, Signal,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError};
use tl_model::health::HelperState;
use tl_model::ids::ProcessKey;
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::table::{Manager, UnitRef};
use tl_model::time::BootId;
use tl_ui::action_tracker::{
    ActionTracker, ActionTrackerEvent, PushedToast, RetryActionAsAdmin, ShowActionDetails,
    SummaryTexts, ToastTexts,
};
use tl_ui::components::dialog::TlDialogView;
use tl_ui::components::toast::{self, ToastKey, ToastKind, ToastQueue};
use tl_ui::components::window_frame::TlSecondaryWindow;
use tl_ui::hub::{GlobalHub, SnapshotHub};
use tl_ui::testing::fake_engine::FakeEngine;

struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("content")
    }
}

struct Rig {
    engine: Arc<FakeEngine>,
    vcx: VisualTestContext,
    queue: Entity<ToastQueue>,
    tracker: Entity<ActionTracker>,
}

impl Rig {
    fn track(&mut self, request: ActionRequest, texts: ToastTexts) -> ActionId {
        self.try_track(request, texts).expect("sent")
    }

    fn try_track(
        &mut self,
        request: ActionRequest,
        texts: ToastTexts,
    ) -> Result<ActionId, CommandError> {
        let queue = self.queue.downgrade();
        let tracker = self.tracker.clone();
        let id = self
            .vcx
            .update(|_, cx| tracker.update(cx, |t, cx| t.track(request, queue, texts, cx)));
        self.vcx.run_until_parked();
        id
    }

    fn emit(&mut self, id: ActionId, target: Option<u16>, state: ActionState) {
        self.engine.emit(ActionEvent { id, target, state });
        self.vcx.run_until_parked();
    }

    /// `Started`, one state per target, then the summary of those states.
    fn finish(&mut self, id: ActionId, states: &[ActionState]) {
        self.emit(id, None, ActionState::Started);
        for (i, state) in states.iter().enumerate() {
            self.emit(id, Some(i as u16), state.clone());
        }
        self.emit(id, None, ActionState::summarize(states));
    }

    fn visible(&mut self) -> Vec<String> {
        self.queue.read_with(&self.vcx, |q, _| {
            q.visible_texts().iter().map(|t| t.to_string()).collect()
        })
    }

    fn pushed(&mut self) -> Vec<PushedToast> {
        self.tracker
            .read_with(&self.vcx, |t, _| t.pushed().to_vec())
    }

    fn last(&mut self) -> PushedToast {
        self.pushed().pop().expect("a toast was pushed")
    }

    fn pending(&mut self) -> usize {
        self.tracker.read_with(&self.vcx, |t, _| t.pending_len())
    }

    fn advance(&mut self, d: Duration) {
        self.vcx.executor().advance_clock(d);
        self.vcx.run_until_parked();
    }

    fn dispatch(&mut self, action: impl gpui_kit::Action) {
        self.vcx.dispatch_action(action);
        self.vcx.run_until_parked();
    }

    fn actions_sent(&self) -> Vec<ActionRequest> {
        self.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(request) => Some(request),
                _ => None,
            })
            .collect()
    }
}

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        toast::init(cx);
        cx.set_reduce_motion(true);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::i18n::init_strings(cx, "en-US");
    });
}

/// A window with a toast queue and a tracker on a new `FakeEngine`. With `helper`, a hub whose
/// snapshot has that helper state.
fn rig(cx: &mut TestAppContext, helper: Option<HelperState>) -> Rig {
    init(cx);
    let engine = FakeEngine::new();
    if let Some(helper) = helper {
        engine.publish_with(|s| Arc::make_mut(&mut s.health).helper = helper);
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        cx.update(|cx| cx.set_global(GlobalHub(hub)));
    }
    let window = cx.add_window(|window, cx| {
        let host = cx.new(|_| Host);
        Root::new(host, window, cx)
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.update(|window, _| window.activate_window());
    let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(64.))));
    let client: Arc<dyn EngineClient> = engine.clone();
    let tracker = vcx.update(|_, cx| cx.new(|cx| ActionTracker::new(client, cx)));
    vcx.run_until_parked();
    Rig {
        engine,
        vcx,
        queue,
        tracker,
    }
}

fn process(i: u32) -> ActionTarget {
    ActionTarget::Process(ProcessKey {
        start_ticks: 1_000 + u64::from(i),
        boot_id: BootId(7),
        pid: 100 + i,
    })
}

fn unit(name: &str) -> ActionTarget {
    ActionTarget::Unit(UnitRef {
        manager: Manager::System,
        name: name.into(),
    })
}

fn request(kind: ActionKind, targets: impl IntoIterator<Item = ActionTarget>) -> ActionRequest {
    ActionRequest {
        id: ActionId(0),
        kind,
        targets: targets.into_iter().collect(),
        privileged: false,
    }
}

fn end_task(n: u32) -> ActionRequest {
    request(ActionKind::Signal(Signal::Term), (0..n).map(process))
}

fn end_task_texts() -> ToastTexts {
    ToastTexts {
        started: Some((ToastKind::Info, "End task requested.".into())),
        ..ToastTexts::new("Process terminated", "Could not end task.")
    }
}

fn service_summary() -> SummaryTexts {
    SummaryTexts {
        started: "Starting 6 services...".into(),
        all_ok: "6 services started".into(),
        partial: Box::new(|ok, total, failed| {
            format!("{ok} of {total} services started. {failed} failed.").into()
        }),
    }
}

fn job_failed() -> ActionState {
    ActionState::Failed(ActionError::UnitJob {
        result: "failed".into(),
        error_name: None,
    })
}

const EPERM: ActionState = ActionState::Failed(ActionError::PermissionDenied);

fn toast(kind: ToastKind, text: &str, key: ToastKey, button: Option<&str>) -> PushedToast {
    PushedToast {
        kind,
        text: SharedString::from(text.to_owned()),
        key: Some(key),
        button: button.map(|b| SharedString::from(b.to_owned())),
    }
}

#[gpui_kit::test]
fn next_id_starts_at_one(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let ids = r
        .tracker
        .read_with(&r.vcx, |t, _| [t.next_id(), t.next_id()]);
    assert_eq!(ids, [ActionId(1), ActionId(2)]);
    let other = rig(cx, None);
    let first = other.tracker.read_with(&other.vcx, |t, _| t.next_id());
    assert_eq!(first, ActionId(1), "each tracker counts on its own");
    let id = r.track(end_task(1), end_task_texts());
    assert_eq!(id, ActionId(3), "track takes the next id for id 0");
}

#[gpui_kit::test]
fn action_toast_replaces_in_place_per_request(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let id = r.track(end_task(1), end_task_texts());
    assert_eq!(r.actions_sent().len(), 1);
    assert_eq!(r.actions_sent()[0].id, id);

    r.emit(id, None, ActionState::Started);
    assert_eq!(r.visible(), ["End task requested."]);
    assert_eq!(
        r.last(),
        toast(ToastKind::Info, "End task requested.", ToastKey(id.0), None)
    );

    r.emit(id, Some(0), ActionState::Succeeded);
    r.emit(id, None, ActionState::Succeeded);
    assert_eq!(r.visible(), ["Process terminated"], "replaced in place");
    assert_eq!(
        r.last(),
        toast(
            ToastKind::Success,
            "Process terminated",
            ToastKey(id.0),
            None
        )
    );
    assert_eq!(r.pending(), 0);

    // A whole-request refusal shows the reject text; AlreadyRunning is Info.
    let id = r.track(
        request(ActionKind::StartupRunNow, [process(9)]),
        ToastTexts::new("Started", "Could not start."),
    );
    r.emit(
        id,
        None,
        ActionState::Rejected(RejectReason::AlreadyRunning),
    );
    assert_eq!(
        r.last(),
        toast(ToastKind::Info, "Already running", ToastKey(id.0), None)
    );
    let id = r.track(end_task(1), end_task_texts());
    r.emit(id, None, ActionState::Rejected(RejectReason::KernelThread));
    assert_eq!(
        r.last(),
        toast(
            ToastKind::Warning,
            "Kernel threads cannot be signalled.",
            ToastKey(id.0),
            None
        )
    );
    assert_eq!(r.pending(), 0);
}

#[gpui_kit::test]
fn outcome_unknown_after_deadline_plus_5s(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let id = r.track(end_task(1), end_task_texts());
    r.emit(id, None, ActionState::Started);
    r.advance(Duration::from_secs(6));
    assert_eq!(r.pending(), 1, "2 s + 5 s not passed");
    assert!(r.pushed().iter().all(|t| t.text != "Outcome unknown"));

    r.advance(Duration::from_secs(2));
    assert_eq!(r.pending(), 0, "removed");
    assert_eq!(
        r.last(),
        toast(ToastKind::Warning, "Outcome unknown", ToastKey(id.0), None)
    );
    assert_eq!(r.visible(), ["Outcome unknown"]);

    // A late summary for the removed entry changes nothing.
    let before = r.pushed().len();
    r.emit(id, None, ActionState::Succeeded);
    assert_eq!(r.pushed().len(), before);
}

#[gpui_kit::test]
fn service_jobs_have_no_timeout(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    r.track(
        request(ActionKind::UnitStart, [unit("a.service")]),
        ToastTexts::new("Service started", "Could not start the service."),
    );
    r.track(
        request(
            ActionKind::Dump {
                mode: DumpMode::Mini,
                path: "/var/tmp/core".into(),
            },
            [process(1)],
        ),
        ToastTexts::new("Dump created", "Could not create the dump."),
    );
    r.advance(Duration::from_secs(60));
    assert_eq!(r.pending(), 2);
    r.advance(Duration::from_secs(540));
    assert_eq!(r.pending(), 2, "Dump has no deadline either");
    assert!(r.pushed().iter().all(|t| t.text != "Outcome unknown"));
}

#[gpui_kit::test]
fn summary_toast_for_four_or_more_targets(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let names = ["a", "b", "c", "d", "e", "f"].map(|n| format!("{n}.service"));
    let texts = || ToastTexts {
        started: Some((ToastKind::Info, "Start requested.".into())),
        summary: Some(service_summary()),
        ..ToastTexts::new("Service started", "Could not start the service.")
    };
    let id = r.track(
        request(ActionKind::UnitStart, names.iter().map(|n| unit(n))),
        texts(),
    );
    r.emit(id, None, ActionState::Started);
    assert_eq!(
        r.last(),
        toast(
            ToastKind::Progress,
            "Starting 6 services...",
            ToastKey(id.0),
            None
        )
    );
    for (i, ok) in [true, false, true, true, false, true]
        .into_iter()
        .enumerate()
    {
        let state = if ok {
            ActionState::Succeeded
        } else {
            job_failed()
        };
        r.emit(id, Some(i as u16), state);
    }
    assert_eq!(r.pushed().len(), 1, "no per-target toasts");
    r.emit(id, None, job_failed());
    assert_eq!(
        r.last(),
        toast(
            ToastKind::Warning,
            "4 of 6 services started. 2 failed.",
            ToastKey(id.0),
            Some("Details")
        )
    );
    assert_eq!(r.visible(), ["4 of 6 services started. 2 failed."]);

    let id = r.track(
        request(ActionKind::UnitStart, names.iter().map(|n| unit(n))),
        texts(),
    );
    r.finish(id, &vec![ActionState::Succeeded; 6]);
    assert_eq!(
        r.last(),
        toast(
            ToastKind::Success,
            "6 services started",
            ToastKey(id.0),
            None
        )
    );

    // Below the threshold: one toast per target, and the request toast goes.
    let id = r.track(end_task(3), end_task_texts());
    r.finish(
        id,
        &[ActionState::Succeeded, job_failed(), ActionState::Succeeded],
    );
    let keys: Vec<_> = r.pushed().iter().rev().take(3).map(|t| t.key).collect();
    assert_eq!(
        keys,
        [2, 1, 0].map(|i| Some(ToastKey(id.0 << 8 | i))),
        "one toast per target"
    );
    assert!(!r.visible().iter().any(|t| t == "End task requested."));
}

#[gpui_kit::test]
fn no_started_text_no_progress_toast(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let texts = ToastTexts {
        summary: Some(service_summary()),
        summary_min_targets: 2,
        ..ToastTexts::new("Ended", "Could not end.")
    };
    let id = r.track(end_task(3), texts);
    r.emit(id, None, ActionState::Started);
    assert!(r.pushed().is_empty());
    assert!(r.visible().is_empty());
}

#[gpui_kit::test]
fn shared_toast_key_replaces_in_place(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let texts = |success: &str| ToastTexts {
        toast_key: Some(ToastKey(77)),
        ..ToastTexts::new(success.to_owned(), "Could not update the priority.")
    };
    let first = r.track(
        request(ActionKind::SetNice(5), [process(1)]),
        texts("Scheduling updated"),
    );
    let second = r.track(
        request(ActionKind::SetNice(5), [process(1)]),
        texts("Priority updated"),
    );
    r.finish(first, &[ActionState::Succeeded]);
    r.finish(second, &[ActionState::Succeeded]);
    assert_eq!(r.visible(), ["Priority updated"]);
    assert!(r.pushed().iter().all(|t| t.key == Some(ToastKey(77))));
}

#[gpui_kit::test]
fn tracker_events_by_id(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let seen: Rc<RefCell<Vec<ActionTrackerEvent>>> = Rc::default();
    let sink = seen.clone();
    let tracker = r.tracker.clone();
    r.vcx.update(|_, cx| {
        cx.subscribe(&tracker, move |_, ev: &ActionTrackerEvent, _| {
            sink.borrow_mut().push(*ev)
        })
        .detach();
    });
    let id = r.track(end_task(3), end_task_texts());
    r.finish(id, &[ActionState::Succeeded, EPERM, ActionState::Succeeded]);
    assert_eq!(
        *seen.borrow(),
        [
            ActionTrackerEvent::Started(id),
            ActionTrackerEvent::Finished {
                id,
                ok: 2,
                total: 3,
                failed: 1,
            },
        ]
    );
}

#[gpui_kit::test]
fn details_dialog_lists_failed_targets(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let names = ["a", "b", "c", "d", "e", "f"].map(|n| format!("{n}.service"));
    let texts = ToastTexts {
        summary: Some(service_summary()),
        ..ToastTexts::new("Service started", "Could not start the service.")
    };
    let id = r.track(
        request(ActionKind::UnitStart, names.iter().map(|n| unit(n))),
        texts,
    );
    let ok = ActionState::Succeeded;
    r.finish(
        id,
        &[
            ok.clone(),
            job_failed(),
            ok.clone(),
            ok.clone(),
            ActionState::Failed(ActionError::NotAuthorized),
            ok,
        ],
    );

    let before: Vec<_> = r
        .vcx
        .update(|_, cx| cx.windows().iter().map(|w| w.window_id()).collect());
    r.dispatch(ShowActionDetails(id.0));
    let dialog = r
        .vcx
        .update(|_, cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(&w.window_id()))
        .expect("the Details dialog opened");
    let handle = WindowHandle::<Root>::new(dialog.window_id());
    let view = r
        .vcx
        .update(|_, cx| {
            TlSecondaryWindow::<TlDialogView>::find(handle, cx)
                .map(|frame| frame.read(cx).content().clone())
        })
        .expect("a TlDialog");
    let (title, destructive, cancel, text) = view.read_with(&r.vcx, |v, _| {
        let d = v.dialog();
        (
            d.window_title().to_string(),
            d.is_destructive(),
            d.has_cancel(),
            d.text().to_string(),
        )
    });
    assert_eq!(title, "tasklens");
    assert!(!destructive);
    assert!(!cancel, "the one button is Close");
    assert_eq!(text, "4 of 6 services started. 2 failed.");
    let opened = r
        .tracker
        .read_with(&r.vcx, |t, _| t.opened_details().to_vec());
    assert_eq!(opened.len(), 1);
    assert_eq!(
        opened[0].lines,
        [
            "b.service: The service job ended with result: failed.",
            "e.service: Administrator access was refused.",
        ]
    );
}

#[gpui_kit::test]
fn retry_button_only_when_enabled_and_eperm(cx: &mut TestAppContext) {
    // Before P3: the full message and no button.
    let mut r = rig(cx, None);
    let id = r.track(end_task(1), end_task_texts());
    r.finish(id, &[EPERM]);
    assert_eq!(
        r.last(),
        toast(
            ToastKind::Error,
            "Could not end task. Permission denied. This process belongs to another user.",
            ToastKey(id.0),
            None
        )
    );
    // Enabled, but tl-helper is missing (no hub reads as Missing): still no button, and no
    // status chip to tell without a shell.
    let texts = || ToastTexts {
        retry_as_admin: true,
        ..end_task_texts()
    };
    let id = r.track(end_task(1), texts());
    r.finish(id, &[EPERM]);
    assert_eq!(r.last().button, None);
    assert_eq!(r.tracker.read_with(&r.vcx, |t, _| t.restricted_notes()), 0);

    let mut r = rig(cx, Some(HelperState::Locked));
    let id = r.track(end_task(1), texts());
    r.finish(id, &[EPERM]);
    assert_eq!(
        r.last(),
        toast(
            ToastKind::Error,
            "Permission denied",
            ToastKey(id.0),
            Some("Retry as administrator")
        )
    );
    let id = r.track(end_task(1), texts());
    r.finish(id, &[ActionState::Failed(ActionError::NoSuchProcess)]);
    assert_eq!(
        r.last(),
        toast(
            ToastKind::Error,
            "Could not end task. The process has already exited.",
            ToastKey(id.0),
            None
        )
    );
}

#[gpui_kit::test]
fn retry_sends_only_failed_targets_privileged(cx: &mut TestAppContext) {
    let mut r = rig(cx, Some(HelperState::Locked));
    let texts = ToastTexts {
        retry_as_admin: true,
        ..end_task_texts()
    };
    let id = r.track(end_task(3), texts);
    r.finish(id, &[ActionState::Succeeded, EPERM, EPERM]);
    let buttons: Vec<_> = r.pushed().iter().filter_map(|t| t.button.clone()).collect();
    assert_eq!(buttons, ["Retry as administrator"; 2]);

    r.dispatch(RetryActionAsAdmin(id.0));
    let sent = r.actions_sent();
    assert_eq!(sent.len(), 2);
    let retry = &sent[1];
    assert!(retry.privileged);
    assert_ne!(retry.id, id);
    assert_eq!(retry.kind, ActionKind::Signal(Signal::Term));
    assert_eq!(retry.targets.as_slice(), [process(1), process(2)]);
    assert_eq!(r.pending(), 1);

    // The second toast's button finds the retry already sent.
    r.dispatch(RetryActionAsAdmin(id.0));
    assert_eq!(r.actions_sent().len(), 2);
}

#[gpui_kit::test]
fn busy_send_shows_warning(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    r.engine.set_busy(true);
    assert_eq!(
        r.try_track(end_task(1), end_task_texts()),
        Err(CommandError::Busy)
    );
    assert_eq!(r.pending(), 0, "nothing recorded");

    let queue: WeakEntity<ToastQueue> = r.queue.downgrade();
    let tracker = r.tracker.clone();
    let id = r.vcx.update(|_, cx| {
        tracker.update(cx, |t, cx| {
            t.track_or_toast(end_task(1), queue, end_task_texts(), cx)
        })
    });
    r.vcx.run_until_parked();
    assert_eq!(id, None);
    assert_eq!(
        r.visible(),
        ["Too many actions are running. Try again shortly."]
    );
    assert_eq!(r.last().kind, ToastKind::Warning);
    assert!(r.engine.commands().is_empty());
}

/// With the main window open and tl-helper missing, an EPERM reaches the status chip once.
mod with_shell {
    use gpui_kit::{EventEmitter, FocusHandle, InteractiveElement as _};
    use tl_model::command::Tab;
    use tl_model::settings::{Settings, UiState};
    use tl_ui::app::BootSettings;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::window::main::{
        self as shell, MainWindow, PageSlot, ShellPage, open_main_window, set_test_page,
    };
    use tl_ui::window::status::StatusChanged;

    use super::*;

    struct FakePage {
        focus: FocusHandle,
    }

    impl EventEmitter<StatusChanged> for FakePage {}

    impl Render for FakePage {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().id("fake-page").track_focus(&self.focus).size_full()
        }
    }

    impl ShellPage for FakePage {
        fn primary_focus(&self) -> FocusHandle {
            self.focus.clone()
        }
    }

    fn fake(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
        let page = cx.new(|cx| FakePage {
            focus: cx.focus_handle().tab_stop(true),
        });
        PageSlot::new(page, cx)
    }

    #[gpui_kit::test]
    fn eperm_without_helper_notes_restricted_refusal(cx: &mut TestAppContext) {
        init(cx);
        cx.update(|cx| {
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            tl_ui::actions::bind_keys_global(cx);
            set_test_page(cx, Tab::Processes, fake);
            set_test_page(cx, Tab::Services, fake);
            shell::init(cx);
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();

        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let tracker = vcx.update(|_, cx| cx.new(|cx| ActionTracker::new(client, cx)));
        let texts = ToastTexts {
            retry_as_admin: true,
            ..end_task_texts()
        };
        let id = vcx
            .update(|_, cx| {
                tracker.update(cx, |t, cx| {
                    t.track(end_task(2), WeakEntity::new_invalid(), texts, cx)
                })
            })
            .expect("sent");
        engine.emit(ActionEvent {
            id,
            target: None,
            state: ActionState::Started,
        });
        for target in 0..2 {
            engine.emit(ActionEvent {
                id,
                target: Some(target),
                state: EPERM,
            });
        }
        engine.emit(ActionEvent {
            id,
            target: None,
            state: EPERM,
        });
        vcx.run_until_parked();
        assert_eq!(tracker.read_with(&vcx, |t, _| t.restricted_notes()), 1);
        assert!(
            tracker
                .read_with(&vcx, |t, _| t.pushed().to_vec())
                .iter()
                .all(|t| t.button.is_none()),
            "no retry while tl-helper is missing"
        );
    }
}
