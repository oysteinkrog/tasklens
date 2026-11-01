//! Overview group widgets (ADR-019 "Group widgets", UI-PRF-061 phase 2): the members of a
//! group, how the items fill the widget, the "+n more" item, hotplug, and the render contract.
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, EntityId, IntoElement, ParentElement as _, Pixels,
    Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, point,
    px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::layout::{
    CPU_BOX_CONTAINER, ContainerLayout, DISK_BOX_CONTAINER, DeviceSelector, FormSet, GroupItems,
    GroupKind, GroupOptions, GroupSelector, LAYOUT_VERSION, MetricKey, SourceSelector, WidgetCell,
    WidgetContent, WidgetOptions, WidgetSetting,
};
use tl_model::layout_boxes::{DEVICE_CPU, DEVICE_DISKS, METRIC_DISK_IO, METRIC_LOAD};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, CoreSmoothing, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::time::now_boot_ns;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::widget::compose::Composition;
use tl_ui::pages::performance::widget::container::WidgetContainer;
use tl_ui::pages::performance::widget::edit::{
    OUTLINE, WidgetEditor, corner_id, frame_id, gear_id, grip_id,
};
use tl_ui::pages::performance::widget::group_widget::{GroupState, GroupWidget};
use tl_ui::pages::performance::widget::metric_widget::{part_id, widget_id};
use tl_ui::pages::performance::widget::source::SourceStore;
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

/// The first logical CPU's device index; core `n` is at `CORE0 + n`.
const CORE0: u16 = 10;
/// The first disk's device index.
const DISK0: u16 = 100;
const ROWS_FORMS: FormSet = FormSet::CHART
    .union(FormSet::BAR)
    .union(FormSet::VALUE)
    .union(FormSet::LABEL);
const CELL_FORMS: FormSet = FormSet::CHART.union(FormSet::VALUE).union(FormSet::LABEL);

