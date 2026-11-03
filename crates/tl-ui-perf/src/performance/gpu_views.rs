//! The GPU page's two page parts as views of their own, which the page hosts as the fixed
//! widgets `gpu_engine_grid` and `gpu_displays` (ADR-019 phase 3 "Fixed widgets added in
//! phase 3", `tasklens-hzire.4`; UI-PRF-032, UI-PRF-033).
//!
//! [`GpuEngineGridView`] draws the engine `ChartGridView` (one element for every cell, ADR-009
//! rule 7) with its heading tooltips, the probe's tooltip and the right-press that names the
//! engine for "Hide this engine graph", or the "3D / Compute" placeholder cell while the probe
//! is missing. Its columns follow its width as in 1.3.0. Its natural height is the grid's
//! height at that width, which a flow layout places it at. In a grid layout its rows share the
//! widget's height, at least `grid_cell_h` 112 px each, and a taller grid scrolls inside it.
//!
//! [`GpuDisplaysView`] draws the "Displays" heading and one row per display, or "No display
//! connected", as tall as its content.
//!
//! The page owns the grid's spec and pushes what each view shows. A view compares the whole
//! state it is given and renders again only when it changed. When its natural height changes,
//! it emits [`PartResized`], and the page places its flow layout again.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, App, Context, Div, ElementId, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels,
    Point, Render, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _,
    Window, canvas, div, px,
};
use tl_ui_base::selector::TlSelector;
use tl_ui_base::text::style::label_style;
use tl_ui_base::theme::{current, metrics};
use tl_ui_chart::chart::labels::LABEL_ROW;
use tl_ui_chart::chart::{ChartGridView, ChartView, grid_height, grid_layout};
use tl_ui_kit::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};

use super::menu::ClickedEngine;

/// `grid_cell_h`, × ui_scale.
pub const ENGINE_CELL_H: f32 = 112.;
/// Minimum engine cell width, × ui_scale.
pub const ENGINE_CELL_MIN_W: f32 = 160.;

/// The trigger over a hovered engine heading that has a tooltip.
pub const ENGINE_TIP_ID: &str = "perf-gpu-engine-tooltip";

/// Width used for the grid height before the first paint has measured the box.
const UNMEASURED_W: f32 = 1.0e6;

/// A display row's height, logical px.
const DISPLAY_ROW_H: f32 = 24.;

/// The debug selector of the engine grid of page container `container`.
pub fn engines_id(container: &str) -> String {
    format!("perf-gpu-engines-{container}")
}

/// The debug selector of the Displays block of page container `container`.
pub fn displays_id(container: &str) -> String {
    format!("perf-gpu-displays-{container}")
}

/// A part's natural height changed: a flow layout places the parts again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartResized;

/// Sets `natural` to `h` (logical px) and, when it moved by more than half a pixel, emits
/// [`PartResized`] from `this` after the frame (an event during the frame is lost).
pub(crate) fn report_height<V: EventEmitter<PartResized> + 'static>(
    natural: &Rc<Cell<f32>>,
    h: f32,
    this: &gpui_kit::WeakEntity<V>,
    cx: &mut App,
) {
    if (natural.get() - h).abs() <= 0.5 {
        return;
    }
    natural.set(h);
    let this = this.clone();
    cx.defer(move |cx| {
        let _ = this.update(cx, |_, cx| cx.emit(PartResized));
    });
}

/// What the engine grid shows besides its cells' data, compared whole.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnginesShown {
    /// The engine keys in grid order, one per cell.
    pub keys: Vec<Arc<str>>,
    /// The probe is missing: one "3D / Compute" placeholder cell reading `n/a`.
    pub missing: bool,
    /// The probe's tooltip over the whole grid while the probe is not Ok.
    pub probe_tip: Option<SharedString>,
}

impl EnginesShown {
    /// The number of cells drawn: the placeholder alone while the probe is missing.
    pub fn cells(&self) -> usize {
        if self.missing { 1 } else { self.keys.len() }
    }
}

/// The GPU page's engine grid part.
pub struct GpuEngineGridView {
    /// The page container id, for the debug selectors and the tooltip ids.
    container: SharedString,
    grid: Entity<ChartGridView>,
    /// The "3D / Compute" cell shown while the probe is missing.
    placeholder: Option<Entity<ChartView>>,
    shown: EnginesShown,
    /// The box width at the last paint.
    width: Rc<Cell<Pixels>>,
    /// The grid's top-left corner in the window at the last paint. It moves with the scroll
    /// offset of a grid taller than its widget.
    origin: Rc<Cell<Point<Pixels>>>,
    /// The box height at the last paint, px; 0 before the first.
    height: Rc<Cell<Pixels>>,
    /// The grid's height at the box width with 112 px rows, logical px; 0 before the first
    /// paint.
    natural: Rc<Cell<f32>>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl EventEmitter<PartResized> for GpuEngineGridView {}

impl GpuEngineGridView {
    pub fn new(
        container: SharedString,
        grid: Entity<ChartGridView>,
        placeholder: Option<Entity<ChartView>>,
        shown: EnginesShown,
    ) -> Self {
        Self {
            container,
            grid,
            placeholder,
            shown,
            width: Rc::new(Cell::new(px(0.))),
            origin: Rc::new(Cell::new(Point::default())),
            height: Rc::new(Cell::new(px(0.))),
            natural: Rc::new(Cell::new(0.)),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        }
    }

