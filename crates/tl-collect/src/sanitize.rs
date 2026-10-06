//! Sanitizer for untrusted process text (ADR-015).
//!
//! Every string read from `/proc/<pid>` passes through here before it enters `ProcessMeta`, so
//! the UI, logs and copy text never see raw bytes. One pass, no regex, and no allocation when
//! `out` already has capacity for the result.

/// Read limit for `/proc/<pid>/comm`, in bytes.
pub const COMM_MAX: usize = 64;
/// Read limit for `/proc/<pid>/cmdline`, in bytes.
pub const CMDLINE_MAX: usize = 32 * 1024;
/// Read limit for the `/proc/<pid>/exe` link target, in bytes.
pub const EXE_LINK_MAX: usize = 4096;
/// Read limit for `/proc/<pid>/status`, in bytes.
pub const STATUS_MAX: usize = 8 * 1024;
/// Appended when the input was cut at its limit.
pub const ELLIPSIS: char = '\u{2026}';

/// Sanitizes `raw` into `out`, which is cleared first.
///
/// Uses at most `max` bytes of `raw` and appends [`ELLIPSIS`] when it cut. Invalid UTF-8 becomes
/// U+FFFD exactly as `String::from_utf8_lossy` does. `\n`, `\r`, NUL, U+2028 (line separator) and
/// U+2029 (paragraph separator) become a space. Other C0 controls, DEL and C1 controls become
/// U+FFFD. Bidirectional controls and zero-width characters are removed, except U+200D (zero-width
/// joiner), which is kept so emoji sequences stay whole (UI-FND-053). Trailing spaces are trimmed.
pub fn sanitize_text(raw: &[u8], max: usize, out: &mut String) {
    out.clear();
    let truncated = raw.len() > max;
    let raw = if truncated { &raw[..max] } else { raw };
    // `utf8_chunks` is what `from_utf8_lossy` uses inside: one U+FFFD per invalid sequence, and a
    // multi-byte sequence cut by the limit becomes U+FFFD. Unlike `from_utf8_lossy` it never
    // allocates.
    for chunk in raw.utf8_chunks() {
        for c in chunk.valid().chars() {
            push_clean(c, out);
        }
        if !chunk.invalid().is_empty() {
            out.push(char::REPLACEMENT_CHARACTER);
        }
    }
    let kept = out.trim_end_matches(' ').len();
    out.truncate(kept);
    if truncated {
        out.push(ELLIPSIS);
    }
}

/// Sanitizes `/proc/<pid>/cmdline`: drops one trailing NUL, then NUL separators become spaces.
pub fn sanitize_cmdline(raw: &[u8], out: &mut String) {
    let raw = raw.strip_suffix(b"\0").unwrap_or(raw);
    sanitize_text(raw, CMDLINE_MAX, out);
}

/// Sanitizes `/proc/<pid>/comm`: drops one trailing `\n`.
pub fn sanitize_comm(raw: &[u8], out: &mut String) {
    let raw = raw.strip_suffix(b"\n").unwrap_or(raw);
    sanitize_text(raw, COMM_MAX, out);
}

/// Sanitizes a link target such as `/proc/<pid>/exe`. A ` (deleted)` suffix stays as text
/// (PROC-027).
pub fn sanitize_link(raw: &[u8], out: &mut String) {
    sanitize_text(raw, EXE_LINK_MAX, out);
}

fn push_clean(c: char, out: &mut String) {
    match c {
        '\n' | '\r' | '\0' | '\u{2028}' | '\u{2029}' => out.push(' '),
        '\u{0001}'..='\u{001F}' | '\u{007F}'..='\u{009F}' => out.push(char::REPLACEMENT_CHARACTER),
        '\u{200B}'
        | '\u{200C}'
        | '\u{200E}'
        | '\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' => {}
        _ => out.push(c),
    }
}
