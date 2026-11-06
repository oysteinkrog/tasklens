//! `AnimationDriver`: one per window. It turns the frame requests of every moving chart into one
//! `on_next_frame` per frame, records frame statistics for the phase-0 bench and detects the
//! zed#64499 frame stall (ADR-009 "Animation driver", ADR-010 "zed#64499 mitigation" and
//! "Hidden state", ADR-014).
//!
//! Storage: the window's constructor creates the driver with `cx.new(AnimationDriver::new)`,
//! keeps the `Entity` in its root view state (that strong handle keeps the driver alive) and calls
//! [`AnimationDriver::register`]. [`AnimationDriver::global`] then finds it through a per-window
//! registry that holds weak handles only.
//!
//! No other code requests frames: `Window::request_animation_frame` is not used (ADR-009).
//! Reduced motion is decided by the charts, which do not request; the driver does not read it.
//!
//! Pixel pacing (wlf.31, on by default since tasklens-nosaa): a moving chart asks through
//! [`AnimationDriver::request_paced`] for its next frame only when its plot has moved one physical
//! pixel: a pixel timer waits for that step and then arms one `on_next_frame`. The frame itself
//! still comes from the compositor. The setting `performance.pixel_paced_scrolling` turns it on
//! or off; the driver reads it live, so a change applies on the next frame of every open chart.
//! [`PACING_ENV`] is a developer override that wins over the setting. ADR-009 "Pixel pacing" has
//! the measurements and why the other design (one `on_next_frame` every frame, notifying only
//! charts that are due) lost.

use std::ffi::OsStr;
use std::mem;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, AsyncApp, Context, Entity, EntityId, Global, Subscription, WeakEntity, Window, WindowId,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::settings_global::TlSettings;

/// An armed frame with no callback for this long while visible is a stall (ADR-010).
pub const STALL_AFTER: Duration = Duration::from_millis(250);
/// Capacity of `FrameStats.samples` and `PaintLog.records`. Neither grows past it.
pub const SAMPLE_CAPACITY: usize = 65_536;
/// Developer override for pixel pacing: `timer` or `off` wins over the setting
/// `performance.pixel_paced_scrolling`. Anything else, or unset, leaves the setting in charge.
/// Read once per driver.
pub const PACING_ENV: &str = "TL_PIXEL_PACING";
/// A paced chart is notified by a frame that comes at most this long before its pixel step: about
/// half a frame at 120 Hz to 144 Hz, so the frame nearest the step draws it.
pub const PACE_SLACK: Duration = Duration::from_millis(4);
/// Once one paced requester is due, the ones due up to this much later draw in the same frame.
/// Charts that paint in one frame request one after another, so their due times differ by up to
/// about 1 ms; without this they split over two frames (wlf.31 measurement).
pub const PACE_MERGE: Duration = Duration::from_millis(2);

/// How moving charts are paced (wlf.31).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pacing {
    /// Every moving chart redraws on every compositor frame.
    #[default]
    Off,
    /// A timer waits for the next pixel step, then one `on_next_frame` is armed.
    Timer,
}

impl Pacing {
    /// `timer` and `off`; anything else, or `None`, is no override.
    pub fn parse(value: Option<&OsStr>) -> Option<Self> {
        match value.and_then(OsStr::to_str) {
            Some("timer") => Some(Self::Timer),
            Some("off") => Some(Self::Off),
            _ => None,
        }
    }

    /// The override from [`PACING_ENV`].
    pub fn from_env() -> Option<Self> {
        Self::parse(std::env::var_os(PACING_ENV).as_deref())
    }

    /// The pacing in force. The override `env` wins. Else `setting` is
    /// `performance.pixel_paced_scrolling`: `true` is `Timer`, `false` is `Off`. `None` (no
    /// `TlSettings` global, as in tests that install none) is `Off`.
    pub fn resolve(env: Option<Self>, setting: Option<bool>) -> Self {
        match (env, setting) {
            (Some(pacing), _) => pacing,
            (None, Some(true)) => Self::Timer,
            (None, Some(false) | None) => Self::Off,
        }
    }

    /// The bench report name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Timer => "timer",
        }
    }
}

type FrameCallback = Box<dyn Fn(&mut Window, &mut App)>;

