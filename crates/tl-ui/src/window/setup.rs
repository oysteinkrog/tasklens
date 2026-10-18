//! "tasklens Additional Setup": one card per optional system change with its status
//! (R-028, PERF-105, APP-042, UI-SHL-094, UI-PRF-052).
//!
//! A Tool window keyed `WindowKey::AdditionalSetup`. `OpenAdditionalSetup` opens or focuses it
//! and asks the engine for the status once per dispatch. The statuses come from
//! `HealthSnapshot.setup.status`; the window reads no system file (ADR-001 rule 1). The buttons
//! row of each card holds Enable and Revert for CPU power draw (E29-025), then View script and
//! Copy command (E29-026); the Journal access card has one Copy button. The Fan sensors card
//! has Run sensor detection or Revert (E29-031), then the same two script buttons.
//!
//! Enable and Revert open a confirmation, then send `Command::RunSetup` with a closed
//! `SetupStep`. The result comes back as `HealthSnapshot.setup.run`. The window shows the
//! running state, a success line or a failure block with the script output. The output is shown
//! and copied on request, never logged.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    Action, App, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement,
    RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    Task, Window, div, px, size,
};
use tl_model::command::{
    Command, RaplStatus, SetupPhase, SetupRun, SetupState, SetupStatus, SetupStep,
};
use tl_model::health::SourceId;
use tracing::warn;

use super::registry::{GlobalRegistry, OpenOutcome, WindowKey};
use crate::app::EngineGlobal;
use crate::components::badge::{TlBadge, TlBadgeKind};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::dialog::{DialogResult, TlDialog};
use crate::components::motion::M_SLOW;
use crate::components::progress::TlSpinner;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::{Strings, tr, tr_args};
use crate::text::fonts::MONO_FAMILY;
use crate::text::style::{t_md, t_sm};
use crate::theme::{current, metrics};
use crate::util::clipboard_write;

/// The packaged `tasklens-setup` script, embedded at build time. The UI reads no system file
/// (E29-OI-15).
pub const SETUP_SCRIPT: &str = include_str!("../../../../packaging/tasklens-setup");

/// Script window size, × ui_scale.
const SCRIPT_SIZE: (f32, f32) = (640., 480.);
/// Window size, × ui_scale.
const SIZE: (f32, f32) = (640., 520.);
const MIN_SIZE: (f32, f32) = (560., 400.);
/// Content width, × ui_scale.
const CONTENT_WIDTH: f32 = 560.;
/// Page padding, × ui_scale.
const PAGE_PAD: f32 = 16.;
/// Spinner size, × ui_scale.
const SPINNER: f32 = 12.;
/// Widest status badge, × ui_scale. "Enabled (existing rule)" needs more than the 120 default.
const STATUS_BADGE_MAX_WIDTH: f32 = 168.;
/// Highest the script output grows before it scrolls, × ui_scale.
const OUTPUT_MAX_HEIGHT: f32 = 160.;

const TITLE: &str = "setup-title";

/// The cards of the window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SetupRow {
    CpuPower,
    JournalAccess,
    AdminHelper,
    FanSensors,
    ProcessNetwork,
}

/// The shown cards, top to bottom.
const CARDS: [SetupRow; 5] = [
    SetupRow::CpuPower,
    SetupRow::JournalAccess,
    SetupRow::AdminHelper,
    SetupRow::FanSensors,
    SetupRow::ProcessNetwork,
];

impl SetupRow {
    /// The index of the card, or `None` for a row without a card.
    fn card_index(self) -> Option<usize> {
        CARDS.iter().position(|r| *r == self)
    }

    fn title_key(self) -> &'static str {
        match self {
            Self::CpuPower => "setup-row-cpu-title",
            Self::JournalAccess => "setup-row-journal-title",
            Self::AdminHelper => "setup-row-helper-title",
            Self::FanSensors => "setup-row-fan-title",
            Self::ProcessNetwork => "setup-row-network-title",
        }
    }

    fn description_key(self) -> &'static str {
        match self {
            Self::CpuPower => "setup-row-cpu-description",
            Self::JournalAccess => "setup-row-journal-description",
            Self::AdminHelper => "setup-row-helper-description",
            Self::FanSensors => "setup-row-fan-description",
            Self::ProcessNetwork => "setup-row-network-description",
        }
    }
}

/// Opens the Additional Setup window, or focuses it. `focus_row` scrolls that card into view and
/// flashes its border.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct OpenAdditionalSetup {
    pub focus_row: Option<SetupRow>,
}

/// Opens the confirmation of a setup step, from a card button.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetupConfirmStep {
    pub step: SetupStep,
}

