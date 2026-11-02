//! Gallery render goldens (E15-A3, ADR-013, UI-FND-030): every gallery page rendered on lavapipe
//! at `ui_scale` 1.0 and 1.1 and compared with `tests/golden/gallery_<page>_<scale>.png`.
//! `TL_UPDATE_GOLDEN=1` rewrites them. Reduced motion is forced on by the harness, so animations
//! show their end state. Captures are at device scale 2.0 (R-291).
//!
//! Run with `cargo test -p tl-ui-kit --features render-tests --test render -- render_gallery::`.

use std::panic::{AssertUnwindSafe, catch_unwind};

use gpui_kit::{AppContext as _, TestAppContext, px, size};
use tl_ui_kit::components::gallery::{
    self, GALLERY_PAGES, GalleryView, WINDOW_HEIGHT, WINDOW_WIDTH,
};
use tl_ui_pagehost::render::{RenderHarness, run_on_lavapipe};
use tl_ui_settings::settings::{AppearanceSettings, ThemeSettings};

/// Where this crate's golden PNGs live.
const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");

/// Compares `img` with `tests/golden/<name>.png`, or writes it when `TL_UPDATE_GOLDEN=1`.
fn assert_golden(name: &str, img: &tl_ui_testkit::render::RgbaImage) {
    tl_ui_pagehost::render::assert_golden(std::path::Path::new(GOLDEN_DIR), name, img);
}

/// UI-FND-023: at most 2 spinners or indeterminate bars per window.
const ANIMATED_BUDGET: usize = 2;

#[test]
fn gallery_pages_match_goldens() {
    run_on_lavapipe("render_gallery::gallery_pages_match_goldens", || {
        let mut harness = RenderHarness::new();
        let mut failures = Vec::new();
        for (scale, suffix) in [(1.0, "1_0"), (1.1, "1_1")] {
            harness.set_scale(scale);
            for (ix, page) in GALLERY_PAGES.iter().enumerate() {
                let img =
                    harness.render(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |window, cx| {
                        gallery::init(cx);
                        cx.new(|cx| GalleryView::new(ix, window, cx))
                    });
                let name = format!("gallery_{page}_{suffix}");
                // Compare every page before failing, so one run lists all of them.
                if let Err(err) = catch_unwind(AssertUnwindSafe(|| assert_golden(&name, &img))) {
                    let msg = err
                        .downcast_ref::<String>()
                        .cloned()
                        .unwrap_or_else(|| name.clone());
                    failures.push(msg);
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

#[gpui_kit::test]
fn animated_element_budget(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui_base::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        gallery::init(cx);
    });
    for (ix, page) in GALLERY_PAGES.iter().enumerate() {
        let (view, vcx) = cx.add_window_view(|window, cx| GalleryView::new(ix, window, cx));
        vcx.run_until_parked();
        let count = view.read_with(vcx, |v, _| v.animated_count());
        assert!(
            count <= ANIMATED_BUDGET,
            "page {page} shows {count} spinners or indeterminate bars, budget {ANIMATED_BUDGET}"
        );
    }
}
