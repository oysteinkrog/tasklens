//! Display rules for untrusted process text (UI-FND-053, ADR-015).
//!
//! Process-derived text is sanitized by `tl-collect::sanitize` before it reaches `ProcessMeta`:
//! C0 and C1 controls became U+FFFD, bidi controls and zero-width characters were removed. This
//! module trusts that and only marks U+FFFD in Name cells, applies the name fallback chain and,
//! in debug builds, checks that no control character survived.
//!
//! Rules 3 to 8 of UI-FND-053, which every cell that shows this text follows:
//! - Each table cell is its own paragraph with base direction LTR. Right-to-left letters (Hebrew,
//!   Arabic) are kept and shaped by cosmic-text's bidi algorithm, so an RTL name renders
//!   right-to-left inside a left-aligned cell and never reorders neighbouring cells, the "+N" run
//!   or the ellipsis.
//! - The ellipsis goes at the logical end of the string, so it shows at the left of an RTL run.
//! - U+2028 and U+2029 count as controls; a cell is always a single line.
//! - U+200D (zero-width joiner) is kept so emoji sequences render as one glyph.
//! - Tabs became U+FFFD, so a cell value never holds a tab or a newline and TSV copy is
//!   unambiguous.
//! - Copy and export use the sanitized text as displayed, never the raw bytes.
//! - The UI itself is LTR in every shipped language; nothing is mirrored.
//!
//! Never log the text this module handles.

use std::ops::Range;

use gpui_kit::{Font, Hsla, SharedString, TextRun};
use smallvec::SmallVec;

/// Splits `text` into byte ranges, `true` for each run of U+FFFD characters.
///
/// Callers cache the result and split again only when the text changes.
pub fn split_fffd_runs(text: &str) -> SmallVec<[(Range<usize>, bool); 4]> {
    let mut runs = SmallVec::new();
    let mut start = 0;
    let mut in_fffd = false;
    for (i, c) in text.char_indices() {
        let is_fffd = c == char::REPLACEMENT_CHARACTER;
        if is_fffd != in_fffd && i > start {
            runs.push((start..i, in_fffd));
            start = i;
        }
        in_fffd = is_fffd;
    }
    if start < text.len() {
        runs.push((start..text.len(), in_fffd));
    }
    runs
}

/// One `TextRun` per run of `text`: U+FFFD runs in `warning` (`status.warning`), the rest in
/// `normal`.
///
/// For Name cells only. Other cells and tooltips draw U+FFFD in the normal colour. `font` is the
/// cell's font, which `TextRun` needs.
pub fn name_runs(
    text: &SharedString,
    font: &Font,
    normal: Hsla,
    warning: Hsla,
) -> SmallVec<[TextRun; 4]> {
    split_fffd_runs(text)
        .into_iter()
        .map(|(range, is_fffd)| TextRun {
            len: range.len(),
            font: font.clone(),
            color: if is_fffd { warning } else { normal },
            background_color: None,
            underline: None,
            strikethrough: None,
        })
        .collect()
}

/// The name to show for a process: `name`, else `comm`, else `[pid N]` written into `scratch`.
///
/// A string that is empty or holds only U+FFFD counts as missing.
pub fn display_name<'a>(
    name: &'a str,
    comm: &'a str,
    pid: u32,
    scratch: &'a mut String,
) -> &'a str {
    if !is_blank(name) {
        return name;
    }
    if !is_blank(comm) {
        return comm;
    }
    use std::fmt::Write as _;
    scratch.clear();
    let _ = write!(scratch, "[pid {pid}]");
    scratch
}

fn is_blank(text: &str) -> bool {
    text.chars().all(|c| c == char::REPLACEMENT_CHARACTER)
}

/// In debug builds, asserts that no control character survived sanitizing. Does nothing in
/// release builds.
///
/// Rejects C0, DEL, C1, U+200B, U+200C, U+200E, U+200F, U+202A to U+202E, U+2066 to U+2069,
/// U+2028 and U+2029. U+200D (zero-width joiner) is allowed.
pub fn debug_assert_clean(text: &str) {
    if cfg!(debug_assertions) {
        // The message names the code point only; the text itself is never logged.
        let bad = text.chars().find(|&c| is_forbidden(c));
        debug_assert!(
            bad.is_none(),
            "unsanitized control character U+{:04X} in display text",
            bad.map_or(0, u32::from)
        );
    }
}

