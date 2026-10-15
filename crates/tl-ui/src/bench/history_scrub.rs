//! Bench scene `history-scrub` (PROC-092, ADR-014 P3 scene, UI-PRC-032).
//!
//! [`ScrubSynth`] makes a synthetic 6 h history from [`SEED`]: 2,000 processes sampled at 1 Hz,
//! 200 of which change each second. [`ScrubEngine`] answers each `HistoryQuery` on its own
//! thread with a view built from that history on the fly, never from disk. The scene opens the
//! Processes tab in History mode with the CPU metric at the phase-0 fixture size. [`ScrubDriver`]
//! moves the scrub point 1 s per frame across the 10-minute strip window, zooms out to the whole
//! span and back every 10 s, and moves the pointer over the plot every 500 ms so the top-3 card
//! asks for its list. The report is the ADR-014 JSON plus the UI thread CPU of the frames that
//! followed a scrub point or strip window change (target: p99 at most 4 ms).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity, IntoElement,
    Modifiers, MouseMoveEvent, ParentElement as _, PlatformInput, Render, Styled as _, Task,
    Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowOptions, div, point,
    px, size,
};
use serde::Serialize;
use tl_model::actions::ActionEvent;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, HistoryWhat};
use tl_model::demand::Demand;
use tl_model::history::{
    HISTORY_MAX_POINTS, HistoryMetric, HistoryPeaks, HistoryPoint, HistoryProcessRow, HistoryRange,
    HistorySeries, HistorySource, HistoryStackBucket, HistoryStoreState, HistoryTop, HistoryTopRow,
    HistoryTotals, HistoryView,
};
use tl_model::ids::ProcessKey;
use tl_model::journal::{JournalHandle, JournalQuery};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{ComponentHeader, Snapshot};
use tl_model::time::{BootId, TimeNs, now_boot_ns};
use tl_model::validity::Validity;

use super::scenes::{BenchError, BenchOptions, BenchScene, WINDOW_H, WINDOW_W, fixture};
use super::stats::{
    BenchClocks, BenchReport, FrameProbe, PROBE_CAPACITY, count_missed, percentiles_ms,
    process_cpu_pct, read_rss_pss_mb,
};
use super::synth::{BenchRng, SEED};
use crate::anim::AnimationDriver;
use crate::app::{AppEnv, TasklensUi, UiInit};
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::history::HistoryPage;
use crate::window::main::APP_ID;

const NS_PER_S: u64 = 1_000_000_000;
const NS_PER_MS: f64 = 1_000_000.0;

/// Oldest retained second on the boot clock. Fixed, never `now`, so two runs give the same views.
pub const SPAN_START_NS: TimeNs = 3_600 * NS_PER_S;
/// Length of the synthetic history: 6 h.
pub const SPAN_S: u64 = 21_600;
/// Newest retained second.
pub const SPAN_END_NS: TimeNs = SPAN_START_NS + SPAN_S * NS_PER_S;
/// Processes alive over the whole span.
pub const PROCESSES: usize = 2_000;
/// Process `p` changes at the seconds `s` where `(s + p) % CHANGE_EVERY_S == 0`, so 200 of the
/// 2,000 change each second.
const CHANGE_EVERY_S: u64 = 10;
/// PID of process 0.
const FIRST_PID: u32 = 1_000;
/// Per-process CPU is uniform in `[0, CPU_MAX_PCT)` percent of the machine, so the system total
/// stays near 50%.
const CPU_MAX_PCT: f32 = 0.05;
/// Machine memory for the totals and the top-3 card (64 GiB).
const MEM_TOTAL_BYTES: u64 = 64 << 30;
/// Used memory in a Span answer's totals (16 GiB).
const MEM_USED_BYTES: u64 = 16 << 30;
/// CLOCK_REALTIME minus CLOCK_BOOTTIME of the synthetic history, for wall-clock labels.
const REALTIME_OFFSET_NS: i64 = 1_790_000_000 * NS_PER_S as i64;

/// The strip window the page starts with and the driver sweeps (UI-PRC-032 default).
pub const NEAR_VIEW_NS: TimeNs = 600 * NS_PER_S;
/// The driver zooms out to the whole span this often, and back on the next frame.
pub const ZOOM_EVERY: Duration = Duration::from_secs(10);
/// The driver moves the pointer over the plot this often.
pub const HOVER_EVERY: Duration = Duration::from_millis(500);

