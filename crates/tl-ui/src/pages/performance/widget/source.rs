//! The data of one widget source, shared by every widget that shows it (ADR-019 "Entities and
//! data flow").
//!
//! [`SourceData`] is one entity per source in use. It resolves its [`SourceSelector`] to a
//! device and a series from the device catalog (again only when the catalog changes), reads the
//! current value, the bar's denominator and the stats from each snapshot, formats the value and
//! stats text once for all its widgets, and notifies only when something a widget shows has
//! changed. It emits [`AvailabilityChanged`] when its device goes or comes back, or the device
//! stops or starts reporting the metric; that is the only source event a container listens to.
//!
//! [`SourceStore`] (one per Overview page) maps selectors to their `SourceData`. It keeps weak
//! handles only: a `SourceData` lives while a widget holds it, and is dropped with the last one.
//! The store also tells slot sources (`DeviceSelector::Slot`) which GPUs the box shows.
//!
//! Phase 1 reads GPU devices only. The source reads the Overview's snapshot (`box_snapshot`),
//! so it follows the timeline's pause like the boxes do.

use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, SharedString, Subscription, WeakEntity,
};
use rustc_hash::FxHashMap;
use tl_model::ids::{DeviceIdx, SeriesId};
use tl_model::layout::{
    DeviceSelector, FormSet, METRIC_CLOCK_GFX, METRIC_CLOCK_MEM, METRIC_POWER, METRIC_TEMP,
    METRIC_VRAM, MetricKey, SourceSelector,
};
use tl_model::settings::OverviewRange;
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::system::{GpuEngine, GpuTotals, Metric};
use tl_model::validity::Validity;

use super::meta::{MetricMeta, ValueText};
use crate::chart::YScale;
use crate::chart::labels::{NetFormat, format_value};
use crate::components::validity::copy_text;
use crate::hub::{SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::pages::performance::DeviceKey;
use crate::pages::performance::details::Template;
use crate::pages::performance::gpu::{GpuVendor, gpu_totals};
use crate::pages::performance::overview::picked::{box_mask, box_snapshot};
use crate::pages::performance::overview::range::{Figures, current_range, figures_text};
use crate::pages::performance::overview::timeline::OverviewPause;
use crate::settings_global::TlSettings;

/// The name of VRAM.
const KEY_VRAM: &str = "perf-overview-gpu-vram";
/// The stats line, "min {$min}  avg {$avg}  peak {$peak}".
const KEY_FIGURES: &str = "perf-overview-figures";

/// The parts of a snapshot a source reads: the device record, the series and the history for
/// the stats, the catalog for the device, and the inventory for the vendor's engine names.
const MASK: ComponentMask = ComponentMask(
    ComponentMask::SYSTEM.0
        | ComponentMask::SERIES.0
        | ComponentMask::DEVICES.0
        | ComponentMask::DEVICE_HISTORY.0
        | ComponentMask::INVENTORY.0,
);

/// Whether a source can be shown, and if not, why (UI-PRF-061 "Unavailable widget").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Availability {
    Available,
    /// The device is gone, or the slot has no device: "Not connected".
    Gone,
    /// The device does not report the metric: "Not reported by this GPU".
    NotReported,
    /// The metric key is unknown to this build: "Needs a newer tasklens".
    Unknown,
}

/// Emitted when a source's [`Availability`] changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvailabilityChanged(pub Availability);

