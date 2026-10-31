//! Wire side of the engine service (ADR-018, D-038): the envelope's zstd layer and its memory
//! limits, snapshot deltas and `RemoteClient`, the window's `EngineClient` over the protocol,
//! the engine service's endpoint checks, handshake and per-session wire side (`server`), and the
//! ssh transport (`ssh`).
//!
//! No asupersync, Tokio or GPUI in this crate's dependency closure (`xtask check-deps`).

pub mod client;
pub mod delta;
pub mod envelope;
pub mod server;
pub mod ssh;
