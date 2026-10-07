//! The Network page: one page per interface with the Throughput chart above the Details grid,
//! its scale (auto or link speed), the disconnected state and the page's demand
//! (docs/ui/performance.md UI-PRF-031, UI-PRF-020; PERF-018, PERF-054, PERF-055).
//!
//! The chart is a cached view that re-renders for its own series. The page re-renders only when
//! its header changes. The spec is built again only when the link speed, a chart setting or the
//! theme changed.
//!
//! A disconnected interface needs nothing from the page: the engine marks its samples invalid,
//! so the chart draws a gap after the last valid sample (E19-003).
//!
//! The page's words are `en-US.ftl` keys, translated once per strings version into [`Words`].

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId, FocusHandle,
    Focusable, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StyleRefinement, Styled as _, Subscription, Window, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::settings::NetworkUnit;
use tl_model::snapshot::Snapshot;
use tl_model::validity::Validity;

use super::details::KeyValueModel;
use super::page::{DevicePage, DevicePageFrame, GRAPH_H_SINGLE, header_text, page_words};
use super::{DeviceKey, DeviceKind, DeviceRow};
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartState, ChartView, GridStyle, LegendStyle,
    ValueAlign, ValueFormat, YScale,
};
use crate::components::menu::TlMenu;
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::settings_global::{Section, TlSettings};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Debug selector of the chart box.
pub const THROUGHPUT_ID: &str = "perf-net-throughput";

const KEY_THROUGHPUT: &str = "perf-net-heading-throughput";
const KEY_DOWNLOAD: &str = "perf-net-legend-download";
const KEY_UPLOAD: &str = "perf-net-legend-upload";

const NS_PER_S: u64 = 1_000_000_000;
/// Alpha of the upload line against `net_line`.
const UPLOAD_ALPHA: f32 = 0.7;
/// Bits per second in one Mb/s, the unit of the sysfs `speed`.
const BPS_PER_MBPS: f32 = 1_000_000.;

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
pub struct Words {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    pub throughput: SharedString,
    pub download: SharedString,
    pub upload: SharedString,
}

impl Words {
    /// Translates every word when the strings changed since the last call. True when it did.
    fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(cx.try_global::<Strings>().map(|s| s.strings_version));
        if self.version == version {
            return false;
        }
        self.version = version;
        let t = |key| tr(cx, key);
        self.throughput = t(KEY_THROUGHPUT);
        self.download = t(KEY_DOWNLOAD);
        self.upload = t(KEY_UPLOAD);
        true
    }
}

/// The settings the page draws from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageSettings {
    range_ns: u64,
    smooth: bool,
    rounded: bool,
    /// `units.network`.
    unit: NetworkUnit,
    /// `units.network_auto_scale`.
    auto_scale: bool,
    /// `performance.network_dynamic_scale`.
    dynamic_scale: bool,
}

impl PageSettings {
    fn read(cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        Self {
            range_ns: u64::from(s.performance.graph_range_s) * NS_PER_S,
            smooth: s.motion.smooth_graphs,
            rounded: s.appearance.rounded_corners,
            unit: s.units.network,
            auto_scale: s.units.network_auto_scale,
            dynamic_scale: s.performance.network_dynamic_scale,
        }
    }
}

/// The versions of the sections [`PageSettings`] reads.
fn section_versions(cx: &App) -> [u64; 4] {
    let tl = TlSettings::get(cx);
    [
        tl.version(Section::Performance),
        tl.version(Section::Motion),
        tl.version(Section::Appearance),
        tl.version(Section::Units),
    ]
}

/// The Throughput chart's Y scale (UI-PRF-031): the link speed when the scale follows it and the
/// speed is known, otherwise auto, with powers of two under bytes (UI-PRF-001 fallback).
pub fn throughput_scale(
    dynamic_scale: bool,
    link_speed_bps: Option<f32>,
    unit: NetworkUnit,
) -> YScale {
    match link_speed_bps {
        Some(speed) if !dynamic_scale => YScale::LinkSpeed(speed),
        _ => YScale::AutoRate {
            iec: unit == NetworkUnit::Bytes,
        },
    }
}

/// The interface's sysfs link speed in bits per second, `None` when it is not Valid or not above
/// 0 (sysfs `speed` -1).
fn link_speed_bps(snap: &Snapshot, device: DeviceIdx) -> Option<f32> {
    snap.system
        .nets
        .iter()
        .find(|(i, _)| *i == device)
        .map(|(_, n)| n.link_speed_mbps)
        .filter(|m| m.validity == Validity::Valid && m.value > 0)
        .map(|m| m.value as f32 * BPS_PER_MBPS)
}

