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
    assert!(
        !entries[0].keep_open,
        "the last checked row must not flip its mark"
    );

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
