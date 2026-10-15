//! Services delegate, action and key binding tests.

#![cfg(feature = "test-hooks")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::base::test_support::snapshots;
use gpui_kit::{
    Action, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    KeyContext, ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window,
    div, point, px,
};
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::health::ManagerState;
use tl_model::ids::ProcessKey;
use tl_model::settings::{AppearanceSettings, ServicesSettings, ThemeSettings, UiState};
use tl_model::snapshot::{ServicesSnapshot, Snapshot};
use tl_model::table::{
    Manager, ServiceColumn, ServiceStateFilter, SysColumnId, SysNodeId, SysRow, SysTable, UnitRef,
    UnitType,
};
use tl_model::time::BootId;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::menu::{self, TlMenu};
use tl_ui::components::toast::{self, ToastQueue};
use tl_ui::demand::{DEMAND_COALESCE, DemandAggregator, GlobalDemand};
use tl_ui::hub::SnapshotHub;
use tl_ui::pages::services::actions::*;
use tl_ui::pages::services::delegate::{ServicesDelegate, services_columns};
use tl_ui::pages::services::filter_strip::{
    ServiceFilterState, apply_remove_filters, apply_reset_unit_types, apply_toggle_manager,
    apply_toggle_state, apply_toggle_unit_type, render_strip, unit_types_menu,
};
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
        toast::init(cx);
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

#[test]
fn strip_state_or_text_and() {
    let engine = FakeEngine::new();
    let mut strip = ServiceFilterState::default();
    strip.toggle_state(StateFilterArg::Running);
    strip.toggle_state(StateFilterArg::Failed);
    let sort = tl_model::table::SysSortSpec {
        column: service_id(ServiceColumn::Name),
        ascending: true,
    };
    let mut query = SysQuery::new(SysTable::Services, sort, Some(strip.to_filter()));
    query.set_filter("ssh", engine.as_ref());
    query.set_services_filter(strip.to_filter(), engine.as_ref());

    let sent = engine.commands();
    let Command::SetSysTableQuery(query) = sent.last().expect("services query") else {
        panic!("expected a system table query");
    };
    assert_eq!(query.filter.as_ref(), "ssh");
    assert_eq!(
        query
            .services
            .as_ref()
            .expect("services filter")
            .states
            .as_slice(),
        [ServiceStateFilter::Running, ServiceStateFilter::Failed]
    );
}

#[test]
fn is_default_ignores_type_order() {
    let mut strip = ServiceFilterState::default();
    strip.toggle_unit_type(UnitTypeArg::Service);
    assert!(!strip.is_default());
    strip.toggle_unit_type(UnitTypeArg::Service);
    assert_eq!(strip.unit_types.last(), Some(&UnitTypeArg::Service));
    assert!(strip.is_default(), "the default four in any order");
}

