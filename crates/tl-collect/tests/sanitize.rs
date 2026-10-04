//! Tests for the process text sanitizer (ADR-015). These are security regression tests: keep them.

use proptest::prelude::*;
use tl_collect::sanitize::{
    CMDLINE_MAX, ELLIPSIS, sanitize_cmdline, sanitize_comm, sanitize_link, sanitize_text,
};

fn is_removed(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
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
    assert_eq!(text("a\u{0080}b\u{0085}c\u{009F}d".as_bytes(), 64), "a\u{FFFD}b\u{FFFD}c\u{FFFD}d");
    assert_eq!(text(b"a\nb\rc\0d", 64), "a b c d");
    assert_eq!(text(b"line\n\n", 64), "line");
}

#[test]
fn bidi_and_zero_width_removed() {
    let removed = ('\u{202A}'..='\u{202E}').chain('\u{2066}'..='\u{2069}').chain('\u{200B}'..='\u{200F}');
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
fn invalid_utf8_lossy() {
    assert_eq!(text(b"a\xff\xfeb", 64), "a\u{FFFD}\u{FFFD}b");
    // A multi-byte sequence cut by the limit becomes U+FFFD, then the ellipsis.
    assert_eq!(text("ab\u{00E9}".as_bytes(), 3), format!("ab\u{FFFD}{ELLIPSIS}"));
    // Same result as from_utf8_lossy for valid characters.
    let raw = b"caf\xc3\xa9 \xe2\x82 x";
    assert_eq!(text(raw, 64), String::from_utf8_lossy(raw));
}

#[test]
fn cmdline_1mib_truncated_to_32kib() {
    let raw = vec![b'x'; 1024 * 1024];
    let mut out = String::new();
    sanitize_cmdline(&raw, &mut out);
    let expected: String = std::iter::repeat_n('x', CMDLINE_MAX).chain([ELLIPSIS]).collect();
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
    let raw: Vec<u8> = (0..32 * 1024).map(|i| if i % 7 == 0 { 0 } else { b'a' + (i % 26) as u8 }).collect();
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
}