/// Stands in for the Overview page: a cached view holding the container as a cached child.
struct Page {
    container: Entity<WidgetContainer>,
    render_count: Cell<u32>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.render_count.set(self.render_count.get() + 1);
        div().size_full().child(
            self.container
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

struct Host {
    page: Option<(Entity<Page>, f32, f32)>,
    editor: Option<(Entity<WidgetEditor>, f32, f32)>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .children(self.page.iter().map(|(p, w, h)| {
                div()
                    .flex_none()
                    .w(px(*w))
                    .h(px(*h))
                    .child(p.clone().cached(StyleRefinement::default().size_full()))
            }))
            .children(self.editor.iter().map(|(e, w, h)| {
                div()
                    .flex_none()
                    .w(px(*w))
                    .h(px(*h))
                    .child(e.clone().cached(StyleRefinement::default().size_full()))
            }))
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

fn disk_entry(n: u16) -> DeviceEntry {
    let name = format!("sd{}", char::from(b'a' + n as u8));
    entry(
        DISK0 + n,
        DeviceIdentity::Disk {
            major: 8,
            minor: u32::from(n) * 16,
            name: Arc::from(name.as_str()),
            wwn_or_serial: None,
        },
        &format!("disk:{name}"),
        &name,
    )
}

/// The CPU package, `cores` logical CPUs and the disks `disks` (by number).
fn catalog(version: u64, cores: u16, disks: &[u16]) -> DeviceCatalog {
    let mut entries = vec![
        entry(0, DeviceIdentity::CpuPackage, DEVICE_CPU, "CPU"),
        entry(
            1,
            DeviceIdentity::Aggregate(AggregateKind::Disks),
            DEVICE_DISKS,
            "Disks",
        ),
    ];
    for n in 0..cores {
        entries.push(entry(
            CORE0 + n,
            DeviceIdentity::CpuLogical { id: n },
            &format!("cpu:{n}"),
            &format!("CPU {n}"),
        ));
    }
    entries.extend(disks.iter().map(|&n| disk_entry(n)));
    DeviceCatalog {
        version,
        entries: Arc::from(entries),
    }
}

fn group_widget(id: u32, group: GroupSelector, forms: FormSet, cell: WidgetCell) -> WidgetSetting {
    WidgetSetting {
        id,
        content: WidgetContent::Group(group),
        forms,
        cell: Some(cell),
        options: WidgetOptions::default(),
        device_name: None,
    }
}

fn cores(items: GroupItems, limit: Option<u16>) -> GroupSelector {
    GroupSelector {
        device: DeviceSelector::Key(Arc::from(DEVICE_CPU)),
        members: GroupKind::Cores,
        metric: MetricKey::new(METRIC_LOAD),
        options: GroupOptions {
            limit,
            items,
            arrange: None,
        },
    }
}

fn disks(items: GroupItems, limit: Option<u16>) -> GroupSelector {
    GroupSelector {
        device: DeviceSelector::Key(Arc::from(DEVICE_DISKS)),
        members: GroupKind::Disks,
        metric: MetricKey::new(METRIC_DISK_IO),
        options: GroupOptions {
            limit,
            items,
            arrange: None,
        },
    }
}

fn cell(col: u16, row: u16, cols: u16, rows: u16) -> WidgetCell {
    WidgetCell {
        col,
        row,
        cols,
        rows,
    }
}

fn layout(container: &str, widgets: Vec<WidgetSetting>) -> ContainerLayout {
    ContainerLayout {
        container: container.to_owned(),
        version: LAYOUT_VERSION,
        preset: None,
        next_id: widgets.iter().map(|w| w.id + 1).max().unwrap_or(1),
        widgets,
    }
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    hub: Entity<SnapshotHub>,
    host: Entity<Host>,
    vcx: &'a mut VisualTestContext,
    _driver: Entity<AnimationDriver>,
}

fn open<'a>(cx: &'a mut TestAppContext, cores: u16, disks: &[u16]) -> Rig<'a> {
    let engine = FakeEngine::new();
    let catalog = catalog(1, cores, disks);
    engine.publish_with(|s| s.devices = Arc::new(catalog));
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        let mut settings = Settings::default();
        // Core values without smoothing, so a sample changes only its own core.
        settings.performance.overview_core_smoothing = CoreSmoothing::Off;
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let (host, vcx) = cx.add_window_view(|_, _| Host {
        page: None,
        editor: None,
    });
    vcx.simulate_scale_factor_change(1.0);
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    vcx.simulate_resize(size(px(1600.), px(1200.)));
    vcx.run_until_parked();
    Rig {
        engine,
        hub,
        host,
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

impl Rig<'_> {
    /// A container `id` showing `layout` in a box `w × h` px, drawn.
    fn container(&mut self, layout: ContainerLayout, w: f32, h: f32) -> Entity<WidgetContainer> {
        let hub = self.hub.clone();
        let host = self.host.clone();
        let store = Rc::new(RefCell::new(SourceStore::new(&hub)));
        let id = layout.container.clone();
        let c = self.vcx.update(|_, cx| {
            let c = cx.new(|cx| WidgetContainer::new(id, layout, &hub, store, cx));
            host.update(cx, |host, cx| {
                let page = cx.new(|_| Page {
                    container: c.clone(),
                    render_count: Cell::new(0),
                });
                host.page = Some((page, w, h));
                cx.notify();
            });
            c
        });
        draw(self.vcx);
        c
    }

    fn resize(&mut self, w: f32, h: f32) {
        let host = self.host.clone();
        self.vcx.update(|_, cx| {
            host.update(cx, |host, cx| {
                if let Some(p) = host.page.as_mut() {
                    p.1 = w;
                    p.2 = h;
                }
                cx.notify();
            })
        });
        draw(self.vcx);
    }

    fn publish_catalog(&mut self, catalog: DeviceCatalog) {
        self.engine.publish_with(|s| s.devices = Arc::new(catalog));
        self.vcx.run_until_parked();
        draw(self.vcx);
    }

    /// One-second `CPU_TOTAL` series of each core in `values` (core number, version, value).
    fn core_series(&mut self, version: u64, values: &[(u16, u64, f32)]) {
        let newest = now_boot_ns();
        let views: Vec<Arc<SeriesView>> = values
            .iter()
            .map(|&(n, v, value)| {
                Arc::new(SeriesView {
                    id: SeriesId {
                        device: DeviceIdx(CORE0 + n),
                        metric: MetricId::CPU_TOTAL,
                    },
                    version: v,
                    period_ns: 1_000_000_000,
                    samples: vec![
                        Sample::valid(newest - 1_000_000_000, value),
                        Sample::valid(newest, value),
                    ]
                    .into(),
                })
            })
            .collect();
        self.engine.publish_with(|s| {
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: Arc::from(views),
                slow: Arc::from([]),
            });
        });
        self.vcx.run_until_parked();
        draw(self.vcx);
    }

