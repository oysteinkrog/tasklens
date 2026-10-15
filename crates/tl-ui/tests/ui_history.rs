//! History mode page: header text and render on the HISTORY bit only (tasklens-8fx.19), and the
//! query controller with one Frame query in flight (tasklens-8fx.20), and the History table
//! (tasklens-8fx.22), and queries on strip window, metric and selection changes
//! (tasklens-8fx.38), and the timeline strip's cached geometry and gap bands (tasklens-8fx.24), and
//! the filter text typed before the page existed (tasklens-8fx.39), and the top-3 hover card
//! (tasklens-8fx.26), and the minimap (tasklens-8fx.27).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::{
    Action, AppContext as _, Entity, KeyContext, Keystroke, TestAppContext, VisualTestContext,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, HistoryWhat};
use tl_model::details::DetailsSnapshot;
use tl_model::history::{
    HistoryMetric, HistoryPeaks, HistoryPoint, HistoryProcessRow, HistoryRange, HistorySeries,
    HistoryStackBucket, HistoryStoreState, HistoryTop, HistoryTopRow, HistoryTotals, HistoryView,
};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::settings::{AppearanceSettings, Settings, SortState, ThemeSettings, UiState};
use tl_model::snapshot::ComponentHeader;
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::Validity;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::hub::{GlobalHub, SnapshotHub};
use tl_ui::pages::processes::history::actions::{HistoryLive, bind_keys_history};
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

