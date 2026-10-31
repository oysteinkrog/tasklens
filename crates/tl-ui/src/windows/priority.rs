//! Custom Set Priority dialog (UI-PWN-023, UI-PWN-024, PROC-066, PROC-069, PROC-079, R-038,
//! E23-007).
//!
//! The dialog sets the scheduling policy, the nice value and the realtime priority of its
//! targets. "Set priority" sends:
//! - Normal or Batch, same policy as when the dialog opened: one `SetNice(n)`.
//! - Normal or Batch, another policy: `SetSched { policy, 0 }`, and when every target succeeded,
//!   `SetNice(n)`. Both use one toast key, so one toast shows the result.
//! - Idle: `SetSched { Idle, 0 }`.
//! - FIFO or RR: the Realtime confirmation first (UI-PRC-014), then `SetSched { policy, rt }`.
//!   Cancel there sends nothing and the dialog stays open.
//!
//! The dialog closes once the request is sent. Nothing is checked here beyond the ranges: the
//! engine's failure gives the PROC-058 toast, such as "Permission denied".

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::base::slider::SliderState;
use gpui_kit::base::{ElementExt as _, Root};
use gpui_kit::component::Icon;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, WeakEntity, Window, WindowHandle, div,
    px, size,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionKind, ActionRequest, ActionState, ActionTarget, SchedPolicy};
use tracing::warn;

use crate::action_tracker::{ActionTrackerEvent, SummaryTexts, ToastTexts, short_reason};
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::icons::TlIcon;
use crate::components::select::{TlSelect, TlSelectItem, TlSelectState};
use crate::components::slider::{KeyMove, SliderPageDown, SliderPageUp, TlSlider, key_move};
use crate::components::stepper::TlStepper;
use crate::components::toast::{ToastKey, ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::window_frame::{TlSecondaryWindow, frame_bottom};
use crate::host::HostContext;
use crate::i18n::Strings;
use crate::pages::performance::details::Template;
use crate::pages::processes::confirm::{ConfirmKind, ConfirmResult, confirm};
use crate::text::style::{t_base, t_sm, t_xs};
use crate::theme::{current, metrics};
use crate::windows::actions::{PRIORITY_DIALOG_CONTEXT, PriorityApply, PrioritySliderStep};
use crate::windows::frame::{PwnWindow, open_pwn};

/// The nice range.
pub const NICE_MIN: i8 = -20;
pub const NICE_MAX: i8 = 19;
/// The realtime priority range for FIFO and RR.
pub const RT_MIN: u8 = 1;
pub const RT_MAX: u8 = 99;

/// Sizes at 100% scale (UI-PWN-023).
const SUBTITLE_H: f32 = 24.;
const ROW_H: f32 = 32.;
const SCALE_ROW_H: f32 = 18.;
const LABEL_W: f32 = 128.;
const CONTROL_W: f32 = 260.;
/// TlSlider's track length outside a menu; the scale labels line up under it.
const TRACK_W: f32 = 200.;
const WARNING_ICON: f32 = 14.;
const FOOTER_H: f32 = 52.;

const KEY_TITLE: &str = "pwn-priority-title";
const KEY_POLICY: &str = "pwn-priority-policy";
const KEY_POLICY_OTHER: &str = "pwn-priority-policy-other";
const KEY_POLICY_BATCH: &str = "pwn-priority-policy-batch";
const KEY_POLICY_IDLE: &str = "pwn-priority-policy-idle";
const KEY_POLICY_FIFO: &str = "pwn-priority-policy-fifo";
const KEY_POLICY_RR: &str = "pwn-priority-policy-rr";
const KEY_APP_TIP: &str = "pwn-priority-policy-app-tooltip";
const KEY_NICE: &str = "pwn-priority-nice";
const KEY_NICE_NAME: &str = "pwn-priority-nice-name";
const KEY_NICE_HIGHEST: &str = "pwn-priority-nice-highest";
const KEY_NICE_LOWEST: &str = "pwn-priority-nice-lowest";
const KEY_NICE_TIP: &str = "pwn-priority-nice-tooltip";
const KEY_NOT_USED: &str = "pwn-priority-not-used";
const KEY_RT: &str = "pwn-priority-realtime";
const KEY_WARNING: &str = "pwn-priority-warning";
const KEY_WARNING_CHILDREN: &str = "pwn-priority-warning-children";
const KEY_CANCEL: &str = "pwn-priority-cancel";
const KEY_APPLY: &str = "pwn-priority-apply";
const KEY_TOAST_OK: &str = "pwn-priority-toast-success";
const KEY_TOAST_FAIL: &str = "pwn-priority-toast-failure";
const KEY_TOAST_MANY: &str = "pwn-priority-toast-success-many";
const KEY_TOAST_PARTIAL: &str = "pwn-priority-toast-partial";
const KEY_TOAST_THREADS: &str = "pwn-priority-toast-threads";
const KEY_TOAST_PARTIAL_THREADS: &str = "pwn-priority-toast-partial-threads";

/// The policies in list order, with their select values.
const POLICIES: [(SchedPolicy, &str); 5] = [
    (SchedPolicy::Other, "other"),
    (SchedPolicy::Batch, "batch"),
    (SchedPolicy::Idle, "idle"),
    (SchedPolicy::Fifo, "fifo"),
    (SchedPolicy::Rr, "rr"),
];

fn policy_value(policy: SchedPolicy) -> &'static str {
    POLICIES
        .iter()
        .find(|(p, _)| *p == policy)
        .map_or("other", |(_, v)| v)
}

