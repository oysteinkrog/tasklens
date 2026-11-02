//! The Disk box's presets as flow layouts (ADR-019 "Presets", UI-PRF-061 phase 2).
//!
//! A flow layout is the widget list of one Disk box mode (`layout_boxes::convert_disk_box`):
//! the disks group as cells in `disk_charts`, the disks group as rows in `rows`, and the
//! `io` chart of the disk total in `total_chart`. In every mode that one widget fills the body.
//! The box draws it at its 1.2.0 place by the rules of UI-PRF-017, UI-PRF-019 and UI-PRF-059:
//! the disk sections and the `disk_charts` cells are the disks group's `GroupWidget` with a
//! `GroupFlowLook` (the busiest-first expansion and "+n more" are worked out inside it), and
//! the total chart is a `MetricWidget` with `FlowLook::Chart`. The box draws only what is not a
//! widget: the title line and the `rows` splitter.
//!
//! The box makes a [`DiskPlan`] on each render: the mode and the widget it drew, but no body
//! size and no sample value. The [`placer`] puts that widget over the whole body, so the
//! container keeps its entity and Edit Widgets pins it where the user saw it. A sample never
//! changes the plan.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Bounds, Pixels, Size, point, px};
use smallvec::SmallVec;
use tl_ui_settings::layout::{
    ContainerLayout, DeviceSelector, GroupKind, WidgetContent, WidgetSetting, grid_cols,
};
use tl_ui_settings::layout_boxes::{DEVICE_DISKS, METRIC_DISK_IO, convert_disk_box};
use tl_ui_settings::settings::OverviewGridSetting;

use super::layout::Arrange;
use crate::pages::performance::widget::container::{FlowInput, FlowPlace, list_flow};
use crate::pages::performance::widget::grid::{self, InnerPlacement, PlacedWidget};
use crate::pages::performance::widget::source::Availability;

/// The Disk box settings a flow layout is made from: the old keys of UI-PRF-019.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DiskFlowKeys<'a> {
    pub mode: &'a str,
    pub arrange: Option<Arrange>,
    pub split: Option<f32>,
}

/// The flow layout of the Disk box with `keys`: the widget list of
/// `layout_boxes::convert_disk_box`, so it is the same list a converted old setting gives.
pub fn flow_layout(keys: DiskFlowKeys<'_>) -> ContainerLayout {
    convert_disk_box(&OverviewGridSetting {
        kind: "disk".to_owned(),
        mode: Some(keys.mode.to_owned()),
        arrange: keys.arrange.map(|a| a.id().to_owned()),
        split: keys.split.map(f64::from),
        ..OverviewGridSetting::default()
    })
}

/// Whether `w` is on the disk total (`aggregate:disk`).
fn on_total(device: &DeviceSelector) -> bool {
    matches!(device, DeviceSelector::Key(k) if &**k == DEVICE_DISKS)
}

/// The id of the disks group: the disk sections or the `disk_charts` cells.
pub fn disks_of(layout: &ContainerLayout) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| matches!(&w.content, WidgetContent::Group(g) if g.members == GroupKind::Disks))
        .map(|w| w.id)
}

/// Whether `w` shows the read and write chart of the disk total.
fn is_total_chart(w: &WidgetSetting) -> bool {
    w.content
        .metric()
        .is_some_and(|s| s.metric.as_str() == METRIC_DISK_IO && on_total(&s.device))
}

/// The id of the `total_chart` widget: Read and write of the disk total.
pub fn total_of(layout: &ContainerLayout) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| is_total_chart(w))
        .map(|w| w.id)
}

/// The Disk box mode a [`DiskPlan`] is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiskLook {
    /// `disk_charts`: one read and write chart cell per disk.
    DiskCharts,
    /// `rows`: one section per disk, with a splitter between the text and the sparklines.
    Rows,
    /// `total_chart`: the summed read and write chart.
    TotalChart,
}

impl DiskLook {
    /// The preset id of the mode.
    pub fn id(self) -> &'static str {
        match self {
            Self::DiskCharts => "disk_charts",
            Self::Rows => "rows",
            Self::TotalChart => "total_chart",
        }
    }
}

/// What the Disk box shows in a flow layout, apart from the body size: the mode and the widget
/// that fills the body. The box makes it from its mode on each render.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DiskPlan {
    pub look: DiskLook,
    /// The widget the box drew: the disks group, or the total chart.
    pub widget: Option<u32>,
}

impl DiskPlan {
    /// Every widget's rectangle in a body of `area`, px from the body's top left corner: the
    /// one widget over the whole body.
    pub fn rects(&self, area: Size<Pixels>) -> SmallVec<[(u32, Bounds<Pixels>); 1]> {
        self.widget
            .map(|id| (id, Bounds::new(point(px(0.), px(0.)), area)))
            .into_iter()
            .collect()
    }
}

/// The plan of the last render and the widgets it drew, in drawing order.
#[derive(Default, Debug)]
struct PlanState {
    plan: Option<DiskPlan>,
    drawn: SmallVec<[u32; 1]>,
}

