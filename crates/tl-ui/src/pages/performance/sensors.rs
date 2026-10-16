//! The Fan and Battery pages. The Fan page: the Fan speed chart with the PWM series on its own 0..100 axis, the Temperature
//! chart of the paired sensor, and the Details rows (docs/ui/performance.md UI-PRF-034; PERF-024,
//! PERF-076, PERF-077).
//!
//! The page re-renders only when its header, sensor choice, empty-sensor state or Details rows
//! change. Each chart is a cached view that re-renders for its own series.
//!
//! The PWM series is drawn on the RPM axis: the page keeps a derived `SeriesView` with every
//! value scaled by `axis_max / 100` and puts it in the chart's second slot after each snapshot
//! (E25-OI-5). The axis has no labels for it.

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::fmt::Write as _;
use std::sync::Arc;

use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId, FocusHandle,
    Focusable, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StyleRefinement, Styled as _, Subscription, Window, div, px,
};
use smallvec::{SmallVec, smallvec};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{BatteryDetails, InvField, Snapshot};
use tl_model::system::{FanControlMode, Metric, SensorKind, SensorReading};
use tl_model::time::now_boot_ns;
use tl_model::units;
use tl_model::validity::Validity;

use super::details::{Fallback, KeyValueModel};
use super::page::{DevicePage, DevicePageFrame, GRAPH_H_STACKED, header_text, page_words};
use super::{
    DeviceKey, DeviceKind, DeviceRow, PerformanceSettingsView, sidebar_order, type_number,
    visible_rows,
};
use crate::chart::view::find_tiered;
use crate::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GridStyle, LegendStyle, ValueAlign,
    ValueFormat, YScale,
};
use crate::components::menu::TlMenu;
use crate::demand::DemandAggregator;
use crate::hub::{ComponentMask, SnapshotChanged, SnapshotHub};
use crate::i18n::tr;
use crate::settings_global::{Section, TlSettings};
use crate::text::style::label_style;
use crate::theme::palette::accent_color;
use crate::theme::{TlTheme, current, metrics};

/// Debug selector of the Fan speed chart box.
pub const SPEED_ID: &str = "perf-fan-speed";
/// Debug selector of the Temperature chart box.
pub const TEMPERATURE_ID: &str = "perf-fan-temperature";
/// Debug selector of the Battery chart box.
pub const BATTERY_CHART_ID: &str = "perf-battery-chart";

/// `<hwmon name>:<device path>:temp<n>`, the persistent key of a temperature sensor without its
/// `sensor:` prefix (table (a) `[fans]`).
pub type SensorRef = Arc<str>;

const NS_PER_S: u64 = 1_000_000_000;
/// Fill alpha of a type that has one colour (UI-PRF-005 table note).
const FILL_ALPHA: f32 = 0.85;
/// The fan colour name (UI-PRF-005: "Violet").
const FAN_COLOR: &str = "violet";

const KEY_SPEED: &str = "perf-fan-heading-speed";
const KEY_TEMPERATURE: &str = "perf-fan-heading-temperature";
const KEY_RPM: &str = "perf-fan-legend-rpm";
const KEY_PWM: &str = "perf-fan-legend-pwm";
const KEY_CHOOSE: &str = "perf-fan-choose-sensor";

pub const ROW_SPEED: &str = "perf-fan-row-speed";
pub const ROW_PWM: &str = "perf-fan-row-pwm";
pub const ROW_MODE: &str = "perf-fan-row-mode";
pub const ROW_TEMPERATURE: &str = "perf-fan-row-temperature";
pub const ROW_CHIP: &str = "perf-fan-row-chip";

/// The five Details rows in grid order (UI-PRF-034).
pub const FAN_DETAIL_ROWS: [&str; 5] = [ROW_SPEED, ROW_PWM, ROW_MODE, ROW_TEMPERATURE, ROW_CHIP];

/// The battery colour name (UI-PRF-005: "Mint").
const BAT_COLOR: &str = "mint";

const BAT_KEY_POWER: &str = "perf-bat-heading-power";
const BAT_KEY_PERCENTAGE: &str = "perf-bat-heading-percentage";
const BAT_KEY_RANGE_POWER: &str = "perf-bat-range-power";

pub const BAT_ROW_PERCENTAGE: &str = "perf-bat-row-percentage";
pub const BAT_ROW_ENERGY: &str = "perf-bat-row-energy";
pub const BAT_ROW_VOLTAGE: &str = "perf-bat-row-voltage";
pub const BAT_ROW_POWER: &str = "perf-bat-row-power";
pub const BAT_ROW_STATE: &str = "perf-bat-row-state";
pub const BAT_ROW_TIME_EMPTY: &str = "perf-bat-row-time-to-empty";
pub const BAT_ROW_TIME_FULL: &str = "perf-bat-row-time-to-full";
pub const BAT_ROW_CYCLES: &str = "perf-bat-row-cycles";
pub const BAT_ROW_SERIAL: &str = "perf-bat-row-serial";
pub const BAT_ROW_TYPE: &str = "perf-bat-row-type";
pub const BAT_ROW_POWERING: &str = "perf-bat-row-powering";
pub const BAT_ROW_TECHNOLOGY: &str = "perf-bat-row-technology";
pub const BAT_ROW_CAPACITY: &str = "perf-bat-row-capacity";
pub const BAT_ROW_ENERGY_EMPTY: &str = "perf-bat-row-energy-empty";
pub const BAT_ROW_ENERGY_FULL: &str = "perf-bat-row-energy-full";
pub const BAT_ROW_ENERGY_DESIGN: &str = "perf-bat-row-energy-full-design";
pub const BAT_ROW_VOLTAGE_MIN: &str = "perf-bat-row-voltage-min";
pub const BAT_ROW_VOLTAGE_MAX: &str = "perf-bat-row-voltage-max";
pub const BAT_ROW_THRESHOLD: &str = "perf-bat-row-threshold";
pub const BAT_ROW_THRESHOLD_START: &str = "perf-bat-row-threshold-start";
pub const BAT_ROW_THRESHOLD_END: &str = "perf-bat-row-threshold-end";
pub const BAT_ROW_TEMPERATURE: &str = "perf-bat-row-temperature";

