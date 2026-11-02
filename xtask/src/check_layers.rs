//! `cargo xtask check-layers`: no upward module paths inside tl-ui (ADR-020, tasklens-zljr5.2).
//!
//! ADR-020 splits tl-ui into layered crates. Before the split, every file under
//! `crates/tl-ui/src` already belongs to one target crate (the layer table below). A file may
//! name modules of its own crate and of the crates its ADR-020 row allows, nothing else.
//! An extracted crate's files keep their paths relative to `src/` (ADR-020 move method), so the
//! check joins tl-ui's tree and the extracted crates' trees (`UI_SRCS`) into one module tree, and
//! fails when a file sits in another crate's tree than its layer names.
//!
//! The check reads every `.rs` file and finds each module path it names: `crate::`, `self::` and
//! `super::` chains, with the `use` brace groups under them, over several lines when needed.
//! `self::` and `super::` are resolved against the file's own module path and the inline `mod`
//! blocks around them. Each path maps to the file that holds its deepest module, and that file to
//! its crate. Comments and string literals are skipped. `mod` declarations are not edges.
//! Only `#[cfg(test)]` code (an item under that attribute, or a file module declared under it)
//! may name the dev-only tl-ui-testkit.
//!
//! Today's upward edges are listed in `xtask/check_layers_allow.txt`, one `<file> -> <module>` per
//! line. The list may only shrink: an entry that no longer matches an upward edge fails the check,
//! so the cycle bead that removes an edge also removes its line. `--write-allowlist` writes the
//! list from the current tree, and `--summary` prints the entry count per crate pair.
//!
//! The allowed-edges map is also where check-deps takes the tl-ui-* dependencies of every tl-ui
//! crate from, so the two checks cannot disagree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

/// tl-ui's source tree, relative to the workspace root. Files of crates not yet extracted live
/// here.
pub const UI_SRC: &str = "crates/tl-ui/src";

/// The source trees the check reads, relative to the workspace root: tl-ui's and one per extracted
/// crate. Each extraction bead adds its crate (scripts/split/extract.py prints the line).
const UI_SRCS: &[(&str, &str)] = &[
    (SHELL, UI_SRC),
    ("tl-ui-base", "crates/tl-ui-base/src"),
    ("tl-ui-kit", "crates/tl-ui-kit/src"),
    ("tl-ui-chart", "crates/tl-ui-chart/src"),
    ("tl-ui-table", "crates/tl-ui-table/src"),
];

/// The allowlist of today's upward edges, relative to the workspace root.
const ALLOWLIST: &str = "xtask/check_layers_allow.txt";

/// The shell crate: tl-ui after the split.
const SHELL: &str = "tl-ui";

/// The dev-only test kit. Every tl-ui crate may use it from unit tests (ADR-020), as a
/// dev-dependency, so an edge into it is allowed from `#[cfg(test)]` code only. Production code
/// that names it fails, as a normal dependency on it fails check-deps (no row lists it).
const TESTKIT: &str = "tl-ui-testkit";

/// ADR-020 "Where each file goes": path prefix under `src/` (tl-ui's or the extracted crate's)
/// and its crate. A prefix that ends in `/` is a directory and also matches the module file next
/// to it (`a/` matches `a.rs`). Any other prefix is one file. The longest matching prefix wins.
/// The table holds the files that the cycle beads create too (`template.rs`, `app_env.rs`,
/// `registry.rs`, `shell.rs`, `motion.rs`, `toast_types.rs`, `pages/performance/core/`).
const LAYERS: &[(&str, &str)] = &[
    // tl-ui-base
    ("theme/", "tl-ui-base"),
    ("text/", "tl-ui-base"),
    ("i18n.rs", "tl-ui-base"),
    ("template.rs", "tl-ui-base"),
    ("selector.rs", "tl-ui-base"),
    ("util.rs", "tl-ui-base"),
    ("focus.rs", "tl-ui-base"),
    ("anim.rs", "tl-ui-base"),
    ("motion.rs", "tl-ui-base"),
    ("toast_types.rs", "tl-ui-base"),
    ("hub.rs", "tl-ui-base"),
    ("demand.rs", "tl-ui-base"),
    ("settings_global.rs", "tl-ui-base"),
    ("host.rs", "tl-ui-base"),
    ("host_settings.rs", "tl-ui-base"),
    ("hosts_api.rs", "tl-ui-base"),
    ("hosts_edit.rs", "tl-ui-base"),
    ("action_tracker.rs", "tl-ui-base"),
    ("registry.rs", "tl-ui-base"),
    ("app_env.rs", "tl-ui-base"),
    ("shell.rs", "tl-ui-base"),
    ("engine_link.rs", "tl-ui-base"),
    ("request_ends.rs", "tl-ui-base"),
    // tl-ui-kit
    ("components/", "tl-ui-kit"),
    ("helper_flow.rs", "tl-ui-kit"),
    ("actions.rs", "tl-ui-kit"),
    // tl-ui-chart
    ("chart/", "tl-ui-chart"),
    ("net_totals.rs", "tl-ui-chart"),
    // tl-ui-table
    ("table/", "tl-ui-table"),
    ("text_cache.rs", "tl-ui-table"),
    // tl-ui-perf-core
    ("pages/performance/core/", "tl-ui-perf-core"),
    ("pages/minigraph/", "tl-ui-perf-core"),
    ("tray_bridge.rs", "tl-ui-perf-core"),
    ("windows/detached_graph.rs", "tl-ui-perf-core"),
    // tl-ui-overview
    ("pages/performance/overview/", "tl-ui-overview"),
    ("pages/performance/widget/", "tl-ui-overview"),
    ("pages/performance/memory_apps.rs", "tl-ui-overview"),
    // tl-ui-perf
    ("pages/performance/", "tl-ui-perf"),
    ("windows/mini.rs", "tl-ui-perf"),
    // tl-ui-procs
    ("pages/processes/", "tl-ui-procs"),
    ("windows/props/", "tl-ui-procs"),
    ("windows/app_props.rs", "tl-ui-procs"),
    ("windows/affinity.rs", "tl-ui-procs"),
    ("windows/priority.rs", "tl-ui-procs"),
    ("windows/signal.rs", "tl-ui-procs"),
    ("windows/handle_search.rs", "tl-ui-procs"),
    ("windows/notices.rs", "tl-ui-procs"),
    ("windows/actions.rs", "tl-ui-procs"),
    ("windows/disk_in_use.rs", "tl-ui-procs"),
    ("windows/frame.rs", "tl-ui-procs"),
    // tl-ui-sys
    ("pages/services/", "tl-ui-sys"),
    ("pages/connections/", "tl-ui-sys"),
    ("pages/startup/", "tl-ui-sys"),
    ("windows/service_properties/", "tl-ui-sys"),
    ("windows/connection_properties.rs", "tl-ui-sys"),
    ("windows/startup_properties.rs", "tl-ui-sys"),
    ("windows/confirm_startup_delete.rs", "tl-ui-sys"),
    // tl-ui (shell)
    ("lib.rs", SHELL),
    ("app.rs", SHELL),
    ("window/", SHELL),
    ("pages/mod.rs", SHELL),
    ("pages/information/", SHELL),
    ("windows/mod.rs", SHELL),
    ("windows/run.rs", SHELL),
    ("windows/run_history.rs", SHELL),
    ("windows/low_memory.rs", SHELL),
    ("windows/smart.rs", SHELL),
    ("bench/", SHELL),
    // tl-ui-testkit (CS-B1 moves it out)
    ("testing/", TESTKIT),
];

