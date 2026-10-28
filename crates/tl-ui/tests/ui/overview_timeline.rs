//! The Overview 24-hour timeline box (UI-PRF-057, D-030). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, TestAppContext, VisualTestContext, point, px, size,
};
use tl_model::command::{Command, HistoryWhat};
use tl_model::demand::{DeviceHistoryRequest, DeviceRange};
use tl_model::history::{HistoryMetric, HistoryTop};
use tl_model::ids::{DeviceIdx, GroupKey, MetricId, ProcessKey, SeriesId};
use tl_model::journal::JournalAccess;
use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
use tl_model::settings::{OverviewLayoutSetting, OverviewRange, TimelineClick};
use tl_model::snapshot::{
    ComponentHeader, DeviceHistory, DeviceTimeline, HistPoint, ProcessSnapshot, TimelineEvent,
    TimelineEventKind,
};
use tl_model::system::{DiskTotals, GpuTotals, LinkState, MemoryTotals, Metric, NetTotals};
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::{FieldValidity, Validity};
use tl_ui::pages::performance::overview::layout::BoxKind;
use tl_ui::pages::performance::overview::range::{STATUS_SWITCH, segment_id};
use tl_ui::pages::performance::overview::timeline::{
    BACK_TO_LIVE_ID, BANDS_ID, DAY_NS, HISTORY_ASK_ID_BASE, HISTORY_ASK_TIMEOUT, PREVIEW_HOLD,
    PauseKind, Paused, TimelineBox, click_choices, click_mode, set_paused, set_test_clock,
};
use tl_ui::settings_global::TlSettings;

use super::overview_range::{Rig, click, draw, open, overview, set_range};

const S: i64 = 1_000_000_000;
const MIN: i64 = 60 * S;
const HOUR: i64 = 60 * MIN;

/// The test clock's CLOCK_REALTIME now: 2026-10-28 09:20 UTC.
const NOW: i64 = 1_793_179_200 * S;
/// The test clock's CLOCK_BOOTTIME now: the machine booted 2 hours ago.
const BOOT: TimeNs = 2 * HOUR as TimeNs;

/// The rig with a window tall enough to show the timeline without scrolling, and the
/// timeline's clocks pinned at [`NOW`] and [`BOOT`], so no test reads the host's clock or
/// uptime.
fn rig(cx: &mut TestAppContext) -> Rig<'_> {
    set_test_clock(Some((NOW, BOOT)));
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
            realtime_offset_ns: NOW - BOOT as i64,
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

    let end = NOW;
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
    let end = NOW;
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
    let end = NOW;
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

    // With recording on, a click asks the process history for that second first. The test
    // clock booted 2 hours ago, so the clicks within the last half hour fall in this boot.
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
    let oldest = BOOT - 10 * MIN as u64;
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
    let end = NOW;
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
    let end = NOW;
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
    let end = NOW;
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

// ---- Boxes at the picked time ----

const MB: f64 = 1_000_000.;

