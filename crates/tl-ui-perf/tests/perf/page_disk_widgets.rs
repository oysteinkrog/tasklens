//! The Disk page as a widget container (ADR-019 phase 3, UI-PRF-061, `tasklens-hzire.7`): the
//! default flow layout draws the 1.3.0 parts in their 1.3.0 places, each disk has its own
//! layout, a disk that is gone shows its widgets as unavailable and keeps its entry, every
//! disk metric shows in the forms it offers, Edit Widgets adds and applies, and a sample renders
//! no page or container.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, EntityId, IntoElement, ParentElement as _,
    Pixels, Render, StyleRefinement, Styled as _, Subscription, TestAppContext, VisualTestContext,
    Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::{DiskDetails, PartitionRow, PartitionTable, SmartKind};
use tl_model::system::{DiskTotals, Metric};
use tl_model::table::Total;
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
use tl_ui_overview::widget::container::{WidgetContainer, WidgetView};
use tl_ui_overview::widget::details_widget::details_id;
use tl_ui_overview::widget::edit::saved_layout;
use tl_ui_overview::widget::fixed_widget::FixedWidgetView;
use tl_ui_overview::widget::metric_widget::{MetricWidget, widget_id};
use tl_ui_overview::widget::source::Availability;
use tl_ui_perf::performance::disk::{DiskPage, PART_PARTITIONS};
use tl_ui_perf::performance::memory_views::part_id;
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

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// Disk A (an NVMe drive) and disk B (a SATA drive).
const DISK_A: DeviceIdx = DeviceIdx(2);
const DISK_B: DeviceIdx = DeviceIdx(3);

fn identity(n: u8) -> DeviceIdentity {
    let (name, serial) = if n == 1 {
        ("nvme0n1", "eui.0025388b")
    } else {
        ("sda", "WD-WCC4E1234567")
    };
    DeviceIdentity::Disk {
        major: if n == 1 { 259 } else { 8 },
        minor: 0,
        name: Arc::from(name),
        wwn_or_serial: Some(Arc::from(serial)),
    }
}

fn idx(n: u8) -> DeviceIdx {
    if n == 1 { DISK_A } else { DISK_B }
}

/// The persistent key of disk `n` (1 is A, 2 is B), as the sidebar and the page use it.
fn key(n: u8) -> DeviceKey {
    DeviceKey::from_identity(&identity(n)).expect("disk key")
}

fn container(n: u8) -> String {
    page_container(&key(n).0)
}

fn entry(n: u8, present: bool) -> DeviceEntry {
    DeviceEntry {
        idx: idx(n),
        identity: identity(n),
        generation: 0,
        display_name: Arc::from(if n == 1 { "nvme0n1" } else { "sda" }),
        present,
        parent: None,
        persistent_key: key(n).0,
        net_kind: None,
        hot_c: None,
    }
}

/// Disk `n`'s record, every value valid.
fn record(n: u8, active: f32) -> DiskTotals {
    DiskTotals {
        device: idx(n),
        active_pct: Metric::valid(active),
        read_bps: Metric::valid(4.2 * MIB as f64),
        write_bps: Metric::valid(1.5 * MIB as f64),
        response_ms: Metric::valid(0.4237),
        read_total: 812 * GIB,
        written_total: 400 * GIB,
        capacity: 1_000_204_886_016,
        temp_c: Metric::valid(41.0),
    }
}

fn partition(name: &str, start: u64) -> PartitionRow {
    PartitionRow {
        name: Arc::from(name),
        start_bytes: start,
        size_bytes: 100 * MIB,
        holder: None,
        mount: None,
        extra_mounts: Arc::from([]),
        label: None,
        fs_type: Some(Arc::from("ext4")),
        usage_pct: Total {
            value: 42.1,
            validity: Validity::Valid,
        },
        flags: Arc::from([]),
        type_name: Arc::from("Linux filesystem"),
        read_only: false,
        unallocated: false,
    }
}

