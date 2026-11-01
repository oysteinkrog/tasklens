//! The GPU box's presets as flow layouts (ADR-019 "Flow placement", UI-PRF-061).
//!
//! A flow layout is a list of widgets placed by the rules of its preset: the GPU box modes of
//! UI-PRF-017, UI-PRF-019 and UI-PRF-059. The fixed engine lists of those rules are replaced by
//! the widget list: a section shows an engine, the VRAM row or the temperature and power line
//! only when the layout has that widget for the section's slot.
//!
//! Each part the box shows (an `engine_charts` cell, a spark row, the section chart, or a value
//! on the shared temperature and power line) is the widget's own `MetricWidget` entity, which
//! draws the 1.1.0 part (`FlowLook`); the box draws only what is not a widget (the section
//! headings, the "+n more" rows and the splitter). A widget the rules leave out takes no space
//! and has no entity.
//!
//! The box makes a [`FlowPlan`] on each render: the parts of each section and their widgets,
//! the split and the busiest engine, but no body size and no sample value. The [`placer`]
//! works out every widget's rectangle from the plan and the container's body size with the
//! fit rules the box draws by (`FlowPlan::rects`), so the container keeps the entities of
//! those widgets and drops the others, and Edit Widgets pins the widgets to the grid where the
//! user saw them. The container places again on a resize and a layout change, and the box
//! asks it to when the plan changed: a new engine list, mode, arrangement or split, and in the
//! collapsed fit state (UI-PRF-059) a sample that makes another engine the busiest one. No
//! other sample places anything; the temperature and power line keeps its place while its
//! values come and go.
//!
//! Drawing and placement share one function for each rectangle that is not plain flex: the
//! first chart of a stacked `chart_engines` body with the split moved has the height
//! [`stacked_chart_h`] gives for the body height at layout, and each value of the line is as
//! wide as [`line_text_w`] measures its text when the box draws it ([`LineWidths`]), the
//! " · " between two of them too. A sample that changes a value's text places nothing: the
//! next placement takes the new widths.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Bounds, Pixels, Refineable as _, Size, Window, point, px, size};
use smallvec::SmallVec;
use tl_model::layout::{
    ContainerLayout, DeviceSelector, FormSet, METRIC_POWER, METRIC_TEMP, METRIC_VRAM, MetricKey,
    WidgetSetting, convert_gpu_box, grid_cols,
};
use tl_model::settings::OverviewGridSetting;
use tl_model::system::GpuEngine;

use super::gpu_box::{
    Kept, MAX_GPUS, SectionRows, cell_min, charts_fit, charts_fixed_h, engine_lines, fit, kept,
    section_h, spacing,
};
use super::inner_split::clamp_split;
use super::layout::Arrange;
use super::parts::{MIN_CHART_H, arrange_min_h};
use crate::pages::performance::widget::container::{FlowInput, FlowPlace, list_flow};
use crate::pages::performance::widget::grid::{self, InnerPlacement, PlacedWidget};
use crate::pages::performance::widget::source::Availability;
use crate::text::style::t_xs;
use crate::theme::UiMetrics;

/// The flow layout of preset `mode` with `arrange`, `split` and the box's hidden engine names
/// (D-029): the widget list of `layout::convert_gpu_box`, so it is the same list a converted
/// old setting gives.
pub fn flow_layout(
    mode: &str,
    arrange: Option<Arrange>,
    split: Option<f32>,
    hidden: &[String],
) -> ContainerLayout {
    convert_gpu_box(&OverviewGridSetting {
        kind: "gpu".to_owned(),
        mode: Some(mode.to_owned()),
        arrange: arrange.map(|a| a.id().to_owned()),
        split: split.map(f64::from),
        hidden: hidden.to_vec(),
        ..OverviewGridSetting::default()
    })
}

/// Whether `w` is a widget of slot `slot`.
fn in_slot(w: &WidgetSetting, slot: usize) -> bool {
    matches!(w.source.device, DeviceSelector::Slot(n) if usize::from(n) == slot)
}

/// Whether `w` is the section chart of `chart_engines` and `chart`: the 3D engine with the chart
/// alone.
fn is_section_chart(w: &WidgetSetting) -> bool {
    w.forms == FormSet::CHART && w.source.metric.gpu_engine() == Some(GpuEngine::Gpu)
}

/// The id of the widget of slot `slot` showing `metric`, leaving out the section chart.
pub fn widget_of(layout: &ContainerLayout, slot: usize, metric: &str) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| in_slot(w, slot) && w.source.metric.as_str() == metric && !is_section_chart(w))
        .map(|w| w.id)
}

