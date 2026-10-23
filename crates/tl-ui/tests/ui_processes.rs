//! Process table query state (tasklens-pdu.11). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::TestAppContext;
use tl_model::client::EngineClient;
use tl_model::command::Command;
use tl_model::ids::{ColumnId, NodeId, ProcessKey};
use tl_model::process::GroupKind;
use tl_model::settings::{Settings, UiState};
use tl_model::table::{ProcessTableView, SortSpec, TableMode, TableQuery};
use tl_model::time::BootId;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::pages::processes::cells::ProcColumnKind;
use tl_ui::pages::processes::table::{GroupHeader, ProcessQuery};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

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
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, Snapshot};
    use tl_model::table::{Manager, ProcessTableView, TableRow, UnitRef};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::app::EngineGlobal;
    use tl_ui::demand::{DemandAggregator, GlobalDemand};
    use tl_ui::hub::SnapshotHub;
    use tl_ui::i18n::{init_strings, tr_calls};
    use tl_ui::pages::processes::cells::{NO_SOURCE, PROCESS_COLUMNS, process_column};
    use tl_ui::pages::processes::delegate::{ProcRowId, ProcessDelegate, SHIPPED_PHASE, UnitIds};
    use tl_ui::pages::processes::table::GroupHeader;
    use tl_ui::table::actions::{OpenProperties, ResizeColumnsToFit};
    use tl_ui::table::cells::CellOut;
    use tl_ui::table::columns::TableId;
    use tl_ui::table::tree_table::{IconSlot, RowKind, TlTreeTable, TreeTableDelegate};
    use tl_ui::testing::fake_engine::FakeEngine;

    pub(super) fn setup(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::table::actions::bind_keys_table(cx);
            tl_ui::theme::store::init(
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
            ProcessDelegate::new(snap, &Settings::default(), &UiState::default(), window, cx)
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
        use tl_ui::table::cells::{record_painted_texts, take_painted_texts};

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
            let d =
                ProcessDelegate::new(snap, &Settings::default(), &UiState::default(), window, cx);
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
            vcx.debug_bounds("TlCell-glyph").is_some(),
            "the warming-up GPU cell draws its glyph"
        );
    }

    /// A Network cell with per-process rates paints the rate, "0 Mbps" for a process with none,
    /// and no number while the attribution warms up (tasklens-pdu.66).
    #[gpui_kit::test]
    fn network_cell_draws_value(cx: &mut TestAppContext) {
        use gpui_kit::base::Root;
        use tl_model::validity::ProcessField;
        use tl_ui::table::cells::{record_painted_texts, take_painted_texts};

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
            let d =
                ProcessDelegate::new(snap, &Settings::default(), &UiState::default(), window, cx);
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
            vcx.debug_bounds("TlCell-glyph").is_some(),
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
                let m = *tl_ui::theme::metrics(cx);
                let w = tl_ui::text::metrics::TextMetricsCache::default().template_width(
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
            ColumnId::IoPriority,
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
            let d =
                ProcessDelegate::new(snap, &Settings::default(), &UiState::default(), window, cx);
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
            let m = *tl_ui::theme::metrics(cx);
            (m.text.base, m.sp[6], m.ui_scale)
        });
        let advance = |vcx: &mut VisualTestContext, text: &str| {
            vcx.update(|window, cx| {
                let mut cache = tl_ui::text::metrics::TextMetricsCache::default();
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
    use tl_ui::app::EngineGlobal;
    use tl_ui::hub::{GlobalHub, SnapshotHub};
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::table::GroupHeader;
    use tl_ui::table::actions::{CollapseAll, OpenProperties};
    use tl_ui::table::tree_table::HEADER_H_TWO;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{FilterChanged, GlobalShellEvents, ShellEvents};

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
            let page = cx.new(|cx| ProcessesPage::new(window, cx));
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
            let page = cx.new(|cx| ProcessesPage::new(window, cx));
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
            use tl_ui::table::tree_table::TreeTableDelegate as _;
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
            o.vcx.debug_bounds("TlValidityGlyph-text").is_some(),
            "… drawn"
        );
        assert!(o.vcx.debug_bounds("TlTreeTable-empty").is_none());
        assert!(o.vcx.debug_bounds("TlNameCell").is_none());
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
        let before = tl_ui::i18n::tr_calls();
        for item in [200, 400, 600] {
            handle.scroll_to_item_strict(item, ScrollStrategy::Top);
            o.vcx.update(|window, _| window.refresh());
            redraw(o.vcx);
        }
        assert_eq!(tl_ui::i18n::tr_calls(), before, "a frame called tr");
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

/// The Processes arm of the main window's page match (tasklens-pdu.13).
mod shell {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
        Render, Styled as _, TestAppContext, VisualTestContext, Window, div,
    };
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_ui::actions::SelectTab;
    use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
    use tl_ui::components::toast;
    use tl_ui::demand::{DemandAggregator, GlobalDemand};
    use tl_ui::hub::{GlobalHub, SnapshotHub};
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::table::tree_table::TreeTableDelegate as _;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{
        self as shell, MainWindow, PageSlot, ShellPage, main_window_view, open_main_window,
        set_test_page,
    };
    use tl_ui::window::status::StatusChanged;

    use super::delegate::{every_kind, snapshot};

    /// The Performance tab's stand-in, so there is a second tab to switch to.
    struct OtherPage {
        focus: FocusHandle,
    }

    impl gpui_kit::EventEmitter<StatusChanged> for OtherPage {}

    impl Render for OtherPage {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().track_focus(&self.focus).size_full()
        }
    }

    impl ShellPage for OtherPage {
        fn primary_focus(&self) -> FocusHandle {
            self.focus.clone()
        }
    }

    pub(super) fn other(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
        let page = cx.new(|cx| OtherPage {
            focus: cx.focus_handle().tab_stop(true),
        });
        PageSlot::new(page, cx)
    }

    fn open(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
        open_with(cx, UiState::default())
    }

    /// Opens the real main window with `state` as the saved UI state.
    pub(super) fn open_with(
        cx: &mut TestAppContext,
        state: UiState,
    ) -> (Arc<FakeEngine>, VisualTestContext) {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            let appearance = AppearanceSettings::default();
            tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
            tl_ui::i18n::init_strings(cx, "en-US");
            tl_ui::table::actions::bind_keys_table(cx);
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state,
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            cx.set_global(EngineGlobal(client.clone()));
            tl_ui::action_tracker::install(client.clone(), cx);
            let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalHub(hub));
            cx.set_global(GlobalDemand(demand));
            toast::init(cx);
            tl_ui::actions::bind_keys_global(cx);
            cx.set_global(StartOptions::default());
            set_test_page(cx, Tab::Performance, other);
            shell::init(cx);
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        (engine, vcx)
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn processes(vcx: &mut VisualTestContext) -> Option<Entity<ProcessesPage>> {
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        vcx.read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
    }

    pub(super) fn select(vcx: &mut VisualTestContext, tab: Tab) {
        vcx.dispatch_action(SelectTab(tab as usize));
        draw(vcx);
    }

    fn table_renders(vcx: &mut VisualTestContext, page: &Entity<ProcessesPage>) -> u32 {
        vcx.update(|_, cx| page.read(cx).table().read(cx).render_count.get())
    }

    #[gpui_kit::test]
    fn processes_page_created_on_first_visit_and_kept(cx: &mut TestAppContext) {
        let (_, mut vcx) = open(cx);
        let page = processes(&mut vcx).expect("Processes built on the first visit");
        assert!(table_renders(&mut vcx, &page) > 0, "the table is drawn");
        assert!(vcx.debug_bounds("ProcessesPage").is_some());
        assert!(vcx.debug_bounds("TlTreeTable").is_some());

        select(&mut vcx, Tab::Performance);
        assert!(vcx.debug_bounds("ProcessesPage").is_none());
        select(&mut vcx, Tab::Processes);
        let again = processes(&mut vcx).expect("still built");
        assert_eq!(again.entity_id(), page.entity_id());
        assert!(vcx.debug_bounds("ProcessesPage").is_some());
    }

    #[gpui_kit::test]
    fn inactive_page_not_rendered(cx: &mut TestAppContext) {
        let (engine, mut vcx) = open(cx);
        let page = processes(&mut vcx).expect("Processes built");
        select(&mut vcx, Tab::Performance);
        let before = table_renders(&mut vcx, &page);

        // A view for the current query: the active page would redraw the table.
        let (rows, groups, records) = every_kind();
        let mut s = tl_model::snapshot::Snapshot::clone(&snapshot(2, rows, groups, records));
        s.sequence = engine.latest().sequence + 1;
        Arc::make_mut(&mut s.table).query_generation = 1;
        engine.publish(s);
        draw(&mut vcx);
        assert_eq!(table_renders(&mut vcx, &page), before);
        assert!(vcx.debug_bounds("ProcessesPage").is_none());
        let rows = vcx.update(|_, cx| page.read(cx).table().read(cx).delegate.row_count());
        assert_eq!(rows, 8, "the view was installed while hidden");
    }
}