    fn group(&mut self, c: &Entity<WidgetContainer>, id: u32) -> Entity<GroupWidget> {
        self.vcx
            .read_entity(c, |c, _| c.group(id).cloned())
            .expect("a group widget")
    }

    /// The group's items: (member device key, item entity id).
    fn items(&mut self, g: &Entity<GroupWidget>) -> Vec<(String, EntityId)> {
        self.vcx.read_entity(g, |g, _| {
            g.items()
                .map(|(s, w)| {
                    let key = match &s.device {
                        DeviceSelector::Key(k) => k.to_string(),
                        DeviceSelector::Slot(n) => format!("slot{n}"),
                    };
                    (key, w.entity_id())
                })
                .collect()
        })
    }

    /// The notify counts of the group and of each of its items.
    fn group_notifies(&mut self, g: &Entity<GroupWidget>) -> (u32, Vec<u32>) {
        let items: Vec<_> = self
            .vcx
            .read_entity(g, |g, _| g.items().map(|(_, w)| w.clone()).collect());
        let own = self.vcx.read_entity(g, |g, _| g.notify_count.get());
        let each = items
            .iter()
            .map(|w| self.vcx.read_entity(w, |w, _| w.notify_count.get()))
            .collect();
        (own, each)
    }

    fn container_notifies(&mut self, c: &Entity<WidgetContainer>) -> u32 {
        self.vcx.read_entity(c, |c, _| c.notify_count.get())
    }

    fn has(&mut self, selector: String) -> bool {
        let p: &'static str = Box::leak(selector.into_boxed_str());
        self.vcx.debug_bounds(p).is_some()
    }
}

/// 32 cores as Rows fill the widget's height as the core rows do (D-040): two columns of 16
/// rows, each 30 px in a 480 px high body. In a narrow body they do not fit at 18 px and
/// scroll, keeping 18 px rows.
#[gpui_kit::test]
fn overview_group_rows_fill_d040(cx: &mut TestAppContext) {
    let mut r = open(cx, 32, &[]);
    let w = group_widget(
        1,
        cores(GroupItems::Rows, None),
        ROWS_FORMS,
        cell(0, 0, 12, 8),
    );
    let c = r.container(layout(CPU_BOX_CONTAINER, vec![w]), 750., 480.);
    let g = r.group(&c, 1);
    assert_eq!(
        r.vcx.read_entity(&g, |g, _| g.state()),
        GroupState::Members { count: 32 }
    );
    let fill = r.vcx.read_entity(&g, |g, _| g.fill().clone());
    assert!(!fill.scrolls);
    assert_eq!((fill.cols, fill.rows), (2, 16));
    assert_eq!(fill.composition, Composition::Row);
    assert!(fill.items.iter().all(|b| b.size.height == px(30.)));
    assert_eq!(fill.items[15].bottom(), px(480.));
    // C0 to C15 in the first column.
    let items = r.items(&g);
    assert_eq!(items.len(), 32);
    assert_eq!(items[0].0, "cpu:0");
    assert_eq!(items[16].0, "cpu:16");
    assert_eq!(fill.items[16].top(), px(0.));
    // Every item is a Row however tall it grows.
    let first = r
        .vcx
        .read_entity(&g, |g, _| g.items().next().map(|(_, w)| w.clone()))
        .expect("an item");
    assert_eq!(
        r.vcx.read_entity(&first, |w, _| w.composed().composition),
        Composition::Row
    );
    assert_eq!(
        r.vcx.read_entity(&first, |w, _| w.texts()[0].to_string()),
        "C0"
    );

    // A narrow body: one column of 18 px rows that scrolls.
    r.resize(250., 200.);
    let fill = r.vcx.read_entity(&g, |g, _| g.fill().clone());
    assert!(fill.scrolls);
    assert_eq!(fill.cols, 1);
    assert!(fill.items.iter().all(|b| b.size.height == px(18.)));
    assert!(r.has("perf-widget-1-scroll".into()));
    // The group kept its entity and its items.
    assert_eq!(r.group(&c, 1).entity_id(), g.entity_id());
    assert_eq!(r.items(&g), items);
}

