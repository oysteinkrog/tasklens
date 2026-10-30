//! The memory-by-app chart (docs/ui/performance.md UI-PRF-060, D-036): memory in use as a stacked
//! chart of memory groups. From the bottom: the top N groups, largest first, then "Other apps",
//! then the six parts of "Shared and kernel" (`memgroup:k:*`, [`KERNEL_PARTS`]). The top of the
//! stack is In use. History written by 1.0.8 has only `memgroup:shared`: the band "Shared and
//! kernel (older data)" is that series minus the six parts (`ChartSeries::minus`), so it is the
//! whole of it before the split and 0 after, and every sample still stacks to In use. A kernel
//! band that is 0 over the visible range is left out.
//!
//! The engine publishes the top 20 groups, the two fixed bands and the six parts as `MEM_GROUP`
//! series on `DeviceIdentity::MemGroup` devices. This chart ranks every group with data in the visible
//! range by its mean, keeps the order sticky ([`Ranker`]: at most every 10 s, and only for a 10%
//! lead), draws the top N in their own colours and folds the rest into "Other apps". A group keeps
//! its colour through `performance.memory_by_app.colors` ([`pick_colors`]).
//!
//! The legend lists the group bands and "Other apps" top first, then under a divider the kernel
//! bands top first, and shows whole rows only, with "+n more" for the rest ([`legend_fit`]). Hovering a band or its legend row raises it (the others drop to 40%) and
//! shows a tooltip. Clicking a group's legend row opens the Processes tab filtered to that group
//! (`ProcessesPage::open_mem_group`); "Other apps" and the kernel rows have no click.
//! The Memory page puts "Groups shown" in its context menu.
//!
//! The chart sits on the Memory page ([`AppsPlace::Page`]) or in the Overview Memory box
//! ([`AppsPlace::Overview`], mode `apps`). In the box it draws over the box range, and at 1 h,
//! 24 h and 7 d, or while the Overview is held at a time, it reads the device metric history as
//! the other box charts do; the ranking and the legend values then read the same history. On the
//! page it draws over the shared range and reads the history at 1 h, 24 h and 7 d. The legend
//! sits at the right, or below the chart when the body is narrower than
//! [`LEGEND_BELOW_W`].
//!
//! The chart itself is a cached `ChartView`; this view re-renders for the legend and hover only.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, Context, Entity, Hsla,
    InteractiveElement as _, IntoElement, MouseMoveEvent, ParentElement as _, Pixels, Point,
    Render, SharedString, Size, StatefulInteractiveElement as _, StyleRefinement, Styled as _,
    Subscription, Window, canvas, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::command::Tab;
use tl_model::devices::DeviceIdentity;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{GroupColor, MEMORY_BY_APP_COLORS_MAX, MEMORY_BY_APP_SLOTS};
use tl_model::snapshot::Snapshot;
use tl_model::time::now_boot_ns;
use tl_model::units;
use tracing::warn;

use super::details::Template;
use super::overview::parts::{ChartSettings, box_chart, class_probe};
use super::overview::timeline::{self, OverviewPause};
use super::page::page_range_ns;
use crate::chart::geometry::right_edge_ns;
use crate::chart::view::{HistoryClock, derived_view, find_tiered, history_view, reads_history};
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale,
};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::pages::processes::ProcessesPage;
use crate::settings_global::{Section, TlSettings};
use crate::text::style::{numeric_style, t_sm};
use crate::theme::{UiMetrics, current, metrics};
use crate::window::main::main_window_view;

/// Debug selector of the chart box.
pub const APPS_ID: &str = "perf-mem-apps";
/// Debug selector of the legend; each row adds `-<n>`, top row first.
pub const LEGEND_ID: &str = "perf-mem-apps-legend";
/// Debug selector of the hover tooltip.
pub const TOOLTIP_ID: &str = "perf-mem-apps-tooltip";

/// Key of the "Other apps" band (`memgroup:other`).
pub const OTHER_KEY: &str = "other";
/// Key of the "Shared and kernel" band (`memgroup:shared`), the sum of the six parts.
pub const SHARED_KEY: &str = "shared";
/// Key prefix of the six parts of "Shared and kernel". They are never memory groups.
pub const KERNEL_PREFIX: &str = "k:";
/// The six parts of "Shared and kernel", bottom first (UI-PRF-060): key, fill alpha against
/// `mem_fill`, and whether the value is worked out rather than read (the tooltip says so).
pub const KERNEL_PARTS: [(&str, f32, bool); 6] = [
    ("k:shmem", 0.25, false),
    ("k:zswap", 0.30, false),
    ("k:slab", 0.35, false),
    ("k:kernel", 0.40, false),
    ("k:driver", 0.45, true),
    ("k:filecache", 0.50, true),
];

/// The chart re-ranks at most this often.
pub const RERANK_NS: u64 = 10_000_000_000;
/// A group takes the place of the one above it only when its mean is at least this much larger.
pub const RANK_MARGIN: f64 = 1.10;
/// Alpha factor of the bands that are not hovered.
pub const DIM: f32 = 0.4;
/// The "Groups shown" menu choices.
pub const GROUPS_SHOWN: [u8; 4] = [5, 10, 15, 20];

/// Legend row height and width at 100%.
pub const LEGEND_ROW_H: f32 = 20.;
const LEGEND_W: f32 = 240.;
/// In the Overview box the legend goes below the chart when the body is narrower than this,
/// logical px at 100% (UI-PRF-060).
pub const LEGEND_BELOW_W: f32 = 480.;
const SWATCH: f32 = 10.;
/// Fill alpha of a group band against its line colour.
const GROUP_FILL_ALPHA: f32 = 0.6;
/// "Other apps" is `text_secondary` at 50%; "Shared and kernel" is `mem_fill` at 40%.
const OTHER_ALPHA: f32 = 0.5;
const SHARED_ALPHA: f32 = 0.4;

/// Where the chart sits (UI-PRF-060).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppsPlace {
    /// The Memory page's "By app" view: `graph_h_overall` high, the shared range wide (the live
    /// series, or the device metric history at 1 h, 24 h and 7 d), the legend at the right.
    Page,
    /// The Overview Memory box's `apps` mode: the box body, the box range from the live series
    /// or the device metric history, the legend at the right or below.
    Overview,
}

/// Where the series come from now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Live,
    /// The device metric history, on CLOCK_REALTIME.
    History,
    /// The Overview is held at a time and the history has not answered yet: nothing to show.
    Waiting,
}

/// The sticky order of the memory groups (UI-PRF-060).
#[derive(Clone, Debug, Default)]
pub struct Ranker {
    order: Vec<Arc<str>>,
    ranked_at: Option<u64>,
}