/// ADR-020 "Allowed edges": the tl-ui crates each tl-ui crate may depend on. tl-model and
/// gpui-kit are allowed for all but tl-ui-settings and come from the check-deps rows. A crate
/// missing here may depend on no tl-ui crate.
pub const EDGES: &[(&str, &[&str])] = &[
    ("tl-ui-settings", &[]),
    ("tl-ui-base", &["tl-ui-settings"]),
    ("tl-ui-kit", &["tl-ui-base", "tl-ui-settings"]),
    (
        "tl-ui-chart",
        &["tl-ui-kit", "tl-ui-base", "tl-ui-settings"],
    ),
    (
        "tl-ui-table",
        &["tl-ui-kit", "tl-ui-base", "tl-ui-settings"],
    ),
    (
        "tl-ui-perf-core",
        &[
            "tl-ui-chart",
            "tl-ui-table",
            "tl-ui-kit",
            "tl-ui-base",
            "tl-ui-settings",
        ],
    ),
    (
        "tl-ui-overview",
        &[
            "tl-ui-perf-core",
            "tl-ui-chart",
            "tl-ui-table",
            "tl-ui-kit",
            "tl-ui-base",
            "tl-ui-settings",
        ],
    ),
    (
        "tl-ui-perf",
        &[
            "tl-ui-overview",
            "tl-ui-perf-core",
            "tl-ui-chart",
            "tl-ui-table",
            "tl-ui-kit",
            "tl-ui-base",
            "tl-ui-settings",
        ],
    ),
    (
        "tl-ui-procs",
        &[
            "tl-ui-perf-core",
            "tl-ui-chart",
            "tl-ui-table",
            "tl-ui-kit",
            "tl-ui-base",
            "tl-ui-settings",
        ],
    ),
    (
        "tl-ui-sys",
        &[
            "tl-ui-procs",
            "tl-ui-perf-core",
            "tl-ui-chart",
            "tl-ui-table",
            "tl-ui-kit",
            "tl-ui-base",
            "tl-ui-settings",
        ],
    ),
    (
        SHELL,
        &[
            "tl-ui-sys",
            "tl-ui-procs",
            "tl-ui-perf",
            "tl-ui-overview",
            "tl-ui-perf-core",
            "tl-ui-chart",
            "tl-ui-table",
            "tl-ui-kit",
            "tl-ui-base",
            "tl-ui-settings",
        ],
    ),
    (TESTKIT, &[]),
    (
        "tl-ui-pagehost",
        &["tl-ui-testkit", "tl-ui-kit", "tl-ui-base", "tl-ui-settings"],
    ),
];

/// True for `tl-ui` and every `tl-ui-*` crate name.
pub fn is_ui_crate(name: &str) -> bool {
    name == SHELL || name.starts_with("tl-ui-")
}

/// True when the ADR-020 row of `src` lists `dst`. Used by check-deps for the dependency rows.
pub fn may_depend(src: &str, dst: &str) -> bool {
    EDGES
        .iter()
        .find(|(name, _)| *name == src)
        .is_some_and(|(_, row)| row.contains(&dst))
}

/// True when code in crate `src` may name a module of crate `dst`. `test` is true for
/// `#[cfg(test)]` code, the only code that may name the dev-only test kit.
fn edge_allowed(src: &str, dst: &str, test: bool) -> bool {
    src == dst || may_depend(src, dst) || (test && dst == TESTKIT)
}

/// The crate of a file, from its path under `src/` and the longest matching layer prefix.
fn layer_of(file: &str) -> Option<&'static str> {
    let as_dir = file.strip_suffix(".rs").map(|stem| format!("{stem}/"));
    LAYERS
        .iter()
        .filter(|(prefix, _)| {
            if prefix.ends_with('/') {
                file.starts_with(prefix) || as_dir.as_deref().is_some_and(|d| d.starts_with(prefix))
            } else {
                file == *prefix
            }
        })
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, krate)| *krate)
}

/// The crate name as printed: the shell is marked, so `tl-ui-base -> tl-ui (shell)` reads clearly.
fn shown(krate: &str) -> String {
    if krate == SHELL {
        format!("{SHELL} (shell)")
    } else {
        krate.to_owned()
    }
}

pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    let mut write = false;
    let mut summary = false;
    for arg in &args {
        match arg.as_str() {
            "--write-allowlist" => write = true,
            "--summary" => summary = true,
            other => bail!(
                "check-layers: unknown argument {other}; it takes --write-allowlist and --summary"
            ),
        }
    }
    let root = workspace_root();
    let tree = read_trees(&root)?;
    let scan = scan_tree(&tree);
    if !scan.unmapped.is_empty() {
        for file in &scan.unmapped {
            println!(
                "{UI_SRC}/{file}: no ADR-020 crate for this file; add a row to ADR-020 and to \
                 LAYERS in xtask/src/check_layers.rs"
            );
        }
        bail!(
            "check-layers: {} file(s) without a crate",
            scan.unmapped.len()
        );
    }

    let allow_path = root.join(ALLOWLIST);
    if write {
        let text = render_allowlist(&scan.edges);
        std::fs::write(&allow_path, text)
            .with_context(|| format!("could not write {}", allow_path.display()))?;
        println!("check-layers: wrote {ALLOWLIST}");
    }
    let allow_text = std::fs::read_to_string(&allow_path)
        .with_context(|| format!("could not read {}", allow_path.display()))?;
    let allowlist = parse_allowlist(&allow_text)?;
    if summary {
        let names: BTreeSet<String> = tree.keys().cloned().collect();
        for line in summary_lines(&allowlist, &names) {
            println!("{line}");
        }
    }

    let violations = check(&scan.edges, &allowlist);
    for violation in &violations {
        println!("{violation}");
    }
    if !violations.is_empty() {
        bail!("check-layers: {} violation(s)", violations.len());
    }
    println!("check-layers: ok ({} allowlisted edge(s))", allowlist.len());
    Ok(())
}

