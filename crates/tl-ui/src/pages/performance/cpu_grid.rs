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
//! The headings, tooltips and legend words are `perf-cpu-*` and `perf-legend-*` keys in
//! `en-US.ftl`, translated once per strings version into [`GridWords`].

use std::cell::Cell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{
    AnyElement, App, AppContext as _, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, SharedString, StyleRefinement, Styled as _, Window, canvas, div,
    px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::system::{CoreKind, CpuTopology};
use tl_model::validity::Validity;

use crate::chart::grid_element::{
    CellMark, ChartGridSpec, ChartGridView, GridCellSpec, grid_height, grid_layout,
};
use crate::chart::labels::LABEL_ROW;
use crate::chart::{LegendStyle, ValueFormat, YScale};
use crate::components::icons::AppIcon;
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::hub::SnapshotHub;
use crate::i18n::{Strings, tr, tr_en};
use crate::theme::{TlTheme, metrics};

use super::details::Template;

/// Debug selector of the box around the grid.
pub const GRID_ID: &str = "perf-cpu-grid";
/// Debug selector of the tooltip trigger over the hovered heading.
pub const TRIGGER_ID: &str = "perf-cpu-grid-tooltip";

/// `grid_cell_min_w`, × ui_scale.
const CELL_MIN_W: f32 = 120.;
/// `grid_cell_h`, × ui_scale.
const CELL_H: f32 = 112.;

const KEY_CELL: &str = "perf-cpu-cell";
const KEY_TIP: &str = "perf-cpu-cell-tooltip";
const KEY_TIP_NO_CLOCK: &str = "perf-cpu-cell-tooltip-noclock";
const KEY_TOTAL: &str = "perf-legend-total";
const KEY_KERNEL: &str = "perf-legend-kernel";
const KEY_PREFERRED: &str = "perf-cpu-tip-preferred";

/// Width used for the height before the first paint has measured the box: wide enough for the
/// full column count, which is the usual case.
const UNMEASURED_W: f32 = 1.0e6;

/// The grid's words, translated once per strings version.
#[derive(Clone, Debug, PartialEq)]
pub struct GridWords {
    /// The strings version, `None` without strings.
    version: Option<u64>,
    /// "CPU {$n}".
    pub cell: Template,
    /// "Core {$core}, thread {$thread}, {$mhz} MHz".
    pub tip: Template,
    /// "Core {$core}, thread {$thread}".
    pub tip_no_clock: Template,
    /// "Preferred core (rank {$rank})".
    pub preferred: Template,
    pub total: SharedString,
    pub kernel: SharedString,
}

impl Default for GridWords {
    /// The en-US words, for a grid built before the strings can be read.
    fn default() -> Self {
        Self {
            version: None,
            cell: Template::en(KEY_CELL, &["n"]),
            tip: Template::en(KEY_TIP, &["core", "thread", "mhz"]),
            tip_no_clock: Template::en(KEY_TIP_NO_CLOCK, &["core", "thread"]),
            preferred: Template::en(KEY_PREFERRED, &["rank"]),
            total: tr_en(KEY_TOTAL, &[]),
            kernel: tr_en(KEY_KERNEL, &[]),
        }
    }
}

impl GridWords {
    /// The words in the current strings; en-US without strings (tests).
    pub fn new(cx: &App) -> Self {
        let version = strings_version(cx);
        if version.is_none() {
            return Self::default();
        }
        Self {
            version,
            cell: Template::new(cx, KEY_CELL, &["n"]),
            tip: Template::new(cx, KEY_TIP, &["core", "thread", "mhz"]),
            tip_no_clock: Template::new(cx, KEY_TIP_NO_CLOCK, &["core", "thread"]),
            preferred: Template::new(cx, KEY_PREFERRED, &["rank"]),
            total: tr(cx, KEY_TOTAL),
            kernel: tr(cx, KEY_KERNEL),
        }
    }
}

fn strings_version(cx: &App) -> Option<u64> {
    cx.try_global::<Strings>().map(|s| s.strings_version)
}

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
    /// The preferred-core rank of each starred CPU, per logical CPU id ([`stars`]).
    stars: Vec<Option<u32>>,
    /// The box width of the last paint; 0 before the first.
    width: Rc<Cell<Pixels>>,
    words: GridWords,
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
        let words = GridWords::new(cx);
        let m = metrics(cx);
        let devices = logical_devices.to_vec();
        let clocks = vec![None; devices.len()];
        let (mut places, mut scratch, mut cells) = (Vec::new(), String::new(), Vec::new());
        build_cells(
            &words,
            &topology,
            &devices,
            kernel,
            &clocks,
            &[],
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
            legend: legend(kernel, &words),
            value_fmt: ValueFormat::Percent,
            hover: true,
        };
        Self {
            view: cx.new(|cx| {
                let snap = Arc::clone(hub.read(cx).current());
                ChartGridView::new(spec, hub, cx).with_device_history(&snap)
            }),
            cores: topology,
            kernel,
            scratch,
            devices,
            places,
            clocks,
            stars: Vec::new(),
            width: Rc::new(Cell::new(px(0.))),
            words,
        }
    }

    pub fn words(&self) -> &GridWords {
        &self.words
    }

    /// Translates the words when the strings changed since they were last read, and rebuilds
    /// the headings, tooltips and legend with them. True when it did.
    pub fn refresh_strings(&mut self, cx: &mut App) -> bool {
        if self.words.version == strings_version(cx) {
            return false;
        }
        self.words = GridWords::new(cx);
        let mut spec = self.view.read(cx).spec.clone();
        build_cells(
            &self.words,
            &self.cores,
            &self.devices,
            self.kernel,
            &self.clocks,
            &self.stars,
            &mut self.places,
            &mut self.scratch,
            &mut spec.cells,
        );
        spec.legend = legend(self.kernel, &self.words);
        self.view.update(cx, |v, cx| v.set_spec(spec, cx));
        true
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
        spec.legend = legend(on, &self.words);
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
            &self.words,
            &self.cores,
            &self.devices,
            self.kernel,
            &self.clocks,
            &self.stars,
            &mut self.places,
            &mut self.scratch,
            &mut spec.cells,
        );
        self.view.update(cx, |v, cx| v.set_spec(spec, cx));
    }

    /// Takes the preferred-core ranking per logical CPU id (`amd_pstate_prefcore_ranking`,
    /// PERF-031). Rebuilds the cells only when the starred CPUs changed.
    pub fn set_prefcore(&mut self, ranks: &[Option<u32>], cx: &mut App) {
        let mut new = Vec::new();
        stars(ranks, &mut new);
        if new == self.stars {
            return;
        }
        self.stars = new;
        let mut spec = self.view.read(cx).spec.clone();
        build_cells(
            &self.words,
            &self.cores,
            &self.devices,
            self.kernel,
            &self.clocks,
            &self.stars,
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
            stars,
            scratch,
            words,
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
                let star = stars.get(i).copied().flatten();
                cell_tooltip(words, *core, *thread, now, star, scratch);
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
    words: &GridWords,
    topology: &CpuTopology,
    logical_devices: &[DeviceIdx],
    kernel: bool,
    out: &mut Vec<GridCellSpec>,
) {
    cell_specs_ranked(words, topology, logical_devices, kernel, &[], out);
}

/// [`cell_specs`] with the preferred-core ranking per logical CPU id.
pub fn cell_specs_ranked(
    words: &GridWords,
    topology: &CpuTopology,
    logical_devices: &[DeviceIdx],
    kernel: bool,
    ranks: &[Option<u32>],
    out: &mut Vec<GridCellSpec>,
) {
    let (mut places, mut scratch, mut starred) = (Vec::new(), String::new(), Vec::new());
    stars(ranks, &mut starred);
    build_cells(
        words,
        topology,
        logical_devices,
        kernel,
        &[],
        &starred,
        &mut places,
        &mut scratch,
        out,
    );
}

/// The rank of each CPU whose rank is the highest, per logical CPU id, when the ranking differs
/// between CPUs; empty otherwise (a missing ranking file gives no stars).
fn stars(ranks: &[Option<u32>], out: &mut Vec<Option<u32>>) {
    out.clear();
    let known = ranks.iter().flatten();
    let (Some(&max), Some(&min)) = (known.clone().max(), known.min()) else {
        return;
    };
    if max == min {
        return;
    }
    out.extend(ranks.iter().map(|r| r.filter(|&r| r == max)));
}

/// [`cell_specs`] with the clock and the preferred-core star per logical CPU id (missing ids
/// have none). Refills `places`.
#[allow(clippy::too_many_arguments)]
fn build_cells(
    words: &GridWords,
    topology: &CpuTopology,
    devices: &[DeviceIdx],
    kernel: bool,
    clocks: &[Option<u32>],
    stars: &[Option<u32>],
    places_out: &mut Vec<Option<(u16, u16)>>,
    scratch: &mut String,
    out: &mut Vec<GridCellSpec>,
) {
    places(topology, devices.len(), places_out);
    let has = |k| topology.cores.iter().any(|c| c.kind == k);
    let hybrid = has(CoreKind::Performance) && has(CoreKind::Efficiency);
    out.clear();
    for (id, &device) in devices.iter().enumerate() {
        let star = stars.get(id).copied().flatten();
        let tooltip = places_out[id].map(|(core, thread)| {
            let mhz = clocks.get(id).copied().flatten();
            cell_tooltip(words, core, thread, mhz, star, scratch);
            SharedString::from(scratch.as_str())
        });
        let kind = || {
            let cpu = u16::try_from(id).ok()?;
            let core = topology.cores.iter().find(|c| c.logical.contains(&cpu))?;
            Some(core.kind)
        };
        let mark = match (star, hybrid.then(kind).flatten()) {
            (Some(_), _) => Some(CellMark::Kit(IconName::Star)),
            (None, Some(CoreKind::Performance)) => Some(CellMark::App(AppIcon::CpuPCore)),
            (None, Some(CoreKind::Efficiency)) => Some(CellMark::App(AppIcon::CpuECore)),
            _ => None,
        };
        scratch.clear();
        words.cell.write_each(scratch, |_, o| {
            let _ = write!(o, "{id}");
        });
        let heading = SharedString::from(scratch.as_str());
        out.push(cell(topology, id, device, kernel, heading, tooltip, mark));
    }
}

/// The heading tooltip, with " · Preferred core (rank 236)" after it on a starred CPU
/// (UI-PRF-023: the grid has one tooltip per heading). Replaces `out`.
fn cell_tooltip(
    words: &GridWords,
    core: u16,
    thread: u16,
    mhz: Option<u32>,
    star: Option<u32>,
    out: &mut String,
) {
    heading_tooltip(words, core, thread, mhz, out);
    if let Some(rank) = star {
        out.push_str(" · ");
        words.preferred.write_each(out, |_, o| {
            let _ = write!(o, "{rank}");
        });
    }
}

/// "Core 3, thread 1, 4200 MHz"; without a clock "Core 3, thread 1". Replaces `out`.
pub fn heading_tooltip(
    words: &GridWords,
    core: u16,
    thread: u16,
    mhz: Option<u32>,
    out: &mut String,
) {
    out.clear();
    let template = if mhz.is_some() {
        &words.tip
    } else {
        &words.tip_no_clock
    };
    template.write_each(out, |i, o| {
        let _ = match i {
            0 => write!(o, "{core}"),
            1 => write!(o, "{thread}"),
            _ => write!(o, "{}", mhz.unwrap_or_default()),
        };
    });
}

fn cell(
    topology: &CpuTopology,
    id: usize,
    device: DeviceIdx,
    kernel: bool,
    heading: SharedString,
    tooltip: Option<SharedString>,
    mark: Option<CellMark>,
) -> GridCellSpec {
    let online = u16::try_from(id).is_ok_and(|id| topology.online.contains(id));
    GridCellSpec {
        heading,
        fill_series: SeriesId {
            device,
            metric: MetricId::CPU_TOTAL,
        },
        overlay_series: kernel.then_some(SeriesId {
            device,
            metric: MetricId::CPU_KERNEL,
        }),
        offline: !online,
        mark,
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
fn legend(kernel: bool, words: &GridWords) -> SmallVec<[(SharedString, LegendStyle); 2]> {
    let mut legend = smallvec![(words.total.clone(), LegendStyle::Solid)];
    if kernel {
        legend.push((words.kernel.clone(), LegendStyle::Dashed));
    }
    legend
}
