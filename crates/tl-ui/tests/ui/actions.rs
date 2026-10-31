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
    let mut tracker_hub = None;
    if let Some(helper) = helper {
        engine.publish_with(|s| Arc::make_mut(&mut s.health).helper = helper);
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        tracker_hub = Some(hub.clone());
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
    let tracker = vcx.update(|_, cx| cx.new(|cx| ActionTracker::new(client, tracker_hub, cx)));
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
    SummaryTexts::new(
        "Starting 6 services...",
        "6 services started",
        |ok, total, failed| {
            let n = failed.len();
            (
                ToastKind::Warning,
                format!("{ok} of {total} services started. {n} failed.").into(),
            )
        },
    )
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
                name: "x_1.core".into(),
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
fn summary_partial_gets_failures_and_chooses_kinds(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let seen: Rc<RefCell<Vec<(u16, ActionState)>>> = Rc::default();
    let sink = seen.clone();
    let mut summary = SummaryTexts::new("Ending 3...", "3 ended", move |ok, total, failures| {
        sink.borrow_mut()
            .extend(failures.iter().map(|(i, s)| (*i, (*s).clone())));
        let kind = if ok == 0 {
            ToastKind::Error
        } else {
            ToastKind::Warning
        };
        (kind, format!("{ok} of {total}").into())
    });
    summary.started_kind = ToastKind::Info;
    let texts = ToastTexts {
        started: Some((ToastKind::Info, "End requested.".into())),
        summary: Some(summary),
        summary_min_targets: 2,
        ..ToastTexts::new("Ended", "Could not end.")
    };
    let id = r.track(end_task(3), texts);
    let gone = ActionState::Failed(ActionError::NoSuchProcess);
    r.finish(id, &[EPERM, ActionState::Succeeded, gone.clone()]);
    assert_eq!(
        r.pushed()[0],
        toast(ToastKind::Info, "Ending 3...", ToastKey(id.0), None),
        "the owner's started kind"
    );
    assert_eq!(
        *seen.borrow(),
        [(0, EPERM), (2, gone)],
        "failures in target order"
    );
    assert_eq!(r.last().kind, ToastKind::Warning);
    assert_eq!(r.last().text, "1 of 3");
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

/// Records every tracker event.
fn events(r: &mut Rig) -> Rc<RefCell<Vec<ActionTrackerEvent>>> {
    let seen: Rc<RefCell<Vec<ActionTrackerEvent>>> = Rc::default();
    let sink = seen.clone();
    let tracker = r.tracker.clone();
    r.vcx.update(|_, cx| {
        cx.subscribe(&tracker, move |_, ev: &ActionTrackerEvent, _| {
            sink.borrow_mut().push(*ev)
        })
        .detach();
    });
    seen
}

fn owner_texts() -> ToastTexts {
    ToastTexts {
        started: Some((ToastKind::Progress, "Starting service...".into())),
        summary: Some(service_summary()),
        owner_toasts: true,
        ..ToastTexts::new("Service started", "Could not start the service.")
    }
}

#[gpui_kit::test]
fn owner_toasts_push_nothing_and_report_each_target(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let seen = events(&mut r);
    let targets = ["a", "b", "c", "d"].map(|n| unit(&format!("{n}.service")));
    let id = r.track(request(ActionKind::UnitStart, targets), owner_texts());
    r.emit(id, None, ActionState::Started);
    r.emit(id, Some(0), ActionState::Succeeded);
    let so_far = r
        .tracker
        .read_with(&r.vcx, |t, _| t.results(id).map(<[_]>::to_vec));
    assert_eq!(so_far, Some(vec![(0, ActionState::Succeeded)]));
    r.emit(id, Some(1), EPERM);
    // Targets 2 and 3 are reported only by the summary.
    r.emit(id, None, ActionState::Succeeded);

    assert!(r.pushed().is_empty(), "the owner shows the toasts");
    assert!(r.visible().is_empty());
    assert_eq!(
        *seen.borrow(),
        [
            ActionTrackerEvent::Started(id),
            ActionTrackerEvent::TargetFinished { id, index: 0 },
            ActionTrackerEvent::TargetFinished { id, index: 1 },
            ActionTrackerEvent::Finished {
                id,
                ok: 3,
                total: 4,
                failed: 1,
            },
        ]
    );
    let finals = r
        .tracker
        .read_with(&r.vcx, |t, _| t.results(id).map(<[_]>::to_vec));
    assert_eq!(
        finals,
        Some(vec![
            (0, ActionState::Succeeded),
            (1, EPERM),
            (2, ActionState::Succeeded),
            (3, ActionState::Succeeded),
        ]),
        "readable after Finished, summary-only targets with the summary state"
    );
    assert_eq!(r.pending(), 0);
}

#[gpui_kit::test]
fn owner_toasts_timeout_keeps_known_results(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let seen = events(&mut r);
    let texts = ToastTexts {
        owner_toasts: true,
        ..end_task_texts()
    };
    let id = r.track(end_task(2), texts);
    r.emit(id, Some(0), ActionState::Succeeded);
    r.advance(Duration::from_secs(60));

    assert!(r.pushed().is_empty(), "no Outcome unknown toast either");
    assert!(matches!(
        seen.borrow().last(),
        Some(ActionTrackerEvent::Finished { id: f, ok: 1, total: 2, failed: 1 }) if *f == id
    ));
    let finals = r
        .tracker
        .read_with(&r.vcx, |t, _| t.results(id).map(<[_]>::to_vec));
    assert_eq!(finals, Some(vec![(0, ActionState::Succeeded)]));
}

#[gpui_kit::test]
fn owner_results_keep_the_last_16(cx: &mut TestAppContext) {
    let mut r = rig(cx, None);
    let texts = || ToastTexts {
        owner_toasts: true,
        ..end_task_texts()
    };
    let ids: Vec<ActionId> = (0..17)
        .map(|_| {
            let id = r.track(end_task(1), texts());
            r.finish(id, &[ActionState::Succeeded]);
            id
        })
        .collect();
    let kept = |r: &mut Rig, id| r.tracker.read_with(&r.vcx, |t, _| t.results(id).is_some());
    assert!(!kept(&mut r, ids[0]), "the oldest is dropped");
    assert!(kept(&mut r, ids[1]));
    assert!(kept(&mut r, ids[16]));
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
        let tracker = vcx.update(|_, cx| cx.new(|cx| ActionTracker::new(client, None, cx)));
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

mod desktop_on_host {
    use super::*;
    use tl_ui::host::HostContext;

    fn open_uri() -> ActionRequest {
        request(ActionKind::OpenUri("file:///a".into()), [])
    }

    fn remote_rig(cx: &mut TestAppContext, name: Option<&str>) -> Rig {
        let mut r = rig(cx, None);
        if let Some(name) = name {
            let tracker = r.tracker.clone();
            r.vcx.update(|_, cx| {
                tracker.update(cx, |t, _| t.set_remote_name(Arc::from(name)));
            });
        }
        r
    }

    #[gpui_kit::test]
    fn remote_success_toast_says_opened_on_host(cx: &mut TestAppContext) {
        let mut r = remote_rig(cx, Some("home"));
        let id = r.track(open_uri(), ToastTexts::new("", ""));
        r.finish(id, &[ActionState::Succeeded]);
        assert_eq!(r.last().text.as_ref(), "Opened on home");
    }

    #[gpui_kit::test]
    fn remote_launch_and_switch_say_started_and_switched(cx: &mut TestAppContext) {
        let mut r = remote_rig(cx, Some("home"));
        let id = r.track(
            request(
                ActionKind::ActivateWindow {
                    internal_id: "{w}".into(),
                },
                [],
            ),
            ToastTexts::new("Process window shown.", "Could not show the window."),
        );
        r.finish(id, &[ActionState::Succeeded]);
        assert_eq!(r.last().text.as_ref(), "Switched to window on home");
        let id = r.track(
            request(ActionKind::StartupRunNow, []),
            ToastTexts::new("", ""),
        );
        r.finish(id, &[ActionState::Succeeded]);
        assert_eq!(r.last().text.as_ref(), "Started on home");
    }

    #[gpui_kit::test]
    fn remote_failure_names_the_host(cx: &mut TestAppContext) {
        let mut r = remote_rig(cx, Some("home"));
        let texts = ToastTexts::new("", "Could not open file location.");
        let id = r.track(open_uri(), texts);
        r.finish(id, &[ActionState::Failed(ActionError::Unsupported)]);
        let text = r.last().text.to_string();
        assert!(
            text.starts_with("Could not open file location on home."),
            "{text}"
        );
    }

    #[gpui_kit::test]
    fn remote_no_desktop_session_names_the_host(cx: &mut TestAppContext) {
        let mut r = remote_rig(cx, Some("home"));
        let id = r.track(open_uri(), ToastTexts::new("", "Could not open."));
        r.finish(id, &[ActionState::Rejected(RejectReason::NoDesktopSession)]);
        assert_eq!(
            r.last().text.as_ref(),
            "home has no desktop session. Nobody is logged in to its screen."
        );
    }

    #[gpui_kit::test]
    fn local_host_toasts_are_unchanged(cx: &mut TestAppContext) {
        let mut r = remote_rig(cx, None);
        let id = r.track(
            open_uri(),
            ToastTexts::new("Opened it", "Could not open file location."),
        );
        r.finish(id, &[ActionState::Succeeded]);
        assert_eq!(r.last().text.as_ref(), "Opened it");
        let id = r.track(
            open_uri(),
            ToastTexts::new("", "Could not open file location."),
        );
        r.finish(id, &[ActionState::Failed(ActionError::Unsupported)]);
        let text = r.last().text.to_string();
        assert!(!text.contains(" on home"), "{text}");
        assert_eq!(
            text,
            "Could not open file location. Not supported on this system."
        );
    }

    #[gpui_kit::test]
    fn other_kinds_on_a_remote_host_are_unchanged(cx: &mut TestAppContext) {
        let mut r = remote_rig(cx, Some("home"));
        let id = r.track(end_task(1), end_task_texts());
        r.finish(id, &[ActionState::Succeeded]);
        assert!(!r.last().text.contains("on home"), "{:?}", r.last().text);
    }

    #[gpui_kit::test]
    fn menu_label_gets_the_suffix_only_on_a_remote_host(cx: &mut TestAppContext) {
        let mut r = remote_rig(cx, None);
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (local, remote) = r.vcx.update(|_, cx| {
            let remote = HostContext::remote(Arc::from("home"), client, 1 << 48, cx);
            let local = HostContext::from_parts(None, None, None, None, None);
            let label = |h: &HostContext, cx: &gpui_kit::App| {
                h.desktop_label("Switch to window".into(), cx).to_string()
            };
            (label(&local, cx), label(&remote, cx))
        });
        assert_eq!(local, "Switch to window");
        assert_eq!(remote, "Switch to window on home");
    }
}

/// Root actions on a remote host: the confirmation names the host, and a remote-admin refusal
/// says how to turn it on (ADR-018, tasklens-q4x76.32).
mod remote_root {
    use super::*;
    use gpui_kit::{AnyWindowHandle, Modifiers};

    fn rig_on(cx: &mut TestAppContext, host: Option<&str>) -> Rig {
        let mut r = rig(cx, None);
        if let Some(host) = host {
            let tracker = r.tracker.clone();
            r.vcx.update(|_, cx| {
                tracker.update(cx, |t, _| t.set_remote_name(Arc::from(host)));
            });
        }
        r
    }

    fn privileged_end_task() -> ActionRequest {
        ActionRequest {
            privileged: true,
            ..end_task(1)
        }
    }

    /// Sends `request` the way an owner does, through `track_or_toast`.
    fn send(r: &mut Rig, request: ActionRequest) -> Option<ActionId> {
        let queue = r.queue.downgrade();
        let tracker = r.tracker.clone();
        let id = r.vcx.update(|_, cx| {
            tracker.update(cx, |t, cx| {
                t.track_or_toast(
                    request,
                    queue,
                    ToastTexts::new("", "Could not end task."),
                    cx,
                )
            })
        });
        r.vcx.run_until_parked();
        id
    }

    fn dialogs(r: &mut Rig) -> Vec<(AnyWindowHandle, Entity<TlDialogView>)> {
        r.vcx.update(|_, cx| {
            cx.windows()
                .into_iter()
                .filter_map(|w| {
                    let handle = w.downcast::<Root>()?;
                    let frame = TlSecondaryWindow::<TlDialogView>::find(handle, cx)?;
                    Some((w, frame.read(cx).content().clone()))
                })
                .collect()
        })
    }

    fn choose(r: &mut Rig, selector: &'static str) {
        let (any, _) = dialogs(r).pop().expect("a confirmation");
        let mut vcx = VisualTestContext::from_window(any, &r.vcx.cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let at = vcx.debug_bounds(selector).expect("dialog button").center();
        vcx.simulate_click(at, Modifiers::none());
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn root_action_on_a_remote_host_asks_first_and_names_the_host(cx: &mut TestAppContext) {
        let mut r = rig_on(cx, Some("home"));
        assert!(send(&mut r, privileged_end_task()).is_none());
        assert!(
            r.actions_sent().is_empty(),
            "nothing sent before the answer"
        );
        let open = dialogs(&mut r);
        assert_eq!(open.len(), 1);
        let (text, destructive) = open[0].1.read_with(&r.vcx, |v, _| {
            (v.dialog().text().to_string(), v.dialog().is_destructive())
        });
        assert_eq!(text, "End task as root on home?");
        assert!(destructive);
        choose(&mut r, "TlDialogAction");
        let sent = r.actions_sent();
        assert_eq!(sent.len(), 1, "sent after the confirmation");
        assert!(sent[0].privileged);
    }

    #[gpui_kit::test]
    fn cancelling_the_root_confirmation_sends_nothing(cx: &mut TestAppContext) {
        let mut r = rig_on(cx, Some("home"));
        send(&mut r, privileged_end_task());
        choose(&mut r, "TlDialogCancel");
        assert!(r.actions_sent().is_empty());
        assert_eq!(r.pending(), 0);
    }

    #[gpui_kit::test]
    fn system_unit_and_socket_actions_ask_on_a_remote_host(cx: &mut TestAppContext) {
        let mut r = rig_on(cx, Some("home"));
        send(
            &mut r,
            request(ActionKind::UnitRestart, [unit("cups.service")]),
        );
        let text = dialogs(&mut r)[0]
            .1
            .read_with(&r.vcx, |v, _| v.dialog().text().to_string());
        assert_eq!(text, "Change service as root on home?");
        assert!(r.actions_sent().is_empty());
    }

    #[gpui_kit::test]
    fn local_host_sends_root_actions_at_once(cx: &mut TestAppContext) {
        let mut r = rig_on(cx, None);
        assert!(send(&mut r, privileged_end_task()).is_some());
        assert!(dialogs(&mut r).is_empty(), "no dialog on this PC");
        assert_eq!(r.actions_sent().len(), 1);
    }

    #[gpui_kit::test]
    fn a_remote_action_that_needs_no_root_sends_at_once(cx: &mut TestAppContext) {
        let mut r = rig_on(cx, Some("home"));
        assert!(send(&mut r, end_task(1)).is_some());
        assert!(dialogs(&mut r).is_empty());
        assert_eq!(r.actions_sent().len(), 1);
    }

    #[gpui_kit::test]
    fn refusal_for_remote_admin_says_how_to_turn_it_on(cx: &mut TestAppContext) {
        let mut r = rig_on(cx, Some("home"));
        let id = r.track(
            privileged_end_task(),
            ToastTexts::new("", "Could not end task."),
        );
        r.finish(id, &[ActionState::Rejected(RejectReason::RemoteAdminOff)]);
        assert_eq!(
            r.last().text.as_ref(),
            "Remote admin is off on home. Run tasklens remote-admin on there."
        );
    }

    /// Fails `end_task(1)` with EPERM on remote host "home" whose helper is `helper` and whose
    /// remote admin is `remote_admin`. Returns the last toast's button.
    fn eperm_button_on_home(
        cx: &mut TestAppContext,
        helper: HelperState,
        remote_admin: bool,
    ) -> Option<SharedString> {
        let mut r = rig(cx, Some(helper));
        r.engine
            .publish_host_settings(|v| v.remote_admin = remote_admin);
        r.vcx.run_until_parked();
        let tracker = r.tracker.clone();
        r.vcx
            .update(|_, cx| tracker.update(cx, |t, _| t.set_remote_name(Arc::from("home"))));
        let texts = ToastTexts {
            retry_as_admin: true,
            ..ToastTexts::new("", "Could not end task.")
        };
        let id = r.track(end_task(1), texts);
        r.finish(id, &[ActionState::Failed(ActionError::PermissionDenied)]);
        r.last().button
    }

    #[gpui_kit::test]
    fn retry_as_administrator_on_a_remote_host_follows_that_hosts_helper(cx: &mut TestAppContext) {
        let button = eperm_button_on_home(cx, HelperState::Missing, true);
        assert_eq!(button, None, "no helper on home: no retry button");
    }

    #[gpui_kit::test]
    fn retry_as_administrator_on_a_remote_host_needs_remote_admin_on(cx: &mut TestAppContext) {
        let button = eperm_button_on_home(cx, HelperState::Unlocked, false);
        assert_eq!(
            button, None,
            "remote admin off on home: the retry would be refused"
        );
    }

    #[gpui_kit::test]
    fn retry_as_administrator_shows_on_a_remote_host_with_helper_and_remote_admin(
        cx: &mut TestAppContext,
    ) {
        let button = eperm_button_on_home(cx, HelperState::Unlocked, true);
        assert!(button.is_some(), "helper and remote admin on: retry shows");
    }

    #[gpui_kit::test]
    fn on_sent_gets_the_id_after_the_confirmation(cx: &mut TestAppContext) {
        let mut r = rig_on(cx, Some("home"));
        let got: Rc<RefCell<Vec<ActionId>>> = Rc::default();
        let sink = got.clone();
        let (queue, tracker) = (r.queue.downgrade(), r.tracker.clone());
        let on_sent: tl_ui::action_tracker::OnSentId =
            Rc::new(move |id, _| sink.borrow_mut().push(id));
        let now = r.vcx.update(|_, cx| {
            tracker.update(cx, |t, cx| {
                t.track_or_toast_then(
                    privileged_end_task(),
                    queue,
                    ToastTexts::new("", "Could not end task."),
                    Some(on_sent),
                    cx,
                )
            })
        });
        r.vcx.run_until_parked();
        assert!(now.is_none());
        assert!(got.borrow().is_empty(), "not before the answer");
        choose(&mut r, "TlDialogAction");
        assert_eq!(got.borrow().len(), 1, "once, after the confirmation");
        assert_eq!(r.actions_sent().len(), 1);
    }
}
