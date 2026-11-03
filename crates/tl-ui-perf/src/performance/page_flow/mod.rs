//! The flow placement of a device page (ADR-019 phase 3 "Presets of the pages", UI-PRF-020).
//!
//! A page's flow layout draws each widget at its 1.3.0 place and height: one column under the
//! page header, the parts `sp4` apart, `details_gap` above the Details widget and `sp6` above
//! each block, and `page_pad` under the last part. Two parts marked as a pair sit side by side,
//! each half the width less half a gap. A widget that takes no space (a metric the device does
//! not report, a block the page hides) is left out, and the parts under it move up.
//!
//! The placer draws only what is not a widget: the gaps. Each page says how tall each of its
//! parts is ([`PagePart`]); the Details widget and the blocks are as tall as their content,
//! which their views measure ([`PartHeight::Natural`]). The page's own module (`page_flow::gpu`
//! and the others, added by the page beads) gives its heights; [`default_part`] is the rule
//! every page shares.
//!
//! Pure functions over the flow input: no drawing, no state.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{Bounds, point, px, size};
use tl_ui_settings::layout::{FixedWidget, FormSet, WidgetContent, WidgetSetting};

use super::page::{DETAILS_GAP, GRAPH_H_STACKED, PAGE_PAD};
use super::widget::compose::ROW_H;
use super::widget::container::{FlowInput, FlowPlace};
use super::widget::grid::{InnerGrid, InnerPlacement, PlacedWidget};
use super::widget::source::Availability;

pub mod cpu;
pub mod disk;
pub mod gpu;
pub mod memory;
pub mod net;
pub mod sensors;

/// The gap between two page parts (`sp4`), logical px.
pub const PART_GAP: f32 = 8.;
/// The gap above a page block (`sp6`), logical px.
pub const BLOCK_GAP: f32 = 16.;
/// The height of the "Details" heading row and the gap under it (`DETAILS_HEADING_H` and
/// `sp4`), logical px: the Details widget's height before its grid is measured.
const DETAILS_HEAD: f32 = 24. + PART_GAP;

/// How tall a page part is.
#[derive(Clone, Debug)]
pub enum PartHeight {
    /// This many logical px (a chart).
    Fixed(f32),
    /// As tall as its content, which its view measures into the cell (logical px); `estimate`
    /// until the first measure.
    Natural {
        measured: Rc<Cell<f32>>,
        estimate: f32,
    },
}

impl PartHeight {
    /// The height now, logical px.
    pub fn get(&self) -> f32 {
        match self {
            PartHeight::Fixed(h) => *h,
            PartHeight::Natural { measured, estimate } => {
                let h = measured.get();
                if h > 0. { h } else { *estimate }
            }
        }
    }
}

/// One part of a page column.
#[derive(Clone, Debug)]
pub struct PagePart {
    pub height: PartHeight,
    /// The gap above the part, logical px. The first placed part has none.
    pub gap: f32,
    /// `Some(row)`: the part sits beside the next part of the same pair row (the GPU page's
    /// Memory and Temperature charts are row 1, Power and Clocks row 2). One part of a row
    /// alone takes the full width.
    pub pair: Option<u8>,
}

/// The part a page widget is: `None` when it takes no space.
pub type PartOf = Rc<dyn Fn(&WidgetSetting) -> Option<PagePart>>;

