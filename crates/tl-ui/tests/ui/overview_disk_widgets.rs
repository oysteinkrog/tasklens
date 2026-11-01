//! The Overview Disk box on widgets (ADR-019 phase 2 "Presets", UI-PRF-061 phase 2): each old
//! mode is a flow layout whose one widget fills the body and draws the 1.2.0 part, a disk sample
//! renders only the disks group, a disk that goes away shows its widget as unavailable, Edit
//! Widgets adds, removes, moves and applies Disk widgets, and a reset converted box reloads as
//! the built-in default. Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Pixels,
    Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::layout::{
    ContainerLayout, DISK_BOX_CONTAINER, GroupKind, WidgetContent, box_layout, convert_old_settings,
};
use tl_model::layout_boxes::{DEVICE_DISKS, METRIC_DISK_ACTIVE, METRIC_DISK_IO, METRIC_DISK_READ};
use tl_model::settings::{
    AppearanceSettings, OverviewLayoutSetting, Settings, ThemeSettings, UiState,
};
use tl_model::system::{DiskTotals, Metric};
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::DeviceKey;
use tl_ui::pages::performance::actions::{
    PerfOverviewSetBoxArrange, PerfOverviewSetBoxMode, PerfWidgetEditBox, WidgetSource,
};
use tl_ui::pages::performance::overview::disk_box::{
    DISK_ROW_ID_PREFIX, DiskBox, GRID_ID, TOTAL_CHART_ID, cell_id,
};
use tl_ui::pages::performance::overview::disk_flow::{disks_of, total_of};
use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui::pages::performance::sidebar::DeviceSidebar;
use tl_ui::pages::performance::widget::container::{WidgetContainer, container_id};
use tl_ui::pages::performance::widget::edit::{WidgetEditor, default_layout};
use tl_ui::pages::performance::widget::group_widget::{GroupFlowLook, GroupState};
use tl_ui::pages::performance::widget::metric_widget::{FlowLook, widget_id};
use tl_ui::pages::performance::widget::source::Availability;
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

const MIB: f64 = 1024. * 1024.;

/// The two disks: (catalog index, name).
const DISKS: [(u16, &str); 2] = [(20, "sda"), (21, "nvme0n1")];

struct Host {
    page: Entity<OverviewPage>,
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

fn disk_identity(minor: u32, name: &str) -> DeviceIdentity {
    DeviceIdentity::Disk {
        major: 8,
        minor,
        name: Arc::from(name),
        wwn_or_serial: None,
    }
}

/// The CPU package, memory, the disk total and the two disks; nvme0n1 is present only when
/// `nvme` is.
fn catalog(version: u64, nvme: bool) -> DeviceCatalog {
    let mut entries = vec![
        entry(0, DeviceIdentity::CpuPackage, "cpu", "CPU"),
        entry(1, DeviceIdentity::Memory, "memory", "Memory"),
        entry(
            2,
            DeviceIdentity::Aggregate(AggregateKind::Disks),
            DEVICE_DISKS,
            "Disks",
        ),
    ];
    for (i, (idx, name)) in DISKS.into_iter().enumerate() {
        let mut e = entry(
            idx,
            disk_identity(i as u32 * 16, name),
            &format!("disk:{name}"),
            name,
        );
        e.present = nvme || name != "nvme0n1";
        entries.push(e);
    }
    DeviceCatalog {
        version,
        entries: Arc::from(entries),
    }
}

/// Disk totals at `read` MiB/s and 40% active. sda reports a temperature, nvme0n1 none.
fn totals(idx: u16, read: f64) -> DiskTotals {
    DiskTotals {
        device: DeviceIdx(idx),
        active_pct: Metric::valid(40.),
        read_bps: Metric::valid(read * MIB),
        write_bps: Metric::valid(0.),
        response_ms: Metric::default(),
        read_total: 0,
        written_total: 0,
        capacity: 0,
        temp_c: if idx == DISKS[0].0 {
            Metric::valid(38.)
        } else {
            Metric::with(Validity::Unsupported)
        },
    }
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    page: Entity<OverviewPage>,
    vcx: &'a mut VisualTestContext,
    _host: Entity<Host>,
    _driver: Entity<AnimationDriver>,
}

/// One window of `w × h` with the Overview page filling it, and both disks reading 1 MiB/s.
fn open(cx: &mut TestAppContext, w: f32, h: f32) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog(1, true));
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        sys.disks = DISKS
            .iter()
            .map(|(i, _)| (DeviceIdx(*i), totals(*i, 1.)))
            .collect();
    });
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
        let page = cx.new(|cx| {
            OverviewPage::new(
                DeviceKey(Arc::from(OVERVIEW_KEY)),
                hub,
                demand,
                &sidebar,
                cx,
            )
        });
        Host { page, sidebar }
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(w), px(h)));
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    draw(vcx);
    let page = vcx.read_entity(&host, |h, _| h.page.clone());
    Rig {
        engine,
        page,
        vcx,
        _host: host,
        _driver: driver,
    }
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