fn boot_settings(cx: &mut TestAppContext, saved: &Rc<RefCell<Vec<ServicesSettings>>>) {
    let saved = Rc::clone(saved);
    cx.update(|cx| {
        gpui_kit::init(cx);
        toast::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::i18n::init_strings(cx, "en-US");
        menu::bind_keys(cx);
        cx.set_reduce_motion(true);
        cx.set_global(BootSettings {
            settings: Default::default(),
            state: Default::default(),
            persist: Some(Box::new(move |settings, _| {
                saved.borrow_mut().push(settings.services.clone());
            })),
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
}

fn services_query(strip: &ServiceFilterState) -> SysQuery {
    let sort = tl_model::table::SysSortSpec {
        column: service_id(ServiceColumn::Name),
        ascending: true,
    };
    SysQuery::new(SysTable::Services, sort, Some(strip.to_filter()))
}

fn sent_queries(engine: &FakeEngine) -> Vec<tl_model::table::SysTableQuery> {
    engine
        .commands()
        .into_iter()
        .filter_map(|command| match command {
            Command::SetSysTableQuery(query) => Some(query),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn remove_filters_restores_defaults(cx: &mut TestAppContext) {
    let saved = Rc::default();
    boot_settings(cx, &saved);
    let engine = FakeEngine::new();
    let mut strip = ServiceFilterState::default();
    let mut query = services_query(&strip);
    query.set_filter("ssh", engine.as_ref());
    cx.update(|cx| {
        apply_toggle_state(
            &mut strip,
            StateFilterArg::Failed,
            &mut query,
            engine.as_ref(),
            cx,
        );
        apply_toggle_manager(&mut strip, ManagerArg::System, &mut query, engine.as_ref());
        apply_toggle_manager(&mut strip, ManagerArg::User, &mut query, engine.as_ref());
        assert!(!strip.is_default());

        apply_remove_filters(&mut strip, &mut query, engine.as_ref(), cx);
    });

    assert!(strip.is_default());
    let last = sent_queries(&engine).pop().expect("a query was sent");
    assert_eq!(last.filter.as_ref(), "ssh", "the text filter survives");
    assert_eq!(
        last.services,
        Some(ServiceFilterState::default().to_filter())
    );
    assert_eq!(query.query().filter.as_ref(), "ssh");
}

#[gpui_kit::test]
fn strip_persists_states_and_types(cx: &mut TestAppContext) {
    let saved = Rc::default();
    boot_settings(cx, &saved);
    let engine = FakeEngine::new();
    let mut strip = ServiceFilterState::default();
    let mut query = services_query(&strip);

    cx.update(|cx| {
        apply_toggle_state(
            &mut strip,
            StateFilterArg::Failed,
            &mut query,
            engine.as_ref(),
            cx,
        );
    });
    {
        let writes = saved.borrow();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].filter_states, [ServiceStateFilter::Failed]);
    }

    apply_toggle_manager(&mut strip, ManagerArg::User, &mut query, engine.as_ref());
    assert_eq!(saved.borrow().len(), 1, "manager state is session-only");
    assert!(!strip.user);
    assert_eq!(sent_queries(&engine).len(), 2, "each toggle sends a query");

    cx.update(|cx| {
        apply_reset_unit_types(&mut strip, &mut query, engine.as_ref(), cx);
    });
    let writes = saved.borrow();
    let last = writes.last().expect("reset writes types");
    assert_eq!(
        last.unit_types,
        [
            UnitType::Service,
            UnitType::Socket,
            UnitType::Timer,
            UnitType::Mount
        ]
    );
}

/// A window with the page's action handlers for the unit type menu and a toast queue.
struct MenuHost {
    focus: FocusHandle,
    strip: Rc<RefCell<ServiceFilterState>>,
    query: Rc<RefCell<SysQuery>>,
    engine: Arc<FakeEngine>,
}

impl Render for MenuHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strip = Rc::clone(&self.strip);
        let query = Rc::clone(&self.query);
        let engine = Arc::clone(&self.engine);
        div()
            .size(px(800.))
            .key_context("ServicesHost")
            .track_focus(&self.focus)
            .on_action(move |action: &ServicesToggleUnitType, window, cx| {
                apply_toggle_unit_type(
                    &mut strip.borrow_mut(),
                    action.kind.clone(),
                    &mut query.borrow_mut(),
                    engine.as_ref(),
                    window,
                    cx,
                );
            })
            .children(menu::context_menu_layer(window, cx))
    }
}

struct MenuRig {
    cx: VisualTestContext,
    host: Entity<MenuHost>,
    menu: Entity<menu::TlMenuPopup>,
    engine: Arc<FakeEngine>,
    queue: Entity<ToastQueue>,
    saved: Rc<RefCell<Vec<ServicesSettings>>>,
}

fn open_menu_rig(cx: &mut TestAppContext, settings: ServicesSettings) -> MenuRig {
    let saved = Rc::default();
    boot_settings(cx, &saved);
    let engine = FakeEngine::new();
    let strip = ServiceFilterState::from_settings(&settings);
    let query = Rc::new(RefCell::new(services_query(&strip)));
    let strip = Rc::new(RefCell::new(strip));
    let slot = Rc::new(RefCell::new(None));
    let keep = Rc::clone(&slot);
    let (host_strip, host_query, host_engine) =
        (Rc::clone(&strip), Rc::clone(&query), Arc::clone(&engine));
    let handle = cx.add_window(move |window, cx| {
        let host = cx.new(|cx| MenuHost {
            focus: cx.focus_handle(),
            strip: host_strip,
            query: host_query,
            engine: host_engine,
        });
        *keep.borrow_mut() = Some(host.clone());
        Root::new(host, window, cx)
    });
    let host = slot.borrow().clone().expect("host");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    let queue = vcx.update(|window, cx| {
        let top = tl_ui::theme::metrics(cx).sp[4];
        cx.new(|cx| ToastQueue::new(window, cx, top))
    });
    let menu = vcx.update(|window, cx| {
        let focus = host.read(cx).focus.clone();
        window.focus(&focus, cx);
        let state = strip.borrow().clone();
        let menu = TlMenu::build(window, cx, focus, move |menu, window, cx| {
            unit_types_menu(&state, menu, window, cx)
        });
        menu::open_context_menu_at(point(px(100.), px(100.)), window, cx, menu.clone());
        menu
    });
    vcx.run_until_parked();
    MenuRig {
        cx: vcx,
        host,
        menu,
        engine,
        queue,
        saved,
    }
}

impl MenuRig {
    fn entries(&mut self) -> Vec<menu::MenuEntryInfo> {
        let menu = self.menu.clone();
        self.cx
            .update(|window, cx| menu.read(cx).entries_for_test(window))
    }

    fn menu_open(&mut self) -> bool {
        self.cx
            .update(|window, cx| menu::context_menu_open(window, cx))
    }

    fn strip(&mut self) -> ServiceFilterState {
        self.host
            .read_with(&self.cx, |host, _| host.strip.borrow().clone())
    }
}

#[gpui_kit::test]
fn unit_types_last_item_refused(cx: &mut TestAppContext) {
    let only_services = ServicesSettings {
        unit_types: vec![UnitType::Service],
        ..ServicesSettings::default()
    };
    let mut rig = open_menu_rig(cx, only_services);
    let entries = rig.entries();
    assert!(entries[0].checked);
    assert!(entries[0].keep_open);

    rig.cx.simulate_keystrokes("down enter");
    rig.cx.run_until_parked();

    assert_eq!(
        rig.strip().unit_types.as_slice(),
        [UnitTypeArg::Service],
        "state is unchanged"
    );
    assert!(sent_queries(&rig.engine).is_empty(), "no query is sent");
    assert!(rig.saved.borrow().is_empty(), "nothing is saved");
    let toasts = rig
        .queue
        .read_with(&rig.cx, |queue, _| queue.visible_texts());
    assert_eq!(toasts.len(), 1);
    assert_eq!(
        toasts[0].as_ref(),
        "At least one unit type must stay selected"
    );
    assert!(rig.menu_open(), "a refusal keeps the menu open");
    assert!(rig.entries()[0].checked, "the refused item stays checked");
}

#[gpui_kit::test]
fn unit_types_menu_stays_open_after_state_changes(cx: &mut TestAppContext) {
    let only_timers = ServicesSettings {
        unit_types: vec![UnitType::Timer],
        ..ServicesSettings::default()
    };
    let mut rig = open_menu_rig(cx, only_timers);
    // Enable Services, then disable Timers: both are valid, so the menu stays open.
    rig.cx.simulate_keystrokes("down enter");
    rig.cx.run_until_parked();
    rig.cx.simulate_keystrokes("down down enter");
    rig.cx.run_until_parked();
    assert!(rig.menu_open(), "a valid toggle keeps the menu open");
    assert_eq!(rig.strip().unit_types.as_slice(), [UnitTypeArg::Service]);
}

#[gpui_kit::test]
fn unit_types_menu_stays_open_on_toggle(cx: &mut TestAppContext) {
    let mut rig = open_menu_rig(cx, ServicesSettings::default());
    let entries = rig.entries();
    assert_eq!(entries.len(), 12);
    assert!(entries[..10].iter().all(|entry| entry.keep_open));
    assert_eq!(entries[2].label.as_deref(), Some("Timers"));
    assert!(entries[2].checked);

    rig.cx.simulate_keystrokes("down down down enter");
    rig.cx.run_until_parked();

    assert!(rig.menu_open(), "the menu stays open");
    let entries = rig.entries();
    assert!(!entries[2].checked, "Timers toggled off");
    assert!(entries[0].checked && entries[1].checked);
    assert!(!rig.strip().unit_types.contains(&UnitTypeArg::Timer));
    let queries = sent_queries(&rig.engine);
    assert_eq!(queries.len(), 1);
    assert!(
        !queries[0]
            .services
            .as_ref()
            .expect("services filter")
            .unit_types
            .contains(&UnitType::Timer)
    );
    let saved = rig.saved.borrow();
    assert_eq!(
        saved.last().expect("saved types").unit_types,
        [UnitType::Service, UnitType::Socket, UnitType::Mount]
    );
}

struct FilterStripHost {
    state: ServiceFilterState,
}

impl Render for FilterStripHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        render_strip(&self.state, false, false, false, window, cx)
    }
}

#[gpui_kit::test]
fn unit_types_count_suffix(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::i18n::init_strings(cx, "en-US");
    });
    let handle = cx.add_window(|window, cx| {
        let mut state = ServiceFilterState::default();
        state.toggle_unit_type(UnitTypeArg::Automount);
        state.toggle_unit_type(UnitTypeArg::Path);
        Root::new(cx.new(|_| FilterStripHost { state }), window, cx)
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let buttons = vcx.update(|window, _| snapshots(window));
    let unit_types = buttons
        .iter()
        .find(|snapshot| {
            snapshot.path().last().is_some_and(|id| {
                id == &gpui_kit::ElementId::Name("services-filter-unit-types".into())
            })
        })
        .expect("Unit types button");
    assert_eq!(unit_types.label(), Some("Unit types (6)"));
}

/// Services copy block and copy source (tasklens-hk6.14).
mod copy {
    use std::sync::Arc;

    use super::*;
    use gpui_kit::{ClipboardItem, TestAppContext, VisualTestContext};
    use tl_model::actions::SchedPolicy;
    use tl_model::ids::ProcessKey;
    use tl_model::process::{
        GroupKind, ProcFlags, ProcState, ProcessExtras, ProcessMeta, ProcessRecord,
    };
    use tl_model::snapshot::{
        ProcessSnapshot, ServiceStatus, ServicesSnapshot, Snapshot, StartupType, UnitRecord,
    };
    use tl_model::system::CpuSet;
    use tl_model::table::{Manager, ServiceColumn, SysNodeId, SysRow, Total, UnitRef, UnitType};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::pages::services::copy::{
        alias_column, block_service, column_alias, services_copy_texts,
    };
    use tl_ui::pages::services::delegate::{ServicesDelegate, row_key};
    use tl_ui::pages::sys_copy::{SysCopySource as _, copy_row};

    const GOLDEN: &str = "Name: sshd.service\n\
Display name: OpenSSH Daemon\n\
Status: Running\n\
Start type: Enabled\n\
PID: 812\n\
User: root\n\
Binary: /usr/bin/sshd";

    fn a(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    fn missing<T: Default>() -> Total<T> {
        Total {
            value: T::default(),
            validity: Validity::Missing,
        }
    }

    fn unit_ref(name: &str) -> UnitRef {
        UnitRef {
            manager: Manager::System,
            name: a(name),
        }
    }

    fn unit_record(name: &str, description: &str, pid: u32, path: &str) -> UnitRecord {
        UnitRecord {
            unit: unit_ref(name),
            kind: UnitType::Service,
            description: a(description),
            status: ServiceStatus::Running,
            sub_state: a("running"),
            load_state: a("loaded"),
            startup: StartupType::Enabled,
            main_pid: pid,
            user: a("root"),
            path: a(path),
            unit_file: None,
            active_since_us: None,
            triggered_by: Arc::from([]),
            control_group: None,
            can_reload: false,
            has_cgroup_procs: false,
            cgroup_pids: Arc::from([]),
            cpu_pct: missing(),
            memory_bytes: missing(),
            disk_bps: missing(),
            tasks: missing(),
        }
    }

    fn process_key() -> ProcessKey {
        ProcessKey {
            start_ticks: 10,
            boot_id: BootId(1),
            pid: 812,
        }
    }

    fn process_record() -> ProcessRecord {
        ProcessRecord {
            key: process_key(),
            ppid: 1,
            uid: 0,
            meta: Arc::new(ProcessMeta {
                comm: a("sshd"),
                name: a("sshd"),
                exe: Some(a("/usr/bin/sshd")),
                cmdline: Some(a("/usr/bin/sshd -D")),
                user: a("root"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a("sshd"),
            }),
            group: GroupKind::App { app_id: a("sshd") }.key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn process_extras() -> ProcessExtras {
        ProcessExtras {
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            start_boot_ns: 0,
            rss_peak_bytes: 0,
            rss_shared_bytes: 0,
            commit_bytes: 0,
            vsize_bytes: 0,
            vsize_peak_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            policy: SchedPolicy::Other,
            rt_prio: 0,
            affinity: CpuSet::empty(),
            handles: None,
            exe_name: a("sshd"),
            exe_path: a("/usr/bin/sshd"),
            exe_deleted: false,
            unit: Some(unit_ref("sshd.service")),
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
        }
    }

    fn sys_row(node: SysNodeId, depth: u8, record: Option<u32>) -> SysRow {
        SysRow {
            node,
            depth,
            expanded: true,
            has_children: record.is_none(),
            record,
        }
    }

    fn install(delegate: &mut ServicesDelegate) {
        let sshd = unit_ref("sshd.service");
        let cron = unit_ref("cron.service");
        let rows: Arc<[SysRow]> = Arc::from([
            sys_row(SysNodeId::ServiceGroup(Manager::System), 0, None),
            sys_row(SysNodeId::Unit(sshd.clone()), 1, Some(0)),
            sys_row(SysNodeId::UnitProcess(sshd, process_key()), 2, Some(0)),
            sys_row(SysNodeId::Unit(cron), 1, Some(1)),
        ]);
        let mut snapshot = Snapshot::empty(BootId(1), 1);
        snapshot.services = Arc::new(ServicesSnapshot {
            rows,
            units: Arc::from([
                unit_record("sshd.service", "OpenSSH Daemon", 812, "/usr/bin/sshd"),
                unit_record("cron.service", "Regular Jobs", 0, ""),
            ]),
            managers: [ManagerState::Connected, ManagerState::Unavailable],
            ..ServicesSnapshot::default()
        });
        snapshot.processes = Arc::new(ProcessSnapshot {
            records: Arc::from([process_record()]),
            extras: Arc::from([process_extras()]),
            ..ProcessSnapshot::default()
        });
        delegate.install(&Arc::new(snapshot));
    }

    struct Rig {
        table: Entity<TlTreeTable<ServicesDelegate>>,
        vcx: VisualTestContext,
        queue: Entity<ToastQueue>,
    }

    impl Rig {
        fn select(&mut self, nodes: &[SysNodeId]) {
            let ids: Vec<u64> = nodes.iter().map(row_key).collect();
            self.table.update(&mut self.vcx, |table, _| {
                table.selection.clear();
                for id in ids {
                    table.selection.ctrl_click(id);
                }
            });
        }

        fn copy(&mut self) {
            let table = self.table.clone();
            self.vcx.update(|window, cx| {
                table.update(cx, |table, cx| copy_row(&*table, window, cx));
            });
            self.vcx.run_until_parked();
        }

        fn clipboard(&mut self) -> Option<String> {
            self.vcx
                .update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
        }

        fn toasts(&mut self) -> Vec<String> {
            self.queue.read_with(&self.vcx, |q, _| {
                q.visible_texts().iter().map(|t| t.to_string()).collect()
            })
        }
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let (table, _engine) = delegate_table_with_engine(cx, UiState::default());
        cx.update(|cx| {
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let window = cx.windows().into_iter().next().expect("window");
        let mut vcx = VisualTestContext::from_window(window, cx);
        vcx.update(|window, _| window.activate_window());
        let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(64.))));
        table.update(&mut vcx, |table, _| install(&mut table.delegate));
        vcx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("before".into())));
        vcx.run_until_parked();
        Rig { table, vcx, queue }
    }

    fn sshd_node() -> SysNodeId {
        SysNodeId::Unit(unit_ref("sshd.service"))
    }

    fn cron_node() -> SysNodeId {
        SysNodeId::Unit(unit_ref("cron.service"))
    }

    #[test]
    fn service_copy_block_golden() {
        let mut out = String::new();
        block_service(
            &unit_record("sshd.service", "OpenSSH Daemon", 812, "/usr/bin/sshd"),
            &mut out,
        );
        assert_eq!(out, GOLDEN);
    }

    #[test]
    fn service_copy_empty_values() {
        let mut out = String::new();
        block_service(
            &unit_record("cron.service", "Regular Jobs", 0, ""),
            &mut out,
        );
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[4], "PID: ");
        assert_eq!(lines[6], "Binary: ");
        assert!(!out.ends_with('\n'));
    }

    #[test]
    fn services_copy_texts_are_the_spec_texts() {
        let texts = services_copy_texts();
        assert_eq!(texts.row_ok, "Service details copied.");
        assert_eq!(texts.row_failed, "Could not copy service details.");
        assert_eq!(texts.nothing_selected, "Select a service row first.");
    }

    #[test]
    fn services_copy_column_alias_round_trip() {
        let all = [
            ServiceColumn::Name,
            ServiceColumn::DisplayName,
            ServiceColumn::Status,
            ServiceColumn::StartupType,
            ServiceColumn::Pid,
            ServiceColumn::User,
            ServiceColumn::Path,
            ServiceColumn::Manager,
            ServiceColumn::UnitType,
            ServiceColumn::SubState,
            ServiceColumn::LoadState,
            ServiceColumn::Cpu,
            ServiceColumn::Memory,
            ServiceColumn::Disk,
            ServiceColumn::Tasks,
            ServiceColumn::ActiveSince,
            ServiceColumn::UnitFile,
            ServiceColumn::TriggeredBy,
            ServiceColumn::Description,
        ];
        for column in all {
            assert_eq!(alias_column(column_alias(column)), column);
        }
    }

    #[gpui_kit::test]
    fn copy_row_services_block_format(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.select(&[
            SysNodeId::ServiceGroup(Manager::System),
            cron_node(),
            sshd_node(),
        ]);
        r.copy();
        let cron = "Name: cron.service\nDisplay name: Regular Jobs\nStatus: Running\n\
Start type: Enabled\nPID: \nUser: root\nBinary: ";
        assert_eq!(r.clipboard(), Some(format!("{GOLDEN}\n\n{cron}")));
        assert_eq!(r.toasts(), ["Service details copied."]);

        r.select(&[]);
        r.vcx
            .update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("before".into())));
        r.copy();
        assert_eq!(r.clipboard().as_deref(), Some("before"));
        assert!(
            r.toasts()
                .iter()
                .any(|t| t == "Select a service row first.")
        );
    }

    #[gpui_kit::test]
    fn copy_child_row_uses_process_block(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.select(&[SysNodeId::UnitProcess(
            unit_ref("sshd.service"),
            process_key(),
        )]);
        r.copy();
        let text = r.clipboard().expect("clipboard");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Name: sshd");
        assert_eq!(lines[1], "PID: 812");
        assert!(!text.contains("Display name"));
        assert_eq!(r.toasts(), ["Service details copied."]);
    }

    #[gpui_kit::test]
    fn copy_rows_follow_view_order(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.select(&[cron_node(), sshd_node()]);
        let ids = r
            .table
            .read_with(&r.vcx, |table, _| table.copy_rows().to_vec());
        assert_eq!(ids, [row_key(&sshd_node()), row_key(&cron_node())]);
    }
}

