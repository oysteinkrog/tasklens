//! The Battery page as a widget container (ADR-019 phase 3, UI-PRF-061 "Phase 3: device
//! pages", `tasklens-hzire.9`): the default flow layout places the Power chart, the charge
//! history and Details where 1.3.0 drew them, a peripheral shows Percentage and no history,
//! the history appears and goes with its data, Edit Widgets adds, moves and applies, Cancel restores, a sample that changes
//! nothing shown renders no page or container and a changed sample notifies neither, the
//! Details widget shows the 1.3.0 rows, Select Pages and Edit Widgets exclude each other, and
//! Reset Page saves the default.

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, Focusable as _, IntoElement, ParentElement as _,
    Pixels, Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div,
    px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{
    BatteryDetails, ComponentHeader, DeviceDetailsSnapshot, InvField, SeriesSet,
};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::host::HostContext;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_chart::chart::ChartView;
use tl_ui_kit::components::kv_grid;
use tl_ui_overview::widget::container::WidgetContainer;
use tl_ui_overview::widget::details_widget::{DetailsShown, DetailsView};
use tl_ui_overview::widget::edit::{TOOLBAR_ID, WidgetEditor, saved_layout};
use tl_ui_overview::widget::grid::{PAGE_GUTTER, PAGE_ROW_UNIT};
use tl_ui_overview::widget::metric_widget::widget_id;
use tl_ui_perf::performance::page::{
    DevicePage, PAGE_BODY_ID, PAGE_HEADER_H, PAGE_PAD, PerfSidebar,
};
use tl_ui_perf::performance::sensors::BatteryPage;
use tl_ui_perf::performance::sidebar::DeviceSidebar;
use tl_ui_perf::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_perf_core::core::actions::{
    PerfEnterEditMode, PerfWidgetEditStart, PerfWidgetResetPage, WidgetSource,
};
use tl_ui_perf_core::core::details::KeyValueModel;
use tl_ui_settings::layout::METRIC_POWER;
use tl_ui_settings::layout::{FixedWidget, WidgetContent};
use tl_ui_settings::layout_pages::METRIC_PERCENT;
use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

const BAT_KEY: &str = "bat:BAT0";
const PAGE: &str = "page.bat:BAT0";
const BAT: u16 = 30;

struct Host {
    page: Entity<BatteryPage>,
    sidebar: Entity<DeviceSidebar>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(
                self.page
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .child(
                div()
                    .absolute()
                    .w(px(0.))
                    .h(px(0.))
                    .overflow_hidden()
                    .child(self.sidebar.clone()),
            )
    }
}

fn entry(idx: u16, identity: DeviceIdentity, key: &str, name: &str) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from(name),
        present: true,
        parent: None,
        persistent_key: Arc::from(key),
        net_kind: None,
        hot_c: None,
    }
}

/// The battery.
fn catalog() -> DeviceCatalog {
    DeviceCatalog {
        version: 1,
        entries: Arc::from(vec![entry(
            BAT,
            DeviceIdentity::Battery {
                native_path: Arc::from("BAT0"),
            },
            BAT_KEY,
            "BAT0",
        )]),
    }
}

/// `n` history points every 30 minutes, newest 10 minutes ago.
fn history(n: i64) -> Arc<[(i64, f32)]> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    (0..n)
        .map(|i| (now - 600 - 1800 * (n - 1 - i), 50. + i as f32))
        .collect::<Vec<_>>()
        .into()
}

/// The battery record: a system battery drawing `watts`, with a history of `points`.
fn record(system: bool, watts: f32, points: i64) -> BatteryDetails {
    let mut b = crate::battery_page::record("Battery", system, "Discharging");
    b.energy_rate_w = InvField {
        value: watts,
        validity: Validity::Valid,
    };
    b.history = (points > 0).then(|| history(points));
    b
}

