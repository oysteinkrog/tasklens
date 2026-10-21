//! Settings and state schema, canonical table (a) (ADR-011). No file I/O.
//!
//! `tl-app` loads and saves `settings.toml` and `state.toml`; this module only defines their
//! shape, defaults and clamp rules. TOML section names are the `Settings` and `UiState` field
//! names and keys are the struct field names.

use std::collections::BTreeMap;
use std::ops::{Add, Div, Mul};

use serde::{Deserialize, Serialize};

use crate::actions::DumpMode;
use crate::command::Tab;
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

/// `performance.overview_core_smoothing` (UI-PRF-018, D-024): the time constant of the moving
/// average the Overview core rows show.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoreSmoothing {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl CoreSmoothing {
    /// The time constant in seconds; 0 for `Off` (raw samples).
    pub const fn tau_s(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Low => 0.5,
            Self::Medium => 1.0,
            Self::High => 2.0,
        }
    }
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
    /// "Save power on battery": while on battery, CPU updates drop from 10 Hz to 1 Hz and
    /// charts stop sliding (wlf.32).
    pub save_power_on_battery: bool,
}

impl Default for MotionSettings {
    fn default() -> Self {
        Self {
            animations: true,
            smooth_graphs: true,
            smooth_scroll: true,
            smooth_scroll_amount: 15,
            tab_switch_ms: 250,
            save_power_on_battery: true,
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
    /// Smoothing of the Overview core rows (UI-PRF-018, D-024).
    pub overview_core_smoothing: CoreSmoothing,
    /// The old opt-in Top processes switch (crw.45). Read once by [`Settings::clamp`], which
    /// turns `true` into an `overview` layout, and never written (UI-PRF-018).
    #[serde(skip_serializing)]
    pub overview_top_processes: bool,
    /// `[performance.overview]`: the Overview page layout (UI-PRF-018, D-023). `None` is the
    /// default layout. A value of the wrong type anywhere in it is dropped by [`Settings::clamp`]
    /// with the part it is in, so it never resets the rest of `[performance]`.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "loose_some"
    )]
    pub overview: Option<OverviewLayoutSetting>,
}

/// The key [`Settings::clamp`] returns when it dropped invalid parts of `[performance.overview]`.
/// The load step logs it with `warn!` (UI-PRF-018), not as a clamped value.
pub const OVERVIEW_KEY: &str = "performance.overview";

/// The kinds `[performance.overview]` accepts, in default order.
pub const OVERVIEW_BOX_KINDS: [&str; 6] = ["cpu", "memory", "gpu", "disk", "net", "processes"];

/// The smallest and largest Overview weight. Load and save clamp into this range; the minimum
/// matches the 3 decimals a weight is written with, so a saved layout always reloads.
pub const OVERVIEW_WEIGHT_RANGE: (f64, f64) = (0.001, 1000.);

/// `[performance.overview]` (UI-PRF-018): rows top to bottom, each with its boxes left to right.
/// Weights are shares, not pixels. `tl-ui` turns this into its layout.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OverviewLayoutSetting {
    #[serde(deserialize_with = "loose_vec")]
    pub rows: Vec<OverviewRowSetting>,
}

/// One Overview row: its share of the page height and its boxes.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OverviewRowSetting {
    #[serde(serialize_with = "serialize_weight", deserialize_with = "loose")]
    pub height: f64,
    #[serde(deserialize_with = "loose_vec")]
    pub boxes: Vec<OverviewBoxSetting>,
}

/// One Overview box: a kind from [`OVERVIEW_BOX_KINDS`] and its share of the row width. The
/// kind is a string so an unknown kind drops only this box, not the whole section.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OverviewBoxSetting {
    #[serde(deserialize_with = "loose")]
    pub kind: String,
    #[serde(serialize_with = "serialize_weight", deserialize_with = "loose")]
    pub width: f64,
    /// The box's layout mode, an id from [`OVERVIEW_BOX_MODES`] for its kind (UI-PRF-019, D-025).
    /// `None` is the kind's default mode.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "loose_some"
    )]
    pub mode: Option<String>,
    /// The share of the first part in a mode with two parts, in [`OVERVIEW_SPLIT_RANGE`]
    /// (UI-PRF-019). `None` is the mode's default split.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_split",
        deserialize_with = "loose_some"
    )]
    pub split: Option<f64>,
    /// The crw.63 name of the CPU box's `split` (UI-PRF-018 "CPU box splitter"). Loading moves
    /// it into `split`; it is never written.
    #[serde(skip_serializing, deserialize_with = "loose_some")]
    pub cpu_split: Option<f64>,
}

/// The layout modes of each Overview box kind (UI-PRF-019), the default mode first.
pub const OVERVIEW_BOX_MODES: [(&str, &[&str]); 6] = [
    ("cpu", &["chart_cores", "chart", "cores", "core_charts"]),
    ("memory", &["rows", "chart_rows", "chart"]),
    ("gpu", &["engine_charts", "chart_engines", "chart"]),
    ("disk", &["disk_charts", "rows", "total_chart"]),
    ("net", &["columns", "combined", "adapter_charts"]),
    ("processes", &["list"]),
];

/// The range a box's `split` is accepted in.
pub const OVERVIEW_SPLIT_RANGE: (f64, f64) = (0.2, 0.8);

/// The modes of `kind` in [`OVERVIEW_BOX_MODES`]; none for an unknown kind.
fn modes_of(kind: &str) -> &'static [&'static str] {
    OVERVIEW_BOX_MODES
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or(&[], |(_, modes)| *modes)
}

/// Writes a split rounded to 3 decimals.
fn serialize_split<S: serde::Serializer>(w: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match w {
        Some(w) => s.serialize_f64((w * 1000.).round() / 1000.),
        None => s.serialize_none(),
    }
}

/// Writes a weight clamped to [`OVERVIEW_WEIGHT_RANGE`] and rounded to 3 decimals.
fn serialize_weight<S: serde::Serializer>(w: &f64, s: S) -> Result<S::Ok, S::Error> {
    let (min, max) = OVERVIEW_WEIGHT_RANGE;
    s.serialize_f64((w.clamp(min, max) * 1000.).round() / 1000.)
}

