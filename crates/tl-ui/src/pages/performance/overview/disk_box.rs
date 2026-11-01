//! The Overview page's Disk box (UI-PRF-017, D-022, UI-PRF-031, crw.55): one section per shown
//! whole disk, at most 8, then "+n more". A section is a header (name, model and active percent)
//! and, when expanded, a Read row and a Write row, each a sparkline of the disk's series and the
//! current rate.
//!
//! The page hands in the disks of the sidebar's visible cards; partitions never get a section.
//! The box formats its text only when the inputs' hash changes. A disk's sparkline charts are
//! built only when it gets a section, never for a text or catalog change. When the sections do
//! not fit the box height they collapse to their headers, the busiest disks expanded first
//! (`fit`); a resize renders again only when that changes. Pointer movement never notifies.
//!
//! UI-PRF-019 gives the box three modes. `disk_charts` is the default (D-026): a grid of one
//! chart per disk, read and write as two lines. The cells are equal, at least 160 × 60 px, and
//! fill the box; cells that do not fit whole are left out and counted in "+n more"
//! (UI-PRF-059). Its Arrange choice (D-027) puts the
//! cells in one column (the default), one row, or that grid. `rows` is the layout above with a
//! vertical splitter between the label part (headers, words and rates) and the sparkline part.
//! `total_chart` is one chart of the summed read and write rates.
//!
//! D-041, ADR-019 phase 2: the modes are presets of the box's widget layout. A preset is a flow
//! layout (`disk_flow.rs`) with one widget that fills the body: the disks group in `disk_charts`
//! and `rows`, the total `io` chart in `total_chart`. The disks group's `GroupWidget` draws the
//! sections or the cells with a `GroupFlowLook` from the [`DiskModel`] the box keeps (text,
//! charts and "+n more"), and works out which sections fit and expand in its own render, so a
//! sample that changes a rate or an active percent renders only that group. The total chart is
//! a `MetricWidget` with `FlowLook::Chart`. The box draws only the title line and the `rows`
//! splitter. A grid layout (the user's own widgets, after Apply) is drawn by the box's
//! [`WidgetEditor`] and its `WidgetContainer`; so is the box while Edit Widgets is on.

use std::cell::{Cell, Ref, RefCell};
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EntityId, EventEmitter, FontWeight,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, Size, StyleRefinement,
    Styled as _, Subscription, TextStyleRefinement, Window, div, prelude::FluentBuilder as _, px,
    relative, size,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::layout::{ContainerLayout, DISK_BOX_CONTAINER};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::{InvField, Snapshot};
use tl_model::validity::Validity;

use super::super::DeviceKey;
use super::cpu_box::value_text;
use super::disk_flow::{self, DiskFlowKeys, DiskLook, DiskPlan, PlanCell};
use super::fit::{fit_text, whole_rows};
use super::inner_split::{InnerSplit, InnerSplitChanged, SplitAxis};
use super::layout::{Arrange, ArrangeChoice, BoxMode};
use super::mem_box::ROW_LABEL_W;
use super::parts::{
    ChartSettings, ListFit, PLOT_LINE, SPARK_GRID, box_chart, box_frame, chart_section_versions,
    class_probe, edit_chrome, grid_cells_gaps, list_fit_gaps, opens_device, spark_chart,
};
use super::picked::{box_mask, box_snapshot};
use super::range::RangeText;
use super::timeline::OverviewPause;
use crate::chart::labels::LABEL_ROW;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale, format_value,
};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::pages::performance::widget::container::WidgetContainer;
use crate::pages::performance::widget::edit::{WidgetEditEvent, WidgetEditor};
use crate::pages::performance::widget::group_widget::GroupFlowLook;
use crate::pages::performance::widget::metric_widget::FlowLook;
use crate::pages::performance::widget::source::SourceStore;
use crate::selector::TlSelector;
use crate::settings_global::TlSettings;
use crate::text::style::{label_style, numeric_style, t_sm, t_xs};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The box's debug selector.
pub const DISK_BOX_ID: &str = "perf-overview-box-disk";

/// The box's layout modes (UI-PRF-019). `disk_charts` is the default (D-026). `rows` is the
/// disk sections of UI-PRF-017.
///
/// The `rows` split is the label part's share of the body.
pub const MODES: &[BoxMode] = &[
    BoxMode {
        id: "disk_charts",
        label: "perf-overview-mode-disk-charts",
        split: None,
    },
    BoxMode {
        id: "rows",
        label: "perf-overview-mode-rows",
        split: Some(0.4),
    },
    BoxMode {
        id: "total_chart",
        label: "perf-overview-mode-total-chart",
        split: None,
    },
];

/// The modes with an Arrange choice (UI-PRF-019, D-027): `disk_charts` stacks its cells
/// vertically by default.
pub const ARRANGE: &[(&str, ArrangeChoice)] =
    &[("disk_charts", ArrangeChoice::Saved(Arrange::Vertical))];

/// The debug selector of the splitter between the label part and the sparkline part.
pub const DISK_SPLITTER_ID: &str = "perf-overview-disk-splitter";
/// The debug selector of the splitter's accent line.
pub const DISK_SPLITTER_LINE_ID: &str = "perf-overview-disk-splitter-line";
/// The debug selector of the `total_chart` chart.
pub const TOTAL_CHART_ID: &str = "perf-overview-disk-total-chart";
/// The debug selector of "No widgets. Use Edit Widgets to add some." (UI-PRF-061).
pub const EMPTY_ID: &str = "perf-overview-disk-widgets-empty";

/// The smallest `disk_charts` cell, logical px before `ui_scale` (D-026).
pub const CELL_MIN_W: f32 = 160.;
pub const CELL_MIN_H: f32 = 60.;

/// The narrowest `disk_charts` cell whose header fits on one line, logical px. A narrower cell
/// puts the read and write rates on a second header line.
pub const HEAD_ONE_LINE_W: f32 = 320.;

/// The narrowest `disk_charts` cell whose rates keep their first-letter label, logical px. A
/// narrower cell shows the bare values.
pub const HEAD_LABEL_W: f32 = 200.;

/// The width a `disk_charts` cell header keeps for the disk name, logical px.
const HEAD_NAME_MIN_W: f32 = 48.;

/// The debug selector of the `disk_charts` grid viewport.
pub const GRID_ID: &str = "perf-overview-disk-grid";

/// The `rows` parts' minimums: the label part keeps the word column (72 px), the gap and the
/// widest rate ("1023.9 MiB/s", about 88 px), the sparkline part a short sparkline. Logical px
/// before `ui_scale`.
const LABEL_PART_MIN_W: f32 = 168.;
const SPARK_PART_MIN_W: f32 = 64.;

/// Which parts the box shows (UI-PRF-019).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DiskMode {
    Rows,
    DiskCharts,
    TotalChart,
}

impl DiskMode {
    /// The mode `id`; an unknown id is the default.
    fn from_id(id: &str) -> Self {
        match id {
            "rows" => Self::Rows,
            "total_chart" => Self::TotalChart,
            _ => Self::DiskCharts,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::DiskCharts => MODES[0].id,
            Self::Rows => MODES[1].id,
            Self::TotalChart => MODES[2].id,
        }
    }

    fn look(self) -> DiskLook {
        match self {
            Self::DiskCharts => DiskLook::DiskCharts,
            Self::Rows => DiskLook::Rows,
            Self::TotalChart => DiskLook::TotalChart,
        }
    }
}

/// The page saves the share the box ended a drag or a double-click on.
impl EventEmitter<InnerSplitChanged> for DiskBox {}