/// The id of slot `slot`'s section chart.
pub fn chart_of(layout: &ContainerLayout, slot: usize) -> Option<u32> {
    layout
        .widgets
        .iter()
        .find(|w| in_slot(w, slot) && is_section_chart(w))
        .map(|w| w.id)
}

/// The id of slot `slot`'s widget of engine `key` (`GpuEngine::key`).
pub fn engine_widget(layout: &ContainerLayout, slot: usize, key: &str) -> Option<u32> {
    let engine = GpuEngine::ALL.into_iter().find(|e| e.key() == key)?;
    widget_of(layout, slot, MetricKey::engine(engine).as_str())
}

/// Whether slot `slot` shows engine `key`.
pub fn shows_engine(layout: &ContainerLayout, slot: usize, key: &str) -> bool {
    engine_widget(layout, slot, key).is_some()
}

/// Whether slot `slot` shows the VRAM row.
pub fn shows_vram(layout: &ContainerLayout, slot: usize) -> bool {
    widget_of(layout, slot, METRIC_VRAM).is_some()
}

/// Whether slot `slot` shows the temperature and the power draw on its line.
pub fn shows_line(layout: &ContainerLayout, slot: usize) -> (bool, bool) {
    (
        widget_of(layout, slot, METRIC_TEMP).is_some(),
        widget_of(layout, slot, METRIC_POWER).is_some(),
    )
}

/// One part of a section and the widget that draws it: the widget's id and its metric.
pub type Part = (u32, MetricKey);

/// The GPU box mode a [`FlowPlan`] is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlanLook {
    /// `engine_charts`: a grid of engine cells, then the rows under it.
    Cells,
    /// `chart_engines`: the section chart, then the engine rows, VRAM and the line.
    Rows,
    /// `chart`: the section chart and the VRAM row.
    Chart,
}

impl PlanLook {
    /// The preset id of the mode.
    pub fn id(self) -> &'static str {
        match self {
            Self::Cells => "engine_charts",
            Self::Rows => "chart_engines",
            Self::Chart => "chart",
        }
    }
}

/// One section of a [`FlowPlan`]: its parts and their widgets.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct PlanSection {
    /// The section chart (`chart_engines` and `chart`).
    pub chart: Option<Part>,
    /// One entry per engine cell or engine row the box built, in drawing order: the widget that
    /// draws it, if the layout has one.
    pub engines: SmallVec<[Option<Part>; 8]>,
    /// The VRAM row.
    pub vram: Option<Part>,
    /// The values of the temperature and power line that the GPU reports, temperature first.
    /// The line shows when there is one.
    pub line: SmallVec<[Part; 2]>,
}

/// What the GPU box shows in a flow layout, apart from the body size: the mode, the parts of
/// each section and the widget of each part, the split, and the busiest engine. The box makes
/// it from the engine lists, the layout and the split on each render. [`placer`] works out
/// every widget's rectangle from it and the container's body size with the same fit rules
/// the box draws by, so no rectangle comes from a paint.
#[derive(Clone, PartialEq, Debug)]
pub struct FlowPlan {
    pub look: PlanLook,
    pub arrange: Arrange,
    pub m: UiMetrics,
    /// Every GPU the box has, with a section or not.
    pub total: usize,
    /// The sections have a VRAM row.
    pub vram: bool,
    pub sections: SmallVec<[PlanSection; MAX_GPUS]>,
    /// `chart_engines` Side by side: the chart's share of the body width.
    pub side_share: Option<f32>,
    /// `chart_engines` stacked, with the split moved: the saved share, before the clamps. The
    /// first chart's height comes from it and the body height through [`stacked_chart_h`].
    pub stacked_share: Option<f32>,
    /// The index of the first section's busiest engine.
    pub busiest: usize,
    /// The box's render showed the first section collapsed (UI-PRF-059), so `busiest` decides
    /// which engine row shows.
    pub collapsed: bool,
}

impl FlowPlan {
    /// The body rows of each section for `gpu_box::fit`: `chart` has no engine rows and no
    /// line; `chart_engines` has the line when the GPU reports its temperature or power.
    pub(super) fn list_rows(&self) -> SmallVec<[SectionRows; MAX_GPUS]> {
        self.sections
            .iter()
            .map(|s| SectionRows {
                engines: if self.look == PlanLook::Chart {
                    0
                } else {
                    s.engines.len()
                },
                vram: self.vram,
                line: self.look == PlanLook::Rows && !s.line.is_empty(),
            })
            .collect()
    }