/// Cells fill the widget with equal cells and scroll when they do not fit at their minimum.
#[gpui_kit::test]
fn overview_group_cells_fill_and_scroll(cx: &mut TestAppContext) {
    let mut r = open(cx, 8, &[]);
    let w = group_widget(
        1,
        cores(GroupItems::Cells, None),
        CELL_FORMS,
        cell(0, 0, 12, 8),
    );
    let c = r.container(layout(CPU_BOX_CONTAINER, vec![w]), 600., 400.);
    let g = r.group(&c, 1);
    let fill = r.vcx.read_entity(&g, |g, _| g.fill().clone());
    assert!(!fill.scrolls);
    assert_eq!(fill.composition, Composition::Cell);
    let first = fill.items[0].size;
    assert!(fill.items.iter().all(|b| b.size == first));
    assert!(first.width >= px(120.) && first.height >= px(48.));
    r.resize(200., 100.);
    let fill = r.vcx.read_entity(&g, |g, _| g.fill().clone());
    assert!(fill.scrolls);
    assert!(fill.content.height > px(100.));
}

/// With a limit, the group shows that many members and a "+n more" item for the rest. The
/// members past the limit have no item.
#[gpui_kit::test]
fn overview_group_limit_more_item(cx: &mut TestAppContext) {
    let mut r = open(cx, 8, &[]);
    let w = group_widget(
        1,
        cores(GroupItems::Rows, Some(4)),
        ROWS_FORMS,
        cell(0, 0, 12, 8),
    );
    let c = r.container(layout(CPU_BOX_CONTAINER, vec![w]), 600., 300.);
    let g = r.group(&c, 1);
    let keys: Vec<String> = r.items(&g).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["cpu:0", "cpu:1", "cpu:2", "cpu:3"]);
    assert_eq!(r.vcx.read_entity(&g, |g, _| g.hidden()), 4);
    assert_eq!(
        r.vcx.read_entity(&g, |g, _| g.more_text().to_string()),
        "+4 more"
    );
    // The "+4 more" item is the fifth row.
    let fill = r.vcx.read_entity(&g, |g, _| g.fill().clone());
    assert_eq!(fill.items.len(), 5);
    assert!(r.has(part_id(1, "more")));
    // As cells, "+4 more" is a cell of its own.
    let w = group_widget(
        1,
        cores(GroupItems::Cells, Some(4)),
        CELL_FORMS,
        cell(0, 0, 12, 8),
    );
    r.vcx.update(|_, cx| {
        c.update(cx, |c, cx| {
            c.set_layout(layout(CPU_BOX_CONTAINER, vec![w]), cx)
        })
    });
    draw(r.vcx);
    assert_eq!(r.group(&c, 1).entity_id(), g.entity_id());
    let fill = r.vcx.read_entity(&g, |g, _| g.fill().clone());
    assert_eq!(fill.items.len(), 5);
    assert_eq!(fill.composition, Composition::Cell);
    assert!(r.has(part_id(1, "more")));
    // Without a limit every member shows and there is no "+n more".
    let w = group_widget(
        1,
        cores(GroupItems::Rows, None),
        ROWS_FORMS,
        cell(0, 0, 12, 8),
    );
    r.vcx.update(|_, cx| {
        c.update(cx, |c, cx| {
            c.set_layout(layout(CPU_BOX_CONTAINER, vec![w]), cx)
        })
    });
    draw(r.vcx);
    assert_eq!(r.items(&g).len(), 8);
    assert!(!r.has(part_id(1, "more")));
}