/// Paints the window once.
fn draw(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

/// A view at `version` with a CPU total, one selected series and a stack over 100..700 s, and
/// the given gaps.
fn strip_view(version: u64, gaps: Vec<(TimeNs, TimeNs)>) -> HistoryView {
    let points: Vec<HistoryPoint> = (100..=700u64)
        .step_by(10)
        .map(|t| HistoryPoint {
            t_ns: t * S,
            min: (t % 50) as f32,
            max: (t % 50) as f32 + 5.0,
        })
        .collect();
    let stack: Vec<HistoryStackBucket> = (100..=700u64)
        .step_by(10)
        .map(|t| HistoryStackBucket {
            t_ns: t * S,
            top: [10.0, 5.0, 2.0],
            other: 3.0,
        })
        .collect();
    let points: Arc<[HistoryPoint]> = points.into();
    HistoryView {
        header: ComponentHeader {
            version,
            ..ComponentHeader::default()
        },
        store_state: HistoryStoreState::Ready,
        series: vec![
            HistorySeries {
                key: None,
                metric: HistoryMetric::Cpu,
                points: Arc::clone(&points),
            },
            HistorySeries {
                key: Some(key(7)),
                metric: HistoryMetric::Cpu,
                points,
            },
        ]
        .into(),
        stack: stack.into(),
        gaps: gaps.into(),
        ..view(700 * S)
    }
}

fn publish_view(engine: &FakeEngine, vcx: &mut VisualTestContext, v: HistoryView) {
    engine.publish_with(|s| s.history = Arc::new(v));
    vcx.run_until_parked();
}

fn builds(page: &Entity<HistoryPage>, vcx: &mut VisualTestContext) -> u32 {
    vcx.read_entity(page, |p, _| p.scrubber().build_count)
}

#[gpui_kit::test]
fn scrubber_geometry_cached_until_key_changes(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    publish_view(&engine, vcx, strip_view(1, Vec::new()));
    draw(vcx);
    let first = builds(&page, vcx);
    assert!(first >= 1);
    // The same view, size and range: no rebuild.
    draw(vcx);
    draw(vcx);
    assert_eq!(builds(&page, vcx), first);
    // Moving the handle or the hover hairline builds nothing either.
    page.update(vcx, |p, cx| p.set_scrub(Some(400 * S), cx));
    vcx.run_until_parked();
    draw(vcx);
    assert_eq!(builds(&page, vcx), first);
    // A new view version rebuilds once.
    publish_view(&engine, vcx, strip_view(2, Vec::new()));
    draw(vcx);
    draw(vcx);
    assert_eq!(builds(&page, vcx), first + 1);
    // So does a new strip window.
    page.update(vcx, |p, cx| p.set_view_range((200 * S, 700 * S), cx));
    vcx.run_until_parked();
    draw(vcx);
    draw(vcx);
    assert_eq!(builds(&page, vcx), first + 2);
    // Labels count back from the newest second, with "now" at the right edge.
    let labels = vcx.read_entity(&page, |p, _| p.scrubber().label_texts());
    assert_eq!(labels.last().map(String::as_str), Some("now"));
    assert!(labels.iter().any(|l| l == "-5:00"), "{labels:?}");
}

#[gpui_kit::test]
fn scrubber_draws_gap_band(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    publish_view(&engine, vcx, strip_view(1, Vec::new()));
    draw(vcx);
    assert!(vcx.read_entity(&page, |p, _| p.scrubber().painted_gaps.is_empty()));

    // 60 s and 120 s gaps inside the 100..700 s window.
    let gaps = vec![(200 * S, 260 * S), (400 * S, 520 * S)];
    publish_view(&engine, vcx, strip_view(2, gaps));
    draw(vcx);
    let bands = vcx.read_entity(&page, |p, _| p.scrubber().painted_gaps.to_vec());
    assert_eq!(bands.len(), 2);
    let w0 = f32::from(bands[0].end - bands[0].start);
    let w1 = f32::from(bands[1].end - bands[1].start);
    assert!(w0 > 0.0);
    assert!((w1 / w0 - 2.0).abs() < 0.05, "{w0} {w1}");
    // The second band starts 200 s after the first: 200 / 60 band widths to the right.
    let dx = f32::from(bands[1].start - bands[0].start);
    assert!((dx / w0 - 200.0 / 60.0).abs() < 0.05, "{dx} {w0}");
}

/// A filter typed on Processes before History mode starts applies to the new page's table, and
/// clearing it shows every row (tasklens-8fx.39). Nothing builds `HistoryPage` in the shell yet,
/// so making the page stands for turning History mode on.
#[gpui_kit::test]
fn filter_typed_before_history_mode_applies(cx: &mut TestAppContext) {
    use tl_model::command::Tab;
    use tl_ui::app::StartOptions;
    use tl_ui::components::toast;
    use tl_ui::demand::{DemandAggregator, GlobalDemand};
    use tl_ui::window::filter::FILTER_DEBOUNCE;
    use tl_ui::window::main::{self as shell, main_window_view, open_main_window};

    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        let appearance = AppearanceSettings::default();
        tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
        tl_ui::theme::metrics::init_metrics(&appearance, cx);
        tl_ui::i18n::init_strings(cx, "en-US");
        tl_ui::table::actions::bind_keys_table(cx);
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
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
        shell::init(cx);
    });
    let handle = cx.update(open_main_window).expect("the main window opens");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    let vcx = &mut vcx;
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();

    let filter = vcx.update(|_, cx| {
        let main = main_window_view(cx).expect("main window open");
        main.read(cx).filter_box().clone()
    });
    assert_eq!(vcx.read_entity(&filter, |f, _| f.tab()), Tab::Processes);
    vcx.update(|window, cx| {
        let input = filter.read(cx).input().clone();
        input.update(cx, |s, cx| s.focus(window, cx));
    });
    vcx.simulate_input("fire");
    vcx.executor().advance_clock(FILTER_DEBOUNCE * 2);
    vcx.run_until_parked();
    assert_eq!(
        vcx.read_entity(&filter, |f, _| f.text(Tab::Processes).to_string()),
        "fire"
    );

    let page = vcx.update(|window, cx| cx.new(|cx| HistoryPage::new(window, cx)));
    vcx.run_until_parked();
    let rows = vec![prow(2, "firefox", 1.0), prow(3, "bash", 2.0)];
    publish_rows(&engine, vcx, HistoryRange::At(600 * S), rows);
    let shown = |vcx: &mut VisualTestContext| {
        with_table(&page, vcx, |t| {
            let d = &t.delegate;
            (0..d.row_count())
                .map(|ix| d.row_id(ix).pid)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(shown(vcx), [2], "the earlier filter applies");

    vcx.update(|window, cx| filter.update(cx, |f, cx| f.clear(window, cx)));
    vcx.run_until_parked();
    assert_eq!(shown(vcx), [3, 2], "clearing shows every row");
}

/// A page with the History keys bound, a view whose span is 100..1000 s and the strip focused.
fn keys_page(cx: &mut TestAppContext) -> (Entity<HistoryPage>, &mut VisualTestContext) {
    let engine = init(cx);
    cx.update(bind_keys_history);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let v = HistoryView {
        header: ComponentHeader {
            version: 1,
            ..ComponentHeader::default()
        },
        span: Some((100 * S, 1000 * S)),
        frame_t: Some(1000 * S),
        store_state: HistoryStoreState::Ready,
        ..HistoryView::default()
    };
    publish_view(&engine, vcx, v);
    let focus = vcx.read_entity(&page, |p, _| p.strip_focus().clone());
    vcx.update(|window, cx| window.focus(&focus, cx));
    draw(vcx);
    (page, vcx)
}

fn scrub_of(page: &Entity<HistoryPage>, vcx: &mut VisualTestContext) -> Option<TimeNs> {
    vcx.read_entity(page, |p, _| p.state().scrub)
}

#[gpui_kit::test]
fn scrubber_keys_step_1s_1min(cx: &mut TestAppContext) {
    let (page, vcx) = keys_page(cx);
    vcx.simulate_keystrokes("left");
    assert_eq!(scrub_of(&page, vcx), Some(999 * S));
    vcx.simulate_keystrokes("shift-right");
    assert_eq!(scrub_of(&page, vcx), None);
    vcx.simulate_keystrokes("shift-left");
    assert_eq!(scrub_of(&page, vcx), Some(940 * S));
    vcx.simulate_keystrokes("right");
    assert_eq!(scrub_of(&page, vcx), Some(941 * S));
    page.update(vcx, |p, cx| p.set_scrub(Some(100 * S), cx));
    draw(vcx);
    vcx.simulate_keystrokes("left");
    assert_eq!(scrub_of(&page, vcx), Some(100 * S));
}

#[gpui_kit::test]
fn home_end_oldest_and_live(cx: &mut TestAppContext) {
    let (page, vcx) = keys_page(cx);
    vcx.simulate_keystrokes("home");
    assert_eq!(scrub_of(&page, vcx), Some(100 * S));
    vcx.simulate_keystrokes("end");
    assert_eq!(scrub_of(&page, vcx), None);
    page.update(vcx, |p, cx| {
        p.set_scrub(Some(500 * S), cx);
        p.set_range(Some((200 * S, 300 * S)), cx);
    });
    draw(vcx);
    vcx.simulate_keystrokes("end");
    let (scrub, range) = vcx.read_entity(&page, |p, _| (p.state().scrub, p.state().range));
    assert_eq!((scrub, range), (None, None));
}

#[gpui_kit::test]
fn esc_returns_live_before_global(cx: &mut TestAppContext) {
    let (page, vcx) = keys_page(cx);
    vcx.update(|_, cx| tl_ui::actions::bind_keys_global(cx));
    page.update(vcx, |p, cx| p.set_scrub(Some(500 * S), cx));
    draw(vcx);
    vcx.simulate_keystrokes("escape");
    assert_eq!(scrub_of(&page, vcx), None);
    // The keymap ranks the History rule above the global one only while scrubbed.
    let resolve = |vcx: &mut VisualTestContext, contexts: &[&str]| -> Option<Box<dyn Action>> {
        vcx.update(|_, cx| {
            let input = [Keystroke::parse("escape").unwrap()];
            let stack: Vec<KeyContext> = contexts
                .iter()
                .map(|c| KeyContext::parse(c).expect("key context"))
                .collect();
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let (bindings, _) = keymap.bindings_for_input(&input, &stack);
            bindings.first().map(|b| b.action().boxed_clone())
        })
    };
    let scrubbed = resolve(vcx, &["Global", "History scrubbed"]).expect("a binding");
    assert!(scrubbed.partial_eq(&HistoryLive));
    let live = resolve(vcx, &["Global", "History"]).expect("a binding");
    assert!(live.partial_eq(&tl_ui::actions::EscapeChain));
}

#[gpui_kit::test]
fn digit_keys_select_metric(cx: &mut TestAppContext) {
    let (page, vcx) = keys_page(cx);
    let metric = |page: &Entity<HistoryPage>, vcx: &mut VisualTestContext| {
        vcx.read_entity(page, |p, _| p.state().metric)
    };
    vcx.simulate_keystrokes("3");
    assert_eq!(metric(&page, vcx), HistoryMetric::Disk);
    vcx.simulate_keystrokes("5");
    assert_eq!(metric(&page, vcx), HistoryMetric::Gpu);
    vcx.simulate_keystrokes("1");
    assert_eq!(metric(&page, vcx), HistoryMetric::Cpu);
}

/// Row menu, jump and Delete from history (tasklens-8fx.23). The real main window gives the
/// toast queue and the Processes page; the History page sits in a second window, because
/// nothing mounts it in the shell yet (8fx.3).
mod row_actions {
    use tl_model::command::Tab;
    use tl_model::ids::GroupKey;
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::snapshot::ProcessSnapshot;
    use tl_model::validity::FieldValidity;
    use tl_ui::actions::SelectTab;
    use tl_ui::app::StartOptions;
    use tl_ui::components::icons::TlIcon;
    use tl_ui::components::menu::{MenuEntryInfo, MenuEntryKind, TlMenu};
    use tl_ui::demand::{DemandAggregator, GlobalDemand};
    use tl_ui::pages::processes::history::actions::{DeleteFromHistory, HistoryShowInLiveTable};
    use tl_ui::table::actions::OpenProperties;
    use tl_ui::table::tree_table::TreeTableEvent;
    use tl_ui::window::main::{self as shell, main_window_view, open_main_window};

    use super::*;

    struct Rig<'a> {
        engine: Arc<FakeEngine>,
        page: Entity<HistoryPage>,
        vcx: &'a mut VisualTestContext,
        main: VisualTestContext,
    }

    fn proc_record(pid: u32) -> ProcessRecord {
        let name: Arc<str> = Arc::from(format!("proc{pid}"));
        ProcessRecord {
            key: key(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: name.clone(),
                name: name.clone(),
                exe: None,
                cmdline: None,
                user: Arc::from("user"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: name,
            }),
            group: GroupKey(0),
            state: ProcState::Running,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 1.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.,
            io_write_bps: 0.,
            net_rx_bps: 0.,
            net_tx_bps: 0.,
            gpu_util_pct: 0.,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    /// History mode on, pids 2, 3 and 4 in the history table, only pid 2 running.
    fn rig(cx: &mut TestAppContext) -> Rig<'_> {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let mut state = UiState::default();
        state.processes.history_mode = true;
        cx.update(|cx| {
            gpui_kit::init(cx);
            let appearance = AppearanceSettings::default();
            tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
            tl_ui::theme::metrics::init_metrics(&appearance, cx);
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
            tl_ui::components::toast::init(cx);
            tl_ui::actions::bind_keys_global(cx);
            cx.set_global(StartOptions::default());
            shell::init(cx);
        });
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut main = VisualTestContext::from_window(handle.into(), cx);
        main.update(|window, _| window.activate_window());
        main.dispatch_action(SelectTab(Tab::Processes as usize));
        main.run_until_parked();
        engine.publish_with(|s| {
            s.processes = Arc::new(ProcessSnapshot {
                header: ComponentHeader {
                    version: 1,
                    ..ComponentHeader::default()
                },
                records: Arc::from(vec![proc_record(2)]),
                ..ProcessSnapshot::default()
            });
        });
        main.run_until_parked();
        let (page, vcx) = cx.add_window_view(HistoryPage::new);
        vcx.run_until_parked();
        publish_rows(
            &engine,
            vcx,
            HistoryRange::At(600 * S),
            vec![
                prow(2, "live", 3.0),
                prow(3, "gone", 2.0),
                prow(4, "other", 1.0),
            ],
        );
        let focus = vcx.read_entity(&page, |p, _| p.strip_focus().clone());
        vcx.update(|window, cx| window.focus(&focus, cx));
        draw(vcx);
        Rig {
            engine,
            page,
            vcx,
            main,
        }
    }

    impl Rig<'_> {
        fn select(&mut self, pids: &[u32]) {
            let keys = pids.iter().map(|&p| key(p)).collect();
            self.page.update(self.vcx, |p, cx| p.set_selected(keys, cx));
            self.vcx.run_until_parked();
        }

        fn toasts(&mut self) -> Vec<String> {
            self.main.update(|_, cx| {
                let main = main_window_view(cx).expect("main window");
                let queue = main.read(cx).toasts().clone();
                queue
                    .read(cx)
                    .visible_texts()
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            })
        }

        fn history_mode(&mut self) -> bool {
            self.vcx
                .update(|_, cx| TlSettings::state(cx).processes.history_mode)
        }

        fn deletes(&self) -> Vec<Vec<ProcessKey>> {
            self.engine
                .commands()
                .into_iter()
                .filter_map(|c| match c {
                    Command::HistoryDelete { keys } => Some(keys.to_vec()),
                    _ => None,
                })
                .collect()
        }

        fn menu(&mut self, targets: &[u32], col: Option<ColumnId>) -> Vec<MenuEntryInfo> {
            let keys: Vec<ProcessKey> = targets.iter().map(|&p| key(p)).collect();
            let table = self.vcx.read_entity(&self.page, |p, _| p.table().clone());
            let menu = self.vcx.update(|window, cx| {
                let focus = table.read(cx).focus_handle().clone();
                TlMenu::build(window, cx, focus, |menu, window, cx| {
                    table.update(cx, |t, cx| {
                        t.delegate.context_menu(&keys, col, menu, window, cx)
                    })
                })
            });
            self.vcx
                .update(|window, cx| menu.read(cx).entries_for_test(window))
        }

        /// Enter on the row of `pid`: the table's activate path, then `OpenProperties`.
        fn enter(&mut self, pid: u32) {
            let table = self.vcx.read_entity(&self.page, |p, _| p.table().clone());
            table.update(self.vcx, |t, cx| t.delegate.on_activate(key(pid), cx));
            self.vcx.dispatch_action(OpenProperties);
            self.vcx.run_until_parked();
        }
    }

    #[gpui_kit::test]
    fn history_menu_items(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let entries = r.menu(&[3], Some(ColumnId::Cpu));
        let kinds: Vec<MenuEntryKind> = entries.iter().map(|e| e.kind).collect();
        let labels: Vec<Option<String>> = entries
            .iter()
            .map(|e| e.label.as_ref().map(ToString::to_string))
            .collect();
        assert_eq!(entries.len(), 8, "{labels:?}");
        assert_eq!(labels[0].as_deref(), Some("Copy"));
        assert_eq!(labels[1].as_deref(), Some("Copy details"));
        assert_eq!(labels[2].as_deref(), Some("Copy all metrics"));
        assert_eq!(labels[3].as_deref(), Some("Copy rows as table"));
        assert_eq!(kinds[4], MenuEntryKind::Separator);
        assert_eq!(labels[5].as_deref(), Some("Show in live table"));
        assert_eq!(kinds[5], MenuEntryKind::Disabled, "pid 3 is not running");
        assert_eq!(kinds[6], MenuEntryKind::Separator);
        assert_eq!(labels[7].as_deref(), Some("Delete from history"));
        assert_eq!(entries[7].icon, Some(TlIcon::Trash));
        assert!(entries[7].danger);
        assert!(
            entries[7]
                .action
                .as_ref()
                .is_some_and(|a| a.partial_eq(&DeleteFromHistory))
        );

        let live = r.menu(&[2], Some(ColumnId::Cpu));
        assert_eq!(live[5].kind, MenuEntryKind::Item);
        assert!(
            live[5]
                .action
                .as_ref()
                .is_some_and(|a| a.partial_eq(&HistoryShowInLiveTable))
        );
        let keyboard = r.menu(&[2], None);
        assert_eq!(keyboard.len(), 7, "the keyboard menu has no Copy item");
    }

    #[gpui_kit::test]
    fn history_menu_close_drops_captured_rows(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.menu(&[3], Some(ColumnId::Cpu));
        let table = r.vcx.read_entity(&r.page, |p, _| p.table().clone());
        table.update(r.vcx, |t, cx| t.delegate.on_menu_closed(cx));
        // The selection changes after the menu is dismissed.
        r.select(&[2]);
        r.vcx.dispatch_action(DeleteFromHistory);
        r.vcx.run_until_parked();
        assert_eq!(
            r.deletes(),
            [vec![key(2)]],
            "the selected row, not the stale capture"
        );
    }

    #[gpui_kit::test]
    fn delete_from_history_removes_rows_series_and_chips(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.select(&[3]);
        r.vcx.dispatch_action(DeleteFromHistory);
        r.vcx.run_until_parked();
        assert_eq!(r.deletes(), [vec![key(3)]]);
        assert_eq!(r.toasts(), ["History entry deleted"]);
        let selected = r
            .vcx
            .read_entity(&r.page, |p, _| p.state().selected.clone());
        assert!(selected.is_empty(), "the chip is gone at once");
        let in_table = with_table(&r.page, r.vcx, |t| t.selection.len());
        assert_eq!(in_table, 0);
        // The next view has no such row.
        publish_rows(
            &r.engine,
            r.vcx,
            HistoryRange::At(600 * S),
            vec![prow(2, "live", 3.0), prow(4, "other", 1.0)],
        );
        let shown = with_table(&r.page, r.vcx, |t| {
            (0..t.delegate.row_count())
                .map(|ix| t.delegate.row_id(ix).pid)
                .collect::<Vec<_>>()
        });
        assert_eq!(shown, [2, 4]);
    }

    #[gpui_kit::test]
    fn delete_plural_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.select(&[2, 3, 4]);
        r.vcx.dispatch_action(DeleteFromHistory);
        r.vcx.run_until_parked();
        let deletes = r.deletes();
        assert_eq!(deletes.len(), 1);
        let mut pids: Vec<u32> = deletes[0].iter().map(|k| k.pid).collect();
        pids.sort_unstable();
        assert_eq!(pids, [2, 3, 4]);
        assert_eq!(r.toasts(), ["3 history entries deleted"]);
    }

    #[gpui_kit::test]
    fn enter_jumps_when_live(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        assert!(r.history_mode());
        r.enter(2);
        assert!(!r.history_mode(), "the jump leaves History mode");
        assert!(!r.toasts().iter().any(|t| t == "Process not running"));
    }

    #[gpui_kit::test]
    fn double_click_jumps_when_live(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let table = r.vcx.read_entity(&r.page, |p, _| p.table().clone());
        table.update(r.vcx, |_, cx| {
            cx.emit(TreeTableEvent::Activate { row_index: 0 });
        });
        r.vcx.run_until_parked();
        // Row 0 is the highest CPU row, pid 2, which is running.
        assert!(!r.history_mode());
    }

    #[gpui_kit::test]
    fn enter_toasts_when_not_live(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.enter(3);
        assert!(r.history_mode(), "the mode stays");
        assert_eq!(r.toasts(), ["Process not running"]);
    }

    #[gpui_kit::test]
    fn history_copy_details_has_no_command_line(cx: &mut TestAppContext) {
        let r = rig(cx);
        let table = r.vcx.read_entity(&r.page, |p, _| p.table().clone());
        let (details, all, cell) = r.vcx.update(|_, cx| {
            let keys = [key(2), key(3)];
            let d = &table.read(cx).delegate;
            (
                d.copy_details_text(&keys, cx),
                d.copy_all_metrics_text(&keys),
                d.copy_cell_text(&keys, ColumnId::Name),
            )
        });
        assert!(details.contains("Parent PID: \n"), "{details}");
        assert!(details.contains("Name: live"), "{details}");
        assert!(!details.contains("Command line"));
        assert!(!all.contains("Command line"));
        assert_eq!(cell, "live\ngone");
    }
}

