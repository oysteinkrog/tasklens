//! History segment format, store and in-RAM rolling window (ADR-012).

#![forbid(unsafe_code)]

pub mod error;
pub mod format;

pub use error::HistoryError;
