//! Process table query state (tasklens-pdu.11).

use std::sync::Arc;

use gpui_kit::TestAppContext;
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::ids::{ColumnId, NodeId, ProcessKey};
use tl_model::process::GroupKind;
use tl_model::table::{ProcessTableView, SortSpec, TableMode, TableQuery, TableSettings};
use tl_model::time::BootId;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::settings_global::TlSettings;
use tl_ui_procs::pages::processes::cells::ProcColumnKind;
use tl_ui_procs::pages::processes::table::{GroupHeader, ProcessQuery};
use tl_ui_settings::settings::{NetworkUnit, Settings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

fn query() -> ProcessQuery {
    ProcessQuery::new(&Settings::default(), &UiState::default())
}

fn view(generation: u64) -> Arc<ProcessTableView> {
    Arc::new(ProcessTableView {
        query_generation: generation,
        ..ProcessTableView::default()
    })
}

fn process(pid: u32) -> NodeId {
    NodeId::Process(ProcessKey {
        start_ticks: 100,
        boot_id: BootId(0),
        pid,
    })
}

fn app(id: &str) -> NodeId {
    NodeId::Group(GroupKind::App { app_id: id.into() }.key())
}

fn sent(engine: &FakeEngine) -> Vec<TableQuery> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::SetTableQuery(q) => Some(q),
            _ => None,
        })
        .collect()
}

#[test]
fn query_defaults_from_settings_and_state() {
    let q = query();
    assert_eq!(q.mode(), TableMode::Grouped);
    assert_eq!(
        q.sort(),
        SortSpec {
            column: ColumnId::Pid,
            ascending: true
        }
    );
    assert_eq!(q.generation(), 0);
    for h in GroupHeader::ALL {
        assert!(q.is_expanded(&h.node()), "{h:?}");
    }
    assert!(!q.is_expanded(&NodeId::KernelThreads));
    let sent = q.to_query();
    assert_eq!(sent.expanded.len(), 3);
    assert!(!sent.expanded.contains(&NodeId::KernelThreads));

    let mut settings = Settings::default();
    settings.processes.mode = TableMode::Tree;
    let mut state = UiState::default();
    state.sort.processes.column = "cpu".to_owned();
    state.sort.processes.ascending = false;
    state.processes.group_expanded.background = false;
    let q = ProcessQuery::new(&settings, &state);
    assert_eq!(q.mode(), TableMode::Tree);
    assert_eq!(
        q.sort(),
        SortSpec {
            column: ColumnId::Cpu,
            ascending: false
        }
    );
    assert!(q.is_expanded(&GroupHeader::Apps.node()));
    assert!(!q.is_expanded(&GroupHeader::Background.node()));
    assert!(q.is_expanded(&GroupHeader::System.node()));
}

#[test]
fn stale_generation_results_ignored() {
    let engine = FakeEngine::new();
    let mut q = query();
    for _ in 0..3 {
        q.bump_and_send(&*engine);
    }
    let three = view(3);
    assert!(q.accept(&three));
    q.bump_and_send(&*engine);
    assert_eq!(q.generation(), 4);

    assert!(!q.accept(&view(3)));
    assert!(Arc::ptr_eq(q.shown().expect("shown"), &three));

    let four = view(4);
    assert!(q.accept(&four));
    assert!(Arc::ptr_eq(q.shown().expect("shown"), &four));
    assert!(q.accept(&four));
    assert!(Arc::ptr_eq(q.shown().expect("shown"), &four));
}

#[test]
fn sort_first_click_desc_numeric_asc_text() {
    let mut q = query();
    let s = q.set_sort(ColumnId::Cpu, ProcColumnKind::Percent);
    assert_eq!(
        s,
        SortSpec {
            column: ColumnId::Cpu,
            ascending: false
        }
    );
    let s = q.set_sort(ColumnId::Cpu, ProcColumnKind::Percent);
    assert_eq!(
        s,
        SortSpec {
            column: ColumnId::Cpu,
            ascending: true
        }
    );
    assert_eq!(q.sort(), s);

    let s = q.set_sort(ColumnId::Name, ProcColumnKind::Text);
    assert_eq!(
        s,
        SortSpec {
            column: ColumnId::Name,
            ascending: true
        }
    );
}

#[test]
fn collapse_all_keeps_group_headers() {
    let mut q = query();
    q.toggle(app("firefox"));
    q.toggle(process(42));
    q.expand_path(&[app("kate"), process(7), NodeId::KernelThreads]);
    assert_eq!(q.to_query().expanded.len(), 8);

    q.collapse_all();
    let expanded = q.to_query().expanded;
    assert_eq!(expanded.len(), 3);
    for h in GroupHeader::ALL {
        assert!(expanded.contains(&h.node()), "{h:?}");
    }
}

#[gpui_kit::test]
fn group_expand_persists_to_state(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine;
    let saved: Arc<std::sync::Mutex<Vec<UiState>>> = Arc::default();
    let log = Arc::clone(&saved);
    cx.update(|cx| {
        cx.set_global(EngineGlobal(client));
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
            persist: Some(Box::new(move |_, state| {
                log.lock().unwrap().push(state.clone());
            })),
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });

    let mut q = cx.read(|cx| ProcessQuery::new(TlSettings::settings(cx), TlSettings::state(cx)));
    q.toggle(GroupHeader::Apps.node());
    q.toggle(process(42));
    q.set_sort(ColumnId::Memory, ProcColumnKind::MemorySize);
    cx.update(|cx| q.persist(cx));

    let state = cx.read(|cx| TlSettings::state(cx).clone());
    let g = &state.processes.group_expanded;
    assert!(!g.apps);
    assert!(g.background);
    assert!(g.system);
    assert_eq!(state.sort.processes.column, "memory");
    assert!(!state.sort.processes.ascending);
    assert_eq!(saved.lock().unwrap().last(), Some(&state));

    // Expanding a process row alone changes nothing in state.toml.
    q.toggle(process(43));
    cx.update(|cx| q.persist(cx));
    assert_eq!(saved.lock().unwrap().len(), 1);
}

#[test]
fn each_change_sends_one_query() {
    let engine = FakeEngine::new();
    let mut q = query();

    q.set_mode(TableMode::Tree);
    q.bump_and_send(&*engine);
    q.set_sort(ColumnId::Cpu, ProcColumnKind::Percent);
    q.bump_and_send(&*engine);
    q.set_filter(Arc::from("fire"));
    q.bump_and_send(&*engine);
    q.toggle(process(42));
    q.bump_and_send(&*engine);

    let sent = sent(&engine);
    assert_eq!(sent.len(), 4);
    let generations: Vec<u64> = sent.iter().map(|s| s.generation).collect();
    assert_eq!(generations, [1, 2, 3, 4]);
    assert_eq!(sent[0].mode, TableMode::Tree);
    assert_eq!(sent[1].sort.column, ColumnId::Cpu);
    assert_eq!(&*sent[2].filter, "fire");
    assert!(sent[3].expanded.contains(&process(42)));
    assert!(!sent[2].expanded.contains(&process(42)));

    let mut settings = Settings::default();
    settings.processes.mode = TableMode::Tree;
    assert!(!q.sync_settings(&settings, &*engine));
    settings.processes.mode = TableMode::Flat;
    assert!(q.sync_settings(&settings, &*engine));
    let sent = self::sent(&engine);
    assert_eq!(sent.len(), 5);
    assert_eq!(sent[4].mode, TableMode::Flat);
    assert_eq!(sent[4].generation, 5);
}

/// The network unit, `subtree_totals` and `hide_system` are window settings: each query carries
/// them, and a change sends a new query (ADR-018, tasklens-q4x76.22).
#[test]
fn query_carries_table_settings() {
    let engine = FakeEngine::new();
    let mut settings = Settings::default();
    let mut q = ProcessQuery::new(&settings, &UiState::default());
    q.bump_and_send(&*engine);
    assert_eq!(sent(&engine)[0].settings, TableSettings::default());
    assert!(!q.sync_settings(&settings, &*engine), "nothing changed");

    settings.processes.hide_system = true;
    assert!(q.sync_settings(&settings, &*engine));
    settings.units.network = NetworkUnit::Bytes;
    settings.processes.subtree_totals = true;
    assert!(q.sync_settings(&settings, &*engine));
    let sent = sent(&engine);
    assert_eq!(sent.len(), 3);
    assert!(sent[1].settings.hide_system);
    assert_eq!(sent[1].settings.network, NetworkUnit::Bits);
    assert_eq!(
        sent[2].settings,
        TableSettings {
            network: NetworkUnit::Bytes,
            network_auto_scale: false,
            subtree_totals: true,
            hide_system: true,
        }
    );
}

#[test]
fn busy_query_is_resent() {
    let engine = FakeEngine::new();
    let mut q = query();
    engine.set_busy(true);
    q.bump_and_send(&*engine);
    assert!(sent(&engine).is_empty());
    engine.set_busy(false);
    q.resend_if_unsent(&*engine);
    q.resend_if_unsent(&*engine);
    let sent = sent(&engine);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].generation, 1);
}

// ---- ProcessDelegate (tasklens-pdu.12) ----

