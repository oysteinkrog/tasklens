//! Process column registry (E22-002): ids, labels, groups, kinds, templates, widths and heat.

use gpui_kit::TestAppContext;
use rustc_hash::FxHashSet;
use tl_model::ids::ColumnId;
use tl_model::settings::ColumnTables;
use tl_ui::i18n::{init_strings, tr};
use tl_ui::pages::processes::cells::{
    DEFAULT_VISIBLE, HeatRef, PROCESS_COLUMNS, PickerGroup, ProcColumnKind, process_column,
};
use tl_ui::table::columns::ColumnAlign;

const UNSORTABLE: [ColumnId; 6] = [
    ColumnId::Pss,
    ColumnId::Uss,
    ColumnId::Cycles,
    ColumnId::CyclesDelta,
    ColumnId::CtxSwitches,
    ColumnId::CtxSwitchesDelta,
];

#[test]
fn registry_ids_unique_and_complete() {
    assert_eq!(PROCESS_COLUMNS.len(), ColumnId::COUNT);
    assert_eq!(PROCESS_COLUMNS[0].id, ColumnId::Name);
    let ids: FxHashSet<ColumnId> = PROCESS_COLUMNS.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), ColumnId::COUNT, "an id appears twice");
    for id in ColumnId::ALL {
        assert!(ids.contains(&id), "{} missing", id.as_str());
        assert_eq!(process_column(id).id, id);
    }
}

#[test]
fn registry_groups_in_picker_order() {
    let mut last = 0;
    for col in PROCESS_COLUMNS {
        let at = PickerGroup::ALL
            .iter()
            .position(|g| *g == col.group)
            .expect("group listed");
        assert!(at >= last, "{} out of group order", col.id.as_str());
        last = at;
    }
    assert!(PROCESS_COLUMNS.iter().all(|c| (1..=4).contains(&c.phase)));
}

#[gpui_kit::test]
fn registry_labels_resolve(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_strings(cx, "en-US");
        for col in PROCESS_COLUMNS {
            for key in [Some(col.label), col.alias, col.header_tooltip]
                .into_iter()
                .flatten()
            {
                assert_ne!(tr(cx, key).as_ref(), key, "{key} has no string");
            }
        }
        for group in PickerGroup::ALL {
            assert_ne!(tr(cx, group.label_key()).as_ref(), group.label_key());
        }
        let label = |id| tr(cx, process_column(id).label);
        assert_eq!(label(ColumnId::Name), "Name");
        assert_eq!(label(ColumnId::CpuRelative), "CPU (one core = 100%)");
        assert_eq!(label(ColumnId::GpuMemDedicated), "VRAM");
        assert_eq!(label(ColumnId::Gpu15), "GPU 15");
        assert_eq!(label(ColumnId::OomScoreAdj), "OOM adj");
        let alias = |id| process_column(id).alias.map(|k| tr(cx, k));
        assert_eq!(
            alias(ColumnId::Memory).as_deref(),
            Some("Working set private")
        );
        assert_eq!(
            alias(ColumnId::GpuMemDedicated).as_deref(),
            Some("GPU memory dedicated")
        );
        assert_eq!(alias(ColumnId::Cpu), None);
        let tip = |id| process_column(id).header_tooltip.map(|k| tr(cx, k));
        assert_eq!(
            tip(ColumnId::Memory).as_deref(),
            Some("Private memory (RssAnon). Shared and file-backed pages are not counted.")
        );
        assert_eq!(
            tip(ColumnId::WsTotal).as_deref(),
            Some("Resident set size (VmRSS)")
        );
        assert_eq!(
            tip(ColumnId::Network).as_deref(),
            Some("Attributed TCP traffic. UDP and other users' sockets are not counted.")
        );
        assert_eq!(
            tip(ColumnId::Handles).as_deref(),
            Some("Open file descriptors")
        );
        assert_eq!(tr(cx, PickerGroup::Io.label_key()), "I/O");
    });
}

#[gpui_kit::test]
fn registry_unsortable_columns(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_strings(cx, "en-US");
        for id in UNSORTABLE {
            let col = process_column(id);
            assert!(!col.sortable, "{} sortable", id.as_str());
            let tip = col.header_tooltip.map(|k| tr(cx, k));
            assert_eq!(tip.as_deref(), Some("Only measured for visible rows"));
        }
    });
    let sortable = PROCESS_COLUMNS.iter().filter(|c| c.sortable).count();
    assert_eq!(sortable, ColumnId::COUNT - UNSORTABLE.len());
}

#[test]
fn registry_default_set_matches_table_a() {
    let table_a = ColumnTables::default().processes;
    assert_eq!(table_a.order, DEFAULT_VISIBLE);
    assert_eq!(table_a.visible, DEFAULT_VISIBLE);
    for id in DEFAULT_VISIBLE {
        assert!(ColumnId::from_str(id).is_some(), "{id} unknown");
    }
    for col in PROCESS_COLUMNS {
        let width = table_a.widths.get(col.id.as_str()).map(|&w| w as f32);
        assert_eq!(col.default_width, width, "{}", col.id.as_str());
    }
}

#[test]
fn registry_templates_per_kind() {
    let t = |id, net_bytes| process_column(id).template(net_bytes);
    for id in [ColumnId::Pid, ColumnId::Ppid, ColumnId::Sid] {
        assert_eq!(t(id, false), Some("999999 +99"));
    }
    for col in PROCESS_COLUMNS {
        if col.kind == ProcColumnKind::Percent {
            assert_eq!(col.template(false), Some("100.0%"), "{}", col.id.as_str());
        }
    }
    assert_eq!(t(ColumnId::Gpu, false), Some("100.0%"));
    assert_eq!(t(ColumnId::Gpu7, false), Some("100.0%"));
    for id in [
        ColumnId::Network,
        ColumnId::NetSendRate,
        ColumnId::NetRecvRate,
    ] {
        assert_eq!(t(id, false), Some("xxx Mbps"));
        assert_eq!(t(id, true), Some("xxx MiB/s"));
    }
    for id in [
        ColumnId::Disk,
        ColumnId::DiskReadRate,
        ColumnId::IoWriteRate,
    ] {
        assert_eq!(t(id, false), Some("xxx GiB/s"));
        assert_eq!(t(id, true), Some("xxx GiB/s"));
    }
    for id in [
        ColumnId::Memory,
        ColumnId::WsTotal,
        ColumnId::GpuMemDedicated,
    ] {
        assert_eq!(t(id, false), Some("999 MiB"));
    }
    for id in [
        ColumnId::Threads,
        ColumnId::Handles,
        ColumnId::PageFaults,
        ColumnId::CtxSwitches,
        ColumnId::IoReadOps,
        ColumnId::IoWriteOps,
    ] {
        assert_eq!(t(id, false), Some("999999"));
    }
    for id in [
        ColumnId::CpuTime,
        ColumnId::CpuTimeKernel,
        ColumnId::CpuTimeUser,
    ] {
        assert_eq!(t(id, false), Some("999:59:59"));
    }
    for id in [
        ColumnId::Name,
        ColumnId::User,
        ColumnId::ImagePath,
        ColumnId::Cmdline,
    ] {
        assert_eq!(t(id, false), None);
    }
    for col in PROCESS_COLUMNS {
        let left = matches!(col.kind, ProcColumnKind::Text | ProcColumnKind::PowerClass);
        let expected = if left {
            ColumnAlign::Left
        } else {
            ColumnAlign::Right
        };
        assert_eq!(col.align(), expected, "{}", col.id.as_str());
    }
}

#[test]
fn registry_heat_refs() {
    let heat = |id| process_column(id).heat;
    assert_eq!(heat(ColumnId::Cpu), HeatRef::CpuMachine);
    assert_eq!(heat(ColumnId::CpuKernel), HeatRef::CpuMachine);
    assert_eq!(heat(ColumnId::CpuRelative), HeatRef::CpuOneCore);
    assert_eq!(heat(ColumnId::Memory), HeatRef::Memory);
    assert_eq!(heat(ColumnId::Disk), HeatRef::DiskIo);
    assert_eq!(heat(ColumnId::IoReadRate), HeatRef::DiskIo);
    assert_eq!(heat(ColumnId::Network), HeatRef::Network);
    assert_eq!(heat(ColumnId::Gpu), HeatRef::GpuPercent);
    assert_eq!(heat(ColumnId::Gpu3), HeatRef::GpuPercent);
    assert_eq!(heat(ColumnId::GpuMemDedicated), HeatRef::Vram);
    assert_eq!(heat(ColumnId::Power), HeatRef::Power);
    assert_eq!(heat(ColumnId::Threads), HeatRef::None);
    for col in PROCESS_COLUMNS {
        if matches!(
            col.kind,
            ProcColumnKind::Count
                | ProcColumnKind::IntegerId
                | ProcColumnKind::Text
                | ProcColumnKind::Duration
        ) {
            assert_eq!(col.heat, HeatRef::None, "{}", col.id.as_str());
        }
        if col.kind == ProcColumnKind::MemorySize && col.id != ColumnId::GpuMemDedicated {
            assert_eq!(col.heat, HeatRef::Memory, "{}", col.id.as_str());
        }
    }
}

// ---- Cell values and table (d) formatting (E22-004) ----

mod values {
    use std::sync::Arc;

