//! The Fan page as a widget container (ADR-019 phase 3, UI-PRF-061 "Phase 3: device pages",
//! `tasklens-hzire.9`): the default flow layout places the two charts and Details where 1.3.0
//! drew them, Edit Widgets adds, moves and applies, Cancel restores, a sample that changes
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
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::system::{FanControlMode, Metric, SensorKind, SensorReading};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_chart::chart::ChartView;
use tl_ui_kit::components::kv_grid;
use tl_ui_overview::widget::container::WidgetContainer;
use tl_ui_overview::widget::details_widget::{DetailsShown, DetailsView, details_id};
use tl_ui_overview::widget::edit::{TOOLBAR_ID, WidgetEditor, saved_layout};
use tl_ui_overview::widget::grid::{PAGE_GUTTER, PAGE_ROW_UNIT};
use tl_ui_overview::widget::metric_widget::widget_id;
use tl_ui_perf::performance::page::{
    DevicePage, PAGE_BODY_ID, PAGE_HEADER_H, PAGE_PAD, PerfSidebar,
};
use tl_ui_perf::performance::sensors::FanPage;
use tl_ui_perf::performance::sidebar::DeviceSidebar;
use tl_ui_perf::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_perf_core::core::actions::{
    PerfEnterEditMode, PerfWidgetEditStart, PerfWidgetResetPage, WidgetSource,
};
use tl_ui_perf_core::core::details::KeyValueModel;
use tl_ui_settings::layout::METRIC_TEMP;
use tl_ui_settings::layout::{FixedWidget, WidgetContent};
use tl_ui_settings::layout_pages::METRIC_SPEED;
use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

const FAN_KEY: &str = "fan:nct6797::1";
const PAGE: &str = "page.fan:nct6797::1";
const FAN: u16 = 10;
const TEMP: u16 = 20;

struct Host {
    page: Entity<FanPage>,
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

/// One fan and the one temperature sensor of its chip, which pairs with it by default.
fn catalog() -> DeviceCatalog {
    DeviceCatalog {
        version: 1,
        entries: Arc::from(vec![
            entry(
                FAN,
                DeviceIdentity::Fan {
                    chip: Arc::from("nct6797"),
                    index: 1,
                },
                FAN_KEY,
                "fan",
            ),
            entry(
                TEMP,
                DeviceIdentity::Sensor {
                    chip: Arc::from("nct6797"),
                    label: Arc::from("temp1"),
                },
                "sensor:nct6797::temp1",
                "temp",
            ),
        ]),
    }
}

/// The fan at `rpm` and the sensor at `celsius`.
fn sensors(rpm: f32, celsius: f32) -> Arc<[(DeviceIdx, SensorReading)]> {
    Arc::from([
        (
            DeviceIdx(FAN),
            SensorReading {
                device: DeviceIdx(FAN),
                kind: SensorKind::Fan,
                value: Metric::valid(rpm),
                pwm_pct: Metric::valid(45.),
                fan_control: Some(FanControlMode::Automatic),
                fan_connected: true,
                fan_max_rpm: Some(1800),
            },
        ),
        (
            DeviceIdx(TEMP),
            SensorReading {
                device: DeviceIdx(TEMP),
                kind: SensorKind::Temperature,
                value: Metric::valid(celsius),
                pwm_pct: Metric::with(Validity::Unsupported),
                fan_control: None,
                fan_connected: false,
                fan_max_rpm: None,
            },
        ),
    ])
}

/// The page's series: the fan's RPM and PWM and the sensor's temperature, one valid sample each.
fn series_set(rpm: f32, celsius: f32, version: u64) -> Arc<SeriesSet> {
    let one = |device: u16, metric: MetricId, v: f32| {
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(device),
                metric,
            },
            version,
            period_ns: 100_000_000,
            samples: Arc::from([Sample::valid(now_boot_ns(), v)]),
        })
    };
    Arc::new(SeriesSet {
        header: ComponentHeader {
            version,
            ..ComponentHeader::default()
        },
        series: vec![
            one(FAN, MetricId::FAN_RPM, rpm),
            one(FAN, MetricId::FAN_PWM, 45.),
            one(TEMP, MetricId::SENSOR_TEMP, celsius),
        ]
        .into(),
        slow: Arc::from([]),
    })
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    page: Entity<FanPage>,
    sidebar: Entity<DeviceSidebar>,
    vcx: &'a mut VisualTestContext,
    _host: Entity<Host>,
    _driver: Entity<AnimationDriver>,
}

