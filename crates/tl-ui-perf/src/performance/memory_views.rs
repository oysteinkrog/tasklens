//! The Memory page's parts that are not metric widgets, as views of their own (ADR-019 phase 3
//! "Fixed widgets added in phase 3", `tasklens-hzire.6`; UI-PRF-026 to UI-PRF-028).
//!
//! A [`MemoryPartView`] draws one part: the composition bar with its caption (`memory_bar`),
//! the "Compressed memory" block (`memory_compressed`), the "Swap devices" table
//! (`swap_devices`), the "Memory modules" block (`memory_modules`), or the Usage and By app
//! switch that the page's flow layout draws above its first part. The page hosts the first four
//! as fixed widgets and the switch as the container's flow decoration.
//!
//! The page keeps each part's models, as in 1.3.0, and gives the view a draw function that
//! builds the 1.3.0 element from them. On a snapshot the page hands each view what it shows
//! ([`PartShown`]); the view compares the whole of it with what it drew last and renders again
//! only when it changed. The page itself does not render for a part's change.
//!
//! The view measures the height its content takes at its width. When it changes the view emits
//! [`PartResized`], and the page places its flow layout again. In a grid layout a part taller
//! than its widget scrolls inside it.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Context, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    canvas, div,
};

use super::gpu_views::{PartResized, report_height};
use super::widget::details_widget::DetailsShown;
use tl_ui_base::selector::TlSelector;
use tl_ui_base::theme::metrics;

/// The part names of the Memory page, as the debug selectors name them.
pub const PART_SWITCH: &str = "view-switch";
pub const PART_BAR: &str = "memory-bar";
pub const PART_COMPRESSED: &str = "memory-compressed";
pub const PART_SWAP_DEVICES: &str = "swap-devices";
pub const PART_MODULES: &str = "memory-modules";

/// The debug selector of part `part` of page container `container`.
pub fn part_id(container: &str, part: &str) -> String {
    format!("perf-mem-part-{container}-{part}")
}

/// Everything a part draws, compared whole before the view renders again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartShown {
    /// The headings, captions and words the part draws.
    pub texts: Vec<SharedString>,
    /// The key-value grids the part draws.
    pub grids: Vec<DetailsShown>,
    /// The open module rows; the switch's selected view.
    pub flags: Vec<bool>,
    /// Counts and versions the part draws from: the swap rows, the chevron animation.
    pub numbers: Vec<u64>,
}

/// Builds the part's element from the page's models; the page gives it. `None` once the page
/// is gone, or while the part has nothing to draw.
pub type PartDraw = Rc<dyn Fn(&mut Window, &mut App) -> Option<AnyElement>>;

/// One part of the Memory page.
pub struct MemoryPartView {
    container: SharedString,
    part: &'static str,
    shown: PartShown,
    draw: PartDraw,
    /// The content's height at the view's width, logical px; 0 before the first paint.
    natural: Rc<Cell<f32>>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl EventEmitter<PartResized> for MemoryPartView {}

impl MemoryPartView {
    /// Part `part` of page container `container`, showing `shown`, drawn by `draw`.
    pub fn new(
        container: SharedString,
        part: &'static str,
        shown: PartShown,
        draw: PartDraw,
    ) -> Self {
        Self {
            container,
            part,
            shown,
            draw,
            natural: Rc::new(Cell::new(0.)),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        }
    }

    pub fn shown(&self) -> &PartShown {
        &self.shown
    }

    /// The content's measured height, logical px; shared with the page's flow placer.
    pub fn natural_height(&self) -> Rc<Cell<f32>> {
        Rc::clone(&self.natural)
    }

    /// What the part shows now. Renders again only when it changed. Returns whether it did.
    pub fn set_shown(&mut self, shown: PartShown, cx: &mut Context<Self>) -> bool {
        if shown == self.shown {
            return false;
        }
        self.shown = shown;
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
        true
    }
}

impl Render for MemoryPartView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let scale = metrics(cx).ui_scale;
        let content = (self.draw)(window, cx);
        let natural = Rc::clone(&self.natural);
        let this = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                report_height(&natural, f32::from(bounds.size.height) / scale, &this, cx);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let id = part_id(&self.container, self.part);
        let scroll = SharedString::from(format!("{id}-scroll"));
        div().tl_selector(move || id).relative().size_full().child(
            div().id(scroll).size_full().overflow_y_scroll().child(
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
