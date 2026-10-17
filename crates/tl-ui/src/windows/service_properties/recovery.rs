//! Service Properties Recovery tab, P3 edit mode (UI-SYS-034, SYS-013, R-004, R-119).
//!
//! The restart and failure settings of a unit as an editor. Apply sends `UnitWriteRecovery`
//! with the changed keys plus the keys the tasklens drop-in already sets, because the engine
//! replaces the whole file (i3o.22). The engine and the helper derive the drop-in path; the UI
//! never sends one. "Run on failure" is checked against the unit name pattern before Apply can
//! run; the helper's allowlist is the second gate.
//!
//! The form is built from `UnitDetails.recovery` when the details arrive and rebuilt by
//! `reset`. The window pushes each new details Arc in through `set_details`; the tab notifies
//! itself, because the window draws it through `.cached()`.

use std::sync::Arc;

use gpui_kit::base::input::Copy;
use gpui_kit::base::{SelectableText, TextSelection};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EntityId, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::actions::{ActionId, ActionKind, ActionTarget, RecoveryEdit};
use tl_model::health::HelperState;
use tl_model::snapshot::{ComponentMask, RecoveryKeySet, RecoverySettings, UnitDetails};
use tl_model::table::{Manager, UnitRef};
use tl_model::units;
use tracing::warn;

use super::ServicePropsTabView;
use crate::action_tracker::{ActionTrackerEvent, GlobalActionTracker};
use crate::actions::OpenAdditionalSetup;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::dialog::{DialogResult, TlDialog};
use crate::components::input::TlTextInput;
use crate::components::kv_grid::KEY_CONTEXT;
use crate::components::link::TlLink;
use crate::components::select::{TlSelect, TlSelectItem, TlSelectState};
use crate::components::stepper::TlStepper;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::hub::{GlobalHub, SnapshotChanged};
use crate::i18n::tr;
use crate::pages::services::flows::startup_texts;
use crate::pages::sys_table::{ActionKindTag, ActionTexts, SysActions};
use crate::text::style::{t_base, t_sm};
use crate::theme::{current, metrics};
use crate::window::setup::SetupRow;

/// Label column width at 100% scale.
const LABEL_WIDTH: f32 = 200.;
/// Row height at 100% scale.
const ROW_HEIGHT: f32 = 28.;
/// Select width at 100% scale.
const SELECT_WIDTH: f32 = 200.;
/// "Restart service after" stepper slot width at 100% scale.
const RESTART_WIDTH: f32 = 100.;
/// "Give up after" stepper slot width at 100% scale.
const LIMIT_WIDTH: f32 = 80.;

/// "Restart service after" range in ms, and its step.
const RESTART_MS_MAX: u32 = 3_600_000;
const RESTART_MS_STEP: i64 = 100;
/// "Give up after" ranges: starts, and seconds.
const BURST_MAX: u32 = 1000;
const INTERVAL_S_MAX: u32 = 86_400;

/// The file name of the tasklens drop-in in `<unit>.d` (table (c); the engine's
/// `DROP_IN_NAME`).
const DROP_IN_NAME: &str = "50-tasklens-recovery.conf";

/// The longest name part of a "Run on failure" unit, before its suffix.
const MAX_NAME_PART: usize = 255;
/// The unit suffixes "Run on failure" accepts.
const UNIT_SUFFIXES: [&str; 8] = [
    "service", "socket", "target", "timer", "mount", "path", "slice", "scope",
];

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
/// Row indexes.
const ROW_RESTART: usize = 0;
const ROW_RESTART_MS: usize = 1;
const ROW_GIVE_UP: usize = 2;
const ROW_LIMIT_ACTION: usize = 3;
const ROW_FAILURE_ACTION: usize = 4;
const ROW_ON_FAILURE: usize = 5;
const ROW_RESTARTS: usize = 6;

/// `Restart=` (SYS-013).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RestartPolicy {
    #[default]
    No,
    OnSuccess,
    OnFailure,
    OnAbnormal,
    OnWatchdog,
    OnAbort,
    Always,
}

impl RestartPolicy {
    pub const ALL: [Self; 7] = [
        Self::No,
        Self::OnSuccess,
        Self::OnFailure,
        Self::OnAbnormal,
        Self::OnWatchdog,
        Self::OnAbort,
        Self::Always,
    ];

    /// The systemd word.
    pub fn word(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::OnSuccess => "on-success",
            Self::OnFailure => "on-failure",
            Self::OnAbnormal => "on-abnormal",
            Self::OnWatchdog => "on-watchdog",
            Self::OnAbort => "on-abort",
            Self::Always => "always",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.word() == word)
    }
}

/// `StartLimitAction=` and `FailureAction=` (SYS-013).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LimitAction {
    #[default]
    None,
    Reboot,
    RebootForce,
    RebootImmediate,
    Poweroff,
    PoweroffForce,
    PoweroffImmediate,
    Exit,
    ExitForce,
}

