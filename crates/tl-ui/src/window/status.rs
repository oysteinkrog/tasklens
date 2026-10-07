//! The floating status chip at the bottom-left of the page area (E20-006, UI-SHL-050, R-107,
//! APP-029, PROC-051, R-042).
//!
//! The chip shows the active page's status text, the page's clickable status actions and
//! "[PAUSED - Ctrl]" while Ctrl-pause is on (E20-012), then the health items (E20-016,
//! UI-SHL-051). With nothing to show it renders an empty element of zero size, so Performance and
//! Information normally show no chip.
//!
//! Pages emit [`StatusChanged`] on their own entity when their status text changes. `MainWindow`
//! (E20-008) then reads the page's `status_text` and `status_actions` and passes them to
//! [`StatusChip::set_page_text`] and [`StatusChip::set_page_actions`]. Count formatting (U+2009
//! thin-space grouping, table d) is the pages' job; the chip draws the text as given.
//!
//! The chip's text is one `StyledText`, cut at the end with "…" when the parent's `max_w` is too
//! small. The page actions and the health items are Tab stops; nothing else on the chip is
//! clickable.
//!
//! Health items: `MainWindow` passes the health snapshot and the active tab to
//! [`StatusChip::set_health`]. A source of the active tab ([`tab_sources`]) that is Stale, Failed
//! or Disabled gives one item; collection errors show here and never as toasts (ADR-015). A click
//! or Enter on an item dispatches `OpenCollectorHealth`.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::accesskit::Role;
use gpui_kit::{
    AbsoluteLength, Action, App, Context, Div, ElementId, FocusHandle, HighlightStyle, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, StyledText, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use tl_model::command::Tab;
use tl_model::health::{HealthSnapshot, SourceId, SourceState};
use tl_model::time::TimeNs;

use crate::actions::OpenCollectorHealth;
use crate::components::badge::{TlBadge, TlBadgeKind};
use crate::components::focus::TlFocusRing;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::i18n::{Strings, tr};
use crate::pages::performance::details::Template;
use crate::text::style::numeric_style;
use crate::text::untrusted::debug_assert_clean;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Chip height, × ui_scale. Tables add this as bottom padding (UI-SHL-050).
pub const CHIP_H: f32 = 26.0;
/// The chip's max width is the page width minus this, × ui_scale.
pub const CHIP_RESERVE: f32 = 200.0;

/// Left padding, × ui_scale.
const PAD_L: f32 = 8.0;
/// Right padding, × ui_scale.
const PAD_R: f32 = 12.0;
/// The separator between text segments.
const SEP: &str = " | ";
/// The separator before and between health items (UI-SHL-050 segment 3).
const HEALTH_SEP: &str = " \u{b7} ";
/// Health items shown before "+N more" (UI-SHL-051).
const MAX_ITEMS: usize = 2;
/// How long the restricted item shows after a refusal (UI-FND-028).
const RESTRICTED_FOR: Duration = Duration::from_secs(10);

/// One health item: the warning text and its tooltip.
#[derive(Clone, PartialEq, Debug)]
pub struct HealthItem {
    pub source: SourceId,
    pub text: SharedString,
    pub tooltip: SharedString,
}

/// The sources the active tab uses, in item order (UI-SHL-051, decided in E20-016). The
/// minigraph cards on the Processes tab use the last six of its list.
pub fn tab_sources(tab: Tab) -> &'static [SourceId] {
    use SourceId::*;
    match tab {
        Tab::Processes => &[Processes, Cpu, Memory, Disk, Net, GpuNvidia, GpuDrm],
        Tab::Performance => &[Cpu, Memory, Disk, Net, GpuNvidia, GpuDrm, Sensors],
        Tab::Services => &[SystemdSystem, SystemdUser],
        Tab::Startup => &[Startup],
        Tab::Connections => &[Connections],
        Tab::Information => &[Inventory],
    }
}

