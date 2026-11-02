//! The testkit render harness set up for the tl-ui crates (ADR-013, R-291, ADR-020): the bundled
//! assets and fonts, the theme store, and golden comparison in a directory the caller names.
//!
//! Each crate keeps its goldens in its own `tests/golden` and passes that directory to
//! [`assert_golden`]. Headless rendering needs the feature `render-tests`.

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::Arc;

use gpui_kit::App;
use tl_ui_base::text::fonts::load_bundled_fonts;
use tl_ui_base::theme::store::{self, ActivateSource};
use tl_ui_kit::components::icons::TlAssets;
use tl_ui_settings::settings::{AppearanceSettings, ThemeSettings};
use tl_ui_testkit::render::{self as kit, RenderSetup, RgbaImage};

pub use tl_ui_testkit::render::run_on_lavapipe;

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

/// Compares `img` with `<golden_dir>/<name>.png`, or writes the golden when `TL_UPDATE_GOLDEN=1`.
/// See `tl_ui_testkit::render::assert_golden`.
pub fn assert_golden(golden_dir: &Path, name: &str, img: &RgbaImage) {
    kit::assert_golden(golden_dir, name, img);
}