/// CPU of process `p` at second index `s`. It changes only when `(s + p) / CHANGE_EVERY_S` does.
fn cpu_value(seed: u64, p: usize, s: u64) -> f32 {
    let epoch = (s + p as u64) / CHANGE_EVERY_S;
    BenchRng::new(seed ^ ((p as u64) << 32 | epoch)).uniform(0.0, CPU_MAX_PCT)
}

/// The second index (0 is [`SPAN_START_NS`]) at or before `t`, clamped to the span.
fn second_at(t: TimeNs) -> u64 {
    (t.clamp(SPAN_START_NS, SPAN_END_NS) - SPAN_START_NS) / NS_PER_S
}

fn t_of(s: u64) -> TimeNs {
    SPAN_START_NS + s * NS_PER_S
}

/// The tl-history bucket width: `max(1 s, ceil((to - from) / 1023))` rounded up to whole
/// seconds, so aligned buckets give at most 1,024 points.
fn bucket_width_ns(from: TimeNs, to: TimeNs) -> u64 {
    let per_point = to
        .saturating_sub(from)
        .div_ceil(HISTORY_MAX_POINTS as u64 - 1);
    per_point.div_ceil(NS_PER_S).max(1) * NS_PER_S
}

/// Min and max of `value` per bucket over the seconds in `[from, to]` inside the span. Bucket `i`
/// covers `[floor(t / w) * w, +w)`. Also returns the first second of each bucket.
fn decimate(
    from: TimeNs,
    to: TimeNs,
    mut value: impl FnMut(u64) -> f32,
) -> (Vec<HistoryPoint>, Vec<u64>) {
    let mut points = Vec::new();
    let mut firsts = Vec::new();
    let (lo, hi) = (from.max(SPAN_START_NS), to.min(SPAN_END_NS));
    if lo > hi {
        return (points, firsts);
    }
    let w = bucket_width_ns(from, to);
    points.reserve(HISTORY_MAX_POINTS);
    firsts.reserve(HISTORY_MAX_POINTS);
    let first = (lo - SPAN_START_NS).div_ceil(NS_PER_S);
    let last = (hi - SPAN_START_NS) / NS_PER_S;
    for s in first..=last {
        let start = t_of(s) / w * w;
        let v = value(s);
        match points.last_mut() {
            Some(HistoryPoint { t_ns, min, max }) if *t_ns == start => {
                *min = min.min(v);
                *max = max.max(v);
            }
            _ => {
                points.push(HistoryPoint {
                    t_ns: start,
                    min: v,
                    max: v,
                });
                firsts.push(s);
            }
        }
    }
    (points, firsts)
}

/// A strip window and its system total points and stack buckets.
type StripCache = ((TimeNs, TimeNs), Vec<HistoryPoint>, Vec<HistoryStackBucket>);

/// The synthetic 6 h history and the query answers built from it. Holds the last view and the
/// last top-3 list, as the engine's history worker does: a TopAt answer is the last view with
/// the new list, and every answer carries the last list.
pub struct ScrubSynth {
    seed: u64,
    keys: Vec<ProcessKey>,
    names: Vec<Arc<str>>,
    user: Arc<str>,
    /// System CPU total per second, `SPAN_S + 1` values.
    totals: Vec<f32>,
    /// The whole span decimated, shared by every answer that asks for it.
    overview: Arc<[HistoryPoint]>,
    gaps: Arc<[(TimeNs, TimeNs)]>,
    /// The system total points and stack of the last strip window. Each answer copies them into
    /// new `Arc`s, as a fresh build would.
    strip: Option<StripCache>,
    version: u64,
    last_top: Option<Arc<HistoryTop>>,
    last_view: Option<HistoryView>,
}

