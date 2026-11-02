//! Settings the engine side uses: the update interval, `[history]` and the network unit, with
//! their ranges and clamp rule (ADR-011 canonical table (a)). No file I/O.
//!
//! tl-engine, tl-server and tl-remote need only this part. The window's settings and state
//! (`Settings`, `UiState` and the rest of `settings.toml` and `state.toml`) live in
//! `ui::settings` and are re-exported below so their old paths keep working. Outside `ui/`, only
//! the re-export lines here and in `lib.rs` name a path under `ui`. Check with
//! `rg -n "crate::ui" crates/tl-model/src --glob '!ui/**' --glob '!lib.rs'`: it finds this
//! note and the re-export line below, nothing else.

use std::ops::{Add, Div, Mul};

use serde::{Deserialize, Serialize};

pub use crate::ui::settings::*;

/// `processes.interval_ms`: min, max and step.
pub const PROCESS_INTERVAL_MS_RANGE: (u32, u32, u32) = (250, 10_000, 1);
/// `processes.interval_ms` when nothing sets it.
pub const PROCESS_INTERVAL_MS_DEFAULT: u32 = 1000;
/// `history.retention_h`: min, max and step.
pub const HISTORY_RETENTION_H_RANGE: (u8, u8, u8) = (1, 12, 1);

/// `units.network` (R-015): bits are SI base 1000, bytes are IEC base 1024.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkUnit {
    #[default]
    Bits,
    Bytes,
}

/// `[history]` (P3, D-003).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistorySettings {
    pub enabled: bool,
    /// 1..=12; the menu offers `HISTORY_RETENTION_MENU_H`.
    pub retention_h: u8,
    /// `false` deletes the segments at shutdown.
    pub persist: bool,
    /// Not in menus.
    pub max_disk_mb: u32,
    /// P2, ADR-017: 7-day device metric history. `false` stops it and deletes `metrics/`.
    pub device_metrics: bool,
    /// P2, ADR-017. Not in menus.
    pub device_metrics_max_mb: u32,
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            retention_h: 6,
            persist: true,
            max_disk_mb: 256,
            device_metrics: true,
            device_metrics_max_mb: 64,
        }
    }
}

/// The settings the engine uses, sent with `EngineClient::set_settings`. Derived from
/// `Settings`, never persisted.
///
/// The network unit, `subtree_totals` and `hide_system` travel per session in
/// [`TableQuery::settings`](crate::table::TableQuery), and the engine counts every network
/// adapter kind, so two windows on one engine can differ there (ADR-018, tasklens-q4x76.22).
#[derive(Clone, Debug, PartialEq)]
pub struct EngineSettings {
    /// `processes.interval_ms`.
    pub interval_ms: u32,
    pub history: HistorySettings,
}

/// The same values `EngineSettings::from(&Settings::default())` gives.
impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            interval_ms: PROCESS_INTERVAL_MS_DEFAULT,
            history: HistorySettings::default(),
        }
    }
}

/// Clamps `value` into `min..=max`, then rounds it to the nearest multiple of `step` (ties
/// round up). Pushes `key` to `changed` when the value changes.
pub fn clamp_key<T>(
    value: &mut T,
    (min, max, step): (T, T, T),
    key: &'static str,
    changed: &mut Vec<&'static str>,
) where
    T: Copy + Ord + From<u8> + Add<Output = T> + Div<Output = T> + Mul<Output = T>,
{
    let mut v = (*value).clamp(min, max);
    if step > T::from(1) {
        v = (v + step / T::from(2)) / step * step;
    }
    if v != *value {
        *value = v;
        changed.push(key);
    }
}
