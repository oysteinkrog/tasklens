//! The Overview 24-hour timeline box (UI-PRF-057, D-030).
//!
//! Four heat bands (CPU, Memory, Disk, Network) over the last 24 hours from the device metric
//! history's 1 min tier (`DeviceHistory.timeline`, ADR-017), hour ticks in local time under
//! them, a bracket over the span the boxes above show, and event markers above them. Markers
//! closer than [`MERGE_PX`] merge into one with a count. The page asks for the timeline with
//! `Demand.device_timeline` while the box is placed.
//!
//! A click on the bands acts by `performance.timeline_click`: `pause` holds the Overview at the
//! picked time until "Back to live", `preview` holds it while the pointer is down and for
//! [`PREVIEW_HOLD`] after, and `history` opens History mode at that time. Before it opens,
//! the box asks the process history for that second (a `TopAt` query): no answer with data
//! there within [`HISTORY_ASK_TIMEOUT`] gives the toast "No process history at that time". A drag
//! picks a span: the range switch takes the smallest range that holds it, and the boxes show
//! that range centred on the span, paused (previewed in `preview` mode), never in History mode.
//!
//! While the journal shows only the user's own entries, OOM kills and crashes are left out of
//! the markers and the copied text, and the box says why.
//!
//! The paused time lives in the [`OverviewPause`] global. The page turns it into the device
//! history request, and while it is set the range switch disables the ranges the finest tier
//! at that time cannot serve ([`servable`]).
//!
//! Render contract: the box notifies only when the timeline component, the pause, the range or
//! a drag changes. A tick that leaves those alone renders nothing.

use std::cell::Cell;
#[cfg(any(test, feature = "test-hooks"))]
use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, Entity, Global, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Render, SharedString, Styled as _, Subscription,
    Task, WeakEntity, Window, canvas, div, fill, point, px, size,
};
use smallvec::SmallVec;
use tl_model::command::{Command, HistoryWhat};
use tl_model::history::{HistoryMetric, HistoryRange, HistoryTop};
use tl_model::journal::JournalAccess;
use tl_model::settings::{OverviewRange, TimelineClick};
use tl_model::snapshot::{
    DeviceTimeline, HistPoint, TimelineBand, TimelineEvent, TimelineEventKind,
};
use tl_model::time::{TimeNs, now_boot_ns};

use super::fit::whole_rows;
use super::layout::BoxMode;
use super::parts::{box_frame, edit_chrome};
use super::range::{self, RangeText};
use crate::components::toast::ToastKind;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::pages::performance::details::Template;
use crate::settings_global::TlSettings;
use crate::text::style::t_xs;
use crate::theme::palette::DeviceAccent;
use crate::theme::{TlTheme, UiMetrics, current, metrics};
use crate::window::main::push_main_toast;

/// The box's debug selector.
pub const TIMELINE_BOX_ID: &str = "perf-overview-box-timeline";

/// The debug selector of the band area, where clicks and drags land.
pub const BANDS_ID: &str = "perf-overview-timeline-bands";

/// The debug selector of the bracket over the span the boxes show.
pub const BRACKET_ID: &str = "perf-overview-timeline-bracket";

/// The debug selector of the span a drag is picking.
pub const SELECTION_ID: &str = "perf-overview-timeline-selection";

/// The debug selector of the row of event markers above the bands.
pub const MARKER_ROW_ID: &str = "perf-overview-timeline-marker-row";

/// The debug selector of the row of hour ticks under the bands.
pub const TICK_ROW_ID: &str = "perf-overview-timeline-tick-row";

/// The prefix of each event marker's debug selector, followed by its index.
pub const MARKER_ID_PREFIX: &str = "perf-overview-timeline-marker-";

/// The debug selector of the "Back to live" button in the Overview header.
pub const BACK_TO_LIVE_ID: &str = "perf-overview-back-to-live";

/// The box's layout modes (UI-PRF-019). `bands` is the only one.
pub const MODES: &[BoxMode] = &[BoxMode {
    id: "bands",
    label: "perf-overview-mode-bands",
    split: None,
}];

/// One heat band's height, logical px.
pub const BAND_H: f32 = 14.;
/// The gap between bands, logical px.
const BAND_GAP: f32 = 2.;
/// The band label column, logical px.
const LABEL_W: f32 = 64.;
/// The marker row above the bands, logical px.
const MARKER_ROW: f32 = 18.;
/// The hour tick row under the bands, logical px.
const TICK_ROW: f32 = 16.;
/// A marker label shows only with at least this much room before the next marker, logical px.
const LABEL_MIN_ROOM: f32 = 32.;
/// Markers closer than this merge into one, logical px.
pub const MERGE_PX: f32 = 8.;
/// A press that moves less than this is a click, logical px.
const DRAG_MIN_PX: f32 = 4.;
/// How long a preview stays after the pointer is released.
pub const PREVIEW_HOLD: Duration = Duration::from_secs(30);
/// How long a `history` click waits for the process history's answer.
pub const HISTORY_ASK_TIMEOUT: Duration = Duration::from_millis(1_000);
/// The first id of the box's process history queries, apart from the History page's (from 1)
/// and its hover card's (from 2^48).
pub const HISTORY_ASK_ID_BASE: u64 = 1 << 52;
/// An answer this close to the asked second still counts as that second: the newest frame can
/// lag the clock by a second.
const HISTORY_ASK_SLACK_NS: u64 = 2 * NS_PER_SEC as u64;
/// Heat levels a band is drawn with; neighbouring cells of one level paint as one quad.
const LEVELS: f32 = 12.;
/// The alpha of the lowest and the highest level.
const ALPHA: (f32, f32) = (0.15, 1.0);