/// A disk added and removed: the group keeps its entity and cells, the kept disks keep their
/// items, and the container does not notify. With no disks left it shows "None found".
#[gpui_kit::test]
fn overview_group_hotplug_keeps_entity(cx: &mut TestAppContext) {
    let mut r = open(cx, 0, &[0, 1]);
    let w = group_widget(
        1,
        disks(GroupItems::Cells, Some(8)),
        CELL_FORMS,
        cell(0, 0, 6, 8),
    );
    let c = r.container(layout(DISK_BOX_CONTAINER, vec![w]), 400., 400.);
    let g = r.group(&c, 1);
    let before = r.items(&g);
    let keys: Vec<&str> = before.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["disk:sda", "disk:sdb"]);
    let cells = r.vcx.read_entity(&c, |c, _| c.placement().clone());
    let container_notifies = r.container_notifies(&c);

    // A third disk comes.
    r.publish_catalog(catalog(2, 0, &[0, 1, 2]));
    assert_eq!(r.group(&c, 1).entity_id(), g.entity_id());
    let after = r.items(&g);
    assert_eq!(after.len(), 3);
    assert_eq!(&after[..2], &before[..]);
    assert_eq!(after[2].0, "disk:sdc");
    assert_eq!(r.vcx.read_entity(&c, |c, _| c.placement().clone()), cells);
    assert_eq!(r.container_notifies(&c), container_notifies);

    // The first disk goes: the others keep their items, in order.
    r.publish_catalog(catalog(3, 0, &[1, 2]));
    let gone = r.items(&g);
    assert_eq!(gone, after[1..].to_vec());
    assert_eq!(r.vcx.read_entity(&c, |c, _| c.placement().clone()), cells);
    assert_eq!(r.container_notifies(&c), container_notifies);

    // It comes back in its place in the order.
    r.publish_catalog(catalog(4, 0, &[0, 1, 2]));
    let back: Vec<String> = r.items(&g).into_iter().map(|(k, _)| k).collect();
    assert_eq!(back, ["disk:sda", "disk:sdb", "disk:sdc"]);

    // No disks: "None found", and the widget keeps its cells.
    r.publish_catalog(catalog(5, 0, &[]));
    assert_eq!(
        r.vcx.read_entity(&g, |g, _| g.state()),
        GroupState::NoneFound
    );
    assert_eq!(
        r.vcx.read_entity(&g, |g, _| g.reason_text().to_string()),
        "None found"
    );
    assert!(r.has(part_id(1, "reason")));
    assert_eq!(r.group(&c, 1).entity_id(), g.entity_id());
    assert_eq!(r.vcx.read_entity(&c, |c, _| c.placement().clone()), cells);
}

/// A sample on one core notifies only that core's item. The group widgets, the other items,
/// the container and the page do not notify, and no entity is made or dropped.
#[gpui_kit::test]
fn overview_group_sample_notifies_changed_members_only(cx: &mut TestAppContext) {
    let mut r = open(cx, 4, &[0, 1]);
    let values: Vec<(u16, u64, f32)> = (0..4).map(|n| (n, 1, 10.)).collect();
    r.core_series(1, &values);
    let widgets = vec![
        group_widget(
            1,
            cores(GroupItems::Rows, None),
            ROWS_FORMS,
            cell(0, 0, 12, 4),
        ),
        group_widget(
            2,
            disks(GroupItems::Cells, None),
            CELL_FORMS,
            cell(0, 4, 12, 4),
        ),
    ];
    let c = r.container(layout(CPU_BOX_CONTAINER, widgets), 750., 480.);
    let (g_cores, g_disks) = (r.group(&c, 1), r.group(&c, 2));
    let items_before = (r.items(&g_cores), r.items(&g_disks));
    let cores_before = r.group_notifies(&g_cores);
    let disks_before = r.group_notifies(&g_disks);
    let container_before = r.container_notifies(&c);
    let first = r
        .vcx
        .read_entity(&g_cores, |g, _| g.items().nth(3).map(|(_, w)| w.clone()))
        .expect("core 3");
    let value = r.vcx.read_entity(&first, |w, _| w.texts()[1].to_string());
    assert!(value.starts_with("10"), "{value}");

    // Core 3 changes; the others keep their series.
    let mut values: Vec<(u16, u64, f32)> = (0..3).map(|n| (n, 1, 10.)).collect();
    values.push((3, 2, 70.));
    r.core_series(2, &values);

    let cores_after = r.group_notifies(&g_cores);
    assert_eq!(
        cores_after.0, cores_before.0,
        "the cores group does not notify"
    );
    for n in 0..3 {
        assert_eq!(
            cores_after.1[n], cores_before.1[n],
            "core {n} did not change"
        );
    }
    assert!(cores_after.1[3] > cores_before.1[3], "core 3 notifies");
    let value = r.vcx.read_entity(&first, |w, _| w.texts()[1].to_string());
    assert!(value.starts_with("70"), "{value}");
    assert_eq!(r.group_notifies(&g_disks), disks_before, "no disk changed");
    assert_eq!(r.container_notifies(&c), container_before);
    assert_eq!((r.items(&g_cores), r.items(&g_disks)), items_before);
    assert_eq!(r.group(&c, 1).entity_id(), g_cores.entity_id());
    assert_eq!(r.group(&c, 2).entity_id(), g_disks.entity_id());
}

