//! Widget layout settings, `[[performance.layouts]]` (ADR-019, D-041). No file I/O.
//!
//! A container (an Overview box, later a resource page) holds a list of widgets. A widget shows
//! one metric of one device, one metric of every member of a group (every core, disk, adapter or
//! GPU engine), or a fixed view of its own (the Top processes table, the Timeline, Memory by
//! app). This module holds the plain serde data, the GPU metric keys, the load rules that
//! `Settings::clamp` runs, the conversion of the GPU box's old settings and the reset to the
//! default. The other boxes' metric keys, preset lists and conversions are in
//! [`crate::layout_boxes`]. `tl-ui` turns the data into its views.
//!
//! As deserialized, every entry is [`LayoutEntry::Kept`]. `Settings::clamp` reads the entries
//! this build supports into [`LayoutEntry::Known`] and drops the invalid parts.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use crate::ids::MetricId;
use crate::layout_boxes::{self, convert_box};
use crate::settings::{
    OVERVIEW_ARRANGES, OVERVIEW_BOX_MODES, OVERVIEW_POWER_PLACEMENTS, OVERVIEW_POWER_SPLIT_RANGE,
    OVERVIEW_SPLIT_RANGE, OverviewGridSetting, PerformanceSettings,
};
use crate::system::GpuEngine;

/// The newest layout schema version this build reads and writes. It reads versions 1 and 2.
pub const LAYOUT_VERSION: u32 = 2;
/// The version an `overview.gpu` entry with single-metric widgets only is written at, so a
/// 1.2.0 build still reads it. Every other entry is written at [`LAYOUT_VERSION`].
pub const LAYOUT_VERSION_GPU_ONLY: u32 = 1;

/// The container ids of the Overview boxes: `overview.<kind>`.
pub const CPU_BOX_CONTAINER: &str = "overview.cpu";
pub const MEMORY_BOX_CONTAINER: &str = "overview.memory";
pub const GPU_BOX_CONTAINER: &str = "overview.gpu";
pub const DISK_BOX_CONTAINER: &str = "overview.disk";
pub const NET_BOX_CONTAINER: &str = "overview.net";
pub const PROCESSES_BOX_CONTAINER: &str = "overview.processes";
pub const TIMELINE_BOX_CONTAINER: &str = "overview.timeline";

/// The containers this build supports: the seven Overview boxes.
pub const LAYOUT_CONTAINERS: &[&str] = &[
    CPU_BOX_CONTAINER,
    MEMORY_BOX_CONTAINER,
    GPU_BOX_CONTAINER,
    DISK_BOX_CONTAINER,
    NET_BOX_CONTAINER,
    PROCESSES_BOX_CONTAINER,
    TIMELINE_BOX_CONTAINER,
];

/// The column count of each container's inner grid (ADR-019 phase 2 "Containers and inner
/// grids"): 12 for the wide boxes, 6 for a box one page column wide.
pub const OVERVIEW_GRID_COLS: [(&str, u16); 7] = [
    (CPU_BOX_CONTAINER, 12),
    (MEMORY_BOX_CONTAINER, 6),
    (GPU_BOX_CONTAINER, 6),
    (DISK_BOX_CONTAINER, 6),
    (NET_BOX_CONTAINER, 6),
    (PROCESSES_BOX_CONTAINER, 12),
    (TIMELINE_BOX_CONTAINER, 12),
];

/// The column count of a container this build does not know.
const DEFAULT_GRID_COLS: u16 = 6;

/// The prefix of an Overview box container id; the rest is the box kind.
const OVERVIEW_PREFIX: &str = "overview.";

/// `[[performance.layouts]]` in file order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LayoutsSetting(pub Vec<LayoutEntry>);

/// One `[[performance.layouts]]` entry.
#[derive(Clone, Debug, PartialEq)]
pub enum LayoutEntry {
    /// A container this build supports, at a version it reads.
    Known(ContainerLayout),
    /// A container this build does not support, or a newer `version`: kept as read and written
    /// back unchanged. The container keeps using its old settings.
    Kept(Table),
}

/// The widget layout of one container.
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerLayout {
    pub container: String,
    /// The version the entry is written at ([`ContainerLayout::written_version`]). Loading,
    /// [`LayoutsSetting::set`] and the conversions set it; `to_table` always writes the
    /// version the content needs.
    pub version: u32,
    /// `Some`: a flow layout placed by a built-in preset. `None`: a grid layout.
    pub preset: Option<PresetSetting>,
    /// Empty is an empty box, which is not the same as no entry (the default).
    pub widgets: Vec<WidgetSetting>,
    /// The id the next new widget takes. It only grows: removing a widget never lowers it, so
    /// an id is never given to a second widget. On load it is raised to at least the highest
    /// widget id plus 1. Key `next_id`.
    pub next_id: u32,
}

/// The built-in preset of a flow layout.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PresetSetting {
    /// A mode id of the container's kind in [`OVERVIEW_BOX_MODES`]: for the GPU box
    /// "engine_charts", "chart_engines" or "chart".
    pub id: String,
    /// An id from [`OVERVIEW_ARRANGES`]; `None` is the preset's default.
    pub arrange: Option<String>,
    /// In [`OVERVIEW_SPLIT_RANGE`]; `None` is the preset's default.
    pub split: Option<f64>,
    /// The power draw strip's share of the box body, in [`OVERVIEW_POWER_SPLIT_RANGE`]. CPU box
    /// only; `None` is the default.
    pub power_split: Option<f64>,
    /// Where the power draw strip goes, an id from [`OVERVIEW_POWER_PLACEMENTS`]. CPU box only;
    /// `None` is `full_width`.
    pub power_placement: Option<String>,
}

/// One widget: its content in one or more forms.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetSetting {
    /// Unique in its container, taken from [`ContainerLayout::next_id`], so an id is never
    /// reused while the entry exists, not even after the widget that had it is removed.
    pub id: u32,
    pub content: WidgetContent,
    /// Never empty in a loaded metric or group widget. Always empty for a fixed widget.
    pub forms: FormSet,
    /// Required in a grid layout, never written in a flow layout.
    pub cell: Option<WidgetCell>,
    /// Chart options of a metric or group widget. A fixed widget has none.
    pub options: WidgetOptions,
    /// The device's name when the widget was placed, shown only while the device is gone.
    pub device_name: Option<Arc<str>>,
}

/// What a widget shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WidgetContent {
    /// One metric of one device.
    Metric(SourceSelector),
    /// One metric of every member of a group: every core, disk, adapter or GPU engine.
    Group(GroupSelector),
    /// A widget with content of its own that is not a metric.
    Fixed(FixedWidget),
}

impl WidgetContent {
    /// The source of a metric widget.
    pub fn metric(&self) -> Option<&SourceSelector> {
        match self {
            WidgetContent::Metric(s) => Some(s),
            _ => None,
        }
    }

    /// The source of a metric widget, to change.
    pub fn metric_mut(&mut self) -> Option<&mut SourceSelector> {
        match self {
            WidgetContent::Metric(s) => Some(s),
            _ => None,
        }
    }

    /// The device of a metric or group widget.
    pub fn device(&self) -> Option<&DeviceSelector> {
        match self {
            WidgetContent::Metric(s) => Some(&s.device),
            WidgetContent::Group(g) => Some(&g.device),
            WidgetContent::Fixed(_) => None,
        }
    }

    /// The device of a metric or group widget, to change.
    pub fn device_mut(&mut self) -> Option<&mut DeviceSelector> {
        match self {
            WidgetContent::Metric(s) => Some(&mut s.device),
            WidgetContent::Group(g) => Some(&mut g.device),
            WidgetContent::Fixed(_) => None,
        }
    }

    /// The metric key of a metric or group widget.
    pub fn metric_key(&self) -> Option<&MetricKey> {
        match self {
            WidgetContent::Metric(s) => Some(&s.metric),
            WidgetContent::Group(g) => Some(&g.metric),
            WidgetContent::Fixed(_) => None,
        }
    }
}

/// A durable source: never a runtime `SeriesId` or `DeviceIdx`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceSelector {
    pub device: DeviceSelector,
    pub metric: MetricKey,
}

/// The members of a group widget and the metric each one shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GroupSelector {
    /// The device the members belong to: `Key("cpu")` for cores, `Key("aggregate:disk")` for
    /// disks, `Key("aggregate:net")` for adapters, a GPU key (or a slot, GPU box flow layouts
    /// only) for engines.
    pub device: DeviceSelector,
    pub members: GroupKind,
    /// The metric each member shows. For `Engines` it is `engine:*`: each member shows
    /// `engine:<key>` of its engine.
    pub metric: MetricKey,
    pub options: GroupOptions,
}

/// Which members a group has.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GroupKind {
    Cores,
    Disks,
    Adapters,
    Engines,
    /// A name this build does not know: kept, written back, shown as unavailable.
    Other(Arc<str>),
}

impl GroupKind {
    /// Each known group and its name, as written.
    pub const NAMES: [(&'static str, GroupKind); 4] = [
        ("cores", GroupKind::Cores),
        ("disks", GroupKind::Disks),
        ("adapters", GroupKind::Adapters),
        ("engines", GroupKind::Engines),
    ];

    /// The group of `name`; `Other` for a name this build does not know.
    pub fn from_name(name: &str) -> Self {
        Self::NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map_or_else(|| GroupKind::Other(Arc::from(name)), |(_, g)| g.clone())
    }

    /// The name as written.
    pub fn name(&self) -> &str {
        match self {
            GroupKind::Cores => "cores",
            GroupKind::Disks => "disks",
            GroupKind::Adapters => "adapters",
            GroupKind::Engines => "engines",
            GroupKind::Other(name) => name,
        }
    }

    /// How the members are drawn when the entry does not say: rows for cores, cells for the
    /// others.
    pub fn default_items(&self) -> GroupItems {
        match self {
            GroupKind::Cores => GroupItems::Rows,
            _ => GroupItems::Cells,
        }
    }

    /// Whether `device` is the device this group's members belong to. A slot stands for a
    /// GPU, so it fits an engine group only.
    fn fits(&self, device: &DeviceSelector) -> bool {
        match (self, device) {
            (GroupKind::Engines, DeviceSelector::Slot(_)) => true,
            (GroupKind::Engines, DeviceSelector::Key(k)) => k.starts_with("gpu:"),
            (GroupKind::Other(_), _) => true,
            (_, DeviceSelector::Slot(_)) => false,
            (GroupKind::Cores, DeviceSelector::Key(k)) => &**k == layout_boxes::DEVICE_CPU,
            (GroupKind::Disks, DeviceSelector::Key(k)) => &**k == layout_boxes::DEVICE_DISKS,
            (GroupKind::Adapters, DeviceSelector::Key(k)) => &**k == layout_boxes::DEVICE_NETS,
        }
    }
}

/// The options of a group widget.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GroupOptions {
    /// Show at most this many members; the others are counted in a "+n more" item. `None`
    /// shows all. Key `limit`, in [`GROUP_LIMIT_RANGE`].
    pub limit: Option<u16>,
    /// How the members are drawn. Key `items`.
    pub items: GroupItems,
    /// `Cells` only: an id from [`OVERVIEW_ARRANGES`]; `None` is `auto`. Key `arrange`.
    pub arrange: Option<String>,
}

/// The range a group's `limit` is accepted in.
pub const GROUP_LIMIT_RANGE: (u16, u16) = (1, 256);

/// How a group draws its members.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupItems {
    /// One Row composition per member, filling the height as D-040 does.
    Rows,
    /// One Cell composition per member (Tile when the forms have no chart) in a grid.
    Cells,
}

