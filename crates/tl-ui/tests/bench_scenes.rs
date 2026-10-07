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
use tl_ui::bench::engine::{BenchEngine, BenchFeed};
use tl_ui::bench::scenes::{TABLE_SCROLL_PX_PER_S, TableScroller, open_scene};
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
    assert_eq!(BenchScene::from_arg("charts"), None);
    let opts = BenchOptions::default();
    assert_eq!((opts.duration_s, opts.warmup_s, opts.json), (30, 5, None));
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
    let report = run_scene(cx, BenchScene::Static, 2, None, None, true).expect("static passes");
    assert_eq!(report.scene, "static");
    assert_eq!(report.frames, 0, "the probe is not started");
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
