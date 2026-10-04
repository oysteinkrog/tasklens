//! tl-engine: asupersync runtime, workers, reducer, actions, D-Bus adapters and the GPU probe client.

pub mod config;
pub mod controller;
mod runtime;
pub mod source;
pub mod supervise;
pub mod update;

pub use config::{EngineConfig, EngineMode};
