//! Services delegate, action and key binding tests.

#![cfg(feature = "test-hooks")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    Action, AppContext as _, Context, Entity, IntoElement, KeyContext, Render, TestAppContext,
    VisualTestContext, Window, div,
};
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::health::ManagerState;
use tl_model::ids::ProcessKey;
use tl_model::settings::{AppearanceSettings, ThemeSettings, UiState};
use tl_model::snapshot::{ServicesSnapshot, Snapshot};
use tl_model::table::{Manager, ServiceColumn, SysColumnId, SysNodeId, SysRow, SysTable, UnitRef};
use tl_model::time::BootId;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::demand::{DEMAND_COALESCE, DemandAggregator, GlobalDemand};
use tl_ui::hub::SnapshotHub;
use tl_ui::pages::services::actions::*;
use tl_ui::pages::services::delegate::{ServicesDelegate, services_columns};
use tl_ui::pages::sys_table::{SysDelegate, SysQuery};
use tl_ui::settings_global::TlSettings;
use tl_ui::table::columns::ColumnLayout;
use tl_ui::table::tree_table::{RowKind, TlTreeTable, TreeTableDelegate};
use tl_ui::testing::fake_engine::FakeEngine;

struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn service_id(column: ServiceColumn) -> SysColumnId {
    SysColumnId::Services(column)
}

fn delegate_table_with_engine(
    cx: &mut TestAppContext,
    state: UiState,
) -> (Entity<TlTreeTable<ServicesDelegate>>, Arc<FakeEngine>) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        cx.set_global(BootSettings {
            settings: Default::default(),
            state: state.clone(),
            persist: Some(Box::new(|_, _| {})),
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        cx.set_global(EngineGlobal(client.clone()));
        let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        cx.set_global(GlobalDemand(demand));
    });
    let slot = Rc::new(RefCell::new(None));
    let keep = Rc::clone(&slot);
    cx.add_window(move |window, cx| {
        let table = cx.new(|cx| TlTreeTable::new(ServicesDelegate::new(&state), window, cx));
        *keep.borrow_mut() = Some(table.clone());
        Root::new(table, window, cx)
    });
    let table = slot.borrow().clone().expect("delegate table");
    (table, engine)
}

fn install_rows(delegate: &mut ServicesDelegate) {
    let unit = UnitRef {
        manager: Manager::System,
        name: Arc::from("sshd.service"),
    };
    let process = ProcessKey {
        start_ticks: 10,
        boot_id: BootId(1),
        pid: 812,
    };
    let rows: Arc<[SysRow]> = Arc::from([
        SysRow {
            node: SysNodeId::ServiceGroup(Manager::System),
            depth: 0,
            expanded: true,
            has_children: true,
            record: None,
        },
        SysRow {
            node: SysNodeId::Unit(unit.clone()),
            depth: 1,
            expanded: true,
            has_children: true,
            record: Some(0),
        },
        SysRow {
            node: SysNodeId::UnitProcess(unit, process),
            depth: 2,
            expanded: false,
            has_children: false,
            record: Some(0),
        },
        SysRow {
            node: SysNodeId::ServiceGroup(Manager::User),
            depth: 0,
            expanded: true,
            has_children: true,
            record: None,
        },
    ]);
    let mut snapshot = Snapshot::empty(BootId(1), 1);
    snapshot.services = Arc::new(ServicesSnapshot {
        rows,
        managers: [ManagerState::Connected, ManagerState::Unavailable],
        ..ServicesSnapshot::default()
    });
    delegate.install(&Arc::new(snapshot));
}

#[test]
fn services_default_columns_and_widths() {
    let visible: Vec<_> = services_columns()
        .iter()
        .filter(|column| column.visible)
        .map(|column| (column.label, column.default_width))
        .collect();
    assert_eq!(
        visible,
        [
            ("Name", 250.0),
            ("Display name", 300.0),
            ("Status", 100.0),
            ("Startup type", 120.0),
            ("PID", 80.0),
            ("User", 200.0),
            ("Path", 260.0),
        ]
    );
}

