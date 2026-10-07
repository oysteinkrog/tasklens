//! Bench scenes `perf-cpu`, `static`, `bridge` and `table-2000` and the run loop (ADR-014,
//! UI-PRF-053, E01-007, E01-008).
//!
//! A scene opens one window with the production Performance tab, CPU page selected, fed by
//! [`BenchEngine`]. `table-2000` opens the production Processes tab instead, over the
//! [`super::synth::TableSynth`] table, and scrolls it with [`TableScroller`]. `soak` (E01-015,
//! ADR-010 step 2) opens the `perf-cpu` window plus two [`SoakGraphView`] windows, fed once per
//! second, sends no input and counts the stalls of all three `AnimationDriver`s. [`run`] waits out the warm-up, takes its baselines, measures for the
//! duration, builds the [`BenchReport`], writes the JSON when asked and prints it on one line;
//! [`run_app`] then quits. Nothing runs while measuring but the scene's own views: one timer waits out the
//! duration, and the frame probe records each frame.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity, IntoElement,
    ParentElement as _, QuitMode, Render, Styled as _, Task, TitlebarOptions,
    UniformListScrollHandle, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations,
    WindowOptions, canvas, div, point, px, size,
};
use rustc_hash::FxHashSet;
use smallvec::{SmallVec, smallvec};
use tl_model::client::EngineClient;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::{Settings, UiState};
use tl_model::time::now_boot_ns;
use tracing::info;

use super::engine::{BenchEngine, BenchFeed, DEV_MEMORY, DEV_PACKAGE, MEM_TOTAL_BYTES};
use super::stats::{
    BenchClocks, BenchReport, FrameProbe, LatencyPct, PROBE_CAPACITY, count_missed, percentiles_ms,
    process_cpu_pct, read_rss_pss_mb, write_json,
};
use super::synth::SEED;
use crate::anim::{AnimationDriver, PaintRecord};
use crate::app::{AppEnv, TasklensUi, UiInit};
use crate::chart::view::ChartView;
use crate::chart::{
    ChartLabels, ChartSeries, ChartSpec, GridStyle, ValueAlign, ValueFormat, YScale,
};
use crate::components::icons::TlAssets;
use crate::demand::GlobalDemand;
use crate::hub::GlobalHub;
use crate::pages::performance::cpu::CpuPage;
use crate::pages::performance::{DeviceKey, PerformancePage};
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::delegate::ProcessDelegate;
use crate::table::tree_table::TlTreeTable;
use crate::theme::current;
use crate::window::main::APP_ID;

/// Phase-0 fixture window size, logical px.
pub const WINDOW_W: f32 = 1600.;
pub const WINDOW_H: f32 = 1000.;
/// UI-PRF-053: `bridge` needs this many real samples in 30 s.
pub const BRIDGE_MIN_SAMPLES_30S: u64 = 295;
/// Soak graph windows (E01-015), logical px.
pub const SOAK_GRAPH_W: f32 = 600.;
pub const SOAK_GRAPH_H: f32 = 300.;
pub const SOAK_TITLES: [&str; 2] = ["tasklens Soak Graph 1", "tasklens Soak Graph 2"];
/// ADR-014 `table-2000` scroll speed, logical px per second.
pub const TABLE_SCROLL_PX_PER_S: f32 = 1_200.;

/// A bench scene (APP-052 `--bench <scene>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchScene {
    PerfCpu,
    Table2000,
    Bridge,
    Static,
    Soak,
}

impl BenchScene {
    pub const ALL: [BenchScene; 5] = [
        Self::PerfCpu,
        Self::Table2000,
        Self::Bridge,
        Self::Static,
        Self::Soak,
    ];

    /// "perf-cpu", "table-2000", "bridge", "static" or "soak".
    pub fn from_arg(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scene| scene.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PerfCpu => "perf-cpu",
            Self::Table2000 => "table-2000",
            Self::Bridge => "bridge",
            Self::Static => "static",
            Self::Soak => "soak",
        }
    }
}

/// `--duration`, `--warmup` and `--json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchOptions {
    pub duration_s: u32,
    pub warmup_s: u32,
    pub json: Option<PathBuf>,
}

impl Default for BenchOptions {
    fn default() -> Self {
        Self {
            duration_s: 30,
            warmup_s: 5,
            json: None,
        }
    }
}

