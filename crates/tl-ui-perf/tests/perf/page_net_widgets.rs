//! The Network page as a widget container (ADR-019 phase 3, UI-PRF-061, `tasklens-hzire.8`):
//! the default flow layout draws the 1.3.0 parts in their 1.3.0 places, each adapter has its
//! own layout, an adapter that is gone shows its widgets as unavailable and keeps its entry,
//! every adapter metric shows in the forms it offers, Edit Widgets adds and applies, and a
//! sample renders no page or container.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, EntityId, IntoElement, ParentElement as _,
    Pixels, Render, StyleRefinement, Styled as _, Subscription, TestAppContext, VisualTestContext,
    Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId};
use tl_model::snapshot::{InvField, NetDetails, WifiInfo};
use tl_model::system::{LinkState, Metric, NetTotals};
use tl_model::validity::Validity;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::host::HostContext;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::theme::current;
use tl_ui_chart::chart::{ChartView, LegendStyle};
use tl_ui_kit::components::kv_grid;
use tl_ui_overview::widget::container::{WidgetContainer, WidgetView};
use tl_ui_overview::widget::details_widget::details_id;
use tl_ui_overview::widget::edit::saved_layout;
use tl_ui_overview::widget::fixed_widget::FixedWidgetView;
use tl_ui_overview::widget::metric_widget::{MetricWidget, widget_id};
use tl_ui_overview::widget::source::Availability;
use tl_ui_perf::performance::memory_views::part_id;
use tl_ui_perf::performance::net::{
    NetPage, NetSettingsDesktop, PART_SETTINGS, SETTINGS_LINK_ID, SettingsDesktop,
};
use tl_ui_perf::performance::page::{DevicePage as _, PAGE_PAD};
use tl_ui_perf::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_perf_core::core::actions::WidgetSource;
use tl_ui_settings::layout::{
    ContainerLayout, DeviceSelector, FixedWidget, FormSet, LAYOUT_VERSION, MetricKey,
    SourceSelector, WidgetCell, WidgetContent, WidgetOptions, WidgetSetting,
};
use tl_ui_settings::layout_pages::page_container;
use tl_ui_settings::settings::{
    AppearanceSettings, PerformanceSettings, Settings, ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

const MBPS: f64 = 125_000.;
const GIB: u64 = 1 << 30;

/// Adapter A (wired, `enp38s0`) and adapter B (Wi-Fi, `wlan0`).
const NET_A: DeviceIdx = DeviceIdx(2);
const NET_B: DeviceIdx = DeviceIdx(3);

fn identity(n: u8) -> DeviceIdentity {
    DeviceIdentity::Net {
        netns_ino: 1,
        ifindex: u32::from(n) + 1,
        name: Arc::from(if n == 1 { "enp38s0" } else { "wlan0" }),
    }
}

fn idx(n: u8) -> DeviceIdx {
    if n == 1 { NET_A } else { NET_B }
}

fn class(n: u8) -> NetKind {
    if n == 1 {
        NetKind::Wired
    } else {
        NetKind::Wireless
    }
}

/// The persistent key of adapter `n` (1 is A, 2 is B), as the sidebar and the page use it.
fn key(n: u8) -> DeviceKey {
    DeviceKey::from_identity(&identity(n)).expect("adapter key")
}

fn container(n: u8) -> String {
    page_container(&key(n).0)
}

fn entry(n: u8, present: bool) -> DeviceEntry {
    DeviceEntry {
        idx: idx(n),
        identity: identity(n),
        generation: 0,
        display_name: Arc::from(if n == 1 { "enp38s0" } else { "wlan0" }),
        present,
        parent: None,
        persistent_key: key(n).0,
        net_kind: Some(class(n)),
        hot_c: None,
    }
}

/// Adapter `n`'s record at 1000 Mbps, download at `rx_mbps`.
fn record(n: u8, rx_mbps: f64) -> NetTotals {
    NetTotals {
        device: idx(n),
        rx_bps: Metric::valid(rx_mbps * MBPS),
        tx_bps: Metric::valid(0.4 * MBPS),
        rx_total: 12 * GIB,
        tx_total: 3 * GIB,
        link_speed_mbps: Metric::valid(1000),
        state: LinkState::Up,
    }
}

fn text(s: &str) -> InvField<Arc<str>> {
    InvField {
        value: Arc::from(s),
        validity: Validity::Valid,
    }
}

/// Adapter `n`'s details; B has its Wi-Fi link.
fn details(n: u8) -> NetDetails {
    NetDetails {
        type_label: Arc::from(if n == 1 { "Ethernet" } else { "Wi-Fi" }),
        connection: None,
        adapter_name: text("Intel I350 Gigabit Network Connection"),
        driver: text("igb"),
        mac: text("aa:bb:cc:dd:ee:ff"),
        ifindex: u32::from(n) + 1,
        link_speed_mbps: InvField {
            value: 1000,
            validity: Validity::Valid,
        },
        state: LinkState::Up,
        addresses: Arc::from([Arc::<str>::from("192.168.1.10/24")]),
        kind: class(n),
        wifi: (n == 2).then(|| WifiInfo {
            ssid: Arc::from("Home"),
            signal_dbm: Some(-52),
            signal_pct: 78,
            rx_mbps: Some(866.),
            tx_mbps: Some(866.),
            freq_mhz: 5180,
            channel: Some(36),
            security: Arc::from("WPA2"),
        }),
    }
}

/// Publishes both adapters (A present or not), their records with A's download at `rx_mbps`,
/// and their details.
fn publish(engine: &FakeEngine, a_present: bool, rx_mbps: f64) {
    engine.publish_with(|s| {
        let mut entries = vec![entry(2, true)];
        entries.insert(0, entry(1, a_present));
        s.devices = Arc::new(DeviceCatalog {
            version: s.devices.version + 1,
            entries: Arc::from(entries),
        });
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        let mut nets = vec![(NET_B, record(2, 3.))];
        if a_present {
            nets.insert(0, (NET_A, record(1, rx_mbps)));
        }
        sys.nets = Arc::from(nets);
        let d = Arc::make_mut(&mut s.device_details);
        d.header.version += 1;
        d.nets = Arc::from(vec![(NET_A, details(1)), (NET_B, details(2))]);
    });
}

/// Publishes a sample that carries only new values: the adapter records, A's download at
/// `rx_mbps`. The catalog and the details stay as they are.
fn publish_values(engine: &FakeEngine, rx_mbps: f64) {
    engine.publish_with(|s| {
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.nets = Arc::from(vec![(NET_A, record(1, rx_mbps)), (NET_B, record(2, 3.))]);
    });
}

/// The page, cached, so it renders only when it notifies.
struct Host(Entity<NetPage>);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.0
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    host: HostContext,
    /// Adapter A's page, shown in the window.
    page: Entity<NetPage>,
    vcx: &'a mut VisualTestContext,
    _driver: Entity<AnimationDriver>,
}

