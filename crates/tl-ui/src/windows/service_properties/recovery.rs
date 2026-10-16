//! Service Properties Recovery tab, P2 view (UI-SYS-033, SYS-013).
//!
//! The restart and failure settings of a unit, read-only. Six rows are `TlKeyValueGrid` rows.
//! "Run on failure" holds one link per `OnFailure=` unit, which a grid row cannot hold, so it
//! is drawn between two grids that share the label column. The window pushes each new details
//! Arc in through `set_details`; the tab formats the values then and notifies itself, because
//! the window draws it through `.cached()`. The tab is never dirty.

use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::base::TextSelection;
use gpui_kit::base::input::Copy;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, App, Context, EntityId, FocusHandle, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use smallvec::SmallVec;
use tl_model::actions::ActionId;
use tl_model::snapshot::{RecoverySettings, UnitDetails};
use tl_model::table::UnitRef;
use tl_model::units;
use tl_model::validity::Validity;
use tracing::warn;

use super::{ServicePropertiesWindow, ServicePropsTab, ServicePropsTabView};
use crate::components::kv_grid::{
    KEY_CONTEXT, KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue,
    UnsupportedWord,
};
use crate::components::link::TlLink;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::{tr, tr_args};
use crate::text::style::{t_base, t_sm};
use crate::theme::{current, metrics};

/// Label column width at 100% scale.
const LABEL_WIDTH: f32 = 200.;
/// Row height at 100% scale.
const ROW_HEIGHT: f32 = 28.;

/// Row index of "Run on failure".
const LINKS_ROW: usize = 5;

/// Row label keys, in row order.
const LABEL_KEYS: [&str; 7] = [
    "services-sys033-label-on-failure",
    "services-sys033-label-restart-after",
    "services-sys033-label-give-up-after",
    "services-sys033-label-when-giving-up",
    "services-sys033-label-failure-action",
    "services-sys033-label-run-on-failure",
    "services-sys033-label-restarts",
];

gpui_kit::actions!(tasklens, [ServiceRecoveryCopyValue]);

/// Opens (or focuses) the Service Properties of an `OnFailure=` unit.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct ServiceRecoveryOpenUnit {
    pub name: SharedString,
}

/// How a value reads (R-050). "None" is a real empty value and reads as a value.
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

    /// A text field, or "Not reported" when the word is missing or empty.
    fn word(&self, text: Option<&Arc<str>>) -> Field {
        match text.filter(|t| !t.is_empty()) {
            Some(t) => Field::value(t.to_string()),
            None => self.not_reported(),
        }
    }
}

/// A duration field systemd reports as `u64::MAX` for "infinity" or "unset" is not a value.
fn known(v: Option<u64>) -> Option<u64> {
    v.filter(|&v| v != u64::MAX)
}

fn restart_after(r: &RecoverySettings, w: &Words, cx: &App) -> Field {
    let Some(usec) = known(r.restart_usec) else {
        return w.not_reported();
    };
    let ms = usec / 1000;
    let steps = r.restart_steps.filter(|&s| s > 0);
    match (steps, known(r.restart_max_delay_usec)) {
        (Some(steps), Some(max)) => Field::value(tr_args(
            cx,
            "services-sys033-restart-steps",
            &[
                ("ms", FluentValue::from(ms)),
                ("max", FluentValue::from(max / 1000)),
                ("steps", FluentValue::from(i64::from(steps))),
            ],
        )),
        _ => Field::value(tr_args(
            cx,
            "services-sys033-restart-ms",
            &[("ms", FluentValue::from(ms))],
        )),
    }
}

fn give_up_after(r: &RecoverySettings, w: &Words, cx: &App) -> Field {
    match (r.start_limit_burst, known(r.start_limit_interval_usec)) {
        (Some(burst), Some(usec)) => Field::value(tr_args(
            cx,
            "services-sys033-give-up",
            &[
                ("burst", FluentValue::from(i64::from(burst))),
                ("secs", FluentValue::from(usec / 1_000_000)),
            ],
        )),
        _ => w.not_reported(),
    }
}

/// The formatted values, rebuilt only when the details Arc changes.
struct Fields {
    rows: [Field; 7],
    /// The `OnFailure=` unit names, one link each.
    links: Vec<SharedString>,
}

impl Fields {
    fn collecting(w: &Words) -> Self {
        Self {
            rows: std::array::from_fn(|_| w.collecting()),
            links: Vec::new(),
        }
    }

    fn new(d: &UnitDetails, w: &Words, cx: &App) -> Self {
        let r = &d.recovery;
        let links: Vec<SharedString> = r
            .on_failure
            .iter()
            .map(|n| SharedString::from(n.to_string()))
            .collect();
        let run_on_failure = if links.is_empty() {
            Field::value(tr(cx, "services-sys033-none"))
        } else {
            let names: Vec<&str> = links.iter().map(|n| &**n).collect();
            Field::value(names.join(", "))
        };
        let restarts = match r.n_restarts {
            Some(n) => {
                let mut text = String::new();
                units::count(&mut text, u64::from(n));
                Field::value(text)
            }
            None => w.not_reported(),
        };
        Self {
            rows: [
                w.word(r.restart.as_ref()),
                restart_after(r, w, cx),
                give_up_after(r, w, cx),
                w.word(r.start_limit_action.as_ref()),
                w.word(r.failure_action.as_ref()),
                run_on_failure,
                restarts,
            ],
            links,
        }
    }
}

/// A toast in `window`'s own stack.
fn window_toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack for the service Recovery tab"),
    }
}

