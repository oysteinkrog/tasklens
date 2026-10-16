//! Process cell tooltips (E22, UI-PRC-004): the texts of `cell_tooltip` and their hookup in the
//! process delegate.

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement, Render, Styled,
    TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::actions::SchedPolicy;
use tl_model::ids::{ColumnId, GroupKey, NodeId, ProcessKey, PseudoKind};
use tl_model::process::{
    GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState, ProcessExtras, ProcessExtrasP2,
    ProcessMeta, ProcessRecord,
};
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{ComponentHeader, ProcessSnapshot, Snapshot};
use tl_model::system::{CpuSet, Metric, ZramTotals};
use tl_model::table::{ProcessTableView, TableRow};
use tl_model::time::BootId;
use tl_model::validity::{FieldValidity, ProcessField, Validity};
use tl_ui::components::tooltip::{DELAY_CONTROL, DELAY_PROCESS_CELL};
use tl_ui::pages::processes::cells::{
    CellCtx, CellTipWords, CellWords, GroupMembers, PowerSensor, RowRef, TipCtx, cell_tooltip,
    process_column,
};
use tl_ui::pages::processes::delegate::ProcessDelegate;
use tl_ui::table::cells::CellOut;
use tl_ui::table::tree_table::{RowKind, TlTreeTable, TreeTableDelegate};

const MIB: u64 = 1 << 20;
const APP_MEMORY: &str = "Anonymous memory charged to this app's cgroup";
const DISK_OPS: &str = "Linux counts storage operations per cgroup, not per process";

fn a(s: &str) -> Arc<str> {
    Arc::from(s)
}

fn firefox() -> GroupKind {
    GroupKind::App {
        app_id: a("firefox"),
    }
}

fn record() -> ProcessRecord {
    ProcessRecord {
        key: ProcessKey {
            start_ticks: 100,
            boot_id: BootId(0),
            pid: 1234,
        },
        ppid: 1,
        uid: 1000,
        meta: Arc::new(ProcessMeta {
            comm: a("firefox"),
            name: a("Firefox"),
            exe: Some(a("/usr/lib/firefox/firefox")),
            cmdline: Some(a("/usr/lib/firefox/firefox --new-window")),
            user: a("oystein"),
            cgroup: None,
            app_id: Some(a("firefox")),
            icon: None,
            description: None,
            desktop_path: None,
            search_lower: a("firefox"),
        }),
        group: firefox().key(),
        state: ProcState::Sleeping,
        flags: ProcFlags::empty(),
        nice: 5,
        threads: 1,
        cpu_pct: 3.2,
        rss_anon_bytes: 100 * MIB,
        rss_bytes: 200 * MIB,
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
        exe_name: a("firefox"),
        exe_path: a("/usr/lib/firefox/firefox"),
        exe_deleted: false,
        unit: None,
        state_letter: b'S',
        frozen: false,
        disk_read_bps: 0.0,
        disk_write_raw_bps: 0.0,
        flag_since_ns: 0,
    }
}