/// Services unit row menu (tasklens-hk6.17).
mod menu_tests {
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use smallvec::SmallVec;
    use tl_model::actions::{ActionKind, ActionTarget};
    use tl_model::client::EngineClient;
    use tl_model::ids::ProcessKey;
    use tl_model::snapshot::{
        ServiceBackend, ServiceStatus, ServicesSnapshot, StartupType, UnitRecord,
    };
    use tl_model::table::{Manager, SysColumnId, SysNodeId, Total, UnitRef, UnitType};
    use tl_model::time::BootId;
    use tl_model::validity::Validity;
    use tl_ui::action_tracker::ActionTracker;
    use tl_ui::components::menu::{MenuEntryInfo, MenuEntryKind, TlMenuPopup};
    use tl_ui::pages::services::menu::{MenuChoice, ServiceMenuTargets, menu_choice, service_menu};
    use tl_ui::pages::sys_table::{ActionKindTag, ActionTexts, SysActions};

    use super::*;

    const BUSY: &str = "A service action is already in progress.";

    fn a(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    fn missing<T: Default>() -> Total<T> {
        Total {
            value: T::default(),
            validity: Validity::Missing,
        }
    }

    fn unit_ref(name: &str) -> UnitRef {
        UnitRef {
            manager: Manager::System,
            name: a(name),
        }
    }

    fn unit_record(name: &str, status: ServiceStatus, startup: StartupType) -> UnitRecord {
        UnitRecord {
            unit: unit_ref(name),
            kind: UnitType::Service,
            description: a("Unit"),
            status,
            sub_state: a("running"),
            load_state: a("loaded"),
            startup,
            main_pid: 812,
            user: a("root"),
            path: a("/usr/bin/daemon"),
            unit_file: Some(a("/usr/lib/systemd/system/daemon.service")),
            active_since_us: None,
            triggered_by: Arc::from([]),
            control_group: None,
            can_reload: false,
            has_cgroup_procs: false,
            cgroup_pids: Arc::from([]),
            cpu_pct: missing(),
            memory_bytes: missing(),
            disk_bps: missing(),
            tasks: missing(),
        }
    }

    fn snapshot(units: Vec<UnitRecord>) -> ServicesSnapshot {
        ServicesSnapshot {
            units: units.into(),
            ..ServicesSnapshot::default()
        }
    }

    fn node(name: &str) -> SysNodeId {
        SysNodeId::Unit(unit_ref(name))
    }

    fn child(name: &str) -> SysNodeId {
        SysNodeId::UnitProcess(
            unit_ref(name),
            ProcessKey {
                start_ticks: 10,
                boot_id: BootId(1),
                pid: 812,
            },
        )
    }

    struct Rig {
        vcx: VisualTestContext,
        actions: Entity<SysActions>,
        /// Kept alive: `SysActions::submit` needs the window's toast queue.
        _queue: Entity<ToastQueue>,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        boot_settings(cx, &Default::default());
        let window = cx.add_window(|window, cx| {
            let host = cx.new(|_| Host);
            Root::new(host, window, cx)
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(64.))));
        let client: Arc<dyn EngineClient> = FakeEngine::new();
        let tracker = vcx.update(|_, cx| cx.new(|cx| ActionTracker::new(client, cx)));
        let actions = vcx.update(|_, cx| cx.new(|cx| SysActions::new(&tracker, cx)));
        Rig {
            vcx,
            actions,
            _queue: queue,
        }
    }

    impl Rig {
        fn capture(
            &mut self,
            snap: &ServicesSnapshot,
            selection: &[SysNodeId],
            col: Option<SysColumnId>,
        ) -> ServiceMenuTargets {
            let actions = self.actions.clone();
            self.vcx
                .update(|_, cx| ServiceMenuTargets::capture(snap, selection, col, actions.read(cx)))
        }

        fn entries(&mut self, t: &ServiceMenuTargets) -> Vec<MenuEntryInfo> {
            let t = t.clone();
            let menu = self.vcx.update(|window, cx| {
                let focus = cx.focus_handle();
                TlMenu::build(window, cx, focus, move |menu, window, cx| {
                    service_menu(menu, &t, ServiceBackend::Systemd, window, cx)
                })
            });
            self.popup_entries(&menu)
        }