/// Disks with a section; the rest are counted in the "+n more" line.
pub const MAX_DISK_ROWS: usize = 8;

const KEY_TITLE: &str = "perf-overview-box-disks";
const KEY_MORE: &str = "perf-overview-more";
const KEY_READ: &str = "perf-disk-legend-read";
const KEY_WRITE: &str = "perf-disk-legend-write";
const KEY_EMPTY: &str = "perf-widget-empty";

/// The rate and active percent column, logical px before `ui_scale`.
const RATE_W: f32 = 76.;

/// Disk inputs whose catalog entry is a whole disk. Unknown inputs are retained because the
/// page may update its card list before the matching catalog snapshot reaches this entity.
fn whole_disks<'a>(
    disks: &'a [(DeviceKey, DeviceIdx, SharedString)],
    catalog: &'a DeviceCatalog,
) -> impl Iterator<Item = &'a (DeviceKey, DeviceIdx, SharedString)> {
    disks.iter().filter(|(_, idx, _)| {
        catalog
            .entries
            .iter()
            .find(|entry| entry.idx == *idx)
            .is_none_or(|entry| entry.parent.is_none())
    })
}

/// One disk's section.
#[derive(Clone)]
pub struct DiskRow {
    pub key: DeviceKey,
    pub idx: DeviceIdx,
    pub name: SharedString,
    /// The drive model, empty when not known.
    pub model: SharedString,
    /// Active time ("40%").
    pub active: SharedString,
    pub read: SharedString,
    pub write: SharedString,
    /// Active time as a fraction, `None` when not known. Ranks the sections for expanding.
    pub busy: Option<f32>,
    /// The sparklines of the disk's `DISK_READ` and `DISK_WRITE` series.
    pub read_chart: Entity<ChartView>,
    pub write_chart: Entity<ChartView>,
    /// The read and write lines of `disk_charts`.
    pub pair_chart: Entity<ChartView>,
}

/// What the disk sections and the `disk_charts` cells show, kept by the box and drawn by the
/// disks group's flow look, and what the last render of them showed.
#[derive(Default)]
pub struct DiskModel {
    /// The sections of the first `MAX_DISK_ROWS` whole disks.
    pub rows: SmallVec<[DiskRow; 8]>,
    /// Disks past `MAX_DISK_ROWS`.
    pub more: usize,
    /// "+n more" by the number of sections shown, empty when every disk has a section.
    pub more_texts: SmallVec<[SharedString; 9]>,
    /// The Read and Write words.
    pub words: [SharedString; 2],
    /// How many sections or cells the last render showed.
    pub shown: usize,
    /// Which sections the last render expanded (bit `i` for section `i`).
    pub expanded: u8,
    /// How many `disk_charts` cells the last render showed.
    pub cells: usize,
}

impl DiskModel {
    /// Whether the last render showed section `i` with its Read and Write rows.
    pub fn expanded(&self, i: usize) -> bool {
        i < MAX_DISK_ROWS && self.expanded & (1 << i) != 0
    }

    /// The "+n more" text the last render showed, empty for none.
    pub fn shown_more_text(&self) -> SharedString {
        self.more_texts.get(self.shown).cloned().unwrap_or_default()
    }
}

