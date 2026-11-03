//! The `static` bench scene's paint count, in a test binary of its own (tasklens-zljr5.54).
//!
//! The bench publisher thread ticks at a real 10 Hz while the test clock is simulated, and in
//! GPUI test mode each sample redraws the window. Inside the full `ui` binary, about 1,000 tests
//! run in parallel and slow this run in wall time, so more real samples land in the 2 simulated
//! seconds and the count reached 26 to 36. In a fresh process the count stays at about one
//! paint per simulated 100 ms frame. Run with `--features bench`.
#![cfg(feature = "bench")]

#[path = "../ui/bench_run.rs"]
mod bench_run;

use gpui_kit::TestAppContext;
use tl_ui::bench::BenchScene;

use bench_run::run_scene;

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
