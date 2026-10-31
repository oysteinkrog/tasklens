//! The one entry point for process actions (tasklens-pdu.58, E22-017, UI-PRC-013 to UI-PRC-015,
//! UI-SYS-016, R-036, D-009).
//!
//! [`run_process_action`] runs a process row command on captured targets: End Task, End
//! Process, End Process Tree, Kill, Freeze and Thaw App, Suspend and Resume, Set Priority, Set
//! I/O Priority, Custom priority, Set Affinity, Send Signal, Real-time signal, Go to parent,
//! Open file location, File properties, Show Window, Open in VirusTotal, Create dump file,
//! Restart unit, Restart Plasma Shell and the three Add graph items.
//! The Processes tab, the Services tab's child process rows and the "Disk is in use" window all
//! call it, so each surface gets the same refusals, confirmations and toasts.
//!
//! Toasts go to the main window, else to the toast stack of the window the command ran in.
//! Confirmations open modal to that window.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    Action, AnyWindowHandle, App, Global, SharedString, Subscription, WeakEntity, Window,
};
use rustc_hash::FxHashSet;
use smallvec::SmallVec;
use tl_model::actions::{
    ActionId, ActionKind, ActionRequest, ActionTarget, RejectReason, SchedPolicy,
};
use tl_model::command::Tab;
use tl_model::ids::{ColumnId, NodeId, ProcessKey};
use tl_model::process::ProcFlags;
use tl_model::snapshot::Snapshot;
use tl_model::table::UnitRef;
use tracing::warn;

use super::actions::{
    CapturedTarget, CapturedTargets, CreateDump, EndProcess, EndProcessTree, EndTask,
    FileProperties, FreezeApp, GoToParent, KillApp, MAX_TARGETS, OpenFileLocation, OpenVirusTotal,
    PriorityClass, RestartUnit, ResumeProcess, SetIoPriorityClass, SetPriority, ShowWindow,
    SuspendProcess, ThawApp,
};
use super::confirm::{ConfirmKind, ConfirmResult, ProtectedAction, confirm};
use super::jump::JumpTexts;
use super::menu::{GraphKind, graph_source};
use super::toasts::{ActionToastTexts, activate_window, refusal_reason, submit_to, toast_words};
use super::{
    ConfirmWords, End, EndPlan, GroupOp, GroupPlan, ProcessesPage, dialog_targets, exe_deleted,
    group_plan, is_plasma_unit, members, other_user, process_keys, record_target, signal_targets,
    topmost_window,
};
use crate::action_tracker::{ActionTrackerEvent, reconnecting_refusal};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::host::HostContext;
use crate::i18n::{Strings, tr};
use crate::pages::minigraph::actions::{AddAppGraph, AddMinigraph, AddNameGraph};
use crate::pages::minigraph::panel::MinigraphPanel;
use crate::pages::minigraph::sources::GraphColumn;
use crate::table::tree_table::RowKind;
use crate::window::main::main_window_view;
use crate::windows::actions::{OpenCustomPriority, SendSignal, SetAffinity, SignalOpenRealtime};
use crate::windows::signal::{open_rt_signal, send_signal};

/// Called with the id of each request the action tracker accepted. It runs after the current
/// update, so it may update the entity that ran the command.
pub type OnSent = Rc<dyn Fn(ActionId, &mut App)>;

/// How one surface runs a process action.
#[derive(Clone, Default)]
pub struct RunOptions {
    /// The snapshot the targets were captured from, for app and unit members and executable
    /// paths. `None` is the hub's current snapshot.
    pub snapshot: Option<Arc<Snapshot>>,
    pub on_sent: Option<OnSent>,
}

/// Runs the process command `action` on `targets` of `host`, with the hub's current snapshot.
/// Returns false when `action` is not a process command; nothing happens then.
pub fn run_process_action(
    host: &HostContext,
    action: &dyn Action,
    targets: CapturedTargets,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    run_process_action_with(host, action, targets, RunOptions::default(), window, cx)
}

