//! The Overview page's CPU box (UI-PRF-017, D-022, UI-PRF-023): the total chart and one bar per
//! logical CPU.
//!
//! The box is its own view. It formats its text only when the inputs' hash changes and notifies
//! only when the text or a core's 1% bucket changed, so an idle tick renders nothing. The chart is
//! a cached child view that re-renders for its own series. Pointer movement never notifies.

use std::cell::{Cell, Ref, RefCell};
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, StyleRefinement, Styled as _, Subscription, Window, div, px,
};
use rustc_hash::FxHasher;
use smallvec::{SmallVec, smallvec};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;

use super::super::cpu::logical_devices;
use super::parts::{
    ChartSettings, CoreBars, CoreBarsModel, SPLIT_W, ScaleLabel, ScaleLabelCache, box_frame,
    chart_section_versions, is_wide, split_body, width_probe,
};
use crate::chart::view::ChangedRuns;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, ValueAlign, ValueFormat,
    YScale, format_value,
};
use crate::components::validity::copy_text;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::settings_global::TlSettings;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// The box's debug selector.
pub const CPU_BOX_ID: &str = "perf-overview-box-cpu";

const KEY_TITLE: &str = "perf-card-heading-cpu";
const KEY_CORE: &str = "perf-overview-core";
const KEY_RANGE: &str = "perf-cpu-heading-utilization";

/// The joint between the clock and the temperature.
const SEP: &str = " · ";

/// The total chart: total with fill, kernel as a line when `kernel`, 0..100.
fn total_spec(cpu: DeviceIdx, s: &ChartSettings, theme: &TlTheme) -> ChartSpec {
    let mut series = smallvec![ChartSeries {
        id: SeriesId {
            device: cpu,
            metric: MetricId::CPU_TOTAL,
        },
        fill: Some(theme.graph.cpu_fill),
        line: theme.graph.cpu_line,
        ..ChartSeries::default()
    }];
    if s.kernel {
        series.push(ChartSeries {
            id: SeriesId {
                device: cpu,
                metric: MetricId::CPU_KERNEL,
            },
            fill: None,
            line: theme.graph.kernel_line,
            ..ChartSeries::default()
        });
    }
    ChartSpec {
        series,
        y: YScale::Fixed {
            min: 0.0,
            max: 100.0,
        },
        range_ns: s.range_ns,
        grid: GridStyle {
            lines: true,
            baseline: false,
            outline: true,
            rounded: s.rounded,
        },
        smooth: s.smooth,
        background: ChartBg::List,
        ..ChartSpec::default()
    }
}