fn row(n: u8) -> DeviceRow {
    DeviceRow {
        key: key(n),
        kind: DeviceKind::Net,
        idx: idx(n),
        generation: 0,
        net_class: Some(class(n)),
        fan_connected: None,
    }
}

/// A 1200 × 900 window with adapter A's page, built with `perf` as the loaded performance
/// settings, on a desktop with `desktop`'s network settings tool.
fn open(
    cx: &mut TestAppContext,
    perf: PerformanceSettings,
    desktop: Option<SettingsDesktop>,
) -> Rig<'_> {
    let engine = FakeEngine::new();
    publish(&engine, true, 12.);
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
        cx.set_global(NetSettingsDesktop(desktop));
        let mut settings = Settings {
            performance: perf,
            ..Settings::default()
        };
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
    let host = HostContext::from_parts(Some(Arc::clone(&client)), None, None, None, None);
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let (hub2, demand2, host2) = (hub.clone(), demand.clone(), host.clone());
    let (view, vcx) = cx.add_window_view(move |_, cx| {
        let page = cx.new(|cx| NetPage::new(&host2, key(1), row(1), hub2, demand2, cx));
        Host(page)
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1200.), px(900.)));
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    let page = vcx.read_entity(&view, |h, _| h.0.clone());
    vcx.update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    draw(vcx);
    Rig {
        engine,
        hub,
        demand,
        host,
        page,
        vcx,
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

fn near(a: Pixels, b: f32) -> bool {
    (f32::from(a) - b).abs() < 1.
}

fn metric(n: u8, key: &str, forms: FormSet, cell: Option<WidgetCell>) -> WidgetSetting {
    WidgetSetting {
        id: 0,
        content: WidgetContent::Metric(SourceSelector {
            device: DeviceSelector::Key(self::key(n).0),
            metric: MetricKey::new(key),
        }),
        forms,
        cell,
        options: WidgetOptions::default(),
        device_name: None,
    }
}

fn fixed(kind: FixedWidget, cell: Option<WidgetCell>) -> WidgetSetting {
    WidgetSetting {
        content: WidgetContent::Fixed(kind),
        ..metric(1, "", FormSet::empty(), cell)
    }
}

fn cell(row: u16) -> Option<WidgetCell> {
    Some(WidgetCell {
        col: 0,
        row,
        cols: 12,
        rows: 9,
    })
}

/// A grid layout of page `n` with `widgets`, ids from 1.
fn grid_layout(n: u8, widgets: Vec<WidgetSetting>) -> ContainerLayout {
    let widgets: Vec<WidgetSetting> = widgets
        .into_iter()
        .enumerate()
        .map(|(i, w)| WidgetSetting {
            id: i as u32 + 1,
            ..w
        })
        .collect();
    ContainerLayout {
        container: container(n),
        version: LAYOUT_VERSION,
        preset: None,
        next_id: widgets.len() as u32 + 1,
        widgets,
    }
}

/// The metric keys of a layout's metric widgets, in list order.
fn metrics_of(l: &ContainerLayout) -> Vec<String> {
    l.widgets
        .iter()
        .filter_map(|w| w.content.metric_key().map(|k| k.as_str().to_owned()))
        .collect()
}

impl Rig<'_> {
    fn container(&mut self) -> Entity<WidgetContainer> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| page.read(cx).container().clone())
    }

    fn bounds(&mut self, id: String) -> Bounds<Pixels> {
        let id = leak(id);
        self.vcx.debug_bounds(id).expect(id)
    }

    fn metric_id(&mut self, key: &str) -> u32 {
        let container = self.container();
        self.vcx.update(|_, cx| {
            container
                .read(cx)
                .layout()
                .widgets
                .iter()
                .find(|w| w.content.metric_key().is_some_and(|k| k.as_str() == key))
                .map(|w| w.id)
                .expect("a widget")
        })
    }

    /// Builds the page of adapter `n` again (as the window does when it first shows it),
    /// without showing it.
    fn build_page(&mut self, n: u8) -> Entity<NetPage> {
        let (hub, demand, host) = (self.hub.clone(), self.demand.clone(), self.host.clone());
        self.vcx
            .update(|_, cx| cx.new(|cx| NetPage::new(&host, key(n), row(n), hub, demand, cx)))
    }

    fn saved(&mut self, n: u8) -> Option<ContainerLayout> {
        self.vcx.update(|_, cx| saved_layout(&container(n), cx))
    }

    fn update_perf(&mut self, f: impl FnOnce(&mut PerformanceSettings)) {
        self.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| f(&mut d.settings.performance));
        });
        draw(self.vcx);
    }
}

