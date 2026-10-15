//! The window-pick result (E22-045, PROC-055, UI-SHL-014, R-239, R-149).
//!
//! The shell starts a pick (E20-027) and its `WINDOWS` hub handler hands each new
//! `WindowsSnapshot.pick` here once, through [`PickWatch`]. The shell handles it even when the
//! Processes page was never built: [`on_pick_result`] ends picking, shows the toast for
//! `NoWindow`, `Cancelled` or an unknown PID, and for a known PID selects the Processes tab
//! (which builds the page) and calls [`ProcessesPage::on_picked`].
//!
//! A plain click with the main window active selects the row through the jump target
//! (PROC-071). A Ctrl+click, or any click while the main window is not active (tasklens cannot
//! see the modifier then), opens the Picked confirmation: "Select" selects the row, "Terminate"
//! runs End Task under E22-017's rules.

use gpui_kit::{App, Context, Window};
use tl_model::command::Tab;
use tl_model::ids::{NodeId, ProcessKey};
use tl_model::process::{ProcFlags, ProcessRecord};
use tl_model::snapshot::{PickOutcome, PickResult, WindowsSnapshot};
use tracing::debug;

use super::actions::{CapturedTarget, CapturedTargets};
use super::confirm::{ConfirmKind, ConfirmResult, confirm};
use super::jump::JumpTexts;
use super::{End, ProcessesPage, record_target};
use crate::components::toast::ToastKind;
use crate::hub::GlobalHub;
use crate::i18n::tr;
use crate::window::main::{MainWindow, PICK_NONE_TEXT, PICK_TOAST};

const NO_WINDOW_TEXT: &str = "prc-055-no-window-found";
const UNKNOWN_TEXT: &str = "prc-055-unknown-process";

/// The `seq` of the last pick result handled. A result is handled once: only when its `seq` is
/// above this one.
#[derive(Clone, Copy, Debug, Default)]
pub struct PickWatch {
    last_seq: u64,
}

impl PickWatch {
    /// A watch that treats the result already in `windows` as handled, so a main window that
    /// opens later does not act on an old pick.
    pub fn new(windows: &WindowsSnapshot) -> Self {
        Self {
            last_seq: windows.pick.map_or(0, |p| p.seq),
        }
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// The pick result in `windows` when it is new, and marks it handled.
    pub fn take(&mut self, windows: &WindowsSnapshot) -> Option<PickResult> {
        let pick = windows.pick?;
        if pick.seq <= self.last_seq {
            return None;
        }
        self.last_seq = pick.seq;
        Some(pick)
    }
}

/// What tasklens knows about Ctrl at the click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickModifier {
    /// The main window is active and Ctrl is up.
    Plain,
    /// The main window is active and Ctrl is down.
    Ctrl,
    /// The main window is not active, so GPUI does not report modifiers (PROC-055).
    Unknown,
}

impl PickModifier {
    fn of(window: &Window) -> Self {
        if !window.is_window_active() {
            PickModifier::Unknown
        } else if window.modifiers().control {
            PickModifier::Ctrl
        } else {
            PickModifier::Plain
        }
    }
}

/// Handles one new pick result in the main window: ends picking, then the toast or the page's
/// handler (PROC-055 steps 1 to 4).
pub fn on_pick_result(
    shell: &mut MainWindow,
    result: PickResult,
    window: &mut Window,
    cx: &mut Context<MainWindow>,
) {
    debug!(target: "tasklens::ui", outcome = ?result.outcome, "window pick result");
    shell.end_window_pick(cx);
    let pid = match result.outcome {
        PickOutcome::NoWindow => return pick_toast(shell, NO_WINDOW_TEXT, cx),
        PickOutcome::Cancelled => return pick_toast(shell, PICK_NONE_TEXT, cx),
        PickOutcome::Picked { pid } => pid,
    };
    let Some(target) = live_target(cx, |r| r.key.pid == pid) else {
        return pick_toast(shell, UNKNOWN_TEXT, cx);
    };
    let modifier = PickModifier::of(window);
    shell.select_tab(Tab::Processes, window, cx);
    let Some(page) = shell.page_entity::<ProcessesPage>(Tab::Processes) else {
        return;
    };
    page.update(cx, |p, cx| p.on_picked(target, modifier, window, cx));
}

/// The Info toast `key` with the picker's toast key, so it replaces the start toast or an
/// earlier "No window selected".
fn pick_toast(shell: &MainWindow, key: &str, cx: &mut Context<MainWindow>) {
    let text = tr(cx, key);
    shell.toasts().update(cx, |q, cx| {
        q.push(ToastKind::Info, text, Some(PICK_TOAST), None, cx)
    });
}

/// The first non-exited record of the hub's live snapshot that `pred` accepts, as a process
/// row target. The page may be paused (Ctrl-pause), so its own snapshot can be old.
fn live_target(cx: &App, pred: impl Fn(&ProcessRecord) -> bool) -> Option<CapturedTarget> {
    let hub = cx.try_global::<GlobalHub>()?.0.read(cx);
    hub.current()
        .processes
        .records
        .iter()
        .find(|r| !r.flags.contains(ProcFlags::EXITED) && pred(r))
        .map(record_target)
}

impl ProcessesPage {
    /// A picked process (PROC-055 steps 5 and 6): a plain click selects it; a Ctrl+click or a
    /// click tasklens could not see the modifier of opens the Picked confirmation.
    pub fn on_picked(
        &mut self,
        target: CapturedTarget,
        modifier: PickModifier,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let NodeId::Process(key) = target.node else {
            return;
        };
        if modifier == PickModifier::Plain {
            self.select_by_key(key, JumpTexts::default(), window, cx);
            return;
        }
        let kind = ConfirmKind::Picked {
            name: target.name.clone(),
            pid: key.pid,
        };
        let page = cx.entity().downgrade();
        let parent = window.window_handle();
        // The dialog opens modal to this window, which is being updated now. Its result runs
        // inside an update of the same window, so the follow-up waits for that to end.
        cx.defer(move |cx| {
            confirm(kind, parent, cx, move |result, cx| {
                if result == ConfirmResult::Cancelled {
                    return;
                }
                cx.defer(move |cx| {
                    let done = parent.update(cx, |_, window, cx| {
                        page.update(cx, |p, cx| match result {
                            ConfirmResult::Second => p.end_picked(key, target, window, cx),
                            _ => p.select_by_key(key, JumpTexts::default(), window, cx),
                        })
                    });
                    if !matches!(done, Ok(Ok(()))) {
                        debug!(target: "tasklens::ui", "the main window closed before the pick");
                    }
                });
            });
        });
    }

    /// End Task on the picked process, with its record as it is now. A process that exited
    /// meanwhile keeps the record from the pick, marked exited, so End Task shows its exited
    /// toast.
    fn end_picked(
        &mut self,
        key: ProcessKey,
        picked: CapturedTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = live_target(cx, |r| r.key == key).unwrap_or_else(|| CapturedTarget {
            flags: picked.flags | ProcFlags::EXITED,
            ..picked
        });
        self.set_menu_targets(CapturedTargets::new([target], None));
        self.end(End::Task, window, cx);
    }
}
