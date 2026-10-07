//! `TlSettings`: the one GPUI Global that holds the parsed `settings.toml` and `state.toml`
//! for the whole UI (ADR-011, shell.md "Conventions", APP-033, R-071).
//!
//! Every settings write in tl-ui goes through `TlSettings::update` (persisting) or
//! `TlSettings::preview` (in memory only). Each settings section has its own version counter, so
//! a view that observes the Global redraws only when a section it draws changed. `update` calls
//! the tl-app persist callback and pushes changed `EngineSettings` to the engine.
//!
//! `SettingKey` and `SettingValue` name one table (a) key and a value for it. The generic
//! `ToggleSetting` and `SetSetting` Actions (E20-002) carry them. Logs name dotted keys only,
//! never values (ADR-015).

use std::sync::Arc;

use gpui_kit::{App, BorrowAppContext, Global, SharedString};
use serde::Deserialize;
use serde::de::IntoDeserializer;
use tl_model::settings::{EngineSettings, Settings, UiState};
use tracing::{debug, warn};

use crate::app::{BootSettings, EngineGlobal, PersistFn};
use crate::i18n;

/// The parsed settings and state.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsData {
    pub settings: Settings,
    pub state: UiState,
}

/// One `settings.toml` section. Each has its own version counter in `TlSettings`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Section {
    General,
    Theme,
    Appearance,
    Motion,
    Units,
    Processes,
    Tables,
    Properties,
    Performance,
    Fans,
    Graphs,
    History,
    Window,
    Tray,
    Startup,
    Network,
    Privilege,
    Rendering,
    Services,
    Lowmem,
    Debug,
}

impl Section {
    /// Every section in table (a) order. The index is `Section as usize`.
    pub const ALL: [Section; 21] = [
        Section::General,
        Section::Theme,
        Section::Appearance,
        Section::Motion,
        Section::Units,
        Section::Processes,
        Section::Tables,
        Section::Properties,
        Section::Performance,
        Section::Fans,
        Section::Graphs,
        Section::History,
        Section::Window,
        Section::Tray,
        Section::Startup,
        Section::Network,
        Section::Privilege,
        Section::Rendering,
        Section::Services,
        Section::Lowmem,
        Section::Debug,
    ];

    /// The TOML section name.
    pub fn as_str(self) -> &'static str {
        match self {
            Section::General => "general",
            Section::Theme => "theme",
            Section::Appearance => "appearance",
            Section::Motion => "motion",
            Section::Units => "units",
            Section::Processes => "processes",
            Section::Tables => "tables",
            Section::Properties => "properties",
            Section::Performance => "performance",
            Section::Fans => "fans",
            Section::Graphs => "graphs",
            Section::History => "history",
            Section::Window => "window",
            Section::Tray => "tray",
            Section::Startup => "startup",
            Section::Network => "network",
            Section::Privilege => "privilege",
            Section::Rendering => "rendering",
            Section::Services => "services",
            Section::Lowmem => "lowmem",
            Section::Debug => "debug",
        }
    }

    /// True when this section is the same in `a` and `b`.
    fn same(self, a: &Settings, b: &Settings) -> bool {
        match self {
            Section::General => a.general == b.general,
            Section::Theme => a.theme == b.theme,
            Section::Appearance => a.appearance == b.appearance,
            Section::Motion => a.motion == b.motion,
            Section::Units => a.units == b.units,
            Section::Processes => a.processes == b.processes,
            Section::Tables => a.tables == b.tables,
            Section::Properties => a.properties == b.properties,
            Section::Performance => a.performance == b.performance,
            Section::Fans => a.fans == b.fans,
            Section::Graphs => a.graphs == b.graphs,
            Section::History => a.history == b.history,
            Section::Window => a.window == b.window,
            Section::Tray => a.tray == b.tray,
            Section::Startup => a.startup == b.startup,
            Section::Network => a.network == b.network,
            Section::Privilege => a.privilege == b.privilege,
            Section::Rendering => a.rendering == b.rendering,
            Section::Services => a.services == b.services,
            Section::Lowmem => a.lowmem == b.lowmem,
            Section::Debug => a.debug == b.debug,
        }
    }
}

