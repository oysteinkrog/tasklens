//! The device page containers (ADR-019 "Phase 3", D-041): their ids, the metric keys phase 3
//! adds, the page presets and the conversion of the 1.3.0 page settings. No file I/O.
//!
//! A page container's id is `page.` and the page's persistent device key (PERF-005), kept
//! whole: `page.cpu`, `page.gpu:GPU-…`, `page.disk:…`. Each GPU, disk, adapter, fan and battery
//! has its own entry, and the entry of a device that is gone is kept.
//!
//! [`PAGE_KINDS`] lists the device kinds whose pages this build shows as widget containers. A
//! page of another kind is not a layout container here: its entry stays `LayoutEntry::Kept`
//! and is written back unchanged. The `*_for` functions take the kind list as an argument, so
//! the tests can run them for every page kind before the page beads add theirs.
//!
//! Conversion is deterministic and idempotent: it reads the old page keys and the container
//! id, never a device, and it runs only for a page with no entry.

use crate::ids::MetricId;
use crate::layout::{
    ContainerLayout, DeviceSelector, FixedWidget, FormSet, LayoutEntry, METRIC_POWER, METRIC_TEMP,
    MetricKey, PresetSetting, SourceSelector, WidgetContent,
};
use crate::layout_boxes::{
    DEVICE_CPU, DEVICE_MEMORY, DeviceKind, METRIC_CORE_CLOCKS, METRIC_CPU_TOTAL,
    METRIC_DISK_ACTIVE, METRIC_DISK_IO, METRIC_MEM_SWAP, METRIC_NET_TRAFFIC, METRIC_POWER_DRAW,
    MetricSeries,
};
use crate::settings::{
    CpuGraphMode, CpuSecondaryGraph, GpuOptionalGraph, MemoryView, PerformanceSettings,
};

/// The prefix of a page container id; the rest is the page's persistent device key.
pub const PAGE_CONTAINER_PREFIX: &str = "page.";

/// The column count of every page container's inner grid.
pub const PAGE_GRID_COLS: u16 = 12;

/// The device kinds whose pages this build shows as widget containers. Each page bead adds its
/// kind (`tasklens-hzire.3` to `.9`). `tasklens-hzire.3` adds Aggregate storage.
pub const PAGE_KINDS: &[DeviceKind] = &[DeviceKind::Disks];

/// Every device kind that has a page: CPU, Memory, GPU, disk, Aggregate storage, adapter,
/// fan, battery and Aggregate graphics.
pub const PAGE_DEVICE_KINDS: [DeviceKind; 9] = [
    DeviceKind::Cpu,
    DeviceKind::Memory,
    DeviceKind::Gpu,
    DeviceKind::Disk,
    DeviceKind::Disks,
    DeviceKind::Net,
    DeviceKind::Fan,
    DeviceKind::Battery,
    DeviceKind::AggregateGpu,
];

/// `cpu`: `CPU_TOTAL` of every present logical CPU, one line each (UI-PRF-023).
pub const METRIC_CORES_OVERLAID: &str = "cores_overlaid";
/// `cpu`: the same as stacked bands.
pub const METRIC_CORES_STACKED: &str = "cores_stacked";
/// `cpu`: the Clock speed chart, speed filled and the fastest core dashed (UI-PRF-025).
pub const METRIC_CLOCK: &str = "clock";
/// `cpu`: the fastest core's clock.
pub const METRIC_FREQ_MAX: &str = "freq_max";
/// `memory`: in use filled, committed dashed, modified as a thin line (UI-PRF-026).
pub const METRIC_PHYSICAL: &str = "physical";
/// `memory`: committed memory as a percent of the commit limit.
pub const METRIC_COMMIT_PCT: &str = "commit_pct";
/// `memory`: modified (dirty) memory.
pub const METRIC_DIRTY: &str = "dirty";
/// `gpu:…`: dedicated memory filled and shared memory dashed. `aggregate:gpu`: the summed
/// memory, one line.
pub const METRIC_GPU_MEMORY: &str = "memory";
/// `gpu:…`: shared memory.
pub const METRIC_VRAM_SHARED: &str = "vram_shared";
/// `gpu:…`: graphics clock filled and memory clock dashed (UI-PRF-032).
pub const METRIC_GPU_CLOCKS: &str = "clocks";
/// `gpu:…`: fan speed, from the GPU record.
pub const METRIC_FAN: &str = "fan";
/// `disk:…`: average response time, from the disk record.
pub const METRIC_RESPONSE: &str = "response";
/// `disk:…`: running totals, from the disk record.
pub const METRIC_TOTAL_READ: &str = "total_read";
pub const METRIC_TOTAL_WRITTEN: &str = "total_written";
/// `net:…`: running totals, from the adapter record.
pub const METRIC_TOTAL_RX: &str = "total_rx";
pub const METRIC_TOTAL_TX: &str = "total_tx";
/// `net:…`: the Wi-Fi signal.
pub const METRIC_SIGNAL: &str = "signal";
/// `fan:…`: RPM filled and PWM dashed (UI-PRF-034).
pub const METRIC_SPEED: &str = "speed";
/// `fan:…`: RPM.
pub const METRIC_RPM: &str = "rpm";
/// `fan:…`: PWM.
pub const METRIC_PWM: &str = "pwm";
/// `bat:…`: charge percentage. `power` is [`METRIC_POWER`].
pub const METRIC_PERCENT: &str = "percent";
/// `aggregate:gpu`: utilization of the busiest adapter.
pub const METRIC_UTIL: &str = "util";

