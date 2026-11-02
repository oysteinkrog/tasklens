//! tl-ui-chart render goldens, headless on lavapipe (ADR-013, ADR-020). A separate test target
//! so the plain `chart` binary does not link the renderer.
//!
//! Run with `cargo test -p tl-ui-chart --features render-tests --test render`.

mod render_chart;