/// The Processes toolbar buttons (tasklens-pdu.25, UI-PRC-019, UI-SHL-012, UI-SHL-013) and their
/// registration in the main window (tasklens-pdu.26).
mod toolbar {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{
        App, Bounds, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, Modifiers,
        ParentElement as _, Pixels, Render, Styled as _, TestAppContext, VisualTestContext, Window,
        div, px,
    };
    use tl_model::command::Tab;
    use tl_model::health::HelperState;
    use tl_model::settings::{AppearanceSettings, ThemeSettings, UiState};
    use tl_model::table::TableMode;
    use tl_ui::actions::{CycleViewMode, ToggleHideSystem};
    use tl_ui::components::icons::TlIcon;
    use tl_ui::i18n::tr_calls;
    use tl_ui::pages::processes::toolbar::{
        HIDE_SYSTEM_ID, ProcessesButtons, ProcessesToolbar, VIEW_MODE_ID, toolbar_words,
    };
    use tl_ui::window::toolbar::{Toolbar, ToolbarContent, ToolbarModel};

    use super::shell::{open_with, select};

    fn model(tab: Tab) -> ToolbarModel {
        ToolbarModel {
            tab,
            history_mode: false,
            hide_system: false,
            mode: TableMode::Grouped,
            helper: HelperState::Missing,
            helper_installed: false,
            picker_available: false,
            picking: false,
            window_active: true,
        }
    }