/// A value of the right type, or anything else. Lets one bad part of `[performance.overview]`
/// fail alone instead of failing the whole `[performance]` section.
#[derive(Deserialize)]
#[serde(untagged)]
enum Loose<T> {
    Value(T),
    Bad(serde::de::IgnoredAny),
}

/// The value a part of the wrong type loads as. [`OverviewLayoutSetting::drop_invalid`] always
/// drops it, so a type error drops its part and logs the same warn line as a bad value.
trait Unreadable {
    fn unreadable() -> Self;
}

impl Unreadable for f64 {
    fn unreadable() -> Self {
        f64::NAN
    }
}

impl Unreadable for String {
    /// Not a kind.
    fn unreadable() -> Self {
        String::new()
    }
}

impl Unreadable for OverviewBoxSetting {
    fn unreadable() -> Self {
        Self {
            kind: String::unreadable(),
            width: 1.,
            mode: None,
            split: None,
            cpu_split: None,
        }
    }
}

impl Unreadable for OverviewRowSetting {
    fn unreadable() -> Self {
        Self {
            height: f64::unreadable(),
            boxes: Vec::new(),
        }
    }
}

impl Unreadable for OverviewLayoutSetting {
    fn unreadable() -> Self {
        Self {
            rows: vec![OverviewRowSetting::unreadable()],
        }
    }
}

fn loose<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Unreadable,
{
    Ok(match Loose::deserialize(d)? {
        Loose::Value(v) => v,
        Loose::Bad(_) => T::unreadable(),
    })
}

fn loose_some<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Unreadable,
{
    loose(d).map(Some)
}

/// A list where each item of the wrong type is unreadable on its own. A value that is not a
/// list is one unreadable item.
fn loose_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Unreadable,
{
    Ok(match Loose::<Vec<Loose<T>>>::deserialize(d)? {
        Loose::Value(items) => items
            .into_iter()
            .map(|item| match item {
                Loose::Value(v) => v,
                Loose::Bad(_) => T::unreadable(),
            })
            .collect(),
        Loose::Bad(_) => vec![T::unreadable()],
    })
}

impl OverviewLayoutSetting {
    /// The default layout: CPU (width 2) and Memory on a row of height 3, then GPU, Disk and Net
    /// on a row of height 2.
    pub fn btop_default() -> Self {
        let row = |height, boxes: &[(&str, f64)]| OverviewRowSetting {
            height,
            boxes: boxes
                .iter()
                .map(|&(kind, width)| OverviewBoxSetting {
                    kind: kind.to_owned(),
                    width,
                    ..OverviewBoxSetting::default()
                })
                .collect(),
        };
        Self {
            rows: vec![
                row(3., &[("cpu", 2.), ("memory", 1.)]),
                row(2., &[("gpu", 1.), ("disk", 1.), ("net", 1.)]),
            ],
        }
    }

    /// The default plus a third row of height 2 with Top processes, the layout an old
    /// `overview_top_processes = true` migrates to.
    pub fn with_top_procs() -> Self {
        let mut layout = Self::btop_default();
        layout.rows.push(OverviewRowSetting {
            height: 2.,
            boxes: vec![OverviewBoxSetting {
                kind: "processes".to_owned(),
                width: 1.,
                ..OverviewBoxSetting::default()
            }],
        });
        layout
    }

    /// Drops unknown and repeated kinds, weights that are NaN, infinite, zero or negative, and
    /// rows left with no box. Clamps the other weights into [`OVERVIEW_WEIGHT_RANGE`]. Returns
    /// `true` when it dropped or clamped anything.
    pub fn drop_invalid(&mut self) -> bool {
        let mut changed = false;
        let mut seen: Vec<&'static str> = Vec::new();
        self.rows.retain_mut(|row| {
            let Some(clamped) = fix_weight(&mut row.height) else {
                changed = true;
                return false;
            };
            changed |= clamped;
            row.boxes.retain_mut(|b| {
                let kind = OVERVIEW_BOX_KINDS.iter().copied().find(|k| *k == b.kind);
                let fixed = kind
                    .filter(|k| !seen.contains(k))
                    .zip(fix_weight(&mut b.width));
                let Some((kind, clamped)) = fixed else {
                    changed = true;
                    return false;
                };
                changed |= clamped;
                changed |= b.drop_invalid_mode(kind);
                seen.push(kind);
                true
            });
            if row.boxes.is_empty() {
                changed = true;
                return false;
            }
            true
        });
        changed
    }
}

impl OverviewBoxSetting {
    /// Moves a CPU `cpu_split` into `split`, and drops a mode that `kind` does not have and a
    /// split outside [`OVERVIEW_SPLIT_RANGE`] (UI-PRF-019). Returns `true` when it dropped
    /// anything; the move alone is not a change.
    fn drop_invalid_mode(&mut self, kind: &str) -> bool {
        let mut changed = false;
        if let Some(legacy) = self.cpu_split.take() {
            if kind == "cpu" {
                self.split = self.split.or(Some(legacy));
            } else {
                changed = true;
            }
        }
        if self
            .mode
            .as_deref()
            .is_some_and(|m| !modes_of(kind).contains(&m))
        {
            self.mode = None;
            changed = true;
        }
        let (lo, hi) = OVERVIEW_SPLIT_RANGE;
        if self.split.is_some_and(|v| !(lo..=hi).contains(&v)) {
            self.split = None;
            changed = true;
        }
        changed
    }
}

/// `None` for a weight that is NaN, infinite, zero or negative. Otherwise clamps it into
/// [`OVERVIEW_WEIGHT_RANGE`] and returns whether it changed.
fn fix_weight(w: &mut f64) -> Option<bool> {
    if !w.is_finite() || *w <= 0. {
        return None;
    }
    let (min, max) = OVERVIEW_WEIGHT_RANGE;
    let clamped = w.clamp(min, max);
    let changed = clamped != *w;
    *w = clamped;
    Some(changed)
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
            overview_core_smoothing: CoreSmoothing::default(),
            overview_top_processes: false,
            overview: None,
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

/// `[window]` in `state.toml`. The main window position is never restored (ADR-010).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    /// Logical px, at least 800.
    pub width: u32,
    /// Logical px, at least 500.
    pub height: u32,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            maximized: false,
        }
    }
}