fn near(a: Pixels, b: Pixels) -> bool {
    (f32::from(a) - f32::from(b)).abs() < 1.
}

fn leak(id: String) -> &'static str {
    Box::leak(id.into_boxed_str())
}

fn key(name: &str) -> DeviceKey {
    DeviceKey(Arc::from(format!("disk:{name}")))
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

    fn set_mode(&mut self, mode: &str) {
        self.page_action(&PerfOverviewSetBoxMode {
            kind: "disk".into(),
            mode: mode.into(),
        });
    }

    fn disk(&mut self) -> Entity<DiskBox> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| page.read(cx).disk_box().clone())
    }

    fn container(&mut self) -> Entity<WidgetContainer> {
        let disk = self.disk();
        self.vcx.update(|_, cx| disk.read(cx).container().clone())
    }

    fn editor(&mut self) -> Entity<WidgetEditor> {
        let disk = self.disk();
        self.vcx.update(|_, cx| disk.read(cx).editor().clone())
    }

    fn flow(&mut self) -> ContainerLayout {
        let disk = self.disk();
        self.vcx.update(|_, cx| disk.read(cx).flow().clone())
    }

    fn bounds_of(&mut self, id: String) -> gpui_kit::Bounds<Pixels> {
        let id = leak(id);
        self.vcx.debug_bounds(id).expect(id)
    }

    /// The placement of widget `id` in the container's body, px.
    fn placed(&mut self, id: u32) -> Option<gpui_kit::Bounds<Pixels>> {
        let container = self.container();
        self.vcx
            .update(|_, cx| container.read(cx).placement().bounds(id))
    }

    /// New disk totals: sda reads `read` MiB/s.
    fn publish_read(&mut self, read: f64) {
        self.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.disks = Arc::from([
                (DeviceIdx(DISKS[0].0), totals(DISKS[0].0, read)),
                (DeviceIdx(DISKS[1].0), totals(DISKS[1].0, 1.)),
            ]);
        });
        draw(self.vcx);
    }

    /// Takes the Network box off the page, so the Disk box has its room. Widget selectors have
    /// the container id (tasklens-6swdr.10), so other boxes do not shadow them.
    fn leave_out_net(&mut self) {
        use tl_ui::pages::performance::overview::net_box::NET_BOX_ID;
        self.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| {
                let perf = &mut d.settings.performance;
                let mut layout = perf
                    .overview
                    .clone()
                    .unwrap_or_else(OverviewLayoutSetting::grid_default);
                layout.grid.retain(|b| b.kind != "net");
                perf.overview = Some(layout);
            });
        });
        draw(self.vcx);
        assert!(
            self.vcx.debug_bounds(NET_BOX_ID).is_none(),
            "no Network box"
        );
    }

    /// nvme0n1 goes away from the catalog, or comes back.
    fn publish_nvme(&mut self, present: bool) {
        self.engine.publish_with(|s| {
            s.devices = Arc::new(catalog(s.devices.version + 1, present));
        });
        draw(self.vcx);
    }
}

