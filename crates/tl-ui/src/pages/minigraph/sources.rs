//! The minigraph model (PERF-080, PERF-081, PERF-085, R-084, R-085, R-100, UI-PRF-040): card
//! sources and their `state.toml` strings, process columns, slot geometry, the default layout
//! and the mapping to and from `[[minigraphs]]` entries.

use std::borrow::Cow;
use std::sync::Arc;

use gpui_kit::private::schemars::{JsonSchema, Schema, SchemaGenerator};
use gpui_kit::{App, SharedString};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tl_model::ids::{ColumnId, GraphTarget, ProcessKey, ProcessSeriesId};
use tl_model::settings::{MinigraphState, NetworkUnit};
use tl_model::snapshot::Snapshot;
use tl_model::time::BootId;

use super::GraphColor;
use super::card::{CardState, resolve_colors};
use crate::chart::{ChartSeries, ValueFormat, YScale, format_value};
use crate::i18n::{tr, tr_args};
use crate::pages::performance::DeviceKey;
use crate::settings_global::{SettingKey, TlSettings};
use crate::theme::TlTheme;

/// A card's identity, assigned at load and stable while the app runs. Never stored.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "gpui_kit::private::schemars")]
pub struct MinigraphId(pub u32);

/// What a card draws. In action JSON it is its `state.toml` string ("cpu", "gpu:<key>",
/// "process:...", ...), so `ProcessKey` needs no JSON schema (ADR-001).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum MinigraphSource {
    Cpu,
    Memory,
    Disk,
    Network,
    /// `None` is the first GPU in sidebar order (R-085).
    Gpu(Option<DeviceKey>),
    Vram(Option<DeviceKey>),
    GpuTemp(Option<DeviceKey>),
    /// One process instance (R-100). Its command line is never stored.
    Process {
        key: ProcessKey,
    },
    /// Every process with this image name (PERF-085).
    Name(Arc<str>),
    /// Every process of this app id (PERF-085).
    App(Arc<str>),
}

/// The process column a process, name or app card draws (PERF-085). Its JSON form is
/// [`GraphColumn::as_str`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "gpui_kit::private::schemars")]
pub enum GraphColumn {
    Cpu,
    Memory,
    Rss,
    RssShared,
    Disk,
    Network,
    Gpu,
    Vram,
    GpuShared,
}

/// One card.
#[derive(Clone, PartialEq, Debug)]
pub struct MinigraphEntry {
    pub id: MinigraphId,
    pub source: MinigraphSource,
    pub column: GraphColumn,
    /// Time range in seconds (PERF-081).
    pub range_s: u16,
    pub color: GraphColor,
    /// Grid slot of an attached card; see [`slot_cell`].
    pub slot: u16,
    pub detached: bool,
    /// Detached window id; ignored when `detached` is false (R-084).
    pub window: u32,
}