/// The Battery Details rows in grid order (PERF-078). The two time rows share one slot: the
/// page shows the one the state asks for and hides the other.
pub const BATTERY_DETAIL_ROWS: [&str; 22] = [
    BAT_ROW_PERCENTAGE,
    BAT_ROW_ENERGY,
    BAT_ROW_VOLTAGE,
    BAT_ROW_POWER,
    BAT_ROW_STATE,
    BAT_ROW_TIME_EMPTY,
    BAT_ROW_TIME_FULL,
    BAT_ROW_CYCLES,
    BAT_ROW_SERIAL,
    BAT_ROW_TYPE,
    BAT_ROW_POWERING,
    BAT_ROW_TECHNOLOGY,
    BAT_ROW_CAPACITY,
    BAT_ROW_ENERGY_EMPTY,
    BAT_ROW_ENERGY_FULL,
    BAT_ROW_ENERGY_DESIGN,
    BAT_ROW_VOLTAGE_MIN,
    BAT_ROW_VOLTAGE_MAX,
    BAT_ROW_THRESHOLD,
    BAT_ROW_THRESHOLD_START,
    BAT_ROW_THRESHOLD_END,
    BAT_ROW_TEMPERATURE,
];

const BAT_KEY_WH: &str = "perf-bat-wh-value";
const BAT_KEY_V: &str = "perf-bat-v-value";
const BAT_KEY_YES: &str = "perf-bat-yes";
const BAT_KEY_NO: &str = "perf-bat-no";
const BAT_KEY_ENABLED: &str = "perf-bat-enabled";
const BAT_KEY_DISABLED: &str = "perf-bat-disabled";

/// The engine's state names and the Fluent key of each word.
const BAT_STATES: [(&str, &str); 7] = [
    ("Charging", "perf-bat-state-charging"),
    ("Discharging", "perf-bat-state-discharging"),
    ("Empty", "perf-bat-state-empty"),
    ("Fully charged", "perf-bat-state-full"),
    ("Pending charge", "perf-bat-state-pending-charge"),
    ("Pending discharge", "perf-bat-state-pending-discharge"),
    ("Unknown", "perf-bat-state-unknown"),
];

/// The value the Temperature chart shows without a sensor.
const NO_VALUE: &str = "-";

/// The fan's chip and path (`<hwmon name>:<path>`) and index from its persistent key
/// `fan:<chip>:<path>:<n>`.
fn fan_key_parts(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix("fan:")?.rsplit_once(':')
}

/// The default sensor of the fan with persistent key `fan_key` (PERF-077): the temperature
/// `temp<n>` of the same chip and path, only when the chip has exactly one fan.
pub fn default_sensor(snap: &Snapshot, fan_key: &str) -> Option<SensorRef> {
    let (chip_path, n) = fan_key_parts(fan_key)?;
    let fans = snap
        .devices
        .entries
        .iter()
        .filter(|e| matches!(e.identity, DeviceIdentity::Fan { .. }))
        .filter(|e| fan_key_parts(&e.persistent_key).is_some_and(|(c, _)| c == chip_path))
        .count();
    if fans != 1 {
        return None;
    }
    let sensor = format!("{chip_path}:temp{n}");
    sensor_device(snap, &sensor).map(|_| SensorRef::from(sensor))
}

/// The catalog device of `sensor` (a [`SensorRef`]).
pub fn sensor_device(snap: &Snapshot, sensor: &str) -> Option<DeviceIdx> {
    snap.devices
        .entries
        .iter()
        .find(|e| {
            e.persistent_key
                .strip_prefix("sensor:")
                .is_some_and(|k| k == sensor)
        })
        .map(|e| e.idx)
}

/// The sensor the page shows: the saved choice of the fan when set, else the default pairing.
/// A saved choice that is not in the catalog gives `None`.
fn choose_sensor(snap: &Snapshot, fan_key: &str, saved: Option<&str>) -> Option<SensorRef> {
    match saved {
        Some(s) => sensor_device(snap, s).map(|_| SensorRef::from(s)),
        None => default_sensor(snap, fan_key),
    }
}

/// `raw` (a PWM percent series) scaled into an axis of `axis_max`: every value becomes
/// `pct / 100 × axis_max`. The version mixes in the axis, so a chart cache keyed on the version
/// sees a new axis.
pub fn pwm_scaled(raw: &SeriesView, axis_max: f32) -> SeriesView {
    let samples: Vec<Sample> = raw
        .samples
        .iter()
        .map(|s| Sample {
            value: s.value / 100.0 * axis_max,
            ..*s
        })
        .collect();
    SeriesView {
        id: raw.id,
        version: raw
            .version
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add(u64::from(axis_max.to_bits())),
        period_ns: raw.period_ns,
        samples: samples.into(),
    }
}

/// The page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
struct Words {
    version: Option<Option<u64>>,
    speed: SharedString,
    temperature: SharedString,
    rpm: SharedString,
    pwm: SharedString,
    choose: SharedString,
}

impl Words {
    /// Translates every word when the strings changed since the last call. True when it did.
    fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(
            cx.try_global::<crate::i18n::Strings>()
                .map(|s| s.strings_version),
        );
        if self.version == version {
            return false;
        }
        self.version = version;
        self.speed = tr(cx, KEY_SPEED);
        self.temperature = tr(cx, KEY_TEMPERATURE);
        self.rpm = tr(cx, KEY_RPM);
        self.pwm = tr(cx, KEY_PWM);
        self.choose = tr(cx, KEY_CHOOSE);
        true
    }
}

/// The settings the page draws from.
#[derive(Clone, Debug, PartialEq)]
struct PageSettings {
    range_ns: u64,
    smooth: bool,
    rounded: bool,
    /// `[fans]` entry of this fan.
    label: Option<String>,
    temp: Option<String>,
}