impl Ranker {
    /// Ranks `means` (group key, mean over the visible range) at `now_ns`. Groups ranked before
    /// keep their order unless the one below has a mean at least [`RANK_MARGIN`] times larger;
    /// new groups join below them, largest first. Groups without a mean leave. Within
    /// [`RERANK_NS`] of the last ranking (unless `force` or nothing is ranked yet) only the new
    /// groups join, below the others, so every group with data is in a band. Returns whether
    /// the order changed.
    pub fn rank(&mut self, means: &[(Arc<str>, f64)], now_ns: u64, force: bool) -> bool {
        let fresh_of = |order: &[Arc<str>]| {
            let mut fresh: Vec<(Arc<str>, f64)> = means
                .iter()
                .filter(|(k, _)| !order.contains(k))
                .cloned()
                .collect();
            fresh.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            fresh
        };
        if !force
            && !self.order.is_empty()
            && self
                .ranked_at
                .is_some_and(|t| now_ns.saturating_sub(t) < RERANK_NS)
        {
            let fresh = fresh_of(&self.order);
            let changed = !fresh.is_empty();
            self.order.extend(fresh.into_iter().map(|(k, _)| k));
            return changed;
        }
        self.ranked_at = Some(now_ns);
        let mean_of = |key: &str| means.iter().find(|(k, _)| &**k == key).map(|m| m.1);
        let mut order: Vec<(Arc<str>, f64)> = self
            .order
            .iter()
            .filter_map(|k| mean_of(k).map(|m| (Arc::clone(k), m)))
            .collect();
        order.extend(fresh_of(&self.order));
        // Each swap moves a strictly larger mean up, so this ends.
        loop {
            let mut swapped = false;
            for i in 1..order.len() {
                let (above, below) = (order[i - 1].1, order[i].1);
                if below > above && below >= above * RANK_MARGIN {
                    order.swap(i - 1, i);
                    swapped = true;
                }
            }
            if !swapped {
                break;
            }
        }
        let order: Vec<Arc<str>> = order.into_iter().map(|(k, _)| k).collect();
        let changed = order != self.order;
        self.order = order;
        changed
    }

    /// Every ranked group, first place first.
    pub fn order(&self) -> &[Arc<str>] {
        &self.order
    }
}

/// FNV-1a over the key bytes: stable across builds, so a key hashes to the same slot every run.
fn key_hash(key: &str) -> u64 {
    key.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The colour slot of each key in `shown` (top band first), and whether `picks` changed.
///
/// Saved picks are walked most recent first, so when two shown keys have the same saved slot,
/// the one shown most recently keeps it, whatever their rank. Any other key takes the slot its
/// key hashes to, or the next free slot among the shown keys. With more than 12 keys shown no
/// slot is free, and a key keeps its saved slot (or its hash slot), so slots repeat. The shown
/// keys then move to the end of `picks` (most recently shown last; saved keys keep their order
/// among themselves, so rank never decides it), and the oldest picks past 64 are dropped.
pub fn pick_colors(picks: &mut Vec<GroupColor>, shown: &[Arc<str>]) -> (Vec<u8>, bool) {
    let n = usize::from(MEMORY_BY_APP_SLOTS);
    let mut slots: Vec<Option<u8>> = vec![None; shown.len()];
    let mut used = vec![false; n];
    for p in picks.iter().rev() {
        let s = usize::from(p.slot) % n;
        if let Some(i) = shown.iter().position(|k| **k == p.key)
            && !used[s]
        {
            slots[i] = Some(s as u8);
            used[s] = true;
        }
    }
    for (i, key) in shown.iter().enumerate() {
        if slots[i].is_some() {
            continue;
        }
        let home = picks
            .iter()
            .find(|p| p.key == **key)
            .map_or((key_hash(key) % n as u64) as usize, |p| {
                usize::from(p.slot) % n
            });
        let slot = (0..n)
            .map(|d| (home + d) % n)
            .find(|&s| !used[s])
            .unwrap_or(home);
        used[slot] = true;
        slots[i] = Some(slot as u8);
    }
    let slots: Vec<u8> = slots.into_iter().map(Option::unwrap_or_default).collect();

    let slot_of = |key: &str| {
        shown.iter().position(|k| **k == *key).map(|i| GroupColor {
            key: key.to_string(),
            slot: slots[i],
        })
    };
    let mut next: Vec<GroupColor> = picks
        .iter()
        .filter(|p| !shown.iter().any(|k| **k == p.key))
        .cloned()
        .collect();
    // A key saved twice (a hand-edited file) moves once, at its later place.
    next.extend(
        picks
            .iter()
            .enumerate()
            .filter(|&(j, p)| !picks[j + 1..].iter().any(|q| q.key == p.key))
            .filter_map(|(_, p)| slot_of(&p.key)),
    );
    next.extend(
        shown
            .iter()
            .filter(|k| !picks.iter().any(|p| p.key == ***k))
            .filter_map(|k| slot_of(k)),
    );
    if next.len() > MEMORY_BY_APP_COLORS_MAX {
        next.drain(..next.len() - MEMORY_BY_APP_COLORS_MAX);
    }
    let changed = next != *picks;
    *picks = next;
    (slots, changed)
}

/// Legend rows that fit whole in `height` px of `row_h` rows: `(rows shown, rows behind
/// "+n more")`. When all rows do not fit, the last row that fits is "+n more" (UI-PRF-017).
pub fn legend_fit(rows: usize, height: f32, row_h: f32) -> (usize, usize) {
    let fit = if row_h > 0. {
        ((height + 0.01) / row_h).floor().max(0.) as usize
    } else {
        rows
    };
    if rows <= fit {
        return (rows, 0);
    }
    let shown = fit.saturating_sub(1);
    (shown, rows - shown)
}

/// Where the tooltip of the band in legend row `row` points, in chart-local px: the chart's
/// right edge (`chart_w`), level with the middle of the row. `None` when the row is not among the
/// `shown` rows, each `row_h` tall.
pub fn legend_anchor(
    row: usize,
    shown: usize,
    row_h: f32,
    chart_w: Pixels,
) -> Option<Point<Pixels>> {
    let row = Some(row).filter(|&r| r < shown)?;
    Some(Point {
        x: chart_w,
        y: px((row as f32 + 0.5) * row_h),
    })
}

/// The legend rows of `bands` (bottom first) as band indices, first row first: the group bands
/// and "Other apps" top first, then the kernel bands top first (UI-PRF-060: they follow "Other
/// apps", under a divider).
pub fn legend_order(bands: &[Band]) -> Vec<usize> {
    let top_first = (0..bands.len()).rev();
    top_first
        .clone()
        .filter(|&i| !bands[i].kind.is_kernel())
        .chain(top_first.filter(|&i| bands[i].kind.is_kernel()))
        .collect()
}

/// The bands of a chart with `top_n` own bands over `ranked` groups (rank order): the top N
/// keys, then the keys that fold into "Other apps".
pub fn split_top(ranked: &[Arc<str>], top_n: usize) -> (&[Arc<str>], &[Arc<str>]) {
    ranked.split_at(ranked.len().min(top_n))
}

/// What a band is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandKind {
    /// One of the top N groups, with its colour slot.
    Group(u8),
    Other,
    /// "Shared and kernel" where the six parts have no samples (history written by 1.0.8):
    /// `memgroup:shared` minus the parts.
    Shared,
    /// A part of "Shared and kernel", by its index in [`KERNEL_PARTS`].
    Kernel(u8),
}