/// The plot of the last paint.
fn plot(
    page: &Entity<HistoryPage>,
    vcx: &mut VisualTestContext,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    vcx.read_entity(page, |p, _| p.scrubber().plot_bounds)
}

/// A point at `frac` of the plot width, on the plot's vertical middle.
fn at(plot: gpui_kit::Bounds<gpui_kit::Pixels>, frac: f32) -> gpui_kit::Point<gpui_kit::Pixels> {
    gpui_kit::point(
        plot.left() + plot.size.width * frac,
        plot.top() + plot.size.height / 2.0,
    )
}

fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    draw(vcx);
}

#[gpui_kit::test]
fn click_scrubs_snapped(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton};
    let (page, vcx) = keys_page(cx);
    let plot = plot(&page, vcx);
    let p = at(plot, 0.25);
    vcx.simulate_mouse_move(p, None, Modifiers::none());
    vcx.simulate_mouse_down(p, MouseButton::Left, Modifiers::none());
    settle(vcx);
    vcx.simulate_mouse_up(p, MouseButton::Left, Modifiers::none());
    settle(vcx);
    // View 400..1000 s: 25% is 550 s.
    assert_eq!(scrub_of(&page, vcx), Some(550 * S));
}

#[gpui_kit::test]
fn shift_drag_sets_range(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton};
    let (page, vcx) = keys_page(cx);
    let plot = plot(&page, vcx);
    let (a, b) = (at(plot, 0.2), at(plot, 0.4));
    let shift = Modifiers::shift();
    vcx.simulate_mouse_move(a, None, shift);
    vcx.simulate_mouse_down(a, MouseButton::Left, shift);
    settle(vcx);
    vcx.simulate_mouse_move(b, MouseButton::Left, shift);
    settle(vcx);
    vcx.simulate_mouse_up(b, MouseButton::Left, shift);
    settle(vcx);
    let range = vcx.read_entity(&page, |p, _| p.state().range);
    assert_eq!(range, Some((520 * S, 640 * S)));
    let header = vcx.read_entity(&page, |p, _| p.header().to_string());
    assert!(header.starts_with("History Range:"), "{header}");
}

