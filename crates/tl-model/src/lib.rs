//! Data contracts shared by every tasklens crate: types, units, wire framing and the EngineClient trait. No I/O, no async runtime.
//!
//! The module order is frozen (docs/plan/00-epics.md section 2). Blank lines keep rustfmt from sorting it.

pub mod time;

pub mod ids;

pub mod validity;

pub mod series;

pub mod process;

pub mod system;

pub mod devices;

pub mod health;

pub mod snapshot;

pub mod table;

pub mod details;

pub mod demand;

pub mod actions;

pub mod command;

pub mod settings;

pub mod units;

pub mod wire;

pub mod gpu_proto;

pub mod helper_proto;

pub mod journal;

pub mod history;

pub mod client;

pub mod request;

pub mod tray;

pub mod remote_proto;
