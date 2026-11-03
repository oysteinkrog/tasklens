//! The GPU page's flow placement (ADR-019 phase 3 "Presets of the pages", UI-PRF-032;
//! `tasklens-hzire.4`).
//!
//! The 1.3.0 page in one column: the engine grid as tall as its rows at the page width, the
//! Memory and Temperature charts side by side (150 px), the Power and Clocks charts side by
//! side when the layout lists them (one alone takes the full width), Details, then Displays.
//! With every engine hidden the engine grid takes no space; while the probe is missing it is
//! the one placeholder cell.

use std::cell::Cell;
use std::rc::Rc;

use tl_ui_settings::layout::{
    FixedWidget, FormSet, METRIC_POWER, METRIC_TEMP, WidgetContent, WidgetSetting,
};
use tl_ui_settings::layout_pages::{METRIC_GPU_CLOCKS, METRIC_GPU_MEMORY};

use super::{BLOCK_GAP, PART_GAP, PagePart, PartHeight, column_flow, default_part};
use crate::performance::gpu_views::ENGINE_CELL_H;
use crate::performance::page::GRAPH_H_STACKED;
use crate::performance::widget::compose::fixed_min;
use crate::performance::widget::container::FlowPlace;

/// The pair row of Memory and Temperature.
const ROW_MEMORY: u8 = 1;
/// The pair row of Power and Clocks.
const ROW_OPTIONAL: u8 = 2;

/// The measured heights and the engine count the GPU page's flow reads.
#[derive(Clone, Debug)]
pub struct GpuParts {
    /// The engine grid's height at the page width, logical px (`GpuEngineGridView`).
    pub engines: Rc<Cell<f32>>,
    /// The cells the engine grid draws: 0 with every engine hidden.
    pub engine_cells: Rc<Cell<usize>>,
    /// The Details widget's height (`DetailsView`).
    pub details: Rc<Cell<f32>>,
    /// The Displays block's height (`GpuDisplaysView`).
    pub displays: Rc<Cell<f32>>,
}

/// The part GPU page widget `w` is in the flow, `None` when it takes no space.
pub fn gpu_part(w: &WidgetSetting, parts: &GpuParts) -> Option<PagePart> {
    let chart = |row| PagePart {
        height: PartHeight::Fixed(GRAPH_H_STACKED),
        gap: PART_GAP,
        pair: Some(row),
    };
    match &w.content {
        WidgetContent::Fixed(FixedWidget::GpuEngineGrid) => {
            (parts.engine_cells.get() > 0).then(|| PagePart {
                height: PartHeight::Natural {
                    measured: Rc::clone(&parts.engines),
                    estimate: ENGINE_CELL_H,
                },
                gap: PART_GAP,
                pair: None,
            })
        }
        WidgetContent::Fixed(FixedWidget::GpuDisplays) => Some(PagePart {
            height: PartHeight::Natural {
                measured: Rc::clone(&parts.displays),
                estimate: fixed_min(&FixedWidget::GpuDisplays).height,
            },
            gap: BLOCK_GAP,
            pair: None,
        }),
        WidgetContent::Metric(source) if w.forms.contains(FormSet::CHART) => {
            Some(match source.metric.as_str() {
                METRIC_GPU_MEMORY | METRIC_TEMP => chart(ROW_MEMORY),
                METRIC_POWER | METRIC_GPU_CLOCKS => chart(ROW_OPTIONAL),
                _ => default_part(w, &parts.details),
            })
        }
        _ => Some(default_part(w, &parts.details)),
    }
}

/// The placer of the GPU page's flow layouts.
pub fn gpu_flow(parts: GpuParts) -> FlowPlace {
    column_flow(Rc::new(move |w: &WidgetSetting| gpu_part(w, &parts)))
}

#[cfg(test)]
mod tests {
    use gpui_kit::{px, size};
    use tl_ui_settings::layout::ContainerLayout;
    use tl_ui_settings::layout_pages::convert_gpu_page;
    use tl_ui_settings::settings::{GpuOptionalGraph, PerformanceSettings};

    use super::*;
    use crate::performance::page::PAGE_PAD;
    use crate::performance::page_flow::place_column;
    use crate::performance::widget::container::{FlowInput, FlowItem};
    use crate::performance::widget::grid::InnerPlacement;
    use crate::performance::widget::source::Availability;

    const PAGE: &str = "page.gpu:GPU-0a1b2c3d";

    fn layout(optional: &[GpuOptionalGraph]) -> ContainerLayout {
        let perf = PerformanceSettings {
            gpu_optional_graphs: optional.to_vec(),
            ..PerformanceSettings::default()
        };
        convert_gpu_page(&perf, PAGE)
    }

    fn parts(cells: usize) -> GpuParts {
        GpuParts {
            engines: Rc::new(Cell::new(232.)),
            engine_cells: Rc::new(Cell::new(cells)),
            details: Rc::new(Cell::new(300.)),
            displays: Rc::new(Cell::new(56.)),
        }
    }

    fn placed(l: &ContainerLayout, parts: &GpuParts) -> InnerPlacement {
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
            area: size(px(1000.), px(600.)),
            scale: 1.,
            frozen: None,
        };
        place_column(&input, &|w| gpu_part(w, parts))
    }

    /// The default: engines, Memory and Temperature side by side, Details, Displays, at the
    /// 1.3.0 places (UI-PRF-032).
    #[test]
    fn default_places_the_1_3_0_parts() {
        let l = layout(&[]);
        let p = placed(&l, &parts(4));
        let b = |id| p.bounds(id).expect("placed");
        // Ids from 1 in list order: engines, memory, temp, Details, Displays.
        assert_eq!(b(1).origin.y, px(0.));
        assert_eq!(b(1).size.height, px(232.));
        assert_eq!(b(2).origin.y, px(240.));
        assert_eq!(b(3).origin.y, px(240.));
        assert_eq!(b(2).size.width, px(496.));
        assert_eq!(b(3).origin.x, px(504.));
        assert_eq!(b(2).size.height, px(150.));
        assert_eq!(b(4).origin.y, px(240. + 150. + 24.));
        assert_eq!(b(5).origin.y, px(414. + 300. + 16.));
        assert_eq!(p.content_h, px(730. + 56. + PAGE_PAD));
    }

    /// Power alone takes the full width under Memory and Temperature; with Clocks the two
    /// split the row.
    #[test]
    fn optional_row_one_or_two() {
        let l = layout(&[GpuOptionalGraph::Power]);
        let p = placed(&l, &parts(4));
        let power = p.bounds(4).expect("power");
        assert_eq!(power.origin.y, px(240. + 150. + 8.));
        assert_eq!(power.size.width, px(1000.));
        let l = layout(&[GpuOptionalGraph::Power, GpuOptionalGraph::Clocks]);
        let p = placed(&l, &parts(4));
        let (power, clocks) = (p.bounds(4).expect("power"), p.bounds(5).expect("clocks"));
        assert_eq!(power.origin.y, clocks.origin.y);
        assert_eq!(power.size.width, px(496.));
        assert_eq!(clocks.origin.x, px(504.));
    }

    /// With every engine hidden the grid takes no space and the charts move up.
    #[test]
    fn hidden_engines_take_no_space() {
        let l = layout(&[]);
        let p = placed(&l, &parts(0));
        assert!(p.bounds(1).is_none());
        assert_eq!(p.bounds(2).expect("memory").origin.y, px(0.));
    }
}