impl GroupItems {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "rows" => Some(GroupItems::Rows),
            "cells" => Some(GroupItems::Cells),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            GroupItems::Rows => "rows",
            GroupItems::Cells => "cells",
        }
    }
}

/// A widget with content of its own.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FixedWidget {
    /// The Top processes table (UI-PRF-017), in `overview.processes` only.
    Processes,
    /// The 24-hour timeline (UI-PRF-057), in `overview.timeline` only.
    Timeline,
    /// The Memory by app chart and its legend (UI-PRF-060), in `overview.memory` only.
    MemoryByApp,
    /// A name this build does not know: kept, written back, shown as unavailable.
    Other(Arc<str>),
}

impl FixedWidget {
    /// The fixed widget of `name`; `Other` for a name this build does not know.
    pub fn from_name(name: &str) -> Self {
        match name {
            "processes" => FixedWidget::Processes,
            "timeline" => FixedWidget::Timeline,
            "memory_by_app" => FixedWidget::MemoryByApp,
            other => FixedWidget::Other(Arc::from(other)),
        }
    }

    /// The name as written.
    pub fn name(&self) -> &str {
        match self {
            FixedWidget::Processes => "processes",
            FixedWidget::Timeline => "timeline",
            FixedWidget::MemoryByApp => "memory_by_app",
            FixedWidget::Other(name) => name,
        }
    }

    /// The only container the widget may be in; `None` for `Other`, which any container keeps.
    pub fn container(&self) -> Option<&'static str> {
        match self {
            FixedWidget::Processes => Some(PROCESSES_BOX_CONTAINER),
            FixedWidget::Timeline => Some(TIMELINE_BOX_CONTAINER),
            FixedWidget::MemoryByApp => Some(MEMORY_BOX_CONTAINER),
            FixedWidget::Other(_) => None,
        }
    }
}

/// Which device a widget shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DeviceSelector {
    /// The device's persistent key (PERF-005, R-155, ADR-017), for example
    /// "gpu:GPU-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9" or "gpu:0000:01:00.0".
    Key(Arc<str>),
    /// The nth device the box shows (0 or 1 for the GPU box), in sidebar order. Flow layouts
    /// of the GPU box only, so a default or converted layout follows the GPUs present.
    Slot(u8),
}

/// The highest slot a flow layout widget may name.
pub const SLOT_MAX: u8 = 1;

/// A stable metric name, written as text. An unknown key is kept. A key is never renamed or
/// reused.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MetricKey(pub Arc<str>);

/// The prefix of an engine metric key; the rest is a [`GpuEngine::key`].
pub const METRIC_ENGINE_PREFIX: &str = "engine:";
/// The metric of an engine group: each member shows `engine:<key>` of its engine.
pub const METRIC_ENGINE_ANY: &str = "engine:*";
pub const METRIC_VRAM: &str = "vram";
pub const METRIC_CLOCK_GFX: &str = "clock_gfx";
pub const METRIC_CLOCK_MEM: &str = "clock_mem";
pub const METRIC_POWER: &str = "power";
pub const METRIC_TEMP: &str = "temp";

impl MetricKey {
    pub fn new(key: &str) -> Self {
        Self(Arc::from(key))
    }

    /// `engine:<key>` for `engine`.
    pub fn engine(engine: GpuEngine) -> Self {
        Self(Arc::from(format!("{METRIC_ENGINE_PREFIX}{}", engine.key())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The engine of an `engine:<key>` key.
    pub fn gpu_engine(&self) -> Option<GpuEngine> {
        self.0
            .strip_prefix(METRIC_ENGINE_PREFIX)
            .and_then(GpuEngine::from_key)
    }

    /// The series this key names on a GPU; `None` for a key this build does not know there.
    /// The keys of the other device kinds are in [`layout_boxes::metric_series`].
    pub fn metric(&self) -> Option<MetricId> {
        if let Some(engine) = self.gpu_engine() {
            return Some(engine.metric());
        }
        match &*self.0 {
            METRIC_VRAM => Some(MetricId::GPU_MEM),
            METRIC_CLOCK_GFX => Some(MetricId::GPU_CLOCK),
            METRIC_CLOCK_MEM => Some(MetricId::GPU_MEM_CLOCK),
            METRIC_POWER => Some(MetricId::GPU_POWER),
            METRIC_TEMP => Some(MetricId::GPU_TEMP),
            _ => None,
        }
    }
}

bitflags::bitflags! {
    /// The forms a widget shows. Never empty in a loaded metric or group widget. Written as
    /// names in [`FormSet::NAMES`] order.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct FormSet: u8 {
        const CHART = 1;
        const BAR = 2;
        const VALUE = 4;
        const STATS = 8;
        const LABEL = 16;
    }
}

impl FormSet {
    /// Each form and its name, in written order.
    pub const NAMES: [(FormSet, &'static str); 5] = [
        (FormSet::CHART, "chart"),
        (FormSet::BAR, "bar"),
        (FormSet::VALUE, "value"),
        (FormSet::STATS, "stats"),
        (FormSet::LABEL, "label"),
    ];

    /// The forms named in `names`; unknown names are ignored.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        names
            .into_iter()
            .filter_map(|n| Self::NAMES.iter().find(|(_, name)| *name == n))
            .fold(Self::empty(), |set, (f, _)| set | *f)
    }

    /// The names of the forms in the set, in written order.
    pub fn names(self) -> impl Iterator<Item = &'static str> {
        Self::NAMES
            .into_iter()
            .filter(move |(f, _)| self.contains(*f))
            .map(|(_, n)| n)
    }
}

/// A widget's cells on the container's inner grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WidgetCell {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

impl WidgetCell {
    /// Whether the two share a cell.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.col < other.col + other.cols
            && other.col < self.col + self.cols
            && self.row < other.row + other.rows
            && other.row < self.row + self.rows
    }
}

/// Chart options. Written only when not the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WidgetOptions {
    pub fill: bool,
    pub dashed: bool,
}

impl Default for WidgetOptions {
    fn default() -> Self {
        Self {
            fill: true,
            dashed: false,
        }
    }
}

/// The column count of `container`'s inner grid ([`OVERVIEW_GRID_COLS`]).
pub fn grid_cols(container: &str) -> u16 {
    OVERVIEW_GRID_COLS
        .iter()
        .find(|(c, _)| *c == container)
        .map_or(DEFAULT_GRID_COLS, |(_, cols)| *cols)
}

/// The Overview box kind of a container id such as "overview.gpu".
pub fn overview_kind(container: &str) -> Option<&str> {
    container.strip_prefix(OVERVIEW_PREFIX)
}

/// The container id of the Overview box `kind`, such as "overview.gpu" for "gpu".
pub fn overview_container(kind: &str) -> String {
    format!("{OVERVIEW_PREFIX}{kind}")
}

/// The preset ids of `container`, the default first.
pub(crate) fn presets_of(container: &str) -> &'static [&'static str] {
    overview_kind(container)
        .and_then(|kind| OVERVIEW_BOX_MODES.iter().find(|(k, _)| *k == kind))
        .map_or(&[], |(_, modes)| *modes)
}

impl LayoutsSetting {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The entry of `container`, known or kept.
    pub fn entry(&self, container: &str) -> Option<&LayoutEntry> {
        self.0.iter().find(|e| e.container() == Some(container))
    }

    /// The known layout of `container`.
    pub fn known(&self, container: &str) -> Option<&ContainerLayout> {
        self.0.iter().find_map(|e| match e {
            LayoutEntry::Known(l) if l.container == container => Some(l),
            _ => None,
        })
    }

    /// Replaces the entry of `container` with `layout`, or adds it at the end. Sets the
    /// layout's version to the one its content is written at.
    pub fn set(&mut self, mut layout: ContainerLayout) {
        layout.version = layout.written_version();
        let container = layout.container.clone();
        let entry = LayoutEntry::Known(layout);
        match self
            .0
            .iter_mut()
            .find(|e| e.container() == Some(container.as_str()))
        {
            Some(e) => *e = entry,
            None => self.0.push(entry),
        }
    }

    /// Removes every entry of `container`. Returns whether there was one.
    pub fn remove(&mut self, container: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|e| e.container() != Some(container));
        self.0.len() != before
    }

    /// The ADR-019 load rules. Each entry is read again from its table, so running this twice
    /// gives the same result. Returns `true` when it dropped or changed anything, for one
    /// `warn!` line.
    pub fn load(&mut self) -> bool {
        let mut dropped = false;
        let mut out: Vec<LayoutEntry> = Vec::with_capacity(self.0.len());
        for entry in std::mem::take(&mut self.0) {
            let table = match entry {
                LayoutEntry::Known(l) => l.to_table(),
                LayoutEntry::Kept(t) => t,
            };
            let Some(container) = table.get("container").and_then(Value::as_str) else {
                dropped = true;
                continue;
            };
            if out.iter().any(|e| e.container() == Some(container)) {
                dropped = true;
                continue;
            }
            let version = match table.get("version") {
                None => Some(1),
                Some(v) => v.as_integer().and_then(|v| u32::try_from(v).ok()),
            };
            let readable = version.is_some_and(|v| (1..=LAYOUT_VERSION).contains(&v));
            if !readable || !LAYOUT_CONTAINERS.contains(&container) {
                out.push(LayoutEntry::Kept(table));
                continue;
            }
            let (layout, changed) = ContainerLayout::from_table(container, &table);
            dropped |= changed;
            out.push(LayoutEntry::Known(layout));
        }
        self.0 = out;
        dropped
    }
}

impl LayoutEntry {
    /// The entry's `container`, when it has a string one.
    pub fn container(&self) -> Option<&str> {
        match self {
            LayoutEntry::Known(l) => Some(&l.container),
            LayoutEntry::Kept(t) => t.get("container").and_then(Value::as_str),
        }
    }
}

impl Serialize for LayoutsSetting {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let tables: Vec<Table> = self
            .0
            .iter()
            .map(|e| match e {
                LayoutEntry::Known(l) => l.to_table(),
                LayoutEntry::Kept(t) => t.clone(),
            })
            .collect();
        tables.serialize(s)
    }
}

impl<'de> Deserialize<'de> for LayoutsSetting {
    /// Every entry loads as `Kept`. An item that is not a table, or a value that is not a list,
    /// loads as an empty table, which the load rules drop with the warn line.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let entries = match Value::deserialize(d)? {
            Value::Array(items) => items
                .into_iter()
                .map(|item| match item {
                    Value::Table(t) => LayoutEntry::Kept(t),
                    _ => LayoutEntry::Kept(Table::new()),
                })
                .collect(),
            _ => vec![LayoutEntry::Kept(Table::new())],
        };
        Ok(Self(entries))
    }
}

/// A split rounded to 3 decimals, as `[performance.overview]` writes it.
fn round_split(v: f64) -> f64 {
    (v * 1000.).round() / 1000.
}

