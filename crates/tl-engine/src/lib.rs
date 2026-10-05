//! tl-engine: asupersync runtime, workers, reducer, actions, D-Bus adapters and the GPU probe client.
//!
//! `unsafe_code = "deny"` comes from the workspace lints. Only `gpu/spawn.rs` opts out, with its
//! own file-level allow (ADR-001). No public item here or in `config` names an asupersync type.

use std::sync::Arc;

pub mod bridge;
pub mod config;
pub mod controller;
pub mod dbus;
pub mod gpu;
mod helper;
pub mod reducer;
pub mod runtime;
pub mod source;
pub mod supervise;
pub mod update;
pub mod workers;
// Chain-edit lines added by later beads: dbus (E08-A2); session, reducer, bridge (E09);
// actions (E11); gpu (E13); launcher, opener (E14); helper (E29); history_query (E30).

pub use bridge::{EngineHandle, EngineHooks}; // E09-019 appends `Engine`, E09-A2 appends `ShutdownReport` (R-262).
pub use config::{EngineConfig, EngineMode};

/// The hooks `EngineHooks::default()` returns. Later beads replace one field each: E11-A1 the
/// action gate, E14-A4 the journal, E30-A2 the record sink.
pub fn default_hooks() -> EngineHooks {
    EngineHooks {
        action_gate: None,
        record_sink: None,
        journal: None,
    }
}

/// The only error `Engine::start` returns (E09).
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("runtime: {0}")]
    Runtime(Arc<str>),
    #[error("spawn: {0}")]
    Spawn(Arc<str>),
    #[error("supervise: {0}")]
    Supervise(#[from] supervise::SuperviseError),
}
