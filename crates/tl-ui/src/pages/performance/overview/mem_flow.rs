//! The Memory box's presets as flow layouts (ADR-019 "Presets", UI-PRF-061 phase 2).
//!
//! A flow layout is the widget list of one Memory box mode (`layout_boxes::convert_memory_box`):
//! the five rows (In use, Cached, Available, Swap, Commit) with their sparklines in `rows`, the
//! used chart over the five rows in `chart_rows`, the composition chart in `chart`, one chart
//! cell per row in `charts`, and the Memory by app fixed widget in `apps`. The box draws each
//! part at its 1.2.0 place by the rules of UI-PRF-017, UI-PRF-019 and UI-PRF-060, and each part
//! is its widget's own entity: a row or a cell is a `MetricWidget` with `FlowLook::MemRow` or
//! `FlowLook::MemCell`, the charts are `MetricWidget`s with `FlowLook::Chart`, and Memory by
//! app is a `FixedWidgetView`. The box draws only what is not a widget: the title line and the
//! splitters.
//!
//! The box makes a [`MemPlan`] on each render: the mode, the split as the box lays it out, the
//! row height class and the widget of each part, but no body size and no sample value. The
//! [`placer`] works out every widget's rectangle from the plan and the container's body size
//! with the same flex and grid rules the box draws by ([`MemPlan::rects`]), so the container
//! keeps the entities of those widgets and Edit Widgets pins them where the user saw them. A
//! sample never changes the plan.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Bounds, Pixels, Size, point, px, size};
use smallvec::SmallVec;
use tl_model::layout::{
    ContainerLayout, DeviceSelector, FixedWidget, FormSet, WidgetContent, WidgetSetting, grid_cols,
};
use tl_model::layout_boxes::{
    DEVICE_MEMORY, METRIC_MEM_AVAILABLE, METRIC_MEM_CACHED, METRIC_MEM_COMMIT,
    METRIC_MEM_COMPOSITION, METRIC_MEM_SWAP, METRIC_MEM_USED, convert_memory_box,
};
use tl_model::settings::OverviewGridSetting;

use super::layout::Arrange;
use super::mem_box::{ROWS, charts_fit, row_min_h};
use crate::pages::performance::widget::container::{FlowInput, FlowPlace, list_flow};
use crate::pages::performance::widget::grid::{self, InnerPlacement, PlacedWidget};
use crate::pages::performance::widget::source::Availability;
use crate::theme::UiMetrics;

/// The metric of each Memory box row, in row order.
pub const ROW_KEYS: [&str; ROWS] = [
    METRIC_MEM_USED,
    METRIC_MEM_CACHED,
    METRIC_MEM_AVAILABLE,
    METRIC_MEM_SWAP,
    METRIC_MEM_COMMIT,
];

/// The slack a fit check allows for sub-pixel layout, px (as `fit::WholeRows`).
const EPS: f32 = 0.01;

/// The Memory box settings a flow layout is made from: the old keys of UI-PRF-019.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MemFlowKeys<'a> {
    pub mode: &'a str,
    pub arrange: Option<Arrange>,
    pub split: Option<f32>,
}

/// The flow layout of the Memory box with `keys`: the widget list of
/// `layout_boxes::convert_memory_box`, so it is the same list a converted old setting gives.
pub fn flow_layout(keys: MemFlowKeys<'_>) -> ContainerLayout {
    convert_memory_box(&OverviewGridSetting {
        kind: "memory".to_owned(),
        mode: Some(keys.mode.to_owned()),
        arrange: keys.arrange.map(|a| a.id().to_owned()),
        split: keys.split.map(f64::from),
        ..OverviewGridSetting::default()
    })
}

/// Whether `w` shows `metric` of memory.
fn is_mem_metric(w: &WidgetSetting, metric: &str) -> bool {
    w.content.metric().is_some_and(|s| {
        s.metric.as_str() == metric
            && matches!(&s.device, DeviceSelector::Key(k) if &**k == DEVICE_MEMORY)
    })
}

