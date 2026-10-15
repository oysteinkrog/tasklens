//! History mode page: header line, layout slots and the state the strip, minimap, chips and
//! table share (UI-PRC-030, PROC-088, PROC-091).
//!
//! `HistoryPage` replaces the live process table while `history_mode` is on. It subscribes to
//! the hub in `new` and notifies only when a change carries `ComponentMask::HISTORY` (ADR-005).
//! The header text is rebuilt outside render, into a reused `String`, and a new `SharedString`
//! is made only when the text changes, so the header re-shapes only then. Nothing here notifies
//! inside `render`.
//!
//! `QueryController` keeps at most one Frame query in flight (UI-PRC-030). The page asks on mode
//! entry, on every scrub or range change and once per second while live; a 1,000 ms timeout
//! resends an unanswered query (R-169 OI-6). A strip window, metric or selection change asks too
//! while scrubbed or with a range set, where no tick runs.
//!
//! The table (E30-019) is a child `TlTreeTable` over [`table::HistoryTableDelegate`]. The page
//! hands it each installed view, the global filter text (the filter box's text when the page is
//! made, then each `FilterChanged`) and the settings it follows, and copies its selection into
//! `HistoryUiState::selected` in selection order.

pub mod actions;
pub mod hover;
pub mod minimap;
pub mod scrubber;
pub mod table;

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, KeyContext, ParentElement as _, Render, SharedString, StyleRefinement,
    Styled as _, Subscription, Task, WeakEntity, Window, div, px,
};
use smallvec::SmallVec;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, HistoryWhat, Tab};
use tl_model::history::{HistoryMetric, HistoryRange, HistoryStoreState, HistoryView};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::settings::{Settings, UiState};
use tl_model::time::TimeNs;
use tl_model::units::{self, LocalDateTime};

use self::actions::{
    DeleteFromHistory, HISTORY_CONTEXT, HistoryLive, HistoryMetricCpu, HistoryMetricDisk,
    HistoryMetricGpu, HistoryMetricMemory, HistoryMetricNetwork, HistoryOldest, HistoryScrub,
    HistorySetRange, HistoryShowInLiveTable, HistoryStepBack, HistoryStepBackMinute,
    HistoryStepForward, HistoryStepForwardMinute, SCRUBBED,
};
use crate::components::toast::ToastKind;
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::{Strings, tr, tr_args, tr_en};
use crate::pages::performance::details::Template;
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::actions::{CopyAllMetrics, CopyDetails};
use crate::pages::processes::jump::JumpTexts;
use crate::settings_global::{Section, TlSettings};
use crate::table::actions::{CopyCell, OpenProperties};
use crate::table::tree_table::{TlTreeTable, TreeTableDelegate as _, TreeTableEvent};
use crate::theme::{bump_style_version, current, metrics};
use crate::util::clipboard_write;
use crate::window::main::{FilterChanged, GlobalShellEvents, main_window_view, push_main_toast};

use self::minimap::{KeptOverview, MinimapCache, MinimapView};
use self::scrubber::{ScrubberCache, StripView};
use self::table::{HistoryTableDelegate, is_live};

const NS_PER_SEC: u64 = 1_000_000_000;

/// The strip window shown by default: the last 10 minutes, or the whole span when shorter.
pub const DEFAULT_VIEW_NS: u64 = 600 * NS_PER_SEC;

/// Layout heights in logical px at 100% scale (UI-PRC-030).
const HEADER_H: f32 = 32.0;
const STRIP_H: f32 = 120.0;
const MINIMAP_H: f32 = 24.0;

/// The live query period (UI-PRC-030).
const LIVE_TICK: Duration = Duration::from_secs(1);
/// An unanswered Frame query is resent after this (R-169 OI-6).
const QUERY_TIMEOUT: Duration = Duration::from_millis(1000);
/// The minimap overview is asked at most this often (UI-PRC-033).
const OVERVIEW_EVERY_NS: u64 = 60 * NS_PER_SEC;
/// The strip draws series for at most this many selected processes.
const MAX_SERIES: usize = 8;

/// The settings sections the table follows, in this order: `processes` (`hide_system`), `units`,
/// `tables`.
const SECTIONS: [Section; 3] = [Section::Processes, Section::Units, Section::Tables];

/// What the strip, minimap, chips and table share. Session only; nothing here persists.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryUiState {
    pub metric: HistoryMetric,
    /// `None` is live.
    pub scrub: Option<TimeNs>,
    pub range: Option<(TimeNs, TimeNs)>,
    /// The strip window. `(0, 0)` until the first span is known.
    pub view_range: (TimeNs, TimeNs),
    /// Selection order. The strip draws series for the first 8.
    pub selected: Vec<ProcessKey>,
    pub banner_hidden: bool,
    pub off_toast_shown: bool,
}

impl Default for HistoryUiState {
    fn default() -> Self {
        Self {
            metric: HistoryMetric::Cpu,
            scrub: None,
            range: None,
            view_range: (0, 0),
            selected: Vec::new(),
            banner_hidden: false,
            off_toast_shown: false,
        }
    }
}

/// The default strip window for `span`: its last 10 minutes, or all of it when shorter.
pub fn default_view_range(span: (TimeNs, TimeNs)) -> (TimeNs, TimeNs) {
    let (oldest, newest) = span;
    (oldest.max(newest.saturating_sub(DEFAULT_VIEW_NS)), newest)
}

