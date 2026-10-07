//! The jump target (E22-A3, PROC-071, UI-PRC-022, UI-PRC-010, UI-PRC-015): select one process
//! by identity from anywhere in the app.
//!
//! `select_by_key` switches to the Processes tab, leaves History mode, expands the record's path
//! with one query and waits for the view of that generation. That view decides: the row is
//! there, so it is selected, centred and focused; or it is not, and a filter or Hide system
//! processes is on, so the "not visible" toast offers Show; or it is not found. Show clears the
//! filter and Hide system processes and expands once more, so a jump costs at most two query
//! round trips. Whether a row is filtered or hidden is the engine's rule (PROC-046), so the page
//! asks the view instead of predicting it.

use gpui_kit::{Action as _, AnyWindowHandle, App, Context, SharedString, Window};
use rustc_hash::FxHashMap;
use tl_model::command::Tab;
use tl_model::ids::{NodeId, ProcessKey};
use tl_model::process::{GroupKind, ProcFlags, ProcessRecord};
use tl_model::snapshot::ProcessSnapshot;
use tl_model::table::TableMode;

use super::ProcessesPage;
use super::actions::ProcessesShowHidden;
use super::delegate::ProcRowId;
use super::table::GroupHeader;
use crate::actions::{ClearFilter, SelectTab};
use crate::components::toast::{ToastAction, ToastKind};
use crate::hub::GlobalHub;
use crate::i18n::tr;
use crate::settings_global::TlSettings;
use crate::window::main::{ShellState, push_main_toast};

/// The caller's toast texts. An empty or `None` text means this module's default string.
#[derive(Clone, Debug, Default)]
pub struct JumpTexts {
    /// Shown when the tab changed; `None` is "Switched to Processes tab".
    pub switched_tab: Option<SharedString>,
    /// The caller's success toast. Replaces `switched_tab` when set, for example "Switched to
    /// owning process.".
    pub success: Option<SharedString>,
    /// Empty (the default) is "Process not found in current view".
    pub not_found: SharedString,
    /// The caller's "not visible" text; `None` is the `not_found` text. Always shown with a Show
    /// button.
    pub hidden: Option<SharedString>,
}

/// A jump waiting for the view of `generation`.
pub(super) struct PendingJump {
    key: ProcessKey,
    texts: JumpTexts,
    tab_changed: bool,
    generation: u64,
    /// Show already cleared the filter and Hide system processes: a missing row is not found.
    after_show: bool,
    window: AnyWindowHandle,
}

/// The page's jump state.
#[derive(Default)]
pub(super) struct JumpState {
    pending: Option<PendingJump>,
    /// The jump the "not visible" toast's Show button continues.
    hidden: Option<PendingJump>,
}

impl ProcessesPage {
    /// Selects the process `key` (PROC-071): see the module docs. Keep the signature; E20, E23,
    /// E27, E28 and E30 call it.
    pub fn select_by_key(
        &mut self,
        key: ProcessKey,
        texts: JumpTexts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = cx.try_global::<ShellState>().map(|s| s.active_tab);
        let tab_changed = active.is_some_and(|t| t != Tab::Processes);
        if tab_changed {
            window.dispatch_action(Box::new(SelectTab(Tab::Processes as usize)), cx);
        }
        if cx.has_global::<TlSettings>() && TlSettings::state(cx).processes.history_mode {
            TlSettings::update(cx, |d| d.state.processes.history_mode = false);
        }
        self.jump.hidden = None;
        self.jump.pending = None;
        let job = PendingJump {
            key,
            texts,
            tab_changed,
            generation: 0,
            after_show: false,
            window: window.window_handle(),
        };
        self.expand_and_wait(job, cx);
    }

    /// Resolves `pid` to the key of its current non-exited record once, then `select_by_key`.
    /// No such record: the not found toast.
    pub fn select_by_pid(
        &mut self,
        pid: u32,
        texts: JumpTexts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = live_processes(cx)
            .records
            .iter()
            .find(|r| r.key.pid == pid && !r.flags.contains(ProcFlags::EXITED))
            .map(|r| r.key);
        match key {
            Some(key) => self.select_by_key(key, texts, window, cx),
            None => not_found(&texts, cx),
        }
    }