impl BandKind {
    /// "Shared and kernel" or one of its parts: below the divider in the legend.
    pub fn is_kernel(self) -> bool {
        matches!(self, Self::Shared | Self::Kernel(_))
    }
}

/// One band, bottom first in [`MemoryAppsChart::bands`].
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub key: Arc<str>,
    pub name: SharedString,
    pub kind: BandKind,
    /// The fill colour (a group band fills at [`GROUP_FILL_ALPHA`] of it).
    pub color: Hsla,
    /// The line at the band's top: the 1 px separator `chart_bg` for the kernel parts.
    pub line: Hsla,
    /// The series stacked for this band. "Other apps" has the groups outside the top N and the
    /// `memgroup:other` series.
    pub series: SmallVec<[SeriesId; 1]>,
    /// The series the band's one series is drawn less of (`ChartSeries::minus`): the six parts
    /// for [`BandKind::Shared`], else empty.
    pub minus: Arc<[SeriesId]>,
    /// Processes in the band now; `None` when not known (a group outside the engine's top 20).
    pub processes: Option<u32>,
    /// Bytes at the newest sample.
    pub value: f64,
}

/// A memory group with a series in the snapshot.
#[derive(Clone, Debug)]
struct Group {
    key: Arc<str>,
    name: Arc<str>,
    device: DeviceIdx,
}

fn series_id(device: DeviceIdx) -> SeriesId {
    SeriesId {
        device,
        metric: MetricId::MEM_GROUP,
    }
}

/// The value of `view` at `t_ns`: the nearest sample (the earlier one on a tie) when it is valid
/// and within 1.5 periods, else 0. A gap sample counts as 0: the engine writes one when a group
/// leaves its top 20, and the group's bytes are then in "Other apps".
fn value_at(view: &SeriesView, t_ns: u64) -> f64 {
    let samples: &[Sample] = &view.samples;
    let split = samples.partition_point(|s| s.t_ns < t_ns);
    let Some(s) = [
        split.checked_sub(1),
        (split < samples.len()).then_some(split),
    ]
    .into_iter()
    .flatten()
    .map(|i| samples[i])
    .min_by_key(|s| s.t_ns.abs_diff(t_ns)) else {
        return 0.;
    };
    let slack = view.period_ns.saturating_mul(3) / 2;
    if s.is_valid() && s.t_ns.abs_diff(t_ns) <= slack.max(1) {
        f64::from(s.value)
    } else {
        0.
    }
}

/// The page's words.
#[derive(Clone, Debug, Default)]
pub struct AppsWords {
    pub heading: SharedString,
    pub range: SharedString,
    pub more: Option<Template>,
    pub count: Option<Template>,
    pub share: Option<Template>,
    pub processes: Option<Template>,
    pub groups_shown: SharedString,
    /// The name of [`BandKind::Shared`] while the parts are published.
    pub shared_older: SharedString,
    /// The tooltip line of the parts that are worked out.
    pub estimate: SharedString,
}

impl AppsWords {
    fn refresh(&mut self, version: &mut u64, cx: &App) -> bool {
        let v = cx.global::<Strings>().strings_version;
        if v == *version {
            return false;
        }
        *version = v;
        *self = AppsWords {
            heading: tr(cx, "perf-mem-heading-physical"),
            range: tr(cx, "perf-mem-range-memory"),
            more: Some(Template::new(cx, "perf-mem-apps-more", &["count"])),
            count: Some(Template::new(cx, "perf-mem-apps-count", &["count"])),
            share: Some(Template::new(cx, "perf-mem-apps-share", &["share"])),
            processes: Some(Template::counted(
                cx,
                "perf-mem-apps-processes",
                &["count"],
                0,
            )),
            groups_shown: tr(cx, "perf-mem-apps-groups-shown"),
            shared_older: tr(cx, "perf-mem-apps-shared-older"),
            estimate: tr(cx, "perf-mem-apps-estimate"),
        };
        true
    }
}

/// The chart settings: `top_n`, the range and the chart look.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AppsSettings {
    top_n: usize,
    range_ns: u64,
    smooth: bool,
    rounded: bool,
}

impl AppsSettings {
    /// The Overview box takes its range and look from the box charts (UI-PRF-055).
    fn read(place: AppsPlace, cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        let top_n = usize::from(s.performance.memory_by_app.top_n);
        match place {
            AppsPlace::Page => Self {
                top_n,
                range_ns: page_range_ns(cx),
                smooth: s.motion.smooth_graphs,
                rounded: s.appearance.rounded_corners,
            },
            AppsPlace::Overview => {
                let box_chart = ChartSettings::from(s);
                Self {
                    top_n,
                    range_ns: box_chart.range_ns,
                    smooth: box_chart.smooth,
                    rounded: box_chart.rounded,
                }
            }
        }
    }
}

/// The hovered band and, over the chart, the pointer and its time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Hover {
    band: Option<usize>,
    /// Chart-local pointer position and the time under it; `None` over the legend.
    at: Option<(Point<Pixels>, u64)>,
}

/// The memory-by-app chart with its legend (UI-PRF-060).
pub struct MemoryAppsChart {
    place: AppsPlace,
    chart: Entity<ChartView>,
    hub: Entity<SnapshotHub>,
    /// The series of every memory group device for the chart range, sorted by device, from
    /// [`source`](Self::source).
    views: Vec<(DeviceIdx, Arc<SeriesView>)>,
    /// The history clock while `views` come from the device metric history.
    clock: Option<HistoryClock>,
    /// What `views` were resolved from: the source, the device history version and the device
    /// catalog version. History views are built again only when one changes.
    views_from: (Source, u64, u64),
    ranker: Ranker,
    /// Every group with a series, by key.
    groups: Vec<Group>,
    /// Devices of the two fixed bands and the six parts of "Shared and kernel".
    other: Option<DeviceIdx>,
    shared: Option<DeviceIdx>,
    parts: [Option<DeviceIdx>; 6],
    /// `memgroup:shared` minus the six parts, at the times of `memgroup:shared`.
    older: Option<Arc<SeriesView>>,
    /// Whether "Shared and kernel (older data)" and each part, in [`KERNEL_PARTS`] order, is
    /// above 0 somewhere in the visible range; the others have no band.
    kernel_on: [bool; 7],
    /// The ranked keys the bands were built from, and the "Other apps" keys.
    top: Vec<Arc<str>>,
    rest: Vec<Arc<str>>,
    bands: Vec<Band>,
    total: u64,
    settings: AppsSettings,
    settings_versions: [u64; 3],
    style_version: u64,
    strings_version: u64,
    words: AppsWords,
    hover: Hover,
    /// The chart box bounds of the last paint, for pointer hit tests.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The Overview body size of the last paint, for the legend place and rows.
    body: Rc<Cell<Size<Pixels>>>,
    _subs: SmallVec<[Subscription; 5]>,
    /// The group key and name the last legend click opened.
    #[cfg(any(test, feature = "test-hooks"))]
    pub last_open: Option<(Arc<str>, SharedString)>,
}

