//! The Overview page's layout as data (UI-PRF-017, UI-PRF-056, D-030): the boxes on a snap grid
//! of 12 columns and rows of a unit height, each box with its top left cell and its span.
//! Minimum sizes come from the box kind. The profile saves it as `[performance.overview]`
//! ([`OverviewLayoutSetting`]); [`OverviewLayout::from_setting`] and
//! [`OverviewLayout::to_setting`] convert. Each box kind also has a layout mode and a split
//! (UI-PRF-019, D-025), and an arrangement where its mode offers one (D-027), kept per kind in
//! [`BoxConfig`] and saved on the box's entry.
//!
//! [`place`] turns a layout, the kinds present now and the page area into box bounds. It runs on
//! resize and on presence changes, never per tick, and does not allocate for up to 8 boxes.

use gpui_kit::{Bounds, Pixels, Size, point, px, size};
use smallvec::{SmallVec, smallvec};
use tl_model::settings::{OverviewGridSetting, OverviewLayoutSetting};

use super::super::page::PAGE_PAD;
use super::{cpu_box, disk_box, gpu_box, mem_box, net_box, procs_box, timeline};
use crate::theme::DeviceAccent;

/// The grid's columns (UI-PRF-056).
pub const GRID_COLS: u16 = 12;

/// The gutter between grid cells (`sp3`), logical px.
const GUTTER: f32 = 6.;

/// The default height of one grid row, logical px (`performance.overview_row_unit_px`).
pub const ROW_UNIT_DEFAULT: f32 = 40.;

/// The smallest span of any box in columns and rows (UI-PRF-056).
const MIN_SPAN: (u16, u16) = (3, 4);

/// Below this page width the boxes stack in one column, logical px.
pub const STACK_W: f32 = 720.;

/// The gap between stacked boxes, logical px.
const STACK_GAP: f32 = 8.;

/// The Overview boxes: the five device boxes and the 24-hour timeline of the default layout
/// (UI-PRF-057), and the opt-in Top processes box (crw.45).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BoxKind {
    Cpu,
    Memory,
    Gpu,
    Disk,
    Net,
    TopProcs,
    Timeline,
}

impl BoxKind {
    pub const ALL: [Self; 7] = [
        Self::Cpu,
        Self::Memory,
        Self::Gpu,
        Self::Disk,
        Self::Net,
        Self::TopProcs,
        Self::Timeline,
    ];

    /// The colour family shared by this box, its charts and its sidebar card. The optional Top
    /// processes box uses the CPU family because its only measure is CPU utilization; the
    /// timeline, which shows every device, uses it too.
    pub const fn accent(self) -> DeviceAccent {
        match self {
            Self::Cpu | Self::TopProcs | Self::Timeline => DeviceAccent::Cpu,
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
            Self::Timeline => "timeline",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == key)
    }

    /// Resolves the stable frame selector passed by every Overview box.
    pub fn from_frame_id(id: &str) -> Option<Self> {
        Self::from_key(id.strip_prefix("perf-overview-box-")?)
    }

    /// The kind minimum height, logical px: the height of a stacked box, and the rows a box
    /// spans at least after a move or resize (UI-PRF-056).
    pub const fn min_h(self) -> f32 {
        match self {
            Self::Cpu => 280.,
            Self::TopProcs => 260.,
            Self::Timeline => 160.,
            _ => 220.,
        }
    }

