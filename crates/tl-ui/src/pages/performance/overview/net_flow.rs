//! The Network box's presets as flow layouts (ADR-019 "Presets", UI-PRF-061 phase 2).
//!
//! A flow layout is the widget list of one Network box mode (`layout_boxes::convert_net_box`):
//! the `rx` and `tx` widgets of the counted total in `stacked` and `columns`, the `traffic`
//! widget of the counted total in `combined`, and the adapters group as cells in
//! `adapter_charts`. The box draws each at its 1.2.0 place by the rules of UI-PRF-017,
//! UI-PRF-019 and UI-PRF-059: the Download and Upload parts are `MetricWidget`s with
//! `FlowLook::NetDirection` (the rate line with its 60 s peak and the total in its tooltip, over
//! the chart), the `combined` body is a `MetricWidget` with `FlowLook::NetCombined`, and the
//! adapter cells are the adapters group's `GroupWidget` with `GroupFlowLook::NetCells` (whole
//! cells only and "+n more" are worked out inside it). The box draws only what is not a widget:
//! the title line and the splitter between the Download and Upload parts.
//!
//! The box makes a [`NetPlan`] on each render: the mode, the split as the box lays it out after
//! its clamps, the gap and the widget of each part, but no body size and no sample value. The
//! [`placer`] works out every widget's rectangle from the plan and the container's body size
//! with the same flex rules the box draws by ([`NetPlan::rects`]), so the container keeps the
//! entities of those widgets and Edit Widgets pins them where the user saw them. A sample never
//! changes the plan.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Bounds, Pixels, Size, point, px, size};
use smallvec::SmallVec;
use tl_ui_settings::layout::{
    ContainerLayout, DeviceSelector, GroupKind, WidgetContent, WidgetSetting, grid_cols,
};
use tl_ui_settings::layout_boxes::{
    DEVICE_NETS, METRIC_NET_RX, METRIC_NET_TRAFFIC, METRIC_NET_TX, convert_net_box,
};
use tl_ui_settings::settings::OverviewGridSetting;

use super::layout::Arrange;
use crate::pages::performance::widget::container::{FlowInput, FlowPlace, list_flow};
use crate::pages::performance::widget::grid::{self, InnerPlacement, PlacedWidget};
use crate::pages::performance::widget::source::Availability;

/// The Network box settings a flow layout is made from: the old keys of UI-PRF-019.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct NetFlowKeys<'a> {
    pub mode: &'a str,
    pub arrange: Option<Arrange>,
    pub split: Option<f32>,
}

/// The flow layout of the Network box with `keys`: the widget list of
/// `layout_boxes::convert_net_box`, so it is the same list a converted old setting gives.
pub fn flow_layout(keys: NetFlowKeys<'_>) -> ContainerLayout {
    convert_net_box(&OverviewGridSetting {
        kind: "net".to_owned(),
        mode: Some(keys.mode.to_owned()),
        arrange: keys.arrange.map(|a| a.id().to_owned()),
        split: keys.split.map(f64::from),
        ..OverviewGridSetting::default()
    })
}

/// Whether `w` shows `metric` of the counted total (`aggregate:net`).
fn is_total_metric(w: &WidgetSetting, metric: &str) -> bool {
    w.content.metric().is_some_and(|s| {
        s.metric.as_str() == metric
            && matches!(&s.device, DeviceSelector::Key(k) if &**k == DEVICE_NETS)
    })
}

/// The id of the widget showing `metric` of the counted total.
fn total_widget(layout: &ContainerLayout, metric: &str) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| is_total_metric(w, metric))
        .map(|w| w.id)
}

/// The id of the Download part: `rx` of the counted total.
pub fn down_of(layout: &ContainerLayout) -> Option<u32> {
    total_widget(layout, METRIC_NET_RX)
}

/// The id of the Upload part: `tx` of the counted total.
pub fn up_of(layout: &ContainerLayout) -> Option<u32> {
    total_widget(layout, METRIC_NET_TX)
}

/// The id of the `combined` widget: Download and upload of the counted total.
pub fn traffic_of(layout: &ContainerLayout) -> Option<u32> {
    total_widget(layout, METRIC_NET_TRAFFIC)
}