/// The battery's charge and power series: one valid sample each.
fn series_set(watts: f32, version: u64) -> Arc<SeriesSet> {
    let one = |metric: MetricId, v: f32| {
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(BAT),
                metric,
            },
            version,
            period_ns: 100_000_000,
            samples: Arc::from([Sample::valid(now_boot_ns(), v)]),
        })
    };
    let mut series = vec![
        one(MetricId::BATTERY_PCT, 85.),
        one(MetricId::BATTERY_POWER, watts),
    ];
    series.sort_by_key(|v| v.id);
    Arc::new(SeriesSet {
        header: ComponentHeader {
            version,
            ..ComponentHeader::default()
        },
        series: series.into(),
        slow: Arc::from([]),
    })
}

/// Publishes `b` and the series for `watts`.
fn publish(engine: &FakeEngine, b: BatteryDetails, watts: f32) {
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog());
        let mut d = DeviceDetailsSnapshot::clone(&s.device_details);
        d.header.version += 1;
        d.batteries = Arc::from(vec![(DeviceIdx(BAT), b)]);
        s.device_details = Arc::new(d);
        s.series = series_set(watts, s.series.header.version + 1);
    });
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    page: Entity<BatteryPage>,
    sidebar: Entity<DeviceSidebar>,
    vcx: &'a mut VisualTestContext,
    _host: Entity<Host>,
    _driver: Entity<AnimationDriver>,
}

/// One window of `w × h` with the Battery page filling it: a system battery drawing 12.4 W
/// with 3 history points when `system`, else a peripheral.
fn open(cx: &mut TestAppContext, w: f32, h: f32, system: bool) -> Rig<'_> {
    let engine = FakeEngine::new();
    publish(
        &engine,
        record(system, 12.4, if system { 3 } else { 0 }),
        12.4,
    );
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui_base::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        let mut settings = Settings::default();
        settings.motion.smooth_graphs = false;
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let (host, vcx) = cx.add_window_view(move |_, cx| {
        let sidebar = cx.new(|cx| DeviceSidebar::new(hub.clone(), demand.clone(), cx));
        cx.set_global(PerfSidebar(sidebar.downgrade()));
        let key = DeviceKey(Arc::from(BAT_KEY));
        let row = DeviceRow {
            key: key.clone(),
            kind: DeviceKind::Battery,
            idx: DeviceIdx(BAT),
            generation: 0,
            net_class: None,
            fan_connected: None,
        };
        let page =
            cx.new(|cx| BatteryPage::new(&HostContext::local(cx), key, row, hub, demand, cx));
        Host { page, sidebar }
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(w), px(h)));
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    let (page, sidebar) = vcx.read_entity(&host, |h, _| (h.page.clone(), h.sidebar.clone()));
    vcx.update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    draw(vcx);
    Rig {
        engine,
        page,
        sidebar,
        vcx,
        _host: host,
        _driver: driver,
    }
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

fn leak(id: String) -> &'static str {
    Box::leak(id.into_boxed_str())
}

fn near(a: Pixels, b: Pixels) -> bool {
    (f32::from(a) - f32::from(b)).abs() < 1.
}

fn source(device: &str, metric: &str) -> WidgetSource {
    WidgetSource {
        device: Arc::from(device),
        metric: Arc::from(metric),
    }
}

