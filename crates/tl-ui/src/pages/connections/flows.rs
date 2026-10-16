//! Connections jump flows: Jump to process and Go to service (UI-SYS-065, UI-SYS-063).

use gpui_kit::{App, SharedString, Window};
use tl_model::command::Tab;
use tl_model::table::Manager;
use tracing::warn;

use super::menu::ConnRecordRef;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::tr;
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::jump::JumpTexts;
use crate::pages::services::actions::{GoToService, JumpOrigin, ManagerArg};
use crate::window::main::main_window_view;

/// Selects the owning process on the Processes tab. A shared socket jumps to the lowest PID.
pub(super) fn run_jump_to_process(record: &ConnRecordRef, window: &mut Window, cx: &mut App) {
    let Some(key) = record.owners.iter().min_by_key(|k| k.pid).copied() else {
        let text = tr(cx, "connections-jump-no-owner");
        match ToastQueue::of_window(window, cx) {
            Some(queue) => {
                queue.update(cx, |q, cx| q.push(ToastKind::Warning, text, None, None, cx))
            }
            None => warn!(target: "tasklens::ui", "no toast stack for a Connections toast"),
        }
        return;
    };
    let Some(view) = main_window_view(cx) else {
        return;
    };
    view.update(cx, |v, cx| v.select_tab(Tab::Processes, window, cx));
    let Some(page) = view.read(cx).page_entity::<ProcessesPage>(Tab::Processes) else {
        warn!(target: "tasklens::ui", "no Processes page for Jump to process");
        return;
    };
    let texts = JumpTexts {
        switched_tab: None,
        success: Some(tr(cx, "connections-jump-switched")),
        not_found: Default::default(),
        hidden: Some(tr(cx, "connections-jump-not-visible")),
    };
    page.update(cx, |p, cx| p.select_by_key(key, texts, window, cx));
}

/// Asks the main window to show the socket owner's unit on the Services tab.
pub(super) fn run_go_to_service(record: &ConnRecordRef, window: &mut Window, cx: &mut App) {
    let Some(unit) = &record.unit else {
        return;
    };
    let manager = match unit.manager {
        Manager::System => ManagerArg::System,
        Manager::User => ManagerArg::User,
    };
    window.dispatch_action(
        Box::new(GoToService {
            manager,
            name: SharedString::from(&*unit.name),
            origin: JumpOrigin::Connections,
        }),
        cx,
    );
}
