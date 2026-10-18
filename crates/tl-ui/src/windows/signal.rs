//! Send Signal menu and real-time signal dialog (UI-PWN-026, UI-PWN-027, PROC-065, PROC-059).

use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};

use gpui_kit::base::ElementExt as _;
use gpui_kit::base::Root;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled, Subscription,
    WeakEntity, Window, WindowHandle, div, px, size,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionKind, ActionRequest, ActionTarget, SIGRTMAX, SIGRTMIN, Signal};
use tracing::warn;

use crate::action_tracker::{
    ActionWords, GlobalActionTracker, SummaryTexts, ToastTexts, action_words,
};
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::menu::TlMenu;
use crate::components::stepper::TlStepper;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{TlSecondaryWindow, frame_bottom};
use crate::i18n::tr;
use crate::pages::performance::details::Template;
use crate::pages::processes::confirm::{ConfirmKind, ConfirmResult, ProtectedAction, confirm};
use crate::text::style::{t_base, t_sm};
use crate::theme::{current, metrics};
use crate::windows::actions::{SIGNAL_DIALOG_CONTEXT, SendSignal, SignalSendRealtime};
use crate::windows::frame::{PwnWindow, open_pwn};

/// One standard signal in menu order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalInfo {
    pub name: &'static str,
    pub number: i32,
    pub label: &'static str,
    pub ends_process: bool,
    pub top_level: bool,
}

/// Linux x86-64 standard signals. The named nine come first, then the Other signals submenu.
pub static SIGNALS: [SignalInfo; 31] = [
    SignalInfo {
        name: "TERM",
        number: 15,
        label: "Terminate (TERM)",
        ends_process: true,
        top_level: true,
    },
    SignalInfo {
        name: "KILL",
        number: 9,
        label: "Kill (KILL)",
        ends_process: true,
        top_level: true,
    },
    SignalInfo {
        name: "HUP",
        number: 1,
        label: "Hangup (HUP)",
        ends_process: true,
        top_level: true,
    },
    SignalInfo {
        name: "INT",
        number: 2,
        label: "Interrupt (INT)",
        ends_process: true,
        top_level: true,
    },
    SignalInfo {
        name: "QUIT",
        number: 3,
        label: "Quit (QUIT)",
        ends_process: true,
        top_level: true,
    },
    SignalInfo {
        name: "STOP",
        number: 19,
        label: "Stop (STOP)",
        ends_process: false,
        top_level: true,
    },
    SignalInfo {
        name: "CONT",
        number: 18,
        label: "Continue (CONT)",
        ends_process: false,
        top_level: true,
    },
    SignalInfo {
        name: "USR1",
        number: 10,
        label: "User 1 (USR1)",
        ends_process: true,
        top_level: true,
    },
    SignalInfo {
        name: "USR2",
        number: 12,
        label: "User 2 (USR2)",
        ends_process: true,
        top_level: true,
    },
    SignalInfo {
        name: "ABRT",
        number: 6,
        label: "ABRT (6)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "ALRM",
        number: 14,
        label: "ALRM (14)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "BUS",
        number: 7,
        label: "BUS (7)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "CHLD",
        number: 17,
        label: "CHLD (17)",
        ends_process: false,
        top_level: false,
    },
    SignalInfo {
        name: "FPE",
        number: 8,
        label: "FPE (8)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "ILL",
        number: 4,
        label: "ILL (4)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "IO/POLL",
        number: 29,
        label: "IO/POLL (29)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "PIPE",
        number: 13,
        label: "PIPE (13)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "PROF",
        number: 27,
        label: "PROF (27)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "PWR",
        number: 30,
        label: "PWR (30)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "SEGV",
        number: 11,
        label: "SEGV (11)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "STKFLT",
        number: 16,
        label: "STKFLT (16)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "SYS",
        number: 31,
        label: "SYS (31)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "TRAP",
        number: 5,
        label: "TRAP (5)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "TSTP",
        number: 20,
        label: "TSTP (20)",
        ends_process: false,
        top_level: false,
    },
    SignalInfo {
        name: "TTIN",
        number: 21,
        label: "TTIN (21)",
        ends_process: false,
        top_level: false,
    },
    SignalInfo {
        name: "TTOU",
        number: 22,
        label: "TTOU (22)",
        ends_process: false,
        top_level: false,
    },
    SignalInfo {
        name: "URG",
        number: 23,
        label: "URG (23)",
        ends_process: false,
        top_level: false,
    },
    SignalInfo {
        name: "VTALRM",
        number: 26,
        label: "VTALRM (26)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "WINCH",
        number: 28,
        label: "WINCH (28)",
        ends_process: false,
        top_level: false,
    },
    SignalInfo {
        name: "XCPU",
        number: 24,
        label: "XCPU (24)",
        ends_process: true,
        top_level: false,
    },
    SignalInfo {
        name: "XFSZ",
        number: 25,
        label: "XFSZ (25)",
        ends_process: true,
        top_level: false,
    },
];