/// `[tabs]` in `state.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabsState {
    /// Opened at start when `general.start_tab = "last_used"`.
    pub last_tab: Tab,
}

impl Default for TabsState {
    fn default() -> Self {
        Self {
            last_tab: Tab::Processes,
        }
    }
}

/// Group expand state of the Processes table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupExpandedProcesses {
    pub apps: bool,
    pub background: bool,
    pub system: bool,
}

impl Default for GroupExpandedProcesses {
    fn default() -> Self {
        Self {
            apps: true,
            background: true,
            system: true,
        }
    }
}

/// `[processes]` in `state.toml`.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessesState {
    pub history_mode: bool,
    pub group_expanded: GroupExpandedProcesses,
    /// Consent flag for the VirusTotal lookup (R-095).
    pub virustotal_notice_accepted: bool,
}

/// Group expand state of the Services table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupExpandedServices {
    pub system: bool,
    pub user: bool,
}

impl Default for GroupExpandedServices {
    fn default() -> Self {
        Self {
            system: true,
            user: true,
        }
    }
}

/// `[services]` in `state.toml`.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServicesState {
    pub group_expanded: GroupExpandedServices,
}

/// `[performance]` in `state.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceState {
    /// Sidebar width in logical px, 180..=400.
    pub nav_width: u16,
    /// Device key.
    pub selected_device: String,
}

impl Default for PerformanceState {
    fn default() -> Self {
        Self {
            nav_width: 252,
            selected_device: "cpu".to_owned(),
        }
    }
}

/// Sort state of one table. `column` is a column id.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SortState {
    pub column: String,
    pub ascending: bool,
}

impl SortState {
    fn new(column: &str, ascending: bool) -> Self {
        Self {
            column: column.to_owned(),
            ascending,
        }
    }
}

/// `[sort.*]`, one entry per table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SortTables {
    pub processes: SortState,
    pub history: SortState,
    pub services: SortState,
    pub startup: SortState,
    pub connections: SortState,
}

impl Default for SortTables {
    fn default() -> Self {
        Self {
            processes: SortState::new("pid", true),
            history: SortState::new("cpu", false),
            services: SortState::new("name", true),
            startup: SortState::new("status", true),
            connections: SortState::new("proto", true),
        }
    }
}

/// Column layout of one table. Ids stay strings here; the UI and the loader convert them to
/// `ColumnId` and drop unknown ids (ADR-011).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColumnState {
    pub order: Vec<String>,
    pub visible: Vec<String>,
    /// Column id to width in logical px.
    pub widths: BTreeMap<String, u32>,
}

impl ColumnState {
    /// `order` and `visible` both get `columns`.
    fn new(columns: &[&str], widths: &[(&str, u32)]) -> Self {
        let list: Vec<String> = columns.iter().map(|&c| c.to_owned()).collect();
        Self {
            order: list.clone(),
            visible: list,
            widths: widths.iter().map(|&(c, w)| (c.to_owned(), w)).collect(),
        }
    }
}

/// `[columns.*]`, one entry per table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColumnTables {
    pub processes: ColumnState,
    pub history: ColumnState,
    pub services: ColumnState,
    pub startup: ColumnState,
    pub connections: ColumnState,
}

impl Default for ColumnTables {
    fn default() -> Self {
        Self {
            processes: ColumnState::new(
                &[
                    "name",
                    "user",
                    "pid",
                    "cpu",
                    "memory",
                    "disk",
                    "network",
                    "gpu",
                    "description",
                ],
                &[
                    ("name", 248),
                    ("user", 92),
                    ("pid", 96),
                    ("cpu", 63),
                    ("memory", 92),
                    ("disk", 82),
                    ("network", 92),
                    ("gpu", 72),
                    ("description", 342),
                    ("power", 101),
                ],
            ),
            // UI-PRC-031; `image_path` is the R-007 id.
            history: ColumnState::new(
                &[
                    "name",
                    "pid",
                    "user",
                    "image_path",
                    "cpu",
                    "memory",
                    "disk",
                    "network",
                    "gpu",
                ],
                &[
                    ("name", 248),
                    ("pid", 96),
                    ("user", 92),
                    ("image_path", 200),
                    ("cpu", 63),
                    ("memory", 92),
                    ("disk", 82),
                    ("network", 92),
                    ("gpu", 72),
                ],
            ),
            // SYS-003.
            services: ColumnState::new(
                &[
                    "name",
                    "display_name",
                    "status",
                    "startup_type",
                    "pid",
                    "user",
                    "path",
                ],
                &[
                    ("name", 250),
                    ("display_name", 300),
                    ("status", 100),
                    ("startup_type", 120),
                    ("pid", 80),
                    ("user", 200),
                    ("path", 260),
                ],
            ),
            // SYS-020.
            startup: ColumnState::new(
                &[
                    "name",
                    "publisher",
                    "status",
                    "impact",
                    "type",
                    "location",
                    "command",
                ],
                &[
                    ("name", 220),
                    ("publisher", 180),
                    ("status", 90),
                    ("impact", 100),
                    ("type", 190),
                    ("location", 260),
                    ("command", 170),
                ],
            ),
            // SYS-029.
            connections: ColumnState::new(
                &["proto", "local", "foreign", "state", "pid", "process"],
                &[
                    ("proto", 90),
                    ("local", 260),
                    ("foreign", 260),
                    ("state", 120),
                    ("pid", 80),
                    ("process", 180),
                ],
            ),
        }
    }
}

/// One `[[minigraphs]]` card. Process sources are `process:<boot_id 16 hex>:<pid>:<start_ticks>`
/// (R-100); command lines are never stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MinigraphState {
    pub source: String,
    /// Column id for process sources.
    pub column: String,
    pub range_s: u16,
    /// `"accent"` or a lower-case colour name.
    pub color: String,
    pub slot: u16,
    pub detached: bool,
    /// Detached window id; ignored when `detached` is false (R-084).
    pub window: u32,
}

impl Default for MinigraphState {
    fn default() -> Self {
        Self {
            source: "cpu".to_owned(),
            column: "cpu".to_owned(),
            range_s: 60,
            color: "accent".to_owned(),
            slot: 0,
            detached: false,
            window: 0,
        }
    }
}