        fn popup_entries(&mut self, menu: &Entity<TlMenuPopup>) -> Vec<MenuEntryInfo> {
            self.vcx
                .update(|window, cx| menu.read(cx).entries_for_test(window))
        }

        fn stop_pending(&mut self, name: &str) {
            let actions = self.actions.clone();
            let target: SmallVec<[ActionTarget; 1]> =
                [ActionTarget::Unit(unit_ref(name))].into_iter().collect();
            let texts = ActionTexts {
                pending_one: "Stopping service...".into(),
                pending_many: "sys-sys007-starting-n-services",
                done_one: "Service stopped".into(),
                done_many: "sys-sys007-n-services-started",
                partial_many: "sys-sys007-ok-of",
                failed_prefix: "Failed to stop service".into(),
                failed_suffix: true,
                unknown: "Service action finished.".into(),
                busy: BUSY.into(),
                failed_button: None,
            };
            self.vcx.update(|window, cx| {
                actions.update(cx, |a, cx| {
                    a.submit(
                        ActionKind::UnitStop,
                        ActionKindTag::Stop,
                        target,
                        texts,
                        window,
                        cx,
                    )
                })
            });
            self.vcx.run_until_parked();
        }
    }

    fn labels(entries: &[MenuEntryInfo]) -> Vec<String> {
        entries
            .iter()
            .filter(|e| e.kind != MenuEntryKind::Separator)
            .map(|e| e.label.as_ref().map(|l| l.to_string()).unwrap_or_default())
            .collect()
    }

    fn find<'a>(entries: &'a [MenuEntryInfo], label: &str) -> Option<&'a MenuEntryInfo> {
        entries
            .iter()
            .find(|e| e.label.as_ref().is_some_and(|l| l.as_ref() == label))
    }

    #[gpui_kit::test]
    fn services_menu_order_and_labels(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let mut rec = unit_record("sshd.service", ServiceStatus::Running, StartupType::Enabled);
        rec.can_reload = true;
        let snap = snapshot(vec![rec]);
        let col = Some(service_id(ServiceColumn::Name));
        let t = r.capture(&snap, &[node("sshd.service")], col);
        let entries = r.entries(&t);
        assert_eq!(
            labels(&entries),
            [
                "Start",
                "Stop",
                "Restart",
                "Reload",
                "Startup type",
                "View logs",
                "Properties",
                "File properties",
                "Open file location",
                "Open unit file",
                "Copy",
                "Copy row",
                "Copy rows as table",
                "Jump to process",
            ]
        );
        assert_eq!(
            find(&entries, "Start").unwrap().kind,
            MenuEntryKind::Disabled
        );
        assert_eq!(find(&entries, "Stop").unwrap().kind, MenuEntryKind::Item);
        assert_eq!(
            find(&entries, "Startup type").unwrap().kind,
            MenuEntryKind::Submenu
        );
    }

    #[gpui_kit::test]
    fn menu_reload_hidden_without_canreload(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let rec = unit_record("cron.service", ServiceStatus::Stopped, StartupType::Enabled);
        let snap = snapshot(vec![rec]);
        let t = r.capture(&snap, &[node("cron.service")], None);
        let entries = r.entries(&t);
        assert!(find(&entries, "Reload").is_none());
        assert!(find(&entries, "Reset failed state").is_none());
        assert!(
            find(&entries, "Copy").is_none(),
            "no cell without a pointer"
        );
    }

    #[gpui_kit::test]
    fn menu_pending_disables_with_tooltip(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let rec = unit_record("sshd.service", ServiceStatus::Running, StartupType::Enabled);
        let snap = snapshot(vec![rec]);
        r.stop_pending("sshd.service");
        let t = r.capture(&snap, &[node("sshd.service")], None);
        let entries = r.entries(&t);
        for label in ["Start", "Stop", "Restart"] {
            let e = find(&entries, label).unwrap();
            assert_eq!(e.kind, MenuEntryKind::Disabled, "{label}");
            assert_eq!(e.tooltip.as_deref(), Some(BUSY), "{label}");
        }
    }

    #[gpui_kit::test]
    fn startup_submenu_disabled_for_static(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let rec = unit_record("dbus.service", ServiceStatus::Running, StartupType::Static);
        let snap = snapshot(vec![rec]);
        let t = r.capture(&snap, &[node("dbus.service")], None);
        let entries = r.entries(&t);
        let e = find(&entries, "Startup type").unwrap();
        assert_eq!(e.kind, MenuEntryKind::Disabled);
        assert_eq!(
            e.tooltip.as_deref(),
            Some("This unit has no install section")
        );
    }

    #[gpui_kit::test]
    fn startup_submenu_no_check_on_mixed(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let snap = snapshot(vec![
            unit_record("a.service", ServiceStatus::Running, StartupType::Enabled),
            unit_record("b.service", ServiceStatus::Stopped, StartupType::Disabled),
        ]);
        let t = r.capture(&snap, &[node("a.service"), node("b.service")], None);
        let entries = r.entries(&t);
        let sub = find(&entries, "Startup type")
            .unwrap()
            .submenu
            .clone()
            .expect("submenu");
        let items = r.popup_entries(&sub);
        assert_eq!(labels(&items), ["Enabled", "Disabled", "Masked"]);
        assert!(items.iter().all(|e| !e.checked));

        // One target: its value is checked, and "Enabled (runtime)" counts as Enabled.
        let snap = snapshot(vec![unit_record(
            "a.service",
            ServiceStatus::Running,
            StartupType::EnabledRuntime,
        )]);
        let t = r.capture(&snap, &[node("a.service")], None);
        let entries = r.entries(&t);
        let sub = find(&entries, "Startup type")
            .unwrap()
            .submenu
            .clone()
            .expect("submenu");
        let items = r.popup_entries(&sub);
        let checked: Vec<_> = items.iter().filter(|e| e.checked).collect();
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].label.as_deref(), Some("Enabled"));
    }

    #[gpui_kit::test]
    fn menu_target_captured_at_open(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let old = snapshot(vec![
            unit_record("a.service", ServiceStatus::Stopped, StartupType::Enabled),
            unit_record("b.service", ServiceStatus::Running, StartupType::Enabled),
        ]);
        let t = r.capture(&old, &[node("a.service")], None);
        // The rows re-sort after the menu opened.
        let _resorted = snapshot(vec![
            unit_record("b.service", ServiceStatus::Running, StartupType::Enabled),
            unit_record("a.service", ServiceStatus::Stopped, StartupType::Enabled),
        ]);
        assert_eq!(t.units.as_slice(), [unit_ref("a.service")]);
        assert_eq!(t.records[0].unit, unit_ref("a.service"));
        let entries = r.entries(&t);
        let start = find(&entries, "Start").unwrap();
        assert_eq!(start.kind, MenuEntryKind::Item);
        assert!(start.action.as_ref().unwrap().partial_eq(&ServiceStart));
    }

    #[gpui_kit::test]
    fn menu_choice_mixed_and_child_only(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let snap = snapshot(vec![unit_record(
            "a.service",
            ServiceStatus::Running,
            StartupType::Enabled,
        )]);
        let mixed = [node("a.service"), child("a.service")];
        assert_eq!(menu_choice(&mixed), MenuChoice::Unit);
        let t = r.capture(&snap, &mixed, None);
        assert_eq!(t.units.as_slice(), [unit_ref("a.service")]);
        assert_eq!(menu_choice(&[child("a.service")]), MenuChoice::Process);
        assert_eq!(
            menu_choice(&[SysNodeId::ServiceGroup(Manager::System)]),
            MenuChoice::None
        );
        assert_eq!(menu_choice(&[]), MenuChoice::None);
    }
}

#[gpui_kit::test]
fn init_binds_services_keys(cx: &mut TestAppContext) {
    use tl_model::settings::Settings;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};

    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
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

    let handle = cx.add_window(|window, cx| Root::new(cx.new(|_| Host), window, cx));
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    assert_window_binding(&mut vcx, &ServiceStart, "Table ServiceTable", "ctrl-s");
    assert_window_binding(
        &mut vcx,
        &ServicePropsSelectTab { index: 2 },
        "ServiceProps",
        "ctrl-3",
    );
}

