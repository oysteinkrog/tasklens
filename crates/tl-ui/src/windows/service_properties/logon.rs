//! Service Properties Log On tab (UI-SYS-032, SYS-012, R-050, R-115).
//!
//! A read-only grid of the identity a unit runs as: seven rows in a fixed order, one
//! `TlKeyValueGrid`. The window pushes each new details Arc in through `set_details`; the tab
//! formats the values then and only then, and notifies itself, because the window draws it
//! through `.cached()`. The tab is never dirty.
//!
//! User and group names are shown, never logged (ADR-015).

use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::base::TextSelection;
use gpui_kit::base::input::Copy;
use gpui_kit::component::v_flex;
use gpui_kit::{
    App, Context, EntityId, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, px,
};
use smallvec::SmallVec;
use tl_model::actions::ActionId;
use tl_model::snapshot::UnitDetails;
use tl_model::table::Manager;
use tl_model::validity::Validity;
use tracing::warn;

use super::ServicePropsTabView;
use crate::components::kv_grid::{
    KEY_CONTEXT, KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue,
    UnsupportedWord,
};
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::{tr, tr_args};
use crate::theme::metrics;

/// Label column width at 100% scale.
const LABEL_WIDTH: f32 = 160.;
/// Row height at 100% scale.
const ROW_HEIGHT: f32 = 28.;

/// Row label keys, in row order.
const LABEL_KEYS: [&str; 7] = [
    "services-sys032-label-log-on",
    "services-sys032-label-group",
    "services-sys032-label-supplementary-groups",
    "services-sys032-label-runs-as",
    "services-sys032-label-pam-service",
    "services-sys032-label-manager",
    "services-sys032-label-session",
];

gpui_kit::actions!(tasklens, [ServiceLogOnCopyValue]);

/// How a value reads (R-050).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Word {
    Value,
    Collecting,
    NotReported,
}

#[derive(Clone, Debug)]
struct Field {
    text: SharedString,
    word: Word,
}

impl Field {
    fn value(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            word: Word::Value,
        }
    }

    fn grid_value(&self) -> TlValue {
        let mut value = TlValue::new(self.text.clone());
        match self.word {
            Word::Value => {}
            Word::Collecting => value.validity = Validity::WarmingUp,
            Word::NotReported => {
                value.validity = Validity::Unsupported;
                value.word = Some(UnsupportedWord::NotReported);
            }
        }
        value
    }
}

/// The translated words the values use.
struct Words {
    collecting: SharedString,
    not_reported: SharedString,
}

impl Words {
    fn new(cx: &App) -> Self {
        Self {
            collecting: tr(cx, "fnd-kv-collecting"),
            not_reported: tr(cx, "fnd-kv-not-reported"),
        }
    }

    fn collecting(&self) -> Field {
        Field {
            text: self.collecting.clone(),
            word: Word::Collecting,
        }
    }

    fn not_reported(&self) -> Field {
        Field {
            text: self.not_reported.clone(),
            word: Word::NotReported,
        }
    }
}

/// The unit id without its last ".suffix": `systemd-timesyncd.service` gives
/// `systemd-timesyncd`.
fn unit_stem(id: &str) -> &str {
    id.rsplit_once('.').map_or(id, |(stem, _)| stem)
}

/// The formatted values, rebuilt only when the details Arc changes.
struct Fields {
    rows: [Field; 7],
}

impl Fields {
    fn collecting(w: &Words) -> Self {
        Self {
            rows: std::array::from_fn(|_| w.collecting()),
        }
    }

    fn new(d: &UnitDetails, w: &Words, user_name: Option<&SharedString>, cx: &App) -> Self {
        let user = d.user.as_deref().filter(|u| !u.is_empty());
        let log_on = if d.dynamic_user {
            let name = user.unwrap_or_else(|| unit_stem(&d.id));
            Field::value(tr_args(
                cx,
                "services-sys032-dynamic-user",
                &[("name", FluentValue::from(name))],
            ))
        } else if let Some(user) = user {
            Field::value(user.to_owned())
        } else {
            // No `User=`: the system manager runs the unit as root (SYS-012).
            match (d.manager, user_name) {
                (Manager::System, _) => Field::value(tr(cx, "services-sys032-root")),
                (Manager::User, Some(name)) => Field::value(name.clone()),
                (Manager::User, None) => w.not_reported(),
            }
        };
        let group = match d.group.as_deref().filter(|g| !g.is_empty()) {
            Some(g) => Field::value(g.to_owned()),
            None => w.not_reported(),
        };
        let supplementary = if d.supplementary_groups.is_empty() {
            w.not_reported()
        } else {
            let names: Vec<&str> = d.supplementary_groups.iter().map(|g| &**g).collect();
            Field::value(names.join(", "))
        };
        let runs_as = if d.main_pid == 0 {
            Field::value(tr(cx, "services-sys032-not-running"))
        } else {
            match d.main_pid_ids {
                Some(ids) => Field::value(tr_args(
                    cx,
                    "services-sys032-runs-as",
                    &[
                        ("uid", FluentValue::from(i64::from(ids.uid))),
                        ("gid", FluentValue::from(i64::from(ids.gid))),
                    ],
                )),
                None => w.not_reported(),
            }
        };
        let pam = match d.pam_name.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => Field::value(p.to_owned()),
            None => w.not_reported(),
        };
        let manager = match (d.manager, user_name) {
            (Manager::System, _) => Field::value(tr(cx, "services-sys032-manager-system")),
            (Manager::User, Some(name)) => Field::value(tr_args(
                cx,
                "services-sys032-manager-user",
                &[("user", FluentValue::from(&**name))],
            )),
            (Manager::User, None) => w.not_reported(),
        };
        let session = Field::value(tr(
            cx,
            if d.graphical_session {
                "services-sys032-session-graphical"
            } else {
                "services-sys032-session-none"
            },
        ));
        Self {
            rows: [log_on, group, supplementary, runs_as, pam, manager, session],
        }
    }
}