mod delegate {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::InteractiveElement as _;
    use gpui_kit::{
        AppContext as _, Context, Entity, EntityId, IntoElement, ParentElement, Render, Styled,
        TestAppContext, VisualTestContext, Window, div, px,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Command;
    use tl_model::demand::Demand;
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::{ColumnId, DeviceIdx, GroupKey, NodeId, ProcessKey, PseudoKind};
    use tl_model::process::{
        GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord,
    };
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, Snapshot};
    use tl_model::table::{Manager, ProcessTableView, TableRow, UnitRef};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui_base::app_env::EngineGlobal;
    use tl_ui_base::demand::{DemandAggregator, GlobalDemand};
    use tl_ui_base::host::HostContext;
    use tl_ui_base::hub::SnapshotHub;
    use tl_ui_base::i18n::{init_strings, tr_calls};
    use tl_ui_procs::pages::processes::cells::{NO_SOURCE, PROCESS_COLUMNS, process_column};
    use tl_ui_procs::pages::processes::delegate::{
        ProcRowId, ProcessDelegate, SHIPPED_PHASE, UnitIds,
    };
    use tl_ui_procs::pages::processes::table::GroupHeader;
    use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_ui_table::table::actions::{OpenProperties, ResizeColumnsToFit};
    use tl_ui_table::table::cells::CellOut;
    use tl_ui_table::table::columns::TableId;
    use tl_ui_table::table::tree_table::{IconSlot, RowKind, TlTreeTable, TreeTableDelegate};
    use tl_ui_testkit::fake_engine::FakeEngine;

