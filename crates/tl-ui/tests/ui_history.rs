//! History mode page: header text and render on the HISTORY bit only (tasklens-8fx.19), and the
//! query controller with one Frame query in flight (tasklens-8fx.20), and the History table
//! (tasklens-8fx.22), and queries on strip window, metric and selection changes
//! (tasklens-8fx.38). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use tl_model::client::EngineClient;
use tl_model::command::{Command, HistoryWhat};
use tl_model::details::DetailsSnapshot;
use tl_model::history::{
    HistoryMetric, HistoryPeaks, HistoryProcessRow, HistoryRange, HistoryStoreState, HistoryTotals,
    HistoryView,
};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::settings::{AppearanceSettings, Settings, SortState, ThemeSettings, UiState};
use tl_model::snapshot::ComponentHeader;
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::Validity;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::hub::{GlobalHub, SnapshotHub};
use tl_ui::pages::processes::history::table::HistoryTableDelegate;
use tl_ui::pages::processes::history::{
    HistoryPage, HistoryUiState, HistoryWords, default_view_range, header_text,
};
use tl_ui::settings_global::TlSettings;
use tl_ui::table::cells::CellOut;
use tl_ui::table::tree_table::{TlTreeTable, TreeTableDelegate};
use tl_ui::testing::fake_engine::FakeEngine;

const S: TimeNs = 1_000_000_000;

/// A view with a frame, 213 processes and a span ending at `newest`.
fn view(newest: TimeNs) -> HistoryView {
    HistoryView {
        span: Some((0, newest)),
        frame_t: Some(newest),
        process_count: 213,
        ..HistoryView::default()
    }
}

fn text(view: Option<&HistoryView>, s: &HistoryUiState) -> String {
    let mut out = String::new();
    header_text(&HistoryWords::default(), view, s, &mut out);
    out
}

fn scrubbed(at: TimeNs) -> HistoryUiState {
    HistoryUiState {
        scrub: Some(at),
        ..HistoryUiState::default()
    }
}

#[test]
fn history_header_text_states() {
    let live = HistoryUiState::default();
    assert_eq!(text(None, &live), "Collecting first history snapshot...");
    assert_eq!(
        text(Some(&HistoryView::default()), &live),
        "Collecting first history snapshot..."
    );

    let v = view(10_000 * S);
    assert_eq!(
        text(Some(&v), &live),
        "History - Drag scrubber below to view timeline"
    );
    assert_eq!(
        text(Some(&v), &scrubbed(10_000 * S - 252 * S)),
        "History Snapshot: 213 processes - 4 min 12 sec ago"
    );

    // Boot-clock 1000 s is today 12:01:30 local.
    let from = 1000 * S;
    let noon = jiff::Zoned::now()
        .with()
        .hour(12)
        .minute(1)
        .second(30)
        .subsec_nanosecond(0)
        .build()
        .expect("valid local time");
    let wall_ns = i64::try_from(noon.timestamp().as_nanosecond()).expect("fits i64");
    let v = HistoryView {
        realtime_offset_ns: wall_ns - from as i64,
        ..view(10_000 * S)
    };
    let range = HistoryUiState {
        scrub: Some(from),
        range: Some((from, from + 162 * S)),
        ..HistoryUiState::default()
    };
    assert_eq!(
        text(Some(&v), &range),
        "History Range: 12:01:30 - 12:04:12 (2 min 42 sec), averages"
    );

    assert_eq!(HistoryWords::default().loading.as_ref(), "Loading history…");
}

#[test]
fn header_age_formats() {
    let now = 10_000 * S;
    let v = view(now);
    let at = |age: TimeNs| text(Some(&v), &scrubbed(now - age * S));
    assert_eq!(at(45), "History Snapshot: 213 processes - 45 sec ago");
    assert_eq!(
        at(252),
        "History Snapshot: 213 processes - 4 min 12 sec ago"
    );
    assert_eq!(at(3900), "History Snapshot: 213 processes - 1 h 5 min ago");

    // The count is grouped with a thin space.
    let big = HistoryView {
        process_count: 1234,
        ..view(now)
    };
    assert_eq!(
        text(Some(&big), &scrubbed(now - 5 * S)),
        "History Snapshot: 1\u{2009}234 processes - 5 sec ago"
    );
}

#[test]
fn default_view_range_is_last_ten_minutes_or_whole_span() {
    assert_eq!(default_view_range((0, 3600 * S)), (3000 * S, 3600 * S));
    assert_eq!(default_view_range((100 * S, 400 * S)), (100 * S, 400 * S));
}