impl BenchOptions {
    /// The defaults of `scene`: 600 s with no warm-up for `soak` (R-160 OI-12), else 30 s and
    /// 5 s (ADR-014).
    pub fn for_scene(scene: BenchScene) -> Self {
        match scene {
            BenchScene::Soak => Self {
                duration_s: 600,
                warmup_s: 0,
                json: None,
            },
            _ => Self::default(),
        }
    }
}

/// Why a scene failed. tl-app prints it and exits 1 (E01-009).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchError(pub String);

impl fmt::Display for BenchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BenchError {}

/// The fixture settings of `scene` (UI-PRF-053 common fixture): Default Dark, 100% UI scale,
/// 60 s graph range, kernel overlays on, smooth graphs except in `static`, CPU page selected
/// with the sidebar at 252 px. `table-2000` shows the Power column after R-006's nine default
/// process columns (R-160 OI-7).
pub fn fixture(scene: BenchScene) -> (Settings, UiState) {
    // The default theme is Default Dark without follow-system, so `init` activates it with no
    // crossfade.
    let mut settings = Settings::default();
    settings.appearance.ui_scale_pct = 100;
    settings.motion.smooth_graphs = scene != BenchScene::Static;
    settings.performance.graph_range_s = 60;
    settings.performance.cpu_kernel = true;
    let mut state = UiState::default();
    state.performance.nav_width = 252;
    state.performance.selected_device = DeviceKey::CPU.to_owned();
    if scene == BenchScene::Table2000 {
        let columns = &mut state.columns.processes;
        for list in [&mut columns.order, &mut columns.visible] {
            if !list.iter().any(|c| c == "power") {
                list.push("power".to_owned());
            }
        }
    }
    (settings, state)
}

/// The tab a scene window shows.
pub enum SceneContent {
    Performance(Entity<PerformancePage>),
    Processes(Entity<ProcessesPage>),
}

/// The scene window's root: the scene's tab and the window's animation driver.
pub struct SceneView {
    content: SceneContent,
    driver: Entity<AnimationDriver>,
    /// Window paints, counted by an empty canvas at the root. `static` reports these as its
    /// frames: it redraws once per sample but never starts the frame probe.
    paints: Rc<Cell<u64>>,
}

impl SceneView {
    fn new(scene: BenchScene, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        if scene == BenchScene::Table2000 {
            let page = cx.new(|cx| ProcessesPage::new(window, cx));
            page.update(cx, |page, cx| page.focus_table(window, cx));
            return Self {
                content: SceneContent::Processes(page),
                driver,
                paints: Rc::default(),
            };
        }
        let hub = cx.global::<GlobalHub>().0.clone();
        let demand = cx.global::<GlobalDemand>().0.clone();
        // Hidden-state demand follows this window as it follows the main window.
        let handle = window.window_handle();
        demand.update(cx, |d, _| d.set_main_window(handle));
        let page = cx.new(|cx| PerformancePage::new(hub, demand, cx));
        let cpu = page
            .read(cx)
            .page(&DeviceKey(Arc::from(DeviceKey::CPU)))
            .and_then(|view| view.clone().downcast::<CpuPage>().ok());
        if let Some(cpu) = cpu {
            cpu.update(cx, |cpu, cx| {
                cpu.bench_show_both = true;
                cx.notify();
            });
        }
        let focus = page.read(cx).primary_focus();
        focus.focus(window, cx);
        Self {
            content: SceneContent::Performance(page),
            driver,
            paints: Rc::default(),
        }
    }

    pub fn content(&self) -> &SceneContent {
        &self.content
    }

    /// The Performance tab, in every scene but `table-2000`.
    pub fn page(&self) -> Option<&Entity<PerformancePage>> {
        match &self.content {
            SceneContent::Performance(page) => Some(page),
            SceneContent::Processes(_) => None,
        }
    }

    /// The Processes tab, in `table-2000` only.
    pub fn processes(&self) -> Option<&Entity<ProcessesPage>> {
        match &self.content {
            SceneContent::Processes(page) => Some(page),
            SceneContent::Performance(_) => None,
        }
    }

    pub fn driver(&self) -> &Entity<AnimationDriver> {
        &self.driver
    }

    /// Window paints since the window opened.
    pub fn paints(&self) -> u64 {
        self.paints.get()
    }
}

