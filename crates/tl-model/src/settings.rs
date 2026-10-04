//! Settings and state schema, canonical table (a) (ADR-011). No file I/O.
//!
//! `tl-app` loads and saves `settings.toml`; this module only defines its shape and defaults.
//! TOML section names are the `Settings` field names and keys are the struct field names.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::actions::DumpMode;
use crate::table::{ServiceStateFilter, TableMode, UnitType};

/// First line of `settings.toml` and `state.toml`.
pub const SCHEMA_VERSION: u32 = 1;

/// `general.language` values in table (a) order. `"system"` follows the desktop locale.
pub const LANGUAGES: [&str; 12] = [
    "system", "en-US", "de-DE", "es-ES", "fr-FR", "ja-JP", "ko-KR", "pt-BR", "ru-RU", "uk-UA",
    "zh-CN", "nb-NO",
];

/// The Refresh menu choices for `processes.interval_ms`.
pub const PROCESS_INTERVAL_MENU_MS: [u32; 6] = [250, 500, 1000, 2000, 5000, 10000];

/// `general.start_tab` (R-024). `LastUsed` opens `state.toml` `last_tab`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartTab {
    #[default]
    Processes,
    Performance,
    Services,
    Startup,
    Connections,
    Information,
    LastUsed,
}

/// `appearance.decorations` (R-021). Label "Use system title bar"; a change needs a restart.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decorations {
    #[default]
    Client,
    Server,
}

/// `units.network` (R-015): bits are SI base 1000, bytes are IEC base 1024.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkUnit {
    #[default]
    Bits,
    Bytes,
}

/// `performance.graph_range_s` choices in seconds. 300 and 600 come with P2.
pub const GRAPH_RANGES_S: [u16; 4] = [30, 60, 300, 600];

/// The menu choices for `history.retention_h`.
pub const HISTORY_RETENTION_MENU_H: [u8; 7] = [1, 2, 3, 4, 6, 8, 12];

/// `tables.column_resize` (R-075), applied to every table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnResize {
    #[default]
    Proportional,
    LastColumn,
}

/// `performance.cpu_graph_mode`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuGraphMode {
    Overall,
    #[default]
    PerCore,
    AllCoresOverlaid,
    AllCoresStacked,
}

/// `performance.cpu_secondary_graph`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CpuSecondaryGraph {
    None,
    #[default]
    Power,
    Clock,
    Temperature,
}

/// Entries of `performance.gpu_optional_graphs`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuOptionalGraph {
    Power,
    Clocks,
}

/// Values of `performance.device_overrides`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceVisibility {
    Show,
    Hide,
}

/// `[general]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    pub start_tab: StartTab,
    /// One of `LANGUAGES`.
    pub language: String,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            start_tab: StartTab::Processes,
            language: "system".to_owned(),
        }
    }
}

/// `[theme]`. Theme names match `themes.json` or the built-in themes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSettings {
    pub active: String,
    pub follow_system: bool,
    pub light: String,
    pub dark: String,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            active: "Default Dark".to_owned(),
            follow_system: false,
            light: "Default Light".to_owned(),
            dark: "Default Dark".to_owned(),
        }
    }
}

/// `[appearance]` (R-020, R-021).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    /// 75..=200, step 5.
    pub ui_scale_pct: u16,
    /// 80..=150, step 5. Row height = 28 × row_height_pct/100 × ui_scale_pct/100 logical px.
    pub row_height_pct: u16,
    pub decorations: Decorations,
    pub rounded_corners: bool,
    pub heat_coloring: bool,
    /// 0..=100.
    pub heat_intensity_pct: u8,
    /// 0..=100.
    pub shadow_intensity_pct: u8,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            ui_scale_pct: 100,
            row_height_pct: 100,
            decorations: Decorations::Client,
            rounded_corners: true,
            heat_coloring: true,
            heat_intensity_pct: 50,
            shadow_intensity_pct: 100,
        }
    }
}

