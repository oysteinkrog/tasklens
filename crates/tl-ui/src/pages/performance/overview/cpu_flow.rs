//! The CPU box's presets as flow layouts (ADR-019 "Presets", UI-PRF-061 phase 2).
//!
//! A flow layout is the widget list of one CPU box mode (`layout_boxes::convert_cpu_box`):
//! the `total` chart, the cores group as rows or cells, and after them the Core clocks chart
//! and the power draw strip when they are switched on. The box draws each part at its 1.2.0
//! place by the rules of UI-PRF-017, UI-PRF-018, UI-PRF-019 and D-040, and each part is its
//! widget's own entity: the total chart, the Core clocks chart and the strip are
//! `MetricWidget`s with `FlowLook::Chart`, the core rows and the `core_charts` cells are the
//! cores group's `GroupWidget` with a `GroupFlowLook`. The box draws only what is not a widget:
//! the title line, the gutter and the splitters.
//!
//! The box makes a [`CpuPlan`] on each render: the mode, which parts show, the splits as the
//! box lays them out and the widget of each part, but no body size and no sample value. The
//! [`placer`] works out every widget's rectangle from the plan and the container's body size
//! with the same flex rules the box draws by ([`CpuPlan::rects`]), so the container keeps the
//! entities of those widgets and Edit Widgets pins them where the user saw them. A sample never
//! changes the plan.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Bounds, Pixels, Size, point, px, size};
use smallvec::SmallVec;
use tl_model::layout::{ContainerLayout, GroupKind, WidgetContent, WidgetSetting, grid_cols};
use tl_model::layout_boxes::{
    DEVICE_CPU, METRIC_CORE_CLOCKS, METRIC_CPU_TOTAL, METRIC_POWER_DRAW, convert_cpu_box,
};
use tl_model::settings::OverviewGridSetting;

use super::layout::{Arrange, PowerPlacement};
use crate::pages::performance::widget::container::{FlowInput, FlowPlace, list_flow};
use crate::pages::performance::widget::grid::{self, InnerPlacement, PlacedWidget};
use crate::pages::performance::widget::source::Availability;
use crate::theme::UiMetrics;

/// The CPU box settings a flow layout is made from: the old keys of UI-PRF-019.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CpuFlowKeys<'a> {
    pub mode: &'a str,
    pub arrange: Option<Arrange>,
    pub split: Option<f32>,
    pub power: bool,
    pub power_split: Option<f32>,
    pub power_placement: Option<PowerPlacement>,
    pub clocks: bool,
}

/// The flow layout of the CPU box with `keys`: the widget list of
/// `layout_boxes::convert_cpu_box`, so it is the same list a converted old setting gives.
pub fn flow_layout(keys: CpuFlowKeys<'_>) -> ContainerLayout {
    convert_cpu_box(&OverviewGridSetting {
        kind: "cpu".to_owned(),
        mode: Some(keys.mode.to_owned()),
        arrange: keys.arrange.map(|a| a.id().to_owned()),
        split: keys.split.map(f64::from),
        power: keys.power,
        power_split: keys.power_split.map(f64::from),
        power_placement: keys.power_placement.map(|p| p.id().to_owned()),
        clocks: keys.clocks,
        ..OverviewGridSetting::default()
    })
}

/// Whether `w` shows `metric` of the CPU package.
fn is_cpu_metric(w: &WidgetSetting, metric: &str) -> bool {
    w.content.metric().is_some_and(|s| {
        s.metric.as_str() == metric
            && matches!(&s.device, tl_model::layout::DeviceSelector::Key(k) if &**k == DEVICE_CPU)
    })
}

/// The id of the widget showing `metric` of the CPU package.
fn metric_widget(layout: &ContainerLayout, metric: &str) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| is_cpu_metric(w, metric))
        .map(|w| w.id)
}

/// The id of the total chart's widget.
pub fn total_of(layout: &ContainerLayout) -> Option<u32> {
    metric_widget(layout, METRIC_CPU_TOTAL)
}

/// The id of the Core clocks chart's widget.
pub fn clocks_of(layout: &ContainerLayout) -> Option<u32> {
    metric_widget(layout, METRIC_CORE_CLOCKS)
}

/// The id of the power draw strip's widget.
pub fn power_of(layout: &ContainerLayout) -> Option<u32> {
    metric_widget(layout, METRIC_POWER_DRAW)
}

/// The id of the cores group: the core rows or the `core_charts` cells.
pub fn cores_of(layout: &ContainerLayout) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| matches!(&w.content, WidgetContent::Group(g) if g.members == GroupKind::Cores))
        .map(|w| w.id)
}