/// The default flow layout draws the 1.3.0 page at 1200 × 900: the Throughput chart (240 px)
/// across the body, `details_gap` above Details and `sp6` above the Network settings link. The
/// chart carries the 1.3.0 heading, legend, dashed upload line and summed value. Nothing is
/// written: an adapter page has no old key to convert.
#[gpui_kit::test]
fn page_net_flow_matches_1_3_0(cx: &mut TestAppContext) {
    let mut r = open(
        cx,
        PerformanceSettings::default(),
        Some(SettingsDesktop::Kde),
    );
    assert!(r.saved(1).is_none(), "no entry");
    let c = container(1);
    let traffic = r.metric_id("traffic");
    let t = r.bounds(widget_id(&c, traffic));
    let details = r.bounds(details_id(&c));
    let link = r.bounds(part_id(&c, PART_SETTINGS));
    let body_w = 1200. - 2. * PAGE_PAD;
    assert!(near(t.size.width, body_w), "{t:?}");
    assert!(near(t.size.height, 240.), "{t:?}");
    assert!(near(details.top() - t.bottom(), 24.), "details_gap");
    assert!(near(link.top() - details.bottom(), 16.), "sp6");
    assert!(r.vcx.debug_bounds(SETTINGS_LINK_ID).is_some(), "the link");
    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        let chart = page.read(cx).throughput(cx).expect("Throughput chart");
        let v = chart.read(cx);
        assert_eq!(v.labels.heading.as_deref(), Some("Throughput"));
        assert_eq!(v.labels.range_word.as_ref(), "Throughput");
        assert!(v.labels.show_axis_max);
        let legend: Vec<(&str, LegendStyle)> = v
            .labels
            .legend
            .iter()
            .map(|(s, style)| (s.as_ref(), *style))
            .collect();
        assert_eq!(
            legend,
            [
                ("Download", LegendStyle::Solid),
                ("Upload", LegendStyle::Dashed)
            ]
        );
        let ids: Vec<MetricId> = v.spec.series.iter().map(|s| s.id.metric).collect();
        assert_eq!(ids, [MetricId::NET_RX, MetricId::NET_TX]);
        let line = current(cx).graph.net_line;
        let (down, up) = (&v.spec.series[0], &v.spec.series[1]);
        assert!(down.fill.is_some() && !down.dashed, "download filled");
        assert!(up.fill.is_none() && up.dashed, "upload dashed");
        assert_eq!(up.line.a, line.a * 0.7, "upload at 70%");
        assert!(v.spec.hover);
    });
}

