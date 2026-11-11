//! The GPU page's flow placement (ADR-019 phase 3 "Presets of the pages", UI-PRF-032;
//! `tasklens-hzire.4`).
//!
//! The 1.3.0 page in one column: the engine grid as tall as its rows at the page width, the
//! Memory and Temperature charts side by side (150 px), the Power and Clocks charts side by
//! side when the layout lists them (one alone takes the full width), Details, then Displays.
//! With every engine hidden the engine grid takes no space; while the probe is missing it is
//! the one placeholder cell.
//!
//! GPU load by app and VRAM by app (presets `gpu_apps` and `vram_apps`) are 300 px parts at the
//! full width, as Memory by app. With VRAM by app in the Memory chart's place, Temperature
//! alone takes the full width.
//!
//! The Usage, Load by app and VRAM by app switch is not a widget: as on the Memory page, the
//! placer keeps its height above the first part and says where it goes ([`GpuParts::switch_at`]),
//! and the container draws the page's switch view there.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{Bounds, Pixels, point, px, size};
use tl_ui_settings::layout::{
    FixedWidget, FormSet, METRIC_POWER, METRIC_TEMP, WidgetContent, WidgetSetting,
};
use tl_ui_settings::layout_pages::{METRIC_GPU_CLOCKS, METRIC_GPU_MEMORY};

use super::{BLOCK_GAP, PART_GAP, PagePart, PartHeight, default_part, place_column};
use crate::performance::gpu_views::ENGINE_CELL_H;
use crate::performance::page::{GRAPH_H_OVERALL, GRAPH_H_STACKED};
use crate::performance::widget::compose::fixed_min;
use crate::performance::widget::container::{FlowInput, FlowPlace};
use crate::performance::widget::grid::InnerPlacement;

/// The switch's height before it is measured, logical px.
const SWITCH_ESTIMATE: f32 = 28.;

/// The pair row of Memory and Temperature.
const ROW_MEMORY: u8 = 1;
/// The pair row of Power and Clocks.
const ROW_OPTIONAL: u8 = 2;

/// The measured heights and the engine count the GPU page's flow reads, and where the flow
/// puts the switch.
#[derive(Clone, Debug)]
pub struct GpuParts {
    /// The Usage, Load by app and VRAM by app switch's height, logical px (its
    /// `MemoryPartView`).
    pub switch: Rc<Cell<f32>>,
    /// Where the last placement put the switch, body px; the container's flow decoration.
    pub switch_at: Rc<Cell<Option<Bounds<Pixels>>>>,
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
        WidgetContent::Fixed(FixedWidget::GpuByApp | FixedWidget::VramByApp) => Some(PagePart {
            height: PartHeight::Fixed(GRAPH_H_OVERALL),
            gap: PART_GAP,
            pair: None,
        }),
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

/// [`gpu_flow`]'s placement of `input`: the column of [`gpu_part`] under the switch's row, and
/// the switch at the top of the body, as wide as it.
pub fn place_gpu(input: &FlowInput<'_>, parts: &GpuParts) -> InnerPlacement {
    let mut placement = place_column(input, &|w| gpu_part(w, parts));
    if placement.widgets.is_empty() {
        parts.switch_at.set(None);
        return placement;
    }
    let s = if input.scale.is_finite() && input.scale > 0. {
        input.scale
    } else {
        1.
    };
    let measured = parts.switch.get();
    let h = (if measured > 0. {
        measured
    } else {
        SWITCH_ESTIMATE
    } * s)
        .round();
    for w in &mut placement.widgets {
        w.bounds.origin.y += px(h);
    }
    placement.content_h += px(h);
    placement.scrolls = f32::from(placement.content_h) > f32::from(input.area.height) + 0.5;
    parts.switch_at.set(Some(Bounds::new(
        point(px(0.), px(0.)),
        size(input.area.width, px(h)),
    )));
    placement
}

/// The placer of the GPU page's flow layouts.
pub fn gpu_flow(parts: GpuParts) -> FlowPlace {
    Rc::new(move |input: &FlowInput<'_>| place_gpu(input, &parts))
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
    use crate::performance::widget::container::FlowItem;
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
            switch: Rc::new(Cell::new(30.)),
            switch_at: Rc::default(),
            engines: Rc::new(Cell::new(232.)),
            engine_cells: Rc::new(Cell::new(cells)),
            details: Rc::new(Cell::new(300.)),
            displays: Rc::new(Cell::new(56.)),
        }
    }

