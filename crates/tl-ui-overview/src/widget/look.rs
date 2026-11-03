//! How a metric widget's chart and bar look (ADR-019 "Flow placement" and "Inner grid"; tasklens-6swdr.12).
//!
//! A widget draws the same in a flow layout, in Edit Widgets and in a grid layout. Its accent
//! comes from the kind of device it reads (a Memory row from its row colour), and its chart's
//! series, overlays and legend come from the metric's metadata, the widget's options and the
//! settings every box follows (`performance.cpu_kernel`), never from where the widget is placed.
//! Each rule here is the 1.2.0 box part the preset draws for that metric:
//!
//! - CPU `total`: filled in the CPU colour, with the `CPU_KERNEL` line under it while
//!   `performance.cpu_kernel` is on, and the Total and Kernel legend (the CPU box's total chart).
//! - A logical CPU's `load`: 1-second means, as the core sparklines.
//! - Memory: each row's metric in its row colour, `composition` as the three stacked bands with
//!   their legend (the Memory box's `chart` mode).
//! - Disk `io`: read filled, write dashed (the `disk_charts` cells).
//! - Network: download filled, upload as the lighter line, the hidden kinds taken off the
//!   aggregate (the Network box's charts), with the box's range word.
//! - Core clocks: one line per physical core (the CPU page's Core clocks chart).
//!
//! In a device page container a Cell's chart is a 1.3.0 page chart ([`ChartLook::page_labels`],
//! ADR-019 phase 3 "Page style"): the name as its heading, the value at the right of the
//! heading, the legend, and the range text under the plot. A metric that has a 1.3.0 page chart
//! keeps that chart's heading, range word and legend (a disk's Active time and Transfer rate,
//! a GPU's Memory, Temperature, Power and Clocks, the CPU page's Utilization charts and its
//! secondary charts, the Memory page's Physical memory, Commit and Swap charts). On a page a
//! Memory chart draws in the memory colour, as the 1.3.0 Memory page does; the Memory box's row
//! colours stay in the box.
//!
//! Pure functions: no drawing, no state.

use std::sync::Arc;

use gpui_kit::{App, Hsla, SharedString, px, white};
use smallvec::{SmallVec, smallvec};
use tl_model::ids::{MetricId, SeriesId};
use tl_ui_settings::layout::{FormSet, METRIC_POWER, METRIC_TEMP, WidgetOptions};
use tl_ui_settings::layout_boxes::{
    DeviceKind, METRIC_CORE_CLOCKS, METRIC_CPU_TOTAL, METRIC_DISK_ACTIVE, METRIC_DISK_IO,
    METRIC_MEM_AVAILABLE, METRIC_MEM_CACHED, METRIC_MEM_COMMIT, METRIC_MEM_COMPOSITION,
    METRIC_MEM_SWAP, METRIC_MEM_USED, METRIC_NET_TX, METRIC_POWER_DRAW,
};
use tl_ui_settings::layout_pages::{
    METRIC_CLOCK, METRIC_COMMIT_PCT, METRIC_CORES_OVERLAID, METRIC_CORES_STACKED,
    METRIC_GPU_CLOCKS, METRIC_GPU_MEMORY, METRIC_PHYSICAL,
};

use super::meta::Lines;
use crate::overview::cpu_box::SPARK_MEAN_NS;
use crate::overview::mem_box::row_colors;
use crate::overview::net_box::UPLOAD_TINT;
use crate::overview::parts::spark_labels;
use tl_ui_base::i18n::tr;
use tl_ui_base::theme::TlTheme;
use tl_ui_base::theme::derived::blend_over;
use tl_ui_chart::chart::{ChartLabels, ChartSeries, LegendStyle, ValueAlign, ValueFormat};
use tl_ui_perf_core::core::core_clocks::clock_series;
use tl_ui_perf_core::core::look::{DIRTY_ALPHA, OVERLAID_MIN_ALPHA, STACKED_ALPHAS};