impl Rig<'_> {
    fn page_action(&mut self, action: &dyn gpui_kit::Action) {
        let page = self.page.clone();
        self.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            focus.dispatch_action(action, window, cx);
        });
        draw(self.vcx);
    }

    fn container(&mut self) -> Entity<WidgetContainer> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| page.read(cx).container().clone())
    }

    fn editor(&mut self) -> Entity<WidgetEditor> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| page.read(cx).editor().clone())
    }

    fn details(&mut self) -> Entity<DetailsView> {
        let page = self.page.clone();
        self.vcx
            .update(|_, cx| page.read(cx).details_view().clone())
    }

    fn bounds_of(&mut self, id: String) -> Bounds<Pixels> {
        let id = leak(id);
        self.vcx.debug_bounds(id).expect(id)
    }

    /// The ids of the layout's widgets by content, in list order.
    fn ids(&mut self) -> Vec<(u32, WidgetContent)> {
        let container = self.container();
        self.vcx.update(|_, cx| {
            container
                .read(cx)
                .layout()
                .widgets
                .iter()
                .map(|w| (w.id, w.content.clone()))
                .collect()
        })
    }

    /// The id of the metric widget on `metric`.
    fn metric_id(&mut self, metric: &str) -> u32 {
        self.ids()
            .into_iter()
            .find(|(_, c)| c.metric_key().is_some_and(|k| k.as_str() == metric))
            .map(|(id, _)| id)
            .expect("a widget on the metric")
    }

    /// The id of the battery history widget.
    fn history_widget(&mut self) -> u32 {
        self.ids()
            .into_iter()
            .find(|(_, c)| matches!(c, WidgetContent::Fixed(FixedWidget::BatteryHistory)))
            .map(|(id, _)| id)
            .expect("the battery history widget")
    }

    /// How many midnights the history shows.
    fn midnights(&mut self) -> usize {
        let page = self.page.clone();
        self.vcx
            .update(|_, cx| page.read(cx).history_midnights().len())
    }

    /// The id of the Details widget.
    fn details_widget(&mut self) -> u32 {
        self.ids()
            .into_iter()
            .find(|(_, c)| matches!(c, WidgetContent::Fixed(FixedWidget::Details)))
            .map(|(id, _)| id)
            .expect("the Details widget")
    }

    fn edit(&mut self) -> Entity<WidgetEditor> {
        self.page_action(&PerfWidgetEditStart);
        let editor = self.editor();
        assert!(self.vcx.read_entity(&editor, |e, _| e.is_editing()));
        editor
    }

    /// The chart entities of the metric widgets `ids`.
    fn charts<const N: usize>(&mut self, ids: [u32; N]) -> [Entity<ChartView>; N] {
        let container = self.container();
        self.vcx.update(|_, cx| {
            let c = container.read(cx);
            ids.map(|id| {
                c.widget(id)
                    .and_then(|w| w.read(cx).chart().cloned())
                    .expect("a chart widget")
            })
        })
    }

    /// New readings and series: the battery drawing `watts`.
    fn publish_sample(&mut self, watts: f32) {
        self.engine.publish_with(|s| {
            let version = s.series.header.version + 1;
            let mut d = DeviceDetailsSnapshot::clone(&s.device_details);
            d.header.version += 1;
            let mut b = d.batteries[0].1.clone();
            b.energy_rate_w = InvField {
                value: watts,
                validity: Validity::Valid,
            };
            d.batteries = Arc::from(vec![(DeviceIdx(BAT), b)]);
            s.device_details = Arc::new(d);
            s.series = series_set(watts, version);
        });
        draw(self.vcx);
    }

    /// The same battery record again: nothing the page shows changes.
    fn publish_read(&mut self, _watts: f32) {
        self.engine.publish_with(|s| {
            let mut d = DeviceDetailsSnapshot::clone(&s.device_details);
            d.header.version += 1;
            s.device_details = Arc::new(d);
        });
        draw(self.vcx);
    }

    /// A new history of `points` points, or none.
    fn publish_history(&mut self, points: i64) {
        self.engine.publish_with(|s| {
            let mut d = DeviceDetailsSnapshot::clone(&s.device_details);
            d.header.version += 1;
            let mut b = d.batteries[0].1.clone();
            b.history = (points > 0).then(|| history(points));
            d.batteries = Arc::from(vec![(DeviceIdx(BAT), b)]);
            s.device_details = Arc::new(d);
        });
        draw(self.vcx);
    }
}