impl ScrubSynth {
    /// Builds the keys, names and the per-second system total. Each second adds the change of
    /// only the 200 processes that change in it.
    pub fn new(seed: u64) -> Self {
        let keys = (0..PROCESSES)
            .map(|p| ProcessKey {
                start_ticks: 100 + p as u64,
                boot_id: BootId(0),
                pid: FIRST_PID + p as u32,
            })
            .collect();
        let names = (0..PROCESSES)
            .map(|p| Arc::from(format!("proc-{p}")))
            .collect();
        let mut cur: Vec<f32> = (0..PROCESSES).map(|p| cpu_value(seed, p, 0)).collect();
        let mut total: f64 = cur.iter().map(|&v| f64::from(v)).sum();
        let mut totals = Vec::with_capacity(SPAN_S as usize + 1);
        totals.push(total as f32);
        for s in 1..=SPAN_S {
            // The processes with (s + p) % 10 == 0.
            let first = ((CHANGE_EVERY_S - s % CHANGE_EVERY_S) % CHANGE_EVERY_S) as usize;
            for p in (first..PROCESSES).step_by(CHANGE_EVERY_S as usize) {
                let v = cpu_value(seed, p, s);
                total += f64::from(v) - f64::from(cur[p]);
                cur[p] = v;
            }
            totals.push(total.max(0.0) as f32);
        }
        let (overview, _) = decimate(SPAN_START_NS, SPAN_END_NS, |s| totals[s as usize]);
        Self {
            seed,
            keys,
            names,
            user: Arc::from("bench"),
            totals,
            overview: Arc::from(overview),
            gaps: Arc::default(),
            strip: None,
            version: 0,
            last_top: None,
            last_view: None,
        }
    }

    /// Oldest and newest retained second.
    pub fn span(&self) -> (TimeNs, TimeNs) {
        (SPAN_START_NS, SPAN_END_NS)
    }

    /// Answers one `HistoryQuery`, bumps the header version and keeps the view as the last one.
    /// Every metric reads the synthetic CPU values; the scene shows only CPU.
    pub fn answer(&mut self, id: u64, range: HistoryRange, what: HistoryWhat) -> HistoryView {
        let mut view = match what {
            HistoryWhat::TopAt { t, metric, n } => {
                self.last_top = Some(Arc::new(self.top(id, t, metric, n)));
                match self.last_view.clone() {
                    Some(view) => view,
                    None => self.frame(id, range, metric, (0, 0), &[], false),
                }
            }
            HistoryWhat::Frame {
                metric,
                view,
                keys,
                overview,
            } => self.frame(id, range, metric, view, &keys, overview),
        };
        self.version += 1;
        view.header.version = self.version;
        view.top = self.last_top.clone();
        self.last_view = Some(view.clone());
        view
    }

    fn frame(
        &mut self,
        id: u64,
        range: HistoryRange,
        metric: HistoryMetric,
        view: (TimeNs, TimeNs),
        keys: &[ProcessKey],
        overview: bool,
    ) -> HistoryView {
        let (frame_s, row_s, totals) = match range {
            HistoryRange::Live => (Some(SPAN_S), SPAN_S, None),
            HistoryRange::At(t) => (Some(second_at(t)), second_at(t), None),
            // The scene never sets a range: the rows hold the values at `to`, not averages.
            HistoryRange::Span { from, to } => {
                let (a, b) = (second_at(from), second_at(to).max(second_at(from)));
                let sum: f64 = self.totals[a as usize..=b as usize]
                    .iter()
                    .map(|&v| f64::from(v))
                    .sum();
                let totals = HistoryTotals {
                    cpu_pct: (sum / (b - a + 1) as f64) as f32,
                    mem_used_bytes: MEM_USED_BYTES,
                    mem_total_bytes: MEM_TOTAL_BYTES,
                };
                (None, b, Some(totals))
            }
        };
        let span_range = matches!(range, HistoryRange::Span { .. });
        let rows: Arc<[HistoryProcessRow]> = (0..PROCESSES)
            .map(|p| {
                let cpu = cpu_value(self.seed, p, row_s);
                HistoryProcessRow {
                    key: self.keys[p],
                    name: Arc::clone(&self.names[p]),
                    comm: Arc::clone(&self.names[p]),
                    user: Arc::clone(&self.user),
                    exe: None,
                    app_id: None,
                    system: false,
                    present: true,
                    previous_boot: false,
                    cpu_pct: cpu,
                    mem_anon_bytes: 0,
                    disk_bps: 0.0,
                    net_bps: 0.0,
                    gpu_pct: 0.0,
                    peak: span_range.then_some(HistoryPeaks {
                        cpu_pct: cpu,
                        mem_anon_bytes: 0,
                        disk_bps: 0.0,
                        net_bps: 0.0,
                        gpu_pct: 0.0,
                    }),
                }
            })
            .collect();

        if self.strip.as_ref().is_none_or(|c| c.0 != view) {
            self.strip = Some(self.build_strip(view));
        }
        let (total_points, stack): (Arc<[HistoryPoint]>, Arc<[HistoryStackBucket]>) = match &self
            .strip
        {
            Some((_, points, stack)) => (Arc::from(points.as_slice()), Arc::from(stack.as_slice())),
            None => (Arc::default(), Arc::default()),
        };
        let mut series = Vec::with_capacity(1 + keys.len());
        series.push(HistorySeries {
            key: None,
            metric,
            points: total_points,
        });
        for &key in keys {
            let p = key.pid.wrapping_sub(FIRST_PID) as usize;
            let points = if self.keys.get(p) == Some(&key) {
                decimate(view.0, view.1, |s| cpu_value(self.seed, p, s)).0
            } else {
                Vec::new()
            };
            series.push(HistorySeries {
                key: Some(key),
                metric,
                points: Arc::from(points),
            });
        }

        HistoryView {
            header: ComponentHeader {
                version: 0,
                observed_start: SPAN_START_NS,
                observed_end: SPAN_END_NS,
                validity: Validity::Valid,
            },
            query_id: id,
            source: HistorySource::Store,
            store_state: HistoryStoreState::Ready,
            span: Some(self.span()),
            gaps: Arc::clone(&self.gaps),
            frame_t: frame_s.map(t_of),
            range,
            process_count: PROCESSES as u32,
            rows,
            totals,
            series: Arc::from(series),
            stack,
            overview: if overview {
                Arc::clone(&self.overview)
            } else {
                Arc::default()
            },
            top: None,
            realtime_offset_ns: REALTIME_OFFSET_NS,
        }
    }