/// Without a network settings tool the link takes no space: nothing follows Details, and Add
/// Widget does not offer it.
#[gpui_kit::test]
fn page_net_no_settings_tool_no_link(cx: &mut TestAppContext) {
    let r = open(cx, PerformanceSettings::default(), None);
    let c = container(1);
    assert!(
        r.vcx
            .debug_bounds(leak(part_id(&c, PART_SETTINGS)))
            .is_none()
    );
    assert!(r.vcx.debug_bounds(SETTINGS_LINK_ID).is_none());
    let page = r.page.clone();
    r.vcx.update(|window, cx| {
        let editor = page.read(cx).editor().clone();
        editor.update(cx, |e, cx| e.start(window, cx));
        let adds: Vec<String> = editor
            .read(cx)
            .fixed_adds(cx)
            .into_iter()
            .map(|a| a.name.to_string())
            .collect();
        assert!(!adds.iter().any(|a| a == "net_settings"), "{adds:?}");
    });
}

/// Each adapter has its own layout: a saved grid layout of adapter A does not reach adapter B,
/// which shows the built-in default, and a change to B's entry leaves A's as it was.
#[gpui_kit::test]
fn page_net_two_devices_separate_layouts(cx: &mut TestAppContext) {
    let a = grid_layout(
        1,
        vec![
            metric(1, "total_rx", FormSet::VALUE | FormSet::LABEL, cell(0)),
            metric(1, "rx", FormSet::BAR | FormSet::VALUE, cell(9)),
        ],
    );
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(a.clone());
    let mut r = open(cx, perf, None);
    let page_b = r.build_page(2);
    draw(r.vcx);
    let (container_a, container_b) = (r.container(), {
        let p = page_b.clone();
        r.vcx.update(|_, cx| p.read(cx).container().clone())
    });
    let (shown_a, shown_b) = r.vcx.update(|_, cx| {
        (
            container_a.read(cx).layout().clone(),
            container_b.read(cx).layout().clone(),
        )
    });
    assert_eq!(shown_a, a);
    assert_eq!(metrics_of(&shown_b), ["traffic"]);
    assert!(shown_b.preset.is_some(), "B shows its built-in flow layout");
    assert!(r.saved(2).is_none(), "nothing written for B");
    assert!(shown_b.widgets.iter().all(|w| match &w.content {
        WidgetContent::Metric(s) => s.device == DeviceSelector::Key(key(2).0),
        _ => true,
    }));
    let b = grid_layout(
        2,
        vec![metric(
            2,
            "signal",
            FormSet::BAR | FormSet::VALUE | FormSet::LABEL,
            cell(0),
        )],
    );
    r.update_perf(|p| p.layouts.set(b.clone()));
    assert_eq!(r.saved(1), Some(a.clone()));
    assert_eq!(r.saved(2), Some(b.clone()));
    let (shown_a, shown_b) = r.vcx.update(|_, cx| {
        (
            container_a.read(cx).layout().clone(),
            container_b.read(cx).layout().clone(),
        )
    });
    assert_eq!(shown_a, a);
    assert_eq!(shown_b, b);
}