pub struct RecoveryTab {
    entity_id: EntityId,
    details: Option<Arc<UnitDetails>>,
    words: Words,
    labels: [SharedString; 7],
    note: SharedString,
    fields: Fields,
    /// The text "Copy value" copies.
    menu_value: Option<SharedString>,
    focus: FocusHandle,
    #[cfg(any(test, feature = "test-hooks"))]
    renders: u32,
}

impl RecoveryTab {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let words = Words::new(cx);
        let labels = LABEL_KEYS.map(|key| tr(cx, key));
        let note = tr(cx, "services-sys033-note");
        let fields = Fields::collecting(&words);
        Self {
            entity_id: cx.entity_id(),
            details: None,
            words,
            labels,
            note,
            fields,
            menu_value: None,
            focus: cx.focus_handle(),
            #[cfg(any(test, feature = "test-hooks"))]
            renders: 0,
        }
    }

    fn rebuild(&mut self, cx: &App) {
        self.fields = match &self.details {
            Some(d) => Fields::new(d, &self.words, cx),
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
        let label = tr(cx, "services-sys033-copy-value");
        let menu = TlMenu::build(window, cx, self.focus.clone(), |menu, _, _| {
            menu.item(label, Box::new(ServiceRecoveryCopyValue))
        });
        open_context_menu_at(position, window, cx, menu);
    }

    fn on_copy_value(
        &mut self,
        _: &ServiceRecoveryCopyValue,
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

    fn on_open_unit(
        &mut self,
        action: &ServiceRecoveryOpenUnit,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(details) = &self.details else {
            return;
        };
        let unit = UnitRef {
            manager: details.manager,
            name: Arc::from(&*action.name),
        };
        // Defer: opening or focusing a window must not run inside this window's update.
        cx.defer(move |cx| ServicePropertiesWindow::open(unit, ServicePropsTab::General, cx));
    }

    /// A grid of rows `first..first + n` of the 7.
    fn grid(
        &self,
        id: &'static str,
        first: usize,
        n: usize,
        cx: &mut Context<Self>,
    ) -> TlKeyValueGrid {
        let m = *metrics(cx);
        let rows: Vec<TlKvRow> = (first..first + n)
            .map(|i| TlKvRow {
                label: self.labels[i].clone(),
                value: self.fields.rows[i].grid_value(),
            })
            .collect();
        let view = cx.weak_entity();
        TlKeyValueGrid::new(id, KvLayout::Inline)
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
            .on_context_menu(move |i, position, window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.open_value_menu(first + i, position, window, cx)
                });
            })
    }

    /// The "Run on failure" row: the label, then one link per unit or the "None" word.
    fn links_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let label_width = px(LABEL_WIDTH * m.ui_scale);
        let row_height = px(ROW_HEIGHT * m.ui_scale);
        let mut label = div()
            .flex_none()
            .w(label_width)
            .h(row_height)
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(theme.text_secondary)
            .child(self.labels[LINKS_ROW].clone());
        *label.text_style() = t_base(&m);
        let field = &self.fields.rows[LINKS_ROW];
        let value: AnyElement = if self.fields.links.is_empty() {
            let color = match field.word {
                Word::Value => theme.text_primary,
                Word::Collecting | Word::NotReported => theme.text_secondary,
            };
            div()
                .text_color(color)
                .child(field.text.clone())
                .into_any_element()
        } else {
            let items = self.fields.links.iter().enumerate().map(|(i, name)| {
                div()
                    .debug_selector(move || format!("ServiceRecovery-link-{i}"))
                    .child(TlLink::new(
                        ("svc-recovery-link", i),
                        name.clone(),
                        Box::new(ServiceRecoveryOpenUnit { name: name.clone() }),
                    ))
            });
            h_flex()
                .flex_wrap()
                .gap_x(px(8. * m.ui_scale))
                .children(items)
                .into_any_element()
        };
        let mut value = div()
            .id(("svc-recovery-value", LINKS_ROW))
            .flex_1()
            .min_w_0()
            .min_h(row_height)
            .flex()
            .items_center()
            .text_color(theme.text_primary)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_value_menu(LINKS_ROW, ev.position, window, cx);
                }),
            )
            .child(value);
        *value.text_style() = t_base(&m);
        h_flex()
            .debug_selector(|| "ServiceRecovery-row-5".into())
            .w_full()
            .items_start()
            .child(label)
            .child(value)
            .into_any_element()
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

    /// The note under the grid.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn note_for_test(&self) -> SharedString {
        self.note.clone()
    }
}

impl ServicePropsTabView for RecoveryTab {
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

impl Render for RecoveryTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.renders += 1;
        }
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let top = self.grid("ServiceRecoveryTop", 0, 5, cx);
        let links = self.links_row(cx);
        let bottom = self.grid("ServiceRecoveryBottom", 6, 1, cx);
        let mut note = div()
            .id("ServiceRecoveryNote")
            .debug_selector(|| "ServiceRecoveryNote".into())
            .pt(px(8. * m.ui_scale))
            .text_color(theme.text_secondary)
            .child(self.note.clone());
        *note.text_style() = t_sm(&m);
        v_flex()
            .id("ServiceRecoveryTab")
            .debug_selector(|| "ServiceRecoveryTab".into())
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
            .on_action(cx.listener(Self::on_open_unit))
            .size_full()
            .overflow_y_scroll()
            .child(top)
            .child(links)
            .child(bottom)
            .child(note)
    }
}