/// Every metric key phase 3 adds, with the device kind it is valid on, in the order of the
/// ADR-019 phase 3 key table.
pub const PAGE_METRICS: [(DeviceKind, &str); 25] = [
    (DeviceKind::Cpu, METRIC_CORES_OVERLAID),
    (DeviceKind::Cpu, METRIC_CORES_STACKED),
    (DeviceKind::Cpu, METRIC_CLOCK),
    (DeviceKind::Cpu, METRIC_FREQ_MAX),
    (DeviceKind::Memory, METRIC_PHYSICAL),
    (DeviceKind::Memory, METRIC_COMMIT_PCT),
    (DeviceKind::Memory, METRIC_DIRTY),
    (DeviceKind::Gpu, METRIC_GPU_MEMORY),
    (DeviceKind::Gpu, METRIC_VRAM_SHARED),
    (DeviceKind::Gpu, METRIC_GPU_CLOCKS),
    (DeviceKind::Gpu, METRIC_FAN),
    (DeviceKind::Disk, METRIC_RESPONSE),
    (DeviceKind::Disk, METRIC_TOTAL_READ),
    (DeviceKind::Disk, METRIC_TOTAL_WRITTEN),
    (DeviceKind::Net, METRIC_TOTAL_RX),
    (DeviceKind::Net, METRIC_TOTAL_TX),
    (DeviceKind::Net, METRIC_SIGNAL),
    (DeviceKind::Fan, METRIC_SPEED),
    (DeviceKind::Fan, METRIC_RPM),
    (DeviceKind::Fan, METRIC_PWM),
    (DeviceKind::Fan, METRIC_TEMP),
    (DeviceKind::Battery, METRIC_POWER),
    (DeviceKind::Battery, METRIC_PERCENT),
    (DeviceKind::AggregateGpu, METRIC_UTIL),
    (DeviceKind::AggregateGpu, METRIC_GPU_MEMORY),
];

/// The series of a phase 3 key on a device of `kind`; `None` for a key phase 3 does not add
/// there. [`crate::layout_boxes::metric_series`] asks this after the phase 1 and phase 2 keys.
pub fn page_metric_series(kind: DeviceKind, key: &str) -> Option<MetricSeries> {
    use MetricSeries::{NoSeries, One, Pair, Paired, PerLogical, Triple};
    let series = match (kind, key) {
        (DeviceKind::Cpu, METRIC_CORES_OVERLAID | METRIC_CORES_STACKED) => {
            PerLogical(MetricId::CPU_TOTAL)
        }
        (DeviceKind::Cpu, METRIC_CLOCK) => Pair([MetricId::CPU_FREQ, MetricId::CPU_CLOCK_MAX]),
        (DeviceKind::Cpu, METRIC_FREQ_MAX) => One(MetricId::CPU_CLOCK_MAX),
        (DeviceKind::Memory, METRIC_PHYSICAL) => Triple([
            MetricId::MEM_USED,
            MetricId::MEM_COMMIT,
            MetricId::MEM_DIRTY,
        ]),
        (DeviceKind::Memory, METRIC_COMMIT_PCT) => One(MetricId::MEM_COMMIT),
        (DeviceKind::Memory, METRIC_DIRTY) => One(MetricId::MEM_DIRTY),
        (DeviceKind::Gpu, METRIC_GPU_MEMORY) => Pair([MetricId::GPU_MEM, MetricId::GPU_MEM_SHARED]),
        (DeviceKind::Gpu, METRIC_VRAM_SHARED) => One(MetricId::GPU_MEM_SHARED),
        (DeviceKind::Gpu, METRIC_GPU_CLOCKS) => {
            Pair([MetricId::GPU_CLOCK, MetricId::GPU_MEM_CLOCK])
        }
        (DeviceKind::Gpu, METRIC_FAN) => NoSeries,
        (DeviceKind::Disk, METRIC_RESPONSE | METRIC_TOTAL_READ | METRIC_TOTAL_WRITTEN) => NoSeries,
        (DeviceKind::Net, METRIC_TOTAL_RX | METRIC_TOTAL_TX | METRIC_SIGNAL) => NoSeries,
        (DeviceKind::Fan, METRIC_SPEED) => Pair([MetricId::FAN_RPM, MetricId::FAN_PWM]),
        (DeviceKind::Fan, METRIC_RPM) => One(MetricId::FAN_RPM),
        (DeviceKind::Fan, METRIC_PWM) => One(MetricId::FAN_PWM),
        (DeviceKind::Fan, METRIC_TEMP) => Paired(MetricId::SENSOR_TEMP),
        (DeviceKind::Battery, METRIC_POWER) => One(MetricId::BATTERY_POWER),
        (DeviceKind::Battery, METRIC_PERCENT) => One(MetricId::BATTERY_PCT),
        (DeviceKind::AggregateGpu, METRIC_UTIL) => One(MetricId::GPU_UTIL),
        (DeviceKind::AggregateGpu, METRIC_GPU_MEMORY) => One(MetricId::GPU_MEM),
        _ => return None,
    };
    Some(series)
}

/// The container id of the page of the device with persistent key `key`: `page.<key>`.
pub fn page_container(key: &str) -> String {
    format!("{PAGE_CONTAINER_PREFIX}{key}")
}

/// The persistent device key of a page container id; `None` for any other id.
pub fn page_key(container: &str) -> Option<&str> {
    container.strip_prefix(PAGE_CONTAINER_PREFIX)
}

/// The device kind of a page container; `None` for any other id and for a key whose kind has
/// no page (a logical CPU, the network aggregate, an unknown key).
pub fn page_kind(container: &str) -> Option<DeviceKind> {
    let kind = DeviceKind::of_key(page_key(container)?);
    PAGE_DEVICE_KINDS.contains(&kind).then_some(kind)
}

/// Whether `container` is a page container whose kind is in `kinds`.
pub fn is_page_container_for(container: &str, kinds: &[DeviceKind]) -> bool {
    page_kind(container).is_some_and(|k| kinds.contains(&k))
}

/// The preset ids of the pages of `kind`, the default first (ADR-019 phase 3 "Presets of the
/// pages"). The CPU ids are the `cpu_graph_mode` values, the Memory ids the `memory_view`
/// values. Empty for a kind with no page.
pub fn page_presets(kind: DeviceKind) -> &'static [&'static str] {
    match kind {
        DeviceKind::Cpu => &[
            PRESET_PER_CORE,
            PRESET_OVERALL,
            PRESET_ALL_CORES_OVERLAID,
            PRESET_ALL_CORES_STACKED,
            PRESET_BOARD,
        ],
        DeviceKind::Memory => &[PRESET_USAGE, PRESET_APPS],
        k if PAGE_DEVICE_KINDS.contains(&k) => &[PRESET_DEFAULT],
        _ => &[],
    }
}