/// Services Start, Stop, Restart, Reload and Reset failed flows (tasklens-hk6.15).
mod flows {
    use std::sync::Arc;

    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use tl_model::actions::{
        ActionError, ActionEvent, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget,
    };
    use tl_model::command::Tab;
    use tl_model::ids::NodeId;
    use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::settings::Settings;
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, StartupType, UnitRecord};
    use tl_model::table::{Manager, UnitRef};
    use tl_model::table::{ProcessTableView, TableRow, Total};
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::action_tracker::{GlobalActionTracker, PushedToast};
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::toast::{ToastKey, ToastKind};
    use tl_ui::pages::services::actions::StartupChoice;
    use tl_ui::pages::services::flows::{
        UnitVerb, file_uri, run_file_properties, run_jump_to_process, run_open_file_location,
        run_open_unit_file, run_set_startup, run_unit_verb, startup_change_kind, unit_action_texts,
    };
    use tl_ui::pages::sys_table::{ActionKindTag, SysActions};
    use tl_ui::window::main::{main_window_view, open_main_window_on};
    use tl_ui::window::registry::{GlobalRegistry, WindowKey};
    use tl_ui::windows::service_properties::{ServicePropertiesWindow, ServicePropsTab};

    use super::*;

    fn unit(name: &str) -> UnitRef {
        UnitRef {
            manager: Manager::System,
            name: Arc::from(name),
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        main: VisualTestContext,
        actions: Entity<SysActions>,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
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
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Services, cx))
            .expect("the main window opens");
        let mut main = VisualTestContext::from_window(handle.into(), cx);
        main.update(|window, _| window.activate_window());
        main.run_until_parked();
        let actions = main.update(|_, cx| {
            let tracker = cx.global::<GlobalActionTracker>().0.clone();
            cx.new(|cx| SysActions::new(&tracker, cx))
        });
        Rig {
            engine,
            main,
            actions,
        }
    }

    impl Rig {
        fn run(&mut self, v: UnitVerb, names: &[&str]) {
            let units: Vec<UnitRef> = names.iter().map(|n| unit(n)).collect();
            let actions = self.actions.clone();
            self.main
                .update(|window, cx| run_unit_verb(v, &units, &actions, window, cx));
            self.main.run_until_parked();
        }

        fn sent(&self) -> Vec<ActionRequest> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::Action(r) => Some(r),
                    _ => None,
                })
                .collect()
        }

        fn emit(&mut self, id: ActionId, target: Option<u16>, state: ActionState) {
            self.engine.emit(ActionEvent { id, target, state });
            self.main.run_until_parked();
        }

        /// `Started`, one state per target, then the summary of those states.
        fn finish(&mut self, id: ActionId, states: &[ActionState]) {
            self.emit(id, None, ActionState::Started);
            for (i, state) in states.iter().enumerate() {
                self.emit(id, Some(i as u16), state.clone());
            }
            self.emit(id, None, ActionState::summarize(states));
        }

        fn toasts(&mut self) -> Vec<String> {
            self.main.update(|_, cx| {
                let view = main_window_view(cx).expect("main window");
                let texts = view.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(|t| t.to_string()).collect()
            })
        }

        fn pending_tag(&mut self, name: &str) -> Option<ActionKindTag> {
            let target = ActionTarget::Unit(unit(name));
            self.actions
                .read_with(&self.main, |a, _| a.pending_tag(&target))
        }

        fn pushed(&mut self) -> Vec<PushedToast> {
            self.actions
                .read_with(&self.main, |a, _| a.pushed().to_vec())
        }
    }

    fn only_id(rig: &Rig) -> ActionId {
        let sent = rig.sent();
        assert_eq!(sent.len(), 1, "one request is sent");
        sent[0].id
    }

    #[gpui_kit::test]
    fn start_flow_toasts_and_overlay(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.run(UnitVerb::Start, &["a.service"]);
        assert_eq!(rig.toasts(), ["Starting service..."]);
        assert_eq!(rig.pending_tag("a.service"), Some(ActionKindTag::Start));
        let id = only_id(&rig);
        rig.finish(id, &[ActionState::Succeeded]);
        assert_eq!(rig.toasts(), ["Service started"]);
        assert_eq!(rig.pending_tag("a.service"), None);
        let pushed = rig.pushed();
        assert!(pushed.iter().all(|p| p.key == Some(ToastKey(id.0))));
        assert_eq!(pushed.last().expect("toast").kind, ToastKind::Success);
    }

    #[gpui_kit::test]
    fn failed_start_shows_view_logs(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.run(UnitVerb::Start, &["a.service"]);
        let id = only_id(&rig);
        let failed = ActionState::Failed(ActionError::UnitJob {
            result: "failed".into(),
            error_name: None,
        });
        rig.finish(id, &[failed]);
        assert_eq!(rig.toasts(), ["Failed to start service: failed"]);
        let last = rig.pushed().last().cloned().expect("toast");
        assert_eq!(last.kind, ToastKind::Error);
        assert_eq!(last.button.as_deref(), Some("View logs"));
        let action = rig.main.update(|_, cx| {
            let (_, make) = unit_action_texts(UnitVerb::Start, cx)
                .failed_button
                .expect("button");
            make(&ActionTarget::Unit(unit("a.service")))
        });
        rig.main
            .update(|window, cx| window.dispatch_action(action, cx));
        rig.main.run_until_parked();
        let handle = rig.main.update(|_, cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry
                .read(cx)
                .get(&WindowKey::Service(unit("a.service")))
        });
        let handle = handle
            .and_then(|h| h.downcast::<gpui_kit::base::Root>())
            .expect("Service Properties opened");
        let view = rig
            .main
            .update(|_, cx| ServicePropertiesWindow::find(handle, cx))
            .expect("view");
        assert_eq!(
            view.read_with(&rig.main, |v, _| v.active()),
            ServicePropsTab::Logs
        );
    }

    #[gpui_kit::test]
    fn busy_rejection_toast(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.run(UnitVerb::Start, &["a.service"]);
        rig.run(UnitVerb::Start, &["a.service"]);
        assert_eq!(
            rig.toasts().last().map(String::as_str),
            Some("A service action is already in progress.")
        );
        assert_eq!(rig.sent().len(), 1);
    }

    #[gpui_kit::test]
    fn reset_failed_no_overlay(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.run(UnitVerb::ResetFailed, &["a.service"]);
        // The delegate draws no overlay for this tag (hk6.11).
        assert_eq!(
            rig.pending_tag("a.service"),
            Some(ActionKindTag::ResetFailed)
        );
        let id = only_id(&rig);
        rig.finish(id, &[ActionState::Succeeded]);
        assert_eq!(rig.toasts(), ["Failed state cleared"]);
        rig.main.update(|_, cx| {
            assert!(
                unit_action_texts(UnitVerb::ResetFailed, cx)
                    .failed_button
                    .is_none()
            );
        });
    }

    #[gpui_kit::test]
    fn stop_five_units_summary_toast(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let names = [
            "a.service",
            "b.service",
            "c.service",
            "d.service",
            "e.service",
        ];
        rig.run(UnitVerb::Stop, &names);
        assert_eq!(rig.toasts(), ["Stopping 5 services..."]);
        let id = only_id(&rig);
        rig.finish(id, &vec![ActionState::Succeeded; 5]);
        assert_eq!(rig.toasts(), ["5 services stopped"]);
    }

    #[gpui_kit::test]
    fn outcome_unknown_warning(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.run(UnitVerb::Restart, &["a.service"]);
        let id = only_id(&rig);
        rig.finish(id, &[ActionState::OutcomeUnknown("timeout".into())]);
        assert_eq!(rig.toasts(), ["Service action finished."]);
        assert_eq!(rig.pushed().last().expect("toast").kind, ToastKind::Warning);
    }
    fn unit_rec(name: &str, path: &str, unit_file: Option<&str>, pid: u32) -> UnitRecord {
        fn missing<T: Default>() -> Total<T> {
            Total {
                value: T::default(),
                validity: Validity::Missing,
            }
        }
        UnitRecord {
            unit: unit(name),
            kind: UnitType::Service,
            description: Arc::from("Unit"),
            status: tl_model::snapshot::ServiceStatus::Running,
            sub_state: Arc::from("running"),
            load_state: Arc::from("loaded"),
            startup: StartupType::Enabled,
            main_pid: pid,
            user: Arc::from("root"),
            path: Arc::from(path),
            unit_file: unit_file.map(Arc::from),
            active_since_us: None,
            triggered_by: Arc::from([]),
            control_group: None,
            can_reload: false,
            has_cgroup_procs: false,
            cgroup_pids: Arc::from([]),
            cpu_pct: missing(),
            memory_bytes: missing(),
            disk_bps: missing(),
            tasks: missing(),
        }
    }

    impl Rig {
        fn startup(&mut self, choice: StartupChoice, units: &[(&str, StartupType)]) {
            let units: Vec<(UnitRef, StartupType)> =
                units.iter().map(|(n, t)| (unit(n), *t)).collect();
            let actions = self.actions.clone();
            self.main
                .update(|window, cx| run_set_startup(choice, &units, &actions, window, cx));
            self.main.run_until_parked();
        }

        fn open(
            &mut self,
            f: fn(&UnitRecord, &Entity<SysActions>, &mut Window, &mut gpui_kit::App),
            u: &UnitRecord,
        ) {
            let actions = self.actions.clone();
            self.main.update(|window, cx| f(u, &actions, window, cx));
            self.main.run_until_parked();
        }

        fn draw(&mut self) {
            self.main.run_until_parked();
            self.main.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn active_tab(&mut self) -> Tab {
            self.main.update(|_, cx| {
                main_window_view(cx)
                    .expect("main window")
                    .read(cx)
                    .active_tab()
            })
        }

        fn queries(&self) -> usize {
            self.engine
                .commands()
                .iter()
                .filter(|c| matches!(c, Command::SetTableQuery(_)))
                .count()
        }
    }

    #[gpui_kit::test]
    fn startup_change_from_masked_unmasks_first(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.startup(
            StartupChoice::Enabled,
            &[("a.service", StartupType::Masked)],
        );
        let sent = rig.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].kind, ActionKind::UnitEnable);
        assert_eq!(
            sent[0].targets.as_slice(),
            [ActionTarget::Unit(unit("a.service"))]
        );
        assert_eq!(rig.toasts(), ["Changing startup type..."]);
        assert_eq!(
            rig.pending_tag("a.service"),
            Some(ActionKindTag::SetStartup)
        );
    }

    #[gpui_kit::test]
    fn startup_same_value_sends_nothing(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        assert_eq!(
            startup_change_kind(StartupType::EnabledRuntime, StartupChoice::Enabled),
            None
        );
        assert_eq!(
            startup_change_kind(StartupType::Static, StartupChoice::Masked),
            Some(ActionKind::UnitMask)
        );
        rig.startup(
            StartupChoice::Enabled,
            &[("a.service", StartupType::EnabledRuntime)],
        );
        assert!(rig.sent().is_empty());
    }

    #[gpui_kit::test]
    fn startup_access_denied_toast(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        rig.startup(
            StartupChoice::Enabled,
            &[("a.service", StartupType::Disabled)],
        );
        let id = only_id(&rig);
        let failed = ActionState::Failed(ActionError::UnitJob {
            result: "failed".into(),
            error_name: Some("org.freedesktop.DBus.Error.AccessDenied".into()),
        });
        rig.finish(id, &[failed]);
        assert_eq!(
            rig.toasts(),
            ["Failed to change startup type: not authorized"]
        );
    }

    #[gpui_kit::test]
    fn file_properties_falls_back_to_unit_file(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let u = unit_rec("sshd.service", "/usr/bin/sshd", Some("/etc/x.service"), 0);
        rig.open(run_file_properties, &u);
        assert!(rig.toasts().is_empty());
        let sent = rig.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].kind,
            ActionKind::ShowInFileManager {
                path: "/usr/bin/sshd".into(),
                properties: true
            }
        );
        let u = unit_rec("b.service", "", Some("/etc/b.service"), 0);
        rig.open(run_file_properties, &u);
        // The first request is still pending, so the second is rejected as busy; check the kind
        // through the first completing.
        let id = sent[0].id;
        rig.emit(id, None, ActionState::Started);
        rig.emit(id, None, ActionState::Succeeded);
        rig.open(run_file_properties, &u);
        let sent = rig.sent();
        assert_eq!(
            sent.last().expect("request").kind,
            ActionKind::ShowInFileManager {
                path: "/etc/b.service".into(),
                properties: true
            }
        );
    }

    #[gpui_kit::test]
    fn open_file_location_no_exec_start(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let u = unit_rec("a.service", "", Some("/etc/a.service"), 0);
        rig.open(run_open_file_location, &u);
        assert_eq!(rig.toasts(), ["Service image unavailable"]);
        assert!(rig.sent().is_empty());
    }

    #[gpui_kit::test]
    fn open_unit_file_uri_encoding(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        assert_eq!(file_uri("/a b/é"), "file:///a%20b/%C3%A9");
        let u = unit_rec(
            "a b.service",
            "",
            Some("/usr/lib/systemd/system/a b.service"),
            0,
        );
        rig.open(run_open_unit_file, &u);
        let sent = rig.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].kind,
            ActionKind::OpenUri("file:///usr/lib/systemd/system/a%20b.service".into())
        );
        rig.emit(sent[0].id, None, ActionState::Started);
        rig.emit(sent[0].id, None, ActionState::Succeeded);
        assert!(rig.toasts().is_empty());
    }

    #[gpui_kit::test]
    fn jump_to_process_main_pid_zero_toast(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let u = unit_rec("a.service", "/usr/bin/a", None, 0);
        let snap = (*rig.engine.latest()).clone();
        let tab = rig.active_tab();
        rig.main
            .update(|window, cx| run_jump_to_process(&u, &snap, window, cx));
        rig.main.run_until_parked();
        assert_eq!(rig.toasts(), ["Service not running"]);
        assert_eq!(rig.active_tab(), tab, "the tab does not change");
    }

    fn proc_record(pid: u32) -> ProcessRecord {
        let a = |s: &str| Arc::<str>::from(s);
        ProcessRecord {
            key: ProcessKey {
                start_ticks: u64::from(pid),
                boot_id: tl_model::time::BootId(0),
                pid,
            },
            ppid: 1,
            uid: 0,
            meta: Arc::new(ProcessMeta {
                comm: a("daemon"),
                name: a("daemon"),
                exe: None,
                cmdline: None,
                user: a("root"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a("daemon"),
            }),
            group: GroupKind::System.key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    #[gpui_kit::test]
    fn jump_to_process_calls_entry_point(cx: &mut TestAppContext) {
        let mut rig = rig(cx);
        let record = proc_record(812);
        let key = record.key;
        rig.engine.publish_with(|s| {
            let mut processes = ProcessSnapshot {
                records: vec![record].into(),
                ..ProcessSnapshot::default()
            };
            processes.header.version = s.sequence;
            s.processes = Arc::new(processes);
        });
        rig.draw();
        let u = unit_rec("a.service", "/usr/bin/a", None, 812);
        let snap = (*rig.engine.latest()).clone();
        let before = rig.queries();
        rig.main
            .update(|window, cx| run_jump_to_process(&u, &snap, window, cx));
        rig.main.run_until_parked();
        assert_eq!(rig.active_tab(), Tab::Processes);
        assert_eq!(rig.queries(), before + 1, "the jump sent one query");
        let generation = rig
            .engine
            .commands()
            .iter()
            .rev()
            .find_map(|c| match c {
                Command::SetTableQuery(q) => Some(q.generation),
                _ => None,
            })
            .expect("query");
        rig.engine.publish_with(|s| {
            let header = ComponentHeader {
                version: s.sequence,
                ..ComponentHeader::default()
            };
            s.table = Arc::new(ProcessTableView {
                header,
                query_generation: generation,
                rows: vec![TableRow {
                    node: NodeId::Process(key),
                    depth: 0,
                    expanded: false,
                    has_children: false,
                    record: Some(0),
                    group: None,
                    descendants: 0,
                    subtree: None,
                }]
                .into(),
                ..ProcessTableView::default()
            });
        });
        rig.draw();
        assert_eq!(rig.toasts(), ["Switched to service process."]);
    }
}

// ---- Group and unit cells (UI-SYS-014, UI-SYS-015, UI-SYS-018) ----

use gpui_kit::assets::IconName;
use tl_model::snapshot::{ServiceStatus, StartupType, UnitRecord};
use tl_model::table::Total;
use tl_model::validity::Validity;
use tl_ui::pages::services::delegate::unit_icon;
use tl_ui::pages::sys_status::DotStyle;
use tl_ui::pages::sys_table::ActionKindTag;
use tl_ui::table::cells::CellOut;
use tl_ui::table::tree_table::{NameIcon, RowFlags};
use tl_ui::theme::current;

use tl_ui::pages::sys_status::LabelTone;

fn record(name: &str, kind: UnitType, status: ServiceStatus) -> UnitRecord {
    UnitRecord {
        unit: UnitRef {
            manager: Manager::System,
            name: Arc::from(name),
        },
        kind,
        description: Arc::from("Secure Shell"),
        status,
        sub_state: Arc::from("running"),
        load_state: Arc::from("loaded"),
        startup: StartupType::Enabled,
        main_pid: 0,
        user: Arc::from("root"),
        path: Arc::from("/usr/bin/sshd"),
        unit_file: Some(Arc::from("/usr/lib/systemd/system/sshd.service")),
        active_since_us: None,
        triggered_by: Arc::from([]),
        control_group: None,
        can_reload: false,
        has_cgroup_procs: false,
        cgroup_pids: Arc::from([]),
        cpu_pct: Total::default(),
        memory_bytes: Total::default(),
        disk_bps: Total::default(),
        tasks: Total::default(),
    }
}

/// Group (row 0), then one row per record, then the User group.
fn delegate(
    cx: &mut TestAppContext,
    records: Vec<UnitRecord>,
    managers: [ManagerState; 2],
) -> ServicesDelegate {
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        let mut d = ServicesDelegate::new(&UiState::default());
        d.refresh_look(cx);
        let mut rows = vec![SysRow {
            node: SysNodeId::ServiceGroup(Manager::System),
            depth: 0,
            expanded: true,
            has_children: true,
            record: None,
        }];
        for (i, r) in records.iter().enumerate() {
            rows.push(SysRow {
                node: SysNodeId::Unit(r.unit.clone()),
                depth: 1,
                expanded: false,
                has_children: false,
                record: Some(i as u32),
            });
        }
        rows.push(SysRow {
            node: SysNodeId::ServiceGroup(Manager::User),
            depth: 0,
            expanded: true,
            has_children: true,
            record: None,
        });
        let mut snap = Snapshot::empty(BootId(1), 1);
        snap.services = Arc::new(ServicesSnapshot {
            rows: rows.into(),
            units: records.into(),
            managers,
            manager_counts: [214, 3],
            ..ServicesSnapshot::default()
        });
        d.install(&Arc::new(snap));
        d
    })
}

