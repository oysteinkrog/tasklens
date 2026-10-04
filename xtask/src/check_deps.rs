//! `cargo xtask check-deps`: dependency and API rules from ADR-001 and ADR-004.
//!
//! Check 1 finds banned crates in the Linux normal graph of the shipped binaries. Check 3 finds
//! `asupersync::` paths outside tl-engine. Check 4 finds banned asupersync APIs in tl-engine
//! production code. Every check runs before the command fails, so one run lists every violation.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, bail};

/// Package names of the shipped binaries. `tl-app` builds the `tasklens` binary (R-158).
pub const SHIPPED_BINARIES: &[&str] = &["tl-app", "tl-gpu-probe", "tl-helper", "tl-netmon"];

/// Crates that must not appear in the normal graph of a shipped binary (ADR-001 "No Tokio").
pub const BANNED_CRATES: &[&str] = &[
    "tokio",
    "tokio-util",
    "tokio-stream",
    "tokio-macros",
    "async-std",
    "async-global-executor",
    "actix-rt",
    "reqwest",
    "hyper",
];

pub const TARGET: &str = "x86_64-unknown-linux-gnu";

/// ADR-004 banned names, matched as whole words.
const BANNED_WORDS: &[&str] = &["AppSpec", "GenServer", "actor", "race_drained", "ctrl_c"];

/// ADR-004 banned names, matched as literal text.
const BANNED_LITERALS: &[&str] = &[
    "race!",
    "select!",
    "Cx::race_timeout",
    "asupersync::signal",
    "asupersync::process",
    "asupersync::fs",
    "Cx::for_request",
    "Cx::for_testing",
    "ChildSpec::depends_on",
];

/// ADR-004 names that are banned only in some uses. A match is accepted when the same line
/// carries `// check-deps: allow <reason>`.
const ALLOWABLE_LITERALS: &[&str] = &["RuntimeHandle::spawn(", ".with_poll_quota("];

const ALLOW_COMMENT: &str = "// check-deps: allow";

pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    if !args.is_empty() {
        bail!("check-deps takes no arguments, got: {}", args.join(" "));
    }
    let root = workspace_root();
    let checks: [(&str, fn(&Path) -> anyhow::Result<Vec<String>>); 3] = [
        ("banned crates", check_banned_crates),
        ("asupersync outside tl-engine", check_asupersync_outside_engine),
        ("banned asupersync APIs", check_banned_apis),
    ];

    let mut violations = Vec::new();
    let mut errors = Vec::new();
    for (name, check) in checks {
        match check(&root) {
            Ok(found) => violations.extend(found),
            Err(err) => errors.push(format!("check {name} failed: {err:#}")),
        }
    }

    for violation in &violations {
        println!("{violation}");
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
    if !violations.is_empty() {
        bail!("check-deps: {} violation(s)", violations.len());
    }
    println!("check-deps: ok");
    Ok(())
}

/// The workspace root is the parent of the xtask package directory.
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap_or(manifest_dir).to_path_buf()
}

/// Runs a command in `root` and returns its output, or an error when it cannot start.
fn run_command(root: &Path, program: &str, args: &[&str]) -> anyhow::Result<Output> {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("could not run `{}`", command_line(program, args)))
}

fn command_line(program: &str, args: &[&str]) -> String {
    std::iter::once(program).chain(args.iter().copied()).collect::<Vec<_>>().join(" ")
}

fn command_failed(program: &str, args: &[&str], output: &Output) -> anyhow::Error {
    anyhow::anyhow!(
        "`{}` exited with {}: {}",
        command_line(program, args),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

/// Check 1. For each banned crate, inverts the Linux normal graph of the workspace and reports
/// every shipped binary that reaches it.
fn check_banned_crates(root: &Path) -> anyhow::Result<Vec<String>> {
    let shipped = existing_shipped_binaries(root)?;
    let mut violations = Vec::new();
    for banned in BANNED_CRATES {
        let mut specs = vec![(*banned).to_owned()];
        while let Some(spec) = specs.pop() {
            let args = [
                "tree",
                "--workspace",
                "-e",
                "normal",
                "--target",
                TARGET,
                "-i",
                spec.as_str(),
                "--prefix",
                "none",
                "--format",
                "{p}",
            ];
            let output = run_command(root, "cargo", &args)?;
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !output.status.success() {
                if stderr.contains("did not match any packages") {
                    // Not in Cargo.lock at all.
                    continue;
                }
                if stderr.contains("is ambiguous") {
                    // Several versions are locked. Query each one.
                    specs.extend(ambiguous_specs(&stderr, banned));
                    continue;
                }
                return Err(command_failed("cargo", &args, &output));
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            for package in shipped_in_tree(&stdout, &shipped) {
                let violation =
                    format!("banned crate {banned} in the dependency graph of {package}");
                if !violations.contains(&violation) {
                    violations.push(violation);
                }
            }
        }
    }
    Ok(violations)
}

/// Shipped binary packages that are workspace members now. P3 and P4 packages are skipped until
/// they exist.
fn existing_shipped_binaries(root: &Path) -> anyhow::Result<Vec<&'static str>> {
    let args = ["metadata", "--no-deps", "--format-version", "1"];
    let output = run_command(root, "cargo", &args)?;
    if !output.status.success() {
        return Err(command_failed("cargo", &args, &output));
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("cargo metadata printed invalid JSON")?;
    let names: Vec<&str> = metadata["packages"]
        .as_array()
        .context("cargo metadata has no packages array")?
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();
    Ok(SHIPPED_BINARIES.iter().copied().filter(|name| names.contains(name)).collect())
}

/// The `name@version` specs cargo lists after an "is ambiguous" error for `name`.
fn ambiguous_specs(stderr: &str, name: &str) -> Vec<String> {
    let prefix = format!("{name}@");
    stderr
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(&prefix))
        .map(str::to_owned)
        .collect()
}

/// Shipped binaries named as the first word of a line of `cargo tree --prefix none` output.
fn shipped_in_tree<'a>(tree: &str, shipped: &[&'a str]) -> Vec<&'a str> {
    let mut found = Vec::new();
    for line in tree.lines() {
        if let Some(package) = shipped.iter().find(|name| first_word_is(line, name))
            && !found.contains(package)
        {
            found.push(*package);
        }
    }
    found
}