/// The workspace root is the parent of the xtask package directory.
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap_or(manifest_dir).to_path_buf()
}

/// Every `.rs` file of every tree in `UI_SRCS`, keyed by its path relative to its `src/`. An
/// extracted crate's `lib.rs` only declares its modules and imports the lower crates' modules, so
/// it is skipped: tl-ui's `lib.rs` stays the root of the joined tree. A file must sit in the tree
/// of the crate its layer names, so a file left behind in tl-ui or moved to the wrong crate fails.
fn read_trees(root: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let mut tree = BTreeMap::new();
    let mut misplaced = Vec::new();
    for &(krate, dir) in UI_SRCS {
        for (file, text) in read_tree(&root.join(dir))? {
            if krate != SHELL && file == "lib.rs" {
                continue;
            }
            match layer_of(&file) {
                Some(layer) if src_dir(layer) != dir => misplaced.push(format!(
                    "{dir}/{file}: ADR-020 puts this file in {layer}, whose sources are in {}",
                    src_dir(layer)
                )),
                None if krate != SHELL => misplaced.push(format!(
                    "{dir}/{file}: no ADR-020 crate for this file; add a row to ADR-020 and to \
                     LAYERS in xtask/src/check_layers.rs"
                )),
                _ => {}
            }
            if tree.insert(file.clone(), text).is_some() {
                bail!("check-layers: {file} is in two source trees");
            }
        }
    }
    if !misplaced.is_empty() {
        for line in &misplaced {
            println!("{line}");
        }
        bail!(
            "check-layers: {} file(s) in the wrong crate",
            misplaced.len()
        );
    }
    Ok(tree)
}

/// The source tree that holds a crate's files: its own once it is extracted, tl-ui's until then.
fn src_dir(krate: &str) -> &'static str {
    UI_SRCS
        .iter()
        .find(|(name, _)| *name == krate)
        .map_or(UI_SRC, |(_, dir)| *dir)
}

/// Every `.rs` file under `src`, keyed by its path relative to `src` with `/` separators.
fn read_tree(src: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let mut paths = Vec::new();
    collect_rs_files(src, &mut paths)?;
    let mut tree = BTreeMap::new();
    for path in paths {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let relative = path.strip_prefix(src).unwrap_or(&path);
        tree.insert(relative.to_string_lossy().replace('\\', "/"), text);
    }
    Ok(tree)
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

/// One upward edge: a path in `file` at `line` that names `module`, which lives in a crate that
/// the crate of `file` may not depend on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Edge {
    src: &'static str,
    dst: &'static str,
    file: String,
    module: String,
    line: usize,
    path: String,
}

/// The result of scanning the tree: upward edges and files with no crate.
#[derive(Debug, Default)]
struct Scan {
    edges: Vec<Edge>,
    unmapped: Vec<String>,
}

/// Scans every file in `tree` and keeps the edges that ADR-020 does not allow.
fn scan_tree(tree: &BTreeMap<String, String>) -> Scan {
    let names: BTreeSet<String> = tree.keys().cloned().collect();
    let scans: BTreeMap<&str, FileScan> = tree
        .iter()
        .map(|(file, source)| (file.as_str(), scan_paths(file, source)))
        .collect();
    let test_files = test_files(&scans, &names);
    let mut scan = Scan::default();
    for (file, file_scan) in &scans {
        let Some(src) = layer_of(file) else {
            scan.unmapped.push((*file).to_owned());
            continue;
        };
        let test_file = test_files.contains(*file);
        for hit in &file_scan.hits {
            let (target, module) = resolve(&hit.path, &names);
            // A target with no crate is an unmapped file, which is reported on its own.
            let Some(dst) = layer_of(&target) else {
                continue;
            };
            if edge_allowed(src, dst, hit.test || test_file) {
                continue;
            }
            scan.edges.push(Edge {
                src,
                dst,
                file: (*file).to_owned(),
                module: module_name(&module),
                line: hit.line,
                path: module_name(&hit.path),
            });
        }
    }
    scan.edges
        .sort_by(|a, b| (&a.file, a.line, &a.path).cmp(&(&b.file, b.line, &b.path)));
    scan.edges.dedup();
    scan
}

/// The files that only test builds compile: a file module declared under `#[cfg(test)]`
/// (`#[cfg(test)] mod tests;`), and every file module declared inside such a file.
fn test_files(scans: &BTreeMap<&str, FileScan>, names: &BTreeSet<String>) -> BTreeSet<String> {
    let mut test = BTreeSet::new();
    loop {
        let before = test.len();
        for (file, file_scan) in scans {
            let file_is_test = test.contains(*file);
            for decl in &file_scan.mods {
                if !(decl.test || file_is_test) {
                    continue;
                }
                let stem = decl.path.join("/");
                let child = [format!("{stem}.rs"), format!("{stem}/mod.rs")]
                    .into_iter()
                    .find(|candidate| names.contains(candidate));
                if let Some(child) = child {
                    test.insert(child);
                }
            }
        }
        if test.len() == before {
            return test;
        }
    }
}

/// `a::b` for a module path, `crate` for the root.
fn module_name(segments: &[String]) -> String {
    if segments.is_empty() {
        "crate".to_owned()
    } else {
        segments.join("::")
    }
}

/// The module path of a file: `lib.rs` is the root, `a/mod.rs` is `a`, `a/b.rs` is `a::b`.
fn file_module(file: &str) -> Vec<String> {
    let stem = file.strip_suffix(".rs").unwrap_or(file);
    let mut segments: Vec<String> = stem.split('/').map(str::to_owned).collect();
    let last = segments.last().map(String::as_str);
    if last == Some("mod") || (segments.len() == 1 && last == Some("lib")) {
        segments.pop();
    }
    segments
}