/// Publishes both disks (A present or not), their records with A's Active time at `active`,
/// and their details; with `partitions`, A's partition table.
fn publish(engine: &FakeEngine, a_present: bool, active: f32, partitions: bool) {
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
        let mut disks = vec![(DISK_B, record(2, 3.))];
        if a_present {
            disks.insert(0, (DISK_A, record(1, active)));
        }
        sys.disks = Arc::from(disks);
        let details = Arc::make_mut(&mut s.device_details);
        details.header.version += 1;
        let table = partitions.then(|| {
            Arc::new(PartitionTable {
                kind: Arc::from("GPT"),
                rows: Arc::from(vec![
                    partition("nvme0n1p1", MIB),
                    partition("nvme0n1p2", 101 * MIB),
                ]),
            })
        });
        let detail = |partitions| DiskDetails {
            model: Total {
                value: Arc::from("WD_BLACK SN850X 1000GB"),
                validity: Validity::Valid,
            },
            smart: SmartKind::Nvme,
            partitions,
            ..DiskDetails::default()
        };
        details.disks = Arc::from(vec![(DISK_A, detail(table)), (DISK_B, detail(None))]);
    });
}

/// Publishes a sample that carries only new values: the disk records, A's Active time at
/// `active`. The catalog and the details stay as they are.
fn publish_values(engine: &FakeEngine, active: f32) {
    engine.publish_with(|s| {
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.disks = Arc::from(vec![(DISK_A, record(1, active)), (DISK_B, record(2, 3.))]);
    });
}

/// The page, cached, so it renders only when it notifies.
struct Host(Entity<DiskPage>);

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
    /// Disk A's page, shown in the window.
    page: Entity<DiskPage>,
    vcx: &'a mut VisualTestContext,
    _driver: Entity<AnimationDriver>,
}

fn row(n: u8) -> DeviceRow {
    DeviceRow {
        key: key(n),
        kind: DeviceKind::Disk,
        idx: idx(n),
        generation: 0,
        net_class: None,
        fan_connected: None,
    }
}

/// A 1200 × 900 window with disk A's page, built with `perf` as the loaded performance
/// settings. A has its partition table.
fn open(cx: &mut TestAppContext, perf: PerformanceSettings) -> Rig<'_> {
    let engine = FakeEngine::new();
    publish(&engine, true, 12., true);
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
        let page = cx.new(|cx| DiskPage::new(&host2, key(1), row(1), hub2, demand2, cx));
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

    /// Builds the page of disk `n` again (as the window does when it first shows it), without
    /// showing it.
    fn build_page(&mut self, n: u8) -> Entity<DiskPage> {
        let (hub, demand, host) = (self.hub.clone(), self.demand.clone(), self.host.clone());
        self.vcx
            .update(|_, cx| cx.new(|cx| DiskPage::new(&host, key(n), row(n), hub, demand, cx)))
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

/// The default flow layout draws the 1.3.0 page at 1200 × 900: Active time and Transfer rate
/// (150 px, `sp4` apart) across the body, `details_gap` above Details and `sp6` above the
/// partitions block. Nothing is written: a disk page has no old key to convert.
#[gpui_kit::test]
fn page_disk_default_flow_places_1_3_0_parts(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    assert!(r.saved(1).is_none(), "no entry");
    let c = container(1);
    let (active, io) = (r.metric_id("active"), r.metric_id("io"));
    let a = r.bounds(widget_id(&c, active));
    let t = r.bounds(widget_id(&c, io));
    let details = r.bounds(details_id(&c));
    let partitions = r.bounds(part_id(&c, PART_PARTITIONS));
    let body_w = 1200. - 2. * PAGE_PAD;
    assert!(near(a.size.width, body_w), "{a:?}");
    assert!(near(t.size.width, body_w), "{t:?}");
    assert!(near(a.size.height, 150.), "{a:?}");
    assert!(near(t.size.height, 150.), "{t:?}");
    assert!(near(t.top() - a.bottom(), 8.), "sp4 between the charts");
    assert!(near(details.top() - t.bottom(), 24.), "details_gap");
    assert!(near(partitions.top() - details.bottom(), 16.), "sp6");
    // The charts carry the 1.3.0 headings and legend.
    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        let p = page.read(cx);
        let a = p.active_chart(cx).expect("Active time chart");
        let a = a.read(cx);
        assert_eq!(a.labels.heading.as_deref(), Some("Active time"));
        let t = p.transfer_chart(cx).expect("Transfer rate chart");
        let t = t.read(cx);
        assert_eq!(t.labels.heading.as_deref(), Some("Transfer rate"));
        let legend: Vec<&str> = t.labels.legend.iter().map(|(s, _)| s.as_ref()).collect();
        assert_eq!(legend, ["Read", "Write"]);
        // The block shows the table's heading.
        let block = p.partitions().expect("the table is read");
        assert_eq!(block.heading.as_ref(), "2 partitions - GPT");
        assert!(p.partitions_table().is_some(), "the block drew its table");
    });
}