/// A Read or Write sparkline: filled, auto-scaled IEC rates.
fn io_spec(device: DeviceIdx, metric: MetricId, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    ChartSpec {
        series: smallvec![ChartSeries {
            id: SeriesId { device, metric },
            fill: Some(theme.graph.disk_fill),
            line: theme.graph.disk_line,
            ..ChartSeries::default()
        }],
        y: YScale::AutoRate { iec: true },
        range_ns: s.range_ns,
        grid: SPARK_GRID,
        smooth: s.smooth,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

/// A `disk_charts` cell or the total chart: read filled, write dashed, auto-scaled IEC rates.
fn pair_spec(device: DeviceIdx, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    let line = theme.graph.disk_line;
    ChartSpec {
        series: smallvec![
            ChartSeries {
                id: SeriesId {
                    device,
                    metric: MetricId::DISK_READ,
                },
                fill: Some(theme.graph.disk_fill),
                line,
                ..ChartSeries::default()
            },
            ChartSeries {
                id: SeriesId {
                    device,
                    metric: MetricId::DISK_WRITE,
                },
                line,
                dashed: true,
                ..ChartSeries::default()
            },
        ],
        y: YScale::AutoRate { iec: true },
        range_ns: s.range_ns,
        grid: GridStyle {
            lines: true,
            baseline: false,
            outline: true,
            rounded: s.rounded,
            vertical_at: None,
        },
        smooth: s.smooth,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

/// No heading and no value: the cell header or the box carries the numbers.
fn pair_labels() -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt: ValueFormat::RateIec,
        value_align: ValueAlign::Hidden,
        range_word: SharedString::default(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// The aggregate disks device, whose series sum the physical disks. The fixed entries come
/// first in every catalog, so index 1 stands in until the catalog has been read.
fn aggregate_disks(catalog: &DeviceCatalog) -> DeviceIdx {
    catalog
        .entries
        .iter()
        .find(|e| e.identity == DeviceIdentity::Aggregate(AggregateKind::Disks))
        .map_or(DeviceIdx(1), |e| e.idx)
}

/// The trimmed text of `f` when it is Valid or Stale and not blank.
fn valid_text(f: &InvField<Arc<str>>) -> Option<&str> {
    let ok = matches!(f.validity, Validity::Valid | Validity::Stale) && !f.value.trim().is_empty();
    ok.then(|| f.value.trim())
}

/// The model of disk `idx`: the device details' model, else the inventory drive's, as on the
/// sidebar card.
fn model_of(snap: &Snapshot, idx: DeviceIdx) -> Option<&str> {
    snap.device_details
        .disks
        .iter()
        .find(|(i, _)| *i == idx)
        .and_then(|(_, d)| valid_text(&d.model))
        .or_else(|| {
            snap.inventory
                .disks
                .iter()
                .find(|d| d.device == Some(idx))
                .and_then(|d| valid_text(&d.model))
        })
}

/// What fits the box body: the number of sections shown and how many of them are expanded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Fit {
    shown: usize,
    expanded: usize,
}

impl Fit {
    fn class(self) -> u32 {
        self.shown as u32 | (self.expanded as u32) << 8
    }
}

/// What fits a body of `body` (everything before the first paint) for `rows` sections out of
/// `total` disks. Every line (header, Read, Write, "+n more") is 18 px with 4 px between lines.
/// When the headers and the "+n more" line do not fit, only the headers that fit show, with one
/// line left for "+n more". Otherwise the spare lines expand sections, two lines each.
fn fit(body: Size<Pixels>, rows: usize, total: usize, m: &UiMetrics) -> Fit {
    if body.height == px(0.) {
        return Fit {
            shown: rows,
            expanded: rows,
        };
    }
    let gap = f32::from(m.sp[2]);
    let line = LABEL_ROW * m.ui_scale + gap;
    let lines = ((f32::from(body.height) + gap) / line).floor().max(0.) as usize;
    let more = usize::from(total > rows);
    if rows + more > lines {
        return Fit {
            shown: lines.saturating_sub(1).min(rows),
            expanded: 0,
        };
    }
    Fit {
        shown: rows,
        expanded: rows.min((lines - rows - more) / 2),
    }
}

/// The smallest `disk_charts` cell, in px.
fn cell_min(m: &UiMetrics) -> Size<Pixels> {
    size(px(CELL_MIN_W * m.ui_scale), px(CELL_MIN_H * m.ui_scale))
}

/// The `disk_charts` grid for `rows` cells in `body`, placed as `arrange` says (`list_fit_gaps`).
/// Only whole cells show (UI-PRF-059); a disk past `MAX_DISK_ROWS` or a cell that would be cut
/// is counted in the "+n more" line, which takes its height from the body first. Cells side
/// by side sit 2 px apart; a cell under another starts right under its plot, so the plots get
/// that height (tasklens-o55.54). Before the first paint the body is empty and every disk gets
/// a cell.
fn disk_fit(
    arrange: Arrange,
    body: Size<Pixels>,
    rows: usize,
    total: usize,
    m: &UiMetrics,
) -> ListFit {
    list_fit_gaps(
        arrange,
        rows,
        total,
        body,
        cell_min(m),
        cell_gap(m),
        px(LABEL_ROW * m.ui_scale) + m.sp[2],
    )
}

/// The gap between `disk_charts` cells: 2 px between columns, none between rows.
fn cell_gap(m: &UiMetrics) -> Size<Pixels> {
    size(m.sp[1], m.sp[0])
}

/// The debug selector of a disk's `disk_charts` cell.
pub fn cell_id(key: &DeviceKey) -> String {
    format!("{DISK_BOX_ID}-cell:{}", key.0)
}

/// The debug selector of the chart area of a disk's `disk_charts` cell, below its header.
pub fn chart_id(key: &DeviceKey) -> String {
    format!("{DISK_BOX_ID}-chart:{}", key.0)
}

/// The debug selector of a part of a disk's `disk_charts` cell header: "name", "active", "read"
/// or "write".
pub fn head_id(key: &DeviceKey, part: &str) -> String {
    format!("{DISK_BOX_ID}-head-{part}:{}", key.0)
}

/// The debug selector of a disk's Read ("read") or Write ("write") row.
pub fn io_row_id(key: &DeviceKey, which: &str) -> String {
    format!("{DISK_BOX_ID}-{which}:{}", key.0)
}

/// The Disk box.
pub struct DiskBox {
    /// The range figures the page gives (UI-PRF-055).
    range_text: RangeText,
    hub: Entity<SnapshotHub>,
    /// The latest disk cards from the sidebar, including any entry whose parent was not known yet.
    disks: SmallVec<[(DeviceKey, DeviceIdx, SharedString); 8]>,
    /// The sections' text and charts, shared with the disks group's flow look.
    model: Rc<RefCell<DiskModel>>,
    mode: DiskMode,
    /// How `disk_charts` places its cells (D-027).
    arrange: Arrange,
    /// The gap between the label part and the sparkline part (`rows`).
    row_split: InnerSplit,
    /// The summed read and write lines of `total_chart`, and the aggregate device they read.
    total_chart: Entity<ChartView>,
    aggregate: DeviceIdx,
    catalog_version: u64,
    title: SharedString,
    text_bits: u64,
    /// The body size of the last paint, for the fit of the sections and cells.
    body: Rc<Cell<Size<Pixels>>>,
    settings: ChartSettings,
    settings_versions: [u64; 3],
    style_version: u64,
    strings_version: u64,
    /// The flow layout of the shown preset (ADR-019): which widget the box shows.
    flow: ContainerLayout,
    /// The saved grid layout, when the user has one: the editor draws it.
    saved_grid: Option<ContainerLayout>,
    /// Edit Widgets is on: the editor draws the body.
    editing: bool,
    editor: Entity<WidgetEditor>,
    container: Entity<WidgetContainer>,
    /// The plan of the last flow render, which the container's placer reads.
    plan: PlanCell,
    _subs: [Subscription; 5],
    _editor_sub: Subscription,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

/// The saved grid layout of the Disk box, if any.
fn saved_grid(cx: &App) -> Option<ContainerLayout> {
    if !cx.has_global::<TlSettings>() {
        return None;
    }
    TlSettings::settings(cx)
        .performance
        .layouts
        .known(DISK_BOX_CONTAINER)
        .filter(|l| l.preset.is_none())
        .cloned()
}

/// The keys of the default mode with its default arrangement.
fn default_keys() -> DiskFlowKeys<'static> {
    DiskFlowKeys {
        mode: MODES[0].id,
        arrange: Some(Arrange::Vertical),
        split: None,
    }
}

impl DiskBox {
    pub fn new(
        hub: &Entity<SnapshotHub>,
        disks: &[(DeviceKey, DeviceIdx, SharedString)],
        cx: &mut Context<Self>,
    ) -> Self {
        let snap = box_snapshot(hub, cx);
        let settings = ChartSettings::read(cx);
        let aggregate = aggregate_disks(&snap.devices);
        let total_chart = box_chart(
            pair_spec(aggregate, &settings, current(cx)),
            pair_labels(),
            hub,
            cx,
        );
        let model = Rc::new(RefCell::new(DiskModel {
            words: [tr(cx, KEY_READ), tr(cx, KEY_WRITE)],
            ..DiskModel::default()
        }));
        let flow = disk_flow::flow_layout(default_keys());
        let saved = saved_grid(cx);
        let plan = PlanCell::default();
        let container = {
            let store = Rc::new(RefCell::new(SourceStore::new(hub)));
            let shown = saved.clone().unwrap_or_else(|| flow.clone());
            let plan = plan.clone();
            cx.new(|cx| {
                let mut c = WidgetContainer::new(DISK_BOX_CONTAINER, shown, hub, store, cx);
                c.set_flow_placer(Some(disk_flow::placer(plan)), cx);
                // The box draws the widget of its flow layouts in its 1.2.0 part.
                c.set_hosted(true, cx);
                c
            })
        };
        let editor = cx.new(|cx| WidgetEditor::new(container.clone(), hub, cx));
        let editor_sub = cx.subscribe(&editor, Self::on_widget_edit);
        let mut this = Self {
            range_text: RangeText::default(),
            hub: hub.clone(),
            disks: disks.iter().cloned().collect(),
            model,
            mode: DiskMode::DiskCharts,
            arrange: Arrange::Vertical,
            row_split: InnerSplit::new(
                DISK_SPLITTER_ID,
                DISK_SPLITTER_LINE_ID,
                MODES[1].split.unwrap_or_default(),
            )
            .with_mins(LABEL_PART_MIN_W, SPARK_PART_MIN_W),
            total_chart,
            aggregate,
            catalog_version: snap.devices.version,
            title: tr(cx, KEY_TITLE),
            text_bits: 0,
            body: Rc::new(Cell::new(size(px(0.), px(0.)))),
            settings,
            settings_versions: chart_section_versions(cx),
            style_version: metrics(cx).style_version,
            strings_version: cx.global::<Strings>().strings_version,
            _subs: [
                cx.subscribe(hub, Self::on_snapshot),
                cx.observe_global::<OverviewPause>(Self::on_pause),
                cx.observe_global::<TlSettings>(Self::sync_settings),
                cx.observe_global::<UiMetrics>(Self::sync_style),
                cx.observe_global::<Strings>(Self::sync_strings),
            ],
            flow,
            saved_grid: saved,
            editing: false,
            editor,
            container,
            plan,
            _editor_sub: editor_sub,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.build_rows(&snap, cx);
        this
    }

    /// New disks from the page. Nothing happens when they are the same.
    pub fn set_disks(
        &mut self,
        disks: &[(DeviceKey, DeviceIdx, SharedString)],
        cx: &mut Context<Self>,
    ) {
        if self.disks.as_slice() == disks {
            return;
        }
        self.disks.clear();
        self.disks.extend(disks.iter().cloned());
        let snap = box_snapshot(&self.hub, cx);
        self.sections_changed(&snap, cx);
    }

    /// The editor of the box's widgets (Edit Widgets, UI-PRF-061).
    pub fn editor(&self) -> &Entity<WidgetEditor> {
        &self.editor
    }

    /// The container of the box's widgets.
    pub fn container(&self) -> &Entity<WidgetContainer> {
        &self.container
    }

    /// The flow layout of the shown preset. Its preset keeps the mode and the arrangement; the
    /// split is the box's own, so a drag does not lay the widget out again.
    pub fn flow(&self) -> &ContainerLayout {
        &self.flow
    }

    /// The saved grid layout the box shows, if any.
    pub fn grid(&self) -> Option<&ContainerLayout> {
        self.saved_grid.as_ref()
    }

    /// The plan of the last flow render: the widget the placer places.
    pub fn flow_plan(&self) -> Option<DiskPlan> {
        self.plan.plan()
    }

    /// The widgets the last flow render drew, in drawing order.
    pub fn drawn_widgets(&self) -> SmallVec<[u32; 1]> {
        self.plan.drawn()
    }

    /// Whether the box draws the widget of its flow layout itself (no grid layout, not editing).
    fn host_draws(&self) -> bool {
        !self.editing && self.saved_grid.is_none()
    }

    /// Gets the container ready for Edit Widgets: with a flow layout it takes the preset's
    /// widget that the last render drew, placed where it was drawn, so the editor pins it
    /// there. With a grid layout the container already holds it.
    pub fn prepare_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing || self.saved_grid.is_some() {
            return;
        }
        let drawn = self.plan.drawn();
        let mut layout = self.flow.clone();
        layout.widgets.retain(|w| drawn.contains(&w.id));
        self.container.update(cx, |c, cx| {
            c.set_layout(layout, cx);
            c.place_again(cx);
        });
    }

    /// Edit Widgets started or ended. When it ends, the box shows the saved layout again.
    fn on_widget_edit(
        &mut self,
        _: Entity<WidgetEditor>,
        ev: &WidgetEditEvent,
        cx: &mut Context<Self>,
    ) {
        match ev {
            WidgetEditEvent::Started => self.editing = true,
            WidgetEditEvent::Ended { .. } => {
                self.editing = false;
                self.saved_grid = saved_grid(cx);
                self.show_saved(cx);
            }
        }
        self.notify(cx);
    }

    /// The container holds the saved grid layout, or the flow layout the box draws.
    fn show_saved(&mut self, cx: &mut Context<Self>) {
        let shown = self.saved_grid.clone().unwrap_or_else(|| self.flow.clone());
        self.container.update(cx, |c, cx| c.set_layout(shown, cx));
    }

    /// The saved grid layout came, went or changed: outside Edit Widgets the box shows it.
    fn sync_grid(&mut self, cx: &mut Context<Self>) {
        let grid = saved_grid(cx);
        if grid == self.saved_grid {
            return;
        }
        self.saved_grid = grid;
        if self.editing {
            return;
        }
        self.show_saved(cx);
        self.notify(cx);
    }

    /// The flow layout of the shown mode and arrangement. Returns whether it changed.
    fn sync_flow(&mut self, cx: &mut Context<Self>) -> bool {
        let has_arrange = ARRANGE.iter().any(|(id, _)| *id == self.mode.id());
        let flow = disk_flow::flow_layout(DiskFlowKeys {
            mode: self.mode.id(),
            arrange: has_arrange.then_some(self.arrange),
            split: None,
        });
        if flow == self.flow {
            return false;
        }
        self.flow = flow;
        if self.host_draws() {
            self.show_saved(cx);
        }
        true
    }

    fn notify(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// The sections' text or list changed: the disks group draws them, so only it renders
    /// again. When the box draws no disks group (a grid layout keeps its own sources, and
    /// `total_chart` shows no sections) nothing renders; before the first flow render makes the
    /// group's entity, the box renders.
    fn notify_disks(&mut self, cx: &mut Context<Self>) {
        if !self.host_draws() || self.mode == DiskMode::TotalChart {
            return;
        }
        let group = disk_flow::disks_of(&self.flow)
            .and_then(|id| self.container.read(cx).group(id).cloned());
        match group {
            Some(g) => g.update(cx, |g, cx| g.flow_changed(cx)),
            None => self.notify(cx),
        }
    }

    /// The shown mode's id.
    pub fn mode(&self) -> &'static str {
        self.mode.id()
    }

    /// Shows mode `id`; an unknown id is the default mode. Only this box renders again.
    pub fn set_mode(&mut self, id: &str, cx: &mut Context<Self>) {
        let mode = DiskMode::from_id(id);
        if mode != self.mode {
            self.mode = mode;
            self.sync_flow(cx);
            self.notify(cx);
        }
    }

    /// How `disk_charts` places its cells.
    pub fn arrange(&self) -> Arrange {
        self.arrange
    }

    /// Places the `disk_charts` cells as `arrange` says. Only this box renders again, and only
    /// when it shows `disk_charts`.
    pub fn set_arrange(&mut self, arrange: Arrange, cx: &mut Context<Self>) {
        if arrange != self.arrange {
            self.arrange = arrange;
            self.sync_flow(cx);
            if self.mode == DiskMode::DiskCharts {
                self.notify(cx);
            }
        }
    }

    /// The label part's share of the body as laid out in `rows`, after the clamps; 0 in the
    /// other modes.
    pub fn split(&self, m: &UiMetrics) -> f32 {
        match self.mode {
            DiskMode::Rows => self.row_split.share(SplitAxis::Row, m.sp[5], m.ui_scale),
            _ => 0.,
        }
    }

    /// Takes the saved share for `rows`. Nothing changes while the gap is held.
    pub fn set_split(&mut self, share: f32, cx: &mut Context<Self>) {
        if self.mode == DiskMode::Rows && self.row_split.set_share(share) {
            self.notify(cx);
        }
    }

    fn row_inner_split(&mut self) -> &mut InnerSplit {
        &mut self.row_split
    }

    /// How many `disk_charts` cells the last render showed.
    pub fn cells(&self) -> usize {
        self.model.borrow().cells
    }

    pub fn rows(&self) -> Ref<'_, [DiskRow]> {
        Ref::map(self.model.borrow(), |m| m.rows.as_slice())
    }

    pub fn more(&self) -> usize {
        self.model.borrow().more
    }

    /// How many sections the last render showed.
    pub fn shown(&self) -> usize {
        self.model.borrow().shown
    }

    /// Whether the last render showed section `i` with its Read and Write rows.
    pub fn expanded(&self, i: usize) -> bool {
        self.model.borrow().expanded(i)
    }

    /// The "+n more" text the last render showed, empty for none.
    pub fn shown_more_text(&self) -> SharedString {
        self.model.borrow().shown_more_text()
    }

    /// The shared model the disks group draws.
    pub fn model(&self) -> &Rc<RefCell<DiskModel>> {
        &self.model
    }

    /// "sda\tRead 1.0 MiB/s\tWrite 0 KiB/s" per row, then "+n more".
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.title);
        out.push('\n');
        let model = self.model.borrow();
        for r in &model.rows {
            out.push_str(&r.name);
            for (word, rate) in [(&model.words[0], &r.read), (&model.words[1], &r.write)] {
                out.push('\t');
                out.push_str(word);
                out.push(' ');
                out.push_str(rate);
            }
            out.push('\n');
        }
        if let Some(more) = model
            .more_texts
            .get(model.rows.len())
            .filter(|t| !t.is_empty())
        {
            out.push_str(more);
            out.push('\n');
        }
    }

    /// The key of the first section, which the title line opens.
    fn first_key(&self) -> Option<DeviceKey> {
        self.model.borrow().rows.first().map(|r| r.key.clone())
    }

    /// The disk list or the catalog changed: new sections, and one render of the disks group.
    /// The box renders too when the disk its title opens changed.
    fn sections_changed(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        let first = self.first_key();
        self.build_rows(snap, cx);
        if self.first_key() != first {
            self.notify(cx);
        }
        self.notify_disks(cx);
    }

    /// The sections of the whole disks. A disk that had a section keeps its charts; only a new
    /// one builds them.
    fn build_rows(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let hub = self.hub.clone();
        let disks: SmallVec<[_; 8]> = whole_disks(&self.disks, &snap.devices).cloned().collect();
        let mut old = std::mem::take(&mut self.model.borrow_mut().rows);
        let mut rows = SmallVec::<[DiskRow; 8]>::new();
        for (key, idx, name) in disks.iter().take(MAX_DISK_ROWS) {
            let row = match old.iter().position(|r| r.key == *key) {
                Some(i) => {
                    let mut r = old.remove(i);
                    if r.idx != *idx {
                        r.idx = *idx;
                        self.set_specs(&r, &theme, cx);
                    }
                    r.name = name.clone();
                    r
                }
                None => {
                    let chart = |metric, cx: &mut Context<Self>| {
                        let spec = io_spec(*idx, metric, &self.settings, &theme);
                        spark_chart(spec, ValueFormat::RateIec, &hub, cx)
                    };
                    DiskRow {
                        key: key.clone(),
                        idx: *idx,
                        name: name.clone(),
                        model: SharedString::default(),
                        active: SharedString::default(),
                        read: SharedString::default(),
                        write: SharedString::default(),
                        busy: None,
                        read_chart: chart(MetricId::DISK_READ, cx),
                        write_chart: chart(MetricId::DISK_WRITE, cx),
                        pair_chart: box_chart(
                            pair_spec(*idx, &self.settings, &theme),
                            pair_labels(),
                            &hub,
                            cx,
                        ),
                    }
                }
            };
            rows.push(row);
        }
        {
            let mut model = self.model.borrow_mut();
            model.rows = rows;
            model.more = disks.len().saturating_sub(MAX_DISK_ROWS);
        }
        self.sync_more_texts(cx);
        self.sync_text(snap, true);
    }

    /// "+n more" for each number of sections shown.
    fn sync_more_texts(&mut self, cx: &mut Context<Self>) {
        let (rows, total) = {
            let model = self.model.borrow();
            (model.rows.len(), model.rows.len() + model.more)
        };
        let texts: SmallVec<[SharedString; 9]> = (0..=rows)
            .map(|shown| {
                let n = total - shown;
                if n == 0 {
                    SharedString::default()
                } else {
                    tr_args(cx, KEY_MORE, &[("n", FluentValue::from(n))])
                }
            })
            .collect();
        self.model.borrow_mut().more_texts = texts;
    }

    /// Takes the range figures the page worked out (UI-PRF-055). Notifies
    /// only when they changed.
    pub fn set_range_text(&mut self, text: RangeText, cx: &mut Context<Self>) {
        if self.range_text != text {
            self.range_text = text;
            cx.notify();
        }
    }

    /// The range figures the box shows.
    pub fn range_text(&self) -> &RangeText {
        &self.range_text
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let mask = box_mask(ev.mask, cx);
        if !mask.intersects(
            ComponentMask::DEVICES
                | ComponentMask::SYSTEM
                | ComponentMask::DEVICE_DETAILS
                | ComponentMask::INVENTORY,
        ) {
            return;
        }
        let snap = box_snapshot(&hub, cx);
        if mask.contains(ComponentMask::DEVICES) && snap.devices.version != self.catalog_version {
            self.catalog_version = snap.devices.version;
            let aggregate = aggregate_disks(&snap.devices);
            if aggregate != self.aggregate {
                self.aggregate = aggregate;
                self.rebuild_specs(cx);
            }
            self.sections_changed(&snap, cx);
            return;
        }
        if self.sync_text(&snap, false) {
            self.notify_disks(cx);
        }
    }

    /// The page was held at a time or made live: every section reads the other snapshot.
    fn on_pause(&mut self, cx: &mut Context<Self>) {
        let snap = box_snapshot(&self.hub, cx);
        self.sync_text(&snap, true);
        self.notify(cx);
        self.notify_disks(cx);
    }

    /// Every section's model, active percent and rates when their inputs changed, or always
    /// with `force`. True when the text was formatted again.
    fn sync_text(&mut self, snap: &Snapshot, force: bool) -> bool {
        let totals = |idx: DeviceIdx| {
            snap.system
                .disks
                .iter()
                .find(|(i, _)| *i == idx)
                .map(|(_, t)| t)
        };
        let mut model = self.model.borrow_mut();
        let mut h = FxHasher::default();
        for r in &model.rows {
            r.idx.0.hash(&mut h);
            model_of(snap, r.idx).hash(&mut h);
            let Some(t) = totals(r.idx) else {
                h.write_u8(0);
                continue;
            };
            h.write_u64(t.read_bps.value.to_bits());
            t.read_bps.validity.hash(&mut h);
            h.write_u64(t.write_bps.value.to_bits());
            t.write_bps.validity.hash(&mut h);
            h.write_u32(t.active_pct.value.to_bits());
            t.active_pct.validity.hash(&mut h);
        }
        let bits = h.finish();
        if !force && bits == self.text_bits {
            return false;
        }
        self.text_bits = bits;
        let unit = NetworkUnit::default();
        for r in &mut model.rows {
            r.model = model_of(snap, r.idx)
                .map_or_else(SharedString::default, |m| SharedString::from(m.to_owned()));
            let Some(t) = totals(r.idx) else {
                let missing = value_text(Validity::Missing, |_| {});
                r.active = missing.clone();
                r.read = missing.clone();
                r.write = missing;
                r.busy = None;
                continue;
            };
            let rate = |m: &tl_model::system::Metric<f64>| {
                value_text(m.validity, |out| {
                    format_value(ValueFormat::RateIec, m.value as f32, unit, out);
                })
            };
            r.read = rate(&t.read_bps);
            r.write = rate(&t.write_bps);
            r.active = value_text(t.active_pct.validity, |out| {
                format_value(ValueFormat::Percent, t.active_pct.value, unit, out);
            });
            r.busy = matches!(t.active_pct.validity, Validity::Valid | Validity::Stale)
                .then(|| t.active_pct.value / 100.);
        }
        true
    }

    /// Applies the chart settings when one of their sections changed, and shows a saved grid
    /// layout that came, went or changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        self.sync_grid(cx);
        let versions = chart_section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = ChartSettings::read(cx);
        if new == self.settings {
            return;
        }
        self.settings = new;
        self.rebuild_specs(cx);
    }

    /// A new theme or UI scale: new chart colours, and the sections draw in the new style.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_specs(cx);
        cx.notify();
        self.notify_disks(cx);
    }

    /// A new language: new title, row words and "+n more".
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.title = tr(cx, KEY_TITLE);
        self.model.borrow_mut().words = [tr(cx, KEY_READ), tr(cx, KEY_WRITE)];
        self.sync_more_texts(cx);
        cx.notify();
        self.notify_disks(cx);
    }

    /// Both sparklines of `r` on its current device.
    fn set_specs(&self, r: &DiskRow, theme: &TlTheme, cx: &mut Context<Self>) {
        let read = io_spec(r.idx, MetricId::DISK_READ, &self.settings, theme);
        r.read_chart.update(cx, |v, cx| v.set_spec(read, cx));
        let write = io_spec(r.idx, MetricId::DISK_WRITE, &self.settings, theme);
        r.write_chart.update(cx, |v, cx| v.set_spec(write, cx));
        let pair = pair_spec(r.idx, &self.settings, theme);
        r.pair_chart.update(cx, |v, cx| v.set_spec(pair, cx));
    }

    fn rebuild_specs(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let rows: SmallVec<[DiskRow; 8]> = self.model.borrow().rows.clone();
        for r in &rows {
            self.set_specs(r, &theme, cx);
        }
        let total = pair_spec(self.aggregate, &self.settings, &theme);
        self.total_chart.update(cx, |v, cx| v.set_spec(total, cx));
    }
}

