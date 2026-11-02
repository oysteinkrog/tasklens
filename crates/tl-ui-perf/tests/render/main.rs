//! tl-ui-perf render goldens, headless on lavapipe (ADR-013, ADR-020). A separate test target
//! so the plain `perf` binary does not link the renderer.
//!
//! Run with `cargo test -p tl-ui-perf --features render-tests --test render`.

mod render_perf;