/// The en-US display name of a source in a health item (UI-SHL-051). Two sources with the same
/// name give one item.
pub fn source_name(id: SourceId) -> &'static str {
    match id {
        SourceId::Cpu => "CPU",
        SourceId::Memory => "Memory",
        SourceId::Disk => "Disks",
        SourceId::Net => "Network",
        SourceId::GpuNvidia | SourceId::GpuDrm => "GPU",
        SourceId::Sensors => "Sensors",
        SourceId::Processes => "Processes",
        SourceId::ProcessDetail => "Process details",
        SourceId::Connections => "Connections",
        SourceId::SystemdSystem | SourceId::SystemdUser => "Services",
        SourceId::Logind => "Logind",
        SourceId::Startup => "Startup",
        SourceId::Inventory => "Inventory",
        SourceId::KWin => "KWin",
        SourceId::Udisks => "UDisks",
        SourceId::UPower => "UPower",
        SourceId::NetworkManager => "NetworkManager",
        SourceId::HistoryRecorder => "History",
        SourceId::Journal => "Journal",
    }
}

/// The translation key of [`source_name`]. The Collector Health window's keys where the names
/// match; GPU and Services have chip keys of their own. Logind, UDisks and UPower are on no tab,
/// so the chip never shows them.
fn source_key(id: SourceId) -> &'static str {
    match id {
        SourceId::Cpu => "shell-health-source-cpu",
        SourceId::Memory => "shell-health-source-memory",
        SourceId::Disk => "shell-health-source-disk",
        SourceId::Net => "shell-health-source-net",
        SourceId::GpuNvidia | SourceId::GpuDrm => "shell-health-chip-gpu",
        SourceId::Sensors => "shell-health-source-sensors",
        SourceId::Processes => "shell-health-source-processes",
        SourceId::ProcessDetail => "shell-health-source-process-detail",
        SourceId::Connections => "shell-health-source-connections",
        SourceId::SystemdSystem | SourceId::SystemdUser => "shell-health-chip-services",
        SourceId::Logind => "shell-health-source-logind",
        SourceId::Startup => "shell-health-source-startup",
        SourceId::Inventory => "shell-health-source-inventory",
        SourceId::KWin => "shell-health-source-kwin",
        SourceId::Udisks => "shell-health-source-udisks",
        SourceId::UPower => "shell-health-source-upower",
        SourceId::NetworkManager => "shell-health-source-network-manager",
        SourceId::HistoryRecorder => "shell-health-source-history",
        SourceId::Journal => "shell-health-source-journal",
    }
}

/// The health item words, translated once per strings version (R-132).
struct HealthWords {
    names: [SharedString; SourceId::COUNT],
    stale: Template,
    failed: Template,
    disabled: Template,
    more: Template,
    no_data: Template,
    restricted: Template,
    unknown_error: SharedString,
    restarting: SharedString,
    open: SharedString,
}

impl HealthWords {
    fn new(cx: &App) -> Self {
        Self {
            names: SourceId::ALL.map(|id| tr(cx, source_key(id))),
            stale: Template::new(cx, "shell-health-stale", &["source"]),
            failed: Template::new(cx, "shell-health-failed", &["source"]),
            disabled: Template::new(cx, "shell-health-disabled", &["source"]),
            more: Template::new(cx, "shell-health-more", &["n"]),
            no_data: Template::new(cx, "shell-health-no-data", &["secs"]),
            restricted: Template::new(cx, "shell-health-restricted", &["n"]),
            unknown_error: tr(cx, "shell-health-unknown-error"),
            restarting: tr(cx, "shell-health-restarting"),
            open: tr(cx, "shell-health-open"),
        }
    }
}

/// What one health item shows, compared before any text is built.
#[derive(Clone, PartialEq, Debug)]
enum ItemKey {
    /// Whole seconds since the last fresh data; `None` when there was none.
    Stale(SourceId, Option<u64>),
    Failed(SourceId, Option<Arc<str>>),
    Disabled(SourceId, Option<Arc<str>>),
}

