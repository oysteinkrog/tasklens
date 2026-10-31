//! "tasklens Collector Health": one row per engine source with its state, data age, restart
//! count and last error (UI-SHL-099, APP-026, APP-029, R-071, ADR-003, ADR-004).
//!
//! A Tool window keyed `WindowKey::CollectorHealth`; `OpenCollectorHealth` focuses it when open.
//! The rows are rebuilt on a HEALTH change only, and a 1 s timer that lives with the window
//! moves the ages. A cell's text is formatted only when its value changes, so a frame formats
//! nothing and makes no `tr` call (R-132). Esc and Ctrl+W close it through the frame.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, AppContext as _, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription, Task, Window, div,
    px, size,
};
use tl_model::health::{HealthSnapshot, SourceHealth, SourceId, SourceState};
use tl_model::time::TimeNs;
use tl_model::units::{count, short_age};

use super::registry::{GlobalRegistry, WindowKey};
use crate::actions::OpenCollectorHealth;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::host::HostContext;
use crate::hub::{ComponentMask, HostClock, SnapshotChanged};
use crate::i18n::{Strings, tr};
use crate::pages::performance::details::Template;
use crate::text::style::{numeric_style, t_sm};
use crate::text::untrusted::debug_assert_clean;
use crate::theme::{TlTheme, current, metrics};

/// Window size, × ui_scale.
const SIZE: (f32, f32) = (720., 360.);
/// Column widths, × ui_scale. Last error fills the rest.
const COL_SOURCE: f32 = 140.;
const COL_STATE: f32 = 100.;
const COL_AGE: f32 = 100.;
const COL_RESTARTS: f32 = 80.;
/// How often the ages move.
const TICK: Duration = Duration::from_secs(1);
/// Last data of a source that has sent nothing yet: the WarmingUp glyph (ADR-003).
const WARMING_UP: &str = "\u{2026}";

const TITLE: &str = "shell-health-title";
const HEADINGS: [&str; 5] = [
    "shell-health-col-source",
    "shell-health-col-state",
    "shell-health-col-last-data",
    "shell-health-col-restarts",
    "shell-health-col-last-error",
];
/// The words of the shown states, in `StateLook` order after Off.
const STATE_KEYS: [&str; STATE_COUNT - 1] = [
    "shell-health-state-running",
    "shell-health-state-late",
    "shell-health-state-stale",
    "shell-health-state-failed",
    "shell-health-state-disabled",
    "shell-health-state-unsupported",
];
const AGO: &str = "shell-health-ago";

/// Source name keys in `SourceId` order (UI-SHL-051, UI-SHL-099).
const SOURCE_KEYS: [&str; SourceId::COUNT] = [
    "shell-health-source-cpu",
    "shell-health-source-memory",
    "shell-health-source-disk",
    "shell-health-source-net",
    "shell-health-source-gpu-nvidia",
    "shell-health-source-gpu-drm",
    "shell-health-source-sensors",
    "shell-health-source-processes",
    "shell-health-source-process-detail",
    "shell-health-source-connections",
    "shell-health-source-systemd-system",
    "shell-health-source-systemd-user",
    "shell-health-source-logind",
    "shell-health-source-startup",
    "shell-health-source-inventory",
    "shell-health-source-kwin",
    "shell-health-source-udisks",
    "shell-health-source-upower",
    "shell-health-source-network-manager",
    "shell-health-source-history",
    "shell-health-source-journal",
    "shell-health-source-device-history",
];

const STATE_COUNT: usize = 7;

/// The State column: the word and the colour role. An Off source (not demanded) has no row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StateLook {
    Off,
    Running,
    Late,
    Stale,
    Failed,
    Disabled,
    Unsupported,
}

