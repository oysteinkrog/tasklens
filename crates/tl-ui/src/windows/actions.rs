//! Process window Actions and their key bindings (E23, UI-PWN conventions "Actions", canonical
//! table (b) rows PropsWindow, KvGrid, AffinityDialog, PriorityDialog, SignalDialog, RunDialog
//! and HandleSearch, UI-PRC-013, UI-PRC-020, R-120, R-128, R-133, D-008).
//!
//! Naming rule: an Action named in the UI docs keeps that name. An Action the docs do not name
//! gets the R-133 prefix of its window: `Props`, `Affinity`, `Priority`, `Signal`, `Run` or
//! `HandleSearch`. All live in the `tasklens` namespace.
//!
//! `OpenVirusTotal` and `CreateDump` are declared by the Processes page (`pages/processes/
//! actions.rs`) and re-exported here: GPUI fails on a duplicate name in one namespace.
//! `OpenRunDialog`, `OpenHandleSearch` and `FocusFilter` live in the shell `actions.rs`,
//! `OpenProperties` in `table/actions.rs`.
//!
//! Slider Left/Right/Home/End and stepper Up/Down in PriorityDialog and SignalDialog are the
//! foundation components' own keys (TlSlider, TlStepper) and are not bound here.

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, App, KeyBinding, gpui};
use serde::{Deserialize, Serialize};
use tl_model::actions::SchedPolicy;

pub use crate::pages::processes::actions::{CreateDump, OpenVirusTotal};

/// Key context of a Properties window root.
pub const PROPS_WINDOW_CONTEXT: &str = "PropsWindow";
/// Key context of the field grid inside a Properties tab.
pub const KV_GRID_CONTEXT: &str = "KvGrid";
/// Key context of the Strings tab options bar.
pub const STRINGS_OPTIONS_CONTEXT: &str = "StringsOptions";
/// Key context of the Set Affinity dialog.
pub const AFFINITY_DIALOG_CONTEXT: &str = "AffinityDialog";
/// Key context of the Custom priority dialog.
pub const PRIORITY_DIALOG_CONTEXT: &str = "PriorityDialog";
/// Key context of the Real-time signal dialog.
pub const SIGNAL_DIALOG_CONTEXT: &str = "SignalDialog";
/// Key context of the Run window.
pub const RUN_DIALOG_CONTEXT: &str = "RunDialog";
/// Key context of the Search File Handles window.
pub const HANDLE_SEARCH_CONTEXT: &str = "HandleSearch";

gpui_kit::actions!(
    tasklens,
    [
        // Row-menu items; they carry nothing and read the captured row-menu targets
        // (UI-PRC-013).
        SetAffinity,
        OpenCustomPriority,
        SignalOpenRealtime,
        PropsNextTab,
        PropsPrevTab,
        PropsRefreshNow,
        PropsFocusFilter,
        PropsCloseWindow,
        PropsCopyAllFields,
        PropsCopyValue,
        PropsActivateField,
        PropsFieldMenu,
        PropsFieldUp,
        PropsFieldDown,
        PropsFieldLeft,
        PropsFieldRight,
        PropsFieldHome,
        PropsFieldEnd,
        PropsToggleRevealEnv,
        PropsVerifyPackage,
        PropsStartStringScan,
        PropsCancelStringScan,
        PropsEscape,
        AffinityApply,
        AffinityReset,
        AffinityToggle,
        AffinityToggleGroup,
        AffinityMoveLeft,
        AffinityMoveRight,
        AffinityMoveUp,
        AffinityMoveDown,
        AffinityHome,
        AffinityEnd,
        PriorityApply,
        SignalSendRealtime,
        RunCommand,
        RunCommandElevated,
        ClearRunHistory,
        BrowseRunTarget,
        RunHistoryUp,
        RunHistoryDown,
        StartHandleSearch,
        /// Stops a running search, else closes the window when idle (UI-PWN-030).
        StopHandleSearch,
        ClearHandleSearch,
        HandleSearchAll,
        HandleSearchEndTask,
        HandleSearchEndProcess,
        HandleSearchGoToProcess,
        HandleSearchCopyPaths,
        HandleSearchCopyRows,
        OpenDumpFolder,
    ]
);

/// Selects Properties tab `0` (1..=7, Alt+1 to Alt+7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PropsSelectTab(pub u8);

/// Sends signal `signo` to the captured process targets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct SendSignal {
    pub signo: i32,
}

/// Sets the nice value of the selected thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct SetThreadNice(pub i8);

/// Sets the scheduling policy of the selected thread. `rt_prio` is 1..=99 for `fifo` and
/// `rr`, else 0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct SetThreadSched {
    pub policy: SchedPolicyWire,
    pub rt_prio: u8,
}

/// Sends signal `signo` to the selected thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct SignalThread {
    pub signo: i32,
}

/// Moves the Custom priority slider by `delta` (Page Up +5, Page Down -5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct PrioritySliderStep {
    pub delta: i8,
}

