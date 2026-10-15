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