    /// Every widget's rectangle in a body of `area`, px from the body's top left corner, in
    /// drawing order. `line` has the drawn widths of the temperature and power values.
    pub fn rects(&self, area: Size<Pixels>, line: &LineWidths) -> Vec<(&Part, Bounds<Pixels>)> {
        let mut out = Vec::new();
        match self.look {
            PlanLook::Cells => self.cell_rects(area, &mut out),
            PlanLook::Rows | PlanLook::Chart => self.row_rects(area, line, &mut out),
        }
        out
    }

    /// `engine_charts`: the engine cells of each section's grid and its VRAM row, as
    /// `GpuBox::charts_body` lays them out.
    fn cell_rects<'p>(&'p self, area: Size<Pixels>, out: &mut Vec<(&'p Part, Bounds<Pixels>)>) {
        let m = &self.m;
        let (row, gap, section_gap) = spacing(m);
        let engines: SmallVec<[usize; MAX_GPUS]> =
            self.sections.iter().map(|s| s.engines.len()).collect();
        let fit = charts_fit(self.arrange, area, &engines, self.total, self.vram, m);
        let n = fit.sections.min(self.sections.len());
        let more = self.total > n;
        let min = cell_min(m);
        let items: SmallVec<[FlexItem; MAX_GPUS]> = (0..n)
            .map(|i| {
                let extra = usize::from(i + 1 == n && more);
                let grid = arrange_min_h(self.arrange, engines[i], area.width, min, m.sp[2]);
                FlexItem {
                    basis: charts_fixed_h(engines[i], extra, self.vram, i > 0, m) + f32::from(grid),
                    grow: 1.,
                    shrink: 1.,
                    min: 0.,
                }
            })
            .collect();
        let heights = flex_col(&items, f32::from(area.height), section_gap);
        let mut top = 0.;
        for (i, s) in self.sections.iter().take(n).enumerate() {
            let g = fit.grids[i];
            let head = if i > 0 { row + gap } else { 0. };
            let rows = SectionRows {
                engines: usize::from(g.shown < engines[i]),
                vram: self.vram,
                line: false,
            };
            let keep = kept(rows, i + 1 == n && more, if i == 0 { fit.cut } else { 0 });
            let grid_top = top + head;
            let (cw, ch) = (f32::from(g.grid.cell.width), f32::from(g.grid.cell.height));
            let cols = g.grid.cols.max(1);
            for (c, part) in s.engines.iter().take(g.shown).enumerate() {
                let Some(part) = part else {
                    continue;
                };
                let (r, q) = ((c / cols) as f32, (c % cols) as f32);
                out.push((
                    part,
                    rect(q * (cw + gap), grid_top + r * (ch + gap), cw, ch),
                ));
            }
            if let Some(vram) = s.vram.as_ref().filter(|_| keep.vram) {
                // The rows under the grid sit at the section's bottom; the grid takes the rest.
                let k = usize::from(keep.engines) + 1 + usize::from(keep.more);
                let rest_h = k as f32 * row + (k - 1) as f32 * gap;
                let rest_top = if engines[i] > 0 {
                    grid_top + (heights[i] - head - gap - rest_h).max(0.) + gap
                } else {
                    grid_top
                };
                let y = rest_top + if keep.engines { row + gap } else { 0. };
                out.push((vram, rect(0., y, f32::from(area.width), row)));
            }
            top += heights[i] + section_gap;
        }
    }

