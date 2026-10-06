//! The atomic file write every tasklens file goes through (ADR-011, R-130).

/// Writes `<name>.tmp-<pid>`, syncs it, renames it over the target and syncs the directory
/// (E07-010, R-146).
pub use tl_ops::fs::write_atomic;
