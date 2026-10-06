//! The CPU per-core grid: one `ChartGridView` cell per logical CPU, built from the topology
//! (docs/ui/performance.md UI-PRF-002, UI-PRF-023; PERF-028, PERF-029, PERF-031).
//!
//! Cell specs are built only when the topology or the kernel toggle changes. The 1 Hz clock
//! update re-formats the heading tooltip of a CPU only when its clock changed, and it does not
//! re-render the grid: the tooltip text is read when a tooltip opens.
//!
//! The grid view's own element reports which heading the pointer is over. [`CpuGrid::element`]
//! wraps the cached grid view in a box and places a [`HoverTooltip`] trigger over that heading,
//! so the tooltip opens below the heading after [`DELAY_CONTROL`].
//!
//! The words below match `perf-cpu-*` and `perf-legend-*` in `en-US.ftl`, like the fallback
//! words in `details.rs`.

use std::cell::Cell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, SharedString, StyleRefinement, Styled as _, Window, canvas, div,
    px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::system::CpuTopology;
use tl_model::validity::Validity;

use crate::chart::grid_element::{
    ChartGridSpec, ChartGridView, GridCellSpec, grid_height, grid_layout,
};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{LegendStyle, ValueFormat, YScale};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::hub::SnapshotHub;
use crate::theme::{TlTheme, metrics};

/// Debug selector of the box around the grid.
pub const GRID_ID: &str = "perf-cpu-grid";
/// Debug selector of the tooltip trigger over the hovered heading.
pub const TRIGGER_ID: &str = "perf-cpu-grid-tooltip";

/// `grid_cell_min_w`, × ui_scale.
const CELL_MIN_W: f32 = 120.;
/// `grid_cell_h`, × ui_scale.
const CELL_H: f32 = 112.;

const WORD_CPU: &str = "CPU ";
const WORD_CORE: &str = "Core ";
const WORD_THREAD: &str = ", thread ";
const WORD_MHZ: &str = " MHz";
const WORD_TOTAL: &str = "Total";
const WORD_KERNEL: &str = "Kernel";

/// Width used for the height before the first paint has measured the box: wide enough for the
/// full column count, which is the usual case.
const UNMEASURED_W: f32 = 1.0e6;

/// The per-core grid of the CPU page. The page owns it and calls [`CpuGrid::element`] in its
/// render.
pub struct CpuGrid {
    pub view: Entity<ChartGridView>,
    cores: Arc<CpuTopology>,
    kernel: bool,
    /// Reused tooltip text buffer.
    scratch: String,
    /// `DeviceIdx` per logical CPU id.
    devices: Vec<DeviceIdx>,
    /// (core id, thread index) per logical CPU id; `None` for a CPU the topology does not list.
    places: Vec<Option<(u16, u16)>>,
    /// The MHz each heading tooltip shows, per logical CPU id.
    clocks: Vec<Option<u32>>,
    /// The box width of the last paint; 0 before the first.
    width: Rc<Cell<Pixels>>,
}