#[gpui_kit::test]
fn history_renders_only_on_history_bit(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        tl_ui::i18n::init_strings(cx, "en-US");
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        cx.set_global(GlobalHub(hub));
    });
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let count =
        |vcx: &mut gpui_kit::VisualTestContext| vcx.read_entity(&page, |p, _| p.render_count.get());
    let first = count(vcx);
    assert!(first >= 1);
    assert_eq!(
        vcx.read_entity(&page, |p, _| p.header().to_string()),
        "Collecting first history snapshot..."
    );

    // A change without the HISTORY bit.
    engine.publish_with(|s| {
        s.details = Arc::new(DetailsSnapshot {
            header: ComponentHeader {
                version: 1,
                ..ComponentHeader::default()
            },
            ..DetailsSnapshot::default()
        });
    });
    vcx.run_until_parked();
    assert_eq!(count(vcx), first);

    // A change with it.
    engine.publish_with(|s| {
        s.history = Arc::new(HistoryView {
            header: ComponentHeader {
                version: 1,
                ..ComponentHeader::default()
            },
            store_state: HistoryStoreState::Ready,
            ..view(700 * S)
        });
    });
    vcx.run_until_parked();
    assert!(count(vcx) > first);
    vcx.read_entity(&page, |p, _| {
        assert_eq!(
            p.header().as_ref(),
            "History - Drag scrubber below to view timeline"
        );
        assert_eq!(p.state().view_range, (100 * S, 700 * S));
    });
}

fn init(cx: &mut TestAppContext) -> Arc<FakeEngine> {
    let engine = FakeEngine::new();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        tl_ui::i18n::init_strings(cx, "en-US");
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        cx.set_global(GlobalHub(hub));
    });
    engine
}

/// The `(id, range, what)` of every history query sent.
fn queries(engine: &FakeEngine) -> Vec<(u64, HistoryRange, HistoryWhat)> {
    engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::HistoryQuery { id, range, what } => Some((id, range, what)),
            _ => None,
        })
        .collect()
}

fn overview(what: &HistoryWhat) -> bool {
    matches!(what, HistoryWhat::Frame { overview: true, .. })
}

/// Publishes a history view answering query `id`.
fn answer(engine: &FakeEngine, id: u64, vcx: &mut VisualTestContext) {
    engine.publish_with(|s| {
        let version = s.history.header.version + 1;
        s.history = Arc::new(HistoryView {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            query_id: id,
            ..view(700 * S)
        });
    });
    vcx.run_until_parked();
}

fn advance(vcx: &mut VisualTestContext, d: Duration) {
    vcx.executor().advance_clock(d);
    vcx.run_until_parked();
}