/// The id of the adapters group: the `adapter_charts` cells.
pub fn adapters_of(layout: &ContainerLayout) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| matches!(&w.content, WidgetContent::Group(g) if g.members == GroupKind::Adapters))
        .map(|w| w.id)
}

/// The Network box mode a [`NetPlan`] is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NetLook {
    /// `stacked`: the Download part over the Upload part.
    Stacked,
    /// `columns`: the Download and Upload parts side by side.
    Columns,
    /// `combined`: the two rate lines over one chart with both lines.
    Combined,
    /// `adapter_charts`: one combined chart cell per counted adapter.
    AdapterCharts,
}

impl NetLook {
    /// The preset id of the mode.
    pub fn id(self) -> &'static str {
        match self {
            Self::Stacked => "stacked",
            Self::Columns => "columns",
            Self::Combined => "combined",
            Self::AdapterCharts => "adapter_charts",
        }
    }
}

/// What the Network box shows in a flow layout, apart from the body size: the mode, the
/// Download part's share and the gap after it as the box lays them out, and the widget of each
/// part. The box makes it from its mode and split on each render.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct NetPlan {
    pub look: NetLook,
    /// `stacked` and `columns`: the Download part's share of the body height or width.
    pub share: f32,
    /// `stacked` and `columns`: the gap between the two parts, px.
    pub gap: f32,
    /// The Download part in `stacked` and `columns`, else the one widget that fills the body.
    pub first: Option<u32>,
    /// The Upload part in `stacked` and `columns`.
    pub second: Option<u32>,
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

impl NetPlan {
    /// Every widget's rectangle in a body of `area`, px from the body's top left corner.
    pub fn rects(&self, area: Size<Pixels>) -> SmallVec<[(u32, Bounds<Pixels>); 2]> {
        let (w, h) = (f32::from(area.width), f32::from(area.height));
        let mut out = SmallVec::new();
        match self.look {
            NetLook::Stacked => {
                let first = first_len(self.share, h, self.gap);
                if let Some(id) = self.first {
                    out.push((id, rect(0., 0., w, first)));
                }
                if let Some(id) = self.second {
                    let y = first + self.gap;
                    out.push((id, rect(0., y, w, h - y)));
                }
            }
            NetLook::Columns => {
                let first = first_len(self.share, w, self.gap);
                if let Some(id) = self.first {
                    out.push((id, rect(0., 0., first, h)));
                }
                if let Some(id) = self.second {
                    let x = first + self.gap;
                    out.push((id, rect(x, 0., w - x, h)));
                }
            }
            NetLook::Combined | NetLook::AdapterCharts => {
                if let Some(id) = self.first {
                    out.push((id, rect(0., 0., w, h)));
                }
            }
        }
        out
    }

    /// The widgets the plan draws, in drawing order.
    pub fn drawn(&self) -> SmallVec<[u32; 2]> {
        self.first.into_iter().chain(self.second).collect()
    }
}

/// The [`NetPlan`] shared by the box (which makes it) and its [`placer`].
#[derive(Clone, Default, Debug)]
pub struct PlanCell(Rc<RefCell<Option<NetPlan>>>);

impl PlanCell {
    /// Takes the plan of a render. Returns whether it changed, so the box must place its
    /// widgets again.
    pub fn set(&self, plan: NetPlan) -> bool {
        let mut st = self.0.borrow_mut();
        let changed = *st != Some(plan);
        *st = Some(plan);
        changed
    }

    /// The plan of the last flow render.
    pub fn plan(&self) -> Option<NetPlan> {
        *self.0.borrow()
    }

    /// The widgets the last flow render drew, in drawing order.
    pub fn drawn(&self) -> SmallVec<[u32; 2]> {
        self.plan().map(|p| p.drawn()).unwrap_or_default()
    }
}