impl LimitAction {
    pub const ALL: [Self; 9] = [
        Self::None,
        Self::Reboot,
        Self::RebootForce,
        Self::RebootImmediate,
        Self::Poweroff,
        Self::PoweroffForce,
        Self::PoweroffImmediate,
        Self::Exit,
        Self::ExitForce,
    ];

    /// The systemd word.
    pub fn word(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Reboot => "reboot",
            Self::RebootForce => "reboot-force",
            Self::RebootImmediate => "reboot-immediate",
            Self::Poweroff => "poweroff",
            Self::PoweroffForce => "poweroff-force",
            Self::PoweroffImmediate => "poweroff-immediate",
            Self::Exit => "exit",
            Self::ExitForce => "exit-force",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.word() == word)
    }

    /// True for a value that restarts or powers off the computer: the warning line shows.
    pub fn restarts_computer(self) -> bool {
        matches!(
            self,
            Self::Reboot
                | Self::RebootForce
                | Self::RebootImmediate
                | Self::Poweroff
                | Self::PoweroffForce
                | Self::PoweroffImmediate
        )
    }
}

/// The values of the Recovery editor.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RecoveryForm {
    pub restart: RestartPolicy,
    /// 0..=3 600 000, step 100.
    pub restart_ms: u32,
    /// 0..=1000.
    pub burst: u32,
    /// 0..=86 400.
    pub interval_s: u32,
    pub start_limit_action: LimitAction,
    pub failure_action: LimitAction,
    /// Unit names separated by spaces.
    pub on_failure: SharedString,
}

/// A duration field systemd reports as `u64::MAX` for "infinity" or "unset" is not a value.
fn known(v: Option<u64>) -> Option<u64> {
    v.filter(|&v| v != u64::MAX)
}

/// The space-separated tokens of "Run on failure". Only the space character separates, so a
/// tab or a newline stays inside a token and makes it invalid.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(' ').filter(|t| !t.is_empty())
}

impl RecoveryForm {
    /// The form for the unit's current values. A missing or unknown word shows the default,
    /// and a number out of range shows the nearest end of the range.
    pub fn from_settings(r: &RecoverySettings) -> Self {
        let clamp = |v: u64, max: u32| u32::try_from(v).unwrap_or(u32::MAX).min(max);
        let names: Vec<&str> = r.on_failure.iter().map(|n| &**n).collect();
        Self {
            restart: r
                .restart
                .as_deref()
                .and_then(RestartPolicy::from_word)
                .unwrap_or_default(),
            restart_ms: known(r.restart_usec).map_or(0, |us| clamp(us / 1000, RESTART_MS_MAX)),
            burst: r.start_limit_burst.unwrap_or(0).min(BURST_MAX),
            interval_s: known(r.start_limit_interval_usec)
                .map_or(0, |us| clamp(us / 1_000_000, INTERVAL_S_MAX)),
            start_limit_action: r
                .start_limit_action
                .as_deref()
                .and_then(LimitAction::from_word)
                .unwrap_or_default(),
            failure_action: r
                .failure_action
                .as_deref()
                .and_then(LimitAction::from_word)
                .unwrap_or_default(),
            on_failure: names.join(" ").into(),
        }
    }
}

/// True when every space-separated token of `text` is a unit name: 1 to 255 characters from
/// `A-Za-z0-9:_.\@%-`, then one of the unit suffixes. Empty text is valid (no units).
pub fn validate_on_failure(text: &str) -> bool {
    tokens(text).all(|token| {
        let Some((name, suffix)) = token.rsplit_once('.') else {
            return false;
        };
        (1..=MAX_NAME_PART).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b":_.\\@%-".contains(&b))
            && UNIT_SUFFIXES.contains(&suffix)
    })
}