fn scrub(page: &Entity<HistoryPage>, vcx: &mut VisualTestContext, t: TimeNs) {
    page.update(vcx, |p, cx| p.set_scrub(Some(t), cx));
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn history_one_query_in_flight(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let sent = queries(&engine);
    assert_eq!(sent.len(), 1, "mode entry sends one query");
    assert_eq!(sent[0].0, 1);
    assert_eq!(sent[0].1, HistoryRange::Live);

    for t in [100, 200, 300] {
        scrub(&page, vcx, t * S);
    }
    assert_eq!(queries(&engine).len(), 1);
    assert_eq!(vcx.read_entity(&page, |p, _| p.query_in_flight()), Some(1));
}

#[gpui_kit::test]
fn pending_replaced_by_latest(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    for t in [100, 200, 300] {
        scrub(&page, vcx, t * S);
    }
    let pending = vcx.read_entity(&page, |p, _| p.query_pending());
    assert_eq!(pending.map(|(r, _)| r), Some(HistoryRange::At(300 * S)));

    answer(&engine, 1, vcx);
    let sent = queries(&engine);
    assert_eq!(sent.len(), 2, "exactly one command after the answer");
    assert_eq!(sent[1].1, HistoryRange::At(300 * S));
    vcx.read_entity(&page, |p, _| {
        assert_eq!(p.query_in_flight(), Some(sent[1].0));
        assert_eq!(p.query_pending(), None);
    });
}

#[gpui_kit::test]
fn foreign_answer_ignored(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    scrub(&page, vcx, 100 * S);
    answer(&engine, 99, vcx);
    assert_eq!(vcx.read_entity(&page, |p, _| p.query_in_flight()), Some(1));
    assert_eq!(queries(&engine).len(), 1);
}

#[gpui_kit::test]
fn live_queries_each_second(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (_page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    for _ in 0..5 {
        let last = queries(&engine).last().expect("a query").0;
        answer(&engine, last, vcx);
        advance(vcx, Duration::from_secs(1));
    }
    let sent = queries(&engine);
    assert_eq!(sent.len(), 6, "mode entry plus 5 ticks");
    assert!(sent[1..].iter().all(|(_, r, _)| *r == HistoryRange::Live));
}

#[gpui_kit::test]
fn overview_once_per_minute(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (_page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    for _ in 0..60 {
        let last = queries(&engine).last().expect("a query").0;
        answer(&engine, last, vcx);
        advance(vcx, Duration::from_secs(1));
    }
    let sent = queries(&engine);
    assert_eq!(sent.len(), 61);
    assert!(overview(&sent[0].2), "mode entry asks for the overview");
    assert!(sent[1..60].iter().all(|(_, _, w)| !overview(w)));
    assert!(overview(&sent[60].2), "asked again after 60 s");
}

#[gpui_kit::test]
fn busy_retried_next_tick(cx: &mut TestAppContext) {
    let engine = init(cx);
    engine.set_busy(true);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    assert!(queries(&engine).is_empty());
    assert!(vcx.read_entity(&page, |p, _| p.query_pending()).is_some());

    // Live: the 1 s tick sends it, still with the overview.
    engine.set_busy(false);
    advance(vcx, Duration::from_secs(1));
    let sent = queries(&engine);
    assert_eq!(sent.len(), 1);
    assert!(overview(&sent[0].2));

    // Scrubbed: the retry wake sends it.
    answer(&engine, sent[0].0, vcx);
    engine.set_busy(true);
    scrub(&page, vcx, 100 * S);
    assert_eq!(queries(&engine).len(), 1);
    engine.set_busy(false);
    advance(vcx, Duration::from_secs(1));
    let sent = queries(&engine);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].1, HistoryRange::At(100 * S));
}

#[gpui_kit::test]
fn unanswered_query_resent_after_timeout(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    answer(&engine, 1, vcx);
    scrub(&page, vcx, 100 * S);
    advance(vcx, Duration::from_millis(999));
    assert_eq!(queries(&engine).len(), 2);
    advance(vcx, Duration::from_millis(1));
    let sent = queries(&engine);
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2].1, HistoryRange::At(100 * S));
    assert_eq!(
        vcx.read_entity(&page, |p, _| p.query_in_flight()),
        Some(sent[2].0)
    );
}

const METRICS: [ColumnId; 5] = [
    ColumnId::Cpu,
    ColumnId::Memory,
    ColumnId::Disk,
    ColumnId::Network,
    ColumnId::Gpu,
];

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        start_ticks: u64::from(pid) * 10,
        boot_id: BootId(1),
        pid,
    }
}

/// A present row with `cpu` percent and `pid` MiB of memory.
fn prow(pid: u32, name: &str, cpu: f32) -> HistoryProcessRow {
    HistoryProcessRow {
        key: key(pid),
        name: Arc::from(name),
        comm: Arc::from(name),
        user: Arc::from("alice"),
        exe: Some(Arc::from(format!("/usr/bin/{name}").as_str())),
        app_id: None,
        system: false,
        present: true,
        previous_boot: false,
        cpu_pct: cpu,
        mem_anon_bytes: u64::from(pid) << 20,
        disk_bps: 0.0,
        net_bps: 0.0,
        gpu_pct: 0.0,
        peak: None,
    }
}

/// Publishes a view at a scrub second with `rows`.
fn publish_rows(
    engine: &FakeEngine,
    vcx: &mut VisualTestContext,
    range: HistoryRange,
    rows: Vec<HistoryProcessRow>,
) {
    engine.publish_with(|s| {
        let version = s.history.header.version + 1;
        s.history = Arc::new(HistoryView {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            store_state: HistoryStoreState::Ready,
            range,
            totals: Some(HistoryTotals {
                cpu_pct: 12.5,
                mem_used_bytes: 4 << 30,
                mem_total_bytes: 16 << 30,
            }),
            rows: rows.into(),
            ..view(700 * S)
        });
    });
    vcx.run_until_parked();
}

