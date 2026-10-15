//! Service Properties General tab (UI-SYS-031, SYS-011, SYS-006, SYS-007, R-050).
//!
//! Thirteen rows in a fixed order. Name and Display, and Main PID to Manager, are two
//! `TlKeyValueGrid`s; the rows between them hold links, the Startup type combo and the Start,
//! Stop and Restart buttons, so they are drawn here with the same label column. The window
//! pushes each new details Arc in through `set_details`; the tab formats the values then and
//! only then, and notifies itself, because the window draws it through `.cached()`.
//!
//! Paths and command lines are shown, never logged (ADR-015).

use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::base::input::Copy;
use gpui_kit::base::{SelectableText, TextSelection};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EntityId,
    FocusHandle, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, Point, Render, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionTarget};
use tl_model::snapshot::{ServiceStatus, StartupType, UnitDetails};
use tl_model::table::{Manager, UnitRef};
use tl_model::units::{self, LocalDateTime};
use tl_model::validity::Validity;
use tracing::warn;

use super::ServicePropsTabView;
use crate::action_tracker::{ActionTrackerEvent, GlobalActionTracker};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::kv_grid::{
    KEY_CONTEXT, KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue,
    UnsupportedWord,
};
use crate::components::link::TlLink;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::select::{TlSelect, TlSelectItem, TlSelectState};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::hub::GlobalHub;
use crate::i18n::{tr, tr_args};
use crate::pages::services::actions::StartupChoice;
use crate::pages::services::copy::status_word;
use crate::pages::services::flows::{
    UnitVerb, file_uri, run_jump_to_process, run_open_uri, run_unit_verb, startup_texts,
    submit_set_startup,
};
use crate::pages::sys_status::{
    DotStyle, StatusLook, SysStatus, cell_colors, status_label, status_look,
};
use crate::pages::sys_table::{ActionKindTag, ActionTexts, SysActions, SysActionsEvent};
use crate::text::style::t_base;
use crate::theme::{TlTheme, UiMetrics, current, metrics};
use crate::window::main::ShellState;

/// Label column width at 100% scale.
const LABEL_WIDTH: f32 = 120.;
/// Row height at 100% scale.
const ROW_HEIGHT: f32 = 28.;
/// Startup combo width at 100% scale.
const COMBO_WIDTH: f32 = 200.;
/// Most lines the Path value wraps to.
const PATH_LINES: usize = 3;

/// Row label keys, in row order.
const LABEL_KEYS: [&str; 13] = [
    "services-sys031-label-name",
    "services-sys031-label-display",
    "services-sys031-label-desc",
    "services-sys031-label-path",
    "services-sys031-label-unit-file",
    "services-sys031-label-drop-ins",
    "services-sys031-label-startup",
    "services-sys031-label-status",
    "services-sys031-label-main-pid",
    "services-sys031-label-memory",
    "services-sys031-label-tasks",
    "services-sys031-label-cpu-time",
    "services-sys031-label-manager",
];

/// Combo item values.
const ENABLED: &str = "enabled";
const DISABLED: &str = "disabled";
const MASKED: &str = "masked";
/// The one item of a combo that cannot change: the current word.
const CURRENT: &str = "current";

gpui_kit::actions!(
    tasklens,
    [ServiceGeneralCopyValue, ServiceGeneralJumpToProcess]
);

/// A documentation, unit file or drop-in link. `unit_file` picks the failure text.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct ServiceGeneralOpenLink {
    pub uri: SharedString,
    pub unit_file: bool,
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

/// One entry of a link row. `link` is `None` for an entry drawn as plain text.
#[derive(Clone, Debug)]
struct LinkItem {
    text: SharedString,
    link: Option<ServiceGeneralOpenLink>,
}

/// The Desc, Unit file and Drop-ins values: links, or `empty` when there are none.
#[derive(Clone, Debug)]
struct LinkList {
    items: Vec<LinkItem>,
    empty: Field,
}

impl LinkList {
    fn text(&self, separator: &str) -> SharedString {
        if self.items.is_empty() {
            return self.empty.text.clone();
        }
        let parts: Vec<&str> = self.items.iter().map(|i| i.text.as_ref()).collect();
        parts.join(separator).into()
    }
}

/// The translated words the values use.
struct Words {
    collecting: SharedString,
    not_reported: SharedString,
    none: SharedString,
}