/// The keys of every item for `tab`, before the limit of two.
fn item_keys(health: &HealthSnapshot, tab: Tab, now_ns: TimeNs) -> SmallVec<[ItemKey; 4]> {
    let mut names: SmallVec<[&'static str; 4]> = SmallVec::new();
    let mut keys = SmallVec::new();
    for &id in tab_sources(tab) {
        let h = health.get(id);
        let key = match h.state {
            SourceState::Stale => ItemKey::Stale(
                id,
                h.last_ok_ns
                    .map(|t| now_ns.saturating_sub(t) / 1_000_000_000),
            ),
            SourceState::Failed => ItemKey::Failed(id, h.last_error.clone()),
            SourceState::Disabled { .. } => ItemKey::Disabled(id, h.last_error.clone()),
            SourceState::Off
            | SourceState::Running
            | SourceState::Late
            | SourceState::Unsupported => continue,
        };
        let name = source_name(id);
        if !names.contains(&name) {
            names.push(name);
            keys.push(key);
        }
    }
    keys
}

/// Emitted by a page entity when its `status_text` output changed (counts or selection size).
#[derive(Clone, Copy, Debug)]
pub struct StatusChanged;

/// How a [`StatusAction`] is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusActionKind {
    /// " | " and then the label, underlined on hover.
    Link,
    /// A `TlBadge` Info, one space after the text before it.
    Badge,
}

/// A clickable part a page adds after its text (for example E22's " | 42 restricted" link and its
/// "Administrator mode" badge, P3). Returned by `ShellPage::status_actions` (E20-008).
pub struct StatusAction {
    pub kind: StatusActionKind,
    pub label: SharedString,
    pub tooltip: SharedString,
    /// Dispatched on click, Enter or Space.
    pub action: Box<dyn Action>,
}

impl Clone for StatusAction {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            label: self.label.clone(),
            tooltip: self.tooltip.clone(),
            action: self.action.boxed_clone(),
        }
    }
}

impl PartialEq for StatusAction {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.label == other.label
            && self.tooltip == other.tooltip
            && self.action.name() == other.action.name()
            && self.action.partial_eq(other.action.as_ref())
    }
}

/// The floating status chip.
pub struct StatusChip {
    /// The active page's text, segments joined by " | ".
    page_text: SharedString,
    page_actions: SmallVec<[StatusAction; 2]>,
    paused: bool,
    /// Reused buffer for [`Self::text`].
    buf: String,
    /// What the text element draws: the page text, and "[PAUSED - Ctrl]" when there are no page
    /// actions. With page actions the paused tag is drawn after them as its own element.
    text: SharedString,
    /// Byte ranges of the " | " separators in `text`.
    separators: SmallVec<[Range<usize>; 4]>,
    /// Byte range of the paused tag in `text`.
    paused_range: Option<Range<usize>>,
    /// "[PAUSED - Ctrl]" in the current language.
    paused_label: SharedString,
    /// One focus handle per page action, in order. Grows as needed.
    action_focus: SmallVec<[FocusHandle; 2]>,
    /// The active tab from the last [`Self::set_health`].
    tab: Tab,
    /// Every item's key for `tab`, before the limit.
    item_keys: SmallVec<[ItemKey; 4]>,
    /// The shown items, at most [`MAX_ITEMS`].
    items: SmallVec<[HealthItem; 2]>,
    /// "+N more" when more than [`MAX_ITEMS`] items exist.
    more: Option<SharedString>,
    /// The refused target count while the restricted item lasts.
    restricted: Option<u32>,
    /// "Restricted: N processes".
    restricted_text: SharedString,
    /// Ends the restricted item. A new refusal replaces it.
    restricted_task: Option<Task<()>>,
    /// One focus handle per health part: two items, "+N more" and the restricted item.
    health_focus: [FocusHandle; MAX_ITEMS + 2],
    words: HealthWords,
    strings_version: u64,
    style_version: u64,
    _subs: SmallVec<[Subscription; 2]>,
    #[cfg(any(test, feature = "test-hooks"))]
    renders: u32,
    #[cfg(any(test, feature = "test-hooks"))]
    drawn: Vec<(SharedString, Hsla)>,
    #[cfg(any(test, feature = "test-hooks"))]
    text_layout: Option<gpui_kit::TextLayout>,
}

fn strings_version(cx: &App) -> u64 {
    cx.try_global::<Strings>().map_or(0, |s| s.strings_version)
}