    /// `chart_engines` and `chart`: each section's chart, engine rows, VRAM row and line, as
    /// `GpuBox::list_body` lays them out.
    fn row_rects<'p>(
        &'p self,
        area: Size<Pixels>,
        line: &LineWidths,
        out: &mut Vec<(&'p Part, Bounds<Pixels>)>,
    ) {
        let m = &self.m;
        let (row, gap, section_gap) = spacing(m);
        let with_rows = self.look == PlanLook::Rows;
        let side = self.side_share.filter(|_| with_rows);
        let rows = self.list_rows();
        let shown = fit(area, &rows, self.total, side, m);
        let n = shown.sections.min(self.sections.len());
        let more = self.total > n;
        let (w, h) = (f32::from(area.width), f32::from(area.height));
        let chart_min = MIN_CHART_H * m.ui_scale;
        let rows_h = |l: usize| {
            if l == 0 {
                0.
            } else {
                l as f32 * row + (l - 1) as f32 * gap
            }
        };
        let keeps: SmallVec<[Kept; MAX_GPUS]> = (0..n)
            .map(|i| {
                kept(
                    rows[i],
                    i + 1 == n && more,
                    if i == 0 { shown.cut } else { 0 },
                )
            })
            .collect();
        let lines: SmallVec<[usize; MAX_GPUS]> = keeps
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let engines = if k.engines {
                    engine_lines(rows[i].engines, shown.collapsed && i == 0, shown.busiest)
                } else {
                    0
                };
                engines + usize::from(k.vram) + usize::from(k.line) + usize::from(k.more)
            })
            .collect();
        // The box draws the first chart at this height, from the same function at layout.
        let custom = self
            .stacked_share
            .filter(|_| with_rows && side.is_none() && shown.chart)
            .map(|share| stacked_chart_h(share, &lines, h, m));
        let items: SmallVec<[FlexItem; MAX_GPUS]> = (0..n)
            .map(|i| {
                let l = lines[i];
                let head = if i > 0 { row + gap } else { 0. };
                let (basis, grow, min) = if !shown.chart {
                    (0., 1., rows_h(l))
                } else if side.is_some() {
                    (section_h(l, i > 0, true, m), 1., head)
                } else if let Some(ch) = custom.filter(|_| i == 0) {
                    (ch + l as f32 * (gap + row), 0., ch + gap + rows_h(l))
                } else {
                    (
                        section_h(l, i > 0, false, m),
                        1.,
                        head + chart_min + gap + rows_h(l),
                    )
                };
                FlexItem {
                    basis,
                    grow,
                    shrink: 0.,
                    min,
                }
            })
            .collect();
        let heights = flex_col(&items, h, section_gap);
        let mut top = 0.;
        for (i, s) in self.sections.iter().take(n).enumerate() {
            let (keep, l) = (keeps[i], lines[i]);
            let head = if i > 0 { row + gap } else { 0. };
            let chart = s.chart.as_ref().filter(|_| shown.chart);
            // Where the rows start, and their left edge and width.
            let (x, rw, mut y) = if !shown.chart {
                (0., w, top + head)
            } else if let Some(share) = side {
                let parts_h = (heights[i] - head).max(chart_min);
                let cw = share * w;
                if let Some(c) = chart {
                    out.push((c, rect(0., top + head, cw, parts_h)));
                }
                let sp = f32::from(m.sp[5]);
                (cw + sp, (w - cw - sp).max(0.), top + head)
            } else {
                let ch = match custom.filter(|_| i == 0) {
                    Some(ch) => ch,
                    None => (heights[i] - head - gap - rows_h(l)).max(chart_min),
                };
                if let Some(c) = chart {
                    out.push((c, rect(0., top + head, w, ch)));
                }
                (0., w, top + head + ch + gap)
            };
            let mut next = || {
                let at = y;
                y += row + gap;
                at
            };
            if with_rows && keep.engines {
                let collapsed = shown.collapsed && i == 0;
                if collapsed && shown.busiest {
                    let busiest = self.busiest.min(s.engines.len().saturating_sub(1));
                    let at = next();
                    if let Some(Some(p)) = s.engines.get(busiest) {
                        out.push((p, rect(x, at, rw, row)));
                    }
                    next();
                } else if collapsed {
                    next();
                } else {
                    for p in &s.engines {
                        let at = next();
                        if let Some(p) = p {
                            out.push((p, rect(x, at, rw, row)));
                        }
                    }
                }
            }
            if keep.vram {
                let at = next();
                if let Some(v) = &s.vram {
                    out.push((v, rect(x, at, rw, row)));
                }
            }
            if with_rows && keep.line {
                // The values the box drew sit side by side at their text's width, joined by
                // " · ". A value it did not draw (no reading yet) keeps a place with no width.
                let at = next();
                let mut lx = x;
                let mut first = true;
                for p in &s.line {
                    match line.width(p.0) {
                        Some(w) => {
                            if !first {
                                lx += line.sep;
                            }
                            first = false;
                            out.push((p, rect(lx, at, w, row)));
                            lx += w;
                        }
                        None => out.push((p, rect(lx, at, 0., row))),
                    }
                }
            }
            top += heights[i] + section_gap;
        }
    }
}

/// A rectangle from its left, top, width and height in px.
fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
}

/// The least height under the first chart of a stacked `chart_engines` body whose sections
/// show `lines` body rows each, logical px: the first section's rows and the later sections at
/// their smallest. Dragging the split down stops there, so every engine row and the VRAM row
/// stay in the body (crw.73).
pub(super) fn stacked_min_second(lines: &[usize], m: &UiMetrics) -> f32 {
    let (_, _, section_gap) = spacing(m);
    let n = lines.len() as f32;
    let basis: f32 = lines
        .iter()
        .enumerate()
        .map(|(i, l)| section_h(*l, i > 0, false, m))
        .sum();
    let under = basis + (n - 1.).max(0.) * section_gap - MIN_CHART_H * m.ui_scale;
    ((under - f32::from(m.sp[5])) / m.ui_scale).max(0.)
}

