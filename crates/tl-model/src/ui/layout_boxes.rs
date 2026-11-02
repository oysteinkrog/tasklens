//! The metric keys, preset lists and conversions of the Overview CPU, Memory, Disk, Network, Top
//! processes and Timeline boxes (ADR-019 "Phase 2", D-041). No file I/O.
//!
//! A metric key is looked up together with its device's kind ([`DeviceKind`]), so `temp` and
//! `power` can mean a CPU or a GPU metric. [`metric_series`] maps each key to its series. Each
//! box's old modes are presets: [`preset_widgets`] gives a preset's widget list, and the
//! `convert_*_box` functions turn a box's old grid keys into a flow layout, as
//! [`crate::ui::layout::convert_gpu_box`] does for the GPU box. Conversion is deterministic: it
//! reads no device, and a group stands for whatever members are present.

use crate::ids::MetricId;
use crate::ui::layout::{
    CPU_BOX_CONTAINER, ContainerLayout, DISK_BOX_CONTAINER, DeviceSelector, FixedWidget, FormSet,
    GroupItems, GroupKind, GroupOptions, GroupSelector, MEMORY_BOX_CONTAINER, METRIC_POWER,
    METRIC_TEMP, MetricKey, NET_BOX_CONTAINER, PROCESSES_BOX_CONTAINER, PresetSetting,
    SourceSelector, TIMELINE_BOX_CONTAINER, WidgetContent, convert_gpu_box, converted_preset,
    presets_of,
};
use crate::ui::settings::{
    OVERVIEW_POWER_PLACEMENTS, OVERVIEW_POWER_SPLIT_RANGE, OverviewGridSetting,
};

/// The persistent key of the CPU package (PERF-005).
pub const DEVICE_CPU: &str = "cpu";
/// The persistent key of memory.
pub const DEVICE_MEMORY: &str = "memory";
/// The persistent key of the disk aggregate.
pub const DEVICE_DISKS: &str = "aggregate:disk";
/// The persistent key of the network aggregate.
pub const DEVICE_NETS: &str = "aggregate:net";
/// The prefix of a logical CPU's persistent key, `cpu:<n>`.
pub const CPU_LOGICAL_PREFIX: &str = "cpu:";
/// The prefix of a disk's persistent key.
pub const DISK_PREFIX: &str = "disk:";
/// The prefix of a network adapter's persistent key.
pub const NET_PREFIX: &str = "net:";
/// The prefix of a GPU's persistent key.
pub const GPU_PREFIX: &str = "gpu:";
/// The persistent key of the GPU aggregate (Aggregate graphics).
pub const DEVICE_GPUS: &str = "aggregate:gpu";
/// The prefix of a fan's persistent key.
pub const FAN_PREFIX: &str = "fan:";
/// The prefix of a battery's persistent key.
pub const BATTERY_PREFIX: &str = "bat:";

/// `cpu`: utilization.
pub const METRIC_CPU_TOTAL: &str = "total";
/// `cpu`: kernel time.
pub const METRIC_CPU_KERNEL: &str = "kernel";
/// `cpu` and `cpu:<n>`: speed.
pub const METRIC_FREQ: &str = "freq";
/// `cpu`: core power. `temp` and `power` are [`METRIC_TEMP`] and [`METRIC_POWER`].
pub const METRIC_POWER_CORES: &str = "power_cores";
/// `cpu`: package power filled and core power dashed, one chart.
pub const METRIC_POWER_DRAW: &str = "power_draw";
/// `cpu`: the clock of every physical core, one line each.
pub const METRIC_CORE_CLOCKS: &str = "core_clocks";
/// `cpu` and `memory`: pressure stall, "some".
pub const METRIC_PRESSURE: &str = "pressure";
/// `cpu:<n>`: the logical CPU's load.
pub const METRIC_LOAD: &str = "load";
/// `memory`: the five rows of the Memory box.
pub const METRIC_MEM_USED: &str = "used";
pub const METRIC_MEM_CACHED: &str = "cached";
pub const METRIC_MEM_AVAILABLE: &str = "available";
pub const METRIC_MEM_SWAP: &str = "swap";
pub const METRIC_MEM_COMMIT: &str = "commit";
/// `memory`: in use, cached and available stacked.
pub const METRIC_MEM_COMPOSITION: &str = "composition";
/// Disks: active time.
pub const METRIC_DISK_ACTIVE: &str = "active";
/// Disks: read and write rates.
pub const METRIC_DISK_READ: &str = "read";
pub const METRIC_DISK_WRITE: &str = "write";
/// Disks: read and write, two lines in one chart.
pub const METRIC_DISK_IO: &str = "io";
/// Adapters: download and upload rates.
pub const METRIC_NET_RX: &str = "rx";
pub const METRIC_NET_TX: &str = "tx";
/// Adapters: download and upload, two lines in one chart.
pub const METRIC_NET_TRAFFIC: &str = "traffic";