impl Words {
    fn new(cx: &App) -> Self {
        Self {
            collecting: tr(cx, "fnd-kv-collecting"),
            not_reported: tr(cx, "fnd-kv-not-reported"),
            none: tr(cx, "services-sys031-none"),
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

    fn none(&self) -> Field {
        Field::value(self.none.clone())
    }
}

/// The formatted values, rebuilt only when the details Arc changes.
struct Fields {
    name: Field,
    display: Field,
    desc: LinkList,
    path: Field,
    unit_file: LinkList,
    drop_ins: LinkList,
    /// `None` until the details arrive.
    status: Option<SysStatus>,
    status_text: Field,
    /// Start is on for an inactive unit, Stop and Restart for an active one.
    active: bool,
    main_pid: Field,
    has_main_pid: bool,
    memory: Field,
    tasks: Field,
    cpu: Field,
    manager: Field,
}

impl Fields {
    fn collecting(w: &Words) -> Self {
        let list = || LinkList {
            items: Vec::new(),
            empty: w.collecting(),
        };
        Self {
            name: w.collecting(),
            display: w.collecting(),
            desc: list(),
            path: w.collecting(),
            unit_file: list(),
            drop_ins: list(),
            status: None,
            status_text: w.collecting(),
            active: false,
            main_pid: w.collecting(),
            has_main_pid: false,
            memory: w.collecting(),
            tasks: w.collecting(),
            cpu: w.collecting(),
            manager: w.collecting(),
        }
    }

    fn new(d: &UnitDetails, w: &Words, cx: &mut App) -> Self {
        let desc = LinkList {
            items: d
                .documentation
                .iter()
                .map(|entry| LinkItem {
                    text: SharedString::from(entry.to_string()),
                    link: doc_uri(entry).then(|| ServiceGeneralOpenLink {
                        uri: SharedString::from(entry.to_string()),
                        unit_file: false,
                    }),
                })
                .collect(),
            empty: w.not_reported(),
        };
        let file_link = |path: &str| LinkItem {
            text: SharedString::from(path.to_owned()),
            link: Some(ServiceGeneralOpenLink {
                uri: file_uri(path).into(),
                unit_file: true,
            }),
        };
        let unit_file = LinkList {
            items: d
                .fragment_path
                .as_deref()
                .filter(|p| !p.is_empty())
                .map(file_link)
                .into_iter()
                .collect(),
            empty: w.none(),
        };
        let drop_ins = LinkList {
            items: d.drop_in_paths.iter().map(|p| file_link(p)).collect(),
            empty: w.none(),
        };
        let path = if d.exec_start_argv.is_empty() {
            w.none()
        } else {
            let parts: Vec<&str> = d.exec_start_argv.iter().map(|a| a.as_ref()).collect();
            Field::value(parts.join(" "))
        };

        let service_status = ServiceStatus::from_states(&d.active_state, &d.sub_state);
        let status = status_word(service_status);
        let label = status_label(cx, status);
        let since = d
            .active_enter_timestamp_us
            .filter(|_| service_status != ServiceStatus::Stopped)
            .and_then(|us| local_time(us / 1_000_000));
        let mut args = vec![
            ("status", FluentValue::from(label.to_string())),
            ("sub", FluentValue::from(d.sub_state.to_string())),
        ];
        let text = match since {
            Some(since) => {
                args.push(("since", FluentValue::from(since)));
                tr_args(cx, "services-sys031-status-since", &args)
            }
            None => tr_args(cx, "services-sys031-status", &args),
        };
        let active = matches!(
            service_status,
            ServiceStatus::Running | ServiceStatus::Active | ServiceStatus::Reloading
        );

        let main_pid = if d.main_pid == 0 {
            w.none()
        } else {
            Field::value(d.main_pid.to_string())
        };
        let memory = match d.memory_current {
            Some(n) => {
                let mut out = String::new();
                units::bytes(&mut out, n);
                Field::value(out)
            }
            None => w.not_reported(),
        };
        let tasks = match d.tasks_current {
            Some(n) => Field::value(n.to_string()),
            None => w.not_reported(),
        };
        let cpu = match d.cpu_usage_nsec {
            Some(ns) => {
                let mut out = String::new();
                units::cpu_time(&mut out, ns / 1_000_000_000);
                Field::value(out)
            }
            None => w.not_reported(),
        };
        let manager = Field::value(tr(
            cx,
            match d.manager {
                Manager::System => "services-sys031-manager-system",
                Manager::User => "services-sys031-manager-user",
            },
        ));

        Self {
            name: Field::value(d.id.to_string()),
            display: Field::value(d.description.to_string()),
            desc,
            path,
            unit_file,
            drop_ins,
            status: Some(status),
            status_text: Field::value(text),
            active,
            main_pid,
            has_main_pid: d.main_pid != 0,
            memory,
            tasks,
            cpu,
            manager,
        }
    }
}

/// `man:`, `http:` and `https:` entries are links; other schemes are plain text.
fn doc_uri(entry: &str) -> bool {
    ["man:", "http:", "https:"]
        .iter()
        .any(|scheme| entry.starts_with(scheme))
}

/// "YYYY-MM-DD HH:MM:SS" in the system time zone.
fn local_time(unix_s: u64) -> Option<String> {
    let secs = i64::try_from(unix_s).ok()?;
    let z = jiff::Timestamp::from_second(secs)
        .ok()?
        .to_zoned(jiff::tz::TimeZone::system());
    let mut out = String::new();
    units::date_time(
        &mut out,
        LocalDateTime {
            year: i32::from(z.year()),
            month: z.month() as u8,
            day: z.day() as u8,
            hour: z.hour() as u8,
            minute: z.minute() as u8,
            second: z.second() as u8,
        },
    );
    Some(out)
}

fn choice_value(choice: &StartupChoice) -> &'static str {
    match choice {
        StartupChoice::Enabled => ENABLED,
        StartupChoice::Disabled => DISABLED,
        StartupChoice::Masked => MASKED,
    }
}

fn parse_choice(value: &str) -> Option<StartupChoice> {
    match value {
        ENABLED => Some(StartupChoice::Enabled),
        DISABLED => Some(StartupChoice::Disabled),
        MASKED => Some(StartupChoice::Masked),
        _ => None,
    }
}

/// The combo value of a startup type. "Enabled (runtime)" shows "Enabled". Alias and Linked map
/// to none.
fn startup_choice(startup: StartupType) -> Option<StartupChoice> {
    match startup {
        StartupType::Enabled | StartupType::EnabledRuntime => Some(StartupChoice::Enabled),
        StartupType::Disabled => Some(StartupChoice::Disabled),
        StartupType::Masked => Some(StartupChoice::Masked),
        _ => None,
    }
}

fn startup_type(choice: &StartupChoice) -> StartupType {
    match choice {
        StartupChoice::Enabled => StartupType::Enabled,
        StartupChoice::Disabled => StartupType::Disabled,
        StartupChoice::Masked => StartupType::Masked,
    }
}

/// The word of a startup type that has no install section.
fn locked_word_key(startup: StartupType) -> Option<&'static str> {
    match startup {
        StartupType::Static => Some("services-sys031-startup-static"),
        StartupType::Indirect => Some("services-sys031-startup-indirect"),
        StartupType::Generated => Some("services-sys031-startup-generated"),
        StartupType::Transient => Some("services-sys031-startup-transient"),
        _ => None,
    }
}