fn section_versions(cx: &App) -> [u64; 3] {
    let tl = TlSettings::get(cx);
    [
        tl.version(Section::Performance),
        tl.version(Section::Motion),
        tl.version(Section::Appearance),
    ]
}

impl MemoryAppsChart {
    /// The Memory page's chart.
    pub fn new(hub: Entity<SnapshotHub>, cx: &mut Context<Self>) -> Self {
        Self::new_in(hub, AppsPlace::Page, cx)
    }

    /// The chart for `place`. In the Overview box the chart is a box chart: it reads the device
    /// metric history at 1 h, 24 h and 7 d and holds while the Overview waits for a picked time.
    pub fn new_in(hub: Entity<SnapshotHub>, place: AppsPlace, cx: &mut Context<Self>) -> Self {
        let settings = AppsSettings::read(place, cx);
        let mut words = AppsWords::default();
        let mut strings_version = u64::MAX;
        words.refresh(&mut strings_version, cx);
        let snap = Arc::clone(hub.read(cx).current());
        let total = snap.system.memory.total;
        let spec = Self::spec_of(&[], None, total, settings);
        let labels = Self::labels_of(place, &words);
        let chart = match place {
            AppsPlace::Page => cx.new(|cx| {
                ChartView::new(spec, labels, &hub, cx)
                    .with_newest_marks()
                    .with_device_history(&snap)
            }),
            AppsPlace::Overview => box_chart(spec, labels, &hub, cx),
        };
        let mut subs: SmallVec<[Subscription; 5]> = smallvec![
            cx.subscribe(&hub, Self::on_snapshot),
            cx.observe_global::<TlSettings>(Self::sync_settings),
            cx.observe_global::<UiMetrics>(Self::sync_style),
            cx.observe_global::<Strings>(Self::sync_words),
        ];
        if place == AppsPlace::Overview {
            subs.push(cx.observe_global::<OverviewPause>(Self::on_pause));
        }
        let mut this = Self {
            place,
            chart,
            hub,
            views: Vec::new(),
            clock: None,
            views_from: (Source::Live, u64::MAX, u64::MAX),
            ranker: Ranker::default(),
            groups: Vec::new(),
            other: None,
            shared: None,
            parts: [None; 6],
            older: None,
            kernel_on: [false; 7],
            top: Vec::new(),
            rest: Vec::new(),
            bands: Vec::new(),
            total,
            settings,
            settings_versions: section_versions(cx),
            style_version: metrics(cx).style_version,
            strings_version,
            words,
            hover: Hover::default(),
            bounds: Rc::default(),
            body: Rc::default(),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            last_open: None,
        };
        this.refresh(&snap, true, cx);
        this
    }

    pub fn place(&self) -> AppsPlace {
        self.place
    }

    pub fn chart(&self) -> &Entity<ChartView> {
        &self.chart
    }

    /// The bands, bottom first.
    pub fn bands(&self) -> &[Band] {
        &self.bands
    }

    /// The hovered band index into [`bands`](Self::bands).
    pub fn hovered(&self) -> Option<usize> {
        self.hover.band
    }

    pub fn words(&self) -> &AppsWords {
        &self.words
    }

    /// Raises band `band` (the legend row hover), or none.
    pub fn set_hovered(&mut self, band: Option<usize>, cx: &mut Context<Self>) {
        let hover = Hover { band, at: None };
        if hover != self.hover {
            self.hover = hover;
            self.rebuild_spec(cx);
            cx.notify();
        }
    }

    /// The page chart has its heading, value and range word; the box frame draws those in the
    /// Overview.
    fn labels_of(place: AppsPlace, words: &AppsWords) -> ChartLabels {
        match place {
            AppsPlace::Page => ChartLabels {
                heading: Some(words.heading.clone()),
                value_fmt: ValueFormat::Bytes,
                value_align: ValueAlign::Right,
                range_word: words.range.clone(),
                legend: SmallVec::new(),
                show_axis_max: false,
            },
            AppsPlace::Overview => ChartLabels {
                heading: None,
                value_fmt: ValueFormat::Bytes,
                value_align: ValueAlign::Hidden,
                range_word: SharedString::default(),
                legend: SmallVec::new(),
                show_axis_max: false,
            },
        }
    }

    /// The stacked spec of `bands` (bottom first), 0..MemTotal. With a hovered band the others
    /// drop to [`DIM`].
    fn spec_of(bands: &[Band], hovered: Option<usize>, total: u64, s: AppsSettings) -> ChartSpec {
        let mut series: SmallVec<[ChartSeries; 4]> = SmallVec::new();
        for (i, band) in bands.iter().enumerate() {
            let dim = if hovered.is_some_and(|h| h != i) {
                DIM
            } else {
                1.
            };
            let line = band.line;
            let fill = match band.kind {
                BandKind::Group(_) => Hsla {
                    a: band.color.a * GROUP_FILL_ALPHA,
                    ..band.color
                },
                BandKind::Other | BandKind::Shared | BandKind::Kernel(_) => band.color,
            };
            let last = band.series.len().saturating_sub(1);
            for (j, id) in band.series.iter().enumerate() {
                // Only the top series of a folded band draws a line, so "Other apps" reads as
                // one band.
                let line = if j == last {
                    line
                } else {
                    Hsla { a: 0., ..line }
                };
                series.push(ChartSeries {
                    id: *id,
                    fill: Some(Hsla {
                        a: fill.a * dim,
                        ..fill
                    }),
                    line: Hsla {
                        a: line.a * dim,
                        ..line
                    },
                    minus: Arc::clone(&band.minus),
                    ..ChartSeries::default()
                });
            }
        }
        ChartSpec {
            series,
            y: YScale::FixedTotal(total as f32),
            range_ns: s.range_ns,
            grid: GridStyle {
                lines: true,
                baseline: false,
                outline: true,
                rounded: s.rounded,
                vertical_at: None,
            },
            smooth: s.smooth,
            hover: false,
            stack: true,
            background: ChartBg::Window,
        }
    }

    fn rebuild_spec(&self, cx: &mut Context<Self>) {
        let spec = Self::spec_of(&self.bands, self.hover.band, self.total, self.settings);
        self.chart.update(cx, |v, cx| v.set_spec(spec, cx));
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        // While the views come from the history, live series ticks change nothing drawn here.
        let series = match self.views_from.0 {
            Source::Live => ComponentMask::SERIES,
            Source::History | Source::Waiting => ComponentMask::DEVICE_HISTORY,
        };
        let wanted =
            series | ComponentMask::DEVICE_HISTORY | ComponentMask::SYSTEM | ComponentMask::DEVICES;
        if !ev.mask.intersects(wanted) {
            return;
        }
        let snap = Arc::clone(hub.read(cx).current());
        self.refresh(&snap, false, cx);
    }