/// The edit Apply sends: each key whose form value differs from the unit's current value, plus
/// each key `in_dropin` lists (R-004). A key that is unchanged but in the drop-in carries the
/// unit's current value as reported, so a write never rounds or rewrites it.
pub fn changed_keys(
    form: &RecoveryForm,
    current: &RecoverySettings,
    in_dropin: &RecoveryKeySet,
) -> RecoveryEdit {
    let base = RecoveryForm::from_settings(current);
    let names: Vec<Arc<str>> = tokens(&form.on_failure).map(Arc::from).collect();
    let names_changed = names.as_slice() != &*current.on_failure;
    RecoveryEdit {
        restart: pick(
            form.restart != base.restart,
            in_dropin.restart,
            || Arc::from(form.restart.word()),
            current.restart.clone(),
        ),
        restart_usec: pick(
            form.restart_ms != base.restart_ms,
            in_dropin.restart_sec,
            || u64::from(form.restart_ms) * 1000,
            known(current.restart_usec),
        ),
        restart_steps: None,
        restart_max_delay_usec: None,
        start_limit_burst: pick(
            form.burst != base.burst,
            in_dropin.start_limit_burst,
            || form.burst,
            current.start_limit_burst,
        ),
        start_limit_interval_usec: pick(
            form.interval_s != base.interval_s,
            in_dropin.start_limit_interval_sec,
            || u64::from(form.interval_s) * 1_000_000,
            known(current.start_limit_interval_usec),
        ),
        start_limit_action: pick(
            form.start_limit_action != base.start_limit_action,
            in_dropin.start_limit_action,
            || Arc::from(form.start_limit_action.word()),
            current.start_limit_action.clone(),
        ),
        failure_action: pick(
            form.failure_action != base.failure_action,
            in_dropin.failure_action,
            || Arc::from(form.failure_action.word()),
            current.failure_action.clone(),
        ),
        on_failure: pick(
            names_changed,
            in_dropin.on_failure,
            || Arc::from(names),
            Some(current.on_failure.clone()),
        ),
    }
}

/// One key of [`changed_keys`]: the form value when `changed`, else the current value (or the
/// form value when none is reported) when `kept`, else nothing.
fn pick<T>(changed: bool, kept: bool, new: impl FnOnce() -> T, old: Option<T>) -> Option<T> {
    if changed {
        Some(new())
    } else if kept {
        Some(old.unwrap_or_else(new))
    } else {
        None
    }
}

/// The keys `edit` sets, added to `keys`.
fn with_edit_keys(mut keys: RecoveryKeySet, edit: &RecoveryEdit) -> RecoveryKeySet {
    keys.restart |= edit.restart.is_some();
    keys.restart_sec |= edit.restart_usec.is_some();
    keys.start_limit_burst |= edit.start_limit_burst.is_some();
    keys.start_limit_interval_sec |= edit.start_limit_interval_usec.is_some();
    keys.start_limit_action |= edit.start_limit_action.is_some();
    keys.failure_action |= edit.failure_action.is_some();
    keys.on_failure |= edit.on_failure.is_some();
    keys
}

/// `current` with the fields `edit` sets replaced: the unit's values once the edit is saved.
fn overlay(current: &RecoverySettings, edit: &RecoveryEdit) -> RecoverySettings {
    let mut r = current.clone();
    if let Some(v) = &edit.restart {
        r.restart = Some(v.clone());
    }
    if let Some(v) = edit.restart_usec {
        r.restart_usec = Some(v);
    }
    if let Some(v) = edit.start_limit_burst {
        r.start_limit_burst = Some(v);
    }
    if let Some(v) = edit.start_limit_interval_usec {
        r.start_limit_interval_usec = Some(v);
    }
    if let Some(v) = &edit.start_limit_action {
        r.start_limit_action = Some(v.clone());
    }
    if let Some(v) = &edit.failure_action {
        r.failure_action = Some(v.clone());
    }
    if let Some(v) = &edit.on_failure {
        r.on_failure = v.clone();
    }
    r
}

/// The drop-in path "Saved as" shows (table (c)).
fn saved_as(unit: &UnitRef) -> SharedString {
    let dir = match unit.manager {
        Manager::System => "/etc/systemd/system",
        Manager::User => "~/.config/systemd/user",
    };
    format!("{dir}/{}.d/{DROP_IN_NAME}", unit.name).into()
}

/// The translated texts of the tab.
struct Words {
    labels: [SharedString; 7],
    collecting: SharedString,
    not_reported: SharedString,
    suffix_ms: SharedString,
    starts_within: SharedString,
    suffix_s: SharedString,
    invalid_units: SharedString,
    power_warning: SharedString,
    needs_helper: SharedString,
    additional_setup: SharedString,
    saved_as: SharedString,
    remove: SharedString,
}

impl Words {
    fn new(cx: &App) -> Self {
        Self {
            labels: LABEL_KEYS.map(|key| tr(cx, key)),
            collecting: tr(cx, "fnd-kv-collecting"),
            not_reported: tr(cx, "fnd-kv-not-reported"),
            suffix_ms: tr(cx, "services-sys034-suffix-ms"),
            starts_within: tr(cx, "services-sys034-starts-within"),
            suffix_s: tr(cx, "services-sys034-suffix-s"),
            invalid_units: tr(cx, "services-sys034-invalid-units"),
            power_warning: tr(cx, "services-sys034-power-warning"),
            needs_helper: tr(cx, "services-sys034-needs-helper"),
            additional_setup: tr(cx, "services-sys034-additional-setup"),
            saved_as: tr(cx, "services-sys034-saved-as"),
            remove: tr(cx, "services-sys034-remove"),
        }
    }

    /// Row `row`'s label without its trailing ":", the control's accessible name.
    fn name(&self, row: usize) -> SharedString {
        self.labels[row].trim_end_matches(':').to_string().into()
    }
}

