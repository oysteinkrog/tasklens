//! `cargo xtask check-deps`: dependency and API rules from ADR-001 and ADR-004.
//!
//! Check 1 finds banned crates in the Linux normal graph of the shipped binaries. Check 2 holds
//! each member's direct dependencies to its ADR-001 row and applies the asupersync and tl-ui
//! closure rules. Check 3 finds `asupersync::` paths outside tl-engine. Check 4 finds banned asupersync APIs in tl-engine
//! production code. Every check runs before the command fails, so one run lists every violation.

use std::collections::{BTreeSet, HashMap};
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

/// ADR-001 allowed direct normal and build dependencies per workspace member, with the R-130,
/// R-131, R-132 and R-142 additions. Dev-dependencies are outside the rows (R-142 item 18).
pub const ALLOWED: &[(&str, &[&str])] = &[
    (
        "tl-model",
        &[
            "serde",
            "postcard",
            "thiserror",
            "async-channel",
            "bitflags",
            "smallvec",
            "rustix",
            "rustc-hash",
        ],
    ),
    (
        "tl-collect",
        &[
            "tl-model",
            "rustix",
            "memchr",
            "thiserror",
            "tracing",
            "smallvec",
            "rustc-hash",
            "netlink-packet-core",
            "netlink-packet-sock-diag",
            "netlink-sys",
            "sha2",
            "md-5",
            "bitflags",
        ],
    ),
    (
        "tl-ops",
        &["tl-model", "rustix", "libc", "thiserror", "tracing"],
    ),
    (
        "tl-engine",
        &[
            "tl-model",
            "tl-collect",
            "tl-ops",
            "tl-history",
            "asupersync",
            "arc-swap",
            "async-channel",
            "zbus",
            "futures-lite",
            "rustix",
            "libc",
            "postcard",
            "serde",
            "thiserror",
            "tracing",
            "rustc-hash",
            "smallvec",
            "ureq",
            "serde_json",
            "bitflags",
        ],
    ),
    (
        "tl-gpu-probe",
        &[
            "tl-model",
            "nvml-wrapper",
            "rustix",
            "postcard",
            "serde",
            "ash",
            "khronos-egl",
        ],
    ),
    (
        "tl-ui",
        &[
            "tl-model",
            "gpui-kit",
            "tracing",
            "smallvec",
            "rustc-hash",
            "serde",
            "serde_json",
            "fluent-bundle",
            "unic-langid",
            "async-channel",
            "jiff",
            "unicode-segmentation",
            "image",
        ],
    ),
    (
        "tl-app",
        &[
            "tl-model",
            "tl-engine",
            "tl-ui",
            "tl-ops",
            "tl-tray",
            "zbus",
            "toml",
            "serde",
            "anyhow",
            "tracing",
            "tracing-subscriber",
            "tracing-appender",
            "rustix",
            "clap",
            "async-channel",
            "wgpu",
        ],
    ),
    (
        "tl-history",
        &[
            "tl-model",
            "zstd",
            "crc32fast",
            "postcard",
            "serde",
            "thiserror",
        ],
    ),
    ("tl-tray", &["tl-model", "ksni", "async-channel"]),
    (
        "tl-helper",
        &[
            "tl-model",
            "tl-ops",
            "tl-collect",
            "rustix",
            "postcard",
            "serde",
        ],
    ),
    ("xtask", &["serde", "serde_json", "anyhow"]),
];

/// Members whose normal-dependency closure may contain asupersync (ADR-001 rule 2).
const ASUPERSYNC_ALLOWED: &[&str] = &["tl-engine", "tl-app"];

/// Names that must not be in tl-ui's normal-dependency closure (ADR-001 check 2).
const UI_CLOSURE_BANNED: &[&str] = &["tl-engine", "tl-collect", "tl-ops", "arc-swap", "zbus"];

/// tl-ui closure names that count only on a path that avoids gpui-kit. They reach tl-ui through
/// gpui-kit, and transitive dependencies of an allowed crate do not count (R-158, R-259).
const UI_CLOSURE_VIA_GPUI_KIT_OK: &[&str] = &["arc-swap", "zbus"];

/// ADR-004 banned names, matched as whole words.
const BANNED_WORDS: &[&str] = &["AppSpec", "GenServer", "actor", "race_drained", "ctrl_c"];

/// ADR-004 banned macro calls, matched when no identifier character comes before them, so
/// `tracing::trace!` is not a `race!` match.
const BANNED_MACROS: &[&str] = &["race!"];