const NS_PER_SEC: i64 = 1_000_000_000;
const HOUR_NS: i64 = 3_600 * NS_PER_SEC;
/// The span the timeline covers.
pub const DAY_NS: i64 = 24 * HOUR_NS;

// ---- Paused view ----

/// How the Overview is held at a time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PauseKind {
    /// Until "Back to live".
    Pause,
    /// While the pointer is down and for [`PREVIEW_HOLD`] after.
    Preview,
}

/// The time the Overview shows instead of now, on CLOCK_REALTIME.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Paused {
    pub at_realtime_ns: i64,
    pub kind: PauseKind,
}

/// The Overview's paused time; `None` is live.
#[derive(Default)]
pub struct OverviewPause(pub Option<Paused>);

impl Global for OverviewPause {}

/// The paused time, `None` while live.
pub fn paused(cx: &App) -> Option<Paused> {
    cx.try_global::<OverviewPause>().and_then(|p| p.0)
}

/// Holds the Overview at `p`, or makes it live with `None`. Observers run only on a change.
pub fn set_paused(p: Option<Paused>, cx: &mut App) {
    if paused(cx) != p {
        cx.set_global(OverviewPause(p));
    }
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static TEST_CLOCK: Cell<Option<(i64, TimeNs)>> = const { Cell::new(None) };
}

/// Pins the clocks the timeline and the paused view read on this thread: CLOCK_REALTIME at
/// `realtime_ns` and CLOCK_BOOTTIME at `boot_ns`. `None` reads the real clocks again. Tests use
/// it so they do not depend on the host's clock or uptime.
#[cfg(any(test, feature = "test-hooks"))]
pub fn set_test_clock(clock: Option<(i64, TimeNs)>) {
    TEST_CLOCK.set(clock);
}

/// CLOCK_REALTIME now.
pub fn now_realtime_ns() -> i64 {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some((realtime, _)) = TEST_CLOCK.get() {
        return realtime;
    }
    i64::try_from(jiff::Timestamp::now().as_nanosecond()).unwrap_or(i64::MAX)
}

/// CLOCK_BOOTTIME now.
pub fn now_boot() -> TimeNs {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some((_, boot)) = TEST_CLOCK.get() {
        return boot;
    }
    now_boot_ns()
}

/// The end of `range` held at `at`: the picked time centred in the range, never after `now`.
pub fn paused_end(at: i64, range: OverviewRange, now: i64) -> i64 {
    at.saturating_add(range.span_ns() as i64 / 2).min(now)
}

/// Whether the finest tier that holds a time `age_ns` old serves `range` (UI-PRF-057): 1 min
/// and 5 min need the 1 s tier (the last hour), 1 h the 10 s tier or finer (the last 24 hours).
pub fn servable(range: OverviewRange, age_ns: i64) -> bool {
    match range {
        OverviewRange::M1 | OverviewRange::M5 => age_ns <= HOUR_NS,
        OverviewRange::H1 => age_ns <= DAY_NS,
        OverviewRange::H24 | OverviewRange::D7 => true,
    }
}

/// The smallest preset range that holds a span of `span_ns`.
pub fn smallest_range(span_ns: u64) -> OverviewRange {
    OverviewRange::ALL
        .into_iter()
        .find(|r| r.span_ns() >= span_ns)
        .unwrap_or(OverviewRange::D7)
}

/// `range`, or the smallest larger range [`servable`] at a time `age_ns` old.
pub fn servable_range(range: OverviewRange, age_ns: i64) -> OverviewRange {
    OverviewRange::ALL
        .into_iter()
        .filter(|r| r.span_ns() >= range.span_ns())
        .find(|&r| servable(r, age_ns))
        .unwrap_or(OverviewRange::D7)
}

/// `performance.timeline_click`, with `history` read as `pause` while `history.enabled` is off.
pub fn click_mode(cx: &App) -> TimelineClick {
    if !cx.has_global::<TlSettings>() {
        return TimelineClick::default();
    }
    let s = TlSettings::settings(cx);
    match s.performance.timeline_click {
        TimelineClick::History if !s.history.enabled => TimelineClick::Pause,
        click => click,
    }
}

/// The click choices offered now: `history` only while `history.enabled` is on.
pub fn click_choices(cx: &App) -> SmallVec<[TimelineClick; 3]> {
    let recording = cx.has_global::<TlSettings>() && TlSettings::settings(cx).history.enabled;
    let mut choices: SmallVec<[TimelineClick; 3]> =
        SmallVec::from_slice(&[TimelineClick::Pause, TimelineClick::Preview]);
    if recording {
        choices.push(TimelineClick::History);
    }
    choices
}

/// The saved value of `click`.
pub const fn click_key(click: TimelineClick) -> &'static str {
    match click {
        TimelineClick::Pause => "pause",
        TimelineClick::Preview => "preview",
        TimelineClick::History => "history",
    }
}

/// `click`'s menu and Settings word.
pub const fn click_label(click: TimelineClick) -> &'static str {
    match click {
        TimelineClick::Pause => "perf-overview-timeline-click-pause",
        TimelineClick::Preview => "perf-overview-timeline-click-preview",
        TimelineClick::History => "perf-overview-timeline-click-history",
    }
}

/// The value saved as `key`.
pub fn click_from_key(key: &str) -> Option<TimelineClick> {
    [
        TimelineClick::Pause,
        TimelineClick::Preview,
        TimelineClick::History,
    ]
    .into_iter()
    .find(|c| click_key(*c) == key)
}

// ---- Geometry ----

/// One painted run of a band: from `x0` to `x1` px at heat `level` (1..=LEVELS).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BandRun {
    pub x0: f32,
    pub x1: f32,
    pub level: u8,
}