/// The CPU box mode a [`CpuPlan`] is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CpuLook {
    /// `chart_cores`: the total chart and the core rows, side by side or stacked.
    ChartCores,
    /// `chart`: the total chart alone.
    Chart,
    /// `cores`: the core rows alone.
    Cores,
    /// `core_charts`: one chart cell per logical CPU.
    CoreCharts,
}

impl CpuLook {
    /// The preset id of the mode.
    pub fn id(self) -> &'static str {
        match self {
            Self::ChartCores => "chart_cores",
            Self::Chart => "chart",
            Self::Cores => "cores",
            Self::CoreCharts => "core_charts",
        }
    }
}

/// The power draw strip of a plan: its widget, the share of the parts above it, and whether it
/// sits under the total chart instead of across the whole body.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlanPower {
    pub id: u32,
    /// The parts' share of the length the strip shares with them (`InnerSplit::share`).
    pub parts_share: f32,
    pub under_graph: bool,
}

/// What the CPU box shows in a flow layout, apart from the body size: the mode, the side the
/// two parts of `chart_cores` take, the splits as the box lays them out after their clamps, and
/// the widget of each part. The box makes it from its mode and settings on each render.
#[derive(Clone, PartialEq, Debug)]
pub struct CpuPlan {
    pub look: CpuLook,
    pub m: UiMetrics,
    /// `chart_cores`: the parts sit side by side (else stacked).
    pub side: bool,
    /// `chart_cores`: the chart part's share of the body width (side by side) or height.
    pub share: f32,
    pub total: Option<u32>,
    pub cores: Option<u32>,
    /// The Core clocks chart, when the box draws it under the total chart.
    pub clocks: Option<u32>,
    /// The power draw strip, when the box draws it.
    pub power: Option<PlanPower>,
}

/// A rectangle in px from the body's top left corner.
fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w.max(0.)), px(h.max(0.))))
}

/// A flex line of a part with a basis of `share` of `len` that may shrink, a `gap`, and a part
/// that takes the rest: the first part's length, as the flex layout gives it.
fn first_len(share: f32, len: f32, gap: f32) -> f32 {
    (share * len).min((len - gap).max(0.)).max(0.)
}

impl CpuPlan {
    /// Every widget's rectangle in a body of `area`, px from the body's top left corner.
    pub fn rects(&self, area: Size<Pixels>) -> SmallVec<[(u32, Bounds<Pixels>); 4]> {
        let m = &self.m;
        let (w, h) = (f32::from(area.width), f32::from(area.height));
        let gap5 = f32::from(m.sp[5]);
        let mut out = SmallVec::new();
        // The strip across the whole body: the parts over it.
        let full_width = self.power.filter(|p| !p.under_graph);
        let parts = match full_width {
            Some(p) => {
                let parts_h = first_len(p.parts_share, h, gap5);
                out.push((p.id, rect(0., parts_h + gap5, w, h - parts_h - gap5)));
                rect(0., 0., w, parts_h)
            }
            None => rect(0., 0., w, h),
        };
        match self.look {
            CpuLook::Chart => self.graph(parts, &mut out),
            CpuLook::Cores => {
                if let Some(id) = self.cores {
                    out.push((id, self.rows_rect(parts)));
                }
            }
            CpuLook::CoreCharts => {
                if let Some(id) = self.cores {
                    out.push((id, parts));
                }
            }
            CpuLook::ChartCores => {
                let gutter = f32::from(super::cpu_box::core_gutter(m));
                let (x, y) = (f32::from(parts.origin.x), f32::from(parts.origin.y));
                let (pw, ph) = (f32::from(parts.size.width), f32::from(parts.size.height));
                let (chart, rows) = if self.side {
                    let cw = first_len(self.share, pw, gutter);
                    (
                        rect(x, y, cw, ph),
                        rect(x + cw + gutter, y, pw - cw - gutter, ph),
                    )
                } else {
                    let ch = first_len(self.share, ph, gutter);
                    (
                        rect(x, y, pw, ch),
                        rect(x, y + ch + gutter, pw, ph - ch - gutter),
                    )
                };
                let under = self.power.filter(|p| p.under_graph);
                let graph = match under {
                    Some(p) => {
                        let (gx, gy) = (f32::from(chart.origin.x), f32::from(chart.origin.y));
                        let (gw, gh) = (f32::from(chart.size.width), f32::from(chart.size.height));
                        let parts_h = first_len(p.parts_share, gh, gap5);
                        out.push((p.id, rect(gx, gy + parts_h + gap5, gw, gh - parts_h - gap5)));
                        rect(gx, gy, gw, parts_h)
                    }
                    None => chart,
                };
                self.graph(graph, &mut out);
                // Stacked, the rows keep the gap above the "Now" label; side by side they do
                // not.
                if let Some(id) = self.cores {
                    let rows = if self.side {
                        rows
                    } else {
                        self.rows_rect(rows)
                    };
                    out.push((id, rows));
                }
            }
        }
        out
    }

