//! The Overview page's layout as data (UI-PRF-017, UI-PRF-018, D-023): rows top to bottom, each
//! with a height weight and its boxes left to right, each box with a width weight. Minimum sizes
//! come from the box kind. The profile saves it as `[performance.overview]`
//! ([`OverviewLayoutSetting`]); [`OverviewLayout::from_setting`] and
//! [`OverviewLayout::to_setting`] convert. Each box kind also has a layout mode and a split
//! (UI-PRF-019, D-025), and an arrangement where its mode offers one (D-027), kept per kind in
//! [`BoxConfig`] and saved on the box's entry.
//!
//! [`place`] turns a layout, the kinds present now and the page area into box bounds. It runs on
//! resize and on presence changes, never per tick, and does not allocate for up to 8 boxes.

use gpui_kit::{Bounds, Pixels, Size, point, px, size};
use smallvec::{SmallVec, smallvec};
use tl_model::settings::{
    OVERVIEW_WEIGHT_RANGE, OverviewBoxSetting, OverviewLayoutSetting, OverviewRowSetting,
};

use super::super::page::PAGE_PAD;
use super::{cpu_box, disk_box, gpu_box, mem_box, net_box, procs_box};
use crate::theme::DeviceAccent;

/// The gap between boxes and rows (`sp4`), logical px.
const GAP: f32 = 8.;

/// Below this page width the boxes stack in one column, logical px.
pub const STACK_W: f32 = 720.;

/// The Overview boxes: the five of the default layout and the opt-in Top processes box
/// (crw.45).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BoxKind {
    Cpu,
    Memory,
    Gpu,
    Disk,
    Net,
    TopProcs,
}

impl BoxKind {
    pub const ALL: [Self; 6] = [
        Self::Cpu,
        Self::Memory,
        Self::Gpu,
        Self::Disk,
        Self::Net,
        Self::TopProcs,
    ];

    /// The colour family shared by this box, its charts and its sidebar card. The optional Top
    /// processes box uses the CPU family because its only measure is CPU utilization.
    pub const fn accent(self) -> DeviceAccent {
        match self {
            Self::Cpu | Self::TopProcs => DeviceAccent::Cpu,
            Self::Memory => DeviceAccent::Memory,
            Self::Gpu => DeviceAccent::Gpu,
            Self::Disk => DeviceAccent::Disk,
            Self::Net => DeviceAccent::Net,
        }
    }

    /// The kind's name in `[performance.overview]` and in the frame selector.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Gpu => "gpu",
            Self::Disk => "disk",
            Self::Net => "net",
            Self::TopProcs => "processes",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == key)
    }

    /// Resolves the stable frame selector passed by every Overview box.
    pub fn from_frame_id(id: &str) -> Option<Self> {
        Self::from_key(id.strip_prefix("perf-overview-box-")?)
    }

    /// The smallest height the box is placed at, logical px.
    pub const fn min_h(self) -> f32 {
        match self {
            Self::Cpu => 280.,
            Self::TopProcs => 260.,
            _ => 220.,
        }
    }

    /// The smallest width the box is placed at in the wide layout, logical px.
    pub const fn min_w(self) -> f32 {
        match self {
            Self::Cpu | Self::TopProcs => 280.,
            _ => 200.,
        }
    }

    /// The layout modes the box offers, its default first (UI-PRF-019).
    pub fn modes(self) -> &'static [BoxMode] {
        match self {
            Self::Cpu => cpu_box::MODES,
            Self::Memory => mem_box::MODES,
            Self::Gpu => gpu_box::MODES,
            Self::Disk => disk_box::MODES,
            Self::Net => net_box::MODES,
            Self::TopProcs => procs_box::MODES,
        }
    }

    /// The mode `id` of this kind.
    pub fn mode(self, id: &str) -> Option<&'static BoxMode> {
        self.modes().iter().find(|m| m.id == id)
    }

    /// What the "Arrange" submenu does in mode `id`; `None` when the mode has no Arrange
    /// choice (UI-PRF-019, D-027).
    pub fn arrange(self, id: &str) -> Option<ArrangeChoice> {
        let choices: &[(&str, ArrangeChoice)] = match self {
            Self::Disk => disk_box::ARRANGE,
            Self::Net => net_box::ARRANGE,
            _ => &[],
        };
        choices.iter().find(|(m, _)| *m == id).map(|&(_, c)| c)
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One layout mode of a box (UI-PRF-019): its id in `[performance.overview]`, the string key
/// of its "Layout" menu item, and its default split when it has two parts.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BoxMode {
    pub id: &'static str,
    pub label: &'static str,
    pub split: Option<f32>,
}

/// How a mode places several charts of one kind (UI-PRF-019 "Arrange", D-027): the mode's own
/// grid rule, one column, or one row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arrange {
    Auto,
    Vertical,
    Horizontal,
}

impl Arrange {
    /// The menu order.
    pub const ALL: [Self; 3] = [Self::Auto, Self::Vertical, Self::Horizontal];

    /// The value in `[performance.overview]`.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Vertical => "vertical",
            Self::Horizontal => "horizontal",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    /// The string key of its "Arrange" menu item.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "perf-overview-arrange-auto",
            Self::Vertical => "perf-overview-arrange-vertical",
            Self::Horizontal => "perf-overview-arrange-horizontal",
        }
    }
}

