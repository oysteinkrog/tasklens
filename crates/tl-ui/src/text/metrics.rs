//! `TextMetricsCache`: measured widths of numeric column templates and per-character advances
//! (ADR-009 "Text caching", canonical table (d), PROC-023).
//!
//! Tables size numeric columns from the template widths, so a column never resizes as values
//! change. Measurement runs only on a cache miss. Both maps clear when the `style_version` in
//! `UiMetrics` changes.

use gpui_kit::{App, Font, FontWeight, Global, Pixels, SharedString, TextRun, Window, black, font};
use rustc_hash::FxHashMap;

use super::fonts::UI_FAMILY;
use super::style::TNUM;

/// Templates for the minimum widths of numeric columns (canonical table (d)).
pub const NUMERIC_TEMPLATES: [&str; 7] = [
    "999999 +99",
    "100.0%",
    "xxx Mbps",
    "xxx MiB/s",
    "xxx GiB/s",
    "999 MiB",
    "999:59:59",
];

/// Template for the PID column (canonical table (d)).
pub const PID_TEMPLATE: &str = "999999";

/// Cache of measured text widths for the table font. Installed as a GPUI global.
#[derive(Default)]
pub struct TextMetricsCache {
    /// Key: template, font size bits, window scale factor bits.
    templates: FxHashMap<(SharedString, u32, u32), Pixels>,
    /// Key: character, font size bits, window scale factor bits.
    chars: FxHashMap<(char, u32, u32), Pixels>,
    style_version: u64,
    misses: u64,
}

impl Global for TextMetricsCache {}

impl TextMetricsCache {
    /// Width of `template` in Noto Sans 400 with `tnum` at `size`.
    pub fn template_width(
        &mut self,
        template: &str,
        size: Pixels,
        window: &Window,
        _cx: &App,
    ) -> Pixels {
        let key = (
            SharedString::new(template),
            f32::from(size).to_bits(),
            window.scale_factor().to_bits(),
        );
        if let Some(width) = self.templates.get(&key) {
            return *width;
        }
        self.misses += 1;
        let numeric = Font {
            features: TNUM.clone(),
            ..font(UI_FAMILY)
        };
        let width = measure(template, numeric, size, window);
        self.templates.insert(key, width);
        width
    }

    /// Advance of `ch` in Noto Sans 400 at `size`, with proportional figures as in labels and
    /// paths. Used by middle ellipsis (E15-003).
    pub fn char_advance(&mut self, ch: char, size: Pixels, window: &Window, _cx: &App) -> Pixels {
        let key = (
            ch,
            f32::from(size).to_bits(),
            window.scale_factor().to_bits(),
        );
        if let Some(width) = self.chars.get(&key) {
            return *width;
        }
        self.misses += 1;
        let label = Font {
            weight: FontWeight::NORMAL,
            ..font(UI_FAMILY)
        };
        let mut buf = [0u8; 4];
        let width = measure(ch.encode_utf8(&mut buf), label, size, window);
        self.chars.insert(key, width);
        width
    }

    /// Clears both maps when `style_version` differs from the version the cache was built for.
    pub fn clear_if_stale(&mut self, style_version: u64) {
        if style_version != self.style_version {
            self.templates.clear();
            self.chars.clear();
            self.style_version = style_version;
        }
    }

    /// Number of measurements made so far, for tests and the bench report.
    pub fn miss_count(&self) -> u64 {
        self.misses
    }
}

fn measure(text: &str, font: Font, size: Pixels, window: &Window) -> Pixels {
    let run = TextRun {
        len: text.len(),
        font,
        color: black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .layout_line(text, size, &[run], None)
        .width
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, TestDispatcher};
    use tl_model::settings::AppearanceSettings;

    use super::*;
    use crate::text::fonts::load_bundled_fonts;
    use crate::theme::metrics::compute;

    /// A test context backed by the real Linux text system, with the bundled fonts loaded.
    fn cosmic_cx() -> TestAppContext {
        let text_system = gpui_kit::platform::current_platform(true).text_system();
        let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
        cx.update(|cx| {
            let report = load_bundled_fonts(cx);
            assert!(report.ui_ok, "failed faces: {:?}", report.failed);
        });
        cx
    }

    #[test]
    fn template_widths_monotonic_in_scale() {
        let mut cx = cosmic_cx();
        let vcx = cx.add_empty_window();
        vcx.update(|window, cx| {
            let mut cache = TextMetricsCache::default();
            let widths: Vec<f32> = [75, 100, 150, 200]
                .into_iter()
                .map(|pct| {
                    let appearance = AppearanceSettings {
                        ui_scale_pct: pct,
                        ..AppearanceSettings::default()
                    };
                    let m = compute(&appearance, 0);
                    f32::from(cache.template_width("100.0%", m.text.base, window, cx))
                })
                .collect();
            assert!(widths[0] > 0.0, "zero width at 75: {widths:?}");
            assert!(
                widths.windows(2).all(|w| w[1] > w[0]),
                "widths do not increase with scale: {widths:?}"
            );
        });
    }

    #[test]
    fn cache_hit_and_clear_on_style_version() {
        let mut cx = cosmic_cx();
        let vcx = cx.add_empty_window();
        vcx.update(|window, cx| {
            let m = compute(&AppearanceSettings::default(), 0);
            let mut cache = TextMetricsCache::default();
            cache.clear_if_stale(m.style_version);

            let first = cache.template_width(NUMERIC_TEMPLATES[0], m.text.base, window, cx);
            assert_eq!(cache.miss_count(), 1);
            let second = cache.template_width(NUMERIC_TEMPLATES[0], m.text.base, window, cx);
            assert_eq!(cache.miss_count(), 1, "second call was not a cache hit");
            assert_eq!(first, second);

            cache.clear_if_stale(m.style_version);
            cache.template_width(NUMERIC_TEMPLATES[0], m.text.base, window, cx);
            assert_eq!(cache.miss_count(), 1, "same version must not clear");

            cache.clear_if_stale(m.style_version + 1);
            let third = cache.template_width(NUMERIC_TEMPLATES[0], m.text.base, window, cx);
            assert_eq!(cache.miss_count(), 2, "new version did not re-measure");
            assert_eq!(first, third);

            let a = cache.char_advance('a', m.text.base, window, cx);
            cache.char_advance('a', m.text.base, window, cx);
            assert!(a > Pixels::ZERO);
            assert_eq!(cache.miss_count(), 3);
        });
    }
}