impl PageSettings {
    fn read(key: &DeviceKey, cx: &App) -> Self {
        let s = TlSettings::settings(cx);
        let fan = s.fans.get(&*key.0);
        Self {
            range_ns: u64::from(s.performance.graph_range_s) * NS_PER_S,
            smooth: s.motion.smooth_graphs,
            rounded: s.appearance.rounded_corners,
            label: fan
                .and_then(|f| f.label.clone())
                .filter(|l| !l.trim().is_empty()),
            temp: fan.and_then(|f| f.temp.clone()),
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
        tl.version(Section::Fans),
    ]
}

fn plot_grid(s: &PageSettings) -> GridStyle {
    GridStyle {
        lines: true,
        baseline: false,
        outline: true,
        rounded: s.rounded,
    }
}

fn series(device: DeviceIdx, metric: MetricId) -> SeriesId {
    SeriesId { device, metric }
}

fn fan_line(theme: &TlTheme) -> gpui_kit::Hsla {
    accent_color(FAN_COLOR).unwrap_or(theme.graph.gpu_line)
}

/// The Fan speed chart: RPM filled, PWM dashed (UI-PRF-034). `fan_max` is the hardware maximum
/// in RPM when the chip reports it.
pub fn speed_spec(
    device: DeviceIdx,
    fan_max: Option<u32>,
    s: &PageSettingsView,
    theme: &TlTheme,
) -> ChartSpec {
    let line = fan_line(theme);
    ChartSpec {
        series: smallvec![
            ChartSeries {
                id: series(device, MetricId::FAN_RPM),
                fill: Some(line.opacity(FILL_ALPHA)),
                line,
                ..ChartSeries::default()
            },
            ChartSeries {
                id: series(device, MetricId::FAN_PWM),
                fill: None,
                line,
                dashed: true,
                ..ChartSeries::default()
            },
        ],
        y: YScale::StickyUp {
            hw_max: fan_max.filter(|&m| m > 0).map(|m| m as f32),
        },
        range_ns: s.range_ns,
        smooth: s.smooth,
        grid: s.grid.clone(),
        background: ChartBg::Window,
        ..ChartSpec::default()
    }
}

/// The chart settings of one spec, copied out of [`PageSettings`].
pub struct PageSettingsView {
    range_ns: u64,
    smooth: bool,
    grid: GridStyle,
}

impl PageSettingsView {
    fn of(s: &PageSettings) -> Self {
        Self {
            range_ns: s.range_ns,
            smooth: s.smooth,
            grid: plot_grid(s),
        }
    }
}

fn speed_labels(words: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(words.speed.clone()),
        value_fmt: ValueFormat::Rpm,
        value_align: ValueAlign::Right,
        range_word: words.speed.clone(),
        legend: smallvec![
            (words.rpm.clone(), LegendStyle::Solid),
            (words.pwm.clone(), LegendStyle::Dashed),
        ],
        show_axis_max: true,
    }
}

/// The Temperature chart of `sensor`. Without a sensor the plot has no series.
fn temperature_spec(sensor: Option<DeviceIdx>, s: &PageSettingsView, theme: &TlTheme) -> ChartSpec {
    let line = fan_line(theme);
    ChartSpec {
        series: sensor
            .map(|device| ChartSeries {
                id: series(device, MetricId::SENSOR_TEMP),
                fill: Some(line.opacity(FILL_ALPHA)),
                line,
                ..ChartSeries::default()
            })
            .into_iter()
            .collect(),
        y: YScale::StickyTemp,
        range_ns: s.range_ns,
        smooth: s.smooth,
        grid: s.grid.clone(),
        background: ChartBg::Window,
        ..ChartSpec::default()
    }
}

/// The Temperature labels. Without a sensor the chart's own value is hidden: the page draws `-`.
fn temperature_labels(has_sensor: bool, words: &Words) -> ChartLabels {
    ChartLabels {
        heading: Some(words.temperature.clone()),
        value_fmt: ValueFormat::Celsius,
        value_align: if has_sensor {
            ValueAlign::Right
        } else {
            ValueAlign::Hidden
        },
        range_word: words.temperature.clone(),
        legend: SmallVec::new(),
        show_axis_max: has_sensor,
    }
}

/// The Fan Details model: the five rows in order.
pub fn fan_details_model() -> KeyValueModel {
    KeyValueModel::new(&FAN_DETAIL_ROWS)
}

/// The words of the Control mode row.
fn mode_key(mode: FanControlMode) -> &'static str {
    match mode {
        FanControlMode::Automatic => "perf-fan-mode-auto",
        FanControlMode::Manual => "perf-fan-mode-manual",
        FanControlMode::FullSpeed => "perf-fan-mode-full",
    }
}

/// One row from a metric: Valid formats on a bits change, Stale keeps the value and marks it,
/// the rest show a fallback word. A reading of 0 is a measured zero.
fn metric_row(
    model: &mut KeyValueModel,
    label: &'static str,
    m: Metric<f32>,
    age_ns: u64,
    write: impl FnOnce(&mut String, f32),
) -> bool {
    let bits = u64::from(m.value.to_bits());
    match m.validity {
        Validity::Valid => model.set_valid(label, bits, |o| write(o, m.value)),
        Validity::Stale => {
            model.set_valid(label, bits, |o| write(o, m.value)) | model.set_stale(label, age_ns)
        }
        Validity::Unsupported => {
            model.set_fallback(label, m.validity, Fallback::NotSupported, None)
        }
        Validity::Missing => {
            model.set_fallback(label, Validity::Unsupported, Fallback::Unavailable, None)
        }
        v => model.set_fallback(label, v, Fallback::Unavailable, None),
    }
}

/// The reading of `device` in the snapshot.
fn reading(snap: &Snapshot, device: DeviceIdx) -> Option<&SensorReading> {
    snap.system
        .sensors
        .iter()
        .find(|(i, _)| *i == device)
        .map(|(_, r)| r)
}

/// The fan's maximum RPM in the snapshot.
fn fan_max_of(snap: &Snapshot, device: DeviceIdx) -> Option<u32> {
    reading(snap, device)
        .filter(|r| r.kind == SensorKind::Fan)
        .and_then(|r| r.fan_max_rpm)
        .filter(|&m| m > 0)
}

/// What [`FanPage::update_details`] reads.
struct DetailInputs<'a> {
    now_boot_ns: u64,
    observed_end_ns: u64,
    fan: Option<&'a SensorReading>,
    temperature: Option<&'a SensorReading>,
    chip: Option<&'a str>,
}

/// Fills the Details rows. True when a row changed.
fn update_fan_details(model: &mut KeyValueModel, inp: &DetailInputs, cx: &App) -> bool {
    let age_ns = inp.now_boot_ns.saturating_sub(inp.observed_end_ns);
    let missing = || Metric::<f32>::with(Validity::Missing);
    let mut changed = false;
    changed |= metric_row(
        model,
        ROW_SPEED,
        inp.fan.map_or(missing(), |r| r.value),
        age_ns,
        |o, v| {
            let _ = write!(o, "{v:.0} RPM");
        },
    );
    changed |= metric_row(
        model,
        ROW_PWM,
        inp.fan.map_or(missing(), |r| r.pwm_pct),
        age_ns,
        |o, v| {
            let _ = write!(o, "{v:.0}%");
        },
    );
    changed |= match inp.fan.and_then(|r| r.fan_control) {
        Some(mode) => model.set_text(ROW_MODE, &tr(cx, mode_key(mode))),
        None => model.set_fallback(ROW_MODE, Validity::Unsupported, Fallback::NotReported, None),
    };
    changed |= match inp.temperature {
        Some(r) => metric_row(model, ROW_TEMPERATURE, r.value, age_ns, units::celsius),
        None => model.set_fallback(
            ROW_TEMPERATURE,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        ),
    };
    changed |= match inp.chip {
        Some(chip) => model.set_text(ROW_CHIP, chip),
        None => model.set_fallback(ROW_CHIP, Validity::Unsupported, Fallback::NotReported, None),
    };
    changed
}