/// The CPU box's range word and legend (`cpu_box::total_labels`).
const KEY_CPU_RANGE: &str = "perf-cpu-heading-utilization";
const KEY_LEGEND_TOTAL: &str = "perf-legend-total";
const KEY_LEGEND_KERNEL: &str = "perf-legend-kernel";
/// The Network box's range word (`net_box::chart_labels`).
const KEY_NET_RANGE: &str = "perf-net-heading-throughput";
/// The disk page charts' headings, range word and legend (`disk::active_labels`,
/// `disk::transfer_labels`).
const KEY_DISK_ACTIVE: &str = "perf-disk-heading-active";
const KEY_DISK_TRANSFER: &str = "perf-disk-heading-transfer";
const KEY_DISK_THROUGHPUT: &str = "perf-disk-range-throughput";
const KEY_DISK_READ: &str = "perf-disk-legend-read";
const KEY_DISK_WRITE: &str = "perf-disk-legend-write";
/// The GPU page charts' headings, range words and legends (`gpu::memory_labels`,
/// `gpu::temperature_labels`, `gpu::power_labels`, `gpu::clocks_labels`).
const KEY_GPU_MEMORY: &str = "perf-gpu-heading-memory";
const KEY_GPU_MEMORY_RANGE: &str = "perf-gpu-range-memory";
const KEY_GPU_DEDICATED: &str = "perf-gpu-legend-dedicated";
const KEY_GPU_SHARED: &str = "perf-gpu-legend-shared";
const KEY_GPU_TEMPERATURE: &str = "perf-gpu-heading-temperature";
const KEY_GPU_POWER: &str = "perf-gpu-heading-power";
const KEY_GPU_POWER_RANGE: &str = "perf-gpu-row-power";
const KEY_GPU_CLOCKS: &str = "perf-gpu-heading-clocks";
const KEY_GPU_CLOCKS_RANGE: &str = "perf-gpu-row-clock";
const KEY_GPU_CLOCK: &str = "perf-gpu-legend-gpu-clock";
const KEY_GPU_MEM_CLOCK: &str = "perf-gpu-legend-mem-clock";
/// The Memory page charts' headings, range words and legend (`memory::physical_labels`,
/// `memory::commit_labels`, `memory::swap_labels` of 1.3.0).
const KEY_MEM_PHYSICAL: &str = "perf-mem-heading-physical";
const KEY_MEM_RANGE: &str = "perf-mem-range-memory";
const KEY_MEM_USED: &str = "perf-mem-legend-used";
const KEY_MEM_COMMITTED: &str = "perf-mem-legend-committed";
const KEY_MEM_COMMIT: &str = "perf-mem-heading-commit";
const KEY_MEM_SWAP: &str = "perf-mem-heading-swap";
const KEY_MEM_SWAP_RANGE: &str = "perf-mem-range-swap";

/// What a page style Cell's chart shows besides the plot: the widget's name, which of the name
/// and the value its forms keep, and whether the value is the sum of the chart's lines.
#[derive(Clone, Debug, PartialEq)]
pub struct PageChart {
    pub name: SharedString,
    /// The name and value forms the widget keeps (`FormSet::LABEL`, `FormSet::VALUE`).
    pub forms: FormSet,
    /// The value is the sum of every line's newest sample (`ValueText::Sum`).
    pub sum: bool,
}

/// The names of the composition chart's bands, in stacking order (`mem_box::comp_labels`).
const KEY_BANDS: [&str; 3] = [
    "perf-overview-mem-used",
    "perf-overview-mem-cached",
    "perf-overview-mem-available",
];

/// The line and fill colour of a widget's chart and bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accent {
    pub line: Hsla,
    pub fill: Hsla,
}

/// The Memory box row of `metric`: In use, Cached, Available, Swap, Commit.
fn mem_row(metric: &str) -> Option<usize> {
    [
        METRIC_MEM_USED,
        METRIC_MEM_CACHED,
        METRIC_MEM_AVAILABLE,
        METRIC_MEM_SWAP,
        METRIC_MEM_COMMIT,
    ]
    .iter()
    .position(|k| *k == metric)
}

/// The accent of `metric` on a device of `kind`: the box's device colour, and for a Memory row
/// that row's colour with its fill at 30%, as the Memory box's sparklines.
pub fn accent(kind: DeviceKind, metric: &str, theme: &TlTheme) -> Accent {
    let g = &theme.graph;
    let (line, fill) = match kind {
        DeviceKind::Cpu | DeviceKind::CpuLogical => (g.cpu_line, g.cpu_fill),
        DeviceKind::Memory => match mem_row(metric) {
            Some(i) if i > 0 => {
                let line = row_colors(theme)[i];
                (
                    line,
                    Hsla {
                        a: line.a * 0.3,
                        ..line
                    },
                )
            }
            _ => (g.mem_line, g.mem_fill),
        },
        DeviceKind::Disk | DeviceKind::Disks => (g.disk_line, g.disk_fill),
        DeviceKind::Net | DeviceKind::Nets => (g.net_line, g.net_fill),
        DeviceKind::Gpu
        | DeviceKind::AggregateGpu
        | DeviceKind::Fan
        | DeviceKind::Battery
        | DeviceKind::Other => (g.gpu_line, g.gpu_fill),
    };
    Accent { line, fill }
}