fn is_forbidden(c: char) -> bool {
    matches!(
        c,
        '\u{0000}'..='\u{001F}'
            | '\u{007F}'..='\u{009F}'
            | '\u{200B}'
            | '\u{200C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

#[cfg(test)]
mod tests {
    use gpui_kit::{font, hsla};

    use super::*;

    #[test]
    fn fffd_run_split_and_coloured() {
        let text = "ab\u{FFFD}\u{FFFD}cd";
        let runs = split_fffd_runs(text);
        assert_eq!(
            runs.as_slice(),
            &[(0..2, false), (2..8, true), (8..10, false)]
        );
        assert_eq!(&text[runs[1].0.clone()], "\u{FFFD}\u{FFFD}");

        let normal = hsla(0.0, 0.0, 0.9, 1.0);
        let warning = hsla(0.13, 1.0, 0.48, 1.0);
        let shared = SharedString::new(text);
        let text_runs = name_runs(&shared, &font("Noto Sans"), normal, warning);
        let colours: Vec<Hsla> = text_runs.iter().map(|r| r.color).collect();
        assert_eq!(colours, [normal, warning, normal]);
        let lens: Vec<usize> = text_runs.iter().map(|r| r.len).collect();
        assert_eq!(lens, [2, 6, 2]);
    }

    #[test]
    fn split_edge_cases() {
        assert!(split_fffd_runs("").is_empty());
        assert_eq!(split_fffd_runs("abc").as_slice(), &[(0..3, false)]);
        assert_eq!(split_fffd_runs("\u{FFFD}").as_slice(), &[(0..3, true)]);
        assert_eq!(
            split_fffd_runs("\u{FFFD}a\u{FFFD}").as_slice(),
            &[(0..3, true), (3..4, false), (4..7, true)]
        );
    }

    #[test]
    fn empty_name_falls_back_to_comm_then_pid() {
        let mut scratch = String::new();
        assert_eq!(display_name("", "bash", 1, &mut scratch), "bash");
        assert_eq!(
            display_name("\u{FFFD}", "", 4242, &mut scratch),
            "[pid 4242]"
        );
        assert_eq!(
            display_name("", "\u{FFFD}\u{FFFD}", 7, &mut scratch),
            "[pid 7]"
        );
        assert_eq!(display_name("zsh", "bash", 1, &mut scratch), "zsh");
        assert_eq!(
            display_name("a\u{FFFD}", "bash", 1, &mut scratch),
            "a\u{FFFD}"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_check_rejects_bidi_allows_zwj() {
        // Family emoji: three people joined by U+200D.
        debug_assert_clean("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}");
        // Right-to-left letters and U+FFFD are allowed.
        debug_assert_clean("\u{05E9}\u{05DC}\u{05D5}\u{05DD} \u{FFFD}");
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "U+202E")]
    fn debug_check_rejects_bidi_allows_zwj_panics_on_rlo() {
        debug_assert_clean("evil\u{202E}gpj.exe");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_check_rejects_every_forbidden_char() {
        let forbidden = [
            '\u{0000}', '\u{0009}', '\u{000A}', '\u{000C}', '\u{001F}', '\u{007F}', '\u{0080}',
            '\u{009F}', '\u{200B}', '\u{200C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}',
            '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
            '\u{2028}', '\u{2029}',
        ];
        for c in forbidden {
            let text = format!("a{c}b");
            let result = std::panic::catch_unwind(|| debug_assert_clean(&text));
            assert!(result.is_err(), "U+{:04X} was not rejected", c as u32);
        }
        for c in [' ', '\u{00A0}', '\u{200D}', '\u{2026}', '\u{FFFD}'] {
            debug_assert_clean(&format!("a{c}b"));
        }
    }
}
