//! The CPU page's flow placement (ADR-019 phase 3 "Presets of the pages", UI-PRF-023,
//! UI-PRF-025; `tasklens-hzire.5`).
//!
//! The 1.3.0 page in one column: the mode's part (the per-core grid as tall as its rows at the
//! page width, or a 300 px chart, or the total chart at 40% of the width beside the core board,
//! or CPU by app at 300 px with its legend at the right, UI-PRF-062), the secondary chart
//! (120 px), then Details.
//!
//! The secondary graph is a list of widgets: `power` lists Power draw and its two fallbacks,
//! Temperature and Clock speed. Of the secondary widgets only the first that the CPU can show
//! takes space ([`shown_secondary`], the rule of `cpu::effective_secondary`). When it stands in
//! for Power draw, the page puts the 1.3.0 fallback note on its heading.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{Bounds, point, px, size};
use tl_ui_settings::layout::{FixedWidget, FormSet, METRIC_TEMP, WidgetContent, WidgetSetting};
use tl_ui_settings::layout_boxes::{METRIC_CORE_CLOCKS, METRIC_CPU_TOTAL, METRIC_POWER_DRAW};
use tl_ui_settings::layout_pages::{METRIC_CLOCK, METRIC_CORES_OVERLAID, METRIC_CORES_STACKED};

use super::{PART_GAP, PagePart, PartHeight, default_part, place_column};
use crate::performance::core::cpu::{PowerState, SecondaryAvail};
use crate::performance::page::{GRAPH_H_OVERALL, SECONDARY_GRAPH_H};
use crate::performance::widget::container::{FlowInput, FlowPlace};
use crate::performance::widget::source::Availability;

/// The total chart's share of the Core board row's width, as in 1.3.0.
pub const BOARD_CHART_SHARE: f32 = 0.4;
/// The gap between the total chart and the core board (`core_gutter`, `sp6`), logical px.
pub const BOARD_GUTTER: f32 = 16.;
/// The grid's row height before it is measured (`grid_cell_h`), logical px.
const GRID_ESTIMATE: f32 = 112.;
/// The pair row of the total chart and the core board.
const ROW_BOARD: u8 = 1;

/// The measured heights and the secondary availability the CPU page's flow reads.
#[derive(Clone, Debug)]
pub struct CpuParts {
    /// The per-core grid's height at the page width, logical px (`CpuCoreGridView`).
    pub grid: Rc<Cell<f32>>,
    /// The Details widget's height (`DetailsView`).
    pub details: Rc<Cell<f32>>,
    /// What the secondary chart can show, from the page's snapshot.
    pub avail: Rc<Cell<SecondaryAvail>>,
}

/// The metric key of a secondary chart widget: Power draw, Temperature, Clock speed or Core
/// clocks with a chart. `None` for any other widget.
pub fn secondary_metric(w: &WidgetSetting) -> Option<&str> {
    let WidgetContent::Metric(source) = &w.content else {
        return None;
    };
    let key = source.metric.as_str();
    let secondary = matches!(
        key,
        METRIC_POWER_DRAW | METRIC_TEMP | METRIC_CLOCK | METRIC_CORE_CLOCKS
    );
    (secondary && w.forms.contains(FormSet::CHART)).then_some(key)
}

/// Whether the CPU can show the secondary chart of `metric` (PERF-032): power draw while RAPL
/// is readable, temperature with a CPU hwmon, the clock charts while the clock is read.
fn allowed(metric: &str, avail: SecondaryAvail) -> bool {
    match metric {
        METRIC_POWER_DRAW => avail.power == PowerState::Readable,
        METRIC_TEMP => avail.temperature,
        _ => avail.clock,
    }
}

/// The secondary widget that takes space among `widgets`, in list order: the first secondary
/// chart widget the CPU can show and `available` keeps. With it, whether it stands in for a
/// Power draw widget listed before it (the 1.3.0 fallback).
pub fn shown_secondary<'a>(
    widgets: impl IntoIterator<Item = &'a WidgetSetting>,
    avail: SecondaryAvail,
    available: impl Fn(u32) -> bool,
) -> Option<(u32, bool)> {
    let mut power_listed = false;
    for w in widgets {
        let Some(metric) = secondary_metric(w) else {
            continue;
        };
        if allowed(metric, avail) && available(w.id) {
            return Some((w.id, metric != METRIC_POWER_DRAW && power_listed));
        }
        power_listed |= metric == METRIC_POWER_DRAW;
    }
    None
}