/// The debug selector prefix of a disk section's header, followed by its device key.
pub const DISK_ROW_ID_PREFIX: &str = "perf-overview-disk-row:";

/// The debug selector of a part of a disk section's label rows: "model", "active", "read" or
/// "write".
pub fn row_part_id(key: &DeviceKey, part: &str) -> String {
    format!("{DISK_BOX_ID}-row-{part}:{}", key.0)
}

/// A right-aligned number column: a rate or the active percent. It is `RATE_W` wide at least and
/// grows to its text, so a rate never wraps; with less room it drops decimals and keeps its unit
/// (UI-FND-051 rule 3, UI-PRF-017).
fn rate_cell(id: String, text: &SharedString, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    // The text style first: assigning it later would drop the nowrap and the color.
    let mut d = div();
    *d.text_style() = numeric_style(m, m.text.sm);
    d.tl_selector(move || id)
        .min_w(px(RATE_W * m.ui_scale))
        .flex()
        .justify_end()
        .whitespace_nowrap()
        .text_color(theme.text_primary)
        .child(fit_text(text.clone()))
        .into_any_element()
}

/// A section header: name, model and active percent. A click opens that disk (crw.44).
fn header(r: &DiskRow, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    // The text style goes in first: assigning it later would drop the nowrap, the ellipsis and
    // the color, and a long model would wrap onto the next row.
    let mut name = div();
    *name.text_style() = label_style(m, m.text.sm, FontWeight::MEDIUM);
    let name = name
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_primary)
        .child(r.name.clone());
    let mut model = div();
    *model.text_style() = t_sm(m);
    let model = model
        .tl_selector({
            let id = row_part_id(&r.key, "model");
            move || id
        })
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(r.model.clone());
    let key = r.key.0.clone();
    let row = div()
        .tl_selector(move || format!("{DISK_ROW_ID_PREFIX}{key}"))
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(m.sp[4])
        .h(px(LABEL_ROW * m.ui_scale))
        .child(name)
        .child(model)
        .child(rate_cell(
            row_part_id(&r.key, "active"),
            &r.active,
            m,
            theme,
        ));
    opens_device(row, Some(r.key.clone())).into_any_element()
}