/// Sends a step with no confirmation, from a card button.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetupRunStep {
    pub step: SetupStep,
}

/// Sends the step again after a failure.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetupRetryStep {
    pub step: SetupStep,
}

/// Opens the script window of a step, from a card button.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetupViewScript {
    pub step: SetupStep,
}

/// Copies the `pkexec` command line of a step.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetupCopyCommand {
    pub step: SetupStep,
}

/// Copies the command of the Journal access card.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetupCopyJournal;

/// Copies the output of the failed step.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetupCopyOutput;

/// The status badge of one row: its kind and the Fluent keys of its text and tooltip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RowStatus {
    pub kind: TlBadgeKind,
    pub text_key: &'static str,
    pub tooltip_key: Option<&'static str>,
}

const fn status(
    kind: TlBadgeKind,
    text_key: &'static str,
    tooltip_key: Option<&'static str>,
) -> RowStatus {
    RowStatus {
        kind,
        text_key,
        tooltip_key,
    }
}

/// The badge of `row`. `None` before the engine answered: the card shows a spinner.
pub fn row_status(row: SetupRow, st: Option<&SetupStatus>) -> Option<RowStatus> {
    let st = st?;
    let ok = TlBadgeKind::Success;
    let warn = TlBadgeKind::Warning;
    Some(match row {
        SetupRow::CpuPower => match st.rapl {
            RaplStatus::NoZone => status(TlBadgeKind::Neutral, "setup-status-not-needed", None),
            RaplStatus::Readable {
                tasklens_rule: true,
            } => status(ok, "setup-status-enabled", None),
            RaplStatus::Readable {
                tasklens_rule: false,
            } => status(
                ok,
                "setup-status-enabled-existing",
                Some("setup-tooltip-existing-rule"),
            ),
            RaplStatus::NotReadable => status(warn, "setup-status-not-enabled", None),
        },
        SetupRow::JournalAccess if st.journal_group => status(ok, "setup-status-enabled", None),
        SetupRow::JournalAccess => status(warn, "setup-status-not-enabled", None),
        SetupRow::AdminHelper if st.helper_installed => status(ok, "setup-status-installed", None),
        SetupRow::AdminHelper => status(
            warn,
            "setup-status-not-installed",
            Some("setup-tooltip-helper-missing"),
        ),
        SetupRow::FanSensors if st.hwmon_fan => {
            status(TlBadgeKind::Neutral, "setup-status-not-needed", None)
        }
        SetupRow::FanSensors if !st.lm_sensors => {
            status(TlBadgeKind::Neutral, "setup-status-not-available", None)
        }
        SetupRow::FanSensors if st.sensors_conf => status(ok, "setup-status-enabled", None),
        SetupRow::FanSensors => status(warn, "setup-status-not-enabled", None),
        SetupRow::ProcessNetwork => status(TlBadgeKind::Neutral, "setup-status-not-needed", None),
    })
}

/// Registers the `OpenAdditionalSetup` handler. `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
    // Deferred: from a menu or a link the host window is mid-update.
    cx.on_action(|a: &OpenAdditionalSetup, cx| {
        let focus_row = a.focus_row;
        cx.defer(move |cx| open_additional_setup(focus_row, cx));
    });
}

/// Focuses the open window, or opens it, and asks the engine for the status once. Without a
/// window registry (tests) it always opens one.
pub fn open_additional_setup(focus_row: Option<SetupRow>, cx: &mut App) {
    let s = metrics(cx).ui_scale;
    let spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        tr(cx, TITLE),
        size(px(SIZE.0 * s), px(SIZE.1 * s)),
    )
    .min_size(size(px(MIN_SIZE.0 * s), px(MIN_SIZE.1 * s)))
    .key(WindowKey::AdditionalSetup);
    let open = move |cx: &mut App| {
        TlSecondaryWindow::open(cx, spec, move |_, cx| {
            cx.new(|cx| SetupWindow::new(focus_row, cx))
        })
        .map(Into::into)
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            let opened = registry.update(cx, |r, cx| {
                r.open_or_focus(WindowKey::AdditionalSetup, cx, open)
            });
            if let Some(row) = focus_row
                && opened == OpenOutcome::Focused
            {
                focus_open_window(row, cx);
            }
        }
        None => {
            let _ = open(cx);
        }
    }
    request_status(cx);
}

