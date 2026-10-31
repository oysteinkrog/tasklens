//! Widget layout settings, `[[performance.layouts]]` (ADR-019, D-041). No file I/O.
//!
//! A container (an Overview box, later a resource page) holds a list of widgets. Each widget
//! shows one metric of one device in one or more forms. This module holds the plain serde data,
//! the metric keys, the load rules that `Settings::clamp` runs, the conversion of the GPU box's
//! old settings and the reset to the default. `tl-ui` turns the data into its views.
//!
//! As deserialized, every entry is [`LayoutEntry::Kept`]. `Settings::clamp` reads the entries
//! this build supports into [`LayoutEntry::Known`] and drops the invalid parts.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use crate::ids::MetricId;
use crate::settings::{
    OVERVIEW_ARRANGES, OVERVIEW_BOX_MODES, OVERVIEW_SPLIT_RANGE, OverviewGridSetting,
    PerformanceSettings,
};
use crate::system::GpuEngine;

/// The layout schema version this build reads and writes.
pub const LAYOUT_VERSION: u32 = 1;
/// The container id of the Overview GPU box.
pub const GPU_BOX_CONTAINER: &str = "overview.gpu";
/// The containers this build supports. Phase 1: [`GPU_BOX_CONTAINER`] only.
pub const LAYOUT_CONTAINERS: &[&str] = &[GPU_BOX_CONTAINER];
/// The column count of the GPU box's inner grid.
pub const GPU_BOX_GRID_COLS: u16 = 6;

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
#[derive(Clone, Debug, PartialEq)]
pub struct PresetSetting {
    /// A mode id of the container's kind in [`OVERVIEW_BOX_MODES`]: for the GPU box
    /// "engine_charts", "chart_engines" or "chart".
    pub id: String,
    /// An id from [`OVERVIEW_ARRANGES`]; `None` is the preset's default.
    pub arrange: Option<String>,
    /// In [`OVERVIEW_SPLIT_RANGE`]; `None` is the preset's default.
    pub split: Option<f64>,
}

/// One widget: one source in one or more forms.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetSetting {
    /// Unique in its container, taken from [`ContainerLayout::next_id`], so an id is never
    /// reused while the entry exists, not even after the widget that had it is removed.
    pub id: u32,
    pub source: SourceSelector,
    pub forms: FormSet,
    /// Required in a grid layout, never written in a flow layout.
    pub cell: Option<WidgetCell>,
    pub options: WidgetOptions,
    /// The device's name when the widget was placed, shown only while the device is gone.
    pub device_name: Option<Arc<str>>,
}

/// A durable source: never a runtime `SeriesId` or `DeviceIdx`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceSelector {
    pub device: DeviceSelector,
    pub metric: MetricKey,
}

/// Which device a widget shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DeviceSelector {
    /// The device's persistent key (PERF-005, R-155, ADR-017), for example
    /// "gpu:GPU-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9" or "gpu:0000:01:00.0".
    Key(Arc<str>),
    /// The nth device the box shows (0 or 1 for the GPU box), in sidebar order. Flow layouts
    /// only, so a default or converted layout follows the GPUs present.
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

    /// The series this key names; `None` for a key this build does not know.
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
    /// The forms a widget shows. Never empty in a loaded widget. Written as names in
    /// [`FormSet::NAMES`] order.
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

/// The column count of `container`'s inner grid. Phase 1 supports only the GPU box.
pub fn grid_cols(_container: &str) -> u16 {
    GPU_BOX_GRID_COLS
}

/// The Overview box kind of a container id such as "overview.gpu".
fn overview_kind(container: &str) -> Option<&str> {
    container.strip_prefix(OVERVIEW_PREFIX)
}