fn group(node: NodeId, kind: GroupRowKind) -> GroupRow {
    GroupRow {
        node,
        key: firefox().key(),
        kind,
        class: firefox(),
        name: a("Firefox"),
        icon: None,
        members: 1,
        pids: 1,
        note: None,
        memory_bytes: 100 * MIB,
        disk_bps: 0.0,
        net_bps: 0.0,
        gpu_pct: 0.0,
        cpu_pct: 3.2,
        rss_anon_bytes: 100 * MIB,
        rss_bytes: 200 * MIB,
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
        desktop_path: None,
        description: None,
        user: None,
        frozen: false,
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

/// One process with its extras, and the tooltip sources.
struct Fixture {
    view: ProcessTableView,
    records: Vec<ProcessRecord>,
    extras: Vec<ProcessExtras>,
    extras_p2: Vec<ProcessExtrasP2>,
    settings: Settings,
    words: CellWords,
    tip_words: CellTipWords,
    online: CpuSet,
    members: GroupMembers,
    zram: Metric<ZramTotals>,
    power: PowerSensor,
}

impl Fixture {
    fn new(r: ProcessRecord) -> Self {
        let mut f = Self {
            view: ProcessTableView::default(),
            records: vec![r],
            extras: vec![extras()],
            extras_p2: vec![ProcessExtrasP2::default()],
            settings: Settings::default(),
            words: CellWords::from_fn(Arc::from),
            tip_words: CellTipWords::default(),
            online: CpuSet::empty(),
            members: GroupMembers::default(),
            zram: Metric::default(),
            power: PowerSensor::Readable,
        };
        f.members.refresh(&f.view, &f.records, &f.extras);
        f
    }

    fn ctx(&self) -> CellCtx<'_> {
        CellCtx {
            view: &self.view,
            records: &self.records,
            extras: &self.extras,
            settings: &self.settings,
            words: &self.words,
            members: &self.members,
            ncpu: 8,
            online: &self.online,
            mem_total: 64 << 30,
            vram_total: 0,
            boot_time_s: 1_700_000_000,
            clk_tck: 100,
        }
    }

    fn tip_at(&self, row: RowRef, col: ColumnId) -> Option<String> {
        let tip = TipCtx {
            words: &self.tip_words,
            extras_p2: &self.extras_p2,
            zram: self.zram,
            power: self.power,
        };
        let mut out = String::new();
        cell_tooltip(row, process_column(col), &self.ctx(), &tip, &mut out).then_some(out)
    }

    /// The tooltip of the process row's `col` cell.
    fn tip(&self, col: ColumnId) -> Option<String> {
        let row = row_of(&self.records[0]);
        self.tip_at(
            RowRef::Record(&self.records[0], &row, RowKind::Process),
            col,
        )
    }

    fn group_tip(&self, g: &GroupRow, kind: RowKind, col: ColumnId) -> Option<String> {
        self.tip_at(RowRef::Group(g, kind), col)
    }
}

#[test]
fn cell_tooltips_texts() {
    let mut f = Fixture::new(record());
    assert_eq!(f.tip(ColumnId::Priority).as_deref(), Some("nice 5"));

    let app = group(NodeId::Group(GroupKey(7)), GroupRowKind::App);
    assert_eq!(
        f.group_tip(&app, RowKind::App, ColumnId::Memory).as_deref(),
        Some(APP_MEMORY)
    );
    let unit = group(NodeId::Group(GroupKey(8)), GroupRowKind::Unit);
    assert_eq!(
        f.group_tip(&unit, RowKind::Unit, ColumnId::Memory)
            .as_deref(),
        Some(APP_MEMORY)
    );
    // The app row's Command line cell shows its earliest member's.
    assert_eq!(
        f.group_tip(&app, RowKind::App, ColumnId::Cmdline)
            .as_deref(),
        Some("/usr/lib/firefox/firefox --new-window")
    );

    for col in [
        ColumnId::DiskReads,
        ColumnId::DiskWrites,
        ColumnId::DiskReadsDelta,
        ColumnId::DiskWritesDelta,
    ] {
        assert_eq!(f.tip(col).as_deref(), Some(DISK_OPS), "{col:?}");
    }

    f.extras_p2[0].oom_score_adj = Some(-900);
    assert_eq!(
        f.tip(ColumnId::OomScore).as_deref(),
        Some("oom_score_adj -900")
    );

    f.extras_p2[0].power_w = Some(3.44);
    f.extras_p2[0].power_trend_w = Some(1.25);
    assert_eq!(
        f.tip(ColumnId::Power).as_deref(),
        Some("≈ 3.4 W (estimate)")
    );
    assert_eq!(
        f.tip(ColumnId::PowerTrend).as_deref(),
        Some("≈ 1.2 W (estimate)")
    );
    f.extras_p2[0].power_w = None;
    f.power = PowerSensor::NeedsSetup;
    assert_eq!(f.tip(ColumnId::Power).as_deref(), Some("Needs setup"));
    f.power = PowerSensor::Absent;
    assert_eq!(
        f.tip(ColumnId::Power).as_deref(),
        Some("No power sensor available")
    );
    f.power = PowerSensor::Readable;
    assert_eq!(f.tip(ColumnId::Power), None, "no estimate, no guess");

    let zram = group(
        NodeId::Pseudo(PseudoKind::MemoryCompression),
        GroupRowKind::Pseudo,
    );
    f.zram = Metric::valid(ZramTotals {
        orig_data: 1_288_490_189,
        compr_data: 400 * MIB,
        mem_used: 410 * MIB,
    });
    assert_eq!(
        f.group_tip(&zram, RowKind::Pseudo, ColumnId::Memory)
            .as_deref(),
        Some("original 1.20 GiB, compressed 400.00 MiB, ratio 3.1")
    );

    let mut irq = group(NodeId::Pseudo(PseudoKind::Interrupts), GroupRowKind::Pseudo);
    assert_eq!(
        f.group_tip(&irq, RowKind::Pseudo, ColumnId::Cpu),
        None,
        "with IRQ time accounting the CPU cell has a value"
    );
    irq.validity.set(ProcessField::Cpu, Validity::Unsupported);
    assert_eq!(
        f.group_tip(&irq, RowKind::Pseudo, ColumnId::Cpu).as_deref(),
        Some("Interrupt time is included in task time on this kernel")
    );
}

#[test]
fn user_tooltip_shows_a_different_euid() {
    let mut r = record();
    r.euid = Some(0);
    assert_eq!(
        Fixture::new(r).tip(ColumnId::User).as_deref(),
        Some("euid 0")
    );
    let mut r = record();
    r.euid = Some(r.uid);
    assert_eq!(
        Fixture::new(r).tip(ColumnId::User),
        None,
        "equal to the real UID"
    );
    assert_eq!(Fixture::new(record()).tip(ColumnId::User), None, "not set");
}

#[test]
fn network_tooltip_says_shared() {
    let mut r = record();
    r.net_shared = true;
    let f = Fixture::new(r);
    assert_eq!(f.tip(ColumnId::Network).as_deref(), Some("shared"));
    assert_eq!(f.tip(ColumnId::NetSendRate), None, "the Network cell only");
    assert_eq!(Fixture::new(record()).tip(ColumnId::Network), None);
}

#[test]
fn disk_write_tooltip_shows_cancelled_bytes() {
    let mut r = record();
    r.cancelled_write_bps = 1_258_291.0;
    assert_eq!(
        Fixture::new(r).tip(ColumnId::DiskWriteRate).as_deref(),
        Some("cancelled: 1.20 MiB/s")
    );
    let mut r = record();
    r.cancelled_write_bps = 1023.0;
    assert_eq!(
        Fixture::new(r).tip(ColumnId::DiskWriteRate),
        None,
        "nothing below 1 KiB/s"
    );
    assert_eq!(Fixture::new(record()).tip(ColumnId::DiskWriteRate), None);
}

#[test]
fn gpu_n_tooltip_shows_that_gpus_vram() {
    let mut f = Fixture::new(record());
    f.extras_p2[0].gpu_vram_bytes = [0, 1_310_000_000].into_iter().collect();
    assert_eq!(
        f.tip(ColumnId::Gpu1).as_deref(),
        Some("VRAM on GPU 1: 1.22 GiB")
    );
    assert_eq!(f.tip(ColumnId::Gpu0), None, "no VRAM on GPU 0");
    assert_eq!(f.tip(ColumnId::Gpu2), None, "past the list");
    assert_eq!(f.tip(ColumnId::Gpu), None, "the GPU column has none");
    assert_eq!(Fixture::new(record()).tip(ColumnId::Gpu0), None, "not set");
}

#[test]
fn cmdline_tooltip_wraps_at_80() {
    let mut r = record();
    let line: String = ('a'..='z').cycle().take(200).collect();
    Arc::make_mut(&mut r.meta).cmdline = Some(a(&line));
    let tip = Fixture::new(r).tip(ColumnId::Cmdline).expect("a tooltip");
    let lines: Vec<&str> = tip.split('\n').collect();
    assert_eq!(lines, [&line[..80], &line[80..160], &line[160..]]);

    // At most 512 characters, counted as characters.
    let mut r = record();
    Arc::make_mut(&mut r.meta).cmdline = Some(a(&"é".repeat(2_000)));
    let tip = Fixture::new(r).tip(ColumnId::Cmdline).expect("a tooltip");
    assert_eq!(tip.chars().filter(|&c| c != '\n').count(), 512);
    assert_eq!(tip.matches('\n').count(), 6, "512 characters make 7 lines");
}

#[test]
fn cell_tooltip_none_for_plain_cells() {
    let f = Fixture::new(record());
    for col in [
        ColumnId::Cpu,
        ColumnId::Memory,
        ColumnId::User,
        ColumnId::Pid,
        ColumnId::Network,
        ColumnId::Disk,
        ColumnId::Description,
        ColumnId::OomScore,
    ] {
        assert_eq!(f.tip(col), None, "{col:?}");
    }

    // Empty and unreadable command lines have none; the validity tooltip covers Restricted.
    for cmdline in [Some(a("")), None] {
        let mut r = record();
        Arc::make_mut(&mut r.meta).cmdline = cmdline;
        assert_eq!(Fixture::new(r).tip(ColumnId::Cmdline), None);
    }

    // Priority waits for the extras, like its cell.
    let mut f = Fixture::new(record());
    f.extras.clear();
    assert_eq!(f.tip(ColumnId::Priority), None);

    let app = group(NodeId::Group(GroupKey(7)), GroupRowKind::App);
    for col in [ColumnId::Cpu, ColumnId::Priority, ColumnId::DiskReads] {
        assert_eq!(f.group_tip(&app, RowKind::App, col), None, "{col:?}");
    }
    assert_eq!(
        f.group_tip(&app, RowKind::GroupHeader, ColumnId::Memory),
        None,
        "group headers have no tooltips"
    );
    let kthreads = group(NodeId::KernelThreads, GroupRowKind::KernelThreads);
    assert_eq!(
        f.group_tip(&kthreads, RowKind::KernelThread, ColumnId::Memory),
        None
    );
}

#[gpui_kit::test]
fn cell_tip_words_resolve(cx: &mut TestAppContext) {
    cx.update(|cx| {
        tl_ui::i18n::init_strings(cx, "en-US");
        let w = CellTipWords::new(cx);
        let en = CellTipWords::default();
        assert_eq!(w.app_memory.as_ref(), APP_MEMORY);
        assert_eq!(w.disk_ops, en.disk_ops);
        assert_eq!(w.nice, en.nice);
        assert_eq!(w.zram, en.zram);
        assert_eq!(w.power_no_sensor.as_ref(), "No power sensor available");
    });
}

// ---- Delegate hookup ----

struct Host {
    table: Entity<TlTreeTable<ProcessDelegate>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(1000.))
            .h(px(400.))
            .flex()
            .flex_col()
            .child(self.table.clone())
    }
}

