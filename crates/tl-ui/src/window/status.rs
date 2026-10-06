//! The floating status chip at the bottom-left of the page area (E20-006, UI-SHL-050, R-107,
//! APP-029, PROC-051, R-042).
//!
//! The chip shows the active page's status text, the page's clickable status actions and
//! "[PAUSED - Ctrl]" while Ctrl-pause is on (E20-012). E20-016 adds health items. With nothing to
//! show it renders an empty element of zero size, so Performance and Information normally show
//! no chip.
//!
//! Pages emit [`StatusChanged`] on their own entity when their status text changes. `MainWindow`
//! (E20-008) then reads the page's `status_text` and `status_actions` and passes them to
//! [`StatusChip::set_page_text`] and [`StatusChip::set_page_actions`]. Count formatting (U+2009
//! thin-space grouping, table d) is the pages' job; the chip draws the text as given.
//!
//! The chip's text is one `StyledText`, cut at the end with "…" when the parent's `max_w` is too
//! small. The page actions are Tab stops; nothing else on the chip is clickable.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::accesskit::Role;
use gpui_kit::{
    AbsoluteLength, Action, App, Context, Div, ElementId, FocusHandle, HighlightStyle, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, StyledText, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;

use crate::components::badge::{TlBadge, TlBadgeKind};
use crate::components::focus::TlFocusRing;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::i18n::{Strings, tr};
use crate::text::style::numeric_style;
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
                this.rebuild();
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

    /// False when every segment is empty.
    pub fn is_visible(&self) -> bool {
        !self.page_text.is_empty() || !self.page_actions.is_empty() || self.paused
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
    }
}