fn policy_of(value: &str) -> Option<SchedPolicy> {
    POLICIES.iter().find(|(_, v)| *v == value).map(|(p, _)| *p)
}

fn is_realtime(policy: SchedPolicy) -> bool {
    matches!(policy, SchedPolicy::Fifo | SchedPolicy::Rr)
}

/// What "Set priority" sends first. `then_nice` follows `first` once every target succeeded.
#[derive(Clone, Debug, PartialEq)]
pub struct PriorityPlan {
    pub first: ActionKind,
    pub then_nice: Option<i8>,
    /// The Realtime confirmation comes before `first`.
    pub confirm: bool,
}

/// The requests for `policy`, `nice` and `rt_prio` when the dialog opened with `initial` policy.
pub fn plan(initial: SchedPolicy, policy: SchedPolicy, nice: i8, rt_prio: u8) -> PriorityPlan {
    let sched = |rt_prio| ActionKind::SetSched { policy, rt_prio };
    match policy {
        SchedPolicy::Other | SchedPolicy::Batch if policy == initial => PriorityPlan {
            first: ActionKind::SetNice(nice),
            then_nice: None,
            confirm: false,
        },
        SchedPolicy::Other | SchedPolicy::Batch => PriorityPlan {
            first: sched(0),
            then_nice: Some(nice),
            confirm: false,
        },
        // The dialog never offers Deadline and the engine refuses it.
        SchedPolicy::Idle | SchedPolicy::Deadline => PriorityPlan {
            first: sched(0),
            then_nice: None,
            confirm: false,
        },
        SchedPolicy::Fifo | SchedPolicy::Rr => PriorityPlan {
            first: sched(rt_prio.clamp(RT_MIN, RT_MAX)),
            then_nice: None,
            confirm: true,
        },
    }
}

// ---- Words (R-132) ----

/// The dialog texts, translated once when the dialog opens.
pub struct PriorityWords {
    pub title: SharedString,
    pub policy: SharedString,
    /// In [`POLICIES`] order.
    pub policies: [SharedString; 5],
    pub app_tooltip: SharedString,
    pub nice: SharedString,
    pub nice_name: SharedString,
    pub nice_highest: SharedString,
    pub nice_lowest: SharedString,
    pub nice_tooltip: SharedString,
    pub not_used: SharedString,
    pub realtime: SharedString,
    pub warning: SharedString,
    pub warning_children: SharedString,
    pub cancel: SharedString,
    pub apply: SharedString,
    pub toast_success: SharedString,
    pub toast_failure: SharedString,
    pub toast_success_many: SharedString,
    toast_partial: Template,
    pub toast_threads: SharedString,
    toast_partial_threads: Template,
}