impl StatusChip {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut subs = SmallVec::new();
        subs.push(cx.observe_global::<Strings>(|this, cx| {
            let version = strings_version(cx);
            if version != this.strings_version {
                this.strings_version = version;
                this.paused_label = tr(cx, "shell-status-paused");
                this.words = HealthWords::new(cx);
                this.rebuild();
                this.rebuild_health();
                if let Some(n) = this.restricted {
                    this.restricted_text = this.restricted_label(n);
                }
                cx.notify();
            }
        }));
        subs.push(cx.observe_global::<UiMetrics>(|this, cx| {
            let version = metrics(cx).style_version;
            if version != this.style_version {
                this.style_version = version;
                cx.notify();
            }
        }));
        Self {
            page_text: SharedString::default(),
            page_actions: SmallVec::new(),
            paused: false,
            buf: String::new(),
            text: SharedString::default(),
            separators: SmallVec::new(),
            paused_range: None,
            paused_label: tr(cx, "shell-status-paused"),
            action_focus: SmallVec::new(),
            tab: Tab::Processes,
            item_keys: SmallVec::new(),
            items: SmallVec::new(),
            more: None,
            restricted: None,
            restricted_text: SharedString::default(),
            restricted_task: None,
            health_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            words: HealthWords::new(cx),
            strings_version: strings_version(cx),
            style_version: metrics(cx).style_version,
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            renders: 0,
            #[cfg(any(test, feature = "test-hooks"))]
            drawn: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            text_layout: None,
        }
    }

    /// Called by MainWindow with the page's `status_text` output. Notifies only when the text
    /// changed.
    pub fn set_page_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.page_text.as_ref() != text {
            self.page_text = SharedString::new(text);
            self.rebuild();
            cx.notify();
        }
    }

    /// Called by MainWindow with the page's `status_actions`. Notifies only when they changed.
    pub fn set_page_actions(
        &mut self,
        actions: SmallVec<[StatusAction; 2]>,
        cx: &mut Context<Self>,
    ) {
        if actions != self.page_actions {
            while self.action_focus.len() < actions.len() {
                self.action_focus.push(cx.focus_handle().tab_stop(true));
            }
            self.page_actions = actions;
            self.rebuild();
            cx.notify();
        }
    }

    /// Notifies only on change.
    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        if paused != self.paused {
            self.paused = paused;
            self.rebuild();
            cx.notify();
        }
    }

    /// Rebuilds the items from the snapshot for the active tab. Notifies only when the item list
    /// changed.
    pub fn set_health(
        &mut self,
        health: &HealthSnapshot,
        tab: Tab,
        now_ns: TimeNs,
        cx: &mut Context<Self>,
    ) {
        let keys = item_keys(health, tab, now_ns);
        let restricted_before = self.restricted_shown();
        self.tab = tab;
        let restricted_changed = self.restricted_shown() != restricted_before;
        if keys != self.item_keys {
            self.item_keys = keys;
            self.rebuild_health();
            cx.notify();
        } else if restricted_changed {
            cx.notify();
        }
    }

    /// For 10 s after a privileged action was refused with EPERM while tl-helper is missing
    /// (UI-FND-028). The ActionTracker (E16-A3) calls it with the refused target count.
    pub fn note_restricted_refusal(&mut self, count: u32, cx: &mut Context<Self>) {
        self.restricted = Some(count);
        self.restricted_text = self.restricted_label(count);
        self.restricted_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESTRICTED_FOR).await;
            let _ = this.update(cx, |this, cx| {
                this.restricted = None;
                this.restricted_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The shown health items, without "+N more" and the restricted item.
    pub fn health_items(&self) -> &[HealthItem] {
        &self.items
    }

    /// True while a Stale item is shown: its tooltip age moves, so `MainWindow` calls
    /// [`Self::set_health`] every second.
    pub fn has_stale(&self) -> bool {
        self.item_keys
            .iter()
            .take(MAX_ITEMS)
            .any(|k| matches!(k, ItemKey::Stale(..)))
    }

    /// False when every segment is empty.
    pub fn is_visible(&self) -> bool {
        !self.page_text.is_empty()
            || !self.page_actions.is_empty()
            || self.paused
            || !self.items.is_empty()
            || self.restricted_shown()
    }

    /// The restricted item shows on every tab but Processes, whose tab text has the count.
    fn restricted_shown(&self) -> bool {
        self.restricted.is_some() && self.tab != Tab::Processes
    }

    fn restricted_label(&mut self, n: u32) -> SharedString {
        self.buf.clear();
        let _ = self.words.restricted.write(&mut self.buf, &[&n]);
        SharedString::new(self.buf.as_str())
    }

    /// Builds the item texts from `item_keys`. Runs once per change of the list.
    fn rebuild_health(&mut self) {
        let words = &self.words;
        let buf = &mut self.buf;
        let mut text = |template: &Template, value: &dyn std::fmt::Display| {
            buf.clear();
            let _ = template.write(buf, &[value]);
            SharedString::new(buf.as_str())
        };
        let error = |e: &Option<Arc<str>>, fallback: &SharedString| match e {
            Some(e) => {
                debug_assert_clean(e);
                SharedString::new(e.as_ref())
            }
            None => fallback.clone(),
        };
        self.items = self
            .item_keys
            .iter()
            .take(MAX_ITEMS)
            .map(|key| match key {
                ItemKey::Stale(id, secs) => HealthItem {
                    source: *id,
                    text: text(&words.stale, &words.names[id.index()]),
                    tooltip: match secs {
                        Some(secs) => text(&words.no_data, secs),
                        None => words.unknown_error.clone(),
                    },
                },
                ItemKey::Failed(id, e) => HealthItem {
                    source: *id,
                    text: text(&words.failed, &words.names[id.index()]),
                    tooltip: error(e, &words.unknown_error),
                },
                ItemKey::Disabled(id, e) => HealthItem {
                    source: *id,
                    text: text(&words.disabled, &words.names[id.index()]),
                    tooltip: error(e, &words.restarting),
                },
            })
            .collect();
        let rest = self.item_keys.len().saturating_sub(MAX_ITEMS);
        self.more = (rest > 0).then(|| text(&words.more, &rest));
    }

    /// The health parts in draw order: (text, tooltip).
    fn health_parts(&self) -> impl Iterator<Item = (&SharedString, &SharedString)> {
        self.items
            .iter()
            .map(|i| (&i.text, &i.tooltip))
            .chain(self.more.iter().map(|m| (m, &self.words.open)))
            .chain(
                self.restricted_shown()
                    .then_some((&self.restricted_text, &self.words.open)),
            )
    }

    /// One health part: the leading " · " and the clickable text, both in `status.warning`.
    fn health_part(
        &self,
        ix: usize,
        (text, tooltip): &(SharedString, SharedString),
        theme: &TlTheme,
        m: &UiMetrics,
        window: &mut Window,
        cx: &mut App,
    ) -> [Div; 2] {
        let warning = theme.status.warning;
        let lead = plain(SharedString::new_static(HEALTH_SEP)).text_color(warning);
        let id = ElementId::NamedInteger("StatusChipHealth".into(), ix as u64);
        let focus = self.health_focus[ix].clone();
        let mut part = div()
            .id(id.clone())
            .debug_selector(move || format!("StatusChip-health-{ix}"))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .role(Role::Link)
            .aria_label(text.clone())
            .track_focus(&focus)
            .whitespace_nowrap()
            .text_color(warning)
            .hover(|s| s.underline())
            .child(text.clone());
        if TlFocusRing::show(focus.is_focused(window), window) {
            part = TlFocusRing::ring(part, m.radius_small, theme);
        }
        let part = part.on_click(|_, window, cx| {
            cx.stop_propagation();
            window.dispatch_action(Box::new(OpenCollectorHealth), cx);
        });
        let tooltip_id = ElementId::NamedChild(Arc::new(id), "tooltip".into());
        let state = HoverTooltip::state(tooltip_id, window, cx);
        let build = TlTooltip::text(tooltip.clone()).build();
        let part = HoverTooltip::attach(&state, part, DELAY_CONTROL, build, window, cx);
        [lead, div().flex_none().child(part)]
    }

    /// Renders since creation.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn renders(&self) -> u32 {
        self.renders
    }

    /// The chip's parts as last rendered, left to right, with their text colours. A badge
    /// reports its label in its text colour.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn drawn(&self) -> &[(SharedString, Hsla)] {
        &self.drawn
    }

    /// The chip's text as last rendered: every part joined.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn drawn_text(&self) -> String {
        self.drawn.iter().map(|(s, _)| s.as_ref()).collect()
    }

    /// The text element's text as laid out in the last frame, with any "…" cut. `None` while
    /// hidden. Call it only after a draw: GPUI's `TextLayout::text` panics before layout.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn laid_out_text(&self) -> Option<String> {
        self.text_layout.as_ref().map(|layout| layout.text())
    }

    /// Rebuilds `text`, `separators` and `paused_range` from the inputs. Runs once per change.
    fn rebuild(&mut self) {
        self.buf.clear();
        self.separators.clear();
        self.paused_range = None;
        let mut rest = self.page_text.as_ref();
        while let Some(at) = rest.find(SEP) {
            self.buf.push_str(&rest[..at]);
            let start = self.buf.len();
            self.buf.push_str(SEP);
            self.separators.push(start..self.buf.len());
            rest = &rest[at + SEP.len()..];
        }
        self.buf.push_str(rest);
        if self.paused && self.page_actions.is_empty() {
            if !self.buf.is_empty() {
                self.buf.push(' ');
            }
            let start = self.buf.len();
            self.buf.push_str(&self.paused_label);
            self.paused_range = Some(start..self.buf.len());
        }
        self.text = SharedString::new(self.buf.as_str());
    }

    /// The text's highlights: separators in `text_secondary`, the paused tag in `accent`.
    fn highlights(&self, theme: &TlTheme) -> SmallVec<[(Range<usize>, HighlightStyle); 5]> {
        let style = |color: Hsla| HighlightStyle {
            color: Some(color),
            ..HighlightStyle::default()
        };
        self.separators
            .iter()
            .map(|r| (r.clone(), style(theme.text_secondary)))
            .chain(
                self.paused_range
                    .iter()
                    .map(|r| (r.clone(), style(theme.accent))),
            )
            .collect()
    }

    /// One page action: its leading separator or space, then the clickable part.
    fn action_parts(
        &self,
        ix: usize,
        theme: &TlTheme,
        m: &UiMetrics,
        window: &mut Window,
        cx: &mut App,
    ) -> [Div; 2] {
        let action = &self.page_actions[ix];
        let lead = match action.kind {
            StatusActionKind::Link => {
                plain(SharedString::new_static(SEP)).text_color(theme.text_secondary)
            }
            StatusActionKind::Badge => plain(SharedString::new_static(" ")),
        };
        let id = ElementId::NamedInteger("StatusChipAction".into(), ix as u64);
        let focus = self.action_focus[ix].clone();
        let mut part = div()
            .id(id.clone())
            .debug_selector(move || format!("StatusChip-action-{ix}"))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .aria_label(action.label.clone())
            .track_focus(&focus);
        part = match action.kind {
            StatusActionKind::Link => part
                .role(Role::Link)
                .whitespace_nowrap()
                .text_color(theme.text_primary)
                .hover(|s| s.underline())
                .child(action.label.clone()),
            StatusActionKind::Badge => part
                .role(Role::Button)
                .child(TlBadge::new(TlBadgeKind::Info, action.label.clone())),
        };
        if TlFocusRing::show(focus.is_focused(window), window) {
            part = TlFocusRing::ring(part, m.radius_small, theme);
        }
        let dispatch = action.action.boxed_clone();
        let part = part.on_click(move |_, window, cx| {
            cx.stop_propagation();
            window.dispatch_action(dispatch.boxed_clone(), cx);
        });
        let tooltip_id = ElementId::NamedChild(Arc::new(id), "tooltip".into());
        let state = HoverTooltip::state(tooltip_id, window, cx);
        let build = TlTooltip::text(action.tooltip.clone()).build();
        let part = HoverTooltip::attach(&state, part, DELAY_CONTROL, build, window, cx);
        [lead, div().flex_none().child(part)]
    }

    #[cfg(any(test, feature = "test-hooks"))]
    fn record(&mut self, theme: &TlTheme) {
        let mut drawn = Vec::new();
        let mut at = 0;
        let mut marks: SmallVec<[(Range<usize>, Hsla); 5]> = self
            .separators
            .iter()
            .map(|r| (r.clone(), theme.text_secondary))
            .collect();
        if let Some(r) = &self.paused_range {
            marks.push((r.clone(), theme.accent));
        }
        for (range, color) in marks {
            if range.start > at {
                drawn.push((
                    self.text[at..range.start].to_owned().into(),
                    theme.text_primary,
                ));
            }
            drawn.push((self.text[range.clone()].to_owned().into(), color));
            at = range.end;
        }
        if at < self.text.len() {
            drawn.push((self.text[at..].to_owned().into(), theme.text_primary));
        }
        for action in &self.page_actions {
            match action.kind {
                StatusActionKind::Link => {
                    drawn.push((SEP.into(), theme.text_secondary));
                    drawn.push((action.label.clone(), theme.text_primary));
                }
                StatusActionKind::Badge => {
                    let colors = crate::components::badge::badge_colors(TlBadgeKind::Info, theme);
                    drawn.push((" ".into(), theme.text_primary));
                    drawn.push((action.label.clone(), colors.text));
                }
            }
        }
        if self.paused && !self.page_actions.is_empty() {
            drawn.push((" ".into(), theme.text_primary));
            drawn.push((self.paused_label.clone(), theme.accent));
        }
        for (text, _) in self.health_parts() {
            drawn.push((HEALTH_SEP.into(), theme.status.warning));
            drawn.push((text.clone(), theme.status.warning));
        }
        self.drawn = drawn;
    }
}