/// The overlay key of a pending Start, Stop, Restart or Reload.
fn pending_key(tag: ActionKindTag) -> Option<&'static str> {
    match tag {
        ActionKindTag::Start => Some("services-sys031-starting"),
        ActionKindTag::Stop => Some("services-sys031-stopping"),
        ActionKindTag::Restart => Some("services-sys031-restarting"),
        ActionKindTag::Reload => Some("services-sys031-reloading"),
        _ => None,
    }
}

/// The Startup combo: what it lists, what it shows and whether it can change.
struct ComboState {
    items: Vec<TlSelectItem>,
    selected: &'static str,
    enabled: bool,
    tooltip: Option<SharedString>,
}

/// Value text the user can select, as the grid draws its values. `t_base`.
fn selectable(id: (&'static str, usize), text: SharedString, m: &UiMetrics) -> SelectableText {
    SelectableText::new(id, text).text_style(t_base(m))
}

/// The Status value: the dot and label of `status_cell` (UI-SYS-003), with selectable text.
fn status_value(
    look: StatusLook,
    text: SharedString,
    theme: &TlTheme,
    m: &UiMetrics,
) -> AnyElement {
    let colors = cell_colors(look, false, false, theme);
    let dot = colors.dot.map(|color| {
        let d = div()
            .debug_selector(|| "sys-status-dot".into())
            .flex_none()
            .size(px(8.0 * m.ui_scale))
            .rounded_full();
        match look.dot {
            DotStyle::Ring(_) => d.border(px(1.5 * m.ui_scale)).border_color(color),
            _ => d.bg(color),
        }
    });
    let mut row = h_flex().min_w_0().items_center();
    if let Some(dot) = dot {
        row = row.gap(m.sp[3]).child(dot);
    }
    row.child(
        div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(colors.label)
            .child(selectable(("svc-general-text", 7), text, m)),
    )
    .into_any_element()
}

/// A toast in `window`'s own stack.
fn window_toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack for the service General tab"),
    }
}