/// A snapshot whose table shows one app row with its process under it.
fn app_snapshot() -> Arc<Snapshot> {
    let r = record();
    let mut s = Snapshot::empty(BootId(0), 0);
    s.processes = Arc::new(ProcessSnapshot {
        records: Arc::from([r.clone()]),
        extras: Arc::from([extras()]),
        ..ProcessSnapshot::default()
    });
    let app_row = TableRow {
        node: NodeId::Group(firefox().key()),
        depth: 0,
        expanded: true,
        has_children: true,
        record: None,
        group: Some(0),
        descendants: 1,
        subtree: None,
    };
    let proc_row = TableRow {
        depth: 1,
        ..row_of(&r)
    };
    s.table = Arc::new(ProcessTableView {
        header: ComponentHeader {
            version: 1,
            validity: Validity::Valid,
            ..ComponentHeader::default()
        },
        rows: Arc::from([app_row, proc_row]),
        groups: Arc::from([group(NodeId::Group(firefox().key()), GroupRowKind::App)]),
        ..ProcessTableView::default()
    });
    Arc::new(s)
}

fn open(cx: &mut TestAppContext) -> (Entity<TlTreeTable<ProcessDelegate>>, VisualTestContext) {
    let snap = app_snapshot();
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
    let mut table = None;
    let window = cx.add_window(|window, cx| {
        let d = ProcessDelegate::new(snap, &Settings::default(), &UiState::default(), window, cx);
        let t = cx.new(|cx| TlTreeTable::new(d, window, cx));
        table = Some(t.clone());
        let host = cx.new(|_| Host { table: t });
        Root::new(host, window, cx)
    });
    let vcx = VisualTestContext::from_window(window.into(), cx);
    (table.expect("the table"), vcx)
}

