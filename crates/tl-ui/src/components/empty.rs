//! `TlEmptyState`: the centred block a table or pane shows when it has no rows (UI-FND-029,
//! PROC-077, PROC-079, SYS-036, APP-059).
//!
//! Empty is not loading: a tab shows its skeleton until its first data, and this block for every
//! later empty result. The owner keeps the table header visible above the block. When the
//! emptiness comes from a filter, the owner passes its `ClearFilter` action and the block shows a
//! "Clear filter" button that dispatches it. The action is any `Box<dyn Action>`, so this crate
//! does not depend on the shell. An owner may add a heading above the text, rename the button
//! (the system tables use "Retry" with `RefreshCurrentTab`) and draw the icon in the warning
//! colour for an error state (UI-SYS-005).

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{
    Action, App, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, prelude::FluentBuilder as _, px,
};

use super::button::{TlButton, TlButtonKind, TlButtonSize};
use super::icons::{TlIcon, icon};
use crate::text::style::{t_base, t_md, t_sm};
use crate::theme::{current, metrics};

/// The button label unless the owner sets another.
const CLEAR_FILTER_LABEL: &str = "Clear filter";

/// Max width of the text line under a heading, before the UI scale (UI-SYS-005).
const TEXT_MAX_W: f32 = 420.;

/// The icon colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TlEmptyTone {
    /// `text_secondary`.
    #[default]
    Secondary,
    /// `status.warning`, for an error state.
    Warning,
}

/// An icon from the app set or from the full kit catalog.
#[derive(Clone, Copy)]
enum EmptyIcon {
    Tl(TlIcon),
    Kit(IconName),
}

/// The empty-state block. Build it with [`TlEmptyState::new`] and render it as a child of the
/// content area, below the table header.
#[derive(IntoElement)]
pub struct TlEmptyState {
    text: SharedString,
    heading: Option<SharedString>,
    icon: Option<EmptyIcon>,
    tone: TlEmptyTone,
    second_line: Option<SharedString>,
    one_line: bool,
    clear_filter: Option<Rc<dyn Action>>,
    button_label: SharedString,
}

impl TlEmptyState {
    /// `text` is the exact string from the owning spec, such as "No threads found.". An empty
    /// text draws no text line (a state with only a heading).
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            heading: None,
            icon: None,
            tone: TlEmptyTone::Secondary,
            second_line: None,
            one_line: false,
            clear_filter: None,
            button_label: SharedString::new_static(CLEAR_FILTER_LABEL),
        }
    }

    /// An `i_lg` icon above the text, in `text_secondary` unless `tone` says otherwise.
    pub fn icon(mut self, icon: TlIcon) -> Self {
        self.icon = Some(EmptyIcon::Tl(icon));
        self
    }

    /// `icon` with an icon from the kit catalog, for one the app set does not have.
    pub fn kit_icon(mut self, icon: IconName) -> Self {
        self.icon = Some(EmptyIcon::Kit(icon));
        self
    }

    /// The icon colour.
    pub fn tone(mut self, tone: TlEmptyTone) -> Self {
        self.tone = tone;
        self
    }

    /// A `t_md` semibold heading in `text_primary` above the text.
    pub fn heading(mut self, heading: impl Into<SharedString>) -> Self {
        self.heading = Some(heading.into());
        self
    }

    /// Keeps the text on one line and ends an over-long one with an ellipsis (error texts).
    pub fn one_line(mut self) -> Self {
        self.one_line = true;
        self
    }

    /// A `t_sm` line below the text.
    pub fn second_line(mut self, text: impl Into<SharedString>) -> Self {
        self.second_line = Some(text.into());
        self
    }

    /// Shows a "Clear filter" button that dispatches `action`. Pass the shell's `ClearFilter`
    /// when the emptiness comes from a filter.
    pub fn clear_filter(mut self, action: Box<dyn Action>) -> Self {
        self.clear_filter = Some(Rc::from(action));
        self
    }

    /// The button label, "Clear filter" unless set.
    pub fn button_label(mut self, label: impl Into<SharedString>) -> Self {
        self.button_label = label.into();
        self
    }
}