/// The first chart's height, px, in a stacked `chart_engines` body `len` px high whose split
/// was moved to `share` and whose sections show `lines` body rows each: the share clamped to
/// the split range and the parts' minimums, times the body height. The box draws the chart at
/// this height at every layout (`GpuBox::list_body`), and the placer places the chart widget
/// with it, so the two never differ.
pub fn stacked_chart_h(share: f32, lines: &[usize], len: f32, m: &UiMetrics) -> f32 {
    let mins = (MIN_CHART_H, stacked_min_second(lines, m));
    let share = clamp_split(share, len, f32::from(m.sp[5]), mins, m.ui_scale);
    (share * len).max(MIN_CHART_H * m.ui_scale)
}

/// The widths of the temperature and power values the box drew on their line, px, by widget
/// id, and the width of the " · " between two of them. A value the box did not draw has none.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct LineWidths {
    pub values: SmallVec<[(u32, f32); 4]>,
    pub sep: f32,
}

impl LineWidths {
    /// The drawn width of widget `id`'s value.
    pub fn width(&self, id: u32) -> Option<f32> {
        self.values.iter().find(|(i, _)| *i == id).map(|(_, w)| *w)
    }
}

/// The width the line's small text `text` takes when drawn: the window's shaping of it in the
/// `t_xs` style over the inherited text style, rounded up to a whole px as GPUI's text layout
/// rounds a text's width.
pub fn line_text_w(text: &str, m: &UiMetrics, window: &Window) -> f32 {
    let mut style = window.text_style();
    style.refine(&t_xs(m));
    let run = style.to_run(text.len());
    let font_size = style.font_size.to_pixels(window.rem_size());
    let w = window
        .text_system()
        .layout_line(text, font_size, &[run], None)
        .width;
    f32::from(w).ceil()
}

/// One item of a flex column: its basis, its grow and shrink factors and its least height, px.
#[derive(Clone, Copy, Debug)]
struct FlexItem {
    basis: f32,
    grow: f32,
    shrink: f32,
    min: f32,
}

/// The heights a flex column `avail` px high, with `gap` between its items, gives `items`: the
/// flexible lengths of CSS flexbox (section 9.7) without max sizes. Spare height goes by the
/// grow factors; missing height comes off by the shrink factors times the bases. An item is
/// never lower than its least height.
fn flex_col(items: &[FlexItem], avail: f32, gap: f32) -> SmallVec<[f32; MAX_GPUS]> {
    let n = items.len();
    let mut out: SmallVec<[f32; MAX_GPUS]> = items.iter().map(|i| i.basis.max(i.min)).collect();
    if n == 0 {
        return out;
    }
    let inner = avail - gap * (n - 1) as f32;
    let grow = out.iter().sum::<f32>() < inner;
    let weight = |i: &FlexItem| if grow { i.grow } else { i.shrink * i.basis };
    let mut frozen: SmallVec<[bool; MAX_GPUS]> = items
        .iter()
        .map(|i| weight(i) <= 0. || (!grow && i.basis < i.min))
        .collect();
    while frozen.iter().any(|f| !f) {
        let used: f32 = (0..n)
            .map(|k| if frozen[k] { out[k] } else { items[k].basis })
            .sum();
        let free = inner - used;
        let total: f32 = (0..n)
            .filter(|k| !frozen[*k])
            .map(|k| weight(&items[k]))
            .sum();
        let mut clamped: SmallVec<[bool; MAX_GPUS]> = SmallVec::new();
        for k in 0..n {
            if frozen[k] {
                clamped.push(false);
                continue;
            }
            let share = if total > 0. {
                free * weight(&items[k]) / total
            } else {
                0.
            };
            let raw = items[k].basis + share;
            out[k] = raw.max(items[k].min);
            clamped.push(raw < items[k].min);
        }
        if !clamped.iter().any(|c| *c) {
            break;
        }
        for k in 0..n {
            frozen[k] |= clamped[k];
        }
    }
    out
}

/// What the GPU box placed and drew: the plan of its last flow render, the widgets that
/// render drew, in drawing order, and the widths of the line values it drew.
#[derive(Default, Debug)]
struct PlanState {
    plan: Option<FlowPlan>,
    drawn: SmallVec<[u32; 16]>,
    line: LineWidths,
}

/// The [`FlowPlan`] shared by the box (which makes it) and its [`placer`].
#[derive(Clone, Default, Debug)]
pub struct PlanCell(Rc<RefCell<PlanState>>);