/// The current values as last saved by this tab, kept until the next details Arc.
#[derive(Clone)]
struct Saved {
    settings: RecoverySettings,
    keys: RecoveryKeySet,
}

fn select_state(
    words: impl Iterator<Item = &'static str>,
    cx: &mut Context<RecoveryTab>,
) -> Entity<TlSelectState> {
    let items = words.map(|w| TlSelectItem::new(w, w)).collect();
    cx.new(|cx| TlSelectState::new(items, None, cx))
}

pub struct RecoveryTab {
    unit: UnitRef,
    actions: Entity<SysActions>,
    /// The window view: notified when the tab's dirty state changes, for the footer.
    parent: EntityId,
    entity_id: EntityId,
    details: Option<Arc<UnitDetails>>,
    not_loaded: bool,
    helper: HelperState,
    words: Words,
    /// `None` until the details arrive.
    form: Option<RecoveryForm>,
    /// The values the last successful save gave the unit, until the next details Arc.
    applied: Option<Saved>,
    /// The save in flight. The controls are locked until it finishes.
    sent: Option<(ActionId, Saved)>,
    restart: Entity<TlSelectState>,
    limit_action: Entity<TlSelectState>,
    failure_action: Entity<TlSelectState>,
    restart_ms: Entity<InputState>,
    burst: Entity<InputState>,
    interval: Entity<InputState>,
    on_failure: Entity<InputState>,
    /// The "Run on failure" field text must be written from the form on the next render.
    sync_input: bool,
    focus: FocusHandle,
    #[cfg(any(test, feature = "test-hooks"))]
    renders: u32,
    _subs: Vec<Subscription>,
}

