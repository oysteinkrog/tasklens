//! The Overview 24-hour timeline box (UI-PRF-057, D-030). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, TestAppContext, VisualTestContext, point, px, size,
};
use tl_model::command::{Command, HistoryWhat};
use tl_model::demand::DeviceRange;
use tl_model::history::{HistoryMetric, HistoryTop};
use tl_model::journal::JournalAccess;
use tl_model::settings::{OverviewRange, TimelineClick};
use tl_model::snapshot::{
    ComponentHeader, DeviceHistory, DeviceTimeline, HistPoint, TimelineEvent, TimelineEventKind,
};
use tl_model::time::{TimeNs, now_boot_ns};
use tl_ui::pages::performance::overview::range::{STATUS_SWITCH, segment_id};
use tl_ui::pages::performance::overview::timeline::{
    BACK_TO_LIVE_ID, BANDS_ID, DAY_NS, HISTORY_ASK_ID_BASE, HISTORY_ASK_TIMEOUT, PREVIEW_HOLD,
    PauseKind, TimelineBox, click_choices, click_mode,
};
use tl_ui::settings_global::TlSettings;

use super::overview_range::{Rig, click, draw, now_realtime, open, overview, set_range};

const S: i64 = 1_000_000_000;
const MIN: i64 = 60 * S;
const HOUR: i64 = 60 * MIN;

/// The rig with a window tall enough to show the timeline without scrolling.
fn rig(cx: &mut TestAppContext) -> Rig<'_> {
    let r = open(cx);
    r.vcx.simulate_resize(size(px(1600.), px(1400.)));
    draw(r.vcx);
    r
}

fn timeline_box(r: &mut Rig) -> Entity<TimelineBox> {
    let page = overview(r);
    r.vcx.update(|_, cx| page.read(cx).timeline_box().clone())
}

fn point_at(t: i64, mean: f32) -> HistPoint {
    HistPoint {
        t_realtime_ns: t,
        mean,
        min: mean,
        max: mean,
    }
}

fn event(kind: TimelineEventKind, t: i64) -> TimelineEvent {
    TimelineEvent {
        kind,
        start_realtime_ns: t,
        duration_ms: 60_000,
        peak: 1.,
        process: None,
    }
}

/// Publishes `tl` as the snapshot's timeline.
fn publish(r: &mut Rig, tl: DeviceTimeline) {
    r.engine.publish_with(|s| {
        let version = s.device_history.header.version + 1;
        s.device_history = Arc::new(DeviceHistory {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            realtime_offset_ns: now_realtime() - now_boot_ns() as i64,
            timeline: Some(Arc::new(tl)),
            ..DeviceHistory::default()
        });
    });
    draw(r.vcx);
}

/// A timeline that ends now, with no bands.
fn empty_timeline(end: i64) -> DeviceTimeline {
    DeviceTimeline {
        end_realtime_ns: end,
        step_ns: 60 * S as u64,
        ..DeviceTimeline::default()
    }
}

/// The point at `frac` of the band area's width, half way down.
fn bands_at(cx: &mut VisualTestContext, frac: f32) -> Point<Pixels> {
    let b = cx.debug_bounds(BANDS_ID).expect("the bands");
    point(b.left() + b.size.width * frac, b.top() + b.size.height / 2.)
}

fn press(cx: &mut VisualTestContext, at: Point<Pixels>) {
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    draw(cx);
}

fn drag_to(cx: &mut VisualTestContext, at: Point<Pixels>) {
    cx.simulate_event(MouseMoveEvent {
        position: at,
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::none(),
    });
    draw(cx);
}

fn release(cx: &mut VisualTestContext, at: Point<Pixels>) {
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    draw(cx);
}

fn click_bands(r: &mut Rig, frac: f32) {
    let at = bands_at(r.vcx, frac);
    press(r.vcx, at);
    release(r.vcx, at);
}