/// A group and a single widget on `cpu:3` share one source.
#[gpui_kit::test]
fn overview_group_shares_member_source(cx: &mut TestAppContext) {
    let mut r = open(cx, 4, &[]);
    let single = WidgetSetting {
        id: 2,
        content: WidgetContent::Metric(SourceSelector {
            device: DeviceSelector::Key(Arc::from("cpu:3")),
            metric: MetricKey::new(METRIC_LOAD),
        }),
        forms: FormSet::VALUE | FormSet::LABEL,
        cell: Some(cell(0, 4, 12, 1)),
        options: WidgetOptions::default(),
        device_name: None,
    };
    let widgets = vec![
        group_widget(
            1,
            cores(GroupItems::Rows, None),
            ROWS_FORMS,
            cell(0, 0, 12, 4),
        ),
        single,
    ];
    let c = r.container(layout(CPU_BOX_CONTAINER, widgets), 750., 300.);
    let g = r.group(&c, 1);
    let single_source = r
        .vcx
        .read_entity(&c, |c, _| c.source(2).map(|s| s.entity_id()))
        .expect("the single widget's source");
    let member_source = r
        .vcx
        .read_entity(&g, |g, cx| {
            g.items()
                .nth(3)
                .map(|(_, w)| w.read(cx).source().entity_id())
        })
        .expect("core 3's item");
    assert_eq!(member_source, single_source);
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// The four edges of `b`, `t` px thick.
fn ring(b: Bounds<Pixels>, t: f32) -> [Bounds<Pixels>; 4] {
    let t = px(t);
    [
        Bounds::new(b.origin, size(b.size.width, t)),
        Bounds::new(point(b.left(), b.bottom() - t), size(b.size.width, t)),
        Bounds::new(b.origin, size(t, b.size.height)),
        Bounds::new(point(b.right() - t, b.top()), size(t, b.size.height)),
    ]
}

/// `a` and `b` share more than a rounding error of area.
fn covers(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    let e = px(0.25);
    a.left() < b.right() - e
        && b.left() < a.right() - e
        && a.top() < b.bottom() - e
        && b.top() < a.bottom() - e
}

impl Rig<'_> {
    /// An editor around a container showing `layout`, in a box `w × h` px, drawn.
    fn editor(
        &mut self,
        layout: ContainerLayout,
        w: f32,
        h: f32,
    ) -> (Entity<WidgetEditor>, Entity<WidgetContainer>) {
        let hub = self.hub.clone();
        let host = self.host.clone();
        let id = layout.container.clone();
        let out = self.vcx.update(|_, cx| {
            let store = Rc::new(RefCell::new(SourceStore::new(&hub)));
            let c = cx.new(|cx| WidgetContainer::new(id, layout, &hub, store, cx));
            let e = cx.new(|cx| WidgetEditor::new(c.clone(), &hub, cx));
            host.update(cx, |host, cx| {
                host.editor = Some((e.clone(), w, h));
                cx.notify();
            });
            (e, c)
        });
        draw(self.vcx);
        out
    }

    fn ed<R>(
        &mut self,
        e: &Entity<WidgetEditor>,
        f: impl FnOnce(&mut WidgetEditor, &mut Window, &mut Context<WidgetEditor>) -> R,
    ) -> R {
        let out = self
            .vcx
            .update(|window, cx| e.update(cx, |e, cx| f(e, window, cx)));
        draw(self.vcx);
        out
    }

    fn set_ui_scale(&mut self, pct: u16) {
        self.vcx.update(|_, cx| {
            let appearance = AppearanceSettings {
                ui_scale_pct: pct,
                ..AppearanceSettings::default()
            };
            let _ = tl_ui::theme::metrics::recompute(&appearance, cx);
        });
        draw(self.vcx);
    }

    /// The bounds of every text of group `id`'s items: their text parts and chart labels, and
    /// the group's own "+n more" line.
    fn group_texts(&mut self, g: &Entity<GroupWidget>, id: u32) -> Vec<(String, Bounds<Pixels>)> {
        let items: Vec<_> = self
            .vcx
            .read_entity(g, |g, _| g.items().map(|(_, w)| w.clone()).collect());
        let mut texts = Vec::new();
        for item in items {
            let (member, chart) = self
                .vcx
                .read_entity(&item, |w, _| (w.id(), w.chart().cloned()));
            for part in ["name", "value", "stats"] {
                if let Some(b) = self.vcx.debug_bounds(leak(part_id(member, part))) {
                    texts.push((format!("{member:x} {part}"), b));
                }
            }
            if let Some(chart) = chart {
                texts.extend(self.vcx.read_entity(&chart, |v, _| {
                    v.label_cache()
                        .painted
                        .iter()
                        .map(|p| (format!("{member:x} {:?}", p.kind), p.bounds()))
                        .collect::<Vec<_>>()
                }));
            }
        }
        if let Some(b) = self.vcx.debug_bounds(leak(part_id(id, "more"))) {
            texts.push(("more".to_owned(), b));
        }
        texts
    }
}