fn focus_open_window(row: SetupRow, cx: &mut App) {
    let Some(handle) = cx
        .try_global::<GlobalRegistry>()
        .and_then(|r| r.0.read(cx).get(&WindowKey::AdditionalSetup))
        .and_then(|h| h.downcast::<gpui_kit::base::Root>())
    else {
        return;
    };
    let Some(frame) = TlSecondaryWindow::<SetupWindow>::find(handle, cx) else {
        return;
    };
    let content = frame.read(cx).content().clone();
    content.update(cx, |w, cx| w.focus(row, cx));
}

fn request_status(cx: &App) {
    let Some(engine) = cx.try_global::<EngineGlobal>() else {
        return;
    };
    if let Err(err) = engine.0.send(Command::SetupStatus) {
        warn!(target: "tasklens::ui", ?err, "setup status not requested");
    }
}

/// The words of one confirmation dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmWords {
    pub heading: SharedString,
    pub text: SharedString,
    pub action: SharedString,
    pub destructive: bool,
}

/// The confirmation texts of `step`. Only the CPU power draw steps have a card so far.
pub fn confirm_words(step: SetupStep, cx: &App) -> ConfirmWords {
    let (heading, text, action, destructive) = match step {
        SetupStep::RaplRevert => (
            "setup-confirm-revert-title",
            "setup-confirm-revert-text",
            "setup-button-revert",
            true,
        ),
        _ => (
            "setup-confirm-enable-title",
            "setup-confirm-enable-text",
            "setup-button-enable",
            false,
        ),
    };
    ConfirmWords {
        heading: tr(cx, heading),
        text: tr(cx, text),
        action: tr(cx, action),
        destructive,
    }
}

/// Opens the confirmation of `step`. Confirm sends `Command::RunSetup`. Focus starts on Cancel
/// for a destructive dialog and on the action button otherwise (R-104).
pub fn confirm_step(step: SetupStep, window: &mut Window, cx: &mut App) {
    let words = confirm_words(step, cx);
    let mut dialog = TlDialog::confirm(words.text, words.action)
        .heading(words.heading)
        .cancel_label(tr(cx, "shell-confirm-cancel"));
    if words.destructive {
        dialog = dialog.destructive();
    }
    let opened = dialog.open(window, cx, move |result, _, cx| {
        if let DialogResult::Confirmed { .. } = result {
            send_step(step, cx);
        }
    });
    if opened.is_err() {
        warn!(target: "tasklens::ui", "setup confirmation did not open");
    }
}

/// Sends `Command::RunSetup` for `step`. A full queue or a stopped engine changes nothing.
pub fn send_step(step: SetupStep, cx: &App) {
    let Some(engine) = cx.try_global::<EngineGlobal>() else {
        return;
    };
    if let Err(err) = engine.0.send(Command::RunSetup { step }) {
        warn!(target: "tasklens::ui", ?err, "setup step not sent");
    }
}

fn send_reprobe(source: SourceId, cx: &App) {
    let Some(engine) = cx.try_global::<EngineGlobal>() else {
        return;
    };
    if let Err(err) = engine.0.send(Command::ReprobeSource(source)) {
        warn!(target: "tasklens::ui", ?err, ?source, "reprobe not sent");
    }
}

/// The card a step belongs to.
fn step_row(step: SetupStep) -> Option<SetupRow> {
    match step {
        SetupStep::RaplEnable | SetupStep::RaplRevert => Some(SetupRow::CpuPower),
        SetupStep::SensorsDetect | SetupStep::SensorsRevert => Some(SetupRow::FanSensors),
        _ => None,
    }
}

/// The last result of a step that this window shows under the buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Done { step: SetupStep },
    Failed { step: SetupStep, text: Arc<str> },
}

/// What a card button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonAction {
    /// Enable or Revert: opens the confirmation.
    Confirm(SetupStep),
    /// Sends the step with no confirmation.
    Run(SetupStep),
    ViewScript(SetupStep),
    CopyCommand(SetupStep),
    CopyJournal,
}

/// One button of a card's buttons row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ButtonSpec {
    label_key: &'static str,
    action: ButtonAction,
    disabled: bool,
}

/// The step whose script the CPU card shows: Enable while the files are not readable, else
/// Revert (also for a rule that tasklens did not write, which has no Revert button).
fn script_step(rapl: RaplStatus) -> SetupStep {
    match rapl {
        RaplStatus::NotReadable => SetupStep::RaplEnable,
        _ => SetupStep::RaplRevert,
    }
}

/// The subcommand of a script step, as `tasklens-setup` takes it.
fn subcommand(step: SetupStep) -> &'static str {
    step.args()[0]
}

/// The command line the Copy command button copies.
pub fn setup_command_line(step: SetupStep) -> String {
    format!(
        "pkexec /usr/lib/tasklens/tasklens-setup {}",
        subcommand(step)
    )
}

