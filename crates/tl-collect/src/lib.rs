//! Synchronous samplers and parsers for procfs and sysfs. No async code, no runtime.
pub mod counters;
pub mod error;
pub mod fdcache;
pub mod parse;
pub mod sample;
pub mod sysroot;

pub use error::CollectError;
pub use sysroot::SysRoot;