    /// A bare root that draws only the Toolbar with the Processes buttons, so their layout is
    /// measured without the rest of the title bar.
    struct Host {
        focus: FocusHandle,
        model: ToolbarModel,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let page = ProcessesToolbar.buttons(&self.model, window, cx);
            div()
                .track_focus(&self.focus)
                .size(px(600.))
                .flex()
                .items_start()
                .child(Toolbar::new(self.model.clone(), page))
        }
    }

    fn rig(cx: &mut TestAppContext, model: ToolbarModel) -> (Entity<Host>, &mut VisualTestContext) {
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
        let (view, cx) = cx.add_window_view(move |_, cx| Host {
            focus: cx.focus_handle(),
            model,
        });
        cx.update(|window, cx| {
            window.activate_window();
            view.read(cx).focus.clone().focus(window, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (view, cx)
    }

    /// The real main window with `state`, counting the two Actions as they reach the app. The
    /// window has no handler of its own for them, so each click arrives here.
    struct Shell {
        vcx: VisualTestContext,
        hide: Rc<Cell<usize>>,
        cycle: Rc<Cell<usize>>,
    }

    fn shell(cx: &mut TestAppContext, state: UiState) -> Shell {
        let (_, mut vcx) = open_with(cx, state);
        let hide = Rc::new(Cell::new(0));
        let cycle = Rc::new(Cell::new(0));
        vcx.update(|_, cx| {
            let (h, c) = (hide.clone(), cycle.clone());
            cx.on_action(move |_: &ToggleHideSystem, _| h.set(h.get() + 1));
            cx.on_action(move |_: &CycleViewMode, _| c.set(c.get() + 1));
        });
        Shell { vcx, hide, cycle }
    }

    fn click(vcx: &mut VisualTestContext, selector: &'static str) {
        let at = button(vcx, selector).center();
        vcx.simulate_click(at, Modifiers::none());
        vcx.run_until_parked();
    }

    /// The debug selectors of the two buttons, `TlButton-<id>`.
    const HIDE: &str = "TlButton-processes-hide-system";
    const CYCLER: &str = "TlButton-processes-view-mode";

    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Option<Bounds<Pixels>> {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds(selector)
    }

    fn button(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        bounds(cx, selector).unwrap_or_else(|| panic!("{selector} drawn"))
    }

    fn buttons(cx: &mut App, model: &ToolbarModel) -> Option<ProcessesButtons> {
        let words = toolbar_words(cx);
        ProcessesButtons::for_model(model, &words)
    }

    #[gpui_kit::test]
    fn toolbar_buttons_only_on_processes_tab(cx: &mut TestAppContext) {
        assert_eq!(HIDE, format!("TlButton-{HIDE_SYSTEM_ID}"));
        assert_eq!(CYCLER, format!("TlButton-{VIEW_MODE_ID}"));
        let (view, cx) = rig(cx, model(Tab::Processes));
        let hide = button(cx, HIDE);
        let cycler = button(cx, CYCLER);
        for b in [hide, cycler] {
            assert_eq!((b.size.width, b.size.height), (px(24.), px(24.)));
        }
        assert!(
            hide.origin.x < cycler.origin.x,
            "toggle first, cycler second"
        );

        let b = cx
            .update(|_, cx| buttons(cx, &model(Tab::Processes)))
            .unwrap();
        assert!(!b.hide_on, "hide_system off draws the toggle off");
        assert_eq!(b.hide_tooltip.as_ref(), "Hide system processes");
        let on = ToolbarModel {
            hide_system: true,
            ..model(Tab::Processes)
        };
        let b = cx.update(|_, cx| buttons(cx, &on)).unwrap();
        assert!(b.hide_on);
        assert_eq!(b.hide_tooltip.as_ref(), "Show system processes");

        for tab in Tab::ALL.into_iter().filter(|t| *t != Tab::Processes) {
            let m = model(tab);
            let empty = cx.update(|window, cx| {
                buttons(cx, &m).is_none() && ProcessesToolbar.buttons(&m, window, cx).is_empty()
            });
            assert!(empty, "{tab:?}: no buttons");
        }

        cx.update(|_, cx| {
            view.update(cx, |host, cx| {
                host.model = model(Tab::Performance);
                cx.notify();
            });
        });
        cx.run_until_parked();
        assert!(bounds(cx, HIDE).is_none());
        assert!(bounds(cx, CYCLER).is_none());
        let toolbar = bounds(cx, "Toolbar").expect("Toolbar drawn");
        assert_eq!(toolbar.size.width, px(0.), "no space reserved");
    }

    #[gpui_kit::test]
    fn toolbar_registered_for_processes_tab(cx: &mut TestAppContext) {
        let mut s = shell(cx, UiState::default());
        let vcx = &mut s.vcx;
        // Processes is the start tab.
        let hide = button(vcx, HIDE);
        let cycler = button(vcx, CYCLER);
        assert!(hide.origin.x < cycler.origin.x, "slot 2, then slot 3");

        for tab in Tab::ALL.into_iter().filter(|t| *t != Tab::Processes) {
            select(vcx, tab);
            assert!(bounds(vcx, HIDE).is_none(), "{tab:?}: no toggle");
            assert!(bounds(vcx, CYCLER).is_none(), "{tab:?}: no cycler");
        }

        select(vcx, Tab::Processes);
        assert_eq!(button(vcx, HIDE), hide);
        assert_eq!(button(vcx, CYCLER), cycler);
    }

    #[gpui_kit::test]
    fn cycler_icon_and_tooltip_per_mode(cx: &mut TestAppContext) {
        let (_, cx) = rig(cx, model(Tab::Processes));
        let cases = [
            (
                TableMode::Grouped,
                TlIcon::Rows2,
                "Toggle view mode (current: Task Manager Grouping)",
            ),
            (
                TableMode::Tree,
                TlIcon::ListTree,
                "Toggle view mode (current: Tree View)",
            ),
            (
                TableMode::Flat,
                TlIcon::List,
                "Toggle view mode (current: List View)",
            ),
        ];
        cx.update(|_, cx| {
            // The words are cached for this strings version: later lookups translate nothing.
            toolbar_words(cx);
            let before = tr_calls();
            for (mode, icon, tooltip) in cases {
                let m = ToolbarModel {
                    mode,
                    ..model(Tab::Processes)
                };
                let b = buttons(cx, &m).unwrap();
                assert_eq!(b.cycler_icon, icon, "{mode:?}");
                assert_eq!(b.cycler_tooltip.as_ref(), tooltip, "{mode:?}");
                assert!(!b.cycler_disabled);
            }
            assert_eq!(tr_calls(), before, "no tr call per frame");
        });
    }

    #[gpui_kit::test]
    fn cycler_disabled_in_history(cx: &mut TestAppContext) {
        let mut state = UiState::default();
        state.processes.history_mode = true;
        let mut s = shell(cx, state);
        let history = ToolbarModel {
            history_mode: true,
            ..model(Tab::Processes)
        };
        let b = s.vcx.update(|_, cx| buttons(cx, &history)).unwrap();
        assert!(b.cycler_disabled);
        assert_eq!(b.cycler_tooltip.as_ref(), "History shows a flat list");
        assert_eq!(b.hide_tooltip.as_ref(), "Hide system processes");

        click(&mut s.vcx, CYCLER);
        assert_eq!(s.cycle.get(), 0, "the disabled cycler dispatches nothing");
        click(&mut s.vcx, HIDE);
        assert_eq!(s.hide.get(), 1, "the toggle stays enabled");
    }

    #[gpui_kit::test]
    fn toggle_dispatches_toggle_hide_system(cx: &mut TestAppContext) {
        let mut s = shell(cx, UiState::default());
        click(&mut s.vcx, HIDE);
        assert_eq!((s.hide.get(), s.cycle.get()), (1, 0));
        click(&mut s.vcx, CYCLER);
        assert_eq!((s.hide.get(), s.cycle.get()), (1, 1));
    }
}

/// The jump target: `ProcessesPage::select_by_key` and `select_by_pid` (tasklens-pdu.3,
/// PROC-071, UI-PRC-015).
mod jump {
    use std::sync::Arc;

    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
    use tl_model::client::EngineClient as _;
    use tl_model::command::Tab;
    use tl_model::ids::{NodeId, ProcessKey};
    use tl_model::process::{GroupKind, ProcFlags, ProcessRecord};
    use tl_model::settings::UiState;
    use tl_model::snapshot::Snapshot;
    use tl_model::table::{TableMode, TableQuery, TableRow};
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::actions::ProcessesShowHidden;
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::pages::processes::jump::JumpTexts;
    use tl_ui::pages::processes::table::GroupHeader;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{ShellPage, main_window_view};

    use super::delegate::{key, record, row, snapshot};
    use super::shell::{open_with, select};

    /// Fillers before the target, so centring has to scroll.
    const FILLERS: u32 = 300;
    const TARGET: u32 = 5000;
    const PARENT: u32 = 4000;
    const GRANDPARENT: u32 = 3000;

    fn firefox() -> tl_model::ids::GroupKey {
        GroupKind::App {
            app_id: Arc::from("firefox"),
        }
        .key()
    }

    /// Fillers pid 100.., then the Firefox chain grandparent > parent > target.
    fn records() -> Vec<ProcessRecord> {
        let mut out: Vec<ProcessRecord> = (0..FILLERS)
            .map(|i| record(100 + i, "filler", ProcFlags::empty()))
            .collect();
        for (pid, ppid) in [(GRANDPARENT, 1), (PARENT, GRANDPARENT), (TARGET, PARENT)] {
            let mut r = record(pid, "firefox", ProcFlags::empty());
            r.group = firefox();
            r.ppid = ppid;
            out.push(r);
        }
        out.sort_by_key(|r| r.key);
        out
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
        version: u64,
    }

    fn rig(cx: &mut TestAppContext, state: UiState) -> Rig {
        let (engine, mut vcx) = open_with(cx, state);
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
            .expect("Processes built");
        let mut r = Rig {
            engine,
            vcx,
            page,
            version: 1,
        };
        // The records the jump looks up, in a view of an old generation.
        r.publish(records(), &[], 0);
        r
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// A snapshot holding `records` whose view has `shown` (pids) as rows, for `generation`.
        fn publish(&mut self, records: Vec<ProcessRecord>, shown: &[u32], generation: u64) {
            let rows: Vec<TableRow> = shown
                .iter()
                .map(|&pid| {
                    let ix = records
                        .iter()
                        .position(|r| r.key.pid == pid)
                        .expect("shown pid is a record");
                    row(NodeId::Process(key(pid)), 0, Some(ix as u32), None)
                })
                .collect();
            self.version += 1;
            let mut s = Snapshot::clone(&snapshot(self.version, rows, Vec::new(), records));
            s.sequence = self.engine.latest().sequence + 1;
            Arc::make_mut(&mut s.table).query_generation = generation;
            Arc::make_mut(&mut s.processes).header.version = self.version;
            self.engine.publish(s);
            self.draw();
        }

        /// Every filler, then the target, as the view of the last query sent.
        fn answer_with_target(&mut self) {
            let mut shown: Vec<u32> = (0..FILLERS).map(|i| 100 + i).collect();
            shown.push(TARGET);
            let generation = self.last_query().generation;
            self.publish(records(), &shown, generation);
        }

        fn last_query(&self) -> TableQuery {
            super::sent(&self.engine).pop().expect("a query was sent")
        }

        fn jump(&mut self, key: ProcessKey, texts: JumpTexts) {
            let page = self.page.clone();
            self.vcx.update(|window, cx| {
                page.update(cx, |p, cx| p.select_by_key(key, texts, window, cx))
            });
            self.draw();
        }

        fn toasts(&mut self) -> Vec<String> {
            self.vcx.update(|_, cx| {
                let main = main_window_view(cx).expect("main window open");
                let texts = main.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(|t| t.to_string()).collect()
            })
        }

        fn active_tab(&mut self) -> Tab {
            self.vcx.update(|_, cx| {
                main_window_view(cx)
                    .expect("main window open")
                    .read(cx)
                    .active_tab()
            })
        }

        fn selected(&mut self) -> Vec<ProcRowId> {
            let page = self.page.clone();
            self.vcx
                .update(|_, cx| page.read(cx).table().read(cx).selection.ids().collect())
        }

        fn target_row_visible_after_scroll(&mut self) -> bool {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                let t = page.read(cx).table().read(cx);
                let range = t.visible_range();
                range.start > 0 && range.contains(&(FILLERS as usize))
            })
        }

        fn table_focused(&mut self) -> bool {
            let page = self.page.clone();
            self.vcx
                .update(|window, cx| page.read(cx).primary_focus().is_focused(window))
        }
    }

    #[gpui_kit::test]
    fn jump_expands_group_and_ancestors_then_centres(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        select(&mut r.vcx, Tab::Performance);
        let before = super::sent(&r.engine).len();
        r.jump(key(TARGET), JumpTexts::default());
        assert_eq!(r.active_tab(), Tab::Processes);
        let sent = super::sent(&r.engine);
        assert_eq!(sent.len(), before + 1, "one query");
        let expanded = &sent.last().unwrap().expanded;
        for node in [
            GroupHeader::Apps.node(),
            NodeId::Group(firefox()),
            NodeId::Process(key(PARENT)),
            NodeId::Process(key(GRANDPARENT)),
        ] {
            assert!(expanded.contains(&node), "{node:?} expanded");
        }
        assert!(r.toasts().is_empty(), "no toast before the view");

        r.answer_with_target();
        assert_eq!(r.selected(), [ProcRowId::Process(key(TARGET))]);
        assert!(r.target_row_visible_after_scroll(), "centred");
        assert!(r.table_focused());
        assert_eq!(r.toasts(), ["Switched to Processes tab"]);
    }

    #[gpui_kit::test]
    fn jump_tree_mode_expands_ancestors(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.processes.mode = TableMode::Tree);
        });
        r.draw();
        let before = super::sent(&r.engine).len();
        r.jump(key(TARGET), JumpTexts::default());
        let sent = super::sent(&r.engine);
        assert_eq!(sent.len(), before + 1, "one query");
        let q = sent.last().unwrap();
        assert_eq!(q.mode, TableMode::Tree);
        assert!(q.expanded.contains(&NodeId::Process(key(PARENT))));
        assert!(q.expanded.contains(&NodeId::Process(key(GRANDPARENT))));
        r.answer_with_target();
        assert_eq!(r.selected(), [ProcRowId::Process(key(TARGET))]);
        assert!(r.toasts().is_empty(), "same tab: no toast");
    }

    #[gpui_kit::test]
    fn jump_caller_text_replaces_switched_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        select(&mut r.vcx, Tab::Performance);
        r.jump(
            key(TARGET),
            JumpTexts {
                success: Some("Switched to owning process.".into()),
                ..JumpTexts::default()
            },
        );
        r.answer_with_target();
        assert_eq!(r.toasts(), ["Switched to owning process."]);
    }

    #[gpui_kit::test]
    fn jump_missing_toasts_not_found(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        let mut recs = records();
        let ix = recs.iter().position(|x| x.key.pid == TARGET).unwrap();
        recs[ix].flags = ProcFlags::EXITED;
        r.publish(recs, &[], 0);
        let before = super::sent(&r.engine).len();
        r.jump(key(TARGET), JumpTexts::default());
        assert_eq!(r.toasts(), ["Process not found in current view"]);
        assert_eq!(
            super::sent(&r.engine).len(),
            before,
            "no query for an exited record"
        );

        // A key the snapshot never held.
        r.jump(key(99_999), JumpTexts::default());
        assert_eq!(super::sent(&r.engine).len(), before);
        let toasts = r.toasts();
        assert!(
            toasts
                .last()
                .is_some_and(|t| t.starts_with("Process not found in current view")),
            "{toasts:?}"
        );
    }

    #[gpui_kit::test]
    fn jump_hidden_offers_show_button(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            page.update(cx, |p, cx| {
                p.table()
                    .update(cx, |t, _| t.delegate.query_mut().set_filter("foo".into()));
            });
        });
        r.jump(
            key(TARGET),
            JumpTexts {
                hidden: Some("Owning process not visible in the current view.".into()),
                ..JumpTexts::default()
            },
        );
        // The filtered view lacks the target.
        let generation = r.last_query().generation;
        r.publish(records(), &[100, 101], generation);
        assert_eq!(
            r.toasts(),
            ["Owning process not visible in the current view."]
        );
        assert!(r.selected().is_empty());

        // The toast's Show button dispatches this; no element handles it, so it reaches the
        // page's app-level listener as the button's dispatch does.
        r.vcx.dispatch_action(ProcessesShowHidden);
        r.draw();
        let q = r.last_query();
        assert_eq!(q.filter.as_ref(), "", "the filter is cleared");
        assert!(q.generation > generation, "Show expands again");
        let hide = r
            .vcx
            .update(|_, cx| TlSettings::settings(cx).processes.hide_system);
        assert!(!hide);
        r.answer_with_target();
        assert_eq!(r.selected(), [ProcRowId::Process(key(TARGET))]);
    }

    #[gpui_kit::test]
    fn jump_leaves_history_mode(cx: &mut TestAppContext) {
        let mut state = UiState::default();
        state.processes.history_mode = true;
        let mut r = rig(cx, state);
        r.jump(key(TARGET), JumpTexts::default());
        let history = r
            .vcx
            .update(|_, cx| TlSettings::state(cx).processes.history_mode);
        assert!(!history);
    }

    #[gpui_kit::test]
    fn select_by_pid_resolves_identity(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        // An exited record with the same pid and an older start is never the target.
        let mut recs = records();
        let mut old = record(TARGET, "firefox", ProcFlags::EXITED);
        old.key.start_ticks = 1;
        recs.push(old);
        recs.sort_by_key(|x| x.key);
        r.publish(recs, &[], 0);
        let page = r.page.clone();
        r.vcx.update(|window, cx| {
            page.update(cx, |p, cx| {
                p.select_by_pid(TARGET, JumpTexts::default(), window, cx)
            })
        });
        r.draw();
        assert!(
            r.last_query()
                .expanded
                .contains(&NodeId::Process(key(PARENT)))
        );
        r.answer_with_target();
        assert_eq!(r.selected(), [ProcRowId::Process(key(TARGET))]);

        r.vcx.update(|window, cx| {
            page.update(cx, |p, cx| {
                p.select_by_pid(4321, JumpTexts::default(), window, cx)
            })
        });
        r.draw();
        assert_eq!(r.toasts(), ["Process not found in current view"]);
    }
}

