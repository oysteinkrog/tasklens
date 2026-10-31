//! Process action toast texts and request submission (UI-PRC-015, PROC-058 to PROC-069, R-035,
//! ADR-005, UI-FND-020).
//!
//! The page builds an [`ActionToastTexts`] with [`texts_for`] when it submits an
//! `ActionRequest`, and [`submit`] or [`submit_to`] hands both to the host's `ActionTracker`.
//! Every state of one request replaces one toast, keyed `ToastKey(id.0)`. Two or more targets
//! get one summary toast
//! that counts the results and names the reasons, such as "Ended 4 of 5 processes (1 permission
//! denied)". Identity, permission and "Outcome unknown" texts are the tracker's own.
//!
//! Texts follow R-132: [`toast_words`] translates them once per strings version. The free
//! functions [`texts_for`] and [`refusal_text`] have no `App`, so they give the en-US text. The
//! texts hold names and PIDs only, never command lines.

use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, Context, Global, SharedString, WeakEntity};
use tl_model::actions::{ActionId, ActionKind, ActionRequest, ActionState, RejectReason, Signal};
use tl_model::process::ProcFlags;
use tracing::warn;

use super::ProcessesPage;
use super::actions::{CapturedTarget, CapturedTargets};
use crate::action_tracker::{ActionWords, SummaryTexts, ToastTexts, action_words};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::host::HostContext;
use crate::i18n::{Strings, tr, tr_en};
use crate::pages::performance::details::Template;
use crate::table::tree_table::RowKind;

/// (done, total, failed) to the kind and text of a mixed or failed multi-target result.
/// `failed` holds one (count, reason) per distinct reason, in the order of first occurrence,
/// such as (1, "permission denied").
pub type PartialText = Rc<dyn Fn(u32, u32, &[(u32, SharedString)]) -> (ToastKind, SharedString)>;

/// The toasts of one process action request (UI-PRC-015).
#[derive(Clone)]
pub struct ActionToastTexts {
    /// Shown on `Started`, such as (Info, "End task requested.").
    pub requested: Option<(ToastKind, SharedString)>,
    /// The final result when every target succeeded.
    pub success: (ToastKind, SharedString),
    /// One target failed; the tracker adds `message(err)` after it.
    pub failure: (ToastKind, SharedString),
    /// The result of two or more targets when any failed. `None` for kinds that act on one
    /// target at a time.
    pub partial: Option<PartialText>,
}

impl ActionToastTexts {
    /// The tracker's texts. `words` gives the reasons of the failed targets. Process actions
    /// summarise from 2 targets on (R-197).
    pub fn into_toast_texts(self, words: Rc<ActionWords>) -> ToastTexts {
        let summary = self.partial.map(|partial| {
            let (started_kind, started) = self
                .requested
                .clone()
                .unwrap_or((ToastKind::Progress, SharedString::default()));
            SummaryTexts {
                started,
                started_kind,
                all_ok: self.success.1.clone(),
                partial: Box::new(move |ok, total, failures| {
                    let grouped = group_reasons(failures.iter().map(|(_, s)| *s), &words);
                    partial(u32::from(ok), u32::from(total), &grouped)
                }),
            }
        });
        ToastTexts {
            started: self.requested,
            summary,
            summary_min_targets: 2,
            ..ToastTexts::new(self.success.1, self.failure.1)
        }
    }
}

/// One (count, reason) per distinct reason, in the order of first occurrence.
fn group_reasons<'a>(
    states: impl IntoIterator<Item = &'a ActionState>,
    words: &ActionWords,
) -> Vec<(u32, SharedString)> {
    let mut grouped: Vec<(u32, SharedString)> = Vec::new();
    for reason in states.into_iter().filter_map(|s| words.short_reason(s)) {
        match grouped.iter_mut().find(|(_, r)| *r == reason) {
            Some((count, _)) => *count += 1,
            None => grouped.push((1, reason)),
        }
    }
    grouped
}