/// [`run_process_action`] with the surface's snapshot and a callback for the sent request ids.
pub fn run_process_action_with(
    host: &HostContext,
    action: &dyn Action,
    targets: CapturedTargets,
    options: RunOptions,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let a = action.as_any();
    let handled = a.is::<EndTask>()
        || a.is::<EndProcess>()
        || a.is::<EndProcessTree>()
        || a.is::<KillApp>()
        || a.is::<FreezeApp>()
        || a.is::<ThawApp>()
        || a.is::<SuspendProcess>()
        || a.is::<ResumeProcess>()
        || a.is::<SetPriority>()
        || a.is::<SetIoPriorityClass>()
        || a.is::<OpenCustomPriority>()
        || a.is::<SetAffinity>()
        || a.is::<SendSignal>()
        || a.is::<SignalOpenRealtime>()
        || a.is::<GoToParent>()
        || a.is::<OpenFileLocation>()
        || a.is::<FileProperties>()
        || a.is::<ShowWindow>()
        || a.is::<OpenVirusTotal>()
        || a.is::<CreateDump>()
        || a.is::<RestartUnit>()
        || a.is::<AddMinigraph>()
        || a.is::<AddNameGraph>()
        || a.is::<AddAppGraph>();
    if !handled {
        return false;
    }
    let snapshot = options
        .snapshot
        .or_else(|| host.hub().map(|hub| Arc::clone(hub.read(cx).current())));
    let Some(snapshot) = snapshot else {
        warn!(target: "tasklens::ui", "no snapshot; the process action did not run");
        return true;
    };
    let run = Run {
        host: host.clone(),
        snapshot,
        queue: toast_queue(window, cx),
        on_sent: options.on_sent,
        parent: window.window_handle(),
    };
    if a.is::<EndTask>() {
        run.end(End::Task, targets, cx);
    } else if a.is::<EndProcess>() {
        run.end_process(targets, cx);
    } else if a.is::<EndProcessTree>() {
        run.end(End::Tree, targets, cx);
    } else if a.is::<KillApp>() {
        run.group_action(GroupOp::Kill, targets, cx);
    } else if a.is::<FreezeApp>() {
        run.group_action(GroupOp::Freeze, targets, cx);
    } else if a.is::<ThawApp>() {
        run.group_action(GroupOp::Thaw, targets, cx);
    } else if a.is::<SuspendProcess>() {
        run.suspend_or_resume(false, targets, cx);
    } else if a.is::<ResumeProcess>() {
        run.suspend_or_resume(true, targets, cx);
    } else if let Some(p) = a.downcast_ref::<SetPriority>() {
        run.set_priority(p.class, targets, cx);
    } else if let Some(p) = a.downcast_ref::<SetIoPriorityClass>() {
        run.set_io_priority(p, targets, cx);
    } else if a.is::<OpenCustomPriority>() {
        run.open_custom_priority(targets, cx);
    } else if a.is::<SetAffinity>() {
        run.set_affinity(targets, cx);
    } else if let Some(s) = a.downcast_ref::<SendSignal>() {
        run.send_signal(s.signo, targets, window, cx);
    } else if a.is::<SignalOpenRealtime>() {
        run.open_rt_signal(targets, cx);
    } else if a.is::<GoToParent>() {
        run.go_to_parent(targets, window, cx);
    } else if a.is::<OpenFileLocation>() {
        run.show_executable(false, targets, cx);
    } else if a.is::<FileProperties>() {
        run.show_executable(true, targets, cx);
    } else if a.is::<ShowWindow>() {
        run.show_window(targets, cx);
    } else if a.is::<OpenVirusTotal>() {
        if let Some(key) = one_process(&targets) {
            crate::windows::notices::open_virustotal(&run.host, key, window, cx);
        }
    } else if a.is::<CreateDump>() {
        if let Some(key) = one_process(&targets) {
            crate::windows::notices::start_dump(&run.host, key, window, cx);
        }
    } else if let Some(r) = a.downcast_ref::<RestartUnit>() {
        run.restart_unit(r.unit.clone(), cx);
    } else if let Some(g) = a.downcast_ref::<AddMinigraph>() {
        run.add_graph(GraphKind::Minigraph, g.column, &targets, cx);
    } else if let Some(g) = a.downcast_ref::<AddNameGraph>() {
        run.add_graph(GraphKind::Name, g.column, &targets, cx);
    } else if let Some(g) = a.downcast_ref::<AddAppGraph>() {
        run.add_graph(GraphKind::App, g.column, &targets, cx);
    }
    true
}