/// The settings and state Global.
pub struct TlSettings {
    data: SettingsData,
    /// Index = `Section as usize`.
    versions: [u64; 21],
    /// Bumps on any `UiState` change.
    state_version: u64,
    persist: Option<PersistFn>,
    /// A `preview` changed `data` and no `update` has persisted it yet.
    unpersisted: bool,
    /// The `EngineSettings` the engine has: the boot value, then the last one `update` sent.
    engine_sent: EngineSettings,
    /// `TlSettings` turned `App::reduce_motion` on because `motion.animations` is off. It turns
    /// it off again only in that case, so a reduce-motion set elsewhere stays.
    forced_reduce_motion: bool,
}

impl Global for TlSettings {}

impl TlSettings {
    /// Builds `TlSettings` from the `BootSettings` Global and installs it. Takes the persist
    /// callback; `BootSettings` stays installed.
    pub fn init(cx: &mut App) {
        let boot = cx.global_mut::<BootSettings>();
        let persist = boot.persist.take();
        let data = SettingsData {
            settings: boot.settings.clone(),
            state: boot.state.clone(),
        };
        let engine_sent = EngineSettings::from(&data.settings);
        let forced_reduce_motion = !data.settings.motion.animations && !cx.reduce_motion();
        if forced_reduce_motion {
            cx.set_reduce_motion(true);
        }
        cx.set_global(TlSettings {
            data,
            versions: [1; 21],
            state_version: 1,
            persist,
            unpersisted: false,
            engine_sent,
            forced_reduce_motion,
        });
    }

    pub fn get(cx: &App) -> &TlSettings {
        cx.global::<TlSettings>()
    }

    pub fn settings(cx: &App) -> &Settings {
        &Self::get(cx).data.settings
    }

    pub fn state(cx: &App) -> &UiState {
        &Self::get(cx).data.state
    }

    pub fn version(&self, section: Section) -> u64 {
        self.versions[section as usize]
    }

    pub fn state_version(&self) -> u64 {
        self.state_version
    }

    /// Persisting write: run `f`, clamp, bump the versions of every section that changed,
    /// bump `state_version` if UiState changed, call `persist`, and send EngineSettings if they changed.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut SettingsData)) {
        Self::write(cx, true, f);
    }

    /// In-memory write for live previews (slider Change events): same as `update` but never calls
    /// `persist` and never calls `EngineClient::set_settings`. A later `update` persists the value.
    pub fn preview(cx: &mut App, f: impl FnOnce(&mut SettingsData)) {
        Self::write(cx, false, f);
    }

    /// Flips a Bool key. Any other kind logs a warning and changes nothing.
    pub fn toggle(cx: &mut App, key: SettingKey) {
        if key.kind() != SettingKind::Bool {
            warn!(target: "tasklens::settings", key = key.as_str(), "toggle on non-bool key");
            return;
        }
        Self::update(cx, |d| {
            if let Some(b) = bool_mut(&mut d.settings, key) {
                *b = !*b;
            }
        });
    }

    /// Writes one key. A kind mismatch, an out-of-range integer or a string that names no enum
    /// value logs a warning and changes nothing. `Map` keys always reject.
    pub fn set(cx: &mut App, key: SettingKey, value: SettingValue) {
        let mut rejected = None;
        Self::update(cx, |d| {
            if let Err(why) = assign(&mut d.settings, key, &value) {
                rejected = Some(why);
            }
        });
        if let Some(why) = rejected {
            warn!(target: "tasklens::settings", key = key.as_str(), why, "set rejected");
        }
    }

    fn write(cx: &mut App, persist: bool, f: impl FnOnce(&mut SettingsData)) {
        let current = &cx.global::<TlSettings>().data;
        let mut next = current.clone();
        f(&mut next);
        for key in next.settings.clamp() {
            debug!(target: "tasklens::settings", key, "clamped");
        }
        for key in next.state.clamp() {
            debug!(target: "tasklens::settings", key, "clamped");
        }

        let changed: Vec<Section> = Section::ALL
            .into_iter()
            .filter(|s| !s.same(&current.settings, &next.settings))
            .collect();
        let state_changed = current.state != next.state;
        let language_changed = current.settings.general.language != next.settings.general.language;
        let unpersisted = cx.global::<TlSettings>().unpersisted;
        if changed.is_empty() && !state_changed && !(persist && unpersisted) {
            return;
        }

        let engine = EngineSettings::from(&next.settings);
        let send_engine = persist && engine != cx.global::<TlSettings>().engine_sent;
        cx.update_global::<TlSettings, _>(|tl, cx| {
            for s in &changed {
                tl.versions[*s as usize] += 1;
            }
            if state_changed {
                tl.state_version += 1;
            }
            tl.data = next;
            if persist {
                if let Some(p) = &tl.persist {
                    p(&tl.data.settings, &tl.data.state);
                }
                tl.unpersisted = false;
            } else {
                tl.unpersisted = true;
            }
            tl.sync_reduce_motion(cx);
            if send_engine {
                tl.engine_sent = engine.clone();
            }
        });

        if language_changed {
            let language = TlSettings::settings(cx).general.language.clone();
            i18n::set_language(cx, &language);
        }
        if send_engine && let Some(client) = cx.try_global::<EngineGlobal>() {
            client.0.set_settings(engine);
        }
    }

    /// Mirrors `motion.animations` into `App::reduce_motion`, so the kit components that read
    /// only `reduce_motion` (switch, checkbox, radio, skeleton, progress) go static.
    fn sync_reduce_motion(&mut self, cx: &mut App) {
        let animations = self.data.settings.motion.animations;
        if !animations && !self.forced_reduce_motion && !cx.reduce_motion() {
            cx.set_reduce_motion(true);
            self.forced_reduce_motion = true;
        } else if animations && self.forced_reduce_motion {
            cx.set_reduce_motion(false);
            self.forced_reduce_motion = false;
        }
    }
}