impl Render for SceneView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let paints = Rc::clone(&self.paints);
        let counter = canvas(|_, _, _| {}, move |_, _, _, _| paints.set(paints.get() + 1))
            .absolute()
            .size_0();
        let root = div().size_full().child(counter);
        match &self.content {
            SceneContent::Performance(page) => root.child(page.clone()),
            SceneContent::Processes(page) => root.child(page.clone()),
        }
    }
}

/// A `soak` graph window: one chart of one 1 Hz series and the window's own driver.
pub struct SoakGraphView {
    chart: Entity<ChartView>,
    driver: Entity<AnimationDriver>,
}

impl SoakGraphView {
    /// Graph 0 is the CPU total, graph 1 Memory used.
    fn new(graph: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        let hub = cx.global::<GlobalHub>().0.clone();
        let theme = current(cx).clone();
        let (device, metric, fill, line, y, value_fmt) = if graph == 0 {
            (
                DEV_PACKAGE,
                MetricId::CPU_TOTAL,
                theme.graph.cpu_fill,
                theme.graph.cpu_line,
                YScale::Fixed {
                    min: 0.0,
                    max: 100.0,
                },
                ValueFormat::Percent,
            )
        } else {
            (
                DEV_MEMORY,
                MetricId::MEM_USED,
                theme.graph.mem_fill,
                theme.graph.mem_line,
                YScale::FixedTotal(MEM_TOTAL_BYTES as f32),
                ValueFormat::Bytes,
            )
        };
        let spec = ChartSpec {
            series: smallvec![ChartSeries {
                id: SeriesId {
                    device: DeviceIdx(device),
                    metric,
                },
                fill: Some(fill),
                line,
                ..ChartSeries::default()
            }],
            y,
            grid: GridStyle {
                lines: true,
                outline: true,
                ..GridStyle::default()
            },
            smooth: true,
            ..ChartSpec::default()
        };
        let labels = ChartLabels {
            heading: None,
            value_fmt,
            value_align: ValueAlign::Right,
            range_word: "".into(),
            legend: SmallVec::new(),
            show_axis_max: false,
        };
        let chart = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));
        Self { chart, driver }
    }

    pub fn chart(&self) -> &Entity<ChartView> {
        &self.chart
    }

    pub fn driver(&self) -> &Entity<AnimationDriver> {
        &self.driver
    }
}

impl Render for SoakGraphView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_2()
            .bg(current(cx).window_bg)
            .child(self.chart.clone())
    }
}

/// An open scene window, and in `soak` its two graph windows.
pub struct SceneWindow {
    pub handle: AnyWindowHandle,
    pub view: Entity<SceneView>,
    pub graphs: Vec<(AnyWindowHandle, Entity<SoakGraphView>)>,
}

impl SceneWindow {
    /// Every window's driver: the scene window's first, then the graph windows'.
    pub fn drivers(&self, cx: &App) -> SmallVec<[Entity<AnimationDriver>; 3]> {
        let mut out = SmallVec::new();
        out.push(self.view.read(cx).driver.clone());
        out.extend(self.graphs.iter().map(|(_, g)| g.read(cx).driver.clone()));
        out
    }
}

/// Initialises the UI over `client` with the fixture settings of `scene` and opens the scene
/// window. Only the scene's own tab is built.
pub fn open_scene(
    cx: &mut App,
    scene: BenchScene,
    client: Arc<dyn EngineClient>,
) -> Result<SceneWindow, BenchError> {
    let (settings, state) = fixture(scene);
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
        ..Default::default()
    };
    let (handle, view) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| SceneView::new(scene, window, cx))
    })
    .map_err(|e| BenchError(format!("cannot open the bench window: {e}")))?;
    let mut graphs = Vec::new();
    if scene == BenchScene::Soak {
        for (graph, title) in SOAK_TITLES.into_iter().enumerate() {
            let bounds = Bounds::centered(None, size(px(SOAK_GRAPH_W), px(SOAK_GRAPH_H)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                window_decorations: Some(WindowDecorations::Client),
                window_background: WindowBackgroundAppearance::Opaque,
                app_id: Some(APP_ID.into()),
                focus: false,
                show: true,
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| SoakGraphView::new(graph, window, cx))
            })
            .map_err(|e| BenchError(format!("cannot open {title}: {e}")))?;
            graphs.push(opened);
        }
    }
    Ok(SceneWindow {
        handle,
        view,
        graphs,
    })
}