/// The main window's toast stack, else the stack of `window`.
fn toast_queue(window: &Window, cx: &App) -> Option<WeakEntity<ToastQueue>> {
    if let Some(main) = main_window_view(cx) {
        return Some(main.read(cx).toasts().downgrade());
    }
    ToastQueue::of_window(window, cx).map(|q| q.downgrade())
}

/// The key of the one captured process row.
fn one_process(targets: &CapturedTargets) -> Option<ProcessKey> {
    match targets.items.as_slice() {
        [t] => match t.node {
            NodeId::Process(key) => Some(key),
            _ => None,
        },
        _ => None,
    }
}

/// One command run: where its toasts go and what its confirmation opens over.
struct Run {
    /// The host the command acts on. Set when it runs; never changes.
    host: HostContext,
    snapshot: Arc<Snapshot>,
    queue: Option<WeakEntity<ToastQueue>>,
    on_sent: Option<OnSent>,
    parent: AnyWindowHandle,
}

impl Run {
    fn toast(&self, kind: ToastKind, text: SharedString, cx: &mut App) {
        match self.queue.as_ref().and_then(WeakEntity::upgrade) {
            Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
            None => warn!(target: "tasklens::ui", "no toast stack for the process action toast"),
        }
    }

    /// Restart Plasma Shell and Restart unit (PROC-072): `UnitRestart` on the captured
    /// `UnitRef`. One Plasma Shell restart runs at a time, whichever surface started it.
    fn restart_unit(&self, unit: UnitRef, cx: &mut App) {
        let plasma = is_plasma_unit(&unit);
        if plasma && plasma_restart_running(cx) {
            let text = tr(cx, "processes-toast-plasma-in-progress");
            self.toast(ToastKind::Warning, text, cx);
            return;
        }
        let req = ActionRequest {
            id: ActionId(0),
            kind: ActionKind::UnitRestart,
            targets: SmallVec::from_buf([ActionTarget::Unit(unit)]),
            privileged: false,
        };
        let (requested, success) = if plasma {
            (
                (ToastKind::Info, tr(cx, "processes-toast-plasma-restarting")),
                tr(cx, "processes-toast-plasma-restarted"),
            )
        } else {
            (
                (
                    ToastKind::Progress,
                    tr(cx, "services-sys018-restarting-service"),
                ),
                tr(cx, "services-sys018-service-restarted"),
            )
        };
        let texts = ActionToastTexts {
            requested: Some(requested),
            success: (ToastKind::Success, success),
            failure: (
                ToastKind::Error,
                tr(cx, "services-sys018-failed-to-restart-service"),
            ),
            partial: None,
        };
        let Some(id) = submit_to(&self.host, req, texts, self.queue.clone(), cx) else {
            return;
        };
        if plasma {
            plasma_restart_started(&self.host, id, cx);
        }
        if let Some(on_sent) = self.on_sent.clone() {
            cx.defer(move |cx| on_sent(id, cx));
        }
    }

    /// Items 15 to 17: resolves the one captured row to a card source and adds the card. The
    /// panel shows the toast. An identity that is not in the snapshot gives `Unable to add
    /// minigraph`.
    fn add_graph(
        &self,
        kind: GraphKind,
        column: GraphColumn,
        targets: &CapturedTargets,
        cx: &mut App,
    ) {
        let [target] = targets.items.as_slice() else {
            return;
        };
        let Some(panel) = MinigraphPanel::global(cx) else {
            warn!(target: "tasklens::ui", "no minigraph panel for an Add graph action");
            return;
        };
        let source = graph_source(kind, target, &self.snapshot);
        let _ = panel.update(cx, |p, cx| p.add_process(source, column, cx));
    }

    /// End Process, and Shift+Delete. On one app or unit row it is Kill App (UI-PRC-013 item 2).
    fn end_process(&self, targets: CapturedTargets, cx: &mut App) {
        let group = matches!(
            targets.items.as_slice(),
            [t] if matches!(t.kind, RowKind::App | RowKind::Unit)
        );
        if group {
            self.group_action(GroupOp::Kill, targets, cx);
        } else {
            self.end(End::Process, targets, cx);
        }
    }

