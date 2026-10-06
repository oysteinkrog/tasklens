//! Middle ellipsis for paths in table cells (UI-FND-052a).
//!
//! The file name stays visible and the middle of the path is cut. End ellipsis is not here: it is
//! GPUI's `text_overflow(TextOverflow::Ellipsis)` on a `whitespace_nowrap` element (UI-FND-052).

use gpui_kit::{Pixels, SharedString};
use unicode_segmentation::UnicodeSegmentation;

/// The ellipsis character placed in the middle of a cut path.
pub const ELLIPSIS: char = '\u{2026}';

/// Share of `max_px` up to which the last path component is kept whole.
const WHOLE_TAIL_SHARE: f32 = 0.4;

/// Number of characters kept from a last component that is too wide to keep whole.
const LONG_TAIL_CHARS: usize = 12;

/// Fewest characters the ellipsis may replace. With fewer, the text is returned whole.
const MIN_REMOVED: usize = 2;

/// Cuts the middle of `text` so it fits in `max_px`, keeping the last path component.
///
/// `measure` returns the width of a string. It should be backed by
/// `TextMetricsCache::char_advance` (a sum of per-character advances), so one call costs O(n)
/// additions and shapes nothing. The cut falls only between graphemes. If the text fits, or the
/// ellipsis would replace fewer than 2 characters, the text is returned whole and the cell clips
/// it.
pub fn middle_ellipsis(
    text: &str,
    max_px: Pixels,
    measure: &mut impl FnMut(&str) -> Pixels,
) -> SharedString {
    let graphemes: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    let widths: Vec<Pixels> = graphemes.iter().map(|(_, g)| measure(g)).collect();
    let total = widths.iter().fold(Pixels::ZERO, |acc, w| acc + *w);
    if total <= max_px {
        return SharedString::new(text);
    }

    // Index of the first grapheme of the kept tail.
    let slash = graphemes.iter().rposition(|(_, g)| *g == "/");
    let component_start = slash.map_or(0, |i| i + 1);
    let component_width = widths[component_start..]
        .iter()
        .fold(Pixels::ZERO, |acc, w| acc + *w);
    let tail_start = if component_width <= max_px * WHOLE_TAIL_SHARE {
        // Keep the slash too, so the result reads "…/name".
        slash.unwrap_or(0)
    } else {
        graphemes
            .len()
            .saturating_sub(LONG_TAIL_CHARS)
            .max(component_start)
    };
    let tail_width = widths[tail_start..]
        .iter()
        .fold(Pixels::ZERO, |acc, w| acc + *w);

    let mut ellipsis_buf = [0u8; 4];
    let budget = max_px - measure(ELLIPSIS.encode_utf8(&mut ellipsis_buf)) - tail_width;
    let mut start_end = 0;
    let mut used = Pixels::ZERO;
    while start_end < tail_start && used + widths[start_end] <= budget {
        used += widths[start_end];
        start_end += 1;
    }

    if tail_start - start_end < MIN_REMOVED {
        return SharedString::new(text);
    }

    let head_byte_end = graphemes
        .get(start_end)
        .map_or(text.len(), |(byte, _)| *byte);
    let tail_byte_start = graphemes
        .get(tail_start)
        .map_or(text.len(), |(byte, _)| *byte);
    let mut out = String::with_capacity(head_byte_end + 3 + text.len() - tail_byte_start);
    out.push_str(&text[..head_byte_end]);
    out.push(ELLIPSIS);
    out.push_str(&text[tail_byte_start..]);
    SharedString::from(out)
}

#[cfg(test)]
mod tests {
    use gpui_kit::px;

    use super::*;

    /// Monospace test measure: 1 px per character, the ellipsis included.
    fn mono(s: &str) -> Pixels {
        px(s.chars().count() as f32)
    }

    const STEAM: &str = "/home/oystein/.local/share/Steam/steamapps/common/Game/bin/game.x86_64";

    #[test]
    fn fits_unchanged() {
        let out = middle_ellipsis("/usr/lib/firefox/firefox-bin", px(30.), &mut mono);
        assert_eq!(out.as_ref(), "/usr/lib/firefox/firefox-bin");
    }

    #[test]
    fn keeps_last_component() {
        // The UI-FND-052a example string is 35 characters, so it is the result at 35 px.
        let out = middle_ellipsis(STEAM, px(35.), &mut mono);
        assert_eq!(out.as_ref(), "/home/oystein/.local/s\u{2026}/game.x86_64");

        // At 30 px the same rules give a shorter start.
        let out = middle_ellipsis(STEAM, px(30.), &mut mono);
        assert_eq!(out.as_ref(), "/home/oystein/.lo\u{2026}/game.x86_64");
        assert_eq!(out.chars().count(), 30);
    }

    #[test]
    fn long_tail_keeps_12_chars() {
        let out = middle_ellipsis(
            "/opt/x/averyveryverylongexecutablename.bin",
            px(30.),
            &mut mono,
        );
        assert_eq!(out.as_ref(), "/opt/x/averyveryv\u{2026}ablename.bin");
    }

    #[test]
    fn grapheme_boundary() {
        // A flag is two regional indicators; "e\u{301}" is a letter plus a combining accent.
        let text = "/abcdefgh\u{1F1F3}\u{1F1F4}e\u{301}e\u{301}e\u{301}xyz/name.txt";

        // The budget ends inside the flag: the flag is dropped whole.
        let out = middle_ellipsis(text, px(20.), &mut mono);
        assert_eq!(out.as_ref(), "/abcdefgh\u{2026}/name.txt");

        // The budget ends inside "e\u{301}": the accent is not left behind or split off.
        let out = middle_ellipsis(text, px(22.), &mut mono);
        assert_eq!(out.as_ref(), "/abcdefgh\u{1F1F3}\u{1F1F4}\u{2026}/name.txt");
    }

    #[test]
    fn two_char_minimum() {
        // "W" is 2 px wide and the ellipsis 1 px, so cutting only "W" would fit. The text is one
        // pixel too wide; cutting one character is not allowed, so it comes back whole.
        let mut wide_w = |s: &str| {
            px(s.chars()
                .map(|c| if c == 'W' { 2. } else { 1. })
                .sum::<f32>())
        };
        let text = "/abcdefghijklW/x.bin";
        let out = middle_ellipsis(text, px(20.), &mut wide_w);
        assert_eq!(out.as_ref(), text);
    }
}