    /// The system total over `view` and, per bucket, the top 3 and the rest at the bucket's
    /// first second.
    fn build_strip(&self, view: (TimeNs, TimeNs)) -> StripCache {
        let (points, firsts) = decimate(view.0, view.1, |s| self.totals[s as usize]);
        let stack = points
            .iter()
            .zip(&firsts)
            .map(|(point, &s)| {
                let mut top = [0.0f32; 3];
                for (slot, (_, v)) in top.iter_mut().zip(self.largest(s, 3)) {
                    *slot = v;
                }
                HistoryStackBucket {
                    t_ns: point.t_ns,
                    top,
                    other: (self.totals[s as usize] - top.iter().sum::<f32>()).max(0.0),
                }
            })
            .collect();
        (view, points, stack)
    }

    /// The `n` largest processes at second `s`, largest first, ties by process order.
    fn largest(&self, s: u64, n: usize) -> Vec<(usize, f32)> {
        let mut out: Vec<(usize, f32)> = Vec::with_capacity(n + 1);
        if n == 0 {
            return out;
        }
        for p in 0..PROCESSES {
            let v = cpu_value(self.seed, p, s);
            if out.len() == n && out[n - 1].1 >= v {
                continue;
            }
            let at = out.iter().position(|&(_, o)| o < v).unwrap_or(out.len());
            out.insert(at, (p, v));
            out.truncate(n);
        }
        out
    }

    fn top(&self, id: u64, t: TimeNs, metric: HistoryMetric, n: u8) -> HistoryTop {
        let s = second_at(t);
        let rows: Arc<[HistoryTopRow]> = self
            .largest(s, usize::from(n))
            .into_iter()
            .map(|(p, value)| HistoryTopRow {
                key: self.keys[p],
                name: Arc::clone(&self.names[p]),
                app_id: None,
                value,
            })
            .collect();
        HistoryTop {
            query_id: id,
            t_ns: t_of(s),
            metric,
            cpu_total_pct: self.totals[s as usize],
            mem_total_bytes: MEM_TOTAL_BYTES,
            rows,
        }
    }
}

/// Locks `m`, ignoring poison: a worker panic aborts the bench run anyway.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A `HistoryQuery`: id, range and payload.
type Query = (u64, HistoryRange, HistoryWhat);

struct Shared {
    latest: Mutex<Arc<Snapshot>>,
    wake_tx: async_channel::Sender<()>,
}

impl Shared {
    /// ADR-005 publish order: store the snapshot, then one wake token. A full channel means a
    /// wake is already pending.
    fn publish(&self, snap: Snapshot) {
        *lock(&self.latest) = Arc::new(snap);
        let _ = self.wake_tx.try_send(());
    }
}

