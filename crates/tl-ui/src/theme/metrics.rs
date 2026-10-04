//! `UiMetrics`: every size token, already multiplied by the UI scale (ADR-009 "Metrics and
//! scale", UI-FND-003, R-020, R-077).
//!
//! Views read `metrics(cx)` once per render into a local and take sizes from it. No view
//! multiplies by `ui_scale` by hand, and element builders do no `px()` arithmetic beyond
//! `m.sp[n]`.
//!
//! GPUI applies the compositor scale itself. Nothing in tl-ui reads `window.scale_factor()`
//! except `UiMetrics::hairline`, chart geometry keys and text cache invalidation.

use gpui_kit::{App, Global, Pixels, Window, px};
use tl_model::settings::AppearanceSettings;

/// Text sizes: 11 12 13 14 16 20 × ui_scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextSizes {
    pub xs: Pixels,
    pub sm: Pixels,
    pub base: Pixels,
    pub md: Pixels,
    pub lg: Pixels,
    pub xl: Pixels,
}

/// Icon sizes: 12 14 16 20 × ui_scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconSizes {
    pub xs: Pixels,
    pub sm: Pixels,
    pub md: Pixels,
    pub lg: Pixels,
}

/// The size tokens every view reads. Installed as a GPUI global.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiMetrics {
    /// `ui_scale_pct / 100`.
    pub ui_scale: f32,
    /// 28 × row_height_pct/100 × ui_scale.
    pub row_height: Pixels,
    /// 13 × ui_scale.
    pub base_text: Pixels,
    /// 4 × ui_scale, or 0 when `rounded_corners` is off.
    pub radius_small: Pixels,
    /// 6 × ui_scale, or 0 when `rounded_corners` is off.
    pub radius_card: Pixels,
    /// `sp[0]` = 0; `sp[1..=8]` = 2 4 6 8 12 16 24 32 × ui_scale.
    pub sp: [Pixels; 9],
    pub text: TextSizes,
    pub icon: IconSizes,
    /// 1.3 for every text size. A ratio, not scaled.
    pub line_height: f32,
    /// Bumps on any change here or in the active theme.
    pub style_version: u64,
}

impl Global for UiMetrics {}

impl UiMetrics {
    /// One physical pixel: `1 / window.scale_factor()` logical px.
    pub fn hairline(window: &Window) -> Pixels {
        px(1.0 / window.scale_factor())
    }
}

/// Builds the metrics from the `[appearance]` settings. Pure. The settings loader has already
/// clamped `ui_scale_pct` to 75..=200 and `row_height_pct` to 80..=150. Sets
/// `style_version = prev_version + 1`.
pub fn compute(appearance: &AppearanceSettings, prev_version: u64) -> UiMetrics {
    let s = appearance.ui_scale_pct as f32 / 100.0;
    let row = appearance.row_height_pct as f32 / 100.0;
    let radius = |r: f32| {
        if appearance.rounded_corners {
            px(r * s)
        } else {
            px(0.0)
        }
    };
    let sp = [0.0, 2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0].map(|v: f32| px(v * s));
    UiMetrics {
        ui_scale: s,
        row_height: px(28.0 * row * s),
        base_text: px(13.0 * s),
        radius_small: radius(4.0),
        radius_card: radius(6.0),
        sp,
        text: TextSizes {
            xs: px(11.0 * s),
            sm: px(12.0 * s),
            base: px(13.0 * s),
            md: px(14.0 * s),
            lg: px(16.0 * s),
            xl: px(20.0 * s),
        },
        icon: IconSizes {
            xs: px(12.0 * s),
            sm: px(14.0 * s),
            md: px(16.0 * s),
            lg: px(20.0 * s),
        },
        line_height: 1.3,
        style_version: prev_version + 1,
    }
}

/// Installs the first metrics, with `style_version = 1`.
pub fn init_metrics(appearance: &AppearanceSettings, cx: &mut App) {
    cx.set_global(compute(appearance, 0));
}

