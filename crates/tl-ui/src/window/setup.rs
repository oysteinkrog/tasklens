//! "tasklens Additional Setup": one card per optional system change with its status
//! (R-028, PERF-105, APP-042, UI-SHL-094, UI-PRF-052).
//!
//! A Tool window keyed `WindowKey::AdditionalSetup`. `OpenAdditionalSetup` opens or focuses it
//! and asks the engine for the status once per dispatch. The statuses come from
//! `HealthSnapshot.setup.status`; the window reads no system file (ADR-001 rule 1). The buttons
//! row of each card is empty here: Enable and Revert are E29-025, View script and Copy command
//! are E29-026, the Fan sensors card is E29-031.

use std::time::Duration;

use gpui_kit::{
    Action, App, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement,
    RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    Task, Window, div, px, size,
};
use tl_model::command::{Command, RaplStatus, SetupStatus};
use tracing::warn;

use super::registry::{GlobalRegistry, OpenOutcome, WindowKey};
use crate::app::EngineGlobal;
use crate::components::badge::{TlBadge, TlBadgeKind};
use crate::components::motion::M_SLOW;
use crate::components::progress::TlSpinner;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::{Strings, tr};
use crate::text::fonts::MONO_FAMILY;
use crate::text::style::{t_md, t_sm};
use crate::theme::{current, metrics};

/// Window size, × ui_scale.
const SIZE: (f32, f32) = (640., 520.);
const MIN_SIZE: (f32, f32) = (560., 400.);
/// Content width, × ui_scale.
const CONTENT_WIDTH: f32 = 560.;
/// Page padding, × ui_scale.
const PAGE_PAD: f32 = 16.;
/// Spinner size, × ui_scale.
const SPINNER: f32 = 12.;

const TITLE: &str = "setup-title";

/// The cards of the window. `FanSensors` has no card until E29-031.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SetupRow {
    CpuPower,
    JournalAccess,
    AdminHelper,
    FanSensors,
    ProcessNetwork,
}

/// The shown cards, top to bottom.
const CARDS: [SetupRow; 4] = [
    SetupRow::CpuPower,
    SetupRow::JournalAccess,
    SetupRow::AdminHelper,
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
        SetupRow::FanSensors | SetupRow::ProcessNetwork => {
            status(TlBadgeKind::Neutral, "setup-status-not-needed", None)
        }
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

/// The window's own text, translated once per strings version (R-132).
struct SetupWords {
    titles: [SharedString; 4],
    descriptions: [SharedString; 4],
}

impl SetupWords {
    fn new(cx: &App) -> Self {
        Self {
            titles: CARDS.map(|r| tr(cx, r.title_key())),
            descriptions: CARDS.map(|r| tr(cx, r.description_key())),
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
    /// The badge text; `None` while the spinner shows.
    pub status: Option<String>,
    pub tooltip: Option<String>,
}

/// The content of the Additional Setup window.
pub struct SetupWindow {
    scroll: ScrollHandle,
    status: Option<SetupStatus>,
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
                        let status = hub.read(cx).current().health.setup.status;
                        if status != this.status {
                            this.status = status;
                            cx.notify();
                        }
                    }
                })
            });
        let status = cx
            .try_global::<GlobalHub>()
            .and_then(|h| h.0.read(cx).current().health.setup.status);
        let mut this = Self {
            scroll: ScrollHandle::new(),
            status,
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

    /// Scrolls the card of `row` into view and flashes its border for `M_SLOW`. With reduced
    /// motion the border does not flash. `FanSensors` has no card yet.
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
                CardView {
                    row,
                    title: self.words.titles[ix].to_string(),
                    status: st.map(|s| tr(cx, s.text_key).to_string()),
                    tooltip: st
                        .and_then(|s| s.tooltip_key)
                        .map(|k| tr(cx, k).to_string()),
                }
            })
            .collect()
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
            let badge = st.map(|s| TlBadge::new(s.kind, tr(cx, s.text_key)));
            cards = cards.child(SetupRowCard {
                row,
                title: self.words.titles[ix].clone(),
                description: self.words.descriptions[ix].clone(),
                code,
                badge,
                tooltip,
                pending,
                flashing: self.flash == Some(row),
            });
        }
        let mut root = div()
            .id("AdditionalSetup")
            .size_full()
            .bg(theme.window_bg)
            .text_color(theme.text_primary)
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
    flashing: bool,
}

impl RenderOnce for SetupRowCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
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
            let mut slot = div()
                .id(("AdditionalSetup-status", ix))
                .flex_none()
                .children(self.badge);
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
        // The buttons row (E29-025, E29-026) goes here, `sp4` below.
        card
    }
}