impl PriorityWords {
    fn with(t: impl Fn(&str) -> SharedString, tmpl: impl Fn(&str, &[&str]) -> Template) -> Self {
        let partial_args = &["ok", "total", "failed", "reason"];
        Self {
            title: t(KEY_TITLE),
            policy: t(KEY_POLICY),
            policies: [
                KEY_POLICY_OTHER,
                KEY_POLICY_BATCH,
                KEY_POLICY_IDLE,
                KEY_POLICY_FIFO,
                KEY_POLICY_RR,
            ]
            .map(&t),
            app_tooltip: t(KEY_APP_TIP),
            nice: t(KEY_NICE),
            nice_name: t(KEY_NICE_NAME),
            nice_highest: t(KEY_NICE_HIGHEST),
            nice_lowest: t(KEY_NICE_LOWEST),
            nice_tooltip: t(KEY_NICE_TIP),
            not_used: t(KEY_NOT_USED),
            realtime: t(KEY_RT),
            warning: t(KEY_WARNING),
            warning_children: t(KEY_WARNING_CHILDREN),
            cancel: t(KEY_CANCEL),
            apply: t(KEY_APPLY),
            toast_success: t(KEY_TOAST_OK),
            toast_failure: t(KEY_TOAST_FAIL),
            toast_success_many: t(KEY_TOAST_MANY),
            toast_partial: tmpl(KEY_TOAST_PARTIAL, partial_args),
            toast_threads: t(KEY_TOAST_THREADS),
            toast_partial_threads: tmpl(KEY_TOAST_PARTIAL_THREADS, partial_args),
        }
    }

    /// The current strings, or en-US without them.
    pub fn new(cx: &App) -> Self {
        if cx.try_global::<Strings>().is_some() {
            Self::with(
                |key| crate::i18n::tr(cx, key),
                |key, args| Template::new(cx, key, args),
            )
        } else {
            Self::with(|key| crate::i18n::tr_en(key, &[]), Template::en)
        }
    }

    /// "Priority updated for {ok} of {total} processes. {failed} failed: {reason}", or the
    /// threads form.
    pub fn partial(&self, ok: u16, total: u16, reason: &str, threads: bool) -> SharedString {
        let template = if threads {
            &self.toast_partial_threads
        } else {
            &self.toast_partial
        };
        let mut out = String::new();
        let _ = template.write(&mut out, &[&ok, &total, &(total - ok), &reason]);
        out.into()
    }
}

// ---- The dialog ----

pub struct PriorityDialog {
    /// The host this dialog shows. Set when it opens; never changes.
    host: HostContext,
    targets: SmallVec<[ActionTarget; 1]>,
    policy: Entity<TlSelectState>,
    nice: Entity<SliderState>,
    /// The realtime priority field of the `TlStepper`.
    rt_input: Entity<InputState>,
    rt_prio: u8,
    /// Policy, nice and realtime priority when the dialog opened.
    initial: (SchedPolicy, i8, u8),
    subtitle: SharedString,
    /// The targets are an app's processes: the policy select gets its tooltip.
    app: bool,
    /// The Realtime confirmation is open; "Set priority" waits for it.
    confirming: bool,
    words: Rc<PriorityWords>,
    cancel_focus: FocusHandle,
    /// The parent window's toast queue; results show there (UI-PRC-015).
    parent_toasts: Option<WeakEntity<ToastQueue>>,
    /// The window height last asked for.
    fitted: Rc<std::cell::Cell<Option<Pixels>>>,
    _subs: Vec<Subscription>,
}

/// Opens the Set Priority dialog for `targets`, modal to `parent`. `subtitle` has the Set
/// Affinity forms (`firefox (PID 4321)`, `4 processes`, ...). `initial` is the policy, nice and
/// realtime priority of the right-clicked target. Returns the open error and opens nothing on
/// failure. The dialog acts on `host` and stays on it.
pub fn open(
    host: &HostContext,
    targets: SmallVec<[ActionTarget; 1]>,
    subtitle: SharedString,
    initial: (SchedPolicy, i8, u8),
    parent: AnyWindowHandle,
    cx: &mut App,
) -> gpui_kit::Result<WindowHandle<Root>> {
    open_with(host, targets, subtitle, initial, false, parent, cx)
}

