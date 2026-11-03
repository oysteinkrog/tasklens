//! The Disk page's flow placement (ADR-019 phase 3 "Presets of the pages", UI-PRF-029,
//! UI-PRF-030; `tasklens-hzire.7`).
//!
//! The 1.3.0 page in one column: the Active time and Transfer rate charts (150 px each, `sp4`
//! apart), Details `details_gap` under them, then the partitions block `sp6` under Details. The
//! block takes no space until the disk's partition table is read, as 1.3.0 hides it then.

use std::cell::Cell;
use std::rc::Rc;

use tl_ui_settings::layout::{FixedWidget, WidgetContent, WidgetSetting};

use super::{BLOCK_GAP, PagePart, PartHeight, column_flow, default_part};
use crate::performance::widget::compose::fixed_min;
use crate::performance::widget::container::FlowPlace;

/// The measured heights and the shown flag the Disk page's flow reads.
#[derive(Clone, Debug, Default)]
pub struct DiskParts {
    /// The Details widget's height (`DetailsView`).
    pub details: Rc<Cell<f32>>,
    /// The partitions block's height (its part view).
    pub partitions: Rc<Cell<f32>>,
    /// The partition table is read: the block takes space.
    pub partitions_shown: Rc<Cell<bool>>,
}

/// The part Disk page widget `w` is in the flow, `None` when it takes no space.
pub fn disk_part(w: &WidgetSetting, parts: &DiskParts) -> Option<PagePart> {
    match &w.content {
        WidgetContent::Fixed(kind @ FixedWidget::DiskPartitions) => {
            parts.partitions_shown.get().then(|| PagePart {
                height: PartHeight::Natural {
                    measured: Rc::clone(&parts.partitions),
                    estimate: fixed_min(kind).height,
                },
                gap: BLOCK_GAP,
                pair: None,
            })
        }
        _ => Some(default_part(w, &parts.details)),
    }
}

/// The placer of the Disk page's flow layouts.
pub fn disk_flow(parts: DiskParts) -> FlowPlace {
    column_flow(Rc::new(move |w: &WidgetSetting| disk_part(w, &parts)))
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

    const PAGE: &str = "page.disk:eui.0025388b";

    fn layout() -> ContainerLayout {
        default_page_layout(PAGE).expect("a disk page default")
    }

    fn parts(shown: bool) -> DiskParts {
        DiskParts {
            details: Rc::new(Cell::new(300.)),
            partitions: Rc::new(Cell::new(120.)),
            partitions_shown: Rc::new(Cell::new(shown)),
        }
    }

    fn placed(l: &ContainerLayout, parts: &DiskParts, scale: f32) -> InnerPlacement {
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
        place_column(&input, &|w| disk_part(w, parts))
    }

    /// The default: Active time, Transfer rate, Details and the partitions block at the 1.3.0
    /// places (UI-PRF-029, UI-PRF-030), at 1.0, 1.5 and 2.0 scale.
    #[test]
    fn default_places_the_1_3_0_parts() {
        let l = layout();
        let p = parts(true);
        for scale in [1.0, 1.5, 2.0] {
            let placed = placed(&l, &p, scale);
            let b = |id| placed.bounds(id).expect("placed");
            let r = |v: f32| px((v * scale).round());
            // Ids from 1 in list order: active, io, Details, partitions.
            assert_eq!(b(1).origin.y, px(0.));
            assert_eq!(b(1).size.height, r(150.));
            assert_eq!(b(1).size.width, px(1000. * scale));
            assert_eq!(b(2).origin.y, r(158.));
            assert_eq!(b(2).size.height, r(150.));
            assert_eq!(b(3).origin.y, r(158. + 150. + 24.));
            assert_eq!(b(3).size.height, r(300.));
            assert_eq!(b(4).origin.y, r(332. + 300. + 16.));
            assert_eq!(b(4).size.height, r(120.));
            assert_eq!(placed.content_h, px((648. + 120. + PAGE_PAD) * scale));
        }
    }

    /// Before the partition table is read the block takes no space and nothing follows
    /// Details.
    #[test]
    fn unread_partitions_take_no_space() {
        let l = layout();
        let placed = placed(&l, &parts(false), 1.);
        assert!(placed.bounds(4).is_none());
        assert_eq!(placed.widgets.len(), 3);
        assert_eq!(placed.content_h, px(632. + PAGE_PAD));
    }
}