    /// The kind minimum width, logical px: the columns a box spans at least after a move or
    /// resize (UI-PRF-056).
    pub const fn min_w(self) -> f32 {
        match self {
            Self::Cpu | Self::TopProcs => 280.,
            Self::Timeline => 400.,
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
            Self::Timeline => timeline::MODES,
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
            Self::Cpu => cpu_box::ARRANGE,
            Self::Memory => mem_box::ARRANGE,
            Self::Gpu => gpu_box::ARRANGE,
            Self::Disk => disk_box::ARRANGE,
            Self::Net => net_box::ARRANGE,
            Self::TopProcs | Self::Timeline => &[],
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
    fn from_setting(kind: BoxKind, b: &OverviewGridSetting) -> Self {
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

/// One box on the grid: its kind, its top left cell, and its span in columns and rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GridBox {
    pub kind: BoxKind,
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

impl GridBox {
    /// The first column right of the box.
    pub const fn right(self) -> u16 {
        self.col + self.cols
    }

    /// The first row under the box.
    pub const fn bottom(self) -> u16 {
        self.row + self.rows
    }

    /// Whether the two boxes share a cell.
    pub const fn overlaps(self, other: Self) -> bool {
        self.col < other.right()
            && other.col < self.right()
            && self.row < other.bottom()
            && other.row < self.bottom()
    }

    /// Inside the 12 columns, with spans of at least 1.
    const fn in_grid(self) -> bool {
        self.cols >= 1 && self.rows >= 1 && self.right() <= GRID_COLS
    }
}

/// The whole layout: the boxes on the grid, in no particular order.
#[derive(Clone, PartialEq, Debug)]
pub struct OverviewLayout {
    pub boxes: SmallVec<[GridBox; 8]>,
    /// Each kind's mode and split in [`BoxKind::ALL`] order, kept while its box is hidden.
    configs: [BoxConfig; 7],
    /// The GPU box's hidden engine charts by en-US name, `"VRAM"` for the VRAM row (D-029). Names
    /// the GPU does not report stay. Kept while the box is hidden.
    hidden_gpu: Vec<String>,
}

/// Why a layout is not valid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutError {
    /// No box.
    Empty,
    /// A box outside the 12 columns or with a span of 0.
    OutOfGrid(BoxKind),
    /// Two boxes share a cell.
    Overlap(BoxKind, BoxKind),
    /// A kind appears twice.
    Duplicate(BoxKind),
}

impl OverviewLayout {
    /// The default grid (UI-PRF-056): CPU 8 × 7 and Memory 4 × 7 on top, then GPU, Disk and
    /// Net 4 × 6 each from row 7, then the timeline 12 × 4 from row 13 (UI-PRF-057).
    pub fn grid_default() -> Self {
        let b = |kind, col, row, cols, rows| GridBox {
            kind,
            col,
            row,
            cols,
            rows,
        };
        Self {
            configs: BoxKind::ALL.map(BoxConfig::default_for),
            hidden_gpu: Vec::new(),
            boxes: smallvec![
                b(BoxKind::Cpu, 0, 0, 8, 7),
                b(BoxKind::Memory, 8, 0, 4, 7),
                b(BoxKind::Gpu, 0, 7, 4, 6),
                b(BoxKind::Disk, 4, 7, 4, 6),
                b(BoxKind::Net, 8, 7, 4, 6),
                b(BoxKind::Timeline, 0, 13, 12, 4),
            ],
        }
    }

    /// The saved layout, or the default when there is none. Loading already converted an old
    /// row layout and dropped unknown kinds, repeats, entries outside the grid and overlaps;
    /// anything still invalid gives the default.
    pub fn from_setting(setting: Option<&OverviewLayoutSetting>) -> Self {
        let Some(setting) = setting else {
            return Self::grid_default();
        };
        let mut configs = BoxKind::ALL.map(BoxConfig::default_for);
        let mut hidden_gpu: Vec<String> = Vec::new();
        let mut boxes: SmallVec<[GridBox; 8]> = SmallVec::new();
        for e in &setting.grid {
            let Some(kind) = BoxKind::from_key(&e.kind) else {
                continue;
            };
            let cell = |v: i64| u16::try_from(v).ok();
            let (Some(col), Some(row), Some(cols), Some(rows)) =
                (cell(e.col), cell(e.row), cell(e.cols), cell(e.rows))
            else {
                return Self::grid_default();
            };
            if boxes.iter().all(|b| b.kind != kind) {
                configs[kind.index()] = BoxConfig::from_setting(kind, e);
                if kind == BoxKind::Gpu {
                    hidden_gpu.clone_from(&e.hidden);
                }
            }
            boxes.push(GridBox {
                kind,
                col,
                row,
                cols,
                rows,
            });
        }
        let layout = Self {
            boxes,
            configs,
            hidden_gpu,
        };
        if layout.validate().is_ok() {
            layout
        } else {
            Self::grid_default()
        }
    }

    /// The layout as `[performance.overview]` data, in reading order.
    pub fn to_setting(&self) -> OverviewLayoutSetting {
        OverviewLayoutSetting {
            grid: self
                .reading_order()
                .into_iter()
                .map(|b| {
                    let config = self.config(b.kind);
                    let (mode, split, arrange) = config.to_setting(b.kind);
                    let (power, power_split) = config.power_setting();
                    OverviewGridSetting {
                        kind: b.kind.key().to_owned(),
                        col: i64::from(b.col),
                        row: i64::from(b.row),
                        cols: i64::from(b.cols),
                        rows: i64::from(b.rows),
                        mode,
                        split,
                        arrange,
                        power,
                        power_split,
                        hidden: if b.kind == BoxKind::Gpu {
                            self.hidden_gpu.clone()
                        } else {
                            Vec::new()
                        },
                    }
                })
                .collect(),
            rows: Vec::new(),
        }
    }

    /// The boxes by row, then column.
    pub fn reading_order(&self) -> SmallVec<[GridBox; 8]> {
        let mut boxes = self.boxes.clone();
        boxes.sort_by_key(|b| (b.row, b.col));
        boxes
    }

    /// `kind`'s box.
    pub fn get(&self, kind: BoxKind) -> Option<GridBox> {
        self.boxes.iter().copied().find(|b| b.kind == kind)
    }

    /// `kind`'s box, to change its cells.
    pub fn get_mut(&mut self, kind: BoxKind) -> Option<&mut GridBox> {
        self.boxes.iter_mut().find(|b| b.kind == kind)
    }

    /// The first place a box of `cols` × `rows` cells fits without overlapping any box,
    /// scanning rows top to bottom and columns left to right (UI-PRF-056 "Add Box").
    pub fn first_free(&self, cols: u16, rows: u16) -> (u16, u16) {
        let cols = cols.clamp(1, GRID_COLS);
        let bottom = self.boxes.iter().map(|b| b.bottom()).max().unwrap_or(0);
        for row in 0..=bottom {
            for col in 0..=GRID_COLS - cols {
                let probe = GridBox {
                    kind: BoxKind::Cpu,
                    col,
                    row,
                    cols,
                    rows,
                };
                if self.boxes.iter().all(|b| !b.overlaps(probe)) {
                    return (col, row);
                }
            }
        }
        (0, bottom)
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

    /// The GPU box's hidden engine charts, by en-US name; `"VRAM"` is the VRAM row.
    pub fn hidden_gpu(&self) -> &[String] {
        &self.hidden_gpu
    }

    /// Hides or shows the GPU box's engine chart `name` (the Engines submenu). Returns whether it
    /// changed; any other kind has no engine charts.
    pub fn set_engine_hidden(&mut self, kind: BoxKind, name: &str, hide: bool) -> bool {
        if kind != BoxKind::Gpu {
            return false;
        }
        let at = self.hidden_gpu.iter().position(|n| n == name);
        match (hide, at) {
            (true, None) => self.hidden_gpu.push(name.to_owned()),
            (false, Some(i)) => {
                self.hidden_gpu.remove(i);
            }
            _ => return false,
        }
        true
    }

    /// Gives `kind` its default mode, split, arrangement and shown engines (the box menu's
    /// "Reset Box"). Returns whether it changed.
    pub fn reset_box(&mut self, kind: BoxKind) -> bool {
        let default = BoxConfig::default_for(kind);
        let mut changed = self.config(kind) != default;
        self.configs[kind.index()] = default;
        if kind == BoxKind::Gpu && !self.hidden_gpu.is_empty() {
            self.hidden_gpu.clear();
            changed = true;
        }
        changed
    }

    pub fn has(&self, kind: BoxKind) -> bool {
        self.boxes.iter().any(|b| b.kind == kind)
    }

    pub fn validate(&self) -> Result<(), LayoutError> {
        if self.boxes.is_empty() {
            return Err(LayoutError::Empty);
        }
        for (i, b) in self.boxes.iter().enumerate() {
            if !b.in_grid() {
                return Err(LayoutError::OutOfGrid(b.kind));
            }
            for other in &self.boxes[..i] {
                if other.kind == b.kind {
                    return Err(LayoutError::Duplicate(b.kind));
                }
                if other.overlaps(*b) {
                    return Err(LayoutError::Overlap(other.kind, b.kind));
                }
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

/// The grid's cell sizes on a page, in px (logical px × scale): the column width `cw`, the row
/// height `unit`, the `gutter` between cells and the page padding `pad`.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct GridMetrics {
    pub cw: f32,
    pub unit: f32,
    pub gutter: f32,
    pub pad: f32,
    /// The UI scale, for the kind minimums.
    pub scale: f32,
}

impl GridMetrics {
    /// The grid on a page `width` px wide with rows of `unit` logical px:
    /// `cw = (W − 2·pad − 11·gutter) / 12`.
    pub fn new(width: f32, unit: f32, scale: f32) -> Self {
        let pad = PAGE_PAD * scale;
        let gutter = GUTTER * scale;
        let gutters = f32::from(GRID_COLS - 1) * gutter;
        Self {
            cw: ((width - 2. * pad - gutters) / f32::from(GRID_COLS)).max(0.),
            unit: unit * scale,
            gutter,
            pad,
            scale,
        }
    }

    /// The left edge of column `col`.
    pub fn x(&self, col: u16) -> f32 {
        self.pad + f32::from(col) * (self.cw + self.gutter)
    }

    /// The top edge of row `row`.
    pub fn y(&self, row: u16) -> f32 {
        self.pad + f32::from(row) * (self.unit + self.gutter)
    }

    /// The width of `cols` columns: `cols·cw + (cols − 1)·gutter`.
    pub fn w(&self, cols: u16) -> f32 {
        f32::from(cols) * self.cw + f32::from(cols.saturating_sub(1)) * self.gutter
    }

    /// The height of `rows` rows: `rows·unit + (rows − 1)·gutter`.
    pub fn h(&self, rows: u16) -> f32 {
        f32::from(rows) * self.unit + f32::from(rows.saturating_sub(1)) * self.gutter
    }

    /// The bounds of `b`'s cells, every edge rounded to a whole px.
    pub fn bounds(&self, b: GridBox) -> Bounds<Pixels> {
        snapped(self.x(b.col), self.y(b.row), self.w(b.cols), self.h(b.rows))
    }

    /// `kind`'s smallest span (UI-PRF-056): `max(3, ceil(min_w / cw))` columns, at most 12, and
    /// `max(4, ceil(min_h / unit))` rows.
    pub fn min_span(&self, kind: BoxKind) -> (u16, u16) {
        let (min_cols, min_rows) = MIN_SPAN;
        let span = |min: f32, cell: f32| {
            if cell > 0. {
                (min * self.scale / cell).ceil().min(f32::from(u16::MAX)) as u16
            } else {
                u16::MAX
            }
        };
        (
            span(kind.min_w(), self.cw).clamp(min_cols, GRID_COLS),
            span(kind.min_h(), self.unit).max(min_rows),
        )
    }
}

/// Where the present boxes go, in reading order. `content_h` includes the padding; the page
/// scrolls when `scrolls`. `stacked` is the one-column layout below [`STACK_W`]. `grid` is the
/// cell size of the page width, also when stacked.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Placement {
    pub boxes: SmallVec<[PlacedBox; 8]>,
    pub content_h: Pixels,
    pub scrolls: bool,
    pub stacked: bool,
    pub grid: GridMetrics,
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

/// Places the boxes of `layout` whose kind is `present` in `area` (UI-PRF-056). Logical sizes
/// are × `scale`; `unit` is the row height in logical px.
///
/// - 12 columns over the page width and rows of `unit`, `sp3` gutters, padding `PAGE_PAD`.
///   Edges round to whole px. An absent kind leaves its cells empty.
/// - The page scrolls when the boxes go past the bottom.
/// - Below `STACK_W` the boxes stack in reading order, full width, each at its kind minimum
///   height, 8 px apart.
pub fn place(
    layout: &OverviewLayout,
    present: impl Fn(BoxKind) -> bool,
    area: Size<Pixels>,
    scale: f32,
    unit: f32,
) -> Placement {
    let (w, h) = (f32::from(area.width), f32::from(area.height));
    let grid = GridMetrics::new(w, unit, scale);
    let pad = grid.pad;
    let mut placement = Placement {
        grid,
        ..Placement::default()
    };
    let mut shown = layout.reading_order();
    shown.retain(|b| present(b.kind));
    if shown.is_empty() {
        placement.content_h = px(2. * pad);
        return placement;
    }

    if w < STACK_W * scale {
        placement.stacked = true;
        let inner_w = (w - 2. * pad).max(0.);
        let gap = STACK_GAP * scale;
        let mut y = pad;
        for (i, b) in shown.iter().enumerate() {
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

    let mut bottom: f32 = 0.;
    for b in &shown {
        let bounds = grid.bounds(*b);
        bottom = bottom.max(f32::from(bounds.bottom()));
        placement.boxes.push(PlacedBox {
            kind: b.kind,
            bounds,
        });
    }
    let content_h = bottom + pad;
    placement.content_h = px(content_h);
    placement.scrolls = content_h > h;
    placement
}

#[cfg(test)]
mod tests {
    use tl_model::settings::{OVERVIEW_KIND_MIN, OVERVIEW_MIN_SPAN};

    use super::*;

    fn area(w: f32, h: f32) -> Size<Pixels> {
        size(px(w), px(h))
    }

    fn placed(layout: &OverviewLayout, w: f32, h: f32) -> Placement {
        place(layout, |_| true, area(w, h), 1., ROW_UNIT_DEFAULT)
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

    /// A layout of (kind, col, row, cols, rows) boxes.
    fn layout(boxes: &[(BoxKind, u16, u16, u16, u16)]) -> OverviewLayout {
        OverviewLayout {
            boxes: boxes
                .iter()
                .map(|&(kind, col, row, cols, rows)| GridBox {
                    kind,
                    col,
                    row,
                    cols,
                    rows,
                })
                .collect(),
            configs: BoxKind::ALL.map(BoxConfig::default_for),
            hidden_gpu: Vec::new(),
        }
    }

    #[test]
    fn box_split_converts_to_and_from_the_setting() {
        let mut layout = OverviewLayout::grid_default();
        assert_eq!(layout.to_setting(), OverviewLayoutSetting::grid_default());
        assert_eq!(layout.split(BoxKind::Cpu), Some(0.4));
        assert!(layout.set_split(BoxKind::Cpu, 0.55));
        assert!(!layout.set_split(BoxKind::Cpu, 0.55), "same value");
        let setting = layout.to_setting();
        assert_eq!(setting.grid[0].split, Some(f64::from(0.55_f32)));
        assert_eq!(setting.grid[0].mode, None, "the default mode");
        assert_eq!(setting.grid[1].split, None);
        assert_eq!(OverviewLayout::from_setting(Some(&setting)), layout);
        // A saved value out of range is the default.
        let mut bad = setting;
        bad.grid[0].split = Some(0.95);
        assert_eq!(
            OverviewLayout::from_setting(Some(&bad)).split(BoxKind::Cpu),
            Some(0.4)
        );
        // A kind whose mode has one part keeps no split.
        bad.grid[1].mode = Some("chart".to_owned());
        bad.grid[1].split = Some(0.5);
        let back = OverviewLayout::from_setting(Some(&bad));
        assert_eq!(back.split(BoxKind::Memory), None);
        let mut chart = back;
        assert!(!chart.set_split(BoxKind::Memory, 0.5));
        // Reset Box restores the default and Reset Layout every box.
        assert!(layout.reset_box(BoxKind::Cpu));
        assert!(!layout.reset_box(BoxKind::Cpu));
        assert_eq!(layout, OverviewLayout::grid_default());
    }

    #[test]
    fn box_mode_unknown_loads_default() {
        // An id another kind has, or none has, loads as the kind's default mode.
        let mut setting = OverviewLayoutSetting::grid_default();
        setting.grid[0].mode = Some("rows".to_owned());
        setting.grid[1].mode = Some("bogus".to_owned());
        let layout = OverviewLayout::from_setting(Some(&setting));
        for kind in [BoxKind::Cpu, BoxKind::Memory] {
            assert_eq!(layout.config(kind), BoxConfig::default_for(kind));
        }
        // The default mode is not written; switching to a mode the kind lacks does nothing.
        assert_eq!(layout.to_setting(), OverviewLayoutSetting::grid_default());
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
        let default = OverviewLayout::grid_default();
        assert_eq!(default.validate(), Ok(()));
        assert!(!default.has(BoxKind::TopProcs));
        // The saved form of the default converts both ways.
        assert_eq!(default.to_setting(), OverviewLayoutSetting::grid_default());
        assert_eq!(OverviewLayout::from_setting(None), default);
        assert_eq!(
            OverviewLayout::from_setting(Some(&OverviewLayoutSetting::grid_default())),
            default
        );
    }

    #[test]
    fn overview_grid_converts_rows() {
        // UI-PRF-056: the UI-PRF-018 default rows load as exactly the default grid without the
        // timeline.
        let mut settings = tl_model::settings::Settings::default();
        settings.performance.overview = Some(OverviewLayoutSetting::rows_default());
        assert!(settings.clamp().is_empty());
        let mut want = OverviewLayout::grid_default();
        want.boxes.retain(|b| b.kind != BoxKind::Timeline);
        assert_eq!(
            OverviewLayout::from_setting(settings.performance.overview.as_ref()),
            want
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
        // The kind minimums match the ones the profile load converts old rows with.
        let mins: Vec<(&str, u32, u32)> = BoxKind::ALL
            .iter()
            .map(|k| (k.key(), k.min_w() as u32, k.min_h() as u32))
            .collect();
        assert_eq!(mins, OVERVIEW_KIND_MIN);
        assert_eq!(
            MIN_SPAN,
            (OVERVIEW_MIN_SPAN.0 as u16, OVERVIEW_MIN_SPAN.1 as u16)
        );
    }

    #[test]
    fn overview_top_procs_layout_adds_a_full_row() {
        let setting = OverviewLayoutSetting::with_top_procs(40);
        let layout = OverviewLayout::from_setting(Some(&setting));
        assert_eq!(layout.validate(), Ok(()));
        assert!(layout.has(BoxKind::TopProcs));
        let p = placed(&layout, 1200., 900.);
        let procs = p
            .boxes
            .iter()
            .find(|b| b.kind == BoxKind::TopProcs)
            .expect("placed");
        assert_eq!(procs.bounds.left(), rect_left(&p, BoxKind::Cpu));
        assert_eq!(procs.bounds.right(), rect_right(&p, BoxKind::Net));
        assert!(procs.bounds.size.height >= px(260.));
        // The timeline stays at the bottom, under the band.
        let last = p.boxes.last().expect("placed");
        assert_eq!(last.kind, BoxKind::Timeline);
        assert!(last.bounds.top() > procs.bounds.bottom());
        // 24 rows are 1098 px plus the padding.
        assert!(p.scrolls);
        assert_eq!(p.content_h, px(1130.));
    }

    fn rect_left(p: &Placement, kind: BoxKind) -> Pixels {
        px(rect(p, kind).0)
    }

    fn rect_right(p: &Placement, kind: BoxKind) -> Pixels {
        let (x, _, w, _) = rect(p, kind);
        px(x + w)
    }

    #[test]
    fn overview_grid_default_1200x900() {
        // cw = (1200 − 32 − 66) / 12 = 91.83; rows of 40 with 6 px gutters.
        let p = placed(&OverviewLayout::grid_default(), 1200., 900.);
        assert!(!p.stacked && !p.scrolls);
        assert_eq!(rect(&p, BoxKind::Cpu), (16., 16., 777., 316.));
        assert_eq!(rect(&p, BoxKind::Memory), (799., 16., 385., 316.));
        assert_eq!(rect(&p, BoxKind::Gpu), (16., 338., 385., 270.));
        assert_eq!(rect(&p, BoxKind::Disk), (407., 338., 386., 270.));
        assert_eq!(rect(&p, BoxKind::Net), (799., 338., 385., 270.));
        // The timeline over all 12 columns from row 13, 4 rows high (UI-PRF-057).
        assert_eq!(rect(&p, BoxKind::Timeline), (16., 614., 1168., 178.));
        // 17 rows: 17 × 40 + 16 × 6 = 776, plus the padding.
        assert_eq!(p.content_h, px(808.));
        let order: Vec<BoxKind> = p.boxes.iter().map(|b| b.kind).collect();
        assert_eq!(
            order,
            [
                BoxKind::Cpu,
                BoxKind::Memory,
                BoxKind::Gpu,
                BoxKind::Disk,
                BoxKind::Net,
                BoxKind::Timeline
            ]
        );
        // The gutters are 6 px after rounding, and the right edge is the padding.
        assert_eq!(
            rect_right(&p, BoxKind::Cpu) + px(6.),
            rect_left(&p, BoxKind::Memory)
        );
        assert_eq!(
            rect_right(&p, BoxKind::Gpu) + px(6.),
            rect_left(&p, BoxKind::Disk)
        );
        assert_eq!(rect_right(&p, BoxKind::Net), px(1184.));

        // A missing kind leaves its cells empty; the boxes do not widen.
        let p = place(
            &OverviewLayout::grid_default(),
            |k| k != BoxKind::Gpu,
            area(1200., 900.),
            1.,
            ROW_UNIT_DEFAULT,
        );
        assert!(p.boxes.iter().all(|b| b.kind != BoxKind::Gpu));
        assert_eq!(rect(&p, BoxKind::Disk), (407., 338., 386., 270.));
        // A taller row unit makes the boxes taller and the page scroll.
        let p = place(
            &OverviewLayout::grid_default(),
            |_| true,
            area(1200., 900.),
            1.,
            80.,
        );
        assert_eq!(rect(&p, BoxKind::Cpu).3, 7. * 80. + 6. * 6.);
        assert!(p.scrolls);
    }

    #[test]
    fn overview_grid_kind_minimum() {
        // At 1200 px a column is 91.83 px: CPU needs ceil(280 / 91.83) = 4 columns and
        // ceil(280 / 40) = 7 rows; the other kinds the 3-column floor and 6 rows.
        let g = GridMetrics::new(1200., ROW_UNIT_DEFAULT, 1.);
        assert_eq!(g.min_span(BoxKind::Cpu), (4, 7));
        assert_eq!(g.min_span(BoxKind::TopProcs), (4, 7));
        assert_eq!(g.min_span(BoxKind::Memory), (3, 6));
        // Wide pages keep the 3 × 4 floor; narrow ones need more columns, at most 12.
        let wide = GridMetrics::new(4000., 80., 1.);
        assert_eq!(wide.min_span(BoxKind::Cpu), (3, 4));
        let narrow = GridMetrics::new(720., 24., 1.);
        assert_eq!(narrow.min_span(BoxKind::Cpu), (6, 12));
        assert_eq!(GridMetrics::new(100., 24., 1.).min_span(BoxKind::Cpu).0, 12);
        // The scale cancels out.
        assert_eq!(
            GridMetrics::new(2400., ROW_UNIT_DEFAULT, 2.).min_span(BoxKind::Cpu),
            (4, 7)
        );
        // At draw time a box keeps its saved span, even below its kind minimum.
        let small = layout(&[(BoxKind::Cpu, 0, 0, 2, 4)]);
        let p = placed(&small, 1200., 900.);
        assert_eq!(rect(&p, BoxKind::Cpu).3, 4. * 40. + 3. * 6.);
        assert!(rect(&p, BoxKind::Cpu).2 < BoxKind::Cpu.min_w());
    }

    #[test]
    fn overview_first_free_scans_rows_then_columns() {
        let l = OverviewLayout::grid_default();
        // The default fills rows 0..17 across: a 3 × 4 box goes under it.
        assert_eq!(l.first_free(3, 4), (0, 17));
        let mut l = layout(&[(BoxKind::Cpu, 0, 0, 8, 7), (BoxKind::Net, 8, 3, 4, 6)]);
        // Columns 8..12 are free in rows 0..3 only: 3 rows high fits there, 4 does not.
        assert_eq!(l.first_free(4, 3), (8, 0));
        assert_eq!(l.first_free(4, 4), (0, 7));
        l.boxes.clear();
        assert_eq!(l.first_free(12, 4), (0, 0));
    }

    #[test]
    fn overview_place_narrow_stacks() {
        let p = placed(&OverviewLayout::grid_default(), 700., 900.);
        assert!(p.stacked);
        assert!(p.boxes.iter().all(|b| b.bounds.size.width == px(668.)));
        // Each box at its kind minimum: 280 + 4 × 220 + 160, 5 gaps of 8 and 2 × 16 padding.
        assert_eq!(p.content_h, px(1392.));
        assert!(p.scrolls);
        assert_eq!(rect(&p, BoxKind::Memory), (16., 304., 668., 220.));
        assert_eq!(rect(&p, BoxKind::Net), (16., 988., 668., 220.));
        assert_eq!(rect(&p, BoxKind::Timeline), (16., 1216., 668., 160.));
    }

    #[test]
    fn overview_grid_narrow_stacks() {
        use BoxKind::*;
        // Reading order is by row, then column, whatever order the boxes are stored in.
        let l = layout(&[
            (Net, 6, 8, 6, 6),
            (Disk, 0, 8, 6, 6),
            (Memory, 0, 0, 12, 8),
            (Cpu, 4, 14, 8, 7),
        ]);
        let p = placed(&l, 719., 900.);
        assert!(p.stacked);
        let order: Vec<BoxKind> = p.boxes.iter().map(|b| b.kind).collect();
        assert_eq!(order, [Memory, Disk, Net, Cpu]);
        assert_eq!(rect(&p, Disk), (16., 244., 687., 220.));
        assert_eq!(rect(&p, Cpu).3, 280.);
        // From 720 px the grid places them again.
        assert!(!placed(&l, 720., 900.).stacked);
    }

    #[test]
    fn overview_place_scales() {
        let p = place(
            &OverviewLayout::grid_default(),
            |_| true,
            area(2400., 1800.),
            2.,
            ROW_UNIT_DEFAULT,
        );
        assert_eq!(rect(&p, BoxKind::Cpu), (32., 32., 1553., 632.));
        assert_eq!(rect(&p, BoxKind::Memory), (1597., 32., 771., 632.));
    }

    #[test]
    fn overview_validate_rejects_overlap_out_of_grid_and_duplicate() {
        use BoxKind::*;
        let mut dup = OverviewLayout::grid_default();
        dup.boxes[4].kind = Cpu;
        assert_eq!(dup.validate(), Err(LayoutError::Duplicate(Cpu)));
        let over = layout(&[(Cpu, 0, 0, 8, 7), (Memory, 7, 6, 5, 7)]);
        assert_eq!(over.validate(), Err(LayoutError::Overlap(Cpu, Memory)));
        // Touching edges do not overlap.
        let touch = layout(&[(Cpu, 0, 0, 8, 7), (Memory, 8, 0, 4, 7), (Net, 0, 7, 12, 4)]);
        assert_eq!(touch.validate(), Ok(()));
        for (col, cols, rows) in [(9, 4, 6), (0, 0, 6), (0, 4, 0), (0, 13, 6)] {
            let out = layout(&[(Net, col, 0, cols, rows)]);
            assert_eq!(out.validate(), Err(LayoutError::OutOfGrid(Net)), "{col}");
        }
        assert_eq!(layout(&[]).validate(), Err(LayoutError::Empty));

        // A saved layout that does not validate gives the default.
        let mut saved = OverviewLayoutSetting::grid_default();
        saved.grid[1].col = 6;
        assert_eq!(
            OverviewLayout::from_setting(Some(&saved)),
            OverviewLayout::grid_default()
        );
        saved.grid[1].col = -1;
        assert_eq!(
            OverviewLayout::from_setting(Some(&saved)),
            OverviewLayout::grid_default()
        );
    }
}