/// The heat level of `value` on a scale of `full` (0 below, LEVELS at or above), `None` for a
/// gap or without a scale.
fn level(value: f32, full: f32) -> Option<u8> {
    if value.is_nan() || full <= 0. || full.is_nan() {
        return None;
    }
    let f = (value / full).clamp(0., 1.);
    Some(((f * (LEVELS - 1.)).round() + 1.) as u8)
}

/// The runs of one band `width` px wide over `[end − 24 h, end]`: each point covers its bucket
/// of `step_ns`, gaps paint nothing, and neighbouring buckets of one level join into one run.
/// `full` is the value drawn at full intensity.
pub fn band_cells(
    points: &[HistPoint],
    step_ns: u64,
    end_ns: i64,
    width: f32,
    full: f32,
) -> SmallVec<[BandRun; 32]> {
    let mut out: SmallVec<[BandRun; 32]> = SmallVec::new();
    if width <= 0. {
        return out;
    }
    let start = end_ns - DAY_NS;
    let x = |t: i64| ((t - start) as f64 / DAY_NS as f64 * f64::from(width)) as f32;
    let step = step_ns.max(1) as i64;
    for p in points {
        let Some(level) = level(p.mean, full) else {
            continue;
        };
        let (x0, x1) = (
            x(p.t_realtime_ns).max(0.),
            x(p.t_realtime_ns + step).min(width),
        );
        if x1 <= x0 {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.level == level && (x0 - last.x1).abs() < 0.5 => last.x1 = x1,
            _ => out.push(BandRun { x0, x1, level }),
        }
    }
    out
}

/// The value each band draws at full intensity: 100 for CPU and Disk (percent), the memory
/// total for Memory, and the band's own peak for Network, which has no fixed scale.
pub fn band_full(band: TimelineBand, points: &[HistPoint], mem_total: f32) -> f32 {
    match band {
        TimelineBand::Cpu | TimelineBand::Disk => 100.,
        TimelineBand::Memory => mem_total,
        TimelineBand::Network => points
            .iter()
            .filter(|p| !p.mean.is_nan())
            .map(|p| p.mean)
            .fold(0., f32::max),
    }
}

/// One drawn marker: at `x` px, standing for `count` events from index `first` on.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Marker {
    pub x: f32,
    pub first: usize,
    pub count: usize,
}

/// Merges markers at `xs` (ascending px): a marker closer than [`MERGE_PX`] × `scale` to the
/// first of the current group joins it.
pub fn merge_markers(xs: &[f32], scale: f32) -> SmallVec<[Marker; 16]> {
    let mut out: SmallVec<[Marker; 16]> = SmallVec::new();
    for (i, &x) in xs.iter().enumerate() {
        match out.last_mut() {
            Some(m) if x - m.x < MERGE_PX * scale => m.count += 1,
            _ => out.push(Marker {
                x,
                first: i,
                count: 1,
            }),
        }
    }
    out
}

/// The px of time `t` on a timeline `width` px wide that ends at `end`.
fn x_of(t: i64, end: i64, width: f32) -> f32 {
    ((t - (end - DAY_NS)) as f64 / DAY_NS as f64 * f64::from(width)) as f32
}

/// The time at `x` px on a timeline `width` px wide that ends at `end`.
pub fn t_of(x: f32, end: i64, width: f32) -> i64 {
    let f = f64::from((x / width.max(1.)).clamp(0., 1.));
    end - DAY_NS + (f * DAY_NS as f64) as i64
}

/// Local "HH:MM" of `t`.
fn clock(t: i64) -> String {
    jiff::Timestamp::from_nanosecond(i128::from(t)).map_or_else(
        |_| String::new(),
        |ts| {
            ts.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        },
    )
}

/// The whole local hours inside `[end − 24 h, end]`, with their hour of the day.
fn hour_ticks(end: i64) -> SmallVec<[(i64, u8); 25]> {
    let mut out = SmallVec::new();
    let Ok(ts) = jiff::Timestamp::from_nanosecond(i128::from(end)) else {
        return out;
    };
    let tz = jiff::tz::TimeZone::system();
    let offset = i64::from(tz.to_offset(ts).seconds()) * NS_PER_SEC;
    let start = end - DAY_NS;
    let mut t = (start + offset).div_euclid(HOUR_NS) * HOUR_NS - offset;
    if t < start {
        t += HOUR_NS;
    }
    while t <= end {
        let hour = jiff::Timestamp::from_nanosecond(i128::from(t))
            .map(|ts| ts.to_zoned(tz.clone()).hour())
            .unwrap_or(0);
        out.push((t, hour as u8));
        t += HOUR_NS;
    }
    out
}

// ---- The box ----

/// The fixed words of the box.
struct Words {
    title: SharedString,
    bands: [SharedString; 4],
    /// In [`event_index`] order.
    events: [SharedString; 8],
    merged: Template,
    own_only: SharedString,
}

impl Words {
    fn new(cx: &App) -> Self {
        Self {
            title: tr(cx, "perf-overview-box-timeline"),
            bands: [
                tr(cx, "perf-overview-timeline-band-cpu"),
                tr(cx, "perf-overview-timeline-band-memory"),
                tr(cx, "perf-overview-timeline-band-disk"),
                tr(cx, "perf-overview-timeline-band-network"),
            ],
            events: [
                "perf-overview-timeline-event-pressure",
                "perf-overview-timeline-event-swap",
                "perf-overview-timeline-event-heat",
                "perf-overview-timeline-event-full-load",
                "perf-overview-timeline-event-suspend",
                "perf-overview-timeline-event-boot",
                "perf-overview-timeline-event-oom",
                "perf-overview-timeline-event-crash",
            ]
            .map(|k| tr(cx, k)),
            merged: Template::counted(cx, "perf-overview-timeline-merged", &["count"], 0),
            own_only: tr(cx, "perf-overview-timeline-journal-own-only"),
        }
    }
}

