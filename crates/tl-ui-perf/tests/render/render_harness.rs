//! Self-check of the page host render harness (ADR-013, R-291): a view filled with the Default
//! Dark `window_bg` renders as that colour at ui_scale 1.0 and 1.1. Moved from tl-ui's
//! `render_support` module (tasklens-zljr5.39).
//!
//! Run with `cargo test -p tl-ui-perf --features render-tests --test render -- render_harness::`.

use gpui_kit::{AppContext, Context, IntoElement, Render, Styled, Window, div, px, size};
use tl_ui_base::theme;
use tl_ui_pagehost::render::{RenderHarness, run_on_lavapipe};
use tl_ui_testkit::render::{CHANNEL_TOLERANCE, Rgba};

/// A view filled with the theme's `window_bg`.
struct Fill;

impl Render for Fill {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(theme::current(cx).window_bg)
    }
}

#[test]
fn solid_fill_matches_window_bg() {
    run_on_lavapipe("render_harness::solid_fill_matches_window_bg", || {
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
    });
}