/// What a mode's "Arrange" submenu does (UI-PRF-019, D-027).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ArrangeChoice {
    /// Automatic, Stack vertically and Side by side, saved as the box's `arrange`, with the
    /// mode's default.
    Saved(Arrange),
    /// Stack vertically and Side by side switch between two modes that show the same two
    /// charts, such as the Net box's `stacked` and `columns`.
    Modes {
        vertical: &'static str,
        horizontal: &'static str,
    },
}

impl ArrangeChoice {
    /// The default arrangement of a [`ArrangeChoice::Saved`] choice.
    fn saved_default(self) -> Option<Arrange> {
        match self {
            Self::Saved(a) => Some(a),
            Self::Modes { .. } => None,
        }
    }
}

/// The smallest and largest split.
pub const SPLIT_RANGE: (f32, f32) = (0.2, 0.8);

/// The smallest and largest share of the CPU box body the power draw strip takes.
pub const POWER_SPLIT_RANGE: (f32, f32) = (0.1, 0.5);

/// The default share of the CPU box body the power draw strip takes.
pub const POWER_SPLIT_DEFAULT: f32 = 0.25;

/// A box's mode, split and arrangement. `split` is the first part's share, `None` in a mode
/// with one part. `arrange` is `None` in a mode without a saved Arrange choice. `power` and
/// `power_split` are the CPU box's power draw strip (D-028): it is shown and takes this share
/// of the body; other kinds keep them at their defaults.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BoxConfig {
    pub mode: &'static BoxMode,
    pub split: Option<f32>,
    pub arrange: Option<Arrange>,
    pub power: bool,
    pub power_split: f32,
}

impl BoxConfig {
    /// `kind`'s `mode` with its default split and arrangement.
    pub fn of(kind: BoxKind, mode: &'static BoxMode) -> Self {
        Self {
            mode,
            split: mode.split,
            arrange: kind.arrange(mode.id).and_then(ArrangeChoice::saved_default),
            power: false,
            power_split: POWER_SPLIT_DEFAULT,
        }
    }

    /// The default mode of `kind`, its split and its arrangement.
    pub fn default_for(kind: BoxKind) -> Self {
        Self::of(kind, &kind.modes()[0])
    }

    /// The saved entry's mode, split and arrangement: a mode `kind` does not offer is its
    /// default, and a split out of range or missing and an arrangement unknown or missing are
    /// the mode's default.
    fn from_setting(kind: BoxKind, b: &OverviewBoxSetting) -> Self {
        let mode = b
            .mode
            .as_deref()
            .and_then(|id| kind.mode(id))
            .unwrap_or(&kind.modes()[0]);
        let saved = b
            .split
            .map(|v| v as f32)
            .filter(|v| (SPLIT_RANGE.0..=SPLIT_RANGE.1).contains(v));
        let default = Self::of(kind, mode);
        Self {
            mode,
            split: mode.split.map(|d| saved.unwrap_or(d)),
            arrange: default
                .arrange
                .map(|d| b.arrange.as_deref().and_then(Arrange::from_id).unwrap_or(d)),
            power: kind == BoxKind::Cpu && b.power,
            power_split: b
                .power_split
                .map(|v| v as f32)
                .filter(|v| (POWER_SPLIT_RANGE.0..=POWER_SPLIT_RANGE.1).contains(v))
                .filter(|_| kind == BoxKind::Cpu)
                .unwrap_or(POWER_SPLIT_DEFAULT),
        }
    }

    /// `power` when on, and `power_split` when not the default.
    fn power_setting(self) -> (bool, Option<f64>) {
        let split = ((self.power_split - POWER_SPLIT_DEFAULT).abs() >= 5e-4)
            .then(|| f64::from(self.power_split));
        (self.power, split)
    }

    /// The mode when not the kind's default, the split when not the mode's default, and the
    /// arrangement when not the mode's default.
    fn to_setting(self, kind: BoxKind) -> (Option<String>, Option<f64>, Option<String>) {
        let mode = (self.mode.id != kind.modes()[0].id).then(|| self.mode.id.to_owned());
        let split = self
            .split
            .zip(self.mode.split)
            .filter(|(s, d)| (s - d).abs() >= 5e-4)
            .map(|(s, _)| f64::from(s));
        let arrange = self
            .arrange
            .filter(|&a| Some(a) != Self::of(kind, self.mode).arrange)
            .map(|a| a.id().to_owned());
        (mode, split, arrange)
    }
}

/// One box: its kind and its share of the row width.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BoxSpec {
    pub kind: BoxKind,
    pub width: f32,
}

/// One row: its share of the page height and its boxes, left to right.
#[derive(Clone, PartialEq, Debug)]
pub struct RowSpec {
    pub height: f32,
    pub boxes: SmallVec<[BoxSpec; 4]>,
}

/// The whole layout: rows top to bottom.
#[derive(Clone, PartialEq, Debug)]
pub struct OverviewLayout {
    pub rows: SmallVec<[RowSpec; 4]>,
    /// Each kind's mode and split in [`BoxKind::ALL`] order, kept while its box is hidden.
    configs: [BoxConfig; 6],
}

/// Why a layout is not valid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutError {
    /// No row.
    Empty,
    /// The row at this index has no box.
    EmptyRow(usize),
    /// A height or width that is not finite and above 0.
    BadWeight,
    /// A kind appears twice.
    Duplicate(BoxKind),
}

/// A finite weight above 0.
fn good_weight(w: f32) -> bool {
    w.is_finite() && w > 0.
}