    /// End Task, End Process and End Process Tree (UI-PRC-013 items 1 to 3): drops pseudo and
    /// exited targets, refuses tasklens, PID 1 and kernel threads with a toast (End Process Tree
    /// also a subtree that holds tasklens or PID 1), asks for confirmation where UI-PRC-014
    /// says so (R-198, D-009) and sends one request by key.
    fn end(&self, end: End, targets: CapturedTargets, cx: &mut App) {
        let Some((rows, clicked_col)) = self.prune(targets, refusal_reason, cx) else {
            return;
        };
        let words = confirm_words(cx);
        let Some(plan) = EndPlan::new(end, rows, &self.snapshot.processes, &words) else {
            return;
        };
        if let Some(t) = &plan.refused {
            if let Some(text) = toast_words(cx).refusal_text(RejectReason::Protected, t) {
                self.toast(ToastKind::Warning, text, cx);
            }
            return;
        }
        let used = CapturedTargets::new(plan.rows, clicked_col);
        self.send(end.kind(), plan.keys, used, plan.confirm, cx);
    }

    /// Kill App, Freeze App and Thaw App (UI-PRC-013 items 2 and 4, PROC-059, PROC-064): one
    /// `KillGroup`, `FreezeGroup` or `ThawGroup` request on the one app or unit row, with the
    /// targets of [`group_targets`]. Kill App always asks. Freeze asks first when the row
    /// holds a protected process. A row with no target sends nothing (the menu disables it).
    fn group_action(&self, op: GroupOp, targets: CapturedTargets, cx: &mut App) {
        let Some((rows, clicked_col)) = self.prune(targets, refusal_reason, cx) else {
            return;
        };
        let [t] = rows.as_slice() else {
            return;
        };
        if !matches!(t.kind, RowKind::App | RowKind::Unit) {
            return;
        }
        let GroupPlan {
            targets,
            skipped,
            total,
        } = group_plan(&self.snapshot, t);
        if targets.is_empty() {
            return;
        }
        let ask = match op {
            GroupOp::Kill => Some(ConfirmKind::KillApp {
                app: t.name.clone(),
                count: t.member_count,
                skipped,
                groups: total,
            }),
            GroupOp::Freeze if t.flags.contains(ProcFlags::PROTECTED) => {
                Some(ConfirmKind::Protected {
                    action: ProtectedAction::Freeze,
                    name: t.name.clone(),
                    tree_count: None,
                })
            }
            GroupOp::Freeze | GroupOp::Thaw => None,
        };
        let kind = match op {
            GroupOp::Kill => ActionKind::KillGroup,
            GroupOp::Freeze => ActionKind::FreezeGroup,
            GroupOp::Thaw => ActionKind::ThawGroup,
        };
        let used = CapturedTargets::new([t.clone()], clicked_col);
        self.send_targets(kind, targets, used, ask, cx);
    }

    /// Suspend Process and Resume Process (UI-PRC-013 item 4, PROC-063): one `Suspend` or
    /// `Resume` request with one target per process row. Suspend refuses tasklens, PID 1 and
    /// kernel threads, Resume only kernel threads. Suspend asks first for a protected target,
    /// both ask for another user's process; own processes never ask, even several. App and
    /// unit rows are skipped (Freeze App is E22-035).
    fn suspend_or_resume(&self, resume: bool, targets: CapturedTargets, cx: &mut App) {
        let refuse = |t: &CapturedTarget| {
            refusal_reason(t).filter(|r| !resume || *r == RejectReason::KernelThread)
        };
        let Some((rows, clicked_col)) = self.prune(targets, refuse, cx) else {
            return;
        };
        let mut seen = FxHashSet::default();
        let rows: Vec<CapturedTarget> = rows
            .into_iter()
            .filter(|t| {
                t.kind == RowKind::Process
                    && matches!(t.node, NodeId::Process(key) if seen.insert(key))
            })
            .collect();
        let keys = process_keys(&rows);
        if keys.is_empty() {
            return;
        }
        let w = confirm_words(cx);
        let protected = rows
            .iter()
            .find(|t| !resume && t.flags.contains(ProcFlags::PROTECTED));
        let ask = match protected {
            Some(t) => Some(ConfirmKind::Protected {
                action: ProtectedAction::Suspend,
                name: t.name.clone(),
                tree_count: None,
            }),
            None => other_user(&rows, if resume { &w.resume } else { &w.suspend }),
        };
        let kind = if resume {
            ActionKind::Resume
        } else {
            ActionKind::Suspend
        };
        let used = CapturedTargets::new(rows, clicked_col);
        self.send(kind, keys, used, ask, cx);
    }