/// [`open`] for an app row: `app` gives the policy select the tooltip "Applies to the app's
/// current processes".
pub fn open_with(
    host: &HostContext,
    targets: SmallVec<[ActionTarget; 1]>,
    subtitle: SharedString,
    initial: (SchedPolicy, i8, u8),
    app: bool,
    parent: AnyWindowHandle,
    cx: &mut App,
) -> gpui_kit::Result<WindowHandle<Root>> {
    let words = Rc::new(PriorityWords::new(cx));
    let title = words.title.clone();
    let parent_toasts = parent
        .update(cx, |_, window, cx| ToastQueue::of_window(window, cx))
        .ok()
        .flatten()
        .map(|q| q.downgrade());
    open_pwn(
        cx,
        PwnWindow::SetPriority,
        title,
        Some(parent),
        move |window, cx| {
            cx.new(|cx| {
                PriorityDialog::new(
                    host,
                    targets,
                    subtitle,
                    initial,
                    app,
                    words,
                    parent_toasts,
                    window,
                    cx,
                )
            })
        },
    )
}

impl PriorityDialog {
    /// The content of a dialog opened by [`open`], while it is open.
    pub fn find(handle: WindowHandle<Root>, cx: &App) -> Option<Entity<Self>> {
        TlSecondaryWindow::<Self>::find(handle, cx).map(|frame| frame.read(cx).content().clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        host: &HostContext,
        targets: SmallVec<[ActionTarget; 1]>,
        subtitle: SharedString,
        initial: (SchedPolicy, i8, u8),
        app: bool,
        words: Rc<PriorityWords>,
        parent_toasts: Option<WeakEntity<ToastQueue>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let items = POLICIES
            .iter()
            .zip(words.policies.iter())
            .map(|((_, value), label)| TlSelectItem::new(*value, label.clone()))
            .collect();
        let policy = cx.new(|cx| TlSelectState::new(items, Some(policy_value(initial.0)), cx));
        let nice_value = f32::from(initial.1.clamp(NICE_MIN, NICE_MAX));
        let nice = cx.new(|_| {
            SliderState::new()
                .min(f32::from(NICE_MIN))
                .max(f32::from(NICE_MAX))
                .step(1.)
                .default_value(nice_value)
        });
        let rt_input = cx.new(|cx| InputState::new(window, cx));
        let mut subs = vec![cx.observe(&nice, |_, _, cx| cx.notify())];
        // Enter in the field applies the typed value first (the stepper hears the same event),
        // then sends.
        subs.push(
            cx.subscribe_in(&rt_input, window, |_, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    cx.defer_in(window, |this, window, cx| this.apply(window, cx));
                }
            }),
        );
        policy.read(cx).focus_handle(cx).focus(window, cx);
        Self {
            host: host.clone(),
            targets,
            policy,
            nice,
            rt_input,
            rt_prio: initial.2.clamp(RT_MIN, RT_MAX),
            initial,
            subtitle,
            app,
            confirming: false,
            words,
            cancel_focus: cx.focus_handle(),
            parent_toasts,
            fitted: Rc::default(),
            _subs: subs,
        }
    }

    // ---- Read-outs, also for tests ----

    pub fn targets(&self) -> &[ActionTarget] {
        &self.targets
    }

    pub fn initial(&self) -> (SchedPolicy, i8, u8) {
        self.initial
    }

    pub fn subtitle(&self) -> &SharedString {
        &self.subtitle
    }

    /// The policy selected now.
    pub fn policy(&self, cx: &App) -> SchedPolicy {
        self.policy
            .read(cx)
            .selected_value()
            .and_then(|v| policy_of(v))
            .unwrap_or(self.initial.0)
    }

    pub fn policy_state(&self) -> &Entity<TlSelectState> {
        &self.policy
    }

    /// The nice value on the slider now.
    pub fn nice(&self, cx: &App) -> i8 {
        let v = self.nice.read(cx).value().end().round();
        (v as i8).clamp(NICE_MIN, NICE_MAX)
    }

    pub fn nice_state(&self) -> &Entity<SliderState> {
        &self.nice
    }

    pub fn rt_prio(&self) -> u8 {
        self.rt_prio
    }

    pub fn rt_input(&self) -> &Entity<InputState> {
        &self.rt_input
    }

    /// Nice is on for Normal and Batch.
    pub fn nice_enabled(&self, cx: &App) -> bool {
        matches!(self.policy(cx), SchedPolicy::Other | SchedPolicy::Batch)
    }

    /// Realtime priority is on for FIFO and RR.
    pub fn rt_enabled(&self, cx: &App) -> bool {
        is_realtime(self.policy(cx))
    }

    /// The warning lines show for FIFO and RR.
    pub fn warning_shown(&self, cx: &App) -> bool {
        self.rt_enabled(cx)
    }

    /// The Nice tooltip: the autogroup note while on, "Not used by this policy" while off.
    pub fn nice_tooltip(&self, cx: &App) -> SharedString {
        if self.nice_enabled(cx) {
            self.words.nice_tooltip.clone()
        } else {
            self.words.not_used.clone()
        }
    }

    /// The Realtime priority tooltip: "Not used by this policy" while off.
    pub fn rt_tooltip(&self, cx: &App) -> Option<SharedString> {
        (!self.rt_enabled(cx)).then(|| self.words.not_used.clone())
    }

    /// The policy select tooltip, for app rows only.
    pub fn policy_tooltip(&self) -> Option<SharedString> {
        self.app.then(|| self.words.app_tooltip.clone())
    }

    /// The Realtime confirmation is open.
    pub fn is_confirming(&self) -> bool {
        self.confirming
    }

    /// What "Set priority" sends now.
    pub fn plan(&self, cx: &App) -> PriorityPlan {
        plan(self.initial.0, self.policy(cx), self.nice(cx), self.rt_prio)
    }

    // ---- Changes ----

    /// Selects `policy`, as a pick in the list would.
    pub fn set_policy(&mut self, policy: SchedPolicy, cx: &mut Context<Self>) {
        self.policy
            .update(cx, |s, cx| s.set_selected_value(policy_value(policy), cx));
        cx.notify();
    }

    /// Moves the slider to `nice`, clamped to -20..=19.
    pub fn set_nice(&mut self, nice: i8, window: &mut Window, cx: &mut Context<Self>) {
        let v = f32::from(nice.clamp(NICE_MIN, NICE_MAX));
        self.nice.update(cx, |s, cx| s.set_value(v, window, cx));
    }

    /// Sets the realtime priority, clamped to 1..=99.
    pub fn set_rt_prio(&mut self, rt_prio: u8, cx: &mut Context<Self>) {
        self.rt_prio = rt_prio.clamp(RT_MIN, RT_MAX);
        cx.notify();
    }

    /// "Set priority". FIFO and RR ask with the Realtime confirmation first and send only on
    /// "Set realtime". Every other policy sends at once. The dialog closes once sent.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirming {
            return;
        }
        let plan = self.plan(cx);
        if !plan.confirm {
            self.send(plan, window, cx);
            return;
        }
        self.confirming = true;
        cx.notify();
        let this = cx.entity().downgrade();
        let handle = window.window_handle();
        // `confirm` updates this window, so it runs after this update.
        cx.defer(move |cx| {
            confirm(ConfirmKind::Realtime, handle, cx, move |result, cx| {
                // The answer comes inside an update of this window; send after it.
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.confirming = false;
                            if result == ConfirmResult::Confirmed {
                                this.send(plan, window, cx);
                            } else {
                                cx.notify();
                            }
                        });
                    });
                });
            });
        });
    }

    /// Sends `plan.first`, and `plan.then_nice` once it succeeded, then closes the dialog.
    fn send(&mut self, plan: PriorityPlan, window: &mut Window, cx: &mut Context<Self>) {
        let queue = self
            .parent_toasts
            .clone()
            .or_else(|| ToastQueue::of_window(window, cx).map(|q| q.downgrade()));
        let tracker = self.host.tracker().cloned();
        let (Some(tracker), Some(queue)) = (tracker, queue) else {
            warn!(target: "tasklens::ui", "priority not sent: no action tracker or toasts");
            window.remove_window();
            return;
        };
        let targets: Vec<ActionTarget> = self.targets.iter().cloned().collect();
        let threads = targets
            .iter()
            .all(|t| matches!(t, ActionTarget::Thread { .. }));
        let words = self.words.clone();
        let id = tracker.update(cx, |t, cx| {
            let id = t.next_id();
            let key = ToastKey(id.0);
            let mut texts = toast_texts(&words, threads, key);
            if plan.then_nice.is_some() {
                // The nice request that follows shows the success.
                texts.success = SharedString::default();
                if let Some(summary) = &mut texts.summary {
                    summary.all_ok = SharedString::default();
                }
            }
            let request = ActionRequest {
                id,
                kind: plan.first,
                targets: targets.iter().cloned().collect(),
                privileged: false,
            };
            t.track_or_toast(request, queue.clone(), texts, cx)
        });
        if let (Some(first), Some(nice)) = (id, plan.then_nice) {
            follow_with_nice(&tracker, first, nice, targets, threads, words, queue, cx);
        }
        window.remove_window();
    }

    fn cancel(&mut self, window: &mut Window) {
        window.remove_window();
    }

    // ---- Keys ----

    /// Moves the slider by `delta` steps while Nice is on.
    fn step_nice(&mut self, delta: i8, window: &mut Window, cx: &mut Context<Self>) {
        if !self.nice_enabled(cx) {
            return;
        }
        key_move(&self.nice, KeyMove::Steps(f32::from(delta)), window, cx);
    }

    fn on_slider_step(
        &mut self,
        step: &PrioritySliderStep,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_nice(step.delta, window, cx);
    }

    /// Enter runs "Set priority", except on Cancel, which Enter activates itself.
    fn on_apply(&mut self, _: &PriorityApply, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel_focus.is_focused(window) {
            cx.propagate();
            return;
        }
        self.apply(window, cx);
    }
}