/// A saved weight clamped into [`OVERVIEW_WEIGHT_RANGE`]. A weight that is NaN, infinite, zero
/// or negative becomes 0, which fails `validate`.
fn saved_weight(w: f64) -> f32 {
    let (min, max) = OVERVIEW_WEIGHT_RANGE;
    if w.is_finite() && w > 0. {
        w.clamp(min, max) as f32
    } else {
        0.
    }
}

impl OverviewLayout {
    /// The btop-style default: CPU (width 2) and Memory on a row of height 3, then GPU, Disk and
    /// Net on a row of height 2.
    pub fn btop_default() -> Self {
        let b = |kind, width| BoxSpec { kind, width };
        Self {
            configs: BoxKind::ALL.map(BoxConfig::default_for),
            rows: smallvec![
                RowSpec {
                    height: 3.,
                    boxes: smallvec![b(BoxKind::Cpu, 2.), b(BoxKind::Memory, 1.)],
                },
                RowSpec {
                    height: 2.,
                    boxes: smallvec![
                        b(BoxKind::Gpu, 1.),
                        b(BoxKind::Disk, 1.),
                        b(BoxKind::Net, 1.)
                    ],
                },
            ],
        }
    }

    /// The saved layout, or the default when there is none. Loading already dropped unknown
    /// kinds, repeats, bad weights and empty rows. Weights are clamped into
    /// [`OVERVIEW_WEIGHT_RANGE`] again, so none rounds to 0 in `f32`; anything still invalid
    /// gives the default.
    pub fn from_setting(setting: Option<&OverviewLayoutSetting>) -> Self {
        let Some(setting) = setting else {
            return Self::btop_default();
        };
        let mut seen: SmallVec<[BoxKind; 8]> = SmallVec::new();
        let mut configs = BoxKind::ALL.map(BoxConfig::default_for);
        let rows = setting
            .rows
            .iter()
            .map(|r| RowSpec {
                height: saved_weight(r.height),
                boxes: r
                    .boxes
                    .iter()
                    .filter_map(|b| {
                        let kind = BoxKind::from_key(&b.kind)?;
                        (!seen.contains(&kind)).then(|| {
                            seen.push(kind);
                            configs[kind.index()] = BoxConfig::from_setting(kind, b);
                            BoxSpec {
                                kind,
                                width: saved_weight(b.width),
                            }
                        })
                    })
                    .collect(),
            })
            .collect();
        let layout = Self { rows, configs };
        if layout.validate().is_ok() {
            layout
        } else {
            Self::btop_default()
        }
    }