/// Runs `f` on the page's table.
fn with_table<R>(
    page: &Entity<HistoryPage>,
    vcx: &mut VisualTestContext,
    f: impl FnOnce(&TlTreeTable<HistoryTableDelegate>) -> R,
) -> R {
    vcx.read_entity(page, |p, cx| f(p.table().read(cx)))
}

/// Value, text, validity and tooltip of cell (`ix`, `col`), as on the hovered row.
fn cell(d: &HistoryTableDelegate, ix: usize, col: ColumnId) -> (String, Validity, Option<String>) {
    let mut out = CellOut {
        want_text: true,
        want_tooltip: true,
        ..CellOut::default()
    };
    d.cell(ix, col, &mut out);
    let text = out.arc.as_deref().map_or(out.text.clone(), str::to_owned);
    (text, out.validity, out.tooltip.map(|t| t.to_string()))
}

/// Installs `TlSettings` from `settings`, with an empty state.
fn init_settings(cx: &mut TestAppContext, engine: &Arc<FakeEngine>, settings: Settings) {
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        cx.set_global(EngineGlobal(client));
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
}

#[gpui_kit::test]
fn history_table_default_sort_cpu_desc(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    // CPU 0.0 to 19.0 in a scrambled order.
    let rows = (0..20u32)
        .map(|i| prow(100 + i, &format!("p{i}"), ((i * 7) % 20) as f32))
        .collect();
    publish_rows(&engine, vcx, HistoryRange::At(600 * S), rows);
    with_table(&page, vcx, |t| {
        let d = &t.delegate;
        assert_eq!(d.row_count(), 20);
        let cpu: Vec<f32> = (0..20).map(|ix| d.row(ix).cpu_pct).collect();
        let want: Vec<f32> = (0..20).rev().map(|v| v as f32).collect();
        assert_eq!(cpu, want);
        assert_eq!(d.sort().column, ColumnId::Cpu);
        assert!(!d.sort().ascending);
        let labels: Vec<String> = d
            .columns()
            .visible_columns()
            .iter()
            .map(|c| d.column_spec(*c).label.to_string())
            .collect();
        assert_eq!(
            labels,
            [
                "Name",
                "PID",
                "User",
                "Image path",
                "CPU",
                "Memory",
                "Disk",
                "Network",
                "GPU"
            ]
        );
        let widths: Vec<f32> = d
            .columns()
            .visible_columns()
            .iter()
            .map(|c| f32::from(d.columns().width(*c)))
            .collect();
        assert_eq!(
            widths,
            [248.0, 96.0, 92.0, 200.0, 63.0, 92.0, 82.0, 92.0, 72.0]
        );

        // The live table's formats, and the header totals at the scrub time.
        assert_eq!(cell(d, 0, ColumnId::Cpu).0, "19.0%");
        assert_eq!(cell(d, 0, ColumnId::Pid).0, d.row(0).key.pid.to_string());
        assert_eq!(cell(d, 0, ColumnId::ImagePath).0, "/usr/bin/p17");
        let total = |col| {
            let mut out = CellOut {
                want_text: true,
                ..CellOut::default()
            };
            let shown = d.header_total(col, &mut out);
            (shown, out.text, out.validity)
        };
        assert_eq!(
            total(ColumnId::Cpu),
            (true, "12.5%".into(), Validity::Valid)
        );
        assert_eq!(
            total(ColumnId::Memory),
            (true, "25.0%".into(), Validity::Valid)
        );
        assert_eq!(total(ColumnId::Disk).2, Validity::Missing);
        assert!(!total(ColumnId::Name).0);
        assert_eq!(
            d.column_spec(ColumnId::Name).tooltip.as_deref(),
            Some("Values at the selected time")
        );
    });
}

#[gpui_kit::test]
fn history_missing_identity_row_marked(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let mut gone = prow(7, "gone", 99.0);
    gone.present = false;
    let rows = vec![
        gone,
        prow(1, "a", 1.0),
        prow(2, "b", 2.0),
        prow(3, "c", 3.0),
    ];
    publish_rows(&engine, vcx, HistoryRange::At(600 * S), rows);

    // Selecting it reaches the page state in selection order.
    let table = vcx.read_entity(&page, |p, _| p.table().clone());
    table.update(vcx, |t, cx| {
        t.selection.click(key(2));
        t.selection.ctrl_click(key(7));
        cx.notify();
    });
    vcx.run_until_parked();
    assert_eq!(
        vcx.read_entity(&page, |p, _| p.state().selected.clone()),
        vec![key(2), key(7)]
    );

    with_table(&page, vcx, |t| {
        let d = &t.delegate;
        let last = d.row_count() - 1;
        assert_eq!(d.row_id(last), key(7));
        for col in METRICS {
            let (text, validity, tip) = cell(d, last, col);
            assert_eq!(validity, Validity::Missing, "{col:?}");
            assert!(text.is_empty());
            assert_eq!(tip.as_deref(), Some("No longer present"));
        }
        let name = d.name_cell(last);
        assert_eq!(name.primary.as_ref(), "gone");
        assert_eq!(name.secondary.as_deref(), Some("Process not running"));
        assert!(name.glyphs.is_empty());
        assert!(d.name_cell(0).secondary.is_none());
    });
}