/// The file that holds the deepest module named by `segments`, and that module's path. Names
/// past the last module file (items, inline modules) are dropped.
fn resolve(segments: &[String], files: &BTreeSet<String>) -> (String, Vec<String>) {
    let mut file = "lib.rs".to_owned();
    let mut dir = String::new();
    let mut used = 0;
    for segment in segments {
        let flat = format!("{dir}{segment}.rs");
        let nested = format!("{dir}{segment}/mod.rs");
        if files.contains(&flat) {
            file = flat;
        } else if files.contains(&nested) {
            file = nested;
        } else {
            break;
        }
        dir = format!("{dir}{segment}/");
        used += 1;
    }
    (file, segments[..used].to_vec())
}

/// One module path named in a file: its segments from the crate root, items included, the 1-based
/// line of its last segment, and whether it sits in `#[cfg(test)]` code.
#[derive(Debug, PartialEq, Eq)]
struct Hit {
    line: usize,
    path: Vec<String>,
    test: bool,
}

/// A `mod name;` declaration: the module path of the file it pulls in, and whether the
/// declaration sits in `#[cfg(test)]` code.
#[derive(Debug, PartialEq, Eq)]
struct ModDecl {
    path: Vec<String>,
    test: bool,
}

/// What one file names: its module paths and its `mod name;` declarations.
#[derive(Debug, Default)]
struct FileScan {
    hits: Vec<Hit>,
    mods: Vec<ModDecl>,
}

/// Every `crate::`, `self::` and `super::` path in one file, with `use` groups expanded, and every
/// `mod name;` declaration.
fn scan_paths(file: &str, source: &str) -> FileScan {
    let code = code_only(source);
    let mut line_of = Vec::with_capacity(code.len());
    let mut line = 1;
    for &c in &code {
        line_of.push(line);
        if c == '\n' {
            line += 1;
        }
    }
    let test_mask = cfg_test_mask(&code);
    let base = file_module(file);
    let mut parser = PathParser {
        code: &code,
        line_of: &line_of,
        hits: Vec::new(),
    };
    let mut mods = Vec::new();

    let mut depth = 0usize;
    // Inline `mod name { ... }` blocks around the current position, with the brace depth inside.
    let mut inline: Vec<(String, usize)> = Vec::new();
    let mut pending_mod: Option<(usize, String)> = None;
    let mut i = 0;
    while i < code.len() {
        let c = code[i];
        if c == '{' {
            depth += 1;
            if let Some((_, name)) = pending_mod.take_if(|(brace, _)| *brace == i) {
                inline.push((name, depth));
            }
        } else if c == '}' {
            if inline.last().is_some_and(|(_, open)| *open == depth) {
                inline.pop();
            }
            depth = depth.saturating_sub(1);
        } else if is_ident_start(c) && (i == 0 || !is_ident_char(code[i - 1])) {
            let end = ident_end(&code, i);
            let word: String = code[i..end].iter().collect();
            let current = || {
                let mut path = base.clone();
                path.extend(inline.iter().map(|(name, _)| name.clone()));
                path
            };
            match word.as_str() {
                "mod" => match mod_item(&code, end) {
                    Some((name, next)) if code[next] == '{' => pending_mod = Some((next, name)),
                    Some((name, next)) if code[next] == ';' => {
                        let mut path = current();
                        path.push(name);
                        mods.push(ModDecl {
                            path,
                            test: test_mask[i],
                        });
                    }
                    _ => {}
                },
                "crate" | "self" | "super" if !after_path_sep(&code, i) => {
                    let first = parser.hits.len();
                    let path_end = parser.root(&word, end, &current());
                    for hit in &mut parser.hits[first..] {
                        hit.test = test_mask[i];
                    }
                    // A path holds balanced braces only, so skipping it keeps `depth` right.
                    i = path_end.max(end);
                    continue;
                }
                _ => {}
            }
            i = end;
            continue;
        }
        i += 1;
    }
    FileScan {
        hits: parser.hits,
        mods,
    }
}

/// Walks one path from its root keyword and records the full paths it names.
struct PathParser<'a> {
    code: &'a [char],
    line_of: &'a [usize],
    hits: Vec<Hit>,
}

impl PathParser<'_> {
    /// `word` is `crate`, `self` or `super` and ends at `end`. `current` is the module path of
    /// the code around it. A root not followed by `::` (`pub(crate)`, `self.x`) names nothing.
    /// Returns the end of the path, so a `use` group is not scanned a second time.
    fn root(&mut self, word: &str, end: usize, current: &[String]) -> usize {
        let Some(pos) = self.sep(end) else {
            return end;
        };
        let prefix = match word {
            "crate" => Vec::new(),
            "self" => current.to_vec(),
            _ => parent(current),
        };
        self.after_sep(prefix, pos)
    }

    /// Parses what follows a `::`: a name, a `{` group, `*` or a turbofish. Returns the end.
    fn after_sep(&mut self, prefix: Vec<String>, pos: usize) -> usize {
        let pos = self.skip_ws(pos);
        match self.code.get(pos) {
            Some('{') => self.group(&prefix, pos + 1),
            Some('*') => {
                self.emit(prefix, pos);
                pos + 1
            }
            Some(&c) if is_ident_start(c) => {
                let end = ident_end(self.code, pos);
                let path = extend(prefix, self.name(pos, end));
                match self.sep(end) {
                    Some(next) => self.after_sep(path, next),
                    None => {
                        self.emit(path, pos);
                        end
                    }
                }
            }
            // `::<T>` or anything else ends the path at its prefix.
            _ => {
                self.emit(prefix, pos.saturating_sub(1));
                pos
            }
        }
    }

    /// Parses a `use` group from just inside its `{` to just past its `}`.
    fn group(&mut self, prefix: &[String], mut pos: usize) -> usize {
        loop {
            pos = self.skip_ws(pos);
            match self.code.get(pos) {
                None => return pos,
                Some('}') => return pos + 1,
                Some('{') => pos = self.group(prefix, pos + 1),
                Some('*') => {
                    self.emit(prefix.to_vec(), pos);
                    pos += 1;
                }
                Some(&c) if is_ident_start(c) => {
                    let end = ident_end(self.code, pos);
                    let name = self.name(pos, end);
                    if name == "as" {
                        // Skip the alias.
                        let alias = self.skip_ws(end);
                        pos = ident_end(self.code, alias).max(alias + 1);
                    } else {
                        let path = extend(prefix.to_vec(), name);
                        pos = match self.sep(end) {
                            Some(next) => self.after_sep(path, next),
                            None => {
                                self.emit(path, pos);
                                end
                            }
                        };
                    }
                }
                Some(_) => pos += 1,
            }
        }
    }

    fn emit(&mut self, path: Vec<String>, pos: usize) {
        let line = self
            .line_of
            .get(pos)
            .or(self.line_of.last())
            .copied()
            .unwrap_or(1);
        self.hits.push(Hit {
            line,
            path,
            test: false,
        });
    }

    /// The position just past `::` when it follows `pos` after optional whitespace.
    fn sep(&self, pos: usize) -> Option<usize> {
        let pos = self.skip_ws(pos);
        (self.code.get(pos) == Some(&':') && self.code.get(pos + 1) == Some(&':'))
            .then_some(pos + 2)
    }

    fn skip_ws(&self, pos: usize) -> usize {
        skip_ws(self.code, pos)
    }

    /// The identifier in `pos..end`, without a raw `r#` prefix.
    fn name(&self, pos: usize, end: usize) -> String {
        let mut start = pos;
        if self.code.get(pos) == Some(&'r') && self.code.get(pos + 1) == Some(&'#') {
            start += 2;
        }
        self.code[start..end].iter().collect()
    }
}

