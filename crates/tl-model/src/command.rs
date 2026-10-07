//! Commands from the UI to the engine and AppRequest (ADR-005, R-136).
//!
//! The UI queues a `Command` with `EngineClient::send`. `AppRequest` comes from tl-app's
//! single-instance handler (ADR-010).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::actions::ActionRequest;
use crate::details::{DetailTab, DetailTarget, ScanId, SearchId, StringScanOpts};
use crate::health::SourceId;
use crate::history::{HistoryMetric, HistoryRange};
use crate::ids::ProcessKey;
use crate::snapshot::FrameGeometry;
use crate::table::{SysTableQuery, TableQuery};
use crate::time::TimeNs;

/// The six main tabs in R-023 order. tl-ui's `TabId` is this type (R-156 item 1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tab {
    Processes,
    Performance,
    Services,
    Startup,
    Connections,
    Information,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Processes,
        Tab::Performance,
        Tab::Services,
        Tab::Startup,
        Tab::Connections,
        Tab::Information,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Tab::Processes => "processes",
            Tab::Performance => "performance",
            Tab::Services => "services",
            Tab::Startup => "startup",
            Tab::Connections => "connections",
            Tab::Information => "information",
        }
    }

    /// Exact lowercase match, used by `--tab` and the ActivateAction "tab" parameter.
    #[allow(clippy::should_implement_trait)] // returns Option, not Result, on purpose
    pub fn from_str(s: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|tab| tab.as_str() == s)
    }
}

/// Requests from tl-app to the UI: the single-instance handler (ADR-010) and the lowmem unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppRequest {
    Activate {
        token: Option<Arc<str>>,
    },
    SelectTab(Tab),
    SelectPid(u32),
    Quit,
    /// tl-app could not install or remove the lowmem unit and puts `lowmem.enabled` back to
    /// this value, so the switch and the store agree (m1f.21).
    SetLowMemoryEnabled(bool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandError {
    /// The 256-slot command queue is full.
    Busy,
    /// The engine has shut down.
    EngineStopped,
}

/// Commands for the KWin bridge (PROC-052 to PROC-055).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KWinCommand {
    Activate {
        internal_id: Arc<str>,
    },
    Close {
        internal_id: Arc<str>,
    },
    /// `caption` None means the main window.
    KeepAbove {
        pid: u32,
        caption: Option<Arc<str>>,
        on: bool,
    },
    /// Detached graph windows (R-084, R-086, PERF-086).
    SetFrameGeometry {
        pid: u32,
        caption: Arc<str>,
        geometry: FrameGeometry,
    },
    /// The answer arrives in `WindowsSnapshot.pick` (R-149 item 3).
    Pick {
        seq: u64,
    },
    CancelPick {
        seq: u64,
    },
}

/// One `tasklens-setup` step run through pkexec (table (c), R-149 item 2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SetupStep {
    RaplEnable,
    RaplRevert,
    SensorsDetect,
    SensorsRevert,
    PolkitRule { on: bool },
}

impl SetupStep {
    /// Arguments after `pkexec /usr/lib/tasklens/tasklens-setup`: ["rapl-enable"], ["rapl-revert"],
    /// ["sensors-detect"], ["sensors-revert"], ["polkit-rule", "on" | "off"] (table (c), R-149 item 2).
    ///
    /// This is the only source of `tasklens-setup` arguments. No free-form argument exists.
    pub fn args(self) -> &'static [&'static str] {
        match self {
            SetupStep::RaplEnable => &["rapl-enable"],
            SetupStep::RaplRevert => &["rapl-revert"],
            SetupStep::SensorsDetect => &["sensors-detect"],
            SetupStep::SensorsRevert => &["sensors-revert"],
            SetupStep::PolkitRule { on: true } => &["polkit-rule", "on"],
            SetupStep::PolkitRule { on: false } => &["polkit-rule", "off"],
        }
    }
}

/// Setup state, a versioned part of HealthSnapshot (R-149 item 1). Replaces the earlier SetupStatus and SetupEvent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetupState {
    pub status: Option<SetupStatus>,
    pub run: Option<SetupRun>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RaplStatus {
    NoZone,
    Readable { tasklens_rule: bool },
    NotReadable,
}

/// The answer to `Command::SetupStatus`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SetupStatus {
    pub rapl: RaplStatus,
    pub journal_group: bool,
    pub helper_installed: bool,
    pub lm_sensors: bool,
    pub hwmon_fan: bool,
    pub sensors_conf: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetupRun {
    pub seq: u64,
    pub step: SetupStep,
    pub phase: SetupPhase,
    /// Capped at 64 KiB. The last lines are kept.
    pub output: Arc<str>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SetupPhase {
    Running,
    Succeeded,
    /// pkexec exit 126 or 127.
    Cancelled,
    Failed(Arc<str>),
}

/// History query payload. The E30 block wins over the earlier sketch (R-156 item 3). `HistoryRange` and
/// `HistoryMetric` come from history.rs (E03-062).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryWhat {
    Frame {
        metric: HistoryMetric,
        view: (TimeNs, TimeNs),
        keys: Arc<[ProcessKey]>,
        overview: bool,
    },
    /// Hover card, n = 3.
    TopAt {
        t: TimeNs,
        metric: HistoryMetric,
        n: u8,
    },
}