    use gpui_kit::TestAppContext;
    use proptest::prelude::*;
    use tl_model::actions::SchedPolicy;
    use tl_model::ids::{ColumnId, GroupKey, NodeId, ProcessKey};
    use tl_model::process::{
        GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState, ProcessExtras, ProcessMeta,
        ProcessRecord,
    };
    use tl_model::settings::Settings;
    use tl_model::system::CpuSet;
    use tl_model::table::{Manager, ProcessTableView, TableRow, UnitRef};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, ProcessField, Validity};
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::processes::cells::{
        CellCtx, CellWords, GroupMembers, RowRef, fill_cell, process_column, split_pid_text,
        subtree_column, subtree_total,
    };
    use tl_ui::table::cells::CellOut;
    use tl_ui::table::tree_table::RowKind;
    use tl_ui::text_cache::CellTextCache;

    fn meta() -> ProcessMeta {
        let a = |s: &str| Arc::<str>::from(s);
        ProcessMeta {
            comm: a("firefox"),
            name: a("Firefox"),
            exe: Some(a("/usr/lib/firefox/firefox")),
            cmdline: Some(a("/usr/lib/firefox/firefox --new-window")),
            user: a("oystein"),
            cgroup: None,
            app_id: Some(a("firefox")),
            icon: None,
            description: Some(a("Web browser")),
            desktop_path: None,
            search_lower: a("firefox"),
        }
    }

    pub(super) fn record() -> ProcessRecord {
        ProcessRecord {
            key: ProcessKey {
                start_ticks: 100,
                boot_id: BootId(1),
                pid: 11528,
            },
            ppid: 1,
            uid: 1000,
            meta: Arc::new(meta()),
            group: GroupKind::App {
                app_id: Arc::from("firefox"),
            }
            .key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 12,
            cpu_pct: 3.27,
            rss_anon_bytes: 1_310_720_000,
            rss_bytes: 2_000_000_000,
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

    pub(super) fn extras() -> ProcessExtras {
        let mut affinity = CpuSet::empty();
        for cpu in (0..8).chain(16..24) {
            affinity.insert(cpu);
        }
        ProcessExtras {
            cpu_kernel_pct: 1.0,
            cpu_user_pct: 2.27,
            cpu_relative_pct: 104.6,
            utime_ticks: 4_000,
            stime_ticks: 340_400,
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
            affinity,
            handles: Some(1_234),
            exe_name: Arc::from("firefox"),
            exe_path: Arc::from("/usr/lib/firefox/firefox"),
            exe_deleted: false,
            unit: None,
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
        }
    }

    pub(super) fn group(kind: GroupRowKind) -> GroupRow {
        GroupRow {
            node: NodeId::Group(GroupKey(7)),
            key: GroupKind::App {
                app_id: Arc::from("firefox"),
            }
            .key(),
            kind,
            class: GroupKind::App {
                app_id: Arc::from("firefox"),
            },
            name: Arc::from("Firefox"),
            icon: None,
            members: 3,
            pids: 3,
            note: None,
            memory_bytes: 4096,
            disk_bps: 0.0,
            net_bps: 0.0,
            gpu_pct: 0.0,
            cpu_pct: 12.5,
            rss_anon_bytes: 4096,
            rss_bytes: 8192,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            threads: 40,
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

    fn table_row(has_children: bool, expanded: bool, descendants: u32) -> TableRow {
        TableRow {
            node: NodeId::Process(record().key),
            depth: 0,
            expanded,
            has_children,
            record: Some(0),
            group: None,
            descendants,
            subtree: None,
        }
    }

    /// Words equal to their Fluent keys, for tests that do not read word text.
    fn key_words() -> CellWords {
        CellWords::from_fn(Arc::from)
    }

    struct Fixture {
        view: ProcessTableView,
        records: Vec<ProcessRecord>,
        extras: Vec<ProcessExtras>,
        settings: Settings,
        words: CellWords,
        online: CpuSet,
        members: GroupMembers,
    }

    impl Fixture {
        fn new(r: ProcessRecord, words: CellWords) -> Self {
            Self {
                view: ProcessTableView::default(),
                records: vec![r],
                extras: vec![extras()],
                settings: Settings::default(),
                words,
                online: CpuSet::empty(),
                members: GroupMembers::default(),
            }
            .refreshed()
        }

        fn refreshed(mut self) -> Self {
            self.members
                .refresh(&self.view, &self.records, &self.extras);
            self
        }

        fn ctx(&self) -> CellCtx<'_> {
            CellCtx {
                view: &self.view,
                records: &self.records,
                extras: &self.extras,
                settings: &self.settings,
                words: &self.words,
                members: &self.members,
                ncpu: 32,
                online: &self.online,
                mem_total: 64 << 30,
                vram_total: 0,
                boot_time_s: 1_700_000_000,
                clk_tck: 100,
            }
        }

        /// Fills a process-row cell with `want_text` set.
        fn record_cell(&self, row: &TableRow, id: ColumnId) -> CellOut {
            let mut out = CellOut {
                want_text: true,
                ..CellOut::default()
            };
            let rec = RowRef::Record(&self.records[0], row, RowKind::Process);
            fill_cell(rec, process_column(id), &self.ctx(), &mut out);
            out
        }

        fn group_cell(&self, g: &GroupRow, kind: RowKind, id: ColumnId) -> CellOut {
            let mut out = CellOut {
                want_text: true,
                ..CellOut::default()
            };
            fill_cell(
                RowRef::Group(g, kind),
                process_column(id),
                &self.ctx(),
                &mut out,
            );
            out
        }

        fn text(&self, id: ColumnId) -> String {
            shown(&self.record_cell(&table_row(false, false, 0), id))
        }
    }

    /// The text the cell shows: the Arc text, else the formatted text.
    fn shown(out: &CellOut) -> String {
        out.arc
            .as_deref()
            .map_or_else(|| out.text.clone(), str::to_owned)
    }

    fn with(edit: impl FnOnce(&mut ProcessRecord)) -> Fixture {
        let mut r = record();
        edit(&mut r);
        Fixture::new(r, key_words())
    }

    #[test]
    fn cell_formats_match_table_d() {
        let f = Fixture::new(record(), key_words());
        // Percent
        assert_eq!(f.text(ColumnId::Cpu), "3.3%");
        assert_eq!(with(|r| r.cpu_pct = 0.04).text(ColumnId::Cpu), "0%");
        assert_eq!(with(|r| r.cpu_pct = 0.0).text(ColumnId::Cpu), "0%");
        // MemorySize
        assert_eq!(f.text(ColumnId::Memory), "1.22 GiB");
        assert_eq!(
            with(|r| r.rss_anon_bytes = 512).text(ColumnId::Memory),
            "512 B"
        );
        assert_eq!(
            with(|r| r.rss_anon_bytes = 16_384).text(ColumnId::Memory),
            "16.00 KiB"
        );
        assert_eq!(with(|r| r.rss_anon_bytes = 0).text(ColumnId::Memory), "0 B");
        // DiskRate
        let disk = with(|r| {
            r.io_read_bps = 300_000.0;
            r.io_write_bps = 119_430.0;
        });
        assert_eq!(disk.text(ColumnId::Disk), "0.4 MiB/s");
        assert_eq!(f.text(ColumnId::Disk), "0 MiB/s");
        // NetRate, bits fixed (the default), bits auto, bytes fixed and auto.
        let net = with(|r| r.net_rx_bps = 287_500.0);
        assert_eq!(net.text(ColumnId::Network), "2.3 Mbps");
        assert_eq!(f.text(ColumnId::Network), "0 Mbps");
        let mut auto = with(|r| r.net_rx_bps = 50.0);
        auto.settings.units.network_auto_scale = true;
        assert_eq!(auto.text(ColumnId::Network), "0.4 Kbps");
        let mut bytes = with(|r| r.net_tx_bps = 1_572_864.0);
        bytes.settings.units.network = tl_model::settings::NetworkUnit::Bytes;
        assert_eq!(bytes.text(ColumnId::NetSendRate), "1.5 MiB/s");
        bytes.settings.units.network_auto_scale = true;
        assert_eq!(bytes.text(ColumnId::NetSendRate), "1.5 MiB/s");
        assert_eq!(bytes.text(ColumnId::NetRecvRate), "0 MiB/s");
        // Count
        assert_eq!(
            with(|r| r.threads = 12_345).text(ColumnId::Threads),
            "12\u{2009}345"
        );
        assert_eq!(with(|r| r.threads = 0).text(ColumnId::Threads), "0");
        assert_eq!(f.text(ColumnId::Handles), "1\u{2009}234");
        // Duration: (4_000 + 340_400) ticks at 100 Hz = 3_444 s.
        assert_eq!(f.text(ColumnId::CpuTime), "0:57:24");
        assert_eq!(f.text(ColumnId::CpuTimeUser), "0:00:40");
        // IntegerId
        assert_eq!(f.text(ColumnId::Ppid), "1");
        assert_eq!(with(|r| r.nice = -5).text(ColumnId::Nice), "-5");
        // Text
        assert_eq!(f.text(ColumnId::ImageName), "firefox");
        assert_eq!(f.text(ColumnId::ImagePath), "/usr/lib/firefox/firefox");
        assert_eq!(f.text(ColumnId::User), "oystein");
        let deleted = with(|r| {
            let mut m = meta();
            m.exe = Some(Arc::from("/usr/bin/foo (deleted)"));
            r.meta = Arc::new(m);
        });
        assert_eq!(deleted.text(ColumnId::ImageName), "foo (deleted)");
        assert_eq!(f.text(ColumnId::Affinity), "0-7,16-23");
        // Time: Start time is a date and time, or a time of day for today.
        let start = f.text(ColumnId::StartTime);
        assert!(
            start.len() == "2023-11-14 22:13".len() || start.len() == "22:13:20".len(),
            "{start}"
        );
    }

    #[test]
    fn cmdline_cell_keeps_512_chars() {
        let long: String = "é".repeat(600);
        let f = with(|r| {
            let mut m = meta();
            m.cmdline = Some(Arc::from(long.as_str()));
            r.meta = Arc::new(m);
        });
        assert_eq!(f.text(ColumnId::Cmdline).chars().count(), 512);
        let none = with(|r| {
            let mut m = meta();
            m.cmdline = None;
            r.meta = Arc::new(m);
        });
        let out = none.record_cell(&table_row(false, false, 0), ColumnId::Cmdline);
        assert_eq!(out.validity, Validity::Restricted);
    }

    #[test]
    fn image_path_restricted_without_exe() {
        let f = with(|r| {
            let mut m = meta();
            m.exe = None;
            r.meta = Arc::new(m);
        });
        for id in [ColumnId::ImagePath, ColumnId::ImageName] {
            let out = f.record_cell(&table_row(false, false, 0), id);
            assert_eq!(out.validity, Validity::Restricted);
            assert!(out.text.is_empty() && out.arc.is_none());
        }
    }

    #[test]
    fn validity_glyphs_never_show_zero() {
        let numeric = [
            (ColumnId::Cpu, ProcessField::Cpu),
            (ColumnId::CpuKernel, ProcessField::Cpu),
            (ColumnId::Memory, ProcessField::RssAnon),
            (ColumnId::WsTotal, ProcessField::Rss),
            (ColumnId::Pss, ProcessField::Pss),
            (ColumnId::Uss, ProcessField::Uss),
            (ColumnId::Swap, ProcessField::Swap),
            (ColumnId::Disk, ProcessField::IoRead),
            (ColumnId::DiskWriteRate, ProcessField::IoWrite),
            (ColumnId::Network, ProcessField::NetTx),
            (ColumnId::NetRecvRate, ProcessField::NetRx),
            (ColumnId::Gpu, ProcessField::GpuUtil),
            (ColumnId::GpuMemDedicated, ProcessField::GpuMem),
            (ColumnId::Threads, ProcessField::Threads),
        ];
        let glyph_states = [
            Validity::WarmingUp,
            Validity::Restricted,
            Validity::Unsupported,
            Validity::Missing,
            Validity::Failed,
        ];
        for (col, field) in numeric {
            for state in glyph_states {
                let f = with(|r| r.validity.set(field, state));
                let out = f.record_cell(&table_row(false, false, 0), col);
                assert_eq!(out.validity, state, "{}", col.as_str());
                assert!(out.text.is_empty(), "{} wrote {:?}", col.as_str(), out.text);
                assert!(out.arc.is_none());
            }
            // Stale keeps the value and its text (drawn at 60% opacity by E22-004).
            let f = with(|r| r.validity.set(field, Validity::Stale));
            let out = f.record_cell(&table_row(false, false, 0), col);
            assert_eq!(out.validity, Validity::Stale);
            assert!(!out.text.is_empty(), "{}", col.as_str());
        }
        let g = {
            let mut g = group(GroupRowKind::App);
            g.validity.set(ProcessField::RssAnon, Validity::WarmingUp);
            g
        };
        let f = Fixture::new(record(), key_words());
        let out = f.group_cell(&g, RowKind::App, ColumnId::Memory);
        assert_eq!(out.validity, Validity::WarmingUp);
        assert!(out.text.is_empty());
        // Extras columns before the extras arrive.
        let mut f = Fixture::new(record(), key_words());
        f.extras.clear();
        let out = f.record_cell(&table_row(false, false, 0), ColumnId::WsPeak);
        assert_eq!(out.validity, Validity::WarmingUp);
        assert!(out.text.is_empty());
    }

    #[test]
    fn pid_plus_n_only_on_collapsed_parent() {
        let f = Fixture::new(record(), key_words());
        let collapsed = f.record_cell(&table_row(true, false, 18), ColumnId::Pid);
        assert_eq!(collapsed.text, "11528 +18");
        assert_eq!(split_pid_text(&collapsed.text), ("11528", " +18"));
        let expanded = f.record_cell(&table_row(true, true, 18), ColumnId::Pid);
        assert_eq!(split_pid_text(&expanded.text), ("11528", ""));
        let leaf = f.record_cell(&table_row(false, false, 0), ColumnId::Pid);
        assert_eq!(split_pid_text(&leaf.text), ("11528", ""));
        assert_ne!(collapsed.bits, expanded.bits);
    }

    /// Subtree totals (PROC-019, pdu.33): with `processes.subtree_totals` on, only a process row
    /// with children shows the engine's subtree sum in its Σ columns.
    #[test]
    fn subtree_totals_prefix_only_with_children() {
        let mut f = Fixture::new(record(), key_words());
        // The engine's sum: CPU 12.5%, 40 threads. The record alone has 3.3%.
        f.view.subtree = Arc::from(vec![group(GroupRowKind::App)]);
        f.settings.processes.subtree_totals = true;
        let parent = TableRow {
            subtree: Some(0),
            ..table_row(true, true, 3)
        };
        let leaf = table_row(false, false, 0);
        let has_total = |f: &Fixture, t: &TableRow| {
            subtree_total(RowRef::Record(&f.records[0], t, RowKind::Process), &f.ctx()).is_some()
        };

        // On, a parent: the Σ columns show the sum, the others the process's own value.
        assert!(has_total(&f, &parent));
        assert_eq!(shown(&f.record_cell(&parent, ColumnId::Cpu)), "12.5%");
        assert_eq!(shown(&f.record_cell(&parent, ColumnId::Threads)), "40");
        assert_eq!(
            shown(&f.record_cell(&parent, ColumnId::CpuTime)),
            f.text(ColumnId::CpuTime)
        );
        assert_eq!(shown(&f.record_cell(&parent, ColumnId::Pid)), "11528");

        // On, a leaf: no sum, so no prefix.
        assert!(!has_total(&f, &leaf));
        assert_eq!(shown(&f.record_cell(&leaf, ColumnId::Cpu)), "3.3%");

        // An app row aggregates by itself and never takes a subtree sum.
        let app = group(GroupRowKind::App);
        assert!(subtree_total(RowRef::Group(&app, RowKind::App), &f.ctx()).is_none());

        // Off, a parent the engine still sent a sum for (a collapsed one): no sum, no prefix.
        f.settings.processes.subtree_totals = false;
        assert!(!has_total(&f, &parent));
        assert_eq!(shown(&f.record_cell(&parent, ColumnId::Cpu)), "3.3%");

        for id in [
            ColumnId::Cpu,
            ColumnId::CpuRelative,
            ColumnId::Memory,
            ColumnId::Disk,
            ColumnId::Network,
            ColumnId::Gpu,
            ColumnId::Threads,
            ColumnId::Handles,
        ] {
            assert!(subtree_column(id), "{}", id.as_str());
        }
        for id in [
            ColumnId::Name,
            ColumnId::Pid,
            ColumnId::CpuTime,
            ColumnId::User,
        ] {
            assert!(!subtree_column(id), "{}", id.as_str());
        }
    }

    #[gpui_kit::test]
    fn app_and_unit_rows_blank_pid_and_type_text(cx: &mut TestAppContext) {
        let words = cx.update(|cx| {
            init_strings(cx, "en-US");
            CellWords::new(cx)
        });
        let f = Fixture::new(record(), words);
        let blank = |out: &CellOut| {
            out.validity == Validity::Valid && out.text.is_empty() && out.arc.is_none()
        };
        for (kind, row_kind, type_text) in [
            (GroupRowKind::App, RowKind::App, "App"),
            (GroupRowKind::Unit, RowKind::Unit, "Unit"),
        ] {
            let g = group(kind);
            for id in [ColumnId::Pid, ColumnId::Ppid, ColumnId::ImageName] {
                assert!(blank(&f.group_cell(&g, row_kind, id)), "{}", id.as_str());
            }
            assert_eq!(
                shown(&f.group_cell(&g, row_kind, ColumnId::Type)),
                type_text
            );
            assert_eq!(shown(&f.group_cell(&g, row_kind, ColumnId::Cpu)), "12.5%");
            assert_eq!(
                shown(&f.group_cell(&g, row_kind, ColumnId::Memory)),
                "4.00 KiB"
            );
        }
        // The app row reads User from its member process.
        let app = group(GroupRowKind::App);
        assert_eq!(
            shown(&f.group_cell(&app, RowKind::App, ColumnId::User)),
            "oystein"
        );
        let image_path = f.group_cell(&app, RowKind::App, ColumnId::ImagePath);
        assert_eq!(image_path.validity, Validity::Unsupported);
        // Unit rows read their member text through the index; Image path has no source yet.
        let unit_ref = UnitRef {
            manager: Manager::User,
            name: Arc::from("pipewire.service"),
        };
        let mut uf = Fixture::new(record(), f.words.clone());
        uf.extras[0].unit = Some(unit_ref.clone());
        // A new snapshot comes with a new view version.
        uf.view.header.version += 1;
        let uf = uf.refreshed();
        let unit = GroupRow {
            node: NodeId::Unit(unit_ref),
            ..group(GroupRowKind::Unit)
        };
        assert_eq!(
            shown(&uf.group_cell(&unit, RowKind::Unit, ColumnId::User)),
            "oystein"
        );
        assert_eq!(
            uf.group_cell(&unit, RowKind::Unit, ColumnId::ImagePath)
                .validity,
            Validity::Unsupported
        );
        // Pseudo rows: PID blank, Type "Pseudo".
        let pseudo = group(GroupRowKind::Pseudo);
        assert!(blank(&f.group_cell(
            &pseudo,
            RowKind::Pseudo,
            ColumnId::Pid
        )));
        assert_eq!(
            shown(&f.group_cell(&pseudo, RowKind::Pseudo, ColumnId::Type)),
            "Pseudo"
        );
        // Group header rows are blank in every column.
        for col in [
            ColumnId::Name,
            ColumnId::Cpu,
            ColumnId::Type,
            ColumnId::Memory,
        ] {
            assert!(blank(&f.group_cell(&app, RowKind::GroupHeader, col)));
        }
        // Process rows take the PROC-004 class.
        assert_eq!(f.text(ColumnId::Type), "App");
        let bg = Fixture::new(
            {
                let mut r = record();
                r.group = GroupKind::Background.key();
                r
            },
            f.words.clone(),
        );
        assert_eq!(bg.text(ColumnId::Type), "Background process");
        let kt = Fixture::new(
            {
                let mut r = record();
                r.flags = ProcFlags::KERNEL_THREAD;
                r.group = GroupKind::System.key();
                r
            },
            f.words.clone(),
        );
        assert_eq!(kt.text(ColumnId::Type), "Kernel thread");
        assert_eq!(f.text(ColumnId::State), "Sleeping");
        assert_eq!(f.text(ColumnId::Priority), "Normal");
    }

    #[test]
    fn cell_text_written_only_on_change() {
        let mut f = Fixture::new(record(), key_words());
        let row = table_row(false, false, 0);
        let mut cache: CellTextCache<u32> = CellTextCache::new();
        let mut formats = 0;
        // The table protocol: fill without text, look up the cache, fill with text on a miss.
        let mut lookup = |f: &Fixture, formats: &mut u32| {
            let mut out = CellOut::default();
            let rec = RowRef::Record(&f.records[0], &row, RowKind::Process);
            let col = process_column(ColumnId::Cpu);
            fill_cell(rec, col, &f.ctx(), &mut out);
            assert!(out.text.is_empty(), "text written without want_text");
            cache.get_or_format(0, ColumnId::Cpu, out.bits, out.validity, |s| {
                *formats += 1;
                out.want_text = true;
                fill_cell(rec, col, &f.ctx(), &mut out);
                s.push_str(&out.text);
            })
        };
        assert_eq!(lookup(&f, &mut formats).as_ref(), "3.3%");
        assert_eq!(lookup(&f, &mut formats).as_ref(), "3.3%");
        assert_eq!(formats, 1, "unchanged value formatted again");
        f.records[0].cpu_pct = 7.0;
        assert_eq!(lookup(&f, &mut formats).as_ref(), "7.0%");
        assert_eq!(formats, 2);
        f.records[0]
            .validity
            .set(ProcessField::Cpu, Validity::Stale);
        lookup(&f, &mut formats);
        assert_eq!(formats, 3, "a validity change re-formats");
    }

    #[test]
    fn text_cells_key_on_the_arc() {
        let f = Fixture::new(record(), key_words());
        let row = table_row(false, false, 0);
        let a = f.record_cell(&row, ColumnId::ImagePath);
        let b = f.record_cell(&row, ColumnId::ImagePath);
        assert_eq!(a.bits, b.bits);
        let g = with(|r| {
            let mut m = meta();
            m.exe = Some(Arc::from("/usr/lib/firefox/firefox"));
            r.meta = Arc::new(m);
        });
        assert_ne!(g.record_cell(&row, ColumnId::ImagePath).bits, a.bits);
    }

    #[test]
    fn group_member_index_built_once_per_view() {
        let f = Fixture::new(record(), key_words());
        let mut members = GroupMembers::default();
        let app = group(GroupRowKind::App);
        for _ in 0..60 {
            members.refresh(&f.view, &f.records, &f.extras);
            let ctx = CellCtx {
                members: &members,
                ..f.ctx()
            };
            let mut out = CellOut::default();
            fill_cell(
                RowRef::Group(&app, RowKind::App),
                process_column(ColumnId::User),
                &ctx,
                &mut out,
            );
            assert_eq!(out.arc.as_deref(), Some("oystein"));
        }
        #[cfg(feature = "test-hooks")]
        assert_eq!(members.builds(), 1);
        let mut view = f.view.clone();
        view.header.version += 1;
        members.refresh(&view, &f.records, &f.extras);
        #[cfg(feature = "test-hooks")]
        assert_eq!(members.builds(), 2);
    }

    proptest! {
        #[test]
        fn prop_percent_one_decimal(pct in 0.0f32..=100.0) {
            let f = with(|r| r.cpu_pct = pct);
            let text = f.text(ColumnId::Cpu);
            let body = text.strip_suffix('%').expect("ends in %");
            let ok = body == "0"
                || body.split_once('.').is_some_and(|(int, dec)| {
                    !int.is_empty()
                        && int.bytes().all(|b| b.is_ascii_digit())
                        && dec.len() == 1
                        && dec.bytes().all(|b| b.is_ascii_digit())
                });
            prop_assert!(ok, "{}", text);
        }
    }
}

// ---- Cell painting (E22-004 painting, UI-PRC-004) ----

mod paint {
    use std::sync::Arc;

    use std::rc::Rc;

    use gpui_kit::{
        App, Background, Context, Entity, Hsla, IntoElement, ParentElement, Pixels, Render,
        SharedString, Styled, TestAppContext, VisualTestContext, Window, div, px,
    };
    use tl_model::ids::ColumnId;
    use tl_model::settings::{AppearanceSettings, NetworkUnit, Settings, ThemeSettings};
    use tl_model::system::CpuSet;
    use tl_model::table::ProcessTableView;
    use tl_model::validity::Validity;
    use tl_ui::pages::processes::cells::{
        CellCtx, CellWords, DELETED_SUFFIX, GroupMembers, PaintState, ProcColumn, SUBTREE_PREFIX,
        cell_suffix, dims_zero, heat_alpha, heat_background, process_column,
    };
    use tl_ui::table::cells::{
        CellCtx as PaintCtx, CellOut, CellSuffix, DIM_OPACITY, cell_text_color, value_cell,
        with_overrides,
    };
    use tl_ui::table::columns::{ColumnAlign, ColumnKind};
    use tl_ui::table::tree_table::{RowFlags, STALE_OPACITY, metric_cell_opacity};
    use tl_ui::theme::{TlTheme, current, metrics};

    struct Env {
        view: ProcessTableView,
        settings: Settings,
        words: CellWords,
        members: GroupMembers,
        online: CpuSet,
        vram_total: u64,
    }

    impl Env {
        fn new() -> Self {
            Self {
                view: ProcessTableView::default(),
                settings: Settings::default(),
                words: CellWords::from_fn(Arc::from),
                members: GroupMembers::default(),
                online: CpuSet::empty(),
                vram_total: 0,
            }
        }

        fn ctx(&self) -> CellCtx<'_> {
            CellCtx {
                view: &self.view,
                records: &[],
                extras: &[],
                settings: &self.settings,
                words: &self.words,
                members: &self.members,
                ncpu: 32,
                online: &self.online,
                mem_total: 64 << 30,
                vram_total: self.vram_total,
                boot_time_s: 0,
                clk_tck: 100,
            }
        }
    }

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    }

    fn theme(cx: &mut TestAppContext) -> Arc<TlTheme> {
        cx.update(|cx| {
            setup(cx);
            current(cx).clone()
        })
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    const HEAT_ON: PaintState = PaintState {
        selected: false,
        exited: false,
        heat_on: true,
        heat_intensity_pct: 50,
        dim_zero: false,
        subtree: false,
    };

    /// A Valid CPU-style cell holding `value` as f32 bits.
    fn pct_out(value: f32) -> CellOut {
        CellOut {
            bits: u64::from(value.to_bits()),
            validity: Validity::Valid,
            ..CellOut::default()
        }
    }

    /// UI-PRC-004: heat alpha takes only 2% steps, so the tint holds on hidden differences.
    #[test]
    fn heat_alpha_takes_two_percent_steps() {
        let env = Env::new();
        let ctx = env.ctx();
        let cpu = process_column(ColumnId::Cpu);
        // ncpu 32: reference 12.5 %. 0.1 % CPU steps are 0.8 % alpha at intensity 100.
        for tenth in 0..=200 {
            let alpha = heat_alpha(cpu, f64::from(tenth) / 10.0, &ctx, 100);
            let steps = alpha / 0.02;
            assert!(
                (steps - steps.round()).abs() < 1e-4,
                "{alpha} at {tenth} tenths"
            );
        }
        // 5.0 % and 5.04 % share one step.
        assert_eq!(
            heat_alpha(cpu, 5.0, &ctx, 100),
            heat_alpha(cpu, 5.04, &ctx, 100)
        );
    }

    #[test]
    fn heat_alpha_clamped_and_intensity_scaled() {
        let env = Env::new();
        let ctx = env.ctx();
        let cpu = process_column(ColumnId::Cpu);
        // ncpu 32: reference 12.5 %.
        assert!(close(heat_alpha(cpu, 25.0, &ctx, 50), 0.5));
        // 0.25 rounds to the 2% step 0.26.
        assert!(close(heat_alpha(cpu, 6.25, &ctx, 50), 0.26));
        assert!(
            close(heat_alpha(cpu, 1000.0, &ctx, 80), 0.8),
            "clamps to intensity"
        );
        assert_eq!(heat_alpha(cpu, 0.0, &ctx, 50), 0.0);
        assert_eq!(heat_alpha(cpu, f64::NAN, &ctx, 50), 0.0);
        // One core: 400 %.
        let rel = process_column(ColumnId::CpuRelative);
        assert!(close(heat_alpha(rel, 200.0, &ctx, 100), 0.5));
        // Memory: 10 % of 64 GiB.
        let mem = process_column(ColumnId::Memory);
        let tenth = (64u64 << 30) as f64 / 10.0;
        assert!(close(heat_alpha(mem, tenth / 2.0, &ctx, 100), 0.5));
        // Disk 100 MiB/s; network 80 Mbps in bits mode, 10 MiB/s in bytes mode.
        let disk = process_column(ColumnId::Disk);
        assert!(close(heat_alpha(disk, 50.0 * 1_048_576.0, &ctx, 100), 0.5));
        let net = process_column(ColumnId::Network);
        assert!(close(heat_alpha(net, 5_000_000.0, &ctx, 100), 0.5));
        let mut bytes = Env::new();
        bytes.settings.units.network = NetworkUnit::Bytes;
        assert!(close(
            heat_alpha(net, 5.0 * 1_048_576.0, &bytes.ctx(), 100),
            0.5
        ));
        // GPU 100 %, Power 25 W.
        let gpu = process_column(ColumnId::Gpu);
        assert!(close(heat_alpha(gpu, 50.0, &ctx, 100), 0.5));
        let power = process_column(ColumnId::Power);
        assert!(close(heat_alpha(power, 12.5, &ctx, 100), 0.5));
        // VRAM: 25 % of the total; no VRAM gives 0.
        let vram = process_column(ColumnId::GpuMemDedicated);
        assert_eq!(heat_alpha(vram, 1e9, &ctx, 100), 0.0);
        let mut card = Env::new();
        card.vram_total = 8 << 30;
        assert!(close(
            heat_alpha(vram, (1u64 << 30) as f64, &card.ctx(), 100),
            0.5
        ));
        // Counts are never tinted.
        let threads = process_column(ColumnId::Threads);
        assert_eq!(heat_alpha(threads, 500.0, &ctx, 100), 0.0);
    }

    struct Host {
        out: CellOut,
        text: SharedString,
        col: ColumnId,
        st: PaintState,
    }

    /// The column kind the process delegate gives `col`: left aligned is Text, right Numeric.
    fn kind(col: &ProcColumn) -> ColumnKind {
        match col.align() {
            ColumnAlign::Left => ColumnKind::Text,
            ColumnAlign::Right => ColumnKind::Numeric,
        }
    }

    /// The live table's painter: `value_cell` with the page overrides, 120 px wide, on a row
    /// that is neither hovered nor selected unless `st.selected`.
    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = current(cx).clone();
            let ctx = PaintCtx {
                m: metrics(cx),
                theme: &theme,
                selected: self.st.selected,
                width: px(120.),
            };
            let col = process_column(self.col);
            let cell = value_cell(kind(col), self.text.clone(), &self.out, Rc::default(), &ctx);
            div()
                .size(px(400.))
                .child(with_overrides(cell, &self.out, &ctx))
        }
    }

    fn open(cx: &mut TestAppContext, host: Host) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(setup);
        let (view, vcx) = cx.add_window_view(|_, _| host);
        vcx.run_until_parked();
        (view, vcx)
    }

    /// Painted quads whose fill is `color`.
    fn quads_of(cx: &mut VisualTestContext, color: Hsla) -> usize {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window
                .painted_quads()
                .iter()
                .filter(|q| q.background == Background::from(color))
                .count()
        })
    }

    /// Heat quads drawn for `out` in `col` under `st`. The background is what
    /// `ProcessRowCells::fill` sets; the colour counted is the one heat would have when on.
    fn heat_count(
        cx: &mut TestAppContext,
        col: ColumnId,
        mut out: CellOut,
        st: PaintState,
    ) -> usize {
        let theme = theme(cx);
        let env = Env::new();
        let spec = process_column(col);
        let fill = heat_background(&out, spec, &env.ctx(), HEAT_ON, &theme).unwrap_or(Hsla {
            a: 0.5,
            ..theme.value_heat
        });
        out.background = heat_background(&out, spec, &env.ctx(), st, &theme);
        let (_, vcx) = open(
            cx,
            Host {
                out,
                text: "25.0%".into(),
                col,
                st,
            },
        );
        quads_of(vcx, fill)
    }

    #[gpui_kit::test]
    fn heat_drawn_once_on_a_valid_cpu_cell(cx: &mut TestAppContext) {
        assert_eq!(heat_count(cx, ColumnId::Cpu, pct_out(25.0), HEAT_ON), 1);
    }

    #[gpui_kit::test]
    fn heat_hidden_on_selected_and_when_off(cx: &mut TestAppContext) {
        let selected = PaintState {
            selected: true,
            ..HEAT_ON
        };
        assert_eq!(heat_count(cx, ColumnId::Cpu, pct_out(25.0), selected), 0);
    }

    #[gpui_kit::test]
    fn heat_hidden_when_setting_off(cx: &mut TestAppContext) {
        let off = PaintState {
            heat_on: false,
            ..HEAT_ON
        };
        assert_eq!(heat_count(cx, ColumnId::Cpu, pct_out(25.0), off), 0);
        let theme = theme(cx);
        let env = Env::new();
        let cpu = process_column(ColumnId::Cpu);
        assert!(heat_background(&pct_out(25.0), cpu, &env.ctx(), off, &theme).is_none());
    }

    #[gpui_kit::test]
    fn heat_hidden_on_counts_glyphs_and_zero(cx: &mut TestAppContext) {
        let threads = CellOut {
            bits: 500,
            validity: Validity::Valid,
            ..CellOut::default()
        };
        assert_eq!(heat_count(cx, ColumnId::Threads, threads, HEAT_ON), 0);

        let theme = theme(cx);
        let env = Env::new();
        let ctx = env.ctx();
        let cpu = process_column(ColumnId::Cpu);
        assert!(heat_background(&pct_out(0.0), cpu, &ctx, HEAT_ON, &theme).is_none());
        let threads = CellOut {
            bits: 500,
            validity: Validity::Valid,
            ..CellOut::default()
        };
        let count = process_column(ColumnId::Threads);
        assert!(heat_background(&threads, count, &ctx, HEAT_ON, &theme).is_none());
        let stale = CellOut {
            validity: Validity::Stale,
            ..pct_out(25.0)
        };
        assert!(heat_background(&stale, cpu, &ctx, HEAT_ON, &theme).is_none());
        let heat = heat_background(&pct_out(25.0), cpu, &ctx, HEAT_ON, &theme).expect("heat");
        assert!(close(heat.a, 0.5));
        assert_eq!(
            Hsla {
                a: theme.value_heat.a,
                ..heat
            },
            theme.value_heat
        );
    }

    #[gpui_kit::test]
    fn heat_hidden_on_glyph_cells(cx: &mut TestAppContext) {
        let failed = CellOut {
            validity: Validity::Failed,
            ..pct_out(25.0)
        };
        assert_eq!(heat_count(cx, ColumnId::Cpu, failed, HEAT_ON), 0);
    }

    fn bounds(
        cx: &mut VisualTestContext,
        selector: &'static str,
    ) -> Option<gpui_kit::Bounds<Pixels>> {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds(selector)
    }

    /// The selector of the live text cell for `col`.
    fn text_selector(col: ColumnId) -> &'static str {
        match kind(process_column(col)) {
            ColumnKind::Numeric => "TlCell-numeric",
            ColumnKind::Text => "TlCell-text",
            ColumnKind::Path => "TlCell-path",
        }
    }

    #[gpui_kit::test]
    fn validity_glyph_per_state(cx: &mut TestAppContext) {
        let all = [
            Validity::Valid,
            Validity::WarmingUp,
            Validity::Stale,
            Validity::Restricted,
            Validity::Unsupported,
            Validity::Missing,
            Validity::Failed,
        ];
        let (view, vcx) = open(
            cx,
            Host {
                out: pct_out(3.3),
                text: "3.3%".into(),
                col: ColumnId::Cpu,
                st: PaintState::default(),
            },
        );
        for col in [ColumnId::Cpu, ColumnId::ImagePath] {
            for validity in all {
                view.update(vcx, |host, cx| {
                    host.col = col;
                    host.out.validity = validity;
                    cx.notify();
                });
                vcx.run_until_parked();
                let shows_text = matches!(validity, Validity::Valid | Validity::Stale);
                let text = bounds(vcx, text_selector(col));
                let glyph = bounds(vcx, "TlCell-glyph");
                assert_eq!(text.is_some(), shows_text, "{col:?} {validity:?}");
                assert_eq!(glyph.is_some(), !shows_text, "{col:?} {validity:?}");
                let cell = text.or(glyph).expect("cell drawn");
                assert_eq!(cell.size.width, px(120.));
                let lock = bounds(vcx, "TlValidityGlyph-lock");
                assert_eq!(
                    lock.is_some(),
                    validity == Validity::Restricted,
                    "{col:?} {validity:?}"
                );
                if let Some(lock) = lock {
                    assert_eq!(lock.size.width, px(12.), "the lock is 12 px");
                    if col == ColumnId::Cpu {
                        assert_eq!(lock.right(), cell.right() - px(8.), "right like CPU");
                    } else {
                        assert_eq!(lock.left(), cell.left() + px(8.), "left like Image path");
                    }
                }
            }
        }
    }

    #[gpui_kit::test]
    fn stale_text_dimmed_60(cx: &mut TestAppContext) {
        // The table dims a STALE row's metric cells as a whole, once.
        assert_eq!(STALE_OPACITY, 0.6);
        let stale = RowFlags {
            bits: RowFlags::STALE,
            ..RowFlags::default()
        };
        assert_eq!(metric_cell_opacity(stale), STALE_OPACITY);
        assert_eq!(metric_cell_opacity(RowFlags::default()), 1.0);

        // A Stale value still draws its text; a WarmingUp cell draws its glyph, never dimmed,
        // with the setting on.
        let (view, vcx) = open(
            cx,
            Host {
                out: CellOut {
                    validity: Validity::Stale,
                    ..pct_out(3.3)
                },
                text: "3.3%".into(),
                col: ColumnId::Cpu,
                st: DIM_ON,
            },
        );
        assert!(
            bounds(vcx, "TlCell-numeric").is_some(),
            "Stale shows its value"
        );
        view.update(vcx, |host, cx| {
            host.out = CellOut {
                validity: Validity::WarmingUp,
                ..pct_out(0.0)
            };
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(bounds(vcx, "TlCell-glyph").is_some(), "the glyph path");
        assert!(bounds(vcx, "TlCell-numeric").is_none());
    }

    fn arc_out(text: &str) -> CellOut {
        CellOut {
            arc: Some(Arc::from(text)),
            validity: Validity::Valid,
            ..CellOut::default()
        }
    }

    #[gpui_kit::test]
    fn deleted_suffix_danger_colour(cx: &mut TestAppContext) {
        let theme = theme(cx);
        let text = "/usr/bin/foo (deleted)";
        let suffix = cell_suffix(ColumnId::ImagePath, &arc_out(text), &theme).expect("suffix");
        assert_eq!(&text[text.len() - suffix.len..], DELETED_SUFFIX);
        assert_eq!(suffix.color, theme.status.danger);
        assert_eq!(suffix.selected_color, theme.status.danger);
        assert!(cell_suffix(ColumnId::ImagePath, &arc_out("/usr/bin/foo"), &theme).is_none());
        // Only Image path: a name that ends in " (deleted)" is plain text.
        assert!(cell_suffix(ColumnId::Name, &arc_out(text), &theme).is_none());

        // The live text cell draws it.
        let (_, vcx) = open(
            cx,
            Host {
                out: CellOut {
                    suffix: Some(suffix),
                    ..arc_out(text)
                },
                text: text.into(),
                col: ColumnId::ImagePath,
                st: PaintState::default(),
            },
        );
        assert!(bounds(vcx, "TlCell-text").is_some());
    }

    /// A PID cell's bits: the PID, and N of " +N" in the high half.
    fn pid_out(pid: u32, plus: u32) -> CellOut {
        CellOut {
            bits: u64::from(pid) | u64::from(plus) << 32,
            validity: Validity::Valid,
            ..CellOut::default()
        }
    }

    #[gpui_kit::test]
    fn pid_plus_and_row_state_colours(cx: &mut TestAppContext) {
        let theme = theme(cx);
        let s = cell_suffix(ColumnId::Pid, &pid_out(11528, 18), &theme).expect("+18");
        assert_eq!(
            s,
            CellSuffix {
                len: " +18".len(),
                color: theme.text_secondary,
                selected_color: theme.text_selected_secondary,
            }
        );
        for (plus, text) in [(1, " +1"), (9, " +9"), (10, " +10"), (1000, " +1000")] {
            let s = cell_suffix(ColumnId::Pid, &pid_out(7, plus), &theme).expect("suffix");
            assert_eq!(s.len, text.len(), "{plus}");
        }
        assert!(cell_suffix(ColumnId::Pid, &pid_out(11528, 0), &theme).is_none());
        let warming = CellOut {
            validity: Validity::WarmingUp,
            ..pid_out(11528, 18)
        };
        assert!(cell_suffix(ColumnId::Pid, &warming, &theme).is_none());

        // An exited row draws all its text in `text_secondary`; the suffix is secondary too.
        let exited = CellOut {
            text_color: Some(theme.text_secondary),
            ..pid_out(11528, 18)
        };
        let ctx = cx.update(|cx| *metrics(cx));
        let paint = PaintCtx {
            m: &ctx,
            theme: &theme,
            selected: false,
            width: px(120.),
        };
        assert_eq!(cell_text_color(&exited, &paint), Some(theme.text_secondary));

        let (_, vcx) = open(
            cx,
            Host {
                out: CellOut {
                    suffix: Some(s),
                    ..pid_out(11528, 18)
                },
                text: "11528 +18".into(),
                col: ColumnId::Pid,
                st: PaintState::default(),
            },
        );
        assert!(bounds(vcx, "TlCell-numeric").is_some());
    }

    #[gpui_kit::test]
    fn cpu_text_right_aligned_tnum(cx: &mut TestAppContext) {
        assert_eq!(process_column(ColumnId::Cpu).align(), ColumnAlign::Right);
        assert_eq!(
            process_column(ColumnId::ImagePath).align(),
            ColumnAlign::Left
        );
        let (_, vcx) = open(
            cx,
            Host {
                out: pct_out(3.3),
                text: "3.3%".into(),
                col: ColumnId::Cpu,
                st: PaintState::default(),
            },
        );
        let cell = bounds(vcx, "TlCell-numeric").expect("numeric cell");
        assert_eq!(cell.size.width, px(120.));
    }

    const DIM_ON: PaintState = PaintState {
        selected: false,
        exited: false,
        heat_on: false,
        heat_intensity_pct: 0,
        dim_zero: true,
        subtree: false,
    };

    /// The live text alpha of `out` in `col` under `st`, as a share of the row's primary
    /// colour: `ProcessRowCells::fill` sets `dim`, the table applies `cell_text_color`.
    fn main_alpha(cx: &mut TestAppContext, out: &CellOut, col: ColumnId, st: PaintState) -> f32 {
        let t = theme(cx);
        let m = cx.update(|cx| *metrics(cx));
        let ctx = PaintCtx {
            m: &m,
            theme: &t,
            selected: st.selected,
            width: px(120.),
        };
        let filled = CellOut {
            bits: out.bits,
            validity: out.validity,
            dim: dims_zero(out, process_column(col), st),
            ..CellOut::default()
        };
        cell_text_color(&filled, &ctx).map_or(1.0, |c| c.a / ctx.primary().a)
    }

    fn int_out(value: u64) -> CellOut {
        CellOut {
            bits: value,
            validity: Validity::Valid,
            ..CellOut::default()
        }
    }

    #[gpui_kit::test]
    fn dim_zero_only_for_measured_zero(cx: &mut TestAppContext) {
        let zero = pct_out(0.0);
        assert!(close(
            main_alpha(cx, &zero, ColumnId::Cpu, DIM_ON),
            DIM_OPACITY
        ));
        assert_eq!(DIM_OPACITY, 0.3);
        // Every dimmed kind: Percent, MemorySize, DiskRate, NetRate, Count, BytesCumulative.
        for col in [
            ColumnId::Memory,
            ColumnId::Disk,
            ColumnId::Network,
            ColumnId::Threads,
        ] {
            assert!(
                dims_zero(&int_out(0), process_column(col), DIM_ON),
                "{col:?} zero dims"
            );
        }
        // Not zero, not Valid, or not a measured kind: full opacity.
        assert!(close(
            main_alpha(cx, &pct_out(0.1), ColumnId::Cpu, DIM_ON),
            1.0
        ));
        for validity in [Validity::WarmingUp, Validity::Stale, Validity::Restricted] {
            let out = CellOut {
                validity,
                ..pct_out(0.0)
            };
            assert!(
                !dims_zero(&out, process_column(ColumnId::Cpu), DIM_ON),
                "{validity:?} is not dimmed"
            );
        }
        assert!(
            !dims_zero(&int_out(0), process_column(ColumnId::Nice), DIM_ON),
            "Nice 0 is an id-like value"
        );
        assert!(!dims_zero(
            &int_out(0),
            process_column(ColumnId::Pid),
            DIM_ON
        ));
        assert!(
            !dims_zero(&int_out(0), process_column(ColumnId::User), DIM_ON),
            "text columns never dim"
        );
        // A selected row dims its own colour.
        let selected = PaintState {
            selected: true,
            ..DIM_ON
        };
        assert!(close(
            main_alpha(cx, &zero, ColumnId::Cpu, selected),
            DIM_OPACITY
        ));
    }

    #[gpui_kit::test]
    fn dim_zero_off_by_default(cx: &mut TestAppContext) {
        assert!(!Settings::default().tables.dim_zero_values);
        assert!(!PaintState::default().dim_zero);
        let off = PaintState {
            dim_zero: false,
            ..DIM_ON
        };
        assert!(close(
            main_alpha(cx, &pct_out(0.0), ColumnId::Cpu, off),
            1.0
        ));
    }

    /// The Σ prefix (UI-PRC-002, pdu.33): drawn before the value of a Σ column on a row with a
    /// subtree total; never on a row without one or on glyph cells.
    #[gpui_kit::test]
    fn subtree_prefix_drawn_before_the_value(cx: &mut TestAppContext) {
        let t = theme(cx);
        let prefixed = |out: CellOut| CellOut {
            prefix: Some((SharedString::new_static(SUBTREE_PREFIX), t.text_secondary)),
            ..out
        };
        let (view, vcx) = open(
            cx,
            Host {
                out: prefixed(pct_out(12.5)),
                text: "12.5%".into(),
                col: ColumnId::Cpu,
                st: PaintState::default(),
            },
        );
        let cell = bounds(vcx, "TlCell-numeric").expect("numeric cell");
        let prefix = bounds(vcx, "TlCell-prefix").expect("the prefix");
        assert!(prefix.size.width > px(0.));
        assert!(prefix.left() >= cell.left() && prefix.right() < cell.right() - px(8.));

        let mut set = |out: CellOut| {
            view.update(vcx, |h, cx| {
                h.out = out;
                cx.notify();
            });
            vcx.run_until_parked();
            bounds(vcx, "TlCell-prefix")
        };
        assert!(
            set(pct_out(3.3)).is_none(),
            "no prefix without a subtree total"
        );
        let glyph = prefixed(CellOut {
            validity: Validity::WarmingUp,
            ..pct_out(0.0)
        });
        assert!(set(glyph).is_none(), "no prefix on a glyph");
    }
}

// E22-005: the process Name cell. The table's chevron hit area, its toggle and the group header
// row band are tested in ui_tree_table (`mouse_selection_rules`, group header tests).
mod name_cell {
    use std::sync::Arc;

    use gpui_kit::assets::IconName;
    use gpui_kit::{
        Bounds, Context, IntoElement, ParentElement, Pixels, Render, SharedString, Styled,
        TestAppContext, VisualTestContext, Window, div, point, px, size,
    };
    use proptest::prelude::*;
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::processes::delegate::sibling_pass;
    use tl_ui::pages::processes::name_cell::{
        NameCellData, NameIcon, NameWords, StateGlyph, icon_slot, name_cell, name_flags,
        render_group_header, render_name_cell, row_icon,
    };
    use tl_ui::table::cells::{CellCtx, IconSourceCache, guide_quads, name_look};
    use tl_ui::table::tree_table::{IconSlot, RowFlags, RowKind, STALE_OPACITY};
    use tl_ui::theme::{current, metrics};

    fn data(kind: RowKind, depth: u8, expandable: Option<bool>) -> NameCellData {
        NameCellData {
            kind,
            depth,
            expandable,
            icon: NameIcon::Lucide(IconName::Binary),
            primary: "bash".into(),
            count_run: None,
            state_glyph: None,
            exited: false,
        }
    }

    fn words() -> NameWords {
        NameWords::from_fn(SharedString::new_static)
    }

    enum Cell {
        Name(fn() -> NameCellData),
        Group(&'static str, bool),
    }

    struct Host {
        cell: Cell,
        icons: IconSourceCache,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = current(cx).clone();
            let m = *metrics(cx);
            let ctx = CellCtx {
                m: &m,
                theme: &theme,
                selected: false,
                width: px(400.),
            };
            let cell = match &self.cell {
                Cell::Name(make) => render_name_cell(
                    make(),
                    7,
                    &ctx,
                    &words(),
                    false,
                    &mut self.icons,
                    window,
                    cx,
                ),
                Cell::Group(label, expanded) => {
                    render_group_header(SharedString::new_static(label), *expanded, &ctx)
                }
            };
            div().w(px(400.)).h(m.row_height).child(cell)
        }
    }

    fn open(cx: &mut TestAppContext, cell: Cell) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
        });
        let (_, vcx) = cx.add_window_view(|_, _| Host {
            cell,
            icons: IconSourceCache::new(),
        });
        vcx.run_until_parked();
        vcx
    }

    fn bounds(vcx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        vcx.debug_bounds(selector).expect(selector)
    }

    /// Text x relative to the cell.
    fn text_x(cx: &mut TestAppContext, make: fn() -> NameCellData) -> Pixels {
        let vcx = open(cx, Cell::Name(make));
        bounds(vcx, "TlNameCell-primary").left() - bounds(vcx, "TlNameCell").left()
    }

    #[gpui_kit::test]
    fn name_cell_indent_by_depth_0(cx: &mut TestAppContext) {
        // R-190: 20 + depth × 20 + 6 (chevron box), chevron 16, gap 6, icon 16, gap 6.
        let x = text_x(cx, || data(RowKind::Process, 0, Some(false)));
        assert_eq!(x, px(20. + 6. + 16. + 6. + 16. + 6.));
    }

    #[gpui_kit::test]
    fn name_cell_indent_by_depth_1(cx: &mut TestAppContext) {
        let x = text_x(cx, || data(RowKind::Process, 1, Some(true)));
        assert_eq!(x, px(20. + 20. + 6. + 16. + 6. + 16. + 6.));
    }

    #[gpui_kit::test]
    fn name_cell_indent_by_depth_2(cx: &mut TestAppContext) {
        let x = text_x(cx, || data(RowKind::Process, 2, Some(false)));
        assert_eq!(x, px(20. + 40. + 6. + 16. + 6. + 16. + 6.));
    }

    #[gpui_kit::test]
    fn name_cell_app_text_at_78(cx: &mut TestAppContext) {
        // The 24 px icon moves the text 8 px right of a 16 px icon row (R-190: 78 at depth 0).
        let x = text_x(cx, || NameCellData {
            icon: NameIcon::File(Arc::from("/nonexistent/tasklens/app.png")),
            ..data(RowKind::App, 0, Some(false))
        });
        assert_eq!(x, px(78.));
    }

    #[gpui_kit::test]
    fn chevron_width_reserved_on_leaf(cx: &mut TestAppContext) {
        // No chevron is drawn, but the text sits where an expandable sibling's does.
        let leaf = text_x(cx, || data(RowKind::Process, 1, None));
        assert_eq!(leaf, px(20. + 20. + 6. + 16. + 6. + 16. + 6.));
    }

    #[test]
    fn name_cell_icon_sizes_per_kind() {
        assert_eq!(icon_slot(RowKind::App), IconSlot::Large);
        for kind in [RowKind::Unit, RowKind::Process, RowKind::KernelThread] {
            assert_eq!(icon_slot(kind), IconSlot::Small);
        }
        let unit = name_cell(data(RowKind::Unit, 0, None), &words());
        assert_eq!(unit.icon_size, IconSlot::Small);
        assert!(matches!(unit.icon, NameIcon::Lucide(IconName::Cog)));
        let kthread = name_cell(data(RowKind::KernelThread, 1, None), &words());
        assert!(matches!(kthread.icon, NameIcon::Lucide(IconName::Cpu)));
        let file = NameIcon::File(Arc::from("/usr/share/icons/a.png"));
        assert!(matches!(row_icon(RowKind::App, file), NameIcon::File(_)));
        let app = name_cell(data(RowKind::App, 0, Some(false)), &words());
        assert_eq!(app.icon_size, IconSlot::Large);
        assert!(app.highlight_fffd && app.secondary.is_none() && app.trailing.is_none());
    }

    #[gpui_kit::test]
    fn state_glyph_pause_with_tooltip(cx: &mut TestAppContext) {
        let w = cx.update(|cx| {
            init_strings(cx, "en-US");
            NameWords::new(cx)
        });
        assert_eq!(w.suspended.as_ref(), "Suspended");
        assert_eq!(w.frozen.as_ref(), "Frozen");
        let cell = name_cell(
            NameCellData {
                state_glyph: Some(StateGlyph::Suspended),
                ..data(RowKind::Process, 0, None)
            },
            &w,
        );
        assert_eq!(cell.glyphs.len(), 1);
        assert_eq!(cell.glyphs[0].0, IconName::Pause);
        assert_eq!(cell.glyphs[0].1.as_ref(), "Suspended");
        let frozen = name_cell(
            NameCellData {
                state_glyph: Some(StateGlyph::Frozen),
                ..data(RowKind::Process, 0, None)
            },
            &w,
        );
        assert_eq!(frozen.glyphs[0].0, IconName::Snowflake);
        assert!(
            name_cell(data(RowKind::Process, 0, None), &w)
                .glyphs
                .is_empty()
        );
    }

    #[test]
    fn exited_sets_the_exited_flag() {
        let gone = NameCellData {
            exited: true,
            ..data(RowKind::Process, 0, None)
        };
        assert!(name_flags(&gone).has(RowFlags::EXITED));
        assert!(!name_flags(&data(RowKind::Process, 0, None)).has(RowFlags::EXITED));
    }

    #[gpui_kit::test]
    fn exited_icon_60_and_secondary_text(cx: &mut TestAppContext) {
        let (theme, m) = cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            (current(cx).clone(), *metrics(cx))
        });
        let gone = name_flags(&NameCellData {
            exited: true,
            ..data(RowKind::Process, 0, None)
        });
        let live = name_flags(&data(RowKind::Process, 0, None));
        for selected in [false, true] {
            let ctx = CellCtx {
                m: &m,
                theme: &theme,
                selected,
                width: px(400.),
            };
            assert_eq!(name_look(gone, &ctx), (ctx.secondary(), STALE_OPACITY));
            assert_eq!(name_look(live, &ctx), (ctx.primary(), 1.0));
        }
        // The exited cell still renders through the same painter.
        let vcx = open(
            cx,
            Cell::Name(|| NameCellData {
                exited: true,
                ..data(RowKind::Process, 1, Some(true))
            }),
        );
        let x = bounds(vcx, "TlNameCell-primary").left() - bounds(vcx, "TlNameCell").left();
        assert_eq!(
            x,
            px(20. + 20. + 6. + 16. + 6. + 16. + 6.),
            "chevron unchanged"
        );
    }

    #[gpui_kit::test]
    fn group_header_label_at_30(cx: &mut TestAppContext) {
        let vcx = open(cx, Cell::Group("Apps (11)", true));
        let cell = bounds(vcx, "TlGroupHeaderCell");
        let label = bounds(vcx, "TlGroupHeaderCell-label");
        assert_eq!(label.left() - cell.left(), px(30.));
    }

    // ---- Tree connector lines (bead pdu.14, PROC-017, UI-PRC-003) ----

    const ROW_H: f32 = 28.;

    fn quads(depth: u8, flags: RowFlags) -> Vec<Bounds<Pixels>> {
        guide_quads(depth, flags, point(px(0.), px(0.)), px(ROW_H), 1.0, 1.0).into_vec()
    }

    fn vline(x: f32, top: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(top)), size(px(1.), px(h)))
    }

    fn hline(x: f32, w: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(ROW_H / 2.)), size(px(w), px(1.)))
    }

    #[test]
    fn connector_quads_for_depth_and_siblings() {
        // Depth 2, not the last sibling, the level-0 line continues. R-190: guides at
        // 20 + 20k + 14, the depth-2 chevron at 20 + 40 + 6.
        let open_row = RowFlags {
            bits: 0,
            ancestor_continues: 0b1,
        };
        assert_eq!(
            quads(2, open_row),
            [
                vline(34., 0., ROW_H),
                vline(54., 0., ROW_H),
                hline(54., 12.)
            ]
        );

        // The last sibling: its own line stops at the row centre (1 px past it, so the corner
        // closes).
        let last = RowFlags {
            bits: RowFlags::LAST_SIBLING,
            ancestor_continues: 0,
        };
        assert_eq!(
            quads(2, last),
            [vline(54., 0., ROW_H / 2. + 1.), hline(54., 12.)]
        );

        // At most depth + 1 quads, and never a line at the row's own elbow level from the bits.
        let all = RowFlags {
            bits: 0,
            ancestor_continues: u64::MAX,
        };
        assert_eq!(quads(4, all).len(), 3 + 2);

        // Group header rows are depth 0 with no bits: nothing.
        assert!(quads(0, RowFlags::default()).is_empty());

        // Under a group header the level-0 line hangs from the header chevron centre, x 16.
        let in_group = RowFlags {
            bits: RowFlags::IN_GROUP | RowFlags::LAST_SIBLING,
            ancestor_continues: 0,
        };
        assert_eq!(
            quads(1, in_group),
            [vline(16., 0., ROW_H / 2. + 1.), hline(16., 30.)]
        );
        let deeper = RowFlags {
            bits: RowFlags::IN_GROUP,
            ancestor_continues: 0b1,
        };
        assert_eq!(
            quads(2, deeper),
            [
                vline(16., 0., ROW_H),
                vline(54., 0., ROW_H),
                hline(54., 12.)
            ]
        );
    }

    #[test]
    fn connector_quads_snap_to_physical_pixels() {
        // At device scale 1.5 a line is 1 physical px wide and every edge sits on the grid.
        let flags = RowFlags {
            bits: 0,
            ancestor_continues: 0b1,
        };
        let q = guide_quads(2, flags, point(px(0.3), px(0.)), px(ROW_H), 1.0, 1.5);
        for b in &q {
            for v in [b.origin.x, b.origin.y] {
                let phys = f32::from(v) * 1.5;
                assert!((phys - phys.round()).abs() < 1e-3, "{b:?}");
            }
        }
        assert!((f32::from(q[0].size.width) * 1.5 - 1.).abs() < 1e-3);
    }

    /// The naive O(n²) reading of the sibling flags.
    fn naive(depths: &[u8]) -> Vec<(bool, u64)> {
        let last = |i: usize| {
            let d = depths[i];
            !depths[i + 1..]
                .iter()
                .take_while(|&&x| x >= d)
                .any(|&x| x == d)
        };
        (0..depths.len())
            .map(|i| {
                let d = depths[i];
                let mut bits = 0u64;
                for k in 0..usize::from(d.saturating_sub(1)).min(64) {
                    // The ancestor at depth k + 1: the nearest earlier row at that depth.
                    let a = (0..i).rev().find(|&j| usize::from(depths[j]) == k + 1);
                    if a.is_some_and(|a| !last(a)) {
                        bits |= 1 << k;
                    }
                }
                (last(i), bits)
            })
            .collect()
    }

    fn pass(depths: &[u8]) -> Vec<(bool, u64)> {
        let mut out = Vec::new();
        sibling_pass(depths.iter().copied(), &mut out);
        out
    }

    #[test]
    fn sibling_pass_flags() {
        // A(d0), B(d1), C(d2), D(d1): D follows B, so B is not last and the level-0 line runs
        // on through C. C and D are last.
        let f = pass(&[0, 1, 2, 1]);
        assert_eq!(f, [(true, 0), (false, 0), (true, 0b1), (true, 0)]);
        assert_eq!(f, naive(&[0, 1, 2, 1]));
        // A shallower row ends the run: the second root is not B's sibling.
        assert_eq!(
            pass(&[0, 1, 0, 1]),
            [(false, 0), (true, 0), (true, 0), (true, 0)]
        );
        // Depths past 64 keep the 64 bits they have room for.
        let deep: Vec<u8> = (0..=70).chain([70, 1]).collect();
        assert_eq!(pass(&deep), naive(&deep));
        // The scratch vector is refilled, not appended to.
        let mut out = vec![(false, 7); 9];
        sibling_pass([0u8, 0].into_iter(), &mut out);
        assert_eq!(out, [(false, 0), (true, 0)]);
    }

    /// A random forest of up to 1,000 rows in display order: each depth is at most one more than
    /// the previous row's.
    fn forest() -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec((any::<bool>(), 0u8..8), 1..1000).prop_map(|steps| {
            let mut depths = Vec::with_capacity(steps.len());
            let mut d = 0u8;
            for (i, (down, up)) in steps.into_iter().enumerate() {
                d = if i == 0 {
                    0
                } else if down {
                    d.saturating_add(1).min(80)
                } else {
                    d.saturating_sub(up)
                };
                depths.push(d);
            }
            depths
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn sibling_pass_matches_naive(depths in forest()) {
            prop_assert_eq!(pass(&depths), naive(&depths));
        }
    }
}