#[test]
fn services_row_kinds_and_selectable() {
    let mut delegate = ServicesDelegate::new(&UiState::default());
    install_rows(&mut delegate);
    assert_eq!(delegate.row_kind(0), RowKind::GroupHeader);
    assert_eq!(delegate.row_kind(1), RowKind::Unit);
    assert_eq!(delegate.row_kind(2), RowKind::Process);
    assert!(!delegate.selectable(0));
    assert!(delegate.selectable(1));
    assert!(delegate.selectable(2));
    assert_eq!(delegate.row_expandable(3), None);
}

#[gpui_kit::test]
fn services_cgroup_demand_follows_columns(cx: &mut TestAppContext) {
    let (table, engine) = delegate_table_with_engine(cx, UiState::default());
    cx.update(|cx| {
        table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.set_active(true, cx);
            assert!(!delegate.demand_part().service_cgroup_stats);
            let mut layout: ColumnLayout<SysColumnId> = delegate.default_layout();
            layout
                .set_visible(service_id(ServiceColumn::Memory), true)
                .expect("show memory");
            delegate.on_columns_changed(&layout, cx);
            assert!(delegate.demand_part().service_cgroup_stats);
        });
    });
    cx.executor().advance_clock(DEMAND_COALESCE * 2);
    cx.run_until_parked();
    assert!(
        engine
            .demands()
            .last()
            .is_some_and(|demand| demand.service_cgroup_stats)
    );

    cx.update(|cx| {
        table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            let mut layout = delegate.columns().clone();
            layout
                .set_visible(service_id(ServiceColumn::Memory), false)
                .expect("hide memory");
            delegate.on_columns_changed(&layout, cx);
        });
    });
    cx.executor().advance_clock(DEMAND_COALESCE * 2);
    cx.run_until_parked();
    assert!(
        engine
            .demands()
            .last()
            .is_some_and(|demand| !demand.service_cgroup_stats)
    );
}

#[gpui_kit::test]
fn services_sort_persists_and_sends_query(cx: &mut TestAppContext) {
    let (table, engine) = delegate_table_with_engine(cx, UiState::default());
    cx.update(|cx| {
        table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.on_sort(service_id(ServiceColumn::Status), cx);
            let request = delegate.take_sort_request().expect("sort request");
            assert_eq!(request.column, service_id(ServiceColumn::Status));
            assert!(request.ascending);
            let mut query = SysQuery::new(SysTable::Services, delegate.sort(), None);
            query.set_sort(request, engine.as_ref());
        });
        let saved = &TlSettings::state(cx).sort.services;
        assert_eq!(saved.column, "status");
        assert!(saved.ascending);
    });
    let queries: Vec<_> = engine
        .commands()
        .into_iter()
        .filter_map(|command| match command {
            Command::SetSysTableQuery(query) => Some(query),
            _ => None,
        })
        .collect();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].sort.column, service_id(ServiceColumn::Status));
    assert!(queries[0].sort.ascending);
}

#[gpui_kit::test]
fn group_expand_state_persisted(cx: &mut TestAppContext) {
    let (table, engine) = delegate_table_with_engine(cx, UiState::default());
    cx.update(|cx| {
        table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            install_rows(delegate);
            let system = delegate.row_id(0);
            delegate.on_toggle_expand(system, cx);
            let request = delegate.take_expanded_request().expect("expand request");
            assert!(!request.contains(&SysNodeId::ServiceGroup(Manager::System)));
            let mut query = SysQuery::new(SysTable::Services, delegate.sort(), None);
            query.set_expanded(request, engine.as_ref());
        });
        assert!(!TlSettings::state(cx).services.group_expanded.system);
    });
    let queries: Vec<_> = engine
        .commands()
        .into_iter()
        .filter_map(|command| match command {
            Command::SetSysTableQuery(query) => Some(query),
            _ => None,
        })
        .collect();
    assert_eq!(queries.len(), 1);
    assert!(
        !queries[0]
            .expanded
            .contains(&SysNodeId::ServiceGroup(Manager::System))
    );
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