impl MinigraphSource {
    /// Parses a `[[minigraphs]] source` string:
    /// `cpu`, `memory`, `disk`, `network`, `gpu`, `vram`, `gpu_temp`, `gpu:<key>`, `vram:<key>`,
    /// `gpu_temp:<key>`, `process:<boot_id 16 hex>:<pid>:<start_ticks>`, `name:<image>` or
    /// `app:<id>`. Keys, images and ids must not be empty. `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        let Some((kind, rest)) = s.split_once(':') else {
            return match s {
                "cpu" => Some(Self::Cpu),
                "memory" => Some(Self::Memory),
                "disk" => Some(Self::Disk),
                "network" => Some(Self::Network),
                "gpu" => Some(Self::Gpu(None)),
                "vram" => Some(Self::Vram(None)),
                "gpu_temp" => Some(Self::GpuTemp(None)),
                _ => None,
            };
        };
        if rest.is_empty() {
            return None;
        }
        let key = || Some(DeviceKey(Arc::from(rest)));
        match kind {
            "gpu" => Some(Self::Gpu(key())),
            "vram" => Some(Self::Vram(key())),
            "gpu_temp" => Some(Self::GpuTemp(key())),
            "name" => Some(Self::Name(Arc::from(rest))),
            "app" => Some(Self::App(Arc::from(rest))),
            "process" => parse_process(rest).map(|key| Self::Process { key }),
            _ => None,
        }
    }

    /// The inverse of [`MinigraphSource::parse`].
    pub fn to_state_string(&self) -> String {
        let keyed = |kind: &str, key: &Option<DeviceKey>| match key {
            Some(key) => format!("{kind}:{}", key.0),
            None => kind.to_owned(),
        };
        match self {
            Self::Cpu => "cpu".to_owned(),
            Self::Memory => "memory".to_owned(),
            Self::Disk => "disk".to_owned(),
            Self::Network => "network".to_owned(),
            Self::Gpu(key) => keyed("gpu", key),
            Self::Vram(key) => keyed("vram", key),
            Self::GpuTemp(key) => keyed("gpu_temp", key),
            Self::Process { key } => format!(
                "process:{:016x}:{}:{}",
                key.boot_id.0, key.pid, key.start_ticks
            ),
            Self::Name(image) => format!("name:{image}"),
            Self::App(id) => format!("app:{id}"),
        }
    }

    /// One of the seven system kinds, shown by its `graphs.minigraph_*` setting.
    pub fn is_system(&self) -> bool {
        self.system_label().is_some()
    }

    pub fn system_label(&self) -> Option<&'static str> {
        match self {
            Self::Cpu => Some("CPU"),
            Self::Memory => Some("Memory"),
            Self::Disk => Some("Disk"),
            Self::Network => Some("Network"),
            Self::Gpu(_) => Some("GPU"),
            Self::Vram(_) => Some("VRAM"),
            Self::GpuTemp(_) => Some("GPU temp"),
            Self::Process { .. } | Self::Name(_) | Self::App(_) => None,
        }
    }

    /// What the engine samples for a process, name or app card (R-154); `None` for a system
    /// source.
    pub fn graph_target(&self) -> Option<GraphTarget> {
        match self {
            Self::Process { key } => Some(GraphTarget::Process(*key)),
            Self::Name(image) => Some(GraphTarget::Name(Arc::clone(image))),
            Self::App(id) => Some(GraphTarget::App(Arc::clone(id))),
            _ => None,
        }
    }

    /// The `graphs.minigraph_*` setting that shows a system card.
    pub fn settings_key(&self) -> Option<SettingKey> {
        match self {
            Self::Cpu => Some(SettingKey::GraphsMinigraphCpu),
            Self::Memory => Some(SettingKey::GraphsMinigraphMemory),
            Self::Disk => Some(SettingKey::GraphsMinigraphDisk),
            Self::Network => Some(SettingKey::GraphsMinigraphNetwork),
            Self::Gpu(_) => Some(SettingKey::GraphsMinigraphGpu),
            Self::Vram(_) => Some(SettingKey::GraphsMinigraphVram),
            Self::GpuTemp(_) => Some(SettingKey::GraphsMinigraphGpuTemperature),
            Self::Process { .. } | Self::Name(_) | Self::App(_) => None,
        }
    }
}

impl Serialize for MinigraphSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_state_string())
    }
}

impl<'de> Deserialize<'de> for MinigraphSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Cow::<str>::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format_args!("unknown minigraph source {text:?}"))
        })
    }
}

impl JsonSchema for MinigraphSource {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("MinigraphSource")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        String::json_schema(generator)
    }
}