fn set_click(r: &mut Rig, click: TimelineClick, recording: bool) {
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            d.settings.history.enabled = recording;
            d.settings.performance.timeline_click = click;
        });
    });
    draw(r.vcx);
}

fn range(r: &mut Rig) -> OverviewRange {
    r.vcx
        .update(|_, cx| TlSettings::settings(cx).performance.overview_range)
}

/// The timeline's process history queries, `(id, t)`, oldest first.
fn history_asks(r: &Rig) -> Vec<(u64, TimeNs)> {
    r.engine
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::HistoryQuery {
                id,
                what: HistoryWhat::TopAt { t, .. },
                ..
            } if id >= HISTORY_ASK_ID_BASE => Some((id, t)),
            _ => None,
        })
        .collect()
}

/// Answers query `id` as the engine does: the current view again with `top` set. `t_ns` is the
/// second read (the asked one moved into the retained span); `mem` is 0 when no block holds it.
fn answer(r: &mut Rig, id: u64, t_ns: TimeNs, mem: u64) {
    r.engine.publish_with(|s| {
        let mut v = (*s.history).clone();
        v.header.version += 1;
        v.top = Some(Arc::new(HistoryTop {
            query_id: id,
            t_ns,
            metric: HistoryMetric::Cpu,
            cpu_total_pct: 10.,
            mem_total_bytes: mem,
            rows: Arc::from([]),
        }));
        s.history = Arc::new(v);
    });
    draw(r.vcx);
}

/// The last toast the box pushed, cleared, and the second History mode opened at.
fn take_outcome(r: &mut Rig) -> (Option<String>, Option<TimeNs>) {
    let b = timeline_box(r);
    r.vcx.read_entity(&b, |b, _| {
        (
            b.last_toast.take().map(|t| t.to_string()),
            b.last_history_at.take(),
        )
    })
}

fn history_mode(r: &mut Rig) -> bool {
    r.vcx
        .update(|_, cx| TlSettings::state(cx).processes.history_mode)
}

const NO_HISTORY: &str = "No process history at that time";
const GIB: u64 = 1 << 30;

/// The paused time, or `None` while live.
fn paused(r: &mut Rig) -> Option<(i64, PauseKind)> {
    let page = overview(r);
    r.vcx
        .update(|_, cx| page.read(cx).paused().map(|p| (p.at_realtime_ns, p.kind)))
}

/// The placed timeline asks for the timeline; four bands draw the 1 min means, a gap draws
/// nothing, and full load draws at the top level.
#[gpui_kit::test]
fn timeline_bands_from_history(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    let demand = r.demand.clone();
    let merged = r.vcx.update(|_, cx| demand.read(cx).merged());
    assert!(
        merged.device_timeline,
        "the default layout places the timeline"
    );

    let end = now_realtime();
    let mut tl = empty_timeline(end);
    // CPU: 2 hours at 100%, then a gap hour, then 2 hours at 0%.
    let cpu: Vec<HistPoint> = (0..300)
        .map(|i| {
            let t = end - 5 * HOUR + i * MIN;
            let v = match i {
                0..120 => 100.,
                120..180 => f32::NAN,
                _ => 0.,
            };
            point_at(t, v)
        })
        .collect();
    tl.bands[0] = Arc::from(cpu);
    tl.bands[3] = Arc::from([point_at(end - HOUR, 10.), point_at(end - HOUR + MIN, 20.)]);
    publish(&mut r, tl);
    let b = timeline_box(&mut r);
    let cells = r.vcx.read_entity(&b, |b, _| b.last_cells.borrow().clone());
    assert_eq!(cells[0].len(), 2, "two runs around the gap: {:?}", cells[0]);
    assert_eq!(cells[0][0].level, 12, "100% at full intensity");
    assert_eq!(cells[0][1].level, 1, "0% at the lowest level");
    assert!(
        cells[0][0].x1 < cells[0][1].x0,
        "the gap hour paints nothing"
    );
    assert!(
        cells[1].is_empty() && cells[2].is_empty(),
        "no data, no paint"
    );
    // The network band has no fixed scale: its own peak is full.
    assert_eq!(cells[3].last().map(|c| c.level), Some(12));
}