    /// Steps 2 and 4: a missing or exited record is not found; else one query expands its path
    /// and the jump waits for that generation.
    fn expand_and_wait(&mut self, mut job: PendingJump, cx: &mut Context<Self>) {
        let processes = live_processes(cx);
        let Some(record) = processes
            .get(job.key)
            .filter(|r| !r.flags.contains(ProcFlags::EXITED))
        else {
            not_found(&job.texts, cx);
            return;
        };
        let client = &*self.client;
        job.generation = self.table.update(cx, |t, _| {
            let q = t.delegate.query_mut();
            let path = expand_path(&processes, record, q.mode());
            q.expand_path(&path);
            q.bump_and_send(client);
            q.generation()
        });
        self.jump.pending = Some(job);
    }

    /// Steps 3, 5 and 6, run after each snapshot: once the view of the pending generation is in
    /// the table, the jump selects the row or explains why it cannot.
    pub(super) fn resolve_jump(&mut self, cx: &mut Context<Self>) {
        let Some(job) = &self.jump.pending else {
            return;
        };
        let (installed, query) = {
            let d = &self.table.read(cx).delegate;
            (d.snapshot().table.query_generation, d.query().generation())
        };
        if installed != job.generation || query != job.generation {
            return;
        }
        let Some(job) = self.jump.pending.take() else {
            return;
        };
        let id = ProcRowId::Process(job.key);
        if self.table.update(cx, |t, cx| t.jump_to(id, cx)) {
            let focus = self.focus.clone();
            let handle = job.window;
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| window.focus(&focus, cx));
            });
            let text = job.texts.success.clone().or_else(|| {
                job.tab_changed.then(|| {
                    job.texts
                        .switched_tab
                        .clone()
                        .unwrap_or_else(|| tr(cx, "processes-jump-switched-tab"))
                })
            });
            if let Some(text) = text {
                defer_toast(ToastKind::Info, text, None, cx);
            }
            return;
        }
        let live = live_processes(cx)
            .get(job.key)
            .is_some_and(|r| !r.flags.contains(ProcFlags::EXITED));
        if !live || job.after_show || !self.hides_rows(cx) {
            not_found(&job.texts, cx);
            return;
        }
        let text = job
            .texts
            .hidden
            .clone()
            .unwrap_or_else(|| not_found_text(&job.texts, cx));
        let show = ToastAction {
            label: tr(cx, "processes-jump-show"),
            action: ProcessesShowHidden.boxed_clone(),
        };
        defer_toast(ToastKind::Warning, text, Some(show), cx);
        self.jump.hidden = Some(job);
    }

    /// A filter or Hide system processes can keep a live record out of the view.
    fn hides_rows(&self, cx: &App) -> bool {
        let filtered = !self.table.read(cx).delegate.query().filter().is_empty();
        filtered
            || (cx.has_global::<TlSettings>() && TlSettings::settings(cx).processes.hide_system)
    }

    /// The Show button: clears the filter (the shell box and the query) and Hide system
    /// processes, then expands again and waits for that view.
    pub(super) fn show_hidden(&mut self, cx: &mut Context<Self>) {
        let Some(mut job) = self.jump.hidden.take() else {
            return;
        };
        // Deferred: the button's click is still inside the window's update.
        let handle = job.window;
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                window.dispatch_action(ClearFilter.boxed_clone(), cx);
            });
        });
        self.table.update(cx, |t, _| {
            t.delegate.query_mut().set_filter("".into());
        });
        if cx.has_global::<TlSettings>() && TlSettings::settings(cx).processes.hide_system {
            TlSettings::update(cx, |d| d.settings.processes.hide_system = false);
        }
        job.after_show = true;
        self.expand_and_wait(job, cx);
    }
}