/// The row shape used for the subtitle and success toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalSubject {
    Process { name: SharedString, pid: u32 },
    Processes(u32),
    App { name: SharedString, count: u32 },
}

/// A target owned by another user, for the shared UI-PRC-014 confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtherUserTarget {
    pub name: SharedString,
    pub pid: u32,
    pub user: SharedString,
}

/// A refusal that must happen before confirmation or dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalRefusal {
    Tasklens,
    Pid1,
    KernelThread,
}

/// Display and protection metadata captured with the action targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetNames {
    pub subject: SignalSubject,
    pub refusal: Option<SignalRefusal>,
    pub protected: Option<SharedString>,
    pub other_user: Option<OtherUserTarget>,
}

impl TargetNames {
    pub fn process(name: impl Into<SharedString>, pid: u32) -> Self {
        Self {
            subject: SignalSubject::Process {
                name: name.into(),
                pid,
            },
            refusal: None,
            protected: None,
            other_user: None,
        }
    }

    pub fn processes(count: u32) -> Self {
        Self {
            subject: SignalSubject::Processes(count),
            refusal: None,
            protected: None,
            other_user: None,
        }
    }

    pub fn app(name: impl Into<SharedString>, count: u32) -> Self {
        Self {
            subject: SignalSubject::App {
                name: name.into(),
                count,
            },
            refusal: None,
            protected: None,
            other_user: None,
        }
    }

    pub fn with_refusal(mut self, refusal: SignalRefusal) -> Self {
        self.refusal = Some(refusal);
        self
    }

    pub fn with_protected(mut self, name: impl Into<SharedString>) -> Self {
        self.protected = Some(name.into());
        self
    }

    pub fn with_other_user(mut self, target: OtherUserTarget) -> Self {
        self.other_user = Some(target);
        self
    }

    pub fn count(&self) -> u32 {
        match &self.subject {
            SignalSubject::Process { .. } => 1,
            SignalSubject::Processes(n) | SignalSubject::App { count: n, .. } => *n,
        }
    }

    pub fn subtitle(&self) -> SharedString {
        match &self.subject {
            SignalSubject::Process { name, pid } => format!("{name} (PID {pid})").into(),
            SignalSubject::Processes(n) => format!("{n} processes").into(),
            SignalSubject::App { name, count } => format!("{name} ({count} processes)").into(),
        }
    }
}

fn info(signo: i32) -> Option<&'static SignalInfo> {
    SIGNALS.iter().find(|s| s.number == signo)
}

fn signal(signo: i32) -> Option<Signal> {
    Some(match signo {
        15 => Signal::Term,
        9 => Signal::Kill,
        1 => Signal::Hup,
        2 => Signal::Int,
        3 => Signal::Quit,
        19 => Signal::Stop,
        18 => Signal::Cont,
        10 => Signal::Usr1,
        12 => Signal::Usr2,
        n => Signal::Other(u8::try_from(n).ok()?),
    })
}

fn ends_process(signo: i32) -> bool {
    info(signo).map_or((SIGRTMIN..=SIGRTMAX).contains(&signo), |s| s.ends_process)
}

/// `SIGHUP`, `SIGRTMIN+2`, or `SIG37` for an otherwise unknown signal number.
pub fn signal_label(signo: i32) -> SharedString {
    if (SIGRTMIN..=SIGRTMAX).contains(&signo) {
        return format!("SIGRTMIN+{}", signo - SIGRTMIN).into();
    }
    info(signo)
        .map(|s| format!("SIG{}", s.name))
        .unwrap_or_else(|| format!("SIG{signo}"))
        .into()
}

