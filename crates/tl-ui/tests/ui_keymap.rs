//! Shell keymap tests: the Global rows of canonical table (b) and the two generic setting
//! Actions (E20-002, UI-SHL-120). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{Action, App, KeyContext, Keystroke, TestAppContext};
use tl_model::client::EngineClient;
use tl_model::settings::{Settings, UiState};
use tl_model::table::TableMode;
use tl_ui::actions::{
    CancelWindowPick, CloseWindow, EscapeChain, FilterAccept, FilterEscape, FocusFilter, MenuId,
    NextTab, OpenMenu, OpenRunDialog, OpenSettings, OpenShortcuts, PrevTab, Quit,
    RefreshCurrentTab, ScaleDown, ScaleReset, ScaleUp, SelectTab, SetSetting, ToggleMiniWindow,
    ToggleSetting, bind_keys_global, register_global_handlers,
};
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::slider::{SliderBigStepDown, SliderBigStepUp, SliderStepDown, SliderStepUp};
use tl_ui::settings_global::{SettingKey, SettingValue, TlSettings};
use tl_ui::testing::fake_engine::FakeEngine;

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

fn bound(cx: &mut TestAppContext) {
    cx.update(bind_keys_global);
}

#[gpui_kit::test]
fn keymap_matches_table_b(cx: &mut TestAppContext) {
    bound(cx);
    let g = &["Global"];
    let mut rows: Vec<(String, Box<dyn Action>)> = Vec::new();
    for i in 0..6 {
        rows.push((format!("ctrl-{}", i + 1), Box::new(SelectTab(i))));
        rows.push((format!("alt-{}", i + 1), Box::new(SelectTab(i))));
    }
    let fixed: Vec<(&str, Box<dyn Action>)> = vec![
        ("ctrl-tab", Box::new(NextTab)),
        ("ctrl-shift-tab", Box::new(PrevTab)),
        ("ctrl-n", Box::new(OpenRunDialog)),
        ("ctrl-q", Box::new(Quit)),
        ("ctrl-w", Box::new(CloseWindow)),
        ("ctrl-,", Box::new(OpenSettings)),
        ("ctrl-?", Box::new(OpenShortcuts)),
        ("ctrl-shift-/", Box::new(OpenShortcuts)),
        ("f10", Box::new(OpenMenu(MenuId::File))),
        ("alt-f", Box::new(OpenMenu(MenuId::File))),
        ("alt-v", Box::new(OpenMenu(MenuId::View))),
        ("alt-s", Box::new(OpenMenu(MenuId::Settings))),
        ("alt-h", Box::new(OpenMenu(MenuId::Help))),
        ("alt-d", Box::new(OpenMenu(MenuId::Debug))),
        ("ctrl-f", Box::new(FocusFilter)),
        ("f5", Box::new(RefreshCurrentTab)),
        ("escape", Box::new(EscapeChain)),
        ("ctrl-=", Box::new(ScaleUp)),
        ("ctrl-+", Box::new(ScaleUp)),
        ("ctrl-shift-=", Box::new(ScaleUp)),
        ("ctrl--", Box::new(ScaleDown)),
        ("ctrl-0", Box::new(ScaleReset)),
        ("ctrl-shift-m", Box::new(ToggleMiniWindow)),
    ];
    rows.extend(fixed.into_iter().map(|(k, a)| (k.to_string(), a)));

    cx.read(|cx| {
        assert_resolves(cx, "ctrl-3", g, &SelectTab(2));
        for (keys, action) in &rows {
            assert_resolves(cx, keys, g, action.as_ref());
        }
    });
}

#[gpui_kit::test]
fn picker_escape_wins_over_global(cx: &mut TestAppContext) {
    bound(cx);
    cx.read(|cx| {
        assert_resolves(cx, "escape", &["Global", "Picker"], &CancelWindowPick);
        assert_resolves(cx, "escape", &["Global"], &EscapeChain);
    });
}

#[gpui_kit::test]
fn shell_filter_keys(cx: &mut TestAppContext) {
    bound(cx);
    let stack = &["Global", "ShellFilter"];
    cx.read(|cx| {
        assert_resolves(cx, "escape", stack, &FilterEscape);
        assert_resolves(cx, "enter", stack, &FilterAccept);
        assert_resolves(cx, "down", stack, &FilterAccept);
    });
}

/// `bind_keys_global` also binds the component keys (leader note on jqk.4).
#[gpui_kit::test]
fn slider_gets_arrow_keys(cx: &mut TestAppContext) {
    bound(cx);
    let stack = &["Global", "TlSlider"];
    cx.read(|cx| {
        assert_resolves(cx, "left", stack, &SliderStepDown);
        assert_resolves(cx, "right", stack, &SliderStepUp);
        assert_resolves(cx, "shift-left", stack, &SliderBigStepDown);
        assert_resolves(cx, "shift-right", stack, &SliderBigStepUp);
    });
}

/// Installs `EngineGlobal`, `BootSettings` and `TlSettings`, then the setting handlers.
fn install(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine;
    cx.update(|cx| {
        cx.set_global(EngineGlobal(client));
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
            persist: Some(Box::new(|_, _| {})),
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        register_global_handlers(cx);
    });
}

#[gpui_kit::test]
fn toggle_setting_action_flips_key(cx: &mut TestAppContext) {
    install(cx);
    assert!(cx.read(|cx| !TlSettings::settings(cx).window.close_with_esc));

    cx.update(|cx| cx.dispatch_action(&ToggleSetting(SettingKey::WindowCloseWithEsc)));

    assert!(cx.read(|cx| TlSettings::settings(cx).window.close_with_esc));
}

#[gpui_kit::test]
fn set_setting_action_writes_enum(cx: &mut TestAppContext) {
    install(cx);

    cx.update(|cx| {
        cx.dispatch_action(&SetSetting {
            key: SettingKey::ProcessesMode,
            value: SettingValue::Str("flat".into()),
        })
    });

    assert_eq!(
        cx.read(|cx| TlSettings::settings(cx).processes.mode),
        TableMode::Flat
    );
}
