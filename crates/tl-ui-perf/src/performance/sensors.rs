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

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, AnyView, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StyleRefinement, Styled as _, Subscription, Task, WeakEntity, Window, div, px,
    relative,
};
use rustc_hash::FxHashSet;
use smallvec::{SmallVec, smallvec};
use tl_model::command::{Command, CommandError, DeviceCommand};
use tl_model::demand::Demand;
use tl_model::devices::DeviceIdentity;
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SampleFlags, SeriesView};
use tl_model::snapshot::{BatteryDetails, InvField, Snapshot};
use tl_model::system::{FanControlMode, Metric, SensorKind, SensorReading};
use tl_model::units;
use tl_model::validity::Validity;

use super::actions::{
    PerfRenameFan, PerfSetFanTempSensor, PerfWidgetEditStart, PerfWidgetResetPage,
};
use super::aggregate::{add_demand, page_layout, reset_page, start_page_edit, strings_version};
#[cfg(test)]
use super::core::sensors::fan_key_parts;
pub use super::core::sensors::{SensorRef, default_sensor, sensor_device};
use super::details::{Fallback, KeyValueModel};
use super::gpu_views::PartResized;
use super::memory_views::{MemoryPartView, PartDraw, PartShown};
use super::page::details_draw;
use super::page::{
    DevicePage, DevicePageFrame, header_text, page_history_request, page_range_ns, page_words,
};
use super::page_flow;
use super::widget::container::WidgetContainer;
use super::widget::details_widget::{DetailsResized, DetailsShown, DetailsView};
use super::widget::edit::WidgetEditor;
use super::widget::fixed_widget::FixedContent;
use super::widget::source::SourceStore;
use super::{
    DeviceKey, DeviceKind, DeviceRow, PerformanceSettingsView, sidebar_order, type_number,
    visible_rows,
};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::host::HostContext;
use tl_ui_base::hub::{ComponentMask, SnapshotChanged, SnapshotHub, is_dead_reading};
use tl_ui_base::i18n::Strings;
use tl_ui_base::i18n::tr;
use tl_ui_base::selector::TlSelector;
use tl_ui_base::settings_global::{Section, TlSettings, set_fan_label, set_fan_temp};
use tl_ui_base::text::style::t_xs;
use tl_ui_base::theme::palette::accent_color;
use tl_ui_base::theme::{TlTheme, current, metrics};
use tl_ui_chart::chart::view::find_series;
use tl_ui_chart::chart::{
    ChartBg, ChartLabels, ChartSeries, ChartSpec, ChartView, GapStyle, GridStyle, ValueAlign,
    ValueFormat, YScale,
};
use tl_ui_kit::components::dialog::{DialogResult, TlDialog};
use tl_ui_kit::components::menu::TlMenu;
use tl_ui_settings::layout::FixedWidget;
use tl_ui_settings::layout::{METRIC_POWER, METRIC_TEMP};
use tl_ui_settings::layout_pages::{METRIC_PERCENT, METRIC_SPEED, page_container};

const NS_PER_S: u64 = 1_000_000_000;

pub const ROW_SPEED: &str = "perf-fan-row-speed";
pub const ROW_PWM: &str = "perf-fan-row-pwm";
pub const ROW_MODE: &str = "perf-fan-row-mode";
pub const ROW_TEMPERATURE: &str = "perf-fan-row-temperature";
pub const ROW_CHIP: &str = "perf-fan-row-chip";

/// The five Details rows in grid order (UI-PRF-034).
pub const FAN_DETAIL_ROWS: [&str; 5] = [ROW_SPEED, ROW_PWM, ROW_MODE, ROW_TEMPERATURE, ROW_CHIP];

/// The battery colour name (UI-PRF-005: "Mint").
const BAT_COLOR: &str = "mint";

const BAT_KEY_HEADING_DAY: &str = "perf-bat-heading-day";
/// Monday first, the order of `Weekday::to_monday_zero_offset`.
const DAY_KEYS: [&str; 7] = [
    "perf-day-mon",
    "perf-day-tue",
    "perf-day-wed",
    "perf-day-thu",
    "perf-day-fri",
    "perf-day-sat",
    "perf-day-sun",
];

/// The history chart covers the past 24 hours (R-166, PERF-079).
const HISTORY_RANGE_S: i64 = 86_400;
const HISTORY_RANGE_NS: u64 = HISTORY_RANGE_S as u64 * NS_PER_S;
/// Two points more than this far apart have a gap between them. UPower stores a point only when
/// the charge changes, so a shorter limit would mark most stretches on mains power.
const HISTORY_GAP_S: i64 = 3600;
/// Nominal spacing of the history points, for the chart's overscan.
const HISTORY_PERIOD_NS: u64 = 300 * NS_PER_S;
/// The chart never draws more than this many points (PERF-079).
const HISTORY_MAX_POINTS: usize = 500;
/// How often an open page asks for the history again.
const HISTORY_REFRESH: Duration = Duration::from_secs(600);
/// Height of the history chart in logical px.
const HISTORY_H: f32 = 150.0;

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

/// The sensor the page shows: the saved choice of the fan when set, else the default pairing.
/// A saved choice that is not in the catalog gives `None`.
fn choose_sensor(snap: &Snapshot, fan_key: &str, saved: Option<&str>) -> Option<SensorRef> {
    match saved {
        Some(s) => sensor_device(snap, s).map(|_| SensorRef::from(s)),
        None => default_sensor(snap, fan_key),
    }
}

/// The settings the page draws from.
#[derive(Clone, Debug, PartialEq)]
struct PageSettings {
    /// The shared range the Fan and Battery charts draw.
    range_ns: u64,
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
            range_ns: page_range_ns(cx),
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
        vertical_at: None,
    }
}

fn series(device: DeviceIdx, metric: MetricId) -> SeriesId {
    SeriesId { device, metric }
}

/// The chart settings of one spec, copied out of [`PageSettings`].
pub struct PageSettingsView {
    grid: GridStyle,
}