fn menu_label(signal: &SignalInfo, suffix: &str) -> SharedString {
    if signal.ends_process {
        format!("{} {suffix}", signal.label).into()
    } else {
        signal.label.into()
    }
}

fn add_signal_item(
    menu: TlMenu,
    signal: &SignalInfo,
    disabled: bool,
    suffix: &str,
    tip: &SharedString,
) -> TlMenu {
    let label = menu_label(signal, suffix);
    if disabled {
        menu.item_disabled(label, Some(tip.clone()))
    } else {
        menu.item(
            label,
            Box::new(SendSignal {
                signo: signal.number,
            }),
        )
    }
}

/// Builds the Send Signal submenu on demand.
pub fn signal_submenu(
    mut menu: TlMenu,
    all_kernel_threads: bool,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let suffix = tr(cx, "pwn-signal-ends-process");
    let kernel_tip = tr(cx, "fnd-action-reject-kernel-thread");
    menu = menu.min_w(px(240.));
    for signal in SIGNALS.iter().filter(|s| s.top_level) {
        menu = add_signal_item(menu, signal, all_kernel_threads, &suffix, &kernel_tip);
    }
    menu = menu.separator().submenu(
        tr(cx, "pwn-signal-other-signals"),
        window,
        cx,
        move |mut sub, _, cx| {
            let suffix = tr(cx, "pwn-signal-ends-process");
            let tip = tr(cx, "fnd-action-reject-kernel-thread");
            sub = sub.min_w(px(240.));
            for signal in SIGNALS.iter().filter(|s| !s.top_level) {
                sub = add_signal_item(sub, signal, all_kernel_threads, &suffix, &tip);
            }
            sub
        },
    );
    if all_kernel_threads {
        menu.item_disabled(tr(cx, "pwn-signal-realtime"), Some(kernel_tip))
    } else {
        menu.item(
            tr(cx, "pwn-signal-realtime"),
            Box::new(crate::windows::actions::SignalOpenRealtime),
        )
    }
}

struct SignalWords {
    sent_process: Template,
    sent_many: Template,
    sent_app: Template,
    sent_partial: Template,
    failure: SharedString,
}

impl SignalWords {
    fn new(cx: &App) -> Self {
        Self {
            sent_process: Template::new(cx, "pwn-signal-toast-process", &["signal", "name", "pid"]),
            sent_many: Template::new(cx, "pwn-signal-toast-many", &["signal", "count"]),
            sent_app: Template::new(cx, "pwn-signal-toast-app", &["signal", "name", "count"]),
            sent_partial: Template::new(
                cx,
                "pwn-signal-toast-partial",
                &["signal", "ok", "total", "failed", "reason"],
            ),
            failure: tr(cx, "pwn-signal-toast-failure"),
        }
    }

    fn success(&self, signal: &SharedString, names: &TargetNames) -> SharedString {
        let mut out = String::new();
        match &names.subject {
            SignalSubject::Process { name, pid } => {
                let _ = self.sent_process.write(&mut out, &[signal, name, pid]);
            }
            SignalSubject::Processes(count) => {
                let _ = self.sent_many.write(&mut out, &[signal, count]);
            }
            SignalSubject::App { name, count } => {
                let _ = self.sent_app.write(&mut out, &[signal, name, count]);
            }
        }
        out.into()
    }
}

fn toast_texts(
    words: Rc<SignalWords>,
    action_words: Rc<ActionWords>,
    signo: i32,
    names: TargetNames,
) -> ToastTexts {
    let label = signal_label(signo);
    let success = words.success(&label, &names);
    let mut texts = ToastTexts::new(success.clone(), words.failure.clone());
    texts.retry_as_admin = true;
    texts.summary_min_targets = 2;
    let partial_words = words.clone();
    let partial_label = label.clone();
    texts.summary = Some(SummaryTexts::new(
        SharedString::default(),
        success,
        move |ok, total, failures| {
            let reason = failures
                .iter()
                .find_map(|(_, state)| action_words.short_reason(state))
                .unwrap_or_default();
            let mut out = String::new();
            let failed = total - ok;
            let _ = partial_words
                .sent_partial
                .write(&mut out, &[&partial_label, &ok, &total, &failed, &reason]);
            let kind = if ok == 0 {
                ToastKind::Error
            } else {
                ToastKind::Warning
            };
            (kind, out.into())
        },
    ));
    texts
}