/// One `[[detached_windows]]` entry (P3, R-084). `x` and `y` are written and applied on KDE
/// only, through the KWin bridge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetachedWindowState {
    pub id: u32,
    /// Logical px.
    pub width: u32,
    /// Logical px.
    pub height: u32,
    pub x: i32,
    pub y: i32,
}

impl Default for DetachedWindowState {
    fn default() -> Self {
        Self {
            id: 0,
            width: 180,
            height: 90,
            x: 0,
            y: 0,
        }
    }
}

/// One `[[tray_graphs]]` entry (P3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayGraphState {
    pub source: String,
    pub column: String,
    pub color: String,
    pub range_s: u16,
}

impl Default for TrayGraphState {
    fn default() -> Self {
        Self {
            source: "cpu".to_owned(),
            column: "cpu".to_owned(),
            color: "accent".to_owned(),
            range_s: 60,
        }
    }
}

/// `[lowmem]` in `state.toml` (P3, R-152 OI-7).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LowmemState {
    /// The OOMScoreAdjust the installed watcher unit accepted: -900, or 0 when the user manager
    /// refused -900 (U-24). Left out of the file when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oom_score_adjust: Option<i32>,
}

/// The whole of `state.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub schema_version: u32,
    pub window: WindowState,
    pub tabs: TabsState,
    pub processes: ProcessesState,
    pub services: ServicesState,
    pub performance: PerformanceState,
    /// P3.
    pub lowmem: LowmemState,
    pub sort: SortTables,
    pub columns: ColumnTables,
    pub minigraphs: Vec<MinigraphState>,
    /// P3.
    pub detached_windows: Vec<DetachedWindowState>,
    /// P3.
    pub tray_graphs: Vec<TrayGraphState>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            window: WindowState::default(),
            tabs: TabsState::default(),
            processes: ProcessesState::default(),
            services: ServicesState::default(),
            performance: PerformanceState::default(),
            lowmem: LowmemState::default(),
            sort: SortTables::default(),
            columns: ColumnTables::default(),
            minigraphs: Vec::new(),
            detached_windows: Vec::new(),
            tray_graphs: Vec::new(),
        }
    }
}

/// The settings the engine uses, sent with `EngineClient::set_settings`. Derived from
/// `Settings`, never persisted.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineSettings {
    /// `processes.interval_ms`.
    pub interval_ms: u32,
    /// `units.network`.
    pub network: NetworkUnit,
    /// `units.network_auto_scale`: the process table sorts network rates at the shown step.
    pub network_auto_scale: bool,
    pub show_network_wired: bool,
    pub show_network_wireless: bool,
    pub show_network_vpn: bool,
    pub show_network_virtual: bool,
    pub show_network_other: bool,
    pub history: HistorySettings,
    /// `processes.subtree_totals`.
    pub subtree_totals: bool,
    /// `processes.hide_system`.
    pub hide_system: bool,
}

impl From<&Settings> for EngineSettings {
    fn from(s: &Settings) -> Self {
        let p = &s.performance;
        Self {
            interval_ms: s.processes.interval_ms,
            network: s.units.network,
            network_auto_scale: s.units.network_auto_scale,
            show_network_wired: p.show_network_wired,
            show_network_wireless: p.show_network_wireless,
            show_network_vpn: p.show_network_vpn,
            show_network_virtual: p.show_network_virtual,
            show_network_other: p.show_network_other,
            history: s.history.clone(),
            subtree_totals: s.processes.subtree_totals,
            hide_system: s.processes.hide_system,
        }
    }
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self::from(&Settings::default())
    }
}

/// Clamps `value` into `min..=max`, then rounds it to the nearest multiple of `step` (ties
/// round up). Pushes `key` to `changed` when the value changes.
fn clamp_key<T>(
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

impl Settings {
    /// Load step 4: clamp out-of-range values, return the dotted keys that changed (the caller
    /// logs them). Valid values are left alone. Also migrates `overview_top_processes = true`
    /// once and drops the invalid parts of `[performance.overview]` (UI-PRF-018).
    pub fn clamp(&mut self) -> Vec<&'static str> {
        let mut changed = Vec::new();
        let c = &mut changed;
        let a = &mut self.appearance;
        clamp_key(
            &mut a.ui_scale_pct,
            (75, 200, 5),
            "appearance.ui_scale_pct",
            c,
        );
        clamp_key(
            &mut a.row_height_pct,
            (80, 150, 5),
            "appearance.row_height_pct",
            c,
        );
        clamp_key(
            &mut a.heat_intensity_pct,
            (0, 100, 1),
            "appearance.heat_intensity_pct",
            c,
        );
        clamp_key(
            &mut a.shadow_intensity_pct,
            (0, 100, 1),
            "appearance.shadow_intensity_pct",
            c,
        );
        let m = &mut self.motion;
        clamp_key(
            &mut m.smooth_scroll_amount,
            (1, 50, 1),
            "motion.smooth_scroll_amount",
            c,
        );
        clamp_key(
            &mut m.tab_switch_ms,
            (0, 500, 10),
            "motion.tab_switch_ms",
            c,
        );
        clamp_key(
            &mut self.processes.interval_ms,
            (250, 10_000, 1),
            "processes.interval_ms",
            c,
        );
        if !GRAPH_RANGES_S.contains(&self.performance.graph_range_s) {
            self.performance.graph_range_s = 60;
            c.push("performance.graph_range_s");
        }
        clamp_key(
            &mut self.history.retention_h,
            (1, 12, 1),
            "history.retention_h",
            c,
        );
        if !LANGUAGES.contains(&self.general.language.as_str()) {
            self.general.language = "system".to_owned();
            c.push("general.language");
        }
        if self.services.unit_types.is_empty() {
            self.services.unit_types = UnitType::DEFAULT_SHOWN.to_vec();
            c.push("services.unit_types");
        }
        self.performance.migrate_overview(c);
        changed
    }
}

impl PerformanceSettings {
    /// A profile with `overview_top_processes = true` and no `overview` gets the default plus a
    /// Top processes row; the old key is then cleared and never written. An invalid part of
    /// `overview` is dropped and [`OVERVIEW_KEY`] is pushed to `changed` once, for one `warn!`
    /// line from the load step; with nothing valid left the layout is the default.
    fn migrate_overview(&mut self, changed: &mut Vec<&'static str>) {
        if self.overview_top_processes && self.overview.is_none() {
            self.overview = Some(OverviewLayoutSetting::with_top_procs());
        }
        self.overview_top_processes = false;
        if let Some(layout) = &mut self.overview {
            if layout.drop_invalid() {
                changed.push(OVERVIEW_KEY);
            }
            if layout.rows.is_empty() {
                self.overview = None;
            }
        }
    }
}