/// A system battery with a history: the default flow layout puts the 150 px Power chart, the
/// history `sp4` under it and the Details widget `details_gap` under that, as the 1.3.0 page
/// drew them, at the full body width under the header row. Percentage takes no space.
#[gpui_kit::test]
fn page_widget_default_flow_places_1_3_0_parts(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., true);
    let (power, details) = (r.metric_id(METRIC_POWER), r.details_widget());
    let history = r.history_widget();
    let p = r.bounds_of(widget_id(PAGE, power));
    let h = r.bounds_of(widget_id(PAGE, history));
    let d = r.bounds_of(widget_id(PAGE, details));
    let header = r.bounds_of("perf-page-header".to_owned());
    assert!(near(p.size.height, px(150.)), "{p:?}");
    assert!(near(h.top(), p.bottom() + px(8.)), "sp4: {p:?} {h:?}");
    assert!(
        near(d.top(), h.bottom() + px(24.)),
        "details_gap: {h:?} {d:?}"
    );
    // The history is the 150 px chart and the day label row.
    assert!(h.size.height > px(150.), "{h:?}");
    assert!(near(header.top(), px(PAGE_PAD)), "{header:?}");
    assert!(near(header.size.height, px(PAGE_HEADER_H)));
    assert!(near(p.top(), px(PAGE_PAD + PAGE_HEADER_H + 8.)), "{p:?}");
    assert!(near(p.left(), px(PAGE_PAD)), "{p:?}");
    assert!(near(p.size.width, px(1200. - 2. * PAGE_PAD)), "{p:?}");
    // The Percentage widget is on the page and takes no space on a system battery.
    let percent = r.metric_id(METRIC_PERCENT);
    assert!(
        r.vcx
            .debug_bounds(Box::leak(widget_id(PAGE, percent).into_boxed_str()))
            .is_none()
    );
    let chart_id = leak(tl_ui_perf::performance::sensors::history_chart_id(PAGE));
    let chart = r.vcx.debug_bounds(chart_id).expect("the history chart");
    assert!(near(chart.size.height, px(150.)), "{chart:?}");
    let label = leak(tl_ui_perf::performance::sensors::history_day_label_id(PAGE));
    assert!(r.vcx.debug_bounds(label).is_some() || r.midnights() == 0);
}

/// Any other battery shows Percentage and Details, and no history.
#[gpui_kit::test]
fn page_widget_peripheral_flow(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., false);
    let (percent, details) = (r.metric_id(METRIC_PERCENT), r.details_widget());
    let p = r.bounds_of(widget_id(PAGE, percent));
    let d = r.bounds_of(widget_id(PAGE, details));
    assert!(near(p.size.height, px(150.)), "{p:?}");
    assert!(
        near(d.top(), p.bottom() + px(24.)),
        "details_gap: {p:?} {d:?}"
    );
    let power = r.metric_id(METRIC_POWER);
    assert!(
        r.vcx.debug_bounds(leak(widget_id(PAGE, power))).is_none(),
        "Power takes no space on a peripheral"
    );
    let chart_id = leak(tl_ui_perf::performance::sensors::history_chart_id(PAGE));
    assert!(r.vcx.debug_bounds(chart_id).is_none(), "no history");
}

/// The history takes space when the battery has 2 points and gives it back when it has fewer;
/// Add Widget lists it only while it has something to show.
#[gpui_kit::test]
fn page_widget_history_comes_and_goes(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., true);
    let (power, details) = (r.metric_id(METRIC_POWER), r.details_widget());
    let history = r.history_widget();
    let editor = r.editor();
    let listed = |r: &mut Rig| {
        r.vcx.update(|_, cx| {
            let e = editor.read(cx);
            e.fixed_adds(cx)
                .iter()
                .any(|p| &*p.name == "battery_history")
        })
    };
    // On the page already, so not offered.
    assert!(!listed(&mut r));
    r.publish_history(1);
    assert!(r.vcx.debug_bounds(leak(widget_id(PAGE, history))).is_none());
    let p = r.bounds_of(widget_id(PAGE, power));
    let d = r.bounds_of(widget_id(PAGE, details));
    assert!(
        near(d.top(), p.bottom() + px(24.)),
        "Details moves up: {p:?} {d:?}"
    );
    r.publish_history(2);
    let h = r.bounds_of(widget_id(PAGE, history));
    let d = r.bounds_of(widget_id(PAGE, details));
    assert!(
        near(d.top(), h.bottom() + px(24.)),
        "Details moves down: {h:?} {d:?}"
    );
}