#[gpui_kit::test]
fn previous_boot_marked(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let mut old = prow(5, "old", 0.0);
    old.present = false;
    old.previous_boot = true;
    publish_rows(
        &engine,
        vcx,
        HistoryRange::At(600 * S),
        vec![prow(1, "a", 1.0), old],
    );
    with_table(&page, vcx, |t| {
        let d = &t.delegate;
        let ix = d.row_index(key(5)).expect("row shown");
        let name = d.name_cell(ix);
        assert_eq!(name.secondary.as_deref(), Some("(previous boot)"));
        assert_eq!(name.glyphs.len(), 1);
        assert_eq!(name.glyphs[0].0, IconName::RotateCcwClock);
    });
}

#[gpui_kit::test]
fn range_peak_tooltip(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let mut busy = prow(3, "busy", 10.0);
    busy.peak = Some(HistoryPeaks {
        cpu_pct: 42.1,
        mem_anon_bytes: 3 << 20,
        disk_bps: 0.0,
        net_bps: 0.0,
        gpu_pct: 0.0,
    });
    let range = HistoryRange::Span {
        from: 100 * S,
        to: 200 * S,
    };
    publish_rows(&engine, vcx, range, vec![busy]);
    with_table(&page, vcx, |t| {
        let d = &t.delegate;
        let (text, _, tip) = cell(d, 0, ColumnId::Cpu);
        assert_eq!(text, "10.0%");
        assert_eq!(tip.as_deref(), Some("peak 42.1%"));
        assert_eq!(
            cell(d, 0, ColumnId::Memory).2.as_deref(),
            Some("peak 3.00 MiB")
        );
        assert_eq!(
            d.column_spec(ColumnId::Name).tooltip.as_deref(),
            Some("Averages over the selected range (peak in cell tooltips)")
        );
        // Off the hovered row no tooltip is formatted.
        let mut out = CellOut::default();
        d.cell(0, ColumnId::Cpu, &mut out);
        assert!(out.tooltip.is_none());
    });
}