impl UiState {
    /// Load step 4 for `state.toml`: clamp out-of-range values, return the dotted keys that
    /// changed (the caller logs them).
    pub fn clamp(&mut self) -> Vec<&'static str> {
        let mut changed = Vec::new();
        let c = &mut changed;
        clamp_key(
            &mut self.window.width,
            (800, u32::MAX, 1),
            "window.width",
            c,
        );
        clamp_key(
            &mut self.window.height,
            (500, u32::MAX, 1),
            "window.height",
            c,
        );
        clamp_key(
            &mut self.performance.nav_width,
            (180, 400, 1),
            "performance.nav_width",
            c,
        );
        // OOMScoreAdjust is -1000 to 1000 (proc(5)).
        if let Some(oom) = &mut self.lowmem.oom_score_adjust {
            clamp_key(oom, (-1000, 1000, 1), "lowmem.oom_score_adjust", c);
        }
        changed
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
        assert!(s.motion.save_power_on_battery);
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
        assert_eq!(p.overview_core_smoothing, CoreSmoothing::Medium);
        assert!(!p.overview_top_processes);
        assert_eq!(p.overview, None);

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

    #[test]
    fn ui_state_defaults_table_a() {
        let s = UiState::default();
        assert_eq!(s.schema_version, 1);
        assert_eq!(
            s.window,
            WindowState {
                width: 1280,
                height: 800,
                maximized: false
            }
        );
        assert_eq!(s.tabs.last_tab, Tab::Processes);
        assert!(!s.processes.history_mode);
        let g = &s.processes.group_expanded;
        assert!(g.apps && g.background && g.system);
        assert!(!s.processes.virustotal_notice_accepted);
        let g = &s.services.group_expanded;
        assert!(g.system && g.user);
        assert_eq!(s.performance.nav_width, 252);
        assert_eq!(s.performance.selected_device, "cpu");

        let sort = |st: &SortState| (st.column.clone(), st.ascending);
        assert_eq!(sort(&s.sort.processes), ("pid".to_owned(), true));
        assert_eq!(sort(&s.sort.history), ("cpu".to_owned(), false));
        assert_eq!(sort(&s.sort.services), ("name".to_owned(), true));
        assert_eq!(sort(&s.sort.startup), ("status".to_owned(), true));
        assert_eq!(sort(&s.sort.connections), ("proto".to_owned(), true));

        assert!(s.minigraphs.is_empty());
        assert!(s.detached_windows.is_empty());
        assert!(s.tray_graphs.is_empty());

        let m = MinigraphState::default();
        assert_eq!(
            (
                m.source.as_str(),
                m.column.as_str(),
                m.range_s,
                m.color.as_str()
            ),
            ("cpu", "cpu", 60, "accent")
        );
        assert_eq!((m.slot, m.detached, m.window), (0, false, 0));
        let d = DetachedWindowState::default();
        assert_eq!((d.id, d.width, d.height, d.x, d.y), (0, 180, 90, 0, 0));
        let t = TrayGraphState::default();
        assert_eq!(
            (
                t.source.as_str(),
                t.column.as_str(),
                t.color.as_str(),
                t.range_s
            ),
            ("cpu", "cpu", "accent", 60)
        );
    }

    fn assert_columns(state: &ColumnState, columns: &[&str], widths: &[(&str, u32)]) {
        assert_eq!(state.order, columns);
        assert_eq!(state.visible, columns);
        let expected: BTreeMap<String, u32> =
            widths.iter().map(|&(c, w)| (c.to_owned(), w)).collect();
        assert_eq!(state.widths, expected);
    }

    #[test]
    fn column_defaults_all_tables() {
        let t = ColumnTables::default();
        assert_columns(
            &t.processes,
            &[
                "name",
                "user",
                "pid",
                "cpu",
                "memory",
                "disk",
                "network",
                "gpu",
                "description",
            ],
            &[
                ("name", 248),
                ("user", 92),
                ("pid", 96),
                ("cpu", 63),
                ("memory", 92),
                ("disk", 82),
                ("network", 92),
                ("gpu", 72),
                ("description", 342),
                ("power", 101),
            ],
        );
        assert_columns(
            &t.history,
            &[
                "name",
                "pid",
                "user",
                "image_path",
                "cpu",
                "memory",
                "disk",
                "network",
                "gpu",
            ],
            &[
                ("name", 248),
                ("pid", 96),
                ("user", 92),
                ("image_path", 200),
                ("cpu", 63),
                ("memory", 92),
                ("disk", 82),
                ("network", 92),
                ("gpu", 72),
            ],
        );
        assert_columns(
            &t.services,
            &[
                "name",
                "display_name",
                "status",
                "startup_type",
                "pid",
                "user",
                "path",
            ],
            &[
                ("name", 250),
                ("display_name", 300),
                ("status", 100),
                ("startup_type", 120),
                ("pid", 80),
                ("user", 200),
                ("path", 260),
            ],
        );
        assert_columns(
            &t.startup,
            &[
                "name",
                "publisher",
                "status",
                "impact",
                "type",
                "location",
                "command",
            ],
            &[
                ("name", 220),
                ("publisher", 180),
                ("status", 90),
                ("impact", 100),
                ("type", 190),
                ("location", 260),
                ("command", 170),
            ],
        );
        assert_columns(
            &t.connections,
            &["proto", "local", "foreign", "state", "pid", "process"],
            &[
                ("proto", 90),
                ("local", 260),
                ("foreign", 260),
                ("state", 120),
                ("pid", 80),
                ("process", 180),
            ],
        );
        assert_eq!(UiState::default().columns, t);
    }

