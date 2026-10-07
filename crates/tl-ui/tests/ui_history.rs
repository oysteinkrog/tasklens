//! History mode page: header text and render on the HISTORY bit only (tasklens-8fx.19), and the
//! query controller with one Frame query in flight (tasklens-8fx.20). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use tl_model::client::EngineClient;
use tl_model::command::{Command, HistoryWhat};
use tl_model::details::DetailsSnapshot;
use tl_model::history::{HistoryRange, HistoryStoreState, HistoryView};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::snapshot::ComponentHeader;
use tl_model::time::TimeNs;
use tl_ui::hub::{GlobalHub, SnapshotHub};
use tl_ui::pages::processes::history::{
    HistoryPage, HistoryUiState, HistoryWords, default_view_range, header_text,
};
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
    let (page, vcx) = cx.add_window_view(|_, cx| HistoryPage::new(cx));
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
    let (page, vcx) = cx.add_window_view(|_, cx| HistoryPage::new(cx));
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
    let (page, vcx) = cx.add_window_view(|_, cx| HistoryPage::new(cx));
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
    let (page, vcx) = cx.add_window_view(|_, cx| HistoryPage::new(cx));
    vcx.run_until_parked();
    scrub(&page, vcx, 100 * S);
    answer(&engine, 99, vcx);
    assert_eq!(vcx.read_entity(&page, |p, _| p.query_in_flight()), Some(1));
    assert_eq!(queries(&engine).len(), 1);
}

#[gpui_kit::test]
fn live_queries_each_second(cx: &mut TestAppContext) {
    let engine = init(cx);
    let (_page, vcx) = cx.add_window_view(|_, cx| HistoryPage::new(cx));
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
    let (_page, vcx) = cx.add_window_view(|_, cx| HistoryPage::new(cx));
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
    let (page, vcx) = cx.add_window_view(|_, cx| HistoryPage::new(cx));
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
    let (page, vcx) = cx.add_window_view(|_, cx| HistoryPage::new(cx));
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