/// The header and loading texts, translated once per `strings_version`. `Default` is en-US.
#[derive(Clone, Debug)]
pub struct HistoryWords {
    pub collecting: SharedString,
    pub live: SharedString,
    /// "History Snapshot: {$count} processes - {$min} min {$sec} sec ago".
    pub snapshot: Template,
    /// "History Snapshot: {$count} processes - {$sec} sec ago".
    pub snapshot_sec: Template,
    /// "History Snapshot: {$count} processes - {$hours} h {$min} min ago".
    pub snapshot_hour: Template,
    /// "History Range: {$from} - {$to} ({$duration}), averages".
    pub range: Template,
    /// "{$min} min {$sec} sec".
    pub duration: Template,
    /// "{$sec} sec".
    pub duration_sec: Template,
    /// "{$hours} h {$min} min".
    pub duration_hour: Template,
    pub loading: SharedString,
    /// The strip's right-edge time label.
    pub now: SharedString,
}

const KEY_COLLECTING: &str = "history-header-collecting";
const KEY_LIVE: &str = "history-header-live";
const KEY_SNAPSHOT: &str = "history-header-snapshot";
const KEY_SNAPSHOT_SEC: &str = "history-header-snapshot-sec";
const KEY_SNAPSHOT_HOUR: &str = "history-header-snapshot-hour";
const KEY_RANGE: &str = "history-header-range";
const KEY_DURATION: &str = "history-duration";
const KEY_DURATION_SEC: &str = "history-duration-sec";
const KEY_DURATION_HOUR: &str = "history-duration-hour";
const KEY_LOADING: &str = "history-loading";
const KEY_NOW: &str = "history-scrubber-now";

impl Default for HistoryWords {
    fn default() -> Self {
        Self {
            collecting: tr_en(KEY_COLLECTING, &[]),
            live: tr_en(KEY_LIVE, &[]),
            snapshot: Template::en(KEY_SNAPSHOT, &["count", "min", "sec"]),
            snapshot_sec: Template::en(KEY_SNAPSHOT_SEC, &["count", "sec"]),
            snapshot_hour: Template::en(KEY_SNAPSHOT_HOUR, &["count", "hours", "min"]),
            range: Template::en(KEY_RANGE, &["from", "to", "duration"]),
            duration: Template::en(KEY_DURATION, &["min", "sec"]),
            duration_sec: Template::en(KEY_DURATION_SEC, &["sec"]),
            duration_hour: Template::en(KEY_DURATION_HOUR, &["hours", "min"]),
            loading: tr_en(KEY_LOADING, &[]),
            now: tr_en(KEY_NOW, &[]),
        }
    }
}

impl HistoryWords {
    /// Every text through `tr`.
    pub fn new(cx: &App) -> Self {
        Self {
            collecting: tr(cx, KEY_COLLECTING),
            live: tr(cx, KEY_LIVE),
            snapshot: Template::new(cx, KEY_SNAPSHOT, &["count", "min", "sec"]),
            snapshot_sec: Template::new(cx, KEY_SNAPSHOT_SEC, &["count", "sec"]),
            snapshot_hour: Template::new(cx, KEY_SNAPSHOT_HOUR, &["count", "hours", "min"]),
            range: Template::new(cx, KEY_RANGE, &["from", "to", "duration"]),
            duration: Template::new(cx, KEY_DURATION, &["min", "sec"]),
            duration_sec: Template::new(cx, KEY_DURATION_SEC, &["sec"]),
            duration_hour: Template::new(cx, KEY_DURATION_HOUR, &["hours", "min"]),
            loading: tr(cx, KEY_LOADING),
            now: tr(cx, KEY_NOW),
        }
    }
}

/// Writes the header line for `view` and `s` to `out`. Does not clear `out` and allocates only
/// when `out` is short of room.
///
/// The scrub age counts from the newest retained frame (`span.1`), the latest second the view
/// knows. Range times are local wall-clock time (`t + realtime_offset_ns`).
pub fn header_text(
    w: &HistoryWords,
    view: Option<&HistoryView>,
    s: &HistoryUiState,
    out: &mut String,
) {
    let Some(view) = view.filter(|v| v.frame_t.is_some() || v.span.is_some()) else {
        out.push_str(&w.collecting);
        return;
    };
    if let Some((from, to)) = s.range {
        let secs = to.saturating_sub(from) / NS_PER_SEC;
        w.range.write_each(out, |i, out| match i {
            0 => clock(out, from, view.realtime_offset_ns),
            1 => clock(out, to, view.realtime_offset_ns),
            _ => duration(w, out, secs),
        });
        return;
    }
    let Some(scrub) = s.scrub else {
        out.push_str(&w.live);
        return;
    };
    let now = view.span.map_or(scrub, |(_, newest)| newest);
    let age = now.saturating_sub(scrub) / NS_PER_SEC;
    let count = u64::from(view.process_count);
    let (t, parts) = if age < 60 {
        (&w.snapshot_sec, [age, 0])
    } else if age < 3600 {
        (&w.snapshot, [age / 60, age % 60])
    } else {
        (&w.snapshot_hour, [age / 3600, age % 3600 / 60])
    };
    t.write_each(out, |i, out| match i {
        0 => units::count(out, count),
        _ => push_u64(out, parts[i - 1]),
    });
}

/// "M min S sec", "S sec" under a minute, "H h M min" from an hour on.
fn duration(w: &HistoryWords, out: &mut String, secs: u64) {
    let (t, parts) = if secs < 60 {
        (&w.duration_sec, [secs, 0])
    } else if secs < 3600 {
        (&w.duration, [secs / 60, secs % 60])
    } else {
        (&w.duration_hour, [secs / 3600, secs % 3600 / 60])
    };
    t.write_each(out, |i, out| push_u64(out, parts[i]));
}

fn push_u64(out: &mut String, n: u64) {
    use std::fmt::Write as _;
    let _ = write!(out, "{n}");
}