/// The kind of device a persistent key names. A metric key is looked up with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    /// `cpu`, the CPU package.
    Cpu,
    /// `cpu:<n>`, a logical CPU.
    CpuLogical,
    /// `memory`.
    Memory,
    /// `disk:…`, one disk.
    Disk,
    /// `aggregate:disk`, every disk.
    Disks,
    /// `net:…`, one adapter.
    Net,
    /// `aggregate:net`, the adapters the Network box counts.
    Nets,
    /// `gpu:…`, or a GPU box slot.
    Gpu,
    /// `fan:…`, one fan.
    Fan,
    /// `bat:…`, one battery.
    Battery,
    /// `aggregate:gpu`, every GPU (Aggregate graphics).
    AggregateGpu,
    /// Any other key, including one this build does not know.
    Other,
}

impl DeviceKind {
    /// The kind of the device with persistent key `key`.
    pub fn of_key(key: &str) -> Self {
        match key {
            DEVICE_CPU => DeviceKind::Cpu,
            DEVICE_MEMORY => DeviceKind::Memory,
            DEVICE_DISKS => DeviceKind::Disks,
            DEVICE_NETS => DeviceKind::Nets,
            DEVICE_GPUS => DeviceKind::AggregateGpu,
            k if k
                .strip_prefix(CPU_LOGICAL_PREFIX)
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) =>
            {
                DeviceKind::CpuLogical
            }
            k if k.starts_with(DISK_PREFIX) => DeviceKind::Disk,
            k if k.starts_with(NET_PREFIX) => DeviceKind::Net,
            k if k.starts_with(GPU_PREFIX) => DeviceKind::Gpu,
            k if k.starts_with(FAN_PREFIX) => DeviceKind::Fan,
            k if k.starts_with(BATTERY_PREFIX) => DeviceKind::Battery,
            _ => DeviceKind::Other,
        }
    }

    /// The kind of a selector's device. A slot is a GPU box slot.
    pub fn of(device: &DeviceSelector) -> Self {
        match device {
            DeviceSelector::Key(k) => Self::of_key(k),
            DeviceSelector::Slot(_) => DeviceKind::Gpu,
        }
    }
}

/// The series a metric key draws on its device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricSeries {
    /// One series.
    One(MetricId),
    /// Two lines in one chart (`power_draw`, `io`, `traffic`), the filled or first one first.
    Pair([MetricId; 2]),
    /// Three series stacked (`composition`).
    Stacked([MetricId; 3]),
    /// Three lines in one chart, not stacked (`physical`), the filled one first.
    Triple([MetricId; 3]),
    /// This metric of every physical core, one line each (`core_clocks`). The series are on
    /// the logical CPU devices, not on the widget's own device.
    PerCore(MetricId),
    /// This metric of every present logical CPU, one line each (`cores_overlaid`,
    /// `cores_stacked`). The series are on the logical CPU devices.
    PerLogical(MetricId),
    /// This metric of the device paired with the widget's device: the temperature sensor a
    /// fan's `[fans]` entry names, else its default pairing (fan `temp`).
    Paired(MetricId),
    /// No series: the value comes from the device record only (disk `temp`).
    NoSeries,
}

impl MetricSeries {
    /// The series ids on the widget's own device, the first first. Empty for `NoSeries`; the
    /// one metric for `PerCore`, `PerLogical` and `Paired`, whose devices the source finds.
    pub fn ids(&self) -> &[MetricId] {
        match self {
            MetricSeries::One(id)
            | MetricSeries::PerCore(id)
            | MetricSeries::PerLogical(id)
            | MetricSeries::Paired(id) => std::slice::from_ref(id),
            MetricSeries::Pair(ids) => ids,
            MetricSeries::Stacked(ids) | MetricSeries::Triple(ids) => ids,
            MetricSeries::NoSeries => &[],
        }
    }