/// The buttons of `row`. None before the engine answered, and none for a status without a
/// change to make or a script to show.
fn row_buttons(
    row: SetupRow,
    st: Option<&SetupStatus>,
    running: Option<SetupStep>,
) -> Vec<ButtonSpec> {
    let Some(st) = st else {
        return Vec::new();
    };
    let spec = |label_key, action| ButtonSpec {
        label_key,
        action,
        disabled: false,
    };
    match row {
        SetupRow::CpuPower => {
            let mut buttons = Vec::new();
            let change = match st.rapl {
                RaplStatus::NotReadable => Some(("setup-button-enable", SetupStep::RaplEnable)),
                RaplStatus::Readable {
                    tasklens_rule: true,
                } => Some(("setup-button-revert", SetupStep::RaplRevert)),
                _ => None,
            };
            if let Some((label_key, step)) = change {
                buttons.push(ButtonSpec {
                    disabled: running.is_some_and(|r| step_row(r) == Some(row)),
                    ..spec(label_key, ButtonAction::Confirm(step))
                });
            }
            if st.rapl != RaplStatus::NoZone {
                let step = script_step(st.rapl);
                buttons.push(spec(
                    "setup-button-view-script",
                    ButtonAction::ViewScript(step),
                ));
                buttons.push(spec(
                    "setup-button-copy-command",
                    ButtonAction::CopyCommand(step),
                ));
            }
            buttons
        }
        SetupRow::FanSensors => {
            if st.hwmon_fan || !st.lm_sensors {
                return Vec::new();
            }
            let (label_key, step) = if st.sensors_conf {
                ("setup-button-revert", SetupStep::SensorsRevert)
            } else {
                ("setup-button-run-detection", SetupStep::SensorsDetect)
            };
            vec![
                ButtonSpec {
                    disabled: running.is_some_and(|r| step_row(r) == Some(row)),
                    ..spec(label_key, ButtonAction::Run(step))
                },
                spec("setup-button-view-script", ButtonAction::ViewScript(step)),
                spec("setup-button-copy-command", ButtonAction::CopyCommand(step)),
            ]
        }
        SetupRow::JournalAccess => vec![spec("setup-button-copy", ButtonAction::CopyJournal)],
        _ => Vec::new(),
    }
}

/// Pushes a Success toast with the text of `key` on `window`.
fn toast(key: &str, window: &mut Window, cx: &mut App) {
    let text = tr(cx, key);
    if let Some(queue) = ToastQueue::of_window(window, cx) {
        queue.update(cx, |q, cx| q.push(ToastKind::Success, text, None, None, cx));
    }
}

/// Copies `text` and shows the toast of `key`.
fn copy_with_toast(text: String, key: &str, window: &mut Window, cx: &mut App) {
    if clipboard_write(cx, text).is_ok() {
        toast(key, window, cx);
    }
}

/// Opens the script window of `step`, or focuses it. Without a window registry (tests) it always
/// opens one.
pub fn open_script_window(step: SetupStep, cx: &mut App) {
    let name = subcommand(step);
    let s = metrics(cx).ui_scale;
    let spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        tr_args(cx, "setup-script-title", &[("subcommand", name.into())]),
        size(px(SCRIPT_SIZE.0 * s), px(SCRIPT_SIZE.1 * s)),
    )
    .key(WindowKey::SetupScript(name));
    let open = move |cx: &mut App| {
        TlSecondaryWindow::open(cx, spec, |_, cx| cx.new(|_| ScriptWindow::new())).map(Into::into)
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            registry.update(cx, |r, cx| {
                r.open_or_focus(WindowKey::SetupScript(name), cx, open)
            });
        }
        None => {
            let _ = open(cx);
        }
    }
}

/// The content of a script window: the packaged script, read-only, with a Copy button.
pub struct ScriptWindow {
    scroll: ScrollHandle,
}

/// Copies the script text.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = tasklens, no_json)]
pub struct SetupCopyScript;

impl ScriptWindow {
    fn new() -> Self {
        Self {
            scroll: ScrollHandle::new(),
        }
    }

    /// The text shown.
    pub fn text(&self) -> &'static str {
        SETUP_SCRIPT
    }

    /// The Copy button.
    pub fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        copy_with_toast(SETUP_SCRIPT.to_owned(), "shell-toast-copied", window, cx);
    }
}