/// A one-line text part that keeps its leading and trailing spaces.
fn plain(text: SharedString) -> Div {
    div().flex_none().whitespace_nowrap().child(text)
}

fn stop(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}

impl Render for StatusChip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.renders += 1;
        }
        let theme = current(cx).clone();
        #[cfg(any(test, feature = "test-hooks"))]
        self.record(&theme);
        if !self.is_visible() {
            #[cfg(any(test, feature = "test-hooks"))]
            {
                self.text_layout = None;
            }
            return div().debug_selector(|| "StatusChip".into());
        }

        let m = *metrics(cx);
        let s = m.ui_scale;
        let hairline: AbsoluteLength = UiMetrics::hairline(window).into();

        let text = StyledText::new(self.text.clone()).with_highlights(self.highlights(&theme));
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.text_layout = Some(text.layout().clone());
        }
        let text = (!self.text.is_empty()).then(|| {
            div()
                .debug_selector(|| "StatusChip-text".into())
                .min_w_0()
                .flex_shrink(1.)
                .truncate()
                .child(text)
        });

        let actions: SmallVec<[Div; 4]> = (0..self.page_actions.len())
            .flat_map(|ix| self.action_parts(ix, &theme, &m, window, cx))
            .collect();
        let paused_tail = (self.paused && !self.page_actions.is_empty()).then(|| {
            [
                plain(SharedString::new_static(" ")),
                plain(self.paused_label.clone()).text_color(theme.accent),
            ]
        });
        let parts: SmallVec<[(SharedString, SharedString); 4]> = self
            .health_parts()
            .map(|(t, tip)| (t.clone(), tip.clone()))
            .collect();
        let health: SmallVec<[Div; 8]> = parts
            .iter()
            .enumerate()
            .flat_map(|(ix, part)| self.health_part(ix, part, &theme, &m, window, cx))
            .collect();

        // The text style first: `text_color` below adds to it.
        let mut chip = div();
        *chip.text_style() = numeric_style(&m, m.text.sm);
        let mut chip = chip
            .debug_selector(|| "StatusChip".into())
            .flex()
            .flex_row()
            .items_center()
            .max_w_full()
            .h(px(CHIP_H * s))
            .pl(px(PAD_L * s))
            .pr(px(PAD_R * s))
            .bg(theme.tab_bg)
            .border_color(theme.divider)
            .rounded_tr(m.radius_card)
            .text_color(theme.text_primary)
            .on_mouse_down(MouseButton::Left, stop)
            .on_mouse_down(MouseButton::Right, stop)
            .on_mouse_down(MouseButton::Middle, stop);
        {
            let style = chip.style();
            style.border_widths.top = Some(hairline);
            style.border_widths.right = Some(hairline);
        }
        chip.when_some(text, |chip, text| chip.child(text))
            .children(actions)
            .when_some(paused_tail, |chip, tail| chip.children(tail))
            .children(health)
    }
}