fn event_index(kind: TimelineEventKind) -> usize {
    match kind {
        TimelineEventKind::MemoryPressure => 0,
        TimelineEventKind::SwapStorm => 1,
        TimelineEventKind::Heat => 2,
        TimelineEventKind::FullLoad => 3,
        TimelineEventKind::Suspend => 4,
        TimelineEventKind::Boot => 5,
        TimelineEventKind::OomKill => 6,
        TimelineEventKind::Crash => 7,
    }
}

/// The marker colour of `kind`: warnings for pressure, danger for heat, load and kills, the
/// secondary text colour for sleeps and boots.
fn event_color(kind: TimelineEventKind, theme: &TlTheme) -> Hsla {
    match kind {
        TimelineEventKind::MemoryPressure | TimelineEventKind::SwapStorm => theme.status.warning,
        TimelineEventKind::Heat
        | TimelineEventKind::FullLoad
        | TimelineEventKind::OomKill
        | TimelineEventKind::Crash => theme.status.danger,
        TimelineEventKind::Suspend | TimelineEventKind::Boot => theme.text_secondary,
    }
}

/// Whether `e` is drawn: OOM kills and crashes are left out while the journal shows only the
/// user's own entries (UI-PRF-057).
fn event_shown(d: &DeviceTimeline, e: &TimelineEvent) -> bool {
    d.journal != Some(JournalAccess::OwnOnly)
        || !matches!(
            e.kind,
            TimelineEventKind::OomKill | TimelineEventKind::Crash
        )
}

/// Whether a `TopAt` answer has process history at the asked second `t`, boot clock. The
/// engine moves a time outside the retained span to the span's edge, and finds no block in a
/// gap, which leaves the memory total at 0.
pub fn answer_holds(top: &HistoryTop, t: TimeNs) -> bool {
    let second = t / NS_PER_SEC as u64 * NS_PER_SEC as u64;
    top.mem_total_bytes > 0 && top.t_ns.abs_diff(second) <= HISTORY_ASK_SLACK_NS
}

fn band_accent(band: TimelineBand) -> DeviceAccent {
    match band {
        TimelineBand::Cpu => DeviceAccent::Cpu,
        TimelineBand::Memory => DeviceAccent::Memory,
        TimelineBand::Disk => DeviceAccent::Disk,
        TimelineBand::Network => DeviceAccent::Net,
    }
}

/// A press on the bands, in px from the band area's left edge.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Press {
    x0: f32,
    x: f32,
    /// Moved at least [`DRAG_MIN_PX`]: a drag that picks a span.
    dragging: bool,
}

/// A `history` click waiting for the process history's answer.
struct HistoryAsk {
    id: u64,
    /// The asked second, boot clock.
    t: TimeNs,
    /// The window History mode opens in.
    window: AnyWindowHandle,
}

/// The 24-hour timeline box.
pub struct TimelineBox {
    hub: Entity<SnapshotHub>,
    data: Option<Arc<DeviceTimeline>>,
    /// The memory total for the Memory band, bytes.
    mem_total: f32,
    words: Words,
    /// The pause and the range the bracket was drawn for.
    shown: (Option<Paused>, OverviewRange),
    /// The band area from the last paint, window px.
    area: Rc<Cell<Bounds<Pixels>>>,
    /// The band area's width the markers and ticks were placed for, px.
    width: f32,
    press: Option<Press>,
    /// The timer that ends a preview.
    preview: Option<Task<()>>,
    history_ask: Option<HistoryAsk>,
    next_ask_id: u64,
    style_version: u64,
    strings_version: u64,
    _subs: [Subscription; 5],
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    /// Painted band runs of the last paint, per band.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_cells: RefCell<[SmallVec<[BandRun; 32]>; 4]>,
    /// Markers of the last render.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_markers: RefCell<SmallVec<[Marker; 16]>>,
    /// The text of the last toast the box pushed.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_toast: RefCell<Option<SharedString>>,
    /// The time the last `history` click opened History mode at, boot clock.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_history_at: Cell<Option<TimeNs>>,
}

impl TimelineBox {
    pub fn new(hub: &Entity<SnapshotHub>, cx: &mut Context<Self>) -> Self {
        let snap = Arc::clone(hub.read(cx).current());
        Self {
            hub: hub.clone(),
            data: snap.device_history.timeline.clone(),
            mem_total: snap.system.memory.total as f32,
            words: Words::new(cx),
            shown: (paused(cx), range::current_range(cx)),
            area: Rc::new(Cell::new(Bounds::default())),
            width: 0.,
            press: None,
            preview: None,
            history_ask: None,
            next_ask_id: HISTORY_ASK_ID_BASE,
            style_version: metrics(cx).style_version,
            strings_version: cx.global::<Strings>().strings_version,
            _subs: [
                cx.subscribe(hub, Self::on_snapshot),
                cx.observe_global::<OverviewPause>(Self::sync_shown),
                cx.observe_global::<TlSettings>(Self::sync_shown),
                cx.observe_global::<UiMetrics>(Self::sync_style),
                cx.observe_global::<Strings>(Self::sync_strings),
            ],
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            last_cells: RefCell::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            last_markers: RefCell::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            last_toast: RefCell::default(),
            #[cfg(any(test, feature = "test-hooks"))]
            last_history_at: Cell::new(None),
        }
    }

    /// The timeline the box draws.
    pub fn data(&self) -> Option<&Arc<DeviceTimeline>> {
        self.data.as_ref()
    }

    /// "Needs the systemd-journal group" while the journal shows only the user's own entries,
    /// so OOM kills and crashes are left out (UI-PRF-057).
    pub fn journal_note(&self) -> Option<SharedString> {
        self.data
            .as_ref()
            .filter(|d| d.journal == Some(JournalAccess::OwnOnly))
            .map(|_| self.words.own_only.clone())
    }