/// ADR-004 banned names, matched as literal text.
const BANNED_LITERALS: &[&str] = &[
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

type Check = fn(&Path) -> anyhow::Result<Vec<String>>;

pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    if !args.is_empty() {
        bail!("check-deps takes no arguments, got: {}", args.join(" "));
    }
    let root = workspace_root();
    let checks: [(&str, Check); 4] = [
        ("banned crates", check_banned_crates),
        ("ADR-001 rows and closures", check_rows_and_closures),
        (
            "asupersync outside tl-engine",
            check_asupersync_outside_engine,
        ),
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
    std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
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
    Ok(SHIPPED_BINARIES
        .iter()
        .copied()
        .filter(|name| names.contains(name))
        .collect())
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

/// Check 2. Reads the Linux dependency graph from `cargo metadata` and applies the ADR-001 rows
/// and closure rules.
fn check_rows_and_closures(root: &Path) -> anyhow::Result<Vec<String>> {
    let args = [
        "metadata",
        "--format-version",
        "1",
        "--filter-platform",
        TARGET,
    ];
    let output = run_command(root, "cargo", &args)?;
    if !output.status.success() {
        return Err(command_failed("cargo", &args, &output));
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("cargo metadata printed invalid JSON")?;
    rows_and_closures(&metadata)
}

/// The resolved dependency graph: package names and, per package id, its dependency edges.
struct Graph<'a> {
    names: HashMap<&'a str, &'a str>,
    deps: HashMap<&'a str, Vec<Edge<'a>>>,
}

/// One edge of `resolve.nodes[].deps[]`.
struct Edge<'a> {
    pkg: &'a str,
    normal: bool,
    build: bool,
}

impl<'a> Graph<'a> {
    fn parse(metadata: &'a serde_json::Value) -> anyhow::Result<Self> {
        let mut names = HashMap::new();
        for package in array(metadata, "packages")? {
            names.insert(string(package, "id")?, string(package, "name")?);
        }
        let resolve = metadata
            .get("resolve")
            .filter(|resolve| !resolve.is_null())
            .context("cargo metadata has no resolve field")?;
        let mut deps = HashMap::new();
        for node in array(resolve, "nodes")? {
            let mut edges = Vec::new();
            for dep in array(node, "deps")? {
                let mut normal = false;
                let mut build = false;
                for kind in array(dep, "dep_kinds")? {
                    match kind
                        .get("kind")
                        .context("dep_kinds entry has no kind field")?
                    {
                        serde_json::Value::Null => normal = true,
                        serde_json::Value::String(kind) if kind == "build" => build = true,
                        _ => {}
                    }
                }
                edges.push(Edge {
                    pkg: string(dep, "pkg")?,
                    normal,
                    build,
                });
            }
            deps.insert(string(node, "id")?, edges);
        }
        Ok(Self { names, deps })
    }

    fn name(&self, id: &'a str) -> &'a str {
        self.names.get(id).copied().unwrap_or(id)
    }

    /// Names of the packages reachable from `start` over normal edges, not counting `start`.
    /// Packages named in `skip` are not entered, so paths through them are not followed.
    fn closure(&self, start: &'a str, skip: &[&str]) -> BTreeSet<&'a str> {
        let mut seen = BTreeSet::from([start]);
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            for edge in self.deps.get(id).into_iter().flatten() {
                if edge.normal && !skip.contains(&self.name(edge.pkg)) && seen.insert(edge.pkg) {
                    stack.push(edge.pkg);
                }
            }
        }
        seen.remove(start);
        seen.into_iter().map(|id| self.name(id)).collect()
    }
}

fn array<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> anyhow::Result<&'a Vec<serde_json::Value>> {
    value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .with_context(|| format!("cargo metadata has no {field} array"))
}

fn string<'a>(value: &'a serde_json::Value, field: &str) -> anyhow::Result<&'a str> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .with_context(|| format!("cargo metadata has no {field} string"))
}

