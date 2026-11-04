//! Runs one bench scene under a simulated clock. Shared by `bench_scenes` in the `ui` test
//! binary and by the `bench_static` test binary, which pulls this file in with `#[path]`.
#![cfg(feature = "bench")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::{TestAppContext, VisualTestContext};
use tl_model::client::EngineClient;
use tl_ui::bench::stats::{BenchReport, FrameBudget};
use tl_ui::bench::{BenchClocks, BenchError, BenchOptions, BenchScene, run};

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

type Outcome = Rc<RefCell<Option<Result<BenchReport, BenchError>>>>;

/// Runs `scene` for `duration_s` simulated seconds with no warm-up and returns its result. With
/// `frames`, the scene window gets one simulated frame every 100 ms; without, no frame callback
/// ever runs (a locked screen).
pub fn run_scene(
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
            budget: FrameBudget::default(),
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