    /// "Timeline" and one line per event: its name and local time.
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.words.title);
        out.push('\n');
        if let Some(d) = &self.data {
            for e in d.events.iter().filter(|e| event_shown(d, e)) {
                let _ = writeln!(
                    out,
                    "{}\t{}",
                    self.words.events[event_index(e.kind)],
                    clock(e.start_realtime_ns)
                );
            }
        }
    }

    /// A new timeline component: the box redraws when its `Arc` changed. A process history
    /// answer may settle a `history` click.
    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        if ev.mask.contains(ComponentMask::HISTORY) {
            self.on_history_answer(&hub, cx);
        }
        if !ev.mask.contains(ComponentMask::DEVICE_HISTORY) {
            return;
        }
        let snap = Arc::clone(hub.read(cx).current());
        let same = match (&self.data, &snap.device_history.timeline) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.data = snap.device_history.timeline.clone();
        self.mem_total = snap.system.memory.total as f32;
        cx.notify();
    }

    /// The pause or the range changed: the bracket moves.
    fn sync_shown(&mut self, cx: &mut Context<Self>) {
        let now = (paused(cx), range::current_range(cx));
        if now != self.shown {
            self.shown = now;
            cx.notify();
        }
    }

    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version != self.style_version {
            self.style_version = version;
            cx.notify();
        }
    }

    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version != self.strings_version {
            self.strings_version = version;
            self.words = Words::new(cx);
            cx.notify();
        }
    }

    /// The end of the 24 hours drawn: the timeline's, or now before the first one.
    fn end(&self) -> i64 {
        self.data
            .as_ref()
            .map_or_else(now_realtime_ns, |d| d.end_realtime_ns)
    }

    /// The band area was painted `width` px wide: markers and ticks follow on a change.
    fn measured(&mut self, width: f32, cx: &mut Context<Self>) {
        if (width - self.width).abs() >= 0.5 {
            self.width = width;
            cx.notify();
        }
    }

    // ---- Pointer ----

    fn down(&mut self, x: f32, kind: TimelineClick, cx: &mut Context<Self>) {
        self.press = Some(Press {
            x0: x,
            x,
            dragging: false,
        });
        if kind == TimelineClick::Preview {
            // The preview shows while the pointer is down.
            self.preview = None;
            let at = t_of(x, self.end(), self.width);
            self.hold(at, PauseKind::Preview, cx);
        }
        cx.notify();
    }

    fn moved(&mut self, x: f32, cx: &mut Context<Self>) {
        let scale = metrics(cx).ui_scale;
        let Some(press) = &mut self.press else {
            return;
        };
        press.x = x;
        if !press.dragging && (x - press.x0).abs() >= DRAG_MIN_PX * scale {
            press.dragging = true;
        }
        if press.dragging {
            cx.notify();
        }
    }

    fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else {
            return;
        };
        let end = self.end();
        let width = self.width;
        if press.dragging {
            let (a, b) = (
                t_of(press.x0.min(press.x), end, width),
                t_of(press.x0.max(press.x), end, width),
            );
            range::set_range(smallest_range((b - a) as u64), cx);
            // A span is held as a click holds a time; `history` has no span to open.
            let mode = match click_mode(cx) {
                TimelineClick::Preview => TimelineClick::Preview,
                TimelineClick::Pause | TimelineClick::History => TimelineClick::Pause,
            };
            self.pick_as((a + b) / 2, mode, window, cx);
        } else {
            self.pick(t_of(press.x0, end, width), window, cx);
        }
        cx.notify();
    }

    /// A click at `t` acts by `performance.timeline_click`.
    pub fn pick(&mut self, t: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_as(t, click_mode(cx), window, cx);
    }

    fn pick_as(
        &mut self,
        t: i64,
        mode: TimelineClick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match mode {
            TimelineClick::Pause => {
                self.preview = None;
                self.hold(t, PauseKind::Pause, cx);
            }
            TimelineClick::Preview => {
                self.hold(t, PauseKind::Preview, cx);
                let shown = paused(cx);
                self.preview = Some(cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(PREVIEW_HOLD).await;
                    cx.update(|cx| {
                        if paused(cx) == shown {
                            set_paused(None, cx);
                        }
                    });
                }));
            }
            TimelineClick::History => self.open_history(t, window, cx),
        }
    }

    /// Holds the Overview at `t`. A range the history cannot serve at `t` gives way to the
    /// smallest larger one that it can.
    fn hold(&mut self, t: i64, kind: PauseKind, cx: &mut Context<Self>) {
        let age = now_realtime_ns() - t;
        let range = range::current_range(cx);
        let usable = servable_range(range, age);
        if usable != range {
            range::set_range(usable, cx);
        }
        set_paused(
            Some(Paused {
                at_realtime_ns: t,
                kind,
            }),
            cx,
        );
    }

    /// Asks the process history for the second at `t`, CLOCK_REALTIME. The answer opens History
    /// mode there or shows the toast ([`Self::on_history_answer`]); no answer within
    /// [`HISTORY_ASK_TIMEOUT`] shows the toast.
    fn open_history(&mut self, t: i64, window: &mut Window, cx: &mut Context<Self>) {
        let offset = now_realtime_ns() - now_boot() as i64;
        let Ok(boot_t) = TimeNs::try_from(t - offset) else {
            self.no_history(cx);
            return;
        };
        let id = self.next_ask_id;
        self.next_ask_id += 1;
        let cmd = Command::HistoryQuery {
            id,
            range: HistoryRange::At(boot_t),
            what: HistoryWhat::TopAt {
                t: boot_t,
                metric: HistoryMetric::Cpu,
                n: 1,
            },
        };
        if self.hub.read(cx).client().send(cmd).is_err() {
            self.no_history(cx);
            return;
        }
        self.history_ask = Some(HistoryAsk {
            id,
            t: boot_t,
            window: window.window_handle(),
        });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HISTORY_ASK_TIMEOUT).await;
            let _ = this.update(cx, |b, cx| {
                if b.history_ask.as_ref().is_some_and(|a| a.id == id) {
                    b.history_ask = None;
                    b.no_history(cx);
                }
            });
        })
        .detach();
    }

    /// Settles the waiting `history` click when the hub's process history answers it.
    fn on_history_answer(&mut self, hub: &Entity<SnapshotHub>, cx: &mut Context<Self>) {
        let Some(id) = self.history_ask.as_ref().map(|a| a.id) else {
            return;
        };
        let Some(top) = hub.read(cx).current().history.top.clone() else {
            return;
        };
        if top.query_id != id {
            return;
        }
        let Some(ask) = self.history_ask.take() else {
            return;
        };
        if !answer_holds(&top, ask.t) {
            self.no_history(cx);
            return;
        }
        #[cfg(any(test, feature = "test-hooks"))]
        self.last_history_at.set(Some(ask.t));
        let (t, window) = (ask.t, ask.window);
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                crate::pages::processes::history::open_at(t, window, cx);
            });
        });
    }

    /// The toast "No process history at that time".
    fn no_history(&self, cx: &mut App) {
        let text = tr(cx, "perf-overview-timeline-no-history");
        #[cfg(any(test, feature = "test-hooks"))]
        self.last_toast.replace(Some(text.clone()));
        push_main_toast(cx, ToastKind::Info, text, None, None);
    }
}