/// A TOML number as `f64`; integers count.
fn as_number(v: &Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

/// A TOML integer that fits `T`.
fn as_int<T: TryFrom<i64>>(v: Option<&Value>) -> Option<T> {
    v.and_then(Value::as_integer)
        .and_then(|i| T::try_from(i).ok())
}

impl ContainerLayout {
    /// A flow layout of `container` with `preset` and `widgets` (content and forms) in list
    /// order, ids from 1. The conversions and the built-in defaults use it.
    pub fn flow(
        container: &str,
        preset: PresetSetting,
        widgets: impl IntoIterator<Item = (WidgetContent, FormSet)>,
    ) -> Self {
        let mut next_id = 1;
        let widgets = widgets
            .into_iter()
            .map(|(content, forms)| {
                let w = WidgetSetting {
                    id: next_id,
                    content,
                    forms,
                    cell: None,
                    options: WidgetOptions::default(),
                    device_name: None,
                };
                next_id += 1;
                w
            })
            .collect();
        let mut layout = Self {
            container: container.to_owned(),
            version: LAYOUT_VERSION,
            preset: Some(preset),
            widgets,
            next_id,
        };
        layout.version = layout.written_version();
        layout
    }

    /// The version the entry is written at: [`LAYOUT_VERSION_GPU_ONLY`] for an `overview.gpu`
    /// entry whose widgets are all single-metric widgets, so a 1.2.0 build still reads it, and
    /// [`LAYOUT_VERSION`] for every other entry.
    pub fn written_version(&self) -> u32 {
        let phase1 = self.container == GPU_BOX_CONTAINER
            && self
                .widgets
                .iter()
                .all(|w| matches!(w.content, WidgetContent::Metric(_)));
        if phase1 {
            LAYOUT_VERSION_GPU_ONLY
        } else {
            LAYOUT_VERSION
        }
    }

    /// The entry as written: `container`, `version`, `preset` (flow layouts only), `widgets`
    /// and `next_id`.
    pub fn to_table(&self) -> Table {
        let mut t = Table::new();
        t.insert("container".into(), Value::String(self.container.clone()));
        t.insert(
            "version".into(),
            Value::Integer(i64::from(self.written_version())),
        );
        if let Some(p) = &self.preset {
            let mut pt = Table::new();
            pt.insert("id".into(), Value::String(p.id.clone()));
            if let Some(a) = &p.arrange {
                pt.insert("arrange".into(), Value::String(a.clone()));
            }
            if let Some(s) = p.split {
                pt.insert("split".into(), Value::Float(round_split(s)));
            }
            if let Some(s) = p.power_split {
                pt.insert("power_split".into(), Value::Float(round_split(s)));
            }
            if let Some(pl) = &p.power_placement {
                pt.insert("power_placement".into(), Value::String(pl.clone()));
            }
            t.insert("preset".into(), Value::Table(pt));
        }
        let widgets = self
            .widgets
            .iter()
            .map(|w| Value::Table(w.to_table()))
            .collect();
        t.insert("widgets".into(), Value::Array(widgets));
        t.insert("next_id".into(), Value::Integer(i64::from(self.next_id)));
        t
    }

    /// Reads a known container's entry by the ADR-019 load rules. Returns the layout and
    /// whether any part was dropped or replaced. Unknown keys are ignored (ADR-011).
    fn from_table(container: &str, t: &Table) -> (Self, bool) {
        let mut changed = false;
        let preset = t
            .get("preset")
            .map(|v| PresetSetting::from_value(container, v, &mut changed));
        let ctx = ReadContext {
            container,
            flow: preset.is_some(),
            cols: grid_cols(container),
        };
        let mut widgets: Vec<WidgetSetting> = Vec::new();
        let items: &[Value] = match t.get("widgets") {
            None => &[],
            Some(Value::Array(items)) => items,
            Some(_) => {
                changed = true;
                &[]
            }
        };
        for item in items {
            let read = item
                .as_table()
                .and_then(|w| WidgetSetting::from_table(w, &ctx, &mut changed));
            let kept = read.filter(|w| {
                !widgets.iter().any(|k| {
                    k.id == w.id
                        || matches!((k.cell, w.cell), (Some(a), Some(b)) if a.overlaps(&b))
                        || matches!(
                            (&k.content, &w.content),
                            (WidgetContent::Fixed(a), WidgetContent::Fixed(b)) if a == b
                        )
                })
            });
            match kept {
                Some(w) => widgets.push(w),
                None => changed = true,
            }
        }
        let floor = widgets.iter().map(|w| w.id + 1).max().unwrap_or(1);
        let next_id = as_int::<u32>(t.get("next_id")).unwrap_or(0).max(floor);
        let mut layout = Self {
            container: container.to_owned(),
            version: LAYOUT_VERSION,
            preset,
            widgets,
            next_id,
        };
        layout.version = layout.written_version();
        (layout, changed)
    }
}

/// What a widget's load rules need to know about its entry.
struct ReadContext<'a> {
    container: &'a str,
    /// Whether the entry is a flow layout.
    flow: bool,
    /// The container's column count.
    cols: u16,
}

impl ReadContext<'_> {
    /// A slot is a GPU box selector of a flow layout.
    fn slot_allowed(&self) -> bool {
        self.flow && self.container == GPU_BOX_CONTAINER
    }
}

impl PresetSetting {
    /// An unknown or missing id reads as the kind's default preset; an invalid `arrange`,
    /// `split`, `power_split` or `power_placement` reads as `None` (UI-PRF-019). The power
    /// keys are read in the CPU box only.
    fn from_value(container: &str, v: &Value, changed: &mut bool) -> Self {
        let presets = presets_of(container);
        let default = presets.first().copied().unwrap_or_default();
        let Some(t) = v.as_table() else {
            *changed = true;
            return Self {
                id: default.to_owned(),
                ..Self::default()
            };
        };
        let id = match t.get("id").and_then(Value::as_str) {
            Some(id) if presets.contains(&id) => id,
            _ => {
                *changed = true;
                default
            }
        };
        let mut text = |key: &str, allowed: &[&str]| {
            t.get(key).and_then(|a| {
                let a = a.as_str().filter(|a| allowed.contains(a));
                *changed |= a.is_none();
                a.map(str::to_owned)
            })
        };
        let arrange = text("arrange", &OVERVIEW_ARRANGES);
        let cpu = container == CPU_BOX_CONTAINER;
        let power_placement = if cpu {
            text("power_placement", &OVERVIEW_POWER_PLACEMENTS)
        } else {
            None
        };
        let mut number = |key: &str, (lo, hi): (f64, f64)| {
            t.get(key).and_then(|s| {
                let s = as_number(s).filter(|s| (lo..=hi).contains(s));
                *changed |= s.is_none();
                s
            })
        };
        let split = number("split", OVERVIEW_SPLIT_RANGE);
        let power_split = if cpu {
            number("power_split", OVERVIEW_POWER_SPLIT_RANGE)
        } else {
            None
        };
        if !cpu {
            *changed |= t.contains_key("power_split") || t.contains_key("power_placement");
        }
        Self {
            id: id.to_owned(),
            arrange,
            split,
            power_split,
            power_placement,
        }
    }
}

impl WidgetSetting {
    fn to_table(&self) -> Table {
        let mut t = Table::new();
        t.insert("id".into(), Value::Integer(i64::from(self.id)));
        if let Some(device) = self.content.device() {
            match device {
                DeviceSelector::Key(k) => {
                    t.insert("device".into(), Value::String(k.to_string()));
                }
                DeviceSelector::Slot(s) => {
                    t.insert("slot".into(), Value::Integer(i64::from(*s)));
                }
            }
            if let Some(name) = &self.device_name {
                t.insert("device_name".into(), Value::String(name.to_string()));
            }
        }
        match &self.content {
            WidgetContent::Metric(_) => {}
            WidgetContent::Group(g) => {
                t.insert("group".into(), Value::String(g.members.name().to_owned()));
            }
            WidgetContent::Fixed(f) => {
                t.insert("fixed".into(), Value::String(f.name().to_owned()));
            }
        }
        if let Some(metric) = self.content.metric_key() {
            t.insert("metric".into(), Value::String(metric.as_str().to_owned()));
            let forms = self
                .forms
                .names()
                .map(|n| Value::String(n.to_owned()))
                .collect();
            t.insert("forms".into(), Value::Array(forms));
        }
        if let WidgetContent::Group(g) = &self.content {
            let o = &g.options;
            t.insert("items".into(), Value::String(o.items.name().to_owned()));
            if let Some(limit) = o.limit {
                t.insert("limit".into(), Value::Integer(i64::from(limit)));
            }
            if let Some(a) = &o.arrange {
                t.insert("arrange".into(), Value::String(a.clone()));
            }
        }
        if let Some(c) = self.cell {
            for (key, v) in [
                ("col", c.col),
                ("row", c.row),
                ("cols", c.cols),
                ("rows", c.rows),
            ] {
                t.insert(key.into(), Value::Integer(i64::from(v)));
            }
        }
        if !matches!(self.content, WidgetContent::Fixed(_)) {
            let defaults = WidgetOptions::default();
            if self.options.fill != defaults.fill {
                t.insert("fill".into(), Value::Boolean(self.options.fill));
            }
            if self.options.dashed != defaults.dashed {
                t.insert("dashed".into(), Value::Boolean(self.options.dashed));
            }
        }
        t
    }

    /// One widget by the ADR-019 load rules; `None` drops it. A repeated id, an overlap and a
    /// second fixed widget of one kind are checked by the caller. Sets `changed` when it
    /// ignored a part it could not read.
    fn from_table(t: &Table, ctx: &ReadContext<'_>, changed: &mut bool) -> Option<Self> {
        let id = as_int::<u32>(t.get("id")).filter(|&id| id < u32::MAX)?;
        let fixed = t.get("fixed").is_some();
        let (content, forms) = if fixed {
            (Self::fixed_from_table(t, ctx)?, FormSet::empty())
        } else {
            Self::source_from_table(t, ctx, changed)?
        };
        let cell = Self::cell_from_table(t, ctx, changed)?;
        let defaults = WidgetOptions::default();
        let options = if fixed {
            defaults
        } else {
            let mut flag = |key: &str, default: bool| match t.get(key) {
                None => default,
                Some(v) => v.as_bool().unwrap_or_else(|| {
                    *changed = true;
                    default
                }),
            };
            WidgetOptions {
                fill: flag("fill", defaults.fill),
                dashed: flag("dashed", defaults.dashed),
            }
        };
        let device_name = match t.get("device_name") {
            Some(v) if !fixed => {
                *changed |= v.as_str().is_none();
                v.as_str().map(Arc::from)
            }
            _ => None,
        };
        Some(Self {
            id,
            content,
            forms,
            cell,
            options,
            device_name,
        })
    }

    /// A fixed widget: dropped with a `group`, `device`, `slot` or `metric`, or when its kind
    /// is not allowed in the container. An unknown name is kept as `Other`.
    fn fixed_from_table(t: &Table, ctx: &ReadContext<'_>) -> Option<WidgetContent> {
        if ["group", "device", "slot", "metric"]
            .iter()
            .any(|k| t.contains_key(*k))
        {
            return None;
        }
        let fixed = FixedWidget::from_name(t.get("fixed")?.as_str()?);
        if fixed.container().is_some_and(|c| c != ctx.container) {
            return None;
        }
        Some(WidgetContent::Fixed(fixed))
    }