/// The scene's `EngineClient`. `send` only queues a `HistoryQuery` for the `tl-bench-hist`
/// thread, which builds the view, publishes a snapshot with a higher sequence and history
/// version, then sends one wake. No view is built on the UI thread. Other commands are dropped.
pub struct ScrubEngine {
    shared: Arc<Shared>,
    wake_rx: async_channel::Receiver<()>,
    /// Kept so the action event channel stays open and empty.
    _ev_tx: async_channel::Sender<ActionEvent>,
    ev_rx: async_channel::Receiver<ActionEvent>,
    query_tx: Mutex<Option<mpsc::Sender<Query>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ScrubEngine {
    /// Builds the synthetic history on the calling thread, then starts `tl-bench-hist`. The
    /// first snapshot is empty, with sequence 0.
    pub fn start(seed: u64) -> Arc<Self> {
        let mut synth = ScrubSynth::new(seed);
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (ev_tx, ev_rx) = async_channel::bounded(256);
        let (query_tx, query_rx) = mpsc::channel::<Query>();
        let base = Snapshot::empty(BootId(0), 0);
        let shared = Arc::new(Shared {
            latest: Mutex::new(Arc::new(base.clone())),
            wake_tx,
        });
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("tl-bench-hist".into())
            .spawn(move || {
                let mut snap = base;
                // Ends when `stop` drops the sender.
                while let Ok((id, range, what)) = query_rx.recv() {
                    let view = synth.answer(id, range, what);
                    snap.sequence += 1;
                    snap.published_ns = now_boot_ns();
                    snap.history = Arc::new(view);
                    thread_shared.publish(snap.clone());
                }
            })
            .expect("spawn tl-bench-hist");
        Arc::new(Self {
            shared,
            wake_rx,
            _ev_tx: ev_tx,
            ev_rx,
            query_tx: Mutex::new(Some(query_tx)),
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Stops the worker after the query it is on and joins it. A worker panic is re-raised here.
    pub fn stop(&self) {
        lock(&self.query_tx).take();
        if let Some(thread) = lock(&self.thread).take()
            && let Err(panic) = thread.join()
        {
            std::panic::resume_unwind(panic);
        }
    }
}

impl Drop for ScrubEngine {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.stop();
        }
    }
}

impl EngineClient for ScrubEngine {
    fn latest(&self) -> Arc<Snapshot> {
        Arc::clone(&lock(&self.shared.latest))
    }

    fn wake(&self) -> async_channel::Receiver<()> {
        self.wake_rx.clone()
    }

    fn action_events(&self) -> async_channel::Receiver<ActionEvent> {
        self.ev_rx.clone()
    }

    fn set_demand(&self, _demand: Demand) {}

    fn set_settings(&self, _settings: EngineSettings) {}

    fn send(&self, cmd: Command) -> Result<(), CommandError> {
        if let Command::HistoryQuery { id, range, what } = cmd {
            match lock(&self.query_tx).as_ref() {
                Some(tx) if tx.send((id, range, what)).is_ok() => {}
                _ => return Err(CommandError::EngineStopped),
            }
        }
        Ok(())
    }

    /// The handle's channels are closed: no entries ever arrive.
    fn journal(&self, _query: JournalQuery) -> JournalHandle {
        let (_events_tx, events) = async_channel::bounded(1);
        let (control, _control_rx) = async_channel::bounded(1);
        JournalHandle::new(events, control)
    }
}

/// The scene window's root: the Processes tab and the window's animation driver.
pub struct ScrubView {
    page: Entity<ProcessesPage>,
    driver: Entity<AnimationDriver>,
}

impl ScrubView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        let page = cx.new(|cx| ProcessesPage::new(window, cx));
        Self { page, driver }
    }

    pub fn page(&self) -> &Entity<ProcessesPage> {
        &self.page
    }

    pub fn driver(&self) -> &Entity<AnimationDriver> {
        &self.driver
    }
}

impl Render for ScrubView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.page.clone())
    }
}

