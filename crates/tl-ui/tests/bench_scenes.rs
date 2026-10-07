//! Bench scenes and the run loop (E01-007, UI-PRF-053). Run with `--features bench`.
#![cfg(feature = "bench")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{TestAppContext, VisualTestContext};
use tl_model::client::EngineClient;
use tl_model::ids::{NodeId, ProcessKey};
use tl_model::table::ProcessTableView;
use tl_model::time::now_boot_ns;
use tl_ui::anim::AnimationDriver;
use tl_ui::bench::engine::{BenchEngine, BenchFeed};
use tl_ui::bench::scenes::{SOAK_TITLES, TABLE_SCROLL_PX_PER_S, TableScroller, open_scene};
use tl_ui::bench::stats::{BenchReport, OUTPUT_PERIOD_NS};
use tl_ui::bench::synth::{
    BenchRng, SEED, TABLE_GROUP_SIZE, TABLE_GROUPS, TABLE_PROCESSES, TableSynth,
};
use tl_ui::bench::{BenchClocks, BenchError, BenchOptions, BenchScene, run};
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::cpu::CpuPage;
use tl_ui::testing::fake_engine::FakeEngine;

static FAKE_CPU: AtomicU64 = AtomicU64::new(0);

fn fake_cpu() -> u64 {
    FAKE_CPU.fetch_add(1_000, Ordering::Relaxed)
}