    /// The Overview was held at a time or made live: the source may change.
    fn on_pause(&mut self, cx: &mut Context<Self>) {
        let snap = Arc::clone(self.hub.read(cx).current());
        self.refresh(&snap, true, cx);
    }

    /// Where the series come from, as the box charts decide it (`ChartView::resolve`): both
    /// read the history over 10 minutes. The box also reads it whenever the history answers a
    /// request at a picked time for this range, and waits while the Overview is held and the
    /// history has not answered; the page is never held.
    fn source(&self, snap: &Snapshot, cx: &App) -> Source {
        let range_ns = self.settings.range_ns;
        if self.place == AppsPlace::Page {
            return if reads_history(range_ns) {
                Source::History
            } else {
                Source::Live
            };
        }
        let answered = snap
            .device_history
            .request
            .is_some_and(|r| r.end_realtime_ns.is_some() && r.range.span_ns() == range_ns);
        if timeline::paused(cx).is_some() && !answered {
            Source::Waiting
        } else if answered || reads_history(range_ns) {
            Source::History
        } else {
            Source::Live
        }
    }

    /// Fills `views` with the series of every memory group device. History views (band means
    /// only, as a stacked chart draws them) are built again only when the history or the
    /// catalog changed.
    fn resolve_views(&mut self, snap: &Snapshot, force: bool, cx: &App) {
        let source = self.source(snap, cx);
        let from = (
            source,
            snap.device_history.header.version,
            snap.devices.version,
        );
        if source != Source::Live && from == self.views_from && !force {
            return;
        }
        self.views_from = from;
        self.views.clear();
        self.clock = None;
        let hist = &*snap.device_history;
        if source == Source::History {
            self.clock = Some(HistoryClock {
                offset_ns: hist.realtime_offset_ns,
                end_ns: hist
                    .request
                    .and_then(|r| r.end_realtime_ns)
                    .map(|t| t.max(0) as u64),
            });
        }
        if source == Source::Waiting {
            return;
        }
        for e in snap.devices.entries.iter() {
            if !matches!(e.identity, DeviceIdentity::MemGroup { .. }) {
                continue;
            }
            let id = series_id(e.idx);
            let view = match source {
                Source::History => hist
                    .get(id)
                    .map(|points| Arc::new(history_view(id, points, hist, true))),
                _ => find_tiered(&snap.series, id, self.settings.range_ns)
                    .0
                    .cloned(),
            };
            if let Some(view) = view {
                self.views.push((e.idx, view));
            }
        }
        self.views.sort_by_key(|(d, _)| *d);
    }

    /// The series view of `device` for the chart range.
    fn view(&self, device: DeviceIdx) -> Option<&Arc<SeriesView>> {
        self.views
            .binary_search_by_key(&device, |(d, _)| *d)
            .ok()
            .map(|i| &self.views[i].1)
    }

    /// The time of the newest sample. The reference clock is the "Shared and kernel" band, which
    /// gets a sample on every sweep.
    fn newest(&self) -> Option<u64> {
        self.shared
            .and_then(|d| self.view(d))
            .and_then(|v| v.samples.last().map(|s| s.t_ns))
    }

    /// Builds [`older`](Self::older) from the views and returns which kernel bands are above 0
    /// in the visible range up to `newest`.
    fn kernel_bands_on(&mut self, newest: Option<u64>) -> [bool; 7] {
        let part_views: SmallVec<[Option<&SeriesView>; 6]> = self
            .parts
            .iter()
            .map(|p| p.and_then(|d| self.view(d)).map(|v| &**v))
            .collect();
        let older = self
            .shared
            .and_then(|d| self.view(d))
            .map(|a| Arc::new(derived_view(a, part_views.iter().copied())));
        let mut on = [false; 7];
        if let Some(newest) = newest {
            let from = newest.saturating_sub(self.settings.range_ns);
            let above = |v: &SeriesView| {
                v.samples
                    .iter()
                    .any(|s| s.t_ns > from && s.is_valid() && s.value > 0.)
            };
            on[0] = older.as_deref().is_some_and(above);
            for (i, v) in part_views.iter().enumerate() {
                on[i + 1] = v.is_some_and(above);
            }
        }
        drop(part_views);
        self.older = older;
        on
    }

    /// The name of [`BandKind::Shared`]: "Shared and kernel (older data)" while the parts are
    /// published, else the band's catalog name.
    fn shared_name(&self, cx: &App) -> SharedString {
        if self.parts.iter().any(Option::is_some) {
            return self.words.shared_older.clone();
        }
        self.shared
            .and_then(|d| self.hub.read(cx).current().devices.get(d))
            .map(|e| SharedString::from(e.display_name.to_string()))
            .unwrap_or_else(|| SharedString::from(SHARED_KEY))
    }

    /// Re-reads the groups, ranks them (sticky unless `force`), and rebuilds the bands and the
    /// spec when the bands changed. The legend values follow every call.
    fn refresh(&mut self, snap: &Snapshot, force: bool, cx: &mut Context<Self>) {
        self.resolve_views(snap, force, cx);
        let fixed = (self.other, self.shared, self.parts);
        self.groups.clear();
        self.other = None;
        self.shared = None;
        self.parts = [None; 6];
        for e in snap.devices.entries.iter() {
            if let DeviceIdentity::MemGroup { key } = &e.identity {
                match &**key {
                    OTHER_KEY => self.other = Some(e.idx),
                    SHARED_KEY => self.shared = Some(e.idx),
                    // The parts of "Shared and kernel" are never ranked or given a colour.
                    k if k.starts_with(KERNEL_PREFIX) => {
                        if let Some(i) = KERNEL_PARTS.iter().position(|p| p.0 == k) {
                            self.parts[i] = Some(e.idx);
                        }
                    }
                    _ => self.groups.push(Group {
                        key: Arc::clone(key),
                        name: Arc::clone(&e.display_name),
                        device: e.idx,
                    }),
                }
            }
        }

        let reference = self.shared.and_then(|d| self.view(d));
        let newest = self.newest();
        let mut means: Vec<(Arc<str>, f64)> = Vec::new();
        if let (Some(reference), Some(newest)) = (reference, newest) {
            let from = newest.saturating_sub(self.settings.range_ns);
            let ticks = reference
                .samples
                .iter()
                .filter(|s| s.t_ns > from)
                .count()
                .max(1);
            for g in &self.groups {
                let Some(view) = self.view(g.device) else {
                    continue;
                };
                let sum: f64 = view
                    .samples
                    .iter()
                    .filter(|s| s.t_ns > from && s.is_valid())
                    .map(|s| f64::from(s.value))
                    .sum();
                if sum > 0. {
                    means.push((Arc::clone(&g.key), sum / ticks as f64));
                }
            }
        }
        self.ranker.rank(&means, newest.unwrap_or(0), force);
        let kernel_on = self.kernel_bands_on(newest);

        let (top, rest) = split_top(self.ranker.order(), self.settings.top_n);
        let total = snap.system.memory.total;
        // A fixed band whose device arrives or moves needs new bands too, even with no group
        // ranked.
        let structure_changed = top != self.top.as_slice()
            || rest != self.rest.as_slice()
            || total != self.total
            || (self.other, self.shared, self.parts) != fixed
            || kernel_on != self.kernel_on
            || force;
        if structure_changed {
            self.top = top.to_vec();
            self.rest = rest.to_vec();
            self.total = total;
            self.kernel_on = kernel_on;
            self.build_bands(cx);
            self.rebuild_spec(cx);
            cx.notify();
        }
        if self.update_values(snap, newest.unwrap_or(0)) {
            cx.notify();
        }
        if self.place == AppsPlace::Overview {
            // The box frame shows the value.
            return;
        }
        // The value is In use from the memory totals: a sum of the bands would show "-" while
        // a ranked group has no sample at the newest time.
        let mut text = String::new();
        units::bytes(&mut text, snap.system.memory.used);
        let text = Some(SharedString::from(text));
        self.chart.update(cx, |v, cx| v.set_value_text(text, cx));
    }