    /// The first series; `None` for `NoSeries`.
    pub fn first(&self) -> Option<MetricId> {
        self.ids().first().copied()
    }

    /// Whether the metric draws two or more lines in one chart.
    pub fn is_multi(&self) -> bool {
        matches!(
            self,
            MetricSeries::Pair(..)
                | MetricSeries::Stacked(_)
                | MetricSeries::Triple(_)
                | MetricSeries::PerCore(_)
                | MetricSeries::PerLogical(_)
        )
    }
}

/// The series of `metric` on a device of `kind`; `None` for a key this build does not know on
/// that kind. GPU keys of phase 1 are [`MetricKey::metric`]; the keys phase 3 adds are
/// [`crate::ui::layout_pages::page_metric_series`].
pub fn metric_series(kind: DeviceKind, metric: &MetricKey) -> Option<MetricSeries> {
    box_metric_series(kind, metric)
        .or_else(|| crate::ui::layout_pages::page_metric_series(kind, metric.as_str()))
}

/// The series of a phase 1 or phase 2 key.
fn box_metric_series(kind: DeviceKind, metric: &MetricKey) -> Option<MetricSeries> {
    use MetricSeries::{NoSeries, One, Pair, PerCore, Stacked};
    let key = metric.as_str();
    let series = match kind {
        DeviceKind::Gpu => One(metric.metric()?),
        DeviceKind::Cpu => match key {
            METRIC_CPU_TOTAL => One(MetricId::CPU_TOTAL),
            METRIC_CPU_KERNEL => One(MetricId::CPU_KERNEL),
            METRIC_FREQ => One(MetricId::CPU_FREQ),
            METRIC_TEMP => One(MetricId::CPU_TEMP),
            METRIC_POWER => One(MetricId::CPU_POWER_PKG),
            METRIC_POWER_CORES => One(MetricId::CPU_POWER_CORE),
            METRIC_POWER_DRAW => Pair([MetricId::CPU_POWER_PKG, MetricId::CPU_POWER_CORE]),
            METRIC_CORE_CLOCKS => PerCore(MetricId::CPU_CORE_CLOCK),
            METRIC_PRESSURE => One(MetricId::PSI_CPU_SOME),
            _ => return None,
        },
        DeviceKind::CpuLogical => match key {
            METRIC_LOAD => One(MetricId::CPU_TOTAL),
            METRIC_FREQ => One(MetricId::CPU_FREQ),
            _ => return None,
        },
        DeviceKind::Memory => match key {
            METRIC_MEM_USED => One(MetricId::MEM_USED),
            METRIC_MEM_CACHED => One(MetricId::MEM_CACHED),
            METRIC_MEM_AVAILABLE => One(MetricId::MEM_AVAILABLE),
            METRIC_MEM_SWAP => One(MetricId::SWAP_USED),
            METRIC_MEM_COMMIT => One(MetricId::MEM_COMMIT),
            METRIC_PRESSURE => One(MetricId::PSI_MEM_SOME),
            METRIC_MEM_COMPOSITION => Stacked([
                MetricId::MEM_USED,
                MetricId::MEM_CACHED,
                MetricId::MEM_AVAILABLE,
            ]),
            _ => return None,
        },
        DeviceKind::Disk | DeviceKind::Disks => match key {
            METRIC_DISK_ACTIVE => One(MetricId::DISK_ACTIVE),
            METRIC_DISK_READ => One(MetricId::DISK_READ),
            METRIC_DISK_WRITE => One(MetricId::DISK_WRITE),
            METRIC_DISK_IO => Pair([MetricId::DISK_READ, MetricId::DISK_WRITE]),
            METRIC_TEMP if kind == DeviceKind::Disk => NoSeries,
            _ => return None,
        },
        DeviceKind::Net | DeviceKind::Nets => match key {
            METRIC_NET_RX => One(MetricId::NET_RX),
            METRIC_NET_TX => One(MetricId::NET_TX),
            METRIC_NET_TRAFFIC => Pair([MetricId::NET_RX, MetricId::NET_TX]),
            _ => return None,
        },
        DeviceKind::Fan | DeviceKind::Battery | DeviceKind::AggregateGpu | DeviceKind::Other => {
            return None;
        }
    };
    Some(series)
}

