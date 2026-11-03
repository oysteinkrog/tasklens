//! The Memory page's flow placement (ADR-019 phase 3 "Presets of the pages", UI-PRF-026 to
//! UI-PRF-028, UI-PRF-060; `tasklens-hzire.6`).
//!
//! The 1.3.0 page in one column: the Usage and By app switch at the top right, then the first
//! part (the Physical memory chart, 150 px, or Memory by app, 300 px), the Commit chart, the
//! Swap chart, the composition bar `sp6` under the charts, Details, then the Compressed memory,
//! Swap devices and Memory modules blocks.
//!
//! The switch is not a widget: the placer keeps its height above the first part and says where
//! it goes ([`MemoryParts::switch_at`]), and the container draws the page's switch view there.
//! A `swap` widget takes no space while the system has no swap (SwapTotal 0), and a block takes
//! none while 1.3.0 hides it (no zram and zswap off, no swap device, no module data).

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{Bounds, Pixels, point, px, size};
use tl_ui_settings::layout::{FixedWidget, FormSet, WidgetContent, WidgetSetting};
use tl_ui_settings::layout_boxes::METRIC_MEM_SWAP;

use super::{BLOCK_GAP, PART_GAP, PagePart, PartHeight, default_part, place_column};
use crate::performance::page::{GRAPH_H_OVERALL, GRAPH_H_STACKED};
use crate::performance::widget::compose::fixed_min;
use crate::performance::widget::container::{FlowInput, FlowPlace};
use crate::performance::widget::grid::InnerPlacement;

/// The switch's height before it is measured, logical px.
const SWITCH_ESTIMATE: f32 = 28.;

/// The measured heights and the shown flags the Memory page's flow reads, and where the flow
/// puts the switch.
#[derive(Clone, Debug, Default)]
pub struct MemoryParts {
    /// The Usage and By app switch's height, logical px (its `MemoryPartView`).
    pub switch: Rc<Cell<f32>>,
    /// Where the last placement put the switch, body px; the container's flow decoration.
    pub switch_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The system has swap: a `swap` widget takes space.
    pub swap: Rc<Cell<bool>>,
    /// The composition bar's height with its caption (its `MemoryPartView`).
    pub bar: Rc<Cell<f32>>,
    /// The Details widget's height (`DetailsView`).
    pub details: Rc<Cell<f32>>,
    /// The blocks' heights, in [`BLOCKS`] order.
    pub blocks: [Rc<Cell<f32>>; 3],
    /// Whether each block has something to show, in [`BLOCKS`] order.
    pub shown: [Rc<Cell<bool>>; 3],
}

/// The page's blocks, in 1.3.0 order.
pub const BLOCKS: [FixedWidget; 3] = [
    FixedWidget::MemoryCompressed,
    FixedWidget::SwapDevices,
    FixedWidget::MemoryModules,
];

/// The index of `kind` in [`BLOCKS`].
pub fn block_index(kind: &FixedWidget) -> Option<usize> {
    BLOCKS.iter().position(|k| k == kind)
}

/// Whether `w` is a `swap` widget of memory.
fn is_swap(w: &WidgetSetting) -> bool {
    matches!(&w.content, WidgetContent::Metric(s) if s.metric.as_str() == METRIC_MEM_SWAP)
}

/// The part Memory page widget `w` is in the flow, `None` when it takes no space.
pub fn memory_part(w: &WidgetSetting, parts: &MemoryParts) -> Option<PagePart> {
    let natural = |measured: &Rc<Cell<f32>>, kind: &FixedWidget| PagePart {
        height: PartHeight::Natural {
            measured: Rc::clone(measured),
            estimate: fixed_min(kind).height,
        },
        gap: BLOCK_GAP,
        pair: None,
    };
    if is_swap(w) && !parts.swap.get() {
        return None;
    }
    match &w.content {
        WidgetContent::Fixed(FixedWidget::MemoryByApp) => Some(PagePart {
            height: PartHeight::Fixed(GRAPH_H_OVERALL),
            gap: PART_GAP,
            pair: None,
        }),
        // `sp6` above the bar, as the 1.3.0 graph area.
        WidgetContent::Fixed(kind @ FixedWidget::MemoryBar) => Some(natural(&parts.bar, kind)),
        WidgetContent::Fixed(kind) => match block_index(kind) {
            Some(i) => parts.shown[i]
                .get()
                .then(|| natural(&parts.blocks[i], kind)),
            None => Some(default_part(w, &parts.details)),
        },
        WidgetContent::Metric(_) if w.forms.contains(FormSet::CHART) => Some(PagePart {
            height: PartHeight::Fixed(GRAPH_H_STACKED),
            gap: PART_GAP,
            pair: None,
        }),
        _ => Some(default_part(w, &parts.details)),
    }
}

