//! Tints derived from the theme slots and the appearance settings (UI-FND-001).

use gpui_kit::{Hsla, Rgba};
use tl_model::settings::AppearanceSettings;

use super::{ThemeMode, TlTheme};

/// Colours computed from the theme, rebuilt on theme switch and appearance change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Derived {
    pub new_row_tint: Hsla,
    pub exited_row_tint: Hsla,
    pub focus_ring: Hsla,
    pub shadow: Hsla,
    pub overlay: Hsla,
    pub stale_text: Hsla,
}

/// Black with the given alpha.
fn black(a: f32) -> Hsla {
    Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.0,
        a,
    }
}

impl Derived {
    /// All tints transparent. `rebuild_derived` fills the real values.
    pub(crate) const fn empty() -> Self {
        let clear = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 0.0,
        };
        Derived {
            new_row_tint: clear,
            exited_row_tint: clear,
            focus_ring: clear,
            shadow: clear,
            overlay: clear,
            stale_text: clear,
        }
    }
}

/// Recomputes `t.derived` from the slots, the status colours and `appearance`.
pub fn rebuild_derived(t: &mut TlTheme, appearance: &AppearanceSettings) {
    let shadow_pct = f32::from(appearance.shadow_intensity_pct.min(100)) / 100.0;
    let (shadow_base, overlay_a) = match t.mode {
        ThemeMode::Dark => (0.35, 0.4),
        ThemeMode::Light => (0.18, 0.25),
    };
    t.derived = Derived {
        new_row_tint: t.status.success.alpha(0.18),
        exited_row_tint: t.status.danger.alpha(0.18),
        focus_ring: t.accent.alpha(1.0),
        shadow: black(shadow_base * shadow_pct),
        overlay: black(overlay_a),
        stale_text: t.text_secondary,
    };
}

/// Heat colour for a cell: `row_bg` moved toward `value_heat` by `value / reference`, scaled by
/// `intensity_pct`. The lerp runs per sRGB channel and keeps the alpha of `row_bg`.
/// `reference <= 0` returns `row_bg`. Does not allocate.
pub fn heat(row_bg: Hsla, value_heat: Hsla, value: f32, reference: f32, intensity_pct: u8) -> Hsla {
    if reference <= 0.0 {
        return row_bg;
    }
    let t = (value / reference).clamp(0.0, 1.0) * f32::from(intensity_pct.min(100)) * 0.01;
    let a = Rgba::from(row_bg);
    let b = Rgba::from(value_heat);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
    .into()
}

/// Draws `tint` over `base` with source-over in sRGB. Used for the NEW and EXITED row tints.
pub fn blend_over(base: Hsla, tint: Hsla) -> Hsla {
    base.blend(tint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::tests::test_theme;
    use gpui_kit::rgb;

    const EPS: f32 = 1.0 / 255.0;

    fn close(a: Hsla, b: Hsla) -> bool {
        let (a, b) = (Rgba::from(a), Rgba::from(b));
        (a.r - b.r).abs() <= EPS
            && (a.g - b.g).abs() <= EPS
            && (a.b - b.b).abs() <= EPS
            && (a.a - b.a).abs() <= EPS
    }

    fn row_bg() -> Hsla {
        rgb(0x191B1D).into()
    }

    fn value_heat() -> Hsla {
        rgb(0x3E2A28).into()
    }

    fn appearance(shadow: u8) -> AppearanceSettings {
        AppearanceSettings {
            shadow_intensity_pct: shadow,
            ..AppearanceSettings::default()
        }
    }

    #[test]
    fn heat_endpoints() {
        let r = 4.0;
        assert!(close(heat(row_bg(), value_heat(), 0.0, r, 100), row_bg()));
        assert!(close(heat(row_bg(), value_heat(), r, r, 100), value_heat()));
        assert_eq!(heat(row_bg(), value_heat(), 3.0, 0.0, 100), row_bg());
    }

    #[test]
    fn heat_midpoint_and_clamp() {
        let r = 10.0;
        let mid = Rgba::from(heat(row_bg(), value_heat(), r, r, 50));
        let (a, b) = (Rgba::from(row_bg()), Rgba::from(value_heat()));
        assert!((mid.r - (a.r + b.r) / 2.0).abs() <= EPS);
        assert!((mid.g - (a.g + b.g) / 2.0).abs() <= EPS);
        assert!((mid.b - (a.b + b.b) / 2.0).abs() <= EPS);
        assert!(close(
            heat(row_bg(), value_heat(), 3.0 * r, r, 100),
            value_heat()
        ));
    }

    #[test]
    fn shadow_and_overlay_alphas() {
        let mut dark = test_theme(ThemeMode::Dark);
        rebuild_derived(&mut dark, &appearance(100));
        assert_eq!(dark.derived.shadow.a, 0.35);
        assert_eq!(dark.derived.overlay.a, 0.4);
        rebuild_derived(&mut dark, &appearance(0));
        assert_eq!(dark.derived.shadow.a, 0.0);

        let mut light = test_theme(ThemeMode::Light);
        rebuild_derived(&mut light, &appearance(100));
        assert_eq!(light.derived.shadow.a, 0.18);
        assert_eq!(light.derived.overlay.a, 0.25);
        rebuild_derived(&mut light, &appearance(0));
        assert_eq!(light.derived.shadow.a, 0.0);
    }

    #[test]
    fn tints_follow_status_and_accent() {
        let mut t = test_theme(ThemeMode::Dark);
        rebuild_derived(&mut t, &appearance(100));
        assert_eq!(t.derived.new_row_tint, t.status.success.alpha(0.18));
        assert_eq!(t.derived.exited_row_tint, t.status.danger.alpha(0.18));
        assert_eq!(t.derived.focus_ring, t.accent.alpha(1.0));
        assert_eq!(t.derived.stale_text, t.text_secondary);
        let blended = blend_over(row_bg(), t.derived.new_row_tint);
        assert!(close(
            blended,
            Rgba::from(row_bg())
                .blend(t.derived.new_row_tint.into())
                .into()
        ));
    }
}