// ---- Group row fields and sums (bead 8q0.38) ----

mod group_fields {
    use std::sync::Arc;

    use tl_model::actions::SchedPolicy;
    use tl_model::ids::{ColumnId, GroupKey, NodeId};
    use tl_model::process::{GroupKind, GroupRow, GroupRowKind, ProcessExtras};
    use tl_model::settings::Settings;
    use tl_model::system::CpuSet;
    use tl_model::table::{Manager, ProcessTableView, UnitRef};
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::pages::processes::cells::{
        CellCtx, CellWords, GroupMembers, RowRef, fill_cell, process_column,
    };
    use tl_ui::table::cells::CellOut;
    use tl_ui::table::tree_table::RowKind;

    fn group(kind: GroupRowKind) -> GroupRow {
        GroupRow {
            node: NodeId::Group(GroupKey(7)),
            key: GroupKey(7),
            kind,
            class: GroupKind::Background,
            name: Arc::from("row"),
            icon: None,
            members: 2,
            pids: 2,
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
            threads: 2,
            cpu_kernel_pct: 1.5,
            cpu_user_pct: 2.25,
            cpu_relative_pct: 60.0,
            utime_ticks: 100 * 60,
            stime_ticks: 100 * 3_600,
            commit_bytes: 2 << 20,
            page_faults: 1_234,
            page_faults_delta: 56,
            major_faults: 7,
            handles: Some(42),
            desktop_path: None,
            description: None,
            user: None,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    /// One entry so `CellCtx.extras` is not empty; group rows never read it by index.
    fn extras() -> ProcessExtras {
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
            exe_name: Arc::from("x"),
            exe_path: Arc::from("/usr/bin/x"),
            exe_deleted: false,
            unit: None,
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
        }
    }

    struct Env {
        view: ProcessTableView,
        settings: Settings,
        words: CellWords,
        members: GroupMembers,
        online: CpuSet,
        /// False gives an empty `CellCtx.extras`: the extras have not arrived.
        extras_arrived: bool,
    }

    impl Env {
        fn new() -> Self {
            Self {
                view: ProcessTableView::default(),
                settings: Settings::default(),
                words: CellWords::from_fn(Arc::from),
                members: GroupMembers::default(),
                online: CpuSet::empty(),
                extras_arrived: true,
            }
        }

        fn cell(&self, g: &GroupRow, kind: RowKind, id: ColumnId) -> CellOut {
            let extras = if self.extras_arrived {
                vec![extras()]
            } else {
                Vec::new()
            };
            let ctx = CellCtx {
                view: &self.view,
                records: &[],
                extras: &extras,
                settings: &self.settings,
                words: &self.words,
                members: &self.members,
                ncpu: 32,
                online: &self.online,
                mem_total: 64 << 30,
                vram_total: 0,
                boot_time_s: 0,
                clk_tck: 100,
            };
            let mut out = CellOut {
                want_text: true,
                ..CellOut::default()
            };
            fill_cell(RowRef::Group(g, kind), process_column(id), &ctx, &mut out);
            out
        }
    }

    fn shown(out: &CellOut) -> String {
        out.arc
            .as_deref()
            .map_or_else(|| out.text.clone(), str::to_owned)
    }

    #[test]
    fn unit_row_prefers_its_own_description_and_user() {
        let env = Env::new();
        let unit = GroupRow {
            node: NodeId::Unit(UnitRef {
                manager: Manager::System,
                name: Arc::from("sshd.service"),
            }),
            description: Some(Arc::from("OpenSSH Daemon")),
            user: Some(Arc::from("root")),
            ..group(GroupRowKind::Unit)
        };
        let desc = env.cell(&unit, RowKind::Unit, ColumnId::Description);
        assert_eq!(shown(&desc), "OpenSSH Daemon");
        assert!(Arc::ptr_eq(
            desc.arc.as_ref().expect("an Arc cell"),
            unit.description.as_ref().expect("set above")
        ));
        assert_eq!(
            shown(&env.cell(&unit, RowKind::Unit, ColumnId::User)),
            "root"
        );
    }

    #[test]
    fn app_row_shows_desktop_path() {
        let env = Env::new();
        let app = GroupRow {
            desktop_path: Some(Arc::from("/usr/share/applications/firefox.desktop")),
            ..group(GroupRowKind::App)
        };
        let out = env.cell(&app, RowKind::App, ColumnId::ImagePath);
        assert_eq!(out.validity, Validity::Valid);
        assert_eq!(shown(&out), "/usr/share/applications/firefox.desktop");
        // Without a desktop path the cell stays Unsupported.
        let none = env.cell(&group(GroupRowKind::App), RowKind::App, ColumnId::ImagePath);
        assert_eq!(none.validity, Validity::Unsupported);
    }

    #[test]
    fn group_rows_show_summed_columns() {
        let env = Env::new();
        let g = group(GroupRowKind::App);
        let text = |id| shown(&env.cell(&g, RowKind::App, id));
        for id in [
            ColumnId::CpuKernel,
            ColumnId::CpuUser,
            ColumnId::CpuRelative,
            ColumnId::CpuTime,
            ColumnId::CpuTimeKernel,
            ColumnId::CpuTimeUser,
            ColumnId::Commit,
            ColumnId::PageFaults,
            ColumnId::PageFaultsDelta,
            ColumnId::MajorFaults,
            ColumnId::Handles,
        ] {
            let out = env.cell(&g, RowKind::App, id);
            assert_eq!(out.validity, Validity::Valid, "{}", id.as_str());
            assert!(!shown(&out).is_empty(), "{} is blank", id.as_str());
        }
        assert_eq!(text(ColumnId::CpuTimeUser), "0:01:00");
        assert_eq!(text(ColumnId::CpuTime), "1:01:00");
        assert_eq!(text(ColumnId::Handles), "42");
        assert_eq!(text(ColumnId::MajorFaults), "7");
    }

    #[test]
    fn summed_columns_warm_up_and_restrict() {
        let mut env = Env::new();
        let g = GroupRow {
            handles: None,
            ..group(GroupRowKind::Unit)
        };
        assert_eq!(
            env.cell(&g, RowKind::Unit, ColumnId::Handles).validity,
            Validity::Restricted
        );
        env.extras_arrived = false;
        for id in [ColumnId::CpuKernel, ColumnId::Commit, ColumnId::Handles] {
            assert_eq!(
                env.cell(&g, RowKind::Unit, id).validity,
                Validity::WarmingUp,
                "{}",
                id.as_str()
            );
        }
        // Pseudo rows have no extras sums: n/a (PROC-012).
        let pseudo = group(GroupRowKind::Pseudo);
        let out = Env::new().cell(&pseudo, RowKind::Pseudo, ColumnId::Commit);
        assert_eq!(out.validity, Validity::Unsupported);
    }
}

// ---- Header line 2 system totals (E22-010) ----

mod header_totals {
    use tl_model::ids::ColumnId;
    use tl_model::settings::{NetworkUnit, Settings};
    use tl_model::table::{TableTotals, Total};
    use tl_model::validity::Validity;
    use tl_ui::pages::processes::cells::process_column;
    use tl_ui::pages::processes::header_totals::header_total;
    use tl_ui::table::cells::CellOut;