/// Initialises the UI over `client` with the fixture settings, History mode and recording on,
/// and opens the scene window at the fixture size.
pub fn open_scene(
    cx: &mut App,
    client: Arc<dyn EngineClient>,
) -> Result<(AnyWindowHandle, Entity<ScrubView>), BenchError> {
    let (mut settings, mut state) = fixture(BenchScene::HistoryScrub);
    settings.history.enabled = true;
    state.processes.history_mode = true;
    // The sender drops at once; tl-app's single-instance requests do not reach a bench run.
    let (_requests, app_requests) = async_channel::bounded(4);
    TasklensUi::init(
        cx,
        client,
        UiInit {
            settings,
            state,
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests,
            startup_toasts: Vec::new(),
            env: AppEnv::test(),
        },
    );
    let bounds = Bounds::centered(None, size(px(WINDOW_W), px(WINDOW_H)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_decorations: Some(WindowDecorations::Client),
        window_background: WindowBackgroundAppearance::Opaque,
        app_id: Some(APP_ID.into()),
        focus: true,
        show: true,
        // As in the other scenes: no 30 fps cap for a window KWin did not activate.
        inactive_frame_interval: None,
        ..Default::default()
    };
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| ScrubView::new(window, cx))
    })
    .map_err(|e| BenchError(format!("cannot open the bench window: {e}")))
}

/// Drives the History strip once per frame, the [`super::scenes::TableScroller`] pattern. The
/// first callback after the first view takes that view's 10-minute strip window. Each later
/// callback moves the scrub point 1 s, wrapping at the window's end; every [`ZOOM_EVERY`] it
/// also sets the strip window to the whole span, and on the next callback back. Every
/// [`HOVER_EVERY`] it moves the pointer to the scrub point over the plot. Each callback also
/// records the UI thread CPU since the previous one when that one changed an input.
pub struct ScrubDriver {
    page: Entity<HistoryPage>,
    clocks: BenchClocks,
    /// The 10-minute strip window and the span, from the first view.
    near: Option<((TimeNs, TimeNs), (TimeNs, TimeNs))>,
    scrub: TimeNs,
    zoomed: bool,
    last_zoom: Option<Instant>,
    last_hover: Option<Instant>,
    /// Thread CPU at the previous callback.
    last_cpu: Option<u64>,
    /// The previous callback changed the scrub point or strip window.
    changed: bool,
    scrub_cpu_ns: Vec<u64>,
    active: bool,
}

impl ScrubDriver {
    pub fn new(page: Entity<HistoryPage>, clocks: BenchClocks) -> Self {
        Self {
            page,
            clocks,
            near: None,
            scrub: 0,
            zoomed: false,
            last_zoom: None,
            last_hover: None,
            last_cpu: None,
            changed: false,
            scrub_cpu_ns: Vec::with_capacity(PROBE_CAPACITY),
            active: false,
        }
    }

    /// UI thread CPU of each frame that followed a scrub point or strip window change, in ns.
    pub fn scrub_cpu_ns(&self) -> &[u64] {
        &self.scrub_cpu_ns
    }

    /// The scrub point last set. 0 until the first view arrived.
    pub fn scrub(&self) -> TimeNs {
        self.scrub
    }

    /// Drops the samples taken so far. The next callback records nothing (fresh baseline).
    pub fn reset(&mut self) {
        self.scrub_cpu_ns.clear();
        self.last_cpu = None;
    }

    /// Starts the per-frame callback.
    pub fn start(this: &Rc<RefCell<Self>>, window: &Window) {
        {
            let mut s = this.borrow_mut();
            if s.active {
                return;
            }
            s.active = true;
            s.last_cpu = None;
        }
        Self::arm(this, window);
    }

    /// The next callback does nothing and does not re-register.
    pub fn stop(&mut self) {
        self.active = false;
    }

    fn arm(this: &Rc<RefCell<Self>>, window: &Window) {
        let driver = Rc::clone(this);
        window.on_next_frame(move |window, cx| {
            let mut s = driver.borrow_mut();
            if !s.active {
                return;
            }
            s.frame(Instant::now(), window, cx);
            drop(s);
            Self::arm(&driver, window);
        });
    }