    /// Set Priority (UI-PRC-013 item 6, PROC-066, PROC-069): High to Low send one `SetNice`
    /// request; Idle sends `SetSched { Idle, 0 }`. Realtime always asks first, because it can
    /// freeze the desktop, then sends `SetSched { Rr, 1 }`; the kernel sets reset on fork. App
    /// and unit rows apply to every current member process. Otherwise only another user's
    /// process asks first.
    fn set_priority(&self, class: PriorityClass, targets: CapturedTargets, cx: &mut App) {
        let Some((used, clicked_col)) = self.setting_targets(targets, cx) else {
            return;
        };
        let keys = process_keys(&used);
        let other = other_user(&used, &confirm_words(cx).set_priority);
        let (kind, ask) = match class {
            PriorityClass::Realtime => (
                ActionKind::SetSched {
                    policy: SchedPolicy::Rr,
                    rt_prio: 1,
                },
                Some(ConfirmKind::Realtime),
            ),
            PriorityClass::Idle => (
                ActionKind::SetSched {
                    policy: SchedPolicy::Idle,
                    rt_prio: 0,
                },
                other,
            ),
            class => {
                let Some(nice) = class.nice() else {
                    return;
                };
                (ActionKind::SetNice(nice), other)
            }
        };
        let used = CapturedTargets::new(used, clicked_col);
        self.send(kind, keys, used, ask, cx);
    }

    /// Set I/O Priority (UI-PRC-013 item 8, PROC-068, PROC-069): one `SetIoPriority` request,
    /// with the same targets and confirmation rule as Set Priority.
    fn set_io_priority(&self, a: &SetIoPriorityClass, targets: CapturedTargets, cx: &mut App) {
        let Some((used, clicked_col)) = self.setting_targets(targets, cx) else {
            return;
        };
        let keys = process_keys(&used);
        let ask = other_user(&used, &confirm_words(cx).set_io_priority);
        let used = CapturedTargets::new(used, clicked_col);
        let kind = ActionKind::SetIoPriority(a.class.to_model());
        self.send(kind, keys, used, ask, cx);
    }

    /// The process targets of Set Priority and Set I/O Priority: each process row once, and
    /// every current member process of an app or unit row, at most [`MAX_TARGETS`]. `None`
    /// when there is no process to act on.
    fn setting_targets(
        &self,
        targets: CapturedTargets,
        cx: &mut App,
    ) -> Option<(Vec<CapturedTarget>, Option<ColumnId>)> {
        let (rows, clicked_col) = self.prune(targets, |_| None, cx)?;
        let p = &self.snapshot.processes;
        let mut seen = FxHashSet::default();
        let mut used: Vec<CapturedTarget> = Vec::new();
        for t in rows {
            match (t.kind, &t.node) {
                (RowKind::Process | RowKind::KernelThread, NodeId::Process(key)) => {
                    if seen.insert(*key) {
                        used.push(t);
                    }
                }
                (RowKind::App | RowKind::Unit, node) => used.extend(
                    members(p, node)
                        .into_iter()
                        .map(|i| &p.records[i])
                        .filter(|r| seen.insert(r.key))
                        .map(record_target),
                ),
                _ => {}
            }
        }
        used.truncate(MAX_TARGETS);
        (!used.is_empty()).then_some((used, clicked_col))
    }

    /// Set Affinity… (UI-PRC-021): opens the dialog for the captured rows. An app or unit row
    /// gives its current member processes, at most [`MAX_TARGETS`].
    fn set_affinity(&self, targets: CapturedTargets, cx: &mut App) {
        use crate::windows::affinity::subtitle;
        let Some((rows, _)) = self.prune(targets, |_| None, cx) else {
            return;
        };
        let Some((keys, subject)) = dialog_targets(&rows, &self.snapshot.processes) else {
            return;
        };
        let subtitle = subtitle(&subject, cx);
        let targets = keys.into_iter().map(ActionTarget::Process).collect();
        if let Err(err) =
            crate::windows::affinity::open(&self.host, targets, subtitle, self.parent, cx)
        {
            warn!(target: "tasklens::ui", %err, "the Set Affinity dialog did not open");
        }
    }