/// Sends `SetNice(nice)` to `targets` when request `first` finished with every target
/// succeeded, under `first`'s toast key. The subscription ends with `first`.
#[allow(clippy::too_many_arguments)]
fn follow_with_nice(
    tracker: &Entity<crate::action_tracker::ActionTracker>,
    first: tl_model::actions::ActionId,
    nice: i8,
    targets: Vec<ActionTarget>,
    threads: bool,
    words: Rc<PriorityWords>,
    queue: WeakEntity<ToastQueue>,
    cx: &mut App,
) {
    let slot: Rc<RefCell<Option<Subscription>>> = Rc::default();
    let held = slot.clone();
    let sub = cx.subscribe(tracker, move |tracker, ev: &ActionTrackerEvent, cx| {
        let ActionTrackerEvent::Finished { id, failed, .. } = *ev else {
            return;
        };
        if id != first {
            return;
        }
        // Dropped after this event, not inside it.
        let sub = held.borrow_mut().take();
        cx.defer(move |_| drop(sub));
        if failed > 0 {
            return;
        }
        let texts = toast_texts(&words, threads, ToastKey(first.0));
        let targets = targets.iter().cloned().collect();
        let queue = queue.clone();
        tracker.update(cx, |t, cx| {
            let request = ActionRequest {
                id: t.next_id(),
                kind: ActionKind::SetNice(nice),
                targets,
                privileged: false,
            };
            t.track_or_toast(request, queue, texts, cx);
        });
    });
    *slot.borrow_mut() = Some(sub);
}