/// `<boot_id 16 hex>:<pid>:<start_ticks>`. The boot id is exactly 16 lower-case hex digits and
/// the numbers are plain decimal digits, so a parsed string formats back to itself.
fn parse_process(rest: &str) -> Option<ProcessKey> {
    let mut parts = rest.split(':');
    let (Some(boot), Some(pid), Some(ticks), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let lower_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    if boot.len() != 16 || !boot.bytes().all(lower_hex) {
        return None;
    }
    Some(ProcessKey {
        start_ticks: decimal(ticks)?,
        boot_id: BootId(u64::from_str_radix(boot, 16).ok()?),
        pid: decimal(pid)?,
    })
}

/// Digits only, with no sign and no leading zero (except "0" itself).
fn decimal<T: std::str::FromStr>(s: &str) -> Option<T> {
    let canonical =
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    if !canonical {
        return None;
    }
    s.parse().ok()
}

impl GraphColumn {
    pub const ALL: [GraphColumn; 9] = [
        Self::Cpu,
        Self::Memory,
        Self::Rss,
        Self::RssShared,
        Self::Disk,
        Self::Network,
        Self::Gpu,
        Self::Vram,
        Self::GpuShared,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// The stored `[[minigraphs]] column` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Rss => "rss",
            Self::RssShared => "rss_shared",
            Self::Disk => "disk",
            Self::Network => "network",
            Self::Gpu => "gpu",
            Self::Vram => "vram",
            Self::GpuShared => "gpu_shared",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::Rss => "RSS",
            Self::RssShared => "RSS shared",
            Self::Disk => "Disk",
            Self::Network => "Network",
            Self::Gpu => "GPU",
            Self::Vram => "VRAM",
            Self::GpuShared => "GPU shared",
        }
    }

    /// The string key of [`GraphColumn::label`] (PERF-087).
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Cpu => "minigraph-col-cpu",
            Self::Memory => "minigraph-col-memory",
            Self::Rss => "minigraph-col-rss",
            Self::RssShared => "minigraph-col-rss-shared",
            Self::Disk => "minigraph-col-disk",
            Self::Network => "minigraph-col-network",
            Self::Gpu => "minigraph-col-gpu",
            Self::Vram => "minigraph-col-vram",
            Self::GpuShared => "minigraph-col-gpu-shared",
        }
    }

    /// The process table column behind this graph column.
    pub fn column_id(self) -> ColumnId {
        match self {
            Self::Cpu => ColumnId::Cpu,
            Self::Memory => ColumnId::Memory,
            Self::Rss => ColumnId::WsTotal,
            Self::RssShared => ColumnId::WsShared,
            Self::Disk => ColumnId::Disk,
            Self::Network => ColumnId::Network,
            Self::Gpu => ColumnId::Gpu,
            Self::Vram => ColumnId::GpuMemDedicated,
            Self::GpuShared => ColumnId::GpuMemShared,
        }
    }

    /// The engine samples this column for graph targets (R-154 allows six). RSS, RSS shared
    /// and GPU shared cards get no series and show the warming-up value.
    pub fn engine_sampled(self) -> bool {
        !matches!(self, Self::Rss | Self::RssShared | Self::GpuShared)
    }
}

/// The label of a process, name or app card in parts, so the card can cut the name part alone
/// and keep ` · <col>` (PERF-087, UI-PRF-041).
#[derive(Clone, PartialEq, Debug)]
pub struct ProcessLabel {
    /// `minigraph-label-process`, `-name`, `-app` or `-unnamed`.
    key: &'static str,
    /// The name part: the process name, the image name, the app's display name, or
    /// `Process <pid>` before the snapshot has the process.
    pub name: SharedString,
    /// The pid of a named process card.
    pid: Option<u32>,
    pub column: GraphColumn,
}

impl ProcessLabel {
    /// The whole label.
    pub fn text(&self, cx: &App) -> SharedString {
        self.with_name(&self.name, cx)
    }

    /// The label with `name` as its name part, for a name cut to fit.
    pub fn with_name(&self, name: &str, cx: &App) -> SharedString {
        let col = tr(cx, self.column.label_key());
        let name_arg = if self.key == "minigraph-label-app" {
            "app"
        } else {
            "name"
        };
        match self.pid {
            Some(pid) => tr_args(
                cx,
                self.key,
                &[
                    (name_arg, name.to_owned().into()),
                    ("pid", pid.to_string().into()),
                    ("col", col.to_string().into()),
                ],
            ),
            None => tr_args(
                cx,
                self.key,
                &[
                    (name_arg, name.to_owned().into()),
                    ("col", col.to_string().into()),
                ],
            ),
        }
    }
}

/// What a process, name or app card or tray graph shows (PERF-085, UI-PRF-041). The minigraph
/// card and the tray bridge both build it with [`resolve_process_source`].
#[derive(Clone, PartialEq, Debug)]
pub struct ProcessSourceView {
    /// The engine-sampled series of the source and column, coloured.
    pub series: ChartSeries,
    /// The y scale of the column.
    pub y: YScale,
    pub label: SharedString,
    pub label_parts: ProcessLabel,
    /// The newest valid sample formatted for the column, `…` before the first one.
    pub value: SharedString,
    pub state: CardState,
}

/// The value of a card with no valid sample yet (WarmingUp).
const WARMING_UP_TEXT: &str = "\u{2026}";