impl PageSettingsView {
    fn of(s: &PageSettings) -> Self {
        Self { grid: plot_grid(s) }
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

/// The Fan device page. Its body is one widget container (ADR-019 phase 3, `tasklens-hzire.9`):
/// by default the Fan speed and Temperature charts and the Details widget, in the 1.3.0 places
/// (`page_flow::sensors`). Edit Widgets changes them; the page keeps the header, the sensor
/// choice for the Details rows, and the Details model, and hands the model's rows to its
/// `DetailsView`.
///
/// Render contract: the page renders on a header change, a range change and a layout change,
/// never because of a sample. A Details value change notifies only the `DetailsView`.
pub struct FanPage {
    key: DeviceKey,
    row: DeviceRow,
    /// The container id, `page.fan:...`.
    container_id: SharedString,
    details: KeyValueModel,
    details_view: Entity<DetailsView>,
    container: Entity<WidgetContainer>,
    editor: Entity<WidgetEditor>,
    /// The temperature sensor of the Temperature row; `None` when no sensor is paired.
    sensor: Option<SensorRef>,
    sensor_device: Option<DeviceIdx>,
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
    _subs: SmallVec<[Subscription; 8]>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl FanPage {
    pub fn new(
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = PageSettings::read(&key, cx);
        let snap = Arc::clone(hub.read(cx).current());
        let container_id = SharedString::from(page_container(&key.0));
        let sensor = choose_sensor(&snap, &key.0, settings.temp.as_deref());
        let sensor_device = sensor.as_deref().and_then(|s| sensor_device(&snap, s));

        let mut details = fan_details_model();
        details.refresh_strings(cx);
        let details_view = {
            let shown = DetailsShown::of(&details, strings_version(cx));
            let draw = details_draw(cx.weak_entity());
            let id = container_id.clone();
            cx.new(|_| DetailsView::new(id, shown, draw))
        };
        let natural = details_view.read(cx).natural_height();
        let content: FixedContent = {
            let view: AnyView = details_view.clone().into();
            Rc::new(move |kind: &FixedWidget, _: &mut App| {
                matches!(kind, FixedWidget::Details).then(|| view.clone())
            })
        };
        let container = {
            let store = Rc::new(RefCell::new(SourceStore::for_page(&hub)));
            let layout = page_layout(&container_id, cx);
            let id = container_id.clone();
            let hub = hub.clone();
            cx.new(move |cx| {
                let mut c = WidgetContainer::new(id, layout, &hub, store, cx);
                c.set_flow_placer(Some(page_flow::sensors::fan_flow(natural)), cx);
                c.set_fixed_content(Some(content), cx);
                c
            })
        };
        let editor = {
            let container = container.clone();
            let hub = hub.clone();
            cx.new(move |cx| {
                let mut e = WidgetEditor::new(container, &hub, cx);
                e.set_page_parts(vec![FixedWidget::Details]);
                e
            })
        };

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<Strings>(Self::sync_words));
        // The Details grid got taller or shorter: the parts under it move.
        subs.push(
            cx.subscribe(&details_view, |this, _, _: &DetailsResized, cx| {
                this.container.update(cx, |c, cx| {
                    c.place_again(cx);
                });
            }),
        );
        // A layout change can change what the page asks for.
        subs.push(cx.observe(&container, |this, _, cx| this.send_demand(cx)));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            key,
            row,
            container_id,
            details,
            details_view,
            container,
            editor,
            sensor,
            sensor_device,
            focus: cx.focus_handle(),
            hub,
            demand,
            id,
            active: false,
            window: None,
            header: (SharedString::default(), SharedString::default()),
            settings,
            settings_versions: section_versions(cx),
            _subs: subs,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
    }

    /// The chart of the first placed widget of `metric` that draws one.
    pub fn chart_of(&self, metric: &str, cx: &App) -> Option<Entity<ChartView>> {
        let c = self.container.read(cx);
        c.layout()
            .widgets
            .iter()
            .filter(|w| w.content.metric_key().is_some_and(|k| k.as_str() == metric))
            .find_map(|w| c.widget(w.id)?.read(cx).chart().cloned())
    }

    /// The Fan speed chart, while the page shows it.
    pub fn speed(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_SPEED, cx)
    }

    /// The Temperature chart, while the page shows it.
    pub fn temperature(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(METRIC_TEMP, cx)
    }

    /// The page's widget container.
    pub fn container(&self) -> &Entity<WidgetContainer> {
        &self.container
    }

    /// The editor around the container (Edit Widgets).
    pub fn editor(&self) -> &Entity<WidgetEditor> {
        &self.editor
    }

    /// The Details widget's view.
    pub fn details_view(&self) -> &Entity<DetailsView> {
        &self.details_view
    }

    /// The container id, `page.fan:...`.
    pub fn container_id(&self) -> &SharedString {
        &self.container_id
    }

    /// The sensor of the Temperature row; `None` while no sensor is paired.
    pub fn sensor(&self) -> Option<&str> {
        self.sensor.as_deref()
    }

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// This page's demand part while it is active: the sensors component, the device metric
    /// history at 1 h, 24 h and 7 d, and what the placed widgets read.
    pub fn demand_part(&self, cx: &App) -> Demand {
        let mut demand = Demand {
            any_surface_visible: true,
            sensors: true,
            device_history_range: page_history_request(self.settings.range_ns),
            ..Demand::default()
        };
        add_demand(&mut demand, &self.container.read(cx).demand_part());
        demand
    }

    fn send_demand(&self, cx: &mut App) {
        if self.active {
            let (id, window, part) = (self.id, self.window, self.demand_part(cx));
            self.demand.update(cx, |d, cx| d.set(id, window, part, cx));
        }
    }

    fn notify(&self, cx: &mut Context<Self>) {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
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

    /// Brings the sensor choice, the Details rows and the header up to date with the components
    /// in `mask`. Only a header change renders the page; a row change goes to the Details view.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let devices = mask.contains(ComponentMask::DEVICES);
        let system = mask.contains(ComponentMask::SYSTEM);
        if devices {
            self.sync_sensor(snap);
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
                now_boot_ns: self.hub.read(cx).now_ns(),
                observed_end_ns: snap.system.header.observed_end,
                fan,
                temperature,
                chip,
            };
            if update_fan_details(&mut self.details, &inputs, cx) {
                self.sync_details(cx);
            }
        }
        if devices && self.sync_header(snap, cx) {
            self.notify(cx);
        }
        // The fan went away: an edit ends as Cancel does.
        if devices
            && !snap
                .devices
                .entries
                .iter()
                .any(|e| e.idx == self.row.idx && e.present)
        {
            self.editor.update(cx, |e, cx| e.cancel(cx));
        }
    }