/// Every metric key phase 2 adds, with a device kind it is valid on (ADR-019 phase 2 "Metric
/// keys"). Keys valid on two kinds are listed for each.
pub const BOX_METRICS: [(DeviceKind, &str); 33] = [
    (DeviceKind::Cpu, METRIC_CPU_TOTAL),
    (DeviceKind::Cpu, METRIC_CPU_KERNEL),
    (DeviceKind::Cpu, METRIC_FREQ),
    (DeviceKind::Cpu, METRIC_TEMP),
    (DeviceKind::Cpu, METRIC_POWER),
    (DeviceKind::Cpu, METRIC_POWER_CORES),
    (DeviceKind::Cpu, METRIC_POWER_DRAW),
    (DeviceKind::Cpu, METRIC_CORE_CLOCKS),
    (DeviceKind::Cpu, METRIC_PRESSURE),
    (DeviceKind::CpuLogical, METRIC_LOAD),
    (DeviceKind::CpuLogical, METRIC_FREQ),
    (DeviceKind::Memory, METRIC_MEM_USED),
    (DeviceKind::Memory, METRIC_MEM_CACHED),
    (DeviceKind::Memory, METRIC_MEM_AVAILABLE),
    (DeviceKind::Memory, METRIC_MEM_SWAP),
    (DeviceKind::Memory, METRIC_MEM_COMMIT),
    (DeviceKind::Memory, METRIC_PRESSURE),
    (DeviceKind::Memory, METRIC_MEM_COMPOSITION),
    (DeviceKind::Disk, METRIC_DISK_ACTIVE),
    (DeviceKind::Disk, METRIC_DISK_READ),
    (DeviceKind::Disk, METRIC_DISK_WRITE),
    (DeviceKind::Disk, METRIC_DISK_IO),
    (DeviceKind::Disk, METRIC_TEMP),
    (DeviceKind::Disks, METRIC_DISK_ACTIVE),
    (DeviceKind::Disks, METRIC_DISK_READ),
    (DeviceKind::Disks, METRIC_DISK_WRITE),
    (DeviceKind::Disks, METRIC_DISK_IO),
    (DeviceKind::Net, METRIC_NET_RX),
    (DeviceKind::Net, METRIC_NET_TX),
    (DeviceKind::Net, METRIC_NET_TRAFFIC),
    (DeviceKind::Nets, METRIC_NET_RX),
    (DeviceKind::Nets, METRIC_NET_TX),
    (DeviceKind::Nets, METRIC_NET_TRAFFIC),
];

/// The forms of a chart with its value and name.
const CHART_VALUE_LABEL: FormSet = FormSet::CHART.union(FormSet::VALUE).union(FormSet::LABEL);
/// The forms of a core row and a Memory box row: chart, bar, value and name.
const ROW: FormSet = CHART_VALUE_LABEL.union(FormSet::BAR);
/// The forms of a bar row: bar, value and name.
const BAR_VALUE_LABEL: FormSet = FormSet::BAR.union(FormSet::VALUE).union(FormSet::LABEL);

/// The most disks the Disk box presets show; the others are counted in "+n more".
pub const DISK_GROUP_LIMIT: u16 = 8;

/// A metric widget of the device with persistent key `device`.
fn metric(device: &str, key: &str, forms: FormSet) -> (WidgetContent, FormSet) {
    let source = SourceSelector {
        device: DeviceSelector::Key(device.into()),
        metric: MetricKey::new(key),
    };
    (WidgetContent::Metric(source), forms)
}

/// A group widget.
fn group(
    device: &str,
    members: GroupKind,
    key: &str,
    forms: FormSet,
    options: GroupOptions,
) -> (WidgetContent, FormSet) {
    let group = GroupSelector {
        device: DeviceSelector::Key(device.into()),
        members,
        metric: MetricKey::new(key),
        options,
    };
    (WidgetContent::Group(group), forms)
}

/// A fixed widget, which has no forms.
fn fixed(kind: FixedWidget) -> (WidgetContent, FormSet) {
    (WidgetContent::Fixed(kind), FormSet::empty())
}

/// The CPU box's Core clocks chart, added after a preset's widgets when it is on.
pub fn cpu_core_clocks_widget() -> (WidgetContent, FormSet) {
    metric(
        DEVICE_CPU,
        METRIC_CORE_CLOCKS,
        FormSet::CHART | FormSet::LABEL,
    )
}

/// The CPU box's power draw strip, added after a preset's widgets (and the Core clocks chart)
/// when it is on.
pub fn cpu_power_draw_widget() -> (WidgetContent, FormSet) {
    metric(DEVICE_CPU, METRIC_POWER_DRAW, CHART_VALUE_LABEL)
}