/// One window of `w × h` with the Fan page filling it, the fan at 1200 RPM, the sensor at 46 °C.
fn open(cx: &mut TestAppContext, w: f32, h: f32) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog());
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        sys.sensors = sensors(1200., 46.);
        let version = sys.header.version;
        s.series = series_set(1200., 46., version);
    });
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
        let key = DeviceKey(Arc::from(FAN_KEY));
        let row = DeviceRow {
            key: key.clone(),
            kind: DeviceKind::Fan,
            idx: DeviceIdx(FAN),
            generation: 0,
            net_class: None,
            fan_connected: Some(true),
        };
        let page = cx.new(|cx| FanPage::new(key, row, hub, demand, cx));
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

    /// New readings and series: the fan at `rpm` and the sensor at 46 °C.
    fn publish_sample(&mut self, rpm: f32) {
        self.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.sensors = sensors(rpm, 46.);
            let version = sys.header.version;
            s.series = series_set(rpm, 46., version);
        });
        draw(self.vcx);
    }

    /// New readings only: the fan at `rpm`. The series stay.
    fn publish_read(&mut self, rpm: f32) {
        self.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.sensors = sensors(rpm, 46.);
        });
        draw(self.vcx);
    }
}

/// The page's default flow layout puts the two 150 px charts and the Details widget where the
/// 1.3.0 page drew them: `sp4` between the charts, `details_gap` above Details, the full body
/// width, under the header row.
#[gpui_kit::test]
fn page_widget_default_flow_places_1_3_0_parts(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    let (active, io, details) = (
        r.metric_id(METRIC_SPEED),
        r.metric_id(METRIC_TEMP),
        r.details_widget(),
    );
    let a = r.bounds_of(widget_id(PAGE, active));
    let t = r.bounds_of(widget_id(PAGE, io));
    let d = r.bounds_of(widget_id(PAGE, details));
    let header = r.bounds_of("perf-page-header".to_owned());
    assert!(near(a.size.height, px(150.)), "{a:?}");
    assert!(near(t.size.height, px(150.)), "{t:?}");
    assert!(
        near(t.top(), a.bottom() + px(8.)),
        "sp4 between: {a:?} {t:?}"
    );
    assert!(
        near(d.top(), t.bottom() + px(24.)),
        "details_gap: {t:?} {d:?}"
    );
    // The 1.3.0 frame: padding, header, sp4, then the graph area at the page's content width.
    assert!(near(header.top(), px(PAGE_PAD)), "{header:?}");
    assert!(near(header.size.height, px(PAGE_HEADER_H)));
    assert!(near(a.top(), px(PAGE_PAD + PAGE_HEADER_H + 8.)), "{a:?}");
    assert!(near(a.left(), px(PAGE_PAD)), "{a:?}");
    assert!(near(a.size.width, px(1200. - 2. * PAGE_PAD)), "{a:?}");
    // The Details heading and grid are in the Details widget.
    let heading = r.bounds_of("perf-page-details-heading".to_owned());
    assert!(near(heading.top(), d.top()), "{heading:?} {d:?}");
    let grid = r.bounds_of(details_id(PAGE));
    assert!(d.contains(&grid.origin));
}

/// The Details widget shows the rows the 1.3.0 grid showed: the same model, in order.
#[gpui_kit::test]
fn page_widget_details_rows_match_grid(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
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
            tl_ui_perf::performance::sensors::FAN_DETAIL_ROWS.to_vec()
        );
    });
    // The Fan speed row has a value.
    let active = r.vcx.update(|_, cx| {
        page.read(cx)
            .details()
            .rows
            .iter()
            .find(|row| row.label == tl_ui_perf::performance::sensors::ROW_SPEED)
            .map(|row| (row.value.to_string(), row.hidden))
    });
    let (value, hidden) = active.expect("the Fan speed row");
    assert!(!hidden && !value.is_empty(), "{value:?}");
}

/// The entities the sample tests count: the page, its container, the Details view and the
/// Fan speed and Temperature charts with their widget ids.
struct Counted {
    page: Entity<FanPage>,
    container: Entity<WidgetContainer>,
    details: Entity<DetailsView>,
    ids: [u32; 2],
    charts: [Entity<ChartView>; 2],
}