fn refusal_text(refusal: SignalRefusal, cx: &App) -> SharedString {
    let key = match refusal {
        SignalRefusal::Tasklens => "processes-toast-refuse-self",
        SignalRefusal::Pid1 => "processes-toast-refuse-pid1",
        SignalRefusal::KernelThread => "fnd-action-reject-kernel-thread",
    };
    tr(cx, key)
}

fn push_warning(text: SharedString, window: &Window, cx: &mut App) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(ToastKind::Warning, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast queue for the signal refusal"),
    }
}

fn track_signal(
    signo: i32,
    targets: SmallVec<[ActionTarget; 1]>,
    names: TargetNames,
    queue: WeakEntity<ToastQueue>,
    cx: &mut App,
) {
    let Some(signal) = signal(signo) else {
        warn!(target: "tasklens::ui", signo, "invalid signal number");
        return;
    };
    let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
        warn!(target: "tasklens::ui", "signal not sent: no action tracker");
        return;
    };
    let words = Rc::new(SignalWords::new(cx));
    let action_words = action_words(cx);
    tracker.update(cx, |tracker, cx| {
        tracker.track_or_toast(
            ActionRequest {
                id: tracker.next_id(),
                kind: ActionKind::Signal(signal),
                targets,
                privileged: false,
            },
            queue,
            toast_texts(words, action_words, signo, names),
            cx,
        );
    });
}

/// Applies the protection rules, optionally confirms, then sends one signal request.
pub fn send_signal(
    signo: i32,
    targets: SmallVec<[ActionTarget; 1]>,
    names: TargetNames,
    window: &mut Window,
    cx: &mut App,
) {
    let process_pid = |target: &ActionTarget| match target {
        ActionTarget::Process(key) => Some(key.pid),
        ActionTarget::Thread { process, .. } => Some(process.pid),
        _ => None,
    };
    let inferred = if targets
        .iter()
        .filter_map(process_pid)
        .any(|pid| pid == std::process::id())
    {
        Some(SignalRefusal::Tasklens)
    } else if targets.iter().filter_map(process_pid).any(|pid| pid == 1) {
        Some(SignalRefusal::Pid1)
    } else {
        None
    };
    if let Some(refusal) = inferred.or(names.refusal) {
        push_warning(refusal_text(refusal, cx), window, cx);
        return;
    }
    let Some(queue) = ToastQueue::of_window(window, cx).map(|q| q.downgrade()) else {
        warn!(target: "tasklens::ui", "signal not sent: no toast queue");
        return;
    };
    let label = signal_label(signo);
    let short = label
        .strip_prefix("SIG")
        .unwrap_or(label.as_ref())
        .to_string();
    let action = tr(cx, "pwn-signal-send");
    let ask = if let Some(name) = &names.protected {
        Some(ConfirmKind::Protected {
            action: ProtectedAction::Signal,
            name: name.clone(),
            tree_count: None,
        })
    } else if let Some(other) = &names.other_user {
        Some(ConfirmKind::OtherUser {
            action,
            name: other.name.clone(),
            pid: other.pid,
            user: other.user.clone(),
            destructive: ends_process(signo),
        })
    } else if names.count() > 1 {
        Some(ConfirmKind::SignalMany {
            count: names.count(),
            signal_name: short.into(),
            ends_process: ends_process(signo),
        })
    } else {
        None
    };
    let Some(ask) = ask else {
        track_signal(signo, targets, names, queue, cx);
        return;
    };
    let parent = window.window_handle();
    cx.defer(move |cx| {
        confirm(ask, parent, cx, move |result, cx| {
            if result == ConfirmResult::Confirmed {
                track_signal(signo, targets, names, queue, cx);
            }
        });
    });
}

static LAST_RT_SIGNAL: AtomicI32 = AtomicI32::new(0);

