//! `cargo xtask check-timing`: wall-clock limits under 1 s in test asserts (tasklens-cr0.32).
//!
//! A test may assert ordering, or that something finished. A wall-clock upper limit in a normal
//! test must be at least 2 s or 10 times the measured median, whichever is larger: a limit under
//! 1 s fails when the machine is busy. Real performance budgets belong in `cargo xtask bench`.
//!
//! The check reads every `.rs` file under `crates/*/src` and `crates/*/tests`. It takes each
//! `assert!` statement from `assert!(` to its closing parenthesis, over several lines when
//! needed, and flags it when it holds a timing word and an upper limit under 1 s. A justified
//! case carries `// timing-lint: allow: <reason>` on the line above the assert. Plain string
//! scanning: xtask has no regex crate. Every file is scanned before the command fails, so one
//! run lists every site.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

const ASSERT: &str = "assert!(";

const ALLOW_COMMENT: &str = "// timing-lint: allow:";

/// Duration constructors and the value at which each reaches 1 s.
const CONSTRUCTORS: &[(&str, f64)] = &[
    ("from_millis(", 1e3),
    ("from_micros(", 1e6),
    ("from_nanos(", 1e9),
    ("from_secs_f64(", 1.0),
    ("from_secs_f32(", 1.0),
];

pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    if !args.is_empty() {
        bail!("check-timing takes no arguments, got: {}", args.join(" "));
    }
    let violations = scan_workspace(&workspace_root())?;
    for violation in &violations {
        println!("{violation}");
    }
    if !violations.is_empty() {
        bail!("check-timing: {} violation(s)", violations.len());
    }
    println!("check-timing: ok");
    Ok(())
}

/// The workspace root is the parent of the xtask package directory.
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap_or(manifest_dir).to_path_buf()
}

/// Every flagged assert in `crates/*/src` and `crates/*/tests`, as `file:line` messages.
fn scan_workspace(root: &Path) -> anyhow::Result<Vec<String>> {
    let crates = root.join("crates");
    let entries = std::fs::read_dir(&crates)
        .with_context(|| format!("could not list {}", crates.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let dir = entry
            .with_context(|| format!("could not list {}", crates.display()))?
            .path();
        for sub in ["src", "tests"] {
            let path = dir.join(sub);
            if path.is_dir() {
                collect_rs_files(&path, &mut files)?;
            }
        }
    }
    files.sort();

    let mut violations = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("could not read {}", file.display()))?;
        let relative = file.strip_prefix(root).unwrap_or(&file);
        let display = relative.to_string_lossy().replace('\\', "/");
        for line in scan_source(&text) {
            violations.push(format!(
                "wall-clock limit under 1 s in an assert at {display}:{line}"
            ));
        }
    }
    Ok(violations)
}