/// Download with fill, upload as a fainter dashed line.
fn throughput_spec(device: DeviceIdx, y: YScale, s: &PageSettings, theme: &TlTheme) -> ChartSpec {
    let line = theme.graph.net_line;
    let id = |metric| SeriesId { device, metric };
    ChartSpec {
        series: smallvec![
            ChartSeries {
                id: id(MetricId::NET_RX),
                fill: Some(theme.graph.net_fill),
                line,
                ..ChartSeries::default()
            },
            ChartSeries {
                id: id(MetricId::NET_TX),
                line: Hsla {
                    a: line.a * UPLOAD_ALPHA,
                    ..line
                },
                dashed: true,
                ..ChartSeries::default()
            },
        ],
        y,
        range_ns: s.range_ns,
        grid: GridStyle {
            lines: true,
            baseline: false,
            outline: true,
            rounded: s.rounded,
        },
        smooth: s.smooth,
        background: ChartBg::Window,
        ..ChartSpec::default()
    }
}

fn throughput_labels(w: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(w.throughput.clone()),
        value_fmt: ValueFormat::RateNetwork,
        value_align: ValueAlign::Right,
        range_word: w.throughput.clone(),
        legend: smallvec![
            (w.download.clone(), LegendStyle::Solid),
            (w.upload.clone(), LegendStyle::Dashed)
        ],
        show_axis_max: true,
    }
}

/// The chart state from the network source health: Failed with its error, else Live (the view
/// derives WarmingUp and Stale itself).
fn health_state(snap: &Snapshot) -> ChartState {
    let h = snap.health.get(SourceId::Net);
    match h.state {
        SourceState::Failed => ChartState::Failed(
            h.last_error
                .as_deref()
                .unwrap_or_default()
                .to_owned()
                .into(),
        ),
        _ => ChartState::Live,
    }
}

/// One network interface's device page.
pub struct NetPage {
    key: DeviceKey,
    row: DeviceRow,
    throughput: Entity<ChartView>,
    /// E25-007 and E25-020 fill the rows.
    details: KeyValueModel,
    /// `unit`, `auto_scale` and `dynamic_scale` with the chart settings.
    settings: PageSettings,
    settings_versions: [u64; 4],
    /// Valid sysfs speed in bits per second.
    link_speed_bps: Option<f32>,
    // E25-021: public_ip: PublicIpRowState (added by o55.22 as a chain edit, R-262)
    header: (SharedString, SharedString),
    /// `UiMetrics.style_version` the chart colours were built for.
    style_version: u64,
    words: Words,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The page is the shown page (`set_active`).
    active: bool,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    focus: FocusHandle,
    _subs: SmallVec<[Subscription; 4]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl NetPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = current(cx).clone();
        let settings = PageSettings::read(cx);
        let snap = Arc::clone(hub.read(cx).current());
        let link_speed = link_speed_bps(&snap, row.idx);
        let mut words = Words::default();
        words.refresh(cx);