/// The upload line: the network colour blended with white, as the Network box draws it.
fn upload_line(theme: &TlTheme) -> Hsla {
    blend_over(theme.graph.net_line, white().opacity(UPLOAD_TINT))
}

/// What a widget's chart is built from: the source's metric and series, and the widget's
/// options.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartLook {
    pub kind: DeviceKind,
    pub metric: Arc<str>,
    pub lines: Lines,
    pub smoothed: bool,
    pub options: WidgetOptions,
    /// `performance.cpu_kernel`: the CPU `total` chart draws the kernel line under it.
    pub kernel: bool,
    /// The series the source draws, in order (`SourceData::series_list`).
    pub series: SmallVec<[SeriesId; 4]>,
    /// The series the network aggregate takes off its lines (`SourceData::minus_list`).
    pub minus: SmallVec<[SeriesId; 5]>,
    /// The widget is in a device page container: a Memory chart draws in the memory colour.
    pub page: bool,
}

impl ChartLook {
    fn is_net(&self) -> bool {
        matches!(self.kind, DeviceKind::Net | DeviceKind::Nets)
    }

    /// The CPU `total` chart with its kernel line.
    fn kernel_line(&self) -> bool {
        self.kernel && self.kind == DeviceKind::Cpu && &*self.metric == METRIC_CPU_TOTAL
    }

    /// The hidden kinds' series to take off `line`: the ones of its metric.
    fn minus_of(&self, line: SeriesId) -> Arc<[SeriesId]> {
        self.minus
            .iter()
            .copied()
            .filter(|m| m.metric == line.metric)
            .collect()
    }

    /// One line of the chart in `accent`, filled when the Fill option is on.
    fn line(&self, id: SeriesId, accent: Accent, filled: bool) -> ChartSeries {
        ChartSeries {
            id,
            minus: self.minus_of(id),
            fill: (filled && self.options.fill).then_some(accent.fill),
            line: accent.line,
            dashed: self.options.dashed,
            mean_ns: if self.smoothed { SPARK_MEAN_NS } else { 0 },
            ..ChartSeries::default()
        }
    }

    /// Network upload: the lighter line, no fill, as the Network box's Upload chart.
    fn upload(&self, id: SeriesId, theme: &TlTheme) -> ChartSeries {
        ChartSeries {
            id,
            minus: self.minus_of(id),
            line: upload_line(theme),
            dashed: self.options.dashed,
            ..ChartSeries::default()
        }
    }