/// The texts of `kind` on the targets `t`, in en-US. `None` for kinds this page does not
/// toast through here.
pub fn texts_for(kind: &ActionKind, t: &CapturedTargets) -> Option<ActionToastTexts> {
    EN_WORDS.with(|w| w.clone().texts_for(kind, t))
}

/// The toast text when `reason` refuses an action on `t` before any request is sent
/// (PROC-059), in en-US.
pub fn refusal_text(reason: RejectReason, t: &CapturedTarget) -> Option<SharedString> {
    EN_WORDS.with(|w| w.refusal_text(reason, t))
}

/// The (Warning, "Process exited") toast for an exited row that was activated.
pub fn exited_toast() -> (ToastKind, SharedString) {
    EN_WORDS.with(|w| (ToastKind::Warning, w.exited.clone()))
}

/// Why ending or signalling `t` is refused without a dialog (PROC-059): tasklens itself and
/// PID 1 are `Protected`, kernel threads are `KernelThread`. Other protected processes get a
/// confirmation instead and return `None`.
pub fn refusal_reason(t: &CapturedTarget) -> Option<RejectReason> {
    if t.flags.contains(ProcFlags::IS_SELF) || t.pid == Some(1) {
        Some(RejectReason::Protected)
    } else if t.flags.contains(ProcFlags::KERNEL_THREAD) || t.kind == RowKind::KernelThread {
        Some(RejectReason::KernelThread)
    } else {
        None
    }
}