fn cell(d: &ServicesDelegate, ix: usize, c: ServiceColumn) -> CellOut {
    let mut out = CellOut {
        want_text: true,
        want_tooltip: true,
        ..CellOut::default()
    };
    d.cell(ix, service_id(c), &mut out);
    out
}

const CONNECTED: [ManagerState; 2] = [ManagerState::Connected; 2];

#[gpui_kit::test]
fn unit_row_icon_per_kind(cx: &mut TestAppContext) {
    let kinds = [
        (UnitType::Service, IconName::Cog),
        (UnitType::Socket, IconName::Plug),
        (UnitType::Timer, IconName::Clock),
        (UnitType::Mount, IconName::HardDrive),
        (UnitType::Automount, IconName::HardDrive),
        (UnitType::Path, IconName::FolderOpen),
        (UnitType::Target, IconName::CircleDot),
        (UnitType::Swap, IconName::MemoryStick),
        (UnitType::Slice, IconName::Box),
        (UnitType::Scope, IconName::Box),
    ];
    let records = kinds
        .iter()
        .map(|(k, _)| record(&format!("u.{k:?}"), *k, ServiceStatus::Running))
        .collect();
    let d = delegate(cx, records, CONNECTED);
    for (i, (kind, icon)) in kinds.iter().enumerate() {
        assert_eq!(unit_icon(*kind), *icon);
        assert!(matches!(d.name_cell(i + 1).icon, NameIcon::Lucide(got) if got == *icon));
    }
}

