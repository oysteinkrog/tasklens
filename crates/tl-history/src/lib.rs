//! Process and system history for tasklens (ADR-012). The engine feeds every process sweep and
//! system sample to a `RecordSink`: the `Recorder` builds 60 s blocks for the on-disk `Store`
//! (zstd-compressed segment files of 10 minutes per boot, crash-safe through per-block CRCs),
//! and the `RollingWindow` keeps the last 10 minutes in RAM when history is not persisted. The
//! read side rebuilds the process state at any second, decimates graphs to at most 1,024 points
//! and answers range and top-K queries. Command lines are never stored (ADR-015).
//!
//! `metrics` is the device metric history of ADR-017: 1 s, 10 s and 1 min tiers per device
//! series, kept for 1 h, 24 h and 7 days.

#![forbid(unsafe_code)]

pub mod block;
pub mod error;
pub mod format;
pub mod metrics;
pub mod read;
pub mod recorder;
pub mod retention;
pub mod rolling;
pub mod segment;
pub mod store;

pub use {
    block::{BlockBuilder, EncodedBlock, decode_block, encode_block},
    error::HistoryError,
    format::*,
    metrics::{MetricConfig, MetricHistory},
    read::*,
    recorder::{ClosedBlock, Recorder},
    retention::*,
    rolling::RollingWindow,
    store::{LoadProblem, LoadReport, Store, StoreConfig},
};