/// The value shape of a `SettingKey`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SettingKind {
    Bool,
    Int,
    Str,
    StrList,
    Map,
}

/// A value for `TlSettings::set`.
#[derive(Clone, PartialEq, Debug)]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Str(SharedString),
    StrList(Arc<[SharedString]>),
}

/// Declares `SettingKey` and its table from one list, so the order, names, sections and kinds
/// cannot drift apart.
macro_rules! setting_keys {
    ($($key:ident => $name:literal, $section:ident, $kind:ident;)*) => {
        /// One canonical table (a) `settings.toml` key.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum SettingKey {
            $($key,)*
        }

        impl SettingKey {
            /// Every key in table (a) order.
            pub const ALL: [SettingKey; 85] = [$(SettingKey::$key,)*];

            /// The dotted name, for example `"motion.animations"`. `Fans` is `"fans"`.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(SettingKey::$key => $name,)*
                }
            }

            pub fn section(self) -> Section {
                match self {
                    $(SettingKey::$key => Section::$section,)*
                }
            }

            pub fn kind(self) -> SettingKind {
                match self {
                    $(SettingKey::$key => SettingKind::$kind,)*
                }
            }
        }
    };
}

setting_keys! {
    GeneralStartTab => "general.start_tab", General, Str;
    GeneralLanguage => "general.language", General, Str;
    ThemeActive => "theme.active", Theme, Str;
    ThemeFollowSystem => "theme.follow_system", Theme, Bool;
    ThemeLight => "theme.light", Theme, Str;
    ThemeDark => "theme.dark", Theme, Str;
    AppearanceUiScalePct => "appearance.ui_scale_pct", Appearance, Int;
    AppearanceRowHeightPct => "appearance.row_height_pct", Appearance, Int;
    AppearanceDecorations => "appearance.decorations", Appearance, Str;
    AppearanceRoundedCorners => "appearance.rounded_corners", Appearance, Bool;
    AppearanceHeatColoring => "appearance.heat_coloring", Appearance, Bool;
    AppearanceHeatIntensityPct => "appearance.heat_intensity_pct", Appearance, Int;
    AppearanceShadowIntensityPct => "appearance.shadow_intensity_pct", Appearance, Int;
    MotionAnimations => "motion.animations", Motion, Bool;
    MotionSmoothGraphs => "motion.smooth_graphs", Motion, Bool;
    MotionSmoothScroll => "motion.smooth_scroll", Motion, Bool;
    MotionSmoothScrollAmount => "motion.smooth_scroll_amount", Motion, Int;
    MotionTabSwitchMs => "motion.tab_switch_ms", Motion, Int;
    UnitsNetwork => "units.network", Units, Str;
    UnitsNetworkAutoScale => "units.network_auto_scale", Units, Bool;
    ProcessesIntervalMs => "processes.interval_ms", Processes, Int;
    ProcessesMode => "processes.mode", Processes, Str;
    ProcessesTreeGuideLines => "processes.tree_guide_lines", Processes, Bool;
    ProcessesHighlightNewExited => "processes.highlight_new_exited", Processes, Bool;
    ProcessesFilterIncludeChildren => "processes.filter_include_children", Processes, Bool;
    ProcessesHideSystem => "processes.hide_system", Processes, Bool;
    ProcessesSubtreeTotals => "processes.subtree_totals", Processes, Bool;
    ProcessesDumpType => "processes.dump_type", Processes, Str;
    TablesColumnResize => "tables.column_resize", Tables, Str;
    TablesColumnSeparators => "tables.column_separators", Tables, Bool;
    TablesDimZeroValues => "tables.dim_zero_values", Tables, Bool;
    PropertiesHideUnnamedHandles => "properties.hide_unnamed_handles", Properties, Bool;
    PropertiesSharedObjectsOnly => "properties.shared_objects_only", Properties, Bool;
    PropertiesEnvMask => "properties.env_mask", Properties, Bool;
    PerformanceGraphRangeS => "performance.graph_range_s", Performance, Int;
    PerformanceCpuGraphMode => "performance.cpu_graph_mode", Performance, Str;
    PerformanceCpuKernel => "performance.cpu_kernel", Performance, Bool;
    PerformanceCpuSecondaryGraph => "performance.cpu_secondary_graph", Performance, Str;
    PerformanceGpuOptionalGraphs => "performance.gpu_optional_graphs", Performance, StrList;
    PerformanceHiddenGpuEngines => "performance.hidden_gpu_engines", Performance, StrList;
    PerformanceMemoryComposition => "performance.memory_composition", Performance, Bool;
    PerformanceMemorySwapGraph => "performance.memory_swap_graph", Performance, Bool;
    PerformanceShowDisks => "performance.show_disks", Performance, Bool;
    PerformanceShowGpus => "performance.show_gpus", Performance, Bool;
    PerformanceShowFans => "performance.show_fans", Performance, Bool;
    PerformanceShowBatteries => "performance.show_batteries", Performance, Bool;
    PerformanceShowNetworkWired => "performance.show_network_wired", Performance, Bool;
    PerformanceShowNetworkWireless => "performance.show_network_wireless", Performance, Bool;
    PerformanceShowNetworkVpn => "performance.show_network_vpn", Performance, Bool;
    PerformanceShowNetworkVirtual => "performance.show_network_virtual", Performance, Bool;
    PerformanceShowNetworkOther => "performance.show_network_other", Performance, Bool;
    PerformanceShowDisconnectedFans => "performance.show_disconnected_fans", Performance, Bool;
    PerformanceDeviceOrder => "performance.device_order", Performance, StrList;
    PerformanceDeviceOverrides => "performance.device_overrides", Performance, Map;
    PerformanceCurvedLines => "performance.curved_lines", Performance, Bool;
    PerformanceNetworkDynamicScale => "performance.network_dynamic_scale", Performance, Bool;
    Fans => "fans", Fans, Map;
    GraphsMinigraphCpu => "graphs.minigraph_cpu", Graphs, Bool;
    GraphsMinigraphMemory => "graphs.minigraph_memory", Graphs, Bool;
    GraphsMinigraphDisk => "graphs.minigraph_disk", Graphs, Bool;
    GraphsMinigraphNetwork => "graphs.minigraph_network", Graphs, Bool;
    GraphsMinigraphGpu => "graphs.minigraph_gpu", Graphs, Bool;
    GraphsMinigraphVram => "graphs.minigraph_vram", Graphs, Bool;
    GraphsMinigraphGpuTemperature => "graphs.minigraph_gpu_temperature", Graphs, Bool;
    GraphsKeepDetachedOnClose => "graphs.keep_detached_on_close", Graphs, Bool;
    HistoryEnabled => "history.enabled", History, Bool;
    HistoryRetentionH => "history.retention_h", History, Int;
    HistoryPersist => "history.persist", History, Bool;
    HistoryMaxDiskMb => "history.max_disk_mb", History, Int;
    WindowCloseWithEsc => "window.close_with_esc", Window, Bool;
    WindowAlwaysOnTop => "window.always_on_top", Window, Bool;
    TrayCloseToTray => "tray.close_to_tray", Tray, Bool;
    TrayMinimizeToTray => "tray.minimize_to_tray", Tray, Bool;
    TrayAlwaysShow => "tray.always_show", Tray, Bool;
    StartupStartAtLogin => "startup.start_at_login", Startup, Bool;
    StartupStartMinimized => "startup.start_minimized", Startup, Bool;
    NetworkAutoFetchPublicIp => "network.auto_fetch_public_ip", Network, Bool;
    NetworkPublicIpNoticeAccepted => "network.public_ip_notice_accepted", Network, Bool;
    PrivilegeUnlockAtLaunch => "privilege.unlock_at_launch", Privilege, Bool;
    PrivilegePasswordless => "privilege.passwordless", Privilege, Bool;
    RenderingGpu => "rendering.gpu", Rendering, Str;
    ServicesUnitTypes => "services.unit_types", Services, StrList;
    ServicesFilterStates => "services.filter_states", Services, StrList;
    LowmemEnabled => "lowmem.enabled", Lowmem, Bool;
    DebugShowFps => "debug.show_fps", Debug, Bool;
}

