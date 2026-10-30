//! The Core clocks chart (tasklens-o55.57): one line per physical core, the core's
//! `CPU_CORE_CLOCK` series (the fastest of its SMT siblings), on a 0 to highest-core-maximum
//! scale, with a dashed line at the base clock, a ring at each core's peak in the range and a
//! hover readout of every core, fastest first, with its peak. Hovering a core row highlights
//! that core's line.
//!
//! The Overview CPU box and the CPU page's secondary graph both draw it. The owner gives the
//! chart's range, grid and background ([`CoreClocks::set_base`]); this module keeps the lines in
//! step with the topology, the catalog and the inventory.

use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{App, Entity, Hsla, SharedString};
use smallvec::SmallVec;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::snapshot::Snapshot;
use tl_model::system::CpuTopology;
use tl_model::validity::Validity;

use super::cpu::logical_devices;
use crate::chart::{
    ChartLabels, ChartSeries, ChartSpec, ChartView, LineSet, ValueAlign, ValueFormat, YScale,
};
use crate::i18n::{tr, tr_args};
use crate::theme::{TlTheme, current};

/// Alpha of each core's line: 16 lines that cross stay apart.
pub const LINE_ALPHA: f32 = 0.6;

/// "Core clocks".
pub const KEY_HEADING: &str = "perf-cpu-secondary-clocks";
/// "Core {$n}".
const KEY_CORE: &str = "perf-core-clocks-core";
/// "Peak".
const KEY_PEAK: &str = "perf-core-clocks-peak";

/// The lines of the chart: one per physical core that has a catalog entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoreLines {
    /// The device of each line: the core's lowest online CPU, where the engine puts the series.
    pub devices: Vec<DeviceIdx>,
    /// `CoreInfo.id` of each line.
    pub ids: Vec<u16>,
    /// The line of each logical CPU id, `None` for a CPU of no line.
    pub line_of_cpu: Vec<Option<usize>>,
}

/// One line per core of `topology` whose first CPU has a device in `logical`, in core order.
pub fn core_lines(topology: &CpuTopology, logical: &[DeviceIdx]) -> CoreLines {
    let mut out = CoreLines::default();
    for core in topology.cores.iter() {
        let Some(&head) = core.logical.first() else {
            continue;
        };
        let Some(&device) = logical.get(usize::from(head)).filter(|d| d.0 != u16::MAX) else {
            continue;
        };
        let line = out.devices.len();
        out.devices.push(device);
        out.ids.push(core.id);
        for &cpu in &core.logical {
            let cpu = usize::from(cpu);
            if out.line_of_cpu.len() <= cpu {
                out.line_of_cpu.resize(cpu + 1, None);
            }
            out.line_of_cpu[cpu] = Some(line);
        }
    }
    out
}

/// The scale inputs: the highest core maximum and the base clock, in MHz.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClockScale {
    pub max_mhz: Option<f32>,
    pub base_mhz: Option<f32>,
}

impl ClockScale {
    /// The highest `cpuinfo_max_freq` over the cores, else the processor's maximum; the base
    /// clock from the inventory, else from the CPU totals.
    pub fn read(snap: &Snapshot) -> Self {
        let p = &snap.inventory.processor;
        let valid = |v: Validity| v == Validity::Valid;
        let core_max = valid(p.core_max_mhz.validity)
            .then(|| p.core_max_mhz.value.iter().flatten().copied().max())
            .flatten();
        let max = core_max.or_else(|| valid(p.max_mhz.validity).then_some(p.max_mhz.value));
        let base = valid(p.base_mhz.validity)
            .then_some(p.base_mhz.value)
            .or_else(|| {
                let b = &snap.system.cpu.base_mhz;
                matches!(b.validity, Validity::Valid | Validity::Stale).then_some(b.value)
            });
        let mhz = |v: u32| (v > 0).then_some(v as f32);
        Self {
            max_mhz: max.and_then(mhz),
            base_mhz: base.and_then(mhz),
        }
    }

    /// 0 to the highest core maximum; sticky up from 0 when no maximum is known.
    pub fn y(&self) -> YScale {
        match self.max_mhz {
            Some(max) => YScale::Fixed { min: 0.0, max },
            None => YScale::StickyUp { hw_max: None },
        }
    }
}