pub struct AnimationDriver {
    requests: FxHashSet<EntityId>,
    callbacks: FxHashMap<EntityId, FrameCallback>,
    armed: bool,
    last_frame: Option<Instant>,
    /// When the current armed request was made.
    pending_since: Option<Instant>,
    /// The current stall was already counted and logged.
    stall_reported: bool,
    /// The stall timer loop is running.
    stall_timer: bool,
    hidden: bool,
    /// The pacing in force: [`Pacing::resolve`] of `env_pacing` and the setting.
    pacing: Pacing,
    /// The [`PACING_ENV`] override, read in `new`. While set, the setting is ignored.
    env_pacing: Option<Pacing>,
    /// Re-reads `performance.pixel_paced_scrolling` when the settings change.
    _settings: Subscription,
    /// Paced requesters and the time each is due: one physical pixel after its last paint.
    paced: FxHashMap<EntityId, Instant>,
    /// Timer pacing: the time the pending pixel timer fires and its generation. A timer whose
    /// generation is not the current one does nothing when it fires.
    pixel_timer: Option<(Instant, u64)>,
    pixel_timer_gen: u64,
    /// The thread CPU clock at the last frame, read only while sampling.
    last_cpu: Option<u64>,
    /// `tl_model::time::thread_cpu_ns`; tests swap it to count the calls.
    cpu_clock: fn() -> u64,
    pub stats: FrameStats,
    /// Bench hook (E01-OI-6): set by the bench before measuring.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub paint_log: Option<PaintLog>,
}

/// Bench hook: one record per `ChartElement::paint` while the log is installed (E01-OI-6).
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
pub struct PaintLog {
    /// Pre-allocated to 65,536, then stops growing.
    pub records: Vec<PaintRecord>,
}

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
impl PaintLog {
    /// Appends `record` unless the log is full.
    pub fn push(&mut self, record: PaintRecord) {
        if self.records.len() < SAMPLE_CAPACITY {
            self.records.push(record);
        }
    }
}

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug)]
pub struct PaintRecord {
    pub view: EntityId,
    /// Newest sample painted.
    pub newest_ns: u64,
    /// The chart host's CLOCK_BOOTTIME at paint ([`crate::hub::HostClock`]).
    pub painted_at_ns: u64,
}

#[derive(Default, Clone, Debug)]
pub struct FrameStats {
    /// `on_next_frame` arms.
    pub requested: u64,
    /// Callbacks received.
    pub frames: u64,
    /// Armed and no callback for 250 ms while visible.
    pub stalls: u64,
    /// Callback interval longer than 1.5 × `period_hint_ns`. Not counted while pacing is on.
    pub missed: u64,
    /// Frame callbacks that notified at least one requester: the frames the driver asked to be
    /// drawn (wlf.31).
    pub notify_frames: u64,
    /// Smallest pixel step of a paced request since the last `take_stats`, 0 when none.
    pub min_step_ns: u64,
    /// Set by the bench (6_944_444 at 144 Hz). 0 turns missed-frame counting off.
    pub period_hint_ns: u64,
    pub last_interval_ns: u64,
    pub sampling: bool,
    /// Filled only while `sampling`. Capacity 65,536 from the first sample, then stops growing.
    pub samples: Vec<FrameSample>,
}

#[derive(Clone, Copy, Debug)]
pub struct FrameSample {
    pub interval_ns: u64,
    /// UI thread CPU time between the previous frame callback and this one: the frame's own
    /// work, as the bench probe measures it. 0 for the first sample after sampling turns on.
    pub thread_cpu_ns: u64,
}

/// Window id to driver. Weak handles: the window's root view owns the driver.
#[derive(Default)]
struct AnimationDrivers(FxHashMap<WindowId, WeakEntity<AnimationDriver>>);

impl Global for AnimationDrivers {}