    /// The bands from `top` and `rest`, with colours from the saved picks. Saves the picks when
    /// they changed.
    fn build_bands(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let mut picks = TlSettings::settings(cx)
            .performance
            .memory_by_app
            .colors
            .clone();
        let (slots, changed) = pick_colors(&mut picks, &self.top);
        if changed {
            TlSettings::update(cx, |d| d.settings.performance.memory_by_app.colors = picks);
        }
        let group = |key: &Arc<str>| self.groups.iter().find(|g| g.key == *key);
        let mut bands: Vec<Band> = Vec::with_capacity(self.top.len() + 2);
        for (key, &slot) in self.top.iter().zip(&slots) {
            let Some(g) = group(key) else { continue };
            let color = theme.graph.groups[usize::from(slot) % theme.graph.groups.len()];
            bands.push(Band {
                key: Arc::clone(key),
                name: SharedString::from(g.name.to_string()),
                kind: BandKind::Group(slot),
                color,
                line: color,
                series: smallvec![series_id(g.device)],
                minus: Arc::from([]),
                processes: None,
                value: 0.,
            });
        }
        let name_of = |key: &str, device: Option<DeviceIdx>| -> SharedString {
            device
                .and_then(|d| self.hub.read(cx).current().devices.get(d))
                .map(|e| SharedString::from(e.display_name.to_string()))
                .unwrap_or_else(|| SharedString::from(key.to_owned()))
        };
        if let Some(other) = self.other {
            let mut series: SmallVec<[SeriesId; 1]> = self
                .rest
                .iter()
                .filter_map(group)
                .map(|g| series_id(g.device))
                .collect();
            series.push(series_id(other));
            let color = Hsla {
                a: theme.text_secondary.a * OTHER_ALPHA,
                ..theme.text_secondary
            };
            bands.push(Band {
                key: Arc::from(OTHER_KEY),
                name: name_of(OTHER_KEY, Some(other)),
                kind: BandKind::Other,
                color,
                line: color,
                series,
                minus: Arc::from([]),
                processes: None,
                value: 0.,
            });
        }
        if let Some(shared) = self.shared
            && self.kernel_on[0]
        {
            let color = Hsla {
                a: theme.graph.mem_fill.a * SHARED_ALPHA,
                ..theme.graph.mem_fill
            };
            bands.push(Band {
                key: Arc::from(SHARED_KEY),
                name: self.shared_name(cx),
                kind: BandKind::Shared,
                color,
                line: color,
                series: smallvec![series_id(shared)],
                minus: self.parts.iter().flatten().map(|&d| series_id(d)).collect(),
                processes: None,
                value: 0.,
            });
        }
        for (i, (&(key, alpha, _), part)) in KERNEL_PARTS.iter().zip(self.parts).enumerate() {
            let Some(device) = part.filter(|_| self.kernel_on[i + 1]) else {
                continue;
            };
            bands.push(Band {
                key: Arc::from(key),
                name: name_of(key, Some(device)),
                kind: BandKind::Kernel(i as u8),
                color: Hsla {
                    a: theme.graph.mem_fill.a * alpha,
                    ..theme.graph.mem_fill
                },
                line: theme.graph.chart_bg,
                series: smallvec![series_id(device)],
                minus: Arc::from([]),
                processes: None,
                value: 0.,
            });
        }
        if self.hover.band.is_some_and(|b| b >= bands.len()) {
            self.hover = Hover::default();
        }
        self.bands = bands;
    }

    /// Each band's newest value and process count. Returns whether one changed.
    fn update_values(&mut self, snap: &Snapshot, newest: u64) -> bool {
        let published = &snap.system.memory.groups;
        let count_of = |key: &str| {
            published
                .iter()
                .find(|b| &*b.key == key)
                .map(|b| b.process_count)
        };
        let mut changed = false;
        for i in 0..self.bands.len() {
            let value = self.band_value(&self.bands[i], newest);
            let processes = match self.bands[i].kind {
                BandKind::Group(_) => count_of(&self.bands[i].key),
                BandKind::Shared | BandKind::Kernel(_) => None,
                BandKind::Other => {
                    let rest: u32 = self.rest.iter().filter_map(|k| count_of(k)).sum();
                    count_of(OTHER_KEY).map(|n| n + rest)
                }
            };
            let band = &mut self.bands[i];
            if value != band.value || processes != band.processes {
                band.value = value;
                band.processes = processes;
                changed = true;
            }
        }
        changed
    }

    /// The band under chart-local `pos` and the time there, from the stacked values.
    fn hit(&self, pos: Point<Pixels>) -> Option<(usize, u64)> {
        let b = self.bounds.get();
        let (w, h) = (f32::from(b.size.width) - 2., f32::from(b.size.height) - 2.);
        if w <= 0. || h <= 0. || self.total == 0 {
            return None;
        }
        let reference = self.shared.and_then(|d| self.view(d))?;
        let newest = reference.samples.last()?.t_ns;
        let right = match self.clock {
            Some(clock) => clock.right_edge_ns(now_boot_ns()),
            None => right_edge_ns(
                now_boot_ns(),
                newest,
                reference.period_ns,
                self.settings.smooth,
            ),
        };
        let x = (f32::from(pos.x) - 1.).clamp(0., w);
        let ns = f64::from(w - x) * self.settings.range_ns as f64 / f64::from(w);
        let t = right.saturating_sub(ns.round() as u64);
        let level = f64::from((h - (f32::from(pos.y) - 1.)).clamp(0., h) / h) * self.total as f64;
        let mut top = 0.;
        for (i, band) in self.bands.iter().enumerate() {
            top += self.band_value(band, t);
            if level <= top {
                return Some((i, t));
            }
        }
        None
    }

    /// The value of `band` at `t`: the sum of its series, or for [`BandKind::Shared`] the part
    /// of `memgroup:shared` the six parts do not cover.
    fn band_value(&self, band: &Band, t: u64) -> f64 {
        if band.kind == BandKind::Shared {
            return self.older.as_deref().map_or(0., |v| value_at(v, t));
        }
        band.series
            .iter()
            .filter_map(|id| self.view(id.device))
            .map(|v| value_at(v, t))
            .sum()
    }