#[gpui_kit::test]
fn double_click_returns_live(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent};
    let (page, vcx) = keys_page(cx);
    page.update(vcx, |p, cx| {
        p.set_scrub(Some(500 * S), cx);
        p.set_range(Some((450 * S, 550 * S)), cx);
    });
    draw(vcx);
    let plot = plot(&page, vcx);
    let p = at(plot, 0.5);
    vcx.simulate_mouse_move(p, None, Modifiers::none());
    vcx.simulate_event(MouseDownEvent {
        position: p,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    settle(vcx);
    let (scrub, range) = vcx.read_entity(&page, |p, _| (p.state().scrub, p.state().range));
    assert_eq!((scrub, range), (None, None));
}

#[gpui_kit::test]
fn strip_focus_ring(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton};
    let (page, vcx) = keys_page(cx);
    vcx.simulate_keystrokes("left");
    settle(vcx);
    assert!(vcx.read_entity(&page, |p, _| p.scrubber().focus_ring_painted));
    // A click focuses without the keyboard, so the ring is not shown.
    let p = at(plot(&page, vcx), 0.5);
    vcx.simulate_mouse_move(p, None, Modifiers::none());
    vcx.simulate_mouse_down(p, MouseButton::Left, Modifiers::none());
    settle(vcx);
    assert!(!vcx.read_entity(&page, |p, _| p.scrubber().focus_ring_painted));
}

