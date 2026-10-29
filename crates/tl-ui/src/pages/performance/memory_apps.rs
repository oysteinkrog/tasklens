//! The memory-by-app chart (docs/ui/performance.md UI-PRF-060, D-036): memory in use as a stacked
//! chart of memory groups. From the bottom: the top N groups, largest first, then "Other apps",
//! then "Shared and kernel". The top of the stack is In use.
//!
//! The engine publishes the top 20 groups and the two fixed bands as `MEM_GROUP` series on
//! `DeviceIdentity::MemGroup` devices. This chart ranks every group with data in the visible
//! range by its mean, keeps the order sticky ([`Ranker`]: at most every 10 s, and only for a 10%
//! lead), draws the top N in their own colours and folds the rest into "Other apps". A group keeps
//! its colour through `performance.memory_by_app.colors` ([`pick_colors`]).
//!
//! The legend lists the bands top first and shows whole rows only, with "+n more" for the rest
//! ([`legend_fit`]). Hovering a band or its legend row raises it (the others drop to 40%) and
//! shows a tooltip. The Memory page puts "Groups shown" in its context menu.
//!
//! The chart itself is a cached `ChartView`; this view re-renders for the legend and hover only.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    MouseMoveEvent, ParentElement as _, Pixels, Point, Render, SharedString, StyleRefinement,
    Styled as _, Subscription, Window, canvas, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::devices::DeviceIdentity;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{GroupColor, MEMORY_BY_APP_COLORS_MAX, MEMORY_BY_APP_SLOTS};
use tl_model::snapshot::Snapshot;
use tl_model::time::now_boot_ns;
use tl_model::units;

use super::details::Template;
use crate::chart::geometry::right_edge_ns;
use crate::chart::view::find_tiered;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale,
};
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::{Section, TlSettings};
use crate::text::style::{numeric_style, t_sm};
use crate::theme::{UiMetrics, current, metrics};

/// Debug selector of the chart box.
pub const APPS_ID: &str = "perf-mem-apps";
/// Debug selector of the legend; each row adds `-<n>`, top row first.
pub const LEGEND_ID: &str = "perf-mem-apps-legend";
/// Debug selector of the hover tooltip.
pub const TOOLTIP_ID: &str = "perf-mem-apps-tooltip";

/// Key of the "Other apps" band (`memgroup:other`).
pub const OTHER_KEY: &str = "other";
/// Key of the "Shared and kernel" band (`memgroup:shared`).
pub const SHARED_KEY: &str = "shared";

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
const SWATCH: f32 = 10.;
/// Fill alpha of a group band against its line colour.
const GROUP_FILL_ALPHA: f32 = 0.6;
/// "Other apps" is `text_secondary` at 50%; "Shared and kernel" is `mem_fill` at 40%.
const OTHER_ALPHA: f32 = 0.5;
const SHARED_ALPHA: f32 = 0.4;

/// The sticky order of the memory groups (UI-PRF-060).
#[derive(Clone, Debug, Default)]
pub struct Ranker {
    order: Vec<Arc<str>>,
    ranked_at: Option<u64>,
}