/// Check 2 on parsed `cargo metadata` output. Members are checked in name order.
fn rows_and_closures(metadata: &serde_json::Value) -> anyhow::Result<Vec<String>> {
    let graph = Graph::parse(metadata)?;
    let mut members = array(metadata, "workspace_members")?
        .iter()
        .map(|id| {
            id.as_str()
                .context("cargo metadata has a workspace_members entry that is not a string")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    members.sort_by_key(|id| graph.name(id));

    let mut violations = Vec::new();
    for &id in &members {
        let member = graph.name(id);
        match ALLOWED.iter().find(|(name, _)| *name == member) {
            Some((_, row)) => {
                let direct: BTreeSet<&str> = graph
                    .deps
                    .get(id)
                    .into_iter()
                    .flatten()
                    .filter(|edge| edge.normal || edge.build)
                    .map(|edge| graph.name(edge.pkg))
                    .collect();
                for dep in direct.into_iter().filter(|dep| !row.contains(dep)) {
                    violations.push(format!(
                        "{member} depends on {dep}, which is not in its ADR-001 row"
                    ));
                }
            }
            None => violations.push(format!("{member} has no ADR-001 row")),
        }

        let closure = graph.closure(id, &[]);
        if closure.contains("asupersync") && !ASUPERSYNC_ALLOWED.contains(&member) {
            violations.push(format!("asupersync in the closure of {member}"));
        }
        if member == "tl-ui" {
            let without_gpui_kit = graph.closure(id, &["gpui-kit"]);
            for &name in UI_CLOSURE_BANNED {
                let reached = if UI_CLOSURE_VIA_GPUI_KIT_OK.contains(&name) {
                    &without_gpui_kit
                } else {
                    &closure
                };
                if reached.contains(name) {
                    violations.push(format!("tl-ui closure contains {name}"));
                }
            }
        }
    }
    Ok(violations)
}

/// Check 3. Any `asupersync::` path under `crates/` outside tl-engine.
fn check_asupersync_outside_engine(root: &Path) -> anyhow::Result<Vec<String>> {
    let args = [
        "-n",
        "asupersync::",
        "crates/",
        "--glob",
        "!crates/tl-engine/**",
    ];
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
        let path = entry
            .with_context(|| format!("could not list {}", dir.display()))?
            .path();
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

/// Finds banned API names in one source file, skipping comments and `#[cfg(test)]` modules.
/// A name in a comment is not a use. The allow comment is read from the original line.
fn scan_banned_apis(source: &str) -> Vec<ApiHit> {
    let production = blank_cfg_test_modules(&blank_comments(source));
    let mut hits = Vec::new();
    for (index, (line, original)) in production.lines().zip(source.lines()).enumerate() {
        let line_number = index + 1;
        for &word in BANNED_WORDS {
            if contains_word(line, word) {
                hits.push(ApiHit {
                    pattern: word,
                    line: line_number,
                });
            }
        }
        for &name in BANNED_MACROS {
            if contains_macro(line, name) {
                hits.push(ApiHit {
                    pattern: name,
                    line: line_number,
                });
            }
        }
        for &literal in BANNED_LITERALS {
            if line.contains(literal) {
                hits.push(ApiHit {
                    pattern: literal,
                    line: line_number,
                });
            }
        }
        for &literal in ALLOWABLE_LITERALS {
            if line.contains(literal) && !has_allow_comment(original) {
                hits.push(ApiHit {
                    pattern: literal,
                    line: line_number,
                });
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

/// Replaces line and block comments with spaces, keeping newlines and string literals. A small
/// lexer: it knows string, raw string and char literals so `//` inside them is not a comment.
fn blank_comments(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = chars.clone();
    let blank = |out: &mut Vec<char>, i: usize| {
        if out[i] != '\n' {
            out[i] = ' ';
        }
    };
    let at = |i: usize| chars.get(i).copied();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev = i.checked_sub(1).and_then(at);
        if c == '/' && at(i + 1) == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                blank(&mut out, i);
                i += 1;
            }
        } else if c == '/' && at(i + 1) == Some('*') {
            let mut depth = 0usize;
            while i < chars.len() {
                if chars[i] == '/' && at(i + 1) == Some('*') {
                    depth += 1;
                    blank(&mut out, i);
                    blank(&mut out, i + 1);
                    i += 2;
                } else if chars[i] == '*' && at(i + 1) == Some('/') {
                    depth -= 1;
                    blank(&mut out, i);
                    blank(&mut out, i + 1);
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    blank(&mut out, i);
                    i += 1;
                }
            }
        } else if c == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i += 1;
        } else if c == 'r' && !prev.is_some_and(|p| is_ident_char(p) && p != 'b') {
            let hashes = chars[i + 1..].iter().take_while(|&&h| h == '#').count();
            if at(i + 1 + hashes) == Some('"') {
                let close: Vec<char> = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                i += 2 + hashes;
                while i < chars.len() && !chars[i..].starts_with(&close) {
                    i += 1;
                }
                i += close.len();
            } else {
                i += 1;
            }
        } else if c == '\'' && at(i + 1) == Some('\\') {
            i += 2;
            while i < chars.len() && chars[i] != '\'' {
                i += 1;
            }
            i += 1;
        } else if c == '\'' && at(i + 2) == Some('\'') {
            i += 3;
        } else {
            i += 1;
        }
    }
    out.into_iter().collect()
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

/// True when `name` occurs in `line` with no identifier character before it.
fn contains_macro(line: &str, name: &str) -> bool {
    line.match_indices(name)
        .any(|(start, _)| !line[..start].chars().next_back().is_some_and(is_ident_char))
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
    let name_len = after_mod
        .find(|c: char| !is_ident_char(c))
        .unwrap_or(after_mod.len());
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

    /// Hand-built `cargo metadata` JSON. Each edge is `(from, to, kind)` with kind `None` for a
    /// normal edge. Package ids are the names.
    fn metadata(members: &[&str], edges: &[(&str, &str, Option<&str>)]) -> serde_json::Value {
        let mut names: Vec<&str> = members.to_vec();
        for (from, to, _) in edges {
            for name in [from, to] {
                if !names.contains(name) {
                    names.push(name);
                }
            }
        }
        let packages: Vec<_> = names
            .iter()
            .map(|name| serde_json::json!({ "id": name, "name": name }))
            .collect();
        let nodes: Vec<_> = names
            .iter()
            .map(|name| {
                let deps: Vec<_> = edges
                    .iter()
                    .filter(|(from, _, _)| from == name)
                    .map(|(_, to, kind)| {
                        serde_json::json!({
                            "name": to.replace('-', "_"),
                            "pkg": to,
                            "dep_kinds": [{ "kind": kind, "target": null }],
                        })
                    })
                    .collect();
                serde_json::json!({ "id": name, "deps": deps })
            })
            .collect();
        serde_json::json!({
            "packages": packages,
            "workspace_members": members,
            "resolve": { "nodes": nodes },
        })
    }

    #[test]
    fn closure_follows_normal_edges_only() {
        let json = metadata(&["a"], &[("a", "b", None), ("b", "c", Some("dev"))]);
        let graph = Graph::parse(&json).unwrap();
        assert_eq!(graph.closure("a", &[]), BTreeSet::from(["b"]));
    }

    #[test]
    fn row_violation_reported() {
        let json = metadata(
            &["tl-ops"],
            &[
                ("tl-ops", "tl-model", None),
                ("tl-ops", "serde", None),
                ("tl-ops", "tempfile", Some("dev")),
            ],
        );
        assert_eq!(
            rows_and_closures(&json).unwrap(),
            vec!["tl-ops depends on serde, which is not in its ADR-001 row"]
        );
    }

    #[test]
    fn build_dependency_is_checked() {
        let json = metadata(&["tl-ops"], &[("tl-ops", "cc", Some("build"))]);
        assert_eq!(
            rows_and_closures(&json).unwrap(),
            vec!["tl-ops depends on cc, which is not in its ADR-001 row"]
        );
    }

    #[test]
    fn member_without_row_reported() {
        let json = metadata(&["tl-netmon"], &[]);
        assert_eq!(
            rows_and_closures(&json).unwrap(),
            vec!["tl-netmon has no ADR-001 row"]
        );
    }

    #[test]
    fn asupersync_closure_rule() {
        let json = metadata(
            &["tl-collect", "tl-engine"],
            &[
                ("tl-collect", "asupersync", None),
                ("tl-engine", "asupersync", None),
            ],
        );
        assert_eq!(
            rows_and_closures(&json).unwrap(),
            vec![
                "tl-collect depends on asupersync, which is not in its ADR-001 row",
                "asupersync in the closure of tl-collect",
            ]
        );
    }

    #[test]
    fn ui_closure_rule() {
        let json = metadata(
            &["tl-ui"],
            &[("tl-ui", "gpui-kit", None), ("gpui-kit", "zbus", None)],
        );
        assert_eq!(rows_and_closures(&json).unwrap(), Vec::<String>::new());

        let json = metadata(
            &["tl-ui"],
            &[
                ("tl-ui", "gpui-kit", None),
                ("gpui-kit", "zbus", None),
                ("tl-ui", "serde_json", None),
                ("serde_json", "zbus", None),
            ],
        );
        assert_eq!(
            rows_and_closures(&json).unwrap(),
            vec!["tl-ui closure contains zbus"]
        );
    }

    #[test]
    fn ui_closure_rule_through_engine() {
        let json = metadata(
            &["tl-ui", "tl-engine", "tl-collect", "tl-ops"],
            &[
                ("tl-ui", "tl-engine", None),
                ("tl-engine", "tl-collect", None),
                ("tl-engine", "tl-ops", None),
                ("tl-engine", "asupersync", None),
            ],
        );
        assert_eq!(
            rows_and_closures(&json).unwrap(),
            vec![
                "tl-ui depends on tl-engine, which is not in its ADR-001 row",
                "asupersync in the closure of tl-ui",
                "tl-ui closure contains tl-engine",
                "tl-ui closure contains tl-collect",
                "tl-ui closure contains tl-ops",
            ]
        );
    }

    #[test]
    fn missing_field_is_named() {
        let json = serde_json::json!({ "packages": [], "workspace_members": [] });
        let err = rows_and_closures(&json).unwrap_err();
        assert_eq!(err.to_string(), "cargo metadata has no resolve field");
    }

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
        assert_eq!(
            shipped_in_tree(tree, &["tl-app", "tl-gpu-probe"]),
            vec!["tl-app"]
        );
    }

    #[test]
    fn ambiguous_specs_reads_cargo_help() {
        let stderr = "error: specification `aes` is ambiguous\n\
                      help: re-run this command with one of the following specifications\n  \
                      aes@0.8.4\n  aes@0.9.3\n";
        assert_eq!(
            ambiguous_specs(stderr, "aes"),
            vec!["aes@0.8.4", "aes@0.9.3"]
        );
    }

    #[test]
    fn api_scan_skips_cfg_test_module() {
        let inside =
            "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn f() {\n        select!();\n    }\n}\n";
        assert_eq!(scan_banned_apis(inside), Vec::new());

        let one_line = "#[cfg(test)] mod tests { fn f() { select!(); } }\n";
        assert_eq!(scan_banned_apis(one_line), Vec::new());

        let outside = "fn a() {}\n\nfn f() {\n    select!();\n}\n";
        assert_eq!(
            scan_banned_apis(outside),
            vec![ApiHit {
                pattern: "select!",
                line: 4
            }]
        );

        let after = "#[cfg(test)]\nmod tests {\n    fn f() {}\n}\nfn g() { select!(); }\n";
        assert_eq!(
            scan_banned_apis(after),
            vec![ApiHit {
                pattern: "select!",
                line: 5
            }]
        );
    }

    #[test]
    fn api_scan_keeps_cfg_test_items_that_are_not_modules() {
        let source = "#[cfg(test)]\nfn helper() { select!(); }\n";
        assert_eq!(
            scan_banned_apis(source),
            vec![ApiHit {
                pattern: "select!",
                line: 2
            }]
        );
    }

    #[test]
    fn api_scan_skips_comments() {
        let source =
            "//! Banned: `select!`, `actor`.\n/* race! */\nlet url = \"http://x\"; select!();\n";
        assert_eq!(
            scan_banned_apis(source),
            vec![ApiHit {
                pattern: "select!",
                line: 3
            }]
        );

        let in_string = "let s = \"// not a comment\"; let actor = 1;";
        assert_eq!(
            scan_banned_apis(in_string),
            vec![ApiHit {
                pattern: "actor",
                line: 1
            }]
        );

        let raw = "let s = r#\"\"//\"#; let c = '/'; let actor = 1;";
        assert_eq!(
            scan_banned_apis(raw),
            vec![ApiHit {
                pattern: "actor",
                line: 1
            }]
        );
    }

    #[test]
    fn api_scan_whole_word() {
        assert_eq!(
            scan_banned_apis("let actor = 1;"),
            vec![ApiHit {
                pattern: "actor",
                line: 1
            }]
        );
        assert_eq!(scan_banned_apis("let factory = 1;"), Vec::new());
        assert_eq!(scan_banned_apis("let actors_count = 1;"), Vec::new());
    }

    #[test]
    fn api_scan_race_macro_needs_word_start() {
        assert_eq!(scan_banned_apis("tracing::trace!(\"x\");"), Vec::new());
        assert_eq!(scan_banned_apis("trace!(\"x\");"), Vec::new());
        assert_eq!(
            scan_banned_apis("let x = race!(a, b);"),
            vec![ApiHit {
                pattern: "race!",
                line: 1
            }]
        );
        assert_eq!(
            scan_banned_apis("asupersync::race!(a, b);"),
            vec![ApiHit {
                pattern: "race!",
                line: 1
            }]
        );
    }

    #[test]
    fn allow_comment_accepts_poll_quota() {
        let allowed = "budget.with_poll_quota(8) // check-deps: allow short probe";
        assert_eq!(scan_banned_apis(allowed), Vec::new());

        let bare = "budget.with_poll_quota(8)";
        assert_eq!(
            scan_banned_apis(bare),
            vec![ApiHit {
                pattern: ".with_poll_quota(",
                line: 1
            }]
        );

        let no_reason = "budget.with_poll_quota(8) // check-deps: allow";
        assert_eq!(scan_banned_apis(no_reason).len(), 1);
    }
}