    fn valid<T>(value: T) -> Total<T> {
        Total {
            value,
            validity: Validity::Valid,
        }
    }

    fn unsupported<T: Default>() -> Total<T> {
        Total {
            value: T::default(),
            validity: Validity::Unsupported,
        }
    }

    fn sample() -> TableTotals {
        let mut gpu_adapter_pct = [unsupported(); 16];
        gpu_adapter_pct[0] = valid(20.0);
        gpu_adapter_pct[1] = valid(80.0);
        TableTotals {
            cpu_pct: valid(11.94),
            memory_pct: valid(54.9),
            swap_pct: valid(3.26),
            disk_pct: valid(7.0),
            net_bps: valid(287_500.0),
            net_link_bps: None,
            gpu_pct: valid(80.0),
            gpu_adapter_pct,
            vram_pct: valid(37.5),
            threads: valid(2_345),
            handles: valid(41_234),
        }
    }

    fn bits_settings() -> Settings {
        let mut s = Settings::default();
        s.units.network = NetworkUnit::Bits;
        s
    }

    /// Runs `header_total` the way the table does: first for bits, then again for the text.
    fn total(id: ColumnId, totals: &TableTotals, settings: &Settings) -> Option<CellOut> {
        let col = process_column(id);
        let mut probe = CellOut::default();
        if !header_total(col, totals, settings, &mut probe) {
            return None;
        }
        assert!(probe.text.is_empty(), "text written without want_text");
        let mut out = CellOut {
            want_text: true,
            ..CellOut::default()
        };
        assert!(header_total(col, totals, settings, &mut out));
        assert_eq!((out.bits, out.validity), (probe.bits, probe.validity));
        Some(out)
    }