/// The `en-US.ftl` key of an engine's name on the GPU page for `vendor`.
fn engine_name_key(vendor: GpuVendor, engine: GpuEngine) -> &'static str {
    match engine {
        GpuEngine::Gpu if vendor == GpuVendor::Amd => "perf-gpu-engine-graphics",
        GpuEngine::Gpu => "perf-gpu-engine-3d",
        GpuEngine::MemCtl => "perf-gpu-engine-memctl",
        GpuEngine::Enc => "perf-gpu-engine-enc",
        GpuEngine::Dec => "perf-gpu-engine-dec",
        GpuEngine::Jpeg => "perf-gpu-engine-jpeg",
        GpuEngine::Ofa => "perf-gpu-engine-ofa",
        GpuEngine::Compute => "perf-gpu-engine-compute",
        GpuEngine::Dma => "perf-gpu-engine-dma",
        GpuEngine::Render => "perf-gpu-engine-render",
        GpuEngine::Copy => "perf-gpu-engine-copy",
        GpuEngine::Video => "perf-gpu-engine-video",
        GpuEngine::VideoEnhance => "perf-gpu-engine-video-enhance",
    }
}

/// The `en-US.ftl` key of the name of `metric` on a GPU of `vendor`; `None` for an unknown key.
pub fn metric_name_key(metric: &MetricKey, vendor: GpuVendor) -> Option<&'static str> {
    if let Some(engine) = metric.gpu_engine() {
        return Some(engine_name_key(vendor, engine));
    }
    match metric.as_str() {
        METRIC_VRAM => Some(KEY_VRAM),
        METRIC_CLOCK_GFX => Some("perf-widget-metric-clock-gfx"),
        METRIC_CLOCK_MEM => Some("perf-widget-metric-clock-mem"),
        METRIC_POWER => Some("perf-widget-metric-power"),
        METRIC_TEMP => Some("perf-widget-metric-temp"),
        _ => None,
    }
}

fn as_f32<T: Copy + Default>(m: Metric<T>, f: impl Fn(T) -> f32) -> Metric<f32> {
    Metric {
        value: f(m.value),
        validity: m.validity,
    }
}

/// The current reading of `metric` in the record `t`: 3D, encode and decode from their totals,
/// the other engines from the engine list, and the device facts. `Unsupported` when the device
/// does not report it, or the key is unknown.
pub fn reading(metric: &MetricKey, t: &GpuTotals) -> Metric<f32> {
    if let Some(engine) = metric.gpu_engine() {
        return match engine {
            GpuEngine::Gpu => t.util_pct,
            GpuEngine::Enc => t.enc_pct,
            GpuEngine::Dec => t.dec_pct,
            e => t.engines.get(e),
        };
    }
    match metric.as_str() {
        METRIC_VRAM => as_f32(t.mem_used, |v| v as f32),
        METRIC_CLOCK_GFX => as_f32(t.clock_gfx_mhz, |v| v as f32),
        METRIC_CLOCK_MEM => as_f32(t.clock_mem_mhz, |v| v as f32),
        METRIC_POWER => t.power_w,
        METRIC_TEMP => t.temp_c,
        _ => Metric::with(Validity::Unsupported),
    }
}

fn shown(v: Validity) -> bool {
    matches!(v, Validity::Valid | Validity::Stale)
}

/// The resolved device of a source.
#[derive(Clone, Debug, PartialEq)]
struct Resolved {
    idx: DeviceIdx,
    name: SharedString,
}

/// What a source last read, compared to tell whether its widgets must notify.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reading {
    availability: Availability,
    value: Metric<f32>,
    bar_max: Option<f32>,
    y_scale: YScale,
    offered: FormSet,
    figures: Option<Figures>,
}

/// One source in use: its device, its current value and its stats over the Overview range.
pub struct SourceData {
    hub: Entity<SnapshotHub>,
    selector: SourceSelector,
    meta: Option<&'static MetricMeta>,
    /// The device key the selector stands for: its own key, or the slot's GPU from the store.
    key: Option<Arc<str>>,
    /// The catalog version the device was resolved at; `None` forces a new lookup.
    catalog_version: Option<u64>,
    device: Option<Resolved>,
    vendor: GpuVendor,
    range: OverviewRange,
    reading: Reading,
    name: SharedString,
    value_text: SharedString,
    stats_text: SharedString,
    figures_words: Template,
    strings_version: u64,
    /// Bumped each time the source notifies.
    revision: u64,
    _subs: [Subscription; 4],
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: std::cell::Cell<u32>,
    /// How many times the stats were worked out.
    #[cfg(any(test, feature = "test-hooks"))]
    pub stats_count: std::cell::Cell<u32>,
}