/// In Edit Widgets no edit decoration (grip, gear, border, outline, resize corner, the
/// preview's outline) covers any item text of a group, as Rows and as Cells, at scale 1.0, 1.5
/// and 2.0, selected or not. The items keep the column count and item kind of the applied view.
#[gpui_kit::test]
fn overview_group_edit_decorations_cover_no_text(cx: &mut TestAppContext) {
    let mut r = open(cx, 8, &[0, 1, 2]);
    let values: Vec<(u16, u64, f32)> = (0..8).map(|n| (n, 1, 40.)).collect();
    r.core_series(1, &values);
    let widgets = vec![
        group_widget(
            1,
            cores(GroupItems::Rows, Some(6)),
            ROWS_FORMS,
            cell(0, 0, 12, 6),
        ),
        group_widget(
            2,
            disks(GroupItems::Cells, None),
            CELL_FORMS,
            cell(0, 6, 12, 6),
        ),
    ];
    for pct in [100u16, 150, 200] {
        let scale = f32::from(pct) / 100.;
        let (e, c) = r.editor(
            layout(CPU_BOX_CONTAINER, widgets.clone()),
            720. * scale,
            640. * scale,
        );
        r.set_ui_scale(pct);
        let applied: Vec<(usize, usize, Composition)> = [1, 2]
            .into_iter()
            .map(|id| {
                let g = r.group(&c, id);
                r.vcx.read_entity(&g, |g, _| {
                    (g.fill().cols, g.fill().rows, g.fill().composition)
                })
            })
            .collect();
        r.ed(&e, |e, window, cx| e.start(window, cx));
        assert!(r.vcx.read_entity(&e, |e, _| e.is_editing()));
        for selected in [false, true] {
            for id in [1u32, 2] {
                let at = format!("{pct} % group {id} selected {selected}");
                if selected {
                    r.ed(&e, |e, _, cx| e.select(id, cx));
                }
                let g = r.group(&c, id);
                let fill = r.vcx.read_entity(&g, |g, _| {
                    (g.fill().cols, g.fill().rows, g.fill().composition)
                });
                assert_eq!(fill, applied[id as usize - 1], "{at}: the fit stays");
                let w = r.vcx.debug_bounds(leak(widget_id(id))).expect("widget");
                let frame = r.vcx.debug_bounds(leak(frame_id(id))).expect("frame");
                let mut decorations: Vec<(String, Bounds<Pixels>)> = vec![
                    (
                        "grip".into(),
                        r.vcx.debug_bounds(leak(grip_id(id))).expect("grip"),
                    ),
                    (
                        "gear".into(),
                        r.vcx.debug_bounds(leak(gear_id(id))).expect("gear"),
                    ),
                    (
                        "corner".into(),
                        r.vcx.debug_bounds(leak(corner_id(id))).expect("corner"),
                    ),
                ];
                decorations.extend(ring(frame, 1.).map(|b| ("border".to_owned(), b)));
                decorations.extend(ring(w, OUTLINE).map(|b| ("outline".to_owned(), b)));
                let texts = r.group_texts(&g, id);
                assert!(texts.len() >= 3, "{at}: texts {texts:?}");
                for (part, b) in &texts {
                    for (d, db) in &decorations {
                        assert!(!covers(*db, *b), "{at}: {d} {db:?} on {part} {b:?}");
                    }
                }
            }
        }
        r.ed(&e, |e, _, cx| e.cancel(cx));
    }
}