    pub(super) fn setup(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui_table::table::actions::bind_keys_table(cx);
            tl_ui_base::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            init_strings(cx, "en-US");
        });
    }

    pub(super) fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(0),
            pid,
        }
    }

    pub(super) fn record(pid: u32, name: &str, flags: ProcFlags) -> ProcessRecord {
        let a = |s: &str| Arc::<str>::from(s);
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: a(name),
                name: a(name),
                exe: None,
                cmdline: None,
                user: a("oystein"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a(name),
            }),
            group: GroupKind::System.key(),
            state: ProcState::Sleeping,
            flags,
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

    pub(super) fn group(node: NodeId, kind: GroupRowKind, name: &str, members: u32) -> GroupRow {
        GroupRow {
            node,
            key: GroupKey(7),
            kind,
            class: GroupKind::System,
            name: Arc::from(name),
            icon: None,
            members,
            pids: members,
            note: None,
            memory_bytes: 0,
            disk_bps: 0.0,
            net_bps: 0.0,
            gpu_pct: 0.0,
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
            threads: 0,
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            commit_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            handles: None,
            desktop_path: None,
            description: None,
            user: None,
            frozen: false,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    pub(super) fn row(
        node: NodeId,
        depth: u8,
        record: Option<u32>,
        group: Option<u32>,
    ) -> TableRow {
        TableRow {
            node,
            depth,
            expanded: false,
            has_children: false,
            record,
            group,
            descendants: 0,
            subtree: None,
        }
    }

    pub(super) fn firefox() -> NodeId {
        NodeId::Group(
            GroupKind::App {
                app_id: Arc::from("firefox"),
            }
            .key(),
        )
    }

    fn unit() -> NodeId {
        NodeId::Unit(UnitRef {
            manager: Manager::System,
            name: Arc::from("foo.service"),
        })
    }

    /// One row of every kind, in the order the acceptance test names them, then the Kernel
    /// threads (N) row and an expanded parent.
    pub(super) fn every_kind() -> (Vec<TableRow>, Vec<GroupRow>, Vec<ProcessRecord>) {
        let records = vec![
            record(10, "bash", ProcFlags::empty()),
            record(20, "kworker/0:1", ProcFlags::KERNEL_THREAD),
            record(30, "sshd", ProcFlags::empty()),
        ];
        let groups = vec![
            group(firefox(), GroupRowKind::App, "Firefox", 14),
            group(unit(), GroupRowKind::Unit, "Foo daemon", 2),
            group(
                NodeId::Pseudo(PseudoKind::Interrupts),
                GroupRowKind::Pseudo,
                "Interrupts",
                0,
            ),
            group(
                NodeId::KernelThreads,
                GroupRowKind::KernelThreads,
                "Kernel threads",
                5,
            ),
        ];
        let rows = vec![
            TableRow {
                expanded: true,
                has_children: true,
                descendants: 20,
                ..row(GroupHeader::Apps.node(), 0, None, None)
            },
            TableRow {
                has_children: true,
                ..row(firefox(), 1, None, Some(0))
            },
            row(unit(), 1, None, Some(1)),
            TableRow {
                has_children: true,
                descendants: 3,
                ..row(NodeId::Process(key(10)), 1, Some(0), None)
            },
            row(NodeId::Process(key(20)), 1, Some(1), None),
            row(NodeId::Pseudo(PseudoKind::Interrupts), 1, None, Some(2)),
            row(NodeId::KernelThreads, 1, None, Some(3)),
            TableRow {
                has_children: true,
                expanded: true,
                descendants: 3,
                ..row(NodeId::Process(key(30)), 1, Some(2), None)
            },
        ];
        (rows, groups, records)
    }

    pub(super) fn snapshot(
        version: u64,
        rows: Vec<TableRow>,
        groups: Vec<GroupRow>,
        records: Vec<ProcessRecord>,
    ) -> Arc<Snapshot> {
        let mut s = Snapshot::empty(BootId(0), 0);
        s.table = Arc::new(ProcessTableView {
            header: ComponentHeader {
                version,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: rows.into(),
            groups: groups.into(),
            ..ProcessTableView::default()
        });
        s.processes = Arc::new(ProcessSnapshot {
            records: records.into(),
            ..ProcessSnapshot::default()
        });
        Arc::new(s)
    }

    fn new_delegate(vcx: &mut VisualTestContext, snap: Arc<Snapshot>) -> ProcessDelegate {
        vcx.update(|window, cx| {
            ProcessDelegate::new(
                &HostContext::local(cx),
                snap,
                &Settings::default(),
                &UiState::default(),
                window,
                cx,
            )
        })
    }

    fn text(d: &ProcessDelegate, ix: usize, col: ColumnId) -> (String, Validity) {
        let mut out = CellOut {
            want_text: true,
            ..CellOut::default()
        };
        d.cell(ix, col, &mut out);
        let text = out.arc.as_deref().map_or(out.text.clone(), str::to_owned);
        (text, out.validity)
    }

    #[gpui_kit::test]
    fn process_rows_render_each_kind(cx: &mut TestAppContext) {
        setup(cx);
        let (rows, groups, records) = every_kind();
        let vcx = cx.add_empty_window();
        let d = new_delegate(vcx, snapshot(1, rows, groups, records));

        let kinds: Vec<RowKind> = (0..d.row_count()).map(|ix| d.row_kind(ix)).collect();
        assert_eq!(
            kinds,
            [
                RowKind::GroupHeader,
                RowKind::App,
                RowKind::Unit,
                RowKind::Process,
                RowKind::KernelThread,
                RowKind::Pseudo,
                RowKind::KernelThread,
                RowKind::Process,
            ]
        );

        for (ix, kind) in kinds.iter().enumerate() {
            let want = if *kind == RowKind::App {
                IconSlot::Large
            } else {
                IconSlot::Small
            };
            assert_eq!(d.name_cell(ix).icon_size, want, "row {ix}");
        }

        let app = d.name_cell(1);
        assert_eq!(app.primary.as_ref(), "Firefox (14)");
        assert_eq!(
            &app.primary[app.count_run.clone().expect("count run")],
            "(14)"
        );
        assert_eq!(d.name_cell(0).primary.as_ref(), "Apps (20)");
        assert_eq!(d.name_cell(2).primary.as_ref(), "Foo daemon (2)");
        assert_eq!(d.name_cell(3).primary.as_ref(), "bash");
        assert_eq!(d.name_cell(5).primary.as_ref(), "Interrupts");
        assert_eq!(d.name_cell(6).primary.as_ref(), "Kernel threads (5)");

        // PID is blank on header, app, unit and pseudo rows.
        for ix in [0, 1, 2, 5] {
            assert_eq!(
                text(&d, ix, ColumnId::Pid),
                (String::new(), Validity::Valid),
                "row {ix}"
            );
        }
        // "+N" only on the collapsed parent.
        assert_eq!(text(&d, 3, ColumnId::Pid).0, "10 +3");
        assert_eq!(text(&d, 4, ColumnId::Pid).0, "20");
        assert_eq!(text(&d, 7, ColumnId::Pid).0, "30");

        // Validity tooltips come with the cell; the table shows them on the hovered row.
        let mut out = CellOut::default();
        d.cell(3, ColumnId::StartTime, &mut out);
        assert_eq!(out.validity, Validity::WarmingUp);
        assert_eq!(out.tooltip.as_deref(), Some("Collecting first sample"));
        let mut out = CellOut::default();
        d.cell(3, ColumnId::Cpu, &mut out);
        assert_eq!(out.tooltip, None);
    }

    #[gpui_kit::test]
    fn group_header_row_not_selectable(cx: &mut TestAppContext) {
        setup(cx);
        let (rows, groups, records) = every_kind();
        let vcx = cx.add_empty_window();
        let d = new_delegate(vcx, snapshot(1, rows, groups, records));
        let selectable: Vec<bool> = (0..d.row_count()).map(|ix| d.selectable(ix)).collect();
        assert_eq!(
            selectable,
            [false, true, true, true, true, true, true, true]
        );
    }

    #[gpui_kit::test]
    fn row_index_rebuilt_per_version(cx: &mut TestAppContext) {
        setup(cx);
        let (rows, groups, records) = every_kind();
        let vcx = cx.add_empty_window();
        let mut d = new_delegate(
            vcx,
            snapshot(1, rows.clone(), groups.clone(), records.clone()),
        );
        let bash = ProcRowId::Process(key(10));
        let unit_id = d.id_of(&unit()).expect("unit numbered");
        assert_eq!(d.row_index(bash), Some(3));
        assert_eq!(d.row_index(unit_id), Some(2));
        let v1 = d.table_version();

        // The same view again changes nothing.
        let same = Arc::clone(d.snapshot());
        assert!(!d.install(&same));
        assert_eq!(d.table_version(), v1);

        let mut reordered = rows;
        reordered.swap(2, 3);
        assert!(d.install(&snapshot(2, reordered, groups, records)));
        assert!(d.table_version() > v1);
        assert_eq!(d.row_index(bash), Some(2));
        assert_eq!(d.row_index(unit_id), Some(3));
        assert_eq!(d.row_id(2), bash);
        assert_eq!(d.row_index(ProcRowId::Process(key(99))), None);
    }

    #[test]
    fn unit_ids_interned_never_reused() {
        let unit_in = |manager, name: &str| {
            NodeId::Unit(UnitRef {
                manager,
                name: Arc::from(name),
            })
        };
        let mut ids = UnitIds::default();
        let system = ids.intern(&unit_in(Manager::System, "foo.service"));
        let user = ids.intern(&unit_in(Manager::User, "foo.service"));
        let bar = ids.intern(&unit_in(Manager::System, "bar.service"));
        assert_eq!(system, ProcRowId::Unit(0));
        assert_eq!(user, ProcRowId::Unit(1));
        assert_eq!(bar, ProcRowId::Unit(2));
        assert_eq!(ids.intern(&unit_in(Manager::System, "foo.service")), system);
        assert_eq!(ids.get(&unit_in(Manager::User, "foo.service")), Some(user));
        assert_eq!(ids.get(&unit_in(Manager::User, "new.service")), None);
        assert_eq!(
            ids.intern(&NodeId::Process(key(7))),
            ProcRowId::Process(key(7))
        );
    }

    #[gpui_kit::test]
    fn visible_demand_forwarded_capped_512(cx: &mut TestAppContext) {
        setup(cx);
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine;
        let agg = cx.update(|cx| {
            let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
            let agg = cx.new(|cx| DemandAggregator::new(Arc::clone(&client), &hub, cx));
            cx.set_global(GlobalDemand(agg.clone()));
            agg
        });
        cx.update(|cx| {
            agg.update(cx, |a, cx| {
                let page = Demand {
                    any_surface_visible: true,
                    ..Demand::default()
                };
                a.set(EntityId::from(9_999u64), None, page, cx);
            });
        });
        let (rows, groups, records) = every_kind();
        let vcx = cx.add_empty_window();
        let d = new_delegate(vcx, snapshot(1, rows, groups, records));
        let table = vcx.update(|window, cx| cx.new(|cx| TlTreeTable::new(d, window, cx)));
        vcx.update(|_, cx| table.update(cx, |t, cx| t.delegate.set_active(true, cx)));

        // A scroll that shows 40 rows reports 120 ids: 100 processes, 20 other rows.
        let mut ids: Vec<ProcRowId> = (0..100).map(|p| ProcRowId::Process(key(p))).collect();
        ids.extend((0..20).map(|g| ProcRowId::Group(GroupKey(g))));
        vcx.update(|_, cx| table.update(cx, |t, cx| t.delegate.on_visible_range(&ids, cx)));
        let keys = vcx.update(|_, cx| agg.read(cx).merged().detail_keys);
        let want: Vec<ProcessKey> = (0..100).map(key).collect();
        assert_eq!(&*keys, &want[..]);

        let many: Vec<ProcRowId> = (0..600).map(|p| ProcRowId::Process(key(p))).collect();
        vcx.update(|_, cx| table.update(cx, |t, cx| t.delegate.on_visible_range(&many, cx)));
        let keys = vcx.update(|_, cx| agg.read(cx).merged().detail_keys);
        assert_eq!(keys.len(), 512);
        assert_eq!(keys[511], key(511));

        // The part turns the sweep on for the shown columns (tasklens-pdu.52).
        let merged = vcx.update(|_, cx| agg.read(cx).merged());
        assert!(merged.process_table);
        assert!(merged.process_columns.contains(ColumnId::Name));

        // Leaving the tab withdraws the part; a scroll while inactive sends nothing.
        vcx.update(|_, cx| table.update(cx, |t, cx| t.delegate.set_active(false, cx)));
        vcx.update(|_, cx| table.update(cx, |t, cx| t.delegate.on_visible_range(&ids, cx)));
        let merged = vcx.update(|_, cx| agg.read(cx).merged());
        assert!(!merged.process_table);
        assert!(merged.detail_keys.is_empty());
    }

    /// A shown GPU column asks for the per-process GPU rows; without them every GPU cell stays
    /// `WarmingUp` and draws "…" (tasklens-rsf.30).
    #[gpui_kit::test]
    fn gpu_columns_demand_gpu_process(cx: &mut TestAppContext) {
        setup(cx);
        let (rows, groups, records) = every_kind();
        let vcx = cx.add_empty_window();
        let d = new_delegate(vcx, snapshot(1, rows, groups, records));
        let table = vcx.update(|window, cx| cx.new(|cx| TlTreeTable::new(d, window, cx)));
        let show = |vcx: &mut VisualTestContext, col: ColumnId, on: bool| {
            vcx.update(|_, cx| {
                table.update(cx, |t, cx| {
                    let mut layout = t.delegate.columns().clone();
                    layout.set_visible(col, on).expect("visibility changes");
                    t.delegate.on_columns_changed(&layout, cx);
                    t.delegate.demand_part().gpu_process
                })
            })
        };
        let gpu_process = vcx.update(|_, cx| table.read(cx).delegate.demand_part().gpu_process);
        assert!(gpu_process, "GPU is a default column");
        assert!(!show(vcx, ColumnId::Gpu, false), "no GPU column shown");
        assert!(
            show(vcx, ColumnId::GpuMemDedicated, true),
            "dedicated GPU memory"
        );
        assert!(!show(vcx, ColumnId::GpuMemDedicated, false));
        assert!(show(vcx, ColumnId::Gpu, true), "GPU utilization");
    }

    /// A GPU cell with a current value paints that value. A warming-up one paints no value,
    /// only the "…" glyph (tasklens-rsf.30).
    #[gpui_kit::test]
    fn gpu_cell_draws_value(cx: &mut TestAppContext) {
        use gpui_kit::base::Root;
        use tl_model::validity::ProcessField;
        use tl_ui_table::table::cells::{record_painted_texts, take_painted_texts};

        setup(cx);
        let mut busy = record(10, "bash", ProcFlags::empty());
        busy.gpu_util_pct = 37.0;
        let mut waiting = record(30, "sshd", ProcFlags::empty());
        waiting
            .validity
            .set(ProcessField::GpuUtil, Validity::WarmingUp);
        let rows = vec![
            row(NodeId::Process(key(10)), 0, Some(0), None),
            row(NodeId::Process(key(30)), 0, Some(1), None),
        ];
        let snap = snapshot(1, rows, Vec::new(), vec![busy, waiting]);
        let slot = Rc::new(std::cell::RefCell::new(None));
        let keep = Rc::clone(&slot);
        let handle = cx.add_window(move |window, cx| {
            let d = ProcessDelegate::new(
                &HostContext::local(cx),
                snap,
                &Settings::default(),
                &UiState::default(),
                window,
                cx,
            );
            let table = cx.new(|cx| TlTreeTable::new(d, window, cx));
            *keep.borrow_mut() = Some(table.clone());
            Root::new(table, window, cx)
        });
        let table = slot.borrow().clone().expect("table");
        let vcx = &mut VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();
        record_painted_texts();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let painted = take_painted_texts();
        let body = vcx.debug_bounds("TlTreeTable-body").expect("table body");

        // The GPU texts painted in the body, top to bottom. The header total is not in the body.
        let (drawn, want) = vcx.update(|_, cx| {
            let t = table.read(cx);
            let layout = t.columns();
            let mut drawn: Vec<_> = painted
                .iter()
                .filter(|(bounds, _)| {
                    body.contains(&bounds.center())
                        && layout.column_at_x(bounds.center().x) == Some(ColumnId::Gpu)
                })
                .collect();
            drawn.sort_by(|a, b| f32::from(a.0.origin.y).total_cmp(&f32::from(b.0.origin.y)));
            let drawn: Vec<String> = drawn.iter().map(|(_, text)| text.to_string()).collect();
            (drawn, text(&t.delegate, 0, ColumnId::Gpu))
        });
        assert_eq!(want.1, Validity::Valid);
        assert!(want.0.contains("37"), "{want:?}");
        assert_eq!(drawn, [want.0], "only the current row paints a GPU value");
        assert!(
            vcx.debug_bounds("TlCell-glyph-1-gpu").is_some(),
            "the warming-up GPU cell draws its glyph"
        );
    }

    /// A Network cell with per-process rates paints the rate, "0 Mbps" for a process with none,
    /// and no number while the attribution warms up (tasklens-pdu.66).
    #[gpui_kit::test]
    fn network_cell_draws_value(cx: &mut TestAppContext) {
        use gpui_kit::base::Root;
        use tl_model::validity::ProcessField;
        use tl_ui_table::table::cells::{record_painted_texts, take_painted_texts};

        setup(cx);
        let mut busy = record(10, "node", ProcFlags::empty());
        busy.net_rx_bps = 250_000.0;
        busy.net_tx_bps = 37_500.0;
        let idle = record(20, "am", ProcFlags::empty());
        let mut waiting = record(30, "claude", ProcFlags::empty());
        for f in [ProcessField::NetRx, ProcessField::NetTx] {
            waiting.validity.set(f, Validity::WarmingUp);
        }
        let rows = vec![
            row(NodeId::Process(key(10)), 0, Some(0), None),
            row(NodeId::Process(key(20)), 0, Some(1), None),
            row(NodeId::Process(key(30)), 0, Some(2), None),
        ];
        let snap = snapshot(1, rows, Vec::new(), vec![busy, idle, waiting]);
        let slot = Rc::new(std::cell::RefCell::new(None));
        let keep = Rc::clone(&slot);
        let handle = cx.add_window(move |window, cx| {
            let d = ProcessDelegate::new(
                &HostContext::local(cx),
                snap,
                &Settings::default(),
                &UiState::default(),
                window,
                cx,
            );
            let table = cx.new(|cx| TlTreeTable::new(d, window, cx));
            *keep.borrow_mut() = Some(table.clone());
            Root::new(table, window, cx)
        });
        let table = slot.borrow().clone().expect("table");
        let vcx = &mut VisualTestContext::from_window(handle.into(), cx);
        vcx.run_until_parked();
        record_painted_texts();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let painted = take_painted_texts();
        let body = vcx.debug_bounds("TlTreeTable-body").expect("table body");

        let (drawn, want) = vcx.update(|_, cx| {
            let t = table.read(cx);
            let layout = t.columns();
            let mut drawn: Vec<_> = painted
                .iter()
                .filter(|(bounds, _)| {
                    body.contains(&bounds.center())
                        && layout.column_at_x(bounds.center().x) == Some(ColumnId::Network)
                })
                .collect();
            drawn.sort_by(|a, b| f32::from(a.0.origin.y).total_cmp(&f32::from(b.0.origin.y)));
            let drawn: Vec<String> = drawn.iter().map(|(_, text)| text.to_string()).collect();
            let want: Vec<_> = (0..3)
                .map(|i| text(&t.delegate, i, ColumnId::Network))
                .collect();
            (drawn, want)
        });
        // 287 500 B/s is 2.3 Mbps.
        assert_eq!(want[0], ("2.3 Mbps".to_string(), Validity::Valid));
        assert_eq!(want[1], ("0 Mbps".to_string(), Validity::Valid));
        assert_eq!(want[2].1, Validity::WarmingUp);
        assert_eq!(
            drawn,
            ["2.3 Mbps", "0 Mbps"],
            "the two current rows paint a number, the warming-up row paints none"
        );
        assert!(
            vcx.debug_bounds("TlCell-glyph-2-network").is_some(),
            "the warming-up Network cell draws its glyph"
        );
    }

    fn gpu(n: u16, pci: &str, name: &str, present: bool) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(n),
            identity: DeviceIdentity::Gpu {
                uuid: [n as u8; 16],
                pci_bus_id: Arc::from(pci),
            },
            generation: 0,
            display_name: Arc::from(name),
            present,
            parent: None,
            persistent_key: Arc::from(pci),
            net_kind: None,
            hot_c: None,
        }
    }

    #[gpui_kit::test]
    fn text_columns_default_wide_enough_for_a_typical_value(cx: &mut TestAppContext) {
        setup(cx);
        let (rows, groups, records) = every_kind();
        let vcx = cx.add_empty_window();
        let d = new_delegate(vcx, snapshot(1, rows, groups, records));
        for (id, sample) in [
            (ColumnId::Type, "Background process"),
            (ColumnId::Architecture, "AArch64 (64-bit)"),
            (ColumnId::ImagePath, "/usr/lib/systemd/systemd-journald"),
        ] {
            let need = vcx.update(|window, cx| {
                let m = *tl_ui_base::theme::metrics(cx);
                let w = tl_ui_base::text::metrics::TextMetricsCache::default().template_width(
                    sample,
                    m.text.base,
                    window,
                    cx,
                );
                f32::from(w) / m.ui_scale.max(0.01)
            });
            let width = f32::from(d.column_spec(id).default_width);
            assert!(
                width >= need,
                "{} default width {width} px is narrower than {sample:?} ({need} px)",
                id.as_str()
            );
        }
    }

    #[gpui_kit::test]
    fn gpu_slots_only_for_present_adapters(cx: &mut TestAppContext) {
        setup(cx);
        let (rows, groups, records) = every_kind();
        let snap = snapshot(1, rows.clone(), groups.clone(), records.clone());
        let mut with_gpu = (*snap).clone();
        with_gpu.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: vec![
                gpu(0, "0000:01:00.0", "NVIDIA GeForce RTX 4070", true),
                gpu(1, "0000:02:00.0", "Gone card", false),
            ]
            .into(),
        });
        let vcx = cx.add_empty_window();
        let d = new_delegate(vcx, Arc::new(with_gpu));
        let offered = d.offered_columns();
        assert!(offered.contains(&ColumnId::Gpu0));
        assert!(!offered.contains(&ColumnId::Gpu1));
        assert!(d.columns().order.contains(&ColumnId::Gpu0));
        assert!(!d.columns().order.contains(&ColumnId::Gpu1));
        assert_eq!(
            d.column_spec(ColumnId::Gpu0).tooltip.as_deref(),
            Some("NVIDIA GeForce RTX 4070 (0000:01:00.0)")
        );

        // Phase 2 is offered; a NO_SOURCE column is not.
        assert_eq!(SHIPPED_PHASE, 4);
        assert_eq!(process_column(ColumnId::Version).phase, 2);
        assert!(offered.contains(&ColumnId::Version));
        assert!(!offered.contains(&ColumnId::PageTables));
        assert!(!d.columns().order.contains(&ColumnId::PageTables));
        assert!(offered.contains(&ColumnId::Cpu));

        // No GPU: no slot.
        let d = new_delegate(vcx, snapshot(1, rows, groups, records));
        assert!(!d.offered_columns().contains(&ColumnId::Gpu0));
    }

    #[gpui_kit::test]
    fn phase_2_columns_offered(cx: &mut TestAppContext) {
        setup(cx);
        let (rows, groups, records) = every_kind();
        let vcx = cx.add_empty_window();
        let d = new_delegate(vcx, snapshot(1, rows, groups, records));
        let offered = d.offered_columns();
        let listed = [
            ColumnId::Version,
            ColumnId::Timestamp,
            ColumnId::FileMtime,
            ColumnId::FileSize,
            ColumnId::Company,
            ColumnId::Signer,
            ColumnId::Signature,
            ColumnId::Architecture,
            ColumnId::Session,
            ColumnId::SecurityLabel,
            ColumnId::Sandbox,
            ColumnId::CtxSwitches,
            ColumnId::CtxSwitchesDelta,
            ColumnId::MajorFaults,
            ColumnId::Pss,
            ColumnId::Uss,
            ColumnId::Power,
            ColumnId::PowerTrend,
            ColumnId::SchedPolicy,
            ColumnId::IoPriority,
            ColumnId::Cgroup,
            ColumnId::AppId,
            ColumnId::OomScore,
            ColumnId::OomScoreAdj,
            ColumnId::Tty,
            ColumnId::Cwd,
        ];
        for id in listed {
            assert!(offered.contains(&id), "{id:?} not offered");
        }
        // Every phase 2 column that is not a GPU slot is in the list above.
        for c in PROCESS_COLUMNS.iter().filter(|c| c.phase == 2) {
            let slot =
                (ColumnId::Gpu0 as usize..=ColumnId::Gpu15 as usize).contains(&(c.id as usize));
            assert!(
                slot || listed.contains(&c.id),
                "{:?} missing from the list",
                c.id
            );
        }
    }

    #[gpui_kit::test]
    fn no_source_columns_not_offered(cx: &mut TestAppContext) {
        setup(cx);
        let parked = [
            ColumnId::Elevation,
            ColumnId::CommitPeak,
            ColumnId::CommitDelta,
            ColumnId::PageTables,
            ColumnId::MajorFaultsDelta,
            ColumnId::IoReadBytes,
            ColumnId::IoWriteBytes,
            ColumnId::IoReadDelta,
            ColumnId::IoWriteDelta,
            ColumnId::IoReadRate,
            ColumnId::IoWriteRate,
            ColumnId::IoReadOps,
            ColumnId::IoWriteOps,
            ColumnId::DiskReadBytes,
            ColumnId::DiskWriteBytes,
            ColumnId::DiskTotalBytes,
            ColumnId::DiskReadDelta,
            ColumnId::DiskWriteDelta,
            ColumnId::DiskTotalDelta,
            ColumnId::DiskReads,
            ColumnId::DiskWrites,
            ColumnId::DiskReadsDelta,
            ColumnId::DiskWritesDelta,
            ColumnId::GpuMemShared,
            ColumnId::GpuEngine,
            ColumnId::GpuEncode,
            ColumnId::GpuDecode,
            ColumnId::Cycles,
            ColumnId::CyclesDelta,
            ColumnId::X11Resources,
            ColumnId::Container,
        ];
        let mut found: Vec<ColumnId> = PROCESS_COLUMNS
            .iter()
            .filter(|c| c.phase == NO_SOURCE)
            .map(|c| c.id)
            .collect();
        let mut want = parked.to_vec();
        found.sort_by_key(|c| *c as usize);
        want.sort_by_key(|c| *c as usize);
        assert_eq!(found, want);

        let (rows, groups, records) = every_kind();
        let vcx = cx.add_empty_window();
        let d = new_delegate(vcx, snapshot(1, rows, groups, records));
        let offered = d.offered_columns();
        for id in parked {
            assert!(!offered.contains(&id), "{id:?} offered");
            assert!(!d.columns().order.contains(&id), "{id:?} in the order");
        }
    }

    struct Host {
        table: Entity<TlTreeTable<ProcessDelegate>>,
        opened: Rc<Cell<u32>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let opened = self.opened.clone();
            div()
                .w(px(800.))
                .h(px(400.))
                .flex()
                .flex_col()
                .on_action(move |_: &OpenProperties, _, _| opened.set(opened.get() + 1))
                .child(self.table.clone())
        }
    }

    pub(super) fn redraw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    type Opened<'a> = (
        Entity<TlTreeTable<ProcessDelegate>>,
        Rc<Cell<u32>>,
        Arc<FakeEngine>,
        &'a mut VisualTestContext,
    );

    fn open_host(cx: &mut TestAppContext) -> Opened<'_> {
        setup(cx);
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| cx.set_global(EngineGlobal(client)));
        let (rows, groups, records) = every_kind();
        let snap = snapshot(1, rows, groups, records);
        let opened = Rc::new(Cell::new(0));
        let slot = Rc::new(std::cell::RefCell::new(None));
        let (out, hits) = (slot.clone(), opened.clone());
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let d = ProcessDelegate::new(
                &HostContext::local(cx),
                snap,
                &Settings::default(),
                &UiState::default(),
                window,
                cx,
            );
            let table = cx.new(|cx| TlTreeTable::new(d, window, cx));
            *out.borrow_mut() = Some(table.clone());
            Host {
                table,
                opened: hits,
            }
        });
        vcx.update(|window, _| window.activate_window());
        redraw(vcx);
        let table = slot.borrow_mut().take().expect("table created");
        let handle = vcx.update(|_, cx| table.read(cx).focus_handle().clone());
        vcx.update(|window, cx| window.focus(&handle, cx));
        redraw(vcx);
        (table, opened, engine, vcx)
    }

    fn queries(engine: &FakeEngine) -> usize {
        engine
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::SetTableQuery(_)))
            .count()
    }

    #[gpui_kit::test]
    fn enter_opens_properties_space_toggles(cx: &mut TestAppContext) {
        let (table, opened, engine, vcx) = open_host(cx);
        let bash = ProcRowId::Process(key(10));

        // Enter on a process row goes on to the page's OpenProperties handler.
        vcx.update(|_, cx| table.update(cx, |t, _| t.selection.click(bash)));
        vcx.simulate_keystrokes("enter");
        redraw(vcx);
        assert_eq!(opened.get(), 1);
        assert_eq!(queries(&engine), 0);

        // Enter on a group header toggles it and opens nothing.
        let header = ProcRowId::fixed(&GroupHeader::Apps.node()).expect("header id");
        vcx.update(|_, cx| table.update(cx, |t, _| t.selection.set_focus_only(header)));
        vcx.simulate_keystrokes("enter");
        redraw(vcx);
        assert_eq!(opened.get(), 1);
        assert_eq!(queries(&engine), 1);
        let open = vcx.update(|_, cx| {
            table
                .read(cx)
                .delegate
                .query()
                .is_expanded(&GroupHeader::Apps.node())
        });
        assert!(!open, "the Apps header collapsed");

        // Space on a process row with children toggles it.
        vcx.update(|_, cx| table.update(cx, |t, _| t.selection.click(bash)));
        vcx.simulate_keystrokes("space");
        redraw(vcx);
        assert_eq!(queries(&engine), 2);
        let open = vcx.update(|_, cx| {
            table
                .read(cx)
                .delegate
                .query()
                .is_expanded(&NodeId::Process(key(10)))
        });
        assert!(open);
        assert_eq!(opened.get(), 1);
    }

    #[gpui_kit::test]
    fn frames_translate_nothing(cx: &mut TestAppContext) {
        let (table, _, _, vcx) = open_host(cx);
        redraw(vcx);
        let built = vcx.update(|_, cx| table.read(cx).rows_built_last_frame.get());
        assert!(built >= 8, "{built} rows built");
        let before = tr_calls();
        for _ in 0..3 {
            vcx.update(|window, _| window.refresh());
            redraw(vcx);
        }
        assert_eq!(tr_calls(), before, "a frame called tr");
    }

    /// The seed-42 scene shape (40 App groups of 50 records). Values depend only on the index, so
    /// two calls with different versions hold equal values.
    fn synth_2000(version: u64) -> Arc<Snapshot> {
        let mut rows = vec![TableRow {
            expanded: true,
            has_children: true,
            descendants: 2040,
            ..row(GroupHeader::Apps.node(), 0, None, None)
        }];
        let mut groups = Vec::new();
        let mut records = Vec::new();
        for g in 0..40u32 {
            let node = NodeId::Group(
                GroupKind::App {
                    app_id: Arc::from(format!("app{g}")),
                }
                .key(),
            );
            let mut gr = group(node.clone(), GroupRowKind::App, &format!("App {g}"), 50);
            gr.cpu_pct = (g % 10) as f32 * 0.5;
            gr.memory_bytes = (u64::from(g) + 1) * 1024 * 1024;
            groups.push(gr);
            rows.push(TableRow {
                expanded: true,
                has_children: true,
                descendants: 50,
                ..row(node, 1, None, Some(g))
            });
            for m in 0..50u32 {
                let i = g * 50 + m;
                let mut r = record(1000 + i, &format!("proc{i}"), ProcFlags::empty());
                r.cpu_pct = (m % 50) as f32 * 0.1;
                r.rss_bytes = (u64::from(i) + 1) * 1024 * 1024;
                r.threads = 1 + m;
                let ix = records.len() as u32;
                records.push(r);
                rows.push(row(NodeId::Process(key(1000 + i)), 2, Some(ix), None));
            }
        }
        snapshot(version, rows, groups, records)
    }

    struct Host2000 {
        table: Entity<TlTreeTable<ProcessDelegate>>,
        height: f32,
    }

    impl Render for Host2000 {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(800.))
                .h(px(self.height))
                .flex()
                .flex_col()
                .child(self.table.clone())
        }
    }

    fn open_2000(
        cx: &mut TestAppContext,
        height: f32,
    ) -> (Entity<TlTreeTable<ProcessDelegate>>, &mut VisualTestContext) {
        setup(cx);
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine;
        cx.update(|cx| cx.set_global(EngineGlobal(client)));
        let snap = synth_2000(1);
        let slot = Rc::new(std::cell::RefCell::new(None));
        let out = slot.clone();
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let mut d = ProcessDelegate::new(
                &HostContext::local(cx),
                snap,
                &Settings::default(),
                &UiState::default(),
                window,
                cx,
            );
            d.set_fixed_now(Some(1_000_000_000));
            let table = cx.new(|cx| TlTreeTable::new(d, window, cx));
            *out.borrow_mut() = Some(table.clone());
            Host2000 { table, height }
        });
        vcx.update(|window, _| window.activate_window());
        redraw(vcx);
        let table = slot.borrow_mut().take().expect("table created");
        redraw(vcx);
        (table, vcx)
    }

    /// Installs `snap` and redraws. Returns whether the delegate took a new version.
    fn install(
        vcx: &mut VisualTestContext,
        table: &Entity<TlTreeTable<ProcessDelegate>>,
        snap: Arc<Snapshot>,
    ) -> bool {
        let took = vcx.update(|_, cx| {
            table.update(cx, |t, cx| {
                let took = t.delegate.install(&snap);
                if took {
                    t.on_new_version(cx);
                }
                took
            })
        });
        redraw(vcx);
        took
    }

    fn cache_stats(
        vcx: &mut VisualTestContext,
        table: &Entity<TlTreeTable<ProcessDelegate>>,
    ) -> (u64, u64) {
        vcx.update(|_, cx| table.read(cx).text_cache_stats())
    }

    /// 10 identical table versions give a text cache hit rate above 95 % (ADR-009, PROC-013).
    #[gpui_kit::test]
    fn tree_table_text_cache_hit_rate(cx: &mut TestAppContext) {
        let (table, vcx) = open_2000(cx, 720.);
        let (h0, m0) = cache_stats(vcx, &table);
        assert!(m0 > 0, "the first frame formats");
        for v in 2..=11u64 {
            assert!(install(vcx, &table, synth_2000(v)), "version {v} installed");
        }
        let (h1, m1) = cache_stats(vcx, &table);
        let (hits, misses) = (h1 - h0, m1 - m0);
        assert!(hits > 0);
        let rate = hits as f64 / (hits + misses) as f64;
        assert!(
            rate > 0.95,
            "hit rate {rate} ({hits} hits, {misses} misses)"
        );
    }

    /// Two installs with equal values format no cell text: a cache miss is the only path that
    /// calls the formatter.
    #[gpui_kit::test]
    fn no_format_on_unchanged_install(cx: &mut TestAppContext) {
        let (table, vcx) = open_2000(cx, 720.);
        assert!(install(vcx, &table, synth_2000(2)));
        let (_, m1) = cache_stats(vcx, &table);
        assert!(
            install(vcx, &table, synth_2000(3)),
            "a new version was built"
        );
        let (_, m2) = cache_stats(vcx, &table);
        assert_eq!(m2 - m1, 0, "{} cells formatted again", m2 - m1);
    }

    /// At most 27 row elements are built in a 720 px viewport, and the unhovered metric cells
    /// build no element at all (so they carry no element id).
    #[gpui_kit::test]
    fn row_elements_capped_by_viewport(cx: &mut TestAppContext) {
        use gpui_kit::base::ScrollbarHandle;
        use gpui_kit::point;
        let (table, vcx) = open_2000(cx, 720.);
        let built = vcx.update(|_, cx| table.read(cx).rows_built_last_frame.get());
        assert!(built > 0 && built <= 27, "{built} rows built");
        let divs = vcx.update(|_, cx| table.read(cx).cell_divs_last_frame.get());
        assert_eq!(divs, 0, "no hover: metric cells are painted, not elements");

        vcx.update(|_, cx| {
            let handle = table.read(cx).scroll_handle();
            ScrollbarHandle::set_offset(&handle, point(px(0.), px(-14. * 1000.)));
        });
        vcx.update(|window, _| window.refresh());
        redraw(vcx);
        let built = vcx.update(|_, cx| table.read(cx).rows_built_last_frame.get());
        assert!(
            built > 0 && built <= 27,
            "{built} rows built after scrolling"
        );
        let divs = vcx.update(|_, cx| table.read(cx).cell_divs_last_frame.get());
        assert_eq!(divs, 0);
    }

    /// "Resize columns to fit content" makes the Type column at least as wide as its widest
    /// value, even when no visible row shows that value (bead tasklens-pdu.65 d, shot 19c).
    /// Fails before the fix: the fit measured only the visible rows' "System process" and
    /// shorter words, never "Background process".
    #[gpui_kit::test]
    fn fit_to_content_holds_the_widest_type(cx: &mut TestAppContext) {
        const TYPES: [&str; 6] = [
            "App",
            "Unit",
            "Background process",
            "System process",
            "Kernel thread",
            "Pseudo",
        ];
        let (table, _, _, vcx) = open_host(cx);
        vcx.update(|_, cx| {
            table.update(cx, |t, cx| {
                let mut layout = t.delegate.columns().clone();
                assert!(layout.order.contains(&ColumnId::Type));
                layout.visible.insert(ColumnId::Type);
                let scale = layout.ui_scale();
                layout.rebuild(scale);
                t.delegate.on_columns_changed(&layout, cx);
            })
        });
        redraw(vcx);

        let (size, gap, scale) = vcx.update(|_, cx| {
            let m = *tl_ui_base::theme::metrics(cx);
            (m.text.base, m.sp[6], m.ui_scale)
        });
        let advance = |vcx: &mut VisualTestContext, text: &str| {
            vcx.update(|window, cx| {
                let mut cache = tl_ui_base::text::metrics::TextMetricsCache::default();
                text.chars()
                    .fold(px(0.), |w, ch| w + cache.char_advance(ch, size, window, cx))
            })
        };
        let widest = TYPES
            .iter()
            .copied()
            .max_by(|a, b| {
                let (wa, wb) = (advance(vcx, a), advance(vcx, b));
                wa.partial_cmp(&wb).expect("finite widths")
            })
            .expect("six words");
        let need = advance(vcx, widest) + gap;
        // No row shows the widest word, so only the fit samples can make room for it.
        let shown: Vec<String> = vcx.update(|_, cx| {
            let d = &table.read(cx).delegate;
            (0..d.row_count())
                .map(|ix| text(d, ix, ColumnId::Type).0)
                .collect()
        });
        assert!(!shown.iter().any(|s| s == widest), "{shown:?}");

        vcx.dispatch_action(ResizeColumnsToFit {
            table: TableId::Processes,
        });
        redraw(vcx);
        let w = vcx.update(|_, cx| table.read(cx).delegate.columns().widths[&ColumnId::Type]);
        assert!(
            w * scale >= need,
            "Type is {w:?} after the fit, {widest:?} needs {need:?}"
        );
    }
}