    /// Set Priority > Custom… (UI-PRC-020, UI-PWN-023): opens the Custom priority dialog for the
    /// captured rows. It starts from the policy, nice and realtime priority of the first
    /// captured process, the values its Priority cell shows.
    fn open_custom_priority(&self, targets: CapturedTargets, cx: &mut App) {
        use crate::windows::affinity::{AffinitySubject, subtitle};
        let Some((rows, _)) = self.prune(targets, |_| None, cx) else {
            return;
        };
        let p = &self.snapshot.processes;
        let Some((keys, subject)) = dialog_targets(&rows, p) else {
            return;
        };
        let initial = p.records.iter().position(|r| r.key == keys[0]).map_or(
            (SchedPolicy::Other, 0, 0),
            |i| {
                let nice = p.records[i].nice;
                match p.extras.get(i) {
                    Some(x) => (x.policy, nice, x.rt_prio),
                    None => (SchedPolicy::Other, nice, 0),
                }
            },
        );
        let app = matches!(subject, AffinitySubject::App { .. });
        let subtitle = subtitle(&subject, cx);
        let targets = keys.into_iter().map(ActionTarget::Process).collect();
        if let Err(err) = crate::windows::priority::open_with(
            &self.host,
            targets,
            subtitle,
            initial,
            app,
            self.parent,
            cx,
        ) {
            warn!(target: "tasklens::ui", %err, "the Custom priority dialog did not open");
        }
    }

    /// Send Signal (UI-PRC-013 item 5): process rows map directly to process targets. App and
    /// unit rows use the same primary-process and bwrap resolution as End Task (PROC-065).
    fn send_signal(&self, signo: i32, targets: CapturedTargets, window: &mut Window, cx: &mut App) {
        let Some((rows, _)) = self.prune(targets, |_| None, cx) else {
            return;
        };
        let Some((targets, names)) = signal_targets(&rows, &self.snapshot.processes) else {
            return;
        };
        send_signal(&self.host, signo, targets, names, window, cx);
    }

    /// Real-time signal… uses the same targets and subtitle subject as Send Signal, then opens
    /// UI-PWN-027 modal to the window the command ran in.
    fn open_rt_signal(&self, targets: CapturedTargets, cx: &mut App) {
        let Some((rows, _)) = self.prune(targets, |_| None, cx) else {
            return;
        };
        let Some((targets, names)) = signal_targets(&rows, &self.snapshot.processes) else {
            return;
        };
        if let Err(err) = open_rt_signal(&self.host, targets, names, self.parent, cx) {
            warn!(target: "tasklens::ui", %err, "the real-time signal dialog did not open");
        }
    }

    /// Go to parent process (PROC-071): selects the live record whose PID is the target's
    /// parent PID through the Processes page's jump target, which switches to the Processes tab
    /// first. Nothing for a parent PID of 0 or several targets.
    fn go_to_parent(&self, targets: CapturedTargets, window: &mut Window, cx: &mut App) {
        let [t] = &targets.items[..] else {
            return;
        };
        if !matches!(t.kind, RowKind::Process | RowKind::KernelThread) || t.ppid == 0 {
            return;
        }
        let not_found = tr(cx, "processes-toast-parent-not-found");
        let parent = self.host.hub().and_then(|hub| {
            hub.read(cx)
                .current()
                .processes
                .records
                .iter()
                .find(|r| r.key.pid == t.ppid && !r.flags.contains(ProcFlags::EXITED))
                .map(|r| r.key)
        });
        let Some(parent) = parent else {
            self.toast(ToastKind::Warning, not_found, cx);
            return;
        };
        let texts = JumpTexts {
            success: Some(tr(cx, "processes-toast-switched-to-parent")),
            not_found: not_found.clone(),
            hidden: Some(not_found),
            switched_tab: None,
        };
        // The caller may be the Processes page itself, which is being updated now.
        window.defer(cx, move |window, cx| {
            let Some(view) = main_window_view(cx) else {
                warn!(target: "tasklens::ui", "no main window for Go to parent process");
                return;
            };
            if view
                .read(cx)
                .page_entity::<ProcessesPage>(Tab::Processes)
                .is_none()
            {
                // Selecting the tab builds the page.
                view.update(cx, |v, cx| v.select_tab(Tab::Processes, window, cx));
            }
            let Some(page) = view.read(cx).page_entity::<ProcessesPage>(Tab::Processes) else {
                warn!(target: "tasklens::ui", "no Processes page for Go to parent process");
                return;
            };
            page.update(cx, |p, cx| p.select_by_key(parent, texts, window, cx));
        });
    }