/// The Details widget shows the rows the 1.3.0 grid showed: the same model, in order.
#[gpui_kit::test]
fn page_widget_details_rows_match_grid(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., true);
    let page = r.page.clone();
    let details = r.details();
    r.vcx.update(|_, cx| {
        let model: &KeyValueModel = page.read(cx).details();
        let strings = cx
            .try_global::<tl_ui_base::i18n::Strings>()
            .map(|s| s.strings_version);
        let expected = DetailsShown::of(model, strings);
        assert_eq!(details.read(cx).shown(), &expected);
        let labels: Vec<_> = model.rows.iter().map(|r| r.label).collect();
        assert_eq!(
            labels,
            tl_ui_perf::performance::sensors::BATTERY_DETAIL_ROWS.to_vec()
        );
    });
    // The Percentage row has a value.
    let active = r.vcx.update(|_, cx| {
        page.read(cx)
            .details()
            .rows
            .iter()
            .find(|row| row.label == tl_ui_perf::performance::sensors::BAT_ROW_PERCENTAGE)
            .map(|row| (row.value.to_string(), row.hidden))
    });
    let (value, hidden) = active.expect("the Percentage row");
    assert!(!hidden && !value.is_empty(), "{value:?}");
}

/// The entities the sample tests count: the page, its container, the Details view and the
/// Power chart with their widget ids.
struct Counted {
    page: Entity<BatteryPage>,
    container: Entity<WidgetContainer>,
    details: Entity<DetailsView>,
    ids: [u32; 1],
    charts: [Entity<ChartView>; 1],
}

impl Counted {
    fn new(r: &mut Rig) -> Self {
        let ids = [r.metric_id(METRIC_POWER)];
        Self {
            page: r.page.clone(),
            container: r.container(),
            details: r.details(),
            ids,
            charts: r.charts(ids),
        }
    }

    /// Notifies of the page, the container and the Details view, and the container's
    /// placements.
    fn notifies(&self, r: &mut Rig) -> [u32; 4] {
        r.vcx.update(|_, cx| {
            [
                self.page.read(cx).notify_count.get(),
                self.container.read(cx).notify_count.get(),
                self.container.read(cx).relayout_count.get(),
                self.details.read(cx).notify_count.get(),
            ]
        })
    }

    /// Renders of the page, the container, the Details view and the two charts.
    fn renders(&self, r: &mut Rig) -> [u32; 4] {
        r.vcx.update(|_, cx| {
            [
                self.page.read(cx).render_count.get(),
                self.container.read(cx).render_count.get(),
                self.details.read(cx).render_count.get(),
                self.charts[0].read(cx).render_count.get(),
            ]
        })
    }

    /// The entity ids of the two chart widgets.
    fn entities(&self, r: &mut Rig) -> [Option<gpui_kit::EntityId>; 1] {
        r.vcx.update(|_, cx| {
            let c = self.container.read(cx);
            self.ids.map(|id| c.widget(id).map(|w| w.entity_id()))
        })
    }

    /// The series versions each chart holds.
    fn versions(&self, r: &mut Rig) -> [Vec<Option<u64>>; 1] {
        r.vcx.update(|_, cx| {
            self.charts.clone().map(|c| {
                c.read(cx)
                    .resolved()
                    .iter()
                    .map(|s| s.as_ref().map(|s| s.version))
                    .collect::<Vec<_>>()
            })
        })
    }
}