impl RecoveryTab {
    pub(super) fn new(
        unit: UnitRef,
        actions: Entity<SysActions>,
        parent: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = Words::new(cx);
        let restart = select_state(RestartPolicy::ALL.into_iter().map(|p| p.word()), cx);
        let limit_action = select_state(LimitAction::ALL.into_iter().map(|a| a.word()), cx);
        let failure_action = select_state(LimitAction::ALL.into_iter().map(|a| a.word()), cx);
        let restart_ms = cx.new(|cx| InputState::new(window, cx));
        let burst = cx.new(|cx| InputState::new(window, cx));
        let interval = cx.new(|cx| InputState::new(window, cx));
        let on_failure = cx.new(|cx| InputState::new(window, cx));
        let mut subs = vec![cx.subscribe(&on_failure, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.on_failure_typed(cx);
            }
        })];
        if let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) {
            subs.push(
                cx.subscribe(&tracker, |this, _, ev: &ActionTrackerEvent, cx| {
                    if let ActionTrackerEvent::Finished { id, failed, .. } = *ev {
                        this.on_save_finished(id, failed, cx);
                    }
                }),
            );
        }
        let mut helper = HelperState::default();
        if let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) {
            helper = hub.read(cx).current().health.helper;
            subs.push(cx.subscribe(&hub, |this, hub, ev: &SnapshotChanged, cx| {
                if !ev.mask.intersects(ComponentMask::HEALTH) {
                    return;
                }
                let helper = hub.read(cx).current().health.helper;
                if helper != this.helper {
                    this.helper = helper;
                    cx.notify();
                }
            }));
        }
        Self {
            unit,
            actions,
            parent,
            entity_id: cx.entity_id(),
            details: None,
            not_loaded: false,
            helper,
            words,
            form: None,
            applied: None,
            sent: None,
            restart,
            limit_action,
            failure_action,
            restart_ms,
            burst,
            interval,
            on_failure,
            sync_input: false,
            focus: cx.focus_handle(),
            #[cfg(any(test, feature = "test-hooks"))]
            renders: 0,
            _subs: subs,
        }
    }

    /// The unit's current values: the last save, else the details.
    fn current(&self) -> Option<&RecoverySettings> {
        match &self.applied {
            Some(saved) => Some(&saved.settings),
            None => self.details.as_ref().map(|d| &d.recovery),
        }
    }

    /// The keys the tasklens drop-in sets now.
    fn in_dropin(&self) -> RecoveryKeySet {
        match &self.applied {
            Some(saved) => saved.keys,
            None => self
                .details
                .as_ref()
                .map(|d| d.recovery_dropin_keys)
                .unwrap_or_default(),
        }
    }

    fn baseline(&self) -> Option<RecoveryForm> {
        self.current().map(RecoveryForm::from_settings)
    }

    /// A system unit with no helper installed cannot be edited.
    fn needs_helper(&self) -> bool {
        self.unit.manager == Manager::System && self.helper == HelperState::Missing
    }

    fn controls_enabled(&self) -> bool {
        self.form.is_some() && !self.not_loaded && self.sent.is_none() && !self.needs_helper()
    }

    /// Writes the form into the selects now and into the text field on the next render.
    fn sync_controls(&mut self, cx: &mut App) {
        let Some(form) = &self.form else {
            return;
        };
        let values = [
            (&self.restart, form.restart.word()),
            (&self.limit_action, form.start_limit_action.word()),
            (&self.failure_action, form.failure_action.word()),
        ];
        for (state, value) in values {
            state.update(cx, |s, cx| s.set_selected_value(value, cx));
        }
        self.sync_input = true;
    }

    /// Rebuilds the form from the current values.
    fn rebuild_form(&mut self, cx: &mut App) {
        self.form = self.baseline();
        self.sync_controls(cx);
    }

    /// Changes the form through `f` and tells the window when the dirty state may change.
    fn edit(&mut self, f: impl FnOnce(&mut RecoveryForm), cx: &mut App) {
        if !self.controls_enabled() {
            // Put back the shown values.
            self.sync_controls(cx);
            cx.notify(self.entity_id);
            return;
        }
        let Some(form) = &mut self.form else {
            return;
        };
        let before = form.clone();
        f(form);
        if *form != before {
            cx.notify(self.entity_id);
            cx.notify(self.parent);
        }
    }

    fn on_failure_typed(&mut self, cx: &mut Context<Self>) {
        let text = self.on_failure.read(cx).text().to_string();
        if self.form.as_ref().is_none_or(|f| *f.on_failure == *text) {
            return;
        }
        self.edit(|f| f.on_failure = text.into(), cx);
    }

    fn on_save_finished(&mut self, id: ActionId, failed: u16, cx: &mut Context<Self>) {
        if self.sent.as_ref().is_none_or(|(sent, _)| *sent != id) {
            return;
        }
        let Some((_, saved)) = self.sent.take() else {
            return;
        };
        // On failure the form keeps the edit, so the tab stays dirty.
        if failed == 0 {
            self.applied = Some(saved);
        }
        cx.notify();
        App::notify(cx, self.parent);
    }

    fn on_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.controls_enabled() {
            return;
        }
        let view = cx.weak_entity();
        let dialog = TlDialog::confirm(
            tr(cx, "services-sys034-remove-text"),
            tr(cx, "services-sys034-remove-confirm"),
        )
        .destructive()
        .title(tr(cx, "services-sys034-remove-title"))
        .heading(tr(cx, "services-sys034-remove-heading"))
        .cancel_label(tr(cx, "services-sys034-remove-cancel"));
        let opened = dialog.open(window, cx, move |result, window, cx| {
            if matches!(result, DialogResult::Confirmed { .. }) {
                let _ = view.update(cx, |this, cx| this.send_remove(window, cx));
            }
        });
        if let Err(err) = opened {
            warn!(target: "tasklens::ui", %err, "the remove confirmation did not open");
        }
    }

    fn send_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let texts = ActionTexts {
            pending_one: SharedString::default(),
            done_one: SharedString::default(),
            failed_prefix: tr(cx, "services-sys034-remove-failed"),
            ..startup_texts(cx)
        };
        let targets = smallvec![ActionTarget::Unit(self.unit.clone())];
        self.actions.update(cx, |actions, cx| {
            actions.submit(
                ActionKind::UnitRemoveRecovery,
                ActionKindTag::Other,
                targets,
                texts,
                window,
                cx,
            )
        });
    }

    /// Wraps a control so it shows "Needs the tasklens helper" on hover while the helper is
    /// missing.
    fn with_helper_tip(
        &self,
        id: &'static str,
        control: gpui_kit::Stateful<gpui_kit::Div>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if !self.needs_helper() {
            return control.into_any_element();
        }
        let state = HoverTooltip::state(id, window, cx);
        HoverTooltip::attach(
            &state,
            control,
            DELAY_CONTROL,
            TlTooltip::text(self.words.needs_helper.clone()).build(),
            window,
            cx,
        )
        .into_any_element()
    }

    fn select(
        &self,
        row: usize,
        state: &Entity<TlSelectState>,
        on_change: impl Fn(&mut RecoveryForm, &str) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id: &'static str = match row {
            ROW_RESTART => "ServiceRecoveryRestart",
            ROW_LIMIT_ACTION => "ServiceRecoveryLimitAction",
            _ => "ServiceRecoveryFailureAction",
        };
        let s = metrics(cx).ui_scale;
        let view = cx.weak_entity();
        let select = TlSelect::new(id, state)
            .width(px(SELECT_WIDTH * s))
            .disabled(!self.controls_enabled())
            .on_change(move |value, _, cx| {
                let value = value.clone();
                let _ = view.update(cx, |this, cx| {
                    this.edit(|f| on_change(f, &value), cx);
                });
            });
        let control = div()
            .id(id)
            .debug_selector(move || id.into())
            .role(Role::ComboBox)
            .aria_label(self.words.name(row))
            .flex_none()
            .child(select);
        self.with_helper_tip(id, control, window, cx)
    }

    fn stepper(
        &self,
        id: &'static str,
        state: &Entity<InputState>,
        value: u32,
        max: u32,
        step: i64,
        set: impl Fn(&mut RecoveryForm, u32) + 'static,
        cx: &mut Context<Self>,
    ) -> TlStepper {
        let view = cx.weak_entity();
        TlStepper::new(id, state)
            .range(0, i64::from(max), step)
            .value(i64::from(value))
            .disabled(!self.controls_enabled())
            .on_change(move |v, _, cx| {
                let v = u32::try_from(v.clamp(0, i64::from(max))).unwrap_or(0);
                let _ = view.update(cx, |this, cx| this.edit(|f| set(f, v), cx));
            })
    }

    /// A `t_sm` line under a field.
    fn note_line(
        selector: &'static str,
        text: SharedString,
        color: gpui_kit::Hsla,
        cx: &App,
    ) -> AnyElement {
        let m = metrics(cx);
        let mut line = div()
            .debug_selector(move || selector.into())
            .pt(px(2. * m.ui_scale))
            .text_color(color)
            .child(text);
        *line.text_style() = t_sm(m);
        line.into_any_element()
    }

    /// One row: the label, then `value` with optional lines under it.
    fn row(&self, row: usize, value: AnyElement, cx: &App) -> AnyElement {
        let theme = current(cx);
        let m = metrics(cx);
        let row_height = px(ROW_HEIGHT * m.ui_scale);
        let mut label = div()
            .flex_none()
            .w(px(LABEL_WIDTH * m.ui_scale))
            .h(row_height)
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(theme.text_secondary)
            .child(self.words.labels[row].clone());
        *label.text_style() = t_base(m);
        let mut value = v_flex()
            .flex_1()
            .min_w_0()
            .min_h(row_height)
            .justify_center()
            .text_color(theme.text_primary)
            .child(value);
        *value.text_style() = t_base(m);
        h_flex()
            .debug_selector(move || format!("ServiceRecovery-row-{row}"))
            .w_full()
            .items_start()
            .pb(px(4. * m.ui_scale))
            .child(label)
            .child(value)
            .into_any_element()
    }

    /// The rows before the details arrive: "Collecting…" in every value.
    fn collecting_rows(&self, cx: &App) -> Vec<AnyElement> {
        let theme = current(cx);
        (0..LABEL_KEYS.len())
            .map(|row| {
                let value = div()
                    .text_color(theme.text_secondary)
                    .child(self.words.collecting.clone())
                    .into_any_element();
                self.row(row, value, cx)
            })
            .collect()
    }

    fn edit_rows(
        &mut self,
        form: &RecoveryForm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let enabled = self.controls_enabled();
        let mut rows = Vec::with_capacity(LABEL_KEYS.len());

        let restart = self.select(
            ROW_RESTART,
            &self.restart.clone(),
            |f, v| {
                if let Some(p) = RestartPolicy::from_word(v) {
                    f.restart = p;
                }
            },
            window,
            cx,
        );
        rows.push(self.row(ROW_RESTART, restart, cx));

        let restart_ms = self
            .stepper(
                "ServiceRecoveryRestartMs",
                &self.restart_ms.clone(),
                form.restart_ms,
                RESTART_MS_MAX,
                RESTART_MS_STEP,
                |f, v| f.restart_ms = v,
                cx,
            )
            .suffix(self.words.suffix_ms.clone());
        let restart_ms = div()
            .id("ServiceRecoveryRestartMsBox")
            .role(Role::Group)
            .aria_label(self.words.name(ROW_RESTART_MS))
            .flex_none()
            .min_w(px(RESTART_WIDTH * s))
            .child(restart_ms);
        let restart_ms =
            self.with_helper_tip("ServiceRecoveryRestartMsBox", restart_ms, window, cx);
        rows.push(self.row(ROW_RESTART_MS, restart_ms, cx));

        let burst = self.stepper(
            "ServiceRecoveryBurst",
            &self.burst.clone(),
            form.burst,
            BURST_MAX,
            1,
            |f, v| f.burst = v,
            cx,
        );
        let interval = self.stepper(
            "ServiceRecoveryInterval",
            &self.interval.clone(),
            form.interval_s,
            INTERVAL_S_MAX,
            1,
            |f, v| f.interval_s = v,
            cx,
        );
        let give_up = div()
            .id("ServiceRecoveryGiveUpBox")
            .role(Role::Group)
            .aria_label(self.words.name(ROW_GIVE_UP))
            .flex()
            .items_center()
            .gap(m.sp[3])
            .child(div().flex_none().min_w(px(LIMIT_WIDTH * s)).child(burst))
            .child(div().flex_none().child(self.words.starts_within.clone()))
            .child(div().flex_none().min_w(px(LIMIT_WIDTH * s)).child(interval))
            .child(div().flex_none().child(self.words.suffix_s.clone()));
        let give_up = self.with_helper_tip("ServiceRecoveryGiveUpBox", give_up, window, cx);
        rows.push(self.row(ROW_GIVE_UP, give_up, cx));

        let actions = [
            (
                ROW_LIMIT_ACTION,
                self.limit_action.clone(),
                form.start_limit_action,
            ),
            (
                ROW_FAILURE_ACTION,
                self.failure_action.clone(),
                form.failure_action,
            ),
        ];
        for (row, state, value) in actions {
            let select = self.select(
                row,
                &state,
                move |f, v| {
                    if let Some(a) = LimitAction::from_word(v) {
                        if row == ROW_LIMIT_ACTION {
                            f.start_limit_action = a;
                        } else {
                            f.failure_action = a;
                        }
                    }
                },
                window,
                cx,
            );
            let mut cell = v_flex().child(select);
            if value.restarts_computer() {
                let selector = if row == ROW_LIMIT_ACTION {
                    "ServiceRecoveryWarning-3"
                } else {
                    "ServiceRecoveryWarning-4"
                };
                cell = cell.child(Self::note_line(
                    selector,
                    self.words.power_warning.clone(),
                    theme.status.warning,
                    cx,
                ));
            }
            rows.push(self.row(row, cell.into_any_element(), cx));
        }

        if self.sync_input {
            self.sync_input = false;
            if self.on_failure.read(cx).text().to_string() != *form.on_failure {
                let text = form.on_failure.clone();
                self.on_failure
                    .update(cx, |s, cx| s.set_value(text, window, cx));
            }
        }
        let valid = validate_on_failure(&form.on_failure);
        let input = TlTextInput::new("ServiceRecoveryOnFailure", &self.on_failure)
            .aria_label(self.words.name(ROW_ON_FAILURE))
            .invalid(!valid)
            .disabled(!enabled);
        let mut field = div()
            .id("ServiceRecoveryOnFailureBox")
            .debug_selector(|| "ServiceRecoveryOnFailure".into())
            .w_full()
            .child(input);
        if !valid {
            field = field.aria_description(self.words.invalid_units.clone());
        }
        let field = self.with_helper_tip("ServiceRecoveryOnFailureBox", field, window, cx);
        let mut cell = v_flex().w_full().child(field);
        if !valid {
            cell = cell.child(Self::note_line(
                "ServiceRecoveryOnFailureError",
                self.words.invalid_units.clone(),
                theme.status.danger,
                cx,
            ));
        }
        rows.push(self.row(ROW_ON_FAILURE, cell.into_any_element(), cx));

        let restarts = self.restarts_text();
        let restarts = div()
            .child(SelectableText::new("svc-recovery-restarts", restarts).text_style(t_base(&m)))
            .into_any_element();
        rows.push(self.row(ROW_RESTARTS, restarts, cx));
        rows
    }

    /// The "Restarts so far" value.
    fn restarts_text(&self) -> SharedString {
        match self.details.as_ref().and_then(|d| d.recovery.n_restarts) {
            Some(n) => {
                let mut text = String::new();
                units::count(&mut text, u64::from(n));
                text.into()
            }
            None => self.words.not_reported.clone(),
        }
    }

    /// The "Saved as" line and the remove button.
    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let mut saved = h_flex()
            .debug_selector(|| "ServiceRecoverySavedAs".into())
            .flex_1()
            .min_w_0()
            .gap(m.sp[2])
            .text_color(theme.text_secondary)
            .child(div().flex_none().child(self.words.saved_as.clone()))
            .child(
                div().min_w_0().child(
                    SelectableText::new("svc-recovery-saved-as", saved_as(&self.unit))
                        .text_style(t_sm(&m)),
                ),
            );
        *saved.text_style() = t_sm(&m);
        let exists = self
            .details
            .as_ref()
            .is_some_and(|d| d.recovery_dropin_exists);
        let remove = exists.then(|| {
            let view = cx.weak_entity();
            let mut button = TlButton::new("ServiceRecoveryRemove", TlButtonKind::Secondary)
                .size(TlButtonSize::Sm)
                .label(self.words.remove.clone())
                .disabled(!self.controls_enabled())
                .on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.on_remove(window, cx));
                });
            if self.needs_helper() {
                button = button.tooltip(self.words.needs_helper.clone());
            }
            button
        });
        h_flex()
            .w_full()
            .items_center()
            .gap(m.sp[4])
            .mt(m.sp[4])
            .pt(m.sp[4])
            .border_t_1()
            .border_color(theme.divider)
            .child(saved)
            .children(remove)
            .into_any_element()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn render_count(&self) -> u32 {
        self.renders
    }

    /// The 7 labels and, before the details arrive, "Collecting…" for each; after, the
    /// "Restarts so far" text in its row and the form's words or numbers in the others.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn rows_for_test(&self, _cx: &App) -> Vec<(SharedString, SharedString)> {
        let values: [SharedString; 7] = match &self.form {
            None => std::array::from_fn(|_| self.words.collecting.clone()),
            Some(f) => [
                f.restart.word().into(),
                f.restart_ms.to_string().into(),
                format!("{} {}", f.burst, f.interval_s).into(),
                f.start_limit_action.word().into(),
                f.failure_action.word().into(),
                f.on_failure.clone(),
                self.restarts_text(),
            ],
        };
        self.words.labels.iter().cloned().zip(values).collect()
    }

    /// The form, once the details arrived.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn form_for_test(&self) -> Option<RecoveryForm> {
        self.form.clone()
    }

    /// Changes the form as the user would through the controls.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn edit_for_test(&mut self, f: impl FnOnce(&mut RecoveryForm), cx: &mut Context<Self>) {
        self.edit(f, cx);
        self.sync_controls(cx);
    }

    /// Whether the controls can change, and the tooltip they show.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn controls_for_test(&self) -> (bool, Option<SharedString>) {
        let tip = self.needs_helper().then(|| self.words.needs_helper.clone());
        (self.controls_enabled(), tip)
    }

    /// The "Saved as" path.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn saved_as_for_test(&self) -> SharedString {
        saved_as(&self.unit)
    }
}