    /// Open file location (PROC-073) and File properties (PROC-052): the file manager shows the
    /// executable, or its properties. A restricted path toasts "Process image unavailable"; a
    /// deleted one sends nothing (the item is disabled).
    fn show_executable(&self, properties: bool, targets: CapturedTargets, cx: &mut App) {
        let [t] = &targets.items[..] else {
            return;
        };
        let NodeId::Process(key) = t.node else {
            return;
        };
        if t.exe_deleted {
            return;
        }
        let Some(r) = self
            .snapshot
            .processes
            .records
            .iter()
            .find(|r| r.key == key)
        else {
            return;
        };
        if exe_deleted(&r.meta) {
            return;
        }
        let Some(path) = r.meta.exe.clone() else {
            self.toast(
                ToastKind::Warning,
                tr(cx, "processes-toast-image-unavailable"),
                cx,
            );
            return;
        };
        let req = ActionRequest {
            // The tracker gives id 0 its next ActionId.
            id: ActionId(0),
            kind: ActionKind::ShowInFileManager { path, properties },
            targets: SmallVec::new(),
            privileged: false,
        };
        // No success text: the Info toast, or the file manager window, is the result. A failure
        // shows the reason only.
        let requested = (!properties).then(|| {
            (
                ToastKind::Info,
                tr(cx, "processes-toast-opening-file-location"),
            )
        });
        let texts = ActionToastTexts {
            requested,
            success: (ToastKind::Success, SharedString::default()),
            failure: (ToastKind::Error, SharedString::default()),
            partial: None,
        };
        self.submit(req, texts, cx);
    }

    /// Show Window (PROC-054): brings the topmost window of the captured process, or of any
    /// member of an app or unit row, to the front. The `ActivateWindow` result gives the toast.
    fn show_window(&self, targets: CapturedTargets, cx: &mut App) {
        let [t] = &targets.items[..] else {
            return;
        };
        let Some(hub) = self.host.hub().cloned() else {
            return;
        };
        let internal_id =
            topmost_window(hub.read(cx).current(), t).map(|w| Arc::clone(&w.internal_id));
        let Some(internal_id) = internal_id else {
            self.toast(ToastKind::Warning, tr(cx, "prc-055-no-window-found"), cx);
            return;
        };
        if let Some(text) = reconnecting_refusal(&self.host, cx) {
            self.toast(ToastKind::Warning, text, cx);
            return;
        }
        activate_window(&self.host, internal_id, self.queue.clone(), cx);
    }

    /// Steps 1 and 2 of every process action (E22-017): drops pseudo and exited rows ("Process
    /// exited" when only exited rows were left) and drops the rows `refuse` names, with one
    /// Warning toast per distinct text. `None` when no row is left.
    fn prune(
        &self,
        captured: CapturedTargets,
        refuse: impl Fn(&CapturedTarget) -> Option<RejectReason>,
        cx: &mut App,
    ) -> Option<(Vec<CapturedTarget>, Option<ColumnId>)> {
        let clicked_col = captured.clicked_col;
        let mut rows: Vec<CapturedTarget> = captured
            .items
            .into_iter()
            .filter(|t| t.kind != RowKind::Pseudo)
            .collect();
        if rows.is_empty() {
            return None;
        }
        let words = toast_words(cx);
        rows.retain(|t| !t.flags.contains(ProcFlags::EXITED));
        if rows.is_empty() {
            self.toast(ToastKind::Warning, words.exited.clone(), cx);
            return None;
        }
        let mut refused: SmallVec<[SharedString; 3]> = SmallVec::new();
        rows.retain(|t| {
            let Some(reason) = refuse(t) else {
                return true;
            };
            if let Some(text) = words.refusal_text(reason, t)
                && !refused.contains(&text)
            {
                refused.push(text);
            }
            false
        });
        for text in refused {
            self.toast(ToastKind::Warning, text, cx);
        }
        (!rows.is_empty()).then_some((rows, clicked_col))
    }