/// The derived PWM series and what it was built from.
#[derive(Default)]
struct PwmDerived {
    raw: Option<Arc<SeriesView>>,
    axis_bits: u32,
    view: Option<Arc<SeriesView>>,
}

/// The Fan device page.
pub struct FanPage {
    key: DeviceKey,
    row: DeviceRow,
    speed: Entity<ChartView>,
    temperature: Entity<ChartView>,
    /// The temperature sensor of the Temperature chart; `None` shows "Choose a sensor".
    sensor: Option<SensorRef>,
    sensor_device: Option<DeviceIdx>,
    fan_max: Option<u32>,
    pwm: PwmDerived,
    details: KeyValueModel,
    focus: FocusHandle,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    /// The page is the shown page (`set_active`).
    active: bool,
    /// The window of the last render, for the demand part.
    window: Option<AnyWindowHandle>,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 4],
    /// `UiMetrics.style_version` the chart colours were built for.
    style_version: u64,
    words: Words,
    _subs: SmallVec<[Subscription; 3]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl FanPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = current(cx).clone();
        let settings = PageSettings::read(&key, cx);
        let snap = Arc::clone(hub.read(cx).current());
        let mut words = Words::default();
        words.refresh(cx);

        let sensor = choose_sensor(&snap, &key.0, settings.temp.as_deref());
        let sensor_device = sensor.as_deref().and_then(|s| sensor_device(&snap, s));
        let fan_max = fan_max_of(&snap, row.idx);
        let view = PageSettingsView::of(&settings);
        let spec = speed_spec(row.idx, fan_max, &view, &theme);
        let labels = speed_labels(&words);
        let speed = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));
        let spec = temperature_spec(sensor_device, &view, &theme);
        let labels = temperature_labels(sensor_device.is_some(), &words);
        let temperature = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<crate::theme::UiMetrics>(Self::sync_style));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            key,
            row,
            speed,
            temperature,
            sensor,
            sensor_device,
            fan_max,
            pwm: PwmDerived::default(),
            details: fan_details_model(),
            focus: cx.focus_handle(),
            hub,
            demand,
            id,
            active: false,
            window: None,
            header: (SharedString::default(), SharedString::default()),
            settings,
            settings_versions: section_versions(cx),
            style_version: metrics(cx).style_version,
            words,
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
    }

    pub fn speed(&self) -> &Entity<ChartView> {
        &self.speed
    }

    pub fn temperature(&self) -> &Entity<ChartView> {
        &self.temperature
    }

    /// The sensor of the Temperature chart; `None` while the page asks to choose one.
    pub fn sensor(&self) -> Option<&str> {
        self.sensor.as_deref()
    }

    /// The Fan speed axis max in RPM from the hardware, when the chip reports it.
    pub fn fan_max(&self) -> Option<u32> {
        self.fan_max
    }

    /// The derived PWM series on the RPM axis, as last put in the Fan speed chart.
    pub fn pwm_series(&self) -> Option<&Arc<SeriesView>> {
        self.pwm.view.as_ref()
    }

    /// The text of the Temperature chart's value when it has no sensor: `-`.
    pub fn temperature_value_text(&self) -> Option<&'static str> {
        self.sensor.is_none().then_some(NO_VALUE)
    }

    /// The centred text of the empty Temperature plot: "Choose a sensor".
    pub fn choose_sensor_text(&self) -> Option<&SharedString> {
        self.sensor.is_none().then_some(&self.words.choose)
    }

    /// This page's demand part while it is active: the sensors component.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            sensors: true,
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

    /// Brings the sensor choice, chart specs, derived PWM series, Details rows and header up to
    /// date with the components in `mask`. Notifies only when something the page draws changed.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let mut changed = false;
        let devices = mask.contains(ComponentMask::DEVICES);
        let system = mask.contains(ComponentMask::SYSTEM);

        if devices {
            changed |= self.sync_sensor(snap, cx);
        }
        if system || devices {
            let fan_max = fan_max_of(snap, self.row.idx);
            if fan_max != self.fan_max {
                self.fan_max = fan_max;
                self.rebuild_speed(cx);
            }
        }
        if system || devices || mask.contains(ComponentMask::SERIES) {
            self.sync_pwm(snap, cx);
        }
        if system || devices {
            let fan = reading(snap, self.row.idx);
            let temperature = self.sensor_device.and_then(|d| reading(snap, d));
            let chip = snap
                .devices
                .entries
                .iter()
                .find(|e| e.idx == self.row.idx)
                .and_then(|e| match &e.identity {
                    DeviceIdentity::Fan { chip, .. } => Some(&**chip),
                    _ => None,
                });
            let inputs = DetailInputs {
                now_boot_ns: now_boot_ns(),
                observed_end_ns: snap.system.header.observed_end,
                fan,
                temperature,
                chip,
            };
            changed |= update_fan_details(&mut self.details, &inputs, cx);
        }
        if devices {
            changed |= self.sync_header(snap, cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// Reads the sensor choice: the saved one, else the default pairing. Rebuilds the Temperature
    /// chart when it changed. True when it did.
    fn sync_sensor(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let sensor = choose_sensor(snap, &self.key.0, self.settings.temp.as_deref());
        let device = sensor.as_deref().and_then(|s| sensor_device(snap, s));
        if sensor == self.sensor && device == self.sensor_device {
            return false;
        }
        self.sensor = sensor;
        self.sensor_device = device;
        self.rebuild_temperature(cx);
        true
    }

    fn sync_header(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx) else {
            return false;
        };
        let number = fan_number(&self.key, snap, cx);
        let mut header = header_text(page_words(cx), DeviceKind::Fan, number, entry, snap);
        if let Some(label) = &self.settings.label {
            header.0 = SharedString::from(label.clone());
        }
        if header == self.header {
            return false;
        }
        self.header = header;
        true
    }

    /// Puts the derived PWM series in the Fan speed chart's second slot. The chart resolves the
    /// raw percent series into that slot on its own snapshot event, which runs before this one,
    /// so the page replaces it after each snapshot. The derived view is rebuilt only when the raw
    /// series or the axis max changed.
    fn sync_pwm(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        let range_ns = self.settings.range_ns;
        let id = series(self.row.idx, MetricId::FAN_PWM);
        let raw = find_tiered(&snap.series, id, range_ns).0.cloned();
        let axis = self.axis_max(cx);
        let same = match (&self.pwm.raw, &raw) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        } && self.pwm.axis_bits == axis.to_bits();
        if !same {
            self.pwm.view = raw.as_deref().map(|r| Arc::new(pwm_scaled(r, axis)));
            self.pwm.raw = raw;
            self.pwm.axis_bits = axis.to_bits();
        }
        let derived = self.pwm.view.clone();
        self.speed.update(cx, |v, cx| {
            if v.series.len() != 2 {
                v.resolve(snap);
            }
            let current = &v.series[1];
            let same = match (current, &derived) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                v.series[1] = derived;
                cx.notify();
            }
        });
    }

    /// The Fan speed axis max in RPM the PWM series is scaled to: the hardware maximum, else the
    /// chart's current ceiling, else 1.
    fn axis_max(&self, cx: &App) -> f32 {
        if let Some(m) = self.fan_max {
            return m as f32;
        }
        let ceiling = self.speed.read(cx).scale_state.ceiling;
        if ceiling.is_finite() && ceiling > 0.0 {
            ceiling
        } else {
            1.0
        }
    }

    fn rebuild_speed(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let view = PageSettingsView::of(&self.settings);
        let spec = speed_spec(self.row.idx, self.fan_max, &view, &theme);
        self.speed.update(cx, |v, cx| v.set_spec(spec, cx));
        // The chart resolved the raw series for the new spec; the page puts its own back.
        self.pwm.raw = None;
        let snap = Arc::clone(self.hub.read(cx).current());
        self.sync_pwm(&snap, cx);
    }

    fn rebuild_temperature(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let view = PageSettingsView::of(&self.settings);
        let spec = temperature_spec(self.sensor_device, &view, &theme);
        let labels = temperature_labels(self.sensor_device.is_some(), &self.words);
        self.temperature.update(cx, |v, cx| {
            v.set_spec(spec, cx);
            v.set_labels(labels, cx);
        });
    }

    /// Rebuilds every chart spec from the current settings and theme.
    fn rebuild_all(&mut self, cx: &mut Context<Self>) {
        self.rebuild_speed(cx);
        self.rebuild_temperature(cx);
    }

    /// Applies the page's settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = PageSettings::read(&self.key, cx);
        if new == self.settings {
            return;
        }
        let pairing = new.temp != self.settings.temp;
        let label = new.label != self.settings.label;
        self.settings = new;
        self.rebuild_all(cx);
        let snap = Arc::clone(self.hub.read(cx).current());
        if pairing {
            self.sync_sensor(&snap, cx);
        }
        if label {
            self.sync_header(&snap, cx);
        }
        cx.notify();
    }

    /// Rebuilds the chart colours after a theme or style change.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_all(cx);
        cx.notify();
    }

    fn chart_box(
        &self,
        view: &Entity<ChartView>,
        selector: &'static str,
        m: &crate::theme::UiMetrics,
    ) -> gpui_kit::Div {
        div()
            .debug_selector(move || selector.into())
            .w_full()
            .h(px(GRAPH_H_STACKED * m.ui_scale))
            .child(view.clone().cached(StyleRefinement::default().size_full()))
    }

    /// The empty Temperature plot: the value `-` at the right of the heading row and the centred
    /// text "Choose a sensor".
    fn empty_overlay(&self, m: &crate::theme::UiMetrics, theme: &TlTheme) -> AnyElement {
        let mut value = div()
            .absolute()
            .top(m.sp[3])
            .right(m.sp[3])
            .text_color(theme.text_secondary)
            .child(SharedString::from(NO_VALUE));
        *value.text_style() = label_style(m, m.text.sm, FontWeight::NORMAL);
        let mut choose = div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.text_secondary)
            .child(self.words.choose.clone());
        *choose.text_style() = label_style(m, m.text.sm, FontWeight::NORMAL);
        div()
            .absolute()
            .size_full()
            .child(choose)
            .child(value)
            .into_any_element()
    }
}