/// An adapter that is gone shows its page's widgets as unavailable ("Not connected") and its
/// entry stays; when it comes back the saved layout shows again with the same entities.
#[gpui_kit::test]
fn page_net_device_back_restores_layout(cx: &mut TestAppContext) {
    let layout = grid_layout(
        1,
        vec![
            metric(
                1,
                "traffic",
                FormSet::CHART | FormSet::VALUE | FormSet::LABEL,
                cell(0),
            ),
            metric(1, "total_tx", FormSet::VALUE | FormSet::LABEL, cell(9)),
        ],
    );
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(layout.clone());
    let mut r = open(cx, perf, None);
    let container = r.container();
    let state = |r: &mut Rig| {
        r.vcx.update(|_, cx| {
            let c = container.read(cx);
            [1, 2].map(|id| {
                let w = c.widget(id).expect("placed");
                let entity = w.entity_id();
                let w = w.read(cx);
                (entity, w.availability(), w.texts()[3].to_string())
            })
        })
    };
    let before = state(&mut r);
    assert!(before.iter().all(|(_, a, _)| *a == Availability::Available));
    publish(&r.engine, false, 12.);
    draw(r.vcx);
    let gone = state(&mut r);
    for (_, a, reason) in &gone {
        assert_eq!(*a, Availability::Gone);
        assert_eq!(reason, "Not connected");
    }
    assert_eq!(r.saved(1), Some(layout.clone()), "the entry is kept");
    publish(&r.engine, true, 12.);
    draw(r.vcx);
    let back = state(&mut r);
    for (b, a) in back.iter().zip(&before) {
        assert_eq!(b.1, Availability::Available);
        assert_eq!(b.0, a.0, "same entity");
    }
}

/// The forms of each widget of [`page_net_every_metric_any_form`], in list order.
fn any_form_widgets(n: u8) -> Vec<WidgetSetting> {
    use FormSet as F;
    let mut list: Vec<(&str, FormSet)> = vec![
        ("rx", F::CHART | F::BAR | F::VALUE | F::STATS | F::LABEL),
        ("tx", F::CHART | F::STATS),
        ("traffic", F::CHART | F::VALUE),
        ("total_rx", F::VALUE | F::LABEL),
        ("total_tx", F::VALUE),
    ];
    if n == 2 {
        list.push(("signal", F::BAR | F::VALUE | F::LABEL));
    }
    let mut widgets: Vec<WidgetSetting> = list
        .iter()
        .enumerate()
        .map(|(i, (k, forms))| metric(n, k, *forms, cell(i as u16 * 9)))
        .collect();
    widgets.push(fixed(FixedWidget::NetSettings, cell(list.len() as u16 * 9)));
    widgets
}

/// Every adapter metric (download, upload, both together, the running totals and the Wi-Fi
/// signal) shows in the forms the layout asks for, and the Network settings link in a grid
/// layout. "Source >" and Add Widget list the adapter's metrics; a wired adapter does not list
/// the signal. Without a network settings tool the link shows "Nothing to show on this
/// device".
#[gpui_kit::test]
fn page_net_every_metric_any_form(cx: &mut TestAppContext) {
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(grid_layout(1, any_form_widgets(1)));
    perf.layouts.set(grid_layout(2, any_form_widgets(2)));
    let mut r = open(cx, perf, Some(SettingsDesktop::Gnome));
    let page_b = r.build_page(2);
    draw(r.vcx);
    for (n, page) in [(1u8, r.page.clone()), (2, page_b.clone())] {
        let want = any_form_widgets(n);
        r.vcx.update(|_, cx| {
            let c = page.read(cx).container().read(cx);
            for (i, w) in want.iter().enumerate() {
                let id = i as u32 + 1;
                match &w.content {
                    WidgetContent::Metric(s) => {
                        let name = s.metric.as_str();
                        let widget = c.widget(id).unwrap_or_else(|| panic!("{name}"));
                        let widget = widget.read(cx);
                        assert_eq!(widget.availability(), Availability::Available, "{name}");
                        assert_eq!(widget.drawn_forms(), w.forms, "{name}");
                    }
                    _ => {
                        let fixed = c.fixed(id).expect("the settings link");
                        assert!(fixed.read(cx).content().is_some(), "the page's link view");
                    }
                }
            }
        });
    }
    // The source list of each page: the adapter's own metrics; the signal only on Wi-Fi.
    let keys = |r: &mut Rig, page: &Entity<NetPage>| {
        r.vcx.update(|_, cx| {
            let editor = page.read(cx).editor().clone();
            let groups = editor.read(cx).source_groups(cx);
            assert_eq!(groups.len(), 1, "only this adapter");
            groups[0]
                .items
                .iter()
                .map(|i| i.metric.as_str().to_owned())
                .collect::<Vec<_>>()
        })
    };
    let page_a = r.page.clone();
    assert_eq!(
        keys(&mut r, &page_a),
        ["rx", "tx", "traffic", "total_rx", "total_tx"]
    );
    assert_eq!(
        keys(&mut r, &page_b),
        ["rx", "tx", "traffic", "total_rx", "total_tx", "signal"]
    );
}

