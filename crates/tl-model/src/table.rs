//! Process and system table queries and rows (ADR-003, ADR-009).
//!
//! The engine sorts, filters, groups and flattens the process table. The UI sends a
//! `TableQuery` and renders the `ProcessTableView` it gets back; it never reorders rows.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use smallvec::SmallVec;

use crate::ids::{ColumnId, NodeId, ProcessKey};
use crate::process::GroupRow;
use crate::snapshot::ComponentHeader;
use crate::validity::Validity;

/// Process table layout. The settings value is `"grouped"`, `"tree"` or `"flat"`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableMode {
    #[default]
    Grouped,
    Tree,
    Flat,
}

/// Sort column and direction. The engine always breaks ties by `ProcessKey`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SortSpec {
    pub column: ColumnId,
    pub ascending: bool,
}

/// What the UI asks the engine to show, sent with `Command::SetTableQuery`.
///
/// `filter` holds user text: never log it above debug level.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TableQuery {
    /// Echoed in `ProcessTableView::query_generation` so the UI can tell a stale view.
    pub generation: u64,
    pub mode: TableMode,
    pub sort: SortSpec,
    /// Lowercase. Matches display name, image name, cmdline, user, description, app name, unit
    /// name and group names. Numeric input also matches PID by prefix.
    pub filter: Arc<str>,
    /// Expanded rows. The UI owns expansion state.
    pub expanded: Arc<[NodeId]>,
    /// Tree mode: a filter match also shows the match's descendants (R-141 item 4).
    pub include_children: bool,
}

impl Default for TableQuery {
    /// Matches state.toml `[sort.processes] column = "pid", ascending = true`.
    fn default() -> Self {
        Self {
            generation: 0,
            mode: TableMode::Grouped,
            sort: SortSpec {
                column: ColumnId::Pid,
                ascending: true,
            },
            filter: Arc::from(""),
            expanded: Arc::from([]),
            include_children: false,
        }
    }
}

/// One visible row of the process table, in display order.
///
/// Not `Copy`: `NodeId::Unit` holds an `Arc` (R-141 item 3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TableRow {
    pub node: NodeId,
    pub depth: u8,
    pub expanded: bool,
    pub has_children: bool,
    /// Index into `ProcessSnapshot.records`.
    pub record: Option<u32>,
    /// Index into `ProcessTableView.groups`.
    pub group: Option<u32>,
    /// All descendants of the node, for the Tree-mode "PID +N" form (R-141 item 4).
    pub descendants: u32,
    /// Descendants that match the filter. `None` when no filter is set (R-141 item 4).
    pub subtree: Option<u32>,
}

/// One header total with its own validity. The default is a default value, `WarmingUp`.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Total<T> {
    pub value: T,
    pub validity: Validity,
}

/// The two-line header numbers of the process table (PROC-038, R-141 item 4).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct TableTotals {
    /// /proc/stat total, ADR-008 math.
    pub cpu_pct: Total<f32>,
    /// (MemTotal - MemAvailable) / MemTotal.
    pub memory_pct: Total<f32>,
    /// Swap used %.
    pub swap_pct: Total<f32>,
    /// Highest busy % among physical disks.
    pub disk_pct: Total<f32>,
    /// Total physical-interface throughput, bytes per second.
    pub net_bps: Total<f64>,
    /// Summed link speed in bytes per second. `None` means unknown, and the UI shows the rate.
    pub net_link_bps: Option<u64>,
    /// Highest utilisation among GPUs. `Unsupported` with no GPU.
    pub gpu_pct: Total<f32>,
    /// Columns `gpu_0` to `gpu_15`.
    pub gpu_adapter_pct: [Total<f32>; 16],
    /// Used / total over all GPUs.
    pub vram_pct: Total<f32>,
    /// System thread count.
    pub threads: Total<u64>,
    /// /proc/sys/fs/file-nr field 1, from `ProcessSweep.file_nr` (R-166 B3-7).
    pub handles: Total<u64>,
}