/// `ProcessesPage` (tasklens-pdu.1).
mod page {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, Modifiers,
        MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, Point, Render, ScrollStrategy,
        Styled, TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Command;
    use tl_model::command::Tab;
    use tl_model::ids::ColumnId;
    use tl_model::ids::NodeId;
    use tl_model::process::{ProcFlags, ProcessRecord};
    use tl_model::snapshot::Snapshot;
    use tl_model::table::{TableMode, TableQuery, TableRow};
    use tl_model::validity::Validity;
    use tl_ui_base::app_env::EngineGlobal;
    use tl_ui_base::host::HostContext;
    use tl_ui_base::hub::{GlobalHub, SnapshotHub};
    use tl_ui_base::shell::{FilterChanged, GlobalShellEvents, ShellEvents};
    use tl_ui_procs::pages::processes::ProcessesPage;
    use tl_ui_procs::pages::processes::table::GroupHeader;
    use tl_ui_table::table::actions::{CollapseAll, OpenProperties};
    use tl_ui_table::table::tree_table::HEADER_H_TWO;
    use tl_ui_testkit::fake_engine::FakeEngine;

    use super::delegate::{every_kind, firefox, key, record, redraw, row, setup, snapshot};

    /// Rows a 720 px viewport may build: 676 px of body over 28 px rows, plus the partial rows
    /// at both edges and the measuring row.
    const MAX_ROWS_720: u32 = 27;