/// The fan's index among the visible fans, from 0.
fn fan_number(key: &DeviceKey, snap: &Snapshot, cx: &App) -> u16 {
    use std::collections::HashMap;
    let p = &TlSettings::settings(cx).performance;
    let k = |s: &str| DeviceKey(Arc::from(s));
    let order: Vec<DeviceKey> = p.device_order.iter().map(|s| k(s)).collect();
    let overrides: HashMap<_, _> = p.device_overrides.iter().map(|(s, v)| (k(s), *v)).collect();
    let view = PerformanceSettingsView {
        order: &order,
        overrides: &overrides,
        show_disks: p.show_disks,
        show_gpus: p.show_gpus,
        show_fans: p.show_fans,
        show_disconnected_fans: p.show_disconnected_fans,
        show_batteries: p.show_batteries,
        show_network_wired: p.show_network_wired,
        show_network_wireless: p.show_network_wireless,
        show_network_vpn: p.show_network_vpn,
        show_network_virtual: p.show_network_virtual,
        show_network_other: p.show_network_other,
    };
    let mut rows = Vec::new();
    sidebar_order(&snap.devices, &snap.system.sensors, &view, &mut rows);
    let visible: Vec<DeviceRow> = visible_rows(&rows, &view).cloned().collect();
    type_number(&visible, key)
}