#[gpui_kit::test]
fn plot_click_on_a_bound_x_scrubs_and_clears_the_range(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton};
    let (page, vcx) = keys_page(cx);
    // View 400..1000 s: 500 s is at 1/6 of the plot.
    page.update(vcx, |p, cx| p.set_range(Some((500 * S, 700 * S)), cx));
    draw(vcx);
    let plot = plot(&page, vcx);
    let p = at(plot, 1.0 / 6.0);
    vcx.simulate_mouse_move(p, None, Modifiers::none());
    vcx.simulate_mouse_down(p, MouseButton::Left, Modifiers::none());
    settle(vcx);
    vcx.simulate_mouse_up(p, MouseButton::Left, Modifiers::none());
    settle(vcx);
    let (scrub, range) = vcx.read_entity(&page, |p, _| (p.state().scrub, p.state().range));
    assert_eq!((scrub, range), (Some(500 * S), None));
}

#[gpui_kit::test]
fn hover_move_does_not_notify_the_page(cx: &mut TestAppContext) {
    use gpui_kit::Modifiers;
    use std::{cell::Cell, rc::Rc};
    let (page, vcx) = keys_page(cx);
    let plot = plot(&page, vcx);
    vcx.simulate_mouse_move(at(plot, 0.3), None, Modifiers::none());
    settle(vcx);
    let notified = Rc::new(Cell::new(0u32));
    let n = notified.clone();
    let _sub = vcx.update(|_, cx| cx.observe(&page, move |_, _| n.set(n.get() + 1)));
    vcx.simulate_mouse_move(at(plot, 0.6), None, Modifiers::none());
    settle(vcx);
    assert_eq!(notified.get(), 0);
    let hover = vcx.read_entity(&page, |p, _| p.scrubber().hover_x);
    assert!(hover.is_some());
}

#[gpui_kit::test]
fn page_dropped_when_its_host_lets_go(cx: &mut TestAppContext) {
    use gpui_kit::{Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div};
    struct Host(Option<Entity<HistoryPage>>);
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children(self.0.clone())
        }
    }
    let engine = init(cx);
    let (host, vcx) =
        cx.add_window_view(|window, cx| Host(Some(cx.new(|cx| HistoryPage::new(window, cx)))));
    vcx.run_until_parked();
    let v = HistoryView {
        header: ComponentHeader {
            version: 1,
            ..ComponentHeader::default()
        },
        span: Some((100 * S, 1000 * S)),
        frame_t: Some(1000 * S),
        store_state: HistoryStoreState::Ready,
        ..HistoryView::default()
    };
    publish_view(&engine, vcx, v);
    draw(vcx);
    let page = vcx.read_entity(&host, |h, _| h.0.clone().expect("page"));
    assert!(
        vcx.read_entity(&page, |p, _| p.scrubber().plot_bounds.size.width
            > gpui_kit::px(0.))
    );
    let weak = page.downgrade();
    drop(page);
    host.update(vcx, |h, cx| {
        h.0 = None;
        cx.notify();
    });
    draw(vcx);
    vcx.run_until_parked();
    // The strip view holds the page weakly, so nothing keeps the page alive.
    assert!(weak.upgrade().is_none());
}

/// Hover card (tasklens-8fx.26): a page whose view spans 100..1000 s, so the strip shows
/// 400..1000 s.
fn hover_page(
    cx: &mut TestAppContext,
) -> (Arc<FakeEngine>, Entity<HistoryPage>, &mut VisualTestContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let v = HistoryView {
        header: ComponentHeader {
            version: 1,
            ..ComponentHeader::default()
        },
        span: Some((100 * S, 1000 * S)),
        frame_t: Some(1000 * S),
        store_state: HistoryStoreState::Ready,
        ..HistoryView::default()
    };
    publish_view(&engine, vcx, v);
    draw(vcx);
    (engine, page, vcx)
}

/// The `(id, t, metric)` of every TopAt query sent.
fn top_queries(engine: &FakeEngine) -> Vec<(u64, TimeNs, HistoryMetric)> {
    queries(engine)
        .into_iter()
        .filter_map(|(id, range, what)| match what {
            HistoryWhat::TopAt { t, metric, n } => {
                assert_eq!((range, n), (HistoryRange::At(t), 3));
                Some((id, t, metric))
            }
            HistoryWhat::Frame { .. } => None,
        })
        .collect()
}

/// Answers TopAt query `id` as the engine does: the current view again with `top` set.
fn answer_top(
    engine: &FakeEngine,
    vcx: &mut VisualTestContext,
    (id, t, metric): (u64, TimeNs, HistoryMetric),
    values: &[f32],
) {
    engine.publish_with(|s| {
        let mut v = (*s.history).clone();
        v.header.version += 1;
        v.top = Some(Arc::new(HistoryTop {
            query_id: id,
            t_ns: t,
            metric,
            cpu_total_pct: 25.0,
            mem_total_bytes: 16 << 30,
            rows: values
                .iter()
                .enumerate()
                .map(|(i, &value)| HistoryTopRow {
                    key: key(10 + i as u32),
                    name: Arc::from(format!("p{i}").as_str()),
                    app_id: None,
                    value,
                })
                .collect(),
        }));
        s.history = Arc::new(v);
    });
    vcx.run_until_parked();
}

/// Moves the pointer onto the plot at `frac` of its width and paints.
fn hover_to(page: &Entity<HistoryPage>, vcx: &mut VisualTestContext, frac: f32) {
    let p = at(plot(page, vcx), frac);
    vcx.simulate_mouse_move(p, None, gpui_kit::Modifiers::none());
    settle(vcx);
}