impl Render for TimelineBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let s = m.ui_scale;
        let end = self.end();
        let width = self.width;
        let bands = self.bands(&theme, cx.entity().downgrade());
        let markers = self.markers(end, width, &m, &theme, window, cx);
        let ticks = self.ticks(end, width, &m, &theme);
        let overlays = self.overlays(end, width, &m, &theme, cx);

        let labels = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(LABEL_W * s))
            .gap(px(BAND_GAP * s))
            .children(self.words.bands.iter().map(|w| {
                let mut d = div();
                *d.text_style() = t_xs(&m);
                d.h(px(BAND_H * s))
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_color(theme.text_secondary)
                    .child(w.clone())
            }));
        let band_h = (4. * BAND_H + 3. * BAND_GAP) * s;
        let area = div()
            .debug_selector(|| BANDS_ID.into())
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(band_h))
            .child(bands)
            .children(overlays);
        let area = match self.journal_note() {
            Some(note) => {
                let state = HoverTooltip::state(BANDS_ID, window, cx);
                HoverTooltip::attach(
                    &state,
                    area,
                    DELAY_CONTROL,
                    TlTooltip::text(note).build(),
                    window,
                    cx,
                )
            }
            None => area,
        };
        let row = |id: &'static str, child: AnyElement, h: f32| {
            div()
                .debug_selector(move || id.into())
                .flex()
                .flex_row()
                .flex_none()
                .h(px(h * s))
                .child(div().flex_none().w(px(LABEL_W * s)))
                .child(div().relative().flex_1().min_w_0().h_full().child(child))
        };
        // Only the rows that fit whole draw, top first (UI-PRF-017).
        let body = whole_rows()
            .size_full()
            .child(row(MARKER_ROW_ID, markers, MARKER_ROW))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .child(labels)
                    .child(area),
            )
            .child(row(TICK_ROW_ID, ticks, TICK_ROW))
            .into_any_element();
        box_frame(
            TIMELINE_BOX_ID,
            None,
            self.words.title.clone(),
            SharedString::default(),
            self.journal_note().unwrap_or_default(),
            &RangeText::default(),
            body,
            edit_chrome(cx),
            &m,
            &theme,
        )
        .size_full()
    }
}