/// The render contract (UI-PRF-017, UI-PRF-019, ADR-019) on the page: a sample that changes
/// nothing shown renders nothing at all. Not the page, the container, a widget, a chart or
/// Details renders or notifies, and no widget entity is made or dropped.
#[gpui_kit::test]
fn page_widget_sample_renders_no_page_or_container(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., true);
    let c = Counted::new(&mut r);
    let entities = c.entities(&mut r);
    assert!(entities.iter().all(Option::is_some));

    let (n0, r0) = (c.notifies(&mut r), c.renders(&mut r));
    r.publish_read(1200.);
    assert_eq!(
        c.notifies(&mut r),
        n0,
        "an unchanged sample notifies nothing"
    );
    assert_eq!(c.renders(&mut r), r0, "an unchanged sample renders nothing");
    assert_eq!(c.entities(&mut r), entities, "same widget entities");
}

/// The ADR-019 gate for a sample that moves the charts' series and a Details value: the page
/// and the container never notify and place nothing, no widget entity is made or dropped, the
/// two charts render the new series, and of the page's own views only Details notifies. The
/// same sample again renders nothing.
///
/// The page and the container are not held to zero renders in the frame of a changed sample.
/// GPUI 0.3.7 marks every ancestor of a notifying view dirty (`Window::mark_view_dirty`,
/// crw.34), and D-022 rules out a GPUI fork, so a chart that draws a new series renders its
/// ancestors once. ADR-019 ("Render contract") sets the gate to "never notify" for this case;
/// those renders do fixed work, and the container places nothing.
#[gpui_kit::test]
fn page_widget_changed_sample_notifies_no_page_or_container(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., true);
    let c = Counted::new(&mut r);
    let entities = c.entities(&mut r);
    assert!(entities.iter().all(Option::is_some));

    let (n0, r0, v0) = (c.notifies(&mut r), c.renders(&mut r), c.versions(&mut r));
    r.publish_sample(8.5);
    let (n1, r1) = (c.notifies(&mut r), c.renders(&mut r));
    assert_ne!(c.versions(&mut r), v0, "the charts hold the new series");
    assert_eq!(n1[0], n0[0], "the page does not notify");
    assert_eq!(n1[1], n0[1], "the container does not notify");
    assert_eq!(n1[2], n0[2], "the container places nothing");
    assert_eq!(n1[3], n0[3] + 1, "the Details view notifies once");
    assert!(r1[0] - r0[0] <= 1, "page renders {}", r1[0] - r0[0]);
    assert!(r1[1] - r0[1] <= 1, "container renders {}", r1[1] - r0[1]);
    assert!(r1[3] > r0[3], "the Power chart renders the new series");
    assert_eq!(c.entities(&mut r), entities, "same widget entities");
    assert_eq!(r.charts(c.ids), c.charts, "same chart entities");

    // The same sample again renders nothing.
    let (n2, r2) = (c.notifies(&mut r), c.renders(&mut r));
    r.publish_read(8.5);
    assert_eq!(c.notifies(&mut r), n2, "a repeated sample notifies nothing");
    assert_eq!(c.renders(&mut r), r2, "a repeated sample renders nothing");
}

/// The header stays at the top and the body under it scrolls; at a small window the body is
/// the container's scroll area.
#[gpui_kit::test]
fn page_widget_header_stays_body_scrolls(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 380., true);
    let header = r.bounds_of("perf-page-header".to_owned());
    let body = r.bounds_of(PAGE_BODY_ID.to_owned());
    assert!(near(header.top(), px(PAGE_PAD)));
    assert!(body.top() >= header.bottom(), "{header:?} {body:?}");
    let container = r.container();
    let scrolls = r.vcx.update(|_, cx| container.read(cx).placement().scrolls);
    assert!(scrolls, "the parts are taller than the body");
    r.vcx.update(|_, cx| {
        container.update(cx, |c, _| c.set_scroll_offset(px(120.)));
    });
    draw(r.vcx);
    let after = r.bounds_of("perf-page-header".to_owned());
    assert_eq!(after, header, "the header does not move");
    let offset = r.vcx.update(|_, cx| container.read(cx).scroll_offset());
    assert!(offset > px(0.), "{offset:?}");
}