/// The CPU page presets, named as the `cpu_graph_mode` values.
pub const PRESET_PER_CORE: &str = "per_core";
pub const PRESET_OVERALL: &str = "overall";
pub const PRESET_ALL_CORES_OVERLAID: &str = "all_cores_overlaid";
pub const PRESET_ALL_CORES_STACKED: &str = "all_cores_stacked";
pub const PRESET_BOARD: &str = "board";
/// The Memory page presets, named as the `memory_view` values.
pub const PRESET_USAGE: &str = "usage";
pub const PRESET_APPS: &str = "apps";
/// The one preset of every other page.
pub const PRESET_DEFAULT: &str = "default";

/// The preset id of `mode`.
pub fn cpu_preset(mode: CpuGraphMode) -> &'static str {
    match mode {
        CpuGraphMode::Overall => PRESET_OVERALL,
        CpuGraphMode::PerCore => PRESET_PER_CORE,
        CpuGraphMode::AllCoresOverlaid => PRESET_ALL_CORES_OVERLAID,
        CpuGraphMode::AllCoresStacked => PRESET_ALL_CORES_STACKED,
        CpuGraphMode::Board => PRESET_BOARD,
    }
}

/// The preset id of `view`.
pub fn memory_preset(view: MemoryView) -> &'static str {
    match view {
        MemoryView::Usage => PRESET_USAGE,
        MemoryView::Apps => PRESET_APPS,
    }
}

/// Chart, value and name: every page chart of a preset.
const CHART_VALUE_LABEL: FormSet = FormSet::CHART.union(FormSet::VALUE).union(FormSet::LABEL);
/// Chart and name: the charts that have no value (all cores, core clocks).
const CHART_LABEL: FormSet = FormSet::CHART.union(FormSet::LABEL);

/// A metric widget of the page's own device.
fn metric(device: &str, key: &str, forms: FormSet) -> (WidgetContent, FormSet) {
    let source = SourceSelector {
        device: DeviceSelector::Key(device.into()),
        metric: MetricKey::new(key),
    };
    (WidgetContent::Metric(source), forms)
}

/// A fixed widget, which has no forms.
fn fixed(kind: FixedWidget) -> (WidgetContent, FormSet) {
    (WidgetContent::Fixed(kind), FormSet::empty())
}

/// The widgets of the CPU page preset `preset` (an unknown id gives the default's), without
/// the secondary graph's widgets and Details.
pub fn cpu_mode_widgets(preset: &str) -> Vec<(WidgetContent, FormSet)> {
    let total = || metric(DEVICE_CPU, METRIC_CPU_TOTAL, CHART_VALUE_LABEL);
    match preset {
        PRESET_OVERALL => vec![total()],
        PRESET_ALL_CORES_OVERLAID => vec![metric(DEVICE_CPU, METRIC_CORES_OVERLAID, CHART_LABEL)],
        PRESET_ALL_CORES_STACKED => vec![metric(DEVICE_CPU, METRIC_CORES_STACKED, CHART_LABEL)],
        PRESET_BOARD => vec![total(), fixed(FixedWidget::CpuCoreBoard)],
        _ => vec![fixed(FixedWidget::CpuCoreGrid)],
    }
}

/// The widgets of the CPU page's secondary graph `graph`, in list order. `power` lists the
/// power draw chart and then its two fallbacks; the page shows only the first the device has.
pub fn cpu_secondary_widgets(graph: CpuSecondaryGraph) -> Vec<(WidgetContent, FormSet)> {
    let at = |key| metric(DEVICE_CPU, key, CHART_VALUE_LABEL);
    match graph {
        CpuSecondaryGraph::Power => vec![at(METRIC_POWER_DRAW), at(METRIC_TEMP), at(METRIC_CLOCK)],
        CpuSecondaryGraph::Clock => vec![at(METRIC_CLOCK)],
        CpuSecondaryGraph::Clocks => vec![metric(DEVICE_CPU, METRIC_CORE_CLOCKS, CHART_LABEL)],
        CpuSecondaryGraph::Temperature => vec![at(METRIC_TEMP)],
        CpuSecondaryGraph::None => Vec::new(),
    }
}

/// Converts the CPU page's old keys (`cpu_graph_mode`, `cpu_secondary_graph`) to a flow
/// layout: the mode's widgets, the secondary graph's widgets, then Details.
pub fn convert_cpu_page(perf: &PerformanceSettings, container: &str) -> ContainerLayout {
    let preset = cpu_preset(perf.cpu_graph_mode);
    let mut widgets = cpu_mode_widgets(preset);
    widgets.extend(cpu_secondary_widgets(perf.cpu_secondary_graph));
    widgets.push(fixed(FixedWidget::Details));
    flow(container, preset, widgets)
}

/// Converts the Memory page's old keys (`memory_view`, `memory_swap_graph`,
/// `memory_composition`) to a flow layout: Physical memory (or Memory by app), Commit, Swap
/// when on, the composition bar when on, Details, then the three blocks.
pub fn convert_memory_page(perf: &PerformanceSettings, container: &str) -> ContainerLayout {
    let preset = memory_preset(perf.memory_view);
    let mut widgets = vec![match perf.memory_view {
        MemoryView::Usage => metric(DEVICE_MEMORY, METRIC_PHYSICAL, CHART_VALUE_LABEL),
        MemoryView::Apps => fixed(FixedWidget::MemoryByApp),
    }];
    widgets.push(metric(DEVICE_MEMORY, METRIC_COMMIT_PCT, CHART_VALUE_LABEL));
    if perf.memory_swap_graph {
        widgets.push(metric(DEVICE_MEMORY, METRIC_MEM_SWAP, CHART_VALUE_LABEL));
    }
    if perf.memory_composition {
        widgets.push(fixed(FixedWidget::MemoryBar));
    }
    widgets.extend([
        fixed(FixedWidget::Details),
        fixed(FixedWidget::MemoryCompressed),
        fixed(FixedWidget::SwapDevices),
        fixed(FixedWidget::MemoryModules),
    ]);
    flow(container, preset, widgets)
}