/// Without a network settings tool the link widget of a grid layout keeps its cells and shows
/// nothing.
#[gpui_kit::test]
fn page_net_link_nothing_to_show(cx: &mut TestAppContext) {
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(grid_layout(1, any_form_widgets(1)));
    let mut r = open(cx, perf, None);
    let container = r.container();
    r.vcx.update(|_, cx| {
        let fixed = container.read(cx).fixed(6).expect("the link widget");
        assert!(fixed.read(cx).content().is_none(), "Nothing to show");
    });
}

/// Edit Widgets on the Network page pins the flow layout, adds Total downloaded and applies:
/// the page's entry is the grid layout with the 1.3.0 parts pinned, and the chart keeps its
/// entity.
#[gpui_kit::test]
fn page_net_edit_add_remove_move_apply(cx: &mut TestAppContext) {
    let mut r = open(
        cx,
        PerformanceSettings::default(),
        Some(SettingsDesktop::Kde),
    );
    let container = r.container();
    let traffic = r.metric_id("traffic");
    let entity = r
        .vcx
        .update(|_, cx| container.read(cx).widget(traffic).map(|w| w.entity_id()));
    let editor = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).editor().clone())
    };
    r.vcx
        .update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
    draw(r.vcx);
    let source = WidgetSource {
        device: key(1).0,
        metric: Arc::from("total_rx"),
    };
    let added = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source, cx)))
        .expect("added");
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    let saved = r.saved(1).expect("the page's entry");
    assert!(saved.preset.is_none(), "a grid layout");
    assert!(saved.widgets.iter().any(|w| w.id == added));
    for kind in [FixedWidget::Details, FixedWidget::NetSettings] {
        assert!(
            saved
                .widgets
                .iter()
                .any(|w| matches!(&w.content, WidgetContent::Fixed(f) if *f == kind)),
            "{kind:?} is pinned with the rest"
        );
    }
    assert_eq!(
        r.vcx
            .update(|_, cx| container.read(cx).widget(traffic).map(|w| w.entity_id())),
        entity,
        "Apply keeps the entity"
    );
}

/// Counts the widget and chart entities made while it lives, and the drops of the entities
/// [`shown_entities`] watched.
struct Lifecycle {
    made: Rc<Cell<u32>>,
    dropped: Rc<Cell<u32>>,
    subs: Vec<Subscription>,
}

impl Lifecycle {
    fn new(r: &mut Rig) -> Self {
        let made = Rc::new(Cell::new(0));
        let count = || {
            let made = Rc::clone(&made);
            move || made.set(made.get() + 1)
        };
        let (a, b, c) = (count(), count(), count());
        let subs = r.vcx.update(|_, cx| {
            vec![
                cx.observe_new(move |_: &mut MetricWidget, _, _| a()),
                cx.observe_new(move |_: &mut FixedWidgetView, _, _| b()),
                cx.observe_new(move |_: &mut ChartView, _, _| c()),
            ]
        });
        Self {
            made,
            dropped: Rc::new(Cell::new(0)),
            subs,
        }
    }

