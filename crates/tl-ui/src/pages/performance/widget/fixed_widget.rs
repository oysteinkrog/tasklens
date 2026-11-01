//! One fixed widget: a widget with content of its own that is not a metric (ADR-019 "Fixed
//! widgets", UI-PRF-061 phase 2).
//!
//! A [`FixedWidgetView`] is a GPUI entity keyed by its widget id. It hosts the content view that
//! its box draws today, out of the box frame: Memory by app (`MemoryAppsChart` in its Overview
//! place) in the Memory box. The box keeps the content view and hands it over through the
//! container's [`FixedContent`], so the flow look and a grid layout draw the same view, with its
//! own clicks, tooltips, legend and demand. The view has no forms and no compositions: it draws
//! its content at its rectangle.
//!
//! Render contract: the widget renders again only when its content view, its setting or its
//! edit mode changes. The content is a cached child view that renders for its own changes; a
//! sample never reaches the widget.
//!
//! A fixed kind this build does not know, or one whose box gives no content, shows "Needs a
//! newer tasklens" centred.
//!
//! In Edit Widgets the content keeps the grip and gear's row free at its top, the outline's
//! 2 px on the other sides and the resize corner's width on the right, so no edit decoration
//! covers its text (`tasklens-lhliq.12`).

use std::rc::Rc;

use gpui_kit::{
    AnyView, App, Context, IntoElement, ParentElement as _, Render, SharedString, StyleRefinement,
    Styled as _, Window, div, px,
};
use tl_model::layout::{FixedWidget, WidgetSetting};

use super::metric_widget::{part_id, widget_id};
use crate::i18n::tr;
use crate::selector::TlSelector;
use crate::text::style::t_sm;
use crate::theme::{current, metrics};

/// The content view the host gives for a fixed kind, `None` when it has none.
pub type FixedContent = Rc<dyn Fn(&FixedWidget, &mut App) -> Option<AnyView>>;

/// The edge the content keeps free in Edit Widgets, px at every scale: the selected outline.
const EDIT_EDGE: f32 = super::edit::OUTLINE;
/// The right edge the content keeps free in Edit Widgets, logical px: the resize corner.
const EDIT_RIGHT: f32 = super::edit::CORNER;
/// The top the content keeps free in Edit Widgets, logical px: the grip and the gear's row.
const EDIT_TOP: f32 = super::edit::HANDLE;

const KEY_UNKNOWN: &str = "perf-widget-unknown";

/// One fixed widget of a container.
pub struct FixedWidgetView {
    setting: WidgetSetting,
    /// The id of the container the widget is in, for its debug selectors.
    container: SharedString,
    content: Option<AnyView>,
    /// Edit Widgets is on: the content keeps the edit chrome's margins free.
    edit_inset: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: std::cell::Cell<u32>,
}

impl FixedWidgetView {
    /// A fixed widget for `setting` in container `container` drawing `content`.
    pub fn new(container: SharedString, setting: WidgetSetting, content: Option<AnyView>) -> Self {
        Self {
            setting,
            container,
            content,
            edit_inset: false,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: std::cell::Cell::new(0),
        }
    }

    /// The widget id, unique in its container.
    pub fn id(&self) -> u32 {
        self.setting.id
    }

    pub fn setting(&self) -> &WidgetSetting {
        &self.setting
    }

    /// The content view, if the box gives one.
    pub fn content(&self) -> Option<&AnyView> {
        self.content.as_ref()
    }

    /// New settings for the same widget id (a layout change keeps the entity) and the content
    /// the host gives for them. Renders again only when either changed.
    pub fn set_setting(
        &mut self,
        setting: WidgetSetting,
        content: Option<AnyView>,
        cx: &mut Context<Self>,
    ) {
        let same_content = match (&self.content, &content) {
            (Some(a), Some(b)) => a.entity_id() == b.entity_id(),
            (None, None) => true,
            _ => false,
        };
        if self.setting == setting && same_content {
            return;
        }
        self.setting = setting;
        self.content = content;
        self.notify(cx);
    }

    /// Edit Widgets on or off.
    pub fn set_edit_inset(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.edit_inset != on {
            self.edit_inset = on;
            self.notify(cx);
        }
    }

    fn notify(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }
}

impl Render for FixedWidgetView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let id = self.setting.id;
        let root_id = widget_id(&self.container, id);
        let mut body = div().relative().size_full();
        if self.edit_inset {
            body = body
                .pt(px(EDIT_TOP * m.ui_scale))
                .pl(px(EDIT_EDGE))
                .pb(px(EDIT_EDGE))
                .pr(px(EDIT_RIGHT * m.ui_scale));
        }
        let body = match &self.content {
            Some(view) => body.child(
                div()
                    .relative()
                    .size_full()
                    .child(view.clone().cached(StyleRefinement::default().size_full())),
            ),
            None => {
                let theme = current(cx).clone();
                let mut line = div();
                *line.text_style() = t_sm(&m);
                let p = part_id(&self.container, id, "reason");
                body.flex().items_center().justify_center().child(
                    line.tl_selector(move || p)
                        .max_w_full()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(theme.text_secondary)
                        .child(tr(cx, KEY_UNKNOWN)),
                )
            }
        };
        div()
            .tl_selector(move || root_id)
            .relative()
            .size_full()
            .overflow_hidden()
            .child(body)
    }
}