impl AnimationDriver {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let env_pacing = Pacing::from_env();
        let pacing = Pacing::resolve(env_pacing, Self::pacing_setting(cx));
        let settings = cx.observe_global::<TlSettings>(Self::on_settings);
        Self {
            requests: FxHashSet::default(),
            callbacks: FxHashMap::default(),
            armed: false,
            last_frame: None,
            pending_since: None,
            stall_reported: false,
            stall_timer: false,
            hidden: false,
            pacing,
            env_pacing,
            _settings: settings,
            paced: FxHashMap::default(),
            pixel_timer: None,
            pixel_timer_gen: 0,
            last_cpu: None,
            cpu_clock: tl_model::time::thread_cpu_ns,
            stats: FrameStats::default(),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            paint_log: None,
        }
    }

    /// Makes `driver` the one `global` returns for `window`. Called once by the window's
    /// constructor. Also drops the entries of drivers that no longer exist.
    pub fn register(driver: &Entity<Self>, window: &Window, cx: &mut App) {
        let id = window.window_handle().window_id();
        let drivers = &mut cx.default_global::<AnimationDrivers>().0;
        drivers.retain(|_, d| d.upgrade().is_some());
        drivers.insert(id, driver.downgrade());
    }

    /// The driver of this window, or `None` when the window registered none.
    pub fn try_global(window: &Window, cx: &App) -> Option<Entity<AnimationDriver>> {
        let id = window.window_handle().window_id();
        cx.try_global::<AnimationDrivers>()?.0.get(&id)?.upgrade()
    }

    /// The driver of this window.
    ///
    /// # Panics
    /// When the window's constructor did not call [`AnimationDriver::register`].
    pub fn global(window: &Window, cx: &App) -> Entity<AnimationDriver> {
        Self::try_global(window, cx).expect("the window registered no AnimationDriver")
    }

    /// Asks for `cx.notify(id)` on the next frame. `id` may be any entity, not only a view
    /// (R-209).
    pub fn request(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        self.requests.insert(id);
        self.arm(window, cx);
    }

    /// A moving chart's frame request. `step_ns` is how long its plot takes to move one physical
    /// pixel. With pacing off this is [`Self::request`]. With pacing on, `id` is notified by the
    /// first frame at most [`PACE_SLACK`] before `step_ns` from now; a later call replaces the
    /// due time, because the chart has just drawn.
    pub fn request_paced(
        &mut self,
        id: EntityId,
        step_ns: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pacing == Pacing::Off {
            self.request(id, window, cx);
            return;
        }
        let step_ns = step_ns.max(1);
        if self.stats.min_step_ns == 0 || step_ns < self.stats.min_step_ns {
            self.stats.min_step_ns = step_ns;
        }
        let due = cx.background_executor().now() + Duration::from_nanos(step_ns);
        self.paced.insert(id, due);
        self.schedule_paced(window, cx);
    }

    /// Runs `f` once on the next frame, after the notifies. A second call with the same `id`
    /// before that frame replaces the first `f` (R-209). A callback that wants another frame
    /// calls `request_callback` again.
    pub fn request_callback(
        &mut self,
        id: EntityId,
        f: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.callbacks.insert(id, Box::new(f));
        self.arm(window, cx);
    }

    /// Called by the hub's hidden-state timer (ADR-010). While hidden no frame is armed. Going
    /// visible forgets the armed frame, which a parked frame loop may never deliver, so the next
    /// request arms again.
    pub fn set_hidden(&mut self, hidden: bool) {
        if self.hidden && !hidden {
            self.armed = false;
            self.pending_since = None;
        }
        if hidden {
            // A pending pixel timer does nothing when it fires.
            self.pixel_timer = None;
        }
        self.hidden = hidden;
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }

    pub fn pacing(&self) -> Pacing {
        self.pacing
    }

    /// A pixel timer is pending (timer pacing).
    pub fn has_pixel_timer(&self) -> bool {
        self.pixel_timer.is_some()
    }

    /// Tests and the bench set pacing here, never through the environment. It pins the pacing
    /// like [`PACING_ENV`] does, so a later settings change does not undo it.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub fn set_pacing(&mut self, pacing: Pacing) {
        self.env_pacing = Some(pacing);
        self.pacing = pacing;
    }

    /// Tests: sets the [`PACING_ENV`] override as if read in `new`, then applies the result.
    /// `None` hands pacing back to the setting.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub fn set_env_pacing(&mut self, env: Option<Pacing>, cx: &mut Context<Self>) {
        self.env_pacing = env;
        let pacing = Pacing::resolve(env, Self::pacing_setting(cx));
        self.switch_pacing(pacing, cx);
    }

    /// `performance.pixel_paced_scrolling`, or `None` without the `TlSettings` global.
    fn pacing_setting(cx: &App) -> Option<bool> {
        cx.try_global::<TlSettings>()
            .map(|_| TlSettings::settings(cx).performance.pixel_paced_scrolling)
    }

    /// The settings changed: follow `performance.pixel_paced_scrolling` unless the override is
    /// set.
    fn on_settings(&mut self, cx: &mut Context<Self>) {
        if self.env_pacing.is_some() {
            return;
        }
        let pacing = Pacing::resolve(None, Self::pacing_setting(cx));
        self.switch_pacing(pacing, cx);
    }

    /// Applies a new pacing at once. Timer to off: the pending pixel timer does nothing when it
    /// fires, and every paced requester is notified now; it repaints and requests again, which
    /// with pacing off arms the next frame. Off to timer: the next `request_paced` starts the
    /// pixel timer.
    fn switch_pacing(&mut self, pacing: Pacing, cx: &mut Context<Self>) {
        if pacing == self.pacing {
            return;
        }
        self.pacing = pacing;
        if pacing == Pacing::Off {
            self.pixel_timer = None;
            for (id, _) in self.paced.drain() {
                App::notify(cx, id);
            }
        }
    }

    /// Bench: returns the stats and resets the counters. Keeps `period_hint_ns` and `sampling`.
    pub fn take_stats(&mut self) -> FrameStats {
        let taken = FrameStats {
            samples: mem::take(&mut self.stats.samples),
            ..self.stats.clone()
        };
        self.stats = FrameStats {
            period_hint_ns: taken.period_hint_ns,
            sampling: taken.sampling,
            ..FrameStats::default()
        };
        taken
    }

    /// Replaces the thread CPU clock, so a test can count the calls.
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub fn set_cpu_clock(&mut self, clock: fn() -> u64) {
        self.cpu_clock = clock;
    }

    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub fn install_paint_log(&mut self) {
        self.paint_log = Some(PaintLog {
            records: Vec::with_capacity(SAMPLE_CAPACITY),
        });
    }

    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub fn take_paint_log(&mut self) -> Option<PaintLog> {
        self.paint_log.take()
    }

    fn arm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.armed || self.hidden {
            return;
        }
        let driver = cx.weak_entity();
        window.on_next_frame(move |window, cx| Self::frame(&driver, window, cx));
        self.armed = true;
        self.pending_since = Some(cx.background_executor().now());
        self.stall_reported = false;
        self.stats.requested += 1;
        self.start_stall_timer(cx);
    }

    /// Paced requests wait for their step on the pixel timer.
    fn schedule_paced(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pacing == Pacing::Timer {
            self.start_pixel_timer(window, cx);
        }
    }

    /// Starts the pixel timer for the earliest due time, unless one fires at or before it. The
    /// timer arms a frame; a frame is never armed early, so no frame is drawn before a step.
    fn start_pixel_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hidden {
            return;
        }
        let Some(due) = self.paced.values().min().copied() else {
            return;
        };
        let fire_at = due.checked_sub(PACE_SLACK).unwrap_or(due);
        if self.pixel_timer.is_some_and(|(at, _)| at <= fire_at) {
            return;
        }
        self.pixel_timer_gen += 1;
        let generation = self.pixel_timer_gen;
        self.pixel_timer = Some((fire_at, generation));
        let wait = fire_at.saturating_duration_since(cx.background_executor().now());
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx| {
            cx.background_executor().timer(wait).await;
            let _ = cx.update(|window, cx| {
                this.update(cx, |d, cx| d.pixel_timer_fired(generation, window, cx))
            });
        })
        .detach();
    }

    /// Arms a frame when a paced requester is due, else waits again.
    fn pixel_timer_fired(&mut self, generation: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.pixel_timer.is_none_or(|(_, g)| g != generation) {
            return;
        }
        self.pixel_timer = None;
        let Some(due) = self.paced.values().min().copied() else {
            return;
        };
        if due <= cx.background_executor().now() + PACE_SLACK {
            self.arm(window, cx);
        } else {
            self.start_pixel_timer(window, cx);
        }
    }

    /// The `on_next_frame` callback. The callbacks run outside the driver's update so they can
    /// request again.
    fn frame(driver: &WeakEntity<Self>, window: &mut Window, cx: &mut App) {
        let Ok(mut callbacks) = driver.update(cx, |d, cx| d.on_frame(window, cx)) else {
            return;
        };
        for (_, f) in callbacks.drain() {
            f(window, cx);
        }
        // Give the drained map back so steady state allocates nothing.
        let _ = driver.update(cx, |d, _| {
            if d.callbacks.is_empty() {
                d.callbacks = callbacks;
            }
        });
    }

    /// Counts the frame, notifies every requester and every paced requester that is due, and
    /// returns the callbacks to run.
    fn on_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FxHashMap<EntityId, FrameCallback> {
        self.armed = false;
        self.pending_since = None;
        self.stats.frames += 1;
        let now = cx.background_executor().now();
        // The clock is read only while sampling, so it costs nothing otherwise.
        let cpu = self.stats.sampling.then(|| (self.cpu_clock)());
        let last_cpu = mem::replace(&mut self.last_cpu, cpu);
        if let Some(last) = self.last_frame {
            let interval_ns =
                u64::try_from(now.saturating_duration_since(last).as_nanos()).unwrap_or(u64::MAX);
            self.stats.last_interval_ns = interval_ns;
            let period = self.stats.period_hint_ns;
            if self.pacing == Pacing::Off
                && period > 0
                && interval_ns > period.saturating_mul(3) / 2
            {
                self.stats.missed += 1;
            }
            if self.stats.sampling {
                let samples = &mut self.stats.samples;
                if samples.capacity() == 0 {
                    samples.reserve_exact(SAMPLE_CAPACITY);
                }
                if samples.len() < SAMPLE_CAPACITY {
                    samples.push(FrameSample {
                        interval_ns,
                        thread_cpu_ns: cpu
                            .zip(last_cpu)
                            .map_or(0, |(now, last)| now.saturating_sub(last)),
                    });
                }
            }
        }
        self.last_frame = Some(now);
        let mut notified = !self.requests.is_empty();
        for id in self.requests.drain() {
            App::notify(cx, id);
        }
        if !self.paced.is_empty() {
            let mut edge = now + PACE_SLACK;
            if self.paced.values().any(|due| *due <= edge) {
                edge += PACE_MERGE;
            }
            self.paced.retain(|&id, due| {
                let is_due = *due <= edge;
                if is_due {
                    App::notify(cx, id);
                    notified = true;
                }
                !is_due
            });
            // Requesters not yet due wait for their step; a notified chart requests again when it
            // paints.
            self.schedule_paced(window, cx);
        }
        if notified {
            self.stats.notify_frames += 1;
        }
        mem::take(&mut self.callbacks)
    }

    /// Starts the stall timer loop unless it runs. It stops by itself once nothing is armed.
    fn start_stall_timer(&mut self, cx: &mut Context<Self>) {
        if self.stall_timer {
            return;
        }
        self.stall_timer = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut wait = STALL_AFTER;
            loop {
                cx.background_executor().timer(wait).await;
                match this.update(cx, |d, cx| d.check_stall(cx)) {
                    Ok(Some(next)) => wait = next,
                    _ => break,
                }
            }
        })
        .detach();
    }

    /// One stall timer tick. Returns how long to wait for the next tick, or `None` to stop the
    /// loop. A stall is counted and logged once and never re-arms by itself.
    fn check_stall(&mut self, cx: &mut Context<Self>) -> Option<Duration> {
        let Some(since) = self.pending_since.filter(|_| self.armed) else {
            self.stall_timer = false;
            return None;
        };
        let elapsed = cx
            .background_executor()
            .now()
            .saturating_duration_since(since);
        if elapsed < STALL_AFTER {
            return Some(STALL_AFTER - elapsed);
        }
        if !self.hidden && !self.stall_reported {
            self.stall_reported = true;
            self.stats.stalls += 1;
            let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
            tracing::warn!(target: "tasklens::frame", elapsed_ms, "frame stall");
        }
        Some(STALL_AFTER)
    }
}
