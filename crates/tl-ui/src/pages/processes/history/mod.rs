//! History mode page: header line, layout slots and the state the strip, minimap, chips and
//! table share (UI-PRC-030, PROC-088, PROC-091).
//!
//! `HistoryPage` replaces the live process table while `history_mode` is on. It subscribes to
//! the hub in `new` and notifies only when a change carries `ComponentMask::HISTORY` (ADR-005).
//! The header text is rebuilt outside render, into a reused `String`, and a new `SharedString`
//! is made only when the text changes, so the header re-shapes only then. Nothing here notifies
//! inside `render`.

use std::sync::Arc;

use gpui_kit::{
    App, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    Window, div, px,
};
use smallvec::SmallVec;
use tl_model::history::{HistoryMetric, HistoryStoreState, HistoryView};
use tl_model::ids::ProcessKey;
use tl_model::time::TimeNs;
use tl_model::units::{self, LocalDateTime};

use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::{Strings, tr, tr_en};
use crate::pages::performance::details::Template;
use crate::theme::{current, metrics};

const NS_PER_SEC: u64 = 1_000_000_000;

/// The strip window shown by default: the last 10 minutes, or the whole span when shorter.
pub const DEFAULT_VIEW_NS: u64 = 600 * NS_PER_SEC;

/// Layout heights in logical px at 100% scale (UI-PRC-030).
const HEADER_H: f32 = 32.0;
const STRIP_H: f32 = 120.0;
const MINIMAP_H: f32 = 24.0;

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
    /// Created in `new`, never in render.
    _subs: SmallVec<[Subscription; 2]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
}

impl HistoryPage {
    /// Reads the hub's current history view and subscribes to the hub. Needs `GlobalHub`.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let hub = cx.global::<GlobalHub>().0.clone();
        let history = Arc::clone(&hub.read(cx).current().history);
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, |this, _, ev: &SnapshotChanged, cx| {
            if ev.mask.contains(ComponentMask::HISTORY) {
                let history = Arc::clone(&cx.global::<GlobalHub>().0.read(cx).current().history);
                this.install(history);
                cx.notify();
            }
        }));
        subs.push(cx.observe_global::<Strings>(|this, cx| this.on_strings(cx)));
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
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
        };
        page.install(history);
        page
    }

    pub fn state(&self) -> &HistoryUiState {
        &self.state
    }

    /// The header line text as shown.
    pub fn header(&self) -> &SharedString {
        &self.header
    }

    /// Stores `view`, sets the default strip window on the first span and rebuilds the header.
    fn install(&mut self, view: Arc<HistoryView>) {
        if self.state.view_range == (0, 0)
            && let Some(span) = view.span
        {
            self.state.view_range = default_view_range(span);
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
        cx.notify();
    }

    fn loading(&self) -> bool {
        self.view
            .as_ref()
            .is_some_and(|v| v.store_state == HistoryStoreState::Loading)
    }
}

impl Render for HistoryPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
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
        if self.loading() {
            strip = strip
                .text_size(m.text.base)
                .text_color(theme.text_secondary)
                .child(self.words.loading.clone());
        }
        let minimap = div()
            .flex_none()
            .h(px(MINIMAP_H * scale))
            .mx(margin)
            .mt(m.sp[2]);
        // The history table (E30-019) fills this slot; it stays empty while the store loads.
        let table = div().flex_1().min_h_0().mt(m.sp[4]);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(strip)
            .child(minimap)
            .child(table)
    }
}