    /// A metric or group widget: its device, metric and forms, and a group's options.
    fn source_from_table(
        t: &Table,
        ctx: &ReadContext<'_>,
        changed: &mut bool,
    ) -> Option<(WidgetContent, FormSet)> {
        let names = t.get("forms")?.as_array()?;
        let forms = FormSet::from_names(names.iter().filter_map(Value::as_str));
        if forms.is_empty() {
            return None;
        }
        let device = match (t.get("device"), t.get("slot")) {
            (Some(d), None) => DeviceSelector::Key(Arc::from(d.as_str()?)),
            (None, Some(s)) if ctx.slot_allowed() => {
                DeviceSelector::Slot(as_int::<u8>(Some(s)).filter(|&s| s <= SLOT_MAX)?)
            }
            _ => return None,
        };
        let metric = MetricKey::new(t.get("metric")?.as_str()?);
        let Some(group) = t.get("group") else {
            return Some((
                WidgetContent::Metric(SourceSelector { device, metric }),
                forms,
            ));
        };
        let members = GroupKind::from_name(group.as_str()?);
        if !members.fits(&device) {
            return None;
        }
        let (lo, hi) = GROUP_LIMIT_RANGE;
        let limit = match t.get("limit") {
            None => None,
            Some(v) => Some(as_int::<u16>(Some(v)).filter(|l| (lo..=hi).contains(l))?),
        };
        let items = match t.get("items") {
            None => members.default_items(),
            Some(v) => v
                .as_str()
                .and_then(GroupItems::from_name)
                .unwrap_or_else(|| {
                    *changed = true;
                    members.default_items()
                }),
        };
        let arrange = t.get("arrange").and_then(|a| {
            let a = a.as_str().filter(|a| OVERVIEW_ARRANGES.contains(a));
            *changed |= a.is_none();
            a.map(str::to_owned)
        });
        let group = GroupSelector {
            device,
            members,
            metric,
            options: GroupOptions {
                limit,
                items,
                arrange,
            },
        };
        Some((WidgetContent::Group(group), forms))
    }

    /// The cells: not read in a flow layout; required, at least 1 by 1 and inside the columns
    /// in a grid layout. The outer `None` drops the widget.
    fn cell_from_table(
        t: &Table,
        ctx: &ReadContext<'_>,
        changed: &mut bool,
    ) -> Option<Option<WidgetCell>> {
        let cell_keys = ["col", "row", "cols", "rows"];
        if ctx.flow {
            *changed |= cell_keys.iter().any(|k| t.contains_key(*k));
            return Some(None);
        }
        let [col, row, w, h] = cell_keys.map(|k| as_int::<u16>(t.get(k)));
        let cell = WidgetCell {
            col: col?,
            row: row?,
            cols: w?,
            rows: h?,
        };
        let fits = cell.cols >= 1
            && cell.rows >= 1
            && u32::from(cell.col) + u32::from(cell.cols) <= u32::from(ctx.cols)
            && u32::from(cell.row) + u32::from(cell.rows) <= u32::from(u16::MAX);
        fits.then_some(Some(cell))
    }
}

/// The GPU box's old `hidden` names (en-US, D-029) and the metric keys each one leaves out. The
/// map is the same for every vendor.
pub const GPU_HIDDEN_NAMES: [(&str, &[&str]); 15] = [
    ("3D / Compute", &["gpu"]),
    ("Graphics", &["gpu"]),
    ("Video Encode", &["enc"]),
    ("Video Decode", &["dec"]),
    ("Video Encode/Decode", &["enc", "dec"]),
    ("Memory controller", &["memctl"]),
    ("Compute", &["compute"]),
    ("DMA", &["dma"]),
    ("JPEG", &["jpeg"]),
    ("Optical flow", &["ofa"]),
    ("Render/3D", &["render"]),
    ("Copy", &["copy"]),
    ("Video", &["video"]),
    ("Video Processing", &["video-enhance"]),
    ("VRAM", &[METRIC_VRAM]),
];

/// The metric keys the old `hidden` names leave out: `engine:<key>` for an engine, `vram` for
/// VRAM. Unknown names are ignored.
fn hidden_metrics(hidden: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for name in hidden {
        let Some((_, keys)) = GPU_HIDDEN_NAMES.iter().find(|(n, _)| n == name) else {
            continue;
        };
        for key in *keys {
            let metric = if *key == METRIC_VRAM {
                METRIC_VRAM.to_owned()
            } else {
                format!("{METRIC_ENGINE_PREFIX}{key}")
            };
            if !out.contains(&metric) {
                out.push(metric);
            }
        }
    }
    out
}

/// The preset of a converted box: the entry's mode (or the kind's default) with its valid
/// `arrange` and `split`. The CPU box's power keys are added by its conversion.
pub(crate) fn converted_preset(container: &str, entry: &OverviewGridSetting) -> PresetSetting {
    let presets = presets_of(container);
    let id = entry
        .mode
        .as_deref()
        .filter(|m| presets.contains(m))
        .or_else(|| presets.first().copied())
        .unwrap_or_default();
    let arrange = entry
        .arrange
        .clone()
        .filter(|a| OVERVIEW_ARRANGES.contains(&a.as_str()));
    let (lo, hi) = OVERVIEW_SPLIT_RANGE;
    let split = entry.split.filter(|s| (lo..=hi).contains(s));
    PresetSetting {
        id: id.to_owned(),
        arrange,
        split,
        ..PresetSetting::default()
    }
}

/// Converts the GPU box's old settings (`mode`, `split`, `arrange` and `hidden` of its
/// `[performance.overview]` grid entry) to a flow layout (ADR-019). Deterministic: it reads no
/// device, so the same entry always gives the same layout. An entry with none of the keys gives
/// the built-in default.
///
/// The preset is the entry's mode (or `engine_charts`) with its `arrange` and `split`. The
/// widgets are the mode's list for slot 0, then for slot 1, with ids from 1. `hidden` leaves out
/// the engine widgets and the VRAM widget it names. The utilization chart of `chart_engines`
/// and `chart` is not an engine chart, so `hidden` never leaves it out, as before.
pub fn convert_gpu_box(entry: &OverviewGridSetting) -> ContainerLayout {
    let preset = converted_preset(GPU_BOX_CONTAINER, entry);
    let hidden = hidden_metrics(&entry.hidden);
    let chart_value_label = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
    let row = FormSet::CHART | FormSet::BAR | FormSet::VALUE | FormSet::LABEL;
    let bar_value_label = FormSet::BAR | FormSet::VALUE | FormSet::LABEL;
    // (metric, forms, whether `hidden` can leave it out)
    let mut list: Vec<(MetricKey, FormSet, bool)> = Vec::new();
    match preset.id.as_str() {
        "chart_engines" => {
            list.push((MetricKey::engine(GpuEngine::Gpu), FormSet::CHART, false));
            for e in GpuEngine::ALL
                .into_iter()
                .filter(|e| *e != GpuEngine::MemCtl)
            {
                list.push((MetricKey::engine(e), row, true));
            }
            list.push((MetricKey::new(METRIC_VRAM), bar_value_label, true));
            list.push((MetricKey::new(METRIC_TEMP), FormSet::VALUE, false));
            list.push((MetricKey::new(METRIC_POWER), FormSet::VALUE, false));
        }
        "chart" => {
            list.push((MetricKey::engine(GpuEngine::Gpu), FormSet::CHART, false));
            list.push((MetricKey::new(METRIC_VRAM), bar_value_label, true));
        }
        _ => {
            for e in GpuEngine::ALL {
                list.push((MetricKey::engine(e), chart_value_label, true));
            }
            list.push((MetricKey::new(METRIC_VRAM), bar_value_label, true));
        }
    }
    list.retain(|(m, _, hideable)| !hideable || !hidden.iter().any(|h| h == m.as_str()));
    let widgets = (0..=SLOT_MAX).flat_map(|slot| {
        list.iter().map(move |(metric, forms, _)| {
            let source = SourceSelector {
                device: DeviceSelector::Slot(slot),
                metric: metric.clone(),
            };
            (WidgetContent::Metric(source), *forms)
        })
    });
    ContainerLayout::flow(GPU_BOX_CONTAINER, preset, widgets)
}

/// The grid entry of the Overview box `kind` in `[performance.overview]`, when the layout is
/// saved.
fn grid_entry<'a>(perf: &'a PerformanceSettings, kind: &str) -> Option<&'a OverviewGridSetting> {
    perf.overview
        .as_ref()
        .and_then(|o| o.grid.iter().find(|e| e.kind == kind))
}

/// Whether the box `kind`'s grid entry has a key its conversion reads (ADR-019 phase 2 "the
/// old keys per kind"). Top processes and Timeline have none.
pub fn has_old_keys(kind: &str, entry: &OverviewGridSetting) -> bool {
    let common = entry.mode.is_some() || entry.split.is_some() || entry.arrange.is_some();
    match kind {
        "cpu" => {
            common
                || entry.power
                || entry.power_split.is_some()
                || entry.power_placement.is_some()
                || entry.clocks
        }
        "gpu" => common || !entry.hidden.is_empty(),
        "memory" | "disk" | "net" => common,
        _ => false,
    }
}

/// Removes the old keys of the box `kind` from its grid entry; the other keys (`kind`, the
/// cells) stay.
fn clear_old_keys(kind: &str, entry: &mut OverviewGridSetting) {
    if !has_old_keys(kind, entry) {
        return;
    }
    entry.mode = None;
    entry.split = None;
    entry.arrange = None;
    match kind {
        "cpu" => {
            entry.power = false;
            entry.power_split = None;
            entry.power_placement = None;
            entry.clocks = false;
        }
        "gpu" => entry.hidden.clear(),
        _ => {}
    }
}

/// The conversion step of `Settings::clamp`: for each Overview box with no entry and an old key
/// in its grid entry, adds the converted flow layout, in [`OVERVIEW_BOX_MODES`] order. Returns
/// `true` when it converted any. Once a box's entry exists it does nothing for that box, so
/// each converts once. The old keys stay as loaded.
pub fn convert_old_settings(perf: &mut PerformanceSettings) -> bool {
    let mut converted = false;
    for (kind, _) in OVERVIEW_BOX_MODES {
        let container = overview_container(kind);
        if perf.layouts.entry(&container).is_some() {
            continue;
        }
        let Some(entry) = grid_entry(perf, kind).filter(|e| has_old_keys(kind, e)) else {
            continue;
        };
        if let Some(layout) = convert_box(kind, entry) {
            perf.layouts.0.push(LayoutEntry::Known(layout));
            converted = true;
        }
    }
    converted
}

/// The built-in default of the box `kind`: its conversion of an entry with no keys, or for
/// Top processes and Timeline their one preset. Used in memory and never written.
pub fn default_box_layout(kind: &str) -> Option<ContainerLayout> {
    convert_box(kind, &OverviewGridSetting::default())
        .or_else(|| layout_boxes::fixed_box_default(kind))
}

/// The layout the Overview box `kind` shows: its known entry, else its old keys converted in
/// memory (a kept entry from a newer build leaves the box on its old keys), else the built-in
/// default. `None` for a kind that does not exist.
pub fn box_layout(perf: &PerformanceSettings, kind: &str) -> Option<ContainerLayout> {
    let container = overview_container(kind);
    if let Some(layout) = perf.layouts.known(&container) {
        return Some(layout.clone());
    }
    grid_entry(perf, kind)
        .and_then(|entry| convert_box(kind, entry))
        .or_else(|| default_box_layout(kind))
}