/// Value text, rank and name of each card row.
fn card_rows(page: &Entity<HistoryPage>, vcx: &mut VisualTestContext) -> Vec<(String, String)> {
    vcx.read_entity(page, |p, _| {
        let t = p.card().texts();
        t.rows
            .iter()
            .map(|r| (format!("{} {}", r.rank, r.name), r.value.clone()))
            .collect()
    })
}

#[gpui_kit::test]
fn top3_card_formats_per_metric(cx: &mut TestAppContext) {
    use tl_model::units::NetUnit;
    use tl_ui::pages::processes::history::hover::{TopWords, value_text};

    let (engine, page, vcx) = hover_page(cx);
    hover_to(&page, vcx, 0.5);
    advance(vcx, Duration::from_millis(32));
    let sent = top_queries(&engine);
    assert_eq!(sent.len(), 1);
    // View 400..1000 s: the middle is 700 s.
    assert_eq!((sent[0].1, sent[0].2), (700 * S, HistoryMetric::Cpu));
    answer_top(&engine, vcx, sent[0], &[12.3, 6.1, 2.0]);
    settle(vcx);
    assert_eq!(
        card_rows(&page, vcx),
        [
            ("1 p0 (10)".to_owned(), "CPU: 12.3%".to_owned()),
            ("2 p1 (11)".to_owned(), "CPU: 6.1%".to_owned()),
            ("3 p2 (12)".to_owned(), "CPU: 2.0%".to_owned()),
        ]
    );
    vcx.read_entity(&page, |p, _| {
        let t = p.card().texts();
        assert_eq!(t.total, "CPU total 25.0%");
        assert_eq!(t.time.len(), 8, "HH:MM:SS: {}", t.time);
        // 700 s is 5 min before the newest second, 1000 s.
        assert_eq!(t.offset, "  (-5m 0s)");
        assert!(p.card().painted.is_some());
    });

    let w = TopWords::default();
    let text = |metric, value: f32, net| {
        let mut out = String::new();
        value_text(&w, &mut out, metric, value, 67_320_000_000, net);
        out
    };
    let bits = (NetUnit::Bits, false);
    assert_eq!(
        text(HistoryMetric::Memory, 1_310_000_000.0, bits),
        "Memory: 1.22 / 62.70 GiB"
    );
    assert_eq!(
        text(HistoryMetric::Network, 1_500_000.0, bits),
        "Network: 12.0 Mbps"
    );
    assert_eq!(
        text(HistoryMetric::Network, 1_500_000.0, (NetUnit::Bits, true)),
        "Network: 12.0 Mbps"
    );
    assert_eq!(
        text(HistoryMetric::Disk, 4.2 * 1_048_576.0, bits),
        "Disk: 4.2 MiB/s"
    );
    assert_eq!(text(HistoryMetric::Gpu, 41.0, bits), "GPU: 41.0%");
}

#[gpui_kit::test]
fn top3_query_debounced_and_coalesced(cx: &mut TestAppContext) {
    let (engine, page, vcx) = hover_page(cx);
    // 10 moves 10 ms apart, each to a new second.
    for i in 0..10 {
        if i > 0 {
            advance(vcx, Duration::from_millis(10));
        }
        hover_to(&page, vcx, 0.1 + 0.05 * i as f32);
    }
    advance(vcx, Duration::from_millis(31));
    assert!(top_queries(&engine).is_empty(), "nothing before 32 ms");
    advance(vcx, Duration::from_millis(1));
    let sent = top_queries(&engine);
    assert_eq!(sent.len(), 1, "exactly one query 32 ms after the last move");
    // The last move is at 55%: 730 s.
    assert_eq!(sent[0].1, 730 * S);
    assert!(sent[0].0 >= 1 << 48, "TopAt ids never meet Frame ids");

    // While it is in flight, newer seconds wait, and the newest replaces the older.
    hover_to(&page, vcx, 0.8);
    advance(vcx, Duration::from_millis(32));
    hover_to(&page, vcx, 0.9);
    advance(vcx, Duration::from_millis(32));
    assert_eq!(top_queries(&engine).len(), 1);
    let pending = vcx.read_entity(&page, |p, _| p.top().pending());
    assert_eq!(pending, Some((940 * S, HistoryMetric::Cpu)));
    answer_top(&engine, vcx, sent[0], &[1.0]);
    let sent = top_queries(&engine);
    assert_eq!(sent.len(), 2, "the pending query goes out after the answer");
    assert_eq!(sent[1].1, 940 * S);
    assert_eq!(
        vcx.read_entity(&page, |p, _| p.top().in_flight()),
        Some(sent[1].0)
    );
}

#[gpui_kit::test]
fn top3_card_flips_at_right_edge(cx: &mut TestAppContext) {
    use gpui_kit::{Bounds, point, px, size};
    use tl_ui::pages::processes::history::hover::card_origin;

    let strip = Bounds::new(point(px(20.), px(50.)), size(px(1000.), px(120.)));
    assert_eq!(card_origin(px(500.), strip, 1.0), point(px(512.), px(58.)));
    assert_eq!(card_origin(px(920.), strip, 1.0), point(px(648.), px(58.)));

    let (engine, page, vcx) = hover_page(cx);
    let plot = plot(&page, vcx);
    let scale = vcx.update(|_, cx| cx.global::<tl_ui::theme::UiMetrics>().ui_scale);
    let card_at = |vcx: &mut VisualTestContext, hx: gpui_kit::Pixels| {
        vcx.simulate_mouse_move(
            point(plot.left() + hx, plot.top() + px(10.)),
            None,
            gpui_kit::Modifiers::none(),
        );
        settle(vcx);
        advance(vcx, Duration::from_millis(32));
        if let Some(q) = top_queries(&engine).last().copied() {
            answer_top(&engine, vcx, q, &[5.0, 4.0]);
        }
        settle(vcx);
        vcx.read_entity(&page, |p, _| p.card().painted.expect("the card is shown"))
    };
    // The hairline 100 px from the right edge: the card sits left of it.
    let hx = plot.size.width - px(100.);
    let card = card_at(vcx, hx);
    let expect = plot.left() + hx - px(272. * scale);
    assert!((f32::from(card.origin.x - expect)).abs() < 0.5, "{card:?}");
    assert!((f32::from(card.origin.y - plot.top()) - 8. * scale).abs() < 0.5);
    assert!((f32::from(card.size.width) - 260. * scale).abs() < 0.5);
    // Far from the edge: 12 px right of the hairline.
    let card = card_at(vcx, px(100.));
    let expect = plot.left() + px(100. + 12. * scale);
    assert!((f32::from(card.origin.x - expect)).abs() < 0.5, "{card:?}");
}