/// Resolves a process, name or app source against `snap`: its series, label, value text and
/// state. `None` for a system source. Names come from `snap.processes`: a process card shows
/// `Process <pid>` until the record is there, an app card shows its id until a member process
/// is there.
pub fn resolve_process_source(
    source: &MinigraphSource,
    column: GraphColumn,
    color: GraphColor,
    snap: &Snapshot,
    theme: &TlTheme,
    cx: &App,
) -> Option<ProcessSourceView> {
    let target = source.graph_target()?;
    let id = ProcessSeriesId {
        target,
        column: column.column_id(),
    };
    let (fill, line) = resolve_colors(source, color, theme);
    let newest = column
        .engine_sampled()
        .then(|| snap.process_series.get(&id))
        .flatten()
        .and_then(|v| v.samples.iter().rev().find(|s| s.is_valid()))
        .map(|s| s.value);
    let series = ChartSeries {
        process: Some(id),
        fill,
        line,
        ..ChartSeries::default()
    };
    let label_parts = process_label(source, column, snap, cx)?;
    let unit = if cx.has_global::<TlSettings>() {
        TlSettings::settings(cx).units.network
    } else {
        NetworkUnit::default()
    };
    let (y, fmt) = column_scale(source, column);
    let value = newest.map_or(SharedString::new_static(WARMING_UP_TEXT), |v| {
        let mut s = String::new();
        format_value(fmt, v, unit, &mut s);
        s.into()
    });
    Some(ProcessSourceView {
        series,
        y,
        label: label_parts.text(cx),
        label_parts,
        value,
        // E26-012 (tasklens-xdh.14) adds No match, Ended and Restricted.
        state: CardState::Live,
    })
}

/// The label parts of a process, name or app source; `None` for a system source.
pub fn process_label(
    source: &MinigraphSource,
    column: GraphColumn,
    snap: &Snapshot,
    cx: &App,
) -> Option<ProcessLabel> {
    let (key, name, pid) = match source {
        MinigraphSource::Process { key } => match snap.processes.get(*key) {
            Some(r) => (
                "minigraph-label-process",
                SharedString::from(Arc::clone(&r.meta.name)),
                Some(key.pid),
            ),
            None => (
                "minigraph-label-unnamed",
                tr_args(
                    cx,
                    "minigraph-process-unnamed",
                    &[("pid", key.pid.to_string().into())],
                ),
                None,
            ),
        },
        MinigraphSource::Name(image) => (
            "minigraph-label-name",
            SharedString::from(Arc::clone(image)),
            None,
        ),
        MinigraphSource::App(id) => (
            "minigraph-label-app",
            app_display_name(snap, id).unwrap_or_else(|| SharedString::from(Arc::clone(id))),
            None,
        ),
        _ => return None,
    };
    Some(ProcessLabel {
        key,
        name,
        pid,
        column,
    })
}

/// The display name of app `id`, as the process table names its app row: the name of the
/// earliest started member process. `None` when no process of the app is in the snapshot.
fn app_display_name(snap: &Snapshot, id: &str) -> Option<SharedString> {
    snap.processes
        .records
        .iter()
        .filter(|r| r.meta.app_id.as_deref() == Some(id))
        .min_by_key(|r| r.key.start_ticks)
        .map(|r| SharedString::from(Arc::clone(&r.meta.name)))
}

/// The y scale and value format of a process card's column (UI-PRF-041 "by column"). A CPU
/// card of one process is 0..100 %; a name or app sum may pass 100 %, so it sticks up.
pub fn column_scale(source: &MinigraphSource, column: GraphColumn) -> (YScale, ValueFormat) {
    let percent = YScale::Fixed {
        min: 0.0,
        max: 100.0,
    };
    let sticky = YScale::StickyUp { hw_max: None };
    match column {
        GraphColumn::Cpu if matches!(source, MinigraphSource::Process { .. }) => {
            (percent, ValueFormat::Percent)
        }
        GraphColumn::Cpu => (sticky, ValueFormat::Percent),
        GraphColumn::Memory
        | GraphColumn::Rss
        | GraphColumn::RssShared
        | GraphColumn::Vram
        | GraphColumn::GpuShared => (sticky, ValueFormat::Bytes),
        GraphColumn::Disk => (YScale::AutoRate { iec: true }, ValueFormat::RateIec),
        GraphColumn::Network => (YScale::AutoRate { iec: false }, ValueFormat::RateNetwork),
        GraphColumn::Gpu => (percent, ValueFormat::Percent),
    }
}

