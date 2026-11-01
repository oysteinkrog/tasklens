//! The GPU box's presets as flow layouts (ADR-019 "Flow placement", UI-PRF-061).
//!
//! A flow layout is a list of widgets placed by the rules of its preset: the GPU box modes of
//! UI-PRF-017, UI-PRF-019 and UI-PRF-059. The fixed engine lists of those rules are replaced by
//! the widget list: a section shows an engine, the VRAM row or the temperature and power line
//! only when the layout has that widget for the section's slot.
//!
//! The GPU box works out the fit rules and gives each widget it shows the place 1.1.0 drew the
//! part at: an `engine_charts` cell, a spark row, the section chart, or a value on the shared
//! temperature and power line. Each of those is the widget's own `MetricWidget` entity, which
//! draws the 1.1.0 part (`FlowLook`); the box draws only what is not a widget (the section
//! headings, the "+n more" rows and the splitter). A widget the rules leave out takes no space
//! and has no entity.
//!
//! [`DrawnRects`] holds the widgets the box drew in its last render, with the metric each
//! showed, and the rectangle each was painted at. The [`placer`] places exactly those widgets
//! in the box's `WidgetContainer`, so the container keeps their entities and drops the others,
//! and Edit Widgets pins the widgets to the grid where the user saw them. The box places again
//! only when the set it draws changed: a resize, a layout change, and in the collapsed fit
//! state (UI-PRF-059) a sample that makes another engine the busiest one.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Bounds, IntoElement, Pixels, Point, Styled as _, canvas, px};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::layout::{
    ContainerLayout, DeviceSelector, FormSet, METRIC_POWER, METRIC_TEMP, METRIC_VRAM, MetricKey,
    WidgetSetting, convert_gpu_box, grid_cols,
};
use tl_model::settings::OverviewGridSetting;
use tl_model::system::GpuEngine;

use super::layout::Arrange;
use crate::pages::performance::widget::container::{FlowInput, FlowPlace, list_flow};
use crate::pages::performance::widget::grid::{self, InnerPlacement, PlacedWidget};
use crate::pages::performance::widget::source::Availability;

/// The flow layout of preset `mode` with `arrange`, `split` and the box's hidden engine names
/// (D-029): the widget list of `layout::convert_gpu_box`, so it is the same list a converted
/// old setting gives.
pub fn flow_layout(
    mode: &str,
    arrange: Option<Arrange>,
    split: Option<f32>,
    hidden: &[String],
) -> ContainerLayout {
    convert_gpu_box(&OverviewGridSetting {
        kind: "gpu".to_owned(),
        mode: Some(mode.to_owned()),
        arrange: arrange.map(|a| a.id().to_owned()),
        split: split.map(f64::from),
        hidden: hidden.to_vec(),
        ..OverviewGridSetting::default()
    })
}

/// Whether `w` is a widget of slot `slot`.
fn in_slot(w: &WidgetSetting, slot: usize) -> bool {
    matches!(w.source.device, DeviceSelector::Slot(n) if usize::from(n) == slot)
}

/// Whether `w` is the section chart of `chart_engines` and `chart`: the 3D engine with the chart
/// alone.
fn is_section_chart(w: &WidgetSetting) -> bool {
    w.forms == FormSet::CHART && w.source.metric.gpu_engine() == Some(GpuEngine::Gpu)
}

/// The id of the widget of slot `slot` showing `metric`, leaving out the section chart.
pub fn widget_of(layout: &ContainerLayout, slot: usize, metric: &str) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| in_slot(w, slot) && w.source.metric.as_str() == metric && !is_section_chart(w))
        .map(|w| w.id)
}

/// The id of slot `slot`'s section chart.
pub fn chart_of(layout: &ContainerLayout, slot: usize) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| in_slot(w, slot) && is_section_chart(w))
        .map(|w| w.id)
}