impl ServicePropsTabView for RecoveryTab {
    fn is_dirty(&self, _cx: &App) -> bool {
        match (&self.form, self.baseline()) {
            (Some(form), Some(base)) => *form != base,
            _ => false,
        }
    }

    fn is_valid(&self, _cx: &App) -> bool {
        self.form
            .as_ref()
            .is_none_or(|f| validate_on_failure(&f.on_failure))
    }

    fn apply(&mut self, window: &mut Window, cx: &mut App) -> SmallVec<[ActionId; 2]> {
        if !self.controls_enabled() || !self.is_dirty(cx) || !self.is_valid(cx) {
            return SmallVec::new();
        }
        let (Some(form), Some(current)) = (&self.form, self.current()) else {
            return SmallVec::new();
        };
        let edit = changed_keys(form, current, &self.in_dropin());
        let saved = Saved {
            settings: overlay(current, &edit),
            keys: with_edit_keys(self.in_dropin(), &edit),
        };
        let texts = ActionTexts {
            pending_one: tr(cx, "services-sys034-saving"),
            done_one: tr(cx, "services-sys034-saved"),
            failed_prefix: tr(cx, "services-sys034-save-failed"),
            failed_suffix: true,
            ..startup_texts(cx)
        };
        let targets = smallvec![ActionTarget::Unit(self.unit.clone())];
        // A rejected submit (the unit is busy, say) returns no id. The form stays, so the tab
        // stays dirty and the window keeps it open.
        let id = self.actions.update(cx, |actions, cx| {
            actions.submit(
                ActionKind::UnitWriteRecovery(edit),
                ActionKindTag::Other,
                targets,
                texts,
                window,
                cx,
            )
        });
        if let Some(id) = id {
            self.sent = Some((id, saved));
            cx.notify(self.entity_id);
        }
        id.into_iter().collect()
    }