/// The widget list of preset `preset.id` of the Overview box `kind`, in list order (ADR-019
/// phase 2 "Presets"). `preset.arrange` is the arrange of the group the cell presets show. The
/// CPU box's Core clocks and power draw widgets are not in it ([`cpu_core_clocks_widget`],
/// [`cpu_power_draw_widget`]). An unknown preset gives the kind's default preset's list; an
/// unknown kind or the GPU box gives an empty list (the GPU box lists are
/// [`convert_gpu_box`]'s, which need the slots).
pub fn preset_widgets(kind: &str, preset: &PresetSetting) -> Vec<(WidgetContent, FormSet)> {
    let container = crate::ui::layout::overview_container(kind);
    let presets = presets_of(&container);
    let id = if presets.contains(&preset.id.as_str()) {
        preset.id.as_str()
    } else {
        presets.first().copied().unwrap_or_default()
    };
    let arrange = preset.arrange.clone();
    let rows = |limit| GroupOptions {
        limit,
        items: GroupItems::Rows,
        arrange: None,
    };
    let cells = |limit| GroupOptions {
        limit,
        items: GroupItems::Cells,
        arrange: arrange.clone(),
    };
    const MEM_ROWS: [&str; 5] = [
        METRIC_MEM_USED,
        METRIC_MEM_CACHED,
        METRIC_MEM_AVAILABLE,
        METRIC_MEM_SWAP,
        METRIC_MEM_COMMIT,
    ];
    let mem_rows = |forms| {
        MEM_ROWS
            .into_iter()
            .map(move |k| metric(DEVICE_MEMORY, k, forms))
    };
    let cores = |forms, options| group(DEVICE_CPU, GroupKind::Cores, METRIC_LOAD, forms, options);
    let total = || metric(DEVICE_CPU, METRIC_CPU_TOTAL, FormSet::CHART);
    match (kind, id) {
        ("cpu", "chart") => vec![total()],
        ("cpu", "cores") => vec![cores(ROW, rows(None))],
        ("cpu", "core_charts") => vec![cores(CHART_VALUE_LABEL, cells(None))],
        ("cpu", _) => vec![total(), cores(ROW, rows(None))],
        ("memory", "chart_rows") => {
            let mut list = vec![metric(DEVICE_MEMORY, METRIC_MEM_USED, FormSet::CHART)];
            list.extend(mem_rows(BAR_VALUE_LABEL));
            list
        }
        ("memory", "chart") => vec![metric(
            DEVICE_MEMORY,
            METRIC_MEM_COMPOSITION,
            FormSet::CHART,
        )],
        ("memory", "charts") => mem_rows(CHART_VALUE_LABEL).collect(),
        ("memory", "apps") => vec![fixed(FixedWidget::MemoryByApp)],
        ("memory", _) => mem_rows(ROW).collect(),
        ("disk", "rows") => vec![group(
            DEVICE_DISKS,
            GroupKind::Disks,
            METRIC_DISK_IO,
            CHART_VALUE_LABEL,
            rows(Some(DISK_GROUP_LIMIT)),
        )],
        ("disk", "total_chart") => vec![metric(DEVICE_DISKS, METRIC_DISK_IO, FormSet::CHART)],
        ("disk", _) => vec![group(
            DEVICE_DISKS,
            GroupKind::Disks,
            METRIC_DISK_IO,
            CHART_VALUE_LABEL,
            cells(Some(DISK_GROUP_LIMIT)),
        )],
        ("net", "combined") => vec![metric(DEVICE_NETS, METRIC_NET_TRAFFIC, CHART_VALUE_LABEL)],
        ("net", "adapter_charts") => vec![group(
            DEVICE_NETS,
            GroupKind::Adapters,
            METRIC_NET_TRAFFIC,
            CHART_VALUE_LABEL,
            cells(None),
        )],
        ("net", _) => vec![
            metric(DEVICE_NETS, METRIC_NET_RX, CHART_VALUE_LABEL),
            metric(DEVICE_NETS, METRIC_NET_TX, CHART_VALUE_LABEL),
        ],
        ("processes", _) => vec![fixed(FixedWidget::Processes)],
        ("timeline", _) => vec![fixed(FixedWidget::Timeline)],
        _ => Vec::new(),
    }
}