impl RenderOnce for TlEmptyState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx);
        let fg = theme.text_secondary;
        let icon_fg = match self.tone {
            TlEmptyTone::Secondary => fg,
            TlEmptyTone::Warning => theme.status.warning,
        };
        let heading_fg = theme.text_primary;

        let heading = self.heading.map(|h| {
            let mut el = div().debug_selector(|| "TlEmptyState-heading".into());
            *el.text_style() = t_md(&m);
            el.font_weight(FontWeight::SEMIBOLD)
                .text_color(heading_fg)
                .child(h)
        });
        // Under a heading the text is capped at 420 px and centred (UI-SYS-005). Without one it
        // is laid out as before, so the older callers keep their exact pixels.
        let capped = heading.is_some();
        let text = (!self.text.is_empty()).then(|| {
            let mut el = div().debug_selector(|| "TlEmptyState-text".into());
            *el.text_style() = t_base(&m);
            el.when(capped, |el| {
                el.max_w(px(TEXT_MAX_W * m.ui_scale)).text_center()
            })
            .when(self.one_line, |el| el.truncate())
            .child(self.text)
        });
        let second_line = self.second_line.map(|line| {
            let mut el = div().debug_selector(|| "TlEmptyState-second-line".into());
            *el.text_style() = t_sm(&m);
            el.child(line)
        });

        div()
            .debug_selector(|| "TlEmptyState".into())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(m.sp[4])
            .p(m.sp[8])
            .text_color(fg)
            .when_some(self.icon, |d, name| {
                d.child(match name {
                    EmptyIcon::Tl(name) => icon(name, m.icon.lg, icon_fg),
                    EmptyIcon::Kit(name) => Icon::new(name).size(m.icon.lg).text_color(icon_fg),
                })
            })
            .children(heading)
            .children(text)
            .children(second_line)
            .when_some(self.clear_filter, |d, action| {
                d.child(
                    TlButton::new("TlEmptyState-clear-filter", TlButtonKind::Secondary)
                        .size(TlButtonSize::Sm)
                        .label(self.button_label)
                        .action(action.boxed_clone()),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui_kit::{
        Context, FocusHandle, Modifiers, Render, TestAppContext, VisualTestContext, px,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    gpui_kit::actions!(tl_empty_test, [TestAction]);

    const BUTTON: &str = "TlButton-TlEmptyState-clear-filter";

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    }

    /// Renders one empty state and counts `TestAction`s that reach the root. The root holds
    /// focus, as the owning table view does in the app.
    struct Host {
        filtering: bool,
        hits: Rc<Cell<usize>>,
        focus: FocusHandle,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let hits = self.hits.clone();
            let mut empty = TlEmptyState::new("No processes match the filter");
            if self.filtering {
                empty = empty.clear_filter(Box::new(TestAction));
            }
            div()
                .id("root")
                .track_focus(&self.focus)
                .size(px(400.))
                .on_action(move |_: &TestAction, _, _| hits.set(hits.get() + 1))
                .child(empty)
        }
    }

    fn host(cx: &mut TestAppContext, filtering: bool) -> (&mut VisualTestContext, Rc<Cell<usize>>) {
        cx.update(setup);
        let hits = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let hits = hits.clone();
            move |window, cx| {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                Host {
                    filtering,
                    hits,
                    focus,
                }
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (cx, hits)
    }

    #[gpui_kit::test]
    fn clear_filter_button_only_when_filtering(cx: &mut TestAppContext) {
        let (cx, _) = host(cx, true);
        assert!(cx.debug_bounds("TlEmptyState-text").is_some(), "text shows");
        assert!(cx.debug_bounds(BUTTON).is_some(), "button shows");
        assert!(cx.debug_bounds("TlButton-label").is_some());
    }

    #[gpui_kit::test]
    fn no_button_without_clear_filter(cx: &mut TestAppContext) {
        let (cx, _) = host(cx, false);
        assert!(cx.debug_bounds("TlEmptyState-text").is_some(), "text shows");
        assert!(cx.debug_bounds(BUTTON).is_none(), "no button");
    }

    #[gpui_kit::test]
    fn button_dispatches_owner_action(cx: &mut TestAppContext) {
        let (cx, hits) = host(cx, true);
        let button = cx.debug_bounds(BUTTON).expect("button drawn");
        cx.simulate_click(button.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(hits.get(), 1, "TestAction is dispatched once");
    }
}