impl TimelineBox {
    /// The band canvas: one quad per run, a line per boot, and the pointer listeners.
    fn bands(&self, theme: &TlTheme, this: WeakEntity<Self>) -> AnyElement {
        let data = self.data.clone();
        let mem_total = self.mem_total;
        let colors = TimelineBand::ALL.map(|b| theme.graph.device_accent(band_accent(b)));
        let boot_color = theme.text_secondary;
        let area = Rc::clone(&self.area);
        let end = self.end();
        canvas(
            move |bounds, _, _| bounds,
            move |bounds, _, window, cx| {
                area.set(bounds);
                let s = metrics(cx).ui_scale;
                let w = f32::from(bounds.size.width);
                let Some(entity) = this.upgrade() else {
                    return;
                };
                let measured = entity.clone();
                cx.defer(move |cx| measured.update(cx, |b, cx| b.measured(w, cx)));
                if let Some(d) = &data {
                    for (i, band) in TimelineBand::ALL.into_iter().enumerate() {
                        let points = &d.bands[i];
                        let full = band_full(band, points, mem_total);
                        let cells = band_cells(points, d.step_ns, d.end_realtime_ns, w, full);
                        let top = bounds.top() + px(i as f32 * (BAND_H + BAND_GAP) * s);
                        for c in &cells {
                            let a = ALPHA.0
                                + (ALPHA.1 - ALPHA.0) * f32::from(c.level - 1) / (LEVELS - 1.);
                            let b = Bounds::new(
                                point(bounds.left() + px(c.x0), top),
                                size(px(c.x1 - c.x0), px(BAND_H * s)),
                            );
                            window.paint_quad(fill(b, colors[i].alpha(a)));
                        }
                        #[cfg(any(test, feature = "test-hooks"))]
                        {
                            entity.read(cx).last_cells.borrow_mut()[i] = cells;
                        }
                    }
                    for &boot in d.boots.iter() {
                        let x = x_of(boot, end, w);
                        if !(0. ..=w).contains(&x) {
                            continue;
                        }
                        let b = Bounds::new(
                            point(bounds.left() + px(x), bounds.top()),
                            size(px(1.), bounds.size.height),
                        );
                        window.paint_quad(fill(b, boot_color));
                    }
                }
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, _, cx| {
                    if phase.bubble()
                        && ev.button == MouseButton::Left
                        && bounds.contains(&ev.position)
                    {
                        let x = f32::from(ev.position.x - bounds.left());
                        let kind = click_mode(cx);
                        e.update(cx, |b, cx| b.down(x, kind, cx));
                        cx.stop_propagation();
                    }
                });
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                    if phase.bubble() && e.read(cx).press.is_some() {
                        let x = f32::from(ev.position.x - bounds.left()).clamp(0., w);
                        e.update(cx, |b, cx| b.moved(x, cx));
                    }
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                    if phase.bubble() && entity.read(cx).press.is_some() {
                        entity.update(cx, |b, cx| b.up(window, cx));
                    }
                });
            },
        )
        .size_full()
        .into_any_element()
    }

    /// The event markers above the bands, merged where they are closer than [`MERGE_PX`]. Each
    /// shows its name and local time, or the count of a merged group, and lists its events in
    /// a tooltip.
    fn markers(
        &self,
        end: i64,
        width: f32,
        m: &UiMetrics,
        theme: &TlTheme,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let s = m.ui_scale;
        let mut root = div().relative().size_full();
        let Some(d) = self.data.as_ref().filter(|_| width > 0.) else {
            #[cfg(any(test, feature = "test-hooks"))]
            self.last_markers.borrow_mut().clear();
            return root.into_any_element();
        };
        let shown: SmallVec<[(usize, f32); 16]> = d
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| event_shown(d, e))
            .map(|(i, e)| (i, x_of(e.start_realtime_ns, end, width)))
            .filter(|(_, x)| (0. ..=width).contains(x))
            .collect();
        let xs: SmallVec<[f32; 16]> = shown.iter().map(|(_, x)| *x).collect();
        let merged = merge_markers(&xs, s);
        #[cfg(any(test, feature = "test-hooks"))]
        self.last_markers.replace(merged.clone());
        for (n, mk) in merged.iter().enumerate() {
            let group = &shown[mk.first..mk.first + mk.count];
            let first = &d.events[group[0].0];
            let mut label = String::new();
            if mk.count == 1 {
                let _ = write!(
                    label,
                    "{} {}",
                    self.words.events[event_index(first.kind)],
                    clock(first.start_realtime_ns)
                );
            } else {
                let _ = self
                    .words
                    .merged
                    .write_count(&mut label, mk.count as u64, &[&mk.count]);
            }
            let lines: Vec<SharedString> = group
                .iter()
                .map(|(i, _)| {
                    let e = &d.events[*i];
                    let mut line = format!(
                        "{} {}",
                        self.words.events[event_index(e.kind)],
                        clock(e.start_realtime_ns)
                    );
                    if let Some(p) = &e.process {
                        let _ = write!(line, " · {p}");
                    }
                    line.into()
                })
                .collect();
            // The label ends before the next marker.
            let room = merged
                .get(n + 1)
                .map_or(width - mk.x, |next| next.x - mk.x)
                .max(2. * s);
            let color = event_color(first.kind, theme);
            let id = format!("{MARKER_ID_PREFIX}{n}");
            let mut text = div();
            *text.text_style() = t_xs(m);
            let text = text
                .pl(px(3. * s))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_color(color)
                .child(SharedString::from(label));
            let marker = div()
                .debug_selector(move || id.clone())
                .absolute()
                .left(px(mk.x))
                .top_0()
                .bottom_0()
                .w(px(room))
                .flex()
                .flex_row()
                .items_end()
                .overflow_hidden()
                .child(div().flex_none().w(px(2. * s)).h_full().bg(color))
                // A label cut to a letter or two says nothing; the tooltip still lists it.
                .children((room >= LABEL_MIN_ROOM * s).then_some(text));
            let state = HoverTooltip::state((MARKER_ID_PREFIX, n), window, cx);
            let marker = HoverTooltip::attach(
                &state,
                marker,
                DELAY_CONTROL,
                TlTooltip::lines(lines).build(),
                window,
                cx,
            );
            root = root.child(marker);
        }
        root.into_any_element()
    }

    /// The hour ticks under the bands: a short line at every whole local hour, and its hour
    /// ("00".."23") where there is room, every 3rd or 6th hour on narrow boxes.
    fn ticks(&self, end: i64, width: f32, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
        let s = m.ui_scale;
        let mut root = div().relative().size_full();
        if width <= 0. {
            return root.into_any_element();
        }
        let per_hour = width / 24.;
        let every = if per_hour >= 24. * s {
            1
        } else if per_hour >= 10. * s {
            3
        } else {
            6
        };
        let label_w = 20. * s;
        for (t, hour) in hour_ticks(end) {
            let x = x_of(t, end, width);
            root = root.child(
                div()
                    .absolute()
                    .left(px(x))
                    .top_0()
                    .w(px(1.))
                    .h(px(4. * s))
                    .bg(theme.divider),
            );
            if hour % every != 0 {
                continue;
            }
            let left = (x - label_w / 2.).clamp(0., (width - label_w).max(0.));
            let mut label = div();
            *label.text_style() = t_xs(m);
            root = root.child(
                label
                    .absolute()
                    .left(px(left))
                    .top(px(4. * s))
                    .w(px(label_w))
                    .flex()
                    .justify_center()
                    .whitespace_nowrap()
                    .text_color(theme.text_secondary)
                    .child(SharedString::from(format!("{hour:02}"))),
            );
        }
        root.into_any_element()
    }

    /// The bracket over the span the boxes show, and the span a drag is picking.
    fn overlays(
        &self,
        end: i64,
        width: f32,
        m: &UiMetrics,
        theme: &TlTheme,
        cx: &App,
    ) -> SmallVec<[AnyElement; 2]> {
        let mut out = SmallVec::new();
        if width <= 0. {
            return out;
        }
        let s = m.ui_scale;
        let range = range::current_range(cx);
        let shown_end = paused(cx).map_or(end, |p| {
            paused_end(p.at_realtime_ns, range, now_realtime_ns())
        });
        let x1 = x_of(shown_end, end, width).clamp(0., width);
        let x0 = x_of(shown_end - range.span_ns() as i64, end, width).clamp(0., width);
        let w = (x1 - x0).max(2. * s);
        out.push(
            div()
                .debug_selector(|| BRACKET_ID.into())
                .absolute()
                .left(px((x1 - w).max(0.)))
                .top(px(-2. * s))
                .bottom(px(-2. * s))
                .w(px(w))
                .border_1()
                .border_color(theme.accent)
                .bg(theme.accent.alpha(0.08))
                .into_any_element(),
        );
        if let Some(press) = self.press.filter(|p| p.dragging) {
            let (a, b) = (press.x0.min(press.x), press.x0.max(press.x));
            out.push(
                div()
                    .debug_selector(|| SELECTION_ID.into())
                    .absolute()
                    .left(px(a))
                    .top_0()
                    .bottom_0()
                    .w(px(b - a))
                    .bg(theme.accent.alpha(0.2))
                    .into_any_element(),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_merge_within_8_px_of_the_group_start() {
        let merged = merge_markers(&[10., 14., 17.9, 18., 40.], 1.);
        let got: Vec<(f32, usize, usize)> =
            merged.iter().map(|m| (m.x, m.first, m.count)).collect();
        assert_eq!(got, [(10., 0, 3), (18., 3, 1), (40., 4, 1)]);
        // The distance scales with the UI.
        assert_eq!(merge_markers(&[10., 25.], 2.).len(), 1);
        assert!(merge_markers(&[], 1.).is_empty());
    }

    #[test]
    fn servable_ranges_follow_the_finest_tier() {
        use OverviewRange::*;
        assert!(servable(M1, 30 * 60 * NS_PER_SEC));
        assert!(!servable(M5, 2 * HOUR_NS));
        assert!(servable(H1, 2 * HOUR_NS));
        assert!(!servable(H1, 2 * DAY_NS));
        assert!(servable(D7, 6 * DAY_NS));
        assert_eq!(servable_range(M1, 5 * HOUR_NS), H1);
        assert_eq!(servable_range(M5, 2 * DAY_NS), H24);
        assert_eq!(servable_range(M5, 10 * 60 * NS_PER_SEC), M5);
    }

    #[test]
    fn smallest_range_holds_the_span() {
        use OverviewRange::*;
        assert_eq!(smallest_range(30 * NS_PER_SEC as u64), M1);
        assert_eq!(smallest_range(2 * 60 * NS_PER_SEC as u64), M5);
        assert_eq!(smallest_range(30 * 60 * NS_PER_SEC as u64), H1);
        assert_eq!(smallest_range(3 * HOUR_NS as u64), H24);
        assert_eq!(smallest_range(3 * DAY_NS as u64), D7);
        assert_eq!(smallest_range(30 * DAY_NS as u64), D7);
    }

    #[test]
    fn band_runs_join_equal_levels_and_skip_gaps() {
        let end = 10 * DAY_NS;
        let step = 60 * NS_PER_SEC;
        let p = |i: i64, v: f32| HistPoint {
            t_realtime_ns: end - DAY_NS + i * step,
            mean: v,
            min: v,
            max: v,
        };
        let points = [p(0, 50.), p(1, 50.), p(2, f32::NAN), p(3, 100.), p(4, 0.)];
        let runs = band_cells(&points, step as u64, end, 1440., 100.);
        let got: Vec<(f32, f32, u8)> = runs.iter().map(|r| (r.x0, r.x1, r.level)).collect();
        assert_eq!(got, [(0., 2., 7), (3., 4., 12), (4., 5., 1)]);
        // Points older than the 24 hours are clipped; no scale paints nothing.
        assert!(band_cells(&[p(-5, 50.)], step as u64, end, 1440., 100.).is_empty());
        assert!(band_cells(&points, step as u64, end, 1440., 0.).is_empty());
    }

    #[test]
    fn t_and_x_round_trip() {
        let end = 3 * DAY_NS;
        assert_eq!(t_of(0., end, 960.), end - DAY_NS);
        assert_eq!(t_of(960., end, 960.), end);
        assert_eq!(t_of(480., end, 960.), end - DAY_NS / 2);
        assert!((x_of(end - DAY_NS / 4, end, 960.) - 720.).abs() < 1e-3);
    }

    #[test]
    fn hour_ticks_cover_the_day() {
        let end = 20_000 * DAY_NS + 1_234 * NS_PER_SEC;
        let ticks = hour_ticks(end);
        assert_eq!(ticks.len(), 24, "{ticks:?}");
        assert!(ticks.windows(2).all(|w| w[1].0 - w[0].0 == HOUR_NS));
        assert!(
            ticks
                .iter()
                .all(|&(t, _)| (end - DAY_NS..=end).contains(&t))
        );
    }

    #[test]
    fn paused_end_is_never_after_now() {
        let now = 100 * HOUR_NS;
        assert_eq!(
            paused_end(now - HOUR_NS, OverviewRange::M1, now),
            now - HOUR_NS + 30 * NS_PER_SEC
        );
        assert_eq!(paused_end(now - HOUR_NS, OverviewRange::H24, now), now);
    }
}
