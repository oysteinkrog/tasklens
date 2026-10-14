//! Services action and key binding tests (tasklens-hk6.34, canonical table (b)).

use gpui_kit::base::Root;
use gpui_kit::{
    Action, AppContext as _, Context, IntoElement, KeyContext, Render, TestAppContext,
    VisualTestContext, Window, div,
};
use tl_ui::pages::services::actions::*;

struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn assert_window_binding(
    vcx: &mut VisualTestContext,
    action: &dyn Action,
    context: &str,
    keys: &str,
) {
    let parsed = KeyContext::parse(context).expect("key context");
    let bindings = vcx.update(|window, _| window.bindings_for_action_in_context(action, parsed));
    assert!(
        bindings.iter().any(|binding| {
            binding.keystrokes().len() == 1 && binding.keystrokes()[0].unparse() == keys
        }),
        "{keys} in {context:?} is not bound to {action:?}; got {bindings:?}"
    );
}

#[gpui_kit::test]
fn services_keymap_contexts(cx: &mut TestAppContext) {
    cx.update(|cx| {
        tl_ui::table::actions::bind_keys_table(cx);
        bind_keys_services(cx);
    });
    let handle = cx.add_window(|window, cx| Root::new(cx.new(|_| Host), window, cx));
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();

    let rows: [(&str, &str, Box<dyn Action>); 16] = [
        ("ctrl-s", "Table ServiceTable", Box::new(ServiceStart)),
        ("ctrl-e", "Table ServiceTable", Box::new(ServiceStop)),
        ("ctrl-r", "Table ServiceTable", Box::new(ServiceRestart)),
        ("ctrl-tab", "ServiceProps", Box::new(ServicePropsNextTab)),
        (
            "ctrl-pagedown",
            "ServiceProps",
            Box::new(ServicePropsNextTab),
        ),
        (
            "ctrl-shift-tab",
            "ServiceProps",
            Box::new(ServicePropsPrevTab),
        ),
        ("ctrl-pageup", "ServiceProps", Box::new(ServicePropsPrevTab)),
        (
            "ctrl-1",
            "ServiceProps",
            Box::new(ServicePropsSelectTab { index: 0 }),
        ),
        (
            "ctrl-2",
            "ServiceProps",
            Box::new(ServicePropsSelectTab { index: 1 }),
        ),
        (
            "ctrl-3",
            "ServiceProps",
            Box::new(ServicePropsSelectTab { index: 2 }),
        ),
        (
            "ctrl-4",
            "ServiceProps",
            Box::new(ServicePropsSelectTab { index: 3 }),
        ),
        (
            "ctrl-5",
            "ServiceProps",
            Box::new(ServicePropsSelectTab { index: 4 }),
        ),
        ("enter", "ServiceProps", Box::new(ServicePropsOk)),
        ("ctrl-a", "JournalList", Box::new(ServiceLogsSelectAll)),
        ("ctrl-c", "JournalList", Box::new(ServiceLogsCopySelected)),
        ("home", "JournalList", Box::new(ServiceLogsTop)),
    ];
    for (keys, context, action) in rows {
        assert_window_binding(&mut vcx, action.as_ref(), context, keys);
    }
    assert_window_binding(&mut vcx, &ServiceLogsBottom, JOURNAL_LIST_CONTEXT, "end");

    let process_context = KeyContext::parse("ProcessTable").expect("ProcessTable context");
    let process_bindings = vcx
        .update(|window, _| window.bindings_for_action_in_context(&ServiceStart, process_context));
    assert!(
        process_bindings.is_empty(),
        "ctrl-s in ProcessTable must not resolve to ServiceStart"
    );
}

#[test]
fn unit_type_arg_round_trip() {
    let values = [
        UnitTypeArg::Service,
        UnitTypeArg::Socket,
        UnitTypeArg::Timer,
        UnitTypeArg::Mount,
        UnitTypeArg::Automount,
        UnitTypeArg::Path,
        UnitTypeArg::Target,
        UnitTypeArg::Swap,
        UnitTypeArg::Slice,
        UnitTypeArg::Scope,
    ];

    for value in values {
        assert_eq!(UnitTypeArg::from_setting(value.as_setting()), Some(value));
    }
    assert_eq!(UnitTypeArg::from_setting("device"), None);
}