impl DevicePage for FanPage {
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
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let mut temperature = self
            .chart_box(&self.temperature, TEMPERATURE_ID, &m)
            .relative();
        if self.sensor.is_none() {
            temperature = temperature.child(self.empty_overlay(&m, &theme));
        }
        v_flex()
            .w_full()
            .gap(m.sp[4])
            .child(self.chart_box(&self.speed, SPEED_ID, &m))
            .child(temperature)
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    /// E25-032 adds "Rename fan…" and "Temperature sensor >".
    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        menu
    }

    /// E25-029 replaces this with the full copy text.
    fn copy_text(&self, out: &mut String) {
        let (word, model) = &self.header;
        out.push_str(word);
        if !model.is_empty() {
            out.push(' ');
            out.push_str(model);
        }
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

impl Focusable for FanPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FanPage {
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
        if self.words.refresh(cx) {
            let labels = speed_labels(&self.words);
            self.speed.update(cx, |v, cx| v.set_labels(labels, cx));
            let labels = temperature_labels(self.sensor_device.is_some(), &self.words);
            self.temperature
                .update(cx, |v, cx| v.set_labels(labels, cx));
            let snap = Arc::clone(self.hub.read(cx).current());
            self.apply_snapshot(&snap, ComponentMask::ALL, cx);
        }
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

// ---- Battery ----

/// The battery chart colour.
fn battery_line(theme: &TlTheme) -> gpui_kit::Hsla {
    accent_color(BAT_COLOR).unwrap_or(theme.graph.gpu_line)
}

/// The battery chart (UI-PRF-035). A system battery draws its signed power: charge above the
/// zero line, discharge below. Any other device draws its charge percentage.
pub fn battery_spec(
    device: DeviceIdx,
    system: bool,
    s: &PageSettingsView,
    theme: &TlTheme,
) -> ChartSpec {
    let line = battery_line(theme);
    let (metric, y) = if system {
        (MetricId::BATTERY_POWER, YScale::Symmetric)
    } else {
        (
            MetricId::BATTERY_PCT,
            YScale::Fixed {
                min: 0.0,
                max: 100.0,
            },
        )
    };
    ChartSpec {
        series: smallvec![ChartSeries {
            id: series(device, metric),
            fill: Some(line.opacity(FILL_ALPHA)),
            line,
            ..ChartSeries::default()
        }],
        y,
        range_ns: s.range_ns,
        smooth: s.smooth,
        grid: s.grid.clone(),
        background: ChartBg::Window,
        ..ChartSpec::default()
    }
}

/// The battery page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
struct BatWords {
    version: Option<Option<u64>>,
    power: SharedString,
    percentage: SharedString,
    range_power: SharedString,
}

impl BatWords {
    /// True when the strings changed since the last call.
    fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(
            cx.try_global::<crate::i18n::Strings>()
                .map(|s| s.strings_version),
        );
        if self.version == version {
            return false;
        }
        self.version = version;
        self.power = tr(cx, BAT_KEY_POWER);
        self.percentage = tr(cx, BAT_KEY_PERCENTAGE);
        self.range_power = tr(cx, BAT_KEY_RANGE_POWER);
        true
    }
}

fn battery_labels(system: bool, words: &BatWords) -> ChartLabels {
    if system {
        ChartLabels {
            heading: Some(words.power.clone()),
            value_fmt: ValueFormat::Watts,
            value_align: ValueAlign::Right,
            range_word: words.range_power.clone(),
            legend: SmallVec::new(),
            show_axis_max: true,
        }
    } else {
        ChartLabels {
            heading: Some(words.percentage.clone()),
            value_fmt: ValueFormat::Percent,
            value_align: ValueAlign::Right,
            range_word: words.percentage.clone(),
            legend: SmallVec::new(),
            show_axis_max: false,
        }
    }
}

/// The Battery Details model: the rows in order, with the owner texts the rows use.
pub fn battery_details_model() -> KeyValueModel {
    let mut model = KeyValueModel::new(&BATTERY_DETAIL_ROWS).with_texts(&[
        (BAT_KEY_WH, &["wh"]),
        (BAT_KEY_V, &["v"]),
        (BAT_KEY_YES, &[]),
        (BAT_KEY_NO, &[]),
        (BAT_KEY_ENABLED, &[]),
        (BAT_KEY_DISABLED, &[]),
        (BAT_STATES[0].1, &[]),
        (BAT_STATES[1].1, &[]),
        (BAT_STATES[2].1, &[]),
        (BAT_STATES[3].1, &[]),
        (BAT_STATES[4].1, &[]),
        (BAT_STATES[5].1, &[]),
        (BAT_STATES[6].1, &[]),
    ]);
    model.hide(BAT_ROW_TIME_FULL);
    model
}

fn set_hidden(model: &mut KeyValueModel, label: &'static str, hidden: bool) -> bool {
    let was = model
        .rows
        .iter()
        .find(|r| r.label == label)
        .is_some_and(|r| r.hidden);
    if was == hidden {
        return false;
    }
    if hidden {
        model.hide(label);
    } else {
        model.show(label);
    }
    true
}

/// The state word key; "Unknown" for a name the page does not know.
fn state_key(state: &str) -> &'static str {
    BAT_STATES
        .iter()
        .find(|(name, _)| *name == state)
        .map_or(BAT_STATES[6].1, |(_, key)| key)
}

/// The states that show "Time to full" instead of "Time to empty".
fn shows_time_to_full(state: &str) -> bool {
    matches!(state, "Charging" | "Pending charge")
}

fn inv_shown(validity: Validity) -> bool {
    matches!(validity, Validity::Valid | Validity::Stale)
}

/// A numeric row. Valid and Stale run `write`; the rest show a fallback.
fn inv_row(
    model: &mut KeyValueModel,
    label: &'static str,
    f: InvField<f32>,
    age_ns: u64,
    write: impl FnOnce(&mut KeyValueModel, u32) -> bool,
) -> bool {
    match f.validity {
        Validity::Valid => write(model, f.value.to_bits()),
        Validity::Stale => write(model, f.value.to_bits()) | model.set_stale(label, age_ns),
        Validity::Unsupported => {
            model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None)
        }
        v => model.set_fallback(label, v, Fallback::Unavailable, None),
    }
}

fn float_row(
    model: &mut KeyValueModel,
    label: &'static str,
    f: InvField<f32>,
    age_ns: u64,
    fmt: fn(&mut String, f32),
) -> bool {
    let value = f.value;
    inv_row(model, label, f, age_ns, move |m, bits| {
        m.set_valid(label, u64::from(bits), |o| fmt(o, value))
    })
}

/// A row whose text is a template with one argument written with two decimals.
fn unit_row(
    model: &mut KeyValueModel,
    label: &'static str,
    key: &'static str,
    f: InvField<f32>,
    age_ns: u64,
) -> bool {
    let value = f.value;
    let template = model.text(key);
    inv_row(model, label, f, age_ns, move |m, bits| {
        m.set_valid(label, u64::from(bits), |o| {
            template.write_each(o, |_, o| {
                let _ = write!(o, "{value:.2}");
            });
        })
    })
}