    #[test]
    fn settings_clamp_out_of_range() {
        let mut s = Settings::default();
        s.appearance.ui_scale_pct = 203;
        s.appearance.row_height_pct = 79;
        s.motion.tab_switch_ms = 247;
        s.processes.interval_ms = 100;
        s.performance.graph_range_s = 45;
        s.history.retention_h = 0;
        s.general.language = "xx-XX".to_owned();

        let mut changed = s.clamp();

        assert_eq!(s.appearance.ui_scale_pct, 200);
        assert_eq!(s.appearance.row_height_pct, 80);
        assert_eq!(s.motion.tab_switch_ms, 250);
        assert_eq!(s.processes.interval_ms, 250);
        assert_eq!(s.performance.graph_range_s, 60);
        assert_eq!(s.history.retention_h, 1);
        assert_eq!(s.general.language, "system");
        changed.sort_unstable();
        assert_eq!(
            changed,
            [
                "appearance.row_height_pct",
                "appearance.ui_scale_pct",
                "general.language",
                "history.retention_h",
                "motion.tab_switch_ms",
                "performance.graph_range_s",
                "processes.interval_ms",
            ]
        );
    }

    #[test]
    fn settings_clamp_other_keys() {
        let mut s = Settings::default();
        s.appearance.ui_scale_pct = 112;
        s.appearance.heat_intensity_pct = 101;
        s.appearance.shadow_intensity_pct = 255;
        s.motion.smooth_scroll_amount = 0;
        s.motion.tab_switch_ms = 900;
        s.processes.interval_ms = 20_000;
        s.history.retention_h = 13;
        s.services.unit_types.clear();

        let changed = s.clamp();

        assert_eq!(s.appearance.ui_scale_pct, 110);
        assert_eq!(s.appearance.heat_intensity_pct, 100);
        assert_eq!(s.appearance.shadow_intensity_pct, 100);
        assert_eq!(s.motion.smooth_scroll_amount, 1);
        assert_eq!(s.motion.tab_switch_ms, 500);
        assert_eq!(s.processes.interval_ms, 10_000);
        assert_eq!(s.history.retention_h, 12);
        assert_eq!(s.services.unit_types, UnitType::DEFAULT_SHOWN);
        assert_eq!(changed.len(), 8);
        assert!(changed.contains(&"services.unit_types"));
    }

    #[test]
    fn settings_clamp_noop_on_defaults() {
        let mut s = Settings::default();
        assert!(s.clamp().is_empty());
        assert_eq!(s, Settings::default());

        let mut u = UiState::default();
        assert!(u.clamp().is_empty());
        assert_eq!(u, UiState::default());
    }

    #[test]
    fn ui_state_clamp() {
        let mut u = UiState::default();
        u.window.width = 640;
        u.window.height = 300;
        u.performance.nav_width = 500;

        let changed = u.clamp();

        assert_eq!(u.window.width, 800);
        assert_eq!(u.window.height, 500);
        assert_eq!(u.performance.nav_width, 400);
        assert_eq!(
            changed,
            ["window.width", "window.height", "performance.nav_width"]
        );

        u.performance.nav_width = 100;
        assert_eq!(u.clamp(), ["performance.nav_width"]);
        assert_eq!(u.performance.nav_width, 180);
    }

    #[test]
    fn engine_settings_from_settings() {
        let mut s = Settings::default();
        s.processes.interval_ms = 2000;
        s.units.network = NetworkUnit::Bytes;

        let e = EngineSettings::from(&s);

        assert_eq!(e.interval_ms, 2000);
        assert_eq!(e.network, NetworkUnit::Bytes);
        assert_eq!(e.history, HistorySettings::default());
        assert!(e.show_network_wired);
        assert!(e.show_network_wireless);
        assert!(e.show_network_vpn);
        assert!(!e.show_network_virtual);
        assert!(e.show_network_other);
        assert!(!e.subtree_totals);
        assert!(!e.hide_system);
        assert_eq!(EngineSettings::default().interval_ms, 1000);
    }

    /// Loads `text` as `settings.toml` the way load steps 3 and 4 do.
    fn load_toml(text: &str) -> (Settings, Vec<&'static str>) {
        let mut s: Settings = toml::from_str(text).expect("valid TOML");
        let changed = s.clamp();
        (s, changed)
    }

    /// One `cpu` box in a one-row layout, with `extra` added to its entry.
    fn cpu_entry(extra: &str) -> String {
        format!(
            "[performance.overview]\nrows = [{{ height = 1.0, boxes = [{{ kind = \"cpu\", \
             width = 1.0, {extra} }}] }}]\n"
        )
    }

    #[test]
    fn overview_cpu_split_saved_and_clamped() {
        // A split is written with 3 decimals on its box entry and loads back.
        let mut s = Settings::default();
        let mut layout = OverviewLayoutSetting::btop_default();
        layout.rows[0].boxes[0].split = Some(0.456_789);
        s.performance.overview = Some(layout);
        let text = toml::to_string(&s).expect("serializes");
        assert!(text.contains("split = 0.457"), "{text}");
        assert!(!text.contains("cpu_split"), "{text}");
        let (back, changed) = load_toml(&text);
        assert!(changed.is_empty(), "{changed:?}");
        let rows = back.performance.overview.expect("saved").rows;
        assert_eq!(rows[0].boxes[0].split, Some(0.457));
        assert_eq!(rows[0].boxes[1].split, None);
        // The default layout writes no split and no mode.
        let mut s = Settings::default();
        s.performance.overview = Some(OverviewLayoutSetting::btop_default());
        let text = toml::to_string(&s).expect("serializes");
        let (_, overview) = text
            .split_once("[[performance.overview.rows]]")
            .expect("an overview section");
        // Up to the next plain `[section]`; the overview's own tables start with `[[`.
        let overview = overview
            .split("\n[")
            .take_while(|part| !part.starts_with(|c: char| c.is_ascii_lowercase()))
            .collect::<Vec<_>>()
            .join("\n[");
        assert!(
            !overview.contains("split") && !overview.contains("mode"),
            "{overview}"
        );

        // The crw.63 `cpu_split` loads as `split` without a log line, and is written as `split`.
        let (s, changed) = load_toml(&cpu_entry("cpu_split = 0.6"));
        assert!(changed.is_empty(), "{changed:?}");
        let b = &s.performance.overview.as_ref().expect("kept").rows[0].boxes[0];
        assert_eq!((b.split, b.cpu_split), (Some(0.6), None));
        let text = toml::to_string(&s).expect("serializes");
        assert!(
            text.contains("split = 0.6") && !text.contains("cpu_split"),
            "{text}"
        );
        // A saved `split` wins over an old `cpu_split`.
        let (s, _) = load_toml(&cpu_entry("split = 0.3, cpu_split = 0.6"));
        let b = &s.performance.overview.expect("kept").rows[0].boxes[0];
        assert_eq!(b.split, Some(0.3));

        // Out of range and not a number load as the default, with one log line.
        for key in ["split", "cpu_split"] {
            for v in ["0.1", "0.9", "\"x\"", "nan"] {
                let (s, changed) = load_toml(&cpu_entry(&format!("{key} = {v}")));
                assert_eq!(changed, ["performance.overview"], "{key} {v}");
                let b = &s.performance.overview.expect("kept").rows[0].boxes[0];
                assert_eq!((b.split, b.cpu_split), (None, None), "{key} {v}");
            }
        }
        // `split` is per box; an old `cpu_split` on another kind is dropped.
        let (s, changed) = load_toml(
            "[performance.overview]\nrows = [{ height = 1.0, boxes = [{ kind = \"cpu\", width = \
             1.0, cpu_split = 0.5 }, { kind = \"memory\", width = 1.0, cpu_split = 0.5 }, \
             { kind = \"net\", width = 1.0, split = 0.7 }] }]\n",
        );
        assert_eq!(changed, ["performance.overview"]);
        let boxes = &s.performance.overview.expect("kept").rows[0].boxes;
        let splits: Vec<_> = boxes.iter().map(|b| b.split).collect();
        assert_eq!(splits, [Some(0.5), None, Some(0.7)]);
    }