    fn on_pointer(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let origin = self.bounds.get().origin;
        let pos = ev.position - origin;
        let hover = match self.hit(pos) {
            Some((band, t)) => Hover {
                band: Some(band),
                at: Some((pos, t)),
            },
            None => Hover::default(),
        };
        if hover != self.hover {
            let band_changed = hover.band != self.hover.band;
            self.hover = hover;
            if band_changed {
                self.rebuild_spec(cx);
            }
            cx.notify();
        }
    }

    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = AppsSettings::read(self.place, cx);
        if new == self.settings {
            return;
        }
        self.settings = new;
        let snap = Arc::clone(self.hub.read(cx).current());
        self.refresh(&snap, true, cx);
    }

    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        // New bands start at 0 bytes and unknown counts: fill them again from the snapshot, so
        // the legend and tooltip do not wait for the next one.
        self.build_bands(cx);
        let snap = Arc::clone(self.hub.read(cx).current());
        self.update_values(&snap, self.newest().unwrap_or(0));
        self.rebuild_spec(cx);
        cx.notify();
    }

    fn sync_words(&mut self, cx: &mut Context<Self>) {
        let mut version = self.strings_version;
        if !self.words.refresh(&mut version, cx) {
            return;
        }
        self.strings_version = version;
        let shared = self.shared_name(cx);
        for band in self.bands.iter_mut().filter(|b| b.kind == BandKind::Shared) {
            band.name = shared.clone();
        }
        let labels = Self::labels_of(self.place, &self.words);
        self.chart.update(cx, |v, cx| v.set_labels(labels, cx));
        cx.notify();
    }

    /// The tooltip lines of band `i` at time `t` (`None`: the newest sample): name, value,
    /// share of In use and the process count, and for a worked-out kernel part that it is an
    /// estimate.
    pub fn tooltip_lines(&self, i: usize, t: Option<u64>, _: &App) -> SmallVec<[String; 4]> {
        let Some(band) = self.bands.get(i) else {
            return SmallVec::new();
        };
        let (value, in_use) = match t {
            Some(t) => (
                self.band_value(band, t),
                self.bands
                    .iter()
                    .map(|b| self.band_value(b, t))
                    .sum::<f64>(),
            ),
            None => (band.value, self.bands.iter().map(|b| b.value).sum::<f64>()),
        };
        let mut lines: SmallVec<[String; 4]> = smallvec![band.name.to_string()];
        let mut text = String::new();
        units::bytes(&mut text, value.max(0.) as u64);
        lines.push(text);
        if in_use > 0.
            && let Some(share) = &self.words.share
        {
            let mut pct = String::new();
            units::percent(&mut pct, (value / in_use * 100.) as f32);
            let mut text = String::new();
            let _ = share.write(&mut text, &[&pct]);
            lines.push(text);
        }
        if let (Some(n), Some(processes)) = (band.processes, &self.words.processes) {
            let mut text = String::new();
            let _ = processes.write_count(&mut text, u64::from(n), &[&n]);
            lines.push(text);
        }
        if let BandKind::Kernel(k) = band.kind
            && KERNEL_PARTS[usize::from(k)].2
        {
            lines.push(self.words.estimate.to_string());
        }
        lines
    }

    fn tooltip(
        &self,
        lines: SmallVec<[String; 4]>,
        at: Point<Pixels>,
        cx: &App,
    ) -> impl IntoElement {
        let (m, theme) = (metrics(cx), current(cx));
        let mut tip = div()
            .debug_selector(|| TOOLTIP_ID.into())
            .absolute()
            .map(|tip| {
                // Keep the tooltip inside the chart box: above or left of the pointer in the
                // lower or right half.
                let size = self.bounds.get().size;
                let gap = px(12. * m.ui_scale);
                let tip = if at.x > size.width / 2. {
                    tip.right(size.width - at.x + gap)
                } else {
                    tip.left(at.x + gap)
                };
                if at.y > size.height / 2. {
                    tip.bottom(size.height - at.y + gap)
                } else {
                    tip.top(at.y + gap)
                }
            })
            .flex()
            .flex_col()
            .px(m.sp[3])
            .py(m.sp[2])
            .bg(theme.menu_bg)
            .border_1()
            .border_color(theme.divider)
            .rounded(px(4. * m.ui_scale))
            .text_color(theme.text_primary);
        *tip.text_style() = t_sm(m);
        for (n, line) in lines.into_iter().enumerate() {
            let row = div().child(line);
            tip = tip.child(if n == 0 {
                row.font_weight(gpui_kit::FontWeight::MEDIUM)
            } else {
                row.text_color(theme.text_secondary)
            });
        }
        tip
    }

    /// Opens the Processes tab filtered to group band `i` (`ProcessesPage::open_mem_group`).
    /// "Other apps" and "Shared and kernel" open nothing. Selecting the tab builds the page.
    pub fn open_group(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(band) = self
            .bands
            .get(i)
            .filter(|b| matches!(b.kind, BandKind::Group(_)))
        else {
            return;
        };
        let (key, name) = (Arc::clone(&band.key), band.name.clone());
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.last_open = Some((Arc::clone(&key), name.clone()));
        }
        // The click listener runs inside this entity's update, and selecting the tab updates the
        // Performance page and its children.
        window.defer(cx, move |window, cx| {
            let Some(view) = main_window_view(cx) else {
                warn!(target: "tasklens::ui", "no main window for the memory legend click");
                return;
            };
            if view
                .read(cx)
                .page_entity::<ProcessesPage>(Tab::Processes)
                .is_none()
            {
                // Selecting the tab builds the page.
                view.update(cx, |v, cx| v.select_tab(Tab::Processes, window, cx));
            }
            let Some(page) = view.read(cx).page_entity::<ProcessesPage>(Tab::Processes) else {
                warn!(target: "tasklens::ui", "no Processes page for the memory legend click");
                return;
            };
            page.update(cx, |p, cx| p.open_mem_group(key, name, window, cx));
        });
    }

    /// One legend row: swatch, name, process count and value. `divider` draws the line above the
    /// first kernel row.
    fn legend_row(
        &self,
        i: usize,
        row: usize,
        divider: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (m, theme) = (*metrics(cx), current(cx).clone());
        let band = &self.bands[i];
        let swatch = match band.kind {
            BandKind::Group(_) => band.color,
            _ => Hsla {
                a: (band.color.a * 2.).min(1.),
                ..band.color
            },
        };
        let mut value = String::new();
        units::bytes(&mut value, band.value.max(0.) as u64);
        let count = band
            .processes
            .filter(|n| *n > 1)
            .zip(self.words.count.as_ref())
            .map(|(n, t)| {
                let mut s = String::new();
                let _ = t.write(&mut s, &[&n]);
                s
            });
        let dim = self.hover.band.is_some_and(|h| h != i);
        let group = matches!(band.kind, BandKind::Group(_));
        let mut value_el = div()
            .flex_none()
            .text_color(theme.text_secondary)
            .child(value);
        *value_el.text_style() = numeric_style(&m, m.text.sm);
        div()
            .id((LEGEND_ID, row))
            .debug_selector(move || format!("{LEGEND_ID}-{row}"))
            .flex()
            .flex_row()
            .items_center()
            .gap(m.sp[2])
            .h(px(LEGEND_ROW_H * m.ui_scale))
            .w_full()
            .when(divider, |d| d.border_t_1().border_color(theme.divider))
            .when(dim, |d| d.opacity(0.6))
            .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                let band = if *on { Some(i) } else { None };
                if *on || this.hover.band == Some(i) {
                    this.set_hovered(band, cx);
                }
            }))
            .when(group, |row| {
                row.cursor_pointer().on_click(cx.listener(
                    move |this, _: &ClickEvent, window, cx| {
                        this.open_group(i, window, cx);
                    },
                ))
            })
            .child(div().flex_none().size(px(SWATCH * m.ui_scale)).bg(swatch))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(theme.text_primary)
                    .child(band.name.clone()),
            )
            .children(count.map(|c| div().flex_none().text_color(theme.text_secondary).child(c)))
            .child(value_el)
    }
}

