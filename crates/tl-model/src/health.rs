//! Source health published in every snapshot (ADR-003, ADR-004).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::time::TimeNs;

/// One per supervised worker (ADR-004 worker list). Order and numbers are frozen.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SourceId {
    Cpu = 0,
    Memory = 1,
    Disk = 2,
    Net = 3,
    GpuNvidia = 4,
    GpuDrm = 5,
    Sensors = 6,
    Processes = 7,
    ProcessDetail = 8,
    Connections = 9,
    SystemdSystem = 10,
    SystemdUser = 11,
    Logind = 12,
    Startup = 13,
    Inventory = 14,
    KWin = 15,
    Udisks = 16,
    UPower = 17,
    NetworkManager = 18,
    HistoryRecorder = 19,
    Journal = 20,
}

impl SourceId {
    pub const COUNT: usize = 21;

    /// Every source, in number order.
    pub const ALL: [SourceId; 21] = [
        SourceId::Cpu,
        SourceId::Memory,
        SourceId::Disk,
        SourceId::Net,
        SourceId::GpuNvidia,
        SourceId::GpuDrm,
        SourceId::Sensors,
        SourceId::Processes,
        SourceId::ProcessDetail,
        SourceId::Connections,
        SourceId::SystemdSystem,
        SourceId::SystemdUser,
        SourceId::Logind,
        SourceId::Startup,
        SourceId::Inventory,
        SourceId::KWin,
        SourceId::Udisks,
        SourceId::UPower,
        SourceId::NetworkManager,
        SourceId::HistoryRecorder,
        SourceId::Journal,
    ];

    /// The discriminant, used to index `HealthSnapshot::sources`.
    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SourceState {
    /// Not demanded; the worker sleeps (IDLE_RECHECK).
    #[default]
    Off,
    /// Producing fresh data.
    Running,
    /// 3 consecutive jobs longer than 0.8 × period (ADR-004).
    Late,
    /// Last fresh data older than max(3 × period, 1.5 s) (ADR-008).
    Stale,
    /// Last attempt failed; `last_error` says why.
    Failed,
    /// Restart budget spent (5 in 60 s); off for 60 s (ADR-004).
    Disabled { until: TimeNs },
    /// Terminal: the controller never retries (GPU probe not installed, LibraryNotFound,
    /// DriverNotLoaded, KWin bridge not available) (R-155 item 4).
    Unsupported,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceHealth {
    pub state: SourceState,
    /// CLOCK_BOOTTIME of the last fresh update, for "No new data for 12 s".
    pub last_ok_ns: Option<TimeNs>,
    /// Short, sanitized error text for tooltips and empty states. One line, at most 200
    /// characters, no control characters, never a command line, environment value or file
    /// content (ADR-015). The engine sanitizes it before it is stored.
    pub last_error: Option<Arc<str>>,
}

/// Privileged helper state for the shield (UI-FND-028, ADR-006). P3; Missing before then.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HelperState {
    #[default]
    Missing,
    Locked,
    Starting,
    Unlocked,
}

/// systemd manager connection state for the Services tab (ADR-003 ServicesSnapshot.managers).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ManagerState {
    Connected,
    #[default]
    Unavailable,
    Reconnecting,
}

/// Health of every source plus the helper state. Replaced as a whole `Arc` only when a state
/// changes.
#[derive(Clone, Debug)]
pub struct HealthSnapshot {
    /// Bumps when any state, the helper state, helper_start, caps or setup changes.
    pub version: u64,
    pub sources: [SourceHealth; SourceId::COUNT],
    pub helper: HelperState,
    /// (seq, outcome). The UI toasts once per new seq (R-148 item 2).
    pub helper_start: Option<(u64, HelperStartOutcome)>,
    /// R-141 item 5.
    pub caps: EngineCaps,
    // `pub setup: Arc<SetupState>` is appended by E03-063 (command.rs owns SetupState, R-149 item 1).
}

impl HealthSnapshot {
    pub fn get(&self, id: SourceId) -> &SourceHealth {
        &self.sources[id.index()]
    }
}

/// Result of a failed StartHelperSession (R-148 item 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperStartOutcome {
    Cancelled,
    NotAuthorized,
    Failed(Arc<str>),
}