/// JSON form of [`SchedPolicy`]: tl-model has no JSON schema derive (ADR-001).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(crate = "gpui::private::schemars")]
pub enum SchedPolicyWire {
    Other,
    Batch,
    Idle,
    Fifo,
    Rr,
}

impl From<SchedPolicyWire> for SchedPolicy {
    fn from(w: SchedPolicyWire) -> Self {
        match w {
            SchedPolicyWire::Other => Self::Other,
            SchedPolicyWire::Batch => Self::Batch,
            SchedPolicyWire::Idle => Self::Idle,
            SchedPolicyWire::Fifo => Self::Fifo,
            SchedPolicyWire::Rr => Self::Rr,
        }
    }
}

impl From<SchedPolicy> for SchedPolicyWire {
    fn from(p: SchedPolicy) -> Self {
        match p {
            SchedPolicy::Other => Self::Other,
            SchedPolicy::Batch => Self::Batch,
            SchedPolicy::Idle => Self::Idle,
            SchedPolicy::Fifo => Self::Fifo,
            SchedPolicy::Rr => Self::Rr,
        }
    }
}

/// Binds the process window rows of canonical table (b).
pub fn bind_keys_process_windows(cx: &mut App) {
    let props = Some(PROPS_WINDOW_CONTEXT);
    let grid_ctx = format!("{PROPS_WINDOW_CONTEXT} > {KV_GRID_CONTEXT}");
    let grid = Some(grid_ctx.as_str());
    let strings_ctx = format!("{PROPS_WINDOW_CONTEXT} > {STRINGS_OPTIONS_CONTEXT}");
    let strings = Some(strings_ctx.as_str());
    let affinity = Some(AFFINITY_DIALOG_CONTEXT);
    let priority = Some(PRIORITY_DIALOG_CONTEXT);
    let signal = Some(SIGNAL_DIALOG_CONTEXT);
    let run = Some(RUN_DIALOG_CONTEXT);
    let search = Some(HANDLE_SEARCH_CONTEXT);
    let table_ctx = format!(
        "{HANDLE_SEARCH_CONTEXT} > {}",
        crate::table::actions::TABLE_CONTEXT
    );
    let search_table = Some(table_ctx.as_str());

    let tabs: Vec<KeyBinding> = (1..=7u8)
        .map(|i| KeyBinding::new(&format!("alt-{i}"), PropsSelectTab(i), props))
        .collect();
    cx.bind_keys(tabs);

    cx.bind_keys([
        KeyBinding::new("ctrl-tab", PropsNextTab, props),
        KeyBinding::new("ctrl-pagedown", PropsNextTab, props),
        KeyBinding::new("ctrl-shift-tab", PropsPrevTab, props),
        KeyBinding::new("ctrl-pageup", PropsPrevTab, props),
        KeyBinding::new("ctrl-f", PropsFocusFilter, props),
        KeyBinding::new("f5", PropsRefreshNow, props),
        KeyBinding::new("ctrl-w", PropsCloseWindow, props),
        KeyBinding::new("escape", PropsEscape, props),
        KeyBinding::new("up", PropsFieldUp, grid),
        KeyBinding::new("down", PropsFieldDown, grid),
        KeyBinding::new("left", PropsFieldLeft, grid),
        KeyBinding::new("right", PropsFieldRight, grid),
        KeyBinding::new("home", PropsFieldHome, grid),
        KeyBinding::new("end", PropsFieldEnd, grid),
        KeyBinding::new("enter", PropsActivateField, grid),
        KeyBinding::new("ctrl-c", PropsCopyValue, grid),
        KeyBinding::new("shift-f10", PropsFieldMenu, grid),
        KeyBinding::new("menu", PropsFieldMenu, grid),
        KeyBinding::new("enter", PropsStartStringScan, strings),
        KeyBinding::new("left", AffinityMoveLeft, affinity),
        KeyBinding::new("right", AffinityMoveRight, affinity),
        KeyBinding::new("up", AffinityMoveUp, affinity),
        KeyBinding::new("down", AffinityMoveDown, affinity),
        KeyBinding::new("home", AffinityHome, affinity),
        KeyBinding::new("end", AffinityEnd, affinity),
        KeyBinding::new("space", AffinityToggle, affinity),
        KeyBinding::new("shift-space", AffinityToggleGroup, affinity),
        KeyBinding::new("enter", AffinityApply, affinity),
        KeyBinding::new("pageup", PrioritySliderStep { delta: 5 }, priority),
        KeyBinding::new("pagedown", PrioritySliderStep { delta: -5 }, priority),
        KeyBinding::new("enter", PriorityApply, priority),
        KeyBinding::new("enter", SignalSendRealtime, signal),
        KeyBinding::new("up", RunHistoryUp, run),
        KeyBinding::new("down", RunHistoryDown, run),
        KeyBinding::new("enter", RunCommand, run),
        KeyBinding::new("shift-enter", RunCommandElevated, run),
        KeyBinding::new("enter", StartHandleSearch, search),
        KeyBinding::new("escape", StopHandleSearch, search),
        KeyBinding::new("delete", HandleSearchEndTask, search_table),
        KeyBinding::new("ctrl-e", HandleSearchEndTask, search_table),
        KeyBinding::new("shift-delete", HandleSearchEndProcess, search_table),
        KeyBinding::new("ctrl-x", HandleSearchEndProcess, search_table),
        KeyBinding::new("enter", HandleSearchGoToProcess, search_table),
        KeyBinding::new("ctrl-c", HandleSearchCopyPaths, search_table),
        KeyBinding::new("ctrl-shift-c", HandleSearchCopyRows, search_table),
    ]);
}