/// UI-PRF-061 phase 2, ADR-019 "Presets": the default and each old mode of the Disk box are
/// flow layouts whose one widget fills the body and draws the 1.2.0 part: the disks group as
/// `disk_charts` cells or as sections, the total chart as a chart widget. The container places
/// the widget over the body from the box's plan, Edit Widgets pins it to the grid, and Cancel
/// keeps the preset.
#[gpui_kit::test]
fn overview_disk_flow_matches_modes(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    r.leave_out_net();
    let body = container_id(DISK_BOX_CONTAINER);
    let modes: [(&str, Option<&str>); 5] = [
        ("disk_charts", None),
        ("disk_charts", Some("auto")),
        ("disk_charts", Some("horizontal")),
        ("rows", None),
        ("total_chart", None),
    ];
    for (mode, arrange) in modes {
        r.set_mode(mode);
        if let Some(arrange) = arrange {
            r.page_action(&PerfOverviewSetBoxArrange {
                kind: "disk".into(),
                arrange: arrange.into(),
            });
        }
        let flow = r.flow();
        assert_eq!(
            flow.preset.as_ref().map(|p| p.id.as_str()),
            Some(mode),
            "the mode is the preset"
        );
        assert_eq!(flow.widgets.len(), 1, "{mode}: one widget");
        let id = match mode {
            "total_chart" => total_of(&flow).expect("the total chart"),
            _ => disks_of(&flow).expect("the disks group"),
        };
        // The widget fills the body, where the box drew its part.
        let origin = r.bounds_of(body.clone());
        let at = r
            .placed(id)
            .unwrap_or_else(|| panic!("{mode} {arrange:?}: {id} placed"));
        assert!(
            near(at.origin.x, px(0.))
                && near(at.origin.y, px(0.))
                && near(at.size.width, origin.size.width)
                && near(at.size.height, origin.size.height),
            "{mode} {arrange:?}: placed {at:?} in {origin:?}"
        );
        let part = match mode {
            "total_chart" => TOTAL_CHART_ID.to_owned(),
            _ => widget_id(DISK_BOX_CONTAINER, id),
        };
        let painted = r.bounds_of(part);
        assert!(
            near(painted.origin.x - origin.origin.x, at.origin.x)
                && near(painted.origin.y - origin.origin.y, at.origin.y)
                && near(painted.size.width, at.size.width)
                && near(painted.size.height, at.size.height),
            "{mode} {arrange:?}: painted {painted:?}, placed {at:?}"
        );
        // The widget is its own entity in the 1.2.0 look, and draws every disk inside it.
        let container = r.container();
        let look = r.vcx.update(|_, cx| {
            let c = container.read(cx);
            if let Some(g) = c.group(id) {
                match g.read(cx).flow_look() {
                    Some(GroupFlowLook::DiskCells { .. }) => "cells",
                    Some(GroupFlowLook::DiskRows { .. }) => "rows",
                    _ => "other group",
                }
            } else if let Some(w) = c.widget(id) {
                match w.read(cx).flow() {
                    Some(FlowLook::Chart(_)) => "chart",
                    _ => "other",
                }
            } else {
                "none"
            }
        });
        let want = match mode {
            "disk_charts" => "cells",
            "rows" => "rows",
            _ => "chart",
        };
        assert_eq!(look, want, "{mode}: widget {id}");
        if mode != "total_chart" {
            for (_, name) in DISKS {
                let part = match mode {
                    "rows" => format!("{DISK_ROW_ID_PREFIX}disk:{name}"),
                    _ => cell_id(&key(name)),
                };
                let b = r.bounds_of(part);
                assert!(
                    painted.contains(&b.origin) && b.bottom() <= painted.bottom() + px(0.5),
                    "{mode} {arrange:?}: {name} {b:?} in {painted:?}"
                );
            }
        }
        if mode == "disk_charts" {
            let grid = r.bounds_of(GRID_ID.to_owned());
            assert!(painted.contains(&grid.origin), "the grid is in the group");
        }

        // Edit Widgets pins the drawn widget to the grid, and Cancel puts the preset back.
        r.page_action(&PerfWidgetEditBox {
            kind: "disk".into(),
        });
        let editor = r.editor();
        let working = r
            .vcx
            .update(|_, cx| editor.read(cx).working_layout(cx))
            .expect("Edit Widgets is on");
        assert!(working.preset.is_none(), "{mode}: pinned to a grid layout");
        let pinned: Vec<u32> = working.widgets.iter().map(|w| w.id).collect();
        assert_eq!(pinned, [id], "{mode}: the drawn widget, no more");
        assert!(working.widgets.iter().all(|w| w.cell.is_some()));
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
        draw(r.vcx);
        assert!(!r.vcx.read_entity(&editor, |e, _| e.is_editing()));
        assert_eq!(r.flow(), flow, "{mode}: Cancel keeps the flow layout");
    }
}

