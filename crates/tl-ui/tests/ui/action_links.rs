//! Actions link test (ADR-020 "Actions", tasklens-zljr5.34).
//!
//! An action declared in a crate that the binary does not link is missing from GPUI's action
//! registry, so a keymap entry for it stops resolving. Each tl-ui-* crate that declares actions
//! exposes an `init` or `bind_keys_*` function that `TasklensUi::init` calls, and that call keeps
//! the crate linked. Two checks:
//!
//! - `keymap_actions_resolve`: after app init, every action the keymap binds builds by name.
//! - `every_crate_with_actions_is_initialised`: a source sweep. Every tl-ui-* crate that declares
//!   an action (`actions!(` or `#[derive(.., Action)]`) has a function called from
//!   `TasklensUi::init` in `crates/tl-ui/src/app.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{ActionBuildError, TestAppContext};
use tl_model::client::EngineClient;
use tl_ui::app::{AppEnv, TasklensUi, UiInit};
use tl_ui_settings::settings::{Settings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

fn app_init(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine;
    let (_tx, rx) = async_channel::bounded(4);
    let init = UiInit {
        settings: Settings::default(),
        state: UiState::default(),
        themes_json: None,
        persist: Box::new(|_, _| {}),
        persist_themes: Box::new(|_| {}),
        app_requests: rx,
        startup_toasts: Vec::new(),
        env: AppEnv::test(),
    };
    cx.update(|cx| TasklensUi::init(cx, client, init));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn keymap_actions_resolve(cx: &mut TestAppContext) {
    app_init(cx);
    cx.update(|cx| {
        let keymap = cx.key_bindings();
        let names: BTreeSet<&'static str> = keymap
            .borrow()
            .bindings()
            .map(|binding| binding.action().name())
            .collect();
        assert!(
            names.len() > 50,
            "app init bound only {} actions",
            names.len()
        );
        let mut missing = Vec::new();
        let mut unbuilt = BTreeMap::new();
        for name in &names {
            match cx.build_action(name, None) {
                Ok(_) => {}
                Err(ActionBuildError::NotFound { .. }) => missing.push(*name),
                Err(err @ ActionBuildError::BuildError { .. }) => {
                    unbuilt.insert(*name, err.to_string());
                }
            }
        }
        assert!(
            missing.is_empty(),
            "bound actions that do not resolve by name: {missing:?}"
        );
        let expected: BTreeSet<&str> = DATA_ACTIONS.iter().copied().collect();
        let unexpected: BTreeMap<_, _> = unbuilt
            .iter()
            .filter(|(name, _)| !expected.contains(**name))
            .collect();
        assert!(
            unexpected.is_empty(),
            "bound actions that do not build from no input and are not in DATA_ACTIONS: \
             {unexpected:#?}"
        );
        let stale: Vec<&str> = expected
            .iter()
            .copied()
            .filter(|name| !unbuilt.contains_key(name))
            .collect();
        assert!(
            stale.is_empty(),
            "DATA_ACTIONS entries that are not bound or now build from no input: {stale:?}"
        );
    });
}

/// The bound actions that carry data, such as `SelectTab(n)` or `PerfWidgetMove { dx, dy }`.
/// They resolve by name but do not build from the empty input `None` stands for. Every other
/// bound action must build. Keep this list exact: `keymap_actions_resolve` fails on a name
/// that is missing here and on a name here that is not bound or builds.
const DATA_ACTIONS: &[&str] = &[
    "tasklens::MoveFocusEdge",
    "tasklens::MoveFocusPage",
    "tasklens::OpenMenu",
    "tasklens::PerfMoveFocusedDevice",
    "tasklens::PerfOverviewMoveBox",
    "tasklens::PerfOverviewResizeBox",
    "tasklens::PerfSelectVisibleDevice",
    "tasklens::PerfWidgetMove",
    "tasklens::PerfWidgetResize",
    "tasklens::PrioritySliderStep",
    "tasklens::PropsSelectTab",
    "tasklens::SelectHost",
    "tasklens::SelectTab",
    "tasklens::ServicePropsSelectTab",
];

#[test]
fn every_crate_with_actions_is_initialised() {
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf();
    let app_rs = std::fs::read_to_string(crates_dir.join("tl-ui/src/app.rs")).expect("app.rs");

    // Every tl-ui-* crate: its .rs files by module stem, and whether it declares an action.
    let mut sources: BTreeMap<String, Vec<(PathBuf, String)>> = BTreeMap::new();
    let mut declaring = BTreeSet::new();
    for entry in std::fs::read_dir(&crates_dir).expect("list crates/") {
        let dir = entry.expect("crates/ entry").path();
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if !name.starts_with("tl-ui-") {
            continue;
        }
        let mut files = Vec::new();
        rs_files(&dir.join("src"), &mut files);
        for (_, text) in &files {
            if declares_action(text) {
                declaring.insert(name.to_owned());
            }
        }
        sources.insert(name.to_owned(), files);
    }
    assert!(
        declaring.len() >= 5,
        "the sweep found actions in only {declaring:?}"
    );

    let mut called = BTreeSet::new();
    for path in init_calls(&app_rs) {
        for (krate, files) in &sources {
            if defines(files, &path) {
                called.insert(krate.clone());
            }
        }
    }
    let missing: Vec<&String> = declaring.difference(&called).collect();
    assert!(
        missing.is_empty(),
        "crates that declare actions but have no init or bind_keys call in TasklensUi::init: \
         {missing:?} (called: {called:?})"
    );
}

fn rs_files(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    for entry in std::fs::read_dir(dir).expect("list src/") {
        let path = entry.expect("src/ entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).expect("read .rs file");
            out.push((path, text));
        }
    }
}

/// True when a line outside a `//` comment has `actions!(` or a derive list with `Action`.
fn declares_action(text: &str) -> bool {
    text.lines().map(str::trim_start).any(|line| {
        if line.starts_with("//") {
            return false;
        }
        if line.contains("actions!(") {
            return true;
        }
        line.split("derive(").skip(1).any(|rest| {
            rest.split(')')
                .next()
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .any(|d| d == "Action" || d.ends_with("::Action"))
        })
    })
}

/// The paths of the calls in the body of `TasklensUi::init`, such as
/// `["crate", "table", "actions", "bind_keys_table"]`: each statement line that is a path call
/// ending in `cx);`.
fn init_calls(app_rs: &str) -> Vec<Vec<String>> {
    let start = app_rs
        .find("pub fn init(cx: &mut App, client")
        .expect("TasklensUi::init in app.rs");
    let body = &app_rs[start..];
    let end = body
        .find("perf_step(\"ui_init\")")
        .expect("end of TasklensUi::init");
    body[..end]
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//") && line.ends_with("cx);"))
        .filter_map(|line| {
            let head = line.split('(').next()?;
            let segments: Vec<String> = head.split("::").map(str::to_owned).collect();
            let ok = segments.len() > 1
                && segments
                    .iter()
                    .all(|s| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_'));
            ok.then_some(segments)
        })
        .collect()
}

/// True when one of `files` defines the function `path` ends in and its module is named in
/// `path`: `toast::init` is `fn init(` in `toast.rs`, `processes::bind_keys_processes` is in
/// `processes/mod.rs`.
fn defines(files: &[(PathBuf, String)], path: &[String]) -> bool {
    let (func, modules) = path.split_last().expect("a path");
    let def = format!("fn {func}(");
    files.iter().any(|(file, text)| {
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let module = if stem == "mod" {
            file.parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .unwrap_or_default()
        } else {
            stem
        };
        modules.iter().any(|m| m == module) && text.contains(&def)
    })
}