/// No heading and no value: the box title row carries the numbers.
fn total_labels(range_word: SharedString) -> ChartLabels {
    ChartLabels {
        heading: None,
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Hidden,
        range_word,
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// The newest valid sample of `id`.
pub(super) fn newest(snap: &Snapshot, id: SeriesId) -> Option<f32> {
    snap.series
        .get(id)?
        .samples
        .iter()
        .rev()
        .find(|s| s.is_valid())
        .map(|s| s.value)
}

/// The text for a value of `validity`: `format(out)` for Valid and Stale, else the validity
/// glyph.
pub(super) fn value_text(validity: Validity, format: impl FnOnce(&mut String)) -> SharedString {
    match validity {
        Validity::Valid | Validity::Stale => {
            let mut out = String::new();
            format(&mut out);
            out.into()
        }
        v => copy_text(v, &SharedString::default()),
    }
}

/// The CPU box.
pub struct CpuBox {
    cpu: DeviceIdx,
    total: Entity<ChartView>,
    bars: Rc<RefCell<CoreBarsModel>>,
    /// Logical CPU id to device; `DeviceIdx(u16::MAX)` for an id with no catalog entry.
    logical: Vec<DeviceIdx>,
    /// `CPU_TOTAL` of each entry of `logical`.
    logical_ids: Vec<SeriesId>,
    catalog_version: u64,
    title: SharedString,
    value: SharedString,
    line: SharedString,
    /// Hash of the inputs `value` and `line` were formatted from.
    text_bits: u64,
    scale: Rc<RefCell<ScaleLabelCache>>,
    scale_text: SharedString,
    /// The box width of the last paint, and whether the last render put the bars beside the
    /// chart.
    width: Rc<Cell<Pixels>>,
    wide: bool,
    settings: ChartSettings,
    settings_versions: [u64; 3],
    style_version: u64,
    strings_version: u64,
    _subs: [Subscription; 4],
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl CpuBox {
    pub fn new(hub: &Entity<SnapshotHub>, cpu: DeviceIdx, cx: &mut Context<Self>) -> Self {
        let theme = current(cx).clone();
        let settings = ChartSettings::read(cx);
        let spec = total_spec(cpu, &settings, &theme);
        let labels = total_labels(tr(cx, KEY_RANGE));
        let total = cx.new(|cx| ChartView::new(spec, labels, hub, cx));
        let mut scale_text = String::new();
        format_value(
            ValueFormat::Percent,
            100.,
            NetworkUnit::default(),
            &mut scale_text,
        );
        let snap = Arc::clone(hub.read(cx).current());
        let mut this = Self {
            cpu,
            total,
            bars: Rc::default(),
            logical: Vec::new(),
            logical_ids: Vec::new(),
            catalog_version: u64::MAX,
            title: tr(cx, KEY_TITLE),
            value: SharedString::default(),
            line: SharedString::default(),
            text_bits: 0,
            scale: Rc::default(),
            scale_text: scale_text.into(),
            width: Rc::new(Cell::new(px(0.))),
            wide: true,
            settings,
            settings_versions: chart_section_versions(cx),
            style_version: metrics(cx).style_version,
            strings_version: cx.global::<Strings>().strings_version,
            _subs: [
                cx.subscribe(hub, Self::on_snapshot),
                cx.observe_global::<TlSettings>(Self::sync_settings),
                cx.observe_global::<UiMetrics>(Self::sync_style),
                cx.observe_global::<Strings>(Self::sync_strings),
            ],
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.sync_devices(&snap, cx);
        this.sync_text(&snap, true);
        this
    }

    pub fn total_chart(&self) -> &Entity<ChartView> {
        &self.total
    }

    pub fn bars(&self) -> Ref<'_, CoreBarsModel> {
        self.bars.borrow()
    }

    /// The title, value and line text.
    pub fn text(&self) -> (&SharedString, &SharedString, &SharedString) {
        (&self.title, &self.value, &self.line)
    }

    /// The scale label's cache.
    pub fn scale_label(&self) -> Ref<'_, ScaleLabelCache> {
        self.scale.borrow()
    }

    /// "CPU\t12%", the line when there is one, then "C0\t10%" per logical CPU.
    pub fn copy_text(&self, out: &mut String) {
        out.push_str(&self.title);
        out.push('\t');
        out.push_str(&self.value);
        out.push('\n');
        if !self.line.is_empty() {
            out.push_str(&self.line);
            out.push('\n');
        }
        self.bars.borrow().copy_text(out);
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let devices = ev.mask.contains(ComponentMask::DEVICES);
        let series = ev.mask.contains(ComponentMask::SERIES);
        let system = ev.mask.contains(ComponentMask::SYSTEM);
        if !(devices || series || system) {
            return;
        }
        let snap = Arc::clone(hub.read(cx).current());
        let mut changed = false;
        if devices {
            changed |= self.sync_devices(&snap, cx);
        }
        if series && !changed {
            let runs = ChangedRuns::new(&ev.changed_series);
            if self.logical_ids.iter().any(|id| runs.contains(*id)) {
                changed |= self.sync_bars(&snap);
            }
        }
        if system {
            changed |= self.sync_text(&snap, false);
        }
        if changed {
            #[cfg(any(test, feature = "test-hooks"))]
            self.notify_count.set(self.notify_count.get() + 1);
            cx.notify();
        }
    }

    /// Rebuilds the logical CPU list and the bar names when the catalog changed. True when it
    /// did.
    fn sync_devices(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        if snap.devices.version == self.catalog_version {
            return false;
        }
        self.catalog_version = snap.devices.version;
        logical_devices(&snap.devices, &mut self.logical);
        self.logical_ids.clear();
        self.logical_ids
            .extend(self.logical.iter().map(|&device| SeriesId {
                device,
                metric: MetricId::CPU_TOTAL,
            }));
        self.sync_names(cx);
        self.sync_bars(snap);
        true
    }

    /// "C0", "C1", … for the logical CPUs.
    fn sync_names(&mut self, cx: &mut Context<Self>) {
        let names = (0..self.logical.len())
            .map(|n| tr_args(cx, KEY_CORE, &[("n", FluentValue::from(n))]))
            .collect::<Vec<_>>();
        let mut bars = self.bars.borrow_mut();
        bars.set_cpus(names.into_iter());
    }

    /// The newest valid `CPU_TOTAL` of each logical CPU to the bars. True when a bar's bucket
    /// changed.
    fn sync_bars(&mut self, snap: &Snapshot) -> bool {
        let ids = &self.logical_ids;
        self.bars
            .borrow_mut()
            .set_values(ids.iter().map(|id| newest(snap, *id)))
    }

    /// The value (utilization) and line (clock, temperature) when their inputs changed, or
    /// always with `force`. True when the text was formatted again.
    fn sync_text(&mut self, snap: &Snapshot, force: bool) -> bool {
        let cpu = &snap.system.cpu;
        let mut h = FxHasher::default();
        h.write_u32(cpu.util_pct.value.to_bits());
        cpu.util_pct.validity.hash(&mut h);
        h.write_u32(cpu.freq_mhz.value);
        cpu.freq_mhz.validity.hash(&mut h);
        h.write_u32(cpu.temp_c.value.to_bits());
        cpu.temp_c.validity.hash(&mut h);
        let bits = h.finish();
        if !force && bits == self.text_bits {
            return false;
        }
        self.text_bits = bits;
        let unit = NetworkUnit::default();
        self.value = value_text(cpu.util_pct.validity, |out| {
            format_value(ValueFormat::Percent, cpu.util_pct.value, unit, out);
        });
        let shown = |v: Validity| matches!(v, Validity::Valid | Validity::Stale);
        let mut line = String::new();
        if shown(cpu.freq_mhz.validity) {
            format_value(ValueFormat::Mhz, cpu.freq_mhz.value as f32, unit, &mut line);
        }
        if shown(cpu.temp_c.validity) {
            if !line.is_empty() {
                line.push_str(SEP);
            }
            format_value(ValueFormat::Celsius, cpu.temp_c.value, unit, &mut line);
        }
        self.line = line.into();
        true
    }

    /// Applies the chart settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
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
        self.rebuild_spec(cx);
    }

    /// A new theme or UI scale: new chart colours, and the frame draws in the new style.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_spec(cx);
        cx.notify();
    }

    /// A new language: new title, range word and core names.
    fn sync_strings(&mut self, cx: &mut Context<Self>) {
        let version = cx.global::<Strings>().strings_version;
        if version == self.strings_version {
            return;
        }
        self.strings_version = version;
        self.title = tr(cx, KEY_TITLE);
        let labels = total_labels(tr(cx, KEY_RANGE));
        self.total.update(cx, |v, cx| v.set_labels(labels, cx));
        let values = self.bars.borrow().values.clone();
        self.sync_names(cx);
        self.bars.borrow_mut().set_values(values.into_iter());
        cx.notify();
    }

    fn rebuild_spec(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let spec = total_spec(self.cpu, &self.settings, &theme);
        self.total.update(cx, |v, cx| v.set_spec(spec, cx));
    }
}

impl Render for CpuBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        self.wide = is_wide(self.width.get(), m.ui_scale);
        let chart: AnyElement = div()
            .relative()
            .size_full()
            .child(
                self.total
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .child(ScaleLabel {
                text: self.scale_text.clone(),
                cache: Rc::clone(&self.scale),
            })
            .into_any_element();
        let bars = CoreBars {
            model: Rc::clone(&self.bars),
        }
        .into_any_element();
        let body = split_body(chart, bars, self.wide, &m).into_any_element();
        box_frame(
            CPU_BOX_ID,
            self.title.clone(),
            self.value.clone(),
            self.line.clone(),
            body,
            &m,
            &theme,
        )
        .relative()
        .size_full()
        .child(width_probe(
            Rc::clone(&self.width),
            self.wide,
            px(SPLIT_W * m.ui_scale),
            cx.entity_id(),
        ))
    }
}