fn clocks() -> BenchClocks {
    BenchClocks {
        thread_cpu_ns: fake_cpu,
        process_cpu_ns: fake_cpu,
    }
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

type Outcome = Rc<RefCell<Option<Result<BenchReport, BenchError>>>>;

/// Runs `scene` for `duration_s` simulated seconds with no warm-up and returns its result. With
/// `frames`, the scene window gets one simulated frame every 100 ms; without, no frame callback
/// ever runs (a locked screen).
fn run_scene(
    cx: &mut TestAppContext,
    scene: BenchScene,
    duration_s: u32,
    json: Option<std::path::PathBuf>,
    real: Option<Arc<dyn EngineClient>>,
    frames: bool,
) -> Result<BenchReport, BenchError> {
    cx.executor().allow_parking();
    let outcome: Outcome = Rc::default();
    let slot = Rc::clone(&outcome);
    cx.update(|cx| {
        let opts = BenchOptions {
            duration_s,
            warmup_s: 0,
            json,
        };
        let task = run(cx, scene, opts, clocks(), real);
        cx.spawn(async move |_| *slot.borrow_mut() = Some(task.await))
            .detach();
    });
    let window = cx.update(|cx| cx.windows().last().copied());
    let mut vcx = window.map(|w| VisualTestContext::from_window(w, cx));
    for _ in 0..=duration_s * 10 {
        // Stop the clock once the scene has its result, so no timer runs after it.
        if outcome.borrow().is_some() {
            break;
        }
        match vcx.as_mut() {
            Some(vcx) => {
                vcx.run_until_parked();
                if frames {
                    vcx.update(|window, cx| {
                        window.simulate_next_frame(cx);
                        window.draw(cx).clear(cx);
                    });
                }
                vcx.executor().advance_clock(Duration::from_millis(100));
            }
            None => {
                cx.run_until_parked();
                cx.executor().advance_clock(Duration::from_millis(100));
            }
        }
    }
    match vcx.as_mut() {
        Some(vcx) => vcx.run_until_parked(),
        None => cx.run_until_parked(),
    }
    outcome.borrow_mut().take().expect("the scene finished")
}

#[test]
fn scene_names_round_trip() {
    for scene in BenchScene::ALL {
        assert_eq!(BenchScene::from_arg(scene.as_str()), Some(scene));
    }
    assert_eq!(
        BenchScene::from_arg("table-2000"),
        Some(BenchScene::Table2000)
    );
    assert_eq!(BenchScene::from_arg("soak"), Some(BenchScene::Soak));
    assert_eq!(BenchScene::from_arg("charts"), None);
    let opts = BenchOptions::default();
    assert_eq!((opts.duration_s, opts.warmup_s, opts.json), (30, 5, None));
    let soak = BenchOptions::for_scene(BenchScene::Soak);
    assert_eq!((soak.duration_s, soak.warmup_s), (600, 0));
    assert_eq!(
        BenchOptions::for_scene(BenchScene::PerfCpu),
        BenchOptions::default()
    );
}

#[gpui_kit::test]
fn bench_scene_perf_cpu_builds_32_cells_and_9_cards(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let engine = BenchEngine::start(BenchFeed::Synthetic, SEED, now_boot_ns());
    let window = cx
        .update(|cx| open_scene(cx, BenchScene::PerfCpu, engine.clone()))
        .expect("the scene window opens");
    let mut vcx = VisualTestContext::from_window(window.handle, cx);
    draw(&mut vcx);

    let view = window.view.clone();
    let (cells, cards, both, selected) = vcx.update(|_, cx| {
        let page = view.read(cx).page().expect("the Performance tab").read(cx);
        let cpu = page
            .page(&DeviceKey(Arc::from(DeviceKey::CPU)))
            .expect("CPU page built")
            .clone()
            .downcast::<CpuPage>()
            .expect("a CpuPage")
            .read(cx);
        (
            cpu.grid().view.read(cx).spec.cells.len(),
            page.sidebar().read(cx).cards().len(),
            cpu.bench_show_both,
            page.selected().map(|k| k.0.to_string()),
        )
    });
    assert_eq!(cells, 32, "8 x 4 per-core grid");
    assert_eq!(
        cards, 9,
        "CPU, Memory, GPU 0, Disk 0 to 3, enp38s0, tailscale0"
    );
    assert!(both, "overall chart above the grid");
    assert_eq!(selected.as_deref(), Some(DeviceKey::CPU));
    engine.stop();
}

#[gpui_kit::test]
fn static_scene_requests_no_frames(cx: &mut TestAppContext) {
    // The run fails if the driver requested a frame; `frames` counts the window's paints.
    let report = run_scene(cx, BenchScene::Static, 2, None, None, true).expect("static passes");
    assert_eq!(report.scene, "static");
    assert!(
        (15..=25).contains(&report.frames),
        "one paint per simulated 100 ms frame over 2 s: {}",
        report.frames
    );
}

#[gpui_kit::test]
fn static_scene_counts_no_paints_without_frames(cx: &mut TestAppContext) {
    let report = run_scene(cx, BenchScene::Static, 1, None, None, false).expect("static passes");
    assert_eq!(report.frames, 0);
}

#[gpui_kit::test]
fn json_written_when_requested(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("perf-cpu.json");
    let report = run_scene(cx, BenchScene::PerfCpu, 1, Some(path.clone()), None, true)
        .expect("perf-cpu passes");
    assert_eq!(report.scene, "perf-cpu");
    let text = std::fs::read_to_string(&path).expect("the JSON file exists");
    let json: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(json["scene"], "perf-cpu");
    assert!(json["bridge_latency_ms"].is_null());
    assert!(json["gpu"].is_null());
    assert!(report.frames > 0, "simulated frames were recorded");
    assert_eq!(json["warmup_stalls"], 0);
}

#[gpui_kit::test]
fn moving_scene_without_frames_fails(cx: &mut TestAppContext) {
    let result = run_scene(cx, BenchScene::PerfCpu, 1, None, None, false);
    assert_eq!(
        result.map(|r| r.scene).map_err(|e| e.to_string()),
        Err("perf-cpu: 0 frames drawn; possible frame-callback stall".to_owned())
    );
}

#[gpui_kit::test]
fn bridge_fails_without_samples(cx: &mut TestAppContext) {
    let real: Arc<dyn EngineClient> = FakeEngine::new();
    let result = run_scene(cx, BenchScene::Bridge, 1, None, Some(real), true);
    assert_eq!(
        result.map(|r| r.scene).map_err(|e| e.to_string()),
        Err("bridge: only 0 samples arrived".to_owned())
    );
}

/// The process keys of `view`'s process rows, in display order.
fn process_keys(view: &ProcessTableView) -> Vec<ProcessKey> {
    view.rows
        .iter()
        .filter_map(|r| match r.node {
            NodeId::Process(key) => Some(key),
            _ => None,
        })
        .collect()
}

fn first_tick(seed: u64) -> (TableSynth, BenchRng, Arc<ProcessTableView>) {
    let mut rng = BenchRng::new(seed);
    let mut synth = TableSynth::new(&mut rng);
    let view = synth.tick_1hz(&mut rng, 3_600_000_000_000);
    (synth, rng, view)
}

#[test]
fn bench_table_2000_scene_exists_and_is_deterministic() {
    assert_eq!(
        BenchScene::from_arg("table-2000"),
        Some(BenchScene::Table2000)
    );
    let (_, _, a) = first_tick(SEED);
    let (_, _, b) = first_tick(SEED);
    let keys = process_keys(&a);
    assert_eq!(keys.len(), TABLE_PROCESSES);
    assert_eq!(keys, process_keys(&b));
}

#[test]
fn table_synth_sorted_grouped_2000() {
    let (synth, _, view) = first_tick(SEED);
    let procs = synth.processes();
    assert_eq!(view.rows.len(), 1 + TABLE_GROUPS + TABLE_PROCESSES);
    assert_eq!((view.total, view.shown, view.restricted), (2000, 2000, 0));
    assert_eq!(view.rows[0].depth, 0, "the Apps header first");

    let mut group_cpu: Vec<f32> = Vec::new();
    let mut members: Vec<(f32, ProcessKey)> = Vec::new();
    let check_members = |members: &mut Vec<(f32, ProcessKey)>| {
        assert!(members.is_empty() || members.len() == TABLE_GROUP_SIZE);
        for pair in members.windows(2) {
            let ((ca, ka), (cb, kb)) = (pair[0], pair[1]);
            assert!(ca > cb || (ca == cb && ka < kb), "member order");
        }
        members.clear();
    };
    for row in &view.rows[1..] {
        match (row.group, row.record) {
            (Some(g), None) => {
                check_members(&mut members);
                let group = &view.groups[g as usize];
                assert_eq!(row.depth, 1);
                assert_eq!(group.members as usize, TABLE_GROUP_SIZE);
                group_cpu.push(group.cpu_pct);
            }
            (None, Some(i)) => {
                let r = &procs.records[i as usize];
                assert_eq!(row.depth, 2);
                assert_eq!(row.node, NodeId::Process(r.key));
                members.push((r.cpu_pct, r.key));
            }
            other => panic!("unexpected row {other:?}"),
        }
    }
    check_members(&mut members);
    assert_eq!(group_cpu.len(), TABLE_GROUPS);
    assert!(
        group_cpu.windows(2).all(|p| p[0] >= p[1]),
        "groups by CPU descending"
    );
    for g in view.groups.iter() {
        let sum: f32 = procs
            .records
            .iter()
            .filter(|r| r.group == g.key)
            .map(|r| r.cpu_pct)
            .sum();
        assert!(
            (g.cpu_pct - sum).abs() < 1e-3,
            "group CPU is the member sum"
        );
    }
    assert!(
        procs.records.windows(2).all(|p| p[0].key < p[1].key),
        "records sorted by ProcessKey"
    );
}

#[test]
fn table_synth_changes_every_tick() {
    let (mut synth, mut rng, first) = first_tick(SEED);
    let before = synth.processes();
    let second = synth.tick_1hz(&mut rng, 3_601_000_000_000);
    let after = synth.processes();
    assert!(second.header.version > first.header.version);
    for (a, b) in before.records.iter().zip(after.records.iter()) {
        assert_eq!(a.key, b.key);
        assert_ne!(a.cpu_pct, b.cpu_pct, "{} CPU changed", a.meta.name);
        assert_ne!(a.rss_anon_bytes, b.rss_anon_bytes);
        assert_ne!(a.gpu_util_pct, b.gpu_util_pct);
    }
    assert_ne!(process_keys(&first), process_keys(&second), "order changed");
}

#[gpui_kit::test]
fn table_scroll_bounces_at_1200_px_per_s(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let engine = BenchEngine::start(BenchFeed::Table, SEED, now_boot_ns());
    let window = cx
        .update(|cx| open_scene(cx, BenchScene::Table2000, engine.clone()))
        .expect("the scene window opens");
    let mut vcx = VisualTestContext::from_window(window.handle, cx);
    let page = window
        .view
        .read_with(&vcx, |v, _| v.processes().cloned())
        .expect("the Processes tab");

    // The engine answers the page's first query on its next 1 Hz tick, in real time.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        draw(&mut vcx);
        let rows = vcx.update(|_, cx| {
            page.read(cx)
                .table()
                .read(cx)
                .delegate
                .query()
                .shown()
                .map_or(0, |v| v.rows.len())
        });
        if rows > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "the table never arrived");
        std::thread::sleep(Duration::from_millis(50));
    }

    let scroller = vcx.update(|_, cx| Rc::new(RefCell::new(TableScroller::new(&page, cx))));
    let handle = vcx.update(|_, cx| page.read(cx).scroll_handle(cx));
    let offset = || -f32::from(handle.0.borrow().base_handle.offset().y);
    let start = offset();
    for _ in 0..144 {
        vcx.update(|_, cx| scroller.borrow_mut().frame(OUTPUT_PERIOD_NS, cx));
        draw(&mut vcx);
    }
    let moved = offset() - start;
    assert!(
        (moved - TABLE_SCROLL_PX_PER_S).abs() <= 20.,
        "moved {moved} px in 1 s"
    );

    // Run on to the bottom; the next frame moves up.
    let mut frames = 0;
    while !scroller.borrow().scroll().up {
        vcx.update(|_, cx| scroller.borrow_mut().frame(OUTPUT_PERIOD_NS, cx));
        frames += 1;
        assert!(frames < 100_000, "never reached the bottom");
    }
    let bottom = offset();
    let max = f32::from(handle.0.borrow().base_handle.max_offset().y);
    assert!((bottom - max).abs() < 0.5, "stopped at {bottom}, max {max}");
    vcx.update(|_, cx| scroller.borrow_mut().frame(OUTPUT_PERIOD_NS, cx));
    assert!(offset() < bottom, "moves upward after the bottom");
    engine.stop();
}