/// Local "HH:MM:SS" of the boot-clock time `t`, with the Start time column formatter.
fn clock(out: &mut String, t: TimeNs, realtime_offset_ns: i64) {
    let wall_ns = i128::from(t) + i128::from(realtime_offset_ns);
    let Ok(ts) = jiff::Timestamp::from_nanosecond(wall_ns) else {
        return;
    };
    let z = ts.to_zoned(jiff::tz::TimeZone::system());
    units::start_time(
        out,
        LocalDateTime {
            year: i32::from(z.year()),
            month: z.month() as u8,
            day: z.day() as u8,
            hour: z.hour() as u8,
            minute: z.minute() as u8,
            second: z.second() as u8,
        },
        true,
    );
}

/// The range a Frame query asks for: `Span` while a range is set, `At` while scrubbed, else `Live`.
pub fn query_range(s: &HistoryUiState) -> HistoryRange {
    match (s.range, s.scrub) {
        (Some((from, to)), _) => HistoryRange::Span { from, to },
        (None, Some(t)) => HistoryRange::At(t),
        (None, None) => HistoryRange::Live,
    }
}

/// The Frame payload for `s`: its metric, strip window and first 8 selected keys.
pub fn frame_what(s: &HistoryUiState, overview: bool) -> HistoryWhat {
    HistoryWhat::Frame {
        metric: s.metric,
        view: s.view_range,
        keys: Arc::from(&s.selected[..s.selected.len().min(MAX_SERIES)]),
        overview,
    }
}

/// Keeps at most one Frame query in flight. A query wanted while one is in flight waits in
/// `pending`, and a newer one replaces it.
pub(crate) struct QueryController {
    next_id: u64,
    /// Id of the Frame query sent and not yet answered.
    in_flight: Option<u64>,
    /// Latest wanted query while one is in flight, or one `send` refused with `Busy`.
    pending: Option<(HistoryRange, HistoryWhat)>,
    last_overview_ns: Option<TimeNs>,
    /// The query in flight, resent when its answer is late and nothing newer is pending.
    last_sent: Option<(HistoryRange, HistoryWhat)>,
}

impl QueryController {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            in_flight: None,
            pending: None,
            last_overview_ns: None,
            last_sent: None,
        }
    }

    /// True for the first query and then once 60 s have passed since the last overview, and
    /// records `now_ns` when true. `now_ns` is any monotonic nanosecond clock.
    pub fn take_overview(&mut self, now_ns: TimeNs) -> bool {
        let due = self
            .last_overview_ns
            .is_none_or(|last| now_ns.saturating_sub(last) >= OVERVIEW_EVERY_NS);
        if due {
            self.last_overview_ns = Some(now_ns);
        }
        due
    }

    /// Makes the next query ask for the overview.
    pub fn reset_overview(&mut self) {
        self.last_overview_ns = None;
    }

    /// Sends at once when nothing is in flight, else replaces `pending`. A replaced query's
    /// overview request carries over, so it is not lost.
    pub fn want(&mut self, range: HistoryRange, mut what: HistoryWhat, client: &dyn EngineClient) {
        if let Some((_, HistoryWhat::Frame { overview: true, .. })) = &self.pending
            && let HistoryWhat::Frame { overview, .. } = &mut what
        {
            *overview = true;
        }
        if self.in_flight.is_some() {
            self.pending = Some((range, what));
        } else {
            self.send(range, what, client);
        }
    }

    /// Clears `in_flight` when `view` answers it, then sends `pending` if any. Other answers are
    /// ignored.
    pub fn on_view(&mut self, view: &HistoryView, client: &dyn EngineClient) {
        if self.in_flight != Some(view.query_id) {
            return;
        }
        self.in_flight = None;
        self.last_sent = None;
        self.retry(client);
    }

    /// Gives up on the query in flight. The pending query, or else the late one, waits to be
    /// sent again.
    pub fn expire(&mut self) {
        if self.in_flight.take().is_some() && self.pending.is_none() {
            self.pending = self.last_sent.take();
        }
    }

    /// Sends `pending` when nothing is in flight.
    pub fn retry(&mut self, client: &dyn EngineClient) {
        if self.in_flight.is_none()
            && let Some((range, what)) = self.pending.take()
        {
            self.send(range, what, client);
        }
    }

    pub fn in_flight(&self) -> Option<u64> {
        self.in_flight
    }

    pub fn pending(&self) -> Option<&(HistoryRange, HistoryWhat)> {
        self.pending.as_ref()
    }

    /// The id the next sent query gets.
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// `Busy` keeps the query pending for the next tick. `EngineStopped` drops it.
    fn send(&mut self, range: HistoryRange, what: HistoryWhat, client: &dyn EngineClient) {
        let id = self.next_id;
        let cmd = Command::HistoryQuery {
            id,
            range,
            what: what.clone(),
        };
        match client.send(cmd) {
            Ok(()) => {
                self.next_id += 1;
                self.in_flight = Some(id);
                self.pending = None;
                self.last_sent = Some((range, what));
            }
            Err(CommandError::Busy) => self.pending = Some((range, what)),
            Err(CommandError::EngineStopped) => {}
        }
    }
}

/// True while History mode is on. Without settings (tests) it counts as on.
fn history_mode(cx: &App) -> bool {
    cx.try_global::<TlSettings>()
        .is_none_or(|_| TlSettings::state(cx).processes.history_mode)
}