    struct Host {
        page: Entity<ProcessesPage>,
        opened: Rc<Cell<u32>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let opened = self.opened.clone();
            div()
                .w(px(1000.))
                .h(px(720.))
                .flex()
                .flex_col()
                .on_action(move |_: &OpenProperties, _, _| opened.set(opened.get() + 1))
                .child(self.page.clone())
        }
    }

    struct Opened<'a> {
        page: Entity<ProcessesPage>,
        opened: Rc<Cell<u32>>,
        engine: Arc<FakeEngine>,
        vcx: &'a mut VisualTestContext,
    }

    fn open(cx: &mut TestAppContext) -> Opened<'_> {
        setup(cx);
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            let hub = cx.new(|cx| SnapshotHub::new(client, cx));
            cx.set_global(GlobalHub(hub));
            let events = cx.new(|_| ShellEvents);
            cx.set_global(GlobalShellEvents(events));
        });
        let opened = Rc::new(Cell::new(0));
        let slot = Rc::new(std::cell::RefCell::new(None));
        let (out, hits) = (slot.clone(), opened.clone());
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let page = cx.new(|cx| ProcessesPage::new(&HostContext::local(cx), window, cx));
            *out.borrow_mut() = Some(page.clone());
            Host { page, opened: hits }
        });
        vcx.update(|window, _| window.activate_window());
        redraw(vcx);
        let page = slot.borrow_mut().take().expect("page created");
        vcx.update(|window, cx| page.update(cx, |p, cx| p.focus_table(window, cx)));
        redraw(vcx);
        Opened {
            page,
            opened,
            engine,
            vcx,
        }
    }

    fn sent(engine: &FakeEngine) -> Vec<TableQuery> {
        engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::SetTableQuery(q) => Some(q),
                _ => None,
            })
            .collect()
    }

    /// Publishes `snap` with the hub's next sequence and the given view generation.
    fn publish(engine: &FakeEngine, snap: Arc<Snapshot>, generation: u64) {
        let mut s = Snapshot::clone(&snap);
        s.sequence = engine.latest().sequence + 1;
        Arc::make_mut(&mut s.table).query_generation = generation;
        engine.publish(s);
    }

    /// 2,000 flat process rows with names and CPU values from a seed-42 LCG.
    fn rows_2000(version: u64) -> Arc<Snapshot> {
        let mut state: u64 = 42;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u32
        };
        let mut records: Vec<ProcessRecord> = Vec::with_capacity(2000);
        let mut rows: Vec<TableRow> = Vec::with_capacity(2000);
        for i in 0..2000u32 {
            let pid = 100 + i;
            let mut r = record(
                pid,
                &format!("proc-{}", next() % 10_000),
                ProcFlags::empty(),
            );
            r.cpu_pct = (next() % 1000) as f32 / 10.0;
            records.push(r);
            rows.push(row(NodeId::Process(key(pid)), 0, Some(i), None));
        }
        snapshot(version, rows, Vec::new(), records)
    }

    fn render_count(o: &mut Opened<'_>) -> u32 {
        let page = o.page.clone();
        o.vcx
            .update(|_, cx| page.read(cx).table().read(cx).render_count.get())
    }

    fn rows_built(o: &mut Opened<'_>) -> u32 {
        let page = o.page.clone();
        o.vcx
            .update(|_, cx| page.read(cx).table().read(cx).rows_built_last_frame.get())
    }

    /// Builds the page after the hub already holds a view with rows for `generation`, draws,
    /// and returns the row count the table shows.
    fn open_after_view(cx: &mut TestAppContext, generation: u64) -> usize {
        setup(cx);
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (rows, groups, records) = every_kind();
        publish(&engine, snapshot(2, rows, groups, records), generation);
        cx.update(|cx| {
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            let hub = cx.new(|cx| SnapshotHub::new(client, cx));
            cx.set_global(GlobalHub(hub));
        });
        let slot = Rc::new(std::cell::RefCell::new(None));
        let out = slot.clone();
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let page = cx.new(|cx| ProcessesPage::new(&HostContext::local(cx), window, cx));
            *out.borrow_mut() = Some(page.clone());
            Host {
                page,
                opened: Rc::new(Cell::new(0)),
            }
        });
        vcx.update(|window, _| window.activate_window());
        redraw(vcx);
        let page = slot.borrow_mut().take().expect("page created");
        vcx.update(|_, cx| {
            use tl_ui_table::table::tree_table::TreeTableDelegate as _;
            page.read(cx).table().read(cx).delegate.row_count()
        })
    }

    /// pdu.55: a view the hub held before the page existed must not panic the first draw.
    #[gpui_kit::test]
    fn view_before_page_draws_without_panic(cx: &mut TestAppContext) {
        // Built for no query of this page: ignored, so nothing is drawn.
        assert_eq!(open_after_view(cx, 0), 0);
    }

    #[gpui_kit::test]
    fn view_before_page_for_first_generation_shows_rows(cx: &mut TestAppContext) {
        // The page's first query is generation 1, so a view for it may show.
        let (rows, _, _) = every_kind();
        let shown = open_after_view(cx, 1);
        assert!(shown == 0 || shown == rows.len(), "{shown} rows");
    }

    #[gpui_kit::test]
    fn first_query_sent_on_create(cx: &mut TestAppContext) {
        let o = open(cx);
        let queries = sent(&o.engine);
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].generation, 1);
        assert_eq!(queries[0].mode, TableMode::Grouped);
    }

    #[gpui_kit::test]
    fn tree_table_renders_visible_range_only_2000(cx: &mut TestAppContext) {
        let mut o = open(cx);
        publish(&o.engine, rows_2000(2), 1);
        redraw(o.vcx);
        let page = o.page.clone();
        let count = o.vcx.update(|_, cx| {
            page.read(cx)
                .table()
                .read(cx)
                .delegate
                .query()
                .shown()
                .map(|v| v.rows.len())
        });
        assert_eq!(count, Some(2000));
        let built = rows_built(&mut o);
        assert!(built > 0 && built <= MAX_ROWS_720, "{built} rows built");
    }

    #[gpui_kit::test]
    fn notifies_only_on_drawn_components(cx: &mut TestAppContext) {
        let mut o = open(cx);
        let (rows, groups, records) = every_kind();
        publish(
            &o.engine,
            snapshot(2, rows.clone(), groups.clone(), records.clone()),
            1,
        );
        redraw(o.vcx);
        let before = render_count(&mut o);

        o.engine
            .publish_with(|s| Arc::make_mut(&mut s.series).header.version += 1);
        redraw(o.vcx);
        assert_eq!(
            render_count(&mut o),
            before,
            "a SERIES change notified the table"
        );

        // A view for an older generation is ignored too.
        publish(
            &o.engine,
            snapshot(3, rows.clone(), groups.clone(), records.clone()),
            0,
        );
        redraw(o.vcx);
        assert_eq!(
            render_count(&mut o),
            before,
            "a stale view notified the table"
        );

        publish(&o.engine, snapshot(4, rows, groups, records), 1);
        redraw(o.vcx);
        assert_eq!(render_count(&mut o), before + 1);
    }

    #[gpui_kit::test]
    fn no_skeleton_on_processes_tab(cx: &mut TestAppContext) {
        let mut o = open(cx);
        assert_eq!(rows_built(&mut o), 0);
        let page = o.page.clone();
        let cpu = o.vcx.update(|_, cx| {
            let table = page.read(cx).table().read(cx);
            table
                .header_state()
                .cells
                .iter()
                .find(|c| c.id == ColumnId::Cpu)
                .and_then(|c| c.total.clone())
        });
        let (_, validity) = cpu.expect("CPU has a line 2 total");
        assert_eq!(validity, Validity::WarmingUp);
        assert!(o.vcx.debug_bounds("TlTreeTableHeader-total-cpu").is_some());
        assert!(
            o.vcx
                .debug_bounds("TlValidityGlyph-text-total-cpu")
                .is_some(),
            "… drawn"
        );
        assert!(o.vcx.debug_bounds("TlTreeTable-empty").is_none());
        assert!(o.vcx.debug_bounds("TlNameCell-0-name").is_none());
    }

    #[gpui_kit::test]
    fn collapse_all_action_sends_query(cx: &mut TestAppContext) {
        let o = open(cx);
        let page = o.page.clone();
        o.vcx.update(|_, cx| {
            page.read(cx).table().clone().update(cx, |t, _| {
                let q = t.delegate.query_mut();
                q.toggle(firefox());
                q.toggle(NodeId::Process(key(10)));
            })
        });
        let before = sent(&o.engine).len();
        o.vcx.dispatch_action(CollapseAll);
        redraw(o.vcx);
        let queries = sent(&o.engine);
        assert_eq!(queries.len(), before + 1);
        let last = queries.last().expect("query");
        let mut expanded: Vec<NodeId> = last.expanded.to_vec();
        expanded.sort_by_key(|n| format!("{n:?}"));
        let mut headers: Vec<NodeId> = GroupHeader::ALL.iter().map(|h| h.node()).collect();
        headers.sort_by_key(|n| format!("{n:?}"));
        assert_eq!(expanded, headers);
    }

    #[gpui_kit::test]
    fn scroll_handle_drives_table(cx: &mut TestAppContext) {
        let o = open(cx);
        publish(&o.engine, rows_2000(2), 1);
        redraw(o.vcx);
        let page = o.page.clone();
        let handle = o.vcx.update(|_, cx| page.read(cx).scroll_handle(cx));
        handle.scroll_to_item_strict(1000, ScrollStrategy::Top);
        o.vcx.update(|window, _| window.refresh());
        redraw(o.vcx);
        let first = o
            .vcx
            .update(|_, cx| page.read(cx).table().read(cx).first_visible_row())
            .expect("rows");
        assert!(
            first.1.abs_diff(1000) <= MAX_ROWS_720 as usize,
            "first row {}",
            first.1
        );
    }

    /// 500 flat rows, pids 100..600, in ascending or descending order.
    fn rows_500(version: u64, descending: bool) -> Arc<Snapshot> {
        let mut pids: Vec<u32> = (100..600).collect();
        if descending {
            pids.reverse();
        }
        let records: Vec<ProcessRecord> = pids
            .iter()
            .map(|&p| record(p, &format!("proc-{p}"), ProcFlags::empty()))
            .collect();
        let rows: Vec<TableRow> = pids
            .iter()
            .enumerate()
            .map(|(i, &p)| row(NodeId::Process(key(p)), 0, Some(i as u32), None))
            .collect();
        snapshot(version, rows, Vec::new(), records)
    }

    fn first_row(o: &mut Opened<'_>) -> usize {
        let page = o.page.clone();
        o.vcx
            .update(|_, cx| page.read(cx).table().read(cx).first_visible_row())
            .expect("rows")
            .1
    }

    /// pdu.64: a table at the top stays at the top when a new view reorders the rows. The old
    /// anchor followed the former top row to the far end of the list.
    #[gpui_kit::test]
    fn opens_at_top_and_stays_after_reorder(cx: &mut TestAppContext) {
        let mut o = open(cx);
        publish(&o.engine, rows_500(2, false), 1);
        redraw(o.vcx);
        assert_eq!(first_row(&mut o), 0);
        publish(&o.engine, rows_500(3, true), 1);
        redraw(o.vcx);
        redraw(o.vcx);
        assert_eq!(
            first_row(&mut o),
            0,
            "reordered view opens at its first row"
        );
    }

    /// pdu.64: a filter change scrolls the filtered list to its first match.
    #[gpui_kit::test]
    fn filter_change_scrolls_to_top(cx: &mut TestAppContext) {
        let mut o = open(cx);
        publish(&o.engine, rows_500(2, false), 1);
        redraw(o.vcx);
        let page = o.page.clone();
        let handle = o.vcx.update(|_, cx| page.read(cx).scroll_handle(cx));
        handle.scroll_to_item_strict(400, ScrollStrategy::Top);
        o.vcx.update(|window, _| window.refresh());
        redraw(o.vcx);
        assert!(first_row(&mut o) > 300, "scrolled down first");
        o.vcx.update(|_, cx| {
            let events = cx.global::<GlobalShellEvents>().0.clone();
            events.update(cx, |_, cx| {
                cx.emit(FilterChanged {
                    tab: Tab::Processes,
                    text: "proc-1".into(),
                });
            });
        });
        redraw(o.vcx);
        let generation = sent(&o.engine).last().expect("query sent").generation;
        publish(&o.engine, rows_500(3, false), generation);
        redraw(o.vcx);
        redraw(o.vcx);
        assert_eq!(first_row(&mut o), 0, "first match is visible");
    }

    /// Scrolling frames over a 2,000-row view call `tr` zero times (rendering budget).
    #[gpui_kit::test]
    fn scroll_frames_translate_nothing(cx: &mut TestAppContext) {
        let o = open(cx);
        publish(&o.engine, rows_2000(2), 1);
        redraw(o.vcx);
        let page = o.page.clone();
        let handle = o.vcx.update(|_, cx| page.read(cx).scroll_handle(cx));
        let before = tl_ui_base::i18n::tr_calls();
        for item in [200, 400, 600] {
            handle.scroll_to_item_strict(item, ScrollStrategy::Top);
            o.vcx.update(|window, _| window.refresh());
            redraw(o.vcx);
        }
        assert_eq!(tl_ui_base::i18n::tr_calls(), before, "a frame called tr");
    }

    fn row_at(ix: usize) -> Point<gpui_kit::Pixels> {
        point(px(400.), px(HEADER_H_TWO + ix as f32 * 28. + 14.))
    }

    fn double_click(vcx: &mut VisualTestContext, at: Point<gpui_kit::Pixels>) {
        vcx.simulate_click(at, Modifiers::none());
        vcx.simulate_event(MouseDownEvent {
            position: at,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            position: at,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 2,
        });
        redraw(vcx);
    }

    /// pdu.67: a re-sort while a row is pressed, including the gap between the two presses of a
    /// double-click, keeps the pressed row under the pointer and selected.
    #[gpui_kit::test]
    fn resort_waits_for_press_release(cx: &mut TestAppContext) {
        use std::time::Duration;
        let o = open(cx);
        publish(&o.engine, rows_500(2, false), 1);
        redraw(o.vcx);
        let page = o.page.clone();
        let top = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| {
                let t = page.read(cx).table().read(cx);
                tl_ui_table::table::tree_table::TreeTableDelegate::row_id(&t.delegate, 0)
            })
        };
        let selected = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| {
                page.read(cx)
                    .table()
                    .read(cx)
                    .selection
                    .ids()
                    .collect::<Vec<_>>()
            })
        };
        let pressed = top(o.vcx);
        let down = |vcx: &mut VisualTestContext, click_count| {
            vcx.simulate_event(MouseDownEvent {
                position: row_at(0),
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count,
                first_mouse: false,
            });
        };
        let up = |vcx: &mut VisualTestContext, click_count| {
            vcx.simulate_event(MouseUpEvent {
                position: row_at(0),
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count,
            });
        };
        down(o.vcx, 1);
        up(o.vcx, 1);
        // The re-sort lands between the two presses.
        publish(&o.engine, rows_500(3, true), 1);
        redraw(o.vcx);
        assert_eq!(
            top(o.vcx),
            pressed,
            "the order holds after the first release"
        );
        down(o.vcx, 2);
        assert_eq!(
            selected(o.vcx),
            vec![pressed],
            "the second press hit the pressed row"
        );
        up(o.vcx, 2);
        o.vcx.executor().advance_clock(Duration::from_millis(100));
        redraw(o.vcx);
        assert_eq!(top(o.vcx), pressed, "still held shortly after the release");
        o.vcx.executor().advance_clock(Duration::from_millis(600));
        redraw(o.vcx);
        assert_ne!(
            top(o.vcx),
            pressed,
            "the held view is installed after the double-click window"
        );
    }

    /// pdu.67: a release that never arrives does not freeze the table.
    #[gpui_kit::test]
    fn lost_release_ends_the_hold(cx: &mut TestAppContext) {
        let o = open(cx);
        publish(&o.engine, rows_500(2, false), 1);
        redraw(o.vcx);
        let page = o.page.clone();
        let top = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| {
                let t = page.read(cx).table().read(cx);
                tl_ui_table::table::tree_table::TreeTableDelegate::row_id(&t.delegate, 0)
            })
        };
        let pressed = top(o.vcx);
        o.vcx.simulate_event(MouseDownEvent {
            position: row_at(0),
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        publish(&o.engine, rows_500(3, true), 1);
        redraw(o.vcx);
        assert_eq!(top(o.vcx), pressed, "held while the button is down");
        o.vcx
            .executor()
            .advance_clock(std::time::Duration::from_secs(4));
        redraw(o.vcx);
        assert_ne!(top(o.vcx), pressed, "the hold timed out");
    }

    #[gpui_kit::test]
    fn double_click_toggles_expand_not_properties(cx: &mut TestAppContext) {
        let o = open(cx);
        let (rows, groups, records) = every_kind();
        publish(&o.engine, snapshot(2, rows, groups, records), 1);
        redraw(o.vcx);
        let page = o.page.clone();

        // Row 1 is the Firefox app row, which has children.
        let before = sent(&o.engine).len();
        double_click(o.vcx, row_at(1));
        assert_eq!(sent(&o.engine).len(), before + 1);
        let open = o.vcx.update(|_, cx| {
            page.read(cx)
                .table()
                .read(cx)
                .delegate
                .query()
                .is_expanded(&firefox())
        });
        assert!(open, "the app row expanded");
        assert_eq!(o.opened.get(), 0, "OpenProperties dispatched");

        // Row 4 is a kernel thread leaf.
        double_click(o.vcx, row_at(4));
        assert_eq!(sent(&o.engine).len(), before + 1);
        assert_eq!(o.opened.get(), 0);
    }
}