impl StateLook {
    pub fn of(state: SourceState) -> Self {
        match state {
            SourceState::Off => Self::Off,
            SourceState::Running => Self::Running,
            SourceState::Late => Self::Late,
            SourceState::Stale => Self::Stale,
            SourceState::Failed => Self::Failed,
            SourceState::Disabled { .. } => Self::Disabled,
            SourceState::Unsupported => Self::Unsupported,
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    /// The text colour of the state word.
    pub fn color(self, theme: &TlTheme) -> Hsla {
        match self {
            Self::Running => theme.status.success,
            Self::Late | Self::Stale | Self::Disabled => theme.status.warning,
            Self::Failed => theme.status.danger,
            Self::Off | Self::Unsupported => theme.text_secondary,
        }
    }
}

/// Registers the `OpenCollectorHealth` handler. `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
    // Deferred: from a menu or the status chip the main window is mid-update.
    cx.on_action(|_: &OpenCollectorHealth, cx| {
        cx.defer(|cx| {
            // App-level: the window shows the host the main window shows now.
            let host = HostContext::shown(cx);
            open_collector_health(&host, cx);
        })
    });
}

/// Focuses the open window, or opens it on `host`. Without a window registry (tests) it always
/// opens one.
pub fn open_collector_health(host: &HostContext, cx: &mut App) {
    let host = host.clone();
    let s = metrics(cx).ui_scale;
    let spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        tr(cx, TITLE),
        size(px(SIZE.0 * s), px(SIZE.1 * s)),
    )
    .key(WindowKey::CollectorHealth);
    let open = move |cx: &mut App| {
        TlSecondaryWindow::open(cx, spec, |_, cx| {
            cx.new(|cx| CollectorHealthWindow::new(&host, cx))
        })
        .map(Into::into)
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            registry.update(cx, |r, cx| {
                r.open_or_focus(WindowKey::CollectorHealth, cx, open)
            });
        }
        None => {
            let _ = open(cx);
        }
    }
}

/// The window's own text, translated once per strings version (R-132).
struct HealthWords {
    headings: [SharedString; 5],
    states: [SharedString; STATE_COUNT],
    sources: [SharedString; SourceId::COUNT],
    /// "{ $age } ago".
    ago: Template,
}

impl HealthWords {
    fn new(cx: &App) -> Self {
        Self {
            headings: HEADINGS.map(|k| tr(cx, k)),
            states: std::array::from_fn(|i| match i {
                0 => SharedString::default(),
                i => tr(cx, STATE_KEYS[i - 1]),
            }),
            sources: SOURCE_KEYS.map(|k| tr(cx, k)),
            ago: Template::new(cx, AGO, &["age"]),
        }
    }
}

fn strings_version(cx: &App) -> u64 {
    cx.try_global::<Strings>().map_or(0, |s| s.strings_version)
}

/// One source row. Each text is rebuilt only when its value changes.
#[derive(Debug)]
pub struct HealthRow {
    pub source: SourceId,
    pub state: StateLook,
    /// CLOCK_BOOTTIME of the last fresh data; `None` until the source sends any.
    last_ok_ns: Option<TimeNs>,
    /// Whole seconds shown in `age`.
    age_secs: Option<u64>,
    /// "3s ago", or the WarmingUp glyph.
    pub age: SharedString,
    restarts: u32,
    pub restarts_text: SharedString,
    pub error: Option<SharedString>,
    error_src: Option<Arc<str>>,
}

impl HealthRow {
    fn new(source: SourceId) -> Self {
        Self {
            source,
            state: StateLook::Off,
            last_ok_ns: None,
            age_secs: None,
            age: WARMING_UP.into(),
            restarts: 0,
            restarts_text: "0".into(),
            error: None,
            error_src: None,
        }
    }