/// True when the first whitespace-separated word of `line` is exactly `word`.
fn first_word_is(line: &str, word: &str) -> bool {
    line.split_whitespace().next() == Some(word)
}

/// Check 3. Any `asupersync::` path under `crates/` outside tl-engine.
fn check_asupersync_outside_engine(root: &Path) -> anyhow::Result<Vec<String>> {
    let args = ["-n", "asupersync::", "crates/", "--glob", "!crates/tl-engine/**"];
    let output = run_command(root, "rg", &args)?;
    match output.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| format!("asupersync used outside tl-engine: {line}"))
            .collect()),
        Some(1) => Ok(Vec::new()),
        _ => Err(command_failed("rg", &args, &output)),
    }
}

/// Check 4. Banned asupersync APIs in `crates/tl-engine/src`, outside `#[cfg(test)]` modules.
fn check_banned_apis(root: &Path) -> anyhow::Result<Vec<String>> {
    let src = root.join("crates/tl-engine/src");
    let mut files = Vec::new();
    if src.is_dir() {
        collect_rs_files(&src, &mut files)?;
    }
    files.sort();

    let mut violations = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("could not read {}", file.display()))?;
        let relative = file.strip_prefix(root).unwrap_or(&file);
        let display = relative.to_string_lossy().replace('\\', "/");
        for hit in scan_banned_apis(&text) {
            violations.push(format!(
                "banned asupersync API {} at {display}:{}",
                hit.pattern, hit.line
            ));
        }
    }
    Ok(violations)
}