/// `path` with one more segment. `super` goes one level up, `self` stays and `crate` goes to the
/// root, wherever they stand, so `super::{super::x}` and `self::super::x` resolve like rustc.
fn extend(mut path: Vec<String>, name: String) -> Vec<String> {
    match name.as_str() {
        "super" => parent(&path),
        "self" => path,
        "crate" => Vec::new(),
        _ => {
            path.push(name);
            path
        }
    }
}

/// The module path one level up. The root stays the root.
fn parent(path: &[String]) -> Vec<String> {
    path[..path.len().saturating_sub(1)].to_vec()
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The end of the identifier at `pos`. A raw identifier (`r#type`) counts as one.
fn ident_end(code: &[char], pos: usize) -> usize {
    let mut end = pos;
    if code.get(pos) == Some(&'r')
        && code.get(pos + 1) == Some(&'#')
        && code.get(pos + 2).is_some_and(|&c| is_ident_start(c))
    {
        end += 2;
    }
    while code.get(end).is_some_and(|&c| is_ident_char(c)) {
        end += 1;
    }
    end
}

/// True when `::` comes right before `pos`, so the keyword there is inside a path
/// (`super::super`), not its root.
fn after_path_sep(code: &[char], pos: usize) -> bool {
    let mut back = pos;
    while back > 0 && code[back - 1].is_whitespace() {
        back -= 1;
    }
    back >= 2 && code[back - 1] == ':' && code[back - 2] == ':'
}

/// For a `mod` keyword ending at `end`: the module name and the position of the first character
/// after it (`{` for an inline block, `;` for a file module). `None` when no name follows.
fn mod_item(code: &[char], end: usize) -> Option<(String, usize)> {
    let start = skip_ws(code, end);
    if !code.get(start).is_some_and(|&c| is_ident_start(c)) {
        return None;
    }
    let name_end = ident_end(code, start);
    let mut name: String = code[start..name_end].iter().collect();
    if let Some(raw) = name.strip_prefix("r#") {
        name = raw.to_owned();
    }
    let next = skip_ws(code, name_end);
    (next < code.len()).then_some((name, next))
}

fn skip_ws(code: &[char], mut pos: usize) -> usize {
    while code.get(pos).is_some_and(|c| c.is_whitespace()) {
        pos += 1;
    }
    pos
}

/// True for the code under a `#[cfg(test)]` (or `#[cfg(all(test, ..))]`) attribute: the item
/// that follows it, or the rest of the enclosing block for an inner `#![cfg(test)]`. Only that
/// code may name tl-ui-testkit, which is a dev-dependency (ADR-020).
fn cfg_test_mask(code: &[char]) -> Vec<bool> {
    let mut mask = vec![false; code.len()];
    let mut i = 0;
    while i < code.len() {
        if code[i] != '#' {
            i += 1;
            continue;
        }
        let mut pos = skip_ws(code, i + 1);
        let inner = code.get(pos) == Some(&'!');
        if inner {
            pos = skip_ws(code, pos + 1);
        }
        if code.get(pos) != Some(&'[') {
            i += 1;
            continue;
        }
        let close = attr_end(code, pos);
        let attr: String = code[pos + 1..close]
            .iter()
            .filter(|c| !c.is_whitespace())
            .collect();
        if attr == "cfg(test)" || attr.starts_with("cfg(all(test,") {
            let end = if inner {
                block_end(code, close + 1)
            } else {
                item_end(code, close + 1)
            };
            mask[i..end].fill(true);
        }
        i = close + 1;
    }
    mask
}

/// The `]` that closes the attribute opened at `open`, or the end of the code.
fn attr_end(code: &[char], open: usize) -> usize {
    let mut depth = 0usize;
    for (pos, &c) in code.iter().enumerate().skip(open) {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return pos;
                }
            }
            _ => {}
        }
    }
    code.len()
}

/// The end of the item that starts at `from`: just past its first top-level `;` or past the `}`
/// of its first top-level block. Other attributes in between belong to the same item.
fn item_end(code: &[char], from: usize) -> usize {
    let mut depth = 0usize;
    for (pos, &c) in code.iter().enumerate().skip(from) {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                // A close with nothing open ends the enclosing block, so the item ends here.
                let Some(next) = depth.checked_sub(1) else {
                    return pos;
                };
                depth = next;
                if c == '}' && depth == 0 {
                    return pos + 1;
                }
            }
            ';' if depth == 0 => return pos + 1,
            _ => {}
        }
    }
    code.len()
}

/// The end of the block that `from` sits in: the `}` that closes it, or the end of the code.
fn block_end(code: &[char], from: usize) -> usize {
    let mut depth = 0usize;
    for (pos, &c) in code.iter().enumerate().skip(from) {
        match c {
            '{' => depth += 1,
            '}' => match depth.checked_sub(1) {
                Some(next) => depth = next,
                None => return pos,
            },
            _ => {}
        }
    }
    code.len()
}

