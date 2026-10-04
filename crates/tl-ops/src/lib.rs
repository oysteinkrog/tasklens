//! tl-ops: synchronous OS operations (signals, nice, affinity, ioprio, cgroup writes).
//!
//! The modules are listed in dependency order, one per group, so rustfmt keeps that order.

pub mod error;

mod sys;

pub mod process;

pub mod cgroup;

pub mod fs;

pub mod dump;

pub mod launch;

pub use error::{OpsError, OpsResult};
pub use fs::{PRIVATE_FILE_MODE, write_atomic};