/// The Overview layout of a body of `size` with `rows` legend rows: whether the legend goes
/// below the chart, and the rows shown and behind "+n more". At the right the legend is the body
/// height; below, it takes at most half the body, whole rows only.
pub fn overview_legend(size: Size<Pixels>, rows: usize, m: &UiMetrics) -> (bool, usize, usize) {
    let row_h = LEGEND_ROW_H * m.ui_scale;
    let below = f32::from(size.width) < LEGEND_BELOW_W * m.ui_scale;
    let height = if below {
        (rows as f32 * row_h).min(f32::from(size.height) / 2.)
    } else {
        f32::from(size.height)
    };
    let (shown, more) = legend_fit(rows, height, row_h);
    (below, shown, more)
}

impl Render for MemoryAppsChart {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let bounds = Rc::clone(&self.bounds);
        let probe = canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
            .absolute()
            .inset_0();

        // Group bands and "Other apps" top first, then the kernel bands; whole rows only, the
        // last row that fits says "+n more".
        let order = legend_order(&self.bands);
        let rows = order.len();
        let row_h = LEGEND_ROW_H * m.ui_scale;
        let page_h = super::page::GRAPH_H_OVERALL * m.ui_scale;
        let (below, shown, more) = match self.place {
            AppsPlace::Page => {
                let (shown, more) = legend_fit(rows, page_h, row_h);
                (false, shown, more)
            }
            AppsPlace::Overview => overview_legend(self.body.get(), rows, &m),
        };

        // Over the chart: the values at the pointer's time. Over a legend row: the newest values,
        // at the chart's right edge level with the row, or at its bottom right corner when the
        // legend is below.
        let chart_size = self.bounds.get().size;
        let tooltip = match self.hover {
            Hover {
                band: Some(band),
                at: Some((at, t)),
            } => Some(self.tooltip(self.tooltip_lines(band, Some(t), cx), at, cx)),
            Hover {
                band: Some(band),
                at: None,
            } if below => {
                let at = Point {
                    x: chart_size.width,
                    y: chart_size.height,
                };
                Some(self.tooltip(self.tooltip_lines(band, None, cx), at, cx))
            }
            Hover {
                band: Some(band),
                at: None,
            } => order
                .iter()
                .position(|&i| i == band)
                .and_then(|row| legend_anchor(row, shown, row_h, chart_size.width))
                .map(|at| self.tooltip(self.tooltip_lines(band, None, cx), at, cx)),
            Hover { band: None, .. } => None,
        };
        let chart = div()
            .id(APPS_ID)
            .debug_selector(|| APPS_ID.into())
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(probe)
            .child(
                self.chart
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.on_pointer(ev, cx)))
            .on_hover(cx.listener(|this, on: &bool, _, cx| {
                if !*on && this.hover != Hover::default() {
                    this.hover = Hover::default();
                    this.rebuild_spec(cx);
                    cx.notify();
                }
            }))
            .children(tooltip);
        let chart = if below {
            chart.w_full()
        } else {
            chart.h_full()
        };

        let mut legend = div()
            .debug_selector(|| LEGEND_ID.into())
            .flex()
            .flex_col()
            .flex_none()
            .overflow_hidden();
        legend = if below {
            let lines = shown + usize::from(more > 0);
            legend.w_full().h(px(lines as f32 * row_h))
        } else {
            legend.w(px(LEGEND_W * m.ui_scale)).h_full()
        };
        *legend.text_style() = t_sm(&m);
        for (row, &i) in order.iter().enumerate().take(shown) {
            let divider = row > 0
                && self.bands[i].kind.is_kernel()
                && !self.bands[order[row - 1]].kind.is_kernel();
            legend = legend.child(self.legend_row(i, row, divider, cx));
        }
        if more > 0
            && let Some(t) = &self.words.more
        {
            let mut text = String::new();
            let _ = t.write(&mut text, &[&more]);
            legend = legend.child(
                div()
                    .debug_selector(move || format!("{LEGEND_ID}-more"))
                    .h(px(row_h))
                    .flex()
                    .items_center()
                    .text_color(theme.text_secondary)
                    .child(text),
            );
        }
        let frame = div().flex().gap(m.sp[4]).w_full();
        let frame: AnyElement = match self.place {
            AppsPlace::Page => frame
                .flex_row()
                .h(px(page_h))
                .child(chart)
                .child(legend)
                .into_any_element(),
            AppsPlace::Overview => {
                // The probe re-renders this view when the body crosses the legend width rule
                // or another number of legend rows fits.
                let class = |(below, shown, more): (bool, usize, usize)| {
                    u32::from(below) | ((shown as u32) << 1) | (u32::from(more > 0) << 31)
                };
                let probe_m = m;
                let frame = if below {
                    frame.flex_col()
                } else {
                    frame.flex_row()
                };
                frame
                    .relative()
                    .size_full()
                    .child(chart)
                    .child(legend)
                    .child(class_probe(
                        Rc::clone(&self.body),
                        class((below, shown, more)),
                        move |size| class(overview_legend(size, rows, &probe_m)),
                        cx.entity_id(),
                    ))
                    .into_any_element()
            }
        };
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    #[test]
    fn legend_fit_rules() {
        assert_eq!(legend_fit(5, 100., 20.), (5, 0));
        assert_eq!(legend_fit(6, 100., 20.), (4, 2));
        assert_eq!(legend_fit(6, 0., 20.), (0, 6));
    }

    #[test]
    fn key_hash_is_stable() {
        assert_eq!(key_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(key_hash("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn split_top_counts() {
        let ranked: Vec<Arc<str>> = (0..5).map(|i| key(&format!("exe:{i}"))).collect();
        assert_eq!(split_top(&ranked, 3).0.len(), 3);
        assert_eq!(split_top(&ranked, 10).1.len(), 0);
    }
}
