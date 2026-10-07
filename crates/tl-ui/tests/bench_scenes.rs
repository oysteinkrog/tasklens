//! Bench scenes and the run loop (E01-007, UI-PRF-053). Run with `--features bench`.
#![cfg(feature = "bench")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::{TestAppContext, VisualTestContext};
use tl_model::client::EngineClient;
use tl_model::time::now_boot_ns;
use tl_ui::bench::engine::{BenchEngine, BenchFeed};
use tl_ui::bench::scenes::open_scene;
use tl_ui::bench::stats::BenchReport;
use tl_ui::bench::synth::SEED;
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

/// Runs `scene` for `duration_s` simulated seconds with no warm-up and returns its result.
fn run_scene(
    cx: &mut TestAppContext,
    scene: BenchScene,
    duration_s: u32,
    json: Option<std::path::PathBuf>,
    real: Option<Arc<dyn EngineClient>>,
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
    for _ in 0..=duration_s * 10 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
    }
    cx.run_until_parked();
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
        let page = view.read(cx).page().read(cx);
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
    let report = run_scene(cx, BenchScene::Static, 2, None, None).expect("static passes");
    assert_eq!(report.scene, "static");
    assert_eq!(report.frames, 0, "the probe is not started");
}

#[gpui_kit::test]
fn json_written_when_requested(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("perf-cpu.json");
    let report =
        run_scene(cx, BenchScene::PerfCpu, 1, Some(path.clone()), None).expect("perf-cpu passes");
    assert_eq!(report.scene, "perf-cpu");
    let text = std::fs::read_to_string(&path).expect("the JSON file exists");
    let json: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(json["scene"], "perf-cpu");
    assert!(json["bridge_latency_ms"].is_null());
    assert!(json["gpu"].is_null());
}

#[gpui_kit::test]
fn bridge_fails_without_samples(cx: &mut TestAppContext) {
    let real: Arc<dyn EngineClient> = FakeEngine::new();
    let result = run_scene(cx, BenchScene::Bridge, 1, None, Some(real));
    assert_eq!(
        result.map(|r| r.scene).map_err(|e| e.to_string()),
        Err("bridge: only 0 samples arrived".to_owned())
    );
}