const SUBTITLE_H: f32 = 24.;
const ROW_H: f32 = 32.;
const STEPPER_W: f32 = 72.;
const FOOTER_H: f32 = 52.;

/// The real-time signal picker.
pub struct RtSignalDialog {
    targets: SmallVec<[ActionTarget; 1]>,
    names: TargetNames,
    value: i32,
    input: Entity<InputState>,
    cancel_focus: FocusHandle,
    send_focus: FocusHandle,
    parent: AnyWindowHandle,
    fitted: Rc<std::cell::Cell<Option<gpui_kit::Pixels>>>,
    _events: Subscription,
}

impl RtSignalDialog {
    fn new(
        targets: SmallVec<[ActionTarget; 1]>,
        names: TargetNames,
        parent: AnyWindowHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let value = LAST_RT_SIGNAL.load(Ordering::Relaxed);
        let value = if (SIGRTMIN..=SIGRTMAX).contains(&value) {
            value
        } else {
            SIGRTMIN
        };
        let input = cx.new(|cx| InputState::new(window, cx));
        let view = cx.entity().downgrade();
        let event_input = input.clone();
        let events = cx.subscribe_in(
            &input,
            window,
            move |_, _, event: &InputEvent, window, cx| {
                let typed = || {
                    event_input
                        .read(cx)
                        .text()
                        .to_string()
                        .trim()
                        .parse::<i32>()
                        .ok()
                };
                match event {
                    InputEvent::PressEnter { .. }
                        if typed().is_some_and(|v| (SIGRTMIN..=SIGRTMAX).contains(&v)) =>
                    {
                        let view = view.clone();
                        cx.defer_in(window, move |_, window, cx| {
                            let _ = view.update(cx, |this, cx| this.apply(window, cx));
                        });
                    }
                    InputEvent::Blur => {
                        if let Some(value) = typed() {
                            let value = value.clamp(SIGRTMIN, SIGRTMAX);
                            let _ = view.update(cx, |this, cx| this.set_value(value, cx));
                            event_input.update(cx, |input, cx| {
                                input.set_value(value.to_string(), window, cx);
                            });
                        }
                    }
                    InputEvent::PressEnter { .. } | InputEvent::Change | InputEvent::Focus => {}
                }
            },
        );
        let send_focus = cx.focus_handle();
        send_focus.focus(window, cx);
        Self {
            targets,
            names,
            value,
            input,
            cancel_focus: cx.focus_handle(),
            send_focus,
            parent,
            fitted: Rc::default(),
            _events: events,
        }
    }

    pub fn find(handle: WindowHandle<Root>, cx: &App) -> Option<Entity<Self>> {
        TlSecondaryWindow::<Self>::find(handle, cx).map(|frame| frame.read(cx).content().clone())
    }

    pub fn value(&self) -> i32 {
        self.value
    }

    pub fn set_value(&mut self, value: i32, cx: &mut Context<Self>) {
        self.value = value.clamp(SIGRTMIN, SIGRTMAX);
        cx.notify();
    }

    pub fn label(&self) -> SharedString {
        let mut label = format!("SIGRTMIN+{}", self.value - SIGRTMIN);
        if self.value == SIGRTMAX {
            label.push_str(" (SIGRTMAX)");
        }
        label.into()
    }

    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        LAST_RT_SIGNAL.store(self.value, Ordering::Relaxed);
        let targets = self.targets.clone();
        let names = self.names.clone();
        let signo = self.value;
        let parent = self.parent;
        cx.defer(move |cx| {
            if parent
                .update(cx, |_, window, cx| {
                    send_signal(signo, targets, names, window, cx)
                })
                .is_err()
            {
                warn!(target: "tasklens::ui", "signal not sent: parent window closed");
            }
        });
        window.remove_window();
    }

    fn on_send(&mut self, _: &SignalSendRealtime, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel_focus.is_focused(window) {
            cx.propagate();
        } else {
            self.apply(window, cx);
        }
    }
}

/// Opens the real-time signal dialog modal to `parent`.
pub fn open_rt_signal(
    targets: SmallVec<[ActionTarget; 1]>,
    names: TargetNames,
    parent: AnyWindowHandle,
    cx: &mut App,
) -> gpui_kit::Result<WindowHandle<Root>> {
    open_pwn(
        cx,
        PwnWindow::RealtimeSignal,
        tr(cx, "pwn-signal-title"),
        Some(parent),
        move |window, cx| cx.new(|cx| RtSignalDialog::new(targets, names, parent, window, cx)),
    )
}