fn collect_rs_files(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("could not list {}", dir.display()))?;
    for entry in entries {
        let path = entry.with_context(|| format!("could not list {}", dir.display()))?.path();
        if path.is_dir() {
            collect_rs_files(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// One banned API match: the pattern and its 1-based line number.
#[derive(Debug, PartialEq, Eq)]
struct ApiHit {
    pattern: &'static str,
    line: usize,
}

/// Finds banned API names in one source file, skipping `#[cfg(test)]` modules.
fn scan_banned_apis(source: &str) -> Vec<ApiHit> {
    let production = blank_cfg_test_modules(source);
    let mut hits = Vec::new();
    for (index, line) in production.lines().enumerate() {
        let line_number = index + 1;
        for &word in BANNED_WORDS {
            if contains_word(line, word) {
                hits.push(ApiHit { pattern: word, line: line_number });
            }
        }
        for &literal in BANNED_LITERALS {
            if line.contains(literal) {
                hits.push(ApiHit { pattern: literal, line: line_number });
            }
        }
        for &literal in ALLOWABLE_LITERALS {
            if line.contains(literal) && !has_allow_comment(line) {
                hits.push(ApiHit { pattern: literal, line: line_number });
            }
        }
    }
    hits
}

/// True when `line` carries `// check-deps: allow` followed by a non-empty reason.
fn has_allow_comment(line: &str) -> bool {
    line.find(ALLOW_COMMENT)
        .is_some_and(|start| !line[start + ALLOW_COMMENT.len()..].trim().is_empty())
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// True when `word` occurs in `line` with no identifier character on either side.
fn contains_word(line: &str, word: &str) -> bool {
    line.match_indices(word).any(|(start, _)| {
        let before = line[..start].chars().next_back();
        let after = line[start + word.len()..].chars().next();
        !before.is_some_and(is_ident_char) && !after.is_some_and(is_ident_char)
    })
}

/// Replaces every `#[cfg(test)]` module, from the attribute to its closing brace, with spaces.
/// Newlines stay, so line numbers do not change. The module is found by brace counting from the
/// `mod <name> {` that follows the attribute (other attributes may sit in between).
fn blank_cfg_test_modules(source: &str) -> String {
    const ATTRIBUTE: &str = "#[cfg(test)]";
    let bytes = source.as_bytes();
    let mut blank = vec![false; bytes.len()];
    let mut search_from = 0;
    while let Some(found) = source[search_from..].find(ATTRIBUTE) {
        let start = search_from + found;
        search_from = start + ATTRIBUTE.len();
        let Some(open) = cfg_test_mod_open_brace(source, search_from) else {
            continue;
        };
        let Some(close) = matching_close_brace(bytes, open) else {
            continue;
        };
        blank[start..=close].iter_mut().for_each(|b| *b = true);
        search_from = close + 1;
    }
    source
        .char_indices()
        .map(|(i, c)| if blank[i] && c != '\n' { ' ' } else { c })
        .collect()
}

/// The byte index of the `{` of a `[pub] mod <name> {` item starting at `from`, after optional
/// whitespace and attributes. `None` when the attribute is on something else.
fn cfg_test_mod_open_brace(source: &str, from: usize) -> Option<usize> {
    let mut rest = &source[from..];
    loop {
        rest = rest.trim_start();
        if rest.starts_with("#[") {
            let end = rest.find(']')?;
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    if let Some(after_pub) = rest.strip_prefix("pub") {
        rest = after_pub.trim_start();
        if rest.starts_with('(') {
            let end = rest.find(')')?;
            rest = &rest[end + 1..];
        }
        rest = rest.trim_start();
    }
    let after_mod = rest.strip_prefix("mod")?;
    if !after_mod.starts_with(char::is_whitespace) {
        return None;
    }
    let after_mod = after_mod.trim_start();
    let name_len = after_mod.find(|c: char| !is_ident_char(c)).unwrap_or(after_mod.len());
    if name_len == 0 {
        return None;
    }
    let after_name = after_mod[name_len..].trim_start();
    if !after_name.starts_with('{') {
        // `mod tests;` puts the module in another file. That file is still scanned.
        return None;
    }
    Some(source.len() - after_name.len())
}

/// The byte index of the `}` that closes the `{` at `open`.
fn matching_close_brace(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, &b) in bytes[open..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banned_word_matches_first_word_only() {
        assert!(first_word_is("tokio v1.40.0", "tokio"));
        assert!(!first_word_is("tokio-rustls v0.26", "tokio"));
        assert!(!first_word_is("hyper-util v0.1", "hyper"));
    }

    #[test]
    fn shipped_in_tree_finds_binaries_once() {
        let tree = "tokio v1.40.0\ntl-ui v0.1.0 (/w/crates/tl-ui)\ntl-app v0.1.0 (/w/crates/tl-app)\n\
                    tl-app v0.1.0 (/w/crates/tl-app) (*)\n";
        assert_eq!(shipped_in_tree(tree, &["tl-app", "tl-gpu-probe"]), vec!["tl-app"]);
    }

    #[test]
    fn ambiguous_specs_reads_cargo_help() {
        let stderr = "error: specification `aes` is ambiguous\n\
                      help: re-run this command with one of the following specifications\n  \
                      aes@0.8.4\n  aes@0.9.3\n";
        assert_eq!(ambiguous_specs(stderr, "aes"), vec!["aes@0.8.4", "aes@0.9.3"]);
    }

    #[test]
    fn api_scan_skips_cfg_test_module() {
        let inside = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn f() {\n        select!();\n    }\n}\n";
        assert_eq!(scan_banned_apis(inside), Vec::new());

        let one_line = "#[cfg(test)] mod tests { fn f() { select!(); } }\n";
        assert_eq!(scan_banned_apis(one_line), Vec::new());

        let outside = "fn a() {}\n\nfn f() {\n    select!();\n}\n";
        assert_eq!(scan_banned_apis(outside), vec![ApiHit { pattern: "select!", line: 4 }]);

        let after = "#[cfg(test)]\nmod tests {\n    fn f() {}\n}\nfn g() { select!(); }\n";
        assert_eq!(scan_banned_apis(after), vec![ApiHit { pattern: "select!", line: 5 }]);
    }

    #[test]
    fn api_scan_keeps_cfg_test_items_that_are_not_modules() {
        let source = "#[cfg(test)]\nfn helper() { select!(); }\n";
        assert_eq!(scan_banned_apis(source), vec![ApiHit { pattern: "select!", line: 2 }]);
    }

    #[test]
    fn api_scan_whole_word() {
        assert_eq!(scan_banned_apis("let actor = 1;"), vec![ApiHit { pattern: "actor", line: 1 }]);
        assert_eq!(scan_banned_apis("let factory = 1;"), Vec::new());
        assert_eq!(scan_banned_apis("let actors_count = 1;"), Vec::new());
    }

    #[test]
    fn allow_comment_accepts_poll_quota() {
        let allowed = "budget.with_poll_quota(8) // check-deps: allow short probe";
        assert_eq!(scan_banned_apis(allowed), Vec::new());

        let bare = "budget.with_poll_quota(8)";
        assert_eq!(
            scan_banned_apis(bare),
            vec![ApiHit { pattern: ".with_poll_quota(", line: 1 }]
        );

        let no_reason = "budget.with_poll_quota(8) // check-deps: allow";
        assert_eq!(scan_banned_apis(no_reason).len(), 1);
    }
}