fn proc_record(pid: u32, name: &str, cpu_pct: f32) -> ProcessRecord {
    let name: Arc<str> = Arc::from(name);
    ProcessRecord {
        key: ProcessKey {
            start_ticks: u64::from(pid),
            boot_id: BootId(1),
            pid,
        },
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
        cpu_pct,
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

fn gpu_totals(pct: f32) -> GpuTotals {
    GpuTotals {
        device: DeviceIdx(2),
        util_pct: Metric::valid(pct),
        mem_util_pct: Metric::default(),
        mem_used: Metric::valid((f64::from(pct) / 100. * 8. * GIB as f64) as u64),
        mem_total: Metric::valid(8 * GIB),
        shared_used: Metric::default(),
        shared_total: Metric::default(),
        mem_reserved: Metric::default(),
        enc_pct: Metric::default(),
        dec_pct: Metric::default(),
        enc_sessions: Metric::default(),
        enc_dec_shared: false,
        temp_c: Metric::valid(pct),
        temp_edge_c: Metric::default(),
        temp_junction_c: Metric::default(),
        temp_mem_c: Metric::default(),
        power_w: Metric::valid(pct),
        power_limit_w: Metric::default(),
        clock_gfx_mhz: Metric::default(),
        clock_mem_mhz: Metric::default(),
        fan_pct: Metric::default(),
        pcie_rx_bps: Metric::default(),
        pcie_tx_bps: Metric::default(),
        engines: Default::default(),
        throttling: Default::default(),
        fdinfo_partial: false,
        energy_since_boot: Metric::default(),
        clock_video_mhz: Metric::default(),
        pstate: Metric::default(),
        clock_event_reasons: Metric::default(),
        fan_rpm: Metric::default(),
        pcie_gen: Metric::default(),
        pcie_width: Metric::default(),
        info: Arc::default(),
    }
}

/// Live values for every box, all from `pct`: CPU, GPU and disk busy at `pct`%, `pct`% of
/// 16 GiB memory in use, the disk and the interface moving `pct` MB/s, and one process `name`
/// at `pct`% CPU.
fn publish_live(r: &mut Rig, pct: f32, name: &str) {
    let rate = f64::from(pct) * MB;
    r.engine.publish_with(|s| {
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.cpu.util_pct = Metric::valid(pct);
        let total = 16 * GIB;
        let used = (f64::from(pct) / 100. * total as f64) as u64;
        sys.memory = MemoryTotals {
            validity: Validity::Valid,
            total,
            used,
            available: total - used,
            cached: used / 2,
            committed: used,
            commit_limit: total,
            ..MemoryTotals::default()
        };
        sys.gpus = Arc::from([(DeviceIdx(2), gpu_totals(pct))]);
        sys.disks = Arc::from([(
            DeviceIdx(3),
            DiskTotals {
                device: DeviceIdx(3),
                active_pct: Metric::valid(pct),
                read_bps: Metric::valid(rate),
                write_bps: Metric::valid(rate / 2.),
                response_ms: Metric::valid(1.),
                read_total: 0,
                written_total: 0,
                capacity: 0,
                temp_c: Metric::default(),
            },
        )]);
        sys.nets = Arc::from([(
            DeviceIdx(5),
            NetTotals {
                device: DeviceIdx(5),
                rx_bps: Metric::valid(rate),
                tx_bps: Metric::valid(rate / 2.),
                rx_total: (rate * 100.) as u64,
                tx_total: (rate * 50.) as u64,
                link_speed_mbps: Metric::valid(1_000),
                state: LinkState::Up,
            },
        )]);
        let version = s.processes.header.version + 1;
        s.processes = Arc::new(ProcessSnapshot {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            records: Arc::from([proc_record(4242, name, pct)]),
            ..ProcessSnapshot::default()
        });
    });
    draw(r.vcx);
}

/// The device history's answer to the paused 24 h request: one 1 min point at `at` per series,
/// every value from `pct` as in [`publish_live`].
fn publish_picked(r: &mut Rig, at: i64, pct: f32) {
    let id = |device, metric| SeriesId {
        device: DeviceIdx(device),
        metric,
    };
    let rate = pct * MB as f32;
    let used = pct / 100. * 16. * GIB as f32;
    let mut series = vec![
        (id(0, MetricId::CPU_TOTAL), pct),
        (id(1, MetricId::MEM_USED), used),
        (id(1, MetricId::MEM_CACHED), used / 2.),
        (id(1, MetricId::MEM_AVAILABLE), 16. * GIB as f32 - used),
        (id(1, MetricId::MEM_COMMIT), used),
        (id(2, MetricId::GPU_UTIL), pct),
        (id(2, MetricId::GPU_MEM), pct / 100. * 8. * GIB as f32),
        (id(2, MetricId::GPU_TEMP), pct),
        (id(2, MetricId::GPU_POWER), pct),
        (id(3, MetricId::DISK_ACTIVE), pct),
        (id(3, MetricId::DISK_READ), rate),
        (id(3, MetricId::DISK_WRITE), rate / 2.),
        (id(5, MetricId::NET_RX), rate),
        (id(5, MetricId::NET_TX), rate / 2.),
    ];
    series.sort_by_key(|(id, _)| *id);
    let t = at - at.rem_euclid(MIN);
    let series: Vec<(SeriesId, Arc<[HistPoint]>)> = series
        .into_iter()
        .map(|(id, v)| (id, Arc::from([point_at(t, v)])))
        .collect();
    r.engine.publish_with(|s| {
        let version = s.device_history.header.version + 1;
        s.device_history = Arc::new(DeviceHistory {
            header: ComponentHeader {
                version,
                ..ComponentHeader::default()
            },
            request: Some(DeviceHistoryRequest {
                range: DeviceRange::H24,
                end_realtime_ns: Some(NOW),
            }),
            realtime_offset_ns: NOW - BOOT as i64,
            since_realtime_ns: Some(NOW - 2 * DAY_NS),
            step_ns: MIN as u64,
            series: Arc::from(series),
            ..DeviceHistory::default()
        });
    });
    draw(r.vcx);
}

/// What box `kind` shows, as its copied text. The match has no catch-all, so a new box kind
/// does not compile until this test covers it.
fn box_text(r: &mut Rig, kind: BoxKind) -> String {
    let page = overview(r);
    r.vcx.update(|_, cx| {
        let page = page.read(cx);
        let mut out = String::new();
        match kind {
            BoxKind::Cpu => page.cpu_box().read(cx).copy_text(&mut out),
            BoxKind::Memory => page.mem_box().read(cx).copy_text(&mut out),
            BoxKind::Gpu => page.gpu_box().read(cx).copy_text(&mut out),
            BoxKind::Disk => page.disk_box().read(cx).copy_text(&mut out),
            BoxKind::Net => page.net_box().read(cx).copy_text(&mut out),
            BoxKind::TopProcs => page
                .procs_box()
                .expect("the Top processes box")
                .read(cx)
                .copy_text(&mut out),
            BoxKind::Timeline => page.timeline_box().read(cx).copy_text(&mut out),
        }
        out
    })
}

fn box_texts(r: &mut Rig) -> Vec<(BoxKind, String)> {
    BoxKind::ALL
        .into_iter()
        .map(|k| (k, box_text(r, k)))
        .collect()
}

fn set_pause(r: &mut Rig, at: Option<i64>) {
    r.vcx.update(|_, cx| {
        set_paused(
            at.map(|at_realtime_ns| Paused {
                at_realtime_ns,
                kind: PauseKind::Pause,
            }),
            cx,
        );
    });
    draw(r.vcx);
}

/// While paused, every box kind shows the history's values at the picked time: a new live
/// snapshot changes no box. Back to live, every box shows live values and follows them again.
#[gpui_kit::test]
fn overview_boxes_follow_picked_time(cx: &mut TestAppContext) {
    let mut r = rig(cx);
    // Every box kind placed: the default grid plus Top processes.
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            let unit = d.settings.performance.overview_row_unit_px;
            d.settings.performance.overview = Some(OverviewLayoutSetting::with_top_procs(unit));
            d.settings.performance.overview_range = OverviewRange::H24;
        });
    });
    draw(r.vcx);
    publish(&mut r, empty_timeline(NOW));
    // The timeline box draws only its 24 hours, so its text is the same live and paused.
    let changes = |k: BoxKind| k != BoxKind::Timeline;

    publish_live(&mut r, 33., "live-a");
    let live_a = box_texts(&mut r);
    assert!(box_text(&mut r, BoxKind::Cpu).contains("33.0%"));
    assert!(box_text(&mut r, BoxKind::TopProcs).contains("live-a"));

    // Held 2 hours ago, the history answers with its values there.
    let at = NOW - 2 * HOUR;
    set_pause(&mut r, Some(at));
    publish_picked(&mut r, at, 77.);
    let picked = box_texts(&mut r);
    for ((kind, live), (_, held)) in live_a.iter().zip(&picked) {
        if changes(*kind) {
            assert_ne!(live, held, "{kind:?} shows the picked time");
        }
    }
    let cpu = box_text(&mut r, BoxKind::Cpu);
    assert!(cpu.contains("77.0%") && !cpu.contains("33.0%"), "{cpu}");
    let procs = box_text(&mut r, BoxKind::TopProcs);
    assert!(
        !procs.contains("live-a"),
        "the history keeps no processes: {procs}"
    );

    // A new live snapshot changes no box while paused.
    publish_live(&mut r, 55., "live-b");
    for ((kind, before), (_, after)) in picked.iter().zip(&box_texts(&mut r)) {
        assert_eq!(before, after, "{kind:?} ignores the live snapshot");
        assert!(
            !after.contains("55.0%") && !after.contains("live-b"),
            "{after}"
        );
    }

    // Back to live: every box shows the newest live values, then follows the next snapshot.
    set_pause(&mut r, None);
    let live_b = box_texts(&mut r);
    for ((kind, held), (_, live)) in picked.iter().zip(&live_b) {
        if changes(*kind) {
            assert_ne!(held, live, "{kind:?} is live again");
        }
    }
    assert!(box_text(&mut r, BoxKind::Cpu).contains("55.0%"));
    assert!(box_text(&mut r, BoxKind::TopProcs).contains("live-b"));
    publish_live(&mut r, 66., "live-c");
    for ((kind, before), (_, after)) in live_b.iter().zip(&box_texts(&mut r)) {
        if changes(*kind) {
            assert_ne!(before, after, "{kind:?} follows the live snapshot");
        }
    }
}