    /// Hands the Details view what the grid shows; it notifies only when that changed.
    fn sync_details(&mut self, cx: &mut Context<Self>) {
        let shown = DetailsShown::of(&self.details, strings_version(cx));
        self.details_view.update(cx, |v, cx| v.set_shown(shown, cx));
    }

    /// Reads the sensor choice: the saved one, else the default pairing. True when it changed.
    /// The Temperature widget follows the same choice through its source.
    fn sync_sensor(&mut self, snap: &Snapshot) -> bool {
        let sensor = choose_sensor(snap, &self.key.0, self.settings.temp.as_deref());
        let device = sensor.as_deref().and_then(|s| sensor_device(snap, s));
        if sensor == self.sensor && device == self.sensor_device {
            return false;
        }
        self.sensor = sensor;
        self.sensor_device = device;
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
        let word = header.0.clone();
        self.editor.update(cx, |e, _| e.set_page_name(word));
        self.header = header;
        true
    }

    /// Applies the page's settings when one of their sections changed.
    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        // A saved layout (Apply, Reset Page) shows when the page is not in Edit Widgets.
        if !self.editor.read(cx).is_editing() {
            let layout = page_layout(&self.container_id, cx);
            self.container.update(cx, |c, cx| c.set_layout(layout, cx));
        }
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
        let range_moved = new.range_ns != self.settings.range_ns;
        self.settings = new;
        let snap = Arc::clone(self.hub.read(cx).current());
        if pairing && self.sync_sensor(&snap) {
            self.apply_snapshot(&snap, ComponentMask::SYSTEM, cx);
        }
        if label {
            self.sync_header(&snap, cx);
        }
        if range_moved {
            // The header switch shows the new range.
            self.send_demand(cx);
        }
        if range_moved || label {
            self.notify(cx);
        }
    }

    /// New strings: the Details words and the header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        self.details.refresh_strings(cx);
        let snap = Arc::clone(self.hub.read(cx).current());
        self.apply_snapshot(&snap, ComponentMask::SYSTEM, cx);
        self.sync_details(cx);
        self.sync_header(&snap, cx);
        self.notify(cx);
    }

    /// "Edit Widgets…" in the page menu.
    fn on_edit_start(
        &mut self,
        _: &PerfWidgetEditStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        start_page_edit(&self.editor, &self.hub, window, cx);
    }

    /// "Reset Page" in the page menu (ADR-019 phase 3 "Reset").
    fn on_reset_page(
        &mut self,
        _: &PerfWidgetResetPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        reset_page(&self.container_id, self.header.0.clone(), window, cx);
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

/// One radio item of the "Temperature sensor" submenu.
struct FanSensorOption {
    /// The stored `temp` value: the sensor key without its `sensor:` prefix.
    value: Arc<str>,
    /// "<chip> <label>".
    label: SharedString,
    /// "48 °C", or `-` without a valid reading.
    reading: SharedString,
}

/// True when the sensor's fast or 1 Hz temperature series holds a sample with a real value. The
/// 1 Hz tier reaches 10 min back, which covers the time before the page was built.
fn has_history(snap: &Snapshot, device: DeviceIdx) -> bool {
    let id = series(device, MetricId::SENSOR_TEMP);
    let slow = series(device, MetricId::SENSOR_TEMP.with_tier_1hz());
    [id, slow].into_iter().any(|id| {
        find_series(&snap.series, id).is_some_and(|v| {
            v.samples
                .iter()
                .any(|s| s.flags.contains(SampleFlags::VALID) && !is_dead_reading(s.value))
        })
    })
}

/// Every present hwmon temperature sensor in catalog order, without sensors that read a dead
/// value (-128 °C or 0 °C) and have no history: not in `live` (read a real value since the app
/// was built) and no real sample in either series tier. The `current` sensor stays so its radio
/// shows.
fn fan_sensor_options(
    snap: &Snapshot,
    current: Option<&str>,
    live: &FxHashSet<DeviceIdx>,
) -> Vec<FanSensorOption> {
    snap.devices
        .entries
        .iter()
        .filter(|e| e.present)
        .filter_map(|e| {
            let DeviceIdentity::Sensor { chip, label } = &e.identity else {
                return None;
            };
            let reading = snap
                .system
                .sensors
                .iter()
                .find(|(idx, r)| *idx == e.idx && r.kind == SensorKind::Temperature)
                .map(|(_, r)| r.value)
                .filter(|m| m.validity == Validity::Valid);
            let value = e
                .persistent_key
                .strip_prefix("sensor:")
                .unwrap_or(&e.persistent_key);
            let dead = reading.is_some_and(|m| is_dead_reading(m.value));
            if dead && current != Some(value) && !live.contains(&e.idx) && !has_history(snap, e.idx)
            {
                return None;
            }
            let mut text = String::new();
            match reading {
                Some(m) => units::celsius(&mut text, m.value),
                None => text.push_str(NO_VALUE),
            }
            Some(FanSensorOption {
                value: Arc::from(value),
                label: format!("{chip} {label}").into(),
                reading: text.into(),
            })
        })
        .collect()
}

impl FanPage {
    /// `PerfRenameFan`: opens the Rename dialog with the shown name. Confirming writes
    /// `fans[key].label`; an empty text removes it. The page follows through its settings
    /// observer.
    fn on_rename(&mut self, a: &PerfRenameFan, window: &mut Window, cx: &mut Context<Self>) {
        if a.key != self.key {
            return;
        }
        let key = self.key.0.clone();
        let title = tr(cx, "perf-fan-rename-title");
        let initial = self.header.0.clone();
        let dialog = TlDialog::confirm(title.clone(), tr(cx, "perf-fan-rename-ok"))
            .title(title)
            .text_input(initial, tr(cx, "perf-fan-rename-placeholder"));
        let opened = dialog.open(window, cx, move |result, _, cx| {
            if let DialogResult::ConfirmedText(text) = result {
                set_fan_label(&key, &text, cx);
            }
        });
        if opened.is_err() {
            tracing::debug!(target: "tasklens::ui", "fan rename dialog did not open");
        }
    }

    /// `PerfSetFanTempSensor`: writes `fans[key].temp`, or removes it for `None`.
    fn on_set_sensor(&mut self, a: &PerfSetFanTempSensor, _: &mut Window, cx: &mut Context<Self>) {
        if a.key != self.key {
            return;
        }
        set_fan_temp(&self.key.0, a.sensor.as_deref(), cx);
    }
}

impl DevicePage for FanPage {
    fn hub(&self) -> Option<&Entity<SnapshotHub>> {
        Some(&self.hub)
    }

    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, _: &mut Window, _: &mut App) -> AnyElement {
        // The charts are widgets of the container.
        div().into_any_element()
    }

    fn details(&self) -> &KeyValueModel {
        &self.details
    }

    fn blocks(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 3]> {
        SmallVec::new()
    }

    /// "Rename fan…" and "Temperature sensor >" (PERF-077, UI-PRF-034). The sensor values are
    /// read from the newest snapshot once, when the menu opens.
    fn menu_items(&self, menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
        let key = self.key.clone();
        let hub = self.hub.read(cx);
        let snap = Arc::clone(hub.current());
        let options = fan_sensor_options(&snap, self.sensor.as_deref(), hub.live_sensors());
        let current = self.sensor.clone();
        let none = tr(cx, "perf-fan-sensor-none");
        let rename = Box::new(PerfRenameFan { key: key.clone() });
        menu.item(tr(cx, "perf-fan-menu-rename"), rename).submenu(
            tr(cx, "perf-fan-menu-sensor"),
            window,
            cx,
            move |mut menu, _, _| {
                for o in &options {
                    let selected = current.as_deref() == Some(&*o.value);
                    let action = Box::new(PerfSetFanTempSensor {
                        key: key.clone(),
                        sensor: Some(o.value.clone()),
                    });
                    menu = menu
                        .radio_item(o.label.clone(), selected, action)
                        .value(o.reading.clone());
                }
                let action = Box::new(PerfSetFanTempSensor {
                    key: key.clone(),
                    sensor: None,
                });
                menu.radio_item(none.clone(), current.is_none(), action)
            },
        )
    }

    /// The Fan copy text (PERF-011): the header lines, then the Details rows. The page has no
    /// blocks.
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

    fn widget_editor(&self) -> Option<&Entity<WidgetEditor>> {
        Some(&self.editor)
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
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // The demand part carries the window, so it drops out while the window is hidden.
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            self.send_demand(cx);
        }
        let m = *metrics(cx);
        let theme = current(cx).clone();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_rename))
            .on_action(cx.listener(Self::on_set_sensor))
            .on_action(cx.listener(Self::on_edit_start))
            .on_action(cx.listener(Self::on_reset_page))
            .child(
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

/// The history series of a system battery. Sample times are unix nanoseconds, so the chart
/// (frozen, right edge at its newest sample) needs no boot-time offset. Only the past 24 hours up
/// to `now_s` are kept, at most 500 points. A point more than an hour after the one before it
/// starts a new run (`GAP_BEFORE`). A last sample at `now_s` carries the newest charge to the
/// right edge, so the axis ends at the present.
pub fn history_series(id: SeriesId, points: &[(i64, f32)], now_s: i64, version: u64) -> SeriesView {
    let from = now_s - HISTORY_RANGE_S;
    let mut samples: Vec<Sample> = Vec::with_capacity(points.len().min(HISTORY_MAX_POINTS) + 1);
    let mut last_s = None;
    let mut push = |t_s: i64, pct: f32, samples: &mut Vec<Sample>| {
        let mut sample = Sample::valid(t_s as u64 * NS_PER_S, pct);
        if last_s.is_some_and(|prev| t_s - prev > HISTORY_GAP_S) {
            sample.flags |= SampleFlags::GAP_BEFORE;
        }
        last_s = Some(t_s);
        samples.push(sample);
    };
    let kept = points.iter().filter(|(t, _)| (from..=now_s).contains(t));
    let skip = kept.clone().count().saturating_sub(HISTORY_MAX_POINTS);
    for &(t_s, pct) in kept.skip(skip) {
        push(t_s, pct, &mut samples);
    }
    if let Some(&(newest_s, pct)) = points
        .iter()
        .filter(|(t, _)| *t <= now_s)
        .max_by_key(|p| p.0)
        && newest_s >= from
        && newest_s < now_s
    {
        push(now_s, pct, &mut samples);
    }
    SeriesView {
        id,
        version,
        period_ns: HISTORY_PERIOD_NS,
        samples: samples.into(),
    }
}

/// The local midnights inside the 24 hours before `now_s`: unix seconds and the weekday that
/// starts (0 = Monday). Usually one; a 25-hour day can have none.
pub fn local_midnights(now_s: i64) -> SmallVec<[(i64, u8); 2]> {
    let mut out = SmallVec::new();
    let Ok(ts) = jiff::Timestamp::from_second(now_s) else {
        return out;
    };
    let tz = jiff::tz::TimeZone::system();
    let today = ts.to_zoned(tz.clone()).date();
    let days = [today.yesterday().ok(), Some(today)];
    for date in days.into_iter().flatten() {
        let Ok(start) = date.to_zoned(tz.clone()) else {
            continue;
        };
        let at = start.timestamp().as_second();
        if at > now_s - HISTORY_RANGE_S && at <= now_s {
            out.push((at, date.weekday().to_monday_zero_offset() as u8));
        }
    }
    out
}

fn unix_now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The charge history chart (UI-PRF-035): the past 24 hours, no fill, 0 to 100 %, dashed across
/// gaps, vertical grid lines at `midnights` (fractions of the plot width).
fn history_spec(
    device: DeviceIdx,
    midnights: Arc<[f32]>,
    s: &PageSettingsView,
    theme: &TlTheme,
) -> ChartSpec {
    let line = battery_line(theme);
    let mut grid = s.grid.clone();
    grid.vertical_at = Some(midnights);
    ChartSpec {
        series: smallvec![ChartSeries {
            id: series(device, MetricId::BATTERY_PCT),
            fill: None,
            line,
            gap_style: GapStyle::Dashed,
            ..ChartSeries::default()
        }],
        y: YScale::Fixed {
            min: 0.0,
            max: 100.0,
        },
        range_ns: HISTORY_RANGE_NS,
        smooth: false,
        grid,
        background: ChartBg::Window,
        hover: true,
        ..ChartSpec::default()
    }
}

fn history_labels(words: &BatWords) -> ChartLabels {
    ChartLabels {
        heading: Some(words.heading_day.clone()),
        value_fmt: ValueFormat::Percent,
        value_align: ValueAlign::Hidden,
        range_word: SharedString::default(),
        legend: SmallVec::new(),
        show_axis_max: false,
    }
}

/// The battery page's words, translated once per strings version.
#[derive(Clone, Debug, Default)]
struct BatWords {
    version: Option<Option<u64>>,
    heading_day: SharedString,
    days: [SharedString; 7],
}

impl BatWords {
    /// True when the strings changed since the last call.
    fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(
            cx.try_global::<tl_ui_base::i18n::Strings>()
                .map(|s| s.strings_version),
        );
        if self.version == version {
            return false;
        }
        self.version = version;
        self.heading_day = tr(cx, BAT_KEY_HEADING_DAY);
        self.days = DAY_KEYS.map(|k| tr(cx, k));
        true
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

/// The part name of the battery history view (`memory_views::part_id`).
const PART_HISTORY: &str = "battery-history";

/// The debug selector of the history chart box of container `container`.
pub fn history_chart_id(container: &str) -> String {
    format!("perf-page-{container}-battery-history")
}

/// The debug selector of the first day label under the history chart of container `container`.
pub fn history_day_label_id(container: &str) -> String {
    format!("perf-page-{container}-battery-day-label")
}

/// The Battery device page. Its body is one widget container (ADR-019 phase 3,
/// `tasklens-hzire.9`): by default the Power chart of a system battery or the Percentage chart
/// of any other UPower device, the 24-hour charge history of a system battery with at least 2
/// points, and the Details widget, in the 1.3.0 places (`page_flow::sensors`). The page keeps the
/// header, the history fetch and the Details model.
///
/// Render contract: the page renders on a header change, a range change and a layout change,
/// never because of a sample. A Details value change notifies only the `DetailsView`; a new
/// history notifies only the history view.
pub struct BatteryPage {
    key: DeviceKey,
    row: DeviceRow,
    /// The container id, `page.bat:...`.
    container_id: SharedString,
    details: KeyValueModel,
    details_view: Entity<DetailsView>,
    history_view: Entity<MemoryPartView>,
    container: Entity<WidgetContainer>,
    editor: Entity<WidgetEditor>,
    /// What the page's flow placer reads.
    parts: page_flow::sensors::BatteryParts,
    /// The 24-hour charge history of a system battery with at least 2 points (PERF-079).
    history: Option<Entity<ChartView>>,
    /// The history record the chart was built from, to tell a new one by `Arc::ptr_eq`.
    history_raw: Option<Arc<[(i64, f32)]>>,
    history_version: u64,
    /// The record had a history at the last snapshot.
    history_seen: bool,
    /// Vertical grid positions (fractions of the plot width) and the weekday of each midnight.
    midnights: SmallVec<[(f32, u8); 2]>,
    native_path: Option<Arc<str>>,
    /// Asks for the history again every 10 minutes while the page is active.
    refresh: Option<Task<()>>,
    this: WeakEntity<Self>,
    /// The device powers the system (`PowerSupply`), so the page shows power.
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
    /// The host this page shows. Set when it opens; never changes.
    host: HostContext,
    _subs: SmallVec<[Subscription; 8]>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub render_count: Cell<u32>,
    #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
    pub notify_count: Cell<u32>,
}

impl BatteryPage {
    pub fn new(
        host: &HostContext,
        key: DeviceKey,
        row: DeviceRow,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = PageSettings::read(&key, cx);
        let snap = Arc::clone(hub.read(cx).current());
        let container_id = SharedString::from(page_container(&key.0));
        let mut words = BatWords::default();
        words.refresh(cx);
        let is_system = battery_of(&snap, row.idx).is_some_and(|b| b.power_supply);
        let this = cx.weak_entity();

        let mut details = battery_details_model();
        details.refresh_strings(cx);
        let details_view = {
            let shown = DetailsShown::of(&details, strings_version(cx));
            let draw = details_draw(this.clone());
            let id = container_id.clone();
            cx.new(|_| DetailsView::new(id, shown, draw))
        };
        let history_view = {
            let id = container_id.clone();
            let draw = history_draw(this.clone());
            cx.new(move |_| MemoryPartView::new(id, PART_HISTORY, PartShown::default(), draw))
        };
        let parts = page_flow::sensors::BatteryParts {
            details: details_view.read(cx).natural_height(),
            history: history_view.read(cx).natural_height(),
            history_shown: Rc::new(Cell::new(false)),
            system: Rc::new(Cell::new(is_system)),
        };
        let content: FixedContent = {
            let details: AnyView = details_view.clone().into();
            let history: AnyView = history_view.clone().into();
            let shown = Rc::clone(&parts.history_shown);
            // A history 1.3.0 hides shows "Nothing to show on this device" in a grid layout.
            Rc::new(move |kind: &FixedWidget, _: &mut App| match kind {
                FixedWidget::Details => Some(details.clone()),
                FixedWidget::BatteryHistory => shown.get().then(|| history.clone()),
                _ => None,
            })
        };
        let container = {
            let store = Rc::new(RefCell::new(SourceStore::for_page(&hub)));
            let layout = page_layout(&container_id, cx);
            let id = container_id.clone();
            let hub = hub.clone();
            let flow = page_flow::sensors::battery_flow(parts.clone());
            cx.new(move |cx| {
                let mut c = WidgetContainer::new(id, layout, &hub, store, cx);
                c.set_flow_placer(Some(flow), cx);
                c.set_fixed_content(Some(content), cx);
                c
            })
        };
        let editor = {
            let container = container.clone();
            let hub = hub.clone();
            cx.new(move |cx| {
                let mut e = WidgetEditor::new(container, &hub, cx);
                e.set_page_parts(battery_page_parts(false));
                e
            })
        };

        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, Self::on_snapshot));
        subs.push(cx.observe_global::<TlSettings>(Self::sync_settings));
        subs.push(cx.observe_global::<tl_ui_base::theme::UiMetrics>(Self::sync_style));
        subs.push(cx.observe_global::<Strings>(Self::sync_words));
        // A part got taller or shorter: the parts under it move.
        subs.push(
            cx.subscribe(&details_view, |this, _, _: &DetailsResized, cx| {
                this.place_again(cx);
            }),
        );
        subs.push(cx.subscribe(&history_view, |this, _, _: &PartResized, cx| {
            this.place_again(cx);
        }));
        // A layout change can change what the page asks for.
        subs.push(cx.observe(&container, |this, _, cx| this.send_demand(cx)));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            key,
            row,
            container_id,
            details,
            details_view,
            history_view,
            container,
            editor,
            parts,
            history: None,
            history_raw: None,
            history_version: 0,
            history_seen: false,
            midnights: SmallVec::new(),
            native_path: None,
            refresh: None,
            this: cx.weak_entity(),
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
            host: host.clone(),
            _subs: subs,
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            render_count: Cell::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
            notify_count: Cell::new(0),
        };
        this.apply_snapshot(&snap, ComponentMask::ALL, cx);
        this
    }

    /// The chart of the first placed widget of `metric` that draws one.
    pub fn chart_of(&self, metric: &str, cx: &App) -> Option<Entity<ChartView>> {
        let c = self.container.read(cx);
        c.layout()
            .widgets
            .iter()
            .filter(|w| w.content.metric_key().is_some_and(|k| k.as_str() == metric))
            .find_map(|w| c.widget(w.id)?.read(cx).chart().cloned())
    }

    /// The page's chart: Power on a system battery, Percentage on any other device.
    pub fn chart(&self, cx: &App) -> Option<Entity<ChartView>> {
        self.chart_of(
            if self.is_system {
                METRIC_POWER
            } else {
                METRIC_PERCENT
            },
            cx,
        )
    }

    /// The 24-hour charge history chart, present for a system battery with at least 2 points.
    pub fn history_chart(&self) -> Option<&Entity<ChartView>> {
        self.history.as_ref()
    }

    /// Where the history chart's vertical grid lines are: fractions of the plot width.
    pub fn history_midnights(&self) -> Vec<f32> {
        self.midnights.iter().map(|(x, _)| *x).collect()
    }

    /// True when the page shows power (the battery powers the system).
    pub fn is_system(&self) -> bool {
        self.is_system
    }

    /// The page's widget container.
    pub fn container(&self) -> &Entity<WidgetContainer> {
        &self.container
    }

    /// The editor around the container (Edit Widgets).
    pub fn editor(&self) -> &Entity<WidgetEditor> {
        &self.editor
    }

    /// The Details widget's view.
    pub fn details_view(&self) -> &Entity<DetailsView> {
        &self.details_view
    }

    /// The battery history widget's view.
    pub fn history_view(&self) -> &Entity<MemoryPartView> {
        &self.history_view
    }

    /// The container id, `page.bat:...`.
    pub fn container_id(&self) -> &SharedString {
        &self.container_id
    }

    /// The page is the shown page (`set_active`).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// This page's demand part while it is active: batteries ride on the sensors bit, the device
    /// metric history at 1 h, 24 h and 7 d, and what the placed widgets read.
    pub fn demand_part(&self, cx: &App) -> Demand {
        let mut demand = Demand {
            any_surface_visible: true,
            sensors: true,
            device_history_range: page_history_request(self.settings.range_ns),
            ..Demand::default()
        };
        add_demand(&mut demand, &self.container.read(cx).demand_part());
        demand
    }

    fn send_demand(&self, cx: &mut App) {
        if self.active {
            let (id, window, part) = (self.id, self.window, self.demand_part(cx));
            self.demand.update(cx, |d, cx| d.set(id, window, part, cx));
        }
    }

    fn notify(&self, cx: &mut Context<Self>) {
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.notify_count.set(self.notify_count.get() + 1);
        cx.notify();
    }

    /// A part's height, or the battery's kind or history shown state changed: the flow layout
    /// places the parts again.
    fn place_again(&mut self, cx: &mut Context<Self>) {
        self.container.update(cx, |c, cx| {
            c.place_again(cx);
        });
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

    /// Brings the page's kind, history, Details rows and header up to date. Only a header
    /// change renders the page.
    fn apply_snapshot(&mut self, snap: &Snapshot, mask: ComponentMask, cx: &mut Context<Self>) {
        let details = mask.contains(ComponentMask::DEVICE_DETAILS);
        let devices = mask.contains(ComponentMask::DEVICES);
        let record = battery_of(snap, self.row.idx);
        if details {
            let system = record.is_some_and(|b| b.power_supply);
            if system != self.is_system {
                self.is_system = system;
                self.parts.system.set(system);
                self.place_again(cx);
            }
            self.native_path = record.map(|b| Arc::clone(&b.native_path));
            let history = record.and_then(|b| b.history.clone());
            // The worker drops the history when it reconnects: ask again.
            if self.active && self.history_seen && history.is_none() {
                self.request_history();
            }
            self.history_seen = history.is_some();
            self.sync_history(history, cx);
            let kind = record.map_or("", |b| &*b.kind);
            let word = page_words(cx).battery_kind(kind).clone();
            let age_ns = self
                .hub
                .read(cx)
                .now_ns()
                .saturating_sub(snap.device_details.header.observed_end);
            if update_battery_details(&mut self.details, record, &word, age_ns) {
                self.sync_details(cx);
            }
        }
        if (details || devices) && self.sync_header(snap, cx) {
            self.notify(cx);
        }
        // The battery went away: an edit ends as Cancel does.
        if devices
            && !snap
                .devices
                .entries
                .iter()
                .any(|e| e.idx == self.row.idx && e.present)
        {
            self.editor.update(cx, |e, cx| e.cancel(cx));
        }
    }

    /// Hands the Details view what the grid shows; it notifies only when that changed.
    fn sync_details(&mut self, cx: &mut Context<Self>) {
        let shown = DetailsShown::of(&self.details, strings_version(cx));
        self.details_view.update(cx, |v, cx| v.set_shown(shown, cx));
    }

    /// Sends the history fetch for this battery. Only a system battery shows the history.
    fn request_history(&self) {
        let (true, Some(native_path)) = (self.is_system, self.native_path.clone()) else {
            return;
        };
        let Some(engine) = self.host.client() else {
            return;
        };
        match engine.send(Command::Device(DeviceCommand::BatteryHistory {
            native_path,
        })) {
            Ok(()) => {}
            Err(CommandError::Busy) => {
                tracing::warn!(target: "tasklens::ui", "battery history not queued: busy");
            }
            Err(_) => tracing::warn!(target: "tasklens::ui", "battery history not queued"),
        }
    }

    /// What the history view draws: the words, the midnights and the history's version. The
    /// view renders again only when this changed.
    fn history_shown(&self) -> PartShown {
        PartShown {
            texts: self.words.days.to_vec(),
            numbers: std::iter::once(self.history_version)
                .chain(
                    self.midnights
                        .iter()
                        .flat_map(|(x, d)| [u64::from(x.to_bits()), u64::from(*d)]),
                )
                .collect(),
            flags: vec![self.history.is_some()],
            ..PartShown::default()
        }
    }

    /// Creates, updates or drops the history chart for the record's history. A new `Arc`
    /// rebuilds the series (the chart's geometry once); the same `Arc` changes nothing.
    fn sync_history(&mut self, history: Option<Arc<[(i64, f32)]>>, cx: &mut Context<Self>) {
        let history = history.filter(|h| self.is_system && h.len() >= 2);
        let Some(history) = history else {
            self.history_raw = None;
            self.midnights.clear();
            if self.history.take().is_some() {
                self.history_changed(cx);
            }
            return;
        };
        if self.history.is_some()
            && self
                .history_raw
                .as_ref()
                .is_some_and(|r| Arc::ptr_eq(r, &history))
        {
            return;
        }
        self.history_version += 1;
        let now_s = unix_now_s();
        let id = series(self.row.idx, MetricId::BATTERY_PCT);
        let view = Arc::new(history_series(id, &history, now_s, self.history_version));
        let right_ns = view.samples.last().map_or(0, |s| s.t_ns);
        self.midnights = local_midnights(now_s)
            .into_iter()
            .map(|(at, day)| {
                let left_ns = right_ns.saturating_sub(HISTORY_RANGE_NS);
                let x = (at as u64 * NS_PER_S).saturating_sub(left_ns) as f64;
                ((x / HISTORY_RANGE_NS as f64) as f32, day)
            })
            .collect();
        self.history_raw = Some(history);
        let spec = self.history_spec(cx);
        match &self.history {
            Some(chart) => chart.update(cx, |v, cx| {
                v.set_spec(spec, cx);
                v.series.clear();
                v.series.push(Some(view));
                cx.notify();
            }),
            None => {
                let labels = history_labels(&self.words);
                let hub = self.hub.clone();
                self.history = Some(cx.new(|cx| {
                    let mut v = ChartView::new(spec, labels, &hub, cx).with_newest_marks();
                    v.series.clear();
                    v.series.push(Some(view));
                    v.freeze(cx);
                    v
                }));
            }
        }
        self.history_changed(cx);
    }

    /// The history chart, its midnights or its words changed: the view draws them, and the
    /// flow layout places the parts again when the view appeared or went.
    fn history_changed(&mut self, cx: &mut Context<Self>) {
        let on = self.history.is_some();
        let flipped = self.parts.history_shown.get() != on;
        self.parts.history_shown.set(on);
        let shown = self.history_shown();
        self.history_view.update(cx, |v, cx| v.set_shown(shown, cx));
        if flipped {
            let parts = battery_page_parts(on);
            self.editor.update(cx, |e, _| e.set_page_parts(parts));
            self.place_again(cx);
        }
    }

    fn history_spec(&self, cx: &App) -> ChartSpec {
        let theme = current(cx).clone();
        let view = PageSettingsView::of(&self.settings);
        let at: Arc<[f32]> = self.midnights.iter().map(|(x, _)| *x).collect();
        history_spec(self.row.idx, at, &view, &theme)
    }

    fn sync_header(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let Some(entry) = snap.devices.entries.iter().find(|e| e.idx == self.row.idx) else {
            return false;
        };
        let header = header_text(page_words(cx), DeviceKind::Battery, 0, entry, snap);
        if header == self.header {
            return false;
        }
        let word = header.0.clone();
        self.editor.update(cx, |e, _| e.set_page_name(word));
        self.header = header;
        true
    }

    /// Rebuilds the history chart's spec and labels from the current settings, theme and words.
    fn rebuild_history(&mut self, cx: &mut Context<Self>) {
        if let Some(history) = self.history.clone() {
            let spec = self.history_spec(cx);
            let labels = history_labels(&self.words);
            history.update(cx, |v, cx| {
                v.set_spec(spec, cx);
                v.set_labels(labels, cx);
            });
        }
    }

    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        // A saved layout (Apply, Reset Page) shows when the page is not in Edit Widgets.
        if !self.editor.read(cx).is_editing() {
            let layout = page_layout(&self.container_id, cx);
            self.container.update(cx, |c, cx| c.set_layout(layout, cx));
        }
        let versions = section_versions(cx);
        if versions == self.settings_versions {
            return;
        }
        self.settings_versions = versions;
        let new = PageSettings::read(&self.key, cx);
        if new == self.settings {
            return;
        }
        let range_moved = new.range_ns != self.settings.range_ns;
        self.settings = new;
        self.rebuild_history(cx);
        if range_moved {
            // The header switch shows the new range.
            self.send_demand(cx);
            self.notify(cx);
        }
    }

    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let version = metrics(cx).style_version;
        if version == self.style_version {
            return;
        }
        self.style_version = version;
        self.rebuild_history(cx);
    }

    /// New strings: the Details words, the day names and the header.
    fn sync_words(&mut self, cx: &mut Context<Self>) {
        self.details.refresh_strings(cx);
        if self.words.refresh(cx) {
            self.rebuild_history(cx);
            if self.history.is_some() {
                self.history_changed(cx);
            }
        }
        let snap = Arc::clone(self.hub.read(cx).current());
        self.apply_snapshot(&snap, ComponentMask::DEVICE_DETAILS, cx);
        self.sync_details(cx);
        self.sync_header(&snap, cx);
        self.notify(cx);
    }

    /// "Edit Widgets…" in the page menu.
    fn on_edit_start(
        &mut self,
        _: &PerfWidgetEditStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        start_page_edit(&self.editor, &self.hub, window, cx);
    }

    /// "Reset Page" in the page menu (ADR-019 phase 3 "Reset").
    fn on_reset_page(
        &mut self,
        _: &PerfWidgetResetPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        reset_page(&self.container_id, self.header.0.clone(), window, cx);
    }
}