    fn text(id: ColumnId, totals: &TableTotals) -> String {
        let out = total(id, totals, &bits_settings()).expect("column has a total");
        assert_eq!(out.validity, Validity::Valid, "{}", id.as_str());
        out.text
    }

    #[test]
    fn header_line2_blank_for_text_columns() {
        let totals = sample();
        let s = bits_settings();
        for id in [
            ColumnId::Name,
            ColumnId::User,
            ColumnId::Pid,
            ColumnId::Description,
            ColumnId::Power,
            ColumnId::Cmdline,
            ColumnId::State,
            ColumnId::CpuKernel,
            ColumnId::WsTotal,
            ColumnId::GpuMemShared,
        ] {
            let mut out = CellOut::default();
            assert!(
                !header_total(process_column(id), &totals, &s, &mut out),
                "{} has no line 2",
                id.as_str()
            );
        }
    }

    #[test]
    fn header_totals_formats() {
        let t = sample();
        assert_eq!(text(ColumnId::Cpu, &t), "11.9%");
        assert_eq!(text(ColumnId::Memory, &t), "54.9%");
        assert_eq!(text(ColumnId::Swap, &t), "3.3%");
        assert_eq!(text(ColumnId::Disk, &t), "7.0%");
        assert_eq!(text(ColumnId::Network, &t), "2.3 Mbps");
        assert_eq!(text(ColumnId::Gpu, &t), "80.0%");
        assert_eq!(text(ColumnId::Gpu0, &t), "20.0%");
        assert_eq!(text(ColumnId::Gpu1, &t), "80.0%");
        assert_eq!(text(ColumnId::GpuMemDedicated, &t), "37.5%");
        assert_eq!(text(ColumnId::Threads, &t), "2\u{2009}345");
        assert_eq!(text(ColumnId::Handles, &t), "41\u{2009}234");

        // Known link speed: throughput as a percent of the summed link speed.
        let linked = TableTotals {
            net_bps: valid(12_500_000.0),
            net_link_bps: Some(125_000_000),
            ..sample()
        };
        assert_eq!(text(ColumnId::Network, &linked), "10.0%");
        let s = bits_settings();
        let pct = total(ColumnId::Network, &linked, &s).unwrap();
        let rate = total(ColumnId::Network, &sample(), &s).unwrap();
        assert_ne!(pct.bits, rate.bits);

        // Bytes mode for the rate form.
        let mut bytes = Settings::default();
        bytes.units.network = NetworkUnit::Bytes;
        let out = total(ColumnId::Network, &t, &bytes).unwrap();
        assert_eq!(out.text, "0.3 MiB/s");

        // Same value, same bits: the table cache formats it once.
        let a = total(ColumnId::Cpu, &t, &s).unwrap();
        let b = total(ColumnId::Cpu, &sample(), &s).unwrap();
        assert_eq!(a.bits, b.bits);
        let changed = TableTotals {
            cpu_pct: valid(12.0),
            ..sample()
        };
        assert_ne!(total(ColumnId::Cpu, &changed, &s).unwrap().bits, a.bits);
    }

