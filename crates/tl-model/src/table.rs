//! Process and system table queries and rows (ADR-003, ADR-009).
//!
//! The engine sorts, filters, groups and flattens the process table. The UI sends a
//! `TableQuery` and renders the `ProcessTableView` it gets back; it never reorders rows.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::{ColumnId, NodeId};
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
    struct VariantName;

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