    fn frame(&mut self, now: Instant, window: &mut Window, cx: &mut App) {
        let cpu = (self.clocks.thread_cpu_ns)();
        if self.changed
            && let Some(at) = self.last_cpu
            && self.scrub_cpu_ns.len() < self.scrub_cpu_ns.capacity()
        {
            self.scrub_cpu_ns.push(cpu.saturating_sub(at));
        }
        self.last_cpu = Some(cpu);
        self.changed = false;

        let Some((near, span)) = self.near.or_else(|| self.first_view(now, cx)) else {
            return;
        };
        self.scrub = if self.scrub + NS_PER_S > near.1 {
            near.0
        } else {
            self.scrub + NS_PER_S
        };
        let zoom = if self.zoomed {
            self.zoomed = false;
            Some(near)
        } else if self
            .last_zoom
            .is_some_and(|at| now.duration_since(at) >= ZOOM_EVERY)
        {
            self.zoomed = true;
            self.last_zoom = Some(now);
            Some(span)
        } else {
            None
        };
        let scrub = self.scrub;
        self.page.update(cx, |page, cx| {
            page.set_scrub(Some(scrub), cx);
            if let Some(view_range) = zoom {
                page.set_view_range(view_range, cx);
            }
        });
        self.changed = true;
        if self
            .last_hover
            .is_some_and(|at| now.duration_since(at) >= HOVER_EVERY)
        {
            self.last_hover = Some(now);
            self.hover(window, cx);
        }
    }

    /// Takes the strip window the page set from its first view with a span.
    fn first_view(
        &mut self,
        now: Instant,
        cx: &App,
    ) -> Option<((TimeNs, TimeNs), (TimeNs, TimeNs))> {
        let page = self.page.read(cx);
        let span = page.view().and_then(|v| v.span)?;
        let near = page.state().view_range;
        if near == (0, 0) {
            return None;
        }
        self.near = Some((near, span));
        self.scrub = near.0;
        self.last_zoom = Some(now);
        self.last_hover = Some(now);
        self.near
    }

    /// A pointer move to the scrub point, halfway down the plot of the last paint.
    fn hover(&self, window: &mut Window, cx: &mut App) {
        let (plot, view) = {
            let page = self.page.read(cx);
            (page.scrubber().plot_bounds, page.state().view_range)
        };
        if plot.size.width <= px(0.) || view.1 <= view.0 {
            return;
        }
        let frac = self.scrub.saturating_sub(view.0) as f64 / (view.1 - view.0) as f64;
        let x = plot.left() + plot.size.width * frac.min(1.0) as f32;
        let y = plot.top() + plot.size.height / 2.;
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(x, y),
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );
    }
}

/// p50, p95, p99 and max in ms.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ScrubPercentiles {
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

/// Nearest-rank percentiles as [`percentiles_ms`], plus p95. All 0.0 for an empty slice.
pub fn scrub_percentiles_ms(ns: &[u64]) -> ScrubPercentiles {
    let mut sorted = ns.to_vec();
    sorted.sort_unstable();
    let rank = |pct: usize| {
        if sorted.is_empty() {
            return 0.0;
        }
        let r = (pct * sorted.len()).div_ceil(100).max(1);
        sorted[r - 1] as f64 / NS_PER_MS
    };
    ScrubPercentiles {
        p50: rank(50),
        p95: rank(95),
        p99: rank(99),
        max: rank(100),
    }
}

/// The `history-scrub` JSON: the ADR-014 fields, then the scrub frame times.
#[derive(Serialize)]
struct ScrubReport<'a> {
    #[serde(flatten)]
    base: &'a BenchReport,
    scrub_frame_ms: ScrubPercentiles,
    scrub_frames: u64,
}

/// Runs the scene: opens the window, drives the strip from the start of the warm-up, measures
/// for the duration, writes the JSON when asked and prints it on one line. Returns the ADR-014
/// part of the report.
pub fn run(
    cx: &mut App,
    opts: BenchOptions,
    clocks: BenchClocks,
) -> Task<Result<BenchReport, BenchError>> {
    let engine = ScrubEngine::start(SEED);
    let (handle, view) = match open_scene(cx, engine.clone()) {
        Ok(window) => window,
        Err(e) => return Task::ready(Err(e)),
    };
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = measure(cx, &opts, clocks, handle, &view).await;
        engine.stop();
        let (report, scrub_ns) = result?;
        let scrub = ScrubReport {
            base: &report,
            scrub_frame_ms: scrub_percentiles_ms(&scrub_ns),
            scrub_frames: scrub_ns.len() as u64,
        };
        if let Some(path) = &opts.json {
            let write = || -> std::io::Result<()> {
                let mut json = serde_json::to_vec_pretty(&scrub)?;
                json.push(b'\n');
                std::fs::write(path, json)
            };
            write().map_err(|e| BenchError(format!("cannot write {}: {e}", path.display())))?;
        }
        let line = serde_json::to_string(&scrub)
            .map_err(|e| BenchError(format!("cannot serialize the report: {e}")))?;
        // The one stdout line of a bench run, read by `cargo xtask bench`.
        #[allow(
            clippy::print_stdout,
            reason = "the bench report line (Cargo.toml lint note)"
        )]
        {
            println!("{line}");
        }
        Ok(report)
    })
}

