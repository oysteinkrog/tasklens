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
//! Pure functions: no drawing, no state.

use std::sync::Arc;

use gpui_kit::{App, Hsla, white};
use smallvec::{SmallVec, smallvec};
use tl_model::ids::{MetricId, SeriesId};
use tl_model::layout::WidgetOptions;
use tl_model::layout_boxes::{
    DeviceKind, METRIC_CPU_TOTAL, METRIC_MEM_AVAILABLE, METRIC_MEM_CACHED, METRIC_MEM_COMMIT,
    METRIC_MEM_COMPOSITION, METRIC_MEM_SWAP, METRIC_MEM_USED, METRIC_NET_TX,
};

use super::meta::Lines;
use crate::chart::{ChartLabels, ChartSeries, LegendStyle, ValueFormat};
use crate::i18n::tr;
use crate::pages::performance::core_clocks::clock_series;
use crate::pages::performance::overview::cpu_box::SPARK_MEAN_NS;
use crate::pages::performance::overview::mem_box::row_colors;
use crate::pages::performance::overview::net_box::UPLOAD_TINT;
use crate::pages::performance::overview::parts::spark_labels;
use crate::theme::TlTheme;
use crate::theme::derived::blend_over;

/// The CPU box's range word and legend (`cpu_box::total_labels`).
const KEY_CPU_RANGE: &str = "perf-cpu-heading-utilization";
const KEY_LEGEND_TOTAL: &str = "perf-legend-total";
const KEY_LEGEND_KERNEL: &str = "perf-legend-kernel";
/// The Network box's range word (`net_box::chart_labels`).
const KEY_NET_RANGE: &str = "perf-net-heading-throughput";
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
        DeviceKind::Gpu | DeviceKind::Other => (g.gpu_line, g.gpu_fill),
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
        let accent = accent(self.kind, &self.metric, theme);
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
}