/// The engine-built process table the UI renders (ADR-009).
///
/// `rows` and `groups` are shared slices, so a snapshot that does not change the table reuses
/// them.
#[derive(Clone, Debug, Default)]
pub struct ProcessTableView {
    pub header: ComponentHeader,
    /// `TableQuery::generation` this view was built for.
    pub query_generation: u64,
    pub rows: Arc<[TableRow]>,
    /// Aggregated values, with no double counting of descendants.
    pub groups: Arc<[GroupRow]>,
    pub totals: TableTotals,
    /// Processes in the snapshot.
    pub total: u32,
    /// Processes left after the filter and Hide system processes.
    pub shown: u32,
    /// Processes with at least one `Restricted` field (R-141 item 4).
    pub restricted: u32,
}

/// Which systemd instance owns a unit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Manager {
    System,
    User,
}

/// Identity of a systemd unit. `foo.service` may exist in both managers.
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct UnitRef {
    pub manager: Manager,
    pub name: Arc<str>,
}

/// Where a Startup Apps entry comes from.
///
/// The serde names ("autostartuser" and so on) are never written to disk. They exist for
/// postcard only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartupSource {
    AutostartUser,
    AutostartSystem,
    UserUnit,
    PlasmaEnv,
    PlasmaShutdown,
}

/// Identity of a Startup Apps entry. Selection is kept by `StartupRef` across rescans (R-082).
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct StartupRef {
    pub source: StartupSource,
    /// The .desktop file name for autostart entries, the unit id for user units, the script
    /// file name for Plasma scripts. Stable across rescans.
    pub id: Arc<str>,
}

/// Unit types the Services table can show. Device units are always hidden (SYS-002).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitType {
    Service,
    Socket,
    Timer,
    Mount,
    Automount,
    Path,
    Target,
    Swap,
    Slice,
    Scope,
}

impl UnitType {
    /// Types shown by default, matching `services.unit_types` in table (a).
    pub const DEFAULT_SHOWN: [UnitType; 4] = [
        UnitType::Service,
        UnitType::Socket,
        UnitType::Timer,
        UnitType::Mount,
    ];

    /// The type from the suffix after the last '.'. `None` for a name without a dot and for
    /// any other suffix, including "device".
    pub fn from_unit_name(name: &str) -> Option<UnitType> {
        let (_, suffix) = name.rsplit_once('.')?;
        Some(match suffix {
            "service" => UnitType::Service,
            "socket" => UnitType::Socket,
            "timer" => UnitType::Timer,
            "mount" => UnitType::Mount,
            "automount" => UnitType::Automount,
            "path" => UnitType::Path,
            "target" => UnitType::Target,
            "swap" => UnitType::Swap,
            "slice" => UnitType::Slice,
            "scope" => UnitType::Scope,
            _ => return None,
        })
    }
}

/// State chips of the Services filter strip (UI-SYS-011).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceStateFilter {
    Running,
    Failed,
    Stopped,
    Disabled,
}

/// The Services filter strip (UI-SYS-011).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ServiceFilter {
    /// OR of the chosen states. Empty means all states.
    pub states: SmallVec<[ServiceStateFilter; 4]>,
    /// Never empty: the UI keeps one type selected.
    pub unit_types: SmallVec<[UnitType; 10]>,
    /// Show the System group.
    pub system: bool,
    /// Show the User group.
    pub user: bool,
}

impl Default for ServiceFilter {
    /// Matches table (a) `[services]`.
    fn default() -> Self {
        Self {
            states: SmallVec::new(),
            unit_types: SmallVec::from_slice(&UnitType::DEFAULT_SHOWN),
            system: true,
            user: true,
        }
    }
}

/// The engine-built system tables (UI-SYS-002).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SysTable {
    Services,
    Startup,
    Connections,
}

/// One row of a system table (R-155 item 1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum SysNodeId {
    /// "System services (N)" or "User services (N)".
    ServiceGroup(Manager),
    Unit(UnitRef),
    /// Child process row under a unit.
    UnitProcess(UnitRef, ProcessKey),
    Startup(StartupRef),
    /// Socket cookie.
    Conn(u64),
}

/// Column ids of the system tables (R-170 item 6). Each table has its own id set, so a shared
/// string such as "name" is fine. `as_str` and `from_str` work like `ColumnId`'s.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SysColumnId {
    Services(ServiceColumn),
    Startup(StartupColumn),
    Connections(ConnColumn),
}