/// The flow layout of `container` with the preset of `entry` and its widget list.
fn convert_plain(kind: &str, container: &str, entry: &OverviewGridSetting) -> ContainerLayout {
    let preset = converted_preset(container, entry);
    let widgets = preset_widgets(kind, &preset);
    ContainerLayout::flow(container, preset, widgets)
}

/// Converts the CPU box's old keys (`mode`, `split`, `arrange`, `power`, `power_split`,
/// `power_placement` and `clocks`) to a flow layout: the preset's list, then the Core clocks
/// chart when `clocks` is on, then the power draw strip when `power` is on. `power_split` and
/// `power_placement` go into the preset.
pub fn convert_cpu_box(entry: &OverviewGridSetting) -> ContainerLayout {
    let mut preset = converted_preset(CPU_BOX_CONTAINER, entry);
    let (lo, hi) = OVERVIEW_POWER_SPLIT_RANGE;
    preset.power_split = entry.power_split.filter(|s| (lo..=hi).contains(s));
    preset.power_placement = entry
        .power_placement
        .clone()
        .filter(|p| OVERVIEW_POWER_PLACEMENTS.contains(&p.as_str()));
    let mut widgets = preset_widgets("cpu", &preset);
    if entry.clocks {
        widgets.push(cpu_core_clocks_widget());
    }
    if entry.power {
        widgets.push(cpu_power_draw_widget());
    }
    ContainerLayout::flow(CPU_BOX_CONTAINER, preset, widgets)
}

/// Converts the Memory box's old keys (`mode`, `split`, `arrange`) to a flow layout.
pub fn convert_memory_box(entry: &OverviewGridSetting) -> ContainerLayout {
    convert_plain("memory", MEMORY_BOX_CONTAINER, entry)
}

/// Converts the Disk box's old keys (`mode`, `split`, `arrange`) to a flow layout.
pub fn convert_disk_box(entry: &OverviewGridSetting) -> ContainerLayout {
    convert_plain("disk", DISK_BOX_CONTAINER, entry)
}

/// Converts the Network box's old keys (`mode`, `split`, `arrange`) to a flow layout.
pub fn convert_net_box(entry: &OverviewGridSetting) -> ContainerLayout {
    convert_plain("net", NET_BOX_CONTAINER, entry)
}

/// Converts the old keys of the Overview box `kind`'s grid entry to a flow layout. `None` for
/// Top processes and Timeline, which never convert, and for an unknown kind.
pub fn convert_box(kind: &str, entry: &OverviewGridSetting) -> Option<ContainerLayout> {
    match kind {
        "cpu" => Some(convert_cpu_box(entry)),
        "memory" => Some(convert_memory_box(entry)),
        "gpu" => Some(convert_gpu_box(entry)),
        "disk" => Some(convert_disk_box(entry)),
        "net" => Some(convert_net_box(entry)),
        _ => None,
    }
}