    /// Counts a drop of `entity`.
    fn watch<T: 'static>(&mut self, entity: &Entity<T>, cx: &mut App) -> EntityId {
        let dropped = Rc::clone(&self.dropped);
        self.subs.push(cx.observe_release(entity, move |_, _| {
            dropped.set(dropped.get() + 1);
        }));
        entity.entity_id()
    }

    fn counts(&self) -> (u32, u32) {
        (self.made.get(), self.dropped.get())
    }
}

/// The entities the page shows: every placed widget and the chart of each metric widget, each
/// watched for a drop.
fn shown_entities(r: &mut Rig, life: &mut Lifecycle) -> Vec<EntityId> {
    let container = r.container();
    r.vcx.update(|_, cx| {
        let mut ids = Vec::new();
        for id in container.read(cx).placed_ids() {
            match container.read(cx).view(id).cloned() {
                Some(WidgetView::Metric(w)) => {
                    ids.push(life.watch(&w, cx));
                    if let Some(chart) = w.read(cx).chart().cloned() {
                        ids.push(life.watch(&chart, cx));
                    }
                }
                Some(WidgetView::Group(g)) => ids.push(life.watch(&g, cx)),
                Some(WidgetView::Fixed(f)) => ids.push(life.watch(&f, cx)),
                None => {}
            }
        }
        ids
    })
}

/// A sample that changes nothing the page shows renders no page or container; a sample that
/// moves the adapter's rates notifies only the Details view, never the page, the container or
/// the settings link. No sample makes or drops a widget or chart entity, and nothing is placed
/// again (ADR-019 phase 3 render contract).
#[gpui_kit::test]
fn page_net_sample_renders_no_page_or_container(cx: &mut TestAppContext) {
    let mut r = open(
        cx,
        PerformanceSettings::default(),
        Some(SettingsDesktop::Kde),
    );
    let container = r.container();
    let page = r.page.clone();
    let counts = |r: &mut Rig| {
        r.vcx.update(|_, cx| {
            let p = page.read(cx);
            let c = container.read(cx);
            (
                p.render_count.get(),
                p.notify_count.get(),
                c.render_count.get(),
                c.notify_count.get(),
                p.settings_view().read(cx).notify_count.get(),
                p.place_count.get(),
                p.details_view().read(cx).notify_count.get(),
            )
        })
    };
    publish_values(&r.engine, 12.);
    draw(r.vcx);
    let mut life = Lifecycle::new(&mut r);
    let entities = shown_entities(&mut r, &mut life);
    // Throughput, Details and the settings link, and the chart.
    assert_eq!(entities.len(), 4, "{entities:?}");
    let before = counts(&mut r);
    for _ in 0..3 {
        publish_values(&r.engine, 12.);
        draw(r.vcx);
    }
    assert_eq!(counts(&mut r), before, "nothing shown changed");
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    publish_values(&r.engine, 57.);
    draw(r.vcx);
    let after = counts(&mut r);
    assert_eq!(after.1, before.1, "the page never notifies");
    assert_eq!(after.3, before.3, "the container never notifies");
    assert_eq!(after.4, before.4, "the settings link never notifies");
    assert_eq!(after.5, before.5, "nothing is placed again");
    assert!(
        after.6 > before.6,
        "the Download speed row notifies the Details view"
    );
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    assert_eq!(
        shown_entities(&mut r, &mut life),
        entities,
        "the same entities"
    );
}

/// The page's demand with the default layout is 1.3.0's: network and the device history,
/// nothing a Network page did not ask for.
#[gpui_kit::test]
fn page_net_default_demand_as_1_3_0(cx: &mut TestAppContext) {
    let r = open(cx, PerformanceSettings::default(), None);
    let page = r.page.clone();
    let d = r.vcx.update(|_, cx| page.read(cx).demand_part(cx));
    assert!(d.any_surface_visible && d.nets);
    assert!(!d.disks && !d.memory && !d.gpus && !d.cpu_fast && !d.sensors);
}

/// The page's widgets draw the same in the flow layout, in Edit Widgets and after Apply
/// (tasklens-hzire.10, the look sweep).
#[gpui_kit::test]
fn page_net_look_survives_edit_and_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default(), None);
    let container = r.container();
    let editor = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).editor().clone())
    };
    super::page_look::sweep(r.vcx, &container, &editor, "net");
}