/// Converts the GPU page's old key (`gpu_optional_graphs`) to a flow layout for the GPU of
/// `container`: the engine grid, Memory and Temperature, Power and Clocks when listed,
/// Details, then Displays. Every widget names the page's own GPU key.
pub fn convert_gpu_page(perf: &PerformanceSettings, container: &str) -> ContainerLayout {
    let key = page_key(container).unwrap_or_default();
    let at = |k| metric(key, k, CHART_VALUE_LABEL);
    let mut widgets = vec![
        fixed(FixedWidget::GpuEngineGrid),
        at(METRIC_GPU_MEMORY),
        at(METRIC_TEMP),
    ];
    let optional = &perf.gpu_optional_graphs;
    if optional.contains(&GpuOptionalGraph::Power) {
        widgets.push(at(METRIC_POWER));
    }
    if optional.contains(&GpuOptionalGraph::Clocks) {
        widgets.push(at(METRIC_GPU_CLOCKS));
    }
    widgets.push(fixed(FixedWidget::Details));
    widgets.push(fixed(FixedWidget::GpuDisplays));
    flow(container, PRESET_DEFAULT, widgets)
}

/// The default flow layout of a page with no old keys: a disk, an adapter, a fan, a battery,
/// Aggregate storage and Aggregate graphics. `None` for the other kinds.
fn plain_page(kind: DeviceKind, container: &str) -> Option<ContainerLayout> {
    let key = page_key(container).unwrap_or_default();
    let at = |k| metric(key, k, CHART_VALUE_LABEL);
    let details = || fixed(FixedWidget::Details);
    let widgets = match kind {
        DeviceKind::Disk => vec![
            at(METRIC_DISK_ACTIVE),
            at(METRIC_DISK_IO),
            details(),
            fixed(FixedWidget::DiskPartitions),
        ],
        DeviceKind::Net => vec![
            at(METRIC_NET_TRAFFIC),
            details(),
            fixed(FixedWidget::NetSettings),
        ],
        DeviceKind::Fan => vec![at(METRIC_SPEED), at(METRIC_TEMP), details()],
        DeviceKind::Battery => vec![
            at(METRIC_POWER),
            at(METRIC_PERCENT),
            fixed(FixedWidget::BatteryHistory),
            details(),
        ],
        DeviceKind::Disks => vec![at(METRIC_DISK_ACTIVE), at(METRIC_DISK_IO), details()],
        DeviceKind::AggregateGpu => vec![at(METRIC_UTIL), at(METRIC_GPU_MEMORY), details()],
        _ => return None,
    };
    Some(flow(container, PRESET_DEFAULT, widgets))
}

/// A flow layout of `container` with preset `id` and `widgets`, ids from 1.
fn flow(container: &str, id: &str, widgets: Vec<(WidgetContent, FormSet)>) -> ContainerLayout {
    let preset = PresetSetting {
        id: id.to_owned(),
        ..PresetSetting::default()
    };
    ContainerLayout::flow(container, preset, widgets)
}

/// The page keys of `container` converted to a flow layout: the CPU, Memory and GPU pages read
/// their old keys; every other page gives its default. `None` for an id that is not a page
/// container of a kind with a page.
pub fn convert_page(perf: &PerformanceSettings, container: &str) -> Option<ContainerLayout> {
    match page_kind(container)? {
        DeviceKind::Cpu => Some(convert_cpu_page(perf, container)),
        DeviceKind::Memory => Some(convert_memory_page(perf, container)),
        DeviceKind::Gpu => Some(convert_gpu_page(perf, container)),
        kind => plain_page(kind, container),
    }
}

/// The built-in default of a page: its conversion with every old key at its default. Used in
/// memory and never written, except as the explicit default entry a GPU page reset writes.
pub fn default_page_layout(container: &str) -> Option<ContainerLayout> {
    convert_page(&PerformanceSettings::default(), container)
}

/// Whether an old key the page of `kind` converts from differs from its default. Only the
/// CPU, Memory and GPU pages have old keys.
pub fn page_has_old_keys(perf: &PerformanceSettings, kind: DeviceKind) -> bool {
    let d = PerformanceSettings::default();
    match kind {
        DeviceKind::Cpu => {
            perf.cpu_graph_mode != d.cpu_graph_mode
                || perf.cpu_secondary_graph != d.cpu_secondary_graph
        }
        DeviceKind::Memory => {
            perf.memory_view != d.memory_view
                || perf.memory_swap_graph != d.memory_swap_graph
                || perf.memory_composition != d.memory_composition
        }
        DeviceKind::Gpu => !perf.gpu_optional_graphs.is_empty(),
        _ => false,
    }
}

/// Whether the page `container` must convert now, for the page kinds in `kinds`: its kind is
/// in `kinds`, it has no entry (known or kept) and one of its old keys is off its default.
pub fn page_needs_conversion_for(
    perf: &PerformanceSettings,
    container: &str,
    kinds: &[DeviceKind],
) -> bool {
    let Some(kind) = page_kind(container).filter(|k| kinds.contains(k)) else {
        return false;
    };
    perf.layouts.entry(container).is_none() && page_has_old_keys(perf, kind)
}

/// [`page_needs_conversion_for`] for the kinds of this build. A GPU page asks it the first
/// time the window builds the page.
pub fn page_needs_conversion(perf: &PerformanceSettings, container: &str) -> bool {
    page_needs_conversion_for(perf, container, PAGE_KINDS)
}

/// The page conversion step of `Settings::clamp` for the page kinds in `kinds`: `page.cpu`
/// and `page.memory` convert when they need to ([`page_needs_conversion_for`]). A GPU page's
/// key is not known here, so GPU pages convert when the window first builds them. Returns
/// `true` when it converted any. The old keys stay as loaded.
pub fn convert_old_page_settings_for(perf: &mut PerformanceSettings, kinds: &[DeviceKind]) -> bool {
    let mut converted = false;
    for key in [DEVICE_CPU, DEVICE_MEMORY] {
        let container = page_container(key);
        if !page_needs_conversion_for(perf, &container, kinds) {
            continue;
        }
        if let Some(layout) = convert_page(perf, &container) {
            perf.layouts.0.push(LayoutEntry::Known(layout));
            converted = true;
        }
    }
    converted
}

/// [`convert_old_page_settings_for`] for the kinds of this build.
pub fn convert_old_page_settings(perf: &mut PerformanceSettings) -> bool {
    convert_old_page_settings_for(perf, PAGE_KINDS)
}