impl PlanCell {
    /// Takes the plan of a render. Returns whether it can place a widget elsewhere, so the box
    /// must place its widgets again. On a sample only the busiest engine of a collapsed first
    /// section can do that: a new busiest engine in a section that is not collapsed moves
    /// nothing.
    pub fn set(&self, plan: FlowPlan) -> bool {
        let mut st = self.0.borrow_mut();
        let changed = match st.plan.as_mut() {
            Some(old) => {
                if !plan.collapsed {
                    old.busiest = plan.busiest;
                }
                *old != plan
            }
            None => true,
        };
        st.plan = Some(plan);
        changed
    }

    /// The plan of the last flow render.
    pub fn plan(&self) -> Option<FlowPlan> {
        self.0.borrow().plan.clone()
    }

    /// The widgets a render drew, in drawing order.
    pub fn set_drawn(&self, drawn: SmallVec<[u32; 16]>) {
        self.0.borrow_mut().drawn = drawn;
    }

    /// The widgets the last flow render drew, in drawing order.
    pub fn drawn(&self) -> SmallVec<[u32; 16]> {
        self.0.borrow().drawn.clone()
    }

    /// The widths of the line values a render drew. They are not part of the plan: a value's
    /// text changes on a sample, and a sample places nothing (only the next placement uses
    /// the new widths).
    pub fn set_line(&self, line: LineWidths) {
        self.0.borrow_mut().line = line;
    }

    /// The widths of the line values the last flow render drew.
    pub fn line(&self) -> LineWidths {
        self.0.borrow().line.clone()
    }
}