/// Markers closer than 8 px merge into one with a count; markers far apart stay apart.
#[gpui_kit::test]
fn timeline_event_markers_merge(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    let end = now_realtime();
    let mut tl = empty_timeline(end);
    tl.events = Arc::from([
        event(TimelineEventKind::Heat, end - 6 * HOUR),
        event(TimelineEventKind::MemoryPressure, end - 2 * HOUR),
        // One minute later: well under 8 px on a 24-hour band.
        event(TimelineEventKind::SwapStorm, end - 2 * HOUR + MIN),
        event(TimelineEventKind::OomKill, end - 2 * HOUR + 2 * MIN),
    ]);
    publish(&mut r, tl);
    let b = timeline_box(&mut r);
    let markers = r
        .vcx
        .read_entity(&b, |b, _| b.last_markers.borrow().clone());
    let counts: Vec<usize> = markers.iter().map(|m| m.count).collect();
    assert_eq!(counts, [1, 3]);
    assert_eq!(markers[1].first, 1);
    assert!(
        r.vcx
            .debug_bounds("perf-overview-timeline-marker-1")
            .is_some()
    );
    assert!(
        r.vcx
            .debug_bounds("perf-overview-timeline-marker-2")
            .is_none()
    );
}

/// `pause` holds the page until "Back to live"; `preview` holds it while the pointer is down and
/// for 30 s after; `history` is offered only while recording is on, and outside the kept
/// process history it shows a toast instead of opening History mode.
#[gpui_kit::test]
fn timeline_click_modes(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    let end = now_realtime();
    publish(&mut r, empty_timeline(end));
    set_range(&mut r, OverviewRange::H24);

    // Pause (the default): a click half way along holds the page 12 hours ago.
    click_bands(&mut r, 0.5);
    let (at, kind) = paused(&mut r).expect("paused");
    assert_eq!(kind, PauseKind::Pause);
    assert!((at - (end - DAY_NS / 2)).abs() < 10 * MIN, "12 hours ago");
    let demand = r.demand.clone();
    let merged = r.vcx.update(|_, cx| demand.read(cx).merged());
    let req = merged.device_history_range.expect("a history request");
    assert_eq!(req.range, DeviceRange::H24);
    assert_eq!(
        req.end_realtime_ns,
        Some(at + DAY_NS / 2),
        "centred on the pick"
    );
    // The pause stays until "Back to live".
    r.vcx.executor().advance_clock(PREVIEW_HOLD * 2);
    draw(r.vcx);
    assert!(paused(&mut r).is_some());
    click(r.vcx, BACK_TO_LIVE_ID);
    assert_eq!(paused(&mut r), None);
    assert!(r.vcx.debug_bounds(BACK_TO_LIVE_ID).is_none());
    let merged = r.vcx.update(|_, cx| demand.read(cx).merged());
    assert_eq!(
        merged.device_history_range.and_then(|r| r.end_realtime_ns),
        None
    );

    // Preview: held while the pointer is down, and for 30 s after release.
    set_click(&mut r, TimelineClick::Preview, false);
    let at = bands_at(r.vcx, 0.9);
    press(r.vcx, at);
    assert_eq!(paused(&mut r).map(|p| p.1), Some(PauseKind::Preview));
    release(r.vcx, at);
    r.vcx.executor().advance_clock(PREVIEW_HOLD / 2);
    draw(r.vcx);
    assert!(paused(&mut r).is_some(), "still previewing after 15 s");
    r.vcx.executor().advance_clock(PREVIEW_HOLD);
    draw(r.vcx);
    assert_eq!(paused(&mut r), None, "live again after 30 s");

    // History: hidden and read as pause while recording is off.
    set_click(&mut r, TimelineClick::History, false);
    let (mode, choices) = r.vcx.update(|_, cx| (click_mode(cx), click_choices(cx)));
    assert_eq!(mode, TimelineClick::Pause);
    assert!(!choices.contains(&TimelineClick::History));

    // With recording on, a click asks the process history for that second first. The clicks
    // stay within the last half hour, so they fall in this boot.
    set_click(&mut r, TimelineClick::History, true);
    let choices = r.vcx.update(|_, cx| click_choices(cx));
    assert!(choices.contains(&TimelineClick::History));
    // 28.8 minutes ago.
    click_bands(&mut r, 0.98);
    let asks = history_asks(&r);
    assert_eq!(asks.len(), 1, "one TopAt query");
    let (id, t) = asks[0];
    assert_eq!(
        take_outcome(&mut r),
        (None, None),
        "nothing before the answer"
    );
    // Recording began 10 minutes ago: the engine reads its oldest second instead. The toast
    // shows and nothing else happens.
    let oldest = now_boot_ns() - 10 * MIN as u64;
    answer(&mut r, id, oldest / S as u64 * S as u64, 16 * GIB);
    assert_eq!(take_outcome(&mut r), (Some(NO_HISTORY.to_owned()), None));
    assert_eq!(paused(&mut r), None, "the toast does nothing else");
    assert!(!history_mode(&mut r));
    assert!(t < oldest, "the click was before the recording");

    // A second inside the span but in a gap (no block) is the toast too.
    click_bands(&mut r, 0.985);
    let (id, t) = *history_asks(&r).last().expect("a query");
    answer(&mut r, id, t / S as u64 * S as u64, 0);
    assert_eq!(take_outcome(&mut r).0.as_deref(), Some(NO_HISTORY));

    // No answer within the timeout: the toast.
    click_bands(&mut r, 0.99);
    assert_eq!(take_outcome(&mut r), (None, None));
    r.vcx.executor().advance_clock(HISTORY_ASK_TIMEOUT);
    draw(r.vcx);
    assert_eq!(take_outcome(&mut r), (Some(NO_HISTORY.to_owned()), None));
    assert!(!history_mode(&mut r));

    // A second the history holds opens History mode there.
    click_bands(&mut r, 0.995);
    let (id, t) = *history_asks(&r).last().expect("a query");
    answer(&mut r, id, t / S as u64 * S as u64, 16 * GIB);
    assert_eq!(take_outcome(&mut r), (None, Some(t)), "History mode opened");
    assert!(history_mode(&mut r));
}

