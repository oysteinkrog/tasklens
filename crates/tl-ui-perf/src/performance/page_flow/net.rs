//! The Network page's flow placement (ADR-019 phase 3 "Presets of the pages", UI-PRF-031;
//! `tasklens-hzire.8`).
//!
//! The 1.3.0 page in one column: the Throughput chart (`graph_h_single`, 240 px), Details
//! `details_gap` under it, then the Network settings link `sp6` under Details. The link takes no
//! space on a desktop without a network settings tool, as 1.3.0 hides it there.

use std::cell::Cell;
use std::rc::Rc;

use tl_ui_settings::layout::{FixedWidget, FormSet, WidgetContent, WidgetSetting};
use tl_ui_settings::layout_boxes::METRIC_NET_TRAFFIC;

use super::{BLOCK_GAP, PART_GAP, PagePart, PartHeight, column_flow, default_part};
use crate::performance::page::GRAPH_H_SINGLE;
use crate::performance::widget::compose::fixed_min;
use crate::performance::widget::container::FlowPlace;

/// The measured heights and the shown flag the Network page's flow reads.
#[derive(Clone, Debug, Default)]
pub struct NetParts {
    /// The Details widget's height (`DetailsView`).
    pub details: Rc<Cell<f32>>,
    /// The Network settings link's height (its part view).
    pub settings: Rc<Cell<f32>>,
    /// The desktop has a network settings tool: the link takes space.
    pub settings_shown: Rc<Cell<bool>>,
}

/// The part Network page widget `w` is in the flow, `None` when it takes no space.
pub fn net_part(w: &WidgetSetting, parts: &NetParts) -> Option<PagePart> {
    match &w.content {
        WidgetContent::Metric(source)
            if w.forms.contains(FormSet::CHART) && source.metric.as_str() == METRIC_NET_TRAFFIC =>
        {
            Some(PagePart {
                height: PartHeight::Fixed(GRAPH_H_SINGLE),
                gap: PART_GAP,
                pair: None,
            })
        }
        WidgetContent::Fixed(kind @ FixedWidget::NetSettings) => {
            parts.settings_shown.get().then(|| PagePart {
                height: PartHeight::Natural {
                    measured: Rc::clone(&parts.settings),
                    estimate: fixed_min(kind).height,
                },
                gap: BLOCK_GAP,
                pair: None,
            })
        }
        _ => Some(default_part(w, &parts.details)),
    }
}

/// The placer of the Network page's flow layouts.
pub fn net_flow(parts: NetParts) -> FlowPlace {
    column_flow(Rc::new(move |w: &WidgetSetting| net_part(w, &parts)))
}

#[cfg(test)]
mod tests {
    use gpui_kit::{px, size};
    use tl_ui_settings::layout::ContainerLayout;
    use tl_ui_settings::layout_pages::default_page_layout;

    use super::*;
    use crate::performance::page::PAGE_PAD;
    use crate::performance::page_flow::place_column;
    use crate::performance::widget::container::{FlowInput, FlowItem};
    use crate::performance::widget::grid::InnerPlacement;
    use crate::performance::widget::source::Availability;

    const PAGE: &str = "page.net:enp38s0";

    fn layout() -> ContainerLayout {
        default_page_layout(PAGE).expect("an adapter page default")
    }

    fn parts(shown: bool) -> NetParts {
        NetParts {
            details: Rc::new(Cell::new(400.)),
            settings: Rc::new(Cell::new(20.)),
            settings_shown: Rc::new(Cell::new(shown)),
        }
    }

    fn placed(l: &ContainerLayout, parts: &NetParts, scale: f32) -> InnerPlacement {
        let items: Vec<FlowItem<'_>> = l
            .widgets
            .iter()
            .map(|w| FlowItem {
                setting: w,
                availability: Availability::Available,
            })
            .collect();
        let input = FlowInput {
            layout: l,
            items: &items,
            area: size(px(1000. * scale), px(600. * scale)),
            scale,
            frozen: None,
        };
        place_column(&input, &|w| net_part(w, parts))
    }

    /// The default: Throughput, Details and the Network settings link at the 1.3.0 places
    /// (UI-PRF-031), at 1.0, 1.5 and 2.0 scale.
    #[test]
    fn default_places_the_1_3_0_parts() {
        let l = layout();
        let p = parts(true);
        for scale in [1.0, 1.5, 2.0] {
            let placed = placed(&l, &p, scale);
            let b = |id| placed.bounds(id).expect("placed");
            let r = |v: f32| px((v * scale).round());
            // Ids from 1 in list order: traffic, Details, the settings link.
            assert_eq!(b(1).origin.y, px(0.));
            assert_eq!(b(1).size.height, r(240.));
            assert_eq!(b(1).size.width, px(1000. * scale));
            assert_eq!(b(2).origin.y, r(240. + 24.));
            assert_eq!(b(2).size.height, r(400.));
            assert_eq!(b(3).origin.y, r(664. + 16.));
            assert_eq!(b(3).size.height, r(20.));
            assert_eq!(placed.content_h, px((700. + PAGE_PAD) * scale));
        }
    }

    /// Without a network settings tool the link takes no space and nothing follows Details.
    #[test]
    fn no_settings_tool_takes_no_space() {
        let l = layout();
        let placed = placed(&l, &parts(false), 1.);
        assert!(placed.bounds(3).is_none());
        assert_eq!(placed.widgets.len(), 2);
        assert_eq!(placed.content_h, px(664. + PAGE_PAD));
    }

    /// A Throughput widget without a chart is one row, as on every page.
    #[test]
    fn traffic_without_chart_is_a_row() {
        let mut l = layout();
        l.widgets[0].forms = FormSet::VALUE | FormSet::LABEL;
        let placed = placed(&l, &parts(true), 1.);
        let h = placed.bounds(1).expect("placed").size.height;
        assert_eq!(h, px(crate::performance::widget::compose::ROW_H));
    }
}