/// Computes new metrics. When any field other than `style_version` changed, installs them with
/// the bumped version and returns `true`. Otherwise leaves the global alone and returns `false`.
///
/// Does not call `theme::map::apply` and does not touch `CellTextCache`. The theme store calls
/// `map::apply` after a metrics change, and `CellTextCache` clears itself on a new
/// `style_version`. A window scale factor change is not part of `UiMetrics`: the caller reports
/// it with `bump_style_version`.
pub fn recompute(appearance: &AppearanceSettings, cx: &mut App) -> bool {
    let prev = cx.has_global::<UiMetrics>().then(|| *metrics(cx));
    let prev_version = prev.map_or(0, |m| m.style_version);
    let next = compute(appearance, prev_version);
    if let Some(prev) = prev
        && UiMetrics {
            style_version: prev.style_version,
            ..next
        } == prev
    {
        return false;
    }
    cx.set_global(next);
    true
}

/// Adds 1 to `style_version` on the installed metrics. Every style version change goes through
/// here: theme switches, theme preview edits and scale factor changes.
pub fn bump_style_version(cx: &mut App) {
    cx.global_mut::<UiMetrics>().style_version += 1;
}

/// The installed metrics. Shorthand for `cx.global::<UiMetrics>()`.
pub fn metrics(cx: &App) -> &UiMetrics {
    cx.global::<UiMetrics>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    fn appearance(ui_scale_pct: u16, row_height_pct: u16, rounded: bool) -> AppearanceSettings {
        AppearanceSettings {
            ui_scale_pct,
            row_height_pct,
            rounded_corners: rounded,
            ..AppearanceSettings::default()
        }
    }

    fn close(a: Pixels, b: f32) -> bool {
        (f32::from(a) - b).abs() < 1e-4
    }

    #[test]
    fn defaults_at_100() {
        let m = compute(&appearance(100, 100, true), 0);
        assert_eq!(m.ui_scale, 1.0);
        assert_eq!(m.row_height, px(28.0));
        assert_eq!(m.base_text, px(13.0));
        assert_eq!(m.radius_small, px(4.0));
        assert_eq!(m.radius_card, px(6.0));
        assert_eq!(
            m.sp,
            [0.0, 2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0].map(px)
        );
        let t = m.text;
        assert_eq!(
            [t.xs, t.sm, t.base, t.md, t.lg, t.xl],
            [11.0, 12.0, 13.0, 14.0, 16.0, 20.0].map(px)
        );
        let i = m.icon;
        assert_eq!([i.xs, i.sm, i.md, i.lg], [12.0, 14.0, 16.0, 20.0].map(px));
        assert_eq!(m.line_height, 1.3);
        assert_eq!(m.style_version, 1);
    }

    #[test]
    fn scale_and_row() {
        let m = compute(&appearance(150, 120, true), 3);
        assert_eq!(m.ui_scale, 1.5);
        assert!(close(m.row_height, 50.4), "{:?}", m.row_height);
        assert!(close(m.base_text, 19.5), "{:?}", m.base_text);
        assert!(close(m.sp[4], 12.0), "{:?}", m.sp[4]);
        assert_eq!(m.style_version, 4);
    }

    #[test]
    fn rounded_off_zeroes_radii() {
        for scale in [75, 100, 150, 200] {
            let m = compute(&appearance(scale, 100, false), 0);
            assert_eq!(m.radius_small, px(0.0));
            assert_eq!(m.radius_card, px(0.0));
        }
    }

    #[gpui_kit::test]
    fn recompute_bumps_only_on_change(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init_metrics(&AppearanceSettings::default(), cx);
            assert_eq!(metrics(cx).style_version, 1);
            cx.global_mut::<UiMetrics>().style_version = 5;

            assert!(!recompute(&AppearanceSettings::default(), cx));
            assert_eq!(metrics(cx).style_version, 5);

            assert!(recompute(&appearance(125, 100, true), cx));
            assert_eq!(metrics(cx).style_version, 6);
            assert_eq!(metrics(cx).base_text, px(13.0 * 1.25));
        });
    }

    #[gpui_kit::test]
    fn bump_style_version_increments(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init_metrics(&AppearanceSettings::default(), cx);
            let before = metrics(cx).style_version;
            bump_style_version(cx);
            bump_style_version(cx);
            assert_eq!(metrics(cx).style_version, before + 2);
        });
    }
}
