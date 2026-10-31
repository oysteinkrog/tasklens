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

use std::mem;
use std::time::{Duration, Instant};

use gpui_kit::{App, AsyncApp, Context, Entity, EntityId, Global, WeakEntity, Window, WindowId};
use rustc_hash::{FxHashMap, FxHashSet};

/// An armed frame with no callback for this long while visible is a stall (ADR-010).
pub const STALL_AFTER: Duration = Duration::from_millis(250);
/// Capacity of `FrameStats.samples` and `PaintLog.records`. Neither grows past it.
pub const SAMPLE_CAPACITY: usize = 65_536;

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
    /// The thread CPU clock at the last frame, read only while sampling.
    last_cpu: Option<u64>,
    /// `tl_model::time::thread_cpu_ns`; tests swap it to count the calls.
    cpu_clock: fn() -> u64,
    pub stats: FrameStats,
    /// Bench hook (E01-OI-6): set by the bench before measuring.
    #[cfg(any(test, feature = "test-hooks"))]
    pub paint_log: Option<PaintLog>,
}

/// Bench hook: one record per `ChartElement::paint` while the log is installed (E01-OI-6).
#[cfg(any(test, feature = "test-hooks"))]
pub struct PaintLog {
    /// Pre-allocated to 65,536, then stops growing.
    pub records: Vec<PaintRecord>,
}

#[cfg(any(test, feature = "test-hooks"))]
impl PaintLog {
    /// Appends `record` unless the log is full.
    pub fn push(&mut self, record: PaintRecord) {
        if self.records.len() < SAMPLE_CAPACITY {
            self.records.push(record);
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
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
    /// Callback interval longer than 1.5 × `period_hint_ns`.
    pub missed: u64,
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
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            requests: FxHashSet::default(),
            callbacks: FxHashMap::default(),
            armed: false,
            last_frame: None,
            pending_since: None,
            stall_reported: false,
            stall_timer: false,
            hidden: false,
            last_cpu: None,
            cpu_clock: tl_model::time::thread_cpu_ns,
            stats: FrameStats::default(),
            #[cfg(any(test, feature = "test-hooks"))]
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
        self.hidden = hidden;
    }

    pub fn is_armed(&self) -> bool {
        self.armed
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
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_cpu_clock(&mut self, clock: fn() -> u64) {
        self.cpu_clock = clock;
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn install_paint_log(&mut self) {
        self.paint_log = Some(PaintLog {
            records: Vec::with_capacity(SAMPLE_CAPACITY),
        });
    }

    #[cfg(any(test, feature = "test-hooks"))]
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

    /// The `on_next_frame` callback. The callbacks run outside the driver's update so they can
    /// request again.
    fn frame(driver: &WeakEntity<Self>, window: &mut Window, cx: &mut App) {
        let Ok(mut callbacks) = driver.update(cx, |d, cx| d.on_frame(cx)) else {
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

    /// Counts the frame, notifies every requester and returns the callbacks to run.
    fn on_frame(&mut self, cx: &mut Context<Self>) -> FxHashMap<EntityId, FrameCallback> {
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
            if period > 0 && interval_ns > period.saturating_mul(3) / 2 {
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
        for id in self.requests.drain() {
            App::notify(cx, id);
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