/// A Read or Write text row of the label part: the word and the current rate.
fn io_text_row(
    id: String,
    word: &SharedString,
    rate: &SharedString,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    let s = m.ui_scale;
    let rate_id = format!("{id}-rate");
    let mut label = div();
    *label.text_style() = t_sm(m);
    let label = label
        .flex_none()
        .w(px(ROW_LABEL_W * s))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_secondary)
        .child(word.clone());
    div()
        .tl_selector(move || id)
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(m.sp[4])
        .h(px(LABEL_ROW * s))
        .child(label)
        .child(rate_cell(rate_id, rate, m, theme))
        .into_any_element()
}

/// A Read or Write sparkline cell of the sparkline part, as high as its text row.
fn spark_cell(id: String, chart: &Entity<ChartView>, m: &UiMetrics) -> AnyElement {
    div()
        .tl_selector(move || id)
        .relative()
        .flex_none()
        .w_full()
        .h(px(LABEL_ROW * m.ui_scale))
        .child(chart.clone().cached(StyleRefinement::default().size_full()))
        .into_any_element()
}

/// A header rate: the value led by the first letter of `word` when the cell is at least
/// `HEAD_LABEL_W` px wide, the bare value when it is narrower.
fn rate_text(
    width: Pixels,
    ui_scale: f32,
    word: &SharedString,
    value: &SharedString,
) -> SharedString {
    if width >= px(HEAD_LABEL_W * ui_scale) {
        let letter = word.chars().next().unwrap_or(' ');
        SharedString::from(format!("{letter} {value}"))
    } else {
        value.clone()
    }
}