/// Warm-up, baselines, the measured window and the report, with the scrub frame samples.
async fn measure(
    cx: &mut AsyncApp,
    opts: &BenchOptions,
    clocks: BenchClocks,
    handle: AnyWindowHandle,
    view: &Entity<ScrubView>,
) -> Result<(BenchReport, Vec<u64>), BenchError> {
    let name = BenchScene::HistoryScrub.as_str();
    let closed = |_| BenchError("the bench window closed".to_owned());
    let (anim, page) = cx.update(|cx| {
        let view = view.read(cx);
        let page = view.page.read(cx).history_page().cloned();
        (view.driver.clone(), page)
    });
    let page = page.ok_or_else(|| BenchError(format!("{name}: no History page")))?;
    let stalls = |cx: &mut AsyncApp| cx.update(|cx| anim.read(cx).stats.stalls);
    let at_start = stalls(cx);

    let probe = Rc::new(RefCell::new(FrameProbe::new(clocks, PROBE_CAPACITY)));
    let driver = Rc::new(RefCell::new(ScrubDriver::new(page, clocks)));
    {
        let (probe, driver) = (Rc::clone(&probe), Rc::clone(&driver));
        handle
            .update(cx, |_, window, _| {
                FrameProbe::start(probe, window);
                // The strip moves from the start of the warm-up.
                ScrubDriver::start(&driver, window);
            })
            .map_err(closed)?;
    }

    let executor = cx.background_executor().clone();
    executor
        .timer(Duration::from_secs(u64::from(opts.warmup_s)))
        .await;

    probe.borrow_mut().reset();
    driver.borrow_mut().reset();
    let at_measure = stalls(cx);
    let is_active = |cx: &mut AsyncApp| {
        handle
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap_or(false)
    };
    let active_at = is_active(cx);
    let cpu_at = (clocks.process_cpu_ns)();
    let wall_at = Instant::now();

    executor
        .timer(Duration::from_secs(u64::from(opts.duration_s)))
        .await;

    let cpu_ns = (clocks.process_cpu_ns)().saturating_sub(cpu_at);
    let wall_ns = u64::try_from(wall_at.elapsed().as_nanos()).unwrap_or(u64::MAX);
    probe.borrow_mut().stop();
    driver.borrow_mut().stop();
    let at_end = stalls(cx);
    let window_active = active_at && is_active(cx);

    let probe = probe.borrow();
    let intervals = probe.intervals_ns();
    let frames = intervals.len() as u64;
    if frames == 0 {
        return Err(BenchError(format!(
            "{name}: 0 frames drawn; possible frame-callback stall"
        )));
    }
    let scrub_ns = driver.borrow().scrub_cpu_ns().to_vec();
    if scrub_ns.is_empty() {
        return Err(BenchError(format!(
            "{name}: no scrub frame; the first history view never arrived"
        )));
    }
    let (missed, long_missed) = count_missed(intervals);
    let (rss_mb, pss_mb) = read_rss_pss_mb();
    let report = BenchReport {
        scene: name,
        frames,
        frame_cpu_ms: percentiles_ms(probe.cpu_ns()),
        missed_frames: missed,
        long_missed_frames: long_missed,
        process_cpu_pct: process_cpu_pct(cpu_ns, wall_ns),
        probe_cpu_pct: None,
        rss_mb,
        pss_mb,
        stalls: u32::try_from(at_end.saturating_sub(at_measure)).unwrap_or(u32::MAX),
        warmup_stalls: u32::try_from(at_measure.saturating_sub(at_start)).unwrap_or(u32::MAX),
        window_active,
        bridge_latency_ms: None,
        gpu: None,
    };
    Ok((report, scrub_ns))
}
