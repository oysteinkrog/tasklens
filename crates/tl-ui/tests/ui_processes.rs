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
        vcx.update(|window, cx| page.read(cx).focus_table(window, cx));
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