fn redraw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The delegate's tooltip of cell (`ix`, `col`) with `want_tooltip` as the table sets it.
fn delegate_tip(
    vcx: &mut VisualTestContext,
    table: &Entity<TlTreeTable<ProcessDelegate>>,
    ix: usize,
    col: ColumnId,
    want_tooltip: bool,
) -> Option<String> {
    vcx.update(|_, cx| {
        let mut out = CellOut {
            want_tooltip,
            ..CellOut::default()
        };
        table.read(cx).delegate.cell(ix, col, &mut out);
        out.tooltip.map(|t| t.to_string())
    })
}

#[gpui_kit::test]
fn delegate_sets_cell_tooltip_on_the_hovered_row_only(cx: &mut TestAppContext) {
    let (table, mut vcx) = open(cx);
    assert_eq!(
        delegate_tip(&mut vcx, &table, 0, ColumnId::Memory, true).as_deref(),
        Some(APP_MEMORY)
    );
    assert_eq!(
        delegate_tip(&mut vcx, &table, 0, ColumnId::Memory, false),
        None,
        "rows that are not hovered build no tooltip"
    );
    assert_eq!(
        delegate_tip(&mut vcx, &table, 1, ColumnId::Memory, true),
        None,
        "a process row's Memory cell has none"
    );
    assert_eq!(
        delegate_tip(&mut vcx, &table, 1, ColumnId::Cmdline, true).as_deref(),
        Some("/usr/lib/firefox/firefox --new-window")
    );
    assert_eq!(delegate_tip(&mut vcx, &table, 1, ColumnId::Cpu, true), None);
}