/// The [`DiskPlan`] shared by the box (which makes it) and its [`placer`].
#[derive(Clone, Default, Debug)]
pub struct PlanCell(Rc<RefCell<PlanState>>);

impl PlanCell {
    /// Takes the plan of a render. Returns whether it changed, so the box must place its
    /// widgets again.
    pub fn set(&self, plan: DiskPlan) -> bool {
        let mut st = self.0.borrow_mut();
        let changed = st.plan != Some(plan);
        st.plan = Some(plan);
        changed
    }

    /// The plan of the last flow render.
    pub fn plan(&self) -> Option<DiskPlan> {
        self.0.borrow().plan
    }

    /// The widgets a render drew, in drawing order.
    pub fn set_drawn(&self, drawn: SmallVec<[u32; 1]>) {
        self.0.borrow_mut().drawn = drawn;
    }

    /// The widgets the last flow render drew, in drawing order.
    pub fn drawn(&self) -> SmallVec<[u32; 1]> {
        self.0.borrow().drawn.clone()
    }
}

/// The placer of the Disk box's flow layouts: the widget of the box's plan over the whole body
/// ([`DiskPlan::rects`]). A widget the plan leaves out is not placed, so it has no entity. In
/// Edit Widgets, when none of the layout's widgets is in the plan (the layout is not the one
/// the box shows, as after Reset), the widgets go in list order (`container::list_flow`).
pub fn placer(plan: PlanCell) -> FlowPlace {
    Rc::new(move |input: &FlowInput<'_>| place(&plan, input))
}

fn place(cell: &PlanCell, input: &FlowInput<'_>) -> InnerPlacement {
    let mut widgets = Vec::new();
    {
        let st = cell.0.borrow();
        let preset = input.layout.preset.as_ref().map(|p| p.id.as_str());
        if let Some(plan) = st.plan.as_ref().filter(|p| preset == Some(p.look.id())) {
            for (id, bounds) in plan.rects(input.area) {
                let known = input
                    .items
                    .iter()
                    .any(|i| i.setting.id == id && i.availability != Availability::Unknown);
                if known {
                    widgets.push(PlacedWidget { id, bounds });
                }
            }
        }
    }
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
    use gpui_kit::size;
    use tl_ui_settings::layout::{DISK_BOX_CONTAINER, FormSet, GroupItems};

    use super::*;

    fn keys(mode: &str) -> DiskFlowKeys<'_> {
        DiskFlowKeys {
            mode,
            arrange: None,
            split: None,
        }
    }

    fn group(l: &ContainerLayout) -> &tl_ui_settings::layout::GroupSelector {
        let w = l.widgets.iter().find(|w| Some(w.id) == disks_of(l));
        match w.map(|w| &w.content) {
            Some(WidgetContent::Group(g)) => g,
            _ => panic!("a disks group: {l:?}"),
        }
    }

    #[test]
    fn each_mode_gives_its_widgets() {
        let l = flow_layout(DiskFlowKeys {
            arrange: Some(Arrange::Vertical),
            ..keys("disk_charts")
        });
        assert_eq!(l.container, DISK_BOX_CONTAINER);
        assert_eq!(
            l.preset.as_ref().map(|p| p.id.as_str()),
            Some("disk_charts")
        );
        assert_eq!(l.widgets.len(), 1);
        let g = group(&l);
        assert_eq!(g.metric.as_str(), METRIC_DISK_IO);
        assert_eq!(g.options.items, GroupItems::Cells);
        assert_eq!(g.options.limit, Some(8));
        assert_eq!(g.options.arrange.as_deref(), Some("vertical"));
        assert!(total_of(&l).is_none());
        let forms = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
        assert_eq!(l.widgets[0].forms, forms);

        let l = flow_layout(keys("rows"));
        assert_eq!(l.widgets.len(), 1);
        assert_eq!(group(&l).options.items, GroupItems::Rows);
        assert_eq!(group(&l).options.limit, Some(8));

        let l = flow_layout(keys("total_chart"));
        assert_eq!(l.widgets.len(), 1);
        assert!(disks_of(&l).is_none());
        let id = total_of(&l).expect("the total chart");
        let w = l.widgets.iter().find(|w| w.id == id).unwrap();
        assert_eq!(w.forms, FormSet::CHART);
    }

    #[test]
    fn the_widget_fills_the_body() {
        let p = DiskPlan {
            look: DiskLook::Rows,
            widget: Some(1),
        };
        let r = p.rects(size(px(360.), px(240.)));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, 1);
        assert_eq!(r[0].1.origin, point(px(0.), px(0.)));
        assert_eq!(r[0].1.size, size(px(360.), px(240.)));
        let none = DiskPlan { widget: None, ..p };
        assert!(none.rects(size(px(360.), px(240.))).is_empty());
    }

    #[test]
    fn plan_cell_reports_a_change_once() {
        let cell = PlanCell::default();
        let p = DiskPlan {
            look: DiskLook::Rows,
            widget: Some(1),
        };
        assert!(cell.set(p));
        assert!(!cell.set(p));
        assert!(cell.set(DiskPlan {
            look: DiskLook::TotalChart,
            ..p
        }));
    }
}
