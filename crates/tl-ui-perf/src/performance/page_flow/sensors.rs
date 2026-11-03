//! The Fan and Battery pages' flow placement (ADR-019 phase 3 "Presets of the pages",
//! UI-PRF-034 and UI-PRF-035; `tasklens-hzire.9`).
//!
//! The Fan page is the shared column: the Fan speed and Temperature charts at 150 px, then
//! Details `details_gap` under them.
//!
//! The Battery page is the same column with three rules of 1.3.0: the Power chart takes space
//! on a system battery only and the Percentage chart on any other device only, and the charge
//! history takes space once the battery has one (2 points), `sp4` under the chart, as tall as
//! its chart and day labels.

use std::cell::Cell;
use std::rc::Rc;

use tl_ui_settings::layout::{FixedWidget, METRIC_POWER, WidgetContent, WidgetSetting};
use tl_ui_settings::layout_pages::METRIC_PERCENT;

use super::{PART_GAP, PagePart, PartHeight, column_flow, default_part};
use crate::performance::widget::container::FlowPlace;

/// The height of the history chart (UI-PRF-035), logical px.
const HISTORY_H: f32 = 150.;
/// The height of the day labels under it before the view is measured: one `t_xs` line.
const DAYS_H: f32 = 16.;

/// The placer of the Fan page's flow layouts: every part as the pages share it
/// ([`default_part`]). `details` is the Details view's measured height.
pub fn fan_flow(details: Rc<Cell<f32>>) -> FlowPlace {
    column_flow(Rc::new(move |w: &WidgetSetting| {
        Some(default_part(w, &details))
    }))
}

/// The measured heights and the shown flags the Battery page's flow reads.
#[derive(Clone, Debug, Default)]
pub struct BatteryParts {
    /// The Details widget's height (`DetailsView`).
    pub details: Rc<Cell<f32>>,
    /// The history view's height (its part view).
    pub history: Rc<Cell<f32>>,
    /// The battery has a history to show (a system battery with at least 2 points).
    pub history_shown: Rc<Cell<bool>>,
    /// The battery powers the system: the page shows power, not percentage.
    pub system: Rc<Cell<bool>>,
}

/// The part Battery page widget `w` is in the flow, `None` when it takes no space.
pub fn battery_part(w: &WidgetSetting, parts: &BatteryParts) -> Option<PagePart> {
    match &w.content {
        WidgetContent::Metric(source) if source.metric.as_str() == METRIC_POWER => {
            parts.system.get().then(|| default_part(w, &parts.details))
        }
        WidgetContent::Metric(source) if source.metric.as_str() == METRIC_PERCENT => {
            (!parts.system.get()).then(|| default_part(w, &parts.details))
        }
        WidgetContent::Fixed(FixedWidget::BatteryHistory) => {
            parts.history_shown.get().then(|| PagePart {
                height: PartHeight::Natural {
                    measured: Rc::clone(&parts.history),
                    estimate: HISTORY_H + DAYS_H,
                },
                gap: PART_GAP,
                pair: None,
            })
        }
        _ => Some(default_part(w, &parts.details)),
    }
}

/// The placer of the Battery page's flow layouts.
pub fn battery_flow(parts: BatteryParts) -> FlowPlace {
    column_flow(Rc::new(move |w: &WidgetSetting| battery_part(w, &parts)))
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

    const BAT: &str = "page.bat:BAT0";

    fn battery(system: bool, history: bool) -> BatteryParts {
        BatteryParts {
            details: Rc::new(Cell::new(300.)),
            history: Rc::new(Cell::new(170.)),
            history_shown: Rc::new(Cell::new(history)),
            system: Rc::new(Cell::new(system)),
        }
    }

    fn placed_battery(parts: &BatteryParts, scale: f32) -> Vec<(u32, f32, f32)> {
        let l = default_page_layout(BAT).expect("a battery page default");
        let place = battery_flow(parts.clone());
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

    /// A system battery with a history: Power (150), the history `sp4` under it, Details
    /// `details_gap` under that, at 1.0, 1.5 and 2.0 scale. Percentage takes no space.
    #[test]
    fn system_battery_places_power_history_details() {
        for scale in [1.0, 1.5, 2.0] {
            let p = placed_battery(&battery(true, true), scale);
            let r = |v: f32| (v * scale).round();
            // Ids from 1 in list order: power, percent, history, Details.
            assert_eq!(p[0], (1, 0., r(150.)));
            assert_eq!(p[1], (3, r(158.), r(170.)));
            assert_eq!(p[2], (4, r(158. + 170. + 24.), r(300.)));
            assert_eq!(p[3].1, ((352. + 300. + PAGE_PAD) * scale).round());
        }
    }

    /// Any other battery: Percentage and Details; no history.
    #[test]
    fn peripheral_places_percentage_and_details() {
        let p = placed_battery(&battery(false, false), 1.);
        assert_eq!(p[0], (2, 0., 150.));
        assert_eq!(p[1], (4, 150. + 24., 300.));
    }

    /// A system battery with fewer than 2 history points: the history takes no space.
    #[test]
    fn system_battery_without_history() {
        let p = placed_battery(&battery(true, false), 1.);
        assert_eq!(p[0], (1, 0., 150.));
        assert_eq!(p[1], (4, 150. + 24., 300.));
    }
}