    /// Takes the source's health. Returns whether anything shown changed.
    fn set_health(
        &mut self,
        h: &SourceHealth,
        now: TimeNs,
        ago: &Template,
        buf: &mut String,
    ) -> bool {
        let mut changed = false;
        let state = StateLook::of(h.state);
        if state != self.state {
            self.state = state;
            changed = true;
        }
        if h.restarts != self.restarts {
            self.restarts = h.restarts;
            buf.clear();
            count(buf, u64::from(h.restarts));
            self.restarts_text = SharedString::from(buf.clone());
            changed = true;
        }
        let same_error = match (&h.last_error, &self.error_src) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            (None, None) => true,
            _ => false,
        };
        if !same_error {
            self.error = h.last_error.as_ref().map(|e| {
                debug_assert_clean(e);
                SharedString::from(e.to_string())
            });
            self.error_src = h.last_error.clone();
            changed = true;
        }
        self.last_ok_ns = h.last_ok_ns;
        self.set_age(now, ago, buf) || changed
    }

    /// Moves the age to `now`. Returns whether the text changed.
    fn set_age(&mut self, now: TimeNs, ago: &Template, buf: &mut String) -> bool {
        let secs = self
            .last_ok_ns
            .map(|t| now.saturating_sub(t) / 1_000_000_000);
        if secs == self.age_secs {
            return false;
        }
        self.age_secs = secs;
        self.age = match secs {
            Some(secs) => {
                buf.clear();
                ago.write_each(buf, |_, out| short_age(out, secs));
                SharedString::from(buf.clone())
            }
            None => WARMING_UP.into(),
        };
        true
    }

    fn warming_up(&self) -> bool {
        self.age_secs.is_none()
    }
}

/// The content of the Collector Health window.
pub struct CollectorHealthWindow {
    /// The host this window shows. Set when it opens; never changes.
    host: HostContext,
    rows: Vec<HealthRow>,
    words: HealthWords,
    strings_version: u64,
    /// `HealthSnapshot::version` the rows show.
    health_version: Option<u64>,
    /// The host's clock. Read at every age, so a later offset (the first published snapshot,
    /// a host reboot, a reconnect) moves the ages too.
    host_clock: HostClock,
    /// This PC's CLOCK_BOOTTIME and the executor time at open; now is the host's time at this
    /// PC's anchor plus the executor time passed, so the test clock moves the ages.
    local_anchor: TimeNs,
    clock_anchor: Instant,
    buf: String,
    _strings: Subscription,
    _hub_sub: Option<Subscription>,
    _tick: Task<()>,
}

impl CollectorHealthWindow {
    fn new(host: &HostContext, cx: &mut Context<Self>) -> Self {
        let strings = cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = strings_version(cx);
            if version != this.strings_version {
                this.strings_version = version;
                this.words = HealthWords::new(cx);
                // The ages carry the "ago" template.
                for row in &mut this.rows {
                    row.age_secs = None;
                }
                this.refresh_ages(cx.background_executor().now());
                cx.notify();
            }
        });
        let hub_sub = host.hub().cloned().map(|hub| {
            cx.subscribe(&hub, |this: &mut Self, hub, ev: &SnapshotChanged, cx| {
                if ev.mask.contains(ComponentMask::HEALTH) {
                    let health = hub.read(cx).current().health.clone();
                    let clock = cx.background_executor().now();
                    if this.set_health(&health, clock) {
                        cx.notify();
                    }
                }
            })
        });
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this.update(cx, |this, cx| {
                    let clock = cx.background_executor().now();
                    if this.refresh_ages(clock) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let host_clock = host.clock(cx);
        let mut this = Self {
            host: host.clone(),
            rows: SourceId::ALL.into_iter().map(HealthRow::new).collect(),
            words: HealthWords::new(cx),
            strings_version: strings_version(cx),
            health_version: None,
            host_clock: host_clock.clone(),
            local_anchor: host_clock.local_now_ns(),
            clock_anchor: cx.background_executor().now(),
            buf: String::new(),
            _strings: strings,
            _hub_sub: hub_sub,
            _tick: tick,
        };
        if let Some(hub) = this.host.hub().cloned() {
            let health = hub.read(cx).current().health.clone();
            this.set_health(&health, this.clock_anchor);
        }
        this
    }

    /// The host's CLOCK_BOOTTIME at the executor time `clock`.
    fn now(&self, clock: Instant) -> TimeNs {
        let passed = clock.saturating_duration_since(self.clock_anchor);
        let local = self.local_anchor + passed.as_nanos() as TimeNs;
        self.host_clock.now_at(local)
    }

    /// Takes a new health snapshot at the executor time `clock`. Returns whether a row changed.
    fn set_health(&mut self, health: &HealthSnapshot, clock: Instant) -> bool {
        if self.health_version == Some(health.version) {
            return false;
        }
        self.health_version = Some(health.version);
        let now = self.now(clock);
        let mut changed = false;
        for row in &mut self.rows {
            changed |= row.set_health(health.get(row.source), now, &self.words.ago, &mut self.buf);
        }
        changed
    }

    /// Moves every age to the executor time `clock`. Returns whether a text changed.
    fn refresh_ages(&mut self, clock: Instant) -> bool {
        let now = self.now(clock);
        let mut changed = false;
        for row in &mut self.rows {
            changed |= row.set_age(now, &self.words.ago, &mut self.buf);
        }
        changed
    }

    /// The shown rows, in `SourceId` order: every source not in state Off.
    pub fn rows(&self) -> impl Iterator<Item = &HealthRow> {
        self.rows.iter().filter(|r| r.state != StateLook::Off)
    }

    /// The display name of `row`'s source.
    pub fn source_name(&self, row: &HealthRow) -> &SharedString {
        &self.words.sources[row.source.index()]
    }

    /// The state word of `row`.
    pub fn state_word(&self, row: &HealthRow) -> &SharedString {
        &self.words.states[row.state.index()]
    }
}