/// Attached cards at most (UI-PRF-040).
pub const MAX_SLOTS: u16 = 12;
pub const COLS: u16 = 4;
pub const ROWS: u16 = 3;

/// The grid cell of `slot` as (row from the bottom, column from the right). Slot 0 is the bottom
/// right; slots 0..=3 fill row 0, 4..=7 row 1, 8..=11 row 2.
pub fn slot_cell(slot: u16) -> (u16, u16) {
    (slot / COLS, slot % COLS)
}

/// The default range of a new card in seconds (PERF-081).
const DEFAULT_RANGE_S: u16 = 60;

/// The six cards on by default: CPU, Memory, Disk, Network, GPU and VRAM in slots 0 to 5, with
/// ids 0 to 5. GPU temp is off by default (table (a)); the panel adds it at the next free slot
/// when its setting turns on (E26-004).
pub fn default_entries() -> Vec<MinigraphEntry> {
    [
        (MinigraphSource::Cpu, GraphColumn::Cpu),
        (MinigraphSource::Memory, GraphColumn::Memory),
        (MinigraphSource::Disk, GraphColumn::Disk),
        (MinigraphSource::Network, GraphColumn::Network),
        (MinigraphSource::Gpu(None), GraphColumn::Gpu),
        (MinigraphSource::Vram(None), GraphColumn::Vram),
    ]
    .into_iter()
    .zip(0u16..)
    .map(|((source, column), slot)| MinigraphEntry {
        id: MinigraphId(u32::from(slot)),
        source,
        column,
        range_s: DEFAULT_RANGE_S,
        color: GraphColor::Accent,
        slot,
        detached: false,
        window: 0,
    })
    .collect()
}

/// Builds the cards from `[[minigraphs]]`, taking ids from `next_id`. An unparsable source drops
/// the entry; an unknown column or colour falls back to CPU or Accent. Each drop or fallback is
/// a `warn!`. Attached cards come first, in slot order (file order for equal slots), and a slot
/// already taken moves to the one after the previous card, so no two cards share a slot. Holes
/// stay; [`compact_slots`] removes them. Detached cards follow in file order.
pub fn from_state(state: &[MinigraphState], next_id: &mut u32) -> Vec<MinigraphEntry> {
    let mut entries: Vec<MinigraphEntry> = state
        .iter()
        .filter_map(|s| {
            let Some(source) = MinigraphSource::parse(&s.source) else {
                tracing::warn!(target: "tasklens::minigraph", source = %s.source, "unknown minigraph source, card dropped");
                return None;
            };
            let column = GraphColumn::parse(&s.column).unwrap_or_else(|| {
                tracing::warn!(target: "tasklens::minigraph", column = %s.column, "unknown minigraph column, using cpu");
                GraphColumn::Cpu
            });
            let color = GraphColor::from_str(&s.color).unwrap_or_else(|| {
                tracing::warn!(target: "tasklens::minigraph", color = %s.color, "unknown minigraph colour, using accent");
                GraphColor::Accent
            });
            let id = MinigraphId(*next_id);
            *next_id = next_id.wrapping_add(1);
            Some(MinigraphEntry {
                id,
                source,
                column,
                range_s: s.range_s,
                color,
                slot: s.slot,
                detached: s.detached,
                window: s.window,
            })
        })
        .collect();
    // Stable: equal slots keep file order.
    entries.sort_by_key(|e| (e.detached, if e.detached { 0 } else { e.slot }));
    let mut prev: Option<u16> = None;
    for e in entries.iter_mut().filter(|e| !e.detached) {
        if let Some(p) = prev
            && e.slot <= p
        {
            e.slot = p.saturating_add(1);
        }
        prev = Some(e.slot);
    }
    entries
}

/// The `[[minigraphs]]` entries for `entries`, in the same order.
pub fn to_state(entries: &[MinigraphEntry]) -> Vec<MinigraphState> {
    entries
        .iter()
        .map(|e| MinigraphState {
            source: e.source.to_state_string(),
            column: e.column.as_str().to_owned(),
            range_s: e.range_s,
            color: e.color.as_str().to_owned(),
            slot: e.slot,
            detached: e.detached,
            window: e.window,
        })
        .collect()
}