/// The id of slot `slot`'s widget of engine `key` (`GpuEngine::key`).
pub fn engine_widget(layout: &ContainerLayout, slot: usize, key: &str) -> Option<u32> {
    let engine = GpuEngine::ALL.into_iter().find(|e| e.key() == key)?;
    widget_of(layout, slot, MetricKey::engine(engine).as_str())
}

/// Whether slot `slot` shows engine `key`.
pub fn shows_engine(layout: &ContainerLayout, slot: usize, key: &str) -> bool {
    engine_widget(layout, slot, key).is_some()
}

/// Whether slot `slot` shows the VRAM row.
pub fn shows_vram(layout: &ContainerLayout, slot: usize) -> bool {
    widget_of(layout, slot, METRIC_VRAM).is_some()
}

/// Whether slot `slot` shows the temperature and the power draw on its line.
pub fn shows_line(layout: &ContainerLayout, slot: usize) -> (bool, bool) {
    (
        widget_of(layout, slot, METRIC_TEMP).is_some(),
        widget_of(layout, slot, METRIC_POWER).is_some(),
    )
}

/// The widgets the GPU box drew in its last flow render, and where: the body's origin, per
/// widget id the metric it showed (in drawing order), and the bounds it was painted at, in
/// window px.
#[derive(Default, Debug)]
pub struct Drawn {
    origin: Point<Pixels>,
    shown: SmallVec<[(u32, MetricKey); 16]>,
    rects: FxHashMap<u32, Bounds<Pixels>>,
}

/// The widgets of [`Drawn`], shared by the box (which records them) and its placer.
#[derive(Clone, Default, Debug)]
pub struct DrawnRects(Rc<RefCell<Drawn>>);

impl DrawnRects {
    /// The widgets a render drew, with the metric each showed, in drawing order. Returns
    /// whether that changed, so the box must place its widgets again.
    pub fn set_shown(&self, shown: SmallVec<[(u32, MetricKey); 16]>) -> bool {
        let mut d = self.0.borrow_mut();
        if d.shown == shown {
            return false;
        }
        d.rects
            .retain(|id, _| shown.iter().any(|(shown, _)| shown == id));
        d.shown = shown;
        true
    }

    /// The bounds of widget `id` in the body, px, as last painted; `None` when the last render
    /// did not draw it.
    pub fn bounds(&self, id: u32) -> Option<Bounds<Pixels>> {
        let d = self.0.borrow();
        if !d.shown.iter().any(|(shown, _)| *shown == id) {
            return None;
        }
        d.rects.get(&id).map(|b| Bounds {
            origin: b.origin - d.origin,
            size: b.size,
        })
    }

    /// The ids drawn, in drawing order.
    pub fn ids(&self) -> SmallVec<[u32; 16]> {
        self.0.borrow().shown.iter().map(|(id, _)| *id).collect()
    }