/// The preset ids of `container`, the default first.
fn presets_of(container: &str) -> &'static [&'static str] {
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

    /// Replaces the entry of `container` with `layout`, or adds it at the end.
    pub fn set(&mut self, layout: ContainerLayout) {
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
    /// The entry as written: `container`, `version`, `preset` (flow layouts only), `widgets`
    /// and `next_id`.
    pub fn to_table(&self) -> Table {
        let mut t = Table::new();
        t.insert("container".into(), Value::String(self.container.clone()));
        t.insert("version".into(), Value::Integer(i64::from(self.version)));
        if let Some(p) = &self.preset {
            let mut pt = Table::new();
            pt.insert("id".into(), Value::String(p.id.clone()));
            if let Some(a) = &p.arrange {
                pt.insert("arrange".into(), Value::String(a.clone()));
            }
            if let Some(s) = p.split {
                pt.insert("split".into(), Value::Float(round_split(s)));
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
        let preset = match t.get("preset") {
            None => None,
            Some(v) => Some(PresetSetting::from_value(container, v, &mut changed)),
        };
        let flow = preset.is_some();
        let cols = grid_cols(container);
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
                .and_then(|w| WidgetSetting::from_table(w, flow, cols, &mut changed));
            let kept = read.filter(|w| {
                !widgets.iter().any(|k| {
                    k.id == w.id || matches!((k.cell, w.cell), (Some(a), Some(b)) if a.overlaps(&b))
                })
            });
            match kept {
                Some(w) => widgets.push(w),
                None => changed = true,
            }
        }
        let floor = widgets.iter().map(|w| w.id + 1).max().unwrap_or(1);
        let next_id = as_int::<u32>(t.get("next_id")).unwrap_or(0).max(floor);
        let layout = Self {
            container: container.to_owned(),
            version: LAYOUT_VERSION,
            preset,
            widgets,
            next_id,
        };
        (layout, changed)
    }
}

impl PresetSetting {
    /// An unknown or missing id reads as the kind's default preset; an invalid `arrange` or
    /// `split` reads as `None` (UI-PRF-019).
    fn from_value(container: &str, v: &Value, changed: &mut bool) -> Self {
        let presets = presets_of(container);
        let default = presets.first().copied().unwrap_or_default();
        let Some(t) = v.as_table() else {
            *changed = true;
            return Self {
                id: default.to_owned(),
                arrange: None,
                split: None,
            };
        };
        let id = match t.get("id").and_then(Value::as_str) {
            Some(id) if presets.contains(&id) => id,
            _ => {
                *changed = true;
                default
            }
        };
        let arrange = t.get("arrange").and_then(|a| {
            let a = a.as_str().filter(|a| OVERVIEW_ARRANGES.contains(a));
            *changed |= a.is_none();
            a.map(str::to_owned)
        });
        let (lo, hi) = OVERVIEW_SPLIT_RANGE;
        let split = t.get("split").and_then(|s| {
            let s = as_number(s).filter(|s| (lo..=hi).contains(s));
            *changed |= s.is_none();
            s
        });
        Self {
            id: id.to_owned(),
            arrange,
            split,
        }
    }
}

impl WidgetSetting {
    fn to_table(&self) -> Table {
        let mut t = Table::new();
        t.insert("id".into(), Value::Integer(i64::from(self.id)));
        match &self.source.device {
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
        t.insert(
            "metric".into(),
            Value::String(self.source.metric.as_str().to_owned()),
        );
        let forms = self
            .forms
            .names()
            .map(|n| Value::String(n.to_owned()))
            .collect();
        t.insert("forms".into(), Value::Array(forms));
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
        let defaults = WidgetOptions::default();
        if self.options.fill != defaults.fill {
            t.insert("fill".into(), Value::Boolean(self.options.fill));
        }
        if self.options.dashed != defaults.dashed {
            t.insert("dashed".into(), Value::Boolean(self.options.dashed));
        }
        t
    }

    /// One widget by the ADR-019 load rules; `None` drops it. A repeated id and an overlap are
    /// checked by the caller. Sets `changed` when it ignored a part it could not read.
    fn from_table(t: &Table, flow: bool, cols: u16, changed: &mut bool) -> Option<Self> {
        let id = as_int::<u32>(t.get("id")).filter(|&id| id < u32::MAX)?;
        let names = t.get("forms")?.as_array()?;
        let forms = FormSet::from_names(names.iter().filter_map(Value::as_str));
        if forms.is_empty() {
            return None;
        }
        let device = match (t.get("device"), t.get("slot")) {
            (Some(d), None) => DeviceSelector::Key(Arc::from(d.as_str()?)),
            (None, Some(s)) if flow => {
                DeviceSelector::Slot(as_int::<u8>(Some(s)).filter(|&s| s <= SLOT_MAX)?)
            }
            _ => return None,
        };
        let metric = MetricKey::new(t.get("metric")?.as_str()?);
        let cell_keys = ["col", "row", "cols", "rows"];
        let cell = if flow {
            *changed |= cell_keys.iter().any(|k| t.contains_key(*k));
            None
        } else {
            let [col, row, w, h] = cell_keys.map(|k| as_int::<u16>(t.get(k)));
            let cell = WidgetCell {
                col: col?,
                row: row?,
                cols: w?,
                rows: h?,
            };
            let fits = cell.cols >= 1
                && cell.rows >= 1
                && u32::from(cell.col) + u32::from(cell.cols) <= u32::from(cols)
                && u32::from(cell.row) + u32::from(cell.rows) <= u32::from(u16::MAX);
            if !fits {
                return None;
            }
            Some(cell)
        };
        let defaults = WidgetOptions::default();
        let mut flag = |key: &str, default: bool| match t.get(key) {
            None => default,
            Some(v) => v.as_bool().unwrap_or_else(|| {
                *changed = true;
                default
            }),
        };
        let options = WidgetOptions {
            fill: flag("fill", defaults.fill),
            dashed: flag("dashed", defaults.dashed),
        };
        let device_name = match t.get("device_name") {
            None => None,
            Some(v) => {
                *changed |= v.as_str().is_none();
                v.as_str().map(Arc::from)
            }
        };
        Some(Self {
            id,
            source: SourceSelector { device, metric },
            forms,
            cell,
            options,
            device_name,
        })
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
    let presets = presets_of(GPU_BOX_CONTAINER);
    let id = entry
        .mode
        .as_deref()
        .filter(|m| presets.contains(m))
        .unwrap_or(presets[0]);
    let arrange = entry
        .arrange
        .clone()
        .filter(|a| OVERVIEW_ARRANGES.contains(&a.as_str()));
    let (lo, hi) = OVERVIEW_SPLIT_RANGE;
    let split = entry.split.filter(|s| (lo..=hi).contains(s));
    let hidden = hidden_metrics(&entry.hidden);
    let chart_value_label = FormSet::CHART | FormSet::VALUE | FormSet::LABEL;
    let row = FormSet::CHART | FormSet::BAR | FormSet::VALUE | FormSet::LABEL;
    let bar_value_label = FormSet::BAR | FormSet::VALUE | FormSet::LABEL;
    // (metric, forms, whether `hidden` can leave it out)
    let mut list: Vec<(MetricKey, FormSet, bool)> = Vec::new();
    match id {
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
    let mut widgets = Vec::with_capacity(list.len() * 2);
    let mut next_id = 1;
    for slot in 0..=SLOT_MAX {
        for (metric, forms, _) in &list {
            widgets.push(WidgetSetting {
                id: next_id,
                source: SourceSelector {
                    device: DeviceSelector::Slot(slot),
                    metric: metric.clone(),
                },
                forms: *forms,
                cell: None,
                options: WidgetOptions::default(),
                device_name: None,
            });
            next_id += 1;
        }
    }
    ContainerLayout {
        container: GPU_BOX_CONTAINER.to_owned(),
        version: LAYOUT_VERSION,
        preset: Some(PresetSetting {
            id: id.to_owned(),
            arrange,
            split,
        }),
        widgets,
        next_id,
    }
}

/// The GPU box's grid entry in `[performance.overview]`, when the layout is saved.
fn gpu_grid_entry(perf: &PerformanceSettings) -> Option<&OverviewGridSetting> {
    perf.overview
        .as_ref()
        .and_then(|o| o.grid.iter().find(|e| e.kind == "gpu"))
}

/// Whether the GPU box's grid entry has a key that conversion reads.
fn has_old_gpu_keys(entry: &OverviewGridSetting) -> bool {
    entry.mode.is_some()
        || entry.split.is_some()
        || entry.arrange.is_some()
        || !entry.hidden.is_empty()
}

/// The conversion step of `Settings::clamp`: with no `overview.gpu` entry and an old GPU box
/// key, adds the converted flow layout. Returns `true` when it converted. Once the entry exists
/// it does nothing, so it runs once. The old keys stay as loaded.
pub fn convert_old_settings(perf: &mut PerformanceSettings) -> bool {
    if perf.layouts.entry(GPU_BOX_CONTAINER).is_some() {
        return false;
    }
    let Some(entry) = gpu_grid_entry(perf).filter(|e| has_old_gpu_keys(e)) else {
        return false;
    };
    let layout = convert_gpu_box(entry);
    perf.layouts.0.push(LayoutEntry::Known(layout));
    true
}

/// The layout the GPU box shows: its known entry, else its old keys converted in memory (a
/// kept entry from a newer build leaves the box on its old keys), else the built-in default.
pub fn gpu_box_layout(perf: &PerformanceSettings) -> ContainerLayout {
    if let Some(layout) = perf.layouts.known(GPU_BOX_CONTAINER) {
        return layout.clone();
    }
    match gpu_grid_entry(perf) {
        Some(entry) => convert_gpu_box(entry),
        None => convert_gpu_box(&OverviewGridSetting::default()),
    }
}

/// Saves the default for `container` (ADR-019 "Saving the default"): removes its entry and,
/// for the GPU box, the old keys conversion reads from the box's grid entry (`mode`, `split`,
/// `arrange` and `hidden`). The box's other grid keys stay. The next load then shows the
/// built-in default and converts nothing.
pub fn reset_to_default(perf: &mut PerformanceSettings, container: &str) {
    perf.layouts.remove(container);
    let Some(kind) = overview_kind(container) else {
        return;
    };
    let Some(overview) = perf.overview.as_mut() else {
        return;
    };
    for entry in overview.grid.iter_mut().filter(|e| e.kind == kind) {
        if kind == "gpu" {
            entry.mode = None;
            entry.split = None;
            entry.arrange = None;
            entry.hidden.clear();
        }
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
            .filter(|w| w.source.device == DeviceSelector::Slot(slot))
            .map(|w| {
                (
                    w.source.metric.as_str().to_owned(),
                    w.forms.names().collect(),
                )
            })
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
        let entry = gpu_grid_entry(&s.performance).expect("gpu entry");
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
        let l = gpu_box_layout(&s.performance);
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
            let entry = gpu_grid_entry(&first.performance).expect("gpu entry");
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
            w.source.device,
            DeviceSelector::Key(Arc::from("gpu:GPU-0a1b"))
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
        assert_eq!(l.widgets[0].source.metric.metric(), None);
        assert_eq!(l.widgets[1].source.metric.as_str(), "fan_speed");
        let (back, _) = reload(&s);
        assert_eq!(back, s);
    }

    #[test]
    fn newer_version_and_unknown_container_kept() {
        let text = "[[performance.layouts]]\ncontainer = \"overview.gpu\"\nversion = 2\nshape = \"hex\"\nwidgets = [{ id = 1, wobble = true }]\n\n[[performance.layouts]]\ncontainer = \"page.cpu\"\nversion = 1\nwidgets = [{ id = 9, slot = 4 }]\n";
        let (s, changed) = load(&format!("{}{text}", gpu_entry("mode = \"chart\"")));
        assert!(changed.is_empty(), "{changed:?}");
        let entries = &s.performance.layouts.0;
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| matches!(e, LayoutEntry::Kept(_))));
        // A kept overview.gpu entry blocks conversion; the box shows its old keys.
        let l = gpu_box_layout(&s.performance);
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
                split: Some(0.4)
            }
        );
        let (p, changed) = flow("{ id = \"bogus\", arrange = \"diagonal\", split = 0.9 }");
        assert_eq!(changed, vec![LAYOUTS_KEY]);
        assert_eq!(
            p,
            PresetSetting {
                id: "engine_charts".into(),
                arrange: None,
                split: None
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
        let entry = gpu_grid_entry(&back.performance).expect("gpu entry");
        assert!(!has_old_gpu_keys(entry));
        assert_eq!((entry.col, entry.row, entry.cols, entry.rows), (6, 0, 6, 4));
        assert_eq!(
            gpu_box_layout(&back.performance),
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
}