/// Shows the refusal toast of the first refused target of `t` on the main window and returns
/// true; the caller then sends no request. Returns false when no target is refused.
pub fn refuse(t: &CapturedTargets, cx: &mut App) -> bool {
    let words = toast_words(cx);
    let Some(text) = t
        .items
        .iter()
        .find_map(|target| words.refusal_text(refusal_reason(target)?, target))
    else {
        return false;
    };
    match main_queue(cx).and_then(|q| q.upgrade()) {
        Some(queue) => queue.update(cx, |q, cx| q.push(ToastKind::Warning, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no main window for the refusal toast"),
    }
    true
}

/// Sends `req` through the page host's `ActionTracker` with `texts`, so every state of the
/// request replaces one toast keyed `ToastKey(req.id.0)` on the main window. `None` when the
/// request was not sent; a busy engine shows its own Warning.
pub fn submit(
    page: &mut ProcessesPage,
    req: ActionRequest,
    texts: ActionToastTexts,
    cx: &mut Context<ProcessesPage>,
) -> Option<ActionId> {
    let queue = main_queue(cx);
    submit_to(&page.host, req, texts, queue, cx)
}

/// As [`submit`], through `host`'s tracker with the toasts on `queue`. `None` when there is no
/// tracker or no queue.
pub fn submit_to(
    host: &HostContext,
    req: ActionRequest,
    texts: ActionToastTexts,
    queue: Option<WeakEntity<ToastQueue>>,
    cx: &mut App,
) -> Option<ActionId> {
    submit_to_then(host, req, texts, queue, None, cx)
}

/// As [`submit_to`]. `on_sent` gets the id once the request is sent, which on a remote host is
/// after the root confirmation, so the result here is `None` at once.
pub fn submit_to_then(
    host: &HostContext,
    req: ActionRequest,
    texts: ActionToastTexts,
    queue: Option<WeakEntity<ToastQueue>>,
    on_sent: Option<crate::action_tracker::OnSentId>,
    cx: &mut App,
) -> Option<ActionId> {
    let Some(tracker) = host.tracker().cloned() else {
        warn!(target: "tasklens::ui", "no action tracker; the action was not sent");
        return None;
    };
    let Some(queue) = queue else {
        warn!(target: "tasklens::ui", "no toast stack; the action was not sent");
        return None;
    };
    let mut texts = texts.into_toast_texts(action_words(cx));
    // PROC-058, ADR-006: a permission-denied result offers "Retry as administrator".
    texts.retry_as_admin = true;
    tracker.update(cx, |t, cx| {
        t.track_or_toast_then(req, queue, texts, on_sent, cx)
    })
}

/// Switches to the window `internal_id` on the host: an `ActivateWindow` request whose result
/// toast replaces the old fire-and-forget `KWin(Activate)` (ADR-018, PROC-054). A remote host's
/// toast says "on <host>" (the tracker adds it).
pub fn activate_window(
    host: &HostContext,
    internal_id: Arc<str>,
    queue: Option<WeakEntity<ToastQueue>>,
    cx: &mut App,
) -> Option<ActionId> {
    let req = ActionRequest {
        id: ActionId(0),
        kind: ActionKind::ActivateWindow { internal_id },
        targets: smallvec::SmallVec::new(),
        privileged: false,
    };
    let texts = ActionToastTexts {
        requested: None,
        success: (ToastKind::Success, tr(cx, "processes-toast-window-shown")),
        failure: (
            ToastKind::Error,
            tr(cx, "processes-toast-window-show-failed"),
        ),
        partial: None,
    };
    submit_to(host, req, texts, queue, cx)
}

/// [`activate_window`] with the toasts on the main window.
pub fn activate_window_on_main(
    host: &HostContext,
    internal_id: Arc<str>,
    cx: &mut App,
) -> Option<ActionId> {
    activate_window(host, internal_id, main_queue(cx), cx)
}

fn main_queue(cx: &App) -> Option<WeakEntity<ToastQueue>> {
    let main = crate::window::main::main_window_view(cx)?;
    Some(main.read(cx).toasts().downgrade())
}

/// The texts of a setting that applies to each target: priority, affinity, I/O priority.
struct SettingWords {
    updated: SharedString,
    many_updated: SharedString,
    failed: SharedString,
    /// done, total, failed, reasons.
    partial: Template,
}

impl SettingWords {
    fn with(
        t: &impl Fn(&str) -> SharedString,
        tmpl: &impl Fn(&str, &[&str]) -> Template,
        name: &str,
    ) -> Self {
        Self {
            updated: t(&format!("processes-toast-{name}-updated")),
            many_updated: t(&format!("processes-toast-{name}-many-updated")),
            failed: t(&format!("processes-toast-{name}-failed")),
            partial: tmpl(
                &format!("processes-toast-{name}-partial"),
                &["done", "total", "failed", "reasons"],
            ),
        }
    }
}

/// The page's toast texts, translated once per strings version (R-132).
pub struct ProcessToastWords {
    end_task_requested: SharedString,
    end_task_failed: SharedString,
    end_process_requested: SharedString,
    end_process_failed: SharedString,
    terminated: SharedString,
    end_tree_requested: SharedString,
    tree_terminated: SharedString,
    end_tree_failed: SharedString,
    end_many_requested: SharedString,
    ended: Template,
    ended_partial: Template,
    reason_count: Template,
    suspended: SharedString,
    suspend_failed: SharedString,
    resumed: SharedString,
    resume_failed: SharedString,
    app_frozen: SharedString,
    freeze_failed: SharedString,
    app_thawed: SharedString,
    thaw_failed: SharedString,
    app_killed: SharedString,
    kill_app_failed: SharedString,
    priority: SettingWords,
    affinity: SettingWords,
    io_priority: SettingWords,
    refuse_self: SharedString,
    refuse_pid1: SharedString,
    refuse_kernel: SharedString,
    pub exited: SharedString,
}

impl ProcessToastWords {
    fn with(t: impl Fn(&str) -> SharedString, tmpl: impl Fn(&str, &[&str]) -> Template) -> Self {
        Self {
            end_task_requested: t("processes-toast-end-task-requested"),
            end_task_failed: t("processes-toast-end-task-failed"),
            end_process_requested: t("processes-toast-end-process-requested"),
            end_process_failed: t("processes-toast-end-process-failed"),
            terminated: t("processes-toast-terminated"),
            end_tree_requested: t("processes-toast-end-tree-requested"),
            tree_terminated: t("processes-toast-tree-terminated"),
            end_tree_failed: t("processes-toast-end-tree-failed"),
            end_many_requested: t("processes-toast-end-many-requested"),
            ended: tmpl("processes-toast-ended", &["done", "total"]),
            ended_partial: tmpl(
                "processes-toast-ended-partial",
                &["done", "total", "reasons"],
            ),
            reason_count: tmpl("processes-toast-reason-count", &["count", "reason"]),
            suspended: t("processes-toast-suspended"),
            suspend_failed: t("processes-toast-suspend-failed"),
            resumed: t("processes-toast-resumed"),
            resume_failed: t("processes-toast-resume-failed"),
            app_frozen: t("processes-toast-app-frozen"),
            freeze_failed: t("processes-toast-freeze-failed"),
            app_thawed: t("processes-toast-app-thawed"),
            thaw_failed: t("processes-toast-thaw-failed"),
            app_killed: t("processes-toast-app-killed"),
            kill_app_failed: t("processes-toast-kill-app-failed"),
            priority: SettingWords::with(&t, &tmpl, "priority"),
            affinity: SettingWords::with(&t, &tmpl, "affinity"),
            io_priority: SettingWords::with(&t, &tmpl, "io-priority"),
            refuse_self: t("processes-toast-refuse-self"),
            refuse_pid1: t("processes-toast-refuse-pid1"),
            refuse_kernel: t("fnd-action-reject-kernel-thread"),
            exited: t("processes-toast-exited"),
        }
    }

    fn en() -> Self {
        Self::with(|key| tr_en(key, &[]), Template::en)
    }

    fn translated(cx: &App) -> Self {
        Self::with(|key| tr(cx, key), |key, args| Template::new(cx, key, args))
    }

    /// See [`texts_for`].
    pub fn texts_for(
        self: Rc<Self>,
        kind: &ActionKind,
        t: &CapturedTargets,
    ) -> Option<ActionToastTexts> {
        let multi = t.is_multi();
        let info = |text: &SharedString| Some((ToastKind::Info, text.clone()));
        let ok = |text: &SharedString| (ToastKind::Success, text.clone());
        let err = |text: &SharedString| (ToastKind::Error, text.clone());
        let texts = match kind {
            ActionKind::Signal(signal @ (Signal::Term | Signal::Kill)) => {
                let failed = if *signal == Signal::Term {
                    &self.end_task_failed
                } else {
                    &self.end_process_failed
                };
                if multi {
                    // App and unit rows end every member process.
                    let total: u32 = t.items.iter().map(|i| i.member_count.max(1)).sum();
                    ActionToastTexts {
                        requested: info(&self.end_many_requested),
                        success: (ToastKind::Success, fill(&self.ended, &[&total, &total])),
                        failure: err(failed),
                        partial: Some(self.ended_partial()),
                    }
                } else {
                    let requested = if *signal == Signal::Term {
                        &self.end_task_requested
                    } else {
                        &self.end_process_requested
                    };
                    ActionToastTexts {
                        requested: info(requested),
                        success: ok(&self.terminated),
                        failure: err(failed),
                        partial: None,
                    }
                }
            }
            ActionKind::KillTree => ActionToastTexts {
                requested: info(&self.end_tree_requested),
                success: ok(&self.tree_terminated),
                failure: err(&self.end_tree_failed),
                partial: Some(self.ended_partial()),
            },
            ActionKind::Suspend => self.simple(&self.suspended, &self.suspend_failed),
            ActionKind::Resume => self.simple(&self.resumed, &self.resume_failed),
            ActionKind::FreezeGroup => self.simple(&self.app_frozen, &self.freeze_failed),
            ActionKind::ThawGroup => self.simple(&self.app_thawed, &self.thaw_failed),
            ActionKind::KillGroup => self.simple(&self.app_killed, &self.kill_app_failed),
            ActionKind::SetNice(_) | ActionKind::SetSched { .. } => {
                Self::setting(&self, multi, |w| &w.priority)
            }
            ActionKind::SetAffinity(_) => Self::setting(&self, multi, |w| &w.affinity),
            ActionKind::SetIoPriority(_) => Self::setting(&self, multi, |w| &w.io_priority),
            _ => return None,
        };
        Some(texts)
    }

    /// See [`refusal_text`].
    pub fn refusal_text(&self, reason: RejectReason, t: &CapturedTarget) -> Option<SharedString> {
        match reason {
            RejectReason::KernelThread => Some(self.refuse_kernel.clone()),
            RejectReason::Protected if t.flags.contains(ProcFlags::IS_SELF) => {
                Some(self.refuse_self.clone())
            }
            RejectReason::Protected if t.pid == Some(1) => Some(self.refuse_pid1.clone()),
            _ => None,
        }
    }

    fn simple(&self, success: &SharedString, failure: &SharedString) -> ActionToastTexts {
        ActionToastTexts {
            requested: None,
            success: (ToastKind::Success, success.clone()),
            failure: (ToastKind::Error, failure.clone()),
            partial: None,
        }
    }

    /// "Ended 4 of 5 processes (1 permission denied)", Warning.
    fn ended_partial(self: &Rc<Self>) -> PartialText {
        let words = self.clone();
        Rc::new(move |done, total, failed| {
            let reasons = failed
                .iter()
                .map(|(count, reason)| fill(&words.reason_count, &[count, reason]).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let text = fill(&words.ended_partial, &[&done, &total, &reasons]);
            (ToastKind::Warning, text)
        })
    }

    /// One target: "Priority updated". Several: "Selected process priority updated.", the
    /// failure text as Error when every target failed, else "Priority updated for 3 of 4
    /// processes. 1 failed: permission denied" as Warning.
    fn setting(self: &Rc<Self>, multi: bool, pick: fn(&Self) -> &SettingWords) -> ActionToastTexts {
        let w = pick(self);
        let failure = (ToastKind::Error, w.failed.clone());
        if !multi {
            return ActionToastTexts {
                requested: None,
                success: (ToastKind::Success, w.updated.clone()),
                failure,
                partial: None,
            };
        }
        let words = self.clone();
        let partial: PartialText = Rc::new(move |done, total, failed| {
            let w = pick(&words);
            if done == 0 {
                return (ToastKind::Error, w.failed.clone());
            }
            let count = total - done;
            let reasons = failed
                .iter()
                .map(|(_, reason)| reason.as_ref())
                .collect::<Vec<_>>()
                .join(", ");
            let text = fill(&w.partial, &[&done, &total, &count, &reasons]);
            (ToastKind::Warning, text)
        });
        ActionToastTexts {
            requested: None,
            success: (ToastKind::Success, w.many_updated.clone()),
            failure,
            partial: Some(partial),
        }
    }
}

fn fill(template: &Template, values: &[&dyn fmt::Display]) -> SharedString {
    let mut out = String::new();
    let _ = template.write(&mut out, values);
    out.into()
}

thread_local! {
    static EN_WORDS: Rc<ProcessToastWords> = Rc::new(ProcessToastWords::en());
}

struct ProcessToastWordsCache {
    version: u64,
    words: Rc<ProcessToastWords>,
}

impl Global for ProcessToastWordsCache {}

/// The page's toast texts for the current strings. Translated only when the strings version
/// changed; without strings they are in en-US.
pub fn toast_words(cx: &mut App) -> Rc<ProcessToastWords> {
    let Some(version) = cx.try_global::<Strings>().map(|s| s.strings_version) else {
        return EN_WORDS.with(Rc::clone);
    };
    if let Some(cache) = cx.try_global::<ProcessToastWordsCache>()
        && cache.version == version
    {
        return cache.words.clone();
    }
    let words = Rc::new(ProcessToastWords::translated(cx));
    cx.set_global(ProcessToastWordsCache {
        version,
        words: words.clone(),
    });
    words
}
