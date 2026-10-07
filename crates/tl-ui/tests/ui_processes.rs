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
    use tl_ui::pages::processes::cells::process_column;
    use tl_ui::pages::processes::delegate::{ProcRowId, ProcessDelegate, SHIPPED_PHASE, UnitIds};
    use tl_ui::pages::processes::table::GroupHeader;
    use tl_ui::table::actions::OpenProperties;
    use tl_ui::table::cells::CellOut;
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

    fn group(node: NodeId, kind: GroupRowKind, name: &str, members: u32) -> GroupRow {
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

        // A phase 2 column is not offered while SHIPPED_PHASE is 1.
        assert_eq!(SHIPPED_PHASE, 1);
        assert_eq!(process_column(ColumnId::Version).phase, 2);
        assert!(!offered.contains(&ColumnId::Version));
        assert!(!d.columns().order.contains(&ColumnId::Version));
        assert!(offered.contains(&ColumnId::Cpu));

        // No GPU: no slot.
        let d = new_delegate(vcx, snapshot(1, rows, groups, records));
        assert!(!d.offered_columns().contains(&ColumnId::Gpu0));
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

    fn other(_: &mut Window, cx: &mut Context<MainWindow>) -> PageSlot {
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