/// While paused 5 hours ago, the 1 min and 5 min ranges need the 1 s tier, which keeps the last
/// hour: they are disabled, and the range in use moves up to 1 h.
#[gpui_kit::test]
fn timeline_paused_disables_unservable_ranges(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    let end = now_realtime();
    publish(&mut r, empty_timeline(end));
    assert_eq!(range(&mut r), OverviewRange::M1);
    // 5 hours ago is 19/24 of the way along.
    click_bands(&mut r, 19. / 24.);
    assert!(paused(&mut r).is_some());
    assert_eq!(range(&mut r), OverviewRange::H1, "1 min cannot be served");
    let one_min: &'static str =
        Box::leak(segment_id(STATUS_SWITCH, OverviewRange::M1).into_boxed_str());
    click(r.vcx, one_min);
    assert_eq!(range(&mut r), OverviewRange::H1, "1 min is disabled");
    let day: &'static str =
        Box::leak(segment_id(STATUS_SWITCH, OverviewRange::H24).into_boxed_str());
    click(r.vcx, day);
    assert_eq!(range(&mut r), OverviewRange::H24, "24 h is served");
    // Live again: every range is enabled.
    click(r.vcx, BACK_TO_LIVE_ID);
    click(r.vcx, one_min);
    assert_eq!(range(&mut r), OverviewRange::M1);
}

