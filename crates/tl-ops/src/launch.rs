//! Program launch and Desktop Entry Exec parsing.
//!
//! `parse_exec` turns a desktop entry's `Exec` value into an argument vector as the Desktop Entry
//! Specification 1.5 says ("Possible value types" and "The Exec key"). No shell is involved:
//! `$`, `;`, `|`, `&` and `` ` `` reach the program as literal characters.

use crate::error::{OpsError, OpsResult};

/// The entry values that the `%i`, `%c` and `%k` field codes expand to.
pub struct ExecContext<'a> {
    /// The entry's Name, already chosen by the caller.
    pub name: &'a str,
    /// The entry's Icon value, raw.
    pub icon: Option<&'a str>,
    /// Absolute path of the .desktop file.
    pub desktop_file: Option<&'a str>,
}

/// One argument after splitting, before field codes are applied.
struct Word {
    text: String,
    quoted: bool,
}

/// Parses the value that follows `Exec=` into an argument vector.
///
/// Never log `raw` or the result: they are command lines (ADR-015).
pub fn parse_exec(raw: &str, ctx: &ExecContext<'_>) -> OpsResult<Vec<String>> {
    if raw.contains('\0') {
        return Err(OpsError::InvalidInput("Exec contains a NUL byte"));
    }
    let value = unescape_string_value(raw);
    let words = split_words(&value)?;

    let mut args = Vec::with_capacity(words.len());
    for word in words {
        if word.quoted {
            args.push(word.text);
            continue;
        }
        match word.text.as_str() {
            "%f" | "%F" | "%u" | "%U" => {}
            "%i" => {
                if let Some(icon) = ctx.icon.filter(|i| !i.is_empty()) {
                    args.push("--icon".to_owned());
                    args.push(icon.to_owned());
                }
            }
            text => {
                let expanded = expand_codes(text, ctx)?;
                if !expanded.is_empty() {
                    args.push(expanded);
                }
            }
        }
    }

    if args.is_empty() {
        return Err(OpsError::InvalidInput("Exec is empty"));
    }
    Ok(args)
}

/// Applies the spec "string" escapes: `\s`, `\n`, `\t`, `\r` and `\\`. Any other `\x` pair is
/// kept as both characters.
pub(crate) fn unescape_string_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Splits on runs of spaces outside quotes. Inside a quoted argument `\"`, `` \` ``, `\$` and
/// `\\` give the second character; any other backslash is literal.
fn split_words(value: &str) -> OpsResult<Vec<Word>> {
    let mut words = Vec::new();
    let mut chars = value.chars().peekable();
    loop {
        while chars.next_if_eq(&' ').is_some() {}
        let Some(&first) = chars.peek() else {
            break;
        };

        if first == '"' {
            chars.next();
            let mut text = String::new();
            loop {
                match chars.next() {
                    None => return Err(OpsError::InvalidInput("unterminated quote in Exec")),
                    Some('"') => break,
                    Some('\\') => match chars.peek() {
                        Some(&e @ ('"' | '`' | '$' | '\\')) => {
                            chars.next();
                            text.push(e);
                        }
                        _ => text.push('\\'),
                    },
                    Some(c) => text.push(c),
                }
            }
            // The spec requires a quoted argument to be quoted as a whole.
            if chars.peek().is_some_and(|&c| c != ' ') {
                return Err(OpsError::InvalidInput(
                    "quote inside an unquoted Exec argument",
                ));
            }
            words.push(Word { text, quoted: true });
        } else {
            let mut text = String::new();
            while let Some(c) = chars.next_if(|&c| c != ' ') {
                if c == '"' {
                    return Err(OpsError::InvalidInput(
                        "quote inside an unquoted Exec argument",
                    ));
                }
                text.push(c);
            }
            words.push(Word {
                text,
                quoted: false,
            });
        }
    }
    Ok(words)
}

/// Expands the field codes that may appear inside an unquoted argument: `%c`, `%k`, `%%` and the
/// deprecated codes, which are deleted. The standalone codes (`%f %F %u %U %i`) are handled by
/// the caller, so inside a longer argument they are invalid.
fn expand_codes(text: &str, ctx: &ExecContext<'_>) -> OpsResult<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('c') => out.push_str(ctx.name),
            Some('k') => out.push_str(ctx.desktop_file.unwrap_or("")),
            Some('%') => out.push('%'),
            Some('d' | 'D' | 'n' | 'N' | 'v' | 'm') => {}
            _ => return Err(OpsError::InvalidInput("invalid field code in Exec")),
        }
    }
    Ok(out)
}