/// The toast texts of one request under the dialog's `key`: "Priority updated" for one target,
/// one summary toast for two or more (PROC-069), "Thread priority updated" for threads.
fn toast_texts(words: &Rc<PriorityWords>, threads: bool, key: ToastKey) -> ToastTexts {
    let success = if threads {
        words.toast_threads.clone()
    } else {
        words.toast_success.clone()
    };
    let all_ok = if threads {
        words.toast_threads.clone()
    } else {
        words.toast_success_many.clone()
    };
    let mut texts = ToastTexts::new(success, words.toast_failure.clone());
    texts.toast_key = Some(key);
    texts.summary_min_targets = 2;
    let words = words.clone();
    texts.summary = Some(SummaryTexts {
        started_kind: ToastKind::Info,
        ..SummaryTexts::new(
            SharedString::default(),
            all_ok,
            move |ok, total, failures: &[(u16, &ActionState)]| {
                let reason = failures
                    .iter()
                    .find_map(|(_, state)| match state {
                        ActionState::Failed(err) => Some(short_reason(err)),
                        _ => None,
                    })
                    .unwrap_or_default();
                let kind = if ok == 0 {
                    ToastKind::Error
                } else {
                    ToastKind::Warning
                };
                (kind, words.partial(ok, total, &reason, threads))
            },
        )
    });
    texts
}