/// Renumbers the attached cards 0..n in their slot order, so the grid has no holes. Detached
/// cards keep their slot. Equal slots keep their order in `entries`.
pub fn compact_slots(entries: &mut [MinigraphEntry]) {
    let mut attached: Vec<(u16, usize)> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.detached)
        .map(|(i, e)| (e.slot, i))
        .collect();
    attached.sort_unstable();
    for ((_, i), slot) in attached.into_iter().zip(0u16..) {
        entries[i].slot = slot;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn state(source: &str, slot: u16) -> MinigraphState {
        MinigraphState {
            source: source.to_owned(),
            slot,
            ..MinigraphState::default()
        }
    }

    #[test]
    fn source_round_trip() {
        for s in [
            "cpu",
            "memory",
            "disk",
            "network",
            "gpu",
            "vram",
            "gpu_temp",
            "gpu:gpu:GPU-6f1c2a3b-0d4e-4f5a-8b9c-0123456789ab",
            "vram:gpu:GPU-6f1c2a3b-0d4e-4f5a-8b9c-0123456789ab",
            "gpu_temp:gpu:pci-0000:03:00.0",
            "process:0123456789abcdef:4242:1234567",
            "process:0000000000000000:0:0",
            "name:firefox",
            "name:Web Content",
            "app:org.kde.konsole",
        ] {
            let parsed = MinigraphSource::parse(s).unwrap_or_else(|| panic!("{s} parses"));
            assert_eq!(parsed.to_state_string(), s);
        }
        assert_eq!(
            MinigraphSource::parse("process:0123456789abcdef:4242:1234567"),
            Some(MinigraphSource::Process {
                key: ProcessKey {
                    start_ticks: 1_234_567,
                    boot_id: BootId(0x0123_4567_89ab_cdef),
                    pid: 4242,
                }
            })
        );
        assert_eq!(
            MinigraphSource::parse("gpu:gpu:GPU-1"),
            Some(MinigraphSource::Gpu(Some(DeviceKey(Arc::from(
                "gpu:GPU-1"
            )))))
        );
    }

    #[test]
    fn source_parse_rejects_unknown() {
        for s in [
            "bogus:1",
            "bogus",
            "",
            "CPU",
            "cpu:",
            "gpu:",
            "name:",
            "app:",
            "process:0123456789abcdef:4242",
            "process:0123456789abcdef:4242:1:2",
            "process:0123456789ABCDEF:4242:1",
            "process:123456789abcdef:4242:1",
            "process:0123456789abcdef:+42:1",
            "process:0123456789abcdef:042:1",
            "process:0123456789abcdef:4294967296:1",
        ] {
            assert_eq!(MinigraphSource::parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn source_kinds() {
        let gpu = MinigraphSource::Gpu(Some(DeviceKey(Arc::from("gpu:GPU-1"))));
        assert_eq!(gpu.system_label(), Some("GPU"));
        assert_eq!(gpu.settings_key(), Some(SettingKey::GraphsMinigraphGpu));
        assert_eq!(
            MinigraphSource::GpuTemp(None).settings_key(),
            Some(SettingKey::GraphsMinigraphGpuTemperature)
        );
        assert_eq!(
            MinigraphSource::GpuTemp(None).system_label(),
            Some("GPU temp")
        );
        let name = MinigraphSource::Name(Arc::from("firefox"));
        assert!(!name.is_system());
        assert_eq!(name.settings_key(), None);
        assert!(MinigraphSource::Cpu.is_system());
    }

    #[test]
    fn slot_cell_geometry() {
        assert_eq!(slot_cell(0), (0, 0));
        assert_eq!(slot_cell(3), (0, 3));
        assert_eq!(slot_cell(4), (1, 0));
        assert_eq!(slot_cell(11), (2, 3));
        assert_eq!(MAX_SLOTS, ROWS * COLS);
    }

    #[test]
    fn default_entries_six_cards() {
        let entries = default_entries();
        let sources: Vec<_> = entries.iter().map(|e| e.source.to_state_string()).collect();
        assert_eq!(sources, ["cpu", "memory", "disk", "network", "gpu", "vram"]);
        for (e, slot) in entries.iter().zip(0u16..) {
            assert_eq!(e.slot, slot);
            assert_eq!(e.range_s, 60);
            assert_eq!(e.color, GraphColor::Accent);
            assert!(!e.detached);
        }
    }

    #[test]
    fn compact_slots_no_holes() {
        let file = [
            state("cpu", 0),
            state("memory", 2),
            state("disk", 2),
            state("network", 5),
        ];
        let mut next_id = 10;
        let mut entries = from_state(&file, &mut next_id);
        compact_slots(&mut entries);
        let got: Vec<_> = entries
            .iter()
            .map(|e| (e.source.to_state_string(), e.slot))
            .collect();
        assert_eq!(
            got,
            [
                ("cpu".to_owned(), 0),
                ("memory".to_owned(), 1),
                ("disk".to_owned(), 2),
                ("network".to_owned(), 3),
            ]
        );
        assert_eq!(next_id, 14);
        assert_eq!(entries[0].id, MinigraphId(10));
    }

    #[test]
    fn from_state_drops_bad_sources_and_orders_by_slot() {
        let mut detached = state("name:firefox", 0);
        detached.detached = true;
        detached.window = 7;
        let mut bad_colour = state("memory", 1);
        bad_colour.color = "magenta".to_owned();
        let file = [state("disk", 3), detached, state("bogus:1", 0), bad_colour];
        let mut next_id = 0;
        let entries = from_state(&file, &mut next_id);
        let got: Vec<_> = entries
            .iter()
            .map(|e| (e.source.to_state_string(), e.slot, e.detached))
            .collect();
        assert_eq!(
            got,
            [
                ("memory".to_owned(), 1, false),
                ("disk".to_owned(), 3, false),
                ("name:firefox".to_owned(), 0, true),
            ]
        );
        assert_eq!(entries[0].color, GraphColor::Accent);
        assert_eq!(next_id, 3);
        assert_eq!(to_state(&entries)[2].window, 7);
    }

    #[test]
    fn state_round_trip() {
        let entries = default_entries();
        let mut next_id = 0;
        assert_eq!(from_state(&to_state(&entries), &mut next_id), entries);
    }

    #[test]
    fn column_labels() {
        assert_eq!(GraphColumn::RssShared.label(), "RSS shared");
        assert_eq!(GraphColumn::RssShared.as_str(), "rss_shared");
        for column in GraphColumn::ALL {
            assert_eq!(GraphColumn::parse(column.as_str()), Some(column));
        }
        assert_eq!(GraphColumn::parse("threads"), None);
    }

    fn any_key() -> impl Strategy<Value = Option<DeviceKey>> {
        prop::option::of(".{1,40}".prop_map(|s| DeviceKey(Arc::from(s))))
    }

    fn any_source() -> impl Strategy<Value = MinigraphSource> {
        prop_oneof![
            Just(MinigraphSource::Cpu),
            Just(MinigraphSource::Memory),
            Just(MinigraphSource::Disk),
            Just(MinigraphSource::Network),
            any_key().prop_map(MinigraphSource::Gpu),
            any_key().prop_map(MinigraphSource::Vram),
            any_key().prop_map(MinigraphSource::GpuTemp),
            (any::<u64>(), any::<u32>(), any::<u64>()).prop_map(|(boot, pid, ticks)| {
                MinigraphSource::Process {
                    key: ProcessKey {
                        start_ticks: ticks,
                        boot_id: BootId(boot),
                        pid,
                    },
                }
            }),
            ".{1,40}".prop_map(|s| MinigraphSource::Name(Arc::from(s))),
            ".{1,40}".prop_map(|s| MinigraphSource::App(Arc::from(s))),
        ]
    }

    proptest! {
        #[test]
        fn prop_source_round_trip(source in any_source()) {
            let text = source.to_state_string();
            prop_assert_eq!(MinigraphSource::parse(&text), Some(source));
        }

        #[test]
        fn prop_parsed_strings_format_back(text in ".{0,48}") {
            if let Some(source) = MinigraphSource::parse(&text) {
                prop_assert_eq!(source.to_state_string(), text);
            }
        }

        #[test]
        fn prop_compact_has_no_holes(slots in prop::collection::vec(0u16..MAX_SLOTS, 0..12)) {
            let file: Vec<_> = slots.iter().map(|&s| state("cpu", s)).collect();
            let mut next_id = 0;
            let mut entries = from_state(&file, &mut next_id);
            compact_slots(&mut entries);
            let got: Vec<u16> = entries.iter().map(|e| e.slot).collect();
            let want: Vec<u16> = (0..u16::try_from(slots.len()).unwrap()).collect();
            prop_assert_eq!(got, want);
        }
    }
}