impl CpuGrid {
    /// Builds the cell specs from `topology` and creates the grid view on `hub`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hub: &Entity<SnapshotHub>,
        topology: Arc<CpuTopology>,
        logical_devices: &[DeviceIdx],
        kernel: bool,
        range_ns: u64,
        smooth: bool,
        theme: &TlTheme,
        cx: &mut App,
    ) -> Self {
        let m = metrics(cx);
        let devices = logical_devices.to_vec();
        let clocks = vec![None; devices.len()];
        let (mut places, mut scratch, mut cells) = (Vec::new(), String::new(), Vec::new());
        build_cells(
            &topology,
            &devices,
            kernel,
            &clocks,
            &mut places,
            &mut scratch,
            &mut cells,
        );
        let spec = ChartGridSpec {
            cells,
            fill: theme.graph.cpu_fill,
            line: theme.graph.cpu_line,
            overlay_line: theme.graph.kernel_line,
            y: YScale::Fixed {
                min: 0.0,
                max: 100.0,
            },
            range_ns,
            smooth,
            cell_min_w: px(CELL_MIN_W * m.ui_scale),
            cell_h: px(CELL_H * m.ui_scale),
            gap: m.sp[4],
            legend: legend(kernel),
            value_fmt: ValueFormat::Percent,
        };
        Self {
            view: cx.new(|cx| ChartGridView::new(spec, hub, cx)),
            cores: topology,
            kernel,
            scratch,
            devices,
            places,
            clocks,
            width: Rc::new(Cell::new(px(0.))),
        }
    }

    /// Adds or drops the kernel overlay series of every cell and the "Kernel" legend entry.
    pub fn set_kernel(&mut self, on: bool, cx: &mut App) {
        if self.kernel == on {
            return;
        }
        self.kernel = on;
        let mut spec = self.view.read(cx).spec.clone();
        for cell in &mut spec.cells {
            cell.overlay_series = on.then_some(SeriesId {
                device: cell.fill_series.device,
                metric: MetricId::CPU_KERNEL,
            });
        }
        spec.legend = legend(on);
        self.view.update(cx, |v, cx| v.set_spec(spec, cx));
    }

    /// CPU hotplug: rebuilds the cells when the topology or the devices changed. A CPU that went
    /// offline becomes an offline cell; a new CPU gets a new cell.
    pub fn set_topology(
        &mut self,
        topology: Arc<CpuTopology>,
        logical_devices: &[DeviceIdx],
        cx: &mut App,
    ) {
        if *topology == *self.cores && self.devices == logical_devices {
            return;
        }
        self.cores = topology;
        self.devices.clear();
        self.devices.extend_from_slice(logical_devices);
        self.clocks.resize(logical_devices.len(), None);
        let mut spec = self.view.read(cx).spec.clone();
        build_cells(
            &self.cores,
            &self.devices,
            self.kernel,
            &self.clocks,
            &mut self.places,
            &mut self.scratch,
            &mut spec.cells,
        );
        self.view.update(cx, |v, cx| v.set_spec(spec, cx));
    }

    /// Takes the per-CPU clocks, (logical CPU id, MHz, validity). Re-formats a heading tooltip
    /// only when its MHz changed; a clock that is not Valid drops the MHz part. Does not
    /// re-render the grid.
    pub fn update_clocks(&mut self, per_cpu_mhz: &[(u16, f32, Validity)], cx: &mut App) {
        let Self {
            view,
            places,
            clocks,
            scratch,
            ..
        } = self;
        let changed = |&(cpu, mhz, validity): &(u16, f32, Validity)| {
            let i = usize::from(cpu);
            matches!(places.get(i), Some(Some(_)))
                && clocks.get(i).is_some_and(|c| *c != clock(mhz, validity))
        };
        if !per_cpu_mhz.iter().any(changed) {
            return;
        }
        view.update(cx, |v, _| {
            for &(cpu, mhz, validity) in per_cpu_mhz {
                let i = usize::from(cpu);
                let (Some(Some((core, thread))), Some(slot), Some(cell)) =
                    (places.get(i), clocks.get_mut(i), v.spec.cells.get_mut(i))
                else {
                    continue;
                };
                let now = clock(mhz, validity);
                if *slot == now {
                    continue;
                }
                *slot = now;
                heading_tooltip(*core, *thread, now, scratch);
                cell.tooltip = Some(SharedString::from(scratch.as_str()));
            }
        });
    }

    pub fn set_range(&mut self, range_ns: u64, cx: &mut App) {
        if self.view.read(cx).spec.range_ns == range_ns {
            return;
        }
        let mut spec = self.view.read(cx).spec.clone();
        spec.range_ns = range_ns;
        self.view.update(cx, |v, cx| v.set_spec(spec, cx));
    }

    pub fn set_smooth(&mut self, smooth: bool, cx: &mut App) {
        if self.view.read(cx).spec.smooth == smooth {
            return;
        }
        let mut spec = self.view.read(cx).spec.clone();
        spec.smooth = smooth;
        self.view.update(cx, |v, cx| v.set_spec(spec, cx));
    }

    /// The grid as a cached view in a full-width box whose height follows the measured width,
    /// with the tooltip trigger over the hovered heading.
    pub fn element(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = *metrics(cx);
        let view = self.view.read(cx);
        let spec = &view.spec;
        let dims = Dims {
            n: spec.cells.len(),
            min_w: spec.cell_min_w,
            cell_h: spec.cell_h,
            gap: spec.gap,
        };
        let hovered = view.hovered_cell().zip(view.hovered_tooltip().cloned());
        let width = self.width.get();

        let measure = {
            let cell = Rc::clone(&self.width);
            canvas(
                move |bounds, window, cx| {
                    let old = cell.replace(bounds.size.width);
                    if dims.height(old) != dims.height(bounds.size.width) {
                        cx.notify(window.current_view());
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        let mut root = div()
            .debug_selector(|| GRID_ID.into())
            .relative()
            .w_full()
            .h(dims.height(width))
            .child(
                self.view
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .child(measure);

        if let Some((i, text)) = hovered {
            let layout = grid_layout(dims.n, width, dims.min_w, dims.gap);
            let pad = m.sp[3];
            let (col, row) = (i % layout.cols, i / layout.cols);
            let trigger = div()
                .debug_selector(|| TRIGGER_ID.into())
                .absolute()
                .left((layout.cell_w + dims.gap) * col as f32 + pad)
                .top((dims.cell_h + dims.gap) * row as f32 + pad)
                .w((layout.cell_w - pad * 2.).max(px(0.)))
                .h(px(LABEL_ROW * m.ui_scale));
            // One state per heading, so moving to another heading starts the delay again.
            let state = HoverTooltip::state((TRIGGER_ID, i), window, cx);
            let trigger = HoverTooltip::attach(
                &state,
                trigger,
                DELAY_CONTROL,
                TlTooltip::text(text).build(),
                window,
                cx,
            );
            root = root.child(trigger);
        }
        root.into_any_element()
    }
}

/// The sizes the box height depends on.
#[derive(Clone, Copy)]
struct Dims {
    n: usize,
    min_w: Pixels,
    cell_h: Pixels,
    gap: Pixels,
}

impl Dims {
    /// The grid height for `width`; before the first measure, the height for the full column
    /// count.
    fn height(&self, width: Pixels) -> Pixels {
        let width = if width > px(0.) {
            width
        } else {
            px(UNMEASURED_W)
        };
        grid_height(
            &grid_layout(self.n, width, self.min_w, self.gap),
            self.cell_h,
            self.gap,
        )
    }
}

/// One cell per logical CPU id, ascending: heading "CPU N", the CPU_TOTAL fill, the CPU_KERNEL
/// overlay when `kernel`, offline when the CPU is not online, and the heading tooltip without a
/// clock. `logical_devices[id]` is the `DeviceIdx` of logical CPU `id`.
pub fn cell_specs(
    topology: &CpuTopology,
    logical_devices: &[DeviceIdx],
    kernel: bool,
    out: &mut Vec<GridCellSpec>,
) {
    let (mut places, mut scratch) = (Vec::new(), String::new());
    build_cells(
        topology,
        logical_devices,
        kernel,
        &[],
        &mut places,
        &mut scratch,
        out,
    );
}

/// [`cell_specs`] with the clock per logical CPU id (missing ids have none). Refills `places`.
fn build_cells(
    topology: &CpuTopology,
    devices: &[DeviceIdx],
    kernel: bool,
    clocks: &[Option<u32>],
    places_out: &mut Vec<Option<(u16, u16)>>,
    scratch: &mut String,
    out: &mut Vec<GridCellSpec>,
) {
    places(topology, devices.len(), places_out);
    out.clear();
    for (id, &device) in devices.iter().enumerate() {
        let tooltip = places_out[id].map(|(core, thread)| {
            heading_tooltip(core, thread, clocks.get(id).copied().flatten(), scratch);
            SharedString::from(scratch.as_str())
        });
        out.push(cell(topology, id, device, kernel, tooltip));
    }
}

/// "Core 3, thread 1, 4200 MHz"; without a clock "Core 3, thread 1". Replaces `out`.
pub fn heading_tooltip(core: u16, thread: u16, mhz: Option<u32>, out: &mut String) {
    out.clear();
    let _ = write!(out, "{WORD_CORE}{core}{WORD_THREAD}{thread}");
    if let Some(mhz) = mhz {
        let _ = write!(out, ", {mhz}{WORD_MHZ}");
    }
}

fn cell(
    topology: &CpuTopology,
    id: usize,
    device: DeviceIdx,
    kernel: bool,
    tooltip: Option<SharedString>,
) -> GridCellSpec {
    let online = u16::try_from(id).is_ok_and(|id| topology.online.contains(id));
    GridCellSpec {
        heading: format!("{WORD_CPU}{id}").into(),
        fill_series: SeriesId {
            device,
            metric: MetricId::CPU_TOTAL,
        },
        overlay_series: kernel.then_some(SeriesId {
            device,
            metric: MetricId::CPU_KERNEL,
        }),
        offline: !online,
        mark: None,
        tooltip,
    }
}

/// (core id, index in the core's `logical` list) per logical CPU id below `n`.
fn places(topology: &CpuTopology, n: usize, out: &mut Vec<Option<(u16, u16)>>) {
    out.clear();
    out.resize(n, None);
    for core in topology.cores.iter() {
        for (thread, &cpu) in core.logical.iter().enumerate() {
            if let (Some(slot), Ok(thread)) = (out.get_mut(usize::from(cpu)), u16::try_from(thread))
            {
                *slot = Some((core.id, thread));
            }
        }
    }
}

/// The MHz a tooltip shows: only a Valid, finite, non-negative clock.
fn clock(mhz: f32, validity: Validity) -> Option<u32> {
    (validity == Validity::Valid && mhz.is_finite() && mhz >= 0.0).then(|| mhz.round() as u32)
}

/// "Total" solid, and "Kernel" dashed when the overlay is on. Drawn in cell 0.
fn legend(kernel: bool) -> SmallVec<[(SharedString, LegendStyle); 2]> {
    let mut legend = smallvec![(SharedString::from(WORD_TOTAL), LegendStyle::Solid)];
    if kernel {
        legend.push((SharedString::from(WORD_KERNEL), LegendStyle::Dashed));
    }
    legend
}