/// `[motion]` (R-017). The system reduce-motion setting turns all of these off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionSettings {
    /// Tab, theme and toast motion. The UI ANDs it with the system reduce-motion setting.
    pub animations: bool,
    /// Chart sliding.
    pub smooth_graphs: bool,
    pub smooth_scroll: bool,
    /// 1..=50.
    pub smooth_scroll_amount: u8,
    /// 0..=500, step 10.
    pub tab_switch_ms: u16,
}

impl Default for MotionSettings {
    fn default() -> Self {
        Self {
            animations: true,
            smooth_graphs: true,
            smooth_scroll: true,
            smooth_scroll_amount: 15,
            tab_switch_ms: 250,
        }
    }
}

/// `[units]` (R-015).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UnitsSettings {
    pub network: NetworkUnit,
    pub network_auto_scale: bool,
}

/// `[processes]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessesSettings {
    /// 250..=10000; the menu offers `PROCESS_INTERVAL_MENU_MS`.
    pub interval_ms: u32,
    pub mode: TableMode,
    pub tree_guide_lines: bool,
    pub highlight_new_exited: bool,
    pub filter_include_children: bool,
    pub hide_system: bool,
    /// "Show subtree totals on parent processes".
    pub subtree_totals: bool,
    pub dump_type: DumpMode,
}

impl Default for ProcessesSettings {
    fn default() -> Self {
        Self {
            interval_ms: 1000,
            mode: TableMode::Grouped,
            tree_guide_lines: false,
            highlight_new_exited: true,
            filter_include_children: false,
            hide_system: false,
            subtree_totals: false,
            dump_type: DumpMode::Mini,
        }
    }
}

/// `[tables]`, applied to every table (R-075).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TablesSettings {
    pub column_resize: ColumnResize,
    pub column_separators: bool,
    pub dim_zero_values: bool,
}

/// `[properties]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PropertiesSettings {
    pub hide_unnamed_handles: bool,
    pub shared_objects_only: bool,
    pub env_mask: bool,
}

impl Default for PropertiesSettings {
    fn default() -> Self {
        Self {
            hide_unnamed_handles: true,
            shared_objects_only: true,
            env_mask: true,
        }
    }
}

/// `[performance]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceSettings {
    /// One of `GRAPH_RANGES_S`.
    pub graph_range_s: u16,
    pub cpu_graph_mode: CpuGraphMode,
    pub cpu_kernel: bool,
    pub cpu_secondary_graph: CpuSecondaryGraph,
    pub gpu_optional_graphs: Vec<GpuOptionalGraph>,
    /// Engine keys.
    pub hidden_gpu_engines: Vec<String>,
    pub memory_composition: bool,
    pub memory_swap_graph: bool,
    pub show_disks: bool,
    pub show_gpus: bool,
    pub show_fans: bool,
    pub show_batteries: bool,
    pub show_network_wired: bool,
    pub show_network_wireless: bool,
    /// VPN and tunnel interfaces.
    pub show_network_vpn: bool,
    pub show_network_virtual: bool,
    pub show_network_other: bool,
    pub show_disconnected_fans: bool,
    /// Device keys in sidebar order (PERF-005).
    pub device_order: Vec<String>,
    /// Device key to show or hide.
    pub device_overrides: BTreeMap<String, DeviceVisibility>,
    pub curved_lines: bool,
    /// `false` scales the network graph to the link speed.
    pub network_dynamic_scale: bool,
}

impl Default for PerformanceSettings {
    fn default() -> Self {
        Self {
            graph_range_s: 60,
            cpu_graph_mode: CpuGraphMode::PerCore,
            cpu_kernel: true,
            cpu_secondary_graph: CpuSecondaryGraph::Power,
            gpu_optional_graphs: Vec::new(),
            hidden_gpu_engines: Vec::new(),
            memory_composition: true,
            memory_swap_graph: true,
            show_disks: true,
            show_gpus: true,
            show_fans: true,
            show_batteries: true,
            show_network_wired: true,
            show_network_wireless: true,
            show_network_vpn: true,
            show_network_virtual: false,
            show_network_other: true,
            show_disconnected_fans: false,
            device_order: Vec::new(),
            device_overrides: BTreeMap::new(),
            curved_lines: false,
            network_dynamic_scale: true,
        }
    }
}