/// The id of row `i`'s widget: the widget on the row's metric that is not the used chart of
/// `chart_rows` (the chart alone).
pub fn row_of(layout: &ContainerLayout, i: usize) -> Option<u32> {
    let metric = ROW_KEYS.get(i)?;
    layout
        .widgets
        .iter()
        .find(|w| is_mem_metric(w, metric) && w.forms != FormSet::CHART)
        .map(|w| w.id)
}

/// The ids of the five rows' widgets, in row order.
pub fn rows_of(layout: &ContainerLayout) -> [Option<u32>; ROWS] {
    std::array::from_fn(|i| row_of(layout, i))
}

/// The id of the used chart of `chart_rows`: In use with the chart alone.
pub fn used_chart_of(layout: &ContainerLayout) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| is_mem_metric(w, METRIC_MEM_USED) && w.forms == FormSet::CHART)
        .map(|w| w.id)
}

/// The id of the composition chart of `chart`.
pub fn comp_of(layout: &ContainerLayout) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| is_mem_metric(w, METRIC_MEM_COMPOSITION))
        .map(|w| w.id)
}

/// The id of the Memory by app widget.
pub fn apps_of(layout: &ContainerLayout) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| matches!(&w.content, WidgetContent::Fixed(FixedWidget::MemoryByApp)))
        .map(|w| w.id)
}

/// The Memory box mode a [`MemPlan`] is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemLook {
    /// `rows`: the five rows, each with its sparkline beside it.
    Rows,
    /// `chart_rows`: the used chart over (or beside) the five rows.
    ChartRows,
    /// `chart`: the composition chart alone.
    Chart,
    /// `charts`: one chart cell per row.
    Charts,
    /// `apps`: Memory by app alone.
    Apps,
}

impl MemLook {
    /// The preset id of the mode.
    pub fn id(self) -> &'static str {
        match self {
            Self::Rows => "rows",
            Self::ChartRows => "chart_rows",
            Self::Chart => "chart",
            Self::Charts => "charts",
            Self::Apps => "apps",
        }
    }
}

/// What the Memory box shows in a flow layout, apart from the body size: the mode, the
/// placement of the two parts of `chart_rows`, the chart's share as the box lays it out, the
/// row height class, the arrangement of the `charts` cells, and the widget of each part. The box
/// makes it from its mode and settings on each render.
#[derive(Clone, PartialEq, Debug)]
pub struct MemPlan {
    pub look: MemLook,
    pub m: UiMetrics,
    /// `chart_rows`: the chart sits beside the rows (else over them).
    pub side: bool,
    /// `chart_rows`: the chart's share of the body width (side by side) or height.
    pub share: f32,
    /// The rows are one line high (else two).
    pub wide: bool,
    /// `charts`: how the cells are placed.
    pub arrange: Arrange,
    /// The row widgets, in row order.
    pub rows: [Option<u32>; ROWS],
    /// The used chart of `chart_rows` or the composition chart of `chart`.
    pub chart: Option<u32>,
    /// Memory by app, in `apps`.
    pub apps: Option<u32>,
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

impl MemPlan {
    /// Every widget's rectangle in a body of `area`, px from the body's top left corner.
    pub fn rects(&self, area: Size<Pixels>) -> SmallVec<[(u32, Bounds<Pixels>); 6]> {
        let (w, h) = (f32::from(area.width), f32::from(area.height));
        let mut out = SmallVec::new();
        match self.look {
            MemLook::Rows => self.rows_rects(rect(0., 0., w, h), &mut out),
            MemLook::ChartRows => {
                let gap = f32::from(self.m.sp[5]);
                let (chart, rows) = if self.side {
                    let cw = first_len(self.share, w, gap);
                    (rect(0., 0., cw, h), rect(cw + gap, 0., w - cw - gap, h))
                } else {
                    let ch = first_len(self.share, h, gap);
                    (rect(0., 0., w, ch), rect(0., ch + gap, w, h - ch - gap))
                };
                if let Some(id) = self.chart {
                    out.push((id, chart));
                }
                self.rows_rects(rows, &mut out);
            }
            MemLook::Chart => {
                if let Some(id) = self.chart {
                    out.push((id, rect(0., 0., w, h)));
                }
            }
            MemLook::Charts => {
                let fit = charts_fit(self.arrange, area, &self.m);
                let gap = f32::from(self.m.sp[2]);
                let (cw, ch) = (f32::from(fit.cell.width), f32::from(fit.cell.height));
                let cols = fit.cols.max(1);
                for (i, id) in self.rows.iter().enumerate() {
                    let Some(id) = id else { continue };
                    let (col, row) = ((i % cols) as f32, (i / cols) as f32);
                    out.push((*id, rect(col * (cw + gap), row * (ch + gap), cw, ch)));
                }
            }
            MemLook::Apps => {
                if let Some(id) = self.apps {
                    out.push((id, rect(0., 0., w, h)));
                }
            }
        }
        out
    }