/// Collects `.rs` files below `dir`. Skips symlinks and `fixtures` directories: the sysfs
/// fixtures hold symlink loops and no Rust code.
fn collect_rs_files(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("could not list {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("could not list {}", dir.display()))?;
        let kind = entry
            .file_type()
            .with_context(|| format!("could not stat {}", entry.path().display()))?;
        let path = entry.path();
        if kind.is_dir() && entry.file_name() != "fixtures" {
            collect_rs_files(&path, files)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// The 1-based line of each flagged assert in one source file.
fn scan_source(source: &str) -> Vec<usize> {
    let code = blank_comments_and_strings(source);
    let lines: Vec<&str> = source.lines().collect();
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(found) = code[from..].find(ASSERT) {
        let start = from + found;
        let open = start + ASSERT.len() - 1;
        let Some(close) = matching_close_paren(code.as_bytes(), open) else {
            break;
        };
        from = close + 1;
        let body = &code[open + 1..close];
        let line = 1 + code[..start].matches('\n').count();
        if has_timing_word(body) && has_short_upper_limit(body) && !is_allowed(&lines, line) {
            hits.push(line);
        }
    }
    hits
}

/// True when the line above `line` carries the allow comment with a non-empty reason.
fn is_allowed(lines: &[&str], line: usize) -> bool {
    let Some(above) = line.checked_sub(2).and_then(|index| lines.get(index)) else {
        return false;
    };
    above
        .find(ALLOW_COMMENT)
        .is_some_and(|start| !above[start + ALLOW_COMMENT.len()..].trim().is_empty())
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// True when the assert reads a clock: an identifier such as `elapsed`, `took`, `duration_since`,
/// `as_millis` or a name that ends in `_ms`.
fn has_timing_word(body: &str) -> bool {
    body.split(|c: char| !is_ident_char(c)).any(|ident| {
        ident.contains("elapsed")
            || ident.starts_with("took")
            || ident == "duration_since"
            || ident.starts_with("as_millis")
            || ident.starts_with("as_micros")
            || ident.starts_with("as_nanos")
            || ident.starts_with("as_secs_f")
            || ident.ends_with("_ms")
    })
}

/// True when any clause of the assert has an upper limit under 1 s. Clauses are split at `,`,
/// `&&` and `||`, so a message argument or a second condition is a clause of its own.
fn has_short_upper_limit(body: &str) -> bool {
    body.replace("&&", ",")
        .replace("||", ",")
        .split(',')
        .any(clause_has_short_upper_limit)
}

fn clause_has_short_upper_limit(clause: &str) -> bool {
    let Some((op, at)) = find_comparison(clause) else {
        return false;
    };
    let left = &clause[..at];
    let right = &clause[at + op.len()..];
    // The side that holds the upper limit, and the number next to the operator on that side.
    let (limit, adjacent) = match op {
        ">" | ">=" => (left, trailing_number(left)),
        _ => (right, leading_number(right)),
    };
    if CONSTRUCTORS.iter().any(|&(name, one_second)| {
        limit.match_indices(name).any(|(start, _)| {
            leading_number(&limit[start + name.len()..]).is_some_and(|n| n < one_second)
        })
    }) {
        return true;
    }
    match (adjacent, bare_number_unit(clause)) {
        (Some(n), Some(one_second)) => n < one_second,
        _ => false,
    }
}

/// The value of 1 s in the unit a clause measures in, when it compares a bare number with
/// `as_millis()`, `as_micros()`, `as_nanos()`, `as_secs_f64()` or a name ending in `_ms`.
fn bare_number_unit(clause: &str) -> Option<f64> {
    let idents: Vec<&str> = clause.split(|c: char| !is_ident_char(c)).collect();
    if idents
        .iter()
        .any(|i| i.starts_with("as_millis") || i.ends_with("_ms"))
    {
        Some(1e3)
    } else if idents.iter().any(|i| i.starts_with("as_micros")) {
        Some(1e6)
    } else if idents.iter().any(|i| i.starts_with("as_nanos")) {
        Some(1e9)
    } else if idents.iter().any(|i| i.starts_with("as_secs_f")) {
        Some(1.0)
    } else {
        None
    }
}

/// The first comparison or range operator in a clause and its byte offset. Skips `->`, `=>`,
/// shifts and the `<` of a turbofish.
fn find_comparison(clause: &str) -> Option<(&'static str, usize)> {
    let bytes = clause.as_bytes();
    let at = |i: usize| bytes.get(i).copied();
    let mut i = 0;
    while i < bytes.len() {
        let prev = i.checked_sub(1).and_then(at);
        let next = at(i + 1);
        match bytes[i] {
            b'.' if next == Some(b'.') && at(i + 2) == Some(b'=') => return Some(("..=", i)),
            b'.' if next == Some(b'.') && at(i + 2) != Some(b'.') => return Some(("..", i)),
            b'.' if next == Some(b'.') => i += 2,
            b'<' if next == Some(b'<') => i += 1,
            b'<' if prev == Some(b':') => {}
            b'<' if next == Some(b'=') => return Some(("<=", i)),
            b'<' => return Some(("<", i)),
            b'>' if next == Some(b'>') || matches!(prev, Some(b'-' | b'=')) => i += 1,
            b'>' if next == Some(b'=') => return Some((">=", i)),
            b'>' => return Some((">", i)),
            _ => {}
        }
        i += 1;
    }
    None
}

/// The number literal at the start of `text`, after spaces and opening parentheses.
fn leading_number(text: &str) -> Option<f64> {
    let token: String = text
        .trim_start_matches(|c: char| c.is_whitespace() || c == '(')
        .chars()
        .take_while(|&c| is_ident_char(c) || c == '.')
        .collect();
    parse_number(&token)
}

/// The number literal at the end of `text`, before spaces and closing parentheses.
fn trailing_number(text: &str) -> Option<f64> {
    let trimmed = text.trim_end_matches(|c: char| c.is_whitespace() || c == ')');
    let start = trimmed
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_ident_char(c) || c == '.')
        .last()
        .map_or(trimmed.len(), |(index, _)| index);
    parse_number(&trimmed[start..])
}

/// Parses a Rust number literal such as `100`, `1_000`, `100u64` or `0.5`. `None` for a name.
fn parse_number(token: &str) -> Option<f64> {
    if !token.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let digits: String = token
        .chars()
        .filter(|&c| c != '_')
        .take_while(|&c| c.is_ascii_digit() || c == '.')
        .collect();
    digits.parse().ok()
}

/// The byte offset of the `)` that closes the `(` at `open`.
fn matching_close_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, &b) in bytes[open..].iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Replaces comments and the contents of string and char literals with spaces. Newlines stay,
/// so line numbers do not change, and the quotes stay, so a literal still separates tokens.
fn blank_comments_and_strings(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = chars.clone();
    let at = |i: usize| chars.get(i).copied();
    let blank = |out: &mut Vec<char>, from: usize, to: usize| {
        for c in out.iter_mut().take(to.min(chars.len())).skip(from) {
            if *c != '\n' {
                *c = ' ';
            }
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev = i.checked_sub(1).and_then(at);
        if c == '/' && at(i + 1) == Some('/') {
            let end = (i..chars.len())
                .find(|&j| chars[j] == '\n')
                .unwrap_or(chars.len());
            blank(&mut out, i, end);
            i = end;
        } else if c == '/' && at(i + 1) == Some('*') {
            let start = i;
            let mut depth = 0usize;
            while i < chars.len() {
                if chars[i] == '/' && at(i + 1) == Some('*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && at(i + 1) == Some('/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            blank(&mut out, start, i);
        } else if c == '"' {
            let start = i + 1;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            blank(&mut out, start, i);
            i += 1;
        } else if c == 'r' && !prev.is_some_and(|p| is_ident_char(p) && p != 'b') {
            let hashes = chars[i + 1..].iter().take_while(|&&h| h == '#').count();
            if at(i + 1 + hashes) == Some('"') {
                let close: Vec<char> = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                i += 2 + hashes;
                let start = i;
                while i < chars.len() && !chars[i..].starts_with(&close) {
                    i += 1;
                }
                blank(&mut out, start, i);
                i += close.len();
            } else {
                i += 1;
            }
        } else if c == '\'' && at(i + 1) == Some('\\') {
            let start = i + 1;
            i += 2;
            while i < chars.len() && chars[i] != '\'' {
                i += 1;
            }
            blank(&mut out, start, i);
            i += 1;
        } else if c == '\'' && at(i + 2) == Some('\'') {
            blank(&mut out, i + 1, i + 2);
            i += 3;
        } else {
            i += 1;
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{scan_source, scan_workspace, workspace_root};

    fn wrap(body: &str) -> String {
        format!("fn t() {{\n{body}\n}}\n")
    }

    #[test]
    fn flags_single_line() {
        let src = wrap(r#"    assert!(took < Duration::from_millis(100), "{took:?}");"#);
        assert_eq!(scan_source(&src), vec![2]);
    }

    #[test]
    fn flags_multi_line() {
        let src = wrap(
            r#"    assert!(
        begin.elapsed() < Duration::from_millis(100),
        "start took {:?}",
        begin.elapsed()
    );"#,
        );
        assert_eq!(scan_source(&src), vec![2]);
    }

    #[test]
    fn flags_ms_compare() {
        let src = wrap("    assert!(frame_ms <= 50, \"frame took {frame_ms} ms\");");
        assert_eq!(scan_source(&src), vec![2]);
    }

    #[test]
    fn flags_as_millis_compare() {
        let src = wrap("    assert!(start.elapsed().as_millis() < 300);");
        assert_eq!(scan_source(&src), vec![2]);
    }

    #[test]
    fn flags_range_end() {
        let src = wrap(
            "    assert!((Duration::from_millis(180)..=Duration::from_millis(400)).contains(&elapsed));",
        );
        assert_eq!(scan_source(&src), vec![2]);
    }

    #[test]
    fn flags_deadline_plus_short_slack() {
        let src = wrap("    assert!(took < deadline + Duration::from_millis(100), \"{took:?}\");");
        assert_eq!(scan_source(&src), vec![2]);
    }

    #[test]
    fn flags_limit_on_the_left() {
        let src = wrap("    assert!(Duration::from_millis(100) > start.elapsed());");
        assert_eq!(scan_source(&src), vec![2]);
    }

    #[test]
    fn flags_second_condition() {
        let src =
            wrap("    debug_assert!(done && took < Duration::from_micros(500), \"{took:?}\");");
        assert_eq!(scan_source(&src), vec![2]);
    }

    #[test]
    fn passes_lower_bound() {
        let src = wrap("    assert!(elapsed >= Duration::from_millis(200), \"{elapsed:?}\");");
        assert!(scan_source(&src).is_empty());
    }

    #[test]
    fn passes_range_start() {
        let src = wrap(
            "    assert!((Duration::from_millis(180)..=Duration::from_secs(2)).contains(&elapsed));",
        );
        assert!(scan_source(&src).is_empty());
    }

    #[test]
    fn passes_two_seconds() {
        let src = wrap(
            "    assert!(took < Duration::from_secs(2), \"{took:?}\");\n    assert!(took < deadline + Duration::from_secs(2));",
        );
        assert!(scan_source(&src).is_empty());
    }

    #[test]
    fn passes_allowlisted() {
        let src = wrap(
            "    // timing-lint: allow: lab virtual time\n    assert!(elapsed < Duration::from_millis(100));",
        );
        assert!(scan_source(&src).is_empty());
    }

    #[test]
    fn allow_comment_needs_a_reason() {
        let src =
            wrap("    // timing-lint: allow:\n    assert!(elapsed < Duration::from_millis(100));");
        assert_eq!(scan_source(&src), vec![3]);
    }

    #[test]
    fn passes_without_a_timing_word() {
        let src = wrap("    assert!(cfg.interval < Duration::from_millis(500));");
        assert!(scan_source(&src).is_empty());
    }

    #[test]
    fn ignores_comments_and_strings() {
        let src = wrap(
            "    // assert!(took < Duration::from_millis(100));\n    let s = \"assert!(took < Duration::from_millis(100))\";",
        );
        assert!(scan_source(&src).is_empty());
    }

    #[test]
    fn workspace_is_clean() {
        let violations = scan_workspace(&workspace_root()).expect("scan the workspace");
        assert!(violations.is_empty(), "{}", violations.join("\n"));
    }
}