/// One `[fans]` entry, keyed by fan key (P3). Both fields are left out of the file when `None`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FanOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `"<hwmon name>:<path>:tempN"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temp: Option<String>,
}

/// `[graphs]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphsSettings {
    pub minigraph_cpu: bool,
    pub minigraph_memory: bool,
    pub minigraph_disk: bool,
    pub minigraph_network: bool,
    pub minigraph_gpu: bool,
    pub minigraph_vram: bool,
    pub minigraph_gpu_temperature: bool,
    pub keep_detached_on_close: bool,
}

impl Default for GraphsSettings {
    fn default() -> Self {
        Self {
            minigraph_cpu: true,
            minigraph_memory: true,
            minigraph_disk: true,
            minigraph_network: true,
            minigraph_gpu: true,
            minigraph_vram: true,
            minigraph_gpu_temperature: false,
            keep_detached_on_close: false,
        }
    }
}

/// `[history]` (P3, D-003).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistorySettings {
    pub enabled: bool,
    /// 1..=12; the menu offers `HISTORY_RETENTION_MENU_H`.
    pub retention_h: u8,
    /// `false` deletes the segments at shutdown.
    pub persist: bool,
    /// Not in menus.
    pub max_disk_mb: u32,
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            retention_h: 6,
            persist: true,
            max_disk_mb: 256,
        }
    }
}

/// `[window]`.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    pub close_with_esc: bool,
    pub always_on_top: bool,
}

/// `[tray]` (P3).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TraySettings {
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
    pub always_show: bool,
}

/// `[startup]`.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StartupSettings {
    pub start_at_login: bool,
    pub start_minimized: bool,
}

/// `[network]`. The fetched public IP is never stored here (PERF-060).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    pub auto_fetch_public_ip: bool,
    /// Consent flag for the public IP lookup.
    pub public_ip_notice_accepted: bool,
}

/// `[privilege]` (P3, P4).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivilegeSettings {
    pub unlock_at_launch: bool,
    pub passwordless: bool,
}

/// `[rendering]` (P4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderingSettings {
    /// `"auto"`, `"software"`, `"pci:<bus id>"` or `"name:<adapter name>"` (R-237).
    pub gpu: String,
}

impl Default for RenderingSettings {
    fn default() -> Self {
        Self {
            gpu: "auto".to_owned(),
        }
    }
}

/// `[services]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServicesSettings {
    pub unit_types: Vec<UnitType>,
    /// Empty means all states.
    pub filter_states: Vec<ServiceStateFilter>,
}

impl Default for ServicesSettings {
    fn default() -> Self {
        Self {
            unit_types: UnitType::DEFAULT_SHOWN.to_vec(),
            filter_states: Vec::new(),
        }
    }
}

/// `[lowmem]` (P3).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LowmemSettings {
    pub enabled: bool,
}

/// `[debug]`.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugSettings {
    pub show_fps: bool,
}