    #[test]
    fn overview_box_mode_unknown_loads_default() {
        // A mode of the box's kind loads and is written back.
        let (s, changed) = load_toml(&cpu_entry("mode = \"core_charts\""));
        assert!(changed.is_empty(), "{changed:?}");
        let b = &s.performance.overview.as_ref().expect("kept").rows[0].boxes[0];
        assert_eq!(b.mode.as_deref(), Some("core_charts"));
        let text = toml::to_string(&s).expect("serializes");
        assert!(text.contains("mode = \"core_charts\""), "{text}");
        // An unknown mode, another kind's mode and a value of the wrong type load as the
        // default (`None`) with one log line; the box and its other keys stay.
        for v in ["\"bogus\"", "\"engine_charts\"", "3", "\"\""] {
            let (s, changed) = load_toml(&cpu_entry(&format!("mode = {v}, split = 0.5")));
            assert_eq!(changed, ["performance.overview"], "{v}");
            let b = &s.performance.overview.expect("kept").rows[0].boxes[0];
            assert_eq!((b.kind.as_str(), b.mode.as_deref()), ("cpu", None), "{v}");
            assert_eq!(b.split, Some(0.5), "{v}");
        }
        // Every kind's list starts with its default and every kind has one.
        for kind in OVERVIEW_BOX_KINDS {
            assert!(!modes_of(kind).is_empty(), "{kind}");
        }
    }

    #[test]
    fn overview_defaults_d026() {
        // D-026: the GPU box defaults to one chart per engine. A saved mode still wins, and
        // the old default is now a saved mode like any other.
        assert_eq!(modes_of("gpu")[0], "engine_charts");
        assert_eq!(modes_of("disk")[0], "disk_charts");
        let gpu = "[performance.overview]\nrows = [{ height = 1.0, boxes = [{ kind = \"gpu\", \
                   width = 1.0, mode = \"chart_engines\" }] }]\n";
        let (s, changed) = load_toml(gpu);
        assert!(changed.is_empty(), "{changed:?}");
        let b = &s.performance.overview.as_ref().expect("kept").rows[0].boxes[0];
        assert_eq!(b.mode.as_deref(), Some("chart_engines"));
    }

    #[test]
    fn overview_layout_roundtrip() {
        let mut s = Settings::default();
        let mut layout = OverviewLayoutSetting::btop_default();
        layout.rows[0].height = 2.123_456;
        layout.rows[0].boxes[0].width = 1. / 3.;
        s.performance.overview = Some(layout);
        let text = toml::to_string(&s).expect("serializes");
        assert!(text.contains("[[performance.overview.rows]]"), "{text}");
        assert!(!text.contains("overview_top_processes"), "{text}");
        let (back, changed) = load_toml(&text);
        assert!(changed.is_empty(), "{changed:?}");
        let rows = &back.performance.overview.as_ref().expect("saved").rows;
        // Weights are written with 3 decimals.
        assert_eq!(rows[0].height, 2.123);
        assert_eq!(rows[0].boxes[0].width, 0.333);
        let kinds: Vec<&str> = rows
            .iter()
            .flat_map(|r| r.boxes.iter().map(|b| b.kind.as_str()))
            .collect();
        assert_eq!(kinds, ["cpu", "memory", "gpu", "disk", "net"]);

        // The spec's inline form loads too; a missing table is the default.
        let (inline, _) = load_toml(
            "[performance.overview]\nrows = [{ height = 3.0, boxes = [{ kind = \"cpu\", width = \
             2.0 }, { kind = \"memory\", width = 1.0 }] }, { height = 2.0, boxes = [{ kind = \
             \"gpu\", width = 1.0 }, { kind = \"disk\", width = 1.0 }, { kind = \"net\", width = \
             1.0 }] }]\n",
        );
        assert_eq!(
            inline.performance.overview,
            Some(OverviewLayoutSetting::btop_default())
        );
        assert_eq!(load_toml("").0.performance.overview, None);
    }

