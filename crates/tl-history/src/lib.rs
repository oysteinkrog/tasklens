//! History segment format, store and in-RAM rolling window (ADR-012).

#![forbid(unsafe_code)]

pub mod block;
pub mod error;
pub mod format;
pub mod read;
pub mod segment;
pub mod store;

pub use error::HistoryError;