/// Device requests (R-139 item 2). `disk` is the kernel name (`sda`, `nvme0n1`), the same string as
/// `DeviceIdentity::Disk.name`; the UI resolves a DeviceIdx through DeviceCatalog, workers never see one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceCommand {
    /// UDisks2 SMART read. `update` runs SmartUpdate first.
    SmartRead { disk: Arc<str>, update: bool },
    /// smartctl through pkexec (P3).
    SmartReadAdmin { disk: Arc<str> },
    /// `seq` is a UI counter so a page can tell its own eject.
    Eject { seq: u64, disk: Arc<str> },
    /// Run by the Inventory worker as a job.
    FetchPublicIp,
    /// UPower GetHistory (P3).
    BatteryHistory { native_path: Arc<str> },
}

/// Everything the UI queues with `EngineClient::send` (ADR-005, R-136). Values are moved into
/// the bounded queue, never cloned on the hot path.
#[derive(Clone, Debug)]
pub enum Command {
    SetTableQuery(TableQuery),
    /// P2.
    SetSysTableQuery(SysTableQuery),
    Action(ActionRequest),
    RefreshNow(SourceId),
    ResetBaselines,
    /// P3.
    ReprobeSource(SourceId),
    /// P2.
    KWin(KWinCommand),
    /// P2.
    DetailRefresh {
        target: DetailTarget,
        tab: DetailTab,
    },
    ComputeHashes(ProcessKey),
    VerifyPackage(ProcessKey),
    StringScan {
        scan: ScanId,
        key: ProcessKey,
        opts: StringScanOpts,
    },
    CancelStringScan(ScanId),
    /// P2. `pattern` is user input: never log it above debug level.
    HandleSearch {
        search: SearchId,
        pattern: Arc<str>,
        include_maps: bool,
        privileged: bool,
    },
    CancelHandleSearch(SearchId),
    /// P3.
    HistoryQuery {
        id: u64,
        range: HistoryRange,
        what: HistoryWhat,
    },
    HistoryDelete {
        keys: Arc<[ProcessKey]>,
    },
    /// P3. The answer arrives in `HealthSnapshot.setup`.
    SetupStatus,
    /// Progress arrives in `HealthSnapshot.setup.run`.
    RunSetup {
        step: SetupStep,
    },
    /// P2 (R-139 item 2).
    Device(DeviceCommand),
    /// P3. Shield unlock (R-148 item 1).
    StartHelperSession,
    /// P3. Shield lock.
    EndHelperSession,
}

#[cfg(test)]
mod tests {
    use smallvec::SmallVec;

    use super::*;
    use crate::actions::{ActionId, ActionKind, Signal};
    use crate::table::{SysColumnId, SysSortSpec, SysTable};
    use crate::time::BootId;

    #[test]
    fn tab_roundtrip_and_order() {
        assert_eq!(
            Tab::ALL,
            [
                Tab::Processes,
                Tab::Performance,
                Tab::Services,
                Tab::Startup,
                Tab::Connections,
                Tab::Information,
            ]
        );
        for tab in Tab::ALL {
            assert_eq!(Tab::from_str(tab.as_str()), Some(tab));
        }
    }

    #[test]
    fn tab_from_str_rejects() {
        assert_eq!(Tab::from_str("Processes"), None);
        assert_eq!(Tab::from_str("last_used"), None);
        assert_eq!(Tab::from_str(""), None);
    }

    #[test]
    fn setup_step_args_table_c() {
        assert_eq!(SetupStep::RaplEnable.args(), ["rapl-enable"]);
        assert_eq!(SetupStep::RaplRevert.args(), ["rapl-revert"]);
        assert_eq!(SetupStep::SensorsDetect.args(), ["sensors-detect"]);
        assert_eq!(SetupStep::SensorsRevert.args(), ["sensors-revert"]);
        assert_eq!(
            SetupStep::PolkitRule { on: true }.args(),
            ["polkit-rule", "on"]
        );
        assert_eq!(
            SetupStep::PolkitRule { on: false }.args(),
            ["polkit-rule", "off"]
        );
    }

    #[test]
    fn setup_state_default() {
        let state = SetupState::default();
        assert_eq!(state.status, None);
        assert_eq!(state.run, None);
    }