/// The render contract on widgets: a disk sample notifies the disks group, not the box or the
/// container, and keeps the group's entity. The sections show the new rate. (GPUI draws the
/// ancestors of a notified view again, so the box's render count is not checked here, as in
/// the CPU and Memory widget tests.)
#[gpui_kit::test]
fn overview_disk_sample_renders_only_the_group(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    for mode in ["disk_charts", "rows"] {
        r.set_mode(mode);
        r.publish_read(2.);
        let disk = r.disk();
        let container = r.container();
        let id = disks_of(&r.flow()).expect("the disks group");
        let group = r
            .vcx
            .update(|_, cx| container.read(cx).group(id).cloned())
            .expect("the group's entity");
        let counts = |r: &mut Rig| {
            let (disk, container, group) = (disk.clone(), container.clone(), group.clone());
            r.vcx.update(|_, cx| {
                [
                    disk.read(cx).notify_count.get(),
                    disk.read(cx).render_count.get(),
                    container.read(cx).notify_count.get(),
                    group.read(cx).notify_count.get(),
                    group.read(cx).render_count.get(),
                ]
            })
        };
        let before = counts(&mut r);
        r.publish_read(5.);
        let after = counts(&mut r);
        assert_eq!(after[0], before[0], "{mode}: the box does not notify");
        assert_eq!(after[2], before[2], "{mode}: the container does not notify");
        assert_eq!(after[3], before[3] + 1, "{mode}: the group notifies once");
        assert!(after[4] > before[4], "{mode}: the group renders");
        let same = r
            .vcx
            .update(|_, cx| container.read(cx).group(id).map(|g| g.entity_id()));
        assert_eq!(same, Some(group.entity_id()), "{mode}: same entity");
        let read = r
            .vcx
            .update(|_, cx| disk.read(cx).rows()[0].read.to_string());
        assert!(read.starts_with('5'), "{mode}: {read}");

        // A sample that changes nothing renders nothing.
        let before = counts(&mut r);
        r.publish_read(5.);
        assert_eq!(counts(&mut r), before, "{mode}: an unchanged sample");
    }
}

/// UI-PRF-061 phase 2 "Unavailable and empty": a disk widget of the user's layout keeps its
/// cells and shows "Not connected" under the disk's name while the disk is gone, and its data
/// again when the disk comes back. The disks group drops the gone member and keeps its entity.
#[gpui_kit::test]
fn overview_disk_device_gone_unavailable(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    r.page_action(&PerfWidgetEditBox {
        kind: "disk".into(),
    });
    let editor = r.editor();
    let source = WidgetSource {
        device: Arc::from("disk:nvme0n1"),
        metric: Arc::from(METRIC_DISK_READ),
    };
    let read = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source, cx)))
        .expect("nvme0n1 Read added");
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    let container = r.container();
    let group_id = disks_of(&r.flow()).expect("the pinned disks group");
    let state = |r: &mut Rig| {
        let container = container.clone();
        r.vcx.update(|_, cx| {
            let c = container.read(cx);
            let w = c.widget(read).expect("the Read widget").read(cx);
            let [name, _, _, reason] = w.texts();
            let group = c
                .group(group_id)
                .map(|g| (g.entity_id(), g.read(cx).state()));
            (
                w.availability(),
                name.to_string(),
                reason.to_string(),
                c.placement().bounds(read),
                group,
            )
        })
    };
    let (avail, name, reason, cells, group) = state(&mut r);
    assert_eq!(avail, Availability::Available);
    assert!(name.contains("nvme0n1"), "{name}");
    assert_eq!(reason, "");
    let (group_entity, members) = group.expect("the group is placed");
    assert_eq!(members, GroupState::Members { count: 2 });

    r.publish_nvme(false);
    let (avail, name, reason, gone_cells, group) = state(&mut r);
    assert_eq!(avail, Availability::Gone);
    assert!(name.contains("nvme0n1"), "the gone disk's name: {name}");
    assert_eq!(reason, "Not connected");
    assert_eq!(gone_cells, cells, "the widget keeps its cells");
    assert!(
        r.vcx
            .debug_bounds(leak(widget_id(DISK_BOX_CONTAINER, read)))
            .is_some()
    );
    assert_eq!(
        group,
        Some((group_entity, GroupState::Members { count: 1 }))
    );

    r.publish_nvme(true);
    let (avail, _, reason, _, group) = state(&mut r);
    assert_eq!(avail, Availability::Available, "the disk came back");
    assert_eq!(reason, "");
    assert_eq!(
        group,
        Some((group_entity, GroupState::Members { count: 2 }))
    );
}

