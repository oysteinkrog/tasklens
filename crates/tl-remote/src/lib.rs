//! Wire side of the engine service (ADR-018, D-038): the envelope's zstd layer and its memory
//! limits, and snapshot deltas. Transports, `RemoteClient` and server sessions follow in later
//! beads.
//!
//! No asupersync, Tokio or GPUI in this crate's dependency closure (`xtask check-deps`).

pub mod delta;
pub mod envelope;