    /// The rows in `part`: a column of rows that share its height with `sp[2]` between them,
    /// each at least one line (two when not `wide`) high. A row that does not fit whole is
    /// left out, as `fit::WholeRows` draws them.
    fn rows_rects(&self, part: Bounds<Pixels>, out: &mut SmallVec<[(u32, Bounds<Pixels>); 6]>) {
        let n = self.rows.iter().filter(|r| r.is_some()).count();
        if n == 0 {
            return;
        }
        let (x, y) = (f32::from(part.origin.x), f32::from(part.origin.y));
        let (w, h) = (f32::from(part.size.width), f32::from(part.size.height));
        let gap = f32::from(self.m.sp[2]);
        let min = f32::from(row_min_h(self.wide, self.m.ui_scale));
        let row_h = ((h - gap * (n - 1) as f32) / n as f32).max(min);
        let mut top = 0.;
        for id in self.rows.iter().flatten() {
            if top + row_h > h + EPS {
                break;
            }
            out.push((*id, rect(x, y + top, w, row_h)));
            top += row_h + gap;
        }
    }
}

/// The plan of the last render and the widgets it drew, in drawing order.
#[derive(Default, Debug)]
struct PlanState {
    plan: Option<MemPlan>,
    drawn: SmallVec<[u32; 6]>,
}

/// The [`MemPlan`] shared by the box (which makes it) and its [`placer`].
#[derive(Clone, Default, Debug)]
pub struct PlanCell(Rc<RefCell<PlanState>>);

impl PlanCell {
    /// Takes the plan of a render. Returns whether it changed, so the box must place its
    /// widgets again.
    pub fn set(&self, plan: MemPlan) -> bool {
        let mut st = self.0.borrow_mut();
        let changed = st.plan.as_ref() != Some(&plan);
        st.plan = Some(plan);
        changed
    }

    /// The plan of the last flow render.
    pub fn plan(&self) -> Option<MemPlan> {
        self.0.borrow().plan.clone()
    }

    /// The widgets a render drew, in drawing order.
    pub fn set_drawn(&self, drawn: SmallVec<[u32; 6]>) {
        self.0.borrow_mut().drawn = drawn;
    }

    /// The widgets the last flow render drew, in drawing order.
    pub fn drawn(&self) -> SmallVec<[u32; 6]> {
        self.0.borrow().drawn.clone()
    }
}

/// The placer of the Memory box's flow layouts: every widget of the box's plan at the
/// rectangle the box's flex and grid rules give it in the container's body now
/// ([`MemPlan::rects`]). A widget the plan leaves out is not placed, so it has no entity. In
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
    use tl_model::layout::MEMORY_BOX_CONTAINER;

    use super::*;