    fn device_name(cmd: &DeviceCommand) -> &'static str {
        match cmd {
            DeviceCommand::SmartRead { .. } => "smart_read",
            DeviceCommand::SmartReadAdmin { .. } => "smart_read_admin",
            DeviceCommand::Eject { .. } => "eject",
            DeviceCommand::FetchPublicIp => "fetch_public_ip",
            DeviceCommand::BatteryHistory { .. } => "battery_history",
        }
    }

    #[test]
    fn device_command_variants() {
        let disk: Arc<str> = Arc::from("nvme0n1");
        let all = [
            DeviceCommand::SmartRead {
                disk: disk.clone(),
                update: true,
            },
            DeviceCommand::SmartReadAdmin { disk: disk.clone() },
            DeviceCommand::Eject { seq: 1, disk },
            DeviceCommand::FetchPublicIp,
            DeviceCommand::BatteryHistory {
                native_path: Arc::from("BAT0"),
            },
        ];
        let names: Vec<_> = all.iter().map(device_name).collect();
        assert_eq!(
            names,
            [
                "smart_read",
                "smart_read_admin",
                "eject",
                "fetch_public_ip",
                "battery_history",
            ]
        );
    }

    /// No wildcard arm: adding a `Command` variant breaks this test on purpose.
    fn command_name(cmd: &Command) -> &'static str {
        match cmd {
            Command::SetTableQuery(_) => "set_table_query",
            Command::SetSysTableQuery(_) => "set_sys_table_query",
            Command::Action(_) => "action",
            Command::RefreshNow(_) => "refresh_now",
            Command::ResetBaselines => "reset_baselines",
            Command::ReprobeSource(_) => "reprobe_source",
            Command::KWin(_) => "kwin",
            Command::DetailRefresh { .. } => "detail_refresh",
            Command::ComputeHashes(_) => "compute_hashes",
            Command::VerifyPackage(_) => "verify_package",
            Command::StringScan { .. } => "string_scan",
            Command::CancelStringScan(_) => "cancel_string_scan",
            Command::HandleSearch { .. } => "handle_search",
            Command::CancelHandleSearch(_) => "cancel_handle_search",
            Command::HistoryQuery { .. } => "history_query",
            Command::HistoryDelete { .. } => "history_delete",
            Command::SetupStatus => "setup_status",
            Command::RunSetup { .. } => "run_setup",
            Command::Device(_) => "device",
            Command::StartHelperSession => "start_helper_session",
            Command::EndHelperSession => "end_helper_session",
        }
    }

    #[test]
    fn command_variants_exhaustive() {
        let key = ProcessKey {
            start_ticks: 100,
            boot_id: BootId(42),
            pid: 7,
        };
        let keys: Arc<[ProcessKey]> = Arc::from([key]);
        let all = [
            Command::SetTableQuery(TableQuery::default()),
            Command::SetSysTableQuery(SysTableQuery {
                table: SysTable::Startup,
                generation: 1,
                sort: SysSortSpec {
                    column: SysColumnId::all(SysTable::Startup)[0],
                    ascending: true,
                },
                filter: Arc::from(""),
                expanded: Arc::from([]),
                services: None,
            }),
            Command::Action(ActionRequest {
                id: ActionId(1),
                kind: ActionKind::Signal(Signal::Term),
                targets: SmallVec::new(),
                privileged: false,
            }),
            Command::RefreshNow(SourceId::Cpu),
            Command::ResetBaselines,
            Command::ReprobeSource(SourceId::Sensors),
            Command::KWin(KWinCommand::Pick { seq: 1 }),
            Command::DetailRefresh {
                target: DetailTarget::Process(key),
                tab: DetailTab::General,
            },
            Command::ComputeHashes(key),
            Command::VerifyPackage(key),
            Command::StringScan {
                scan: ScanId(1),
                key,
                opts: StringScanOpts::default(),
            },
            Command::CancelStringScan(ScanId(1)),
            Command::HandleSearch {
                search: SearchId(1),
                pattern: Arc::from("libc"),
                include_maps: false,
                privileged: false,
            },
            Command::CancelHandleSearch(SearchId(1)),
            Command::HistoryQuery {
                id: 1,
                range: HistoryRange::Live,
                what: HistoryWhat::TopAt {
                    t: 0,
                    metric: HistoryMetric::Cpu,
                    n: 3,
                },
            },
            Command::HistoryDelete { keys: keys.clone() },
            Command::SetupStatus,
            Command::RunSetup {
                step: SetupStep::RaplEnable,
            },
            Command::Device(DeviceCommand::FetchPublicIp),
            Command::StartHelperSession,
            Command::EndHelperSession,
        ];
        assert_eq!(all.len(), 21);
        let mut names: Vec<_> = all.iter().map(command_name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 21);

        let frame = HistoryWhat::Frame {
            metric: HistoryMetric::Memory,
            view: (0, 1),
            keys,
            overview: true,
        };
        assert_ne!(
            frame,
            HistoryWhat::TopAt {
                t: 0,
                metric: HistoryMetric::Memory,
                n: 3
            }
        );
    }
}