/// The placer of the GPU box's flow layouts: every widget of the box's plan at the rectangle
/// the fit rules give it in the container's body now (`FlowPlan::rects`), when the layout has
/// that widget for the same metric. A widget the rules leave out (cut, or not in the plan) is
/// not placed, so it has no entity. In Edit Widgets, when none of the layout's widgets is in
/// the plan (the layout is not the one the box shows, as after Reset), the widgets go in list
/// order (`container::list_flow`).
pub fn placer(plan: PlanCell) -> FlowPlace {
    Rc::new(move |input: &FlowInput<'_>| place(&plan, input))
}

fn place(cell: &PlanCell, input: &FlowInput<'_>) -> InnerPlacement {
    let mut widgets = Vec::new();
    {
        let st = cell.0.borrow();
        let preset = input.layout.preset.as_ref().map(|p| p.id.as_str());
        if let Some(plan) = st.plan.as_ref().filter(|p| preset == Some(p.look.id())) {
            for ((id, metric), bounds) in plan.rects(input.area, &st.line) {
                let Some(item) = input.items.iter().find(|i| i.setting.id == *id) else {
                    continue;
                };
                if *metric != item.setting.source.metric
                    || item.availability == Availability::Unknown
                {
                    continue;
                }
                widgets.push(PlacedWidget { id: *id, bounds });
            }
        }
    }
    if widgets.is_empty() && input.frozen.is_some() {
        return list_flow(input);
    }
    let mut placement = grid::place(
        &[],
        grid_cols(&input.layout.container),
        input.area,
        input.scale,
        input.frozen,
    );
    placement.content_h = widgets
        .iter()
        .map(|w| w.bounds.bottom())
        .fold(px(0.), Pixels::max);
    placement.widgets = widgets;
    placement
}

#[cfg(test)]
mod tests {
    use tl_model::layout::GPU_BOX_CONTAINER;

    use super::super::gpu_box::{CELL_MIN_H, CELL_MIN_W};

    use super::*;

    #[test]
    fn flow_layout_is_the_converted_list() {
        let l = flow_layout("chart_engines", Some(Arrange::Auto), None, &[]);
        assert_eq!(l.container, GPU_BOX_CONTAINER);
        assert_eq!(
            l.preset.as_ref().map(|p| p.id.as_str()),
            Some("chart_engines")
        );
        assert!(chart_of(&l, 0).is_some() && chart_of(&l, 1).is_some());
        assert!(shows_engine(&l, 0, "gpu") && shows_engine(&l, 1, "enc"));
        assert!(!shows_engine(&l, 0, "memctl"), "no memory controller row");
        assert_eq!(shows_line(&l, 0), (true, true));
        assert!(shows_vram(&l, 1));
        // The section chart is not the engine row.
        assert_ne!(chart_of(&l, 0), engine_widget(&l, 0, "gpu"));
    }

    #[test]
    fn hidden_names_leave_out_widgets() {
        let hidden = ["Video Encode".to_owned(), "VRAM".to_owned()];
        let l = flow_layout("engine_charts", None, None, &hidden);
        assert!(!shows_engine(&l, 0, "enc") && !shows_engine(&l, 1, "enc"));
        assert!(shows_engine(&l, 0, "dec"));
        assert!(!shows_vram(&l, 0));
        assert_eq!(
            shows_line(&l, 0),
            (false, false),
            "no line in engine_charts"
        );
        let chart = flow_layout("chart", None, None, &[]);
        assert!(chart_of(&chart, 0).is_some());
        assert!(!shows_engine(&chart, 0, "gpu"), "chart has no engine rows");
        assert!(shows_vram(&chart, 0));
    }

    fn part(id: u32, metric: &str) -> Part {
        (id, MetricKey::new(metric))
    }

    /// `chart_engines` with one GPU: 3D, encode and decode rows, VRAM, temperature and power.
    fn rows_plan() -> FlowPlan {
        let m = crate::theme::metrics::compute(&Default::default(), 0);
        FlowPlan {
            look: PlanLook::Rows,
            arrange: Arrange::Auto,
            m,
            total: 1,
            vram: true,
            sections: smallvec::smallvec![PlanSection {
                chart: Some(part(1, "engine:gpu")),
                engines: smallvec::smallvec![
                    Some(part(2, "engine:gpu")),
                    Some(part(3, "engine:enc")),
                    Some(part(4, "engine:dec")),
                ],
                vram: Some(part(5, METRIC_VRAM)),
                line: smallvec::smallvec![part(6, METRIC_TEMP), part(7, METRIC_POWER)],
            }],
            side_share: None,
            stacked_share: None,
            busiest: 2,
            collapsed: false,
        }
    }

    /// The line's values drawn 30 px (temperature) and 44 px (power) wide, 9 px apart.
    fn line_widths() -> LineWidths {
        LineWidths {
            values: smallvec::smallvec![(6, 30.), (7, 44.)],
            sep: 9.,
        }
    }

    fn rects_of(plan: &FlowPlan, w: f32, h: f32) -> Vec<(u32, [f32; 4])> {
        rects_with(plan, w, h, &line_widths())
    }

    fn rects_with(plan: &FlowPlan, w: f32, h: f32, line: &LineWidths) -> Vec<(u32, [f32; 4])> {
        plan.rects(gpui_kit::size(px(w), px(h)), line)
            .into_iter()
            .map(|((id, _), b)| {
                (
                    *id,
                    [
                        f32::from(b.origin.x),
                        f32::from(b.origin.y),
                        f32::from(b.size.width),
                        f32::from(b.size.height),
                    ],
                )
            })
            .collect()
    }

    /// The 1.1.0 parts: the chart takes the spare height, the 18 px rows sit under it 4 px
    /// apart, and the line's two values sit side by side at their drawn widths, the " · "
    /// between them.
    #[test]
    fn rows_plan_places_every_part() {
        let r = rects_of(&rows_plan(), 400., 300.);
        assert_eq!(
            r,
            vec![
                (1, [0., 0., 400., 190.]),
                (2, [0., 194., 400., 18.]),
                (3, [0., 216., 400., 18.]),
                (4, [0., 238., 400., 18.]),
                (5, [0., 260., 400., 18.]),
                (6, [0., 282., 30., 18.]),
                (7, [39., 282., 44., 18.]),
            ]
        );
    }

    /// A 152 px body collapses the section to its busiest engine and "+2 more engines".
    #[test]
    fn collapsed_plan_places_the_busiest_engine() {
        let r = rects_of(&rows_plan(), 400., 152.);
        assert_eq!(
            r,
            vec![
                (1, [0., 0., 400., 64.]),
                (4, [0., 68., 400., 18.]),
                (5, [0., 112., 400., 18.]),
                (6, [0., 134., 30., 18.]),
                (7, [39., 134., 44., 18.]),
            ]
        );
    }

    /// Side by side the chart takes its share of the width and the rows the rest, 12 px apart.
    #[test]
    fn side_plan_puts_rows_beside_the_chart() {
        let plan = FlowPlan {
            side_share: Some(0.5),
            ..rows_plan()
        };
        let r = rects_of(&plan, 400., 300.);
        assert_eq!(r[0], (1, [0., 0., 200., 300.]));
        assert_eq!(r[1], (2, [212., 0., 188., 18.]));
        assert_eq!(r[4], (5, [212., 66., 188., 18.]));
    }

    /// A value with no reading is not drawn: it keeps a place with no width, and the other
    /// value starts the line with no " · " before it.
    #[test]
    fn line_value_not_drawn_has_no_width() {
        let line = LineWidths {
            values: smallvec::smallvec![(7, 44.)],
            sep: 9.,
        };
        let r = rects_with(&rows_plan(), 400., 300., &line);
        assert_eq!(r[5], (6, [0., 282., 0., 18.]));
        assert_eq!(r[6], (7, [0., 282., 44., 18.]));
    }

    /// Stacked with the split moved, the chart's height follows the body height through
    /// `stacked_chart_h`, and the rows sit under it.
    #[test]
    fn stacked_split_follows_the_body_height() {
        let plan = FlowPlan {
            stacked_share: Some(0.5),
            ..rows_plan()
        };
        let m = plan.m;
        for h in [300., 420.] {
            let r = rects_of(&plan, 400., h);
            let ch = stacked_chart_h(0.5, &[5], h, &m);
            assert_eq!(ch, h / 2., "half the body");
            assert_eq!(r[0], (1, [0., 0., 400., ch]));
            assert_eq!(r[1], (2, [0., ch + 4., 400., 18.]));
        }
        // A share that leaves too little room under the chart is clamped: the 5 rows need
        // 5 x 22 = 110 px, so a 300 px body keeps 300 - 110 = 190 px for the chart.
        let low = rects_of(
            &FlowPlan {
                stacked_share: Some(0.8),
                ..plan
            },
            400.,
            300.,
        );
        assert_eq!(low[0].1[3], stacked_chart_h(0.8, &[5], 300., &m));
        assert!((low[0].1[3] - 190.).abs() < 1e-3, "{low:?}");
    }

    /// `engine_charts`: the cells fill the grid above the VRAM row, which sits at the bottom.
    #[test]
    fn cells_plan_places_cells_and_vram() {
        let plan = FlowPlan {
            look: PlanLook::Cells,
            sections: smallvec::smallvec![PlanSection {
                chart: None,
                engines: smallvec::smallvec![
                    Some(part(2, "engine:gpu")),
                    Some(part(3, "engine:enc")),
                ],
                vram: Some(part(5, METRIC_VRAM)),
                line: SmallVec::new(),
            }],
            ..rows_plan()
        };
        let r = rects_of(&plan, 400., 300.);
        assert_eq!(r.len(), 3, "{r:?}");
        let (vram, cells) = (r[2].1, [r[0].1, r[1].1]);
        assert_eq!(vram, [0., 282., 400., 18.]);
        for c in cells {
            assert!(
                c[1] + c[3] <= 278. + 1e-3,
                "a cell above the VRAM row: {c:?}"
            );
            assert!(c[2] >= CELL_MIN_W && c[3] >= CELL_MIN_H, "{c:?}");
        }
        assert_eq!(cells[0][0], 0.);
    }

    /// The flex column: spare height by the grow factors, missing height by shrink times basis,
    /// and never below an item's least height.
    #[test]
    fn flex_col_follows_css() {
        let item = |basis, grow, shrink, min| FlexItem {
            basis,
            grow,
            shrink,
            min,
        };
        let grow = flex_col(&[item(100., 1., 0., 0.), item(50., 1., 0., 0.)], 262., 12.);
        assert_eq!(grow.as_slice(), [150., 100.]);
        let fixed = flex_col(&[item(100., 0., 0., 0.), item(50., 1., 0., 0.)], 262., 12.);
        assert_eq!(fixed.as_slice(), [100., 150.]);
        let shrink = flex_col(&[item(200., 1., 1., 0.), item(100., 1., 1., 0.)], 162., 12.);
        assert_eq!(shrink.as_slice(), [100., 50.]);
        let held = flex_col(
            &[item(200., 1., 1., 180.), item(100., 1., 1., 0.)],
            162.,
            12.,
        );
        assert_eq!(held.as_slice(), [180., 0.]);
    }

    /// Only a change that can move a widget asks for a new placement: a new busiest engine
    /// counts only while the first section is collapsed.
    #[test]
    fn plan_cell_ignores_the_busiest_engine_unless_collapsed() {
        let cell = PlanCell::default();
        assert!(cell.set(rows_plan()), "the first plan");
        assert!(!cell.set(rows_plan()), "the same plan");
        let busier = FlowPlan {
            busiest: 0,
            ..rows_plan()
        };
        assert!(!cell.set(busier.clone()), "not collapsed: nothing moves");
        let collapsed = FlowPlan {
            collapsed: true,
            ..busier
        };
        assert!(cell.set(collapsed.clone()), "the section collapsed");
        assert!(!cell.set(collapsed.clone()), "same busiest engine");
        assert!(
            cell.set(FlowPlan {
                busiest: 1,
                ..collapsed
            }),
            "another busiest engine"
        );
        let mut no_temp = rows_plan();
        no_temp.sections[0].line.remove(0);
        assert!(
            cell.set(no_temp),
            "the GPU stopped reporting its temperature"
        );
    }
}