    fn keys(mode: &str) -> MemFlowKeys<'_> {
        MemFlowKeys {
            mode,
            arrange: None,
            split: None,
        }
    }

    #[test]
    fn each_mode_gives_its_widgets() {
        let l = flow_layout(keys("rows"));
        assert_eq!(l.container, MEMORY_BOX_CONTAINER);
        assert_eq!(l.preset.as_ref().map(|p| p.id.as_str()), Some("rows"));
        let rows = rows_of(&l);
        assert!(rows.iter().all(Option::is_some), "{l:?}");
        let forms = FormSet::CHART | FormSet::BAR | FormSet::VALUE | FormSet::LABEL;
        assert!(l.widgets.iter().all(|w| w.forms == forms), "{l:?}");
        assert!(used_chart_of(&l).is_none() && comp_of(&l).is_none() && apps_of(&l).is_none());

        let l = flow_layout(keys("chart_rows"));
        let chart = used_chart_of(&l).expect("the used chart");
        let rows = rows_of(&l);
        assert!(rows.iter().all(|r| r.is_some_and(|r| r != chart)), "{l:?}");
        let row = l.widgets.iter().find(|w| Some(w.id) == rows[0]).unwrap();
        assert_eq!(row.forms, FormSet::BAR | FormSet::VALUE | FormSet::LABEL);

        let l = flow_layout(keys("chart"));
        assert!(comp_of(&l).is_some() && rows_of(&l).iter().all(Option::is_none));

        let l = flow_layout(MemFlowKeys {
            arrange: Some(Arrange::Vertical),
            ..keys("charts")
        });
        assert!(rows_of(&l).iter().all(Option::is_some));
        assert_eq!(
            l.preset.as_ref().and_then(|p| p.arrange.as_deref()),
            Some("vertical")
        );

        let l = flow_layout(keys("apps"));
        assert_eq!(l.widgets.len(), 1);
        assert!(apps_of(&l).is_some());
    }

    fn plan(look: MemLook) -> MemPlan {
        MemPlan {
            look,
            m: crate::theme::metrics::compute(
                &tl_model::settings::AppearanceSettings::default(),
                0,
            ),
            side: false,
            share: 0.5,
            wide: true,
            arrange: Arrange::Auto,
            rows: [Some(1), Some(2), Some(3), Some(4), Some(5)],
            chart: None,
            apps: None,
        }
    }

    fn rects_of(plan: &MemPlan, w: f32, h: f32) -> Vec<(u32, [f32; 4])> {
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
    fn rows_share_the_height_then_drop_from_the_bottom() {
        let p = plan(MemLook::Rows);
        let gap = f32::from(p.m.sp[2]);
        let r = rects_of(&p, 300., 5. * 40. + 4. * gap);
        assert_eq!(r.len(), 5);
        for (i, (id, b)) in r.iter().enumerate() {
            assert_eq!(*id, i as u32 + 1);
            assert_eq!(*b, [0., i as f32 * (40. + gap), 300., 40.]);
        }
        // Two-line rows that do not all fit: the rows keep their minimum and the last ones go.
        let narrow = MemPlan { wide: false, ..p };
        let min = f32::from(row_min_h(false, 1.));
        let r = rects_of(&narrow, 300., 3. * min + 2. * gap + 1.);
        assert_eq!(r.len(), 3, "{r:?}");
        assert_eq!(r[2].1[3], min);
    }

    #[test]
    fn chart_rows_stacked_and_side_by_side() {
        let p = MemPlan {
            chart: Some(9),
            ..plan(MemLook::ChartRows)
        };
        let gap = f32::from(p.m.sp[5]);
        let r = rects_of(&p, 400., 600.);
        assert_eq!(r[0], (9, [0., 0., 400., 300.]));
        assert_eq!(r[1].1[1], 300. + gap, "the rows start under the chart");
        let side = MemPlan { side: true, ..p };
        let r = rects_of(&side, 800., 300.);
        assert_eq!(r[0], (9, [0., 0., 400., 300.]));
        assert_eq!(r[1].1[0], 400. + gap, "the rows start right of the chart");
        assert_eq!(r[1].1[2], 400. - gap);
    }

    #[test]
    fn charts_place_one_cell_per_row() {
        let p = MemPlan {
            arrange: Arrange::Vertical,
            ..plan(MemLook::Charts)
        };
        let gap = f32::from(p.m.sp[2]);
        let r = rects_of(&p, 360., 5. * 70. + 4. * gap);
        assert_eq!(r.len(), 5);
        for (i, (_, b)) in r.iter().enumerate() {
            assert_eq!(b[0], 0.);
            assert!((b[1] - i as f32 * (b[3] + gap)).abs() < 0.5, "{r:?}");
        }
    }

    #[test]
    fn plan_cell_reports_a_change_once() {
        let cell = PlanCell::default();
        assert!(cell.set(plan(MemLook::Rows)));
        assert!(!cell.set(plan(MemLook::Rows)));
        assert!(cell.set(plan(MemLook::Chart)));
    }
}