/// A drag picks a span: the range switch takes the smallest range that holds it and the page
/// is held at the span's centre.
#[gpui_kit::test]
fn timeline_drag_selects_smallest_range(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    let end = now_realtime();
    publish(&mut r, empty_timeline(end));
    // From 12 to 9 hours ago: 3 hours fits 24 h, not 1 h.
    let (a, b) = (bands_at(r.vcx, 0.5), bands_at(r.vcx, 15. / 24.));
    press(r.vcx, a);
    drag_to(r.vcx, point((a.x + b.x) / 2., a.y));
    assert!(
        r.vcx
            .debug_bounds("perf-overview-timeline-selection")
            .is_some(),
        "the picked span shows while dragging"
    );
    drag_to(r.vcx, b);
    release(r.vcx, b);
    assert_eq!(range(&mut r), OverviewRange::H24);
    let (at, kind) = paused(&mut r).expect("paused");
    assert_eq!(kind, PauseKind::Pause);
    let centre = end - DAY_NS / 2 + 3 * HOUR / 2;
    assert!((at - centre).abs() < 10 * MIN, "held at the span's centre");

    // A 30-minute span fits 1 h.
    click(r.vcx, BACK_TO_LIVE_ID);
    let (a, b) = (bands_at(r.vcx, 0.9), bands_at(r.vcx, 0.9 + 0.5 / 24.));
    press(r.vcx, a);
    drag_to(r.vcx, b);
    release(r.vcx, b);
    assert_eq!(range(&mut r), OverviewRange::H1);

    // In `history` mode a drag pauses too; it never opens History mode.
    click(r.vcx, BACK_TO_LIVE_ID);
    set_click(&mut r, TimelineClick::History, true);
    let (a, b) = (bands_at(r.vcx, 0.5), bands_at(r.vcx, 15. / 24.));
    press(r.vcx, a);
    drag_to(r.vcx, b);
    release(r.vcx, b);
    assert_eq!(range(&mut r), OverviewRange::H24);
    assert_eq!(paused(&mut r).map(|p| p.1), Some(PauseKind::Pause));
    assert!(history_asks(&r).is_empty(), "no process history query");
    assert!(!history_mode(&mut r));
}

/// When the journal shows only the user's own entries, OOM kills and crashes are left out and
/// the timeline says why.
#[gpui_kit::test]
fn timeline_journal_own_only_omits_kinds(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    let end = now_realtime();
    let events = || -> Arc<[TimelineEvent]> {
        Arc::from([
            event(TimelineEventKind::Heat, end - 6 * HOUR),
            event(TimelineEventKind::OomKill, end - 4 * HOUR),
            event(TimelineEventKind::Crash, end - 2 * HOUR),
        ])
    };
    let mut tl = empty_timeline(end);
    tl.journal = Some(JournalAccess::OwnOnly);
    tl.events = events();
    publish(&mut r, tl);
    let b = timeline_box(&mut r);
    let (note, markers, text) = r.vcx.read_entity(&b, |b, _| {
        let mut text = String::new();
        b.copy_text(&mut text);
        (b.journal_note(), b.last_markers.borrow().clone(), text)
    });
    assert_eq!(note.as_deref(), Some("Needs the systemd-journal group"));
    assert_eq!(markers.len(), 1, "only the heat marker: {markers:?}");
    assert!(
        r.vcx
            .debug_bounds("perf-overview-timeline-marker-1")
            .is_none()
    );
    assert!(text.contains("Heat"), "{text}");
    assert!(
        !text.contains("OOM kill") && !text.contains("Crash"),
        "{text}"
    );

    // With the whole journal, all three show.
    let mut tl = empty_timeline(end);
    tl.journal = Some(JournalAccess::All);
    tl.events = events();
    publish(&mut r, tl);
    let (note, markers) = r
        .vcx
        .read_entity(&b, |b, _| (b.journal_note(), b.last_markers.borrow().len()));
    assert_eq!(note, None);
    assert_eq!(markers, 3);
}
