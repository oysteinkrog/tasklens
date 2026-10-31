//! Settings window, Hosts page (UI-SHL-008, ADR-018 "Remote transport", tasklens-q4x76.28).
//!
//! One row per `[[hosts]]` entry: On switch, name, ssh destination, "Test connection", Move up,
//! Move down and Remove. Under the rows, a form adds a host. Enter or leaving a field applies a
//! change to the row's name or destination; a value that is not valid shows its reason under
//! the row and writes nothing. Every write goes through [`crate::hosts_edit`], and the main
//! window adds, removes and renames the host tabs from the settings change, with no restart.
//!
//! The page keeps its state in one entity that lives in a Global, because the Settings window
//! builds its pages again on every render.

use std::sync::Arc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, WeakEntity, Window, div,
    px,
};
use tl_model::settings::HostEntry;

use super::{PageBuilder, PageId};
use crate::app::AppEnv;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::input::{TlInputSize, TlTextInput};
use crate::components::switch::TlSwitch;
use crate::host::HostContext;
use crate::hosts_edit;
use crate::i18n::{tr, tr_args};
use crate::settings_global::TlSettings;
use crate::text::style::t_sm;
use crate::theme::{current, metrics};

const NAME_W: f32 = 140.0;
const TARGET_W: f32 = 220.0;

pub fn page(host: &HostContext, cx: &mut App) -> PageBuilder {
    let mut page = PageBuilder::new(host, PageId::Hosts);
    page.page_description(tr(cx, "settings-hosts-description"));
    page.block(
        tr(cx, "settings-hosts-group"),
        tr(cx, "settings-hosts-group"),
        |window, cx| editor(window, cx).into_any_element(),
    );
    page
}

/// The editor of the open Settings window, made on first use.
fn editor(window: &mut Window, cx: &mut App) -> Entity<HostsEditor> {
    if let Some(slot) = cx.try_global::<EditorSlot>() {
        return slot.0.clone();
    }
    let editor = cx.new(|cx| HostsEditor::new(window, cx));
    cx.set_global(EditorSlot(editor.clone()));
    editor
}

/// The editor entity, kept for the page's renders.
struct EditorSlot(Entity<HostsEditor>);

impl Global for EditorSlot {}

/// The editor of the open Settings window, for tests.
#[cfg(any(test, feature = "test-hooks"))]
pub fn open_editor(cx: &App) -> Option<Entity<HostsEditor>> {
    cx.try_global::<EditorSlot>().map(|s| s.0.clone())
}

/// What a row's "Test connection" shows.
#[derive(Clone, Debug, PartialEq)]
pub enum TestLine {
    None,
    Running,
    Done(Result<(), String>),
}

struct Row {
    on: bool,
    name: Entity<InputState>,
    target: Entity<InputState>,
    /// The entry the row last showed or wrote.
    synced: HostEntry,
    /// Why the last change was not applied.
    error: Option<SharedString>,
    test: TestLine,
    /// Counts tests, so a late answer of an older test is ignored.
    test_gen: u64,
    _subs: Vec<Subscription>,
}

/// The rows of Settings > Hosts and the add form.
pub struct HostsEditor {
    rows: Vec<Row>,
    add_name: Entity<InputState>,
    add_target: Entity<InputState>,
    add_error: Option<SharedString>,
    /// The add form is emptied on the next render.
    clear_add: bool,
    _subs: Vec<Subscription>,
}