/// The History mode page. Built when History mode turns on.
pub struct HistoryPage {
    pub(crate) state: HistoryUiState,
    /// The last installed `Snapshot.history`.
    pub(crate) view: Option<Arc<HistoryView>>,
    words: HistoryWords,
    /// The shown header text, and a reused buffer for the next one.
    header: SharedString,
    scratch: String,
    strings_version: u64,
    queries: QueryController,
    client: Arc<dyn EngineClient>,
    /// Zero of the clock the overview interval counts on.
    epoch: Instant,
    /// When the query in flight was sent.
    sent_at: Option<Instant>,
    /// The 1 s live query loop. `None` while scrubbed or a range is set.
    live_timer: Option<Task<()>>,
    /// Timeout and `Busy` retry while not live. The live loop does both while live.
    retry_timer: Option<Task<()>>,
    table: Entity<TlTreeTable<HistoryTableDelegate>>,
    /// `TlSettings` versions of [`SECTIONS`] last applied.
    section_versions: [u64; 3],
    /// The strip's built geometry and shaped labels, filled by `HistoryScrubber` in paint.
    scrubber: ScrubberCache,
    /// The strip's own view, created on the first render that shows it.
    strip_view: Option<Entity<StripView>>,
    /// Focus of the strip: the History key context is reached through it (E30-018).
    strip_focus: FocusHandle,
    /// The last overview the engine sent. Views that asked for none carry none, so it is kept.
    overview: Option<KeptOverview>,
    /// The minimap's built path and pointer state.
    minimap: MinimapCache,
    /// The minimap's own view, created on the first render that shows it.
    minimap_view: Option<Entity<MinimapView>>,
    /// The top-3 hover card's TopAt queries and timers (E30-023).
    top: hover::TopController,
    /// The hover card's texts and shaped lines.
    card: hover::CardCache,
    /// Counts installed views other than TopAt answers. The strip rebuilds when it changes.
    frame_version: u64,
    /// Created in `new`, never in render.
    _subs: SmallVec<[Subscription; 5]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
}

