//! Shared text styles: numeric runs with tabular figures and proportional label runs
//! (UI-FND-051, UI-FND-003, ADR-009 "Text caching").
//!
//! Rules for numbers in tables and panels (UI-FND-051 rules 1 to 5):
//! 1. Every numeric run uses [`numeric_style`], so digits have equal widths (`tnum`).
//! 2. Numeric columns are right-aligned.
//! 3. Numbers are never ellipsized. A number wider than its cell is clipped at the cell edge.
//! 4. A live count inside a label, such as "Threads [17]", is split into two runs: the label in
//!    [`label_style`] and the number in [`numeric_style`].
//! 5. Minimum numeric column widths come from the template widths in
//!    [`super::metrics::TextMetricsCache`], never from the current values.
//!
//! Sizes come from [`UiMetrics`], which already includes the UI scale. Nothing here multiplies by
//! `ui_scale`.

use std::sync::{Arc, LazyLock};

use gpui_kit::{FontFeatures, FontWeight, Pixels, TextStyleRefinement, px};

use super::fonts::UI_FAMILY;
use crate::theme::metrics::UiMetrics;

/// Line height as a multiple of the font size: 17 px (16.9) for 13 px text.
pub const LINE_HEIGHT_RATIO: f32 = 1.3;

/// The `tnum` feature set, built once. Callers clone it, which clones the `Arc` only.
pub static TNUM: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));

/// Noto Sans 400 with tabular figures, for every numeric run.
pub fn numeric_style(m: &UiMetrics, size: Pixels) -> TextStyleRefinement {
    TextStyleRefinement {
        font_family: Some(UI_FAMILY.into()),
        font_features: Some(TNUM.clone()),
        font_size: Some(size.into()),
        font_weight: Some(FontWeight::NORMAL),
        line_height: Some(px(f32::from(size) * m.line_height).into()),
        ..Default::default()
    }
}

/// Noto Sans with proportional figures (no `tnum`), for labels.
pub fn label_style(m: &UiMetrics, size: Pixels, weight: FontWeight) -> TextStyleRefinement {
    TextStyleRefinement {
        font_family: Some(UI_FAMILY.into()),
        font_size: Some(size.into()),
        font_weight: Some(weight),
        line_height: Some(px(f32::from(size) * m.line_height).into()),
        ..Default::default()
    }
}

/// 11 px × ui_scale, weight 400.
pub fn t_xs(m: &UiMetrics) -> TextStyleRefinement {
    label_style(m, m.text.xs, FontWeight::NORMAL)
}

/// 12 px × ui_scale, weight 400.
pub fn t_sm(m: &UiMetrics) -> TextStyleRefinement {
    label_style(m, m.text.sm, FontWeight::NORMAL)
}

/// 13 px × ui_scale, weight 400.
pub fn t_base(m: &UiMetrics) -> TextStyleRefinement {
    label_style(m, m.text.base, FontWeight::NORMAL)
}

/// 14 px × ui_scale, weight 500.
pub fn t_md(m: &UiMetrics) -> TextStyleRefinement {
    label_style(m, m.text.md, FontWeight::MEDIUM)
}

/// 16 px × ui_scale, weight 600.
pub fn t_lg(m: &UiMetrics) -> TextStyleRefinement {
    label_style(m, m.text.lg, FontWeight::SEMIBOLD)
}

/// 20 px × ui_scale, weight 600.
pub fn t_xl(m: &UiMetrics) -> TextStyleRefinement {
    label_style(m, m.text.xl, FontWeight::SEMIBOLD)
}

#[cfg(test)]
mod tests {
    use gpui_kit::{AbsoluteLength, DefiniteLength};
    use tl_model::settings::AppearanceSettings;

    use super::*;
    use crate::theme::metrics::compute;

    fn m100() -> UiMetrics {
        compute(&AppearanceSettings::default(), 0)
    }

    #[test]
    fn tnum_static_is_shared() {
        let m = m100();
        let a = numeric_style(&m, m.text.base).font_features.unwrap();
        let b = numeric_style(&m, m.text.sm).font_features.unwrap();
        assert!(Arc::ptr_eq(&a.0, &b.0));
        assert!(Arc::ptr_eq(&a.0, &TNUM.0));
        assert_eq!(a.0.as_slice(), &[("tnum".to_owned(), 1)]);
    }

    #[test]
    fn label_has_no_tnum_and_line_height_ratio() {
        let m = m100();
        let s = t_base(&m);
        assert!(s.font_features.is_none());
        assert_eq!(s.font_family.as_deref(), Some(UI_FAMILY));
        assert_eq!(s.font_size, Some(AbsoluteLength::Pixels(px(13.0))));
        let Some(DefiniteLength::Absolute(AbsoluteLength::Pixels(lh))) = s.line_height else {
            panic!("line height is not absolute pixels: {:?}", s.line_height);
        };
        assert!((f32::from(lh) - 13.0 * LINE_HEIGHT_RATIO).abs() < 1e-4);
        assert_eq!(t_md(&m).font_weight, Some(FontWeight::MEDIUM));
        assert_eq!(t_lg(&m).font_weight, Some(FontWeight::SEMIBOLD));
        assert_eq!(t_xl(&m).font_size, Some(AbsoluteLength::Pixels(px(20.0))));
    }
}