/// The ProcessTable keys for End Task and End Process (PROC-049, pdu.29).
mod keys {
    use std::sync::Arc;

    use crate::common::{open_stand_in, shell_free_init_with, stand_in_of};
    use gpui_kit::{Action, App, Entity, KeyContext, Keystroke};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use tl_model::actions::{ActionKind, ActionTarget, Signal};
    use tl_model::client::EngineClient;
    use tl_model::command::Command;
    use tl_model::ids::NodeId;
    use tl_model::process::ProcFlags;
    use tl_model::snapshot::Snapshot;
    use tl_ui_base::app_env::AppEnv;
    use tl_ui_base::i18n::init_strings;
    use tl_ui_procs::pages::processes::actions::{CopyDetails, EndProcess, EndTask};
    use tl_ui_procs::pages::processes::delegate::ProcRowId;
    use tl_ui_procs::pages::processes::{ProcessesPage, bind_keys_processes};
    use tl_ui_settings::settings::{Settings, UiState};
    use tl_ui_testkit::fake_engine::FakeEngine;

    use super::delegate::{key, record, row, snapshot};

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
    }

    /// The real app on the Processes tab, with its action tracker (the End handlers send
    /// through it).
    fn open(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
        let (engine, host) =
            shell_free_init_with(cx, Settings::default(), UiState::default(), AppEnv::test());
        cx.update(|cx| {
            init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let (_, vcx) = open_stand_in(cx, &host);
        (engine, vcx)
    }

    /// The main window with PIDs 300 and 301 (own, unprotected) and PID 300 selected.
    fn rig(cx: &mut TestAppContext) -> Rig {
        let (engine, mut vcx) = open(cx);
        vcx.update(|_, cx| bind_keys_processes(cx));
        let page = vcx.update(|_, cx| stand_in_of(cx).page);
        let mut r = Rig { engine, vcx, page };
        let pids = [300, 301];
        let records = pids
            .iter()
            .map(|&p| record(p, "proc", ProcFlags::empty()))
            .collect();
        let rows = pids
            .iter()
            .enumerate()
            .map(|(i, &p)| row(NodeId::Process(key(p)), 0, Some(i as u32), None))
            .collect();
        let page = r.page.clone();
        let generation = r
            .vcx
            .update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation());
        let mut s = Snapshot::clone(&snapshot(2, rows, Vec::new(), records));
        s.sequence = r.engine.latest().sequence + 1;
        Arc::make_mut(&mut s.table).query_generation = generation;
        r.engine.publish(s);
        r.draw();
        r.vcx.update(|window, cx| {
            let table = page.read(cx).table().clone();
            table.update(cx, |t, cx| {
                t.selection.click(ProcRowId::Process(key(300)));
                cx.notify();
            });
            page.update(cx, |p, cx| p.focus_table(window, cx));
        });
        r.draw();
        r
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn press(&mut self, keys: &str) {
            self.vcx.simulate_keystrokes(keys);
            self.draw();
        }

        /// The action requests sent: kind and target PIDs.
        fn sent(&self) -> Vec<(ActionKind, Vec<u32>)> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::Action(r) => Some(r),
                    _ => None,
                })
                .map(|r| {
                    let pids = r
                        .targets
                        .iter()
                        .filter_map(|t| match t {
                            ActionTarget::Process(k) => Some(k.pid),
                            _ => None,
                        })
                        .collect();
                    (r.kind, pids)
                })
                .collect()
        }
    }

    #[gpui_kit::test]
    fn delete_dispatches_end_task_with_selection(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.press("delete");
        assert_eq!(r.sent(), [(ActionKind::Signal(Signal::Term), vec![300])]);
        r.press("ctrl-e");
        assert_eq!(r.sent().len(), 2, "Ctrl+E runs End Task too");
        assert_eq!(r.sent()[1].0, ActionKind::Signal(Signal::Term));
    }

    #[gpui_kit::test]
    fn ctrl_x_dispatches_end_process(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.press("ctrl-x");
        assert_eq!(r.sent(), [(ActionKind::Signal(Signal::Kill), vec![300])]);
        r.press("shift-delete");
        assert_eq!(r.sent().len(), 2, "Shift+Delete runs End Process too");
        assert_eq!(r.sent()[1].0, ActionKind::Signal(Signal::Kill));
    }

    /// The Action `keys` resolves to with `contexts` as the stack (outermost first).
    fn resolve(cx: &App, keys: &str, contexts: &[&str]) -> Option<Box<dyn Action>> {
        let input = [Keystroke::parse(keys).expect("a keystroke")];
        let stack: Vec<KeyContext> = contexts
            .iter()
            .map(|c| KeyContext::parse(c).expect("key context"))
            .collect();
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let (bindings, _) = keymap.bindings_for_input(&input, &stack);
        bindings.first().map(|b| b.action().boxed_clone())
    }

    #[gpui_kit::test]
    fn init_binds_processes_keys(cx: &mut TestAppContext) {
        // `open` runs `TasklensUi::init` and never calls `bind_keys_processes` itself.
        let (_engine, mut vcx) = open(cx);
        vcx.update(|_, cx| {
            for keys in ["delete", "ctrl-e"] {
                let got = resolve(cx, keys, &["ProcessTable", "Table"]);
                assert!(
                    got.as_ref().is_some_and(|a| a.partial_eq(&EndTask)),
                    "{keys}: expected EndTask, got {got:?}"
                );
            }
            // The table binds Ctrl+C to CopyRow; the process table overrides it.
            let got = resolve(cx, "ctrl-c", &["ProcessTable", "Table"]);
            assert!(
                got.as_ref().is_some_and(|a| a.partial_eq(&CopyDetails)),
                "ctrl-c: expected CopyDetails, got {got:?}"
            );
        });
    }

    #[gpui_kit::test]
    fn keymap_rows_match_table_b_processtable(cx: &mut TestAppContext) {
        cx.update(bind_keys_processes);
        let rows: [(&str, &dyn Action); 5] = [
            ("ctrl-c", &CopyDetails),
            ("delete", &EndTask),
            ("ctrl-e", &EndTask),
            ("shift-delete", &EndProcess),
            ("ctrl-x", &EndProcess),
        ];
        cx.update(|cx| {
            for (keys, expected) in rows {
                let got = resolve(cx, keys, &["ProcessTable", "Table"]);
                assert!(
                    got.as_ref().is_some_and(|a| a.partial_eq(expected)),
                    "{keys}: expected {expected:?}, got {got:?}"
                );
                assert!(
                    resolve(cx, keys, &["Table"]).is_none(),
                    "{keys} is bound only in ProcessTable"
                );
            }
        });
    }
}