impl Ranker {
    /// Ranks `means` (group key, mean over the visible range) at `now_ns`. Does nothing within
    /// [`RERANK_NS`] of the last ranking unless `force` or nothing is ranked yet. Groups ranked before keep their order
    /// unless the one below has a mean at least [`RANK_MARGIN`] times larger; new groups join
    /// below them, largest first. Groups without a mean leave. Returns whether the order changed.
    pub fn rank(&mut self, means: &[(Arc<str>, f64)], now_ns: u64, force: bool) -> bool {
        if !force
            && !self.order.is_empty()
            && self
                .ranked_at
                .is_some_and(|t| now_ns.saturating_sub(t) < RERANK_NS)
        {
            return false;
        }
        self.ranked_at = Some(now_ns);
        let mean_of = |key: &str| means.iter().find(|(k, _)| &**k == key).map(|m| m.1);
        let mut order: Vec<(Arc<str>, f64)> = self
            .order
            .iter()
            .filter_map(|k| mean_of(k).map(|m| (Arc::clone(k), m)))
            .collect();
        let mut fresh: Vec<(Arc<str>, f64)> = means
            .iter()
            .filter(|(k, _)| !order.iter().any(|(o, _)| o == k))
            .cloned()
            .collect();
        fresh.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        order.extend(fresh);
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
/// A key with a saved pick keeps its slot unless a key before it in `shown` took that slot
/// already. Any other key takes the slot its key hashes to, or the next free slot among the
/// shown keys. With more than 12 keys shown, slots repeat. The shown keys then move to the end
/// of `picks` (most recently shown last), and the oldest picks past 64 are dropped.
pub fn pick_colors(picks: &mut Vec<GroupColor>, shown: &[Arc<str>]) -> (Vec<u8>, bool) {
    let n = usize::from(MEMORY_BY_APP_SLOTS);
    let mut slots: Vec<Option<u8>> = vec![None; shown.len()];
    let mut used = vec![false; n];
    for (i, key) in shown.iter().enumerate() {
        if let Some(p) = picks.iter().find(|p| p.key == **key)
            && !used[usize::from(p.slot) % n]
        {
            slots[i] = Some(p.slot % MEMORY_BY_APP_SLOTS);
            used[usize::from(p.slot) % n] = true;
        }
    }
    for (i, key) in shown.iter().enumerate() {
        if slots[i].is_some() {
            continue;
        }
        let home = (key_hash(key) % n as u64) as usize;
        let slot = (0..n)
            .map(|d| (home + d) % n)
            .find(|&s| !used[s])
            .unwrap_or(home);
        used[slot] = true;
        slots[i] = Some(slot as u8);
    }
    let slots: Vec<u8> = slots.into_iter().map(Option::unwrap_or_default).collect();

    let mut next: Vec<GroupColor> = picks
        .iter()
        .filter(|p| !shown.iter().any(|k| **k == p.key))
        .cloned()
        .collect();
    next.extend(shown.iter().zip(&slots).map(|(key, &slot)| GroupColor {
        key: key.to_string(),
        slot,
    }));
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
    Shared,
}

/// One band, bottom first in [`MemoryAppsChart::bands`].
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub key: Arc<str>,
    pub name: SharedString,
    pub kind: BandKind,
    pub color: Hsla,
    /// The series stacked for this band. "Other apps" has the groups outside the top N and the
    /// `memgroup:other` series.
    pub series: SmallVec<[SeriesId; 1]>,
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

/// The value of `view` at `t_ns`: the nearest valid sample within 1.5 periods, else 0.
fn value_at(view: &SeriesView, t_ns: u64) -> f64 {
    let samples: &[Sample] = &view.samples;
    let Some(i) = crate::chart::hover::nearest_index(samples, t_ns) else {
        return 0.;
    };
    let s = samples[i];
    let slack = view.period_ns.saturating_mul(3) / 2;
    if s.t_ns.abs_diff(t_ns) <= slack.max(1) {
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
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            top_n: usize::from(s.performance.memory_by_app.top_n),
            range_ns: u64::from(s.performance.graph_range_s) * 1_000_000_000,
            smooth: s.motion.smooth_graphs,
            rounded: s.appearance.rounded_corners,
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
    chart: Entity<ChartView>,
    hub: Entity<SnapshotHub>,
    ranker: Ranker,
    /// Every group with a series, by key.
    groups: Vec<Group>,
    /// Devices of the two fixed bands.
    other: Option<DeviceIdx>,
    shared: Option<DeviceIdx>,
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
    _subs: SmallVec<[Subscription; 4]>,
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
    pub fn new(hub: Entity<SnapshotHub>, cx: &mut Context<Self>) -> Self {
        let settings = AppsSettings::read(cx);
        let mut words = AppsWords::default();
        let mut strings_version = u64::MAX;
        words.refresh(&mut strings_version, cx);
        let snap = Arc::clone(hub.read(cx).current());
        let total = snap.system.memory.total;
        let spec = Self::spec_of(&[], None, total, settings);
        let labels = Self::labels_of(&words);
        let chart = cx.new(|cx| {
            let mut v = ChartView::new(spec, labels, &hub, cx).with_newest_marks();
            v.set_value_sum(true, cx);
            v
        });
        let subs = smallvec![
            cx.subscribe(&hub, Self::on_snapshot),
            cx.observe_global::<TlSettings>(Self::sync_settings),
            cx.observe_global::<UiMetrics>(Self::sync_style),
            cx.observe_global::<Strings>(Self::sync_words),
        ];
        let mut this = Self {
            chart,
            hub,
            ranker: Ranker::default(),
            groups: Vec::new(),
            other: None,
            shared: None,
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
            _subs: subs,
        };
        this.refresh(&snap, true, cx);
        this
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

    fn labels_of(words: &AppsWords) -> ChartLabels {
        ChartLabels {
            heading: Some(words.heading.clone()),
            value_fmt: ValueFormat::Bytes,
            value_align: ValueAlign::Right,
            range_word: words.range.clone(),
            legend: SmallVec::new(),
            show_axis_max: false,
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
            let (line, fill) = match band.kind {
                BandKind::Group(_) => (
                    band.color,
                    Hsla {
                        a: band.color.a * GROUP_FILL_ALPHA,
                        ..band.color
                    },
                ),
                BandKind::Other | BandKind::Shared => (band.color, band.color),
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
            ..ChartSpec::default()
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
        if !ev
            .mask
            .intersects(ComponentMask::SERIES | ComponentMask::SYSTEM | ComponentMask::DEVICES)
        {
            return;
        }
        let snap = Arc::clone(hub.read(cx).current());
        self.refresh(&snap, false, cx);
    }

    /// The series view of `device` for the chart range.
    fn view<'a>(&self, snap: &'a Snapshot, device: DeviceIdx) -> Option<&'a Arc<SeriesView>> {
        find_tiered(&snap.series, series_id(device), self.settings.range_ns).0
    }

    /// Re-reads the groups, ranks them (sticky unless `force`), and rebuilds the bands and the
    /// spec when the bands changed. The legend values follow every call.
    fn refresh(&mut self, snap: &Snapshot, force: bool, cx: &mut Context<Self>) {
        self.groups.clear();
        self.other = None;
        self.shared = None;
        for e in snap.devices.entries.iter() {
            if let DeviceIdentity::MemGroup { key } = &e.identity {
                match &**key {
                    OTHER_KEY => self.other = Some(e.idx),
                    SHARED_KEY => self.shared = Some(e.idx),
                    _ => self.groups.push(Group {
                        key: Arc::clone(key),
                        name: Arc::clone(&e.display_name),
                        device: e.idx,
                    }),
                }
            }
        }

        // The reference clock: the "Shared and kernel" band gets a sample on every sweep.
        let reference = self.shared.and_then(|d| self.view(snap, d));
        let newest = reference.and_then(|v| v.samples.last().map(|s| s.t_ns));
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
                let Some(view) = self.view(snap, g.device) else {
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

        let (top, rest) = split_top(self.ranker.order(), self.settings.top_n);
        let total = snap.system.memory.total;
        let structure_changed = top != self.top.as_slice()
            || rest != self.rest.as_slice()
            || total != self.total
            || force;
        if structure_changed {
            self.top = top.to_vec();
            self.rest = rest.to_vec();
            self.total = total;
            self.build_bands(cx);
            self.rebuild_spec(cx);
            cx.notify();
        }
        if self.update_values(snap, newest.unwrap_or(0)) {
            cx.notify();
        }
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
            bands.push(Band {
                key: Arc::clone(key),
                name: SharedString::from(g.name.to_string()),
                kind: BandKind::Group(slot),
                color: theme.graph.groups[usize::from(slot) % theme.graph.groups.len()],
                series: smallvec![series_id(g.device)],
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
            bands.push(Band {
                key: Arc::from(OTHER_KEY),
                name: name_of(OTHER_KEY, Some(other)),
                kind: BandKind::Other,
                color: Hsla {
                    a: theme.text_secondary.a * OTHER_ALPHA,
                    ..theme.text_secondary
                },
                series,
                processes: None,
                value: 0.,
            });
        }
        if let Some(shared) = self.shared {
            bands.push(Band {
                key: Arc::from(SHARED_KEY),
                name: name_of(SHARED_KEY, Some(shared)),
                kind: BandKind::Shared,
                color: Hsla {
                    a: theme.graph.mem_fill.a * SHARED_ALPHA,
                    ..theme.graph.mem_fill
                },
                series: smallvec![series_id(shared)],
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
        let range = self.settings.range_ns;
        for band in &mut self.bands {
            let value: f64 = band
                .series
                .iter()
                .filter_map(|id| find_tiered(&snap.series, *id, range).0)
                .map(|v| value_at(v, newest))
                .sum();
            let processes = match band.kind {
                BandKind::Group(_) => count_of(&band.key),
                BandKind::Shared => None,
                BandKind::Other => {
                    let rest: u32 = self.rest.iter().filter_map(|k| count_of(k)).sum();
                    count_of(OTHER_KEY).map(|n| n + rest)
                }
            };
            if value != band.value || processes != band.processes {
                band.value = value;
                band.processes = processes;
                changed = true;
            }
        }
        changed
    }

    /// The band under chart-local `pos` and the time there, from the stacked values.
    fn hit(&self, pos: Point<Pixels>, cx: &App) -> Option<(usize, u64)> {
        let b = self.bounds.get();
        let (w, h) = (f32::from(b.size.width) - 2., f32::from(b.size.height) - 2.);
        if w <= 0. || h <= 0. || self.total == 0 {
            return None;
        }
        let snap = self.hub.read(cx).current();
        let reference = self.shared.and_then(|d| self.view(snap, d))?;
        let newest = reference.samples.last()?.t_ns;
        let right = right_edge_ns(
            now_boot_ns(),
            newest,
            reference.period_ns,
            self.settings.smooth,
        );
        let x = (f32::from(pos.x) - 1.).clamp(0., w);
        let ns = f64::from(w - x) * self.settings.range_ns as f64 / f64::from(w);
        let t = right.saturating_sub(ns.round() as u64);
        let level = f64::from((h - (f32::from(pos.y) - 1.)).clamp(0., h) / h) * self.total as f64;
        let mut top = 0.;
        for (i, band) in self.bands.iter().enumerate() {
            top += self.band_value(snap, band, t);
            if level <= top {
                return Some((i, t));
            }
        }
        None
    }

    fn band_value(&self, snap: &Snapshot, band: &Band, t: u64) -> f64 {
        band.series
            .iter()
            .filter_map(|id| find_tiered(&snap.series, *id, self.settings.range_ns).0)
            .map(|v| value_at(v, t))
            .sum()
    }

    fn on_pointer(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let origin = self.bounds.get().origin;
        let pos = ev.position - origin;
        let hover = match self.hit(pos, cx) {
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
        let new = AppsSettings::read(cx);
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
        self.build_bands(cx);
        self.rebuild_spec(cx);
        cx.notify();
    }

    fn sync_words(&mut self, cx: &mut Context<Self>) {
        let mut version = self.strings_version;
        if !self.words.refresh(&mut version, cx) {
            return;
        }
        self.strings_version = version;
        let labels = Self::labels_of(&self.words);
        self.chart.update(cx, |v, cx| v.set_labels(labels, cx));
        cx.notify();
    }

    /// The tooltip lines of band `i` at time `t` (`None`: the newest sample): name, value,
    /// share of In use and the process count.
    pub fn tooltip_lines(&self, i: usize, t: Option<u64>, cx: &App) -> SmallVec<[String; 4]> {
        let Some(band) = self.bands.get(i) else {
            return SmallVec::new();
        };
        let snap = self.hub.read(cx).current();
        let (value, in_use) = match t {
            Some(t) => (
                self.band_value(snap, band, t),
                self.bands
                    .iter()
                    .map(|b| self.band_value(snap, b, t))
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
            .left(at.x + px(12. * m.ui_scale))
            .top(at.y + px(12. * m.ui_scale))
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

    /// One legend row: swatch, name, process count and value.
    fn legend_row(&self, i: usize, row: usize, cx: &mut Context<Self>) -> impl IntoElement {
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
            .when(dim, |d| d.opacity(0.6))
            .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                let band = if *on { Some(i) } else { None };
                if *on || this.hover.band == Some(i) {
                    this.set_hovered(band, cx);
                }
            }))
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

impl Render for MemoryAppsChart {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let h = super::page::GRAPH_H_OVERALL * m.ui_scale;
        let bounds = Rc::clone(&self.bounds);
        let probe = canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
            .absolute()
            .inset_0();

        let tooltip = self.hover.at.and_then(|(at, t)| {
            let band = self.hover.band?;
            Some(self.tooltip(self.tooltip_lines(band, Some(t), cx), at, cx))
        });
        let chart = div()
            .id(APPS_ID)
            .debug_selector(|| APPS_ID.into())
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
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

        // Top band first; whole rows only, the last row that fits says "+n more".
        let rows = self.bands.len();
        let (shown, more) = legend_fit(rows, h, LEGEND_ROW_H * m.ui_scale);
        let mut legend = div()
            .debug_selector(|| LEGEND_ID.into())
            .flex()
            .flex_col()
            .flex_none()
            .w(px(LEGEND_W * m.ui_scale))
            .h_full()
            .overflow_hidden();
        *legend.text_style() = t_sm(&m);
        for row in 0..shown {
            let i = rows - 1 - row;
            legend = legend.child(self.legend_row(i, row, cx));
        }
        if more > 0
            && let Some(t) = &self.words.more
        {
            let mut text = String::new();
            let _ = t.write(&mut text, &[&more]);
            legend = legend.child(
                div()
                    .debug_selector(move || format!("{LEGEND_ID}-more"))
                    .h(px(LEGEND_ROW_H * m.ui_scale))
                    .flex()
                    .items_center()
                    .text_color(theme.text_secondary)
                    .child(text),
            );
        }
        div()
            .flex()
            .flex_row()
            .gap(m.sp[4])
            .w_full()
            .h(px(h))
            .child(chart)
            .child(legend)
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