/// Each disk has its own layout: a saved grid layout of disk A does not reach disk B, which
/// shows the built-in default, and a change to B's entry leaves A's as it was.
#[gpui_kit::test]
fn page_disk_second_disk_own_layout(cx: &mut TestAppContext) {
    let a = grid_layout(
        1,
        vec![
            metric(1, "response", FormSet::VALUE | FormSet::LABEL, cell(0)),
            metric(1, "temp", FormSet::VALUE | FormSet::LABEL, cell(9)),
        ],
    );
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(a.clone());
    let mut r = open(cx, perf);
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
    assert_eq!(metrics_of(&shown_b), ["active", "io"]);
    assert!(shown_b.preset.is_some(), "B shows its built-in flow layout");
    assert!(r.saved(2).is_none(), "nothing written for B");
    // Every default widget of B names B.
    assert!(shown_b.widgets.iter().all(|w| match &w.content {
        WidgetContent::Metric(s) => s.device == DeviceSelector::Key(key(2).0),
        _ => true,
    }));
    // A grid layout for B: A keeps its own.
    let b = grid_layout(
        2,
        vec![metric(
            2,
            "active",
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

/// A disk that is gone shows its page's widgets as unavailable ("Not connected") and its entry
/// stays; when it comes back the saved layout shows again with the same entities.
#[gpui_kit::test]
fn page_disk_removed_shows_unavailable(cx: &mut TestAppContext) {
    let layout = grid_layout(
        1,
        vec![
            metric(
                1,
                "active",
                FormSet::CHART | FormSet::VALUE | FormSet::LABEL,
                cell(0),
            ),
            metric(1, "response", FormSet::VALUE | FormSet::LABEL, cell(9)),
        ],
    );
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(layout.clone());
    let mut r = open(cx, perf);
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
    publish(&r.engine, false, 12., true);
    draw(r.vcx);
    let gone = state(&mut r);
    for (_, a, reason) in &gone {
        assert_eq!(*a, Availability::Gone);
        assert_eq!(reason, "Not connected");
    }
    assert_eq!(r.saved(1), Some(layout.clone()), "the entry is kept");
    publish(&r.engine, true, 12., true);
    draw(r.vcx);
    let back = state(&mut r);
    for (b, a) in back.iter().zip(&before) {
        assert_eq!(b.1, Availability::Available);
        assert_eq!(b.0, a.0, "same entity");
    }
}

/// The forms of each widget of [`page_disk_every_metric_any_form`], in list order.
fn any_form_widgets(n: u8) -> Vec<WidgetSetting> {
    use FormSet as F;
    let list: [(&str, FormSet); 8] = [
        ("active", F::CHART | F::BAR | F::VALUE | F::STATS | F::LABEL),
        ("read", F::CHART | F::STATS),
        ("write", F::VALUE | F::STATS | F::LABEL),
        ("io", F::CHART | F::VALUE),
        ("response", F::VALUE | F::LABEL),
        ("temp", F::VALUE),
        ("total_read", F::VALUE | F::LABEL),
        ("total_written", F::VALUE | F::LABEL),
    ];
    let mut widgets: Vec<WidgetSetting> = list
        .iter()
        .enumerate()
        .map(|(i, (k, forms))| metric(n, k, *forms, cell(i as u16 * 9)))
        .collect();
    widgets.push(WidgetSetting {
        content: WidgetContent::Fixed(FixedWidget::DiskPartitions),
        ..metric(n, "", FormSet::empty(), cell(72))
    });
    widgets
}

/// Every disk metric (Active time, read and write rates, both together, response time,
/// temperature and the running totals) shows in the forms the layout asks for, and the
/// partitions block in a grid layout. "Source >" and Add Widget list the disk's metrics. Disk B,
/// whose partition table is not read, shows "Nothing to show on this device" for the block and
/// its Add Widget does not offer it.
#[gpui_kit::test]
fn page_disk_every_metric_any_form(cx: &mut TestAppContext) {
    let mut perf = PerformanceSettings::default();
    perf.layouts.set(grid_layout(1, any_form_widgets(1)));
    perf.layouts.set(grid_layout(2, any_form_widgets(2)));
    let mut r = open(cx, perf);
    let container = r.container();
    let want = any_form_widgets(1);
    r.vcx.update(|_, cx| {
        let c = container.read(cx);
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
                    let fixed = c.fixed(id).expect("the partitions block");
                    assert!(fixed.read(cx).content().is_some(), "the page's block view");
                }
            }
        }
    });
    // The source list of the page: the disk's own metrics.
    let page = r.page.clone();
    let keys = r.vcx.update(|_, cx| {
        let editor = page.read(cx).editor().clone();
        let groups = editor.read(cx).source_groups(cx);
        assert_eq!(groups.len(), 1, "only this disk");
        groups[0]
            .items
            .iter()
            .map(|i| i.metric.as_str().to_owned())
            .collect::<Vec<_>>()
    });
    for k in [
        "active",
        "read",
        "write",
        "io",
        "temp",
        "response",
        "total_read",
        "total_written",
    ] {
        assert!(keys.iter().any(|have| have == k), "{k} in {keys:?}");
    }
    // Disk B has no partition table: its block keeps its cells and shows nothing.
    let page_b = r.build_page(2);
    draw(r.vcx);
    r.vcx.update(|window, cx| {
        let p = page_b.read(cx);
        assert!(p.partitions().is_none());
        let c = p.container().read(cx);
        let fixed = c.fixed(9).expect("the partitions block widget");
        assert!(fixed.read(cx).content().is_none(), "Nothing to show");
        let editor = p.editor().clone();
        editor.update(cx, |e, cx| e.start(window, cx));
        let adds: Vec<String> = editor
            .read(cx)
            .fixed_adds(cx)
            .into_iter()
            .map(|a| a.name.to_string())
            .collect();
        assert!(!adds.iter().any(|a| a == "disk_partitions"), "{adds:?}");
    });
}

/// Edit Widgets on the Disk page pins the flow layout, adds Average response time and applies:
/// the page's entry is the grid layout with the 1.3.0 parts pinned, and the charts keep their
/// entities.
#[gpui_kit::test]
fn page_disk_edit_add_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    let container = r.container();
    let active = r.metric_id("active");
    let entity = r
        .vcx
        .update(|_, cx| container.read(cx).widget(active).map(|w| w.entity_id()));
    let editor = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).editor().clone())
    };
    r.vcx
        .update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
    draw(r.vcx);
    let source = WidgetSource {
        device: key(1).0,
        metric: Arc::from("response"),
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
    for kind in [FixedWidget::Details, FixedWidget::DiskPartitions] {
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
            .update(|_, cx| container.read(cx).widget(active).map(|w| w.entity_id())),
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
/// moves the disk's values notifies neither the page, the container nor the partitions block.
/// No sample makes or drops a widget or chart entity (ADR-019 phase 3 render contract).
#[gpui_kit::test]
fn page_disk_sample_renders_no_page_or_container(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
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
                p.partitions_view().read(cx).notify_count.get(),
                p.place_count.get(),
            )
        })
    };
    publish_values(&r.engine, 12.);
    draw(r.vcx);
    let mut life = Lifecycle::new(&mut r);
    let entities = shown_entities(&mut r, &mut life);
    // Active time, Transfer rate, Details and the partitions block, and the two charts.
    assert_eq!(entities.len(), 6, "{entities:?}");
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
    assert_eq!(after.4, before.4, "the partitions block never notifies");
    assert_eq!(after.5, before.5, "nothing is placed again");
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    assert_eq!(
        shown_entities(&mut r, &mut life),
        entities,
        "the same entities"
    );
}