/// The whole of `settings.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub general: GeneralSettings,
    pub theme: ThemeSettings,
    pub appearance: AppearanceSettings,
    pub motion: MotionSettings,
    pub units: UnitsSettings,
    pub processes: ProcessesSettings,
    pub tables: TablesSettings,
    pub properties: PropertiesSettings,
    pub performance: PerformanceSettings,
    /// Fan key to override (P3).
    pub fans: BTreeMap<String, FanOverride>,
    pub graphs: GraphsSettings,
    pub history: HistorySettings,
    pub window: WindowSettings,
    pub tray: TraySettings,
    pub startup: StartupSettings,
    pub network: NetworkSettings,
    pub privilege: PrivilegeSettings,
    pub rendering: RenderingSettings,
    pub services: ServicesSettings,
    pub lowmem: LowmemSettings,
    pub debug: DebugSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            general: GeneralSettings::default(),
            theme: ThemeSettings::default(),
            appearance: AppearanceSettings::default(),
            motion: MotionSettings::default(),
            units: UnitsSettings::default(),
            processes: ProcessesSettings::default(),
            tables: TablesSettings::default(),
            properties: PropertiesSettings::default(),
            performance: PerformanceSettings::default(),
            fans: BTreeMap::new(),
            graphs: GraphsSettings::default(),
            history: HistorySettings::default(),
            window: WindowSettings::default(),
            tray: TraySettings::default(),
            startup: StartupSettings::default(),
            network: NetworkSettings::default(),
            privilege: PrivilegeSettings::default(),
            rendering: RenderingSettings::default(),
            services: ServicesSettings::default(),
            lowmem: LowmemSettings::default(),
            debug: DebugSettings::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::de::value::{Error as ValueError, StrDeserializer};
    use serde::ser::{self, Impossible};

    use super::*;

    #[test]
    fn defaults_general_to_properties() {
        let s = Settings::default();

        assert_eq!(s.general.start_tab, StartTab::Processes);
        assert_eq!(s.general.language, "system");

        assert_eq!(s.theme.active, "Default Dark");
        assert!(!s.theme.follow_system);
        assert_eq!(s.theme.light, "Default Light");
        assert_eq!(s.theme.dark, "Default Dark");

        assert_eq!(s.appearance.ui_scale_pct, 100);
        assert_eq!(s.appearance.row_height_pct, 100);
        assert_eq!(s.appearance.decorations, Decorations::Client);
        assert!(s.appearance.rounded_corners);
        assert!(s.appearance.heat_coloring);
        assert_eq!(s.appearance.heat_intensity_pct, 50);
        assert_eq!(s.appearance.shadow_intensity_pct, 100);

        assert!(s.motion.animations);
        assert!(s.motion.smooth_graphs);
        assert!(s.motion.smooth_scroll);
        assert_eq!(s.motion.smooth_scroll_amount, 15);
        assert_eq!(s.motion.tab_switch_ms, 250);

        assert_eq!(s.units.network, NetworkUnit::Bits);
        assert!(!s.units.network_auto_scale);

        assert_eq!(s.processes.interval_ms, 1000);
        assert_eq!(s.processes.mode, TableMode::Grouped);
        assert!(!s.processes.tree_guide_lines);
        assert!(s.processes.highlight_new_exited);
        assert!(!s.processes.filter_include_children);
        assert!(!s.processes.hide_system);
        assert!(!s.processes.subtree_totals);
        assert_eq!(s.processes.dump_type, DumpMode::Mini);
        assert_eq!(
            PROCESS_INTERVAL_MENU_MS,
            [250, 500, 1000, 2000, 5000, 10000]
        );

        assert_eq!(s.tables.column_resize, ColumnResize::Proportional);
        assert!(!s.tables.column_separators);
        assert!(!s.tables.dim_zero_values);

        assert!(s.properties.hide_unnamed_handles);
        assert!(s.properties.shared_objects_only);
        assert!(s.properties.env_mask);
    }

    #[test]
    fn schema_version_one() {
        assert_eq!(SCHEMA_VERSION, 1);
        assert_eq!(Settings::default().schema_version, 1);
    }

    #[test]
    fn languages_table_a() {
        assert_eq!(
            LANGUAGES,
            [
                "system", "en-US", "de-DE", "es-ES", "fr-FR", "ja-JP", "ko-KR", "pt-BR", "ru-RU",
                "uk-UA", "zh-CN", "nb-NO",
            ]
        );
    }

    /// Serializes `value` to its unit-variant name, then reads the name back.
    fn assert_name<T>(value: T, name: &str)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        assert_eq!(value.serialize(VariantName).ok().as_deref(), Some(name));
        let back = T::deserialize(StrDeserializer::<ValueError>::new(name));
        assert_eq!(back.ok(), Some(value));
    }

    #[test]
    fn enum_serde_names() {
        for (v, n) in [
            (StartTab::Processes, "processes"),
            (StartTab::Performance, "performance"),
            (StartTab::Services, "services"),
            (StartTab::Startup, "startup"),
            (StartTab::Connections, "connections"),
            (StartTab::Information, "information"),
            (StartTab::LastUsed, "last_used"),
        ] {
            assert_name(v, n);
        }
        assert_name(Decorations::Client, "client");
        assert_name(Decorations::Server, "server");
        assert_name(NetworkUnit::Bits, "bits");
        assert_name(NetworkUnit::Bytes, "bytes");
        assert_name(ColumnResize::Proportional, "proportional");
        assert_name(ColumnResize::LastColumn, "last_column");
    }

    #[test]
    fn defaults_performance_to_debug() {
        let s = Settings::default();

        let p = &s.performance;
        assert_eq!(p.graph_range_s, 60);
        assert_eq!(p.cpu_graph_mode, CpuGraphMode::PerCore);
        assert!(p.cpu_kernel);
        assert_eq!(p.cpu_secondary_graph, CpuSecondaryGraph::Power);
        assert!(p.gpu_optional_graphs.is_empty());
        assert!(p.hidden_gpu_engines.is_empty());
        assert!(p.memory_composition);
        assert!(p.memory_swap_graph);
        assert!(p.show_disks);
        assert!(p.show_gpus);
        assert!(p.show_fans);
        assert!(p.show_batteries);
        assert!(p.show_network_wired);
        assert!(p.show_network_wireless);
        assert!(p.show_network_vpn);
        assert!(!p.show_network_virtual);
        assert!(p.show_network_other);
        assert!(!p.show_disconnected_fans);
        assert!(p.device_order.is_empty());
        assert!(p.device_overrides.is_empty());
        assert!(!p.curved_lines);
        assert!(p.network_dynamic_scale);

        assert!(s.fans.is_empty());
        assert_eq!(
            FanOverride::default(),
            FanOverride {
                label: None,
                temp: None
            }
        );

        let g = &s.graphs;
        assert!(g.minigraph_cpu);
        assert!(g.minigraph_memory);
        assert!(g.minigraph_disk);
        assert!(g.minigraph_network);
        assert!(g.minigraph_gpu);
        assert!(g.minigraph_vram);
        assert!(!g.minigraph_gpu_temperature);
        assert!(!g.keep_detached_on_close);

        assert!(!s.history.enabled);
        assert_eq!(s.history.retention_h, 6);
        assert!(s.history.persist);
        assert_eq!(s.history.max_disk_mb, 256);

        assert!(!s.window.close_with_esc);
        assert!(!s.window.always_on_top);

        assert!(!s.tray.close_to_tray);
        assert!(!s.tray.minimize_to_tray);
        assert!(!s.tray.always_show);

        assert!(!s.startup.start_at_login);
        assert!(!s.startup.start_minimized);

        assert!(!s.network.auto_fetch_public_ip);
        assert!(!s.network.public_ip_notice_accepted);

        assert!(!s.privilege.unlock_at_launch);
        assert!(!s.privilege.passwordless);

        assert_eq!(s.rendering.gpu, "auto");

        assert_eq!(
            s.services.unit_types,
            [
                UnitType::Service,
                UnitType::Socket,
                UnitType::Timer,
                UnitType::Mount
            ]
        );
        assert!(s.services.filter_states.is_empty());

        assert!(!s.lowmem.enabled);
        assert!(!s.debug.show_fps);
    }

    #[test]
    fn enum_serde_names_part_two() {
        assert_name(CpuGraphMode::Overall, "overall");
        assert_name(CpuGraphMode::PerCore, "per_core");
        assert_name(CpuGraphMode::AllCoresOverlaid, "all_cores_overlaid");
        assert_name(CpuGraphMode::AllCoresStacked, "all_cores_stacked");
        assert_name(CpuSecondaryGraph::None, "none");
        assert_name(CpuSecondaryGraph::Power, "power");
        assert_name(CpuSecondaryGraph::Clock, "clock");
        assert_name(CpuSecondaryGraph::Temperature, "temperature");
        assert_name(GpuOptionalGraph::Power, "power");
        assert_name(GpuOptionalGraph::Clocks, "clocks");
        assert_name(DeviceVisibility::Show, "show");
        assert_name(DeviceVisibility::Hide, "hide");
    }

    #[test]
    fn menus_and_ranges_constants() {
        assert_eq!(GRAPH_RANGES_S, [30, 60, 300, 600]);
        assert_eq!(HISTORY_RETENTION_MENU_H, [1, 2, 3, 4, 6, 8, 12]);
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
}