pub struct GeneralTab {
    unit: UnitRef,
    actions: Entity<SysActions>,
    /// The window view: notified when the tab's dirty state changes, for the footer.
    parent: EntityId,
    entity_id: EntityId,
    details: Option<Arc<UnitDetails>>,
    not_loaded: bool,
    words: Words,
    labels: [SharedString; 13],
    fields: Fields,
    startup: Entity<TlSelectState>,
    /// The user's choice, while it differs from the current value.
    chosen: Option<StartupChoice>,
    /// The action and choice of the apply in flight. The combo is locked until it finishes.
    sent: Option<(ActionId, StartupChoice)>,
    /// The choice applied last, shown until the next details Arc.
    applied: Option<StartupChoice>,
    /// The text "Copy value" copies.
    menu_value: Option<SharedString>,
    focus: FocusHandle,
    #[cfg(any(test, feature = "test-hooks"))]
    renders: u32,
    _subs: Vec<Subscription>,
}

impl GeneralTab {
    pub(super) fn new(
        unit: UnitRef,
        actions: Entity<SysActions>,
        parent: EntityId,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = Words::new(cx);
        let labels = LABEL_KEYS.map(|key| tr(cx, key));
        let startup = cx.new(|cx| TlSelectState::new(Vec::new(), None, cx));
        let mut subs = vec![cx.subscribe(&actions, |this, _, ev: &SysActionsEvent, cx| {
            let SysActionsEvent::PendingChanged { target } = ev;
            if matches!(target, ActionTarget::Unit(u) if *u == this.unit) {
                cx.notify();
            }
        })];
        if let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) {
            subs.push(
                cx.subscribe(&tracker, |this, _, ev: &ActionTrackerEvent, cx| {
                    if let ActionTrackerEvent::Finished { id, failed, .. } = *ev {
                        this.on_apply_finished(id, failed, cx);
                    }
                }),
            );
        }
        let this = Self {
            unit,
            actions,
            parent,
            entity_id: cx.entity_id(),
            details: None,
            not_loaded: false,
            fields: Fields::collecting(&words),
            words,
            labels,
            startup,
            chosen: None,
            sent: None,
            applied: None,
            menu_value: None,
            focus: cx.focus_handle(),
            #[cfg(any(test, feature = "test-hooks"))]
            renders: 0,
            _subs: subs,
        };
        this.sync_combo(cx);
        this
    }

    /// The combo value the unit has now, or the one applied last.
    fn current_choice(&self) -> Option<StartupChoice> {
        self.applied
            .clone()
            .or_else(|| startup_choice(self.details.as_ref()?.startup))
    }

    fn combo_state(&self, cx: &App) -> ComboState {
        let three = || {
            vec![
                TlSelectItem::new(ENABLED, tr(cx, "services-sys019-enabled")),
                TlSelectItem::new(DISABLED, tr(cx, "services-sys019-disabled")),
                TlSelectItem::new(MASKED, tr(cx, "services-sys019-masked")),
            ]
        };
        let Some(details) = &self.details else {
            return ComboState {
                items: vec![TlSelectItem::new(CURRENT, self.words.collecting.clone())],
                selected: CURRENT,
                enabled: false,
                tooltip: None,
            };
        };
        if let Some(key) = locked_word_key(details.startup) {
            return ComboState {
                items: vec![TlSelectItem::new(CURRENT, tr(cx, key))],
                selected: CURRENT,
                enabled: false,
                tooltip: Some(tr(cx, "services-sys019-no-install-section")),
            };
        }
        if details.startup == StartupType::NoUnitFile {
            return ComboState {
                items: three(),
                selected: "",
                enabled: false,
                tooltip: Some(tr(cx, "services-sys019-no-unit-file")),
            };
        }
        let shown = self.chosen.clone().or_else(|| self.current_choice());
        ComboState {
            items: three(),
            selected: shown.as_ref().map_or("", choice_value),
            enabled: !self.not_loaded && self.sent.is_none(),
            tooltip: None,
        }
    }

    fn sync_combo(&self, cx: &mut App) {
        let state = self.combo_state(cx);
        self.startup.update(cx, |s, cx| {
            if s.items() != state.items.as_slice() {
                s.set_items(state.items, cx);
            }
            s.set_selected_value(state.selected, cx);
        });
    }

    fn choose(&mut self, value: &str, cx: &mut Context<Self>) {
        // The combo is locked while an apply runs: put back the shown value.
        if self.sent.is_some() {
            self.sync_combo(cx);
            return;
        }
        let Some(choice) = parse_choice(value) else {
            return;
        };
        self.chosen = (self.current_choice().as_ref() != Some(&choice)).then_some(choice);
        cx.notify();
        App::notify(cx, self.parent);
    }

    /// The apply in flight finished. Success shows the applied value; failure keeps the choice
    /// so the tab stays dirty. Either way the combo unlocks.
    fn on_apply_finished(&mut self, id: ActionId, failed: u16, cx: &mut Context<Self>) {
        if self.sent.as_ref().is_none_or(|(sent, _)| *sent != id) {
            return;
        }
        let Some((_, choice)) = self.sent.take() else {
            return;
        };
        if failed == 0 {
            self.applied = Some(choice);
            self.chosen = None;
        }
        self.sync_combo(cx);
        cx.notify();
        App::notify(cx, self.parent);
    }

    /// The Status text: the pending overlay label while a Start, Stop, Restart or Reload runs.
    fn status_text(&self, cx: &App) -> SharedString {
        let pending = self
            .actions
            .read(cx)
            .pending_tag(&ActionTarget::Unit(self.unit.clone()));
        match pending.and_then(pending_key) {
            Some(key) if self.details.is_some() => tr(cx, key),
            _ => self.fields.status_text.text.clone(),
        }
    }

    /// Start, Stop and Restart enablement.
    fn buttons_enabled(&self, cx: &App) -> [bool; 3] {
        let pending = self
            .actions
            .read(cx)
            .pending_tag(&ActionTarget::Unit(self.unit.clone()))
            .is_some();
        let idle = !self.not_loaded && !pending && self.details.is_some();
        let active = self.fields.active;
        [idle && !active, idle && active, idle && active]
    }

    /// The shown text of row `row`, in `LABEL_KEYS` order, as "Copy value" copies it.
    fn value_text(&self, row: usize, cx: &App) -> SharedString {
        let f = &self.fields;
        match row {
            0 => f.name.text.clone(),
            1 => f.display.text.clone(),
            2 => f.desc.text("  "),
            3 => f.path.text.clone(),
            4 => f.unit_file.text(""),
            5 => f.drop_ins.text("\n"),
            6 => {
                let s = self.startup.read(cx);
                s.selected_index()
                    .map(|ix| s.items()[ix].label.clone())
                    .unwrap_or_default()
            }
            7 => self.status_text(cx),
            8 => f.main_pid.text.clone(),
            9 => f.memory.text.clone(),
            10 => f.tasks.text.clone(),
            11 => f.cpu.text.clone(),
            _ => f.manager.text.clone(),
        }
    }

    fn open_value_menu(
        &mut self,
        row: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_value = Some(self.value_text(row, cx));
        let label = tr(cx, "services-sys031-copy-value");
        let menu = TlMenu::build(window, cx, self.focus.clone(), |menu, _, _| {
            menu.item(label, Box::new(ServiceGeneralCopyValue))
        });
        open_context_menu_at(position, window, cx, menu);
    }

    fn on_copy_value(
        &mut self,
        _: &ServiceGeneralCopyValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.menu_value.take() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        let toast = tr(cx, "sys-sys006-copied-to-clipboard");
        window_toast(window, cx, ToastKind::Success, toast);
    }

    fn on_open_link(
        &mut self,
        link: &ServiceGeneralOpenLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let failed = if link.unit_file {
            "services-sys017-unit-file-failed"
        } else {
            "services-sys031-link-failed"
        };
        run_open_uri(link.uri.to_string(), failed, &self.actions, window, cx);
    }

    /// Main PID: the main window comes to the front and the Processes page selects the process
    /// (UI-SYS-021).
    fn on_jump(&mut self, _: &ServiceGeneralJumpToProcess, _: &mut Window, cx: &mut Context<Self>) {
        let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) else {
            return;
        };
        let snap = hub.read(cx).current().clone();
        let record = snap
            .services
            .units
            .iter()
            .find(|r| r.unit == self.unit)
            .cloned();
        let main = cx.try_global::<ShellState>().and_then(|s| s.main_window);
        let (Some(record), Some(main)) = (record, main) else {
            warn!(target: "tasklens::ui", "no unit row or main window for the Main PID link");
            return;
        };
        let _ = main.update(cx, |_, window, cx| {
            window.activate_window();
            run_jump_to_process(&record, &snap, window, cx);
        });
    }

    fn on_verb(&mut self, verb: UnitVerb, window: &mut Window, cx: &mut Context<Self>) {
        let units = [self.unit.clone()];
        let actions = self.actions.clone();
        run_unit_verb(verb, &units, &actions, window, cx);
    }

    /// A grid of rows `first..first + n` of the 13.
    fn grid(
        &self,
        id: &'static str,
        rows: Vec<TlKvRow>,
        first: usize,
        label_width: Pixels,
        row_height: Pixels,
        cx: &mut Context<Self>,
    ) -> TlKeyValueGrid {
        let view = cx.weak_entity();
        TlKeyValueGrid::new(id, KvLayout::Inline)
            .row_height(row_height)
            .label_width(label_width)
            .sections(&[TlKvSection {
                title: None,
                blocks: vec![TlKvBlock {
                    heading: None,
                    rows,
                    indent: None,
                }],
            }])
            .on_context_menu(move |n, position, window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.open_value_menu(first + n, position, window, cx)
                });
            })
    }

    /// A row drawn here: "Label:" in the label column, then `value`. Right-click on the value
    /// opens the value menu.
    fn custom_row(
        &self,
        row: usize,
        value: impl IntoElement,
        label_width: Pixels,
        row_height: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let mut label = div()
            .flex_none()
            .w(label_width)
            .h(row_height)
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(theme.text_secondary)
            .child(self.labels[row].clone());
        *label.text_style() = t_base(&m);
        let mut value = div()
            .id(("svc-general-value", row))
            .flex_1()
            .min_w_0()
            .min_h(row_height)
            .flex()
            .items_center()
            .text_color(theme.text_primary)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.focus.focus(window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_value_menu(row, ev.position, window, cx);
                }),
            )
            .child(value);
        *value.text_style() = t_base(&m);
        h_flex()
            .debug_selector(move || format!("ServiceGeneral-row-{row}"))
            .w_full()
            .items_start()
            .child(label)
            .child(value)
            .into_any_element()
    }

    /// The links of a link row, `separator` apart, or the row's empty word.
    fn links(&self, row: usize, list: &LinkList, stacked: bool, cx: &App) -> AnyElement {
        let theme = current(cx);
        let m = metrics(cx);
        if list.items.is_empty() {
            let color = match list.empty.word {
                Word::Value => theme.text_primary,
                Word::Collecting | Word::NotReported => theme.text_secondary,
            };
            return div()
                .text_color(color)
                .child(selectable(
                    ("svc-general-text", row * 100),
                    list.empty.text.clone(),
                    m,
                ))
                .into_any_element();
        }
        let items = list.items.iter().enumerate().map(|(i, item)| {
            let entry = div().debug_selector(move || format!("ServiceGeneral-link-{row}-{i}"));
            match &item.link {
                Some(link) => entry
                    .child(TlLink::new(
                        ("svc-general-link", row * 100 + i),
                        item.text.clone(),
                        Box::new(link.clone()),
                    ))
                    .into_any_element(),
                None => entry
                    .child(selectable(
                        ("svc-general-text", row * 100 + i),
                        item.text.clone(),
                        m,
                    ))
                    .into_any_element(),
            }
        });
        if stacked {
            v_flex().children(items).into_any_element()
        } else {
            // `sp3` is about the width of two spaces at `t_base`.
            h_flex()
                .flex_wrap()
                .gap(m.sp[3])
                .children(items)
                .into_any_element()
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn render_count(&self) -> u32 {
        self.renders
    }

    /// The 13 labels and the shown value texts, in row order.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn rows_for_test(&self, cx: &App) -> Vec<(SharedString, SharedString)> {
        (0..LABEL_KEYS.len())
            .map(|row| (self.labels[row].clone(), self.value_text(row, cx)))
            .collect()
    }

    /// Whether the combo can change, and its tooltip.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn combo_for_test(&self, cx: &App) -> (bool, Option<SharedString>) {
        let state = self.combo_state(cx);
        (state.enabled, state.tooltip)
    }

    /// Picks a combo value as the user would.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn choose_for_test(&mut self, value: &str, cx: &mut Context<Self>) {
        self.startup
            .update(cx, |s, cx| s.set_selected_value(value, cx));
        self.choose(value, cx);
    }

    /// Start, Stop and Restart enablement.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn buttons_for_test(&self, cx: &App) -> [bool; 3] {
        self.buttons_enabled(cx)
    }
}