    #[test]
    fn overview_layout_drops_unknown_kind() {
        let (s, changed) = load_toml(
            "[performance]\ngraph_range_s = 30\n[performance.overview]\nrows = [\
             { height = 3.0, boxes = [{ kind = \"cpu\", width = 2.0 }, { kind = \"fans\", width \
             = 1.0 }, { kind = \"cpu\", width = 1.0 }, { kind = \"net\", width = -1.0 }, { kind = \"gpu\", width = 1e39 }] },\
             { height = 2.0, boxes = [{ kind = \"bogus\", width = 1.0 }] },\
             { height = nan, boxes = [{ kind = \"disk\", width = 1.0 }] },\
             { height = 1.0, boxes = [{ kind = \"memory\", width = inf }, { kind = \"disk\", \
             width = 1.0 }] }]\n",
        );
        // The rest of [performance] is kept.
        assert_eq!(s.performance.graph_range_s, 30);
        assert_eq!(changed, ["performance.overview"], "one log line");
        let layout = s.performance.overview.expect("valid rows kept");
        let rows: Vec<(f64, Vec<&str>)> = layout
            .rows
            .iter()
            .map(|r| (r.height, r.boxes.iter().map(|b| b.kind.as_str()).collect()))
            .collect();
        // 1e39 is clamped to 1000, not dropped.
        assert_eq!(rows, [(3., vec!["cpu", "gpu"]), (1., vec!["disk"])]);
        assert_eq!(layout.rows[0].boxes[1].width, 1000.);

        // Nothing valid left: the default.
        let (s, changed) = load_toml(
            "[performance.overview]\nrows = [{ height = 1.0, boxes = [{ kind = \"fans\", width \
             = 1.0 }] }]\n",
        );
        assert_eq!(s.performance.overview, None);
        assert_eq!(changed, ["performance.overview"]);
    }

    #[test]
    fn overview_weights_clamp_to_range() {
        // Tiny and huge weights clamp into 0.001..=1000 with one warn line.
        let (s, changed) = load_toml(
            "[performance.overview]\nrows = [{ height = 1e-50, boxes = [{ kind = \"cpu\", width \
             = 1e38 }, { kind = \"memory\", width = 1e-50 }] }, { height = 1e38, boxes = [{ kind \
             = \"net\", width = 0.0004 }] }]\n",
        );
        assert_eq!(changed, ["performance.overview"], "one log line");
        let rows = s.performance.overview.expect("clamped, not dropped").rows;
        let weights: Vec<(f64, Vec<f64>)> = rows
            .iter()
            .map(|r| (r.height, r.boxes.iter().map(|b| b.width).collect()))
            .collect();
        assert_eq!(weights, [(0.001, vec![1000., 0.001]), (1000., vec![0.001])]);

        // Weights in range are left alone and log nothing.
        let (_, changed) = load_toml(
            "[performance.overview]\nrows = [{ height = 0.001, boxes = [{ kind = \"cpu\", width \
             = 1000 }] }]\n",
        );
        assert!(changed.is_empty(), "{changed:?}");

        // A weight of 0.0004 in memory is saved as 0.001, so the saved layout reloads.
        let mut s = Settings::default();
        let mut layout = OverviewLayoutSetting::btop_default();
        layout.rows[0].height = 0.0004;
        layout.rows[1].boxes[0].width = 5000.;
        s.performance.overview = Some(layout);
        let text = toml::to_string(&s).expect("serializes");
        let (back, changed) = load_toml(&text);
        assert!(changed.is_empty(), "{changed:?}");
        let rows = back.performance.overview.expect("saved").rows;
        assert_eq!(rows[0].height, 0.001);
        assert_eq!(rows[1].boxes[0].width, 1000.);
    }

    #[test]
    fn overview_type_error_drops_only_its_part() {
        // A value of the wrong type drops the part it is in; the rest of [performance] loads.
        let (s, changed) = load_toml(
            "[performance]\ngraph_range_s = 30\ncurved_lines = true\n[performance.overview]\n\
             rows = [{ height = 3.0, boxes = [{ kind = \"cpu\", width = \"bad\" }, { kind = \
             \"memory\", width = 1.0 }, { kind = 5, width = 1.0 }, \"box\"] }, \
             { height = \"tall\", boxes = [{ kind = \"gpu\", width = 1.0 }] }, \
             { height = 1.0, boxes = \"none\" }, 7, \
             { height = 2.0, boxes = [{ kind = \"net\", width = 2 }] }]\n",
        );
        assert_eq!(s.performance.graph_range_s, 30);
        assert!(s.performance.curved_lines);
        assert_eq!(changed, ["performance.overview"], "one log line");
        let layout = s.performance.overview.expect("valid rows kept");
        let rows: Vec<(f64, Vec<(&str, f64)>)> = layout
            .rows
            .iter()
            .map(|r| {
                let boxes = r.boxes.iter().map(|b| (b.kind.as_str(), b.width)).collect();
                (r.height, boxes)
            })
            .collect();
        assert_eq!(rows, [(3., vec![("memory", 1.)]), (2., vec![("net", 2.)])]);

        // Rows or the whole table of the wrong type: the default, with the warn line.
        for text in [
            "[performance]\ngraph_range_s = 30\noverview = 5\n",
            "[performance]\ngraph_range_s = 30\n[performance.overview]\nrows = \"x\"\n",
        ] {
            let (s, changed) = load_toml(text);
            assert_eq!(s.performance.graph_range_s, 30, "{text}");
            assert_eq!(s.performance.overview, None, "{text}");
            assert_eq!(changed, ["performance.overview"], "{text}");
        }
    }

    #[test]
    fn overview_top_processes_migrates() {
        let (s, changed) = load_toml("[performance]\noverview_top_processes = true\n");
        assert!(changed.is_empty(), "{changed:?}");
        assert!(!s.performance.overview_top_processes);
        let layout = s.performance.overview.clone().expect("migrated");
        assert_eq!(layout, OverviewLayoutSetting::with_top_procs());
        let last = layout.rows.last().expect("a third row");
        assert_eq!(last.height, 2.);
        assert_eq!(last.boxes[0].kind, "processes");
        // The old key is not written, so the migration runs once.
        let text = toml::to_string(&s).expect("serializes");
        assert!(!text.contains("overview_top_processes"), "{text}");
        assert_eq!(load_toml(&text).0, s);

        // A saved layout wins over the old key.
        let (s, _) = load_toml(
            "[performance]\noverview_top_processes = true\n[performance.overview]\nrows = [{ \
             height = 1.0, boxes = [{ kind = \"cpu\", width = 1.0 }] }]\n",
        );
        let rows = s.performance.overview.expect("kept").rows;
        assert_eq!(rows.len(), 1);
        // Off: no layout, the default.
        let (s, _) = load_toml("[performance]\noverview_top_processes = false\n");
        assert_eq!(s.performance.overview, None);
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