/// A `disk_charts` cell header `width` px wide: name, active percent and the current read and
/// write rates, each rate led by the first letter of its word. Below `HEAD_ONE_LINE_W` the rates
/// go on a second line, and below `HEAD_LABEL_W` they drop the letter. Every part shrinks with an
/// ellipsis, the name first, so none spills out of the cell. A click opens that disk.
fn cell_head(
    r: &DiskRow,
    width: Pixels,
    words: &[SharedString; 2],
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    let key = &r.key;
    let one_line = width >= px(HEAD_ONE_LINE_W * m.ui_scale);
    let part = |id: String, text: SharedString, color, style: TextStyleRefinement| {
        // The name gives way first: a number shrinks only once the name is at its minimum, and
        // then drops decimals and keeps its unit. The text style goes in first: assigning it
        // later would drop the nowrap.
        let mut d = div();
        *d.text_style() = style;
        d.tl_selector(move || id)
            .flex()
            .min_w_0()
            .whitespace_nowrap()
            .text_color(color)
            .child(fit_text(text))
    };
    let mut name = div();
    *name.text_style() = label_style(m, m.text.sm, FontWeight::MEDIUM);
    let name = name
        .tl_selector({
            let id = head_id(key, "name");
            move || id
        })
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.text_primary)
        .child(r.name.clone())
        .flex_1()
        .flex_shrink(NAME_SHRINK)
        .min_w(px(HEAD_NAME_MIN_W * m.ui_scale));
    let active = part(
        head_id(key, "active"),
        r.active.clone(),
        theme.text_primary,
        numeric_style(m, m.text.sm),
    );
    let rate = |which: &str, word: &SharedString, value: &SharedString| {
        let text = rate_text(width, m.ui_scale, word, value);
        part(
            head_id(key, which),
            text,
            theme.text_secondary,
            numeric_style(m, m.text.xs),
        )
    };
    let (read, write) = (
        rate("read", &words[0], &r.read),
        rate("write", &words[1], &r.write),
    );
    let line = || {
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(m.sp[4])
            .h(px(PLOT_LINE * m.ui_scale))
    };
    let head = if one_line {
        line().child(name).child(active).child(read).child(write)
    } else {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(line().child(name).child(active))
            .child(line().justify_between().child(read).child(write))
    };
    opens_device(head, Some(r.key.clone())).into_any_element()
}

/// A `disk_charts` cell of `cell` px: the header over the read and write chart, which starts
/// right under it.
fn chart_cell(
    r: &DiskRow,
    cell: Size<Pixels>,
    words: &[SharedString; 2],
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    let (key, chart_key) = (r.key.clone(), r.key.clone());
    div()
        .tl_selector(move || cell_id(&key))
        .flex()
        .flex_col()
        .flex_none()
        .w(cell.width)
        .h(cell.height)
        .overflow_hidden()
        .child(cell_head(r, cell.width, words, m, theme))
        .child(
            div()
                .tl_selector(move || chart_id(&chart_key))
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    r.pair_chart
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                ),
        )
        .into_any_element()
}

/// The flex shrink of a cell header name: large, so the name shrinks to its minimum before a
/// number shrinks. A number factor below 1 would not work: flex then removes only that part of
/// the overflow.
const NAME_SHRINK: f32 = 1000.;