/// The part CPU page widget `w` is in the flow, `None` when it takes no space. `secondary` is
/// the secondary widget that takes space ([`shown_secondary`]).
pub fn cpu_part(w: &WidgetSetting, parts: &CpuParts, secondary: Option<u32>) -> Option<PagePart> {
    let fixed = |h, pair| PagePart {
        height: PartHeight::Fixed(h),
        gap: PART_GAP,
        pair,
    };
    if secondary_metric(w).is_some() {
        return (secondary == Some(w.id)).then(|| fixed(SECONDARY_GRAPH_H, None));
    }
    match &w.content {
        WidgetContent::Fixed(FixedWidget::CpuCoreGrid) => Some(PagePart {
            height: PartHeight::Natural {
                measured: Rc::clone(&parts.grid),
                estimate: GRID_ESTIMATE,
            },
            gap: PART_GAP,
            pair: None,
        }),
        WidgetContent::Fixed(FixedWidget::CpuCoreBoard) => {
            Some(fixed(GRAPH_H_OVERALL, Some(ROW_BOARD)))
        }
        WidgetContent::Fixed(FixedWidget::CpuByApp) => Some(fixed(GRAPH_H_OVERALL, None)),
        WidgetContent::Metric(source) if w.forms.contains(FormSet::CHART) => {
            Some(match source.metric.as_str() {
                METRIC_CPU_TOTAL => fixed(GRAPH_H_OVERALL, Some(ROW_BOARD)),
                METRIC_CORES_OVERLAID | METRIC_CORES_STACKED => fixed(GRAPH_H_OVERALL, None),
                _ => default_part(w, &parts.details),
            })
        }
        _ => Some(default_part(w, &parts.details)),
    }
}

/// [`cpu_flow`]'s placement of `input`: the column of [`cpu_part`], with the total chart and
/// the core board side by side at [`BOARD_CHART_SHARE`] of the width and [`BOARD_GUTTER`]
/// apart, as the 1.3.0 Core board mode.
pub fn place_cpu(input: &FlowInput<'_>, parts: &CpuParts) -> super::InnerPlacement {
    let available = |id: u32| {
        input
            .items
            .iter()
            .any(|i| i.setting.id == id && i.availability == Availability::Available)
    };
    let secondary = shown_secondary(
        input.items.iter().map(|i| i.setting),
        parts.avail.get(),
        available,
    )
    .map(|(id, _)| id);
    let mut placement = place_column(input, &|w| cpu_part(w, parts, secondary));
    let id_of = |kind: fn(&WidgetSetting) -> bool| {
        input.layout.widgets.iter().find(|w| kind(w)).map(|w| w.id)
    };
    let total = id_of(|w| {
        matches!(&w.content, WidgetContent::Metric(s) if s.metric.as_str() == METRIC_CPU_TOTAL)
            && w.forms.contains(FormSet::CHART)
    });
    let board = id_of(|w| matches!(w.content, WidgetContent::Fixed(FixedWidget::CpuCoreBoard)));
    let (Some(total), Some(board)) = (total, board) else {
        return placement;
    };
    let at = |id| placement.widgets.iter().position(|p| p.id == id);
    let (Some(t), Some(b)) = (at(total), at(board)) else {
        return placement;
    };
    let (tb, bb) = (placement.widgets[t].bounds, placement.widgets[b].bounds);
    if tb.origin.y != bb.origin.y {
        return placement;
    }
    let s = if input.scale.is_finite() && input.scale > 0. {
        input.scale
    } else {
        1.
    };
    let (left, width) = (
        f32::from(tb.origin.x.min(bb.origin.x)),
        f32::from(input.area.width),
    );
    let chart_w = (width * BOARD_CHART_SHARE).round();
    let board_x = (chart_w + BOARD_GUTTER * s).round();
    placement.widgets[t].bounds = Bounds::new(
        point(px(left), tb.origin.y),
        size(px(chart_w), tb.size.height),
    );
    placement.widgets[b].bounds = Bounds::new(
        point(px(left + board_x), bb.origin.y),
        size(px((width - board_x).max(0.)), bb.size.height),
    );
    placement
}

