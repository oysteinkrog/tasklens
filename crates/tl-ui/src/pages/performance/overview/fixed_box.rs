//! The Overview boxes that hold one fixed widget: Top processes and Timeline (ADR-019 phase 2,
//! UI-PRF-061, `tasklens-6swdr.8`).
//!
//! A [`FixedBox`] is the box frame (title line, edit chrome) around a `WidgetContainer` and its
//! `WidgetEditor`. The content the box drew before it was a widget, the Top processes rows or
//! the timeline bands, is a view of its own that the box keeps and hands to the container
//! ([`WidgetContainer::set_fixed_content`]), so a `FixedWidgetView` draws the same view in a
//! flow layout, in a grid layout and in Edit Widgets.
//!
//! The default layout is a flow layout of the box's one preset with its fixed widget
//! (`layout_boxes::fixed_box_default`), which fills the whole body (`container::fixed_flow`).
//! A grid layout (the user's own widgets, after Apply) and Edit Widgets are drawn by the editor
//! and the container. With a grid layout that holds no widget the box says so, and the page
//! drops the demand the widget asked for ([`FixedPlaced`]).
//!
//! Render contract: the box renders only for a title, line, language, scale, layout or edit
//! change. The content view renders for its own samples and never makes the box render.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, EventEmitter, IntoElement,
    ParentElement as _, Render, SharedString, StyleRefinement, Styled as _, Subscription, Window,
    div, prelude::FluentBuilder as _,
};
use tl_model::layout::{ContainerLayout, FixedWidget, WidgetContent};
use tl_model::layout_boxes::fixed_box_default;

use super::parts::{box_frame, edit_chrome};
use super::range::RangeText;
use crate::hub::SnapshotHub;
use crate::i18n::{Strings, tr};
use crate::pages::performance::widget::container::{WidgetContainer, fixed_flow};
use crate::pages::performance::widget::edit::{WidgetEditEvent, WidgetEditor};
use crate::pages::performance::widget::fixed_widget::FixedContent;
use crate::pages::performance::widget::source::SourceStore;
use crate::selector::TlSelector;
use crate::settings_global::TlSettings;
use crate::text::style::t_xs;
use crate::theme::{UiMetrics, current, metrics};

/// Reads the title line's summary from the content view.
type LineFn = Rc<dyn Fn(&App) -> SharedString>;

const KEY_EMPTY: &str = "perf-widget-empty";

/// What differs between the boxes that hold one fixed widget.
#[derive(Clone, Copy, Debug)]
pub struct FixedBoxSpec {
    /// The box's debug selector (`perf-overview-box-processes`).
    pub box_id: &'static str,
    /// The Overview kind key (`processes`), which names the built-in default layout.
    pub kind: &'static str,
    /// The container id (`overview.processes`).
    pub container: &'static str,
    /// The fixed widget the box allows.
    pub fixed: fn() -> FixedWidget,
    /// The key of the box's title.
    pub title_key: &'static str,
    /// The debug selector of "No widgets. Use Edit Widgets to add some."
    pub empty_id: &'static str,
}

/// The fixed widget of a box came or went (the user removed it or added it again). The page asks
/// for the widget's demand only while it is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedPlaced(pub bool);

/// A box of one fixed widget.
pub struct FixedBox {
    spec: FixedBoxSpec,
    title: SharedString,
    /// The title line's summary (the timeline's journal note).
    line: SharedString,
    /// Reads the line from the content view.
    line_of: Option<LineFn>,
    /// The built-in flow layout: the preset's fixed widget.
    flow: ContainerLayout,
    /// The saved grid layout, when the user has one: the editor draws it.
    saved_grid: Option<ContainerLayout>,
    /// Edit Widgets is on: the editor draws the body.
    editing: bool,
    /// The widget the last flow render drew.
    drawn: Rc<Cell<Option<u32>>>,
    placed: bool,
    editor: Entity<WidgetEditor>,
    container: Entity<WidgetContainer>,
    style_version: u64,
    strings_version: u64,
    _subs: [Subscription; 5],
    _content_sub: Option<Subscription>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl EventEmitter<FixedPlaced> for FixedBox {}

/// The saved grid layout of `container`, if any.
fn saved_grid(container: &str, cx: &App) -> Option<ContainerLayout> {
    if !cx.has_global::<TlSettings>() {
        return None;
    }
    TlSettings::settings(cx)
        .performance
        .layouts
        .known(container)
        .filter(|l| l.preset.is_none())
        .cloned()
}

/// The id of the first widget of `layout` that is fixed kind `kind`.
fn fixed_id(layout: &ContainerLayout, kind: &FixedWidget) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| matches!(&w.content, WidgetContent::Fixed(f) if f == kind))
        .map(|w| w.id)
}