    /// The layout as `[performance.overview]` data.
    pub fn to_setting(&self) -> OverviewLayoutSetting {
        OverviewLayoutSetting {
            rows: self
                .rows
                .iter()
                .map(|r| OverviewRowSetting {
                    height: f64::from(r.height),
                    boxes: r
                        .boxes
                        .iter()
                        .map(|b| {
                            let config = self.config(b.kind);
                            let (mode, split, arrange) = config.to_setting(b.kind);
                            let (power, power_split) = config.power_setting();
                            OverviewBoxSetting {
                                kind: b.kind.key().to_owned(),
                                width: f64::from(b.width),
                                mode,
                                split,
                                arrange,
                                cpu_split: None,
                                power,
                                power_split,
                            }
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// `kind`'s mode and split.
    pub fn config(&self, kind: BoxKind) -> BoxConfig {
        self.configs[kind.index()]
    }

    /// `kind`'s split, `None` when its mode has one part.
    pub fn split(&self, kind: BoxKind) -> Option<f32> {
        self.config(kind).split
    }

    /// `kind`'s arrangement, `None` when its mode has no saved Arrange choice.
    pub fn arrange(&self, kind: BoxKind) -> Option<Arrange> {
        self.config(kind).arrange
    }

    /// Switches `kind` to its mode `id` with that mode's default split and arrangement. The CPU
    /// box keeps its power draw strip.
    /// Returns `false` when the kind has no such mode or already shows it.
    pub fn set_mode(&mut self, kind: BoxKind, id: &str) -> bool {
        let Some(mode) = kind.mode(id).filter(|m| m.id != self.config(kind).mode.id) else {
            return false;
        };
        // The power draw strip belongs to the box, not to a mode.
        let old = self.config(kind);
        self.configs[kind.index()] = BoxConfig {
            power: old.power,
            power_split: old.power_split,
            ..BoxConfig::of(kind, mode)
        };
        true
    }

    /// Sets `kind`'s arrangement when its mode has a saved Arrange choice. Returns whether it
    /// changed.
    pub fn set_arrange(&mut self, kind: BoxKind, arrange: Arrange) -> bool {
        let config = &mut self.configs[kind.index()];
        if config.arrange.is_none_or(|a| a == arrange) {
            return false;
        }
        config.arrange = Some(arrange);
        true
    }

    /// Sets `kind`'s split when its mode has two parts. Returns whether it changed.
    pub fn set_split(&mut self, kind: BoxKind, share: f32) -> bool {
        let config = &mut self.configs[kind.index()];
        if config.split.is_none_or(|s| s == share) {
            return false;
        }
        config.split = Some(share.clamp(SPLIT_RANGE.0, SPLIT_RANGE.1));
        true
    }

    /// Shows or hides the CPU box's power draw strip. Returns whether it changed; any other
    /// kind has no strip.
    pub fn set_power(&mut self, kind: BoxKind, on: bool) -> bool {
        let config = &mut self.configs[kind.index()];
        if kind != BoxKind::Cpu || config.power == on {
            return false;
        }
        config.power = on;
        true
    }

    /// Sets the strip's share of the CPU box body. Returns whether it changed.
    pub fn set_power_split(&mut self, kind: BoxKind, share: f32) -> bool {
        let config = &mut self.configs[kind.index()];
        let share = share.clamp(POWER_SPLIT_RANGE.0, POWER_SPLIT_RANGE.1);
        if kind != BoxKind::Cpu || config.power_split == share {
            return false;
        }
        config.power_split = share;
        true
    }

    /// Gives `kind` its default mode, split and arrangement (the box menu's "Reset Box").
    /// Returns whether it changed.
    pub fn reset_box(&mut self, kind: BoxKind) -> bool {
        let default = BoxConfig::default_for(kind);
        let changed = self.config(kind) != default;
        self.configs[kind.index()] = default;
        changed
    }

    pub fn has(&self, kind: BoxKind) -> bool {
        self.rows
            .iter()
            .flat_map(|r| &r.boxes)
            .any(|b| b.kind == kind)
    }

    pub fn validate(&self) -> Result<(), LayoutError> {
        if self.rows.is_empty() {
            return Err(LayoutError::Empty);
        }
        let mut seen: SmallVec<[BoxKind; 8]> = SmallVec::new();
        for (i, row) in self.rows.iter().enumerate() {
            if row.boxes.is_empty() {
                return Err(LayoutError::EmptyRow(i));
            }
            if !good_weight(row.height) {
                return Err(LayoutError::BadWeight);
            }
            for b in &row.boxes {
                if !good_weight(b.width) {
                    return Err(LayoutError::BadWeight);
                }
                if seen.contains(&b.kind) {
                    return Err(LayoutError::Duplicate(b.kind));
                }
                seen.push(b.kind);
            }
        }
        Ok(())
    }
}

/// One placed box.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlacedBox {
    pub kind: BoxKind,
    pub bounds: Bounds<Pixels>,
}

/// Where the present boxes go, in row order. `content_h` includes the padding; the page
/// scrolls when `scrolls`. `stacked` is the one-column layout below [`STACK_W`].
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Placement {
    pub boxes: SmallVec<[PlacedBox; 8]>,
    pub content_h: Pixels,
    pub scrolls: bool,
    pub stacked: bool,
}

/// Splits `total` by the weights of `items` (weight, minimum). An item whose share would fall
/// below its minimum takes the minimum and the others share what is left. Returns `false` when
/// the minimums do not fit; every item then has its minimum.
///
/// The sums and products run in `f64`: any finite `f32` weight is valid, and with weights near
/// `f32::MAX` the `f32` sum or `left * w` would overflow to infinity.
fn share(total: f32, items: &[(f32, f32)], out: &mut SmallVec<[f32; 8]>) -> bool {
    let mut fixed: SmallVec<[bool; 8]> = smallvec![false; items.len()];
    out.clear();
    out.extend(items.iter().map(|&(_, min)| min));
    loop {
        let mut left = f64::from(total);
        let mut free_w = 0.;
        for (&(w, min), &f) in items.iter().zip(&fixed) {
            if f {
                left -= f64::from(min);
            } else {
                free_w += f64::from(w);
            }
        }
        if free_w <= 0. {
            break;
        }
        let mut more = false;
        for ((&(w, min), f), v) in items.iter().zip(fixed.iter_mut()).zip(out.iter_mut()) {
            if *f {
                continue;
            }
            *v = (left * f64::from(w) / free_w) as f32;
            if *v < min {
                *v = min;
                *f = true;
                more = true;
            }
        }
        if !more {
            break;
        }
    }
    items.iter().map(|&(_, min)| min).sum::<f32>() <= total + 0.01
}

/// Bounds with every edge rounded to a whole px, so neighbours never overlap or leave a seam.
fn snapped(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    let (left, top) = (x.round(), y.round());
    let (right, bottom) = ((x + w).round(), (y + h).round());
    Bounds::new(
        point(px(left), px(top)),
        size(px(right - left), px(bottom - top)),
    )
}

/// One line of boxes in the wide layout: its row, the range of `shown` it holds, and whether
/// it is an extra line of a row whose minimum widths did not fit.
#[derive(Clone, Copy)]
struct Line {
    row: usize,
    start: usize,
    end: usize,
    extra: bool,
}

/// Places the boxes of `layout` whose kind is `present` in `area`. Logical sizes are × `scale`.
///
/// - Padding `PAGE_PAD` on every side; gap 8 between boxes and rows. Edges round to whole px.
/// - A row's present boxes share `row_w − (n − 1)·gap` by `width`. Rows with no present box
///   drop out; the rest share `H − 2·pad − (rows − 1)·gap` by `height`.
/// - A box or row that would come out below its kind minimum takes the minimum and the others
///   share what is left. When the rows' minimum heights do not fit, the page scrolls.
/// - When a row's minimum widths do not fit, its last boxes wrap onto extra lines at the row's
///   minimum height, so the page never scrolls sideways.
/// - Below `STACK_W` the boxes stack in row order, full width, each at its kind minimum.
pub fn place(
    layout: &OverviewLayout,
    present: impl Fn(BoxKind) -> bool,
    area: Size<Pixels>,
    scale: f32,
) -> Placement {
    let pad = PAGE_PAD * scale;
    let gap = GAP * scale;
    let (w, h) = (f32::from(area.width), f32::from(area.height));
    let inner_w = (w - 2. * pad).max(0.);

    // The present boxes in row order, with their row index.
    let shown: SmallVec<[(usize, BoxSpec); 8]> = layout
        .rows
        .iter()
        .enumerate()
        .flat_map(|(r, row)| row.boxes.iter().map(move |b| (r, *b)))
        .filter(|(_, b)| present(b.kind))
        .collect();
    let mut placement = Placement::default();
    if shown.is_empty() {
        placement.content_h = px(2. * pad);
        return placement;
    }

    if w < STACK_W * scale {
        placement.stacked = true;
        let mut y = pad;
        for (i, (_, b)) in shown.iter().enumerate() {
            if i > 0 {
                y += gap;
            }
            let box_h = b.kind.min_h() * scale;
            placement.boxes.push(PlacedBox {
                kind: b.kind,
                bounds: snapped(pad, y, inner_w, box_h),
            });
            y += box_h;
        }
        let content_h = y.round() + pad;
        placement.content_h = px(content_h);
        placement.scrolls = content_h > h;
        return placement;
    }

    // Break each row into lines whose minimum widths fit, and find each row's minimum height.
    let mut lines: SmallVec<[Line; 8]> = SmallVec::new();
    let mut row_min_h: SmallVec<[f32; 4]> = smallvec![0.; layout.rows.len()];
    let mut line_w = 0.;
    for (i, &(r, b)) in shown.iter().enumerate() {
        let min_w = b.kind.min_w() * scale;
        row_min_h[r] = row_min_h[r].max(b.kind.min_h() * scale);
        match lines.last_mut() {
            Some(line) if line.row == r && line_w + gap + min_w <= inner_w => {
                line.end = i + 1;
                line_w += gap + min_w;
            }
            last => {
                let extra = last.is_some_and(|l| l.row == r);
                lines.push(Line {
                    row: r,
                    start: i,
                    end: i + 1,
                    extra,
                });
                line_w = min_w;
            }
        }
    }

    // Extra lines take their row's minimum height; the main lines share the rest by weight.
    let mut avail_h = h - 2. * pad - (lines.len() as f32 - 1.) * gap;
    let mut rows: SmallVec<[(f32, f32); 8]> = SmallVec::new();
    for line in &lines {
        if line.extra {
            avail_h -= row_min_h[line.row];
        } else {
            rows.push((layout.rows[line.row].height, row_min_h[line.row]));
        }
    }
    let mut heights = SmallVec::new();
    placement.scrolls = !share(avail_h, &rows, &mut heights);

    let mut y = pad;
    let mut main = heights.iter();
    let mut widths = SmallVec::new();
    let mut items: SmallVec<[(f32, f32); 8]> = SmallVec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            y += gap;
        }
        let line_h = if line.extra {
            row_min_h[line.row]
        } else {
            main.next().copied().unwrap_or(row_min_h[line.row])
        };
        let in_line = &shown[line.start..line.end];
        items.clear();
        items.extend(
            in_line
                .iter()
                .map(|(_, b)| (b.width, b.kind.min_w() * scale)),
        );
        let gaps_w = (in_line.len() as f32 - 1.) * gap;
        share(inner_w - gaps_w, &items, &mut widths);
        let mut x = pad;
        for ((_, b), box_w) in in_line.iter().zip(&widths) {
            placement.boxes.push(PlacedBox {
                kind: b.kind,
                bounds: snapped(x, y, *box_w, line_h),
            });
            x += box_w + gap;
        }
        y += line_h;
    }
    placement.content_h = px(y.round() + pad);
    placement
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: f32, h: f32) -> Size<Pixels> {
        size(px(w), px(h))
    }

    /// (x, y, w, h) of `kind`.
    fn rect(p: &Placement, kind: BoxKind) -> (f32, f32, f32, f32) {
        let b = p
            .boxes
            .iter()
            .find(|b| b.kind == kind)
            .expect("placed")
            .bounds;
        (
            f32::from(b.origin.x),
            f32::from(b.origin.y),
            f32::from(b.size.width),
            f32::from(b.size.height),
        )
    }

    /// One row per entry: (height, [(kind, width)]).
    fn layout(rows: &[(f32, &[(BoxKind, f32)])]) -> OverviewLayout {
        OverviewLayout {
            rows: rows
                .iter()
                .map(|(height, boxes)| RowSpec {
                    height: *height,
                    boxes: boxes
                        .iter()
                        .map(|&(kind, width)| BoxSpec { kind, width })
                        .collect(),
                })
                .collect(),
            configs: BoxKind::ALL.map(BoxConfig::default_for),
        }
    }

    #[test]
    fn box_split_converts_to_and_from_the_setting() {
        let mut layout = OverviewLayout::btop_default();
        assert_eq!(layout.to_setting(), OverviewLayoutSetting::btop_default());
        assert_eq!(layout.split(BoxKind::Cpu), Some(0.4));
        assert!(layout.set_split(BoxKind::Cpu, 0.55));
        assert!(!layout.set_split(BoxKind::Cpu, 0.55), "same value");
        let setting = layout.to_setting();
        assert_eq!(setting.rows[0].boxes[0].split, Some(f64::from(0.55_f32)));
        assert_eq!(setting.rows[0].boxes[0].mode, None, "the default mode");
        assert_eq!(setting.rows[0].boxes[1].split, None);
        assert_eq!(OverviewLayout::from_setting(Some(&setting)), layout);
        // A saved value out of range is the default.
        let mut bad = setting;
        bad.rows[0].boxes[0].split = Some(0.95);
        assert_eq!(
            OverviewLayout::from_setting(Some(&bad)).split(BoxKind::Cpu),
            Some(0.4)
        );
        // A kind whose mode has one part keeps no split.
        bad.rows[0].boxes[1].mode = Some("chart".to_owned());
        bad.rows[0].boxes[1].split = Some(0.5);
        let back = OverviewLayout::from_setting(Some(&bad));
        assert_eq!(back.split(BoxKind::Memory), None);
        let mut chart = back;
        assert!(!chart.set_split(BoxKind::Memory, 0.5));
        // Reset Box restores the default and Reset Layout every box.
        assert!(layout.reset_box(BoxKind::Cpu));
        assert!(!layout.reset_box(BoxKind::Cpu));
        assert_eq!(layout, OverviewLayout::btop_default());
    }

    #[test]
    fn box_mode_unknown_loads_default() {
        // An id another kind has, or none has, loads as the kind's default mode.
        let mut setting = OverviewLayoutSetting::btop_default();
        setting.rows[0].boxes[0].mode = Some("rows".to_owned());
        setting.rows[0].boxes[1].mode = Some("bogus".to_owned());
        let layout = OverviewLayout::from_setting(Some(&setting));
        for kind in [BoxKind::Cpu, BoxKind::Memory] {
            assert_eq!(layout.config(kind), BoxConfig::default_for(kind));
        }
        // The default mode is not written; switching to a mode the kind lacks does nothing.
        assert_eq!(layout.to_setting(), OverviewLayoutSetting::btop_default());
        let mut edited = layout.clone();
        assert!(!edited.set_mode(BoxKind::Cpu, "rows"));
        assert!(
            !edited.set_mode(BoxKind::Cpu, cpu_box::MODES[0].id),
            "already shown"
        );
        assert_eq!(edited, layout);
        // Every kind offers its default first, with a menu label.
        for kind in BoxKind::ALL {
            let modes = kind.modes();
            assert!(!modes.is_empty(), "{kind:?}");
            assert!(
                modes
                    .iter()
                    .all(|m| m.label.starts_with("perf-overview-mode-"))
            );
            assert_eq!(kind.mode(modes[0].id), Some(&modes[0]));
        }
    }

    #[test]
    fn overview_default_layout_validates() {
        let default = OverviewLayout::btop_default();
        assert_eq!(default.validate(), Ok(()));
        assert!(!default.has(BoxKind::TopProcs));
        // The saved form of the default converts both ways.
        assert_eq!(default.to_setting(), OverviewLayoutSetting::btop_default());
        assert_eq!(OverviewLayout::from_setting(None), default);
        assert_eq!(
            OverviewLayout::from_setting(Some(&OverviewLayoutSetting::btop_default())),
            default
        );
    }

    #[test]
    fn overview_frame_ids_resolve_device_accents() {
        use DeviceAccent::*;

        for (id, kind, accent) in [
            ("perf-overview-box-cpu", BoxKind::Cpu, Cpu),
            ("perf-overview-box-memory", BoxKind::Memory, Memory),
            ("perf-overview-box-gpu", BoxKind::Gpu, Gpu),
            ("perf-overview-box-disk", BoxKind::Disk, Disk),
            ("perf-overview-box-net", BoxKind::Net, Net),
            ("perf-overview-box-processes", BoxKind::TopProcs, Cpu),
        ] {
            assert_eq!(BoxKind::from_frame_id(id), Some(kind));
            assert_eq!(kind.accent(), accent);
        }
        assert_eq!(BoxKind::from_frame_id("perf-overview-box-other"), None);
        let keys: Vec<&str> = BoxKind::ALL.iter().map(|k| k.key()).collect();
        assert_eq!(keys, tl_model::settings::OVERVIEW_BOX_KINDS);
    }

    #[test]
    fn overview_top_procs_layout_adds_a_full_row() {
        let layout = OverviewLayout::from_setting(Some(&OverviewLayoutSetting::with_top_procs()));
        assert_eq!(layout.validate(), Ok(()));
        assert!(layout.has(BoxKind::TopProcs));
        let p = place(&layout, |_| true, area(1200., 1000.), 1.);
        let procs = p.boxes.last().expect("placed");
        assert_eq!(procs.kind, BoxKind::TopProcs);
        let cpu = p.boxes.first().expect("placed");
        let net = p
            .boxes
            .iter()
            .find(|b| b.kind == BoxKind::Net)
            .expect("placed");
        assert_eq!(procs.bounds.left(), cpu.bounds.left());
        assert_eq!(procs.bounds.right(), net.bounds.right());
        assert!(procs.bounds.size.height >= px(260.));
    }

    #[test]
    fn overview_place_default_1200x900() {
        let p = place(
            &OverviewLayout::btop_default(),
            |_| true,
            area(1200., 900.),
            1.,
        );
        assert!(!p.stacked && !p.scrolls);
        assert_eq!(rect(&p, BoxKind::Cpu), (16., 16., 773., 516.));
        assert_eq!(rect(&p, BoxKind::Memory), (797., 16., 387., 516.));
        for (kind, x) in [
            (BoxKind::Gpu, 16.),
            (BoxKind::Disk, 408.),
            (BoxKind::Net, 800.),
        ] {
            assert_eq!(rect(&p, kind), (x, 540., 384., 344.), "{kind:?}");
        }
        assert_eq!(p.content_h, px(900.));
        let order: Vec<BoxKind> = p.boxes.iter().map(|b| b.kind).collect();
        assert_eq!(
            order,
            [
                BoxKind::Cpu,
                BoxKind::Memory,
                BoxKind::Gpu,
                BoxKind::Disk,
                BoxKind::Net
            ]
        );
    }

    #[test]
    fn overview_place_custom_weights() {
        use BoxKind::*;
        let l = layout(&[
            (1., &[(Cpu, 1.), (Memory, 1.)]),
            (1., &[(Gpu, 1.), (Disk, 2.), (Net, 1.)]),
        ]);
        let p = place(&l, |_| true, area(1200., 900.), 1.);
        assert!(!p.scrolls);
        assert_eq!(rect(&p, Cpu), (16., 16., 580., 430.));
        assert_eq!(rect(&p, Memory), (604., 16., 580., 430.));
        // 1168 − 2 gaps = 1152 split 1:2:1.
        assert_eq!(rect(&p, Gpu), (16., 454., 288., 430.));
        assert_eq!(rect(&p, Disk), (312., 454., 576., 430.));
        assert_eq!(rect(&p, Net), (896., 454., 288., 430.));
    }

    #[test]
    fn overview_place_clamps_to_kind_minimum() {
        use BoxKind::*;
        // CPU's share of 1160 at 1:4 is 232, below its 280 minimum; the first row's share of
        // 860 at 1:9 is 86, below CPU's 280.
        let l = layout(&[(1., &[(Cpu, 1.), (Memory, 4.)]), (9., &[(Net, 1.)])]);
        let p = place(&l, |_| true, area(1200., 900.), 1.);
        assert!(!p.scrolls);
        assert_eq!(rect(&p, Cpu), (16., 16., 280., 280.));
        assert_eq!(rect(&p, Memory), (304., 16., 880., 280.));
        assert_eq!(rect(&p, Net), (16., 304., 1168., 580.));
        assert_eq!(p.content_h, px(900.));

        // A row without CPU has the minimum of the kinds it holds.
        let l = layout(&[(9., &[(Cpu, 1.)]), (1., &[(Memory, 1.), (Disk, 1.)])]);
        let p = place(&l, |_| true, area(1200., 900.), 1.);
        assert_eq!(rect(&p, Disk).3, 220.);
        assert_eq!(rect(&p, Cpu).3, 640.);
    }

    #[test]
    fn overview_place_wraps_row_below_minimum() {
        use BoxKind::*;
        // One row of five: 280 + 4 × 200 + 4 gaps = 1112 does not fit 968, so Net wraps onto
        // an extra line at the row's 280 minimum.
        let l = layout(&[(
            1.,
            &[(Cpu, 1.), (Memory, 1.), (Gpu, 1.), (Disk, 1.), (Net, 1.)],
        )]);
        let p = place(&l, |_| true, area(1000., 900.), 1.);
        assert!(!p.scrolls && !p.stacked);
        // 900 − 32 − 8 − 280 = 580 for the main line; CPU keeps its minimum width.
        assert_eq!(rect(&p, Cpu), (16., 16., 280., 580.));
        assert_eq!(rect(&p, Disk).0 + rect(&p, Disk).2, 984.);
        assert_eq!(rect(&p, Net), (16., 604., 968., 280.));
        assert!(p.boxes.iter().all(|b| b.bounds.right() <= px(984.)));
        assert_eq!(p.content_h, px(900.));

        // Without GPU the row fits again.
        let p = place(&l, |k| k != Gpu, area(1000., 900.), 1.);
        assert_eq!(rect(&p, Net).1, 16.);
    }

    #[test]
    fn overview_place_without_gpu_widens_row() {
        let p = place(
            &OverviewLayout::btop_default(),
            |k| k != BoxKind::Gpu,
            area(1200., 900.),
            1.,
        );
        assert!(p.boxes.iter().all(|b| b.kind != BoxKind::Gpu));
        assert_eq!(rect(&p, BoxKind::Disk), (16., 540., 580., 344.));
        assert_eq!(rect(&p, BoxKind::Net), (604., 540., 580., 344.));
        // A row with no present box is dropped: the top row takes the whole height.
        let top_only = place(
            &OverviewLayout::btop_default(),
            |k| matches!(k, BoxKind::Cpu | BoxKind::Memory),
            area(1200., 900.),
            1.,
        );
        assert_eq!(rect(&top_only, BoxKind::Cpu), (16., 16., 773., 868.));
    }

    #[test]
    fn overview_place_narrow_stacks() {
        let p = place(
            &OverviewLayout::btop_default(),
            |_| true,
            area(700., 900.),
            1.,
        );
        assert!(p.stacked);
        assert!(p.boxes.iter().all(|b| b.bounds.size.width == px(668.)));
        // Each box at its kind minimum: 280 + 4 × 220, 4 gaps of 8 and 2 × 16 padding.
        assert_eq!(p.content_h, px(1224.));
        assert!(p.scrolls);
        assert_eq!(rect(&p, BoxKind::Memory), (16., 304., 668., 220.));
        assert_eq!(rect(&p, BoxKind::Net), (16., 988., 668., 220.));
    }

    #[test]
    fn overview_place_short_uses_min_heights() {
        let p = place(
            &OverviewLayout::btop_default(),
            |_| true,
            area(1200., 500.),
            1.,
        );
        assert!(p.scrolls && !p.stacked);
        assert_eq!(rect(&p, BoxKind::Cpu).3, 280.);
        assert_eq!(rect(&p, BoxKind::Disk).3, 220.);
        assert_eq!(p.content_h, px(540.));
    }

    #[test]
    fn overview_place_scales() {
        let p = place(
            &OverviewLayout::btop_default(),
            |_| true,
            area(2400., 1800.),
            2.,
        );
        assert_eq!(rect(&p, BoxKind::Cpu), (32., 32., 1547., 1032.));
        assert_eq!(rect(&p, BoxKind::Memory), (1595., 32., 773., 1032.));
    }

    #[test]
    fn overview_place_huge_weights_stay_finite() {
        use BoxKind::*;
        // Any finite weight is valid. Equal huge weights place like equal weights of 1, with no
        // infinite or NaN bounds from an overflowing sum or product.
        let ones = place(
            &layout(&[(1., &[(Cpu, 1.), (Memory, 1.)]), (1., &[(Net, 1.)])]),
            |_| true,
            area(1200., 900.),
            1.,
        );
        for huge in [1e38, f32::MAX] {
            let l = layout(&[
                (huge, &[(Cpu, huge), (Memory, huge)]),
                (huge, &[(Net, huge)]),
            ]);
            assert_eq!(l.validate(), Ok(()));
            let p = place(&l, |_| true, area(1200., 900.), 1.);
            assert_eq!(p, ones, "{huge}");
        }

        // A huge weight beside a weight of 1 takes all but the other box's minimum.
        let l = layout(&[(1., &[(Cpu, 1e38), (Memory, 1.)])]);
        let p = place(&l, |_| true, area(1200., 900.), 1.);
        assert_eq!(rect(&p, Memory).2, Memory.min_w());
        assert_eq!(rect(&p, Cpu).2, 1168. - 8. - Memory.min_w());
    }

    #[test]
    fn overview_tiny_and_huge_saved_weights_clamp_and_place() {
        use BoxKind::*;
        // 1e-50 is 0 in f32 and 1e38 is near f32::MAX. Both clamp into 0.001..=1000, so the
        // saved layout is kept and every box has finite bounds.
        let mut saved = OverviewLayoutSetting::btop_default();
        saved.rows[0].height = 1e-50;
        saved.rows[0].boxes[0].width = 1e38;
        saved.rows[0].boxes[1].width = 1e-50;
        saved.rows[1].height = 1e38;
        let l = OverviewLayout::from_setting(Some(&saved));
        assert_ne!(l, OverviewLayout::btop_default());
        assert_eq!(l.rows[0].height, 0.001);
        assert_eq!(l.rows[0].boxes[0].width, 1000.);
        assert_eq!(l.rows[0].boxes[1].width, 0.001);
        assert_eq!(l.rows[1].height, 1000.);
        let p = place(&l, |_| true, area(1200., 900.), 1.);
        assert_eq!(p.boxes.len(), 5);
        assert!(!p.scrolls);
        for b in &p.boxes {
            let (x, y, w, h) = rect(&p, b.kind);
            assert!([x, y, w, h].iter().all(|v| v.is_finite()), "{:?}", b.kind);
            assert!(w >= b.kind.min_w() && h >= b.kind.min_h(), "{:?}", b.kind);
        }
        // The tiny row and box get their minimums, the huge ones the rest.
        assert_eq!(rect(&p, Memory).2, Memory.min_w());
        assert_eq!(rect(&p, Cpu).2, 1168. - 8. - Memory.min_w());
        assert_eq!(rect(&p, Cpu).3, Cpu.min_h());
        assert_eq!(rect(&p, Gpu).3, 900. - 32. - 8. - Cpu.min_h());
    }

    #[test]
    fn overview_validate_rejects_bad_weight_and_duplicate() {
        let mut dup = OverviewLayout::btop_default();
        dup.rows[1].boxes[2].kind = BoxKind::Cpu;
        assert_eq!(dup.validate(), Err(LayoutError::Duplicate(BoxKind::Cpu)));

        for bad in [0., -1., f32::NAN, f32::INFINITY] {
            let mut width = OverviewLayout::btop_default();
            width.rows[0].boxes[1].width = bad;
            assert_eq!(width.validate(), Err(LayoutError::BadWeight), "{bad}");
            let mut height = OverviewLayout::btop_default();
            height.rows[1].height = bad;
            assert_eq!(height.validate(), Err(LayoutError::BadWeight), "{bad}");
        }

        let mut empty_row = OverviewLayout::btop_default();
        empty_row.rows[1].boxes.clear();
        assert_eq!(empty_row.validate(), Err(LayoutError::EmptyRow(1)));

        let mut empty = OverviewLayout::btop_default();
        empty.rows.clear();
        assert_eq!(empty.validate(), Err(LayoutError::Empty));

        // A saved layout that does not validate gives the default.
        for bad in [0., -1., f64::NAN, f64::INFINITY] {
            let mut saved = OverviewLayoutSetting::btop_default();
            saved.rows[0].height = bad;
            assert_eq!(
                OverviewLayout::from_setting(Some(&saved)),
                OverviewLayout::btop_default(),
                "{bad}"
            );
        }
    }
}