#[gpui_kit::test]
fn hide_system_filters_history(cx: &mut TestAppContext) {
    let engine = init(cx);
    let mut settings = Settings::default();
    settings.processes.hide_system = true;
    init_settings(cx, &engine, settings);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let mut sys = prow(1, "systemd", 5.0);
    sys.system = true;
    let rows = vec![sys, prow(2, "firefox", 1.0), prow(3, "bash", 2.0)];
    publish_rows(&engine, vcx, HistoryRange::At(600 * S), rows);
    let shown = |vcx: &mut VisualTestContext| {
        with_table(&page, vcx, |t| {
            let d = &t.delegate;
            (0..d.row_count())
                .map(|ix| d.row_id(ix).pid)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(shown(vcx), [3, 2]);

    vcx.update(|_, cx| TlSettings::update(cx, |d| d.settings.processes.hide_system = false));
    vcx.run_until_parked();
    assert_eq!(shown(vcx), [1, 3, 2]);

    // The filter matches name, comm, user and executable path without case.
    page.update(vcx, |p, cx| p.set_filter("FIRE", cx));
    vcx.run_until_parked();
    assert_eq!(shown(vcx), [2]);
    page.update(vcx, |p, cx| p.set_filter("/usr/bin/ba", cx));
    vcx.run_until_parked();
    assert_eq!(shown(vcx), [3]);
    page.update(vcx, |p, cx| p.set_filter("", cx));
    vcx.run_until_parked();
    assert_eq!(shown(vcx), [1, 3, 2]);
}

#[gpui_kit::test]
fn history_sort_persisted(cx: &mut TestAppContext) {
    let engine = init(cx);
    init_settings(cx, &engine, Settings::default());
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let rows = vec![prow(3, "c", 1.0), prow(1, "a", 3.0), prow(2, "b", 2.0)];
    publish_rows(&engine, vcx, HistoryRange::At(600 * S), rows);
    let table = vcx.read_entity(&page, |p, _| p.table().clone());
    // Memory is numeric: the first click sorts descending, the second ascending.
    for _ in 0..2 {
        table.update(vcx, |t, cx| t.sort_by_column(ColumnId::Memory, cx));
    }
    vcx.run_until_parked();
    let saved = vcx.update(|_, cx| TlSettings::state(cx).sort.history.clone());
    assert_eq!(
        saved,
        SortState {
            column: "memory".into(),
            ascending: true,
        }
    );
    with_table(&page, vcx, |t| {
        let d = &t.delegate;
        let pids: Vec<u32> = (0..d.row_count()).map(|ix| d.row_id(ix).pid).collect();
        assert_eq!(pids, [1, 2, 3]);
    });
}

/// A change to one Frame input of the page.
type Change = dyn Fn(&mut HistoryPage, &mut gpui_kit::Context<HistoryPage>);

/// Answers the query in flight, if any.
fn answer_last(engine: &FakeEngine, page: &Entity<HistoryPage>, vcx: &mut VisualTestContext) {
    if let Some(id) = vcx.read_entity(page, |p, _| p.query_in_flight()) {
        answer(engine, id, vcx);
    }
}

#[gpui_kit::test]
fn scrubbed_frame_inputs_send_one_query_each(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    answer_last(&engine, &page, vcx);
    scrub(&page, vcx, 100 * S);
    answer_last(&engine, &page, vcx);
    let base = queries(&engine).len();

    let changes: [&Change; 3] = [
        &|p, cx| p.set_view_range((50 * S, 150 * S), cx),
        &|p, cx| p.set_metric(HistoryMetric::Memory, cx),
        &|p, cx| p.set_selected(vec![key(4), key(2)], cx),
    ];
    for (i, change) in changes.iter().enumerate() {
        page.update(vcx, |p, cx| change(p, cx));
        vcx.run_until_parked();
        let sent = queries(&engine);
        assert_eq!(sent.len(), base + i + 1, "change {i} sends one query");
        assert_eq!(sent.last().map(|q| q.1), Some(HistoryRange::At(100 * S)));
        answer_last(&engine, &page, vcx);
        // The same value again sends nothing.
        page.update(vcx, |p, cx| change(p, cx));
        vcx.run_until_parked();
        assert_eq!(
            queries(&engine).len(),
            base + i + 1,
            "repeat {i} sends none"
        );
    }
    let Some((
        _,
        _,
        HistoryWhat::Frame {
            metric, view, keys, ..
        },
    )) = queries(&engine).pop()
    else {
        panic!("a Frame query");
    };
    assert_eq!(metric, HistoryMetric::Memory);
    assert_eq!(view, (50 * S, 150 * S));
    assert_eq!(&keys[..], [key(4), key(2)]);

    // With one in flight, a newer change replaces the pending query.
    page.update(vcx, |p, cx| p.set_metric(HistoryMetric::Disk, cx));
    page.update(vcx, |p, cx| p.set_metric(HistoryMetric::Gpu, cx));
    vcx.run_until_parked();
    assert_eq!(queries(&engine).len(), base + 4);
    let pending = vcx.read_entity(&page, |p, _| p.query_pending());
    assert!(matches!(
        pending,
        Some((
            _,
            HistoryWhat::Frame {
                metric: HistoryMetric::Gpu,
                ..
            }
        ))
    ));
}

#[gpui_kit::test]
fn live_frame_inputs_wait_for_tick(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    answer_last(&engine, &page, vcx);
    let base = queries(&engine).len();
    page.update(vcx, |p, cx| {
        p.set_view_range((50 * S, 150 * S), cx);
        p.set_metric(HistoryMetric::Network, cx);
        p.set_selected(vec![key(1)], cx);
    });
    vcx.run_until_parked();
    assert_eq!(queries(&engine).len(), base, "nothing before the tick");
    advance(vcx, Duration::from_secs(1));
    let sent = queries(&engine);
    assert_eq!(sent.len(), base + 1, "the tick sends one");
    assert!(matches!(
        sent.last(),
        Some((
            _,
            HistoryRange::Live,
            HistoryWhat::Frame {
                metric: HistoryMetric::Network,
                ..
            }
        ))
    ));
}
