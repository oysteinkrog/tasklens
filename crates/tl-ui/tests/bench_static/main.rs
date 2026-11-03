//! The `static` bench scene's paint counts, in a test binary of their own (tasklens-zljr5.54).
//!
//! The bench publisher thread ticks at a real 10 Hz while the test clock is simulated, and in
//! GPUI test mode each sample redraws the window. Inside the full `ui` binary, about 1,000 tests
//! run in parallel and slow these runs in wall time, so more real samples land in the simulated
//! seconds: the count with frames reached 26 to 36, and the count without frames went above 0.
//! In a fresh process the first count stays at about one paint per simulated 100 ms frame and
//! the second at 0. Run with `--features bench`.
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

#[gpui_kit::test]
fn static_scene_counts_no_paints_without_frames(cx: &mut TestAppContext) {
    let report = run_scene(cx, BenchScene::Static, 1, None, None, false).expect("static passes");
    assert_eq!(report.frames, 0);
}
