//! Connections flows: Jump to process, Go to service (UI-SYS-065, UI-SYS-063) and Close TCP
//! connection (UI-SYS-064).
//!
//! Close acts on the records captured at menu open, never on a newer snapshot: the request
//! carries the captured cookie and four-tuple, and the helper matches both (R-119, ADR-006).

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, App, Context, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    Window, div,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionKind, ActionTarget};
use tl_model::command::Tab;
use tl_model::snapshot::ConnState;
use tl_model::table::Manager;
use tracing::warn;

use super::ConnectionsPage;
use super::actions::ConnectionCloseTcp;
use super::menu::ConnRecordRef;
use crate::components::dialog::{DialogResult, TlDialog};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::{tr, tr_args};
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::jump::JumpTexts;
use crate::pages::services::actions::{GoToService, JumpOrigin, ManagerArg};
use crate::pages::sys_table::{ActionKindTag, ActionTexts};
use crate::text::style::t_base;
use crate::theme::{current, metrics};
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

/// Addresses listed in the several-sockets confirmation before "and <m> more".
const MAX_ADDRESSES: usize = 5;

/// The texts of the listening-socket confirmation (UI-SYS-064 step 1).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ListenConfirmTexts {
    pub heading: SharedString,
    pub text: SharedString,
    /// Several-sockets form only: up to five addresses, one per line.
    pub addresses: Vec<SharedString>,
    /// "and <m> more".
    pub more: Option<SharedString>,
}

/// The confirmation texts for the local addresses of the listening targets.
pub fn listen_confirm_texts(addresses: &[SharedString], cx: &App) -> ListenConfirmTexts {
    let mut out = ListenConfirmTexts::default();
    if let [one] = addresses {
        out.heading = tr(cx, "connections-close-listen-heading-one");
        out.text = tr_args(
            cx,
            "connections-close-listen-text-one",
            &[("addr", FluentValue::from(one.as_ref()))],
        );
        return out;
    }
    let count = |n: usize| FluentValue::from(n as i64);
    out.heading = tr_args(
        cx,
        "connections-close-listen-heading-many",
        &[("n", count(addresses.len()))],
    );
    out.text = tr(cx, "connections-close-listen-text-many");
    out.addresses = addresses.iter().take(MAX_ADDRESSES).cloned().collect();
    if addresses.len() > MAX_ADDRESSES {
        out.more = Some(tr_args(
            cx,
            "connections-close-listen-more",
            &[("m", count(addresses.len() - MAX_ADDRESSES))],
        ));
    }
    out
}

/// The confirmation, titled "tasklens": "Cancel" has focus and "Close socket" is Danger, so a
/// stray Enter cancels.
fn listen_dialog(texts: &ListenConfirmTexts, cx: &App) -> TlDialog {
    let lines: Vec<SharedString> = texts
        .addresses
        .iter()
        .chain(texts.more.iter())
        .cloned()
        .collect();
    let dialog = TlDialog::confirm(
        texts.text.clone(),
        tr(cx, "connections-close-listen-confirm"),
    )
    .destructive()
    .heading(texts.heading.clone())
    .cancel_label(tr(cx, "connections-close-listen-cancel"));
    if lines.is_empty() {
        return dialog;
    }
    dialog.extra(move |_, cx| address_lines(&lines, cx))
}

fn address_lines(lines: &[SharedString], cx: &mut App) -> AnyElement {
    let theme = current(cx).clone();
    let m = *metrics(cx);
    let mut out = div().flex().flex_col().gap(m.sp[4]);
    for text in lines {
        let mut line = div();
        *line.text_style() = t_base(&m);
        out = out.child(line.text_color(theme.text_primary).child(text.clone()));
    }
    out.into_any_element()
}

/// Toast texts of a close request (UI-SYS-064, UI-SYS-007, R-203 as amended).
fn close_texts(cx: &App) -> ActionTexts {
    ActionTexts {
        pending_one: tr(cx, "connections-close-requested"),
        pending_many: "connections-close-requested-n",
        done_one: tr(cx, "connections-close-done"),
        done_many: "connections-close-done-n",
        partial_many: "connections-close-partial",
        failed_prefix: tr(cx, "connections-close-failed"),
        failed_suffix: true,
        unknown: tr(cx, "connections-close-unknown"),
        busy: tr(cx, "connections-close-busy"),
        failed_button: None,
    }
}

impl ConnectionsPage {
    /// "Close TCP connection": the captured TCP records that are not in TIME-WAIT and have no
    /// close pending. A listening target asks for confirmation first; Cancel sends nothing.
    pub(super) fn on_close_tcp(
        &mut self,
        _: &ConnectionCloseTcp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let captured = self.take_targets(cx);
        let actions = self.frame.actions.read(cx);
        let mut any_closable = false;
        let records: SmallVec<[ConnRecordRef; 1]> = captured
            .records
            .into_iter()
            .filter(|r| r.closable())
            .inspect(|_| any_closable = true)
            .filter(|r| !actions.is_pending(&r.socket_target(), ActionKindTag::CloseTcp))
            .collect();
        if records.is_empty() {
            if any_closable {
                let text = tr(cx, "connections-close-busy");
                if let Some(queue) = ToastQueue::of_window(window, cx) {
                    queue.update(cx, |q, cx| q.push(ToastKind::Warning, text, None, None, cx));
                }
            }
            return;
        }
        let listening: SmallVec<[SharedString; 1]> = records
            .iter()
            .filter(|r| r.state == ConnState::Listening)
            .map(|r| SharedString::from(r.local.to_string()))
            .collect();
        if listening.is_empty() {
            self.submit_close(records, window, cx);
            return;
        }
        let texts = listen_confirm_texts(&listening, cx);
        let page = cx.weak_entity();
        let opened = listen_dialog(&texts, cx).open(window, cx, move |result, window, cx| {
            if matches!(result, DialogResult::Confirmed { .. }) {
                let _ = page.update(cx, |page, cx| page.submit_close(records, window, cx));
            }
        });
        if opened.is_err() {
            warn!(target: "tasklens::ui", "the close socket confirmation did not open");
        }
    }

    /// One `SockDestroy` request for `records`, with targets built from the captured values.
    fn submit_close(
        &mut self,
        records: SmallVec<[ConnRecordRef; 1]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets: SmallVec<[ActionTarget; 1]> =
            records.iter().map(ConnRecordRef::socket_target).collect();
        let texts = close_texts(cx);
        let id = self.frame.actions.update(cx, |actions, cx| {
            actions.submit(
                ActionKind::SockDestroy,
                ActionKindTag::CloseTcp,
                targets,
                texts,
                window,
                cx,
            )
        });
        if let Some(id) = id {
            for (index, record) in records.iter().enumerate() {
                self.close_requests.insert(
                    record.cookie,
                    (id, u16::try_from(index).unwrap_or(u16::MAX)),
                );
            }
        }
    }
}