#[gpui_kit::test]
fn top3_stale_rows_after_150ms(cx: &mut TestAppContext) {
    let (engine, page, vcx) = hover_page(cx);
    hover_to(&page, vcx, 0.3);
    advance(vcx, Duration::from_millis(32));
    let sent = top_queries(&engine);
    answer_top(&engine, vcx, sent[0], &[12.3, 6.1, 2.0]);
    settle(vcx);
    let answered = card_rows(&page, vcx);
    assert_eq!(answered[0].1, "CPU: 12.3%");

    // A new second with no answer: the previous card stays for 149 ms.
    hover_to(&page, vcx, 0.6);
    advance(vcx, Duration::from_millis(149));
    settle(vcx);
    assert_eq!(card_rows(&page, vcx), answered);
    assert!(!vcx.read_entity(&page, |p, _| p.top().stale()));

    advance(vcx, Duration::from_millis(1));
    settle(vcx);
    let ellipsis = (" …".to_owned(), String::new());
    assert_eq!(card_rows(&page, vcx), vec![ellipsis; 3]);
    vcx.read_entity(&page, |p, _| {
        assert!(p.top().stale());
        assert_eq!(p.card().texts().total, "CPU total …");
        assert!(p.card().painted.is_some());
    });
}

#[gpui_kit::test]
fn top_answer_skips_rebuild(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    let (engine, page, vcx) = hover_page(cx);
    hover_to(&page, vcx, 0.5);
    advance(vcx, Duration::from_millis(32));
    settle(vcx);
    let before = builds(&page, vcx);
    let notified = Rc::new(Cell::new(0u32));
    let n = notified.clone();
    let _sub = vcx.update(|_, cx| cx.observe(&page, move |_, _| n.set(n.get() + 1)));
    let sent = top_queries(&engine);
    answer_top(&engine, vcx, sent[0], &[3.0]);
    settle(vcx);
    assert_eq!(builds(&page, vcx), before, "no strip rebuild");
    assert_eq!(notified.get(), 0, "the page is not notified");
    assert_eq!(card_rows(&page, vcx).len(), 1, "the card shows the answer");
}

/// Moves the pointer off the plot, below the strip, and paints.
fn leave_plot(page: &Entity<HistoryPage>, vcx: &mut VisualTestContext) {
    use gpui_kit::{point, px};
    let plot = plot(page, vcx);
    let p = point(plot.left() + px(10.), plot.bottom() + px(200.));
    vcx.simulate_mouse_move(p, None, gpui_kit::Modifiers::none());
    settle(vcx);
}

#[gpui_kit::test]
fn top3_metric_change_asks_again(cx: &mut TestAppContext) {
    let (engine, page, vcx) = hover_page(cx);
    hover_to(&page, vcx, 0.5);
    advance(vcx, Duration::from_millis(32));
    let sent = top_queries(&engine);
    answer_top(&engine, vcx, sent[0], &[12.3]);
    settle(vcx);

    // The pointer stays; the metric changes.
    page.update(vcx, |p, cx| p.set_metric(HistoryMetric::Memory, cx));
    advance(vcx, Duration::from_millis(31));
    assert_eq!(top_queries(&engine).len(), 1, "nothing before 32 ms");
    advance(vcx, Duration::from_millis(1));
    let sent = top_queries(&engine);
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[1].1, sent[1].2), (700 * S, HistoryMetric::Memory));
    answer_top(&engine, vcx, sent[1], &[1_310_000_000.0]);
    settle(vcx);
    assert_eq!(card_rows(&page, vcx)[0].1, "Memory: 1.22 / 16.00 GiB");
}

#[gpui_kit::test]
fn top3_back_to_asked_second_drops_older_wants(cx: &mut TestAppContext) {
    let (engine, page, vcx) = hover_page(cx);
    // A (700 s) in flight, B (820 s) pending, then back to A.
    hover_to(&page, vcx, 0.5);
    advance(vcx, Duration::from_millis(32));
    let a = top_queries(&engine)[0];
    hover_to(&page, vcx, 0.7);
    advance(vcx, Duration::from_millis(32));
    assert_eq!(
        vcx.read_entity(&page, |p, _| p.top().pending()),
        Some((820 * S, HistoryMetric::Cpu))
    );
    hover_to(&page, vcx, 0.5);
    advance(vcx, Duration::from_millis(32));
    assert_eq!(vcx.read_entity(&page, |p, _| p.top().pending()), None);
    answer_top(&engine, vcx, a, &[7.0]);
    settle(vcx);
    assert_eq!(top_queries(&engine).len(), 1, "B is not sent");
    assert_eq!(card_rows(&page, vcx)[0].1, "CPU: 7.0%");

    // B in flight, back to the answered A: B's late answer is not shown under A, A is asked
    // again.
    hover_to(&page, vcx, 0.7);
    advance(vcx, Duration::from_millis(32));
    let b = top_queries(&engine)[1];
    assert_eq!(b.1, 820 * S);
    hover_to(&page, vcx, 0.5);
    advance(vcx, Duration::from_millis(32));
    assert_eq!(top_queries(&engine).len(), 2);
    answer_top(&engine, vcx, b, &[3.0]);
    settle(vcx);
    let sent = top_queries(&engine);
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2].1, 700 * S);
    answer_top(&engine, vcx, sent[2], &[7.5]);
    settle(vcx);
    assert_eq!(card_rows(&page, vcx)[0].1, "CPU: 7.5%");
}

#[gpui_kit::test]
fn top3_no_resend_after_leave(cx: &mut TestAppContext) {
    let (engine, page, vcx) = hover_page(cx);
    hover_to(&page, vcx, 0.5);
    advance(vcx, Duration::from_millis(32));
    assert_eq!(top_queries(&engine).len(), 1);
    leave_plot(&page, vcx);
    assert_eq!(vcx.read_entity(&page, |p, _| p.top().hovered()), None);
    // The unanswered query expires after 1,000 ms and is not sent again.
    for _ in 0..4 {
        advance(vcx, Duration::from_millis(1000));
    }
    assert_eq!(top_queries(&engine).len(), 1);
    assert_eq!(vcx.read_entity(&page, |p, _| p.top().in_flight()), None);
}

/// Minimap (tasklens-8fx.27): a page whose view spans `span`, with the strip window set to
/// `view_range` and the window painted once.
fn minimap_page(
    cx: &mut TestAppContext,
    span: (TimeNs, TimeNs),
    view_range: (TimeNs, TimeNs),
) -> (Arc<FakeEngine>, Entity<HistoryPage>, &mut VisualTestContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let v = HistoryView {
        header: ComponentHeader {
            version: 1,
            ..ComponentHeader::default()
        },
        span: Some(span),
        frame_t: Some(span.1),
        store_state: HistoryStoreState::Ready,
        ..HistoryView::default()
    };
    publish_view(&engine, vcx, v);
    page.update(vcx, |p, cx| p.set_view_range(view_range, cx));
    settle(vcx);
    (engine, page, vcx)
}