/// The page parts Add Widget lists on a Battery page: Details, and the history while the
/// battery has one to show.
fn battery_page_parts(history: bool) -> Vec<FixedWidget> {
    let mut parts = vec![FixedWidget::Details];
    if history {
        parts.push(FixedWidget::BatteryHistory);
    }
    parts
}

/// What the history view draws (UI-PRF-035): the chart box, `sp4`-free (the flow places the gap),
/// with the day labels under it at the grid line of each midnight.
fn history_draw(this: WeakEntity<BatteryPage>) -> PartDraw {
    Rc::new(move |_, cx| {
        let page = this.upgrade()?;
        let page = page.read(cx);
        let history = page.history.clone()?;
        let m = *metrics(cx);
        let theme = current(cx).clone();
        // The labels sit under the 1 px inset of the plot, at the grid line of each midnight.
        let mut days = div()
            .relative()
            .w_full()
            .h(m.text.xs * m.line_height)
            .mx(px(1.0))
            .text_color(theme.text_secondary);
        *days.text_style() = t_xs(&m);
        for (n, (x, day)) in page.midnights.iter().enumerate() {
            let label = div()
                .absolute()
                .left(relative(*x))
                .child(page.words.days[usize::from(*day).min(6)].clone());
            let id = history_day_label_id(&page.container_id);
            days = days.child(if n == 0 {
                label.tl_selector(move || id)
            } else {
                label
            });
        }
        let chart_id = history_chart_id(&page.container_id);
        Some(
            v_flex()
                .w_full()
                .child(
                    div()
                        .tl_selector(move || chart_id)
                        .w_full()
                        .h(px(HISTORY_H * m.ui_scale))
                        .child(history.cached(StyleRefinement::default().size_full())),
                )
                .child(days)
                .into_any_element(),
        )
    })
}