impl gpui_kit::Render for ScriptWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let mut root = div()
            .id("SetupScript")
            .size_full()
            .flex()
            .flex_col()
            .gap(m.sp[4])
            .p(m.sp[5])
            .bg(theme.window_bg)
            .text_color(theme.text_primary)
            .on_action(cx.listener(|this, _: &SetupCopyScript, window, cx| {
                this.copy(window, cx);
            }))
            .child(
                div()
                    .id("SetupScript-text")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(m.sp[3])
                    .bg(theme.input)
                    .rounded(m.radius_small)
                    .font_family(MONO_FAMILY)
                    .child(SharedString::from(SETUP_SCRIPT)),
            )
            .child(
                div().flex().flex_row().child(
                    TlButton::new("SetupScript-copy", TlButtonKind::Primary)
                        .size(TlButtonSize::Sm)
                        .label(tr(cx, "setup-button-copy"))
                        .action(Box::new(SetupCopyScript)),
                ),
            );
        *root.text_style() = t_sm(&m);
        root
    }
}

/// The window's own text, translated once per strings version (R-132).
struct SetupWords {
    titles: [SharedString; 5],
    descriptions: [SharedString; 5],
    /// The Fan sensors description while lm_sensors is not installed.
    fan_missing: SharedString,
}

impl SetupWords {
    fn new(cx: &App) -> Self {
        Self {
            titles: CARDS.map(|r| tr(cx, r.title_key())),
            descriptions: CARDS.map(|r| tr(cx, r.description_key())),
            fan_missing: tr(cx, "setup-row-fan-description-no-lm-sensors"),
        }
    }
}

fn strings_version(cx: &App) -> u64 {
    cx.try_global::<Strings>().map_or(0, |s| s.strings_version)
}

/// One card as shown, for tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardView {
    pub row: SetupRow,
    pub title: String,
    pub description: String,
    /// The badge text; `None` while the spinner shows.
    pub status: Option<String>,
    pub tooltip: Option<String>,
    /// The buttons: label and whether it is disabled.
    pub buttons: Vec<(String, bool)>,
    /// The success line under the buttons.
    pub done: Option<String>,
    /// The output of a failed step, shown in the failure block.
    pub failure: Option<String>,
}

/// The content of the Additional Setup window.
pub struct SetupWindow {
    scroll: ScrollHandle,
    status: Option<SetupStatus>,
    /// The step that runs now.
    running: Option<SetupStep>,
    /// The `seq` of the newest finished run this window has handled.
    handled_seq: u64,
    outcome: Option<Outcome>,
    flash: Option<SetupRow>,
    words: SetupWords,
    strings_version: u64,
    _strings: Subscription,
    _hub_sub: Option<Subscription>,
    _flash_timer: Option<Task<()>>,
}

