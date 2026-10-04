//! Samplers that combine parsers into one reading per tick. Called by tl-engine workers on the blocking pool.
pub mod cpu;
pub mod disk;
pub mod memory;
pub mod net;
pub mod sensors;