    fn reset(&mut self, cx: &mut App) {
        // The window resets after a successful save, possibly before this tab hears of it.
        if let Some((_, saved)) = self.sent.take() {
            self.applied = Some(saved);
        }
        self.rebuild_form(cx);
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
        self.not_loaded = not_loaded;
        if !same {
            let clean = !self.is_dirty(cx);
            self.details = details.cloned();
            self.applied = None;
            // A clean form follows the unit; an edit in progress is kept.
            if clean || self.form.is_none() {
                let base = self.baseline();
                if base != self.form {
                    self.form = base;
                    self.sync_controls(cx);
                }
            }
        }
        cx.notify(self.entity_id);
    }
}

impl Render for RecoveryTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.renders += 1;
        }
        let m = *metrics(cx);
        let setup = self.needs_helper().then(|| {
            div()
                .debug_selector(|| "ServiceRecoverySetupLink".into())
                .pb(m.sp[4])
                .child(TlLink::new(
                    "svc-recovery-setup",
                    self.words.additional_setup.clone(),
                    Box::new(OpenAdditionalSetup {
                        focus_row: Some(SetupRow::AdminHelper),
                    }),
                ))
        });
        let rows = match self.form.clone() {
            Some(form) => self.edit_rows(&form, window, cx),
            None => self.collecting_rows(cx),
        };
        let footer = self.footer(cx);
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
            .size_full()
            .overflow_y_scroll()
            .children(setup)
            .children(rows)
            .child(footer)
    }
}