/// The rule every page shares: a chart is a 150 px part, a widget without a chart one row, a
/// group widget a 150 px part, Details as tall as its grid (`details`, its view's measure) with
/// `details_gap` above, and any other fixed widget as tall as its content would be measured,
/// estimated at its minimum, with `sp6` above.
pub fn default_part(w: &WidgetSetting, details: &Rc<Cell<f32>>) -> PagePart {
    let chart = PagePart {
        height: PartHeight::Fixed(GRAPH_H_STACKED),
        gap: PART_GAP,
        pair: None,
    };
    match &w.content {
        WidgetContent::Metric(_) if w.forms.contains(FormSet::CHART) => chart,
        WidgetContent::Metric(_) => PagePart {
            height: PartHeight::Fixed(ROW_H),
            ..chart
        },
        WidgetContent::Group(_) => chart,
        WidgetContent::Fixed(FixedWidget::Details) => PagePart {
            height: PartHeight::Natural {
                measured: Rc::clone(details),
                estimate: DETAILS_HEAD,
            },
            gap: DETAILS_GAP,
            pair: None,
        },
        WidgetContent::Fixed(kind) => PagePart {
            height: PartHeight::Fixed(super::widget::compose::fixed_min(kind).height),
            gap: BLOCK_GAP,
            pair: None,
        },
    }
}