/// UI-PRF-061 phase 2: in Edit Widgets the Disk box lists one submenu per shown disk with the
/// metrics it reports, then "All disks" and the "All disks >" group; it adds a disk widget, a
/// total widget and a disks group, removes, moves, and Apply saves a grid layout that the box
/// then draws.
#[gpui_kit::test]
fn overview_disk_edit_add_remove_move_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    r.page_action(&PerfWidgetEditBox {
        kind: "disk".into(),
    });
    let editor = r.editor();
    assert!(r.vcx.read_entity(&editor, |e, _| e.is_editing()));

    let groups = r.vcx.update(|_, cx| editor.read(cx).source_groups(cx));
    let keys: Vec<&str> = groups.iter().map(|g| &*g.key).collect();
    assert_eq!(
        keys,
        ["disk:sda", "disk:nvme0n1", DEVICE_DISKS],
        "{groups:?}"
    );
    let names: Vec<String> = groups.iter().map(|g| g.name.to_string()).collect();
    assert_eq!(names, ["sda", "nvme0n1", "All disks"]);
    let labels = |i: usize| -> Vec<String> {
        groups[i]
            .items
            .iter()
            .map(|i| i.label.to_string())
            .collect()
    };
    assert_eq!(
        labels(0),
        [
            "Active time",
            "Read",
            "Write",
            "Read and write",
            "Temperature"
        ]
    );
    assert_eq!(
        labels(1),
        ["Active time", "Read", "Write", "Read and write"],
        "nvme0n1 reports no temperature"
    );
    assert_eq!(
        labels(2),
        ["Active time", "Read", "Write", "Read and write"]
    );
    let add = groups[2].add_group.as_ref().expect("All disks >");
    assert_eq!(add.label.as_ref(), "All disks");
    assert_eq!(add.kind, GroupKind::Disks);
    assert_eq!(add.metrics.len(), 4);
    assert!(groups[..2].iter().all(|g| g.add_group.is_none()));

    let source = |device: &str, metric: &str| WidgetSource {
        device: Arc::from(device),
        metric: Arc::from(metric),
    };
    let read = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source("disk:sda", METRIC_DISK_READ), cx)))
        .expect("sda Read added");
    let total = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source(DEVICE_DISKS, METRIC_DISK_IO), cx)))
        .expect("the total added");
    let active = r
        .vcx
        .update(|_, cx| {
            editor.update(cx, |e, cx| {
                e.add_group(DEVICE_DISKS, "disks", METRIC_DISK_ACTIVE, cx)
            })
        })
        .expect("All disks Active time added");
    draw(r.vcx);
    let pinned = disks_of(&r.flow()).expect("the pinned disks group");

    // Remove the pinned group, then move sda Read one cell toward the free side.
    r.vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.select(pinned, cx);
            assert!(e.remove_selected(cx));
            e.select(read, cx);
        })
    });
    let cell_of = |r: &mut Rig| {
        r.vcx
            .update(|_, cx| editor.read(cx).working_layout(cx))
            .and_then(|l| l.widgets.into_iter().find(|w| w.id == read))
            .and_then(|w| w.cell)
            .expect("Read cell")
    };
    let before = cell_of(&mut r);
    let dx: i32 = if before.col > 0 { -1 } else { 1 };
    assert!(
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.move_selected(dx, 0, cx)))
    );
    let after = cell_of(&mut r);
    assert_eq!(
        i64::from(after.col),
        i64::from(before.col) + i64::from(dx),
        "{before:?} → {after:?}"
    );
    let saved = r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx)));
    assert!(saved, "Apply saves");
    draw(r.vcx);

    let entry = r
        .vcx
        .update(|_, cx| {
            TlSettings::settings(cx)
                .performance
                .layouts
                .known(DISK_BOX_CONTAINER)
                .cloned()
        })
        .expect("an overview.disk entry");
    assert!(entry.preset.is_none(), "a grid layout");
    let ids: Vec<u32> = entry.widgets.iter().map(|w| w.id).collect();
    assert!(
        ids.contains(&read) && ids.contains(&total) && ids.contains(&active),
        "{ids:?}"
    );
    assert!(!ids.contains(&pinned), "removed: {ids:?}");
    let g = entry.widgets.iter().find(|w| w.id == active).unwrap();
    let WidgetContent::Group(g) = &g.content else {
        panic!("a group: {g:?}");
    };
    assert_eq!(g.members, GroupKind::Disks);
    assert_eq!(g.metric.as_str(), METRIC_DISK_ACTIVE);

    // The box draws the user's widgets now, each by its own entity.
    let disk = r.disk();
    assert!(r.vcx.update(|_, cx| disk.read(cx).grid().is_some()));
    let container = r.container();
    let placed = r.vcx.update(|_, cx| container.read(cx).placed_ids());
    for id in [read, total, active] {
        assert!(placed.contains(&id), "{id} in {placed:?}");
        assert!(
            r.vcx
                .debug_bounds(leak(widget_id(DISK_BOX_CONTAINER, id)))
                .is_some(),
            "{id}"
        );
    }
    assert!(
        r.vcx
            .update(|_, cx| container.read(cx).group(active).is_some())
    );
}