    #[test]
    fn header_totals_no_gpu_is_na() {
        let totals = TableTotals {
            gpu_pct: unsupported(),
            gpu_adapter_pct: [unsupported(); 16],
            vram_pct: unsupported(),
            ..sample()
        };
        let s = bits_settings();
        for id in [ColumnId::Gpu, ColumnId::GpuMemDedicated, ColumnId::Gpu0] {
            let out = total(id, &totals, &s).expect("GPU columns keep line 2");
            assert_eq!(out.validity, Validity::Unsupported, "{}", id.as_str());
            assert!(out.text.is_empty());
        }
        // An adapter past the last GPU is Unsupported too.
        let out = total(ColumnId::Gpu2, &sample(), &s).unwrap();
        assert_eq!(out.validity, Validity::Unsupported);
    }

    #[test]
    fn header_totals_warming_up_before_second_sample() {
        // `TableTotals::default()` is the state before the second sample: every total WarmingUp.
        let totals = TableTotals::default();
        let s = bits_settings();
        for id in [
            ColumnId::Cpu,
            ColumnId::Disk,
            ColumnId::Network,
            ColumnId::Gpu,
            ColumnId::Gpu3,
        ] {
            let out = total(id, &totals, &s).expect("column has a total");
            assert_eq!(out.validity, Validity::WarmingUp, "{}", id.as_str());
            assert!(out.text.is_empty());
        }
        let linked = TableTotals {
            net_link_bps: Some(125_000_000),
            ..TableTotals::default()
        };
        let out = total(ColumnId::Network, &linked, &s).unwrap();
        assert_eq!(out.validity, Validity::WarmingUp);
        assert!(out.text.is_empty());

        // Stale still shows the last value.
        let stale = TableTotals {
            cpu_pct: Total {
                value: 11.94,
                validity: Validity::Stale,
            },
            ..sample()
        };
        let out = total(ColumnId::Cpu, &stale, &s).unwrap();
        assert_eq!(
            (out.validity, out.text.as_str()),
            (Validity::Stale, "11.9%")
        );
    }
}

/// NEW and EXITED row highlight (PROC-043, UI-PRC-011): the tint timeline, the linger clamp,
/// exited row dimming, frame requests and Enter on an exited row. The table checks that fix the
/// clock or read toasts need `--features test-hooks`.
mod highlight {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{
        AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement,
        Render, Styled, TestAppContext, VisualTestContext, Window, div, px,
    };
    use tl_model::actions::SchedPolicy;
    use tl_model::ids::{ColumnId, NodeId, ProcessKey};
    use tl_model::process::{
        GroupKind, ProcFlags, ProcState, ProcessExtras, ProcessMeta, ProcessRecord,
    };
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, Snapshot};
    use tl_model::system::CpuSet;
    use tl_model::table::{ProcessTableView, TableRow};
    use tl_model::time::{BootId, TimeNs, now_boot_ns};
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::anim::AnimationDriver;
    use tl_ui::components::motion::ease_out_cubic;
    use tl_ui::pages::processes::cells::{
        TINT_ALPHA, TintKind, linger_ns, row_tint, tint_deadline,
    };
    use tl_ui::pages::processes::delegate::ProcessDelegate;
    use tl_ui::table::actions::OpenProperties;
    use tl_ui::table::cells::{CellCtx, CellOut, name_look};
    use tl_ui::table::tree_table::{RowFlags, STALE_OPACITY, TlTreeTable, TreeTableDelegate};
    use tl_ui::theme::{current, metrics};

    const MS: TimeNs = 1_000_000;

    #[test]
    fn highlight_alpha_timeline() {
        let since = 10_000 * MS;
        let at = |flags: ProcFlags, age_ms: u64, animations: bool| {
            row_tint(flags, since, since + age_ms * MS, 1000, animations, true)
        };
        let new = |age_ms, animations| at(ProcFlags::NEW, age_ms, animations);

        // 0.35 until L - 300 ms (L = 2000 ms at interval 1000 ms).
        for age in [0, 500, 1000, 1700] {
            assert_eq!(
                new(age, true),
                Some((TintKind::New, TINT_ALPHA)),
                "{age} ms"
            );
        }
        // Halfway through the fade: 0.35 × (1 - ease_out_cubic(150 / 300)).
        let (kind, alpha) = new(1850, true).expect("fading");
        assert_eq!(kind, TintKind::New);
        assert!((alpha - TINT_ALPHA * (1.0 - ease_out_cubic(0.5))).abs() < 1e-6);
        assert!(alpha > 0.0 && alpha < TINT_ALPHA);
        // The fade only goes down, and the tint is gone at L.
        let mut last = TINT_ALPHA;
        for age in (1700..2000).step_by(10) {
            let (_, a) = new(age, true).expect("inside L");
            assert!(a <= last, "{age} ms");
            last = a;
        }
        assert_eq!(new(2000, true), None);
        assert_eq!(new(2500, true), None);

        // Reduced motion: 0.35 to the end, then off at L.
        assert_eq!(new(1850, false), Some((TintKind::New, TINT_ALPHA)));
        assert_eq!(new(1999, false), Some((TintKind::New, TINT_ALPHA)));
        assert_eq!(new(2000, false), None);

        // EXITED is the danger tint on the same timeline; no flag or highlight off is none.
        assert_eq!(
            at(ProcFlags::EXITED, 500, true),
            Some((TintKind::Exited, TINT_ALPHA))
        );
        assert_eq!(at(ProcFlags::empty(), 500, true), None);
        assert_eq!(
            row_tint(ProcFlags::NEW, since, since + 500 * MS, 1000, true, false),
            None
        );

        // When the tint next changes: the fade start, every frame while fading, L without
        // animations, never once it is over.
        let next = |age_ms: u64, animations| {
            tint_deadline(
                ProcFlags::NEW,
                since,
                since + age_ms * MS,
                1000,
                animations,
                true,
            )
        };
        assert_eq!(next(500, true), Some(since + 1700 * MS));
        assert_eq!(next(1850, true), Some(since + 1850 * MS));
        assert_eq!(next(500, false), Some(since + 2000 * MS));
        assert_eq!(next(2000, true), None);
    }