fn text_row(model: &mut KeyValueModel, label: &'static str, text: &str) -> bool {
    if text.trim().is_empty() {
        model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None)
    } else {
        model.set_text(label, text.trim())
    }
}

/// The time row of the state: a positive number of seconds shows the short age, anything else
/// reads "Unknown" (UPower reports 0 when it does not know).
fn time_row(model: &mut KeyValueModel, label: &'static str, f: InvField<i64>) -> bool {
    match u64::try_from(f.value) {
        Ok(secs) if inv_shown(f.validity) && secs > 0 => {
            model.set_valid(label, secs, |o| units::short_age(o, secs))
        }
        _ => model.set_fallback(label, Validity::Unsupported, Fallback::Unknown, None),
    }
}

/// Fills the Details rows from the battery record; without one every row reads "Unavailable".
/// True when a row changed.
fn update_battery_details(
    model: &mut KeyValueModel,
    b: Option<&BatteryDetails>,
    kind_word: &str,
    age_ns: u64,
) -> bool {
    let mut changed = false;
    let Some(b) = b else {
        for label in BATTERY_DETAIL_ROWS {
            changed |=
                model.set_fallback(label, Validity::Unsupported, Fallback::Unavailable, None);
        }
        return changed;
    };
    changed |= float_row(
        model,
        BAT_ROW_PERCENTAGE,
        b.percentage,
        age_ns,
        units::percent,
    );
    changed |= unit_row(model, BAT_ROW_ENERGY, BAT_KEY_WH, b.energy_wh, age_ns);
    changed |= unit_row(model, BAT_ROW_VOLTAGE, BAT_KEY_V, b.voltage_v, age_ns);
    changed |= float_row(model, BAT_ROW_POWER, b.energy_rate_w, age_ns, units::watts);
    let state = model.text(state_key(&b.state));
    changed |= model.set_text(BAT_ROW_STATE, state.text());

    let full = shows_time_to_full(&b.state);
    changed |= set_hidden(model, BAT_ROW_TIME_EMPTY, full);
    changed |= set_hidden(model, BAT_ROW_TIME_FULL, !full);
    changed |= time_row(model, BAT_ROW_TIME_EMPTY, b.time_to_empty_s);
    changed |= time_row(model, BAT_ROW_TIME_FULL, b.time_to_full_s);

    changed |= if inv_shown(b.charge_cycles.validity) {
        let n = b.charge_cycles.value;
        model.set_valid(BAT_ROW_CYCLES, u64::from(n), |o| {
            let _ = write!(o, "{n}");
        })
    } else {
        model.set_fallback(
            BAT_ROW_CYCLES,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        )
    };
    changed |= if inv_shown(b.serial.validity) {
        text_row(model, BAT_ROW_SERIAL, &b.serial.value)
    } else {
        model.set_fallback(
            BAT_ROW_SERIAL,
            Validity::Unsupported,
            Fallback::NotReported,
            None,
        )
    };
    changed |= model.set_text(BAT_ROW_TYPE, kind_word);
    let powering = model.text(if b.power_supply {
        BAT_KEY_YES
    } else {
        BAT_KEY_NO
    });
    changed |= model.set_text(BAT_ROW_POWERING, powering.text());
    changed |= text_row(model, BAT_ROW_TECHNOLOGY, &b.technology);
    changed |= float_row(
        model,
        BAT_ROW_CAPACITY,
        b.capacity_pct,
        age_ns,
        units::percent,
    );
    changed |= unit_row(
        model,
        BAT_ROW_ENERGY_EMPTY,
        BAT_KEY_WH,
        b.energy_empty_wh,
        age_ns,
    );
    changed |= unit_row(
        model,
        BAT_ROW_ENERGY_FULL,
        BAT_KEY_WH,
        b.energy_full_wh,
        age_ns,
    );
    changed |= unit_row(
        model,
        BAT_ROW_ENERGY_DESIGN,
        BAT_KEY_WH,
        b.energy_full_design_wh,
        age_ns,
    );
    changed |= unit_row(
        model,
        BAT_ROW_VOLTAGE_MIN,
        BAT_KEY_V,
        b.voltage_min_design_v,
        age_ns,
    );
    changed |= unit_row(
        model,
        BAT_ROW_VOLTAGE_MAX,
        BAT_KEY_V,
        b.voltage_max_design_v,
        age_ns,
    );

    let thresholds = [
        BAT_ROW_THRESHOLD,
        BAT_ROW_THRESHOLD_START,
        BAT_ROW_THRESHOLD_END,
    ];
    match b.charge_threshold {
        Some((enabled, start, end)) => {
            for label in thresholds {
                changed |= set_hidden(model, label, false);
            }
            let word = model.text(if enabled {
                BAT_KEY_ENABLED
            } else {
                BAT_KEY_DISABLED
            });
            changed |= model.set_text(BAT_ROW_THRESHOLD, word.text());
            changed |= model.set_valid(BAT_ROW_THRESHOLD_START, u64::from(start), |o| {
                let _ = write!(o, "{start}%");
            });
            changed |= model.set_valid(BAT_ROW_THRESHOLD_END, u64::from(end), |o| {
                let _ = write!(o, "{end}%");
            });
        }
        None => {
            for label in thresholds {
                changed |= set_hidden(model, label, true);
            }
        }
    }
    changed |= float_row(
        model,
        BAT_ROW_TEMPERATURE,
        b.temperature_c,
        age_ns,
        units::celsius,
    );
    changed
}

/// The battery record of `device` in the snapshot.
fn battery_of(snap: &Snapshot, device: DeviceIdx) -> Option<&BatteryDetails> {
    snap.device_details
        .batteries
        .iter()
        .find(|(i, _)| *i == device)
        .map(|(_, b)| b)
}

/// The Battery device page: one chart (Power for a system battery, Percentage for any other
/// UPower device) and the Details rows.
pub struct BatteryPage {
    key: DeviceKey,
    row: DeviceRow,
    chart: Entity<ChartView>,
    details: KeyValueModel,
    /// The device powers the system (`PowerSupply`), so the chart shows power.
    is_system: bool,
    focus: FocusHandle,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    id: EntityId,
    active: bool,
    window: Option<AnyWindowHandle>,
    header: (SharedString, SharedString),
    settings: PageSettings,
    settings_versions: [u64; 4],
    style_version: u64,
    words: BatWords,
    _subs: SmallVec<[Subscription; 3]>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
}