    /// The total chart in `area`, with the Core clocks chart under it on equal shares when it
    /// shows.
    fn graph(&self, area: Bounds<Pixels>, out: &mut SmallVec<[(u32, Bounds<Pixels>); 4]>) {
        let (x, y) = (f32::from(area.origin.x), f32::from(area.origin.y));
        let (w, h) = (f32::from(area.size.width), f32::from(area.size.height));
        match self.clocks {
            Some(clocks) => {
                let gap = f32::from(self.m.sp[5]);
                let half = ((h - gap) / 2.).max(0.);
                if let Some(id) = self.total {
                    out.push((id, rect(x, y, w, half)));
                }
                out.push((clocks, rect(x, y + half + gap, w, half)));
            }
            None => {
                if let Some(id) = self.total {
                    out.push((id, area));
                }
            }
        }
    }

    /// The core rows in their part: the part less the gap the last meter keeps above the
    /// "Now" label under the box.
    fn rows_rect(&self, part: Bounds<Pixels>) -> Bounds<Pixels> {
        let pad = f32::from(self.m.sp[2]);
        rect(
            f32::from(part.origin.x),
            f32::from(part.origin.y),
            f32::from(part.size.width),
            f32::from(part.size.height) - pad,
        )
    }
}

/// The plan of the last render and the widgets it drew, in drawing order.
#[derive(Default, Debug)]
struct PlanState {
    plan: Option<CpuPlan>,
    drawn: SmallVec<[u32; 4]>,
}

/// The [`CpuPlan`] shared by the box (which makes it) and its [`placer`].
#[derive(Clone, Default, Debug)]
pub struct PlanCell(Rc<RefCell<PlanState>>);

impl PlanCell {
    /// Takes the plan of a render. Returns whether it changed, so the box must place its
    /// widgets again.
    pub fn set(&self, plan: CpuPlan) -> bool {
        let mut st = self.0.borrow_mut();
        let changed = st.plan.as_ref() != Some(&plan);
        st.plan = Some(plan);
        changed
    }

    /// The plan of the last flow render.
    pub fn plan(&self) -> Option<CpuPlan> {
        self.0.borrow().plan.clone()
    }

    /// The widgets a render drew, in drawing order.
    pub fn set_drawn(&self, drawn: SmallVec<[u32; 4]>) {
        self.0.borrow_mut().drawn = drawn;
    }

    /// The widgets the last flow render drew, in drawing order.
    pub fn drawn(&self) -> SmallVec<[u32; 4]> {
        self.0.borrow().drawn.clone()
    }
}