/// The placer of the CPU page's flow layouts.
pub fn cpu_flow(parts: CpuParts) -> FlowPlace {
    Rc::new(move |input: &FlowInput<'_>| place_cpu(input, &parts))
}

#[cfg(test)]
mod tests {
    use gpui_kit::{px, size};
    use tl_ui_settings::layout::ContainerLayout;
    use tl_ui_settings::layout_pages::convert_cpu_page;
    use tl_ui_settings::settings::{CpuGraphMode, CpuSecondaryGraph, PerformanceSettings};

    use super::*;
    use crate::performance::page::{DETAILS_GAP, PAGE_PAD};
    use crate::performance::widget::container::FlowItem;
    use crate::performance::widget::grid::InnerPlacement;

    const ALL: SecondaryAvail = SecondaryAvail {
        power: PowerState::Readable,
        temperature: true,
        clock: true,
    };

    fn layout(mode: CpuGraphMode, graph: CpuSecondaryGraph) -> ContainerLayout {
        let perf = PerformanceSettings {
            cpu_graph_mode: mode,
            cpu_secondary_graph: graph,
            ..PerformanceSettings::default()
        };
        convert_cpu_page(&perf, "page.cpu")
    }

    fn parts(avail: SecondaryAvail) -> CpuParts {
        CpuParts {
            grid: Rc::new(Cell::new(232.)),
            details: Rc::new(Cell::new(300.)),
            avail: Rc::new(Cell::new(avail)),
        }
    }

    fn placed(l: &ContainerLayout, parts: &CpuParts) -> InnerPlacement {
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
        place_cpu(&input, parts)
    }

    /// The default: the grid, Power draw under it at 120 px, Details, at the 1.3.0 places.
    #[test]
    fn default_places_the_1_3_0_parts() {
        let l = layout(CpuGraphMode::PerCore, CpuSecondaryGraph::Power);
        let p = placed(&l, &parts(ALL));
        let b = |id| p.bounds(id).expect("placed");
        // Ids from 1: grid, power draw, temperature, clock, Details.
        assert_eq!(b(1).origin.y, px(0.));
        assert_eq!(b(1).size.height, px(232.));
        assert_eq!(b(2).origin.y, px(240.));
        assert_eq!(b(2).size.height, px(120.));
        assert!(p.bounds(3).is_none() && p.bounds(4).is_none());
        assert_eq!(b(5).origin.y, px(360. + DETAILS_GAP));
        assert_eq!(p.content_h, px(384. + 300. + PAGE_PAD));
    }

    /// Without RAPL the temperature takes Power draw's place; without a sensor, the clock.
    #[test]
    fn power_falls_back_to_temperature_then_clock() {
        let l = layout(CpuGraphMode::PerCore, CpuSecondaryGraph::Power);
        let no_power = SecondaryAvail {
            power: PowerState::NeedsSetup,
            ..ALL
        };
        let p = placed(&l, &parts(no_power));
        assert!(p.bounds(2).is_none());
        assert_eq!(p.bounds(3).expect("temperature").origin.y, px(240.));
        assert_eq!(
            shown_secondary(&l.widgets, no_power, |_| true),
            Some((3, true))
        );
        let neither = SecondaryAvail {
            temperature: false,
            ..no_power
        };
        let p = placed(&l, &parts(neither));
        assert_eq!(p.bounds(4).expect("clock").origin.y, px(240.));
        // Temperature chosen on its own is no fallback.
        let l = layout(CpuGraphMode::PerCore, CpuSecondaryGraph::Temperature);
        assert_eq!(
            shown_secondary(&l.widgets, no_power, |_| true),
            Some((2, false))
        );
    }

    /// Overall, all cores overlaid and stacked, and By app: one 300 px chart.
    #[test]
    fn single_chart_modes_are_300_px() {
        for mode in [
            CpuGraphMode::Overall,
            CpuGraphMode::AllCoresOverlaid,
            CpuGraphMode::AllCoresStacked,
            CpuGraphMode::Apps,
        ] {
            let l = layout(mode, CpuSecondaryGraph::None);
            let p = placed(&l, &parts(ALL));
            let chart = p.bounds(1).expect("chart");
            assert_eq!(chart.size.height, px(300.), "{mode:?}");
            assert_eq!(chart.size.width, px(1000.), "{mode:?}");
            assert_eq!(p.bounds(2).expect("details").origin.y, px(324.), "{mode:?}");
        }
    }

    /// Core board: the total chart at 40% of the width, the board beside it after the gutter.
    #[test]
    fn board_row_is_40_percent_and_the_board() {
        let l = layout(CpuGraphMode::Board, CpuSecondaryGraph::None);
        let p = placed(&l, &parts(ALL));
        let (chart, board) = (p.bounds(1).expect("total"), p.bounds(2).expect("board"));
        assert_eq!(chart.origin.y, board.origin.y);
        assert_eq!(chart.size.width, px(400.));
        assert_eq!(board.origin.x, px(416.));
        assert_eq!(board.size.width, px(584.));
        assert_eq!(chart.size.height, px(300.));
        assert_eq!(board.size.height, px(300.));
    }

    /// `none` lists no secondary widget.
    #[test]
    fn none_has_no_secondary() {
        let l = layout(CpuGraphMode::PerCore, CpuSecondaryGraph::None);
        assert_eq!(shown_secondary(&l.widgets, ALL, |_| true), None);
        let p = placed(&l, &parts(ALL));
        assert_eq!(
            p.bounds(2).expect("details").origin.y,
            px(232. + DETAILS_GAP)
        );
    }
}