/// The placer of a page's flow layout: the available widgets in list order, each the part
/// `part_of` gives, in one column.
pub fn column_flow(part_of: PartOf) -> FlowPlace {
    Rc::new(move |input: &FlowInput<'_>| place_column(input, &*part_of))
}

/// [`column_flow`]'s placement of `input`.
pub fn place_column(
    input: &FlowInput<'_>,
    part_of: &dyn Fn(&WidgetSetting) -> Option<PagePart>,
) -> InnerPlacement {
    let s = if input.scale.is_finite() && input.scale > 0. {
        input.scale
    } else {
        1.
    };
    let width = f32::from(input.area.width);
    let parts: Vec<(u32, PagePart)> = input
        .items
        .iter()
        .filter(|i| i.availability == Availability::Available)
        .filter_map(|i| part_of(i.setting).map(|p| (i.setting.id, p)))
        .collect();
    let mut widgets = Vec::with_capacity(parts.len());
    let mut y = 0.;
    let mut i = 0;
    let rect = |x: f32, y: f32, w: f32, h: f32| {
        let (left, top) = (x.round(), y.round());
        Bounds::new(
            point(px(left), px(top)),
            size(px((x + w).round() - left), px((y + h).round() - top)),
        )
    };
    while i < parts.len() {
        let (id, part) = &parts[i];
        if i > 0 {
            y += part.gap * s;
        }
        let pair =
            part.pair.is_some() && parts.get(i + 1).is_some_and(|(_, p)| p.pair == part.pair);
        if pair {
            let (id2, part2) = &parts[i + 1];
            let gap = PART_GAP * s;
            let half = ((width - gap) / 2.).max(0.);
            let h = part.height.get().max(part2.height.get()) * s;
            widgets.push(PlacedWidget {
                id: *id,
                bounds: rect(0., y, half, h),
            });
            widgets.push(PlacedWidget {
                id: *id2,
                bounds: rect(half + gap, y, width - half - gap, h),
            });
            y += h;
            i += 2;
        } else {
            let h = part.height.get() * s;
            widgets.push(PlacedWidget {
                id: *id,
                bounds: rect(0., y, width, h),
            });
            y += h;
            i += 1;
        }
    }
    // The page's bottom padding scrolls with the parts, as in 1.3.0.
    let content = if widgets.is_empty() {
        0.
    } else {
        y + PAGE_PAD * s
    };
    let mut grid = InnerGrid::of(&input.layout.container, width, s);
    grid.row = grid.unit;
    InnerPlacement {
        widgets,
        content_h: px(content),
        scrolls: content > f32::from(input.area.height) + 0.5,
        grid,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::size;
    use tl_ui_settings::layout::{
        ContainerLayout, DeviceSelector, MetricKey, PresetSetting, SourceSelector, WidgetOptions,
    };

    use super::*;
    use crate::performance::widget::container::FlowItem;

    fn metric(id: u32, forms: FormSet) -> WidgetSetting {
        WidgetSetting {
            id,
            content: WidgetContent::Metric(SourceSelector {
                device: DeviceSelector::Key(Arc::from("aggregate:disk")),
                metric: MetricKey::new("active"),
            }),
            forms,
            cell: None,
            options: WidgetOptions::default(),
            device_name: None,
        }
    }

    fn fixed(id: u32, kind: FixedWidget) -> WidgetSetting {
        WidgetSetting {
            content: WidgetContent::Fixed(kind),
            ..metric(id, FormSet::empty())
        }
    }

    fn layout(widgets: Vec<WidgetSetting>) -> ContainerLayout {
        ContainerLayout {
            container: "page.aggregate:disk".to_owned(),
            version: tl_ui_settings::layout::LAYOUT_VERSION,
            preset: Some(PresetSetting::default()),
            next_id: widgets.len() as u32 + 1,
            widgets,
        }
    }

    fn placed(
        l: &ContainerLayout,
        gone: &[u32],
        scale: f32,
        part_of: &dyn Fn(&WidgetSetting) -> Option<PagePart>,
    ) -> InnerPlacement {
        let items: Vec<FlowItem<'_>> = l
            .widgets
            .iter()
            .map(|w| FlowItem {
                setting: w,
                availability: if gone.contains(&w.id) {
                    Availability::NotReported
                } else {
                    Availability::Available
                },
            })
            .collect();
        let input = FlowInput {
            layout: l,
            items: &items,
            area: size(px(1000. * scale), px(600. * scale)),
            scale,
            frozen: None,
        };
        place_column(&input, part_of)
    }

    #[test]
    fn charts_then_details_at_the_1_3_0_places() {
        let all = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
        let l = layout(vec![
            metric(1, all),
            metric(2, all),
            fixed(3, FixedWidget::Details),
        ]);
        let details = Rc::new(Cell::new(0.));
        for scale in [1.0, 1.5, 2.0] {
            details.set(200.);
            let part = |w: &WidgetSetting| Some(default_part(w, &details));
            let p = placed(&l, &[], scale, &part);
            let b = |id| p.bounds(id).expect("placed");
            let r = |v: f32| px((v * scale).round());
            assert_eq!(b(1).origin.y, px(0.));
            assert_eq!(b(1).size.height, r(150.));
            assert_eq!(b(2).origin.y, r(158.));
            assert_eq!(b(3).origin.y, r(158. + 150. + 24.));
            assert_eq!(b(3).size.height, r(200.));
            assert_eq!(b(1).size.width, px(1000. * scale));
            assert_eq!(p.content_h, px((332. + 200. + PAGE_PAD) * scale));
        }
    }

    #[test]
    fn a_part_that_takes_no_space_closes_up() {
        let all = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
        let l = layout(vec![metric(1, all), metric(2, all)]);
        let details = Rc::new(Cell::new(0.));
        let part = |w: &WidgetSetting| Some(default_part(w, &details));
        let p = placed(&l, &[1], 1., &part);
        assert_eq!(p.widgets.len(), 1);
        assert_eq!(p.bounds(2).expect("placed").origin.y, px(0.));
    }

    #[test]
    fn a_pair_sits_side_by_side() {
        let all = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
        let l = layout(vec![metric(1, all), metric(2, all), metric(3, all)]);
        let part = |w: &WidgetSetting| {
            Some(PagePart {
                height: PartHeight::Fixed(150.),
                gap: PART_GAP,
                pair: (w.id != 3).then_some(1),
            })
        };
        let p = placed(&l, &[], 1., &part);
        let (a, b, c) = (
            p.bounds(1).expect("1"),
            p.bounds(2).expect("2"),
            p.bounds(3).expect("3"),
        );
        assert_eq!(a.origin.y, b.origin.y);
        assert_eq!(a.size.width, px(496.));
        assert_eq!(b.origin.x, px(504.));
        assert_eq!(c.origin.y, px(158.));
        // One of a pair alone takes the full width.
        let p = placed(&l, &[2], 1., &part);
        assert_eq!(p.bounds(1).expect("1").size.width, px(1000.));
    }
}