fn cached_full<V: Render>(view: Entity<V>) -> AnyElement {
    view.cached(StyleRefinement::default().size_full())
        .into_any_element()
}

impl FixedBox {
    /// The box `spec` around `content`. `line` reads the title line's summary from the content
    /// view; the box renders again when it changes.
    pub fn new<V: Render>(
        spec: FixedBoxSpec,
        content: Entity<V>,
        line: Option<fn(&V) -> SharedString>,
        hub: &Entity<SnapshotHub>,
        cx: &mut Context<Self>,
    ) -> Self {
        let flow = fixed_box_default(spec.kind).unwrap_or_else(|| {
            ContainerLayout::flow(spec.container, Default::default(), Vec::new())
        });
        let saved = saved_grid(spec.container, cx);
        let view: AnyView = content.clone().into();
        let kind = (spec.fixed)();
        let provide: FixedContent = {
            let view = view.clone();
            Rc::new(move |asked: &FixedWidget, _: &mut App| (*asked == kind).then(|| view.clone()))
        };
        let container = {
            let store = Rc::new(RefCell::new(SourceStore::new(hub)));
            let shown = saved.clone().unwrap_or_else(|| flow.clone());
            cx.new(|cx| {
                let mut c = WidgetContainer::new(spec.container, shown, hub, store, cx);
                c.set_flow_placer(Some(fixed_flow()), cx);
                c.set_fixed_content(Some(provide), cx);
                // The box draws the fixed widget of its flow layout itself.
                c.set_hosted(true, cx);
                c
            })
        };
        let editor = cx.new(|cx| WidgetEditor::new(container.clone(), hub, cx));
        let line_of: Option<LineFn> = line.map(|f| {
            let content = content.clone();
            Rc::new(move |cx: &App| f(content.read(cx))) as LineFn
        });
        let content_sub = line.map(|f| {
            cx.observe(&content, move |this, content, cx| {
                let next = f(content.read(cx));
                if next != this.line {
                    this.line = next;
                    this.notify(cx);
                }
            })
        });
        let mut this = Self {
            spec,
            title: tr(cx, spec.title_key),
            line: line_of.as_ref().map(|f| f(cx)).unwrap_or_default(),
            line_of,
            flow,
            saved_grid: saved,
            editing: false,
            drawn: Rc::new(Cell::new(None)),
            placed: false,
            style_version: metrics(cx).style_version,
            strings_version: cx.global::<Strings>().strings_version,
            _subs: [
                cx.subscribe(&editor, Self::on_widget_edit),
                // The editor's layout changes reach the container: the widget may come or go.
                cx.observe(&container, |this, _, cx| this.sync_placed(cx)),
                cx.observe_global::<TlSettings>(Self::sync_grid),
                cx.observe_global::<UiMetrics>(Self::sync_style),
                cx.observe_global::<Strings>(Self::sync_strings),
            ],
            _content_sub: content_sub,
            editor,
            container,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.sync_placed(cx);
        this
    }

    /// The editor of the box's widgets (Edit Widgets, UI-PRF-061).
    pub fn editor(&self) -> &Entity<WidgetEditor> {
        &self.editor
    }

    /// The container of the box's widgets.
    pub fn container(&self) -> &Entity<WidgetContainer> {
        &self.container
    }

    /// The built-in flow layout.
    pub fn flow(&self) -> &ContainerLayout {
        &self.flow
    }

    /// The saved grid layout the box shows, if any.
    pub fn grid(&self) -> Option<&ContainerLayout> {
        self.saved_grid.as_ref()
    }

    /// Whether the box shows its fixed widget now: in the flow layout, or as a widget of the
    /// user's layout or of Edit Widgets. The page asks for the widget's demand while it does.
    pub fn is_placed(&self) -> bool {
        self.placed
    }

    /// The box draws the widget of its flow layout itself (no grid layout, not editing).
    fn host_draws(&self) -> bool {
        !self.editing && self.saved_grid.is_none()
    }

    /// Gets the container ready for Edit Widgets: with a flow layout it takes the widget the
    /// last render drew, at its rectangle, so the editor pins it there. With a grid layout the
    /// container already holds it.
    pub fn prepare_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing || self.saved_grid.is_some() {
            return;
        }
        let drawn = self.drawn.get();
        let mut layout = self.flow.clone();
        layout.widgets.retain(|w| Some(w.id) == drawn);
        self.container.update(cx, |c, cx| {
            c.set_layout(layout, cx);
            c.place_again(cx);
        });
    }