#[cfg(test)]
mod tests {
    use gpui_kit::{KeyContext, Keystroke, TestAppContext};

    use super::*;
    use crate::actions::bind_keys_global;
    use crate::table::actions::bind_keys_table;

    /// The Action the keymap resolves `keys` to with `contexts` as the stack (outermost first).
    fn resolve(cx: &App, keys: &str, contexts: &[&str]) -> Option<Box<dyn Action>> {
        let input: Vec<Keystroke> = keys
            .split(' ')
            .map(|k| Keystroke::parse(k).unwrap_or_else(|e| panic!("keystroke {k}: {e:?}")))
            .collect();
        let stack: Vec<KeyContext> = contexts
            .iter()
            .map(|c| KeyContext::parse(c).expect("key context"))
            .collect();
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let (bindings, _) = keymap.bindings_for_input(&input, &stack);
        bindings.first().map(|b| b.action().boxed_clone())
    }

    fn assert_resolves(cx: &App, keys: &str, contexts: &[&str], expected: &dyn Action) {
        let got = resolve(cx, keys, contexts);
        assert!(
            got.as_ref().is_some_and(|a| a.partial_eq(expected)),
            "{keys} in {contexts:?}: expected {expected:?}, got {got:?}"
        );
    }

    /// Bound in `TasklensUi::init` order: Global and Table first, so the process window rows
    /// must win over them.
    fn bound(cx: &mut TestAppContext) {
        cx.update(|cx| {
            bind_keys_global(cx);
            bind_keys_table(cx);
            bind_keys_process_windows(cx);
        });
    }

    #[gpui_kit::test]
    fn props_alt_digits_bound(cx: &mut TestAppContext) {
        bound(cx);
        cx.read(|cx| {
            for i in 1..=7u8 {
                assert_resolves(
                    cx,
                    &format!("alt-{i}"),
                    &["Global", PROPS_WINDOW_CONTEXT],
                    &PropsSelectTab(i),
                );
            }
        });
    }

    #[gpui_kit::test]
    fn run_dialog_keys_bound(cx: &mut TestAppContext) {
        bound(cx);
        let stack = &["Global", RUN_DIALOG_CONTEXT];
        cx.read(|cx| {
            assert_resolves(cx, "up", stack, &RunHistoryUp);
            assert_resolves(cx, "down", stack, &RunHistoryDown);
            assert_resolves(cx, "enter", stack, &RunCommand);
            assert_resolves(cx, "shift-enter", stack, &RunCommandElevated);
        });
    }

    #[gpui_kit::test]
    fn handle_search_table_keys_bound(cx: &mut TestAppContext) {
        bound(cx);
        let stack = &["Global", HANDLE_SEARCH_CONTEXT, "Table"];
        let rows: [(&str, Box<dyn Action>); 7] = [
            ("delete", Box::new(HandleSearchEndTask)),
            ("ctrl-e", Box::new(HandleSearchEndTask)),
            ("shift-delete", Box::new(HandleSearchEndProcess)),
            ("ctrl-x", Box::new(HandleSearchEndProcess)),
            ("enter", Box::new(HandleSearchGoToProcess)),
            ("ctrl-c", Box::new(HandleSearchCopyPaths)),
            ("ctrl-shift-c", Box::new(HandleSearchCopyRows)),
        ];
        cx.read(|cx| {
            for (keys, action) in &rows {
                assert_resolves(cx, keys, stack, action.as_ref());
            }
            let search = &["Global", HANDLE_SEARCH_CONTEXT];
            assert_resolves(cx, "escape", search, &StopHandleSearch);
            assert_resolves(cx, "enter", search, &StartHandleSearch);
        });
    }

    #[test]
    fn data_actions_roundtrip() {
        fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: T) {
            let json = serde_json::to_string(&v).expect("serialize");
            let back: T = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, v, "{json}");
        }
        roundtrip(SendSignal { signo: 10 });
        roundtrip(SetThreadNice(-5));
        roundtrip(SetThreadSched {
            policy: SchedPolicyWire::Fifo,
            rt_prio: 50,
        });
        roundtrip(PropsSelectTab(3));

        // The keymap form builds the same Action.
        let built = SetThreadSched::build(serde_json::json!({ "policy": "rr", "rt_prio": 1 }))
            .expect("build");
        assert!(built.partial_eq(&SetThreadSched {
            policy: SchedPolicyWire::Rr,
            rt_prio: 1,
        }));
        assert_eq!(
            SchedPolicy::from(SchedPolicyWire::from(SchedPolicy::Idle)),
            SchedPolicy::Idle
        );
    }
}