/// The filter box's Processes side: one query per `FilterChanged`, the include-children rule
/// and toasts, and the no-match state (tasklens-pdu.24, UI-PRC-016, UI-PRC-024).
mod filter {
    use std::sync::Arc;

    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
    use tl_model::client::EngineClient as _;
    use tl_model::command::Tab;
    use tl_model::settings::UiState;
    use tl_model::snapshot::Snapshot;
    use tl_model::table::TableMode;
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::filter::include_children_visible;
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{FilterChanged, GlobalShellEvents, main_window_view};

    use super::delegate::snapshot;
    use super::shell::open_with;

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let (engine, mut vcx) = open_with(cx, UiState::default());
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
            .expect("Processes built");
        Rig { engine, vcx, page }
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// What the shell emits after its 100 ms debounce.
        fn filter(&mut self, tab: Tab, text: &str) {
            let text = text.to_owned();
            self.vcx.update(|_, cx| {
                let events = cx.global::<GlobalShellEvents>().0.clone();
                events.update(cx, |_, cx| {
                    cx.emit(FilterChanged {
                        tab,
                        text: text.into(),
                    })
                });
            });
            self.draw();
        }

        fn generation(&mut self) -> u64 {
            let page = self.page.clone();
            self.vcx
                .update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation())
        }

        fn toasts(&mut self) -> Vec<String> {
            self.vcx.update(|_, cx| {
                let main = main_window_view(cx).expect("main window open");
                let texts = main.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(|t| t.to_string()).collect()
            })
        }

        /// An empty view for the current query, as a new table version.
        fn publish_empty(&mut self) {
            let generation = self.generation();
            let version = 10 + generation;
            let mut s = Snapshot::clone(&snapshot(version, Vec::new(), Vec::new(), Vec::new()));
            s.sequence = self.engine.latest().sequence + 1;
            Arc::make_mut(&mut s.table).query_generation = generation;
            self.engine.publish(s);
            self.draw();
        }
    }

    #[gpui_kit::test]
    fn filter_changed_sends_new_generation(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let before = super::sent(&r.engine).len();
        let generation = r.generation();
        r.filter(Tab::Processes, "FireFox");
        let sent = super::sent(&r.engine);
        assert_eq!(sent.len(), before + 1, "exactly one query, at once");
        let q = sent.last().expect("a query");
        assert_eq!(q.filter.as_ref(), "firefox");
        assert!(q.generation > generation, "a new generation");

        // Another tab's filter sends nothing here.
        r.filter(Tab::Services, "foo");
        assert_eq!(super::sent(&r.engine).len(), before + 1);
    }

    #[gpui_kit::test]
    fn filter_lowercased(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.filter(Tab::Processes, "FiReFox");
        let q = super::sent(&r.engine).pop().expect("a query");
        assert_eq!(q.filter.as_ref(), "firefox");
    }

    #[test]
    fn include_children_checkbox_visibility_rule() {
        assert!(include_children_visible(true, TableMode::Grouped));
        assert!(!include_children_visible(false, TableMode::Flat));
        assert!(include_children_visible(false, TableMode::Tree));
        assert!(!include_children_visible(false, TableMode::Grouped));
        // The shell's checkbox applies the same rule.
        for (text, mode) in [
            ("a", TableMode::Grouped),
            ("", TableMode::Flat),
            ("", TableMode::Tree),
        ] {
            assert_eq!(
                tl_ui::window::filter::include_children_visible(Tab::Processes, text, mode),
                include_children_visible(!text.is_empty(), mode),
            );
        }
    }

    #[gpui_kit::test]
    fn include_children_toasts(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let set = |r: &mut Rig, on: bool| {
            r.vcx.update(|_, cx| {
                TlSettings::update(cx, |d| d.settings.processes.filter_include_children = on)
            });
            r.draw();
        };
        set(&mut r, true);
        assert_eq!(r.toasts(), ["Including filtered process children."]);
        set(&mut r, false);
        assert_eq!(
            r.toasts().last().map(String::as_str),
            Some("Hiding unmatched filtered process children.")
        );
        // Another setting changing shows no new toast.
        let count = r.toasts().len();
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.processes.mode = TableMode::Tree)
        });
        r.draw();
        assert_eq!(r.toasts().len(), count);
    }

    #[gpui_kit::test]
    fn empty_filter_state_shows_text_and_button(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        // An empty view without a filter: no no-match text.
        r.publish_empty();
        assert!(r.vcx.debug_bounds("TlTreeTable-empty").is_none());

        r.filter(Tab::Processes, "nothing-matches");
        r.publish_empty();
        let page = r.page.clone();
        let text = r.vcx.update(|_, cx| {
            use tl_ui::table::tree_table::TreeTableDelegate as _;
            page.read(cx).table().read(cx).delegate.empty_text()
        });
        assert_eq!(text.as_deref(), Some("No processes match the filter"));
        let state = r
            .vcx
            .debug_bounds("TlTreeTable-empty-state")
            .expect("the empty state is drawn");
        let body = r.vcx.debug_bounds("TlTreeTable-body").expect("the body");
        assert_eq!(
            state.origin.y - body.origin.y,
            gpui_kit::px(96.),
            "96 below the body top"
        );
        assert!(
            r.vcx.debug_bounds("TlTreeTable-empty").is_none(),
            "not the plain label"
        );
        let button = r
            .vcx
            .debug_bounds("TlButton-TlEmptyState-clear-filter")
            .expect("the Clear filter button");
        // The button dispatches ClearFilter, which the shell turns into an empty filter.
        r.vcx
            .simulate_click(button.center(), gpui_kit::Modifiers::none());
        // Past the shell's 100 ms debounce.
        r.vcx
            .executor()
            .advance_clock(std::time::Duration::from_millis(200));
        r.draw();
        let q = super::sent(&r.engine).pop().expect("a query");
        assert_eq!(q.filter.as_ref(), "", "Clear filter cleared the filter");
        assert!(
            r.vcx.debug_bounds("TlTreeTable-group-row").is_none(),
            "no group header row"
        );
    }
}