/// Saves the default for `container` (ADR-019 "Saving the default"): removes its entry and, for
/// an Overview box, the old keys its conversion reads from the box's grid entry. The box's
/// other grid keys stay. The next load then shows the built-in default and converts nothing.
pub fn reset_to_default(perf: &mut PerformanceSettings, container: &str) {
    perf.layouts.remove(container);
    let Some(kind) = overview_kind(container) else {
        return;
    };
    let Some(overview) = perf.overview.as_mut() else {
        return;
    };
    for entry in overview.grid.iter_mut().filter(|e| e.kind == kind) {
        clear_old_keys(kind, entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{LAYOUTS_CONVERTED_KEY, LAYOUTS_KEY, OVERVIEW_KEY, Settings};

    /// Loads `text` as `settings.toml` the way load steps 3 and 4 do.
    fn load(text: &str) -> (Settings, Vec<&'static str>) {
        let mut s: Settings = toml::from_str(text).expect("valid TOML");
        let changed = s.clamp();
        (s, changed)
    }

    /// Saves and loads again.
    fn reload(s: &Settings) -> (Settings, Vec<&'static str>) {
        load(&toml::to_string(s).expect("serializes"))
    }

    /// A settings file whose GPU box grid entry has `keys`.
    fn gpu_entry(keys: &str) -> String {
        let sep = if keys.is_empty() { "" } else { ", " };
        format!(
            "[performance.overview]\ngrid = [\n  {{ kind = \"cpu\", col = 0, row = 0, cols = 6, rows = 4 }},\n  {{ kind = \"gpu\", col = 6, row = 0, cols = 6, rows = 4{sep}{keys} }},\n]\n"
        )
    }

    fn metrics(l: &ContainerLayout, slot: u8) -> Vec<(String, Vec<&'static str>)> {
        l.widgets
            .iter()
            .filter_map(|w| w.content.metric().map(|s| (s, w.forms)))
            .filter(|(s, _)| s.device == DeviceSelector::Slot(slot))
            .map(|(s, forms)| (s.metric.as_str().to_owned(), forms.names().collect()))
            .collect()
    }

    fn engines(keys: &[&str], forms: &[&'static str]) -> Vec<(String, Vec<&'static str>)> {
        keys.iter()
            .map(|k| (format!("engine:{k}"), forms.to_vec()))
            .collect()
    }

    const ALL_KEYS: [&str; 12] = [
        "gpu",
        "memctl",
        "enc",
        "dec",
        "jpeg",
        "ofa",
        "compute",
        "dma",
        "render",
        "copy",
        "video",
        "video-enhance",
    ];

    /// The layout the GPU box shows.
    fn gpu_layout(perf: &PerformanceSettings) -> ContainerLayout {
        box_layout(perf, "gpu").expect("the GPU box has a layout")
    }

    fn converted(s: &Settings) -> &ContainerLayout {
        s.performance
            .layouts
            .known(GPU_BOX_CONTAINER)
            .expect("an overview.gpu entry")
    }

    #[test]
    fn engine_charts_mode_converts() {
        let (s, changed) = load(&gpu_entry("mode = \"engine_charts\""));
        assert!(changed.contains(&LAYOUTS_CONVERTED_KEY));
        assert!(!changed.contains(&LAYOUTS_KEY));
        let l = converted(&s);
        let p = l.preset.as_ref().expect("a flow layout");
        assert_eq!(p.id, "engine_charts");
        assert_eq!((p.arrange.as_deref(), p.split), (None, None));
        let mut want = engines(&ALL_KEYS, &["chart", "value", "label"]);
        want.push(("vram".into(), vec!["bar", "value", "label"]));
        assert_eq!(metrics(l, 0), want);
        assert_eq!(metrics(l, 1), want);
        let ids: Vec<u32> = l.widgets.iter().map(|w| w.id).collect();
        assert_eq!(ids, (1..=26).collect::<Vec<_>>());
        assert_eq!(l.next_id, 27);
        assert!(l.widgets.iter().all(|w| w.cell.is_none()));
    }

    #[test]
    fn chart_engines_mode_converts() {
        let (s, _) = load(&gpu_entry("mode = \"chart_engines\""));
        let l = converted(&s);
        let mut want = vec![("engine:gpu".to_owned(), vec!["chart"])];
        let rows: Vec<&str> = ALL_KEYS
            .iter()
            .copied()
            .filter(|k| *k != "memctl")
            .collect();
        want.extend(engines(&rows, &["chart", "bar", "value", "label"]));
        want.push(("vram".into(), vec!["bar", "value", "label"]));
        want.push(("temp".into(), vec!["value"]));
        want.push(("power".into(), vec!["value"]));
        assert_eq!(metrics(l, 0), want);
        assert_eq!(metrics(l, 1), want);
        assert_eq!(
            l.preset.as_ref().map(|p| p.id.as_str()),
            Some("chart_engines")
        );
    }

    #[test]
    fn chart_mode_converts() {
        let (s, _) = load(&gpu_entry("mode = \"chart\""));
        let l = converted(&s);
        let want = vec![
            ("engine:gpu".to_owned(), vec!["chart"]),
            ("vram".to_owned(), vec!["bar", "value", "label"]),
        ];
        assert_eq!(metrics(l, 0), want);
        assert_eq!(metrics(l, 1), want);
        assert_eq!(l.next_id, 5);
    }

    #[test]
    fn hidden_engines_left_out() {
        let (s, _) = load(&gpu_entry(
            "hidden = [\"Video Encode/Decode\", \"Graphics\", \"Not an engine\"]",
        ));
        let l = converted(&s);
        assert_eq!(
            l.preset.as_ref().map(|p| p.id.as_str()),
            Some("engine_charts")
        );
        let keys: Vec<&str> = ALL_KEYS
            .iter()
            .copied()
            .filter(|k| !["enc", "dec", "gpu"].contains(k))
            .collect();
        let mut want = engines(&keys, &["chart", "value", "label"]);
        want.push(("vram".into(), vec!["bar", "value", "label"]));
        assert_eq!(metrics(l, 0), want);
        // The old key stays as loaded.
        let entry = grid_entry(&s.performance, "gpu").expect("gpu entry");
        assert_eq!(entry.hidden.len(), 3);
    }

    #[test]
    fn hidden_vram_and_utilization_chart_kept() {
        let (s, _) = load(&gpu_entry(
            "mode = \"chart_engines\", hidden = [\"VRAM\", \"3D / Compute\", \"Memory controller\"]",
        ));
        let l = converted(&s);
        let first = metrics(l, 0);
        assert_eq!(first[0], ("engine:gpu".to_owned(), vec!["chart"]));
        assert!(!first.iter().any(|(m, _)| m == "vram"));
        assert_eq!(first.iter().filter(|(m, _)| m == "engine:gpu").count(), 1);
        let (s, _) = load(&gpu_entry("mode = \"chart\", hidden = [\"VRAM\"]"));
        assert_eq!(
            metrics(converted(&s), 0),
            vec![("engine:gpu".to_owned(), vec!["chart"])]
        );
    }

    #[test]
    fn arrange_and_split_carried() {
        let (s, _) = load(&gpu_entry(
            "mode = \"chart_engines\", arrange = \"horizontal\", split = 0.35",
        ));
        let p = converted(&s).preset.clone().expect("flow");
        assert_eq!(p.arrange.as_deref(), Some("horizontal"));
        assert_eq!(p.split, Some(0.35));
        // A split alone also converts.
        let (s, changed) = load(&gpu_entry("split = 0.6"));
        assert!(changed.contains(&LAYOUTS_CONVERTED_KEY));
        assert_eq!(
            converted(&s).preset.as_ref().and_then(|p| p.split),
            Some(0.6)
        );
    }

    #[test]
    fn no_old_key_no_conversion() {
        for text in [String::new(), gpu_entry("")] {
            let (s, changed) = load(&text);
            assert!(s.performance.layouts.is_empty());
            assert!(!changed.contains(&LAYOUTS_CONVERTED_KEY));
            let out = toml::to_string(&s).expect("serializes");
            assert!(!out.contains("layouts"), "{out}");
        }
        // The built-in default is engine_charts, in memory only.
        let (s, _) = load("");
        let l = gpu_layout(&s.performance);
        assert_eq!(l, convert_gpu_box(&OverviewGridSetting::default()));
        assert_eq!(
            l.preset.as_ref().map(|p| p.id.as_str()),
            Some("engine_charts")
        );
    }

    #[test]
    fn conversion_is_idempotent() {
        for keys in [
            "mode = \"engine_charts\"",
            "mode = \"chart_engines\", arrange = \"vertical\", split = 0.25",
            "mode = \"chart\", hidden = [\"VRAM\"]",
            "hidden = [\"Compute\", \"DMA\"]",
        ] {
            let (first, changed) = load(&gpu_entry(keys));
            assert!(changed.contains(&LAYOUTS_CONVERTED_KEY));
            let (second, changed) = reload(&first);
            assert!(!changed.contains(&LAYOUTS_CONVERTED_KEY), "{keys}");
            assert!(!changed.contains(&LAYOUTS_KEY), "{keys}");
            assert_eq!(second, first, "{keys}");
            // Clamping the loaded value again changes nothing either.
            let mut third = second.clone();
            assert!(third.clamp().is_empty(), "{keys}");
            assert_eq!(third, second);
            // The same entry always gives the same layout.
            let entry = grid_entry(&first.performance, "gpu").expect("gpu entry");
            assert_eq!(convert_gpu_box(entry), convert_gpu_box(entry));
        }
    }

    #[test]
    fn invalid_old_mode_dropped_then_default_converted() {
        let (s, changed) = load(&gpu_entry("mode = \"bogus\", arrange = \"auto\""));
        assert!(changed.contains(&OVERVIEW_KEY));
        let p = converted(&s).preset.clone().expect("flow");
        assert_eq!(p.id, "engine_charts");
        assert_eq!(p.arrange.as_deref(), Some("auto"));
    }

    const GRID: &str =
        "[[performance.layouts]]\ncontainer = \"overview.gpu\"\nversion = 1\nnext_id = 3\n";

    #[test]
    fn absent_and_empty_differ() {
        let (none, _) = load(&gpu_entry(""));
        assert!(none.performance.layouts.is_empty());
        let (empty, changed) = load(&format!("{GRID}widgets = []\n"));
        assert!(changed.is_empty(), "{changed:?}");
        let l = converted(&empty);
        assert!(l.widgets.is_empty());
        assert!(l.preset.is_none());
        let (back, _) = reload(&empty);
        assert_eq!(back, empty);
        let out = toml::to_string(&empty).expect("serializes");
        assert!(out.contains("widgets = []"), "{out}");
    }

    #[test]
    fn empty_entry_blocks_conversion() {
        let text = format!("{}{GRID}widgets = []\n", gpu_entry("mode = \"chart\""));
        let (s, changed) = load(&text);
        assert!(!changed.contains(&LAYOUTS_CONVERTED_KEY));
        assert!(converted(&s).widgets.is_empty());
    }

    #[test]
    fn grid_layout_round_trips() {
        let text = format!(
            "{GRID}widgets = [\n  {{ id = 1, device = \"gpu:GPU-0a1b\", device_name = \"RTX 4090\", metric = \"engine:gpu\", forms = [\"label\", \"chart\", \"value\"], col = 0, row = 0, cols = 3, rows = 4 }},\n  {{ id = 2, device = \"gpu:GPU-0a1b\", metric = \"vram\", forms = [\"bar\", \"stats\"], col = 0, row = 4, cols = 6, rows = 1, fill = false, dashed = true }},\n]\n"
        );
        let (s, changed) = load(&text);
        assert!(changed.is_empty(), "{changed:?}");
        let l = converted(&s);
        assert_eq!(l.widgets.len(), 2);
        let w = &l.widgets[0];
        assert_eq!(
            w.content.device(),
            Some(&DeviceSelector::Key(Arc::from("gpu:GPU-0a1b")))
        );
        assert_eq!(w.forms, FormSet::CHART | FormSet::VALUE | FormSet::LABEL);
        assert_eq!(w.device_name.as_deref(), Some("RTX 4090"));
        assert_eq!(
            w.cell,
            Some(WidgetCell {
                col: 0,
                row: 0,
                cols: 3,
                rows: 4
            })
        );
        assert_eq!(
            l.widgets[1].options,
            WidgetOptions {
                fill: false,
                dashed: true
            }
        );
        let (back, changed) = reload(&s);
        assert!(changed.is_empty(), "{changed:?}");
        assert_eq!(back, s);
        let out = toml::to_string(&s).expect("serializes");
        assert!(!out.contains("fill = true"), "{out}");
        assert!(!out.contains("dashed = false"), "{out}");
    }

    #[test]
    fn unknown_metric_and_device_kept() {
        let text = format!(
            "{GRID}widgets = [\n  {{ id = 1, device = \"gpu:gone\", metric = \"engine:warp\", forms = [\"chart\"], col = 0, row = 0, cols = 3, rows = 4 }},\n  {{ id = 2, device = \"gpu:gone\", metric = \"fan_speed\", forms = [\"value\"], col = 3, row = 0, cols = 3, rows = 1 }},\n]\n"
        );
        let (s, changed) = load(&text);
        assert!(changed.is_empty(), "{changed:?}");
        let l = converted(&s);
        assert_eq!(l.widgets.len(), 2);
        assert_eq!(
            l.widgets[0].content.metric_key().and_then(|k| k.metric()),
            None
        );
        assert_eq!(
            l.widgets[1].content.metric_key().map(MetricKey::as_str),
            Some("fan_speed")
        );
        let (back, _) = reload(&s);
        assert_eq!(back, s);
    }

    #[test]
    fn newer_version_and_unknown_container_kept() {
        let text = "[[performance.layouts]]\ncontainer = \"overview.gpu\"\nversion = 3\nshape = \"hex\"\nwidgets = [{ id = 1, wobble = true }]\n\n[[performance.layouts]]\ncontainer = \"page.cpu\"\nversion = 1\nwidgets = [{ id = 9, slot = 4 }]\n";
        let (s, changed) = load(&format!("{}{text}", gpu_entry("mode = \"chart\"")));
        assert!(changed.is_empty(), "{changed:?}");
        let entries = &s.performance.layouts.0;
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| matches!(e, LayoutEntry::Kept(_))));
        // A kept overview.gpu entry blocks conversion; the box shows its old keys.
        let l = gpu_layout(&s.performance);
        assert_eq!(l.preset.as_ref().map(|p| p.id.as_str()), Some("chart"));
        // Written back unchanged.
        let original: Settings = toml::from_str(text).expect("parses");
        let out = toml::to_string(&s).expect("serializes");
        let back: Settings = toml::from_str(&out).expect("parses");
        assert_eq!(back.performance.layouts, original.performance.layouts);
    }

    #[test]
    fn missing_version_reads_as_1() {
        let (s, changed) =
            load("[[performance.layouts]]\ncontainer = \"overview.gpu\"\nwidgets = []\n");
        assert!(changed.is_empty(), "{changed:?}");
        assert_eq!(converted(&s).version, 1);
        assert!(toml::to_string(&s).unwrap().contains("version = 1"));
    }

    #[test]
    fn entry_drop_rules() {
        let text = "[[performance.layouts]]\nversion = 1\n\n[[performance.layouts]]\ncontainer = 5\n\n[[performance.layouts]]\ncontainer = \"overview.gpu\"\nwidgets = []\n\n[[performance.layouts]]\ncontainer = \"overview.gpu\"\npreset = { id = \"chart\" }\nwidgets = []\n";
        let (s, changed) = load(text);
        assert_eq!(changed, vec![LAYOUTS_KEY]);
        assert_eq!(s.performance.layouts.0.len(), 1);
        assert!(converted(&s).preset.is_none());
        let (s, changed) = load("[performance]\nlayouts = 3\n");
        assert_eq!(changed, vec![LAYOUTS_KEY]);
        assert!(s.performance.layouts.is_empty());
    }

    #[test]
    fn preset_rules() {
        let flow = |preset: &str| {
            let (s, changed) = load(&format!(
                "[[performance.layouts]]\ncontainer = \"overview.gpu\"\npreset = {preset}\nwidgets = []\n"
            ));
            (converted(&s).preset.clone().expect("flow"), changed)
        };
        let (p, changed) = flow("{ id = \"chart_engines\", arrange = \"vertical\", split = 0.4 }");
        assert!(changed.is_empty());
        assert_eq!(
            p,
            PresetSetting {
                id: "chart_engines".into(),
                arrange: Some("vertical".into()),
                split: Some(0.4),
                ..PresetSetting::default()
            }
        );
        let (p, changed) = flow("{ id = \"bogus\", arrange = \"diagonal\", split = 0.9 }");
        assert_eq!(changed, vec![LAYOUTS_KEY]);
        assert_eq!(
            p,
            PresetSetting {
                id: "engine_charts".into(),
                ..PresetSetting::default()
            }
        );
    }

    /// Loads one grid or flow widget line and returns the ids kept.
    fn kept_ids(flow: bool, widgets: &[&str]) -> (Vec<u32>, Vec<&'static str>) {
        let preset = if flow {
            "preset = { id = \"chart\" }\n"
        } else {
            ""
        };
        let text = format!(
            "[[performance.layouts]]\ncontainer = \"overview.gpu\"\n{preset}widgets = [\n{}\n]\n",
            widgets.join(",\n")
        );
        let (s, changed) = load(&text);
        (
            converted(&s).widgets.iter().map(|w| w.id).collect(),
            changed,
        )
    }

    const OK: &str = "{ id = 1, device = \"gpu:a\", metric = \"vram\", forms = [\"bar\"], col = 0, row = 0, cols = 6, rows = 1 }";

    #[test]
    fn widget_drop_rules_drop_only_their_widget() {
        let bad = [
            // id missing, repeated, negative
            "{ device = \"gpu:a\", metric = \"vram\", forms = [\"bar\"], col = 0, row = 1, cols = 1, rows = 1 }",
            "{ id = 1, device = \"gpu:a\", metric = \"vram\", forms = [\"bar\"], col = 0, row = 1, cols = 1, rows = 1 }",
            "{ id = -2, device = \"gpu:a\", metric = \"vram\", forms = [\"bar\"], col = 0, row = 1, cols = 1, rows = 1 }",
            // no known form
            "{ id = 2, device = \"gpu:a\", metric = \"vram\", forms = [\"sparkle\"], col = 0, row = 1, cols = 1, rows = 1 }",
            "{ id = 2, device = \"gpu:a\", metric = \"vram\", forms = [], col = 0, row = 1, cols = 1, rows = 1 }",
            // device and slot, neither, slot in a grid layout
            "{ id = 2, device = \"gpu:a\", slot = 0, metric = \"vram\", forms = [\"bar\"], col = 0, row = 1, cols = 1, rows = 1 }",
            "{ id = 2, metric = \"vram\", forms = [\"bar\"], col = 0, row = 1, cols = 1, rows = 1 }",
            "{ id = 2, slot = 0, metric = \"vram\", forms = [\"bar\"], col = 0, row = 1, cols = 1, rows = 1 }",
            // cells missing, span below 1, past the columns, overlapping
            "{ id = 2, device = \"gpu:a\", metric = \"vram\", forms = [\"bar\"], col = 0, row = 1, cols = 1 }",
            "{ id = 2, device = \"gpu:a\", metric = \"vram\", forms = [\"bar\"], col = 0, row = 1, cols = 0, rows = 1 }",
            "{ id = 2, device = \"gpu:a\", metric = \"vram\", forms = [\"bar\"], col = 4, row = 1, cols = 3, rows = 1 }",
            "{ id = 2, device = \"gpu:a\", metric = \"vram\", forms = [\"bar\"], col = 5, row = 0, cols = 1, rows = 2 }",
            // not a table
            "7",
        ];
        for b in bad {
            let (ids, changed) = kept_ids(false, &[OK, b]);
            assert_eq!(ids, vec![1], "{b}");
            assert_eq!(changed, vec![LAYOUTS_KEY], "{b}");
        }
        // Unknown form names are ignored.
        let (ids, changed) = kept_ids(
            false,
            &[
                OK,
                "{ id = 5, device = \"gpu:a\", metric = \"temp\", forms = [\"value\", \"glow\"], col = 0, row = 1, cols = 2, rows = 1 }",
            ],
        );
        assert_eq!(ids, vec![1, 5]);
        assert!(changed.is_empty());
    }

    #[test]
    fn flow_widget_rules() {
        let slot =
            |s: &str| format!("{{ id = 1, slot = {s}, metric = \"vram\", forms = [\"bar\"] }}");
        let (ids, changed) = kept_ids(true, &[&slot("1")]);
        assert_eq!((ids, changed), (vec![1], vec![]));
        let (ids, _) = kept_ids(true, &[&slot("2")]);
        assert!(ids.is_empty());
        let (ids, _) = kept_ids(
            true,
            &["{ id = 1, device = \"gpu:a\", metric = \"vram\", forms = [\"bar\"] }"],
        );
        assert_eq!(ids, vec![1]);
        // Cells in a flow layout are not read.
        let (s, changed) = load(
            "[[performance.layouts]]\ncontainer = \"overview.gpu\"\npreset = { id = \"chart\" }\nwidgets = [{ id = 1, slot = 0, metric = \"vram\", forms = [\"bar\"], col = 0, row = 0, cols = 1, rows = 1 }]\n",
        );
        assert_eq!(changed, vec![LAYOUTS_KEY]);
        assert_eq!(converted(&s).widgets[0].cell, None);
    }

    #[test]
    fn next_id_raised_never_lowered() {
        let (s, _) =
            load(&format!("{GRID}widgets = [{OK}]\n").replace("next_id = 3", "next_id = 0"));
        assert_eq!(converted(&s).next_id, 2);
        let (s, _) =
            load(&format!("{GRID}widgets = [{OK}]\n").replace("next_id = 3", "next_id = 40"));
        assert_eq!(converted(&s).next_id, 40);
        let (s, _) = load("[[performance.layouts]]\ncontainer = \"overview.gpu\"\nwidgets = []\n");
        assert_eq!(converted(&s).next_id, 1);
    }

    #[test]
    fn reset_to_default_after_conversion() {
        let (mut s, _) = load(&gpu_entry(
            "mode = \"chart_engines\", arrange = \"vertical\", split = 0.3, hidden = [\"VRAM\"]",
        ));
        assert!(s.performance.layouts.known(GPU_BOX_CONTAINER).is_some());
        reset_to_default(&mut s.performance, GPU_BOX_CONTAINER);
        let (back, changed) = reload(&s);
        assert!(changed.is_empty(), "{changed:?}");
        assert!(back.performance.layouts.entry(GPU_BOX_CONTAINER).is_none());
        let entry = grid_entry(&back.performance, "gpu").expect("gpu entry");
        assert!(!has_old_keys("gpu", entry));
        assert_eq!((entry.col, entry.row, entry.cols, entry.rows), (6, 0, 6, 4));
        assert_eq!(
            gpu_layout(&back.performance),
            convert_gpu_box(&OverviewGridSetting::default())
        );
    }

    #[test]
    fn reset_keeps_other_containers() {
        let text = "[[performance.layouts]]\ncontainer = \"page.cpu\"\nversion = 1\n\n[[performance.layouts]]\ncontainer = \"overview.gpu\"\nwidgets = []\n";
        let (mut s, _) = load(text);
        reset_to_default(&mut s.performance, GPU_BOX_CONTAINER);
        assert_eq!(s.performance.layouts.0.len(), 1);
        assert_eq!(s.performance.layouts.0[0].container(), Some("page.cpu"));
    }

    #[test]
    fn metric_keys_map() {
        for e in GpuEngine::ALL {
            let key = MetricKey::engine(e);
            assert_eq!(key.metric(), Some(e.metric()));
            assert_eq!(key.gpu_engine(), Some(e));
        }
        let fixed = [
            (METRIC_VRAM, MetricId::GPU_MEM),
            (METRIC_CLOCK_GFX, MetricId::GPU_CLOCK),
            (METRIC_CLOCK_MEM, MetricId::GPU_MEM_CLOCK),
            (METRIC_POWER, MetricId::GPU_POWER),
            (METRIC_TEMP, MetricId::GPU_TEMP),
        ];
        for (key, id) in fixed {
            assert_eq!(MetricKey::new(key).metric(), Some(id));
        }
        assert_eq!(MetricKey::new("engine:nope").metric(), None);
    }

    #[test]
    fn form_names_in_order() {
        let set = FormSet::from_names(["label", "chart", "nope", "stats"]);
        assert_eq!(set.names().collect::<Vec<_>>(), ["chart", "stats", "label"]);
        assert!(FormSet::from_names(["x"]).is_empty());
    }

    // ---- Phase 2 (ADR-019 "Phase 2") ----

    /// A settings file whose grid has one entry of `kind` with `keys`.
    fn box_entry(kind: &str, keys: &str) -> String {
        let sep = if keys.is_empty() { "" } else { ", " };
        format!(
            "[performance.overview]\ngrid = [\n  {{ kind = \"{kind}\", col = 0, row = 0, cols = 12, rows = 8{sep}{keys} }},\n]\n"
        )
    }

    fn known<'a>(s: &'a Settings, container: &str) -> &'a ContainerLayout {
        s.performance
            .layouts
            .known(container)
            .unwrap_or_else(|| panic!("a {container} entry"))
    }

    /// A container entry with `preset` (empty for a grid layout) and `widgets`.
    fn entry_text(container: &str, preset: &str, widgets: &[&str]) -> String {
        let preset = if preset.is_empty() {
            String::new()
        } else {
            format!("preset = {preset}\n")
        };
        format!(
            "[[performance.layouts]]\ncontainer = \"{container}\"\nversion = 2\n{preset}widgets = [\n{}\n]\n",
            widgets.join(",\n")
        )
    }

    fn ids_in(container: &str, preset: &str, widgets: &[&str]) -> (Vec<u32>, Vec<&'static str>) {
        let (s, changed) = load(&entry_text(container, preset, widgets));
        (
            known(&s, container).widgets.iter().map(|w| w.id).collect(),
            changed,
        )
    }

    #[test]
    fn cpu_box_converts_with_power_and_clocks() {
        let (s, changed) = load(&box_entry(
            "cpu",
            "mode = \"core_charts\", arrange = \"vertical\", split = 0.4, power = true, power_split = 0.3, power_placement = \"under_graph\", clocks = true",
        ));
        assert!(changed.contains(&LAYOUTS_CONVERTED_KEY));
        let l = known(&s, CPU_BOX_CONTAINER);
        let p = l.preset.as_ref().expect("flow");
        assert_eq!(p.id, "core_charts");
        assert_eq!(p.arrange.as_deref(), Some("vertical"));
        assert_eq!(p.split, Some(0.4));
        assert_eq!(p.power_split, Some(0.3));
        assert_eq!(p.power_placement.as_deref(), Some("under_graph"));
        assert_eq!(l.widgets.len(), 3);
        let WidgetContent::Group(g) = &l.widgets[0].content else {
            panic!("a cores group first");
        };
        assert_eq!(g.members, GroupKind::Cores);
        assert_eq!(g.options.items, GroupItems::Cells);
        assert_eq!(g.options.arrange.as_deref(), Some("vertical"));
        let keys: Vec<_> = l
            .widgets
            .iter()
            .filter_map(|w| w.content.metric_key().map(MetricKey::as_str))
            .collect();
        assert_eq!(keys, ["load", "core_clocks", "power_draw"]);
        assert_eq!(l.version, LAYOUT_VERSION);
        // The written preset holds the power keys; the grid entry keeps the old ones.
        let out = toml::to_string(&s).expect("serializes");
        assert!(out.contains("power_placement = \"under_graph\""), "{out}");
        assert!(out.contains("clocks = true"), "{out}");
        let (back, changed) = reload(&s);
        assert!(changed.is_empty(), "{changed:?}");
        assert_eq!(back, s);
    }

    #[test]
    fn written_version_follows_the_content() {
        // A GPU entry of single-metric widgets is version 1, so 1.2.0 reads it.
        let (s, _) = load(&gpu_entry("mode = \"chart\""));
        assert_eq!(converted(&s).version, 1);
        assert!(toml::to_string(&s).unwrap().contains("version = 1"));
        // An engine group makes it version 2.
        let group = "{ id = 1, device = \"gpu:a\", group = \"engines\", metric = \"engine:*\", forms = [\"chart\"], col = 0, row = 0, cols = 6, rows = 8 }";
        let (s, changed) = load(&entry_text(GPU_BOX_CONTAINER, "", &[group]));
        assert!(changed.is_empty(), "{changed:?}");
        assert_eq!(converted(&s).version, 2);
        assert!(toml::to_string(&s).unwrap().contains("version = 2"));
        // Every other container is version 2, even an empty one.
        let (s, _) = load(&entry_text(DISK_BOX_CONTAINER, "", &[]));
        assert_eq!(known(&s, DISK_BOX_CONTAINER).version, 2);
        let mut l = known(&s, DISK_BOX_CONTAINER).clone();
        l.version = 7;
        let mut layouts = LayoutsSetting::default();
        layouts.set(l);
        assert_eq!(
            layouts.known(DISK_BOX_CONTAINER).map(|l| l.version),
            Some(2)
        );
        // Version 3 is newer than this build: kept.
        let (s, _) =
            load(&entry_text(DISK_BOX_CONTAINER, "", &[]).replace("version = 2", "version = 3"));
        assert!(matches!(s.performance.layouts.0[0], LayoutEntry::Kept(_)));
    }

    const CORES: &str = "{ id = 1, device = \"cpu\", group = \"cores\", metric = \"load\", forms = [\"chart\", \"bar\"], items = \"rows\", col = 0, row = 0, cols = 12, rows = 8 }";

    #[test]
    fn group_widget_round_trips() {
        let disks = "{ id = 2, device = \"aggregate:disk\", device_name = \"Disks\", group = \"disks\", metric = \"io\", forms = [\"chart\", \"value\", \"label\"], items = \"cells\", limit = 8, arrange = \"vertical\", col = 0, row = 8, cols = 6, rows = 8, fill = false }";
        let text = entry_text(CPU_BOX_CONTAINER, "", &[CORES]);
        let (s, changed) = load(&text);
        assert!(changed.is_empty(), "{changed:?}");
        let w = &known(&s, CPU_BOX_CONTAINER).widgets[0];
        let WidgetContent::Group(g) = &w.content else {
            panic!("a group");
        };
        assert_eq!(g.device, DeviceSelector::Key(Arc::from("cpu")));
        assert_eq!(g.metric.as_str(), "load");
        assert_eq!(
            g.options,
            GroupOptions {
                limit: None,
                items: GroupItems::Rows,
                arrange: None
            }
        );
        let (s, changed) = load(&entry_text(DISK_BOX_CONTAINER, "", &[disks]));
        assert!(changed.is_empty(), "{changed:?}");
        let w = &known(&s, DISK_BOX_CONTAINER).widgets[0];
        let WidgetContent::Group(g) = &w.content else {
            panic!("a group");
        };
        assert_eq!(g.options.limit, Some(8));
        assert_eq!(g.options.arrange.as_deref(), Some("vertical"));
        assert_eq!(w.device_name.as_deref(), Some("Disks"));
        assert!(!w.options.fill);
        let (back, changed) = reload(&s);
        assert!(changed.is_empty(), "{changed:?}");
        assert_eq!(back, s);
        let out = toml::to_string(&s).unwrap();
        assert!(out.contains("items = \"cells\""), "{out}");
        assert!(out.contains("group = \"disks\""), "{out}");
    }

    #[test]
    fn group_defaults_for_items_and_arrange() {
        let (s, changed) = load(&entry_text(
            DISK_BOX_CONTAINER,
            "",
            &[
                "{ id = 1, device = \"aggregate:disk\", group = \"disks\", metric = \"io\", forms = [\"chart\"], items = \"spiral\", arrange = \"diagonal\", col = 0, row = 0, cols = 6, rows = 8 }",
            ],
        ));
        assert_eq!(changed, vec![LAYOUTS_KEY]);
        let WidgetContent::Group(g) = &known(&s, DISK_BOX_CONTAINER).widgets[0].content else {
            panic!("a group");
        };
        assert_eq!(g.options.items, GroupItems::Cells);
        assert_eq!(g.options.arrange, None);
        // No `items` is the group's default, with no warning.
        let (s, changed) = load(&entry_text(
            CPU_BOX_CONTAINER,
            "",
            &[&CORES.replace("items = \"rows\", ", "")],
        ));
        assert!(changed.is_empty(), "{changed:?}");
        let WidgetContent::Group(g) = &known(&s, CPU_BOX_CONTAINER).widgets[0].content else {
            panic!("a group");
        };
        assert_eq!(g.options.items, GroupItems::Rows);
    }

    #[test]
    fn phase_2_drop_rules_drop_only_their_widget() {
        let ok = "{ id = 9, device = \"cpu\", metric = \"total\", forms = [\"chart\"], col = 0, row = 20, cols = 12, rows = 4 }";
        let bad = [
            // a group whose device does not fit it
            "{ id = 1, device = \"gpu:a\", group = \"cores\", metric = \"load\", forms = [\"bar\"], col = 0, row = 0, cols = 2, rows = 2 }",
            "{ id = 1, device = \"cpu\", group = \"disks\", metric = \"io\", forms = [\"chart\"], col = 0, row = 0, cols = 2, rows = 2 }",
            "{ id = 1, device = \"cpu\", group = \"adapters\", metric = \"rx\", forms = [\"chart\"], col = 0, row = 0, cols = 2, rows = 2 }",
            "{ id = 1, device = \"cpu\", group = \"engines\", metric = \"engine:*\", forms = [\"chart\"], col = 0, row = 0, cols = 2, rows = 2 }",
            // a limit outside 1..=256
            "{ id = 1, device = \"cpu\", group = \"cores\", metric = \"load\", forms = [\"bar\"], limit = 0, col = 0, row = 0, cols = 2, rows = 2 }",
            "{ id = 1, device = \"cpu\", group = \"cores\", metric = \"load\", forms = [\"bar\"], limit = 257, col = 0, row = 0, cols = 2, rows = 2 }",
            // a group with no known form
            "{ id = 1, device = \"cpu\", group = \"cores\", metric = \"load\", forms = [\"glow\"], col = 0, row = 0, cols = 2, rows = 2 }",
            // a group in a grid layout without cells
            "{ id = 1, device = \"cpu\", group = \"cores\", metric = \"load\", forms = [\"bar\"] }",
            // a fixed widget with a group, device, slot or metric
            "{ id = 1, fixed = \"processes\", device = \"cpu\", col = 0, row = 0, cols = 2, rows = 2 }",
            "{ id = 1, fixed = \"processes\", metric = \"total\", col = 0, row = 0, cols = 2, rows = 2 }",
            "{ id = 1, fixed = \"processes\", group = \"cores\", col = 0, row = 0, cols = 2, rows = 2 }",
            "{ id = 1, fixed = \"processes\", slot = 0, col = 0, row = 0, cols = 2, rows = 2 }",
            // a fixed widget of another container
            "{ id = 1, fixed = \"memory_by_app\", col = 0, row = 0, cols = 2, rows = 2 }",
            // a fixed widget in a grid layout without cells
            "{ id = 1, fixed = \"processes\" }",
            // a fixed name that is not a string
            "{ id = 1, fixed = 3, col = 0, row = 0, cols = 2, rows = 2 }",
        ];
        for b in bad {
            for container in [CPU_BOX_CONTAINER, PROCESSES_BOX_CONTAINER] {
                if container == PROCESSES_BOX_CONTAINER && !b.contains("fixed") {
                    continue;
                }
                let (ids, changed) = ids_in(container, "", &[ok, b]);
                assert_eq!(ids, vec![9], "{container}: {b}");
                assert_eq!(changed, vec![LAYOUTS_KEY], "{container}: {b}");
            }
        }
        // A second fixed widget of the same kind is dropped; the first stays.
        let first = "{ id = 1, fixed = \"processes\", col = 0, row = 0, cols = 8, rows = 10 }";
        let second = "{ id = 2, fixed = \"processes\", col = 8, row = 0, cols = 4, rows = 10 }";
        let (ids, changed) = ids_in(PROCESSES_BOX_CONTAINER, "", &[first, second]);
        assert_eq!((ids, changed), (vec![1], vec![LAYOUTS_KEY]));
        // A slot outside the GPU box drops the widget, in a flow layout too.
        let slot = "{ id = 1, slot = 0, metric = \"total\", forms = [\"chart\"] }";
        let (ids, changed) = ids_in(CPU_BOX_CONTAINER, "{ id = \"chart\" }", &[slot]);
        assert_eq!((ids, changed), (vec![], vec![LAYOUTS_KEY]));
        // An engine group of a slot is fine in a GPU box flow layout.
        let engines =
            "{ id = 1, slot = 1, group = \"engines\", metric = \"engine:*\", forms = [\"chart\"] }";
        let (ids, changed) = ids_in(GPU_BOX_CONTAINER, "{ id = \"chart\" }", &[engines]);
        assert_eq!((ids, changed), (vec![1], vec![]));
    }

    #[test]
    fn fixed_widgets_load_without_forms() {
        let (s, changed) = load(&entry_text(
            PROCESSES_BOX_CONTAINER,
            "",
            &[
                "{ id = 1, fixed = \"processes\", forms = [\"chart\"], fill = false, col = 0, row = 0, cols = 8, rows = 10 }",
                "{ id = 2, device = \"cpu\", metric = \"total\", forms = [\"chart\", \"value\", \"label\"], col = 8, row = 0, cols = 4, rows = 10 }",
            ],
        ));
        assert!(changed.is_empty(), "{changed:?}");
        let l = known(&s, PROCESSES_BOX_CONTAINER);
        assert_eq!(
            l.widgets[0].content,
            WidgetContent::Fixed(FixedWidget::Processes)
        );
        assert!(l.widgets[0].forms.is_empty());
        assert_eq!(l.widgets[0].options, WidgetOptions::default());
        // Written with its kind and cells only.
        let table = known(&s, PROCESSES_BOX_CONTAINER).to_table();
        let first = table["widgets"][0].as_table().expect("a table");
        let mut keys: Vec<&str> = first.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["col", "cols", "fixed", "id", "row", "rows"]);
        let (back, _) = reload(&s);
        assert_eq!(back, s);
        // The memory box takes Memory by app, the timeline box its timeline.
        for (container, name) in [
            (MEMORY_BOX_CONTAINER, "memory_by_app"),
            (TIMELINE_BOX_CONTAINER, "timeline"),
        ] {
            let w =
                format!("{{ id = 1, fixed = \"{name}\", col = 0, row = 0, cols = 2, rows = 2 }}");
            let (ids, changed) = ids_in(container, "", &[&w]);
            assert_eq!((ids, changed), (vec![1], vec![]), "{container}");
        }
    }

    #[test]
    fn unknown_group_and_fixed_names_kept() {
        let text = entry_text(
            NET_BOX_CONTAINER,
            "",
            &[
                "{ id = 1, device = \"aggregate:net\", group = \"bridges\", metric = \"rx\", forms = [\"chart\"], col = 0, row = 0, cols = 6, rows = 8 }",
                "{ id = 2, fixed = \"speedtest\", col = 0, row = 8, cols = 6, rows = 4 }",
            ],
        );
        let (s, changed) = load(&text);
        assert!(changed.is_empty(), "{changed:?}");
        let l = known(&s, NET_BOX_CONTAINER);
        let WidgetContent::Group(g) = &l.widgets[0].content else {
            panic!("a group");
        };
        assert_eq!(g.members, GroupKind::Other(Arc::from("bridges")));
        assert_eq!(
            l.widgets[1].content,
            WidgetContent::Fixed(FixedWidget::Other(Arc::from("speedtest")))
        );
        let out = toml::to_string(&s).unwrap();
        assert!(out.contains("group = \"bridges\""), "{out}");
        assert!(out.contains("fixed = \"speedtest\""), "{out}");
        let (back, changed) = reload(&s);
        assert!(changed.is_empty(), "{changed:?}");
        assert_eq!(back, s);
    }

    #[test]
    fn power_keys_in_the_cpu_preset_only() {
        let preset = "{ id = \"chart\", power_split = 0.2, power_placement = \"under_graph\" }";
        let (s, changed) = load(&entry_text(CPU_BOX_CONTAINER, preset, &[]));
        assert!(changed.is_empty(), "{changed:?}");
        let p = known(&s, CPU_BOX_CONTAINER).preset.clone().expect("flow");
        assert_eq!(p.power_split, Some(0.2));
        assert_eq!(p.power_placement.as_deref(), Some("under_graph"));
        let (back, _) = reload(&s);
        assert_eq!(back, s);
        // Out of range or unknown: dropped with a warning.
        let bad = "{ id = \"chart\", power_split = 0.9, power_placement = \"sideways\" }";
        let (s, changed) = load(&entry_text(CPU_BOX_CONTAINER, bad, &[]));
        assert_eq!(changed, vec![LAYOUTS_KEY]);
        let p = known(&s, CPU_BOX_CONTAINER).preset.clone().expect("flow");
        assert_eq!((p.power_split, p.power_placement), (None, None));
        // Another box does not read them.
        let (s, changed) = load(&entry_text(
            MEMORY_BOX_CONTAINER,
            &preset.replace("chart", "rows"),
            &[],
        ));
        assert_eq!(changed, vec![LAYOUTS_KEY]);
        let p = known(&s, MEMORY_BOX_CONTAINER)
            .preset
            .clone()
            .expect("flow");
        assert_eq!((p.power_split, p.power_placement), (None, None));
    }

    #[test]
    fn grid_cols_per_container() {
        assert_eq!(grid_cols(CPU_BOX_CONTAINER), 12);
        assert_eq!(grid_cols(GPU_BOX_CONTAINER), 6);
        assert_eq!(grid_cols(MEMORY_BOX_CONTAINER), 6);
        assert_eq!(grid_cols(DISK_BOX_CONTAINER), 6);
        assert_eq!(grid_cols(NET_BOX_CONTAINER), 6);
        assert_eq!(grid_cols(PROCESSES_BOX_CONTAINER), 12);
        assert_eq!(grid_cols(TIMELINE_BOX_CONTAINER), 12);
        // A 12-column widget fits the CPU box and not the GPU box.
        let wide = "{ id = 1, device = \"cpu\", metric = \"total\", forms = [\"chart\"], col = 6, row = 0, cols = 6, rows = 4 }";
        assert_eq!(ids_in(CPU_BOX_CONTAINER, "", &[wide]).0, vec![1]);
        let wide_gpu = "{ id = 1, device = \"gpu:a\", metric = \"vram\", forms = [\"bar\"], col = 6, row = 0, cols = 6, rows = 1 }";
        assert!(ids_in(GPU_BOX_CONTAINER, "", &[wide_gpu]).0.is_empty());
    }

    #[test]
    fn every_box_converts_once_and_resets() {
        let cases = [
            (
                "cpu",
                "mode = \"cores\", split = 0.3, power = true, clocks = true, power_placement = \"under_graph\", power_split = 0.4",
            ),
            ("memory", "mode = \"chart_rows\", split = 0.5"),
            ("disk", "mode = \"rows\", arrange = \"horizontal\""),
            ("net", "mode = \"combined\""),
            ("gpu", "mode = \"chart\", hidden = [\"VRAM\"]"),
        ];
        for (kind, keys) in cases {
            let container = overview_container(kind);
            let (first, changed) = load(&box_entry(kind, keys));
            assert!(changed.contains(&LAYOUTS_CONVERTED_KEY), "{kind}");
            let entry = grid_entry(&first.performance, kind).expect("grid entry");
            assert_eq!(
                known(&first, &container),
                &convert_box(kind, entry).expect("converts"),
                "{kind}"
            );
            // Converting twice changes nothing.
            let (second, changed) = reload(&first);
            assert!(changed.is_empty(), "{kind}: {changed:?}");
            assert_eq!(second, first, "{kind}");
            // Reset removes the entry and the old keys, and reloads as the default.
            let mut reset = second.clone();
            reset_to_default(&mut reset.performance, &container);
            let (back, changed) = reload(&reset);
            assert!(changed.is_empty(), "{kind}: {changed:?}");
            assert!(
                back.performance.layouts.entry(&container).is_none(),
                "{kind}"
            );
            let entry = grid_entry(&back.performance, kind).expect("grid entry");
            assert!(!has_old_keys(kind, entry), "{kind}");
            assert_eq!(
                (entry.col, entry.row, entry.cols, entry.rows),
                (0, 0, 12, 8)
            );
            assert_eq!(
                box_layout(&back.performance, kind),
                default_box_layout(kind),
                "{kind}"
            );
        }
    }

    #[test]
    fn processes_and_timeline_never_convert() {
        for kind in ["processes", "timeline"] {
            let (s, changed) = load(&box_entry(kind, ""));
            assert!(!changed.contains(&LAYOUTS_CONVERTED_KEY), "{kind}");
            assert!(s.performance.layouts.is_empty(), "{kind}");
            let l = box_layout(&s.performance, kind).expect("a default");
            assert_eq!(l.widgets.len(), 1, "{kind}");
            assert!(
                matches!(l.widgets[0].content, WidgetContent::Fixed(_)),
                "{kind}"
            );
        }
        assert_eq!(box_layout(&PerformanceSettings::default(), "nope"), None);
    }

    #[test]
    fn a_kept_entry_blocks_one_box_only() {
        let text = format!(
            "{}{}",
            "[performance.overview]\ngrid = [\n  { kind = \"cpu\", col = 0, row = 0, cols = 6, rows = 4, mode = \"chart\" },\n  { kind = \"memory\", col = 6, row = 0, cols = 6, rows = 4, mode = \"charts\" },\n]\n",
            "[[performance.layouts]]\ncontainer = \"overview.cpu\"\nversion = 9\nwidgets = []\n"
        );
        let (s, changed) = load(&text);
        assert_eq!(changed, vec![LAYOUTS_CONVERTED_KEY]);
        assert!(matches!(
            s.performance.layouts.entry(CPU_BOX_CONTAINER),
            Some(LayoutEntry::Kept(_))
        ));
        assert!(s.performance.layouts.known(MEMORY_BOX_CONTAINER).is_some());
        // The CPU box shows its old keys.
        let l = box_layout(&s.performance, "cpu").expect("a layout");
        assert_eq!(l.preset.map(|p| p.id).as_deref(), Some("chart"));
    }
}
