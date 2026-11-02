//! The Details widget of a device page: the "Details" heading and the page's Details grid
//! (ADR-019 phase 3 "Fixed widgets added in phase 3", UI-PRF-021, UI-PRF-061).
//!
//! A [`DetailsView`] is its own entity, so a Details value that changes (1 Hz) renders the grid
//! and not the page. The page keeps building its `KeyValueModel` on a snapshot, as in 1.3.0,
//! and then hands the view what the grid shows ([`DetailsShown`]). The view compares the whole
//! of it with what it drew last and notifies only when a row's text, validity, tooltip, link or
//! visibility changed. The page itself does not notify for it.
//!
//! The view draws the grid from the page's model through [`DetailsDraw`], which the page gives:
//! the same heading, grid, link values, prefixes and suffixes as the 1.3.0 page frame. It hosts
//! inside a `FixedWidgetView`, in a flow layout, in Edit Widgets and in a grid layout alike. In
//! a grid layout a grid taller than its widget scrolls inside it.
//!
//! The view measures the height its heading and grid need at their width and keeps it in
//! [`DetailsView::natural_height`], logical px. When it changes the view emits
//! [`DetailsResized`], so the page's flow layout places the parts under it again.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Context, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    canvas, div,
};
use tl_model::validity::Validity;

use crate::core::details::KeyValueModel;
use crate::selector::TlSelector;
use crate::theme::metrics;

/// The debug selector of the Details view of container `container`.
pub fn details_id(container: &str) -> String {
    format!("perf-widget-{container}-details")
}

/// One row as the grid shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownRow {
    pub label: &'static str,
    pub value: SharedString,
    pub validity: Validity,
    pub tooltip: Option<SharedString>,
    pub link: bool,
    pub hidden: bool,
}

/// Everything the grid shows, compared whole before the view notifies.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetailsShown {
    pub rows: Vec<ShownRow>,
    /// The strings version the labels and words come from.
    pub strings: Option<u64>,
}

impl DetailsShown {
    /// What `model` shows now, with the labels in the strings of `strings`.
    pub fn of(model: &KeyValueModel, strings: Option<u64>) -> Self {
        Self {
            rows: model
                .rows
                .iter()
                .map(|r| ShownRow {
                    label: r.label,
                    value: r.value.clone(),
                    validity: r.validity,
                    tooltip: r.tooltip.clone(),
                    link: r.link,
                    hidden: r.hidden,
                })
                .collect(),
            strings,
        }
    }
}

/// Draws the heading and the grid from the page's model; the page gives it.
pub type DetailsDraw = Rc<dyn Fn(&mut Window, &mut App) -> Option<AnyElement>>;

/// The view's natural height changed: a flow layout places the parts again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailsResized;

/// The Details widget's content view.
pub struct DetailsView {
    container: SharedString,
    shown: DetailsShown,
    draw: DetailsDraw,
    /// The height the heading and the grid take at the view's width, logical px; 0 before the
    /// first paint.
    natural: Rc<Cell<f32>>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl EventEmitter<DetailsResized> for DetailsView {}

impl DetailsView {
    /// The Details view of container `container`, showing `shown`, drawn by `draw`.
    pub fn new(container: impl Into<SharedString>, shown: DetailsShown, draw: DetailsDraw) -> Self {
        Self {
            container: container.into(),
            shown,
            draw,
            natural: Rc::new(Cell::new(0.)),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        }
    }

    /// What the grid shows now. Notifies only when it differs from what the view drew.
    /// Returns whether it did.
    pub fn set_shown(&mut self, shown: DetailsShown, cx: &mut Context<Self>) -> bool {
        if shown == self.shown {
            return false;
        }
        self.shown = shown;
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
        true
    }

    pub fn shown(&self) -> &DetailsShown {
        &self.shown
    }

    /// The measured height of the heading and the grid, logical px; shared with the page's
    /// flow placer.
    pub fn natural_height(&self) -> Rc<Cell<f32>> {
        Rc::clone(&self.natural)
    }
}

impl Render for DetailsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let scale = metrics(cx).ui_scale;
        let content = (self.draw)(window, cx);
        let natural = Rc::clone(&self.natural);
        let this = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let h = f32::from(bounds.size.height) / scale;
                if (natural.get() - h).abs() > 0.5 {
                    natural.set(h);
                    let this = this.clone();
                    // After the frame: an event during the frame is lost.
                    cx.defer(move |cx| {
                        let _ = this.update(cx, |_, cx| cx.emit(DetailsResized));
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let id = details_id(&self.container);
        let scroll = format!("{id}-scroll");
        div().tl_selector(move || id).relative().size_full().child(
            div()
                .id(SharedString::from(scroll))
                .size_full()
                .overflow_y_scroll()
                .child(
                    div()
                        .relative()
                        .w_full()
                        .flex()
                        .flex_col()
                        .children(content)
                        .child(measure),
                ),
        )
    }
}