    /// The chart's series, and whether they stack.
    pub fn series(&self, theme: &TlTheme) -> (SmallVec<[ChartSeries; 4]>, bool) {
        let accent = if self.page && self.kind == DeviceKind::Memory {
            // The 1.3.0 Memory page draws every chart in the memory colour (UI-PRF-026).
            Accent {
                line: theme.graph.mem_line,
                fill: theme.graph.mem_fill,
            }
        } else {
            accent(self.kind, &self.metric, theme)
        };
        let Some(&first) = self.series.first() else {
            return (SmallVec::new(), false);
        };
        match self.lines {
            Lines::NoSeries => (SmallVec::new(), false),
            Lines::One => {
                let upload = self.is_net() && &*self.metric == METRIC_NET_TX;
                let mut out: SmallVec<[ChartSeries; 4]> = if upload {
                    smallvec![self.upload(first, theme)]
                } else {
                    smallvec![self.line(first, accent, true)]
                };
                if self.kernel_line() {
                    out.push(ChartSeries {
                        id: SeriesId {
                            device: first.device,
                            metric: MetricId::CPU_KERNEL,
                        },
                        line: theme.graph.kernel_line,
                        ..ChartSeries::default()
                    });
                }
                (out, false)
            }
            Lines::Pair { second_dashed } => {
                let mut out: SmallVec<[ChartSeries; 4]> = smallvec![self.line(first, accent, true)];
                if let Some(&second) = self.series.get(1) {
                    out.push(if self.is_net() {
                        self.upload(second, theme)
                    } else {
                        ChartSeries {
                            dashed: second_dashed,
                            ..self.line(second, accent, false)
                        }
                    });
                }
                (out, false)
            }
            Lines::Stacked => {
                // In use, Cached and Available, as the composition chart's bands.
                let colors = row_colors(theme);
                let out = self
                    .series
                    .iter()
                    .take(3)
                    .enumerate()
                    .map(|(i, &id)| {
                        let line = if i == 0 {
                            theme.graph.mem_line
                        } else {
                            colors[i]
                        };
                        ChartSeries {
                            id,
                            fill: Some(Hsla {
                                a: line.a * if i == 0 { 0.6 } else { 0.4 },
                                ..line
                            }),
                            line,
                            ..ChartSeries::default()
                        }
                    })
                    .collect();
                (out, true)
            }
            Lines::PerCore => {
                let devices: SmallVec<[_; 16]> = self.series.iter().map(|s| s.device).collect();
                (clock_series(&devices, accent.line), false)
            }
            Lines::PerLogical { stacked: false } => {
                // One line per logical CPU at max(1/n, 0.08), no fill (UI-PRF-023).
                let n = self.series.len().max(1) as f32;
                let line = Hsla {
                    a: (1.0 / n).max(OVERLAID_MIN_ALPHA),
                    ..accent.line
                };
                let out = self
                    .series
                    .iter()
                    .map(|&id| ChartSeries {
                        id,
                        line,
                        ..ChartSeries::default()
                    })
                    .collect();
                (out, false)
            }
            Lines::PerLogical { stacked: true } => {
                let out = self
                    .series
                    .iter()
                    .enumerate()
                    .map(|(k, &id)| ChartSeries {
                        id,
                        fill: Some(Hsla {
                            a: STACKED_ALPHAS[k % 2],
                            ..accent.fill
                        }),
                        line: accent.line,
                        ..ChartSeries::default()
                    })
                    .collect();
                (out, true)
            }
            Lines::Physical => {
                // In use filled, committed dashed, modified as a faint 1 px line (UI-PRF-026).
                let mut out: SmallVec<[ChartSeries; 4]> = smallvec![self.line(first, accent, true)];
                if let Some(&commit) = self.series.get(1) {
                    out.push(ChartSeries {
                        dashed: true,
                        ..self.line(commit, accent, false)
                    });
                }
                if let Some(&dirty) = self.series.get(2) {
                    out.push(ChartSeries {
                        line: Hsla {
                            a: accent.line.a * DIRTY_ALPHA,
                            ..accent.line
                        },
                        line_width: px(1.),
                        ..self.line(dirty, accent, false)
                    });
                }
                (out, false)
            }
            Lines::RpmPwm => {
                // RPM filled and PWM dashed (UI-PRF-034). The fan page bead gives PWM its own
                // 0 to 100 axis.
                let mut out: SmallVec<[ChartSeries; 4]> = smallvec![self.line(first, accent, true)];
                if let Some(&pwm) = self.series.get(1) {
                    out.push(ChartSeries {
                        dashed: true,
                        ..self.line(pwm, accent, false)
                    });
                }
                (out, false)
            }
        }
    }

    /// The chart's labels: a sparkline has none (`parts::spark_labels`). A chart of a Cell has
    /// the legend and the range word of the box chart it stands for.
    pub fn labels(&self, format: ValueFormat, cell: bool, cx: &App) -> ChartLabels {
        let mut labels = spark_labels(format);
        if !cell {
            return labels;
        }
        match self.kind {
            DeviceKind::Cpu if &*self.metric == METRIC_CPU_TOTAL => {
                labels.range_word = tr(cx, KEY_CPU_RANGE);
                labels.legend = smallvec![(tr(cx, KEY_LEGEND_TOTAL), LegendStyle::Solid)];
                if self.kernel {
                    labels
                        .legend
                        .push((tr(cx, KEY_LEGEND_KERNEL), LegendStyle::Dashed));
                }
            }
            DeviceKind::Memory if &*self.metric == METRIC_MEM_COMPOSITION => {
                labels.legend = KEY_BANDS
                    .iter()
                    .map(|key| (tr(cx, key), LegendStyle::Solid))
                    .collect();
            }
            DeviceKind::Net | DeviceKind::Nets => {
                labels.range_word = tr(cx, KEY_NET_RANGE);
            }
            _ => {}
        }
        labels
    }