/// The 19 Services ids of UI-SYS-013, in this order:
/// name, display_name, status, startup_type, pid, user, path, manager, unit_type, sub_state,
/// load_state, cpu, memory, disk, tasks, active_since, unit_file, triggered_by, description.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ServiceColumn {
    Name,
    DisplayName,
    Status,
    StartupType,
    Pid,
    User,
    Path,
    Manager,
    UnitType,
    SubState,
    LoadState,
    Cpu,
    Memory,
    Disk,
    Tasks,
    ActiveSince,
    UnitFile,
    TriggeredBy,
    Description,
}

/// The 7 Startup ids: name, publisher, status, impact, type, location, command.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StartupColumn {
    Name,
    Publisher,
    Status,
    Impact,
    Type,
    Location,
    Command,
}

/// The 17 Connections ids: proto, local, foreign, state, pid, process, user, family, local_port,
/// foreign_port, recv_q, send_q, interface, inode, service, remote_host, netns.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ConnColumn {
    Proto,
    Local,
    Foreign,
    State,
    Pid,
    Process,
    User,
    Family,
    LocalPort,
    ForeignPort,
    RecvQ,
    SendQ,
    Interface,
    Inode,
    Service,
    RemoteHost,
    Netns,
}

impl SysColumnId {
    const SERVICES: [SysColumnId; 19] = {
        use ServiceColumn as C;
        use SysColumnId::Services as S;
        [
            S(C::Name),
            S(C::DisplayName),
            S(C::Status),
            S(C::StartupType),
            S(C::Pid),
            S(C::User),
            S(C::Path),
            S(C::Manager),
            S(C::UnitType),
            S(C::SubState),
            S(C::LoadState),
            S(C::Cpu),
            S(C::Memory),
            S(C::Disk),
            S(C::Tasks),
            S(C::ActiveSince),
            S(C::UnitFile),
            S(C::TriggeredBy),
            S(C::Description),
        ]
    };

    const STARTUP: [SysColumnId; 7] = {
        use StartupColumn as C;
        use SysColumnId::Startup as S;
        [
            S(C::Name),
            S(C::Publisher),
            S(C::Status),
            S(C::Impact),
            S(C::Type),
            S(C::Location),
            S(C::Command),
        ]
    };

    const CONNECTIONS: [SysColumnId; 17] = {
        use ConnColumn as C;
        use SysColumnId::Connections as S;
        [
            S(C::Proto),
            S(C::Local),
            S(C::Foreign),
            S(C::State),
            S(C::Pid),
            S(C::Process),
            S(C::User),
            S(C::Family),
            S(C::LocalPort),
            S(C::ForeignPort),
            S(C::RecvQ),
            S(C::SendQ),
            S(C::Interface),
            S(C::Inode),
            S(C::Service),
            S(C::RemoteHost),
            S(C::Netns),
        ]
    };

    pub const fn table(self) -> SysTable {
        match self {
            Self::Services(_) => SysTable::Services,
            Self::Startup(_) => SysTable::Startup,
            Self::Connections(_) => SysTable::Connections,
        }
    }

