//! Tests for the process text sanitizer (ADR-015). These are security regression tests: keep them.

use proptest::prelude::*;
use tl_collect::sanitize::{
    CMDLINE_MAX, ELLIPSIS, sanitize_cmdline, sanitize_comm, sanitize_link, sanitize_text,
};

fn is_removed(c: char) -> bool {
    matches!(
        c,
        '\u{200B}' | '\u{200C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// Copy of `is_forbidden` in `crates/tl-ui/src/text/untrusted.rs`, which `debug_assert_clean`
/// uses. tl-collect must not depend on tl-ui, so keep the two in step by hand.
fn ui_forbidden(c: char) -> bool {
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

fn is_control(c: char) -> bool {
    matches!(c, '\u{0000}'..='\u{001F}' | '\u{007F}'..='\u{009F}')
}

fn text(raw: &[u8], max: usize) -> String {
    let mut out = String::new();
    sanitize_text(raw, max, &mut out);
    out
}

#[test]
fn cmdline_nul_to_space() {
    let mut out = String::new();
    sanitize_cmdline(b"/usr/bin/foo\0--bar\0baz\0", &mut out);
    assert_eq!(out, "/usr/bin/foo --bar baz");

    sanitize_cmdline(b"", &mut out);
    assert_eq!(out, "");
    sanitize_cmdline(b"\0\0\0", &mut out);
    assert_eq!(out, "");
}

#[test]
fn control_chars_replaced() {
    let mut out = String::new();
    sanitize_comm(b"evil\x1b[31m\n", &mut out);
    assert_eq!(out, "evil\u{FFFD}[31m");

    assert_eq!(text(b"a\tb", 64), "a\u{FFFD}b");
    assert_eq!(text(b"a\x01b\x1fc", 64), "a\u{FFFD}b\u{FFFD}c");
    assert_eq!(text(b"a\x7fb", 64), "a\u{FFFD}b");
    assert_eq!(
        text("a\u{0080}b\u{0085}c\u{009F}d".as_bytes(), 64),
        "a\u{FFFD}b\u{FFFD}c\u{FFFD}d"
    );
    assert_eq!(text(b"a\nb\rc\0d", 64), "a b c d");
    assert_eq!(text(b"line\n\n", 64), "line");
}

#[test]
fn bidi_and_zero_width_removed() {
    let removed = ('\u{202A}'..='\u{202E}')
        .chain('\u{2066}'..='\u{2069}')
        .chain(['\u{200B}', '\u{200C}', '\u{200E}', '\u{200F}']);
    for c in removed {
        let raw = format!("a{c}b");
        let out = text(raw.as_bytes(), 64);
        assert_eq!(out, "ab", "U+{:04X} was not removed", c as u32);
    }
    let out = text("x\u{202E}y\u{200B}z".as_bytes(), 64);
    assert!(!out.contains('\u{202E}'));
    assert!(!out.contains('\u{200B}'));
    assert_eq!(out, "xyz");
}

#[test]
fn zwj_kept() {
    assert_eq!(text("a\u{200D}b".as_bytes(), 64), "a\u{200D}b");
    // Family emoji: three people joined by U+200D stays one sequence.
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    assert_eq!(text(family.as_bytes(), 64), family);
}

#[test]
fn line_and_paragraph_separators_removed() {
    let out = text("a\u{2028}b\u{2029}c".as_bytes(), 64);
    assert!(!out.contains('\u{2028}'));
    assert!(!out.contains('\u{2029}'));
    // They are line breaks, so they become a space like `\n`.
    assert_eq!(out, "a b c");
    assert_eq!(text("name\u{2029}".as_bytes(), 64), "name");
}

#[test]
fn invalid_utf8_lossy() {
    assert_eq!(text(b"a\xff\xfeb", 64), "a\u{FFFD}\u{FFFD}b");
    // A multi-byte sequence cut by the limit becomes U+FFFD, then the ellipsis.
    assert_eq!(
        text("ab\u{00E9}".as_bytes(), 3),
        format!("ab\u{FFFD}{ELLIPSIS}")
    );
    // Same result as from_utf8_lossy for valid characters.
    let raw = b"caf\xc3\xa9 \xe2\x82 x";
    assert_eq!(text(raw, 64), String::from_utf8_lossy(raw));
}

#[test]
fn cmdline_1mib_truncated_to_32kib() {
    let raw = vec![b'x'; 1024 * 1024];
    let mut out = String::new();
    sanitize_cmdline(&raw, &mut out);
    let expected: String = std::iter::repeat_n('x', CMDLINE_MAX)
        .chain([ELLIPSIS])
        .collect();
    assert_eq!(out.chars().count(), 32_768 + 1);
    assert_eq!(out, expected);
}

#[test]
fn exact_limit_not_marked() {
    let raw = vec![b'x'; CMDLINE_MAX];
    let mut out = String::new();
    sanitize_cmdline(&raw, &mut out);
    assert_eq!(out.len(), 32_768);
    assert!(!out.contains(ELLIPSIS));
}

#[test]
fn link_deleted_suffix_kept() {
    let mut out = String::new();
    sanitize_link(b"/usr/bin/foo (deleted)", &mut out);
    assert_eq!(out, "/usr/bin/foo (deleted)");
}

#[test]
fn reuses_buffer() {
    let mut out = String::with_capacity(64 * 1024);
    let before = out.capacity();
    let raw: Vec<u8> = (0..32 * 1024)
        .map(|i| if i % 7 == 0 { 0 } else { b'a' + (i % 26) as u8 })
        .collect();
    sanitize_cmdline(&raw, &mut out);
    assert_eq!(out.capacity(), before);
    sanitize_text(b"a\xff\xfeb", CMDLINE_MAX, &mut out);
    assert_eq!(out.capacity(), before);
}

proptest! {
    #[test]
    fn proptest_output_clean(
        raw in proptest::collection::vec(any::<u8>(), 0..=70_000),
        max in prop_oneof![Just(64usize), Just(4096), Just(8 * 1024), Just(CMDLINE_MAX)],
    ) {
        let mut out = String::new();
        sanitize_text(&raw, max, &mut out);
        for c in out.chars() {
            prop_assert!(!is_removed(c), "removed char U+{:04X} in output", c as u32);
            prop_assert!(!is_control(c), "control char U+{:04X} in output", c as u32);
        }
        prop_assert!(out.chars().count() <= max + 1);
        let truncated = raw.len() > max;
        prop_assert_eq!(out.ends_with(ELLIPSIS) || !truncated, true);
        let body = if truncated { out.strip_suffix(ELLIPSIS).unwrap_or(&out) } else { &out };
        prop_assert!(!body.ends_with(' '));
    }

    /// Output passes the tl-ui display check. Random bytes rarely form the characters near the
    /// edges of the rule, so this draws from a mix of plain text and those characters.
    #[test]
    fn proptest_output_passes_ui_check(
        chars in proptest::collection::vec(
            prop_oneof![
                any::<char>(),
                prop::sample::select(vec![
                    '\0', '\t', '\n', '\r', '\u{1B}', '\u{7F}', '\u{85}', '\u{9F}',
                    '\u{200B}', '\u{200C}', '\u{200D}', '\u{200E}', '\u{200F}',
                    '\u{2028}', '\u{2029}', '\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}',
                    '\u{FFFD}', '\u{1F468}', 'a',
                ]),
            ],
            0..=200,
        ),
        max in prop_oneof![Just(16usize), Just(64), Just(4096)],
    ) {
        let raw: String = chars.into_iter().collect();
        let mut out = String::new();
        sanitize_text(raw.as_bytes(), max, &mut out);
        for c in out.chars() {
            prop_assert!(!ui_forbidden(c), "forbidden char U+{:04X} in output", c as u32);
        }
        // U+200D is the one zero-width character that survives.
        if raw.len() <= max {
            let joiners_in = raw.chars().filter(|&c| c == '\u{200D}').count();
            let joiners_out = out.chars().filter(|&c| c == '\u{200D}').count();
            prop_assert_eq!(joiners_in, joiners_out);
        }
    }
}