/// One `CPU_CORE_CLOCK` line per entry of `devices`, no fill, `line` at [`LINE_ALPHA`].
pub fn clock_series(devices: &[DeviceIdx], line: Hsla) -> SmallVec<[ChartSeries; 4]> {
    let line = line.alpha(LINE_ALPHA);
    devices
        .iter()
        .map(|&device| ChartSeries {
            id: SeriesId {
                device,
                metric: MetricId::CPU_CORE_CLOCK,
            },
            fill: None,
            line,
            ..ChartSeries::default()
        })
        .collect()
}

/// "Core clocks" as the heading, no value: one core's clock is not the chart's value. With
/// `range_caption` the range label says "Core clocks" too; the Overview charts have none.
pub fn clocks_labels(range_caption: bool, cx: &App) -> ChartLabels {
    let heading = tr(cx, KEY_HEADING);
    ChartLabels {
        heading: Some(heading.clone()),
        value_fmt: ValueFormat::Mhz,
        value_align: ValueAlign::Hidden,
        range_word: if range_caption {
            heading
        } else {
            SharedString::default()
        },
        legend: SmallVec::new(),
        show_axis_max: true,
    }
}

/// The Core clocks chart of one owner and what its lines are.
pub struct CoreClocks {
    view: Entity<ChartView>,
    /// The owner's range, grid, background and hover; the series and scale are set here.
    base: ChartSpec,
    lines: CoreLines,
    scale: ClockScale,
    topology: Arc<CpuTopology>,
    catalog_version: u64,
    range_caption: bool,
}

impl CoreClocks {
    /// Takes `view` and gives it the lines of `snap` on top of `base`. `range_caption` as in
    /// [`clocks_labels`].
    pub fn new(
        view: Entity<ChartView>,
        base: ChartSpec,
        range_caption: bool,
        snap: &Snapshot,
        cx: &mut App,
    ) -> Self {
        let mut this = Self {
            view,
            base,
            lines: CoreLines::default(),
            scale: ClockScale::default(),
            topology: Arc::default(),
            catalog_version: u64::MAX,
            range_caption,
        };
        this.sync(snap, cx);
        this.apply(cx);
        this
    }

    pub fn view(&self) -> &Entity<ChartView> {
        &self.view
    }

    pub fn lines(&self) -> &CoreLines {
        &self.lines
    }

    pub fn scale(&self) -> ClockScale {
        self.scale
    }

    /// Takes new lines or a new scale from `snap`. True when it did.
    pub fn sync(&mut self, snap: &Snapshot, cx: &mut App) -> bool {
        let scale = ClockScale::read(snap);
        let topology_moved = !Arc::ptr_eq(&self.topology, &snap.system.cpu_topology);
        if !topology_moved && snap.devices.version == self.catalog_version && scale == self.scale {
            return false;
        }
        self.topology = Arc::clone(&snap.system.cpu_topology);
        self.catalog_version = snap.devices.version;
        let mut logical = Vec::new();
        logical_devices(&snap.devices, &mut logical);
        let lines = core_lines(&self.topology, &logical);
        if lines == self.lines && scale == self.scale {
            return false;
        }
        if lines != self.lines {
            self.view.update(cx, |v, cx| v.set_highlight(None, cx));
        }
        self.lines = lines;
        self.scale = scale;
        self.apply(cx);
        true
    }

    /// The owner's range, grid, background or hover changed.
    pub fn set_base(&mut self, base: ChartSpec, cx: &mut App) {
        self.base = base;
        self.apply(cx);
    }

    /// Sets the spec, labels and line set again: new lines, theme or words.
    pub fn apply(&self, cx: &mut App) {
        let theme: Arc<TlTheme> = current(cx).clone();
        let spec = ChartSpec {
            series: clock_series(&self.lines.devices, theme.graph.cpu_line),
            y: self.scale.y(),
            stack: false,
            ..self.base.clone()
        };
        let names: Arc<[SharedString]> = self
            .lines
            .ids
            .iter()
            .map(|&id| tr_args(cx, KEY_CORE, &[("n", FluentValue::from(id))]))
            .collect();
        let set = LineSet {
            names,
            peak_word: tr(cx, KEY_PEAK),
            reference: self.scale.base_mhz,
        };
        let labels = clocks_labels(self.range_caption, cx);
        self.view.update(cx, |v, cx| {
            v.set_spec(spec, cx);
            v.set_labels(labels, cx);
            v.set_line_set(Some(set), cx);
        });
    }

    /// Highlights the line of logical CPU `cpu`, or none.
    pub fn highlight_cpu(&self, cpu: Option<usize>, cx: &mut App) {
        let line = cpu.and_then(|c| self.lines.line_of_cpu.get(c).copied().flatten());
        self.view.update(cx, |v, cx| v.set_highlight(line, cx));
    }
}
