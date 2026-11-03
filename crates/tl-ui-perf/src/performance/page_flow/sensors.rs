//! The Fan and Battery pages' flow placement (ADR-019 phase 3 "Presets of the pages",
//! UI-PRF-034 and UI-PRF-035; `tasklens-hzire.9`).
//!
//! The Fan page is the shared column: the Fan speed and Temperature charts at 150 px, then
//! Details `details_gap` under them.

use std::cell::Cell;
use std::rc::Rc;

use tl_ui_settings::layout::WidgetSetting;

use super::{column_flow, default_part};
use crate::performance::widget::container::FlowPlace;

/// The placer of the Fan page's flow layouts: every part as the pages share it
/// ([`default_part`]). `details` is the Details view's measured height.
pub fn fan_flow(details: Rc<Cell<f32>>) -> FlowPlace {
    column_flow(Rc::new(move |w: &WidgetSetting| {
        Some(default_part(w, &details))
    }))
}

#[cfg(test)]
mod tests {
    use gpui_kit::{px, size};
    use tl_ui_settings::layout::ContainerLayout;
    use tl_ui_settings::layout_pages::default_page_layout;

    use super::*;
    use crate::performance::page::PAGE_PAD;
    use crate::performance::widget::container::{FlowInput, FlowItem};
    use crate::performance::widget::source::Availability;

    const FAN: &str = "page.fan:nct6798:1";

    fn placed(l: &ContainerLayout, gone: &[u32], scale: f32) -> Vec<(u32, f32, f32)> {
        let details = Rc::new(Cell::new(200.));
        let place = fan_flow(details);
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
        let p = place(&input);
        let mut out: Vec<_> = p
            .widgets
            .iter()
            .map(|w| {
                (
                    w.id,
                    f32::from(w.bounds.origin.y),
                    f32::from(w.bounds.size.height),
                )
            })
            .collect();
        out.push((0, f32::from(p.content_h), 0.));
        out
    }

    /// The default: Fan speed and Temperature at 150 px, `sp4` apart, then Details at
    /// `details_gap` (UI-PRF-034), at 1.0, 1.5 and 2.0 scale.
    #[test]
    fn default_places_the_1_3_0_parts() {
        let l = default_page_layout(FAN).expect("a fan page default");
        for scale in [1.0, 1.5, 2.0] {
            let p = placed(&l, &[], scale);
            let r = |v: f32| (v * scale).round();
            // Ids from 1 in list order: speed, temperature, Details.
            assert_eq!(p[0], (1, 0., r(150.)));
            assert_eq!(p[1], (2, r(158.), r(150.)));
            assert_eq!(p[2], (3, r(158. + 150. + 24.), r(200.)));
            assert_eq!(p[3].1, ((332. + 200. + PAGE_PAD) * scale).round());
        }
    }

    /// A chart the fan does not report takes no space and the parts under it move up.
    #[test]
    fn a_part_that_takes_no_space_closes_up() {
        let l = default_page_layout(FAN).expect("a fan page default");
        let p = placed(&l, &[2], 1.);
        assert_eq!(p[0], (1, 0., 150.));
        assert_eq!(p[1], (3, 150. + 24., 200.));
    }
}