fn map(
    page: &Entity<HistoryPage>,
    vcx: &mut VisualTestContext,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    vcx.read_entity(page, |p, _| p.minimap().map_bounds)
}

/// A press at `from`, a move to `to` with the button held, and a release there.
fn drag(
    vcx: &mut VisualTestContext,
    from: gpui_kit::Point<gpui_kit::Pixels>,
    to: gpui_kit::Point<gpui_kit::Pixels>,
) {
    use gpui_kit::{Modifiers, MouseButton};
    vcx.simulate_mouse_move(from, None, Modifiers::none());
    vcx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    settle(vcx);
    vcx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    settle(vcx);
    vcx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    settle(vcx);
}

fn view_range(page: &Entity<HistoryPage>, vcx: &mut VisualTestContext) -> (TimeNs, TimeNs) {
    vcx.read_entity(page, |p, _| p.state().view_range)
}

#[gpui_kit::test]
fn minimap_box_tracks_strip_range(cx: &mut TestAppContext) {
    // 6 h retained, the strip shows the last 10 minutes.
    let span = (0, 6 * 3600 * S);
    let (_engine, page, vcx) = minimap_page(cx, span, (span.1 - 600 * S, span.1));
    let map = map(&page, vcx);
    assert!(map.size.width > gpui_kit::px(0.));
    let b = vcx
        .read_entity(&page, |p, _| p.minimap().box_bounds)
        .expect("a box");
    let width = f32::from(b.size.width);
    let want = f32::from(map.size.width) / 36.0;
    assert!((width - want).abs() <= 1.0, "{width} {want}");
    assert!(f32::from(b.right() - map.right()).abs() <= 1.0);
}

#[gpui_kit::test]
fn minimap_drag_pans_strip(cx: &mut TestAppContext) {
    let (_engine, page, vcx) = minimap_page(cx, (100 * S, 1000 * S), (400 * S, 600 * S));
    let map = map(&page, vcx);
    // The box centre, 500 s, is at 400 / 900 of the map.
    let from = at(map, 400.0 / 900.0);
    let to = at(map, 400.0 / 900.0 + 0.1);
    drag(vcx, from, to);
    let (a, b) = view_range(&page, vcx);
    // 10% of a 900 s span is 90 s, within the time of one px.
    let px_ns = 900.0 * S as f64 / f64::from(f32::from(map.size.width));
    assert!((a as f64 - 490.0 * S as f64).abs() <= px_ns, "{a}");
    assert_eq!(b - a, 200 * S);
}

#[gpui_kit::test]
fn minimap_edge_zooms(cx: &mut TestAppContext) {
    let (_engine, page, vcx) = minimap_page(cx, (100 * S, 1000 * S), (400 * S, 600 * S));
    let map = map(&page, vcx);
    // The left edge, 400 s, is at 300 / 900 of the map; drag it to 450 s.
    drag(vcx, at(map, 300.0 / 900.0), at(map, 350.0 / 900.0));
    let (a, b) = view_range(&page, vcx);
    assert!(a > 400 * S, "{a}");
    assert_eq!(b, 600 * S);
    assert!(b - a < 200 * S);
}

#[gpui_kit::test]
fn minimap_click_centres(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton};
    let (_engine, page, vcx) = minimap_page(cx, (100 * S, 1000 * S), (400 * S, 600 * S));
    let map = map(&page, vcx);
    // 80% of the map is 820 s.
    let p = at(map, 0.8);
    vcx.simulate_mouse_move(p, None, Modifiers::none());
    vcx.simulate_mouse_down(p, MouseButton::Left, Modifiers::none());
    settle(vcx);
    vcx.simulate_mouse_up(p, MouseButton::Left, Modifiers::none());
    settle(vcx);
    assert_eq!(view_range(&page, vcx), (720 * S, 920 * S));
}

/// A view at `version` spanning 100..1000 s with `overview` as its overview.
fn overview_view(version: u64, overview: Arc<[HistoryPoint]>) -> HistoryView {
    HistoryView {
        header: ComponentHeader {
            version,
            ..ComponentHeader::default()
        },
        span: Some((100 * S, 1000 * S)),
        frame_t: Some(1000 * S),
        store_state: HistoryStoreState::Ready,
        overview,
        ..HistoryView::default()
    }
}

fn overview_points() -> Arc<[HistoryPoint]> {
    (100..=1000u64)
        .step_by(10)
        .map(|t| HistoryPoint {
            t_ns: t * S,
            min: (t % 70) as f32,
            max: if t == 500 { f32::NAN } else { (t % 70) as f32 },
        })
        .collect()
}

#[gpui_kit::test]
fn minimap_path_cached(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    let builds =
        |vcx: &mut VisualTestContext| vcx.read_entity(&page, |p, _| p.minimap().build_count);
    publish_view(&engine, vcx, overview_view(1, overview_points()));
    settle(vcx);
    assert_eq!(builds(vcx), 1);
    // Only the strip window changes: the path is not rebuilt.
    for r in [(300 * S, 900 * S), (200 * S, 500 * S)] {
        page.update(vcx, |p, cx| p.set_view_range(r, cx));
        settle(vcx);
    }
    assert_eq!(builds(vcx), 1);
    // A view without an overview keeps the old one.
    publish_view(&engine, vcx, overview_view(2, Arc::from(Vec::new())));
    settle(vcx);
    assert_eq!(builds(vcx), 1);
    // A new overview rebuilds once.
    publish_view(&engine, vcx, overview_view(3, overview_points()));
    settle(vcx);
    settle(vcx);
    assert_eq!(builds(vcx), 2);
}

#[gpui_kit::test]
fn minimap_metric_change_asks_overview(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (page, vcx) = cx.add_window_view(HistoryPage::new);
    vcx.run_until_parked();
    answer(&engine, 1, vcx);
    advance(vcx, Duration::from_secs(1));
    let sent = queries(&engine);
    assert_eq!(sent.len(), 2);
    assert!(!overview(&sent[1].2));
    answer(&engine, sent[1].0, vcx);
    page.update(vcx, |p, cx| p.set_metric(HistoryMetric::Memory, cx));
    advance(vcx, Duration::from_secs(1));
    let sent = queries(&engine);
    assert_eq!(sent.len(), 3);
    assert!(
        overview(&sent[2].2),
        "the new metric's overview is asked at once"
    );
}