impl Render for CollectorHealthWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let cell = |w: Option<f32>| {
            let c = div()
                .h_full()
                .flex()
                .items_center()
                .px(m.sp[3])
                .overflow_hidden()
                .whitespace_nowrap();
            match w {
                Some(w) => c.w(px(w * s)).flex_none(),
                None => c.flex_1().min_w_0(),
            }
        };
        let widths = [
            Some(COL_SOURCE),
            Some(COL_STATE),
            Some(COL_AGE),
            Some(COL_RESTARTS),
            None,
        ];

        let mut header = div()
            .debug_selector(|| "CollectorHealth-header".into())
            .flex()
            .flex_row()
            .flex_none()
            .h(m.row_height)
            .bg(theme.header)
            .border_b_1()
            .border_color(theme.divider)
            .text_color(theme.text_secondary);
        *header.text_style() = t_sm(&m);
        for (ix, (heading, w)) in self.words.headings.iter().zip(widths).enumerate() {
            let c = cell(w).child(heading.clone());
            header = header.child(if ix == 3 { c.justify_end() } else { c });
        }

        let mut body = div()
            .id("CollectorHealth-rows")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        for (ix, row) in self.rows().enumerate() {
            let mut age = cell(Some(COL_AGE)).child(row.age.clone());
            if row.warming_up() {
                age = age.text_color(theme.text_secondary);
            }
            let mut restarts = cell(Some(COL_RESTARTS)).justify_end();
            *restarts.text_style() = numeric_style(&m, m.text.sm);
            let restarts = restarts.child(row.restarts_text.clone());
            let mut error = cell(None).id(("CollectorHealth-error", row.source.index()));
            if let Some(text) = &row.error {
                error = error.child(div().min_w_0().truncate().child(text.clone()));
                let tip = HoverTooltip::state(
                    ("CollectorHealth-error-tip", row.source.index()),
                    window,
                    cx,
                );
                error = HoverTooltip::attach(
                    &tip,
                    error,
                    DELAY_CONTROL,
                    TlTooltip::text(text.clone()).build(),
                    window,
                    cx,
                );
            }
            let mut r = div()
                .flex()
                .flex_row()
                .flex_none()
                .h(m.row_height)
                .text_color(theme.text_primary)
                .child(cell(Some(COL_SOURCE)).child(self.words.sources[row.source.index()].clone()))
                .child(
                    cell(Some(COL_STATE))
                        .text_color(row.state.color(&theme))
                        .child(self.words.states[row.state.index()].clone()),
                )
                .child(age)
                .child(restarts)
                .child(error);
            if ix % 2 == 1 {
                r = r.bg(theme.row_alt);
            }
            body = body.child(r);
        }

        let mut root = div()
            .id("CollectorHealth")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.list_bg);
        *root.text_style() = t_sm(&m);
        root.child(header).child(body)
    }
}
