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
            search_lower: a("firefox"),
        }
    }

    fn record() -> ProcessRecord {
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
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    fn extras() -> ProcessExtras {
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

    fn group(kind: GroupRowKind) -> GroupRow {
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
        CellCtx, CellWords, GroupMembers, PaintState, heat_alpha, heat_background,
        metric_text_style, process_column, render_metric_cell, text_colors,
    };
    use tl_ui::table::cells::CellOut;
    use tl_ui::text::style::TNUM;
    use tl_ui::theme::{TlTheme, current};

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
    };

    /// A Valid CPU-style cell holding `value` as f32 bits.
    fn pct_out(value: f32) -> CellOut {
        CellOut {
            bits: u64::from(value.to_bits()),
            validity: Validity::Valid,
            ..CellOut::default()
        }
    }

    #[test]
    fn heat_alpha_clamped_and_intensity_scaled() {
        let env = Env::new();
        let ctx = env.ctx();
        let cpu = process_column(ColumnId::Cpu);
        // ncpu 32: reference 12.5 %.
        assert!(close(heat_alpha(cpu, 25.0, &ctx, 50), 0.5));
        assert!(close(heat_alpha(cpu, 6.25, &ctx, 50), 0.25));
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

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = current(cx).clone();
            let cell = render_metric_cell(
                &self.out,
                self.text.clone(),
                process_column(self.col),
                px(120.),
                self.st,
                &theme,
                cx,
            );
            div().size(px(400.)).child(cell)
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

    fn heat_count(
        cx: &mut TestAppContext,
        col: ColumnId,
        mut out: CellOut,
        st: PaintState,
    ) -> usize {
        let theme = theme(cx);
        let env = Env::new();
        // The fill is built with heat on, so only the painter's own rules can hide it.
        let fill = heat_background(&out, process_column(col), &env.ctx(), HEAT_ON, &theme)
            .unwrap_or(Hsla {
                a: 0.5,
                ..theme.value_heat
            });
        out.background = Some(fill);
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
                let text = bounds(vcx, "TlProcCell-text");
                let glyph = bounds(vcx, "TlProcCell-glyph");
                assert_eq!(text.is_some(), shows_text, "{col:?} {validity:?}");
                assert_eq!(glyph.is_some(), !shows_text, "{col:?} {validity:?}");
                let cell = text.or(glyph).expect("cell drawn");
                assert_eq!(cell.size.width, px(120.));
                let lock = bounds(vcx, "TlProcCell-lock");
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
        let theme = theme(cx);
        let cpu = process_column(ColumnId::Cpu);
        let c = text_colors("3.3%", cpu, Validity::Stale, PaintState::default(), &theme);
        assert_eq!(c.split, 4);
        assert!(close(c.main.a, theme.text_primary.a * 0.6));
        assert_eq!(c.main, theme.text_primary.opacity(0.6));
        let v = text_colors("3.3%", cpu, Validity::Valid, PaintState::default(), &theme);
        assert_eq!(v.main, theme.text_primary);
    }

    #[gpui_kit::test]
    fn deleted_suffix_danger_colour(cx: &mut TestAppContext) {
        let theme = theme(cx);
        let path = process_column(ColumnId::ImagePath);
        let text = "/usr/bin/foo (deleted)";
        let c = text_colors(text, path, Validity::Valid, PaintState::default(), &theme);
        assert_eq!(&text[c.split..], " (deleted)");
        assert_eq!(c.tail, theme.status.danger);
        assert_eq!(c.main, theme.text_primary);
        let plain = text_colors(
            "/usr/bin/foo",
            path,
            Validity::Valid,
            PaintState::default(),
            &theme,
        );
        assert_eq!(plain.split, "/usr/bin/foo".len());
    }

    #[gpui_kit::test]
    fn pid_plus_and_row_state_colours(cx: &mut TestAppContext) {
        let theme = theme(cx);
        let pid = process_column(ColumnId::Pid);
        let c = text_colors(
            "11528 +18",
            pid,
            Validity::Valid,
            PaintState::default(),
            &theme,
        );
        assert_eq!(c.split, 5);
        assert_eq!((c.main, c.tail), (theme.text_primary, theme.text_secondary));
        let selected = PaintState {
            selected: true,
            ..PaintState::default()
        };
        let s = text_colors("11528 +18", pid, Validity::Valid, selected, &theme);
        assert_eq!(
            (s.main, s.tail),
            (theme.text_selected_primary, theme.text_selected_secondary)
        );
        let exited = PaintState {
            exited: true,
            ..PaintState::default()
        };
        let e = text_colors("11528 +18", pid, Validity::Valid, exited, &theme);
        assert_eq!(
            (e.main, e.tail),
            (theme.text_secondary, theme.text_secondary)
        );
    }

    #[gpui_kit::test]
    fn cpu_text_right_aligned_tnum(cx: &mut TestAppContext) {
        cx.update(setup);
        let m = cx.update(|cx| *tl_ui::theme::metrics(cx));
        let cpu = metric_text_style(process_column(ColumnId::Cpu), &m);
        let features = cpu.font_features.expect("tnum");
        assert!(Arc::ptr_eq(&features.0, &TNUM.0));
        assert_eq!(cpu.font_size, Some(px(13.).into()));
        assert!(
            metric_text_style(process_column(ColumnId::ImagePath), &m)
                .font_features
                .is_none()
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
        let cell = bounds(vcx, "TlProcCell-text").expect("text cell");
        assert_eq!(cell.size.width, px(120.));
    }
}

// E22-005: the process Name cell. The table's chevron hit area, its toggle and the group header
// row band are tested in ui_tree_table (`mouse_selection_rules`, group header tests).
mod name_cell {
    use std::sync::Arc;

    use gpui_kit::assets::IconName;
    use gpui_kit::{
        Bounds, Context, IntoElement, ParentElement, Pixels, Render, SharedString, Styled,
        TestAppContext, VisualTestContext, Window, div, px,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_ui::i18n::init_strings;
    use tl_ui::pages::processes::name_cell::{
        NameCellData, NameIcon, NameWords, StateGlyph, icon_slot, name_cell, name_flags,
        render_group_header, render_name_cell, row_icon,
    };
    use tl_ui::table::cells::{CellCtx, IconSourceCache, name_look};
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
        // Pseudo rows have no extras sums.
        let pseudo = group(GroupRowKind::Pseudo);
        let out = Env::new().cell(&pseudo, RowKind::Pseudo, ColumnId::Commit);
        assert!(out.validity == Validity::Valid && shown(&out).is_empty());
    }
}