/// [`memory_flow`]'s placement of `input`: the column of [`memory_part`] under the switch's
/// row, and the switch at the top of the body, as wide as it.
pub fn place_memory(input: &FlowInput<'_>, parts: &MemoryParts) -> InnerPlacement {
    let mut placement = place_column(input, &|w| memory_part(w, parts));
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

/// The placer of the Memory page's flow layouts.
pub fn memory_flow(parts: MemoryParts) -> FlowPlace {
    Rc::new(move |input: &FlowInput<'_>| place_memory(input, &parts))
}

#[cfg(test)]
mod tests {
    use gpui_kit::{px, size};
    use tl_ui_settings::layout::ContainerLayout;
    use tl_ui_settings::layout_pages::convert_memory_page;
    use tl_ui_settings::settings::{MemoryView, PerformanceSettings};

    use super::*;
    use crate::performance::page::{DETAILS_GAP, PAGE_PAD};
    use crate::performance::widget::container::FlowItem;
    use crate::performance::widget::source::Availability;

    fn layout(view: MemoryView, swap: bool, bar: bool) -> ContainerLayout {
        let perf = PerformanceSettings {
            memory_view: view,
            memory_swap_graph: swap,
            memory_composition: bar,
            ..PerformanceSettings::default()
        };
        convert_memory_page(&perf, "page.memory")
    }

    fn parts(swap: bool, shown: [bool; 3]) -> MemoryParts {
        let cell = |v: f32| Rc::new(Cell::new(v));
        MemoryParts {
            switch: cell(30.),
            switch_at: Rc::default(),
            swap: Rc::new(Cell::new(swap)),
            bar: cell(44.),
            details: cell(300.),
            blocks: [cell(100.), cell(60.), cell(80.)],
            shown: shown.map(|on| Rc::new(Cell::new(on))),
        }
    }

    fn placed(l: &ContainerLayout, parts: &MemoryParts) -> InnerPlacement {
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
        place_memory(&input, parts)
    }

    /// The default: the switch, Physical memory, Commit and Swap 150 px `sp4` apart, the bar
    /// `sp6` under them, Details, then the three blocks `sp6` apart, at the 1.3.0 places.
    #[test]
    fn default_places_the_1_3_0_parts() {
        let l = layout(MemoryView::Usage, true, true);
        let p = parts(true, [true; 3]);
        let placed = placed(&l, &p);
        let b = |id| placed.bounds(id).expect("placed");
        // Ids from 1: physical, commit, swap, bar, Details, compressed, swap devices, modules.
        assert_eq!(
            p.switch_at.get(),
            Some(Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(30.))))
        );
        assert_eq!(b(1).origin.y, px(30.));
        assert_eq!(b(1).size.height, px(150.));
        assert_eq!(b(2).origin.y, px(188.));
        assert_eq!(b(3).origin.y, px(346.));
        assert_eq!(b(4).origin.y, px(346. + 150. + 16.));
        assert_eq!(b(4).size.height, px(44.));
        assert_eq!(b(5).origin.y, px(556. + DETAILS_GAP));
        assert_eq!(b(6).origin.y, px(580. + 300. + 16.));
        assert_eq!(b(7).origin.y, px(896. + 100. + 16.));
        assert_eq!(b(8).origin.y, px(1012. + 60. + 16.));
        assert_eq!(placed.content_h, px(1088. + 80. + PAGE_PAD));
        assert!(placed.scrolls);
    }

    /// No swap: the Swap chart and the Swap devices block take no space; the parts under them
    /// close up. A block 1.3.0 hides takes none either.
    #[test]
    fn swap_and_hidden_blocks_take_no_space() {
        let l = layout(MemoryView::Usage, true, true);
        let p = parts(false, [false, false, true]);
        let placed = placed(&l, &p);
        assert!(placed.bounds(3).is_none(), "no swap chart");
        assert_eq!(
            placed.bounds(4).expect("bar").origin.y,
            px(30. + 158. + 150. + 16.)
        );
        assert!(placed.bounds(6).is_none() && placed.bounds(7).is_none());
        let details = placed.bounds(5).expect("details");
        assert_eq!(
            placed.bounds(8).expect("modules").origin.y,
            details.origin.y + px(300. + 16.)
        );
    }

    /// By app: Memory by app 300 px in Physical memory's place.
    #[test]
    fn by_app_is_300_px() {
        let l = layout(MemoryView::Apps, false, false);
        let p = parts(true, [false; 3]);
        let placed = placed(&l, &p);
        let apps = placed.bounds(1).expect("by app");
        assert_eq!(apps.origin.y, px(30.));
        assert_eq!(apps.size.height, px(300.));
        assert_eq!(apps.size.width, px(1000.));
        assert_eq!(placed.bounds(2).expect("commit").origin.y, px(338.));
        assert_eq!(
            placed.bounds(3).expect("details").origin.y,
            px(338. + 150. + DETAILS_GAP)
        );
    }
}
