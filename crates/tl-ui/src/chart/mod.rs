//! Chart data contract shared by every graph surface (ADR-009 "Chart element", UI-PRF-001,
//! UI-PRF-003, UI-PRF-006). The element, geometry, scale, grid, labels and hover modules build on
//! these types.

pub mod element;
pub mod geometry;
pub mod grid;
pub mod grid_element;
pub mod hover;
pub mod labels;
pub mod scale;
pub mod view;

use std::ops::Range;

use gpui_kit::{Hsla, Path, Pixels, SharedString, px};
use smallvec::SmallVec;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};

/// 60 s, the default chart range (R-016).
pub const DEFAULT_RANGE_NS: u64 = 60_000_000_000;

/// What one chart draws and how.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartSpec {
    pub series: SmallVec<[ChartSeries; 2]>,
    pub y: YScale,
    /// 30 s, 60 s (default), 300 s, 600 s (R-016; 300 and 600 from P2).
    pub range_ns: u64,
    pub grid: GridStyle,
    /// Settings `motion.smooth_graphs`.
    pub smooth: bool,
    /// P2: hitbox and hover readout (UI-PRF-004).
    pub hover: bool,
    /// P2: stacked bands, `YScale::Stacked` (PERF-030).
    pub stack: bool,
    pub background: ChartBg,
}

impl Default for ChartSpec {
    fn default() -> Self {
        Self {
            series: SmallVec::new(),
            y: YScale::Fixed {
                min: 0.0,
                max: 100.0,
            },
            range_ns: DEFAULT_RANGE_NS,
            grid: GridStyle::default(),
            smooth: false,
            hover: false,
            stack: false,
            background: ChartBg::Window,
        }
    }
}

impl ChartSpec {
    /// `YScale::scale_bits` with `stack` folded into bit 9.
    pub fn scale_bits(&self) -> u32 {
        self.y.scale_bits() | (u32::from(self.stack) << 9)
    }
}

/// One series in a chart.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartSeries {
    pub id: SeriesId,
    pub fill: Option<Hsla>,
    pub line: Hsla,
    pub line_width: Pixels,
    pub dashed: bool,
}

impl Default for ChartSeries {
    fn default() -> Self {
        Self {
            id: SeriesId {
                device: DeviceIdx(0),
                metric: MetricId::CPU_TOTAL,
            },
            fill: None,
            line: Hsla::default(),
            line_width: px(1.0),
            dashed: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GridStyle {
    pub lines: bool,
    pub baseline: bool,
    pub outline: bool,
    pub rounded: bool,
}

/// `Window` is `TlTheme.window_bg` (pages, summary tiles); `List` is `TlTheme.list_bg` (sidebar
/// and minigraph cards).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChartBg {
    #[default]
    Window,
    List,
}

/// Y axis rule (UI-PRF-006).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum YScale {
    /// Percentages, 0..100.
    Fixed { min: f32, max: f32 },
    /// Memory, VRAM: 0..total.
    FixedTotal(f32),
    /// 0..max(limit, peak in view).
    Commit(f32),
    /// Pow2 ceiling when `iec`, 1-2-5 otherwise; 10 s fall rule.
    AutoRate { iec: bool },
    /// Bits per second; the owner falls back to `AutoRate` when the speed is unknown.
    LinkSpeed(f32),
    /// 35..45 start, 5 °C steps.
    StickyTemp,
    /// Power, clocks, fan RPM.
    StickyUp { hw_max: Option<f32> },
    /// 0..100 × n.
    Stacked(u16),
    /// Battery power, −m..m.
    Symmetric,
}

impl YScale {
    /// Kind discriminant (0 to 8) with `iec` in bit 8 and `hw_max.is_some()` in bit 10.
    /// `ChartSpec::scale_bits` adds `stack` in bit 9.
    pub fn scale_bits(&self) -> u32 {
        match *self {
            Self::Fixed { .. } => 0,
            Self::FixedTotal(_) => 1,
            Self::Commit(_) => 2,
            Self::AutoRate { iec } => 3 | (u32::from(iec) << 8),
            Self::LinkSpeed(_) => 4,
            Self::StickyTemp => 5,
            Self::StickyUp { hw_max } => 6 | (u32::from(hw_max.is_some()) << 10),
            Self::Stacked(_) => 7,
            Self::Symmetric => 8,
        }
    }
}

/// Everything that forces a geometry rebuild when it changes (ADR-009).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChartGeometryKey {
    pub series_versions: SmallVec<[u64; 2]>,
    pub physical_width: u32,
    pub physical_height: u32,
    pub range_ns: u64,
    pub scale_bits: u32,
    /// `f32::to_bits` of the scale result.
    pub y_min_bits: u32,
    pub y_max_bits: u32,
    pub style_version: u64,
}