/// A short page leaves space under its parts: the rows never grow, flow or grid.
#[gpui_kit::test]
fn page_widget_no_fill_on_short_page(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 1400., true);
    let container = r.container();
    let (content, area) = r.vcx.update(|_, cx| {
        let c = container.read(cx);
        (c.placement().content_h, c.area().height)
    });
    assert!(content < area, "{content:?} {area:?}");
    // Pinned in Edit Widgets and applied, the rows keep the unit.
    let editor = r.edit();
    let id = r.metric_id(METRIC_POWER);
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| {
        e.select(id, cx);
        e.move_selected(0, 1, cx)
    })));
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    let grid = r.vcx.update(|_, cx| container.read(cx).placement().grid);
    assert_eq!(grid.row, PAGE_ROW_UNIT);
    assert_eq!(grid.gutter, PAGE_GUTTER);
    assert_eq!(grid.cols, 12);
}

/// Edit Widgets on the page: the flow layout pins to the 12-column grid, a widget is added
/// and moved, and Apply saves the page's entry, which the page then shows.
#[gpui_kit::test]
fn page_widget_edit_add_move_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., true);
    let editor = r.edit();
    let toolbar = r.bounds_of(TOOLBAR_ID.to_owned());
    let header = r.bounds_of("perf-page-header".to_owned());
    assert!(
        toolbar.top() >= header.bottom(),
        "the toolbar is under the header"
    );
    let container = r.container();
    let pinned = r.vcx.update(|_, cx| container.read(cx).layout().clone());
    assert!(pinned.preset.is_none(), "pinned to the grid");
    // The 150 px charts pin to 7 rows of the page grid, the full width.
    let active = r.metric_id(METRIC_POWER);
    let cell = pinned
        .widgets
        .iter()
        .find(|w| w.id == active)
        .and_then(|w| w.cell)
        .expect("a cell");
    assert_eq!((cell.col, cell.cols, cell.rows), (0, 12, 7), "{cell:?}");
    let added = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source(BAT_KEY, METRIC_POWER), cx)))
        .expect("added");
    draw(r.vcx);
    let entity = r
        .vcx
        .update(|_, cx| container.read(cx).widget(added).map(|w| w.entity_id()));
    assert!(entity.is_some(), "the new widget has an entity");
    let moved = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.move_selected(6, 0, cx)));
    assert!(moved);
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    let saved = r
        .vcx
        .update(|_, cx| saved_layout(PAGE, cx))
        .expect("the page's entry");
    assert!(saved.preset.is_none());
    let w = saved
        .widgets
        .iter()
        .find(|w| w.id == added)
        .expect("the added widget");
    assert_eq!(w.cell.expect("a cell").col, 6);
    let shown = r.vcx.update(|_, cx| container.read(cx).layout().clone());
    assert_eq!(shown, saved, "the page shows the saved layout");
    assert_eq!(
        r.vcx
            .update(|_, cx| container.read(cx).widget(added).map(|w| w.entity_id())),
        entity,
        "Apply keeps the entity"
    );
    // The old page keys never existed; the entry is the only change. The settings section of
    // the box layouts is untouched.
    r.vcx.update(|_, cx| {
        let perf = &TlSettings::settings(cx).performance;
        assert!(perf.layouts.known("overview.fan").is_none());
    });
}

/// Cancel puts the flow layout back, saves nothing and creates no widget entity again.
#[gpui_kit::test]
fn page_widget_edit_cancel_restores(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., true);
    let container = r.container();
    let before = r.vcx.update(|_, cx| container.read(cx).layout().clone());
    let active = r.metric_id(METRIC_POWER);
    let entity = r
        .vcx
        .update(|_, cx| container.read(cx).widget(active).map(|w| w.entity_id()));
    let editor = r.edit();
    r.vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.select(active, cx);
            assert!(e.remove_selected(cx));
        })
    });
    draw(r.vcx);
    r.vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
    draw(r.vcx);
    let after = r.vcx.update(|_, cx| container.read(cx).layout().clone());
    assert_eq!(after, before);
    assert!(r.vcx.update(|_, cx| saved_layout(PAGE, cx)).is_none());
    assert_eq!(
        r.vcx
            .update(|_, cx| container.read(cx).widget(active).map(|w| w.entity_id())),
        entity
    );
}