    /// The engine grid.
    pub fn grid(&self) -> &Entity<ChartGridView> {
        &self.grid
    }

    pub fn shown(&self) -> &EnginesShown {
        &self.shown
    }

    /// The grid's height at the box width, logical px; shared with the page's flow placer.
    pub fn natural_height(&self) -> Rc<Cell<f32>> {
        Rc::clone(&self.natural)
    }

    /// What the grid shows now, and the placeholder cell. Renders again only when either
    /// changed. Returns whether it did.
    pub fn set_shown(
        &mut self,
        shown: EnginesShown,
        placeholder: Option<Entity<ChartView>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let id = |p: &Option<Entity<ChartView>>| p.as_ref().map(|e| e.entity_id());
        let same_placeholder = id(&self.placeholder) == id(&placeholder);
        if shown == self.shown && same_placeholder {
            return false;
        }
        self.shown = shown;
        self.placeholder = placeholder;
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
        true
    }

    /// Hangs the probe's tooltip on `el` while the probe is not Ok.
    fn with_probe_tip(&self, el: Div, window: &mut Window, cx: &mut App) -> AnyElement {
        let Some(text) = self.shown.probe_tip.clone() else {
            return el.into_any_element();
        };
        let id = ElementId::NamedChild(
            Arc::new(ElementId::Name(self.container.clone())),
            "perf-gpu-tip-engines".into(),
        );
        let state = HoverTooltip::state(id, window, cx);
        HoverTooltip::attach(
            &state,
            el,
            DELAY_CONTROL,
            TlTooltip::text(text).build(),
            window,
            cx,
        )
        .into_any_element()
    }
}

impl Render for GpuEngineGridView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let s = m.ui_scale;
        let (min_w, gap) = {
            let spec = &self.grid.read(cx).spec;
            (spec.cell_min_w, spec.gap)
        };
        let base_h = px(ENGINE_CELL_H * s);
        let n = self.shown.cells();
        let measured = |w: Pixels| if w > px(0.) { w } else { px(UNMEASURED_W) };
        let layout = grid_layout(n, measured(self.width.get()), min_w, gap);
        let natural = grid_height(&layout, base_h, gap);
        // A grid layout taller than the rows need shares its height among the rows.
        let cell_h = match layout.rows {
            0 => base_h,
            rows => {
                let fill = (self.height.get() - gap * (rows - 1) as f32) / rows as f32;
                fill.max(base_h)
            }
        };
        let drawn_h = grid_height(&layout, cell_h, gap);