/// The bit mask of the `n` busiest of the first `shown` rows (unknown lowest, ties to the
/// earlier row).
fn busiest(rows: &[DiskRow], shown: usize, n: usize) -> u8 {
    let mut order: SmallVec<[usize; MAX_DISK_ROWS]> = (0..shown.min(rows.len())).collect();
    let busy = |i: usize| rows[i].busy.unwrap_or(-1.);
    order.sort_by(|&a, &b| busy(b).total_cmp(&busy(a)));
    order.iter().take(n).fold(0, |mask, &i| mask | 1 << i)
}

/// The "+n more" line.
pub(super) fn more_line(text: SharedString, m: &UiMetrics, theme: &TlTheme) -> AnyElement {
    let mut line = div()
        .flex_none()
        .h(px(LABEL_ROW * m.ui_scale))
        .whitespace_nowrap()
        .text_color(theme.text_secondary)
        .child(text);
    *line.text_style() = t_xs(m);
    line.into_any_element()
}

/// `rows`: the label part (headers, words and rates, `share` of the width) and the sparkline
/// part. Both parts stack their lines alike, so a sparkline sits level with its text row. The
/// sections that do not fit `body` collapse to their headers, the busiest expanded first; the
/// fit is stored in `model`. `view` renders again when a resize changes the fit. The box draws
/// the splitter over it.
pub(crate) fn disk_rows_element(
    model: &Rc<RefCell<DiskModel>>,
    body: &Rc<Cell<Size<Pixels>>>,
    share: f32,
    m: &UiMetrics,
    theme: &TlTheme,
    view: EntityId,
) -> AnyElement {
    let mut guard = model.borrow_mut();
    let md = &mut *guard;
    let rows = md.rows.len();
    let total = rows + md.more;
    let shown = fit(body.get(), rows, total, m);
    md.shown = shown.shown;
    md.cells = 0;
    md.expanded = busiest(&md.rows, shown.shown, shown.expanded);

    let column = || whole_rows().h_full().gap(m.sp[2]);
    let mut left = column().flex_shrink(1.).min_w_0().w(relative(share));
    let mut right = column().flex_1().min_w_0();
    for (i, r) in md.rows.iter().take(shown.shown).enumerate() {
        left = left.child(header(r, m, theme));
        right = right.child(div().flex_none().h(px(LABEL_ROW * m.ui_scale)));
        if md.expanded(i) {
            left = left
                .child(io_text_row(
                    io_row_id(&r.key, "read"),
                    &md.words[0],
                    &r.read,
                    m,
                    theme,
                ))
                .child(io_text_row(
                    io_row_id(&r.key, "write"),
                    &md.words[1],
                    &r.write,
                    m,
                    theme,
                ));
            right = right
                .child(spark_cell(
                    io_row_id(&r.key, "read-spark"),
                    &r.read_chart,
                    m,
                ))
                .child(spark_cell(
                    io_row_id(&r.key, "write-spark"),
                    &r.write_chart,
                    m,
                ));
        }
    }
    let more = md.shown_more_text();
    if !more.is_empty() {
        left = left.child(more_line(more, m, theme));
    }
    let probe_m = *m;
    div()
        .relative()
        .size_full()
        .child(
            div()
                .relative()
                .flex()
                .flex_row()
                .size_full()
                .gap(m.sp[5])
                .child(left)
                .child(right),
        )
        .child(class_probe(
            Rc::clone(body),
            shown.class(),
            move |b| fit(b, rows, total, &probe_m).class(),
            view,
        ))
        .into_any_element()
}

/// `disk_charts`: one read and write chart per disk that fill `body` in one column, one row or
/// a grid, as `arrange` says. Only whole cells show; the disks left out join "+n more"
/// (UI-PRF-059). The fit is stored in `model`; `view` renders again when a resize changes it.
pub(crate) fn disk_cells_element(
    model: &Rc<RefCell<DiskModel>>,
    body: &Rc<Cell<Size<Pixels>>>,
    arrange: Arrange,
    m: &UiMetrics,
    theme: &TlTheme,
    view: EntityId,
) -> AnyElement {
    let mut guard = model.borrow_mut();
    let md = &mut *guard;
    let rows = md.rows.len();
    let total = rows + md.more;
    let fit = disk_fit(arrange, body.get(), rows, total, m);
    md.shown = fit.shown;
    md.cells = fit.shown;
    md.expanded = 0;
    let cells = grid_cells_gaps(
        &fit.grid,
        md.rows
            .iter()
            .take(fit.shown)
            .map(|r| chart_cell(r, fit.grid.cell, &md.words, m, theme)),
        cell_gap(m),
        GRID_ID,
    );
    // A "+n more" line that does not fit whole is left out (UI-PRF-017).
    let mut el = whole_rows().relative().size_full().gap(m.sp[2]).child(
        div()
            .tl_selector(|| GRID_ID.into())
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(cells),
    );
    let more = md.shown_more_text();
    if !more.is_empty() {
        el = el.child(more_line(more, m, theme));
    }
    let probe_m = *m;
    el.child(class_probe(
        Rc::clone(body),
        fit.class(),
        move |b| disk_fit(arrange, b, rows, total, &probe_m).class(),
        view,
    ))
    .into_any_element()
}

/// The widgets a flow render drew, in drawing order.
type Drawn = SmallVec<[u32; 1]>;

/// "No widgets. Use Edit Widgets to add some.", centred in the body (UI-PRF-061).
fn empty_text(m: &UiMetrics, theme: &TlTheme, cx: &App) -> AnyElement {
    let mut text = div();
    *text.text_style() = t_xs(m);
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            text.tl_selector(|| EMPTY_ID.to_owned())
                .text_color(theme.text_secondary)
                .child(tr(cx, KEY_EMPTY)),
        )
        .into_any_element()
}

fn cached_full<V: Render>(view: Entity<V>) -> AnyElement {
    view.cached(StyleRefinement::default().size_full())
        .into_any_element()
}

impl DiskBox {
    /// The disks group drawing `look`, when the layout has it.
    fn disks_widget(
        &self,
        look: GroupFlowLook,
        drawn: &mut Drawn,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = disk_flow::disks_of(&self.flow)?;
        let group = self
            .container
            .update(cx, |c, cx| c.flow_group(id, look, cx))?;
        drawn.push(id);
        Some(cached_full(group))
    }

    /// `rows`: the disk sections, drawn by the disks group, and the splitter over them.
    fn rows_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        drawn: &mut Drawn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let share = self.split(m);
        let look = GroupFlowLook::DiskRows {
            model: Rc::clone(&self.model),
            body: Rc::clone(&self.body),
            share,
        };
        let part = match self.disks_widget(look, drawn, cx) {
            Some(el) => el,
            None => disk_rows_element(&self.model, &self.body, share, m, theme, cx.entity_id()),
        };
        div()
            .relative()
            .size_full()
            .child(part)
            .child(self.row_split.probe(Self::row_inner_split, cx))
            .child(
                self.row_split
                    .gap(SplitAxis::Row, share, m.sp[5], Self::row_inner_split, cx),
            )
            .into_any_element()
    }

    /// `disk_charts`: the cells, drawn by the disks group.
    fn charts_body(
        &mut self,
        m: &UiMetrics,
        theme: &TlTheme,
        drawn: &mut Drawn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = GroupFlowLook::DiskCells {
            model: Rc::clone(&self.model),
            body: Rc::clone(&self.body),
            arrange: self.arrange,
        };
        match self.disks_widget(look, drawn, cx) {
            Some(el) => el,
            None => disk_cells_element(
                &self.model,
                &self.body,
                self.arrange,
                m,
                theme,
                cx.entity_id(),
            ),
        }
    }

    /// `total_chart`: the summed read and write lines, drawn by their widget.
    fn total_body(&mut self, drawn: &mut Drawn, cx: &mut Context<Self>) -> AnyElement {
        {
            let mut model = self.model.borrow_mut();
            model.shown = model.rows.len();
            model.cells = 0;
            model.expanded = 0;
        }
        let widget = disk_flow::total_of(&self.flow).and_then(|id| {
            let look = FlowLook::Chart(self.total_chart.clone());
            let w = self
                .container
                .update(cx, |c, cx| c.flow_widget(id, look, cx))?;
            drawn.push(id);
            Some(w)
        });
        let chart = match widget {
            Some(w) => cached_full(w),
            None => cached_full(self.total_chart.clone()),
        };
        div()
            .tl_selector(|| TOTAL_CHART_ID.into())
            .relative()
            .size_full()
            .child(chart)
            .into_any_element()
    }
}