/// Saves the default of the page `container` (ADR-019 phase 3 "Saving the default"). The
/// CPU and Memory pages remove their entry and set their old keys back to their defaults, so
/// they do not convert again. A GPU page writes its built-in default as an explicit entry and
/// leaves `gpu_optional_graphs` as it is, because every GPU page reads that key. Every other
/// page removes its entry.
pub fn reset_page_to_default(perf: &mut PerformanceSettings, container: &str) {
    let d = PerformanceSettings::default();
    match page_kind(container) {
        Some(DeviceKind::Gpu) => {
            if let Some(layout) = default_page_layout(container) {
                perf.layouts.set(layout);
            }
            return;
        }
        Some(DeviceKind::Cpu) => {
            perf.cpu_graph_mode = d.cpu_graph_mode;
            perf.cpu_secondary_graph = d.cpu_secondary_graph;
        }
        Some(DeviceKind::Memory) => {
            perf.memory_view = d.memory_view;
            perf.memory_swap_graph = d.memory_swap_graph;
            perf.memory_composition = d.memory_composition;
        }
        _ => {}
    }
    perf.layouts.remove(container);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{GroupKind, LayoutsSetting, container_layout, reset_to_default};
    use crate::layout_boxes::metric_series;
    use crate::settings::{LAYOUTS_CONVERTED_KEY, Settings};

    const GPU: &str = "page.gpu:GPU-0a1b2c3d";
    const GPU2: &str = "page.gpu:pci:0000:03:00.0";

    /// Loads `text` as `settings.toml`, then reads the layouts and converts the pages as a
    /// build with every page kind would.
    fn load_all(text: &str) -> (Settings, bool) {
        let mut s: Settings = toml::from_str(text).expect("valid TOML");
        let changed = s.clamp();
        let mut converted = changed.contains(&LAYOUTS_CONVERTED_KEY);
        s.performance.layouts.load_for(&PAGE_DEVICE_KINDS);
        converted |= convert_old_page_settings_for(&mut s.performance, &PAGE_DEVICE_KINDS);
        (s, converted)
    }

    fn save(s: &Settings) -> String {
        toml::to_string(s).expect("serializes")
    }

    /// One widget as "<device> <metric> [forms]" or "fixed <name>".
    fn describe(l: &ContainerLayout) -> Vec<String> {
        l.widgets
            .iter()
            .map(|w| match &w.content {
                WidgetContent::Metric(s) => {
                    let DeviceSelector::Key(k) = &s.device else {
                        panic!("a page widget names a key");
                    };
                    let forms: Vec<&str> = w.forms.names().collect();
                    format!("{k} {} [{}]", s.metric.as_str(), forms.join(","))
                }
                WidgetContent::Fixed(f) => format!("fixed {}", f.name()),
                WidgetContent::Group(g) => format!("group {}", g.members.name()),
            })
            .collect()
    }

    #[test]
    fn container_ids() {
        assert_eq!(page_container("cpu"), "page.cpu");
        assert_eq!(page_key("page.gpu:GPU-0a1b"), Some("gpu:GPU-0a1b"));
        assert_eq!(page_key("overview.gpu"), None);
        let cases = [
            ("page.cpu", Some(DeviceKind::Cpu)),
            ("page.memory", Some(DeviceKind::Memory)),
            (GPU, Some(DeviceKind::Gpu)),
            (GPU2, Some(DeviceKind::Gpu)),
            ("page.disk:S4EWNX0R123", Some(DeviceKind::Disk)),
            ("page.net:enp5s0", Some(DeviceKind::Net)),
            ("page.fan:nct6798:1", Some(DeviceKind::Fan)),
            ("page.bat:BAT0", Some(DeviceKind::Battery)),
            ("page.aggregate:disk", Some(DeviceKind::Disks)),
            ("page.aggregate:gpu", Some(DeviceKind::AggregateGpu)),
            ("page.cpu:3", None),
            ("page.aggregate:net", None),
            ("page.nope", None),
            ("overview.cpu", None),
        ];
        for (id, kind) in cases {
            assert_eq!(page_kind(id), kind, "{id}");
        }
        // This build supports no page kind yet; the page beads add them.
        assert!(PAGE_KINDS.iter().all(|k| PAGE_DEVICE_KINDS.contains(k)));
        assert!(is_page_container_for(GPU, &PAGE_DEVICE_KINDS));
        assert!(!is_page_container_for(GPU, &[DeviceKind::Cpu]));
    }

    #[test]
    fn every_page_metric_maps_to_its_series() {
        for (kind, key) in PAGE_METRICS {
            let series = metric_series(kind, &MetricKey::new(key))
                .unwrap_or_else(|| panic!("{kind:?} {key} has series"));
            assert_eq!(
                page_metric_series(kind, key),
                Some(series),
                "{kind:?} {key}"
            );
        }
        let at = |kind, key| metric_series(kind, &MetricKey::new(key));
        // Phase 1 and 2 keys keep their series; `power` and `temp` follow the kind.
        assert_eq!(
            at(DeviceKind::Gpu, METRIC_POWER),
            Some(MetricSeries::One(MetricId::GPU_POWER))
        );
        assert_eq!(
            at(DeviceKind::Battery, METRIC_POWER),
            Some(MetricSeries::One(MetricId::BATTERY_POWER))
        );
        assert_eq!(
            at(DeviceKind::Fan, METRIC_TEMP),
            Some(MetricSeries::Paired(MetricId::SENSOR_TEMP))
        );
        assert_eq!(
            at(DeviceKind::Cpu, METRIC_CORES_STACKED),
            Some(MetricSeries::PerLogical(MetricId::CPU_TOTAL))
        );
        assert!(at(DeviceKind::Memory, METRIC_PHYSICAL).is_some_and(|s| s.is_multi()));
        assert_eq!(
            at(DeviceKind::Memory, METRIC_PHYSICAL).map(|s| s.ids().to_vec()),
            Some(vec![
                MetricId::MEM_USED,
                MetricId::MEM_COMMIT,
                MetricId::MEM_DIRTY
            ])
        );
        // A phase 3 key on another kind has none.
        assert_eq!(at(DeviceKind::Disks, METRIC_RESPONSE), None);
        assert_eq!(at(DeviceKind::Nets, METRIC_TOTAL_RX), None);
        assert_eq!(at(DeviceKind::Gpu, METRIC_UTIL), None);
        assert_eq!(at(DeviceKind::AggregateGpu, METRIC_GPU_CLOCKS), None);
        assert_eq!(at(DeviceKind::Fan, METRIC_POWER), None);
    }

    #[test]
    fn every_page_has_a_default_with_its_presets() {
        for (key, kind) in [
            ("cpu", DeviceKind::Cpu),
            ("memory", DeviceKind::Memory),
            ("gpu:GPU-1", DeviceKind::Gpu),
            ("disk:S1", DeviceKind::Disk),
            ("aggregate:disk", DeviceKind::Disks),
            ("net:wlan0", DeviceKind::Net),
            ("fan:nct:1", DeviceKind::Fan),
            ("bat:BAT0", DeviceKind::Battery),
            ("aggregate:gpu", DeviceKind::AggregateGpu),
        ] {
            let container = page_container(key);
            let l = default_page_layout(&container).expect("a default");
            assert_eq!(l.container, container);
            assert_eq!(l.version, crate::layout::LAYOUT_VERSION, "{key}");
            let p = l.preset.as_ref().expect("a flow layout");
            assert_eq!(p.id, page_presets(kind)[0], "{key}");
            // Every list holds Details once; every widget names the page's own device.
            let details = l
                .widgets
                .iter()
                .filter(|w| w.content == WidgetContent::Fixed(FixedWidget::Details))
                .count();
            assert_eq!(details, 1, "{key}");
            for w in &l.widgets {
                match &w.content {
                    WidgetContent::Fixed(f) => {
                        assert!(w.forms.is_empty());
                        assert!(f.allowed_in(&container), "{key} {}", f.name());
                    }
                    content => {
                        assert_eq!(
                            content.device(),
                            Some(&DeviceSelector::Key(key.into())),
                            "{key}"
                        );
                        let m = content.metric_key().expect("a metric");
                        assert!(metric_series(kind, m).is_some(), "{key} {}", m.as_str());
                    }
                }
            }
        }
        assert!(default_page_layout("page.cpu:0").is_none());
        assert!(default_page_layout("overview.cpu").is_none());
    }

    #[test]
    fn every_cpu_combination_converts() {
        use CpuGraphMode::*;
        use CpuSecondaryGraph as S;
        for mode in [Overall, PerCore, AllCoresOverlaid, AllCoresStacked, Board] {
            for graph in [S::None, S::Power, S::Clock, S::Clocks, S::Temperature] {
                let perf = PerformanceSettings {
                    cpu_graph_mode: mode,
                    cpu_secondary_graph: graph,
                    ..PerformanceSettings::default()
                };
                let l = convert_cpu_page(&perf, "page.cpu");
                let mut want: Vec<String> = match mode {
                    Overall => vec!["cpu total [chart,value,label]".into()],
                    PerCore => vec!["fixed cpu_core_grid".into()],
                    AllCoresOverlaid => vec!["cpu cores_overlaid [chart,label]".into()],
                    AllCoresStacked => vec!["cpu cores_stacked [chart,label]".into()],
                    Board => vec![
                        "cpu total [chart,value,label]".into(),
                        "fixed cpu_core_board".into(),
                    ],
                };
                let cvl = |k: &str| format!("cpu {k} [chart,value,label]");
                match graph {
                    S::Power => want.extend([cvl("power_draw"), cvl("temp"), cvl("clock")]),
                    S::Clock => want.push(cvl("clock")),
                    S::Clocks => want.push("cpu core_clocks [chart,label]".into()),
                    S::Temperature => want.push(cvl("temp")),
                    S::None => {}
                }
                want.push("fixed details".into());
                assert_eq!(describe(&l), want, "{mode:?} {graph:?}");
                assert_eq!(
                    l.preset.as_ref().map(|p| p.id.as_str()),
                    Some(cpu_preset(mode))
                );
                let ids: Vec<u32> = l.widgets.iter().map(|w| w.id).collect();
                assert_eq!(ids, (1..=want.len() as u32).collect::<Vec<_>>());
                assert_eq!(convert_cpu_page(&perf, "page.cpu"), l, "deterministic");
            }
        }
    }

    #[test]
    fn every_memory_combination_converts() {
        for view in [MemoryView::Usage, MemoryView::Apps] {
            for swap in [false, true] {
                for bar in [false, true] {
                    let perf = PerformanceSettings {
                        memory_view: view,
                        memory_swap_graph: swap,
                        memory_composition: bar,
                        ..PerformanceSettings::default()
                    };
                    let l = convert_memory_page(&perf, "page.memory");
                    let mut want = vec![match view {
                        MemoryView::Usage => "memory physical [chart,value,label]".to_owned(),
                        MemoryView::Apps => "fixed memory_by_app".to_owned(),
                    }];
                    want.push("memory commit_pct [chart,value,label]".into());
                    if swap {
                        want.push("memory swap [chart,value,label]".into());
                    }
                    if bar {
                        want.push("fixed memory_bar".into());
                    }
                    for f in [
                        "details",
                        "memory_compressed",
                        "swap_devices",
                        "memory_modules",
                    ] {
                        want.push(format!("fixed {f}"));
                    }
                    assert_eq!(describe(&l), want, "{view:?} {swap} {bar}");
                    assert_eq!(
                        l.preset.as_ref().map(|p| p.id.as_str()),
                        Some(memory_preset(view))
                    );
                }
            }
        }
    }

    #[test]
    fn every_gpu_subset_converts() {
        use GpuOptionalGraph::{Clocks, Power};
        for optional in [vec![], vec![Power], vec![Clocks], vec![Clocks, Power]] {
            let perf = PerformanceSettings {
                gpu_optional_graphs: optional.clone(),
                ..PerformanceSettings::default()
            };
            let l = convert_gpu_page(&perf, GPU);
            let key = "gpu:GPU-0a1b2c3d";
            let cvl = |k: &str| format!("{key} {k} [chart,value,label]");
            let mut want = vec![
                "fixed gpu_engine_grid".to_owned(),
                cvl("memory"),
                cvl("temp"),
            ];
            if optional.contains(&Power) {
                want.push(cvl("power"));
            }
            if optional.contains(&Clocks) {
                want.push(cvl("clocks"));
            }
            want.push("fixed details".into());
            want.push("fixed gpu_displays".into());
            assert_eq!(describe(&l), want, "{optional:?}");
            assert_eq!(l.preset.as_ref().map(|p| p.id.as_str()), Some("default"));
        }
    }

    #[test]
    fn load_converts_cpu_and_memory_pages_once() {
        let text = "[performance]\ncpu_graph_mode = \"board\"\nmemory_swap_graph = false\n";
        let (first, converted) = load_all(text);
        assert!(converted);
        let layouts = &first.performance.layouts;
        assert!(layouts.known("page.cpu").is_some());
        assert!(layouts.known("page.memory").is_some());
        // The old keys stay as loaded.
        assert_eq!(first.performance.cpu_graph_mode, CpuGraphMode::Board);
        assert!(!first.performance.memory_swap_graph);
        let out = save(&first);
        assert!(out.contains("cpu_graph_mode = \"board\""), "{out}");
        assert!(out.contains("container = \"page.cpu\""), "{out}");
        // Converting twice changes nothing.
        let (second, converted) = load_all(&out);
        assert!(!converted);
        assert_eq!(second, first);
        let mut third = second.clone();
        assert!(!convert_old_page_settings_for(
            &mut third.performance,
            &PAGE_DEVICE_KINDS
        ));
        assert_eq!(third, second);
    }

    #[test]
    fn defaults_convert_nothing() {
        let (s, converted) = load_all("");
        assert!(!converted);
        assert!(s.performance.layouts.is_empty());
        assert!(!save(&s).contains("layouts"));
        // Only the page that is off its defaults converts.
        let (s, converted) = load_all("[performance]\nmemory_view = \"apps\"\n");
        assert!(converted);
        assert!(s.performance.layouts.entry("page.cpu").is_none());
        assert!(s.performance.layouts.known("page.memory").is_some());
    }

    #[test]
    fn this_build_converts_no_page_yet() {
        // Until the CPU page bead adds its kind, clamp converts nothing.
        if PAGE_KINDS.contains(&DeviceKind::Cpu) {
            return;
        }
        let mut s: Settings =
            toml::from_str("[performance]\ncpu_graph_mode = \"overall\"\n").expect("TOML");
        let changed = s.clamp();
        assert!(!changed.contains(&LAYOUTS_CONVERTED_KEY));
        assert!(s.performance.layouts.is_empty());
        assert!(!page_needs_conversion(&s.performance, "page.cpu"));
    }

    #[test]
    fn a_page_with_an_entry_does_not_convert() {
        let text = "[performance]\ncpu_graph_mode = \"overall\"\n\n[[performance.layouts]]\ncontainer = \"page.cpu\"\nversion = 2\nwidgets = []\n";
        let (s, converted) = load_all(text);
        assert!(!converted);
        let l = s.performance.layouts.known("page.cpu").expect("kept");
        assert!(l.widgets.is_empty() && l.preset.is_none());
    }

    #[test]
    fn unsupported_and_gone_pages_kept_unchanged() {
        let text = format!(
            "[[performance.layouts]]\ncontainer = \"{GPU}\"\nversion = 2\nnext_id = 4\n\n[[performance.layouts.widgets]]\nid = 3\ndevice = \"gpu:GPU-0a1b2c3d\"\nmetric = \"memory\"\nforms = [\"chart\", \"value\"]\ncol = 0\nrow = 0\ncols = 12\nrows = 7\n"
        );
        // A build without the GPU page keeps the entry as read.
        let mut s: Settings = toml::from_str(&text).expect("TOML");
        assert!(s.clamp().is_empty());
        assert!(matches!(
            s.performance.layouts.entry(GPU),
            Some(LayoutEntry::Kept(_))
        ));
        let before = s.performance.layouts.clone();
        let (back, _) = load_all(&save(&s));
        assert!(back.performance.layouts.known(GPU).is_some());
        let mut again: Settings = toml::from_str(&save(&s)).expect("TOML");
        again.clamp();
        assert_eq!(again.performance.layouts, before);
        // A build with GPU pages reads it as known; nothing prunes it while the GPU is gone,
        // and it is written back unchanged.
        let (s, changed) = load_all(&text);
        assert!(!changed);
        let l = s.performance.layouts.known(GPU).expect("known");
        assert_eq!(l.widgets.len(), 1);
        assert_eq!(l.next_id, 4);
        let (again, _) = load_all(&save(&s));
        assert_eq!(again.performance.layouts, s.performance.layouts);
    }

    #[test]
    fn page_fixed_kinds_only_in_their_page() {
        let entry = |container: &str, fixed: &[&str]| {
            let widgets: String = fixed
                .iter()
                .enumerate()
                .map(|(i, f)| format!("  {{ id = {}, fixed = \"{f}\" }},\n", i + 1))
                .collect();
            format!(
                "[[performance.layouts]]\ncontainer = \"{container}\"\nversion = 2\npreset = {{ id = \"default\" }}\nwidgets = [\n{widgets}]\n"
            )
        };
        let names = |container: &str, fixed: &[&str]| -> Vec<String> {
            let (s, _) = load_all(&entry(container, fixed));
            let l = s.performance.layouts.known(container).expect("known");
            l.widgets
                .iter()
                .filter_map(|w| match &w.content {
                    WidgetContent::Fixed(f) => Some(f.name().to_owned()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            names(
                GPU,
                &[
                    "gpu_engine_grid",
                    "cpu_core_grid",
                    "details",
                    "gpu_displays"
                ]
            ),
            ["gpu_engine_grid", "details", "gpu_displays"]
        );
        assert_eq!(
            names(
                "page.memory",
                &[
                    "memory_by_app",
                    "memory_bar",
                    "disk_partitions",
                    "swap_devices"
                ]
            ),
            ["memory_by_app", "memory_bar", "swap_devices"]
        );
        assert_eq!(
            names(
                "page.aggregate:disk",
                &["details", "disk_partitions", "timeline"]
            ),
            ["details"]
        );
        assert_eq!(
            names("page.bat:BAT0", &["battery_history"]),
            ["battery_history"]
        );
        // An Overview box takes no page part; Memory by app stays allowed there.
        let (s, _) = load_all(
            &entry("overview.memory", &["details", "memory_by_app"]).replace(
                "preset = { id = \"default\" }",
                "preset = { id = \"rows\" }",
            ),
        );
        let l = s.performance.layouts.known("overview.memory").expect("box");
        assert_eq!(l.widgets.len(), 1);
        assert_eq!(
            l.widgets[0].content,
            WidgetContent::Fixed(FixedWidget::MemoryByApp)
        );
    }

    #[test]
    fn page_grid_and_presets() {
        use crate::layout::grid_cols;
        assert_eq!(grid_cols("page.cpu"), PAGE_GRID_COLS);
        assert_eq!(grid_cols(GPU), 12);
        assert_eq!(grid_cols("page.nope"), 12);
        // An unknown preset id reads as the page's default.
        let text = "[[performance.layouts]]\ncontainer = \"page.cpu\"\nversion = 2\npreset = { id = \"nope\" }\nwidgets = []\n";
        let (s, _) = load_all(text);
        let l = s.performance.layouts.known("page.cpu").expect("known");
        assert_eq!(l.preset.as_ref().map(|p| p.id.as_str()), Some("per_core"));
        let text = text.replace("nope", "board");
        let (s, _) = load_all(&text);
        let l = s.performance.layouts.known("page.cpu").expect("known");
        assert_eq!(l.preset.as_ref().map(|p| p.id.as_str()), Some("board"));
        // Groups on pages: cores on the CPU page, engines on a GPU page.
        let text = format!(
            "[[performance.layouts]]\ncontainer = \"{GPU}\"\nversion = 2\nwidgets = [\n  {{ id = 1, device = \"gpu:GPU-0a1b2c3d\", group = \"engines\", metric = \"engine:*\", forms = [\"chart\"], col = 0, row = 0, cols = 12, rows = 16 }},\n]\n"
        );
        let (s, _) = load_all(&text);
        let l = s.performance.layouts.known(GPU).expect("known");
        assert!(matches!(
            &l.widgets[0].content,
            WidgetContent::Group(g) if g.members == GroupKind::Engines
        ));
    }

    #[test]
    fn container_layout_gives_entry_or_default() {
        let perf = PerformanceSettings::default();
        for container in ["page.cpu", GPU, "page.bat:BAT0"] {
            assert_eq!(
                container_layout(&perf, container),
                default_page_layout(container)
            );
        }
        assert_eq!(
            container_layout(&perf, "overview.gpu"),
            crate::layout::box_layout(&perf, "gpu")
        );
        assert!(container_layout(&perf, "page.cpu:1").is_none());
        let mut perf = perf;
        let mine = ContainerLayout::flow(
            "page.cpu",
            PresetSetting {
                id: "overall".into(),
                ..PresetSetting::default()
            },
            cpu_mode_widgets("overall"),
        );
        perf.layouts.set(mine.clone());
        assert_eq!(container_layout(&perf, "page.cpu"), Some(mine));
    }

    #[test]
    fn reset_of_cpu_and_memory_clears_their_keys() {
        let text = "[performance]\ncpu_graph_mode = \"overall\"\ncpu_secondary_graph = \"none\"\nmemory_view = \"apps\"\nmemory_composition = false\n";
        let (mut s, converted) = load_all(text);
        assert!(converted);
        reset_to_default(&mut s.performance, "page.cpu");
        assert!(s.performance.layouts.entry("page.cpu").is_none());
        assert_eq!(s.performance.cpu_graph_mode, CpuGraphMode::PerCore);
        assert_eq!(s.performance.cpu_secondary_graph, CpuSecondaryGraph::Power);
        // The Memory page keeps its entry and its keys.
        assert!(s.performance.layouts.known("page.memory").is_some());
        assert_eq!(s.performance.memory_view, MemoryView::Apps);
        reset_to_default(&mut s.performance, "page.memory");
        assert!(s.performance.layouts.is_empty());
        assert_eq!(s.performance.memory_view, MemoryView::Usage);
        assert!(s.performance.memory_composition && s.performance.memory_swap_graph);
        // A save and a load then convert nothing.
        let (back, converted) = load_all(&save(&s));
        assert!(!converted);
        assert!(back.performance.layouts.is_empty());
    }

    #[test]
    fn reset_of_one_gpu_page_leaves_the_shared_key() {
        let mut perf = PerformanceSettings {
            gpu_optional_graphs: vec![GpuOptionalGraph::Power],
            ..PerformanceSettings::default()
        };
        let kinds = &PAGE_DEVICE_KINDS;
        // Both GPU pages convert when first built.
        for c in [GPU, GPU2] {
            assert!(page_needs_conversion_for(&perf, c, kinds));
        }
        let first = convert_gpu_page(&perf, GPU);
        perf.layouts.set(first);
        reset_to_default(&mut perf, GPU);
        // The reset writes the default entry and leaves the key.
        assert_eq!(perf.gpu_optional_graphs, vec![GpuOptionalGraph::Power]);
        assert_eq!(perf.layouts.known(GPU), default_page_layout(GPU).as_ref());
        assert!(!page_needs_conversion_for(&perf, GPU, kinds));
        // The other GPU still converts, with Power.
        assert!(page_needs_conversion_for(&perf, GPU2, kinds));
        let other = convert_gpu_page(&perf, GPU2);
        assert!(
            describe(&other)
                .iter()
                .any(|w| w.ends_with("power [chart,value,label]"))
        );
        // The reset's entry survives a save and a load.
        let mut layouts = LayoutsSetting(perf.layouts.0.clone());
        let table = toml::Value::try_from(&layouts).expect("serializes");
        layouts = table.try_into().expect("reads");
        layouts.load_for(kinds);
        assert_eq!(layouts, perf.layouts);
    }

    #[test]
    fn reset_of_other_pages_removes_the_entry() {
        let mut perf = PerformanceSettings::default();
        let disk = "page.disk:S1";
        perf.layouts.set(default_page_layout(disk).expect("disk"));
        reset_to_default(&mut perf, disk);
        assert!(perf.layouts.is_empty());
        assert_eq!(perf, PerformanceSettings::default());
    }
}