/// The field of a Bool key. `None` for every other kind.
fn bool_mut(s: &mut Settings, key: SettingKey) -> Option<&mut bool> {
    use SettingKey as K;
    let (p, n) = (&mut s.performance, &mut s.network);
    Some(match key {
        K::ThemeFollowSystem => &mut s.theme.follow_system,
        K::AppearanceRoundedCorners => &mut s.appearance.rounded_corners,
        K::AppearanceHeatColoring => &mut s.appearance.heat_coloring,
        K::MotionAnimations => &mut s.motion.animations,
        K::MotionSmoothGraphs => &mut s.motion.smooth_graphs,
        K::MotionSmoothScroll => &mut s.motion.smooth_scroll,
        K::UnitsNetworkAutoScale => &mut s.units.network_auto_scale,
        K::ProcessesTreeGuideLines => &mut s.processes.tree_guide_lines,
        K::ProcessesHighlightNewExited => &mut s.processes.highlight_new_exited,
        K::ProcessesFilterIncludeChildren => &mut s.processes.filter_include_children,
        K::ProcessesHideSystem => &mut s.processes.hide_system,
        K::ProcessesSubtreeTotals => &mut s.processes.subtree_totals,
        K::TablesColumnSeparators => &mut s.tables.column_separators,
        K::TablesDimZeroValues => &mut s.tables.dim_zero_values,
        K::PropertiesHideUnnamedHandles => &mut s.properties.hide_unnamed_handles,
        K::PropertiesSharedObjectsOnly => &mut s.properties.shared_objects_only,
        K::PropertiesEnvMask => &mut s.properties.env_mask,
        K::PerformanceCpuKernel => &mut p.cpu_kernel,
        K::PerformanceMemoryComposition => &mut p.memory_composition,
        K::PerformanceMemorySwapGraph => &mut p.memory_swap_graph,
        K::PerformanceShowDisks => &mut p.show_disks,
        K::PerformanceShowGpus => &mut p.show_gpus,
        K::PerformanceShowFans => &mut p.show_fans,
        K::PerformanceShowBatteries => &mut p.show_batteries,
        K::PerformanceShowNetworkWired => &mut p.show_network_wired,
        K::PerformanceShowNetworkWireless => &mut p.show_network_wireless,
        K::PerformanceShowNetworkVpn => &mut p.show_network_vpn,
        K::PerformanceShowNetworkVirtual => &mut p.show_network_virtual,
        K::PerformanceShowNetworkOther => &mut p.show_network_other,
        K::PerformanceShowDisconnectedFans => &mut p.show_disconnected_fans,
        K::PerformanceCurvedLines => &mut p.curved_lines,
        K::PerformanceNetworkDynamicScale => &mut p.network_dynamic_scale,
        K::GraphsMinigraphCpu => &mut s.graphs.minigraph_cpu,
        K::GraphsMinigraphMemory => &mut s.graphs.minigraph_memory,
        K::GraphsMinigraphDisk => &mut s.graphs.minigraph_disk,
        K::GraphsMinigraphNetwork => &mut s.graphs.minigraph_network,
        K::GraphsMinigraphGpu => &mut s.graphs.minigraph_gpu,
        K::GraphsMinigraphVram => &mut s.graphs.minigraph_vram,
        K::GraphsMinigraphGpuTemperature => &mut s.graphs.minigraph_gpu_temperature,
        K::GraphsKeepDetachedOnClose => &mut s.graphs.keep_detached_on_close,
        K::HistoryEnabled => &mut s.history.enabled,
        K::HistoryPersist => &mut s.history.persist,
        K::WindowCloseWithEsc => &mut s.window.close_with_esc,
        K::WindowAlwaysOnTop => &mut s.window.always_on_top,
        K::TrayCloseToTray => &mut s.tray.close_to_tray,
        K::TrayMinimizeToTray => &mut s.tray.minimize_to_tray,
        K::TrayAlwaysShow => &mut s.tray.always_show,
        K::StartupStartAtLogin => &mut s.startup.start_at_login,
        K::StartupStartMinimized => &mut s.startup.start_minimized,
        K::NetworkAutoFetchPublicIp => &mut n.auto_fetch_public_ip,
        K::NetworkPublicIpNoticeAccepted => &mut n.public_ip_notice_accepted,
        K::PrivilegeUnlockAtLaunch => &mut s.privilege.unlock_at_launch,
        K::PrivilegePasswordless => &mut s.privilege.passwordless,
        K::LowmemEnabled => &mut s.lowmem.enabled,
        K::DebugShowFps => &mut s.debug.show_fps,
        _ => return None,
    })
}