/// Hold Ctrl to pause (UI-PRC-017): the shell owns the rule, the page holds its snapshot.
mod pause {
    use std::sync::Arc;

    use gpui_kit::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext};
    use tl_model::client::EngineClient as _;
    use tl_model::command::Tab;
    use tl_model::ids::NodeId;
    use tl_model::process::ProcFlags;
    use tl_model::settings::UiState;
    use tl_model::snapshot::Snapshot;
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::window::main::{ShellState, main_window_view};

    use super::delegate::{key, record, row, snapshot};
    use super::shell::open_with;

    struct Rig {
        engine: Arc<tl_ui::testing::fake_engine::FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
        version: u64,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let (engine, mut vcx) = open_with(cx, UiState::default());
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
            .expect("Processes built");
        let mut r = Rig {
            engine,
            vcx,
            page,
            version: 1,
        };
        r.publish(&[100, 101, 102]);
        r
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// A new table version with one flat process row per PID, for the current query.
        fn publish(&mut self, pids: &[u32]) {
            self.version += 1;
            let records = pids
                .iter()
                .map(|&p| record(p, &format!("proc-{p}"), ProcFlags::empty()))
                .collect();
            let rows = pids
                .iter()
                .enumerate()
                .map(|(i, &p)| row(NodeId::Process(key(p)), 0, Some(i as u32), None))
                .collect();
            let page = self.page.clone();
            let generation = self
                .vcx
                .update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation());
            let mut s = Snapshot::clone(&snapshot(self.version, rows, Vec::new(), records));
            s.sequence = self.engine.latest().sequence + 1;
            Arc::make_mut(&mut s.table).query_generation = generation;
            self.engine.publish(s);
            self.draw();
        }

        fn modifiers(&mut self, control: bool, shift: bool) {
            self.vcx.simulate_modifiers_change(Modifiers {
                control,
                shift,
                ..Modifiers::default()
            });
            self.draw();
        }

        fn is_paused(&mut self) -> bool {
            let page = self.page.clone();
            self.vcx.update(|_, cx| page.read(cx).is_paused())
        }

        fn shell_paused(&mut self) -> bool {
            self.vcx.update(|_, cx| cx.global::<ShellState>().paused)
        }

        /// The PIDs of the rows the table draws.
        fn shown(&mut self) -> Vec<u32> {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                let d = &page.read(cx).table().read(cx).delegate;
                let snap = d.snapshot();
                d.query()
                    .shown()
                    .map(|v| {
                        v.rows
                            .iter()
                            .filter_map(|r| r.record)
                            .map(|i| snap.processes.records[i as usize].key.pid)
                            .collect()
                    })
                    .unwrap_or_default()
            })
        }

        fn renders(&mut self) -> u32 {
            let page = self.page.clone();
            self.vcx
                .update(|_, cx| page.read(cx).table().read(cx).render_count.get())
        }
    }

    #[gpui_kit::test]
    fn ctrl_down_freezes_rows_and_sets_status(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        assert_eq!(r.shown(), [100, 101, 102]);
        r.modifiers(true, false);
        assert!(r.is_paused());
        assert!(r.shell_paused(), "the status chip shows the paused tag");
        r.publish(&[200, 201]);
        assert_eq!(r.shown(), [100, 101, 102], "rows frozen");

        // Ctrl up: the latest view, rendered once.
        let before = r.renders();
        r.modifiers(false, false);
        assert!(!r.is_paused());
        assert!(!r.shell_paused());
        assert_eq!(r.shown(), [200, 201]);
        assert_eq!(r.renders(), before + 1, "one render on resume");
    }

    #[gpui_kit::test]
    fn ctrl_with_other_modifier_does_not_pause(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.modifiers(false, true);
        r.modifiers(true, true);
        assert!(!r.is_paused(), "Ctrl+Shift");
        r.publish(&[200]);
        assert_eq!(r.shown(), [200]);
    }

    #[gpui_kit::test]
    fn window_deactivate_resumes(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.modifiers(true, false);
        r.publish(&[200]);
        assert!(r.is_paused());
        r.vcx.deactivate_window();
        r.draw();
        assert!(!r.is_paused());
        assert_eq!(r.shown(), [200]);
    }

    #[gpui_kit::test]
    fn no_render_while_paused(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.modifiers(true, false);
        let before = r.renders();
        for n in 0..5 {
            r.publish(&[200 + n]);
        }
        assert_eq!(r.renders(), before, "five snapshots, no table render");
        assert!(r.is_paused());
    }
}

