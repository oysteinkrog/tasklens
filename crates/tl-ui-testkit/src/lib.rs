//! Test support for the tl-ui crates (ADR-013, ADR-020): `FakeEngine`, and the headless render
//! helpers behind the `render` feature. A dev-dependency only; production code never names it.

pub mod fake_engine;
#[cfg(feature = "render")]
pub mod render;