/// Select Pages and Edit Widgets exclude each other: starting Edit Widgets ends Select Pages,
/// and while the page edits, Select Pages does not start.
#[gpui_kit::test]
fn page_widget_select_pages_and_edit_exclusive(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., true);
    let sidebar = r.sidebar.clone();
    let enter = |r: &mut Rig| {
        let sidebar = sidebar.clone();
        r.vcx.update(|window, cx| {
            let focus = sidebar.read(cx).focus_handle(cx);
            focus.dispatch_action(&PerfEnterEditMode, window, cx);
        });
        draw(r.vcx);
    };
    enter(&mut r);
    assert!(r.vcx.read_entity(&sidebar, |s, _| s.edit_mode()));
    let editor = r.edit();
    assert!(
        !r.vcx.read_entity(&sidebar, |s, _| s.edit_mode()),
        "Select Pages ended"
    );
    enter(&mut r);
    assert!(
        !r.vcx.read_entity(&sidebar, |s, _| s.edit_mode()),
        "Select Pages stays off"
    );
    r.vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
    draw(r.vcx);
    enter(&mut r);
    assert!(r.vcx.read_entity(&sidebar, |s, _| s.edit_mode()));
}

/// Reset Page with a flow entry saves the default at once: the entry goes and the page shows
/// its built-in default. With no entry it does nothing.
#[gpui_kit::test]
fn page_widget_reset_clears_page_entry(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., true);
    let container = r.container();
    let default = r.vcx.update(|_, cx| container.read(cx).layout().clone());
    // A flow entry without the Details widget.
    let mut flow = default.clone();
    flow.widgets
        .retain(|w| !matches!(w.content, WidgetContent::Fixed(FixedWidget::Details)));
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| d.settings.performance.layouts.set(flow.clone()));
    });
    draw(r.vcx);
    assert_eq!(
        r.vcx.update(|_, cx| container.read(cx).layout().clone()),
        flow,
        "the page shows its entry"
    );
    r.page_action(&PerfWidgetResetPage);
    assert!(r.vcx.update(|_, cx| saved_layout(PAGE, cx)).is_none());
    assert_eq!(
        r.vcx.update(|_, cx| container.read(cx).layout().clone()),
        default
    );
    // Nothing to reset: nothing changes.
    r.page_action(&PerfWidgetResetPage);
    assert!(r.vcx.update(|_, cx| saved_layout(PAGE, cx)).is_none());
}

/// Add Widget on the page lists the fan's own metrics and, once Details is removed, the
/// Details part.
#[gpui_kit::test]
fn page_widget_add_lists_own_device_and_parts(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900., true);
    let editor = r.edit();
    let (groups, parts) = r.vcx.update(|_, cx| {
        let e = editor.read(cx);
        (e.source_groups(cx), e.fixed_adds(cx))
    });
    assert_eq!(groups.len(), 1);
    assert_eq!(&*groups[0].key, BAT_KEY);
    assert!(groups[0].add_group.is_none());
    assert!(parts.is_empty(), "Details is on the page already");
    let details = r.details_widget();
    r.vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.select(details, cx);
            assert!(e.remove_selected(cx));
        })
    });
    let parts = r.vcx.update(|_, cx| editor.read(cx).fixed_adds(cx));
    assert_eq!(parts.len(), 1);
    assert_eq!(&*parts[0].name, "details");
    let id = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add_fixed("details", cx)))
        .expect("added back");
    draw(r.vcx);
    let container = r.container();
    assert!(r.vcx.update(|_, cx| container.read(cx).fixed(id).is_some()));
}
