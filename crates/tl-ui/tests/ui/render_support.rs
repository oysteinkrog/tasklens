//! tl-ui's setup for the testkit render harness (ADR-013, R-291, ADR-020): the bundled assets,
//! fonts and theme store, and the golden directory `tests/golden`. The `render_*` modules use
//! `RenderHarness`, `assert_golden` and `run_on_lavapipe` from here.
//!
//! Run with `cargo test -p tl-ui --features render-tests --test ui -- render_support::`.

#![cfg(feature = "render-tests")]

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::Arc;

use gpui_kit::App;
use tl_ui::components::icons::TlAssets;
use tl_ui::text::fonts::load_bundled_fonts;
use tl_ui::theme::store::{self, ActivateSource};
use tl_ui_settings::settings::{AppearanceSettings, ThemeSettings};
use tl_ui_testkit::render::{self as kit, RenderSetup, RgbaImage};

pub use tl_ui_testkit::render::run_on_lavapipe;

/// Where the golden PNGs live.
pub const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");

/// The testkit harness with the bundled fonts, the kit and the Default Dark theme.
pub struct RenderHarness(kit::RenderHarness);

impl RenderHarness {
    /// Builds the app. Panics when no headless renderer is available.
    pub fn new() -> Self {
        Self(kit::RenderHarness::new(RenderSetup {
            assets: Arc::new(TlAssets),
            init,
            set_theme,
            apply_metrics,
        }))
    }
}

impl Default for RenderHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for RenderHarness {
    type Target = kit::RenderHarness;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RenderHarness {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

fn init(cx: &mut App) -> u16 {
    let appearance = AppearanceSettings::default();
    load_bundled_fonts(cx);
    gpui_kit::init(cx);
    store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
    appearance.ui_scale_pct
}

fn apply_metrics(ui_scale_pct: u16, cx: &mut App) {
    let appearance = AppearanceSettings {
        ui_scale_pct,
        ..AppearanceSettings::default()
    };
    store::apply_metrics(&appearance, cx);
}

fn set_theme(name: &str, cx: &mut App) {
    store::activate(name, ActivateSource::User, cx)
        .unwrap_or_else(|err| panic!("theme {name}: {err:?}"));
}

/// Compares `img` with `tests/golden/<name>.png`, or writes the golden when
/// `TL_UPDATE_GOLDEN=1`. See `tl_ui_testkit::render::assert_golden`.
pub fn assert_golden(name: &str, img: &RgbaImage) {
    kit::assert_golden(Path::new(GOLDEN_DIR), name, img);
}

#[cfg(test)]
mod tests {
    use gpui_kit::{AppContext, Context, IntoElement, Render, Styled, Window, div, px, size};
    use tl_ui::theme;
    use tl_ui_testkit::render::{CHANNEL_TOLERANCE, Rgba};

    use super::*;

    /// A view filled with the theme's `window_bg`.
    struct Fill;

    impl Render for Fill {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().bg(theme::current(cx).window_bg)
        }
    }

    #[test]
    fn solid_fill_matches_window_bg() {
        run_on_lavapipe(
            "render_support::tests::solid_fill_matches_window_bg",
            || {
                let mut harness = RenderHarness::new();
                let expected = Rgba([0x19, 0x1B, 0x1D, 0xFF]);
                for (scale, w, h) in [(1.0, 200, 80), (1.1, 220, 88)] {
                    harness.set_scale(scale);
                    let img = harness.render(size(px(100.), px(40.)), |_, cx| cx.new(|_| Fill));
                    assert_eq!(img.dimensions(), (w, h), "scale {scale}");
                    let off = img
                        .pixels()
                        .filter(|p| {
                            p.0.iter()
                                .zip(expected.0)
                                .any(|(a, b)| a.abs_diff(b) > CHANNEL_TOLERANCE)
                        })
                        .count();
                    assert_eq!(off, 0, "scale {scale}: {off} pixels are not #191B1D");
                }
            },
        );
    }
}