        let throughput = {
            let y = throughput_scale(settings.dynamic_scale, link_speed, settings.unit);
            let (spec, labels) = (
                throughput_spec(row.idx, y, &settings, &theme),
                throughput_labels(&words),
            );
            cx.new(|cx| {
                let mut view = ChartView::new(spec, labels, &hub, cx);
                // The value is download plus upload.
                view.set_value_sum(true, cx);
                view
            })
        };

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<UiMetrics>(Self::sync_style));
        subs.push(cx.observe_global::<Strings>(Self::sync_words));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            key,
            row,
            throughput,
            details: KeyValueModel::default(),
            settings,
            settings_versions: section_versions(cx),
            link_speed_bps: link_speed,
            header: (SharedString::default(), SharedString::default()),
            style_version: metrics(cx).style_version,
            words,
            hub,
            demand,
            id,
            active: false,
            window: None,
            focus: cx.focus_handle(),
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
    }

    pub fn throughput(&self) -> &Entity<ChartView> {
        &self.throughput
    }

    pub fn unit(&self) -> NetworkUnit {
        self.settings.unit
    }

    pub fn auto_scale(&self) -> bool {
        self.settings.auto_scale
    }

    pub fn dynamic_scale(&self) -> bool {
        self.settings.dynamic_scale
    }

    pub fn link_speed_bps(&self) -> Option<f32> {
        self.link_speed_bps
    }

    pub fn words(&self) -> &Words {
        &self.words
    }

    /// This page's demand part while it is active: network only.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            nets: true,
            ..Demand::default()
        }
    }

    fn send_demand(&self, cx: &mut App) {
        if !self.active {
            return;
        }
        let (id, window, part) = (self.id, self.window, self.demand_part());
        self.demand.update(cx, |d, cx| d.set(id, window, part, cx));
    }

    fn on_snapshot(
        &mut self,
        hub: Entity<SnapshotHub>,
        ev: &SnapshotChanged,
        cx: &mut Context<Self>,
    ) {
        let snap = Arc::clone(hub.read(cx).current());
        self.apply_snapshot(&snap, ev.mask, cx);
    }

    /// Brings the link speed, chart state and header up to date with the components in `mask`.
    /// Notifies only when the header changed.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        if mask.contains(ComponentMask::SYSTEM) {
            let speed = link_speed_bps(snap, self.row.idx);
            if speed != self.link_speed_bps {
                self.link_speed_bps = speed;
                self.rebuild_spec(cx);
            }
        }
        if mask.contains(ComponentMask::HEALTH) {
            let state = health_state(snap);
            self.throughput.update(cx, |v, cx| v.set_state(state, cx));
        }
        if mask.intersects(
            ComponentMask::DEVICES | ComponentMask::INVENTORY | ComponentMask::DEVICE_DETAILS,
        ) && let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx)
        {
            let header = header_text(page_words(cx), DeviceKind::Net, 0, entry, snap);
            if header != self.header {
                self.header = header;
                cx.notify();
            }
        }
    }

    /// Hands the chart its spec from the current settings, link speed and theme. The view keeps
    /// an unchanged spec.
    fn rebuild_spec(&self, cx: &mut App) {
        let s = self.settings;
        let theme = current(cx).clone();
        let y = throughput_scale(s.dynamic_scale, self.link_speed_bps, s.unit);
        let spec = throughput_spec(self.row.idx, y, &s, &theme);
        self.throughput.update(cx, |v, cx| v.set_spec(spec, cx));
    }

    /// Applies the page's settings when one of their sections changed. The value text follows
    /// `units.network` in the chart itself.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = PageSettings::read(cx);
        if new == self.settings {
            return;
        }
        self.settings = new;
        self.rebuild_spec(cx);
    }

    /// A new theme: the chart takes the new colours.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_spec(cx);
        cx.notify();
    }

    /// New strings: the chart labels and the header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        if !self.words.refresh(cx) {
            return;
        }
        let labels = throughput_labels(&self.words);
        self.throughput.update(cx, |v, cx| v.set_labels(labels, cx));
        let snap = Arc::clone(self.hub.read(cx).current());
        self.apply_snapshot(&snap, ComponentMask::DEVICES, cx);
        cx.notify();
    }
}

impl DevicePage for NetPage {
    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, _: &mut Window, cx: &mut App) -> AnyElement {
        let h = GRAPH_H_SINGLE * metrics(cx).ui_scale;
        div()
            .debug_selector(|| THROUGHPUT_ID.into())
            .flex_none()
            .w_full()
            .h(px(h))
            .child(
                self.throughput
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    /// None in P1; E25-031 adds "Network settings" in P3.
    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        menu
    }

    /// E25-029 replaces this with the full Network copy text.
    fn copy_text(&self, out: &mut String) {
        out.push_str(&self.header.0);
        out.push('\n');
        self.details.copy_into(out);
    }

    fn focus_order(&self) -> SmallVec<[FocusHandle; 4]> {
        SmallVec::new()
    }

    fn set_active(&mut self, active: bool, cx: &mut App) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.send_demand(cx);
        } else {
            let id = self.id;
            self.demand.update(cx, |d, cx| d.remove(id, cx));
        }
    }
}

impl Focusable for NetPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for NetPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // The demand part carries the window, so it drops out while the window is hidden.
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            self.send_demand(cx);
        }
        self.details.refresh_strings(cx);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        div().size_full().track_focus(&self.focus).child(
            DevicePageFrame {
                page: self,
                m: &m,
                theme: &theme,
            }
            .render(window, cx),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_speed_scale_unknown_falls_back() {
        assert_eq!(
            throughput_scale(false, Some(1000. * BPS_PER_MBPS), NetworkUnit::Bits),
            YScale::LinkSpeed(1_000_000_000.)
        );
        assert_eq!(
            throughput_scale(false, None, NetworkUnit::Bits),
            YScale::AutoRate { iec: false }
        );
        assert_eq!(
            throughput_scale(false, None, NetworkUnit::Bytes),
            YScale::AutoRate { iec: true }
        );
        assert_eq!(
            throughput_scale(true, Some(1000. * BPS_PER_MBPS), NetworkUnit::Bits),
            YScale::AutoRate { iec: false },
            "the dynamic scale ignores the link speed"
        );
    }
}