impl Render for RtSignalDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let view = cx.entity().downgrade();

        let mut subtitle = div()
            .debug_selector(|| "SignalSubtitle".into())
            .flex_none()
            .flex()
            .items_center()
            .h(px(SUBTITLE_H * s))
            .text_color(theme.text_secondary);
        *subtitle.text_style() = t_sm(&m);

        let stepper = TlStepper::new("SignalStepper", &self.input)
            .range(i64::from(SIGRTMIN), i64::from(SIGRTMAX), 1)
            .value(i64::from(self.value))
            .on_change(move |value, _, cx| {
                let _ = view.update(cx, |this, cx| this.set_value(value as i32, cx));
            });
        let mut row_label = div().flex_none().text_color(theme.text_secondary);
        *row_label.text_style() = t_base(&m);
        let mut signal_name = div().text_color(theme.text_primary);
        *signal_name.text_style() = t_base(&m);
        let row = div()
            .debug_selector(|| "SignalRow".into())
            .flex_none()
            .flex()
            .items_center()
            .gap(m.sp[4])
            .h(px(ROW_H * s))
            .child(row_label.child(tr(cx, "pwn-signal-field")))
            .child(div().flex_none().w(px(STEPPER_W * s)).child(stepper))
            .child(signal_name.child(self.label()));

        let mut note = div().text_color(theme.text_secondary);
        *note.text_style() = t_sm(&m);
        let note = note.child(tr(cx, "pwn-signal-note"));

        let cancel = {
            let view = cx.entity().downgrade();
            TlButton::new("SignalCancel", TlButtonKind::Secondary)
                .label(tr(cx, "pwn-signal-cancel"))
                .focus_handle(self.cancel_focus.clone())
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |_, _| window.remove_window());
                })
        };
        let send = {
            let view = cx.entity().downgrade();
            TlButton::new("SignalSend", TlButtonKind::Primary)
                .label(tr(cx, "pwn-signal-send"))
                .focus_handle(self.send_focus.clone())
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.apply(window, cx));
                })
        };
        let footer = div()
            .debug_selector(|| "SignalFooter".into())
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
            .child(send);

        let fitted = self.fitted.clone();
        let fit = move |bounds: Bounds<gpui_kit::Pixels>, window: &mut Window, cx: &mut App| {
            let want = bounds.bottom();
            if fitted.get() != Some(want) {
                fitted.set(Some(want));
                window.defer(cx, move |window, _| {
                    window.resize(size(
                        window.bounds().size.width,
                        want + frame_bottom(window),
                    ));
                });
            }
        };

        div()
            .id("RtSignalDialog")
            .key_context(SIGNAL_DIALOG_CONTEXT)
            .on_action(cx.listener(Self::on_send))
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
                            .child(subtitle.child(self.names.subtitle()))
                            .child(row)
                            .child(note),
                    )
                    .child(footer),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_other_numbers_match_linux() {
        let numbers: Vec<(&str, i32)> = SIGNALS
            .iter()
            .filter(|s| !s.top_level)
            .map(|s| (s.name, s.number))
            .collect();
        assert_eq!(
            numbers,
            [
                ("ABRT", 6),
                ("ALRM", 14),
                ("BUS", 7),
                ("CHLD", 17),
                ("FPE", 8),
                ("ILL", 4),
                ("IO/POLL", 29),
                ("PIPE", 13),
                ("PROF", 27),
                ("PWR", 30),
                ("SEGV", 11),
                ("STKFLT", 16),
                ("SYS", 31),
                ("TRAP", 5),
                ("TSTP", 20),
                ("TTIN", 21),
                ("TTOU", 22),
                ("URG", 23),
                ("VTALRM", 26),
                ("WINCH", 28),
                ("XCPU", 24),
                ("XFSZ", 25),
            ]
        );
        for info in SIGNALS.iter().filter(|s| s.top_level) {
            assert_eq!(
                signal(info.number).expect("known signal").number(),
                info.number
            );
        }
    }
}