/// One merged path of a single colour.
#[derive(Clone, Debug)]
pub struct PathSet {
    pub color: Hsla,
    pub dashed: bool,
    pub path: Path<Pixels>,
}

/// Built geometry of one chart, reused across frames.
#[derive(Clone, Debug, Default)]
pub struct ChartCache {
    pub key: ChartGeometryKey,
    /// Newest sample time when built (ADR-009 rule 4).
    pub anchor_ns: u64,
    pub px_per_ns: f64,
    /// One per distinct fill colour (rule 7).
    pub fills: SmallVec<[PathSet; 2]>,
    /// One per distinct (line colour, dashed).
    pub strokes: SmallVec<[PathSet; 3]>,
    pub stale_fills: SmallVec<[PathSet; 2]>,
    pub stale_strokes: SmallVec<[PathSet; 3]>,
    /// Chart-local x spans that get the dotted baseline.
    pub gaps: SmallVec<[Range<f32>; 4]>,
    pub y_min: f32,
    pub y_max: f32,
}

/// Value formatter for the value label and axis max (UI-PRF-003).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueFormat {
    Percent,
    Bytes,
    RateIec,
    RateNetwork,
    Mhz,
    Celsius,
    Watts,
    Rpm,
    Count,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegendStyle {
    Solid,
    Dashed,
}

/// `Right` on pages, `Left` in sidebar cards (UI-PRF-011), `Hidden` when the owner draws the
/// value (minigraph cards).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueAlign {
    Right,
    Left,
    Hidden,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartLabels {
    pub heading: Option<SharedString>,
    pub value_fmt: ValueFormat,
    pub value_align: ValueAlign,
    /// "Utilization", "Throughput", "Memory usage", "Power draw", "Clock speed", "Temperature",
    /// "Fan speed", "Percentage", "Commit", "Swap usage", "Active time".
    pub range_word: &'static str,
    pub legend: SmallVec<[(SharedString, LegendStyle); 2]>,
    pub show_axis_max: bool,
}

/// Chart validity (UI-PRF-001). Missing is handled by the owner removing the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChartState {
    Live,
    WarmingUp,
    Stale { age_ns: u64 },
    Failed(SharedString),
    Restricted,
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_key_range_matters() {
        let a = ChartGeometryKey {
            range_ns: 30_000_000_000,
            ..Default::default()
        };
        let b = ChartGeometryKey {
            range_ns: DEFAULT_RANGE_NS,
            ..Default::default()
        };
        assert_ne!(a, b);
        assert_eq!(a, a.clone());
    }

    #[test]
    fn scale_bits_distinct_per_kind() {
        let kinds = [
            YScale::Fixed {
                min: 0.0,
                max: 100.0,
            },
            YScale::FixedTotal(16.0),
            YScale::Commit(32.0),
            YScale::AutoRate { iec: false },
            YScale::LinkSpeed(1e9),
            YScale::StickyTemp,
            YScale::StickyUp { hw_max: None },
            YScale::Stacked(4),
            YScale::Symmetric,
        ];
        let mut bits: Vec<u32> = kinds.iter().map(YScale::scale_bits).collect();
        bits.sort_unstable();
        bits.dedup();
        assert_eq!(bits.len(), 9);

        assert_ne!(
            YScale::AutoRate { iec: true }.scale_bits(),
            YScale::AutoRate { iec: false }.scale_bits()
        );
        assert_ne!(
            YScale::StickyUp { hw_max: Some(5.0) }.scale_bits(),
            YScale::StickyUp { hw_max: None }.scale_bits()
        );
        let flat = ChartSpec::default();
        let stacked = ChartSpec {
            stack: true,
            ..ChartSpec::default()
        };
        assert_ne!(flat.scale_bits(), stacked.scale_bits());
    }
}