/// Writes `value` to the field of `key`. On `Err` nothing was written; the text names why.
fn assign(s: &mut Settings, key: SettingKey, value: &SettingValue) -> Result<(), &'static str> {
    use SettingKey as K;
    match (key.kind(), value) {
        (SettingKind::Map, _) => Err("map key"),
        (SettingKind::Bool, SettingValue::Bool(v)) => {
            if let Some(b) = bool_mut(s, key) {
                *b = *v;
            }
            Ok(())
        }
        (SettingKind::Int, SettingValue::Int(v)) => match key {
            K::AppearanceUiScalePct => int(&mut s.appearance.ui_scale_pct, *v),
            K::AppearanceRowHeightPct => int(&mut s.appearance.row_height_pct, *v),
            K::AppearanceHeatIntensityPct => int(&mut s.appearance.heat_intensity_pct, *v),
            K::AppearanceShadowIntensityPct => int(&mut s.appearance.shadow_intensity_pct, *v),
            K::MotionSmoothScrollAmount => int(&mut s.motion.smooth_scroll_amount, *v),
            K::MotionTabSwitchMs => int(&mut s.motion.tab_switch_ms, *v),
            K::ProcessesIntervalMs => int(&mut s.processes.interval_ms, *v),
            K::PerformanceGraphRangeS => int(&mut s.performance.graph_range_s, *v),
            K::HistoryRetentionH => int(&mut s.history.retention_h, *v),
            K::HistoryMaxDiskMb => int(&mut s.history.max_disk_mb, *v),
            _ => Err("kind mismatch"),
        },
        (SettingKind::Str, SettingValue::Str(v)) => match key {
            K::GeneralStartTab => parse(&mut s.general.start_tab, v),
            K::GeneralLanguage => text(&mut s.general.language, v),
            K::ThemeActive => text(&mut s.theme.active, v),
            K::ThemeLight => text(&mut s.theme.light, v),
            K::ThemeDark => text(&mut s.theme.dark, v),
            K::AppearanceDecorations => parse(&mut s.appearance.decorations, v),
            K::UnitsNetwork => parse(&mut s.units.network, v),
            K::ProcessesMode => parse(&mut s.processes.mode, v),
            K::ProcessesDumpType => parse(&mut s.processes.dump_type, v),
            K::TablesColumnResize => parse(&mut s.tables.column_resize, v),
            K::PerformanceCpuGraphMode => parse(&mut s.performance.cpu_graph_mode, v),
            K::PerformanceCpuSecondaryGraph => parse(&mut s.performance.cpu_secondary_graph, v),
            K::RenderingGpu => text(&mut s.rendering.gpu, v),
            _ => Err("kind mismatch"),
        },
        (SettingKind::StrList, SettingValue::StrList(v)) => match key {
            K::PerformanceGpuOptionalGraphs => {
                parse_list(&mut s.performance.gpu_optional_graphs, v)
            }
            K::PerformanceHiddenGpuEngines => text_list(&mut s.performance.hidden_gpu_engines, v),
            K::PerformanceDeviceOrder => text_list(&mut s.performance.device_order, v),
            K::ServicesUnitTypes => parse_list(&mut s.services.unit_types, v),
            K::ServicesFilterStates => parse_list(&mut s.services.filter_states, v),
            _ => Err("kind mismatch"),
        },
        _ => Err("kind mismatch"),
    }
}