/// A toast in `window`'s own stack.
fn window_toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack for the service Log On tab"),
    }
}

pub struct LogOnTab {
    entity_id: EntityId,
    details: Option<Arc<UnitDetails>>,
    words: Words,
    labels: [SharedString; 7],
    fields: Fields,
    /// The current user's name, for a user manager's unit.
    user_name: Option<SharedString>,
    /// The text "Copy value" copies.
    menu_value: Option<SharedString>,
    focus: FocusHandle,
    #[cfg(any(test, feature = "test-hooks"))]
    renders: u32,
}

impl LogOnTab {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let words = Words::new(cx);
        let labels = LABEL_KEYS.map(|key| tr(cx, key));
        let fields = Fields::collecting(&words);
        let user_name = std::env::var("USER")
            .ok()
            .filter(|u| !u.is_empty())
            .map(SharedString::from);
        Self {
            entity_id: cx.entity_id(),
            details: None,
            words,
            labels,
            fields,
            user_name,
            menu_value: None,
            focus: cx.focus_handle(),
            #[cfg(any(test, feature = "test-hooks"))]
            renders: 0,
        }
    }

    fn rebuild(&mut self, cx: &App) {
        self.fields = match &self.details {
            Some(d) => Fields::new(d, &self.words, self.user_name.as_ref(), cx),
            None => Fields::collecting(&self.words),
        };
    }

    fn open_value_menu(
        &mut self,
        row: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_value = self.fields.rows.get(row).map(|f| f.text.clone());
        let label = tr(cx, "services-sys032-copy-value");
        let menu = TlMenu::build(window, cx, self.focus.clone(), |menu, _, _| {
            menu.item(label, Box::new(ServiceLogOnCopyValue))
        });
        open_context_menu_at(position, window, cx, menu);
    }

    fn on_copy_value(
        &mut self,
        _: &ServiceLogOnCopyValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.menu_value.take() else {
            return;
        };
        if crate::util::clipboard_write(cx, text.to_string()).is_ok() {
            let toast = tr(cx, "sys-sys006-copied-to-clipboard");
            window_toast(window, cx, ToastKind::Success, toast);
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn render_count(&self) -> u32 {
        self.renders
    }

    /// The 7 labels and the shown value texts, in row order.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn rows_for_test(&self, _cx: &App) -> Vec<(SharedString, SharedString)> {
        (0..LABEL_KEYS.len())
            .map(|row| (self.labels[row].clone(), self.fields.rows[row].text.clone()))
            .collect()
    }

    /// Replaces the current user's name, so tests do not depend on `USER`.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_user_name_for_test(&mut self, name: Option<SharedString>, cx: &mut Context<Self>) {
        self.user_name = name;
        self.rebuild(cx);
        cx.notify();
    }
}

impl ServicePropsTabView for LogOnTab {
    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }

    fn apply(&mut self, _window: &mut Window, _cx: &mut App) -> SmallVec<[ActionId; 2]> {
        SmallVec::new()
    }

    fn reset(&mut self, _cx: &mut App) {}

    fn set_details(&mut self, details: Option<&Arc<UnitDetails>>, _not_loaded: bool, cx: &mut App) {
        let same = match (&self.details, details) {
            (Some(have), Some(new)) => Arc::ptr_eq(have, new),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.details = details.cloned();
        self.rebuild(cx);
        cx.notify(self.entity_id);
    }
}

impl Render for LogOnTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.renders += 1;
        }
        let m = *metrics(cx);
        let rows: Vec<TlKvRow> = self
            .fields
            .rows
            .iter()
            .enumerate()
            .map(|(i, field)| TlKvRow {
                label: self.labels[i].clone(),
                value: field.grid_value(),
            })
            .collect();
        let view = cx.weak_entity();
        let grid = TlKeyValueGrid::new("ServiceLogOnGrid", KvLayout::Inline)
            .row_height(px(ROW_HEIGHT * m.ui_scale))
            .label_width(px(LABEL_WIDTH * m.ui_scale))
            .sections(&[TlKvSection {
                title: None,
                blocks: vec![TlKvBlock {
                    heading: None,
                    rows,
                    indent: None,
                }],
            }])
            .on_context_menu(move |n, position, window, cx| {
                let _ = view.update(cx, |this, cx| this.open_value_menu(n, position, window, cx));
            });
        v_flex()
            .id("ServiceLogOnTab")
            .debug_selector(|| "ServiceLogOnTab".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(|_: &Copy, window, cx| {
                let text = TextSelection::selected_text(window, cx);
                let text = text.trim();
                if text.is_empty() {
                    cx.propagate();
                    return;
                }
                let _ = crate::util::clipboard_write(cx, text.to_owned());
            })
            .on_action(cx.listener(Self::on_copy_value))
            .size_full()
            .overflow_y_scroll()
            .child(grid)
    }
}