impl Counted {
    fn new(r: &mut Rig) -> Self {
        let ids = [r.metric_id(METRIC_SPEED), r.metric_id(METRIC_TEMP)];
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
    fn renders(&self, r: &mut Rig) -> [u32; 5] {
        r.vcx.update(|_, cx| {
            [
                self.page.read(cx).render_count.get(),
                self.container.read(cx).render_count.get(),
                self.details.read(cx).render_count.get(),
                self.charts[0].read(cx).render_count.get(),
                self.charts[1].read(cx).render_count.get(),
            ]
        })
    }

    /// The entity ids of the two chart widgets.
    fn entities(&self, r: &mut Rig) -> [Option<gpui_kit::EntityId>; 2] {
        r.vcx.update(|_, cx| {
            let c = self.container.read(cx);
            self.ids.map(|id| c.widget(id).map(|w| w.entity_id()))
        })
    }

    /// The series versions each chart holds.
    fn versions(&self, r: &mut Rig) -> [Vec<Option<u64>>; 2] {
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
    let mut r = open(cx, 1200., 900.);
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
    let mut r = open(cx, 1200., 900.);
    let c = Counted::new(&mut r);
    let entities = c.entities(&mut r);
    assert!(entities.iter().all(Option::is_some));

    let (n0, r0, v0) = (c.notifies(&mut r), c.renders(&mut r), c.versions(&mut r));
    r.publish_sample(1500.);
    let (n1, r1) = (c.notifies(&mut r), c.renders(&mut r));
    assert_ne!(c.versions(&mut r), v0, "the charts hold the new series");
    assert_eq!(n1[0], n0[0], "the page does not notify");
    assert_eq!(n1[1], n0[1], "the container does not notify");
    assert_eq!(n1[2], n0[2], "the container places nothing");
    assert_eq!(n1[3], n0[3] + 1, "the Details view notifies once");
    assert!(r1[0] - r0[0] <= 1, "page renders {}", r1[0] - r0[0]);
    assert!(r1[1] - r0[1] <= 1, "container renders {}", r1[1] - r0[1]);
    assert!(r1[3] > r0[3], "the Fan speed chart renders the new series");
    assert!(
        r1[4] > r0[4],
        "the Temperature chart renders the new series"
    );
    assert_eq!(c.entities(&mut r), entities, "same widget entities");
    assert_eq!(r.charts(c.ids), c.charts, "same chart entities");

    // The same sample again renders nothing.
    let (n2, r2) = (c.notifies(&mut r), c.renders(&mut r));
    r.publish_read(1500.);
    assert_eq!(c.notifies(&mut r), n2, "a repeated sample notifies nothing");
    assert_eq!(c.renders(&mut r), r2, "a repeated sample renders nothing");
}

/// The header stays at the top and the body under it scrolls; at a small window the body is
/// the container's scroll area.
#[gpui_kit::test]
fn page_widget_header_stays_body_scrolls(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 380.);
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
    let mut r = open(cx, 1200., 1400.);
    let container = r.container();
    let (content, area) = r.vcx.update(|_, cx| {
        let c = container.read(cx);
        (c.placement().content_h, c.area().height)
    });
    assert!(content < area, "{content:?} {area:?}");
    // Pinned in Edit Widgets and applied, the rows keep the unit.
    let editor = r.edit();
    let id = r.metric_id(METRIC_SPEED);
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
    let mut r = open(cx, 1200., 900.);
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
    let active = r.metric_id(METRIC_SPEED);
    let cell = pinned
        .widgets
        .iter()
        .find(|w| w.id == active)
        .and_then(|w| w.cell)
        .expect("a cell");
    assert_eq!((cell.col, cell.cols, cell.rows), (0, 12, 7), "{cell:?}");
    let added = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source(FAN_KEY, METRIC_SPEED), cx)))
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
    let mut r = open(cx, 1200., 900.);
    let container = r.container();
    let before = r.vcx.update(|_, cx| container.read(cx).layout().clone());
    let active = r.metric_id(METRIC_SPEED);
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
    let mut r = open(cx, 1200., 900.);
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
    let mut r = open(cx, 1200., 900.);
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
    let mut r = open(cx, 1200., 900.);
    let editor = r.edit();
    let (groups, parts) = r.vcx.update(|_, cx| {
        let e = editor.read(cx);
        (e.source_groups(cx), e.fixed_adds(cx))
    });
    assert_eq!(groups.len(), 1);
    assert_eq!(&*groups[0].key, FAN_KEY);
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

/// The page's widgets draw the same in the flow layout, in Edit Widgets and after Apply
/// (tasklens-hzire.10, the look sweep).
#[gpui_kit::test]
fn page_fan_look_survives_edit_and_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    let container = r.container();
    let editor = r.editor();
    super::page_look::sweep(r.vcx, &container, &editor, "fan");
}

/// The Fan speed and Temperature charts draw in Violet with the fill at 85%, as the 1.3.0 Fan
/// page did (UI-PRF-005), not in the GPU colour (tasklens-hzire.10, visual check).
#[gpui_kit::test]
fn page_fan_charts_use_the_fan_colour(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    let ids = [r.metric_id(METRIC_SPEED), r.metric_id(METRIC_TEMP)];
    let charts = r.charts(ids);
    let violet = tl_ui_base::theme::palette::accent_color("violet").expect("violet");
    r.vcx.update(|_, cx| {
        for (chart, id) in charts.iter().zip(ids) {
            let spec = &chart.read(cx).spec;
            assert!(!spec.series.is_empty(), "widget {id} draws a series");
            for s in &spec.series {
                assert_eq!(s.line, violet, "widget {id}");
            }
            assert_eq!(
                spec.series[0].fill,
                Some(violet.opacity(0.85)),
                "widget {id}"
            );
        }
    });
}