/// A converted Disk box (old `mode`, `split` and `arrange`), reset in Edit Widgets and
/// applied, reloads as the built-in default with no `overview.disk` entry and no old keys
/// (ADR-019 "Saving the default").
#[gpui_kit::test]
fn overview_disk_reset_survives_reload(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    let converted = r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            let mut layout = OverviewLayoutSetting::grid_default();
            let disk = layout
                .grid
                .iter_mut()
                .find(|b| b.kind == "disk")
                .expect("a Disk box");
            disk.mode = Some("rows".to_owned());
            disk.split = Some(0.3);
            disk.arrange = Some("horizontal".to_owned());
            d.settings.performance.overview = Some(layout);
            assert!(convert_old_settings(&mut d.settings.performance));
        });
        box_layout(&TlSettings::settings(cx).performance, "disk").expect("disk")
    });
    draw(r.vcx);
    assert_eq!(
        converted.preset.as_ref().map(|p| p.id.as_str()),
        Some("rows")
    );
    let disk = r.disk();
    assert_eq!(r.vcx.update(|_, cx| disk.read(cx).mode()), "rows");

    r.page_action(&PerfWidgetEditBox {
        kind: "disk".into(),
    });
    let editor = r.editor();
    assert!(
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.reset_confirmed(cx)))
    );
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);

    let perf = r
        .vcx
        .update(|_, cx| TlSettings::settings(cx).performance.clone());
    assert!(perf.layouts.entry(DISK_BOX_CONTAINER).is_none());
    let entry = perf
        .overview
        .as_ref()
        .and_then(|o| o.grid.iter().find(|b| b.kind == "disk"))
        .expect("the Disk box stays on the page");
    assert_eq!(entry.mode, None);
    assert_eq!(entry.split, None);
    assert_eq!(entry.arrange, None);
    let mut reloaded = perf.clone();
    assert!(!convert_old_settings(&mut reloaded), "nothing converts");
    assert_eq!(
        box_layout(&reloaded, "disk").expect("disk"),
        default_layout(DISK_BOX_CONTAINER)
    );
    assert_eq!(r.vcx.update(|_, cx| disk.read(cx).mode()), "disk_charts");
}