/// The Processes tab's status chip text (UI-PRC-018).
mod status {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::Modifiers;
    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
    use tl_model::client::EngineClient as _;
    use tl_model::command::Tab;
    use tl_model::health::HelperState;
    use tl_model::ids::{NodeId, PseudoKind};
    use tl_model::process::{GroupRowKind, ProcFlags};
    use tl_model::settings::UiState;
    use tl_model::snapshot::Snapshot;
    use tl_model::table::TableRow;
    use tl_ui::actions::{EndHelperSession, StartHelperSession};
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::window::main::{FilterChanged, GlobalShellEvents, main_window_view};
    use tl_ui::window::status::{StatusActionKind, StatusChanged};

    use super::delegate::{group, key, record, row, snapshot};
    use super::shell::open_with;

    struct Rig {
        engine: Arc<tl_ui::testing::fake_engine::FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
        version: u64,
        emitted: Rc<Cell<u32>>,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let (engine, mut vcx) = open_with(cx, UiState::default());
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
            .expect("Processes built");
        let emitted = Rc::new(Cell::new(0));
        let sink = emitted.clone();
        vcx.update(|_, cx| {
            cx.subscribe(&page, move |_, _: &StatusChanged, _| {
                sink.set(sink.get() + 1)
            })
            .detach();
        });
        Rig {
            engine,
            vcx,
            page,
            version: 1,
            emitted,
        }
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// A new table version with one process row per PID, `pseudo` pseudo rows, and the
        /// view's `total`, `shown` and `restricted`.
        fn publish(
            &mut self,
            pids: &[u32],
            pseudo: usize,
            total: u32,
            shown: u32,
            restricted: u32,
        ) {
            self.version += 1;
            let records = pids
                .iter()
                .map(|&p| record(p, &format!("proc-{p}"), ProcFlags::empty()))
                .collect();
            let mut rows: Vec<TableRow> = pids
                .iter()
                .enumerate()
                .map(|(i, &p)| row(NodeId::Process(key(p)), 0, Some(i as u32), None))
                .collect();
            let kinds = [PseudoKind::Interrupts, PseudoKind::MemoryCompression];
            let groups: Vec<_> = kinds[..pseudo]
                .iter()
                .map(|&k| group(NodeId::Pseudo(k), GroupRowKind::Pseudo, "pseudo", 0))
                .collect();
            rows.extend(
                groups
                    .iter()
                    .enumerate()
                    .map(|(g, gr)| row(gr.node.clone(), 0, None, Some(g as u32))),
            );
            let page = self.page.clone();
            let generation = self
                .vcx
                .update(|_, cx| page.read(cx).table().read(cx).delegate.query().generation());
            let mut s = Snapshot::clone(&snapshot(self.version, rows, groups, records));
            s.sequence = self.engine.latest().sequence + 1;
            s.health = Arc::clone(&self.engine.latest().health);
            let t = Arc::make_mut(&mut s.table);
            t.query_generation = generation;
            t.total = total;
            t.shown = shown;
            t.restricted = restricted;
            self.engine.publish(s);
            self.draw();
        }

        fn filter(&mut self, text: &str) {
            let text = text.to_owned();
            self.vcx.update(|_, cx| {
                let events = cx.global::<GlobalShellEvents>().0.clone();
                events.update(cx, |_, cx| {
                    cx.emit(FilterChanged {
                        tab: Tab::Processes,
                        text: text.into(),
                    })
                });
            });
            self.draw();
        }

        fn select(&mut self, pids: &[u32]) {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                let table = page.read(cx).table().clone();
                table.update(cx, |t, cx| {
                    t.selection.clear();
                    for &p in pids {
                        t.selection.ctrl_click(ProcRowId::Process(key(p)));
                    }
                    cx.notify();
                });
            });
            self.draw();
        }

        fn text(&mut self) -> String {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                let mut out = String::new();
                page.read(cx).status_text(&mut out);
                out
            })
        }

        /// Publishes a new helper state.
        fn helper(&mut self, state: HelperState) {
            self.engine.publish_with(|s| {
                let health = Arc::make_mut(&mut s.health);
                health.helper = state;
                health.version += 1;
            });
            self.draw();
        }

        /// The (kind, label, tooltip) of each chip part.
        fn actions(&mut self) -> Vec<(StatusActionKind, String, String)> {
            let page = self.page.clone();
            self.vcx.update(|_, cx| {
                page.read(cx)
                    .status_actions()
                    .iter()
                    .map(|a| (a.kind, a.label.to_string(), a.tooltip.to_string()))
                    .collect()
            })
        }

        fn click(&mut self, selector: &'static str) {
            self.draw();
            let at = self
                .vcx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} drawn"))
                .center();
            self.vcx.simulate_click(at, Modifiers::none());
        }

        fn drawn(&mut self, selector: &'static str) -> bool {
            self.draw();
            self.vcx.debug_bounds(selector).is_some()
        }
    }

    /// Counts dispatches of the two helper session actions: (start, end).
    fn count_sessions(r: &mut Rig) -> (Rc<Cell<u32>>, Rc<Cell<u32>>) {
        let start = Rc::new(Cell::new(0));
        let end = Rc::new(Cell::new(0));
        let (s, e) = (start.clone(), end.clone());
        r.vcx.update(|_, cx| {
            cx.on_action(move |_: &StartHelperSession, _| s.set(s.get() + 1));
            cx.on_action(move |_: &EndHelperSession, _| e.set(e.get() + 1));
        });
        (start, end)
    }

    #[gpui_kit::test]
    fn restricted_segment_starts_helper(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let (start, _) = count_sessions(&mut r);
        r.helper(HelperState::Locked);
        r.publish(&[1, 2, 3, 4], 0, 296, 296, 42);
        assert_eq!(r.text(), "296 Processes");
        assert_eq!(
            r.actions(),
            vec![(
                StatusActionKind::Link,
                "42 restricted".to_owned(),
                "Some values for other users' processes need administrator rights. Click to enable."
                    .to_owned()
            )]
        );
        r.click("StatusChip-action-0");
        assert_eq!(start.get(), 1);

        r.publish(&[1, 2, 3, 4], 0, 296, 296, 0);
        assert!(r.actions().is_empty());
        assert!(!r.drawn("StatusChip-action-0"));
    }

    #[gpui_kit::test]
    fn admin_badge_ends_helper(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let (start, end) = count_sessions(&mut r);
        r.helper(HelperState::Unlocked);
        r.publish(&[1, 2, 3, 4], 0, 296, 296, 42);
        let kinds: Vec<_> = r.actions().into_iter().map(|a| (a.0, a.1)).collect();
        assert_eq!(
            kinds,
            vec![
                (StatusActionKind::Link, "42 restricted".to_owned()),
                (StatusActionKind::Badge, "Administrator mode".to_owned()),
            ]
        );
        r.click("StatusChip-action-1");
        assert_eq!(end.get(), 1);
        assert_eq!(start.get(), 0);

        r.helper(HelperState::Locked);
        assert_eq!(r.actions().len(), 1);
        assert!(!r.drawn("StatusChip-action-1"));
    }

    #[gpui_kit::test]
    fn status_text_variants(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.publish(&[1, 2, 3, 4], 0, 296, 296, 0);
        assert_eq!(r.text(), "296 Processes");

        r.select(&[1]);
        assert_eq!(r.text(), "296 Processes", "one selected row: no segment");
        r.select(&[1, 2, 3]);
        assert_eq!(r.text(), "296 Processes | 3 processes selected");

        r.publish(&[1, 2, 3, 4], 0, 296, 296, 42);
        assert_eq!(
            r.text(),
            "296 Processes | 3 processes selected | 42 restricted"
        );

        // A filter: the old counts stay until the view for it arrives.
        r.filter("proc");
        assert_eq!(
            r.text(),
            "296 Processes | 3 processes selected | 42 restricted"
        );
        // The view keeps PIDs 1 and 4; the hidden selected rows are not counted.
        r.publish(&[1, 4], 0, 296, 41, 0);
        assert_eq!(r.text(), "41 / 296 Processes");
        r.select(&[1, 2, 3, 4]);
        assert_eq!(r.text(), "41 / 296 Processes | 2 processes selected");
    }

    #[gpui_kit::test]
    fn status_count_excludes_pseudo_and_hidden_kthreads(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        // Three process rows and two pseudo rows; the engine left the hidden kernel threads
        // out of `total` (Hide system processes, R-280).
        r.publish(&[1, 2, 3], 2, 3, 3, 0);
        assert_eq!(r.text(), "3 Processes");
    }

    #[gpui_kit::test]
    fn status_changed_only_on_change(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.publish(&[1, 2], 0, 2, 2, 0);
        let after_first = r.emitted.get();
        assert!(after_first >= 1, "the first view emits");
        // A new version with the same counts.
        r.publish(&[2, 1], 0, 2, 2, 0);
        r.publish(&[1, 2], 0, 2, 2, 0);
        assert_eq!(r.emitted.get(), after_first, "nothing changed");
        // One selected row changes no text either.
        r.select(&[1]);
        assert_eq!(r.emitted.get(), after_first);
        r.publish(&[1, 2, 3], 0, 3, 3, 0);
        assert_eq!(r.emitted.get(), after_first + 1);
    }
}