impl HistoryPage {
    /// Reads the hub's current history view, builds the table, subscribes to the hub, sends the
    /// mode entry query and starts the live loop. Needs `GlobalHub`.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hub = cx.global::<GlobalHub>().0.clone();
        let (history, processes, client) = {
            let hub = hub.read(cx);
            let s = hub.current();
            (
                Arc::clone(&s.history),
                Arc::clone(&s.processes),
                Arc::clone(hub.client()),
            )
        };
        let (settings, state) = match cx.try_global::<TlSettings>() {
            Some(_) => (
                TlSettings::settings(cx).clone(),
                TlSettings::state(cx).clone(),
            ),
            None => (Settings::default(), UiState::default()),
        };
        let mut delegate = HistoryTableDelegate::new(&settings, &state, cx);
        delegate.install(&history, &processes);
        let defaults = delegate.default_layout();
        let table = cx.new(|cx| {
            let mut t = TlTreeTable::new(delegate, window, cx);
            t.set_default_layout(defaults);
            t.set_resize_mode(settings.tables.column_resize);
            t.set_column_separators(settings.tables.column_separators);
            t
        });
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, |this, _, ev: &SnapshotChanged, cx| {
            if ev.mask.contains(ComponentMask::HISTORY) {
                let history = Arc::clone(&cx.global::<GlobalHub>().0.read(cx).current().history);
                // A TopAt answer changes only `top`: the card repaints, nothing else.
                if this
                    .view
                    .as_ref()
                    .is_some_and(|old| hover::top_only(old, &history))
                {
                    this.view = Some(Arc::clone(&history));
                    this.on_top_view(&history, cx);
                    return;
                }
                this.frame_version += 1;
                this.install(Arc::clone(&history));
                this.install_table(cx);
                this.drive(cx, |q, c| q.on_view(&history, c));
                this.on_top_view(&history, cx);
                cx.notify();
            }
        }));
        subs.push(cx.observe_global::<Strings>(|this, cx| this.on_strings(cx)));
        subs.push(cx.observe_global::<TlSettings>(|this, cx| this.on_settings(cx)));
        subs.push(cx.observe(&table, |this, _, cx| this.sync_selection(cx)));
        subs.push(cx.subscribe_in(
            &table,
            window,
            |this, _, ev: &TreeTableEvent, window, cx| match ev {
                TreeTableEvent::Activate { row_index } => {
                    let key = this.table.read(cx).delegate.row_id(*row_index);
                    this.jump(key, window, cx);
                }
                TreeTableEvent::CopiedRows { .. } => {
                    let text = tr(cx, "processes-toast-copied");
                    push_main_toast(cx, ToastKind::Success, text, None, None);
                }
                TreeTableEvent::CopyFailed => {}
            },
        ));
        if let Some(events) = cx.try_global::<GlobalShellEvents>().map(|g| g.0.clone()) {
            subs.push(cx.subscribe(&events, |this, _, ev: &FilterChanged, cx| {
                if ev.tab == Tab::Processes {
                    this.set_filter(&ev.text, cx);
                }
            }));
        }
        // A filter typed before History mode started sent its FilterChanged before this page
        // listened, so take the box's text once. Deferred: the main window may be mid-update.
        cx.defer_in(window, |this, _, cx| {
            let text = main_window_view(cx).map(|main| {
                main.read(cx)
                    .filter_box()
                    .read(cx)
                    .text(Tab::Processes)
                    .clone()
            });
            if let Some(text) = text.filter(|t| !t.is_empty()) {
                this.set_filter(&text, cx);
            }
        });
        let section_versions = cx
            .try_global::<TlSettings>()
            .map_or([0; 3], |tl| SECTIONS.map(|s| tl.version(s)));
        let (words, strings_version) = match cx.try_global::<Strings>() {
            Some(s) => (HistoryWords::new(cx), s.strings_version),
            None => (HistoryWords::default(), 0),
        };
        let mut page = Self {
            state: HistoryUiState::default(),
            view: None,
            words,
            header: SharedString::default(),
            scratch: String::with_capacity(96),
            strings_version,
            queries: QueryController::new(),
            client,
            epoch: cx.background_executor().now(),
            sent_at: None,
            live_timer: None,
            retry_timer: None,
            table,
            section_versions,
            scrubber: ScrubberCache::default(),
            strip_view: None,
            strip_focus: cx.focus_handle().tab_stop(true),
            overview: None,
            minimap: MinimapCache::default(),
            minimap_view: None,
            top: hover::TopController::default(),
            card: hover::CardCache::default(),
            frame_version: 0,
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
        };
        page.install(history);
        page.request(cx);
        page.update_live_timer(cx);
        page
    }

    pub fn state(&self) -> &HistoryUiState {
        &self.state
    }

    /// The strip's focus handle.
    pub fn strip_focus(&self) -> &FocusHandle {
        &self.strip_focus
    }

    /// The header line text as shown.
    pub fn header(&self) -> &SharedString {
        &self.header
    }

    pub fn table(&self) -> &Entity<TlTreeTable<HistoryTableDelegate>> {
        &self.table
    }

    /// The strip's cache, for its build counter and painted gap bands.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn scrubber(&self) -> &ScrubberCache {
        &self.scrubber
    }

    /// The minimap's cache, for its build counter and painted box.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn minimap(&self) -> &MinimapCache {
        &self.minimap
    }

    /// Sets the global filter text (PROC-046). The table matches it, lowercased, against name,
    /// comm, user and executable path.
    pub fn set_filter(&mut self, text: &str, cx: &mut Context<Self>) {
        let lower: Arc<str> = Arc::from(text.to_lowercase());
        self.table.update(cx, |t, cx| {
            if t.delegate.set_filter(lower) {
                t.on_new_version(cx);
            }
        });
    }

    /// Sets the scrub time (`None` is live) and asks for its frame.
    pub fn set_scrub(&mut self, scrub: Option<TimeNs>, cx: &mut Context<Self>) {
        if self.state.scrub == scrub {
            return;
        }
        self.state.scrub = scrub;
        self.on_query_state(cx);
    }

    /// Steps the scrub point by `delta_s` seconds. Needs a span.
    fn step(&mut self, delta_s: i64, cx: &mut Context<Self>) {
        let Some(span) = self.view.as_ref().and_then(|v| v.span) else {
            return;
        };
        self.set_scrub(step_scrub(self.state.scrub, span, delta_s), cx);
    }

    fn on_step_back(&mut self, _: &HistoryStepBack, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    fn on_step_forward(&mut self, _: &HistoryStepForward, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn on_step_back_minute(
        &mut self,
        _: &HistoryStepBackMinute,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step(-60, cx);
    }

    fn on_step_forward_minute(
        &mut self,
        _: &HistoryStepForwardMinute,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step(60, cx);
    }

    fn on_oldest(&mut self, _: &HistoryOldest, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(span) = self.view.as_ref().and_then(|v| v.span) {
            self.set_scrub(Some(span.0), cx);
        }
    }

    /// Clears the scrub point and the range together, so one query goes out and not two.
    fn on_live(&mut self, _: &HistoryLive, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_live() {
            return;
        }
        self.state.scrub = None;
        self.state.range = None;
        self.on_query_state(cx);
    }

    fn on_scrub(&mut self, a: &HistoryScrub, _: &mut Window, cx: &mut Context<Self>) {
        self.set_scrub(Some(a.t), cx);
    }

    fn on_set_range(&mut self, a: &HistorySetRange, _: &mut Window, cx: &mut Context<Self>) {
        self.set_range(Some((a.from, a.to)), cx);
    }

    fn on_metric(&mut self, metric: HistoryMetric, cx: &mut Context<Self>) {
        self.set_metric(metric, cx);
    }

    /// Sets the selected range (`None` clears it) and asks for its frame.
    pub fn set_range(&mut self, range: Option<(TimeNs, TimeNs)>, cx: &mut Context<Self>) {
        if self.state.range == range {
            return;
        }
        self.state.range = range;
        self.on_query_state(cx);
    }

    /// Sets the strip window. While scrubbed or with a range set, asks for its frame.
    pub fn set_view_range(&mut self, view_range: (TimeNs, TimeNs), cx: &mut Context<Self>) {
        if self.state.view_range == view_range {
            return;
        }
        self.state.view_range = view_range;
        self.on_frame_input(true, cx);
    }

    /// Sets the strip metric. While scrubbed or with a range set, asks for its frame. While the
    /// top-3 card is shown, asks for the hovered second's top list for the new metric. The next
    /// Frame query asks for the new metric's overview.
    pub fn set_metric(&mut self, metric: HistoryMetric, cx: &mut Context<Self>) {
        if self.state.metric == metric {
            return;
        }
        self.state.metric = metric;
        self.overview = None;
        self.queries.reset_overview();
        self.on_frame_input(true, cx);
        if let Some((t, _)) = self.top.wanted() {
            self.top_hover(t, cx);
        }
    }

    /// Sets the selected keys in selection order and selects them in the table. While scrubbed
    /// or with a range set, asks for the frame when the first 8, the keys the strip draws series
    /// for, changed.
    pub fn set_selected(&mut self, selected: Vec<ProcessKey>, cx: &mut Context<Self>) {
        if self.state.selected == selected {
            return;
        }
        self.table.update(cx, |t, cx| {
            t.selection.clear();
            for &key in &selected {
                t.selection.ctrl_click(key);
            }
            cx.notify();
        });
        self.apply_selected(selected, cx);
    }

    /// `set_selected` without touching the table, for a change that came from the table.
    fn apply_selected(&mut self, selected: Vec<ProcessKey>, cx: &mut Context<Self>) {
        let series = |k: &[ProcessKey]| k.len().min(MAX_SERIES);
        let old = &self.state.selected;
        let ask = old[..series(old)] != selected[..series(&selected)];
        self.state.selected = selected;
        self.on_frame_input(ask, cx);
    }

    /// Id of the Frame query in flight.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn query_in_flight(&self) -> Option<u64> {
        self.queries.in_flight()
    }

    /// The query waiting for the one in flight, or for a `Busy` retry.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn query_pending(&self) -> Option<(HistoryRange, HistoryWhat)> {
        self.queries.pending().cloned()
    }

    fn is_live(&self) -> bool {
        self.state.scrub.is_none() && self.state.range.is_none()
    }

    /// A Frame input other than scrub and range changed. While live the next tick asks for it,
    /// so only a scrubbed or ranged page sends a query now.
    fn on_frame_input(&mut self, ask: bool, cx: &mut Context<Self>) {
        if ask && !self.is_live() {
            self.request(cx);
        }
        cx.notify();
    }

    fn on_query_state(&mut self, cx: &mut Context<Self>) {
        self.refresh_header();
        self.request(cx);
        self.update_live_timer(cx);
        cx.notify();
    }

    /// Wants a Frame query for the current state.
    fn request(&mut self, cx: &mut Context<Self>) {
        let elapsed = cx
            .background_executor()
            .now()
            .saturating_duration_since(self.epoch);
        let now_ns = TimeNs::try_from(elapsed.as_nanos()).unwrap_or(TimeNs::MAX);
        let overview = self.queries.take_overview(now_ns);
        let range = query_range(&self.state);
        let what = frame_what(&self.state, overview);
        self.drive(cx, |q, c| q.want(range, what, c));
    }

    /// Runs `f` on the controller, notes when a query went out and re-arms the retry timer.
    fn drive(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut QueryController, &dyn EngineClient),
    ) {
        let next_id = self.queries.next_id();
        f(&mut self.queries, &*self.client);
        if self.queries.next_id() != next_id {
            self.sent_at = Some(cx.background_executor().now());
        }
        self.arm_retry(cx);
    }

    /// While not live and a query is in flight or pending, wakes once its timeout ends or after
    /// one tick for a `Busy` retry.
    fn arm_retry(&mut self, cx: &mut Context<Self>) {
        let wait = match (self.queries.in_flight(), self.sent_at) {
            _ if self.is_live() => None,
            (Some(_), Some(sent)) => {
                let age = cx
                    .background_executor()
                    .now()
                    .saturating_duration_since(sent);
                Some(QUERY_TIMEOUT.saturating_sub(age))
            }
            _ if self.queries.pending().is_some() => Some(LIVE_TICK),
            _ => None,
        };
        self.retry_timer = wait.map(|wait| {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |page, cx| page.tick(cx));
            })
        });
    }

    /// Starts the live loop while live and History mode is on, and stops it otherwise.
    fn update_live_timer(&mut self, cx: &mut Context<Self>) {
        if !self.is_live() || !history_mode(cx) {
            self.live_timer = None;
            return;
        }
        if self.live_timer.is_some() {
            return;
        }
        self.live_timer = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(LIVE_TICK).await;
                    match this.update(cx, |page, cx| page.tick(cx)) {
                        Ok(true) => {}
                        _ => break,
                    }
                }
            }),
        );
    }

    /// One live tick or retry wake. Expires a query unanswered for 1,000 ms, then sends a Live
    /// query while live or the pending one otherwise. False once History mode is off.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if !history_mode(cx) {
            self.retry_timer = None;
            return false;
        }
        let now = cx.background_executor().now();
        if self
            .sent_at
            .is_some_and(|sent| now.saturating_duration_since(sent) >= QUERY_TIMEOUT)
        {
            self.queries.expire();
        }
        if self.is_live() {
            self.request(cx);
        } else {
            self.drive(cx, |q, c| q.retry(c));
        }
        true
    }

    /// Stores `view`, sets the default strip window on the first span, keeps the view's overview
    /// when it carries one and rebuilds the header.
    fn install(&mut self, view: Arc<HistoryView>) {
        if self.state.view_range == (0, 0)
            && let Some(span) = view.span
        {
            self.state.view_range = default_view_range(span);
        }
        if let Some(span) = view.span.filter(|_| !view.overview.is_empty()) {
            self.overview = Some(KeptOverview {
                metric: view.series.first().map_or(self.state.metric, |s| s.metric),
                span,
                points: Arc::clone(&view.overview),
            });
        }
        self.view = Some(view);
        self.refresh_header();
    }

    /// Rebuilds the header text. True when it changed. Call after changing `state`, then notify.
    pub(crate) fn refresh_header(&mut self) -> bool {
        self.scratch.clear();
        header_text(
            &self.words,
            self.view.as_deref(),
            &self.state,
            &mut self.scratch,
        );
        if self.scratch == self.header.as_ref() {
            return false;
        }
        self.header = SharedString::from(self.scratch.clone());
        true
    }

    fn on_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.words = HistoryWords::new(cx);
        self.refresh_header();
        self.table.update(cx, |t, cx| {
            t.delegate.refresh_strings(cx);
            t.on_new_version(cx);
        });
        cx.notify();
    }

    /// Hands the installed view and the live process list (for app icons) to the table.
    fn install_table(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.view.clone() else {
            return;
        };
        let processes = Arc::clone(&cx.global::<GlobalHub>().0.read(cx).current().processes);
        self.table.update(cx, |t, cx| {
            if t.delegate.install(&view, &processes) {
                t.on_new_version(cx);
            }
        });
    }

    /// `processes.hide_system` filters the rows; a `units` change restarts the cell text caches;
    /// `tables` sets resize mode and separators.
    fn on_settings(&mut self, cx: &mut Context<Self>) {
        let Some(tl) = cx.try_global::<TlSettings>() else {
            return;
        };
        let versions = SECTIONS.map(|s| tl.version(s));
        if versions == self.section_versions {
            return;
        }
        let old = std::mem::replace(&mut self.section_versions, versions);
        let restyle = versions[1] != old[1];
        let settings = TlSettings::settings(cx).clone();
        self.table.update(cx, |t, cx| {
            let rows = t.delegate.set_settings(&settings);
            t.set_resize_mode(settings.tables.column_resize);
            t.set_column_separators(settings.tables.column_separators);
            if restyle {
                bump_style_version(cx);
            }
            if rows {
                t.on_new_version(cx);
            } else {
                cx.notify();
            }
        });
    }

    /// Copies the table's selection into `state.selected` through [`Self::apply_selected`]: ids
    /// no longer selected leave, new ones are appended in display order, so the list keeps
    /// selection order.
    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let table = self.table.read(cx);
        let sel = &table.selection;
        let old = &self.state.selected;
        if old.len() == sel.len() && old.iter().all(|k| sel.contains(k)) {
            return;
        }
        let mut selected: Vec<ProcessKey> =
            old.iter().copied().filter(|k| sel.contains(k)).collect();
        let d = &table.delegate;
        let mut added: SmallVec<[ProcessKey; 8]> =
            sel.ids().filter(|k| !selected.contains(k)).collect();
        added.sort_by_key(|k| d.row_index(*k).unwrap_or(usize::MAX));
        selected.extend(added);
        self.apply_selected(selected, cx);
    }

    /// The rows a menu action acts on: the ones the open menu captured, else the selected rows
    /// in view order.
    fn targets(&mut self, cx: &mut Context<Self>) -> (Vec<ProcessKey>, Option<ColumnId>) {
        let captured = self.table.update(cx, |t, _| t.delegate.take_captured());
        if let Some((keys, col)) = captured {
            return (keys.into_vec(), col);
        }
        let d = &self.table.read(cx).delegate;
        let mut keys = self.state.selected.clone();
        keys.sort_by_key(|k| d.row_index(*k).unwrap_or(usize::MAX));
        (keys, None)
    }

    /// Copy, Copy details and Copy all metrics (PROC-070): the target rows onto the clipboard,
    /// then a Success toast. History rows hold no command line, so none is ever copied.
    fn copy(&mut self, what: CopyKind, cx: &mut Context<Self>) {
        let (keys, _) = self.targets(cx);
        let d = &self.table.read(cx).delegate;
        let text = match what {
            CopyKind::Cell(col) => d.copy_cell_text(&keys, col),
            CopyKind::Details => d.copy_details_text(&keys, cx),
            CopyKind::AllMetrics => d.copy_all_metrics_text(&keys),
        };
        if text.is_empty() {
            return;
        }
        // `clipboard_write` cannot fail yet (R-199), so there is no failure toast.
        if clipboard_write(cx, text).is_err() {
            return;
        }
        let many = keys.len() > 1;
        let key = match (what, many) {
            (CopyKind::Cell(_), false) => "processes-toast-copied",
            (CopyKind::Cell(_), true) => "processes-toast-values-copied",
            (CopyKind::Details, _) => "processes-toast-details-copied",
            (CopyKind::AllMetrics, false) => "processes-toast-metrics-copied",
            (CopyKind::AllMetrics, true) => "processes-toast-many-metrics-copied",
        };
        let text = tr(cx, key);
        push_main_toast(cx, ToastKind::Success, text, None, None);
    }

    fn on_copy_cell(&mut self, a: &CopyCell, _: &mut Window, cx: &mut Context<Self>) {
        self.copy(CopyKind::Cell(a.col), cx);
    }

    fn on_copy_details(&mut self, _: &CopyDetails, _: &mut Window, cx: &mut Context<Self>) {
        self.copy(CopyKind::Details, cx);
    }

    fn on_copy_all_metrics(&mut self, _: &CopyAllMetrics, _: &mut Window, cx: &mut Context<Self>) {
        self.copy(CopyKind::AllMetrics, cx);
    }

    /// Delete from history (UI-PRC-037): no confirmation. One `HistoryDelete` carries every
    /// target key; the rows leave with the next `HistoryView`. The keys leave the selection at
    /// once, so their chips go now and the strip drops their series with the next view.
    fn on_delete(&mut self, _: &DeleteFromHistory, _: &mut Window, cx: &mut Context<Self>) {
        let (keys, _) = self.targets(cx);
        if keys.is_empty() {
            return;
        }
        let command = Command::HistoryDelete {
            keys: keys.iter().copied().collect(),
        };
        if let Err(err) = self.client.send(command) {
            tracing::warn!(target: "tasklens::history", ?err, "history delete not sent");
            return;
        }
        let text = match keys.len() {
            1 => tr(cx, "history-toast-deleted"),
            n => tr_args(cx, "history-toast-deleted-n", &[("count", n.into())]),
        };
        push_main_toast(cx, ToastKind::Success, text, None, None);
        let selected = self
            .state
            .selected
            .iter()
            .copied()
            .filter(|k| !keys.contains(k))
            .collect();
        self.set_selected(selected, cx);
    }

    /// Show in live table: the first captured row that is running.
    fn on_show_live(
        &mut self,
        _: &HistoryShowInLiveTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (keys, _) = self.targets(cx);
        if let Some(key) = keys.into_iter().find(|&k| is_live(k, cx)) {
            self.jump(key, window, cx);
        }
    }

    /// Enter on a row: the row the table remembered.
    fn on_open_properties(
        &mut self,
        _: &OpenProperties,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = self.table.update(cx, |t, _| t.delegate.take_activated());
        if let Some(key) = key {
            self.jump(key, window, cx);
        }
    }

    /// Double-click and Enter: a running process is selected in the live table, which leaves
    /// History mode (UI-PRC-022); any other row shows "Process not running".
    fn jump(&mut self, key: ProcessKey, window: &mut Window, cx: &mut Context<Self>) {
        if is_live(key, cx) {
            let page = main_window_view(cx)
                .and_then(|v| v.read(cx).page_entity::<ProcessesPage>(Tab::Processes));
            if let Some(page) = page {
                page.update(cx, |p, cx| {
                    p.select_by_key(key, JumpTexts::default(), window, cx)
                });
                return;
            }
        }
        let text = tr(cx, "history-not-running");
        push_main_toast(cx, ToastKind::Warning, text, None, None);
    }

    fn loading(&self) -> bool {
        self.view
            .as_ref()
            .is_some_and(|v| v.store_state == HistoryStoreState::Loading)
    }
}