        let measure = {
            let (width, height) = (Rc::clone(&self.width), Rc::clone(&self.height));
            let natural_cell = Rc::clone(&self.natural);
            let grid = self.grid.clone();
            let this = cx.entity().downgrade();
            canvas(
                move |bounds, window, cx| {
                    let old_w = width.replace(bounds.size.width);
                    let old_h = height.replace(bounds.size.height);
                    let at = |w: Pixels| grid_layout(n, measured(w), min_w, gap);
                    let now = at(bounds.size.width);
                    let natural = grid_height(&now, base_h, gap);
                    report_height(&natural_cell, f32::from(natural) / s, &this, cx);
                    // The grid's cells take the row height the box gives them.
                    let want = match now.rows {
                        0 => base_h,
                        rows => ((bounds.size.height - gap * (rows - 1) as f32) / rows as f32)
                            .max(base_h),
                    };
                    if grid.read(cx).spec.cell_h != want {
                        let grid = grid.clone();
                        cx.defer(move |cx| {
                            grid.update(cx, |g, cx| {
                                let mut spec = g.spec.clone();
                                spec.cell_h = want;
                                g.set_spec(spec, cx);
                            });
                        });
                    }
                    if at(old_w) != now || old_h != bounds.size.height {
                        cx.notify(window.current_view());
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        // The cell under a right-press, named for the page menu's "Hide this engine graph".
        let hit = {
            let keys = self.shown.keys.clone();
            let origin = Rc::clone(&self.origin);
            move |ev: &MouseDownEvent, cx: &mut App| {
                if ev.button != MouseButton::Right {
                    return;
                }
                let p = ev.position - origin.get();
                let (x, y) = (f32::from(p.x), f32::from(p.y));
                let (w, h, g) = (f32::from(layout.cell_w), f32::from(cell_h), f32::from(gap));
                let (col, row) = (x / (w + g), y / (h + g));
                let inside = x >= 0.
                    && y >= 0.
                    && x - col.floor() * (w + g) < w
                    && y - row.floor() * (h + g) < h;
                let cell = (row.floor() as usize) * layout.cols + col.floor() as usize;
                if inside
                    && (col.floor() as usize) < layout.cols
                    && let Some(key) = keys.get(cell)
                {
                    cx.set_global(ClickedEngine(Some(key.clone())));
                }
            }
        };

        // The grid's corner, read inside the scrolled content so it follows the scroll offset.
        let corner = {
            let origin = Rc::clone(&self.origin);
            canvas(
                move |bounds, _, _| origin.set(bounds.origin),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };
        let id = engines_id(&self.container);
        let grid_box = div()
            .tl_selector(move || id)
            .relative()
            .w_full()
            .flex_none()
            .h(drawn_h)
            .capture_any_mouse_down(move |ev, _, cx| hit(ev, cx))
            .child(corner);
        let grid_box = match (&self.placeholder, self.shown.missing) {
            (Some(p), true) => {
                let cell = div()
                    .w(layout.cell_w)
                    .h(cell_h)
                    .child(p.clone().cached(StyleRefinement::default().size_full()));
                grid_box.child(self.with_probe_tip(cell, window, cx))
            }
            _ => {
                let mut grid = div().relative().size_full().child(
                    self.grid
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                );
                let view = self.grid.read(cx);
                if let Some((i, text)) = view.hovered_cell().zip(view.hovered_tooltip().cloned()) {
                    let pad = m.sp[3];
                    let (col, row) = (i % layout.cols, i / layout.cols);
                    let trigger = div()
                        .tl_selector(|| ENGINE_TIP_ID.into())
                        .absolute()
                        .left((layout.cell_w + gap) * col as f32 + pad)
                        .top((cell_h + gap) * row as f32 + pad)
                        .w((layout.cell_w - pad * 2.).max(px(0.)))
                        .h(px(LABEL_ROW * s));
                    // One state per heading, so moving to another heading restarts the delay.
                    let state = HoverTooltip::state((ENGINE_TIP_ID, i), window, cx);
                    grid = grid.child(HoverTooltip::attach(
                        &state,
                        trigger,
                        DELAY_CONTROL,
                        TlTooltip::text(text).build(),
                        window,
                        cx,
                    ));
                }
                grid_box.child(self.with_probe_tip(grid, window, cx))
            }
        };
        // A grid taller than its widget scrolls inside it.
        let scroll = SharedString::from(format!("{}-scroll", engines_id(&self.container)));
        let body = div().id(scroll).size_full();
        let body = if natural > self.height.get() + px(0.5) {
            body.overflow_y_scroll()
        } else {
            body
        };
        div()
            .relative()
            .size_full()
            .child(body.child(grid_box))
            .child(measure)
    }
}

/// What the Displays block shows, compared whole.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplaysShown {
    pub heading: SharedString,
    /// "No display connected", shown while there is no row.
    pub no_display: SharedString,
    /// One formatted row per display of the GPU's adapter (PERF-073).
    pub rows: Vec<SharedString>,
}

/// The GPU page's Displays block.
pub struct GpuDisplaysView {
    container: SharedString,
    shown: DisplaysShown,
    /// The block's height at its width, logical px; 0 before the first paint.
    natural: Rc<Cell<f32>>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl EventEmitter<PartResized> for GpuDisplaysView {}

impl GpuDisplaysView {
    pub fn new(container: SharedString, shown: DisplaysShown) -> Self {
        Self {
            container,
            shown,
            natural: Rc::new(Cell::new(0.)),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        }
    }

    pub fn shown(&self) -> &DisplaysShown {
        &self.shown
    }

    /// The block's measured height, logical px; shared with the page's flow placer.
    pub fn natural_height(&self) -> Rc<Cell<f32>> {
        Rc::clone(&self.natural)
    }

    /// What the block shows now. Renders again only when it changed. Returns whether it did.
    pub fn set_shown(&mut self, shown: DisplaysShown, cx: &mut Context<Self>) -> bool {
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

impl Render for GpuDisplaysView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let (m, theme) = (*metrics(cx), current(cx).clone());
        let s = m.ui_scale;
        let mut heading = div()
            .text_color(theme.text_primary)
            .child(self.shown.heading.clone());
        *heading.text_style() = label_style(&m, m.text.md, FontWeight::MEDIUM);
        let mut block = v_flex().w_full().gap(m.sp[4]).child(heading);
        let line = |text: SharedString, color| {
            let mut el = div().h(px(DISPLAY_ROW_H * s)).text_color(color).child(text);
            *el.text_style() = label_style(&m, m.text.sm, gpui_kit::FontWeight::NORMAL);
            el
        };
        if self.shown.rows.is_empty() {
            block = block.child(line(self.shown.no_display.clone(), theme.text_secondary));
        }
        for row in &self.shown.rows {
            block = block.child(line(row.clone(), theme.text_primary));
        }
        let natural = Rc::clone(&self.natural);
        let this = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                report_height(&natural, f32::from(bounds.size.height) / s, &this, cx)
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let id = displays_id(&self.container);
        let scroll = SharedString::from(format!("{id}-scroll"));
        div().tl_selector(move || id).relative().size_full().child(
            div()
                .id(scroll)
                .size_full()
                .overflow_y_scroll()
                .child(div().relative().w_full().child(block).child(measure)),
        )
    }
}