impl EventEmitter<AvailabilityChanged> for SourceData {}

impl SourceData {
    /// A source for `selector`. `slot_key` is the device key of the GPU in the selector's slot
    /// (ignored for a `Key` selector).
    pub fn new(
        hub: &Entity<SnapshotHub>,
        selector: SourceSelector,
        slot_key: Option<Arc<str>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let key = match &selector.device {
            DeviceSelector::Key(k) => Some(Arc::clone(k)),
            DeviceSelector::Slot(_) => slot_key,
        };
        let meta = MetricMeta::of(&selector.metric);
        let mut this = Self {
            hub: hub.clone(),
            selector,
            meta,
            key,
            catalog_version: None,
            device: None,
            vendor: GpuVendor::Unknown,
            range: current_range(cx),
            reading: Reading {
                availability: Availability::Gone,
                value: Metric::default(),
                bar_max: None,
                y_scale: YScale::StickyUp { hw_max: None },
                offered: FormSet::empty(),
                figures: None,
            },
            name: SharedString::default(),
            value_text: SharedString::default(),
            stats_text: SharedString::default(),
            figures_words: Template::new(cx, KEY_FIGURES, &["min", "avg", "peak"]),
            strings_version: cx.try_global::<Strings>().map_or(0, |s| s.strings_version),
            revision: 0,
            _subs: [
                cx.subscribe(hub, Self::on_snapshot),
                cx.observe_global::<OverviewPause>(Self::on_pause),
                cx.observe_global::<TlSettings>(Self::on_settings),
                cx.observe_global::<Strings>(Self::on_strings),
            ],
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            stats_count: std::cell::Cell::new(0),
        };
        let snap = box_snapshot(hub, cx);
        this.refresh(&snap, true, cx);
        this.format_all(cx);
        this
    }

    pub fn selector(&self) -> &SourceSelector {
        &self.selector
    }