/// The built-in default of the Top processes or Timeline box: its one preset with its fixed
/// widget. `None` for the other kinds.
pub fn fixed_box_default(kind: &str) -> Option<ContainerLayout> {
    let container = match kind {
        "processes" => PROCESSES_BOX_CONTAINER,
        "timeline" => TIMELINE_BOX_CONTAINER,
        _ => return None,
    };
    let preset = PresetSetting {
        id: presets_of(container).first().copied()?.to_owned(),
        ..PresetSetting::default()
    };
    let widgets = preset_widgets(kind, &preset);
    Some(ContainerLayout::flow(container, preset, widgets))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::layout::{GPU_BOX_CONTAINER, LAYOUT_CONTAINERS, overview_container};
    use crate::ui::settings::OVERVIEW_BOX_MODES;

    #[test]
    fn device_kinds() {
        let cases = [
            ("cpu", DeviceKind::Cpu),
            ("cpu:0", DeviceKind::CpuLogical),
            ("cpu:31", DeviceKind::CpuLogical),
            ("cpu:", DeviceKind::Other),
            ("cpu:x", DeviceKind::Other),
            ("memory", DeviceKind::Memory),
            ("disk:S4EWNX0R123", DeviceKind::Disk),
            ("aggregate:disk", DeviceKind::Disks),
            ("net:enp5s0", DeviceKind::Net),
            ("aggregate:net", DeviceKind::Nets),
            ("aggregate:net:wired", DeviceKind::Other),
            ("gpu:GPU-0a1b", DeviceKind::Gpu),
            ("fan:nct6798:1", DeviceKind::Fan),
            ("bat:BAT0", DeviceKind::Battery),
            ("aggregate:gpu", DeviceKind::AggregateGpu),
            ("sensor:k10temp:temp1", DeviceKind::Other),
        ];
        for (key, kind) in cases {
            assert_eq!(DeviceKind::of_key(key), kind, "{key}");
        }
        assert_eq!(DeviceKind::of(&DeviceSelector::Slot(1)), DeviceKind::Gpu);
    }

    #[test]
    fn every_box_metric_maps_to_its_series() {
        for (kind, key) in BOX_METRICS {
            let series = metric_series(kind, &MetricKey::new(key))
                .unwrap_or_else(|| panic!("{kind:?} {key} has series"));
            if (kind, key) == (DeviceKind::Disk, METRIC_TEMP) {
                assert_eq!(series, MetricSeries::NoSeries);
            } else {
                assert!(series.first().is_some(), "{kind:?} {key}");
            }
        }
        let at = |kind, key| metric_series(kind, &MetricKey::new(key));
        assert_eq!(
            at(DeviceKind::Cpu, METRIC_TEMP),
            Some(MetricSeries::One(MetricId::CPU_TEMP))
        );
        assert_eq!(
            at(DeviceKind::Gpu, METRIC_TEMP),
            Some(MetricSeries::One(MetricId::GPU_TEMP))
        );
        assert_eq!(
            at(DeviceKind::Cpu, METRIC_POWER_DRAW).map(|s| s.ids().to_vec()),
            Some(vec![MetricId::CPU_POWER_PKG, MetricId::CPU_POWER_CORE])
        );
        assert_eq!(
            at(DeviceKind::Memory, METRIC_MEM_COMPOSITION).map(|s| s.ids().to_vec()),
            Some(vec![
                MetricId::MEM_USED,
                MetricId::MEM_CACHED,
                MetricId::MEM_AVAILABLE
            ])
        );
        assert_eq!(
            at(DeviceKind::Cpu, METRIC_CORE_CLOCKS),
            Some(MetricSeries::PerCore(MetricId::CPU_CORE_CLOCK))
        );
        // Keys of the wrong kind and unknown keys have none.
        assert_eq!(at(DeviceKind::Disks, METRIC_TEMP), None);
        assert_eq!(at(DeviceKind::Memory, METRIC_CPU_TOTAL), None);
        assert_eq!(at(DeviceKind::Cpu, "fan"), None);
        assert_eq!(at(DeviceKind::Other, METRIC_NET_RX), None);
        assert!(at(DeviceKind::Net, METRIC_NET_TRAFFIC).is_some_and(|s| s.is_multi()));
        assert!(!at(DeviceKind::Net, METRIC_NET_RX).is_some_and(|s| s.is_multi()));
    }

    #[test]
    fn every_preset_has_a_list() {
        for (kind, modes) in OVERVIEW_BOX_MODES {
            if kind == "gpu" {
                continue;
            }
            for mode in modes {
                let preset = PresetSetting {
                    id: (*mode).to_owned(),
                    ..PresetSetting::default()
                };
                let list = preset_widgets(kind, &preset);
                assert!(!list.is_empty(), "{kind} {mode}");
                for (content, forms) in &list {
                    match content {
                        WidgetContent::Fixed(f) => {
                            assert!(forms.is_empty(), "{kind} {mode}");
                            assert!(f.allowed_in(&overview_container(kind)), "{kind} {mode}");
                        }
                        _ => assert!(!forms.is_empty(), "{kind} {mode}"),
                    }
                }
            }
        }
    }

    #[test]
    fn every_container_has_a_default() {
        for container in LAYOUT_CONTAINERS {
            let kind = crate::ui::layout::overview_kind(container).expect("an Overview box");
            let l = crate::ui::layout::default_box_layout(kind).expect("a default");
            assert_eq!(l.container, *container);
            let p = l.preset.as_ref().expect("a flow layout");
            assert_eq!(p.id, presets_of(container)[0], "{container}");
            assert!(!l.widgets.is_empty(), "{container}");
            let want = if *container == GPU_BOX_CONTAINER {
                1
            } else {
                2
            };
            assert_eq!(l.version, want, "{container}");
        }
        assert!(fixed_box_default("cpu").is_none());
        assert!(convert_box("processes", &OverviewGridSetting::default()).is_none());
        assert!(convert_box("nope", &OverviewGridSetting::default()).is_none());
    }
}