/// The ProcessTable keys for End Task and End Process (PROC-049, pdu.29).
mod keys {
    use std::sync::Arc;

    use gpui_kit::{Action, App, AppContext as _, Entity, KeyContext, Keystroke};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use tl_model::actions::{ActionKind, ActionTarget, Signal};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, Tab};
    use tl_model::ids::NodeId;
    use tl_model::process::ProcFlags;
    use tl_model::settings::{Settings, UiState};
    use tl_model::snapshot::Snapshot;
    use tl_ui::actions::FocusFilter;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::processes::actions::{CopyDetails, EndProcess, EndTask};
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::pages::processes::{ProcessesPage, bind_keys_processes};
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window_on};

    use super::delegate::{key, record, row, snapshot};

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        page: Entity<ProcessesPage>,
    }

    /// The real app on the Processes tab, with its action tracker (the End handlers send
    /// through it).
    fn open(cx: &mut TestAppContext) -> (Arc<FakeEngine>, VisualTestContext) {
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
            init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Processes, cx))
            .expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        (engine, vcx)
    }

    /// The main window with PIDs 300 and 301 (own, unprotected) and PID 300 selected.
    fn rig(cx: &mut TestAppContext) -> Rig {
        let (engine, mut vcx) = open(cx);
        vcx.update(|_, cx| bind_keys_processes(cx));
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let page = vcx
            .read_entity(&main, |m, _| m.page_entity::<ProcessesPage>(Tab::Processes))
            .expect("Processes built");
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

    #[gpui_kit::test]
    fn keys_ignored_when_input_focused(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.dispatch_action(FocusFilter);
        r.draw();
        r.press("delete");
        r.press("shift-delete");
        assert!(r.sent().is_empty(), "no action reached the table");
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

/// The window-pick result (tasklens-pdu.48, PROC-055, UI-SHL-014, R-239). The window opens on
/// the Performance stand-in, so the shell builds the Processes page when a pick needs it.
mod pick {
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{AnyWindowHandle, Entity, Modifiers, TestAppContext, VisualTestContext};
    use tl_model::actions::{ActionKind, ActionTarget, Signal};
    use tl_model::client::EngineClient;
    use tl_model::command::{Command, KWinCommand, Tab};
    use tl_model::ids::NodeId;
    use tl_model::process::{ProcFlags, ProcessRecord};
    use tl_model::settings::{Settings, UiState};
    use tl_model::snapshot::{ComponentHeader, PickOutcome, PickResult, Snapshot, WindowsSnapshot};
    use tl_model::validity::Validity;
    use tl_ui::actions::StartWindowPick;
    use tl_ui::app::{AppEnv, TasklensUi, UiInit};
    use tl_ui::components::dialog::{DialogButton, TlDialogView};
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::pages::processes::confirm::{ConfirmKind, texts as confirm_texts};
    use tl_ui::pages::processes::delegate::ProcRowId;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{MainWindow, main_window_view, open_main_window_on, set_test_page};

    use super::delegate::{key, record, row, snapshot};

    const PID: u32 = 4242;
    const FILLER: u32 = 100;

    struct Rig {
        engine: Arc<FakeEngine>,
        vcx: VisualTestContext,
        main: Entity<MainWindow>,
        version: u64,
        windows_version: u64,
    }

    fn records() -> Vec<ProcessRecord> {
        vec![
            record(FILLER, "filler", ProcFlags::empty()),
            record(PID, "firefox", ProcFlags::empty()),
        ]
    }

    /// The real app with its action tracker, on the Performance stand-in, with a KWin bridge,
    /// the records of [`records`] and a pick started.
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
            init_strings(cx, "en-US");
            cx.set_reduce_motion(true);
            set_test_page(cx, Tab::Performance, super::shell::other);
        });
        let handle = cx
            .update(|cx| open_main_window_on(Tab::Performance, cx))
            .expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
        let mut r = Rig {
            engine,
            vcx,
            main,
            version: 1,
            windows_version: 0,
        };
        r.publish(&[], 0, None);
        r.vcx.dispatch_action(StartWindowPick);
        r.draw();
        assert!(r.picking(), "the pick started");
        assert!(r.page().is_none(), "Processes not built yet");
        r
    }

    impl Rig {
        fn draw(&mut self) {
            self.vcx.run_until_parked();
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        /// A snapshot with [`records`], a view with `shown` (pids) as rows for `generation`,
        /// and a valid windows component holding `pick`.
        fn publish(&mut self, shown: &[u32], generation: u64, pick: Option<PickResult>) {
            let recs = records();
            let rows = shown
                .iter()
                .map(|&pid| {
                    let ix = recs
                        .iter()
                        .position(|r| r.key.pid == pid)
                        .expect("shown pid is a record");
                    row(NodeId::Process(key(pid)), 0, Some(ix as u32), None)
                })
                .collect();
            self.version += 1;
            self.windows_version += 1;
            let mut s = Snapshot::clone(&snapshot(self.version, rows, Vec::new(), recs));
            s.sequence = self.engine.latest().sequence + 1;
            Arc::make_mut(&mut s.table).query_generation = generation;
            Arc::make_mut(&mut s.processes).header.version = self.version;
            s.windows = Arc::new(WindowsSnapshot {
                header: ComponentHeader {
                    version: self.windows_version,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                windows: Arc::from([]),
                pick,
            });
            self.engine.publish(s);
            self.draw();
        }

        fn result(&mut self, seq: u64, outcome: PickOutcome) {
            self.publish(&[], 0, Some(PickResult { seq, outcome }));
        }

        fn picked(&mut self, pid: u32) {
            self.result(1, PickOutcome::Picked { pid });
        }

        fn picking(&mut self) -> bool {
            let main = self.main.clone();
            self.vcx.update(|_, cx| main.read(cx).is_picking())
        }

        fn page(&mut self) -> Option<Entity<ProcessesPage>> {
            let main = self.main.clone();
            self.vcx
                .update(|_, cx| main.read(cx).page_entity::<ProcessesPage>(Tab::Processes))
        }

        fn active_tab(&mut self) -> Tab {
            let main = self.main.clone();
            self.vcx.update(|_, cx| main.read(cx).active_tab())
        }

        fn toasts(&mut self) -> Vec<String> {
            let main = self.main.clone();
            self.vcx.update(|_, cx| {
                let texts = main.read(cx).toasts().read(cx).visible_texts();
                texts.iter().map(ToString::to_string).collect()
            })
        }

        /// The open confirmation windows.
        fn dialogs(&mut self) -> Vec<(AnyWindowHandle, Entity<TlDialogView>)> {
            self.vcx.update(|_, cx| {
                cx.windows()
                    .into_iter()
                    .filter_map(|w| {
                        let handle = w.downcast::<Root>()?;
                        let frame = TlSecondaryWindow::<TlDialogView>::find(handle, cx)?;
                        Some((w, frame.read(cx).content().clone()))
                    })
                    .collect()
            })
        }

        /// The one open confirmation, drawn.
        fn dialog(&mut self) -> (VisualTestContext, Entity<TlDialogView>) {
            let mut open = self.dialogs();
            assert_eq!(open.len(), 1, "one confirmation");
            let (any, view) = open.pop().expect("a confirmation");
            let mut vcx = VisualTestContext::from_window(any, &self.vcx.cx);
            vcx.update(|window, _| window.activate_window());
            vcx.run_until_parked();
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            (vcx, view)
        }

        /// Clicks the open confirmation's button `selector`.
        fn choose(&mut self, selector: &'static str) {
            let (mut vcx, _) = self.dialog();
            let at = vcx.debug_bounds(selector).expect("dialog button").center();
            vcx.simulate_click(at, Modifiers::none());
            vcx.run_until_parked();
            self.draw();
        }

        /// The action requests sent: kind and target PIDs.
        fn actions(&self) -> Vec<(ActionKind, Vec<u32>)> {
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

        /// Answers the page's last query with a view that shows the filler and the target.
        fn answer(&mut self) {
            let generation = super::sent(&self.engine)
                .pop()
                .expect("a query was sent")
                .generation;
            self.publish(&[FILLER, PID], generation, None);
        }

        fn selected(&mut self) -> Vec<ProcRowId> {
            let page = self.page().expect("Processes built");
            self.vcx
                .update(|_, cx| page.read(cx).table().read(cx).selection.ids().collect())
        }

        fn target_visible(&mut self) -> bool {
            let page = self.page().expect("Processes built");
            self.vcx
                .update(|_, cx| page.read(cx).table().read(cx).visible_range().contains(&1))
        }
    }

    #[gpui_kit::test]
    fn pick_result_opens_picked_dialog_when_inactive(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.deactivate_window();
        r.picked(PID);
        assert!(!r.picking(), "end_window_pick ran");
        assert!(r.toasts().is_empty(), "the start toast went");
        let (mut vcx, view) = r.dialog();
        let heading = vcx.update(|_, cx| {
            let kind = ConfirmKind::Picked {
                name: "firefox".into(),
                pid: PID,
            };
            confirm_texts(&kind, cx).title.to_string()
        });
        assert_eq!(heading, "Picked firefox (PID 4242)");
        assert!(vcx.debug_bounds("TlDialogHeading").is_some());
        let (text, focused) = vcx.update(|window, cx| {
            let v = view.read(cx);
            (v.dialog().text().to_string(), v.focused_button(window))
        });
        assert_eq!(text, "What do you want to do with this process?");
        assert_eq!(focused, Some(DialogButton::Action), "Select is the default");
        assert!(vcx.debug_bounds("TlDialogSecond").is_some(), "Terminate");
        assert!(r.actions().is_empty());
    }

    #[gpui_kit::test]
    fn pick_select_jumps_to_row(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.deactivate_window();
        r.picked(PID);
        r.choose("TlDialogAction");
        assert!(r.dialogs().is_empty());
        assert_eq!(r.active_tab(), Tab::Processes);
        r.answer();
        assert_eq!(r.selected(), [ProcRowId::Process(key(PID))]);
        assert!(r.target_visible());
        assert!(r.actions().is_empty(), "Select sends no action");
    }

    #[gpui_kit::test]
    fn pick_terminate_sends_end_task(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.deactivate_window();
        r.picked(PID);
        r.choose("TlDialogSecond");
        assert!(
            r.dialogs().is_empty(),
            "an own process needs no second confirmation"
        );
        assert_eq!(r.actions(), [(ActionKind::Signal(Signal::Term), vec![PID])]);
    }

    #[gpui_kit::test]
    fn pick_ctrl_active_ends_task(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.simulate_modifiers_change(Modifiers::control());
        r.draw();
        r.picked(PID);
        assert!(!r.picking());
        // Leader decision Q2: Ctrl+click asks with the Picked dialog too (PROC-055).
        assert!(r.actions().is_empty(), "nothing before the confirmation");
        r.choose("TlDialogSecond");
        assert_eq!(r.actions(), [(ActionKind::Signal(Signal::Term), vec![PID])]);
    }

    #[gpui_kit::test]
    fn pick_plain_active_selects(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.picked(PID);
        assert!(!r.picking());
        assert!(r.dialogs().is_empty(), "no dialog");
        assert_eq!(r.active_tab(), Tab::Processes);
        r.answer();
        assert_eq!(r.selected(), [ProcRowId::Process(key(PID))]);
        assert!(r.target_visible());
        assert!(r.actions().is_empty());
    }

    #[gpui_kit::test]
    fn pick_no_window_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.result(1, PickOutcome::NoWindow);
        assert!(!r.picking());
        assert_eq!(r.toasts(), ["No window found"]);
        assert_eq!(r.active_tab(), Tab::Performance, "no tab switch");
        assert!(r.dialogs().is_empty());
    }

    #[gpui_kit::test]
    fn pick_cancelled_toast_once(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        // Esc cancels first and shows the toast; the engine's Cancelled reply replaces it.
        r.vcx.simulate_keystrokes("escape");
        r.draw();
        let cancels = r
            .engine
            .commands()
            .into_iter()
            .filter(|c| matches!(c, Command::KWin(KWinCommand::CancelPick { seq: 1 })))
            .count();
        assert_eq!(cancels, 1);
        assert_eq!(r.toasts(), ["No window selected"]);
        r.result(1, PickOutcome::Cancelled);
        assert_eq!(r.toasts(), ["No window selected"]);
        assert!(!r.picking());
    }

    #[gpui_kit::test]
    fn pick_late_cancel_keeps_new_pick(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        // Esc cancels pick 1, then pick 2 starts before the engine answers pick 1.
        r.vcx.simulate_keystrokes("escape");
        r.draw();
        assert!(!r.picking());
        r.vcx.dispatch_action(StartWindowPick);
        r.draw();
        assert!(r.picking(), "pick 2 started");
        let start = "Click a window to select its process. Hold CTRL while clicking to terminate \
                     process. Esc cancels.";
        assert_eq!(r.toasts(), [start]);

        // The late Cancelled for pick 1 leaves pick 2, its toast and Esc alone.
        r.result(1, PickOutcome::Cancelled);
        assert!(r.picking(), "pick 2 still runs");
        assert_eq!(r.toasts(), [start]);
        r.vcx.simulate_keystrokes("escape");
        r.draw();
        assert!(!r.picking(), "Esc still cancels pick 2");
        let cancels: Vec<u64> = r
            .engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::KWin(KWinCommand::CancelPick { seq }) => Some(seq),
                _ => None,
            })
            .collect();
        assert_eq!(cancels, [1, 2]);
        assert_eq!(r.toasts(), ["No window selected"]);
    }

    #[gpui_kit::test]
    fn pick_unknown_process_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.picked(99_999);
        assert!(!r.picking());
        assert_eq!(r.toasts(), ["Unknown process"]);
        assert!(r.dialogs().is_empty());
        assert_eq!(r.active_tab(), Tab::Performance);
    }

    #[gpui_kit::test]
    fn pick_same_seq_ignored(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.vcx.deactivate_window();
        r.picked(PID);
        let (mut vcx, _) = r.dialog();
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        r.draw();
        assert!(r.dialogs().is_empty(), "Esc cancels the dialog");

        // A later snapshot still carries seq 1.
        r.picked(PID);
        assert!(r.dialogs().is_empty(), "handled once");
        r.result(1, PickOutcome::NoWindow);
        assert!(
            r.toasts().is_empty(),
            "same seq, new outcome: still ignored"
        );
        assert!(r.actions().is_empty());
    }
}
