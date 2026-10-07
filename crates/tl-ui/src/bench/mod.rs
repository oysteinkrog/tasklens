//! Phase-0 bench harness (ADR-014): synthetic sources, BenchEngine, frame statistics and scenes.
//! Built only with `--features bench`.

pub mod engine;
pub mod scenes;
pub mod stats;
pub mod synth;

pub use scenes::{BenchError, BenchOptions, BenchScene, run, run_app};
pub use stats::BenchClocks;