#[gpui_kit::test]
fn soak_scene_opens_three_windows(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let engine = BenchEngine::start(BenchFeed::Soak, SEED, now_boot_ns());
    let scene = cx
        .update(|cx| open_scene(cx, BenchScene::Soak, engine.clone()))
        .expect("the soak windows open");
    assert_eq!(scene.graphs.len(), SOAK_TITLES.len());
    assert_eq!(cx.update(|cx| cx.windows().len()), 3);
    let handles: Vec<_> = std::iter::once(scene.handle)
        .chain(scene.graphs.iter().map(|(h, _)| *h))
        .collect();
    // 10 s of simulated time, one frame per window per 100 ms.
    for _ in 0..100 {
        for &h in &handles {
            let mut vcx = VisualTestContext::from_window(h, cx);
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
        cx.executor().advance_clock(Duration::from_millis(100));
    }
    let requested: Vec<u64> = cx.update(|cx| {
        scene
            .drivers(cx)
            .iter()
            .map(|d| d.read(cx).stats.requested)
            .collect()
    });
    assert_eq!(requested.len(), 3);
    assert!(
        requested.iter().all(|&r| r > 0),
        "every window's charts request frames: {requested:?}"
    );
    engine.stop();
}

#[gpui_kit::test]
fn soak_sums_stalls(cx: &mut TestAppContext) {
    // Only the last window gets simulated frames, so the other two stall.
    let report = run_scene(cx, BenchScene::Soak, 3, None, None, true).expect("soak passes");
    assert_eq!(report.scene, "soak");
    let per_window: Vec<u64> = cx.update(|cx| {
        cx.windows()
            .into_iter()
            .map(|h| {
                h.update(cx, |_, window, cx| {
                    AnimationDriver::global(window, cx).read(cx).stats.stalls
                })
                .expect("window open")
            })
            .collect()
    });
    assert_eq!(per_window.len(), 3);
    assert_eq!(
        u64::from(report.stalls + report.warmup_stalls),
        per_window.iter().sum::<u64>(),
        "report {} + {} vs windows {per_window:?}",
        report.stalls,
        report.warmup_stalls
    );
}

/// Work counters of one perf-cpu or static window, summed over its charts.
#[derive(Clone, Copy, Debug, Default)]
struct Work {
    paints: u64,
    sidebar_renders: u32,
    card_renders: u32,
    card_builds: u32,
    card_paths: u32,
    overall_renders: u32,
    overall_builds: u32,
    overall_paths: u32,
    grid_renders: u32,
    grid_builds: u32,
    grid_paths: u32,
    grid_text_shapes: u32,
}

fn work(scene: &tl_ui::bench::scenes::SceneWindow, cx: &mut VisualTestContext) -> Work {
    let view = scene.view.clone();
    cx.update(|_, cx| {
        let v = view.read(cx);
        let page = v.page().expect("the Performance tab").read(cx);
        let sidebar = page.sidebar().read(cx);
        let cpu = page
            .page(&DeviceKey(Arc::from(DeviceKey::CPU)))
            .expect("CPU page built")
            .clone()
            .downcast::<CpuPage>()
            .expect("a CpuPage")
            .read(cx);
        let overall = cpu.overall().read(cx);
        let grid = cpu.grid().view.read(cx);
        let mut w = Work {
            paints: v.paints(),
            sidebar_renders: sidebar.render_count.get(),
            overall_renders: overall.render_count.get(),
            overall_builds: overall.build_count.get(),
            overall_paths: overall.path_paints.get(),
            grid_renders: grid.render_count.get(),
            grid_builds: grid.build_count.get(),
            grid_paths: grid.path_paints.get(),
            grid_text_shapes: grid.heading_shapes.get() + grid.value_shapes.get(),
            ..Work::default()
        };
        for card in sidebar.cards() {
            let c = card.chart.read(cx);
            w.card_renders += c.render_count.get();
            w.card_builds += c.build_count.get();
            w.card_paths += c.path_paints.get();
        }
        w
    })
}

/// Diagnostic for tasklens-wlf.27 and wlf.28, not a gate: how much chart work one redraw asks
/// for in `static` and `perf-cpu`, counted in the test platform (no GPU). Run with
/// `cargo test -p tl-ui --features bench --test bench_scenes redraw_work -- --ignored --nocapture`.
fn redraw_work(cx: &mut TestAppContext, scene_kind: BenchScene) {
    cx.executor().allow_parking();
    {
        let engine = BenchEngine::start(BenchFeed::Synthetic, SEED, now_boot_ns());
        let scene = cx
            .update(|cx| open_scene(cx, scene_kind, engine.clone()))
            .expect("the scene window opens");
        let mut vcx = VisualTestContext::from_window(scene.handle, cx);
        draw(&mut vcx);
        let draws = 20u32;
        let before = work(&scene, &mut vcx);
        // One draw per real 100 ms, so each draw follows one 10 Hz publish.
        for _ in 0..draws {
            std::thread::sleep(Duration::from_millis(100));
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            vcx.executor().advance_clock(Duration::from_millis(100));
        }
        let after = work(&scene, &mut vcx);
        let d = |a: u32, b: u32| f64::from(a - b) / f64::from(draws);
        eprintln!(
            "{}: per draw: paints {:.1}, sidebar renders {:.1}, 9 cards renders {:.1} builds {:.1} \
             paths {:.1}, overall renders {:.1} builds {:.1} paths {:.1}, grid renders {:.1} \
             builds {:.1} paths {:.1} text shapes {:.1}",
            scene_kind.as_str(),
            (after.paints - before.paints) as f64 / f64::from(draws),
            d(after.sidebar_renders, before.sidebar_renders),
            d(after.card_renders, before.card_renders),
            d(after.card_builds, before.card_builds),
            d(after.card_paths, before.card_paths),
            d(after.overall_renders, before.overall_renders),
            d(after.overall_builds, before.overall_builds),
            d(after.overall_paths, before.overall_paths),
            d(after.grid_renders, before.grid_renders),
            d(after.grid_builds, before.grid_builds),
            d(after.grid_paths, before.grid_paths),
            d(after.grid_text_shapes, before.grid_text_shapes),
        );
        engine.stop();
    }
}

#[gpui_kit::test]
#[ignore = "diagnostic; prints counters"]
fn redraw_work_static(cx: &mut TestAppContext) {
    redraw_work(cx, BenchScene::Static);
}

#[gpui_kit::test]
#[ignore = "diagnostic; prints counters"]
fn redraw_work_perf_cpu(cx: &mut TestAppContext) {
    redraw_work(cx, BenchScene::PerfCpu);
}