/// The placer of the Network box's flow layouts: the widgets of the box's plan at the places
/// the box draws them ([`NetPlan::rects`]). A widget the plan leaves out is not placed, so it
/// has no entity. In Edit Widgets, when none of the layout's widgets is in the plan (the layout
/// is not the one the box shows, as after Reset), the widgets go in list order
/// (`container::list_flow`).
pub fn placer(plan: PlanCell) -> FlowPlace {
    Rc::new(move |input: &FlowInput<'_>| place(&plan, input))
}

fn place(cell: &PlanCell, input: &FlowInput<'_>) -> InnerPlacement {
    let mut widgets = Vec::new();
    let preset = input.layout.preset.as_ref().map(|p| p.id.as_str());
    if let Some(plan) = cell.plan().filter(|p| preset == Some(p.look.id())) {
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
    use tl_ui_settings::layout::{FormSet, GroupItems, NET_BOX_CONTAINER};

    use super::*;

    fn keys(mode: &str) -> NetFlowKeys<'_> {
        NetFlowKeys {
            mode,
            arrange: None,
            split: None,
        }
    }

    #[test]
    fn each_mode_gives_its_widgets() {
        let forms = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
        for mode in ["stacked", "columns"] {
            let l = flow_layout(keys(mode));
            assert_eq!(l.container, NET_BOX_CONTAINER);
            assert_eq!(l.preset.as_ref().map(|p| p.id.as_str()), Some(mode));
            assert_eq!(l.widgets.len(), 2, "{mode}");
            let (down, up) = (down_of(&l).expect("rx"), up_of(&l).expect("tx"));
            assert_eq!([down, up], [1, 2], "{mode}: Download first");
            assert!(l.widgets.iter().all(|w| w.forms == forms));
            assert!(traffic_of(&l).is_none() && adapters_of(&l).is_none());
        }

        let l = flow_layout(keys("combined"));
        assert_eq!(l.widgets.len(), 1);
        let id = traffic_of(&l).expect("traffic");
        assert_eq!(l.widgets[0].id, id);
        assert_eq!(l.widgets[0].forms, forms);
        assert!(down_of(&l).is_none() && adapters_of(&l).is_none());

        let l = flow_layout(NetFlowKeys {
            arrange: Some(Arrange::Vertical),
            ..keys("adapter_charts")
        });
        assert_eq!(l.widgets.len(), 1);
        let id = adapters_of(&l).expect("the adapters group");
        let WidgetContent::Group(g) = &l.widgets[0].content else {
            panic!("a group: {l:?}");
        };
        assert_eq!(l.widgets[0].id, id);
        assert_eq!(g.metric.as_str(), METRIC_NET_TRAFFIC);
        assert_eq!(g.options.items, GroupItems::Cells);
        assert_eq!(g.options.limit, None);
        assert_eq!(g.options.arrange.as_deref(), Some("vertical"));
        assert_eq!(l.widgets[0].forms, forms);
    }

    fn plan(look: NetLook) -> NetPlan {
        NetPlan {
            look,
            share: 0.5,
            gap: 6.,
            first: Some(1),
            second: Some(2),
        }
    }

    #[test]
    fn stacked_and_columns_split_the_body() {
        let area = size(px(360.), px(206.));
        let r = plan(NetLook::Stacked).rects(area);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], (1, rect(0., 0., 360., 103.)));
        assert_eq!(r[1], (2, rect(0., 109., 360., 97.)));

        let r = plan(NetLook::Columns).rects(area);
        assert_eq!(r[0], (1, rect(0., 0., 180., 206.)));
        assert_eq!(r[1], (2, rect(186., 0., 174., 206.)));

        // A share that leaves no room for the gap gives the second part nothing.
        let full = NetPlan {
            share: 1.,
            ..plan(NetLook::Stacked)
        };
        let r = full.rects(area);
        assert_eq!(r[0].1.size.height, px(200.));
        assert_eq!(r[1].1.size.height, px(0.));
    }

    #[test]
    fn one_widget_fills_the_body() {
        let area = size(px(360.), px(240.));
        for look in [NetLook::Combined, NetLook::AdapterCharts] {
            let p = NetPlan {
                second: None,
                ..plan(look)
            };
            let r = p.rects(area);
            assert_eq!(r.len(), 1);
            assert_eq!(r[0], (1, rect(0., 0., 360., 240.)));
            assert_eq!(p.drawn().as_slice(), [1]);
        }
        let none = NetPlan {
            first: None,
            second: None,
            ..plan(NetLook::Combined)
        };
        assert!(none.rects(area).is_empty());
    }

    #[test]
    fn plan_cell_reports_a_change_once() {
        let cell = PlanCell::default();
        let p = plan(NetLook::Stacked);
        assert!(cell.set(p));
        assert!(!cell.set(p));
        assert_eq!(cell.drawn().as_slice(), [1, 2]);
        assert!(cell.set(NetPlan { share: 0.4, ..p }));
    }
}