    /// The metric's metadata; `None` for a key this build does not know.
    pub fn meta(&self) -> Option<&'static MetricMeta> {
        self.meta
    }

    pub fn availability(&self) -> Availability {
        self.reading.availability
    }

    /// The device the source reads, while it is present.
    pub fn device(&self) -> Option<DeviceIdx> {
        self.device.as_ref().map(|d| d.idx)
    }

    /// The device's name in the catalog, while it is present.
    pub fn device_name(&self) -> Option<&SharedString> {
        self.device.as_ref().map(|d| &d.name)
    }

    /// The series a chart of the source draws, while the source is available.
    pub fn series(&self) -> Option<SeriesId> {
        if self.reading.availability != Availability::Available {
            return None;
        }
        Some(SeriesId {
            device: self.device()?,
            metric: self.selector.metric.metric()?,
        })
    }

    /// The metric's name: the engine's name for the GPU's vendor, "VRAM", "Graphics clock" and
    /// so on. The raw key for a key this build does not know.
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// The current value in the metric's format, empty while the source is unavailable.
    pub fn value_text(&self) -> &SharedString {
        &self.value_text
    }

    /// "min 6%  avg 42%  peak 100%" over the Overview range, empty without a valid sample.
    pub fn stats_text(&self) -> &SharedString {
        &self.stats_text
    }

    /// How full the bar is, 0 to 1; `None` without a bar or a value.
    pub fn bar_fraction(&self) -> Option<f32> {
        let r = &self.reading;
        let max = r.bar_max?;
        if !shown(r.value.validity) {
            return None;
        }
        let v = r.value.value;
        Some(if v.is_finite() {
            (v / max).clamp(0., 1.)
        } else {
            0.
        })
    }

    /// The chart's Y axis on this device.
    pub fn y_scale(&self) -> YScale {
        self.reading.y_scale
    }

    /// The forms the metric offers on this device (`MetricMeta::offered_forms`).
    pub fn offered_forms(&self) -> FormSet {
        self.reading.offered
    }

    /// Bumped each time the source notifies.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The slot's GPU changed (store): the source reads the new device.
    pub fn set_slot_key(&mut self, key: Option<Arc<str>>, cx: &mut Context<Self>) {
        if !matches!(self.selector.device, DeviceSelector::Slot(_)) || self.key == key {
            return;
        }
        self.key = key;
        self.catalog_version = None;
        let snap = box_snapshot(&self.hub, cx);
        self.update(&snap, true, cx);
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let mask = box_mask(ev.mask, cx);
        if !mask.intersects(MASK) {
            return;
        }
        let series_changed = self
            .series()
            .is_some_and(|id| ev.changed_series.binary_search(&id).is_ok())
            || mask.intersects(ComponentMask::DEVICE_HISTORY);
        let snap = box_snapshot(&hub, cx);
        self.update(&snap, series_changed, cx);
    }

    /// The Overview was held at a time or made live: read the other snapshot.
    fn on_pause(&mut self, cx: &mut Context<Self>) {
        let snap = box_snapshot(&self.hub, cx);
        self.update(&snap, true, cx);
    }

    /// A new Overview range: new stats.
    fn on_settings(&mut self, cx: &mut Context<Self>) {
        let range = current_range(cx);
        if range == self.range {
            return;
        }
        self.range = range;
        let snap = box_snapshot(&self.hub, cx);
        self.update(&snap, true, cx);
    }

    /// A new language: new name and stats words.
    fn on_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.figures_words = Template::new(cx, KEY_FIGURES, &["min", "avg", "peak"]);
        self.format_all(cx);
        self.changed(cx);
    }

    /// Reads `snap`; with `stats`, works the stats out again. Formats and notifies when
    /// anything a widget shows changed, and emits [`AvailabilityChanged`] on an availability
    /// change.
    fn update(&mut self, snap: &Snapshot, stats: bool, cx: &mut Context<Self>) {
        let before = self.reading;
        let vendor = self.vendor;
        let device = self.device.clone();
        self.refresh(snap, stats, cx);
        if self.reading == before && self.vendor == vendor && self.device == device {
            return;
        }
        self.format_all(cx);
        if self.reading.availability != before.availability {
            cx.emit(AvailabilityChanged(self.reading.availability));
        }
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        #[cfg(any(test, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// Resolves the device when the catalog changed and reads the record and the stats.
    fn refresh(&mut self, snap: &Snapshot, stats: bool, cx: &App) {
        if self.catalog_version != Some(snap.devices.version) {
            self.catalog_version = Some(snap.devices.version);
            self.device = self.key.as_deref().and_then(|key| {
                snap.devices
                    .entries
                    .iter()
                    .filter(|e| e.present)
                    .find(|e| DeviceKey::from_identity(&e.identity).is_some_and(|k| &*k.0 == key))
                    .map(|e| Resolved {
                        idx: e.idx,
                        name: SharedString::from(e.display_name.to_string()),
                    })
            });
        }
        let idx = self.device.as_ref().map(|d| d.idx);
        self.vendor = idx.map_or(GpuVendor::Unknown, |i| GpuVendor::of(snap, i));
        let totals = idx.and_then(|i| gpu_totals(snap, i));
        let metric = &self.selector.metric;
        let value = totals.map_or(Metric::default(), |t| reading(metric, t));
        let availability = match (self.meta, idx) {
            (None, _) => Availability::Unknown,
            (_, None) => Availability::Gone,
            _ if value.validity == Validity::Unsupported => Availability::NotReported,
            _ => Availability::Available,
        };
        let Some(meta) = self.meta else {
            self.reading.availability = availability;
            return;
        };
        let figures = if availability != Availability::Available {
            None
        } else if stats || self.reading.availability != Availability::Available {
            #[cfg(any(test, feature = "test-hooks"))]
            self.stats_count.set(self.stats_count.get() + 1);
            let range = current_range(cx);
            self.range = range;
            idx.zip(metric.metric())
                .and_then(|(device, metric)| meta.stats(snap, SeriesId { device, metric }, range))
        } else {
            self.reading.figures
        };
        self.reading = Reading {
            availability,
            value,
            bar_max: meta.bar_max(totals),
            y_scale: meta.y_scale(totals),
            offered: meta.offered_forms(totals),
            figures,
        };
    }

    /// The name, value and stats text from the last reading.
    fn format_all(&mut self, cx: &App) {
        self.name = match metric_name_key(&self.selector.metric, self.vendor) {
            Some(key) => tr(cx, key),
            None => SharedString::from(self.selector.metric.as_str().to_string()),
        };
        let r = self.reading;
        let Some(meta) = self
            .meta
            .filter(|_| r.availability == Availability::Available)
        else {
            self.value_text = SharedString::default();
            self.stats_text = SharedString::default();
            return;
        };
        self.value_text = if shown(r.value.validity) {
            // `MetricMeta::write_value` with the denominator this reading took from the record.
            let mut out = String::new();
            let net = NetFormat::default();
            format_value(meta.format, r.value.value, net, &mut out);
            if let (ValueText::OfTotal, Some(total)) = (meta.value_text, r.bar_max) {
                out.push_str(" / ");
                format_value(meta.format, total, net, &mut out);
            }
            out.into()
        } else {
            copy_text(r.value.validity, &SharedString::default())
        };
        let format = meta.format;
        self.stats_text = figures_text(&self.figures_words, r.figures, |v, out| {
            format_value(format, v, NetFormat::default(), out);
        });
    }
}

