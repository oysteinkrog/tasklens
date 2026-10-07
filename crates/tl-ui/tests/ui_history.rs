//! History mode page: header text and render on the HISTORY bit only (tasklens-8fx.19). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{AppContext as _, TestAppContext};
use tl_model::client::EngineClient;
use tl_model::details::DetailsSnapshot;
use tl_model::history::{HistoryStoreState, HistoryView};
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