/// The placer of the CPU box's flow layouts: every widget of the box's plan at the rectangle
/// the flex rules give it in the container's body now ([`CpuPlan::rects`]). A widget the plan
/// leaves out is not placed, so it has no entity. In Edit Widgets, when none of the layout's
/// widgets is in the plan (the layout is not the one the box shows, as after Reset), the
/// widgets go in list order (`container::list_flow`).
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
    use tl_model::layout::{CPU_BOX_CONTAINER, FormSet, GroupItems};
    use tl_model::layout_boxes::METRIC_LOAD;

    use super::*;

    fn keys(mode: &str) -> CpuFlowKeys<'_> {
        CpuFlowKeys {
            mode,
            arrange: None,
            split: None,
            power: false,
            power_split: None,
            power_placement: None,
            clocks: false,
        }
    }

    #[test]
    fn each_mode_gives_its_widgets() {
        let l = flow_layout(keys("chart_cores"));
        assert_eq!(l.container, CPU_BOX_CONTAINER);
        assert_eq!(
            l.preset.as_ref().map(|p| p.id.as_str()),
            Some("chart_cores")
        );
        let total = l.widgets.iter().find(|w| Some(w.id) == total_of(&l));
        assert_eq!(total.map(|w| w.forms), Some(FormSet::CHART));
        let cores = l.widgets.iter().find(|w| Some(w.id) == cores_of(&l));
        let Some(WidgetContent::Group(g)) = cores.map(|w| &w.content) else {
            panic!("a cores group: {l:?}");
        };
        assert_eq!(g.metric.as_str(), METRIC_LOAD);
        assert_eq!(g.options.items, GroupItems::Rows);
        assert!(clocks_of(&l).is_none() && power_of(&l).is_none());

        let l = flow_layout(keys("chart"));
        assert!(total_of(&l).is_some() && cores_of(&l).is_none());
        let l = flow_layout(keys("cores"));
        assert!(total_of(&l).is_none() && cores_of(&l).is_some());
        let l = flow_layout(CpuFlowKeys {
            arrange: Some(Arrange::Vertical),
            ..keys("core_charts")
        });
        let Some(WidgetContent::Group(g)) = l.widgets.first().map(|w| &w.content) else {
            panic!("a cores group: {l:?}");
        };
        assert_eq!(g.options.items, GroupItems::Cells);
        assert_eq!(g.options.arrange.as_deref(), Some("vertical"));
    }

    #[test]
    fn switches_add_clocks_then_power() {
        let l = flow_layout(CpuFlowKeys {
            power: true,
            clocks: true,
            power_split: Some(0.3),
            power_placement: Some(PowerPlacement::UnderGraph),
            ..keys("chart_cores")
        });
        let (clocks, power) = (clocks_of(&l).expect("clocks"), power_of(&l).expect("power"));
        assert!(clocks < power, "Core clocks first, then the strip");
        let preset = l.preset.as_ref().expect("a flow layout");
        assert_eq!(preset.power_split, Some(0.3));
        assert_eq!(preset.power_placement.as_deref(), Some("under_graph"));
    }

    fn plan(look: CpuLook) -> CpuPlan {
        CpuPlan {
            look,
            m: crate::theme::metrics::compute(
                &tl_model::settings::AppearanceSettings::default(),
                0,
            ),
            side: true,
            share: 0.4,
            total: Some(1),
            cores: Some(2),
            clocks: None,
            power: None,
        }
    }

    fn rects_of(plan: &CpuPlan, w: f32, h: f32) -> Vec<(u32, [f32; 4])> {
        plan.rects(size(px(w), px(h)))
            .into_iter()
            .map(|(id, b)| {
                (
                    id,
                    [
                        f32::from(b.origin.x),
                        f32::from(b.origin.y),
                        f32::from(b.size.width),
                        f32::from(b.size.height),
                    ],
                )
            })
            .collect()
    }

    #[test]
    fn chart_cores_side_by_side_and_stacked() {
        let p = plan(CpuLook::ChartCores);
        let gutter = f32::from(super::super::cpu_box::core_gutter(&p.m));
        let pad = f32::from(p.m.sp[2]);
        let r = rects_of(&p, 1000., 400.);
        assert_eq!(r[0], (1, [0., 0., 400., 400.]));
        assert_eq!(r[1], (2, [400. + gutter, 0., 600. - gutter, 400.]));
        let stacked = CpuPlan { side: false, ..p };
        let r = rects_of(&stacked, 400., 1000.);
        assert_eq!(r[0], (1, [0., 0., 400., 400.]));
        assert_eq!(r[1], (2, [0., 400. + gutter, 400., 600. - gutter - pad]));
    }

    #[test]
    fn strip_and_clocks_take_their_shares() {
        let gap = f32::from(plan(CpuLook::Chart).m.sp[5]);
        let p = CpuPlan {
            clocks: Some(3),
            power: Some(PlanPower {
                id: 4,
                parts_share: 0.75,
                under_graph: false,
            }),
            ..plan(CpuLook::Chart)
        };
        let r = rects_of(&p, 600., 400.);
        let half = (300. - gap) / 2.;
        assert_eq!(r[0], (4, [0., 300. + gap, 600., 100. - gap]));
        assert_eq!(r[1], (1, [0., 0., 600., half]));
        assert_eq!(r[2], (3, [0., half + gap, 600., half]));

        // Under the graph the strip shares the chart's column; the rows keep the full height.
        let p = CpuPlan {
            power: Some(PlanPower {
                id: 4,
                parts_share: 0.5,
                under_graph: true,
            }),
            ..plan(CpuLook::ChartCores)
        };
        let r = rects_of(&p, 1000., 400.);
        assert_eq!(r[0], (4, [0., 200. + gap, 400., 200. - gap]));
        assert_eq!(r[1], (1, [0., 0., 400., 200.]));
        assert_eq!(r[2].1[3], 400.);
    }

    #[test]
    fn plan_cell_reports_a_change_once() {
        let cell = PlanCell::default();
        assert!(cell.set(plan(CpuLook::Cores)));
        assert!(!cell.set(plan(CpuLook::Cores)));
        assert!(cell.set(plan(CpuLook::Chart)));
    }
}