/// Assigns `v` when it fits `T`. `Settings::clamp` then applies the table (a) range.
fn int<T: TryFrom<i64>>(field: &mut T, v: i64) -> Result<(), &'static str> {
    *field = T::try_from(v).map_err(|_| "integer out of range")?;
    Ok(())
}

fn text(field: &mut String, v: &SharedString) -> Result<(), &'static str> {
    *field = v.to_string();
    Ok(())
}

fn text_list(field: &mut Vec<String>, v: &[SharedString]) -> Result<(), &'static str> {
    *field = v.iter().map(|s| s.to_string()).collect();
    Ok(())
}

/// Parses an enum by its serde name, for example `"tree"` for `TableMode::Tree`.
fn parse_one<T: for<'de> Deserialize<'de>>(v: &str) -> Result<T, &'static str> {
    T::deserialize(v.into_deserializer()).map_err(|_: serde::de::value::Error| "unknown enum value")
}

fn parse<T: for<'de> Deserialize<'de>>(field: &mut T, v: &str) -> Result<(), &'static str> {
    *field = parse_one(v)?;
    Ok(())
}

fn parse_list<T: for<'de> Deserialize<'de>>(
    field: &mut Vec<T>,
    v: &[SharedString],
) -> Result<(), &'static str> {
    *field = v.iter().map(|s| parse_one(s)).collect::<Result<_, _>>()?;
    Ok(())
}

/// "Animations on": `motion.animations && !cx.reduce_motion()` (R-017, shell.md conventions).
/// The single reduced-motion gate for every shell animation. Before `TlSettings` is installed
/// only `reduce_motion` counts.
pub fn animations_on(cx: &App) -> bool {
    cx.try_global::<TlSettings>()
        .is_none_or(|tl| tl.data.settings.motion.animations)
        && !cx.reduce_motion()
}

/// The system's reduce-motion setting: `cx.reduce_motion()`, unless `TlSettings` turned it on
/// only because `motion.animations` is off.
pub fn system_reduce_motion(cx: &App) -> bool {
    cx.reduce_motion()
        && !cx
            .try_global::<TlSettings>()
            .is_some_and(|tl| tl.forced_reduce_motion)
}