impl BatteryPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = current(cx).clone();
        let settings = PageSettings::read(&key, cx);
        let snap = Arc::clone(hub.read(cx).current());
        let mut words = BatWords::default();
        words.refresh(cx);
        let is_system = battery_of(&snap, row.idx).is_some_and(|b| b.power_supply);
        let view = PageSettingsView::of(&settings);
        let spec = battery_spec(row.idx, is_system, &view, &theme);
        let labels = battery_labels(is_system, &words);
        let chart = cx.new(|cx| ChartView::new(spec, labels, &hub, cx));

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<crate::theme::UiMetrics>(Self::sync_style));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            key,
            row,
            chart,
            details: battery_details_model(),
            is_system,
            focus: cx.focus_handle(),
            hub,
            demand,
            id,
            active: false,
            window: None,
            header: (SharedString::default(), SharedString::default()),
            settings,
            settings_versions: section_versions(cx),
            style_version: metrics(cx).style_version,
            words,
            _subs: subs,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
    }

    pub fn chart(&self) -> &Entity<ChartView> {
        &self.chart
    }

    /// True when the chart shows power (the battery powers the system).
    pub fn is_system(&self) -> bool {
        self.is_system
    }

    /// This page's demand part while it is active: batteries ride on the sensors bit.
    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            sensors: true,
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

    /// Brings the chart kind, Details rows and header up to date. Notifies only when something
    /// the page draws changed.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let details = mask.contains(ComponentMask::DEVICE_DETAILS);
        let devices = mask.contains(ComponentMask::DEVICES);
        let mut changed = false;
        let record = battery_of(snap, self.row.idx);
        if details {
            let system = record.is_some_and(|b| b.power_supply);
            if system != self.is_system {
                self.is_system = system;
                self.rebuild_chart(cx);
                changed = true;
            }
            let kind = record.map_or("", |b| &*b.kind);
            let word = page_words(cx).battery_kind(kind).clone();
            let age_ns = now_boot_ns().saturating_sub(snap.device_details.header.observed_end);
            changed |= update_battery_details(&mut self.details, record, &word, age_ns);
        }
        if details || devices {
            changed |= self.sync_header(snap, cx);
        }
        if changed {
            cx.notify();
        }
    }

    fn sync_header(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx) else {
            return false;
        };
        let header = header_text(page_words(cx), DeviceKind::Battery, 0, entry, snap);
        if header == self.header {
            return false;
        }
        self.header = header;
        true
    }

    fn rebuild_chart(&mut self, cx: &mut Context<Self>) {
        let theme = current(cx).clone();
        let view = PageSettingsView::of(&self.settings);
        let spec = battery_spec(self.row.idx, self.is_system, &view, &theme);
        let labels = battery_labels(self.is_system, &self.words);
        self.chart.update(cx, |v, cx| {
            v.set_spec(spec, cx);
            v.set_labels(labels, cx);
        });
    }

    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = PageSettings::read(&self.key, cx);
        if new == self.settings {
            return;
        }
        self.settings = new;
        self.rebuild_chart(cx);
        cx.notify();
    }

    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_chart(cx);
        cx.notify();
    }
}

impl DevicePage for BatteryPage {
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
        let m = *metrics(cx);
        v_flex()
            .w_full()
            .child(
                div()
                    .debug_selector(|| BATTERY_CHART_ID.into())
                    .w_full()
                    .h(px(GRAPH_H_STACKED * m.ui_scale))
                    .child(
                        self.chart
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
            )
            .into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    fn menu_items(&self, menu: TlMenu, _: &mut Window, _: &mut App) -> TlMenu {
        menu
    }

    /// The CPU copy form (PERF-011): the header word and the model on their own lines, then
    /// every Details row indented. The page has no blocks.
    fn copy_text(&self, out: &mut String) {
        let (word, model) = &self.header;
        out.push_str(word);
        out.push('\n');
        if !model.is_empty() {
            out.push_str(model);
            out.push('\n');
        }
        let rows_from = out.len();
        self.details.copy_into(out);
        if out.len() > rows_from {
            out.push('\n');
        }
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

impl Focusable for BatteryPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BatteryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            self.send_demand(cx);
        }
        self.details.refresh_strings(cx);
        if self.words.refresh(cx) {
            let labels = battery_labels(self.is_system, &self.words);
            self.chart.update(cx, |v, cx| v.set_labels(labels, cx));
            let snap = Arc::clone(self.hub.read(cx).current());
            self.apply_snapshot(&snap, ComponentMask::ALL, cx);
        }
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

    fn raw(values: &[f32]) -> SeriesView {
        SeriesView {
            id: SeriesId {
                device: DeviceIdx(3),
                metric: MetricId::FAN_PWM,
            },
            version: 7,
            period_ns: NS_PER_S,
            samples: values
                .iter()
                .enumerate()
                .map(|(i, v)| Sample::valid(i as u64 * NS_PER_S, *v))
                .collect(),
        }
    }

    #[test]
    fn pwm_series_scaled_to_axis() {
        let scaled = pwm_scaled(&raw(&[0.0, 25.0, 50.0]), 1800.0);
        let values: Vec<f32> = scaled.samples.iter().map(|s| s.value).collect();
        assert_eq!(values, [0.0, 450.0, 900.0]);
        assert_eq!(scaled.samples.last().map(|s| s.value), Some(900.0));
        assert_eq!(scaled.id.metric, MetricId::FAN_PWM);
        assert_eq!(scaled.samples[2].t_ns, 2 * NS_PER_S);
    }

    #[test]
    fn pwm_series_version_follows_axis() {
        let r = raw(&[10.0]);
        assert_ne!(
            pwm_scaled(&r, 1800.0).version,
            pwm_scaled(&r, 3600.0).version
        );
        assert_eq!(
            pwm_scaled(&r, 1800.0).version,
            pwm_scaled(&r, 1800.0).version
        );
    }

    #[test]
    fn fan_key_parts_split() {
        assert_eq!(
            fan_key_parts("fan:nct6797:/devices/platform/nct6775.656:2"),
            Some(("nct6797:/devices/platform/nct6775.656", "2"))
        );
        assert_eq!(fan_key_parts("fan:nct6797::1"), Some(("nct6797:", "1")));
        assert_eq!(fan_key_parts("gpu:x"), None);
    }
}