impl ServicePropsTabView for GeneralTab {
    fn is_dirty(&self, _cx: &App) -> bool {
        self.chosen
            .as_ref()
            .is_some_and(|c| self.current_choice().as_ref() != Some(c))
    }

    fn apply(&mut self, window: &mut Window, cx: &mut App) -> SmallVec<[ActionId; 2]> {
        let (Some(choice), Some(details)) = (self.chosen.clone(), self.details.clone()) else {
            return SmallVec::new();
        };
        let current = self.applied.as_ref().map_or(details.startup, startup_type);
        let texts = ActionTexts {
            done_one: tr(cx, "services-sys031-changes-applied"),
            ..startup_texts(cx)
        };
        let units = [(self.unit.clone(), current)];
        // A rejected submit (the unit is busy, say) returns no id. `chosen` stays, so the tab
        // stays dirty and the window keeps it open.
        let id = submit_set_startup(choice.clone(), &units, texts, &self.actions, window, cx);
        if let Some(id) = id {
            self.sent = Some((id, choice));
            cx.notify(self.entity_id);
        }
        id.into_iter().collect()
    }

    fn reset(&mut self, cx: &mut App) {
        if let Some((_, sent)) = self.sent.take() {
            self.applied = Some(sent);
        }
        self.chosen = None;
        self.sync_combo(cx);
        cx.notify(self.entity_id);
    }