impl Render for DiskBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let editor = cached_full(self.editor.clone());
        let body = if !self.host_draws() {
            // The user's widgets, or Edit Widgets: the editor draws them.
            let empty = !self.editing
                && self
                    .saved_grid
                    .as_ref()
                    .is_some_and(|g| g.widgets.is_empty());
            div()
                .relative()
                .size_full()
                .child(editor)
                .when(empty, |d| d.child(empty_text(&m, &theme, cx)))
                .into_any_element()
        } else {
            let mut drawn = Drawn::new();
            let body = match self.mode {
                DiskMode::Rows => self.rows_body(&m, &theme, &mut drawn, cx),
                DiskMode::DiskCharts => self.charts_body(&m, &theme, &mut drawn, cx),
                DiskMode::TotalChart => self.total_body(&mut drawn, cx),
            };
            let plan = DiskPlan {
                look: self.mode.look(),
                widget: drawn.first().copied(),
            };
            let ids_changed = self.plan.drawn() != drawn;
            self.plan.set_drawn(drawn);
            if self.plan.set(plan) || ids_changed {
                self.container.update(cx, |c, cx| {
                    c.place_again(cx);
                });
            }
            // The editor sits under the body with no widgets, so its container knows the body's
            // size when Edit Widgets starts.
            div()
                .relative()
                .size_full()
                .child(div().absolute().top_0().left_0().size_full().child(editor))
                .child(body)
                .into_any_element()
        };
        box_frame(
            DISK_BOX_ID,
            self.first_key(),
            self.title.clone(),
            SharedString::default(),
            SharedString::default(),
            &self.range_text,
            body,
            edit_chrome(cx),
            &m,
            &theme,
        )
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_letter_drops_below_head_label_w() {
        let (word, value) = (SharedString::from("Read"), SharedString::from("1.5 MB/s"));
        let at = |w: f32| rate_text(px(w), 1., &word, &value);
        assert_eq!(at(HEAD_LABEL_W), "R 1.5 MB/s");
        assert_eq!(at(HEAD_LABEL_W - 1.), "1.5 MB/s");
        assert_eq!(at(160.), "1.5 MB/s");
        // The threshold follows the UI scale.
        assert_eq!(rate_text(px(HEAD_LABEL_W), 1.5, &word, &value), "1.5 MB/s");
    }
    use tl_model::devices::{DeviceEntry, DeviceIdentity};

    fn disk(
        idx: u16,
        major: u32,
        minor: u32,
        name: &str,
        parent: Option<DeviceIdx>,
    ) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity: DeviceIdentity::Disk {
                major,
                minor,
                name: Arc::from(name),
                wwn_or_serial: None,
            },
            generation: 0,
            display_name: Arc::from(name),
            present: true,
            parent,
            persistent_key: Arc::from(format!("disk:{name}")),
            net_kind: None,
            hot_c: None,
        }
    }

    #[test]
    fn disk_partitions_dm_fixture_has_only_whole_disk_box_inputs() {
        // This is the exact block topology of tl-collect's disk-partitions-dm fixture.
        let catalog = DeviceCatalog {
            version: 1,
            entries: Arc::from([
                disk(0, 179, 0, "mmcblk0", None),
                disk(1, 259, 0, "nvme0n1", None),
                disk(2, 8, 0, "sda", None),
                disk(3, 8, 1, "sda1", Some(DeviceIdx(2))),
                disk(4, 8, 2, "sda2", Some(DeviceIdx(2))),
                disk(5, 254, 0, "dm-0", Some(DeviceIdx(2))),
                disk(6, 8, 16, "sdb", None),
                disk(7, 11, 0, "sr0", None),
                disk(8, 253, 0, "vda", None),
            ]),
        };
        let inputs: Vec<_> = catalog
            .entries
            .iter()
            .map(|entry| {
                (
                    DeviceKey::from_identity(&entry.identity).expect("disk key"),
                    entry.idx,
                    SharedString::from(entry.display_name.to_string()),
                )
            })
            .collect();

        let names: Vec<&str> = whole_disks(&inputs, &catalog)
            .map(|(_, _, name)| name.as_ref())
            .collect();
        assert_eq!(names, ["mmcblk0", "nvme0n1", "sda", "sdb", "sr0", "vda"]);
    }

    #[test]
    fn late_parent_assignment_refilters_existing_disk_inputs() {
        let inputs = vec![
            (
                DeviceKey(Arc::from("disk:sda")),
                DeviceIdx(0),
                SharedString::from("sda"),
            ),
            (
                DeviceKey(Arc::from("disk:sda1")),
                DeviceIdx(1),
                SharedString::from("sda1"),
            ),
        ];
        let before = DeviceCatalog {
            version: 1,
            entries: Arc::from([disk(0, 8, 0, "sda", None), disk(1, 8, 1, "sda1", None)]),
        };
        let after = DeviceCatalog {
            version: 2,
            entries: Arc::from([
                disk(0, 8, 0, "sda", None),
                disk(1, 8, 1, "sda1", Some(DeviceIdx(0))),
            ]),
        };

        assert_eq!(whole_disks(&inputs, &before).count(), 2);
        let names: Vec<&str> = whole_disks(&inputs, &after)
            .map(|(_, _, name)| name.as_ref())
            .collect();
        assert_eq!(names, ["sda"]);
    }

    fn m() -> UiMetrics {
        crate::theme::metrics::compute(&tl_model::settings::AppearanceSettings::default(), 0)
    }

    fn body(h: f32) -> Size<Pixels> {
        size(px(300.), px(h))
    }

    #[test]
    fn fit_expands_what_the_lines_allow() {
        let m = m();
        // 3 disks need 9 lines: 9 · 18 + 8 · 4 = 194 px.
        assert_eq!(
            fit(body(194.), 3, 3, &m),
            Fit {
                shown: 3,
                expanded: 3
            }
        );
        // The 220 px row minimum leaves a 160 px body: 7 lines, 2 sections expanded.
        assert_eq!(
            fit(body(160.), 3, 3, &m),
            Fit {
                shown: 3,
                expanded: 2
            }
        );
        assert_eq!(
            fit(body(0.), 3, 3, &m),
            Fit {
                shown: 3,
                expanded: 3
            }
        );
        // 8 sections and "+2 more" in 6 lines: 5 headers and the "+n more" line.
        assert_eq!(
            fit(body(128.), 8, 10, &m),
            Fit {
                shown: 5,
                expanded: 0
            }
        );
        assert_eq!(
            fit(body(10.), 3, 3, &m),
            Fit {
                shown: 0,
                expanded: 0
            }
        );
    }
}