    /// Edit Widgets started or ended. When it ends, the box shows the saved layout again.
    fn on_widget_edit(
        &mut self,
        _: Entity<WidgetEditor>,
        ev: &WidgetEditEvent,
        cx: &mut Context<Self>,
    ) {
        match ev {
            WidgetEditEvent::Started => self.editing = true,
            WidgetEditEvent::Ended { .. } => {
                self.editing = false;
                self.saved_grid = saved_grid(self.spec.container, cx);
                self.show_saved(cx);
            }
        }
        self.sync_placed(cx);
        self.notify(cx);
    }

    /// The container holds the saved grid layout, or the flow layout the box draws.
    fn show_saved(&mut self, cx: &mut Context<Self>) {
        let shown = self.saved_grid.clone().unwrap_or_else(|| self.flow.clone());
        self.container.update(cx, |c, cx| c.set_layout(shown, cx));
    }

    /// The saved grid layout came, went or changed: outside Edit Widgets the box shows it.
    fn sync_grid(&mut self, cx: &mut Context<Self>) {
        let grid = saved_grid(self.spec.container, cx);
        if grid != self.saved_grid {
            self.saved_grid = grid;
            if !self.editing {
                self.show_saved(cx);
                self.sync_placed(cx);
                self.notify(cx);
            }
        }
    }

    /// A new theme or UI scale.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version != self.style_version {
            self.style_version = version;
            self.notify(cx);
        }
    }

    /// Whether the fixed widget is shown changed.
    fn sync_placed(&mut self, cx: &mut Context<Self>) {
        let kind = (self.spec.fixed)();
        let placed = if self.host_draws() {
            fixed_id(&self.flow, &kind).is_some()
        } else {
            fixed_id(self.container.read(cx).layout(), &kind).is_some()
        };
        if placed != self.placed {
            self.placed = placed;
            cx.emit(FixedPlaced(placed));
        }
    }

    /// A new language: the title.
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.title = tr(cx, self.spec.title_key);
        if let Some(f) = &self.line_of {
            self.line = f(cx);
        }
        self.notify(cx);
    }

    fn notify(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }
}

/// "No widgets. Use Edit Widgets to add some." centred in the body.
fn empty_text(id: &'static str, m: &UiMetrics, cx: &App) -> AnyElement {
    let theme = current(cx).clone();
    let mut text = div();
    *text.text_style() = t_xs(m);
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            text.tl_selector(move || id.to_owned())
                .text_color(theme.text_secondary)
                .child(tr(cx, KEY_EMPTY)),
        )
        .into_any_element()
}

impl Render for FixedBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let editor = cached_full(self.editor.clone());
        let inner: AnyElement = if !self.host_draws() {
            // The user's widgets, or Edit Widgets: the editor draws them.
            let empty = !self.editing
                && self
                    .saved_grid
                    .as_ref()
                    .is_some_and(|g| g.widgets.is_empty());
            div()
                .relative()
                .size_full()
                .child(editor)
                .when(empty, |d| d.child(empty_text(self.spec.empty_id, &m, cx)))
                .into_any_element()
        } else {
            let kind = (self.spec.fixed)();
            let id = fixed_id(&self.flow, &kind);
            let fixed = id.and_then(|id| self.container.update(cx, |c, cx| c.flow_fixed(id, cx)));
            if self.drawn.get() != id {
                self.drawn.set(id);
                self.container.update(cx, |c, cx| {
                    c.place_again(cx);
                });
            }
            // The editor sits under the body with no widgets, so its container knows the body's
            // size when Edit Widgets starts.
            div()
                .relative()
                .size_full()
                .child(div().absolute().top_0().left_0().size_full().child(editor))
                .children(fixed.map(cached_full))
                .into_any_element()
        };
        let body = div().relative().size_full().child(inner).into_any_element();
        box_frame(
            self.spec.box_id,
            None,
            self.title.clone(),
            SharedString::default(),
            self.line.clone(),
            &RangeText::default(),
            body,
            edit_chrome(cx),
            &m,
            &theme,
        )
        .relative()
        .size_full()
    }
}