impl Render for PriorityDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let words = self.words.clone();
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let view = cx.entity().downgrade();
        let nice_on = self.nice_enabled(cx);
        let rt_on = self.rt_enabled(cx);

        let mut subtitle = div()
            .debug_selector(|| "PrioritySubtitle".into())
            .flex_none()
            .flex()
            .items_center()
            .h(px(SUBTITLE_H * s))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(theme.text_secondary);
        *subtitle.text_style() = t_sm(&m);
        let subtitle = subtitle.child(self.subtitle.clone());

        let row = |label: &SharedString, control: gpui_kit::AnyElement| {
            let mut label_el = div()
                .flex_none()
                .w(px(LABEL_W * s))
                .text_color(theme.text_secondary);
            *label_el.text_style() = t_base(&m);
            div()
                .flex_none()
                .flex()
                .items_center()
                .h(px(ROW_H * s))
                .child(label_el.child(label.clone()))
                .child(control)
        };
        let with_tooltip = |id: &'static str,
                            el: gpui_kit::Stateful<gpui_kit::Div>,
                            tip: Option<SharedString>,
                            window: &mut Window,
                            cx: &mut App| match tip {
            Some(tip) => {
                let state = HoverTooltip::state(id, window, cx);
                HoverTooltip::attach(
                    &state,
                    el,
                    DELAY_CONTROL,
                    TlTooltip::text(tip).build(),
                    window,
                    cx,
                )
                .into_any_element()
            }
            None => el.into_any_element(),
        };

        // Policy.
        let select = {
            let view = view.clone();
            TlSelect::new("PriorityPolicy", &self.policy)
                .width(px(CONTROL_W * s))
                .on_change(move |_, _, cx| {
                    let _ = view.update(cx, |_, cx| cx.notify());
                })
        };
        let select = div()
            .id("PriorityPolicyBox")
            .debug_selector(|| "PriorityPolicy".into())
            .flex_none()
            .child(select);
        let select = with_tooltip(
            "PriorityPolicyTip",
            select,
            self.policy_tooltip(),
            window,
            cx,
        );

        // Nice.
        let nice_value = self.nice(cx);
        let slider = div()
            .id("PriorityNice")
            .debug_selector(|| "PriorityNice".into())
            .aria_label(words.nice_name.clone())
            .flex_none()
            .w(px(CONTROL_W * s))
            .child(
                TlSlider::new(&self.nice)
                    .value_text(nice_value.to_string())
                    .disabled(!nice_on),
            );
        let slider = with_tooltip(
            "PriorityNiceTip",
            slider,
            Some(self.nice_tooltip(cx)),
            window,
            cx,
        );
        let scale = {
            let end = |text: &SharedString| {
                let mut el = div().text_color(theme.text_secondary);
                *el.text_style() = t_xs(&m);
                el.child(text.clone())
            };
            div()
                .debug_selector(|| "PriorityNiceScale".into())
                .flex_none()
                .flex()
                .items_center()
                .h(px(SCALE_ROW_H * s))
                .pl(px(LABEL_W * s))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .w(px(TRACK_W * s))
                        .child(end(&words.nice_highest))
                        .child(end(&words.nice_lowest)),
                )
        };

        // Realtime priority.
        let stepper = {
            let view = view.clone();
            TlStepper::new("PriorityRealtime", &self.rt_input)
                .range(i64::from(RT_MIN), i64::from(RT_MAX), 1)
                .value(i64::from(self.rt_prio))
                .disabled(!rt_on)
                .on_change(move |v, _, cx| {
                    let _ = view.update(cx, |this, cx| this.set_rt_prio(v as u8, cx));
                })
        };
        let stepper = div()
            .id("PriorityRealtimeBox")
            .debug_selector(|| "PriorityRealtime".into())
            .aria_label(words.realtime.clone())
            .flex_none()
            .child(stepper);
        let stepper = with_tooltip(
            "PriorityRealtimeTip",
            stepper,
            self.rt_tooltip(cx),
            window,
            cx,
        );

        let warning = rt_on.then(|| {
            let mut line = div().text_color(theme.text_primary);
            *line.text_style() = t_sm(&m);
            let mut second = div().text_color(theme.text_secondary);
            *second.text_style() = t_xs(&m);
            div()
                .debug_selector(|| "PriorityWarning".into())
                .flex_none()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(m.sp[3])
                        .child(
                            Icon::new(TlIcon::TriangleAlert)
                                .size(px(WARNING_ICON * s))
                                .text_color(theme.status.warning),
                        )
                        .child(line.child(words.warning.clone())),
                )
                .child(
                    second
                        .pl(px(WARNING_ICON * s) + m.sp[3])
                        .child(words.warning_children.clone()),
                )
        });

        // Footer.
        let cancel = {
            let view = view.clone();
            TlButton::new("PriorityCancel", TlButtonKind::Secondary)
                .label(words.cancel.clone())
                .focus_handle(self.cancel_focus.clone())
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, _| this.cancel(window));
                })
        };
        let apply = {
            let view = view.clone();
            TlButton::new("PriorityApply", TlButtonKind::Primary)
                .label(words.apply.clone())
                .disabled(self.confirming)
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.apply(window, cx));
                })
        };
        let footer = div()
            .debug_selector(|| "PriorityFooter".into())
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .gap(m.sp[4])
            .h(px(FOOTER_H * s))
            .px(m.sp[6])
            .border_t_1()
            .border_color(theme.divider)
            .child(cancel)
            .child(apply);

        // The window height follows the content.
        let fitted = self.fitted.clone();
        let fit = move |bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
            let want = bounds.bottom();
            if fitted.get() != Some(want) {
                fitted.set(Some(want));
                window.defer(cx, move |window, cx| {
                    let width = window.bounds().size.width;
                    window.resize(size(width, want + frame_bottom(window, cx)));
                });
            }
        };

        div()
            .id("PriorityDialog")
            .key_context(PRIORITY_DIALOG_CONTEXT)
            .on_action(cx.listener(Self::on_apply))
            .on_action(cx.listener(Self::on_slider_step))
            // TlSlider binds Page Up and Page Down to 10 steps; here they move 5.
            .capture_action(cx.listener(|this, _: &SliderPageUp, window, cx| {
                this.step_nice(5, window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &SliderPageDown, window, cx| {
                this.step_nice(-5, window, cx);
                cx.stop_propagation();
            }))
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.window_bg)
            .child(
                div()
                    .on_prepaint(fit)
                    .flex_none()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(m.sp[2])
                            .p(m.sp[6])
                            .child(subtitle)
                            .child(row(&words.policy, select))
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .child(row(&words.nice, slider))
                                    .child(scale),
                            )
                            .child(row(&words.realtime, stepper))
                            .children(warning),
                    )
                    .child(footer),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_by_policy() {
        use SchedPolicy::*;
        assert_eq!(
            plan(Other, Other, 5, 1),
            PriorityPlan {
                first: ActionKind::SetNice(5),
                then_nice: None,
                confirm: false,
            }
        );
        assert_eq!(
            plan(Other, Batch, 3, 1),
            PriorityPlan {
                first: ActionKind::SetSched {
                    policy: Batch,
                    rt_prio: 0
                },
                then_nice: Some(3),
                confirm: false,
            }
        );
        assert_eq!(
            plan(Batch, Idle, 3, 1).first,
            ActionKind::SetSched {
                policy: Idle,
                rt_prio: 0
            }
        );
        let rr = plan(Other, Rr, 0, 10);
        assert!(rr.confirm);
        assert_eq!(
            rr.first,
            ActionKind::SetSched {
                policy: Rr,
                rt_prio: 10
            }
        );
        // Realtime always confirms, also when the policy does not change.
        assert!(plan(Fifo, Fifo, 0, 50).confirm);
        assert_eq!(policy_of(policy_value(Idle)), Some(Idle));
        // The dialog never offers Deadline. A plan for it is a bare SetSched the engine refuses.
        assert_eq!(
            plan(Other, Deadline, 3, 10),
            PriorityPlan {
                first: ActionKind::SetSched {
                    policy: Deadline,
                    rt_prio: 0
                },
                then_nice: None,
                confirm: false,
            }
        );
        assert_eq!(policy_of(policy_value(Deadline)), Some(Other));
    }
}