    /// A probe that records the body's origin. It fills its parent, which must be `relative`.
    pub fn body_probe(&self) -> impl IntoElement + use<> {
        let me = self.clone();
        canvas(
            move |bounds, _, _| me.0.borrow_mut().origin = bounds.origin,
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// A probe that records widget `id` at its parent's bounds. It fills its parent, which must
    /// be `relative`.
    pub fn probe(&self, id: u32) -> impl IntoElement + use<> {
        let me = self.clone();
        canvas(
            move |bounds: Bounds<Pixels>, _, _| {
                me.0.borrow_mut().rects.insert(id, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// The placer of the GPU box's flow layouts: the widgets the box drew in its last render, at
/// the rectangles they were painted at, when the box drew them for the same metric. Widgets the
/// box did not draw (cut by a fit rule, or left out) are not placed, so they have no entity.
/// In Edit Widgets, when the box drew none of the layout's widgets (the layout is not the one it
/// showed, as after Reset), the widgets go in list order (`container::list_flow`).
pub fn placer(drawn: DrawnRects) -> FlowPlace {
    Rc::new(move |input: &FlowInput<'_>| place(&drawn, input))
}

fn place(drawn: &DrawnRects, input: &FlowInput<'_>) -> InnerPlacement {
    let d = drawn.0.borrow();
    let mut widgets = Vec::new();
    for (id, metric) in &d.shown {
        let Some(item) = input.items.iter().find(|i| i.setting.id == *id) else {
            continue;
        };
        if *metric != item.setting.source.metric || item.availability == Availability::Unknown {
            continue;
        }
        let bounds = d.rects.get(id).map_or_else(Bounds::default, |b| Bounds {
            origin: b.origin - d.origin,
            size: b.size,
        });
        widgets.push(PlacedWidget { id: *id, bounds });
    }
    drop(d);
    if widgets.is_empty() && input.frozen.is_some() {
        return list_flow(input);
    }
    let mut placement = grid::place(
        &[],
        grid_cols(&input.layout.container),
        input.area,
        input.scale,
        input.frozen,
    );
    placement.content_h = widgets
        .iter()
        .map(|w| w.bounds.bottom())
        .fold(px(0.), Pixels::max);
    placement.widgets = widgets;
    placement
}

#[cfg(test)]
mod tests {
    use gpui_kit::point;
    use tl_model::layout::GPU_BOX_CONTAINER;

    use super::*;

    #[test]
    fn flow_layout_is_the_converted_list() {
        let l = flow_layout("chart_engines", Some(Arrange::Auto), None, &[]);
        assert_eq!(l.container, GPU_BOX_CONTAINER);
        assert_eq!(
            l.preset.as_ref().map(|p| p.id.as_str()),
            Some("chart_engines")
        );
        assert!(chart_of(&l, 0).is_some() && chart_of(&l, 1).is_some());
        assert!(shows_engine(&l, 0, "gpu") && shows_engine(&l, 1, "enc"));
        assert!(!shows_engine(&l, 0, "memctl"), "no memory controller row");
        assert_eq!(shows_line(&l, 0), (true, true));
        assert!(shows_vram(&l, 1));
        // The section chart is not the engine row.
        assert_ne!(chart_of(&l, 0), engine_widget(&l, 0, "gpu"));
    }

    #[test]
    fn hidden_names_leave_out_widgets() {
        let hidden = ["Video Encode".to_owned(), "VRAM".to_owned()];
        let l = flow_layout("engine_charts", None, None, &hidden);
        assert!(!shows_engine(&l, 0, "enc") && !shows_engine(&l, 1, "enc"));
        assert!(shows_engine(&l, 0, "dec"));
        assert!(!shows_vram(&l, 0));
        assert_eq!(
            shows_line(&l, 0),
            (false, false),
            "no line in engine_charts"
        );
        let chart = flow_layout("chart", None, None, &[]);
        assert!(chart_of(&chart, 0).is_some());
        assert!(!shows_engine(&chart, 0, "gpu"), "chart has no engine rows");
        assert!(shows_vram(&chart, 0));
    }

    #[test]
    fn drawn_rects_are_body_relative() {
        let d = DrawnRects::default();
        d.0.borrow_mut().origin = point(px(10.), px(20.));
        assert!(d.set_shown(smallvec::smallvec![(3, MetricKey::new(METRIC_VRAM))]));
        assert!(
            !d.set_shown(smallvec::smallvec![(3, MetricKey::new(METRIC_VRAM))]),
            "the same set"
        );
        d.0.borrow_mut().rects.insert(
            3,
            Bounds::new(point(px(15.), px(60.)), gpui_kit::size(px(100.), px(18.))),
        );
        assert_eq!(
            d.bounds(3),
            Some(Bounds::new(
                point(px(5.), px(40.)),
                gpui_kit::size(px(100.), px(18.))
            ))
        );
        assert_eq!(d.ids().as_slice(), [3]);
        // A render that does not draw it forgets it.
        assert!(d.set_shown(SmallVec::new()));
        assert_eq!(d.bounds(3), None);
        assert!(d.0.borrow().rects.is_empty());
    }
}