    fn set_details(&mut self, details: Option<&Arc<UnitDetails>>, not_loaded: bool, cx: &mut App) {
        let same = match (&self.details, details) {
            (Some(have), Some(new)) => Arc::ptr_eq(have, new),
            (None, None) => true,
            _ => false,
        };
        if same && not_loaded == self.not_loaded {
            return;
        }
        if !same {
            self.details = details.cloned();
            self.applied = None;
            self.fields = match &self.details {
                Some(d) => Fields::new(d, &self.words, cx),
                None => Fields::collecting(&self.words),
            };
        }
        self.not_loaded = not_loaded;
        if self.chosen.is_none() {
            self.sync_combo(cx);
        }
        cx.notify(self.entity_id);
    }
}

impl Render for GeneralTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.renders += 1;
        }
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let label_width = px(LABEL_WIDTH * m.ui_scale);
        let row_height = px(ROW_HEIGHT * m.ui_scale);
        let f = &self.fields;
        let row = |i: usize, field: &Field| TlKvRow {
            label: self.labels[i].clone(),
            value: field.grid_value(),
        };

        let top = vec![row(0, &f.name), row(1, &f.display)];
        let mut pid = row(8, &f.main_pid);
        if f.has_main_pid {
            pid.value.link = Some(Box::new(ServiceGeneralJumpToProcess));
        }
        let bottom = vec![
            pid,
            row(9, &f.memory),
            row(10, &f.tasks),
            row(11, &f.cpu),
            row(12, &f.manager),
        ];

        let desc = self.links(2, &f.desc, false, cx);
        let mut path = div()
            .debug_selector(|| "ServiceGeneral-path".into())
            .w_full()
            .whitespace_normal()
            .line_clamp(PATH_LINES)
            .text_color(match f.path.word {
                Word::Value => theme.text_primary,
                _ => theme.text_secondary,
            })
            .child(selectable(("svc-general-text", 3), f.path.text.clone(), &m));
        *path.text_style() = t_base(&m);
        let unit_file = self.links(4, &f.unit_file, false, cx);
        let drop_ins = self.links(5, &f.drop_ins, true, cx);

        // Startup type.
        let combo_state = self.combo_state(cx);
        let view = cx.weak_entity();
        let select = TlSelect::new("ServiceGeneralStartup", &self.startup)
            .width(px(COMBO_WIDTH * m.ui_scale))
            .disabled(!combo_state.enabled)
            .on_change(move |value, _, cx| {
                let _ = view.update(cx, |this, cx| this.choose(value, cx));
            });
        let select = div()
            .id("ServiceGeneralStartupBox")
            .debug_selector(|| "ServiceGeneralStartup".into())
            .role(Role::ComboBox)
            .aria_label(tr(cx, "services-sys031-startup-type"))
            .flex_none()
            .child(select);
        let startup = match combo_state.tooltip {
            Some(tip) => {
                let state = HoverTooltip::state("ServiceGeneralStartupTip", window, cx);
                HoverTooltip::attach(
                    &state,
                    select,
                    DELAY_CONTROL,
                    TlTooltip::text(tip).build(),
                    window,
                    cx,
                )
                .into_any_element()
            }
            None => select.into_any_element(),
        };

        // Status and its buttons.
        let status_text = self.status_text(cx);
        let status = match f.status {
            Some(s) => status_value(status_look(s), status_text, &theme, &m),
            None => div()
                .text_color(theme.text_secondary)
                .child(selectable(("svc-general-text", 7), status_text, &m))
                .into_any_element(),
        };
        let [start_on, stop_on, restart_on] = self.buttons_enabled(cx);
        let button = |id: &'static str, key: &str, on: bool, verb: UnitVerb| {
            let view = cx.weak_entity();
            TlButton::new(id, TlButtonKind::Secondary)
                .size(TlButtonSize::Sm)
                .label(tr(cx, key))
                .disabled(!on)
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.on_verb(verb, window, cx));
                })
        };
        let buttons = h_flex()
            .gap(m.sp[4])
            .child(button(
                "ServiceGeneralStart",
                "services-sys017-start",
                start_on,
                UnitVerb::Start,
            ))
            .child(button(
                "ServiceGeneralStop",
                "services-sys017-stop",
                stop_on,
                UnitVerb::Stop,
            ))
            .child(button(
                "ServiceGeneralRestart",
                "services-sys017-restart",
                restart_on,
                UnitVerb::Restart,
            ));
        let status = v_flex()
            .child(div().h(row_height).flex().items_center().child(status))
            .child(div().h(row_height).flex().items_center().child(buttons));

        let grid_top = self.grid("ServiceGeneralTop", top, 0, label_width, row_height, cx);
        let grid_bottom = self.grid(
            "ServiceGeneralBottom",
            bottom,
            8,
            label_width,
            row_height,
            cx,
        );
        let rows = [
            self.custom_row(2, desc, label_width, row_height, cx),
            self.custom_row(3, path, label_width, row_height, cx),
            self.custom_row(4, unit_file, label_width, row_height, cx),
            self.custom_row(5, drop_ins, label_width, row_height, cx),
            self.custom_row(6, startup, label_width, row_height, cx),
            self.custom_row(7, status, label_width, row_height, cx),
        ];
        v_flex()
            .id("ServiceGeneralTab")
            .debug_selector(|| "ServiceGeneralTab".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            // Ctrl+C with a selection in a drawn row copies it, as the grids do.
            .on_action(|_: &Copy, window, cx| {
                let text = TextSelection::selected_text(window, cx);
                let text = text.trim();
                if text.is_empty() {
                    cx.propagate();
                    return;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
            })
            .on_action(cx.listener(Self::on_copy_value))
            .on_action(cx.listener(Self::on_open_link))
            .on_action(cx.listener(Self::on_jump))
            .size_full()
            .overflow_y_scroll()
            .child(grid_top)
            .children(rows)
            .child(grid_bottom)
    }
}