    /// The snake_case id, as written in `[columns.services]`, `[columns.startup]` and
    /// `[columns.connections]`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Services(c) => match c {
                ServiceColumn::Name => "name",
                ServiceColumn::DisplayName => "display_name",
                ServiceColumn::Status => "status",
                ServiceColumn::StartupType => "startup_type",
                ServiceColumn::Pid => "pid",
                ServiceColumn::User => "user",
                ServiceColumn::Path => "path",
                ServiceColumn::Manager => "manager",
                ServiceColumn::UnitType => "unit_type",
                ServiceColumn::SubState => "sub_state",
                ServiceColumn::LoadState => "load_state",
                ServiceColumn::Cpu => "cpu",
                ServiceColumn::Memory => "memory",
                ServiceColumn::Disk => "disk",
                ServiceColumn::Tasks => "tasks",
                ServiceColumn::ActiveSince => "active_since",
                ServiceColumn::UnitFile => "unit_file",
                ServiceColumn::TriggeredBy => "triggered_by",
                ServiceColumn::Description => "description",
            },
            Self::Startup(c) => match c {
                StartupColumn::Name => "name",
                StartupColumn::Publisher => "publisher",
                StartupColumn::Status => "status",
                StartupColumn::Impact => "impact",
                StartupColumn::Type => "type",
                StartupColumn::Location => "location",
                StartupColumn::Command => "command",
            },
            Self::Connections(c) => match c {
                ConnColumn::Proto => "proto",
                ConnColumn::Local => "local",
                ConnColumn::Foreign => "foreign",
                ConnColumn::State => "state",
                ConnColumn::Pid => "pid",
                ConnColumn::Process => "process",
                ConnColumn::User => "user",
                ConnColumn::Family => "family",
                ConnColumn::LocalPort => "local_port",
                ConnColumn::ForeignPort => "foreign_port",
                ConnColumn::RecvQ => "recv_q",
                ConnColumn::SendQ => "send_q",
                ConnColumn::Interface => "interface",
                ConnColumn::Inode => "inode",
                ConnColumn::Service => "service",
                ConnColumn::RemoteHost => "remote_host",
                ConnColumn::Netns => "netns",
            },
        }
    }

    /// Exact, case-sensitive match within that table's id set. `None` for anything else. The
    /// caller logs and drops unknown ids (ADR-011).
    #[allow(clippy::should_implement_trait)] // takes a table and returns Option, on purpose
    pub fn from_str(table: SysTable, s: &str) -> Option<SysColumnId> {
        Self::all(table).iter().copied().find(|c| c.as_str() == s)
    }

    /// Every id of the table in the documented order: 19, 7 or 17 entries.
    pub fn all(table: SysTable) -> &'static [SysColumnId] {
        match table {
            SysTable::Services => &Self::SERVICES,
            SysTable::Startup => &Self::STARTUP,
            SysTable::Connections => &Self::CONNECTIONS,
        }
    }
}

/// Sort column and direction of a system table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SysSortSpec {
    pub column: SysColumnId,
    pub ascending: bool,
}

/// What the UI asks the engine to show for one system table.
///
/// `filter` holds user text: never log it above debug level.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SysTableQuery {
    pub table: SysTable,
    pub generation: u64,
    /// `sort.column.table()` equals `table` (R-170 item 6).
    pub sort: SysSortSpec,
    /// Lowercase text filter from the FilterBox.
    pub filter: Arc<str>,
    /// Expanded rows. The UI owns expansion state.
    pub expanded: Arc<[SysNodeId]>,
    /// `Some` only for `SysTable::Services`.
    pub services: Option<ServiceFilter>,
}

/// One visible row of a system table, in display order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SysRow {
    pub node: SysNodeId,
    pub depth: u8,
    pub expanded: bool,
    pub has_children: bool,
    /// Index into the component's records, units or entries.
    pub record: Option<u32>,
}

#[cfg(test)]
mod process_table_tests {
    use serde::de::value::{Error as ValueError, StrDeserializer};
    use serde::ser::{self, Impossible};

    use super::*;

    #[test]
    fn table_query_default_matches_state_toml() {
        let q = TableQuery::default();
        assert_eq!(q.generation, 0);
        assert_eq!(q.mode, TableMode::Grouped);
        assert_eq!(q.sort.column, ColumnId::Pid);
        assert_eq!(q.sort.column.as_str(), "pid");
        assert!(q.sort.ascending);
        assert!(q.filter.is_empty());
        assert!(q.expanded.is_empty());
        assert!(!q.include_children);
    }

    #[test]
    fn table_query_generation_counts_in_eq() {
        let a = TableQuery::default();
        let b = TableQuery {
            generation: 1,
            ..TableQuery::default()
        };
        assert_ne!(a, b);
    }

    fn not_unit_variant() -> ValueError {
        ser::Error::custom("not a unit variant")
    }

    /// Serializes a unit variant to its name and rejects everything else.
    pub(super) struct VariantName;

    macro_rules! reject {
        ($($name:ident: $ty:ty),* $(,)?) => {
            $(fn $name(self, _: $ty) -> Result<String, ValueError> {
                Err(not_unit_variant())
            })*
        };
    }