impl HostsEditor {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let add_name = cx.new(|cx| InputState::new(window, cx));
        let add_target = cx.new(|cx| InputState::new(window, cx));
        let mut subs = vec![cx.observe_global::<TlSettings>(|_, cx| cx.notify())];
        // Enter in the add form adds the host.
        for input in [&add_name, &add_target] {
            subs.push(
                cx.subscribe_in(input, window, |this, _, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. }) {
                        this.add(cx);
                    }
                }),
            );
        }
        Self {
            rows: Vec::new(),
            add_name,
            add_target,
            add_error: None,
            clear_add: false,
            _subs: subs,
        }
    }

    /// Makes the rows match `[[hosts]]`. A row whose entry changed from outside takes the new
    /// text; a row whose entry is the same keeps what is typed in it.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = hosts_edit::hosts(cx);
        self.rows.truncate(list.len());
        for (i, entry) in list.iter().enumerate() {
            match self.rows.get_mut(i) {
                Some(row) => {
                    if row.synced != *entry {
                        if row.synced.target != entry.target {
                            row.test = TestLine::None;
                            row.test_gen += 1;
                        }
                        let (name, target) = (entry.name.clone(), entry.target.clone());
                        row.name.update(cx, |s, cx| s.set_value(name, window, cx));
                        row.target
                            .update(cx, |s, cx| s.set_value(target, window, cx));
                        row.synced = entry.clone();
                        row.error = None;
                    }
                    row.on = entry.enabled;
                }
                None => {
                    let row = self.new_row(i, entry, window, cx);
                    self.rows.push(row);
                }
            }
        }
    }

    fn new_row(
        &mut self,
        index: usize,
        entry: &HostEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Row {
        let name = cx.new(|cx| InputState::new(window, cx));
        let target = cx.new(|cx| InputState::new(window, cx));
        let (n, t) = (entry.name.clone(), entry.target.clone());
        name.update(cx, |s, cx| s.set_value(n, window, cx));
        target.update(cx, |s, cx| s.set_value(t, window, cx));
        // Enter or leaving the field applies the text.
        let mut subs = Vec::new();
        for input in [&name, &target] {
            subs.push(
                cx.subscribe_in(input, window, move |this, _, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.commit(index, cx);
                    }
                }),
            );
        }
        Row {
            on: entry.enabled,
            name,
            target,
            synced: entry.clone(),
            error: None,
            test: TestLine::None,
            test_gen: 0,
            _subs: subs,
        }
    }

    /// Applies the name and destination typed in row `index`.
    pub fn commit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        let name = row.name.read(cx).value().to_string();
        let target = row.target.read(cx).value().to_string();
        if name.trim() == row.synced.name && target.trim() == row.synced.target {
            row.error = None;
            return;
        }
        row.error = hosts_edit::edit(index, &name, &target, cx)
            .err()
            .map(|why| SharedString::from(why.to_string()));
        cx.notify();
    }

    /// Adds the host typed in the add form. A valid host clears the form.
    pub fn add(&mut self, cx: &mut Context<Self>) {
        let name = self.add_name.read(cx).value().to_string();
        let target = self.add_target.read(cx).value().to_string();
        match hosts_edit::add(&name, &target, cx) {
            Ok(()) => {
                self.add_error = None;
                self.clear_add = true;
            }
            Err(why) => self.add_error = Some(why.to_string().into()),
        }
        cx.notify();
    }

    /// Opens one connection to the typed destination of row `index` and shows the result.
    pub fn test(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        let target = row.target.read(cx).value().trim().to_owned();
        let Some(test) = cx
            .try_global::<AppEnv>()
            .and_then(|env| env.hosts.as_ref())
            .map(|h| Arc::clone(&h.test))
        else {
            row.test = TestLine::Done(Err(tr(cx, "settings-hosts-test-unavailable").to_string()));
            cx.notify();
            return;
        };
        row.test_gen += 1;
        let generation = row.test_gen;
        row.test = TestLine::Running;
        let rx = test(&target);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = rx
                .recv()
                .await
                .unwrap_or_else(|_| Err("the test ended without an answer".to_owned()));
            this.update(cx, |this, cx| {
                if let Some(row) = this.rows.get_mut(index)
                    && row.test_gen == generation
                {
                    row.test = TestLine::Done(result);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn test_text(&self, row: &Row, cx: &App) -> Option<(SharedString, bool)> {
        match &row.test {
            TestLine::None => None,
            TestLine::Running => Some((tr(cx, "settings-hosts-testing"), false)),
            TestLine::Done(Ok(())) => Some((tr(cx, "settings-hosts-test-ok"), false)),
            TestLine::Done(Err(why)) => Some((
                tr_args(
                    cx,
                    "settings-hosts-test-failed",
                    &[("reason", why.clone().into())],
                ),
                true,
            )),
        }
    }
}

/// Test hooks: the same calls the buttons and fields make.
#[cfg(any(test, feature = "test-hooks"))]
impl HostsEditor {
    /// Types `text` in row `index`'s name field.
    pub fn type_name(
        &mut self,
        index: usize,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.rows[index].name.clone();
        input.update(cx, |s, cx| s.set_value(text.to_owned(), window, cx));
    }

    /// Types `text` in row `index`'s destination field.
    pub fn type_target(
        &mut self,
        index: usize,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.rows[index].target.clone();
        input.update(cx, |s, cx| s.set_value(text.to_owned(), window, cx));
    }

    /// Types in the add form.
    pub fn type_add(
        &mut self,
        name: &str,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (n, t) = (self.add_name.clone(), self.add_target.clone());
        n.update(cx, |s, cx| s.set_value(name.to_owned(), window, cx));
        t.update(cx, |s, cx| s.set_value(target.to_owned(), window, cx));
    }

    pub fn press_remove(&mut self, index: usize, cx: &mut Context<Self>) {
        hosts_edit::remove(index, cx);
    }

    pub fn press_move(&mut self, index: usize, up: bool, cx: &mut Context<Self>) {
        hosts_edit::move_entry(index, up, cx);
    }

    pub fn press_enabled(&mut self, index: usize, on: bool, cx: &mut Context<Self>) {
        hosts_edit::set_enabled(index, on, cx);
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The text of row `index`'s name and destination fields.
    pub fn row_text(&self, index: usize, cx: &App) -> (String, String) {
        let row = &self.rows[index];
        (
            row.name.read(cx).value().to_string(),
            row.target.read(cx).value().to_string(),
        )
    }

    pub fn row_error(&self, index: usize) -> Option<SharedString> {
        self.rows[index].error.clone()
    }

    pub fn add_error(&self) -> Option<SharedString> {
        self.add_error.clone()
    }

    pub fn test_line(&self, index: usize) -> TestLine {
        self.rows[index].test.clone()
    }

    /// The text of the add form.
    pub fn add_text(&self, cx: &App) -> (String, String) {
        (
            self.add_name.read(cx).value().to_string(),
            self.add_target.read(cx).value().to_string(),
        )
    }
}

impl Render for HostsEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        if self.clear_add {
            self.clear_add = false;
            for input in [self.add_name.clone(), self.add_target.clone()] {
                input.update(cx, |s, cx| s.set_value(String::new(), window, cx));
            }
        }
        let m = *metrics(cx);
        let s = m.ui_scale;
        let theme = current(cx).clone();
        let mut list = div()
            .debug_selector(|| "HostsEditor".into())
            .flex()
            .flex_col()
            .gap(px(8. * s))
            .w_full();
        if self.rows.is_empty() {
            let mut empty = div()
                .text_color(theme.text_secondary)
                .child(tr(cx, "settings-hosts-empty"));
            *empty.text_style() = t_sm(&m);
            list = list.child(empty);
        }
        let last = self.rows.len().saturating_sub(1);
        for (i, row) in self.rows.iter().enumerate() {
            let editor = cx.weak_entity();
            let button = |id: &'static str, label: SharedString, disabled: bool| {
                TlButton::new(
                    SharedString::from(format!("Host{id}{i}")),
                    TlButtonKind::Secondary,
                )
                .size(TlButtonSize::Sm)
                .label(label)
                .disabled(disabled)
            };
            let line = |text: SharedString, bad: bool| {
                let mut el = div()
                    .text_color(if bad {
                        theme.status.danger
                    } else {
                        theme.text_secondary
                    })
                    .child(text);
                *el.text_style() = t_sm(&m);
                el
            };
            let mut block = div().flex().flex_col().gap(px(4. * s)).child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8. * s))
                    .child(
                        TlSwitch::new(SharedString::from(format!("HostOn{i}")))
                            .checked(row.on)
                            .label(tr(cx, "settings-hosts-enabled"))
                            .on_change(move |on, _, cx| hosts_edit::set_enabled(i, on, cx)),
                    )
                    .child(
                        TlTextInput::new(SharedString::from(format!("HostName{i}")), &row.name)
                            .size(TlInputSize::Sm)
                            .width(px(NAME_W * s))
                            .invalid(row.error.is_some())
                            .aria_label(tr(cx, "settings-hosts-name")),
                    )
                    .child(
                        TlTextInput::new(SharedString::from(format!("HostTarget{i}")), &row.target)
                            .size(TlInputSize::Sm)
                            .width(px(TARGET_W * s))
                            .placeholder(tr(cx, "settings-hosts-target-placeholder"))
                            .invalid(row.error.is_some())
                            .aria_label(tr(cx, "settings-hosts-target")),
                    )
                    .child(
                        button("Test", tr(cx, "settings-hosts-test"), false).on_click(
                            move |_, _, cx| {
                                editor.update(cx, |e, cx| e.test(i, cx)).ok();
                            },
                        ),
                    )
                    .child(
                        button("Up", tr(cx, "settings-hosts-up"), i == 0)
                            .on_click(move |_, _, cx| hosts_edit::move_entry(i, true, cx)),
                    )
                    .child(
                        button("Down", tr(cx, "settings-hosts-down"), i == last)
                            .on_click(move |_, _, cx| hosts_edit::move_entry(i, false, cx)),
                    )
                    .child(
                        button("Remove", tr(cx, "settings-hosts-remove"), false)
                            .on_click(move |_, _, cx| hosts_edit::remove(i, cx)),
                    ),
            );
            if let Some(why) = &row.error {
                block = block.child(line(why.clone(), true));
            }
            if let Some((text, bad)) = self.test_text(row, cx) {
                block = block.child(line(text, bad));
            }
            list = list.child(block);
        }
        let editor = cx.weak_entity();
        let mut form = div().flex().flex_col().gap(px(4. * s)).child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8. * s))
                .child(
                    TlTextInput::new(SharedString::from("HostAddName"), &self.add_name)
                        .size(TlInputSize::Sm)
                        .width(px(NAME_W * s))
                        .placeholder(tr(cx, "settings-hosts-name"))
                        .aria_label(tr(cx, "settings-hosts-name")),
                )
                .child(
                    TlTextInput::new(SharedString::from("HostAddTarget"), &self.add_target)
                        .size(TlInputSize::Sm)
                        .width(px(TARGET_W * s))
                        .placeholder(tr(cx, "settings-hosts-target-placeholder"))
                        .aria_label(tr(cx, "settings-hosts-target")),
                )
                .child(
                    TlButton::new(SharedString::from("HostAdd"), TlButtonKind::Primary)
                        .size(TlButtonSize::Sm)
                        .label(tr(cx, "settings-hosts-add"))
                        .on_click(move |_, _, cx| {
                            editor.update(cx, |e, cx| e.add(cx)).ok();
                        }),
                ),
        );
        if let Some(why) = &self.add_error {
            let mut el = div().text_color(theme.status.danger).child(why.clone());
            *el.text_style() = t_sm(&m);
            form = form.child(el);
        }
        list.child(form)
    }
}