impl DevicePage for BatteryPage {
    fn hub(&self) -> Option<&Entity<SnapshotHub>> {
        Some(&self.hub)
    }

    fn key(&self) -> &DeviceKey {
        &self.key
    }

    fn header(&self) -> (SharedString, SharedString) {
        self.header.clone()
    }

    fn toggles(&self, _: &mut Window, _: &mut App) -> SmallVec<[AnyElement; 2]> {
        SmallVec::new()
    }

    fn graphs(&self, _: &mut Window, _: &mut App) -> AnyElement {
        // The charts are widgets of the container.
        div().into_any_element()
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

    fn widget_editor(&self) -> Option<&Entity<WidgetEditor>> {
        Some(&self.editor)
    }

    fn set_active(&mut self, active: bool, cx: &mut App) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.send_demand(cx);
            self.request_history();
            let this = self.this.clone();
            self.refresh = Some(cx.spawn(async move |cx| {
                loop {
                    cx.background_executor().timer(HISTORY_REFRESH).await;
                    if this.update(cx, |page, _| page.request_history()).is_err() {
                        break;
                    }
                }
            }));
        } else {
            self.refresh = None;
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
        #[cfg(any(test, debug_assertions, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        let handle = window.window_handle();
        if self.window != Some(handle) {
            self.window = Some(handle);
            self.send_demand(cx);
        }
        let m = *metrics(cx);
        let theme = current(cx).clone();
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_edit_start))
            .on_action(cx.listener(Self::on_reset_page))
            .child(
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
    fn history_series_marks_gaps_over_an_hour() {
        let id = series(DeviceIdx(1), MetricId::BATTERY_PCT);
        let now = 1_000_000;
        let points = [
            (now - 7200, 80.0),
            (now - 6000, 79.0),
            (now - 600, 70.0),
            (now - 100_000, 50.0),
        ];
        let view = history_series(id, &points, now, 3);
        let flags: Vec<bool> = view
            .samples
            .iter()
            .map(|s| s.flags.contains(SampleFlags::GAP_BEFORE))
            .collect();
        // The point from 28 h ago is out of range. 6000 s to 600 s is a gap; 600 s to now is not.
        assert_eq!(flags, [false, false, true, false]);
        assert_eq!(view.samples.last().map(|s| s.value), Some(70.0));
        assert_eq!(
            view.samples.last().map(|s| s.t_ns),
            Some(now as u64 * NS_PER_S)
        );
        assert_eq!(view.version, 3);
    }

    #[test]
    fn at_most_one_midnight_in_24_hours() {
        for now in [1_700_000_000, 1_711_846_800, 1_730_000_000] {
            let found = local_midnights(now);
            assert!(found.len() <= 1, "{now}: {found:?}");
            assert!(found.iter().all(|(at, day)| *at <= now && *day < 7));
        }
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