bitflags::bitflags! {
    /// Engine capabilities the UI checks before it enables a menu item (R-141 item 5).
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct EngineCaps: u16 {
        /// org.freedesktop.FileManager1 has an owner (set by the opener).
        const FILE_MANAGER1 = 1;
        /// tl_ops::dump::dump_available() at engine start.
        const GDB = 2;
        /// From SourceUpdate::ConnectionsDestroySupport.
        const CONNECTIONS_DESTROY = 4;
        const KWIN_BRIDGE = 8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> HealthSnapshot {
        HealthSnapshot {
            version: 0,
            sources: std::array::from_fn(|_| SourceHealth::default()),
            helper: HelperState::default(),
            helper_start: None,
            caps: EngineCaps::empty(),
        }
    }

    #[test]
    fn source_id_all_and_index() {
        assert_eq!(SourceId::ALL.len(), 21);
        assert_eq!(SourceId::ALL.len(), SourceId::COUNT);
        for (pos, id) in SourceId::ALL.iter().enumerate() {
            assert_eq!(id.index(), pos);
        }
    }

    #[test]
    fn source_id_numbers_frozen() {
        assert_eq!(SourceId::Cpu as u8, 0);
        assert_eq!(SourceId::Memory as u8, 1);
        assert_eq!(SourceId::Disk as u8, 2);
        assert_eq!(SourceId::Net as u8, 3);
        assert_eq!(SourceId::GpuNvidia as u8, 4);
        assert_eq!(SourceId::GpuDrm as u8, 5);
        assert_eq!(SourceId::Sensors as u8, 6);
        assert_eq!(SourceId::Processes as u8, 7);
        assert_eq!(SourceId::ProcessDetail as u8, 8);
        assert_eq!(SourceId::Connections as u8, 9);
        assert_eq!(SourceId::SystemdSystem as u8, 10);
        assert_eq!(SourceId::SystemdUser as u8, 11);
        assert_eq!(SourceId::Logind as u8, 12);
        assert_eq!(SourceId::Startup as u8, 13);
        assert_eq!(SourceId::Inventory as u8, 14);
        assert_eq!(SourceId::KWin as u8, 15);
        assert_eq!(SourceId::Udisks as u8, 16);
        assert_eq!(SourceId::UPower as u8, 17);
        assert_eq!(SourceId::NetworkManager as u8, 18);
        assert_eq!(SourceId::HistoryRecorder as u8, 19);
        assert_eq!(SourceId::Journal as u8, 20);
    }

    #[test]
    fn defaults() {
        assert_eq!(SourceState::default(), SourceState::Off);
        assert_eq!(HelperState::default(), HelperState::Missing);
        assert_eq!(ManagerState::default(), ManagerState::Unavailable);
        assert_ne!(SourceState::Unsupported, SourceState::Failed);
        let h = SourceHealth::default();
        assert_eq!(h.state, SourceState::Off);
        assert_eq!(h.last_ok_ns, None);
        assert_eq!(h.last_error, None);
    }

    #[test]
    fn health_get_by_id() {
        let mut s = snapshot();
        s.sources[SourceId::GpuNvidia.index()] = SourceHealth {
            state: SourceState::Stale,
            last_ok_ns: Some(12),
            last_error: None,
        };
        let got = s.get(SourceId::GpuNvidia);
        assert_eq!(got.state, SourceState::Stale);
        assert_eq!(got.last_ok_ns, Some(12));
        assert!(std::ptr::eq(got, &s.sources[4]));
        assert_eq!(s.get(SourceId::GpuDrm).state, SourceState::Off);
    }

    #[test]
    fn engine_caps_bits_frozen() {
        assert_eq!(EngineCaps::FILE_MANAGER1.bits(), 1);
        assert_eq!(EngineCaps::GDB.bits(), 2);
        assert_eq!(EngineCaps::CONNECTIONS_DESTROY.bits(), 4);
        assert_eq!(EngineCaps::KWIN_BRIDGE.bits(), 8);
        assert_eq!(EngineCaps::default(), EngineCaps::empty());
    }
}