/// The sample in which the partition table goes or comes back changes what exists: it places
/// the widgets once, and the block takes no space while the table is not read (ADR-019 phase 3,
/// leader note of 2026-11-03).
#[gpui_kit::test]
fn page_disk_partitions_appear_place_once(cx: &mut TestAppContext) {
    let mut r = open(cx, PerformanceSettings::default());
    let page = r.page.clone();
    let placed = |r: &mut Rig| r.vcx.update(|_, cx| page.read(cx).place_count.get());
    let c = container(1);
    let selector = leak(part_id(&c, PART_PARTITIONS));
    assert!(r.vcx.debug_bounds(selector).is_some(), "the block is drawn");
    let start = placed(&mut r);
    publish(&r.engine, true, 12., false);
    draw(r.vcx);
    assert_eq!(
        placed(&mut r),
        start + 1,
        "one placement for the table going"
    );
    assert!(r.vcx.debug_bounds(selector).is_none(), "no space");
    for _ in 0..2 {
        publish(&r.engine, true, 12., false);
        draw(r.vcx);
    }
    assert_eq!(placed(&mut r), start + 1, "same structure, no placement");
    publish(&r.engine, true, 12., true);
    draw(r.vcx);
    assert_eq!(
        placed(&mut r),
        start + 2,
        "one placement for the table coming back"
    );
    assert!(r.vcx.debug_bounds(selector).is_some());
}