/// The sources of one Overview page, by selector.
pub struct SourceStore {
    hub: Entity<SnapshotHub>,
    /// The device keys of the GPUs the box shows, in sidebar order: slot 0, then slot 1.
    slots: Vec<Arc<str>>,
    sources: FxHashMap<SourceSelector, WeakEntity<SourceData>>,
}

impl SourceStore {
    pub fn new(hub: &Entity<SnapshotHub>) -> Self {
        Self {
            hub: hub.clone(),
            slots: Vec::new(),
            sources: FxHashMap::default(),
        }
    }

    /// The source of `selector`: the one in use, or a new one. Widgets on the same selector get
    /// the same entity, so they share its value and stats.
    pub fn source(&mut self, selector: &SourceSelector, cx: &mut App) -> Entity<SourceData> {
        if let Some(data) = self.sources.get(selector).and_then(WeakEntity::upgrade) {
            return data;
        }
        self.prune();
        let slot_key = self.slot_key(&selector.device);
        let hub = self.hub.clone();
        let selector = selector.clone();
        let data = cx.new(|cx| SourceData::new(&hub, selector.clone(), slot_key, cx));
        self.sources.insert(selector, data.downgrade());
        data
    }

    /// The device keys of the GPUs the box shows, slot 0 first. Slot sources follow.
    pub fn set_slots(&mut self, keys: &[Arc<str>], cx: &mut App) {
        if self.slots == keys {
            return;
        }
        self.slots = keys.to_vec();
        self.prune();
        for (selector, data) in &self.sources {
            let Some(data) = data.upgrade() else {
                continue;
            };
            let key = self.slot_key(&selector.device);
            data.update(cx, |d, cx| d.set_slot_key(key, cx));
        }
    }

    /// The number of sources in use.
    pub fn len(&self) -> usize {
        self.sources
            .values()
            .filter(|w| w.upgrade().is_some())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn slot_key(&self, device: &DeviceSelector) -> Option<Arc<str>> {
        match device {
            DeviceSelector::Slot(n) => self.slots.get(usize::from(*n)).cloned(),
            DeviceSelector::Key(_) => None,
        }
    }

    /// Forgets the sources no widget uses any more.
    fn prune(&mut self) {
        self.sources.retain(|_, w| w.upgrade().is_some());
    }
}