    /// The labels of the chart of a page style Cell (ADR-019 phase 3): the 1.3.0 page chart's
    /// heading, range word and legend for a metric that has one, else the name as the heading
    /// and the range word, and the box chart's legend. The heading shows with the Name form and
    /// the value, at the heading's right, with the Value form.
    pub fn page_labels(&self, format: ValueFormat, page: &PageChart, cx: &App) -> ChartLabels {
        let mut labels = self.labels(format, true, cx);
        let disk = matches!(self.kind, DeviceKind::Disk | DeviceKind::Disks);
        let gpu = self.kind == DeviceKind::Gpu;
        let cpu = self.kind == DeviceKind::Cpu;
        let mem = self.kind == DeviceKind::Memory;
        let heading = match &*self.metric {
            // The 1.3.0 CPU page's Overall chart: Utilization, Total and Kernel, no axis text.
            METRIC_CPU_TOTAL if cpu => {
                labels.show_axis_max = false;
                tr(cx, KEY_CPU_RANGE)
            }
            // The all-cores charts are Utilization charts too; the stacked one shows its axis
            // max ("1600%").
            METRIC_CORES_OVERLAID | METRIC_CORES_STACKED if cpu => {
                let word = tr(cx, KEY_CPU_RANGE);
                labels.range_word = word.clone();
                labels.legend = SmallVec::new();
                labels.show_axis_max = &*self.metric == METRIC_CORES_STACKED;
                word
            }
            // The secondary charts (UI-PRF-025): the name as heading and range word, the axis
            // max shown.
            METRIC_POWER_DRAW | METRIC_TEMP | METRIC_CLOCK | METRIC_CORE_CLOCKS if cpu => {
                labels.range_word = page.name.clone();
                labels.legend = SmallVec::new();
                labels.show_axis_max = true;
                page.name.clone()
            }
            METRIC_GPU_MEMORY if gpu => {
                labels.range_word = tr(cx, KEY_GPU_MEMORY_RANGE);
                labels.legend = smallvec![(tr(cx, KEY_GPU_DEDICATED), LegendStyle::Solid)];
                // The shared line and its legend only while the GPU reports shared memory.
                if self.series.len() > 1 {
                    labels
                        .legend
                        .push((tr(cx, KEY_GPU_SHARED), LegendStyle::Dashed));
                }
                labels.show_axis_max = false;
                tr(cx, KEY_GPU_MEMORY)
            }
            METRIC_TEMP if gpu => {
                let word = tr(cx, KEY_GPU_TEMPERATURE);
                labels.range_word = word.clone();
                labels.show_axis_max = true;
                word
            }
            METRIC_POWER if gpu => {
                labels.range_word = tr(cx, KEY_GPU_POWER_RANGE);
                labels.show_axis_max = true;
                tr(cx, KEY_GPU_POWER)
            }
            METRIC_GPU_CLOCKS if gpu => {
                labels.range_word = tr(cx, KEY_GPU_CLOCKS_RANGE);
                labels.legend = smallvec![
                    (tr(cx, KEY_GPU_CLOCK), LegendStyle::Solid),
                    (tr(cx, KEY_GPU_MEM_CLOCK), LegendStyle::Dashed),
                ];
                labels.show_axis_max = false;
                tr(cx, KEY_GPU_CLOCKS)
            }
            // The 1.3.0 Memory page charts (UI-PRF-026): Physical memory with In use and
            // Committed, Commit with its axis text above 100%, Swap.
            METRIC_PHYSICAL if mem => {
                labels.range_word = tr(cx, KEY_MEM_RANGE);
                // The modified line has no legend entry.
                labels.legend = smallvec![
                    (tr(cx, KEY_MEM_USED), LegendStyle::Solid),
                    (tr(cx, KEY_MEM_COMMITTED), LegendStyle::Dashed),
                ];
                labels.show_axis_max = false;
                tr(cx, KEY_MEM_PHYSICAL)
            }
            METRIC_COMMIT_PCT if mem => {
                let word = tr(cx, KEY_MEM_COMMIT);
                labels.range_word = word.clone();
                labels.legend = SmallVec::new();
                labels.show_axis_max = true;
                word
            }
            METRIC_MEM_SWAP if mem => {
                labels.range_word = tr(cx, KEY_MEM_SWAP_RANGE);
                labels.legend = SmallVec::new();
                labels.show_axis_max = false;
                tr(cx, KEY_MEM_SWAP)
            }
            METRIC_DISK_ACTIVE if disk => {
                let word = tr(cx, KEY_DISK_ACTIVE);
                labels.range_word = word.clone();
                word
            }
            METRIC_DISK_IO if disk => {
                labels.range_word = tr(cx, KEY_DISK_THROUGHPUT);
                labels.legend = smallvec![
                    (tr(cx, KEY_DISK_READ), LegendStyle::Solid),
                    (tr(cx, KEY_DISK_WRITE), LegendStyle::Dashed),
                ];
                labels.show_axis_max = true;
                tr(cx, KEY_DISK_TRANSFER)
            }
            _ => {
                if labels.range_word.is_empty() {
                    labels.range_word = page.name.clone();
                }
                page.name.clone()
            }
        };
        labels.heading = page.forms.contains(FormSet::LABEL).then_some(heading);
        labels.value_align = if page.forms.contains(FormSet::VALUE) {
            ValueAlign::Right
        } else {
            ValueAlign::Hidden
        };
        labels
    }
}