    /// The column alone, without the switch row.
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

#[cfg(test)]
mod by_app_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{Bounds, point, px, size};
    use tl_ui_settings::layout_pages::{
        PRESET_DEFAULT, PRESET_GPU_APPS, PRESET_VRAM_APPS, gpu_page_for_preset,
    };
    use tl_ui_settings::settings::PerformanceSettings;

    use super::{GpuParts, place_gpu};
    use crate::performance::widget::container::{FlowInput, FlowItem};
    use crate::performance::widget::grid::InnerPlacement;
    use crate::performance::widget::source::Availability;

    const PAGE: &str = "page.gpu:GPU-0a1b2c3d";

    fn parts() -> GpuParts {
        GpuParts {
            switch: Rc::new(Cell::new(30.)),
            switch_at: Rc::default(),
            engines: Rc::new(Cell::new(232.)),
            engine_cells: Rc::new(Cell::new(4)),
            details: Rc::new(Cell::new(300.)),
            displays: Rc::new(Cell::new(56.)),
        }
    }

    fn placed(preset: &str, parts: &GpuParts) -> InnerPlacement {
        let l = gpu_page_for_preset(&PerformanceSettings::default(), PAGE, preset);
        let items: Vec<FlowItem<'_>> = l
            .widgets
            .iter()
            .map(|w| FlowItem {
                setting: w,
                availability: Availability::Available,
            })
            .collect();
        let input = FlowInput {
            layout: &l,
            items: &items,
            area: size(px(1000.), px(600.)),
            scale: 1.,
            frozen: None,
        };
        place_gpu(&input, parts)
    }

    /// The switch row sits on top, as wide as the body; the parts move down by its height.
    #[test]
    fn switch_row_on_top() {
        let p = parts();
        let placed = placed(PRESET_DEFAULT, &p);
        assert_eq!(
            p.switch_at.get(),
            Some(Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(30.))))
        );
        assert_eq!(placed.bounds(1).expect("engines").origin.y, px(30.));
        assert_eq!(placed.bounds(2).expect("memory").origin.y, px(30. + 240.));
    }

    /// `gpu_apps`: GPU load by app 300 px at the full width in the engine grid's place; Memory
    /// and Temperature stay side by side under it.
    #[test]
    fn gpu_apps_is_300_px() {
        let p = parts();
        let placed = placed(PRESET_GPU_APPS, &p);
        let apps = placed.bounds(1).expect("by app");
        assert_eq!(apps.origin.y, px(30.));
        assert_eq!(apps.size.height, px(300.));
        assert_eq!(apps.size.width, px(1000.));
        let (mem, temp) = (
            placed.bounds(2).expect("memory"),
            placed.bounds(3).expect("temp"),
        );
        assert_eq!(mem.origin.y, px(338.));
        assert_eq!(temp.origin.y, px(338.));
        assert_eq!(mem.size.width, px(496.));
    }

    /// `vram_apps`: VRAM by app 300 px at the full width in the Memory chart's place, and
    /// Temperature alone takes the full width.
    #[test]
    fn vram_apps_is_300_px_and_temperature_full_width() {
        let p = parts();
        let placed = placed(PRESET_VRAM_APPS, &p);
        let apps = placed.bounds(2).expect("by app");
        assert_eq!(apps.origin.y, px(30. + 240.));
        assert_eq!(apps.size.height, px(300.));
        assert_eq!(apps.size.width, px(1000.));
        let temp = placed.bounds(3).expect("temp");
        assert_eq!(temp.origin.y, px(270. + 308.));
        assert_eq!(temp.size.width, px(1000.));
    }
}