/// Installs the `ProcessesShowHidden` handler for `page`. The toast button dispatches from the
/// toast layer, outside the page's element tree, so the handler is app-level.
pub(super) fn register_show_hidden(cx: &mut Context<ProcessesPage>) {
    let page = cx.weak_entity();
    // `App::on_action`: `Context::on_action` is the element-level form.
    let app: &mut App = cx;
    app.on_action(move |_: &ProcessesShowHidden, cx| {
        let _ = page.update(cx, |p, cx| p.show_hidden(cx));
    });
}

/// The hub's current processes: the freshest identities, whatever the table shows.
fn live_processes(cx: &App) -> std::sync::Arc<ProcessSnapshot> {
    std::sync::Arc::clone(&cx.global::<GlobalHub>().0.read(cx).current().processes)
}

/// The rows to expand so `record`'s row is in the view (step 4). Ids that are not rows in this
/// view are ignored by the engine, so the path may name more than it needs.
///
/// Tree mode: every ancestor by ppid. Grouped mode: the class header; then for an app its app
/// row and the ancestors among the app's members; for a kernel thread the Kernel threads row;
/// else the unit row (a unit with one process does not fold, so the id is unused). Flat mode
/// has no rows to expand.
pub fn expand_path(
    processes: &ProcessSnapshot,
    record: &ProcessRecord,
    mode: TableMode,
) -> Vec<NodeId> {
    let mut path = Vec::new();
    match mode {
        TableMode::Tree => ancestors(processes, record, |_| true, &mut path),
        TableMode::Grouped => {
            let kthread = record.flags.contains(ProcFlags::KERNEL_THREAD);
            let background = record.group == GroupKind::Background.key();
            let system = kthread
                || record.group == GroupKind::System.key()
                || record.group == GroupKind::Ungrouped.key();
            if system || background {
                let header = if system {
                    GroupHeader::System
                } else {
                    GroupHeader::Background
                };
                path.push(header.node());
                if kthread {
                    path.push(NodeId::KernelThreads);
                } else if let Some(unit) = unit_of(processes, record) {
                    path.push(NodeId::Unit(unit));
                }
            } else {
                path.push(GroupHeader::Apps.node());
                path.push(NodeId::Group(record.group));
                let group = record.group;
                ancestors(processes, record, |r| r.group == group, &mut path);
            }
        }
        TableMode::Flat => {}
    }
    path
}

/// Pushes `record`'s ancestors by ppid, nearest first, while `keep` holds. Stops at a pid that
/// is not a record or was already seen.
fn ancestors(
    processes: &ProcessSnapshot,
    record: &ProcessRecord,
    keep: impl Fn(&ProcessRecord) -> bool,
    path: &mut Vec<NodeId>,
) {
    let by_pid: FxHashMap<u32, &ProcessRecord> = processes
        .records
        .iter()
        .filter(|r| !r.flags.contains(ProcFlags::EXITED))
        .map(|r| (r.key.pid, r))
        .collect();
    let mut seen = vec![record.key.pid];
    let mut ppid = record.ppid;
    while let Some(parent) = by_pid.get(&ppid) {
        if seen.contains(&ppid) || !keep(parent) {
            break;
        }
        seen.push(ppid);
        path.push(NodeId::Process(parent.key));
        ppid = parent.ppid;
    }
}

fn unit_of(
    processes: &ProcessSnapshot,
    record: &ProcessRecord,
) -> Option<tl_model::table::UnitRef> {
    let ix = processes
        .records
        .binary_search_by(|r| r.key.cmp(&record.key))
        .ok()?;
    processes.extras.get(ix)?.unit.clone()
}

fn not_found_text(texts: &JumpTexts, cx: &App) -> SharedString {
    if texts.not_found.is_empty() {
        tr(cx, "processes-jump-not-found")
    } else {
        texts.not_found.clone()
    }
}

fn not_found(texts: &JumpTexts, cx: &mut App) {
    let text = not_found_text(texts, cx);
    defer_toast(ToastKind::Warning, text, None, cx);
}

/// Deferred, so a caller inside a main window update does not read the window mid-update.
fn defer_toast(kind: ToastKind, text: SharedString, action: Option<ToastAction>, cx: &mut App) {
    cx.defer(move |cx| push_main_toast(cx, kind, text, None, action));
}
