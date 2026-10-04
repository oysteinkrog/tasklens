//! Settings and state schema, canonical table (a) (ADR-011). No file I/O.
//!
//! `tl-app` loads and saves `settings.toml`; this module only defines its shape and defaults.
//! TOML section names are the `Settings` field names and keys are the struct field names.

use serde::{Deserialize, Serialize};

use crate::actions::DumpMode;
use crate::table::TableMode;

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

/// `tables.column_resize` (R-075), applied to every table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnResize {
    #[default]
    Proportional,
    LastColumn,
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