    impl ser::Serializer for VariantName {
        type Ok = String;
        type Error = ValueError;
        type SerializeSeq = Impossible<String, ValueError>;
        type SerializeTuple = Impossible<String, ValueError>;
        type SerializeTupleStruct = Impossible<String, ValueError>;
        type SerializeTupleVariant = Impossible<String, ValueError>;
        type SerializeMap = Impossible<String, ValueError>;
        type SerializeStruct = Impossible<String, ValueError>;
        type SerializeStructVariant = Impossible<String, ValueError>;

        reject!(
            serialize_bool: bool, serialize_i8: i8, serialize_i16: i16, serialize_i32: i32,
            serialize_i64: i64, serialize_u8: u8, serialize_u16: u16, serialize_u32: u32,
            serialize_u64: u64, serialize_f32: f32, serialize_f64: f64, serialize_char: char,
            serialize_str: &str, serialize_bytes: &[u8], serialize_unit_struct: &'static str,
        );

        fn serialize_unit_variant(
            self,
            _: &'static str,
            _: u32,
            variant: &'static str,
        ) -> Result<String, ValueError> {
            Ok(variant.to_owned())
        }

        fn serialize_none(self) -> Result<String, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_some<T: ?Sized + Serialize>(self, _: &T) -> Result<String, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_unit(self) -> Result<String, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_newtype_struct<T: ?Sized + Serialize>(
            self,
            _: &'static str,
            _: &T,
        ) -> Result<String, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_newtype_variant<T: ?Sized + Serialize>(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: &T,
        ) -> Result<String, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_tuple_struct(
            self,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeTupleStruct, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_tuple_variant(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeTupleVariant, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_struct(
            self,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeStruct, ValueError> {
            Err(not_unit_variant())
        }
        fn serialize_struct_variant(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeStructVariant, ValueError> {
            Err(not_unit_variant())
        }
    }

    #[test]
    fn table_mode_serde_lowercase() {
        for (mode, name) in [
            (TableMode::Grouped, "grouped"),
            (TableMode::Tree, "tree"),
            (TableMode::Flat, "flat"),
        ] {
            assert_eq!(mode.serialize(VariantName).ok().as_deref(), Some(name));
            let back = TableMode::deserialize(StrDeserializer::<ValueError>::new(name));
            assert_eq!(back.ok(), Some(mode));

            let bytes = postcard::to_stdvec(&mode).expect("serialize");
            let back: TableMode = postcard::from_bytes(&bytes).expect("deserialize");
            assert_eq!(back, mode);
        }
        let upper = TableMode::deserialize(StrDeserializer::<ValueError>::new("Tree"));
        assert!(upper.is_err());
    }

    #[test]
    fn table_totals_default_warming_up() {
        let t = TableTotals::default();
        let singles = [
            t.cpu_pct.validity,
            t.memory_pct.validity,
            t.swap_pct.validity,
            t.disk_pct.validity,
            t.net_bps.validity,
            t.gpu_pct.validity,
            t.vram_pct.validity,
            t.threads.validity,
            t.handles.validity,
        ];
        let adapters = t.gpu_adapter_pct.iter().map(|a| a.validity);
        for v in singles.into_iter().chain(adapters) {
            assert_eq!(v, Validity::WarmingUp);
        }
        assert_eq!(t.net_link_bps, None);
    }

    #[test]
    fn process_table_view_default_counts_zero() {
        let v = ProcessTableView::default();
        assert_eq!(v.header, ComponentHeader::default());
        assert_eq!(v.query_generation, 0);
        assert!(v.rows.is_empty());
        assert!(v.groups.is_empty());
        assert_eq!(v.totals, TableTotals::default());
        assert_eq!((v.total, v.shown, v.restricted), (0, 0, 0));
    }
}

#[cfg(test)]
mod sys_table_tests {
    use serde::de::value::{Error as ValueError, StrDeserializer};

    use super::process_table_tests::VariantName;
    use super::*;

    #[test]
    fn service_filter_default_matches_table_a() {
        let f = ServiceFilter::default();
        assert_eq!(
            f.unit_types.as_slice(),
            &[
                UnitType::Service,
                UnitType::Socket,
                UnitType::Timer,
                UnitType::Mount
            ]
        );
        assert!(f.states.is_empty());
        assert!(f.system);
        assert!(f.user);
    }

    #[test]
    fn unit_type_from_name_all_ten() {
        for (name, ty) in [
            ("sshd.service", UnitType::Service),
            ("dbus.socket", UnitType::Socket),
            ("fstrim.timer", UnitType::Timer),
            ("home.mount", UnitType::Mount),
            ("proc-sys-fs-binfmt_misc.automount", UnitType::Automount),
            ("cups.path", UnitType::Path),
            ("graphical.target", UnitType::Target),
            ("dev-zram0.swap", UnitType::Swap),
            ("user.slice", UnitType::Slice),
            ("session-2.scope", UnitType::Scope),
        ] {
            assert_eq!(UnitType::from_unit_name(name), Some(ty), "{name}");
        }
    }

    #[test]
    fn unit_type_from_name_rejects_device_and_bare() {
        for name in ["foo.device", "noext", ""] {
            assert_eq!(UnitType::from_unit_name(name), None, "{name:?}");
        }
    }

    #[test]
    fn unit_ref_manager_distinguishes() {
        let system = UnitRef {
            manager: Manager::System,
            name: Arc::from("foo.service"),
        };
        let user = UnitRef {
            manager: Manager::User,
            name: Arc::from("foo.service"),
        };
        assert_ne!(system, user);
        assert_ne!(SysNodeId::Unit(system), SysNodeId::Unit(user));
    }

    #[test]
    fn unit_type_and_state_filter_serde_lowercase() {
        for (ty, name) in [
            (UnitType::Service, "service"),
            (UnitType::Socket, "socket"),
            (UnitType::Timer, "timer"),
            (UnitType::Mount, "mount"),
            (UnitType::Automount, "automount"),
            (UnitType::Path, "path"),
            (UnitType::Target, "target"),
            (UnitType::Swap, "swap"),
            (UnitType::Slice, "slice"),
            (UnitType::Scope, "scope"),
        ] {
            assert_eq!(ty.serialize(VariantName).ok().as_deref(), Some(name));
            let back = UnitType::deserialize(StrDeserializer::<ValueError>::new(name));
            assert_eq!(back.ok(), Some(ty));
        }
        for (state, name) in [
            (ServiceStateFilter::Running, "running"),
            (ServiceStateFilter::Failed, "failed"),
            (ServiceStateFilter::Stopped, "stopped"),
            (ServiceStateFilter::Disabled, "disabled"),
        ] {
            assert_eq!(state.serialize(VariantName).ok().as_deref(), Some(name));
            let back = ServiceStateFilter::deserialize(StrDeserializer::<ValueError>::new(name));
            assert_eq!(back.ok(), Some(state));
        }
    }

    #[test]
    fn sys_column_ids_roundtrip_per_table() {
        for (table, count) in [
            (SysTable::Services, 19),
            (SysTable::Startup, 7),
            (SysTable::Connections, 17),
        ] {
            let all = SysColumnId::all(table);
            assert_eq!(all.len(), count);
            for &id in all {
                assert_eq!(id.table(), table);
                assert_eq!(SysColumnId::from_str(table, id.as_str()), Some(id));
            }
        }
        let names: Vec<_> = SysColumnId::all(SysTable::Connections)
            .iter()
            .map(|c| c.as_str())
            .collect();
        assert_eq!(names.first(), Some(&"proto"));
        assert_eq!(names.last(), Some(&"netns"));
    }

    #[test]
    fn sys_column_ids_are_table_scoped() {
        assert_eq!(
            SysColumnId::from_str(SysTable::Startup, "name"),
            Some(SysColumnId::Startup(StartupColumn::Name))
        );
        assert_eq!(
            SysColumnId::from_str(SysTable::Services, "name"),
            Some(SysColumnId::Services(ServiceColumn::Name))
        );
        assert_eq!(SysColumnId::from_str(SysTable::Startup, "pid"), None);
        assert_eq!(SysColumnId::from_str(SysTable::Services, "Name"), None);
    }
}