impl SetupWindow {
    fn new(focus_row: Option<SetupRow>, cx: &mut Context<Self>) -> Self {
        let strings = cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = strings_version(cx);
            if version != this.strings_version {
                this.strings_version = version;
                this.words = SetupWords::new(cx);
                cx.notify();
            }
        });
        let hub_sub = cx
            .try_global::<GlobalHub>()
            .map(|h| h.0.clone())
            .map(|hub| {
                cx.subscribe(&hub, |this: &mut Self, hub, ev: &SnapshotChanged, cx| {
                    if ev.mask.contains(ComponentMask::HEALTH) {
                        let setup = hub.read(cx).current().health.setup.clone();
                        this.apply_setup(&setup, cx);
                    }
                })
            });
        let setup = cx
            .try_global::<GlobalHub>()
            .map(|h| h.0.read(cx).current().health.setup.clone())
            .unwrap_or_default();
        // A finished run from before the window opened shows nothing and sends nothing.
        let handled_seq = match &setup.run {
            Some(run) if run.phase != SetupPhase::Running => run.seq,
            _ => 0,
        };
        let mut this = Self {
            scroll: ScrollHandle::new(),
            status: setup.status,
            running: match &setup.run {
                Some(SetupRun {
                    step,
                    phase: SetupPhase::Running,
                    ..
                }) => Some(*step),
                _ => None,
            },
            handled_seq,
            outcome: None,
            flash: None,
            words: SetupWords::new(cx),
            strings_version: strings_version(cx),
            _strings: strings,
            _hub_sub: hub_sub,
            _flash_timer: None,
        };
        if let Some(row) = focus_row {
            this.focus(row, cx);
        }
        this
    }

    /// Takes the setup part of a health snapshot: the status, the running step and the result
    /// of a run newer than the last one handled.
    fn apply_setup(&mut self, setup: &SetupState, cx: &mut Context<Self>) {
        let mut changed = setup.status != self.status;
        self.status = setup.status;
        let mut running = None;
        if let Some(run) = &setup.run {
            match &run.phase {
                SetupPhase::Running => {
                    running = Some(run.step);
                    if self.outcome.take().is_some() {
                        changed = true;
                    }
                }
                phase if run.seq > self.handled_seq => {
                    self.handled_seq = run.seq;
                    changed = true;
                    self.outcome = match phase {
                        SetupPhase::Succeeded => {
                            match step_row(run.step) {
                                Some(SetupRow::CpuPower) => send_reprobe(SourceId::Cpu, cx),
                                Some(SetupRow::FanSensors) => send_reprobe(SourceId::Sensors, cx),
                                _ => {}
                            }
                            Some(Outcome::Done { step: run.step })
                        }
                        SetupPhase::Failed(text) => Some(Outcome::Failed {
                            step: run.step,
                            text: Arc::clone(text),
                        }),
                        _ => None,
                    };
                }
                _ => {}
            }
        }
        if running != self.running {
            self.running = running;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// The Enable button of the CPU card.
    pub fn press_enable(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        confirm_step(SetupStep::RaplEnable, window, cx);
    }

    /// The Revert button of the CPU card.
    pub fn press_revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        confirm_step(SetupStep::RaplRevert, window, cx);
    }

    /// The Run sensor detection and Revert buttons of the Fan sensors card. No confirmation.
    pub fn press_run(&mut self, step: SetupStep, cx: &mut Context<Self>) {
        send_step(step, cx);
    }

    /// The Retry button of a failure block.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if let Some(Outcome::Failed { step, .. }) = &self.outcome {
            send_step(*step, cx);
        }
    }

    /// The Copy output button of a failure block.
    pub fn copy_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Outcome::Failed { text, .. }) = &self.outcome else {
            return;
        };
        if clipboard_write(cx, text.to_string()).is_err() {
            return;
        }
        toast("shell-toast-copied", window, cx);
    }

    /// The Copy command button of the CPU card.
    pub fn copy_command(&mut self, step: SetupStep, window: &mut Window, cx: &mut Context<Self>) {
        copy_with_toast(
            setup_command_line(step),
            "setup-toast-command-copied",
            window,
            cx,
        );
    }

    /// The Copy button of the Journal access card. It sends nothing to the engine.
    pub fn copy_journal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = tr(cx, "setup-row-journal-command").to_string();
        copy_with_toast(command, "setup-toast-command-copied", window, cx);
    }

    /// Scrolls the card of `row` into view and flashes its border for `M_SLOW`. With reduced
    /// motion the border does not flash.
    pub fn focus(&mut self, row: SetupRow, cx: &mut Context<Self>) {
        let Some(ix) = row.card_index() else {
            return;
        };
        self.scroll.scroll_to_item(ix);
        if cx.reduce_motion() {
            return;
        }
        self.flash = Some(row);
        self._flash_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(flash_duration()).await;
            let _ = this.update(cx, |this, cx| {
                this.flash = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The card whose border flashes now.
    pub fn flashing(&self) -> Option<SetupRow> {
        self.flash
    }

    /// The cards, top to bottom.
    pub fn cards(&self, cx: &App) -> Vec<CardView> {
        CARDS
            .iter()
            .enumerate()
            .map(|(ix, &row)| {
                let st = row_status(row, self.status.as_ref());
                let (done, failure) = self.outcome_of(row);
                CardView {
                    row,
                    title: self.words.titles[ix].to_string(),
                    description: self.description_of(row, ix).to_string(),
                    status: if self.running.is_some_and(|r| step_row(r) == Some(row)) {
                        Some(tr(cx, "setup-status-running").to_string())
                    } else {
                        st.map(|s| tr(cx, s.text_key).to_string())
                    },
                    tooltip: st
                        .and_then(|s| s.tooltip_key)
                        .map(|k| tr(cx, k).to_string()),
                    buttons: row_buttons(row, self.status.as_ref(), self.running)
                        .iter()
                        .map(|b| (tr(cx, b.label_key).to_string(), b.disabled))
                        .collect(),
                    done: done.then(|| tr(cx, "setup-done").to_string()),
                    failure: failure.map(|t| t.to_string()),
                }
            })
            .collect()
    }
}

impl SetupWindow {
    /// The description of the card at `ix`. Fan sensors names the missing package when
    /// lm_sensors is not installed.
    fn description_of(&self, row: SetupRow, ix: usize) -> SharedString {
        let fan_missing = row == SetupRow::FanSensors
            && self
                .status
                .as_ref()
                .is_some_and(|st| !st.hwmon_fan && !st.lm_sensors);
        if fan_missing {
            self.words.fan_missing.clone()
        } else {
            self.words.descriptions[ix].clone()
        }
    }

    /// Whether the success line shows on `row`, and the output of a failure there.
    fn outcome_of(&self, row: SetupRow) -> (bool, Option<Arc<str>>) {
        match &self.outcome {
            Some(Outcome::Done { step }) if step_row(*step) == Some(row) => (true, None),
            Some(Outcome::Failed { step, text }) if step_row(*step) == Some(row) => {
                (false, Some(Arc::clone(text)))
            }
            _ => (false, None),
        }
    }
}

fn flash_duration() -> Duration {
    M_SLOW
}

impl gpui_kit::Render for SetupWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let mut cards = div()
            .id("AdditionalSetup-scroll")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .p(px(PAGE_PAD * s))
            .gap(m.sp[4])
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        for (ix, &row) in CARDS.iter().enumerate() {
            let st = row_status(row, self.status.as_ref());
            let pending = self.status.is_none();
            let code =
                (row == SetupRow::JournalAccess).then(|| tr(cx, "setup-row-journal-command"));
            let tooltip = st.and_then(|s| s.tooltip_key).map(|k| tr(cx, k));
            let running = self.running.is_some_and(|r| step_row(r) == Some(row));
            let badge = if running {
                Some(TlBadge::new(
                    TlBadgeKind::Neutral,
                    tr(cx, "setup-status-running"),
                ))
            } else {
                st.map(|s| {
                    TlBadge::new(s.kind, tr(cx, s.text_key)).max_width(STATUS_BADGE_MAX_WIDTH)
                })
            };
            let buttons = row_buttons(row, self.status.as_ref(), self.running)
                .into_iter()
                .map(|b| (b, tr(cx, b.label_key)))
                .collect();
            let (done, failure) = self.outcome_of(row);
            let failure = failure
                .zip(self.outcome.as_ref())
                .and_then(|(text, o)| match o {
                    Outcome::Failed { step, .. } => Some((*step, text)),
                    Outcome::Done { .. } => None,
                });
            cards = cards.child(SetupRowCard {
                row,
                title: self.words.titles[ix].clone(),
                description: self.description_of(row, ix),
                code,
                badge,
                tooltip,
                pending,
                running,
                buttons,
                done: done.then(|| tr(cx, "setup-done")),
                failure,
                flashing: self.flash == Some(row),
            });
        }
        let mut root = div()
            .id("AdditionalSetup")
            .size_full()
            .bg(theme.window_bg)
            .text_color(theme.text_primary)
            .on_action(cx.listener(|_, a: &SetupConfirmStep, window, cx| {
                confirm_step(a.step, window, cx);
            }))
            .on_action(cx.listener(|_, a: &SetupViewScript, _, cx| open_script_window(a.step, cx)))
            .on_action(cx.listener(|this, a: &SetupCopyCommand, window, cx| {
                this.copy_command(a.step, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SetupCopyJournal, window, cx| {
                this.copy_journal(window, cx);
            }))
            .on_action(cx.listener(|this, a: &SetupRunStep, _, cx| {
                this.press_run(a.step, cx);
            }))
            .on_action(cx.listener(|_, a: &SetupRetryStep, _, cx| send_step(a.step, cx)))
            .on_action(cx.listener(|this, _: &SetupCopyOutput, window, cx| {
                this.copy_output(window, cx);
            }))
            .child(cards);
        *root.text_style() = t_sm(&m);
        root
    }
}

/// One card of the window (UI-PRF-052).
#[derive(IntoElement)]
struct SetupRowCard {
    row: SetupRow,
    title: SharedString,
    description: SharedString,
    /// The command line shown under the description.
    code: Option<SharedString>,
    badge: Option<TlBadge>,
    tooltip: Option<SharedString>,
    /// No answer from the engine yet: the spinner takes the badge's place.
    pending: bool,
    /// A step of this card runs: the badge reads "Running…" with a spinner.
    running: bool,
    buttons: Vec<(ButtonSpec, SharedString)>,
    done: Option<SharedString>,
    /// The failed step and its output.
    failure: Option<(SetupStep, Arc<str>)>,
    flashing: bool,
}

impl RenderOnce for SetupRowCard {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let ix = self.row.card_index().unwrap_or_default();

        let mut title = div().flex_1().min_w_0().child(self.title);
        *title.text_style() = t_md(&m);
        title = title.font_weight(gpui_kit::FontWeight::MEDIUM);

        let status = if self.pending {
            div()
                .flex_none()
                .child(TlSpinner::new(px(SPINNER * s)))
                .into_any_element()
        } else {
            let mut slot = div().id(("AdditionalSetup-status", ix)).flex_none();
            if !self.running {
                slot = slot.children(self.badge.take());
            }
            if let Some(text) = self.tooltip {
                let tip = HoverTooltip::state(("AdditionalSetup-status-tip", ix), window, cx);
                slot = HoverTooltip::attach(
                    &tip,
                    slot,
                    DELAY_CONTROL,
                    TlTooltip::text(text).build(),
                    window,
                    cx,
                );
            }
            if self.running {
                slot = slot
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(m.sp[2])
                    .child(TlSpinner::new(px(SPINNER * s)));
                slot = slot.children(self.badge);
            }
            slot.into_any_element()
        };

        let mut card = div()
            .id(("AdditionalSetup-card", ix))
            .w_full()
            .max_w(px(CONTENT_WIDTH * s))
            .flex_none()
            .flex()
            .flex_col()
            .p(m.sp[5])
            .bg(theme.list_bg)
            .border_1()
            .border_color(if self.flashing {
                theme.accent
            } else {
                theme.divider
            })
            .rounded(m.radius_card)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(m.sp[4])
                    .child(title)
                    .child(status),
            )
            .child(
                div()
                    .mt(m.sp[2])
                    .text_color(theme.text_secondary)
                    .child(self.description),
            );
        if let Some(code) = self.code {
            card = card.child(
                div()
                    .mt(m.sp[3])
                    .p(m.sp[3])
                    .bg(theme.input)
                    .rounded(m.radius_small)
                    .font_family(MONO_FAMILY)
                    .child(code),
            );
        }
        // The buttons row, `sp4` below. `row_buttons` lists them.
        if !self.buttons.is_empty() {
            let mut row = div().mt(m.sp[4]).flex().flex_row().gap(m.sp[3]);
            for (b, label) in self.buttons {
                let button = match b.action {
                    ButtonAction::Confirm(step) => {
                        TlButton::new(("AdditionalSetup-button", ix), TlButtonKind::Primary)
                            .action(Box::new(SetupConfirmStep { step }))
                    }
                    ButtonAction::Run(step) => {
                        TlButton::new(("AdditionalSetup-button", ix), TlButtonKind::Primary)
                            .action(Box::new(SetupRunStep { step }))
                    }
                    ButtonAction::ViewScript(step) => {
                        TlButton::new(("AdditionalSetup-view", ix), TlButtonKind::Secondary)
                            .action(Box::new(SetupViewScript { step }))
                    }
                    ButtonAction::CopyCommand(step) => {
                        TlButton::new(("AdditionalSetup-copy-cmd", ix), TlButtonKind::Secondary)
                            .action(Box::new(SetupCopyCommand { step }))
                    }
                    ButtonAction::CopyJournal => TlButton::new(
                        ("AdditionalSetup-copy-journal", ix),
                        TlButtonKind::Secondary,
                    )
                    .action(Box::new(SetupCopyJournal)),
                };
                row = row.child(
                    button
                        .size(TlButtonSize::Sm)
                        .label(label)
                        .disabled(b.disabled),
                );
            }
            card = card.child(row);
        }
        if let Some(done) = self.done {
            card = card.child(
                div()
                    .mt(m.sp[4])
                    .text_color(theme.status.success)
                    .child(done),
            );
        }
        if let Some((step, output)) = self.failure {
            let mut heading = div().child(tr(cx, "setup-failed"));
            *heading.text_style() = t_sm(&m);
            heading = heading
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(theme.status.danger);
            card = card.child(
                div()
                    .mt(m.sp[4])
                    .flex()
                    .flex_col()
                    .gap(m.sp[3])
                    .child(heading)
                    .child(
                        div()
                            .id(("AdditionalSetup-output", ix))
                            .max_h(px(OUTPUT_MAX_HEIGHT * s))
                            .overflow_y_scroll()
                            .p(m.sp[3])
                            .bg(theme.input)
                            .rounded(m.radius_small)
                            .font_family(MONO_FAMILY)
                            .child(SharedString::from(output.to_string())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(m.sp[3])
                            .child(
                                TlButton::new(
                                    ("AdditionalSetup-copy", ix),
                                    TlButtonKind::Secondary,
                                )
                                .size(TlButtonSize::Sm)
                                .label(tr(cx, "setup-button-copy-output"))
                                .action(Box::new(SetupCopyOutput)),
                            )
                            .child(
                                TlButton::new(
                                    ("AdditionalSetup-retry", ix),
                                    TlButtonKind::Secondary,
                                )
                                .size(TlButtonSize::Sm)
                                .label(tr(cx, "setup-button-retry"))
                                .action(Box::new(SetupRetryStep { step })),
                            ),
                    ),
            );
        }
        card
    }
}