#[gpui_kit::test]
fn unit_row_pid_blank_when_zero(cx: &mut TestAppContext) {
    let mut running = record("sshd.service", UnitType::Service, ServiceStatus::Running);
    running.main_pid = 812;
    let d = delegate(
        cx,
        vec![
            record("a.service", UnitType::Service, ServiceStatus::Stopped),
            running,
        ],
        CONNECTED,
    );
    assert_eq!(cell(&d, 1, ServiceColumn::Pid).text, "");
    assert_eq!(cell(&d, 2, ServiceColumn::Pid).text, "812");
}

#[gpui_kit::test]
fn unit_row_masked_warning_color(cx: &mut TestAppContext) {
    let mut masked = record("x.service", UnitType::Service, ServiceStatus::Stopped);
    masked.startup = StartupType::Masked;
    let d = delegate(cx, vec![masked], CONNECTED);
    let out = cell(&d, 1, ServiceColumn::StartupType);
    let theme = cx.update(|cx| current(cx).clone());
    assert_eq!(out.arc.as_deref(), Some("Masked"));
    assert_eq!(out.text_color, Some(theme.status.warning));
}

#[gpui_kit::test]
fn unit_row_pending_overlay_label(cx: &mut TestAppContext) {
    let stopped = record("a.service", UnitType::Service, ServiceStatus::Stopped);
    let failed = record("b.service", UnitType::Service, ServiceStatus::Failed);
    let mut d = delegate(cx, vec![stopped.clone(), failed], CONNECTED);
    let theme = cx.update(|cx| current(cx).clone());
    d.set_pending(stopped.unit.clone(), Some(ActionKindTag::Start));
    let out = cell(&d, 1, ServiceColumn::Status);
    assert_eq!(out.arc.as_deref(), Some("Starting…"));
    assert_eq!(out.dot, Some(DotStyle::Filled(LabelTone::Info)));
    assert_eq!(out.text_color, None);
    assert_eq!(out.tooltip.as_deref(), Some("inactive (running)"));
    d.set_pending(stopped.unit.clone(), None);
    assert_eq!(
        cell(&d, 1, ServiceColumn::Status).arc.as_deref(),
        Some("Stopped")
    );
    let out = cell(&d, 2, ServiceColumn::Status);
    assert_eq!(out.arc.as_deref(), Some("Failed"));
    assert_eq!(out.selected_text_color, Some(theme.status.danger));
}

#[gpui_kit::test]
fn group_row_unavailable_label(cx: &mut TestAppContext) {
    let d = delegate(
        cx,
        vec![record(
            "a.service",
            UnitType::Service,
            ServiceStatus::Running,
        )],
        [ManagerState::Connected, ManagerState::Unavailable],
    );
    let cell = d.name_cell(2);
    assert_eq!(cell.primary, "User services");
    assert_eq!(cell.glyphs.len(), 1);
    assert_eq!(cell.glyphs[0].0, IconName::CircleAlert);
    assert_eq!(
        cell.secondary.as_deref(),
        Some("User manager not available")
    );
    assert_eq!(d.row_expandable(2), None);
    assert_eq!(d.name_cell(0).primary, "System services (214)");
}

#[gpui_kit::test]
fn group_row_reconnecting_stale_units(cx: &mut TestAppContext) {
    let mut bare = record("a.service", UnitType::Service, ServiceStatus::Running);
    bare.cpu_pct = Total {
        value: 0.0,
        validity: Validity::Missing,
    };
    bare.main_pid = 812;
    bare.startup = StartupType::Masked;
    let mut d = delegate(
        cx,
        vec![bare.clone()],
        [ManagerState::Reconnecting, ManagerState::Connected],
    );
    let group = d.name_cell(0);
    assert_eq!(group.primary, "System services (214)");
    assert_eq!(group.secondary.as_deref(), Some("(reconnecting…)"));
    assert!(d.row_flags(1).has(RowFlags::STALE));
    assert!(d.row_flags(1).has(RowFlags::SECONDARY_NAME));
    let theme = cx.update(|cx| current(cx).clone());
    let columns = [
        ServiceColumn::Name,
        ServiceColumn::DisplayName,
        ServiceColumn::Status,
        ServiceColumn::StartupType,
        ServiceColumn::Pid,
        ServiceColumn::User,
        ServiceColumn::Path,
        ServiceColumn::Manager,
        ServiceColumn::UnitType,
        ServiceColumn::SubState,
        ServiceColumn::LoadState,
        ServiceColumn::Cpu,
        ServiceColumn::Memory,
        ServiceColumn::Disk,
        ServiceColumn::Tasks,
        ServiceColumn::ActiveSince,
        ServiceColumn::UnitFile,
        ServiceColumn::TriggeredBy,
        ServiceColumn::Description,
    ];
    for column in columns {
        let c = cell(&d, 1, column);
        assert_eq!(c.text_color, Some(theme.text_secondary), "{column:?}");
        assert_eq!(
            c.selected_text_color,
            Some(theme.text_selected_secondary),
            "{column:?}"
        );
    }
    assert_eq!(cell(&d, 1, ServiceColumn::Cpu).validity, Validity::Missing);

    d.set_pending(bare.unit.clone(), Some(ActionKindTag::Stop));
    let out = cell(&d, 1, ServiceColumn::Status);
    assert_eq!(out.arc.as_deref(), Some("Stopping…"));
    assert_eq!(out.dot, Some(DotStyle::Filled(LabelTone::Info)));
    assert_eq!(out.text_color, Some(theme.text_secondary));
    assert_eq!(out.selected_text_color, Some(theme.text_selected_secondary));
    assert_eq!(out.tooltip.as_deref(), Some("active (running)"));
}

#[gpui_kit::test]
fn unit_row_tooltips_and_accessible_name(cx: &mut TestAppContext) {
    let d = delegate(
        cx,
        vec![record(
            "sshd.service",
            UnitType::Service,
            ServiceStatus::Running,
        )],
        CONNECTED,
    );
    assert_eq!(
        cell(&d, 1, ServiceColumn::Status).tooltip.as_deref(),
        Some("active (running)")
    );
    assert_eq!(
        d.accessible_name(1).as_deref(),
        Some("sshd.service, Running, Enabled")
    );
    assert_eq!(
        d.accessible_name(0).as_deref(),
        Some("System services, 214 units, expanded")
    );
}

// ---- Child process rows and process demand (UI-SYS-016) ----

use smallvec::SmallVec;
use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::snapshot::ProcessSnapshot;
use tl_model::units;
use tl_model::validity::FieldValidity;
use tl_ui::pages::services::delegate::{ChildCell, child_column, child_demand};

fn child_key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: u64::from(pid),
        boot_id: BootId(1),
        pid,
    }
}

fn child_process(pid: u32, flags: ProcFlags) -> ProcessRecord {
    let text = |value: &str| Arc::<str>::from(value);
    ProcessRecord {
        key: child_key(pid),
        ppid: 1,
        uid: 0,
        meta: Arc::new(ProcessMeta {
            comm: text("sshd"),
            name: text("sshd"),
            exe: Some(text("/usr/bin/sshd")),
            cmdline: None,
            user: text("root"),
            cgroup: None,
            app_id: None,
            icon: None,
            description: Some(text("OpenSSH daemon")),
            desktop_path: None,
            search_lower: text("sshd"),
        }),
        group: GroupKind::System.key(),
        state: ProcState::Sleeping,
        flags,
        nice: 0,
        threads: 3,
        cpu_pct: 0.5,
        rss_anon_bytes: 5_452_595,
        rss_bytes: 9_000_000,
        pss_bytes: 0,
        uss_bytes: 0,
        swap_bytes: 0,
        io_read_bps: 0.,
        io_write_bps: 0.,
        net_rx_bps: 0.,
        net_tx_bps: 0.,
        gpu_util_pct: 0.,
        gpu_mem_bytes: 0,
        euid: None,
        net_shared: false,
        cancelled_write_bps: 0.,
        validity: FieldValidity::all(Validity::Valid),
    }
}