/// The source with comments, string literals and char literals replaced by spaces. Newlines
/// stay, so line numbers do not change. Lifetimes are kept.
fn code_only(source: &str) -> Vec<char> {
    let chars: Vec<char> = source.chars().collect();
    let n = chars.len();
    let mut out = chars.clone();
    let blank = |out: &mut Vec<char>, from: usize, to: usize| {
        for c in out.iter_mut().take(to.min(n)).skip(from) {
            if *c != '\n' {
                *c = ' ';
            }
        }
    };
    let at = |i: usize| chars.get(i).copied();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let after_ident = i > 0 && is_ident_char(chars[i - 1]);
        if c == '/' && at(i + 1) == Some('/') {
            let mut end = i;
            while end < n && chars[end] != '\n' {
                end += 1;
            }
            blank(&mut out, i, end);
            i = end;
        } else if c == '/' && at(i + 1) == Some('*') {
            let start = i;
            let mut depth = 0usize;
            while i < n {
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
            let start = i;
            i += 1;
            while i < n && chars[i] != '"' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i += 1;
            blank(&mut out, start, i);
        } else if (c == 'r' || (c == 'b' && at(i + 1) == Some('r'))) && !after_ident {
            let open = if c == 'b' { i + 2 } else { i + 1 };
            let hashes = chars[open.min(n)..]
                .iter()
                .take_while(|&&h| h == '#')
                .count();
            if at(open + hashes) == Some('"') {
                let close: Vec<char> = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let start = i;
                i = open + hashes + 1;
                while i < n && !chars[i..].starts_with(&close) {
                    i += 1;
                }
                i += close.len();
                blank(&mut out, start, i);
            } else {
                i += 1;
            }
        } else if c == '\'' && at(i + 1) == Some('\\') {
            let start = i;
            i += 3;
            while i < n && chars[i] != '\'' {
                i += 1;
            }
            i += 1;
            blank(&mut out, start, i);
        } else if c == '\'' && at(i + 2) == Some('\'') {
            blank(&mut out, i, i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// One allowlist line: `<file> -> <module>` and its 1-based line number.
#[derive(Debug, PartialEq, Eq)]
struct AllowEntry {
    line: usize,
    file: String,
    module: String,
}

/// Reads the allowlist. `#` lines and blank lines are skipped; a duplicate entry is an error.
fn parse_allowlist(text: &str) -> anyhow::Result<Vec<AllowEntry>> {
    let mut entries: Vec<AllowEntry> = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((file, module)) = line.split_once(" -> ") else {
            bail!(
                "{ALLOWLIST}:{}: expected `<file> -> <module>`, got `{line}`",
                index + 1
            );
        };
        let (file, module) = (file.trim().to_owned(), module.trim().to_owned());
        if !seen.insert((file.clone(), module.clone())) {
            bail!(
                "{ALLOWLIST}:{}: duplicate entry `{file} -> {module}`",
                index + 1
            );
        }
        entries.push(AllowEntry {
            line: index + 1,
            file,
            module,
        });
    }
    Ok(entries)
}

/// The allowlist text for `edges`: one line per (file, module), grouped by crate pair.
fn render_allowlist(edges: &[Edge]) -> String {
    let mut pairs: BTreeMap<(&str, &str), BTreeSet<(&str, &str)>> = BTreeMap::new();
    for edge in edges {
        pairs
            .entry((edge.src, edge.dst))
            .or_default()
            .insert((&edge.file, &edge.module));
    }
    let mut text = String::from(
        "# Upward module edges inside crates/tl-ui/src that ADR-020 does not allow yet.\n\
         # One `<file under crates/tl-ui/src> -> <module path it names>` per line.\n\
         # Written by `cargo xtask check-layers --write-allowlist` (tasklens-zljr5.2).\n\
         # The list may only shrink: a cycle bead that removes an edge removes its line, and a\n\
         # line that matches no edge fails `cargo xtask check-layers`. CS-C11 empties it.\n",
    );
    for ((src, dst), entries) in &pairs {
        text.push_str(&format!(
            "\n# {} -> {}: {}\n",
            shown(src),
            shown(dst),
            entries.len()
        ));
        for (file, module) in entries {
            text.push_str(&format!("{file} -> {module}\n"));
        }
    }
    text
}

/// `--summary`: the allowlist entry count per (src crate, dst crate) pair and the total.
fn summary_lines(allowlist: &[AllowEntry], files: &BTreeSet<String>) -> Vec<String> {
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for entry in allowlist {
        let src = layer_of(&entry.file).map_or_else(|| "?".to_owned(), shown);
        let segments: Vec<String> = if entry.module == "crate" {
            Vec::new()
        } else {
            entry.module.split("::").map(str::to_owned).collect()
        };
        let dst = layer_of(&resolve(&segments, files).0).map_or_else(|| "?".to_owned(), shown);
        *counts.entry((src, dst)).or_default() += 1;
    }
    let mut lines: Vec<String> = counts
        .iter()
        .map(|((src, dst), count)| format!("{src} -> {dst}: {count}"))
        .collect();
    lines.push(format!("total: {}", allowlist.len()));
    lines
}

/// Upward edges missing from the allowlist, and allowlist entries that match no upward edge.
fn check(edges: &[Edge], allowlist: &[AllowEntry]) -> Vec<String> {
    let allowed: BTreeSet<(&str, &str)> = allowlist
        .iter()
        .map(|entry| (entry.file.as_str(), entry.module.as_str()))
        .collect();
    let found: BTreeSet<(&str, &str)> = edges
        .iter()
        .map(|edge| (edge.file.as_str(), edge.module.as_str()))
        .collect();
    let mut violations: Vec<String> = edges
        .iter()
        .filter(|edge| !allowed.contains(&(edge.file.as_str(), edge.module.as_str())))
        .map(|edge| {
            format!(
                "{}/{}:{}: {} -> {} via {}",
                src_dir(edge.src),
                edge.file,
                edge.line,
                shown(edge.src),
                shown(edge.dst),
                if edge.path == "crate" {
                    edge.path.clone()
                } else {
                    format!("crate::{}", edge.path)
                }
            )
        })
        .collect();
    violations.extend(
        allowlist
            .iter()
            .filter(|entry| !found.contains(&(entry.file.as_str(), entry.module.as_str())))
            .map(|entry| {
                format!(
                    "{ALLOWLIST}:{}: stale entry `{} -> {}`: that upward edge is gone, so \
                     remove the line",
                    entry.line, entry.file, entry.module
                )
            }),
    );
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(file: &str, source: &str) -> Vec<(usize, String)> {
        scan_paths(file, source)
            .hits
            .into_iter()
            .map(|hit| (hit.line, hit.path.join("::")))
            .collect()
    }

    fn tree(files: &[(&str, &str)]) -> BTreeMap<String, String> {
        files
            .iter()
            .map(|(name, text)| ((*name).to_owned(), (*text).to_owned()))
            .collect()
    }

    #[test]
    fn plain_crate_path() {
        assert_eq!(
            paths("theme/mod.rs", "use crate::window::main::ShellState;\n"),
            vec![(1, "window::main::ShellState".to_owned())]
        );
    }

    #[test]
    fn brace_groups_nested() {
        let source = "use crate::{a::x, b::{y, z::{self, w}}, c::*, d as e};\n";
        assert_eq!(
            paths("lib.rs", source),
            vec![
                (1, "a::x".to_owned()),
                (1, "b::y".to_owned()),
                (1, "b::z".to_owned()),
                (1, "b::z::w".to_owned()),
                (1, "c".to_owned()),
                (1, "d".to_owned()),
            ]
        );
    }

    #[test]
    fn multi_line_use() {
        let source = "use crate::{\n    theme::Theme,\n    components::{\n        button::Button,\n        toast,\n    },\n};\n";
        assert_eq!(
            paths("lib.rs", source),
            vec![
                (2, "theme::Theme".to_owned()),
                (4, "components::button::Button".to_owned()),
                (5, "components::toast".to_owned()),
            ]
        );
    }

    #[test]
    fn super_chains_from_file_and_mod_rs() {
        // `a/b/c.rs` is module a::b::c.
        assert_eq!(
            paths("a/b/c.rs", "use super::x;\nuse super::super::y::Z;\n"),
            vec![(1, "a::b::x".to_owned()), (2, "a::y::Z".to_owned())]
        );
        // `a/b/mod.rs` is module a::b.
        assert_eq!(
            paths("a/b/mod.rs", "use super::x;\nuse self::c::D;\n"),
            vec![(1, "a::x".to_owned()), (2, "a::b::c::D".to_owned())]
        );
        assert_eq!(
            paths("a/b.rs", "use super::super::super::q;\n"),
            vec![(1, "q".to_owned())]
        );
    }

    #[test]
    fn super_inside_inline_module() {
        let source = "fn f() {}\n#[cfg(test)]\nmod tests {\n    use super::*;\n    use super::super::Up;\n}\nuse super::After;\n";
        assert_eq!(
            paths("a/b.rs", source),
            vec![
                (4, "a::b".to_owned()),
                (5, "a::Up".to_owned()),
                (7, "a::After".to_owned()),
            ]
        );
    }

    #[test]
    fn comments_and_strings_are_skipped() {
        let source = "// use crate::window::main;\n/// [`crate::window::main`]\n/* crate::app::X\n crate::app::Y */\nlet s = \"crate::app::Z\";\nlet r = r#\"crate::app::W\"#;\nlet c = '{';\nuse crate::kept::K;\n";
        assert_eq!(paths("lib.rs", source), vec![(8, "kept::K".to_owned())]);
    }

    #[test]
    fn non_paths_are_skipped() {
        let source = "pub(crate) fn f(&self) { self.x; Self::new(); my_crate::y(); }\n";
        assert!(paths("lib.rs", source).is_empty());
    }

    #[test]
    fn expression_paths_and_turbofish() {
        let source = "let v = crate::app::Thing { a: 1 };\nlet w = crate::util::parse::<u8>(s);\n";
        assert_eq!(
            paths("lib.rs", source),
            vec![(1, "app::Thing".to_owned()), (2, "util::parse".to_owned())]
        );
    }

    #[test]
    fn layer_table_longest_prefix_wins() {
        assert_eq!(layer_of("pages/performance/mod.rs"), Some("tl-ui-perf"));
        assert_eq!(
            layer_of("pages/performance/core/actions.rs"),
            Some("tl-ui-perf-core")
        );
        assert_eq!(
            layer_of("pages/performance/core/keys.rs"),
            Some("tl-ui-perf-core")
        );
        assert_eq!(
            layer_of("pages/performance/widget/grid.rs"),
            Some("tl-ui-overview")
        );
        assert_eq!(layer_of("motion.rs"), Some("tl-ui-base"));
        assert_eq!(layer_of("actions.rs"), Some("tl-ui-kit"));
        assert_eq!(layer_of("components/button.rs"), Some("tl-ui-kit"));
        assert_eq!(layer_of("pages/mod.rs"), Some(SHELL));
        assert_eq!(layer_of("theme/mod.rs"), Some("tl-ui-base"));
        // A directory prefix also matches the module file next to it.
        assert_eq!(layer_of("chart.rs"), Some("tl-ui-chart"));
        assert_eq!(layer_of("pages/new_page.rs"), None);
    }

    #[test]
    fn every_edge_row_names_known_crates() {
        let names: BTreeSet<&str> = EDGES.iter().map(|(name, _)| *name).collect();
        for (krate, row) in EDGES {
            for dst in *row {
                assert!(names.contains(dst), "{krate} lists unknown crate {dst}");
            }
        }
        for (_, krate) in LAYERS {
            assert!(
                names.contains(krate),
                "layer crate {krate} has no EDGES row"
            );
        }
    }

    const SAMPLE: &[(&str, &str)] = &[
        (
            "lib.rs",
            "pub mod theme;\npub mod window;\npub mod components;\n",
        ),
        ("theme/mod.rs", "use crate::components::button::Button;\n"),
        ("components/mod.rs", "pub mod button;\n"),
        ("components/button.rs", "use crate::theme::Theme;\n"),
        ("window/mod.rs", "pub mod main;\n"),
        ("window/main.rs", "use crate::theme::Theme;\n"),
    ];

    #[test]
    fn allowed_edge_is_not_reported() {
        let scan = scan_tree(&tree(SAMPLE));
        assert!(scan.unmapped.is_empty());
        // kit -> base and shell -> base are allowed; base -> kit is the only upward edge.
        assert_eq!(scan.edges.len(), 1);
        assert_eq!(scan.edges[0].file, "theme/mod.rs");
        assert!(
            check(
                &scan.edges,
                &parse_allowlist("theme/mod.rs -> components::button").unwrap()
            )
            .is_empty()
        );
    }

    #[test]
    fn rejected_edge_is_reported() {
        let mut files = tree(SAMPLE);
        files.insert(
            "theme/store.rs".to_owned(),
            "fn f() {}\nuse crate::window::main::ShellState;\n".to_owned(),
        );
        let scan = scan_tree(&files);
        let allow = parse_allowlist("theme/mod.rs -> components::button\n").unwrap();
        assert_eq!(
            check(&scan.edges, &allow),
            vec![
                "crates/tl-ui-base/src/theme/store.rs:2: tl-ui-base -> tl-ui (shell) via \
                 crate::window::main::ShellState"
                    .to_owned()
            ]
        );
    }

    #[test]
    fn stale_allowlist_entry_is_reported() {
        let scan = scan_tree(&tree(SAMPLE));
        let allow = parse_allowlist(
            "# header\n\ntheme/mod.rs -> components::button\ntheme/mod.rs -> window::main\n",
        )
        .unwrap();
        assert_eq!(
            check(&scan.edges, &allow),
            vec![
                "xtask/check_layers_allow.txt:4: stale entry `theme/mod.rs -> window::main`: \
                 that upward edge is gone, so remove the line"
                    .to_owned()
            ]
        );
    }

    #[test]
    fn written_allowlist_passes_and_summarises() {
        let files = tree(SAMPLE);
        let scan = scan_tree(&files);
        let text = render_allowlist(&scan.edges);
        let allow = parse_allowlist(&text).unwrap();
        assert!(check(&scan.edges, &allow).is_empty());
        let names: BTreeSet<String> = files.keys().cloned().collect();
        assert_eq!(
            summary_lines(&allow, &names),
            vec![
                "tl-ui-base -> tl-ui-kit: 1".to_owned(),
                "total: 1".to_owned()
            ]
        );
    }

    #[test]
    fn unmapped_file_is_reported() {
        let mut files = tree(SAMPLE);
        files.insert("pages/new_page.rs".to_owned(), String::new());
        assert_eq!(
            scan_tree(&files).unmapped,
            vec!["pages/new_page.rs".to_owned()]
        );
    }

    #[test]
    fn bad_allowlist_lines_fail() {
        assert!(parse_allowlist("no arrow here\n").is_err());
        assert!(parse_allowlist("a.rs -> b\na.rs -> b\n").is_err());
    }

    #[test]
    fn super_inside_use_group() {
        // `theme/store.rs` is module theme::store, so `super::{super::x}` is the crate root.
        let source = "use super::{super::window::main::ShellState, Local};\nuse self::super::Up;\n";
        assert_eq!(
            paths("theme/store.rs", source),
            vec![
                (1, "window::main::ShellState".to_owned()),
                (1, "theme::Local".to_owned()),
                (2, "theme::Up".to_owned()),
            ]
        );
    }

    #[test]
    fn grouped_super_cannot_bypass_the_check() {
        let mut files = tree(SAMPLE);
        files.insert(
            "theme/store.rs".to_owned(),
            "use super::{super::window::main::ShellState};\n".to_owned(),
        );
        let scan = scan_tree(&files);
        let allow = parse_allowlist("theme/mod.rs -> components::button\n").unwrap();
        assert_eq!(
            check(&scan.edges, &allow),
            vec![
                "crates/tl-ui-base/src/theme/store.rs:1: tl-ui-base -> tl-ui (shell) via \
                 crate::window::main::ShellState"
                    .to_owned()
            ]
        );
    }

    #[test]
    fn edges_into_testkit_are_allowed_from_tests_only() {
        assert!(edge_allowed("tl-ui-base", TESTKIT, true));
        assert!(!edge_allowed("tl-ui-base", TESTKIT, false));
        assert!(!edge_allowed(TESTKIT, "tl-ui-base", true));
        assert!(may_depend(SHELL, "tl-ui-sys"));
        assert!(!may_depend("tl-ui-procs", "tl-ui-sys"));
        assert!(!may_depend("tl-ui-overview", "tl-ui-perf"));
    }

    #[test]
    fn testkit_from_production_code_is_reported() {
        let mut files = tree(SAMPLE);
        files.insert("testing/mod.rs".to_owned(), "pub mod fake;\n".to_owned());
        files.insert("testing/fake.rs".to_owned(), String::new());
        files.insert(
            "theme/store.rs".to_owned(),
            "#[cfg(any(test, feature = \"test-hooks\"))]\nuse crate::testing::fake::Fake;\n\
             #[cfg(test)]\nmod tests {\n    use crate::testing::fake::InTest;\n}\n\
             #[cfg(test)]\nuse crate::testing::fake::TestUse;\n\
             #[cfg(test)]\nmod more;\n"
                .to_owned(),
        );
        files.insert(
            "theme/store/more.rs".to_owned(),
            "mod deeper;\nuse crate::testing::fake::FromTestFile;\n".to_owned(),
        );
        files.insert(
            "theme/store/more/deeper.rs".to_owned(),
            "use crate::testing::fake::Deeper;\n".to_owned(),
        );
        let scan = scan_tree(&files);
        let allow = parse_allowlist("theme/mod.rs -> components::button\n").unwrap();
        assert_eq!(
            check(&scan.edges, &allow),
            vec![
                "crates/tl-ui-base/src/theme/store.rs:2: tl-ui-base -> tl-ui-testkit via \
                 crate::testing::fake::Fake"
                    .to_owned()
            ]
        );
    }

    #[test]
    fn cfg_test_mask_covers_the_item_only() {
        let source = "#[cfg(test)]\nconst A: [u8; 2] = [1, 2];\nfn after() {}\n";
        let code: Vec<char> = source.chars().collect();
        let mask = cfg_test_mask(&code);
        let a = source.find("A:").unwrap();
        let after = source.find("after").unwrap();
        assert!(mask[a]);
        assert!(!mask[after]);
        let inner = "mod m {\n    #![cfg(test)]\n    fn t() {}\n}\nfn out() {}\n";
        let code: Vec<char> = inner.chars().collect();
        let mask = cfg_test_mask(&code);
        assert!(mask[inner.find("fn t").unwrap()]);
        assert!(!mask[inner.find("fn out").unwrap()]);
    }
}