#[gpui_kit::test]
fn hovered_memory_cell_shows_its_tooltip_at_600ms(cx: &mut TestAppContext) {
    let (table, mut vcx) = open(cx);
    vcx.update(|window, _| window.activate_window());
    redraw(&mut vcx);

    let name = vcx
        .debug_bounds("TlNameCell")
        .expect("the process row's Name cell");
    let x = vcx.update(|_, cx| {
        let layout = table.read(cx).delegate.columns();
        let pos = layout
            .visible_columns()
            .iter()
            .position(|&c| c == ColumnId::Memory)
            .expect("Memory is visible by default");
        layout.offsets()[pos] + layout.width(ColumnId::Memory) / 2.
    });
    // `debug_bounds` keeps the last Name cell drawn, the process row's; the app row is above.
    let at = gpui_kit::point(name.left() + x, name.center().y - name.size.height);
    // The first move sets the hovered row, the second makes its tooltips ready, the third
    // starts the delay.
    for _ in 0..3 {
        vcx.simulate_mouse_move(at, None, Modifiers::none());
        redraw(&mut vcx);
    }
    assert!(
        vcx.debug_bounds("TlTooltip").is_none(),
        "not before the delay"
    );
    assert_eq!(DELAY_PROCESS_CELL, std::time::Duration::from_millis(600));
    vcx.executor().advance_clock(DELAY_CONTROL);
    redraw(&mut vcx);
    assert!(
        vcx.debug_bounds("TlTooltip").is_none(),
        "not at 500 ms, the delay of other tables"
    );
    vcx.executor()
        .advance_clock(DELAY_PROCESS_CELL - DELAY_CONTROL);
    redraw(&mut vcx);
    let tip = vcx
        .debug_bounds("TlTooltip")
        .expect("the Memory cell of the app row shows its tooltip at 600 ms");
    assert!(
        tip.size.width > px(200.),
        "the sentence, not the cut value: {tip:?}"
    );
}

#[gpui_kit::test]
fn delegate_cell_tip_delay_is_600ms(cx: &mut TestAppContext) {
    let (table, mut vcx) = open(cx);
    vcx.update(|_, cx| {
        let d = &table.read(cx).delegate;
        assert_eq!(d.cell_tip_delay(), std::time::Duration::from_millis(600));
    });
}