/// Runs `scene` and returns its report. Called by tl-app inside `Application::run`. `real` is
/// `Some` only for `bridge`. On success the report is printed as one JSON line; on failure
/// tl-app prints the error. The caller quits the app ([`run_app`] does).
pub fn run(
    cx: &mut App,
    scene: BenchScene,
    opts: BenchOptions,
    clocks: BenchClocks,
    real: Option<Arc<dyn EngineClient>>,
) -> Task<Result<BenchReport, BenchError>> {
    let feed = match (scene, real) {
        (BenchScene::Table2000, _) => BenchFeed::Table,
        (BenchScene::Soak, _) => BenchFeed::Soak,
        (BenchScene::Bridge, Some(real)) => BenchFeed::Bridge(real),
        (BenchScene::Bridge, None) => {
            return Task::ready(Err(BenchError("bridge: no real engine client".to_owned())));
        }
        _ => BenchFeed::Synthetic,
    };
    let engine = BenchEngine::start(feed, SEED, now_boot_ns());
    let window = match open_scene(cx, scene, engine.clone()) {
        Ok(window) => window,
        Err(e) => return Task::ready(Err(e)),
    };
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = measure(cx, scene, &opts, clocks, &engine, &window).await;
        engine.stop();
        let report = result?;
        if let Some(path) = &opts.json {
            write_json(&report, path)
                .map_err(|e| BenchError(format!("cannot write {}: {e}", path.display())))?;
        }
        let line = serde_json::to_string(&report)
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

/// tl-app's `--bench` entry (E01-009): runs `scene` in its own GPUI application, built as
/// [`crate::run`] builds it (the bundled assets, explicit quit) but with no main window, and
/// returns once the application has quit. tl-app has no gpui-kit dependency (E33-OI-1).
pub fn run_app(
    scene: BenchScene,
    opts: BenchOptions,
    clocks: BenchClocks,
    real: Option<Arc<dyn EngineClient>>,
) -> Result<BenchReport, BenchError> {
    let outcome: Rc<RefCell<Option<Result<BenchReport, BenchError>>>> = Rc::default();
    let slot = Rc::clone(&outcome);
    gpui_kit::application()
        .with_assets(TlAssets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            let task = run(cx, scene, opts, clocks, real);
            cx.spawn(async move |cx: &mut AsyncApp| {
                // Store the result first: `Application::run` returns as soon as the app quits.
                *slot.borrow_mut() = Some(task.await);
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    outcome.borrow_mut().take().unwrap_or_else(|| {
        Err(BenchError(
            "the bench app quit before the scene finished".to_owned(),
        ))
    })
}

/// Warm-up, baselines, the measured window and the report.
async fn measure(
    cx: &mut AsyncApp,
    scene: BenchScene,
    opts: &BenchOptions,
    clocks: BenchClocks,
    engine: &BenchEngine,
    window: &SceneWindow,
) -> Result<BenchReport, BenchError> {
    let closed = |_| BenchError("the bench window closed".to_owned());
    let bridge = scene == BenchScene::Bridge;
    let soak = scene == BenchScene::Soak;
    let driver = cx.update(|cx| window.view.read(cx).driver.clone());
    // One driver per window; only `soak` has more than one.
    let drivers = cx.update(|cx| window.drivers(cx));
    let counts = |cx: &mut AsyncApp| -> SmallVec<[(u64, u64); 3]> {
        cx.update(|cx| {
            drivers
                .iter()
                .map(|d| {
                    let stats = &d.read(cx).stats;
                    (stats.stalls, stats.frames)
                })
                .collect()
        })
    };

    // Stalls during the warm-up are reported apart from the measured ones, never dropped.
    let at_start = counts(cx);

    // The probe arms `on_next_frame` every frame, so `static` runs without it, and `soak` too:
    // forced frames would hide the zed#64499 stall it looks for.
    let probe = Rc::new(RefCell::new(FrameProbe::new(clocks, PROBE_CAPACITY)));
    if scene != BenchScene::Static && !soak {
        let probe = Rc::clone(&probe);
        window
            .handle
            .update(cx, |_, window, _| FrameProbe::start(probe, window))
            .map_err(closed)?;
    }
    // table-2000 scrolls from the start of the warm-up, so the measured window starts mid-scroll.
    let scroller = if scene == BenchScene::Table2000 {
        let view = window.view.clone();
        let scroller = window
            .handle
            .update(cx, |_, window, cx| {
                let page = view.read(cx).processes().cloned();
                page.map(|page| {
                    let scroller = Rc::new(RefCell::new(TableScroller::new(&page, cx)));
                    TableScroller::start(&scroller, window);
                    scroller
                })
            })
            .map_err(closed)?;
        Some(scroller.ok_or_else(|| BenchError("table-2000: no process table".to_owned()))?)
    } else {
        None
    };

    let executor = cx.background_executor().clone();
    executor
        .timer(Duration::from_secs(u64::from(opts.warmup_s)))
        .await;

    // Baselines, taken after the warm-up.
    probe.borrow_mut().reset();
    if bridge {
        cx.update(|cx| driver.update(cx, |d, _| d.install_paint_log()));
    }
    let at_measure = counts(cx);
    let paints_at = cx.update(|cx| window.view.read(cx).paints());
    let samples_at = engine.bridge_samples();
    let cpu_at = (clocks.process_cpu_ns)();
    let wall_at = Instant::now();

    executor
        .timer(Duration::from_secs(u64::from(opts.duration_s)))
        .await;

    let cpu_ns = (clocks.process_cpu_ns)().saturating_sub(cpu_at);
    let wall_ns = u64::try_from(wall_at.elapsed().as_nanos()).unwrap_or(u64::MAX);
    probe.borrow_mut().stop();
    if let Some(scroller) = &scroller {
        scroller.borrow_mut().stop();
    }
    let samples = engine.bridge_samples().saturating_sub(samples_at);
    let (requested, paint_log) =
        cx.update(|cx| driver.update(cx, |d, _| (d.stats.requested, d.take_paint_log())));
    let at_end = counts(cx);
    let paints = cx
        .update(|cx| window.view.read(cx).paints())
        .saturating_sub(paints_at);
    let delta = |to: &[(u64, u64)], from: &[(u64, u64)]| -> (u64, u64) {
        to.iter().zip(from).fold((0, 0), |(s, f), (t, b)| {
            (s + t.0.saturating_sub(b.0), f + t.1.saturating_sub(b.1))
        })
    };
    let (stalls, driver_frames) = delta(&at_end, &at_measure);
    let (warmup_stalls, _) = delta(&at_measure, &at_start);
    if soak {
        for (n, (end, from)) in at_end.iter().zip(&at_measure).enumerate() {
            info!(
                target: "tasklens::perf",
                window = n + 1,
                stalls = end.0.saturating_sub(from.0),
                frames = end.1.saturating_sub(from.1),
                "soak window"
            );
        }
    }

    if scene == BenchScene::Static && requested != 0 {
        return Err(BenchError(
            "static scene requested animation frames".to_owned(),
        ));
    }
    if bridge {
        let needed = BRIDGE_MIN_SAMPLES_30S * u64::from(opts.duration_s) / 30;
        if samples < needed {
            return Err(BenchError(format!(
                "bridge: only {samples} samples arrived"
            )));
        }
    }

    let probe = probe.borrow();
    let intervals = probe.intervals_ns();
    // The probe's frames; in `soak` the drivers' frame callbacks; in `static`, which runs
    // without the probe, the window's paints.
    let frames = if soak {
        driver_frames
    } else if scene == BenchScene::Static {
        paints
    } else {
        intervals.len() as u64
    };
    // A moving scene with no frame callback at all (a locked screen, an occluded window or the
    // zed#64499 stall) must not pass as a report.
    if scene != BenchScene::Static && frames == 0 {
        return Err(BenchError(format!(
            "{}: 0 frames drawn; possible frame-callback stall",
            scene.as_str()
        )));
    }
    let (missed, long_missed) = count_missed(intervals);
    let (rss_mb, pss_mb) = read_rss_pss_mb();
    let bridge_latency_ms = paint_log
        .filter(|_| bridge)
        .and_then(|log| bridge_latency(&log.records, engine));
    Ok(BenchReport {
        scene: scene.as_str(),
        frames,
        frame_cpu_ms: percentiles_ms(probe.cpu_ns()),
        missed_frames: missed,
        long_missed_frames: long_missed,
        process_cpu_pct: process_cpu_pct(cpu_ns, wall_ns),
        probe_cpu_pct: None,
        rss_mb,
        pss_mb,
        stalls: u32::try_from(stalls).unwrap_or(u32::MAX),
        warmup_stalls: u32::try_from(warmup_stalls).unwrap_or(u32::MAX),
        bridge_latency_ms,
        gpu: None,
    })
}

/// The `table-2000` scroll position: [`TABLE_SCROLL_PX_PER_S`] down to the bottom, then back up
/// to the top, and so on. `offset` is in logical px from the top, so it is `-y` of GPUI's scroll
/// offset (which is negative downward).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TableScroll {
    pub offset: f32,
    pub up: bool,
}

impl TableScroll {
    /// Moves by `TABLE_SCROLL_PX_PER_S * dt`, clamped to `[0, max]`, and turns at either end.
    /// Returns the new offset.
    pub fn advance(&mut self, dt_ns: u64, max: f32) -> f32 {
        let max = max.max(0.);
        let step = TABLE_SCROLL_PX_PER_S * (dt_ns as f64 / 1e9) as f32;
        let next = if self.up {
            self.offset - step
        } else {
            self.offset + step
        };
        self.offset = if next >= max {
            self.up = true;
            max
        } else if next <= 0. {
            self.up = false;
            0.
        } else {
            next
        };
        self.offset
    }
}

/// Scrolls the production process table once per frame (ADR-014 `table-2000`). Each frame it
/// sets the offset of the table's `uniform_list` scroll handle and notifies the table, which is
/// what a mouse-wheel scroll does. Like [`FrameProbe`], it runs from its own `on_next_frame`
/// callback and re-registers while active.
pub struct TableScroller {
    handle: UniformListScrollHandle,
    table: Entity<TlTreeTable<ProcessDelegate>>,
    scroll: TableScroll,
    last: Option<Instant>,
    active: bool,
}

impl TableScroller {
    /// Takes the table's scroll handle once, through `ProcessesPage::scroll_handle`.
    pub fn new(page: &Entity<ProcessesPage>, cx: &App) -> Self {
        let page = page.read(cx);
        Self {
            handle: page.scroll_handle(cx),
            table: page.table().clone(),
            scroll: TableScroll::default(),
            last: None,
            active: false,
        }
    }

    pub fn scroll(&self) -> TableScroll {
        self.scroll
    }

    /// One frame `dt_ns` after the previous one: the next offset, one `set_offset` and one
    /// notify of the table. `max` is the list's content height minus its viewport height, from
    /// its last layout.
    pub fn frame(&mut self, dt_ns: u64, cx: &mut App) {
        let base = self.handle.0.borrow().base_handle.clone();
        let max = f32::from(base.max_offset().y);
        let y = self.scroll.advance(dt_ns, max);
        base.set_offset(point(base.offset().x, px(-y)));
        self.table.update(cx, |_, cx| cx.notify());
    }

    /// Starts the per-frame callback. The first frame only takes the time.
    pub fn start(this: &Rc<RefCell<Self>>, window: &Window) {
        {
            let mut s = this.borrow_mut();
            if s.active {
                return;
            }
            s.active = true;
            s.last = None;
        }
        Self::arm(this, window);
    }

    /// The next callback does nothing and does not re-register.
    pub fn stop(&mut self) {
        self.active = false;
    }

    fn arm(this: &Rc<RefCell<Self>>, window: &Window) {
        let scroller = Rc::clone(this);
        window.on_next_frame(move |window, cx| {
            let mut s = scroller.borrow_mut();
            if !s.active {
                return;
            }
            let now = Instant::now();
            if let Some(last) = s.last {
                let dt = u64::try_from(now.duration_since(last).as_nanos()).unwrap_or(u64::MAX);
                s.frame(dt, cx);
            }
            s.last = Some(now);
            drop(s);
            Self::arm(&scroller, window);
        });
    }
}

/// Wake-to-render latency (UI-PRF-053): for each real sample, `published_ns` of the snapshot
/// that first carried it to the first paint whose newest sample is that one. Paints of
/// synthetic series have no bridge origin and are skipped. `None` when no real sample was
/// painted.
fn bridge_latency(records: &[PaintRecord], engine: &BenchEngine) -> Option<LatencyPct> {
    let mut seen = FxHashSet::default();
    let mut latencies = Vec::new();
    for r in records {
        if seen.contains(&r.newest_ns) {
            continue;
        }
        if let Some(published) = engine.bridge_origin(r.newest_ns) {
            seen.insert(r.newest_ns);
            latencies.push(r.painted_at_ns.saturating_sub(published));
        }
    }
    if latencies.is_empty() {
        return None;
    }
    let p = percentiles_ms(&latencies);
    Some(LatencyPct {
        p50: p.p50,
        p99: p.p99,
    })
}