/// System group (row 0), the expanded sshd.service unit (row 1), one child row per key from
/// row 2, then the User group. `records` is the process snapshot.
fn install_children(
    d: &mut ServicesDelegate,
    children: &[ProcessKey],
    records: Vec<ProcessRecord>,
) {
    let unit = record("sshd.service", UnitType::Service, ServiceStatus::Running);
    let mut rows = vec![
        SysRow {
            node: SysNodeId::ServiceGroup(Manager::System),
            depth: 0,
            expanded: true,
            has_children: true,
            record: None,
        },
        SysRow {
            node: SysNodeId::Unit(unit.unit.clone()),
            depth: 1,
            expanded: true,
            has_children: true,
            record: Some(0),
        },
    ];
    for key in children {
        rows.push(SysRow {
            node: SysNodeId::UnitProcess(unit.unit.clone(), *key),
            depth: 2,
            expanded: false,
            has_children: false,
            // Stale on purpose: the delegate finds the record by key.
            record: Some(7),
        });
    }
    rows.push(SysRow {
        node: SysNodeId::ServiceGroup(Manager::User),
        depth: 0,
        expanded: true,
        has_children: true,
        record: None,
    });
    let mut records = records;
    records.sort_by_key(|r| r.key);
    let mut snap = Snapshot::empty(BootId(1), 1);
    snap.services = Arc::new(ServicesSnapshot {
        rows: rows.into(),
        units: vec![unit].into(),
        managers: CONNECTED,
        ..ServicesSnapshot::default()
    });
    snap.processes = Arc::new(ProcessSnapshot {
        records: records.into(),
        ..ProcessSnapshot::default()
    });
    d.install(&Arc::new(snap));
}

fn child_delegate(
    cx: &mut TestAppContext,
    children: &[ProcessKey],
    records: Vec<ProcessRecord>,
) -> ServicesDelegate {
    let mut d = delegate(cx, Vec::new(), CONNECTED);
    install_children(&mut d, children, records);
    d
}

/// The text a cell shows: its `arc`, else its formatted text.
fn shown(out: &CellOut) -> String {
    out.arc
        .as_deref()
        .map_or_else(|| out.text.clone(), str::to_owned)
}

#[gpui_kit::test]
fn child_rows_column_mapping(cx: &mut TestAppContext) {
    let process = child_process(812, ProcFlags::empty());
    let d = child_delegate(cx, &[child_key(812)], vec![process.clone()]);
    assert_eq!(d.row_kind(2), RowKind::Process);
    assert_eq!(d.row_depth(2), 2);

    for column in [ServiceColumn::Status, ServiceColumn::StartupType] {
        let out = cell(&d, 2, column);
        assert_eq!(shown(&out), "", "{column:?}");
        assert_eq!(out.dot, None, "{column:?}");
    }
    assert_eq!(shown(&cell(&d, 2, ServiceColumn::Pid)), "812");
    let mut memory = String::new();
    units::bytes(&mut memory, process.rss_anon_bytes);
    assert_eq!(memory, "5.20 MiB");
    assert_eq!(shown(&cell(&d, 2, ServiceColumn::Memory)), memory);
    assert_eq!(
        shown(&cell(&d, 2, ServiceColumn::DisplayName)),
        "OpenSSH daemon"
    );
    assert_eq!(shown(&cell(&d, 2, ServiceColumn::User)), "root");
    assert_eq!(shown(&cell(&d, 2, ServiceColumn::Path)), "/usr/bin/sshd");
    assert_eq!(shown(&cell(&d, 2, ServiceColumn::Tasks)), "3");
    for column in [
        ServiceColumn::Manager,
        ServiceColumn::UnitType,
        ServiceColumn::SubState,
        ServiceColumn::LoadState,
        ServiceColumn::ActiveSince,
        ServiceColumn::UnitFile,
        ServiceColumn::TriggeredBy,
        ServiceColumn::Description,
    ] {
        assert_eq!(shown(&cell(&d, 2, column)), "", "{column:?}");
    }
    assert_eq!(d.name_cell(2).primary, "sshd");
    assert!(!d.row_flags(2).has(RowFlags::EXITED));

    let mapping = [
        (ServiceColumn::Name, ChildCell::Name),
        (ServiceColumn::DisplayName, ChildCell::Description),
        (ServiceColumn::Status, ChildCell::Empty),
        (ServiceColumn::StartupType, ChildCell::Empty),
        (ServiceColumn::Pid, ChildCell::Pid),
        (ServiceColumn::User, ChildCell::User),
        (ServiceColumn::Path, ChildCell::ExePath),
        (ServiceColumn::Manager, ChildCell::Empty),
        (ServiceColumn::UnitType, ChildCell::Empty),
        (ServiceColumn::SubState, ChildCell::Empty),
        (ServiceColumn::LoadState, ChildCell::Empty),
        (ServiceColumn::Cpu, ChildCell::Cpu),
        (ServiceColumn::Memory, ChildCell::Memory),
        (ServiceColumn::Disk, ChildCell::Disk),
        (ServiceColumn::Tasks, ChildCell::Threads),
        (ServiceColumn::ActiveSince, ChildCell::Empty),
        (ServiceColumn::UnitFile, ChildCell::Empty),
        (ServiceColumn::TriggeredBy, ChildCell::Empty),
        (ServiceColumn::Description, ChildCell::Empty),
    ];
    assert_eq!(mapping.len(), services_columns().len());
    for (column, want) in mapping {
        assert_eq!(child_column(column), want, "{column:?}");
    }
}

#[gpui_kit::test]
fn child_row_exited_style(cx: &mut TestAppContext) {
    let gone = child_key(900);
    let d = child_delegate(
        cx,
        &[child_key(812), gone],
        vec![child_process(812, ProcFlags::EXITED)],
    );
    let theme = cx.update(|cx| current(cx).clone());
    assert!(d.row_flags(2).has(RowFlags::EXITED));
    assert!(d.name_cell(2).primary == "sshd");
    for column in [
        ServiceColumn::Pid,
        ServiceColumn::Memory,
        ServiceColumn::User,
    ] {
        assert_eq!(
            cell(&d, 2, column).text_color,
            Some(theme.text_secondary),
            "{column:?}"
        );
    }

    // A key no longer in the process snapshot draws as exited until the services update.
    assert!(d.row_flags(3).has(RowFlags::EXITED));
    let pid = cell(&d, 3, ServiceColumn::Pid);
    assert_eq!(shown(&pid), "900");
    assert_eq!(pid.text_color, Some(theme.text_secondary));
    assert_eq!(shown(&cell(&d, 3, ServiceColumn::Memory)), "");
}

/// A delegate table with an engine, the unit and three children installed, the page active
/// and the unit expanded with the three child rows visible.
fn expanded_with_children(
    cx: &mut TestAppContext,
) -> (
    Entity<TlTreeTable<ServicesDelegate>>,
    Arc<FakeEngine>,
    Vec<ProcessKey>,
) {
    let (table, engine) = delegate_table_with_engine(cx, UiState::default());
    let keys = vec![child_key(812), child_key(813), child_key(814)];
    let records = keys
        .iter()
        .map(|k| child_process(k.pid, ProcFlags::empty()))
        .collect();
    cx.update(|cx| {
        table.update(cx, |table, cx| {
            install_children(table.delegate_mut(), &keys, records);
            table.on_new_version(cx);
            let d = table.delegate_mut();
            d.set_active(true, cx);
            let part = d.demand_part();
            assert!(!part.process_table);
            assert!(part.detail_keys.is_empty());
            d.on_toggle_expand(d.row_id(1), cx);
            let ids: Vec<u64> = (0..d.row_count()).map(|ix| d.row_id(ix)).collect();
            d.on_visible_range(&ids, cx);
        });
    });
    cx.executor().advance_clock(DEMAND_COALESCE * 2);
    cx.run_until_parked();
    (table, engine, keys)
}

#[gpui_kit::test]
fn expanding_unit_enables_process_demand(cx: &mut TestAppContext) {
    let (table, engine, keys) = expanded_with_children(cx);
    let part = cx.update(|cx| table.read(cx).delegate.demand_part());
    assert!(part.any_surface_visible && part.services);
    assert!(part.process_table);
    assert_eq!(&*part.detail_keys, &keys[..]);
    let sent = engine.demands().last().cloned().expect("a demand was sent");
    assert!(sent.process_table);
    assert_eq!(&*sent.detail_keys, &keys[..]);

    let many: Vec<ProcessKey> = (0..600).map(child_key).collect();
    assert_eq!(child_demand(1, &many).1.len(), 512);
}

#[gpui_kit::test]
fn collapsing_last_unit_clears_process_demand(cx: &mut TestAppContext) {
    let (table, engine, keys) = expanded_with_children(cx);
    cx.update(|cx| {
        table.update(cx, |table, cx| {
            let d = table.delegate_mut();
            d.on_toggle_expand(d.row_id(1), cx);
        });
    });
    cx.executor().advance_clock(DEMAND_COALESCE * 2);
    cx.run_until_parked();
    let part = cx.update(|cx| table.read(cx).delegate.demand_part());
    assert!(part.any_surface_visible && part.services);
    assert!(!part.process_table);
    assert!(part.detail_keys.is_empty());
    let sent = engine.demands().last().cloned().expect("a demand was sent");
    assert!(!sent.process_table);
    assert!(sent.detail_keys.is_empty());
    assert_eq!(child_demand(0, &keys), (false, SmallVec::new()));
}