    #[test]
    fn linger_clamped_1_to_3_s() {
        assert_eq!(linger_ns(250), 1000 * MS);
        assert_eq!(linger_ns(500), 1000 * MS);
        assert_eq!(linger_ns(750), 1500 * MS);
        assert_eq!(linger_ns(1000), 2000 * MS);
        assert_eq!(linger_ns(5000), 3000 * MS);
        assert_eq!(linger_ns(u32::MAX), 3000 * MS);
    }

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(0),
            pid,
        }
    }

    fn record(pid: u32, flags: ProcFlags) -> ProcessRecord {
        let a = |s: &str| Arc::<str>::from(s);
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: a("bash"),
                name: a("bash"),
                exe: None,
                cmdline: None,
                user: a("oystein"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a("bash"),
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

    fn extras(flag_since_ns: TimeNs) -> ProcessExtras {
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
            exe_name: Arc::from("bash"),
            exe_path: Arc::from("/usr/bin/bash"),
            exe_deleted: false,
            unit: None,
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns,
        }
    }

    /// Row 0: pid 10, NEW since `new_since`. Row 1: pid 20, EXITED since `exited_since`.
    /// Row 2: pid 30, no flag. A flat list.
    fn snapshot(new_since: TimeNs, exited_since: TimeNs) -> Arc<Snapshot> {
        let flags = [ProcFlags::NEW, ProcFlags::EXITED, ProcFlags::empty()];
        let since = [new_since, exited_since, 0];
        let row = |i: u32| TableRow {
            node: NodeId::Process(key(10 * (i + 1))),
            depth: 0,
            expanded: false,
            has_children: false,
            record: Some(i),
            group: None,
            descendants: 0,
            subtree: None,
        };
        let mut s = Snapshot::empty(BootId(0), 0);
        s.table = Arc::new(ProcessTableView {
            header: ComponentHeader {
                version: 1,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: (0..3).map(row).collect(),
            ..ProcessTableView::default()
        });
        s.processes = Arc::new(ProcessSnapshot {
            records: (0..3)
                .map(|i| record(10 * (i as u32 + 1), flags[i]))
                .collect(),
            extras: since.iter().map(|&t| extras(t)).collect(),
            ..ProcessSnapshot::default()
        });
        Arc::new(s)
    }

    struct Host {
        table: Entity<TlTreeTable<ProcessDelegate>>,
        opened: Rc<Cell<u32>>,
        _driver: Entity<AnimationDriver>,
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

    struct Rig {
        table: Entity<TlTreeTable<ProcessDelegate>>,
        driver: Entity<AnimationDriver>,
        /// Read by the Enter check, which needs `test-hooks`.
        #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
        opened: Rc<Cell<u32>>,
        #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
        toasts: Entity<tl_ui::components::toast::ToastQueue>,
        vcx: VisualTestContext,
    }

    fn redraw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// A table over [`snapshot`] in a window with an `AnimationDriver` and a toast queue.
    /// `prepare` runs on the delegate before the table's first frame.
    fn open(
        cx: &mut TestAppContext,
        snap: Arc<Snapshot>,
        prepare: impl FnOnce(&mut ProcessDelegate) + 'static,
    ) -> Rig {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::components::toast::init(cx);
            tl_ui::table::actions::bind_keys_table(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::i18n::init_strings(cx, "en-US");
        });
        let slot = Rc::new(std::cell::RefCell::new(None));
        let opened = Rc::new(Cell::new(0));
        let (out, hits) = (slot.clone(), opened.clone());
        let window = cx.add_window(move |window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            let mut d =
                ProcessDelegate::new(snap, &Settings::default(), &UiState::default(), window, cx);
            d.set_window(Some(window.window_handle()));
            prepare(&mut d);
            let table = cx.new(|cx| TlTreeTable::new(d, window, cx));
            *out.borrow_mut() = Some((table.clone(), driver.clone()));
            let host = cx.new(|_| Host {
                table,
                opened: hits,
                _driver: driver,
            });
            Root::new(host, window, cx)
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        let toasts = vcx.update(|window, cx| {
            cx.new(|cx| tl_ui::components::toast::ToastQueue::new(window, cx, px(40.)))
        });
        redraw(&mut vcx);
        let (table, driver) = slot.borrow_mut().take().expect("table created");
        vcx.update(|window, cx| table.read(cx).focus_handle().clone().focus(window, cx));
        redraw(&mut vcx);
        Rig {
            table,
            driver,
            opened,
            toasts,
            vcx,
        }
    }

    fn requested(rig: &mut Rig) -> u64 {
        let driver = rig.driver.clone();
        rig.vcx.read_entity(&driver, |d, _| d.stats.requested)
    }

    #[gpui_kit::test]
    fn exited_row_text_and_icon_dimmed(cx: &mut TestAppContext) {
        let now = now_boot_ns();
        let mut rig = open(cx, snapshot(now, now), |_| {});
        let table = rig.table.clone();
        rig.vcx.update(|_, cx| {
            let theme = current(cx).clone();
            let m = *metrics(cx);
            let t = table.read(cx);
            let text_color = |ix| {
                let mut out = CellOut::default();
                t.delegate.cell(ix, ColumnId::Pid, &mut out);
                out.text_color
            };
            // Metric cells: secondary text on the exited row only.
            assert_eq!(text_color(1), Some(theme.text_secondary));
            assert_eq!(text_color(0), None);
            assert_eq!(text_color(2), None);
            // Name cell: secondary text and the icon at 60 %.
            let ctx = CellCtx {
                m: &m,
                theme: &theme,
                selected: false,
                width: px(200.),
            };
            let flags = t.delegate.row_flags(1);
            assert!(flags.has(RowFlags::EXITED));
            assert_eq!(name_look(flags, &ctx), (theme.text_secondary, 0.6));
            assert_eq!(STALE_OPACITY, 0.6);
            assert_eq!(
                name_look(t.delegate.row_flags(0), &ctx),
                (theme.text_primary, 1.0)
            );
            assert!(t.delegate.name_cell(1).primary.as_ref() == "bash");
        });
    }

    #[gpui_kit::test]
    fn driver_idle_when_no_fade_active(cx: &mut TestAppContext) {
        // NEW 500 ms ago: its fade starts in 1200 ms. EXITED long gone: no tint.
        let now = now_boot_ns();
        let mut rig = open(cx, snapshot(now - 500 * MS, now - 10_000 * MS), |_| {});
        let table = rig.table.clone();
        rig.vcx
            .update(|_, cx| table.update(cx, |_, cx| cx.notify()));
        redraw(&mut rig.vcx);
        assert_eq!(requested(&mut rig), 0);
        let driver = rig.driver.clone();
        assert!(!rig.vcx.read_entity(&driver, |d, _| d.is_armed()));
        // The NEW row is tinted at full alpha; the others show the plain zebra.
        rig.vcx.update(|_, cx| {
            let theme = current(cx).clone();
            let t = table.read(cx);
            assert_eq!(
                t.delegate.row_tint(0, &theme),
                Some(TintKind::New.color(TINT_ALPHA, &theme))
            );
            assert_eq!(t.delegate.row_tint(1, &theme), None);
            assert_eq!(t.row_background(2, &theme), theme.list_bg);
        });
    }

    #[gpui_kit::test]
    fn highlight_off_draws_no_tint(cx: &mut TestAppContext) {
        let now = now_boot_ns();
        let mut rig = open(cx, snapshot(now, now), |d| {
            let mut s = Settings::default();
            s.processes.highlight_new_exited = false;
            d.set_settings(&s);
        });
        let table = rig.table.clone();
        rig.vcx.update(|_, cx| {
            let theme = current(cx).clone();
            let t = table.read(cx);
            assert_eq!(t.delegate.row_tint(0, &theme), None);
            assert_eq!(t.delegate.row_tint(1, &theme), None);
            assert_eq!(t.row_background(0, &theme), theme.list_bg);
            assert_eq!(t.row_background(1, &theme), theme.row_alt);
            // The exited row still lingers.
            assert!(t.delegate.row_flags(1).has(RowFlags::EXITED));
        });
        assert_eq!(requested(&mut rig), 0);
    }

    #[cfg(feature = "test-hooks")]
    mod hooks {
        use super::*;

        const T: TimeNs = 100_000 * MS;

        fn set_now(rig: &mut Rig, now: TimeNs) {
            let table = rig.table.clone();
            rig.vcx.update(|_, cx| {
                table.update(cx, |t, cx| {
                    t.delegate.set_fixed_now(Some(now));
                    cx.notify();
                })
            });
            redraw(&mut rig.vcx);
        }

        fn frame(rig: &mut Rig) {
            rig.vcx.update(|window, cx| window.simulate_next_frame(cx));
            redraw(&mut rig.vcx);
        }

        fn armed(rig: &mut Rig) -> bool {
            let driver = rig.driver.clone();
            rig.vcx.read_entity(&driver, |d, _| d.is_armed())
        }

        #[gpui_kit::test]
        fn driver_requests_only_while_fading(cx: &mut TestAppContext) {
            // NEW since T, EXITED gone; the clock starts 500 ms after the flag.
            let mut rig = open(cx, snapshot(T, 0), |d| d.set_fixed_now(Some(T + 500 * MS)));
            assert_eq!(requested(&mut rig), 0);
            assert!(!armed(&mut rig));

            // 1850 ms: inside the fade, so every frame asks for the next.
            set_now(&mut rig, T + 1850 * MS);
            assert!(armed(&mut rig));
            let fading = requested(&mut rig);
            assert!(fading >= 1);
            let table = rig.table.clone();
            rig.vcx.update(|_, cx| {
                let theme = current(cx).clone();
                let want = TINT_ALPHA * (1.0 - ease_out_cubic(0.5));
                let tint = table.read(cx).delegate.row_tint(0, &theme).expect("fading");
                assert!((tint.a - want).abs() < 1e-6);
            });
            frame(&mut rig);
            assert!(requested(&mut rig) > fading);

            // Past L: the next frame draws no tint and asks for nothing more.
            rig.vcx.update(|_, cx| {
                table.update(cx, |t, _| t.delegate.set_fixed_now(Some(T + 2100 * MS)))
            });
            frame(&mut rig);
            let done = requested(&mut rig);
            assert!(!armed(&mut rig));
            frame(&mut rig);
            assert_eq!(requested(&mut rig), done);
            rig.vcx.update(|_, cx| {
                let theme = current(cx).clone();
                assert_eq!(table.read(cx).delegate.row_tint(0, &theme), None);
            });
        }

        /// Counts the table's notifies from now on.
        fn notifies(rig: &mut Rig) -> Rc<Cell<u32>> {
            let n = Rc::new(Cell::new(0));
            let (table, count) = (rig.table.clone(), n.clone());
            rig.vcx.update(|_, cx| {
                cx.observe(&table, move |_, _| count.set(count.get() + 1))
                    .detach()
            });
            n
        }

        fn advance(rig: &mut Rig, ms: u64) {
            rig.vcx
                .executor()
                .advance_clock(std::time::Duration::from_millis(ms));
            rig.vcx.run_until_parked();
        }

        #[gpui_kit::test]
        fn fade_start_wakes_the_table(cx: &mut TestAppContext) {
            let mut rig = open(cx, snapshot(T, 0), |d| d.set_fixed_now(Some(T + 500 * MS)));
            assert_eq!(requested(&mut rig), 0);
            // One timer, due at the fade start 1200 ms later, notifies the table.
            let woke = notifies(&mut rig);
            advance(&mut rig, 1199);
            assert_eq!(woke.get(), 0);
            advance(&mut rig, 2);
            assert_eq!(woke.get(), 1);
            // That frame finds the row fading and asks for animation frames.
            set_now(&mut rig, T + 1710 * MS);
            assert!(requested(&mut rig) >= 1);
            assert!(armed(&mut rig));
        }

        #[gpui_kit::test]
        fn reduced_motion_holds_then_switches_off(cx: &mut TestAppContext) {
            let mut rig = open(cx, snapshot(T, 0), |d| d.set_fixed_now(Some(T + 1850 * MS)));
            rig.vcx.update(|_, cx| cx.set_reduce_motion(true));
            set_now(&mut rig, T + 1850 * MS);
            let table = rig.table.clone();
            rig.vcx.update(|_, cx| {
                let theme = current(cx).clone();
                assert_eq!(
                    table.read(cx).delegate.row_tint(0, &theme),
                    Some(TintKind::New.color(TINT_ALPHA, &theme))
                );
            });
            // No fade frames; one timer at L instead.
            let before = requested(&mut rig);
            frame(&mut rig);
            assert_eq!(requested(&mut rig), before);
            let woke = notifies(&mut rig);
            advance(&mut rig, 149);
            assert_eq!(woke.get(), 0);
            advance(&mut rig, 2);
            assert_eq!(woke.get(), 1);
            rig.vcx.update(|_, cx| {
                table.update(cx, |t, _| t.delegate.set_fixed_now(Some(T + 2000 * MS)))
            });
            redraw(&mut rig.vcx);
            rig.vcx.update(|_, cx| {
                let theme = current(cx).clone();
                let t = table.read(cx);
                assert_eq!(t.delegate.row_tint(0, &theme), None);
                assert_eq!(t.row_background(0, &theme), theme.list_bg);
            });
        }

        #[gpui_kit::test]
        fn exited_row_enter_toasts_process_exited(cx: &mut TestAppContext) {
            let mut rig = open(cx, snapshot(T, T), |d| d.set_fixed_now(Some(T)));
            let table = rig.table.clone();
            let select = |rig: &mut Rig, pid| {
                let id = tl_ui::pages::processes::delegate::ProcRowId::Process(key(pid));
                rig.vcx
                    .update(|_, cx| table.update(cx, |t, _| t.selection.click(id)));
            };

            select(&mut rig, 20);
            rig.vcx.simulate_keystrokes("enter");
            redraw(&mut rig.vcx);
            let toasts = rig.toasts.clone();
            assert_eq!(
                rig.vcx.read_entity(&toasts, |q, _| q.visible_texts()),
                vec!["Process exited"]
            );
            assert_eq!(rig.opened.get(), 0);

            // A live row still opens Properties.
            select(&mut rig, 10);
            rig.vcx.simulate_keystrokes("enter");
            redraw(&mut rig.vcx);
            assert_eq!(rig.opened.get(), 1);
        }
    }
}

/// The process row hover tooltip (E22, PROC-039, R-009, PROC-012).
mod row_tooltip {
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement, Render,
        SharedString, Styled, TestAppContext, VisualTestContext, Window, div, px,
    };
    use tl_model::actions::SchedPolicy;
    use tl_model::ids::{GroupKey, NodeId, ProcessKey, PseudoKind};
    use tl_model::process::{
        GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState, ProcessExtras, ProcessMeta,
        ProcessRecord,
    };
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::snapshot::{
        ComponentHeader, ProcessSnapshot, ServiceStatus, ServicesSnapshot, Snapshot, StartupType,
        UnitRecord,
    };
    use tl_model::system::CpuSet;
    use tl_model::table::{Manager, ProcessTableView, TableRow, Total, UnitRef, UnitType};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, ProcessField, Validity};
    use tl_ui::components::tooltip::DELAY_ROW_NAME;
    use tl_ui::pages::processes::cells::RowRef;
    use tl_ui::pages::processes::delegate::ProcessDelegate;
    use tl_ui::pages::processes::tooltip::{RowTipWords, TipLine, row_tooltip_lines};
    use tl_ui::table::tree_table::{RowKind, TlTreeTable};

    fn a(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    fn words() -> RowTipWords {
        RowTipWords::from_fn(|key| {
            SharedString::from(match key {
                "processes-tooltip-path" => "path:",
                "processes-tooltip-restricted" => "restricted",
                "processes-tooltip-user" => "user:",
                "processes-tooltip-uid" => "uid",
                "processes-tooltip-unit" => "Unit:",
                "processes-tooltip-cgroup" => "cgroup:",
                "processes-tooltip-desktop-file" => "Desktop file:",
                "processes-tooltip-some-values-need-administrator-access" => {
                    "Some values need administrator access."
                }
                "processes-tooltip-cpu-time-spent-in-hardware" => {
                    "CPU time spent in hardware and software interrupts"
                }
                "processes-tooltip-memory-held-by-zram-compressed" => {
                    "Memory held by zram compressed swap devices"
                }
                other => panic!("unexpected key {other}"),
            })
        })
    }

    fn unit() -> UnitRef {
        UnitRef {
            manager: Manager::User,
            name: a("app-firefox@1234.service"),
        }
    }

    fn record(pid: u32) -> ProcessRecord {
        ProcessRecord {
            key: ProcessKey {
                start_ticks: u64::from(pid),
                boot_id: BootId(0),
                pid,
            },
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: a("firefox"),
                name: a("Firefox"),
                exe: Some(a("/usr/lib/firefox/firefox")),
                cmdline: Some(a("/usr/lib/firefox/firefox --name firefox")),
                user: a("oystein"),
                cgroup: Some(a(
                    "/user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox@1234.service",
                )),
                app_id: Some(a("firefox")),
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a("firefox"),
            }),
            group: GroupKind::App {
                app_id: a("firefox"),
            }
            .key(),
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

    fn extras(unit: Option<UnitRef>) -> ProcessExtras {
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
            exe_name: a("firefox"),
            exe_path: a("/usr/lib/firefox/firefox"),
            exe_deleted: false,
            unit,
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
        }
    }

    fn missing<T: Default>() -> Total<T> {
        Total {
            value: T::default(),
            validity: Validity::Missing,
        }
    }

    fn unit_record(unit: UnitRef, description: &str) -> UnitRecord {
        UnitRecord {
            unit,
            kind: UnitType::Service,
            description: description.into(),
            status: ServiceStatus::Running,
            sub_state: "running".into(),
            load_state: "loaded".into(),
            startup: StartupType::Enabled,
            main_pid: 0,
            user: "oystein".into(),
            path: "".into(),
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

    fn group(node: NodeId, kind: GroupRowKind) -> GroupRow {
        GroupRow {
            node,
            key: GroupKind::App {
                app_id: a("firefox"),
            }
            .key(),
            kind,
            class: GroupKind::App {
                app_id: a("firefox"),
            },
            name: a("Firefox"),
            icon: None,
            members: 1,
            pids: 1,
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
            threads: 1,
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
            desktop_path: Some(a("/usr/share/applications/firefox.desktop")),
            description: None,
            user: None,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn row_of(r: &ProcessRecord) -> TableRow {
        TableRow {
            node: NodeId::Process(r.key),
            depth: 0,
            expanded: false,
            has_children: false,
            record: Some(0),
            group: None,
            descendants: 0,
            subtree: None,
        }
    }

    /// A snapshot with `records` (extras carry the Firefox unit) and the unit's services entry.
    fn snapshot(records: Vec<ProcessRecord>) -> Snapshot {
        let mut s = Snapshot::empty(BootId(0), 0);
        let n = records.len();
        s.processes = Arc::new(ProcessSnapshot {
            records: records.into(),
            extras: (0..n).map(|_| extras(Some(unit()))).collect(),
            ..ProcessSnapshot::default()
        });
        s.services = Arc::new(ServicesSnapshot {
            units: Arc::from([unit_record(unit(), "Firefox")]),
            ..ServicesSnapshot::default()
        });
        s
    }

    fn texts(lines: &[TipLine]) -> Vec<String> {
        lines
            .iter()
            .map(|(label, value)| match label {
                Some(l) => format!("{l} {value}"),
                None => value.to_string(),
            })
            .collect()
    }

    fn lines_of(row: RowRef, s: &Snapshot) -> (Vec<String>, bool) {
        let mut out = Vec::new();
        let warn = row_tooltip_lines(row, s, &words(), &mut out);
        (texts(&out), warn)
    }

    #[test]
    fn row_tooltip_lines_in_order_and_optional() {
        let r = record(1234);
        let s = snapshot(vec![r.clone()]);
        let row = row_of(&r);
        let (lines, warn) = lines_of(RowRef::Record(&r, &row, RowKind::Process), &s);
        assert_eq!(
            lines,
            [
                "/usr/lib/firefox/firefox --name firefox",
                "path: /usr/lib/firefox/firefox",
                "user: oystein (uid 1000)",
                "Unit: app-firefox@1234.service (Firefox)",
                "cgroup: /user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox@1234.service",
            ]
        );
        assert!(!warn);

        // Without a unit, a cgroup or a readable exe: the Unit and cgroup lines go, the path
        // reads "restricted".
        let mut bare = record(1);
        let meta = Arc::make_mut(&mut bare.meta);
        meta.exe = None;
        meta.cgroup = None;
        let mut s2 = snapshot(vec![bare.clone()]);
        s2.processes = Arc::new(ProcessSnapshot {
            records: Arc::from([bare.clone()]),
            extras: Arc::from([extras(None)]),
            ..ProcessSnapshot::default()
        });
        let row = row_of(&bare);
        let (lines, _) = lines_of(RowRef::Record(&bare, &row, RowKind::Process), &s2);
        assert_eq!(
            lines,
            [
                "/usr/lib/firefox/firefox --name firefox",
                "path: restricted",
                "user: oystein (uid 1000)",
            ]
        );

        // An app row: the earliest member's lines, then its desktop file.
        let g = group(NodeId::Group(GroupKey(7)), GroupRowKind::App);
        let (lines, _) = lines_of(RowRef::Group(&g, RowKind::App), &s);
        assert_eq!(
            lines.last().map(String::as_str),
            Some("Desktop file: /usr/share/applications/firefox.desktop")
        );
        assert_eq!(lines.len(), 6, "{lines:?}");

        // A restricted row ends with the note, shown in status.warning.
        let mut locked = record(1234);
        locked
            .validity
            .set(ProcessField::IoRead, Validity::Restricted);
        let s3 = snapshot(vec![locked.clone()]);
        let row = row_of(&locked);
        let (lines, warn) = lines_of(RowRef::Record(&locked, &row, RowKind::Process), &s3);
        assert_eq!(
            lines.last().map(String::as_str),
            Some("Some values need administrator access.")
        );
        assert!(warn, "the note is the warning line");
    }

    #[test]
    fn row_tooltip_truncates_cmdline_512() {
        let mut r = record(1234);
        let long: String = "é".repeat(2_000);
        Arc::make_mut(&mut r.meta).cmdline = Some(a(&long));
        let s = snapshot(vec![r.clone()]);
        let row = row_of(&r);
        let (lines, _) = lines_of(RowRef::Record(&r, &row, RowKind::Process), &s);
        assert_eq!(lines[0].chars().count(), 512);
    }

    #[test]
    fn row_tooltip_pseudo_and_kthread() {
        let s = snapshot(Vec::new());
        for (kind, text) in [
            (
                PseudoKind::Interrupts,
                "CPU time spent in hardware and software interrupts",
            ),
            (
                PseudoKind::MemoryCompression,
                "Memory held by zram compressed swap devices",
            ),
        ] {
            let g = group(NodeId::Pseudo(kind), GroupRowKind::Pseudo);
            let (lines, warn) = lines_of(RowRef::Group(&g, RowKind::Pseudo), &s);
            assert_eq!(lines, [text]);
            assert!(!warn);
        }

        let mut k = record(42);
        k.flags = ProcFlags::KERNEL_THREAD;
        let meta = Arc::make_mut(&mut k.meta);
        meta.comm = a("kworker/0:1");
        meta.exe = None;
        meta.cmdline = Some(a(""));
        let s = snapshot(vec![k.clone()]);
        let row = row_of(&k);
        let (lines, _) = lines_of(RowRef::Record(&k, &row, RowKind::KernelThread), &s);
        assert_eq!(lines[0], "[kworker/0:1]");
    }

    struct Host {
        table: Entity<TlTreeTable<ProcessDelegate>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(800.))
                .h(px(400.))
                .flex()
                .flex_col()
                .child(self.table.clone())
        }
    }

    fn redraw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui_kit::test]
    fn row_tooltip_delay_600ms(cx: &mut TestAppContext) {
        let mut r = record(1234);
        r.validity.set(ProcessField::IoRead, Validity::Restricted);
        let mut s = snapshot(vec![r.clone()]);
        s.table = Arc::new(ProcessTableView {
            header: ComponentHeader {
                version: 1,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: Arc::from([row_of(&r)]),
            ..ProcessTableView::default()
        });
        let snap = Arc::new(s);
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
        let window = cx.add_window(move |window, cx| {
            let d =
                ProcessDelegate::new(snap, &Settings::default(), &UiState::default(), window, cx);
            let table = cx.new(|cx| TlTreeTable::new(d, window, cx));
            let host = cx.new(|_| Host { table });
            Root::new(host, window, cx)
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        redraw(&mut vcx);

        let name = vcx.debug_bounds("TlNameCell").expect("the row's Name cell");
        // The first move sets the hovered row, the second makes its tooltips ready, the third
        // starts the delay.
        for _ in 0..3 {
            vcx.simulate_mouse_move(name.center(), None, Modifiers::none());
            redraw(&mut vcx);
        }
        assert!(vcx.debug_bounds("TlRowTip").is_none());

        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        redraw(&mut vcx);
        assert!(
            vcx.debug_bounds("TlRowTip").is_none(),
            "no tooltip at 500 ms"
        );

        vcx.executor()
            .advance_clock(DELAY_ROW_NAME - std::time::Duration::from_millis(500));
        redraw(&mut vcx);
        assert!(vcx.debug_bounds("TlRowTip").is_some(), "shown at 600 ms");
        assert!(
            vcx.debug_bounds("TlRowTip-warning").is_some(),
            "the restricted note is the warning line"
        );
    }
}

// pdu.56: the live process table (TlTreeTable with ProcessDelegate) draws the "Σ " prefix of a
// subtree total and dims a measured zero, through CellOut and numeric_cell.
mod live {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled,
        TestAppContext, VisualTestContext, Window, div, px,
    };
    use tl_model::ids::{ColumnId, NodeId, ProcessKey};
    use tl_model::process::GroupRowKind;
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, Snapshot};
    use tl_model::table::{ProcessTableView, TableRow};
    use tl_model::time::BootId;
    use tl_model::validity::Validity;
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::processes::cells::{DELETED_SUFFIX, SUBTREE_PREFIX};
    use tl_ui::pages::processes::delegate::ProcessDelegate;
    use tl_ui::table::cells::{CellCtx, CellOut, CellSuffix, DIM_OPACITY, cell_text_color};
    use tl_ui::table::tree_table::{TlTreeTable, TreeTableDelegate as _};
    use tl_ui::theme::{current, metrics};

    use super::values::{extras, group, record};

    struct Host {
        table: Entity<TlTreeTable<ProcessDelegate>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(1200.)).h(px(400.)).child(self.table.clone())
        }
    }

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            pid,
            ..record().key
        }
    }

    /// Parent PID 10 (expanded, with a subtree total of CPU 12.5%) and its child PID 11 at
    /// CPU 0%. `has_children` is false on the parent when `leaf`.
    fn snapshot(leaf: bool) -> Arc<Snapshot> {
        let mut parent = record();
        parent.key = key(10);
        let mut child = record();
        child.key = key(11);
        child.ppid = 10;
        child.cpu_pct = 0.0;
        let row = |k: ProcessKey, depth: u8, record: u32, children: bool| TableRow {
            node: NodeId::Process(k),
            depth,
            expanded: children,
            has_children: children,
            record: Some(record),
            group: None,
            descendants: u32::from(children),
            subtree: children.then_some(0),
        };
        let mut s = Snapshot::empty(BootId(0), 0);
        s.table = Arc::new(ProcessTableView {
            header: ComponentHeader {
                version: 1,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: vec![row(key(10), 0, 0, !leaf), row(key(11), 1, 1, false)].into(),
            subtree: vec![group(GroupRowKind::App)].into(),
            ..ProcessTableView::default()
        });
        s.processes = Arc::new(ProcessSnapshot {
            records: vec![parent, child].into(),
            extras: vec![extras(), extras()].into(),
            ..ProcessSnapshot::default()
        });
        Arc::new(s)
    }

    fn open(
        cx: &mut TestAppContext,
        settings: Settings,
        snap: Arc<Snapshot>,
    ) -> (Entity<TlTreeTable<ProcessDelegate>>, &mut VisualTestContext) {
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
        let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let out = slot.clone();
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let d = ProcessDelegate::new(snap, &settings, &UiState::default(), window, cx);
            let table = cx.new(|cx| TlTreeTable::new(d, window, cx));
            *out.borrow_mut() = Some(table.clone());
            Host { table }
        });
        vcx.run_until_parked();
        let table = slot.borrow_mut().take().expect("table created");
        (table, vcx)
    }

    fn prefix_drawn(vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.debug_bounds("TlCell-prefix").is_some()
    }

    fn cell(
        vcx: &mut VisualTestContext,
        table: &Entity<TlTreeTable<ProcessDelegate>>,
        ix: usize,
        col: ColumnId,
    ) -> CellOut {
        vcx.update(|_, cx| {
            let mut out = CellOut {
                want_text: true,
                ..CellOut::default()
            };
            table.read(cx).delegate.cell(ix, col, &mut out);
            out
        })
    }

    fn on() -> Settings {
        let mut s = Settings::default();
        s.processes.subtree_totals = true;
        s.tables.dim_zero_values = true;
        s
    }

    #[gpui_kit::test]
    fn live_table_draws_subtree_prefix_and_dim_zero(cx: &mut TestAppContext) {
        let (table, vcx) = open(cx, on(), snapshot(false));
        assert!(prefix_drawn(vcx), "the parent's CPU cell draws the prefix");

        let theme = vcx.update(|_, cx| current(cx).clone());
        let parent = cell(vcx, &table, 0, ColumnId::Cpu);
        assert_eq!(parent.text, "12.5%", "the engine's subtree sum");
        let (text, color) = parent.prefix.clone().expect("a prefix on the parent");
        assert_eq!(text.as_ref(), SUBTREE_PREFIX);
        assert_eq!(color, theme.text_secondary);
        assert!(cell(vcx, &table, 0, ColumnId::Pid).prefix.is_none());

        let child = cell(vcx, &table, 1, ColumnId::Cpu);
        assert!(child.prefix.is_none(), "a leaf has no subtree total");
        assert!(child.dim, "a measured zero is dimmed");
        assert!(!parent.dim);
        let dimmed = vcx.update(|_, cx| {
            let ctx = CellCtx {
                m: metrics(cx),
                theme: &theme,
                selected: false,
                width: px(100.),
            };
            cell_text_color(&child, &ctx)
        });
        assert_eq!(dimmed, Some(theme.text_primary.opacity(DIM_OPACITY)));
    }

    #[gpui_kit::test]
    fn live_table_no_prefix_when_off_or_leaf(cx: &mut TestAppContext) {
        let (table, vcx) = open(cx, Settings::default(), snapshot(false));
        assert!(!prefix_drawn(vcx), "setting off");
        let parent = cell(vcx, &table, 0, ColumnId::Cpu);
        assert_eq!(parent.text, "3.3%", "the process's own value");
        assert!(!cell(vcx, &table, 1, ColumnId::Cpu).dim, "dim zero is off");
    }

    #[gpui_kit::test]
    fn live_table_no_prefix_on_a_leaf(cx: &mut TestAppContext) {
        let (_, vcx) = open(cx, on(), snapshot(true));
        assert!(!prefix_drawn(vcx));
    }

    /// PID 10 collapsed over 3 descendants, its executable deleted.
    fn collapsed_deleted() -> Arc<Snapshot> {
        let mut parent = record();
        parent.key = key(10);
        parent.meta = Arc::new(tl_model::process::ProcessMeta {
            exe: Some(Arc::from(format!(
                "/usr/lib/firefox/firefox{DELETED_SUFFIX}"
            ))),
            ..(*parent.meta).clone()
        });
        let mut s = Snapshot::empty(BootId(0), 0);
        s.table = Arc::new(ProcessTableView {
            header: ComponentHeader {
                version: 1,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: vec![TableRow {
                node: NodeId::Process(key(10)),
                depth: 0,
                expanded: false,
                has_children: true,
                record: Some(0),
                group: None,
                descendants: 3,
                subtree: None,
            }]
            .into(),
            ..ProcessTableView::default()
        });
        s.processes = Arc::new(ProcessSnapshot {
            records: vec![parent].into(),
            extras: vec![extras()].into(),
            ..ProcessSnapshot::default()
        });
        Arc::new(s)
    }

    /// UI-PRC-004 on the live table: the PID's " +N" in `text_secondary` and the Image path's
    /// " (deleted)" in `status.danger`, as the coloured end of the cell text.
    #[gpui_kit::test]
    fn live_table_pid_plus_and_deleted_suffix(cx: &mut TestAppContext) {
        let (table, vcx) = open(cx, Settings::default(), collapsed_deleted());
        let theme = vcx.update(|_, cx| current(cx).clone());
        let pid = cell(vcx, &table, 0, ColumnId::Pid);
        assert_eq!(pid.text, "10 +3");
        assert_eq!(
            pid.suffix,
            Some(CellSuffix {
                len: " +3".len(),
                color: theme.text_secondary,
                selected_color: theme.text_selected_secondary,
            })
        );
        let path = cell(vcx, &table, 0, ColumnId::ImagePath);
        assert!(
            path.arc
                .as_deref()
                .is_some_and(|p| p.ends_with(DELETED_SUFFIX))
        );
        assert_eq!(
            path.suffix,
            Some(CellSuffix {
                len: DELETED_SUFFIX.len(),
                color: theme.status.danger,
                selected_color: theme.status.danger,
            })
        );
        assert!(cell(vcx, &table, 0, ColumnId::Cpu).suffix.is_none());
        // The table draws the row with both suffixes.
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(vcx.debug_bounds("TlCell-numeric").is_some());
    }

    /// Both pseudo rows as the engine sends them (PROC-012): Interrupts with CPU 1.5%, Memory
    /// compression with 64 MiB Memory and 2 MiB/s disk. Every other field is Unsupported.
    fn pseudo_rows() -> Arc<Snapshot> {
        use tl_model::ids::PseudoKind;
        use tl_model::process::GroupRow;
        use tl_model::validity::{FieldValidity, ProcessField as F};

        let pseudo = |kind: PseudoKind, name: &str| GroupRow {
            node: NodeId::Pseudo(kind),
            name: Arc::from(name),
            members: 0,
            pids: 0,
            cpu_pct: 0.0,
            memory_bytes: 0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            threads: 0,
            validity: FieldValidity::all(Validity::Unsupported),
            ..group(GroupRowKind::Pseudo)
        };
        let mut irq = pseudo(PseudoKind::Interrupts, "Interrupts");
        irq.cpu_pct = 1.5;
        irq.validity.set(F::Cpu, Validity::Valid);
        let mut zram = pseudo(PseudoKind::MemoryCompression, "Memory compression (zram)");
        zram.rss_anon_bytes = 64 << 20;
        zram.memory_bytes = 64 << 20;
        zram.io_read_bps = 2.0 * 1_048_576.0;
        zram.disk_bps = 2.0 * 1_048_576.0;
        for f in [F::RssAnon, F::IoRead, F::IoWrite] {
            zram.validity.set(f, Validity::Valid);
        }
        let row = |node: NodeId, group: u32| TableRow {
            node,
            depth: 0,
            expanded: false,
            has_children: false,
            record: None,
            group: Some(group),
            descendants: 0,
            subtree: None,
        };
        let mut s = Snapshot::empty(BootId(0), 0);
        s.table = Arc::new(ProcessTableView {
            header: ComponentHeader {
                version: 1,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: vec![row(irq.node.clone(), 0), row(zram.node.clone(), 1)].into(),
            groups: vec![irq, zram].into(),
            ..ProcessTableView::default()
        });
        s.processes = Arc::new(ProcessSnapshot::default());
        Arc::new(s)
    }

    fn shown(out: &CellOut) -> String {
        out.arc
            .as_deref()
            .map_or_else(|| out.text.clone(), str::to_owned)
    }

    /// UI-PRC-002: `activity` and `archive` at 16 px (the small slot), no chevron, PID blank,
    /// Type "Pseudo", User "root".
    #[gpui_kit::test]
    fn pseudo_rows_icons_and_blank_pid(cx: &mut TestAppContext) {
        use gpui_kit::assets::IconName;
        use tl_ui::table::tree_table::{IconSlot, NameIcon};

        let (table, vcx) = open(cx, Settings::default(), pseudo_rows());
        for (ix, icon, name) in [
            (0, IconName::Activity, "Interrupts"),
            (1, IconName::Archive, "Memory compression (zram)"),
        ] {
            let (name_cell, expandable) = vcx.update(|_, cx| {
                let d = &table.read(cx).delegate;
                (d.name_cell(ix), d.row_expandable(ix))
            });
            assert!(
                matches!(name_cell.icon, NameIcon::Lucide(i) if i == icon),
                "{name}: the Lucide icon"
            );
            assert!(
                matches!(name_cell.icon_size, IconSlot::Small),
                "{name}: 16 px"
            );
            assert_eq!(name_cell.primary.as_ref(), name);
            assert_eq!(expandable, None, "{name}: no chevron");

            let pid = cell(vcx, &table, ix, ColumnId::Pid);
            assert_eq!(pid.validity, Validity::Valid, "{name}: PID blank, not n/a");
            assert!(shown(&pid).is_empty(), "{name}: PID blank");
            assert_eq!(shown(&cell(vcx, &table, ix, ColumnId::Type)), "Pseudo");
            assert_eq!(shown(&cell(vcx, &table, ix, ColumnId::User)), "root");
        }
        // The table draws both rows with their icons.
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(vcx.debug_bounds("TlNameCell").is_some());
    }

    /// PROC-012: a pseudo row shows `n/a` in every column it has no data for. Interrupts has
    /// only CPU; Memory compression has Memory and Disk.
    #[gpui_kit::test]
    fn pseudo_rows_na_columns(cx: &mut TestAppContext) {
        let (table, vcx) = open(cx, Settings::default(), pseudo_rows());
        let validity = |vcx: &mut VisualTestContext, ix, col| cell(vcx, &table, ix, col).validity;
        let na = Validity::Unsupported;

        assert_eq!(validity(vcx, 0, ColumnId::Cpu), Validity::Valid);
        assert_eq!(shown(&cell(vcx, &table, 0, ColumnId::Cpu)), "1.5%");
        for col in [
            ColumnId::Memory,
            ColumnId::Disk,
            ColumnId::Network,
            ColumnId::Threads,
            ColumnId::Handles,
            ColumnId::CpuKernel,
            ColumnId::Description,
            ColumnId::Cmdline,
        ] {
            assert_eq!(validity(vcx, 0, col), na, "Interrupts {col:?}");
        }

        assert_eq!(
            validity(vcx, 1, ColumnId::Cpu),
            na,
            "Memory compression CPU"
        );
        assert_eq!(validity(vcx, 1, ColumnId::Memory), Validity::Valid);
        assert_eq!(shown(&cell(vcx, &table, 1, ColumnId::Memory)), "64.00 MiB");
        assert_eq!(validity(vcx, 1, ColumnId::Disk), Validity::Valid);
        for col in [ColumnId::Network, ColumnId::Threads, ColumnId::Gpu] {
            assert_eq!(validity(vcx, 1, col), na, "Memory compression {col:?}");
        }
    }
}