/// The scrub point after a step of `delta_s` seconds from `scrub` (live counts as the newest
/// second). `None` is live: a forward step that reaches the newest second returns to live.
/// Backward steps stop at the oldest second.
pub fn step_scrub(scrub: Option<TimeNs>, span: (TimeNs, TimeNs), delta_s: i64) -> Option<TimeNs> {
    let base = scrub.unwrap_or(span.1);
    let delta = delta_s.unsigned_abs().saturating_mul(1_000_000_000);
    if delta_s >= 0 {
        let target = base.saturating_add(delta);
        if target >= span.1 {
            return None;
        }
        Some(target.max(span.0))
    } else {
        Some(base.saturating_sub(delta).clamp(span.0, span.1))
    }
}

#[derive(Clone, Copy)]
enum CopyKind {
    Cell(ColumnId),
    Details,
    AllMetrics,
}

impl Render for HistoryPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let page = cx.entity();
        let strip_view = self
            .strip_view
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    // Every page change repaints the strip; only hover skips the page.
                    cx.observe(&page, |_, _, cx| cx.notify()).detach();
                    StripView::new(page.downgrade())
                })
            })
            .clone();
        let minimap_view = self
            .minimap_view
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    cx.observe(&page, |_, _, cx| cx.notify()).detach();
                    MinimapView::new(page.downgrade())
                })
            })
            .clone();
        let m = metrics(cx);
        let theme = current(cx);
        let scale = m.ui_scale;
        let margin = m.sp[5];
        let header = div()
            .flex()
            .items_center()
            .flex_none()
            .h(px(HEADER_H * scale))
            .px(margin)
            .text_size(m.text.base)
            .text_color(theme.text_primary)
            .child(self.header.clone());
        // E30-026 adds the 28 px recording-off banner slot here.
        let mut strip = div()
            .flex_none()
            .h(px(STRIP_H * scale))
            .mx(margin)
            .flex()
            .items_center()
            .justify_center();
        strip = strip.track_focus(&self.strip_focus);
        strip = if self.loading() {
            strip
                .text_size(m.text.base)
                .text_color(theme.text_secondary)
                .child(self.words.loading.clone())
        } else {
            let view = strip_view.clone();
            strip.child(view.cached(StyleRefinement::default().size_full()))
        };
        let mut minimap = div()
            .flex_none()
            .h(px(MINIMAP_H * scale))
            .mx(margin)
            .mt(m.sp[2]);
        if !self.loading() {
            minimap = minimap.child(minimap_view.cached(StyleRefinement::default().size_full()));
        }
        // The history table fills this slot; it stays empty while the store loads.
        let mut table = div().flex_1().min_h_0().mt(m.sp[4]);
        if !self.loading() {
            table = table.child(
                self.table
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            );
        }
        let mut key_context = KeyContext::default();
        key_context.add(HISTORY_CONTEXT);
        if self.state.scrub.is_some() {
            key_context.add(SCRUBBED);
        }
        div()
            .key_context(key_context)
            .on_action(cx.listener(Self::on_step_back))
            .on_action(cx.listener(Self::on_step_forward))
            .on_action(cx.listener(Self::on_step_back_minute))
            .on_action(cx.listener(Self::on_step_forward_minute))
            .on_action(cx.listener(Self::on_oldest))
            .on_action(cx.listener(Self::on_live))
            .on_action(cx.listener(Self::on_scrub))
            .on_action(cx.listener(Self::on_set_range))
            .on_action(cx.listener(Self::on_copy_cell))
            .on_action(cx.listener(Self::on_copy_details))
            .on_action(cx.listener(Self::on_copy_all_metrics))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_show_live))
            .on_action(cx.listener(Self::on_open_properties))
            .on_action(cx.listener(|this, _: &HistoryMetricCpu, _, cx| {
                this.on_metric(HistoryMetric::Cpu, cx)
            }))
            .on_action(cx.listener(|this, _: &HistoryMetricMemory, _, cx| {
                this.on_metric(HistoryMetric::Memory, cx)
            }))
            .on_action(cx.listener(|this, _: &HistoryMetricDisk, _, cx| {
                this.on_metric(HistoryMetric::Disk, cx)
            }))
            .on_action(cx.listener(|this, _: &HistoryMetricNetwork, _, cx| {
                this.on_metric(HistoryMetric::Network, cx)
            }))
            .on_action(cx.listener(|this, _: &HistoryMetricGpu, _, cx| {
                this.on_metric(HistoryMetric::Gpu, cx)
            }))
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(strip)
            .child(minimap)
            .child(table)
    }
}
