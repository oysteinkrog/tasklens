//! Synchronous samplers and parsers for procfs and sysfs. No async code, no runtime.
pub mod apps;
pub mod counters;
pub mod error;
pub mod fdcache;
pub mod gpu_drm;
pub mod hash;
pub mod inventory;
pub mod net;
pub mod parse;
pub mod proc;
pub mod sample;
pub mod sanitize;
pub mod startup;
pub mod sysroot;

pub use error::CollectError;
pub use sysroot::SysRoot;