    /// Sends one `kind` request to `keys` with the toast texts for `used`, after the
    /// confirmation `ask` when there is one (R-198, D-009).
    fn send(
        &self,
        kind: ActionKind,
        keys: Vec<ProcessKey>,
        used: CapturedTargets,
        ask: Option<ConfirmKind>,
        cx: &mut App,
    ) {
        let targets = keys.into_iter().map(ActionTarget::Process).collect();
        self.send_targets(kind, targets, used, ask, cx);
    }

    /// As [`Self::send`], for targets that are not all processes (Kill App, Freeze App and
    /// Thaw App send cgroup and unit targets).
    fn send_targets(
        &self,
        kind: ActionKind,
        targets: SmallVec<[ActionTarget; 1]>,
        used: CapturedTargets,
        ask: Option<ConfirmKind>,
        cx: &mut App,
    ) {
        let Some(texts) = toast_words(cx).texts_for(&kind, &used) else {
            return;
        };
        let req = ActionRequest {
            // The tracker gives id 0 its next ActionId.
            id: ActionId(0),
            kind,
            targets,
            privileged: false,
        };
        let Some(ask) = ask else {
            self.submit(req, texts, cx);
            return;
        };
        let (host, queue, on_sent, parent) = (
            self.host.clone(),
            self.queue.clone(),
            self.on_sent.clone(),
            self.parent,
        );
        // The dialog opens modal to the parent window, which is being updated now.
        cx.defer(move |cx| {
            confirm(ask, parent, cx, move |result, cx| {
                if result == ConfirmResult::Confirmed {
                    submit(&host, req, texts, queue, on_sent, cx);
                }
            });
        });
    }

    fn submit(&self, req: ActionRequest, texts: ActionToastTexts, cx: &mut App) {
        submit(
            &self.host,
            req,
            texts,
            self.queue.clone(),
            self.on_sent.clone(),
            cx,
        );
    }
}

/// Hands `req` to the action tracker and passes the id it got to `on_sent`.
fn submit(
    host: &HostContext,
    req: ActionRequest,
    texts: ActionToastTexts,
    queue: Option<WeakEntity<ToastQueue>>,
    on_sent: Option<OnSent>,
    cx: &mut App,
) {
    if let Some(id) = submit_to(host, req, texts, queue, cx)
        && let Some(on_sent) = on_sent
    {
        cx.defer(move |cx| on_sent(id, cx));
    }
}

/// The Plasma Shell restart in flight (PROC-072). The tracker subscription clears it when that
/// request finishes.
struct PlasmaRestart {
    id: Option<ActionId>,
    _finished: Option<Subscription>,
}

impl Global for PlasmaRestart {}

fn plasma_restart_running(cx: &App) -> bool {
    cx.try_global::<PlasmaRestart>()
        .is_some_and(|p| p.id.is_some())
}

/// Records `id` as the Plasma Shell restart in flight. The first call subscribes to `host`'s
/// tracker.
fn plasma_restart_started(host: &HostContext, id: ActionId, cx: &mut App) {
    if !cx.has_global::<PlasmaRestart>() {
        let tracker = host.tracker().cloned();
        let finished = tracker.map(|tracker| {
            cx.subscribe(&tracker, |_, ev: &ActionTrackerEvent, cx| {
                if let ActionTrackerEvent::Finished { id, .. } = ev {
                    let p = cx.global_mut::<PlasmaRestart>();
                    if p.id == Some(*id) {
                        p.id = None;
                    }
                }
            })
        });
        cx.set_global(PlasmaRestart {
            id: None,
            _finished: finished,
        });
    }
    cx.global_mut::<PlasmaRestart>().id = Some(id);
}

/// The confirmation action names, translated once per strings version (R-132).
struct ConfirmWordsCache {
    version: u64,
    words: Rc<ConfirmWords>,
}

impl Global for ConfirmWordsCache {}

/// The confirmation action names for the current strings. Translated only when the strings
/// version changed; without strings they are in en-US.
fn confirm_words(cx: &mut App) -> Rc<ConfirmWords> {
    let version = cx.try_global::<Strings>().map_or(0, |s| s.strings_version);
    if let Some(cache) = cx.try_global::<ConfirmWordsCache>()
        && cache.version == version
    {
        return cache.words.clone();
    }
    let words = Rc::new(ConfirmWords::new(cx));
    cx.set_global(ConfirmWordsCache {
        version,
        words: words.clone(),
    });
    words
}
