//! The Overview Network box on widgets (ADR-019 phase 2 "Presets", UI-PRF-061 phase 2): each old
//! mode is a flow layout whose widgets draw the 1.2.0 parts where the box drew them, a sample
//! renders only the widgets whose text changed, an adapter that goes away shows its widget as
//! unavailable, Edit Widgets adds, removes, moves and applies Network widgets, and a reset
//! converted box reloads as the built-in default. Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, Focusable as _, IntoElement, ParentElement as _,
    Pixels, Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div,
    px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::DeviceIdx;
use tl_model::layout::{
    ContainerLayout, GroupKind, NET_BOX_CONTAINER, WidgetContent, box_layout, convert_old_settings,
};
use tl_model::layout_boxes::{DEVICE_NETS, METRIC_NET_RX, METRIC_NET_TRAFFIC};
use tl_model::settings::{
    AppearanceSettings, OverviewLayoutSetting, Settings, ThemeSettings, UiState,
};
use tl_model::system::{LinkState, Metric, NetTotals};
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
use tl_ui::pages::performance::overview::net_box::{
    COMBINED_CHART_ID, DOWN_RATE_ID, GRID_ID, NetBox, UP_RATE_ID, cell_id,
};
use tl_ui::pages::performance::overview::net_flow::{adapters_of, down_of, traffic_of, up_of};
use tl_ui::pages::performance::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui::pages::performance::sidebar::DeviceSidebar;
use tl_ui::pages::performance::widget::container::{WidgetContainer, container_id};
use tl_ui::pages::performance::widget::edit::{WidgetEditor, default_layout};
use tl_ui::pages::performance::widget::group_widget::{GroupFlowLook, GroupState};
use tl_ui::pages::performance::widget::metric_widget::{FlowLook, widget_id};
use tl_ui::pages::performance::widget::source::Availability;
use tl_ui::settings_global::TlSettings;
use tl_ui_testkit::fake_engine::FakeEngine;

/// The network total's catalog index.
const NET_AGG: u16 = 51;

/// The two adapters: (catalog index, ifindex, name, kind). wlan0 is the busiest: it uploads
/// 1 GB/s.
const ADAPTERS: [(u16, u32, &str, NetKind); 2] = [
    (30, 2, "eth0", NetKind::Wired),
    (33, 3, "wlan0", NetKind::Wireless),
];

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

/// The CPU package, memory, the network total and the two adapters; wlan0 is present only
/// when `wlan` is.
fn catalog(version: u64, wlan: bool) -> DeviceCatalog {
    let mut entries = vec![
        entry(0, DeviceIdentity::CpuPackage, "cpu", "CPU"),
        entry(1, DeviceIdentity::Memory, "memory", "Memory"),
        entry(
            NET_AGG,
            DeviceIdentity::Aggregate(AggregateKind::Nets),
            DEVICE_NETS,
            "Network",
        ),
    ];
    for (idx, ifindex, name, kind) in ADAPTERS {
        let identity = DeviceIdentity::Net {
            netns_ino: 1,
            ifindex,
            name: Arc::from(name),
        };
        let mut e = entry(idx, identity, &format!("net:{name}"), name);
        e.net_kind = Some(kind);
        e.present = wlan || name != "wlan0";
        entries.push(e);
    }
    DeviceCatalog {
        version,
        entries: Arc::from(entries),
    }
}

/// An adapter's record: `rx` bytes per second down, `tx` up.
fn totals(idx: u16, rx: f64, tx: f64) -> NetTotals {
    NetTotals {
        device: DeviceIdx(idx),
        rx_bps: Metric::valid(rx),
        tx_bps: Metric::valid(tx),
        rx_total: 1 << 30,
        tx_total: 1 << 20,
        link_speed_mbps: Metric::default(),
        state: LinkState::Up,
    }
}

/// The adapters' records: eth0 downloads `eth0` B/s, wlan0 downloads nothing and uploads
/// 1 GB/s, so the download sum is eth0's and wlan0 stays the busiest.
fn nets(eth0: f64) -> Arc<[(DeviceIdx, NetTotals)]> {
    Arc::from([
        (DeviceIdx(ADAPTERS[0].0), totals(ADAPTERS[0].0, eth0, 0.)),
        (DeviceIdx(ADAPTERS[1].0), totals(ADAPTERS[1].0, 0., 1e9)),
    ])
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    page: Entity<OverviewPage>,
    vcx: &'a mut VisualTestContext,
    _host: Entity<Host>,
    _driver: Entity<AnimationDriver>,
}

/// One window of `w × h` with the Overview page filling it and both adapters reporting.
fn open(cx: &mut TestAppContext, w: f32, h: f32) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog(1, true));
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        sys.nets = nets(1000.);
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
            kind: "net".into(),
            mode: mode.into(),
        });
    }

    fn net(&mut self) -> Entity<NetBox> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| page.read(cx).net_box().clone())
    }

    fn container(&mut self) -> Entity<WidgetContainer> {
        let net = self.net();
        self.vcx.update(|_, cx| net.read(cx).container().clone())
    }

    fn editor(&mut self) -> Entity<WidgetEditor> {
        let net = self.net();
        self.vcx.update(|_, cx| net.read(cx).editor().clone())
    }

    fn flow(&mut self) -> ContainerLayout {
        let net = self.net();
        self.vcx.update(|_, cx| net.read(cx).flow().clone())
    }

    fn bounds_of(&mut self, id: String) -> Bounds<Pixels> {
        let id = leak(id);
        self.vcx.debug_bounds(id).expect(id)
    }

    /// The placement of widget `id` in the container's body, px.
    fn placed(&mut self, id: u32) -> Option<Bounds<Pixels>> {
        let container = self.container();
        self.vcx
            .update(|_, cx| container.read(cx).placement().bounds(id))
    }

    /// New adapter records: eth0 downloads `eth0` B/s.
    fn publish_eth0(&mut self, eth0: f64) {
        self.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.nets = nets(eth0);
        });
        draw(self.vcx);
    }

    /// wlan0 goes away from the catalog, or comes back.
    fn publish_wlan(&mut self, present: bool) {
        self.engine.publish_with(|s| {
            s.devices = Arc::new(catalog(s.devices.version + 1, present));
        });
        draw(self.vcx);
    }
}

/// The look a placed widget draws: "down", "up", "combined", "cells", or what else it is.
fn look_of(r: &mut Rig, id: u32) -> &'static str {
    let container = r.container();
    r.vcx.update(|_, cx| {
        let c = container.read(cx);
        if let Some(g) = c.group(id) {
            match g.read(cx).flow_look() {
                Some(GroupFlowLook::NetCells { .. }) => "cells",
                _ => "other group",
            }
        } else if let Some(w) = c.widget(id) {
            match w.read(cx).flow() {
                Some(FlowLook::NetDirection { upload: false, .. }) => "down",
                Some(FlowLook::NetDirection { upload: true, .. }) => "up",
                Some(FlowLook::NetCombined { .. }) => "combined",
                _ => "other",
            }
        } else {
            "none"
        }
    })
}

/// UI-PRF-061 phase 2, ADR-019 "Presets": the default and each old mode of the Network box are
/// flow layouts whose widgets draw the 1.2.0 parts where the box drew them: the Download and
/// Upload parts split by the box's splitter in `stacked` and `columns`, the `combined` body, and
/// the adapter cells of the adapters group. The container places each widget from the box's
/// plan, Edit Widgets pins the drawn widgets to the grid, and Cancel keeps the preset.
#[gpui_kit::test]
fn overview_net_flow_matches_modes(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    let body = container_id(NET_BOX_CONTAINER);
    let modes: [(&str, Option<&str>); 6] = [
        ("stacked", None),
        ("columns", None),
        ("combined", None),
        ("adapter_charts", None),
        ("adapter_charts", Some("horizontal")),
        ("stacked", None),
    ];
    for (mode, arrange) in modes {
        r.set_mode(mode);
        if let Some(arrange) = arrange {
            r.page_action(&PerfOverviewSetBoxArrange {
                kind: "net".into(),
                arrange: arrange.into(),
            });
        }
        let flow = r.flow();
        assert_eq!(
            flow.preset.as_ref().map(|p| p.id.as_str()),
            Some(mode),
            "the mode is the preset"
        );
        let want: Vec<(u32, &str)> = match mode {
            "stacked" | "columns" => vec![
                (down_of(&flow).expect("rx"), "down"),
                (up_of(&flow).expect("tx"), "up"),
            ],
            "combined" => vec![(traffic_of(&flow).expect("traffic"), "combined")],
            _ => vec![(adapters_of(&flow).expect("the adapters group"), "cells")],
        };
        assert_eq!(
            flow.widgets.len(),
            want.len(),
            "{mode}: the preset's widgets"
        );
        let origin = r.bounds_of(body.clone());
        let mut placed = Vec::new();
        for (id, look) in &want {
            // Each widget is placed where the box drew it, as its own entity in the 1.2.0 look.
            let at = r
                .placed(*id)
                .unwrap_or_else(|| panic!("{mode} {arrange:?}: {id} placed"));
            let painted = r.bounds_of(widget_id(NET_BOX_CONTAINER, *id));
            assert!(
                near(painted.origin.x - origin.origin.x, at.origin.x)
                    && near(painted.origin.y - origin.origin.y, at.origin.y)
                    && near(painted.size.width, at.size.width)
                    && near(painted.size.height, at.size.height),
                "{mode} {arrange:?}: {id} painted {painted:?}, placed {at:?}"
            );
            assert_eq!(look_of(&mut r, *id), *look, "{mode}: widget {id}");
            placed.push((at, painted));
        }
        match mode {
            "stacked" | "columns" => {
                let (down, up) = (placed[0].0, placed[1].0);
                if mode == "stacked" {
                    assert!(near(down.size.width, origin.size.width), "full width");
                    assert!(up.origin.y > down.bottom(), "Upload under Download");
                    assert!(near(up.bottom(), origin.size.height), "to the bottom");
                } else {
                    assert!(near(down.size.height, origin.size.height), "full height");
                    assert!(up.origin.x > down.right(), "Upload right of Download");
                    assert!(near(up.right(), origin.size.width), "to the right edge");
                }
                // Each rate line is in its widget.
                for (line, (_, painted)) in [DOWN_RATE_ID, UP_RATE_ID].into_iter().zip(&placed) {
                    let b = r.bounds_of(line.to_owned());
                    assert!(painted.contains(&b.origin), "{mode}: {line} in its widget");
                }
            }
            "combined" => {
                let (at, painted) = placed[0];
                assert!(
                    near(at.size.width, origin.size.width)
                        && near(at.size.height, origin.size.height),
                    "the widget fills the body"
                );
                for part in [DOWN_RATE_ID, UP_RATE_ID, COMBINED_CHART_ID] {
                    let b = r.bounds_of(part.to_owned());
                    assert!(painted.contains(&b.origin), "{part} in the widget");
                }
            }
            _ => {
                let (at, painted) = placed[0];
                assert!(
                    near(at.size.width, origin.size.width)
                        && near(at.size.height, origin.size.height),
                    "the group fills the body"
                );
                let grid = r.bounds_of(GRID_ID.to_owned());
                assert!(painted.contains(&grid.origin), "the grid is in the group");
                for (idx, _, name, _) in ADAPTERS {
                    let b = r.bounds_of(cell_id(DeviceIdx(idx)));
                    assert!(
                        painted.contains(&b.origin) && b.bottom() <= painted.bottom() + px(0.5),
                        "{arrange:?}: {name} {b:?} in {painted:?}"
                    );
                }
                let net = r.net();
                assert_eq!(r.vcx.update(|_, cx| net.read(cx).cells()), 2);
            }
        }

        // Edit Widgets pins the drawn widgets to the grid, and Cancel puts the preset back.
        r.page_action(&PerfWidgetEditBox { kind: "net".into() });
        let editor = r.editor();
        let working = r
            .vcx
            .update(|_, cx| editor.read(cx).working_layout(cx))
            .expect("Edit Widgets is on");
        assert!(working.preset.is_none(), "{mode}: pinned to a grid layout");
        let pinned: Vec<u32> = working.widgets.iter().map(|w| w.id).collect();
        let drawn: Vec<u32> = want.iter().map(|(id, _)| *id).collect();
        assert_eq!(pinned, drawn, "{mode}: the drawn widgets, no more");
        assert!(working.widgets.iter().all(|w| w.cell.is_some()));
        r.vcx
            .update(|_, cx| editor.update(cx, |e, cx| e.cancel(cx)));
        draw(r.vcx);
        assert!(!r.vcx.read_entity(&editor, |e, _| e.is_editing()));
        assert_eq!(r.flow(), flow, "{mode}: Cancel keeps the flow layout");
    }
}

/// The render contract on widgets: a sample that changes the download rate notifies the widget
/// that draws it (the Download part, the `combined` widget or, through the adapter cells, the
/// adapters group), not the box, the container or the Upload part, and keeps the entities. (GPUI
/// draws the ancestors of a notified view again, so the box's render count is not checked here,
/// as in the other box widget tests.)
#[gpui_kit::test]
fn overview_net_sample_renders_only_its_widgets(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    // Steps of 1 MB/s (8 Mbps), so each sample changes the download text.
    let mut eth0 = 1000.;
    for mode in ["stacked", "combined", "adapter_charts"] {
        r.set_mode(mode);
        eth0 += 1_000_000.;
        r.publish_eth0(eth0);
        let net = r.net();
        let container = r.container();
        let flow = r.flow();
        let (changed, still) = match mode {
            "stacked" => (down_of(&flow), up_of(&flow)),
            "combined" => (traffic_of(&flow), None),
            _ => (adapters_of(&flow), None),
        };
        let changed = changed.expect("the widget a download sample changes");
        let entity = |r: &mut Rig, id: u32| {
            let container = container.clone();
            r.vcx.update(|_, cx| {
                let c = container.read(cx);
                c.widget(id)
                    .map(|w| w.entity_id())
                    .or_else(|| c.group(id).map(|g| g.entity_id()))
            })
        };
        let counts = |r: &mut Rig, id: u32| {
            let (net, container) = (net.clone(), container.clone());
            r.vcx.update(|_, cx| {
                let c = container.read(cx);
                let widget = c
                    .widget(id)
                    .map(|w| w.read(cx).notify_count.get())
                    .or_else(|| c.group(id).map(|g| g.read(cx).notify_count.get()))
                    .expect("a widget entity");
                [
                    net.read(cx).notify_count.get(),
                    c.notify_count.get(),
                    widget,
                ]
            })
        };
        let before_entity = entity(&mut r, changed);
        assert!(before_entity.is_some(), "{mode}: {changed} has an entity");
        let before = counts(&mut r, changed);
        let before_still = still.map(|id| counts(&mut r, id)[2]);
        eth0 += 1_000_000.;
        r.publish_eth0(eth0);
        let after = counts(&mut r, changed);
        assert_eq!(after[0], before[0], "{mode}: the box does not notify");
        assert_eq!(after[1], before[1], "{mode}: the container does not notify");
        assert_eq!(after[2], before[2] + 1, "{mode}: {changed} notifies once");
        assert_eq!(
            entity(&mut r, changed),
            before_entity,
            "{mode}: same entity"
        );
        if let (Some(id), Some(before)) = (still, before_still) {
            assert_eq!(
                counts(&mut r, id)[2],
                before,
                "{mode}: Upload does not notify"
            );
        }

        // A sample that changes nothing renders nothing.
        let before = counts(&mut r, changed);
        r.publish_eth0(eth0);
        assert_eq!(
            counts(&mut r, changed),
            before,
            "{mode}: an unchanged sample"
        );
    }
}

/// UI-PRF-061 phase 2 "Unavailable and empty": an adapter widget of the user's layout keeps its
/// cells and shows "Not connected" under the adapter's name while the adapter is gone, and its
/// data again when the adapter comes back. The adapters group drops the gone member and keeps
/// its entity.
#[gpui_kit::test]
fn overview_net_device_gone_unavailable(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    r.page_action(&PerfWidgetEditBox { kind: "net".into() });
    let editor = r.editor();
    let source = WidgetSource {
        device: Arc::from("net:wlan0"),
        metric: Arc::from(METRIC_NET_RX),
    };
    let rx = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source, cx)))
        .expect("wlan0 Download added");
    let group_id = r
        .vcx
        .update(|_, cx| {
            editor.update(cx, |e, cx| {
                e.add_group(DEVICE_NETS, "adapters", METRIC_NET_TRAFFIC, cx)
            })
        })
        .expect("All adapters added");
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    let container = r.container();
    let state = |r: &mut Rig| {
        let container = container.clone();
        r.vcx.update(|_, cx| {
            let c = container.read(cx);
            let w = c.widget(rx).expect("the Download widget").read(cx);
            let [name, _, _, reason] = w.texts();
            let group = c
                .group(group_id)
                .map(|g| (g.entity_id(), g.read(cx).state()));
            (
                w.availability(),
                name.to_string(),
                reason.to_string(),
                c.placement().bounds(rx),
                group,
            )
        })
    };
    let (avail, name, reason, cells, group) = state(&mut r);
    assert_eq!(avail, Availability::Available);
    assert!(name.contains("wlan0"), "{name}");
    assert_eq!(reason, "");
    let (group_entity, members) = group.expect("the group is placed");
    assert_eq!(members, GroupState::Members { count: 2 });

    r.publish_wlan(false);
    let (avail, name, reason, gone_cells, group) = state(&mut r);
    assert_eq!(avail, Availability::Gone);
    assert!(name.contains("wlan0"), "the gone adapter's name: {name}");
    assert_eq!(reason, "Not connected");
    assert_eq!(gone_cells, cells, "the widget keeps its cells");
    assert!(
        r.vcx
            .debug_bounds(leak(widget_id(NET_BOX_CONTAINER, rx)))
            .is_some()
    );
    assert_eq!(
        group,
        Some((group_entity, GroupState::Members { count: 1 }))
    );

    r.publish_wlan(true);
    let (avail, _, reason, _, group) = state(&mut r);
    assert_eq!(avail, Availability::Available, "the adapter came back");
    assert_eq!(reason, "");
    assert_eq!(
        group,
        Some((group_entity, GroupState::Members { count: 2 }))
    );
}

/// UI-PRF-061 phase 2: in Edit Widgets the Network box lists one submenu per counted adapter,
/// then "All adapters" and the "All adapters >" group, each with Download, Upload and Download
/// and upload; it adds an adapter widget, a total widget and an adapters group, removes, moves,
/// and Apply saves a grid layout that the box then draws.
#[gpui_kit::test]
fn overview_net_edit_add_remove_move_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    r.page_action(&PerfWidgetEditBox { kind: "net".into() });
    let editor = r.editor();
    assert!(r.vcx.read_entity(&editor, |e, _| e.is_editing()));

    let groups = r.vcx.update(|_, cx| editor.read(cx).source_groups(cx));
    let keys: Vec<&str> = groups.iter().map(|g| &*g.key).collect();
    assert_eq!(keys, ["net:eth0", "net:wlan0", DEVICE_NETS], "{groups:?}");
    let names: Vec<String> = groups.iter().map(|g| g.name.to_string()).collect();
    assert_eq!(names, ["eth0", "wlan0", "All adapters"]);
    for g in &groups {
        let labels: Vec<String> = g.items.iter().map(|i| i.label.to_string()).collect();
        assert_eq!(
            labels,
            ["Download", "Upload", "Download and upload"],
            "{}",
            g.key
        );
    }
    let add = groups[2].add_group.as_ref().expect("All adapters >");
    assert_eq!(add.label.as_ref(), "All adapters");
    assert_eq!(add.kind, GroupKind::Adapters);
    assert_eq!(add.metrics.len(), 3);
    assert!(groups[..2].iter().all(|g| g.add_group.is_none()));

    let source = |device: &str, metric: &str| WidgetSource {
        device: Arc::from(device),
        metric: Arc::from(metric),
    };
    let rx = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source("net:eth0", METRIC_NET_RX), cx)))
        .expect("eth0 Download added");
    let total = r
        .vcx
        .update(|_, cx| {
            editor.update(cx, |e, cx| {
                e.add(&source(DEVICE_NETS, METRIC_NET_TRAFFIC), cx)
            })
        })
        .expect("the total added");
    let adapters = r
        .vcx
        .update(|_, cx| {
            editor.update(cx, |e, cx| {
                e.add_group(DEVICE_NETS, "adapters", METRIC_NET_RX, cx)
            })
        })
        .expect("All adapters Download added");
    draw(r.vcx);
    let pinned = down_of(&r.flow()).expect("the pinned Download part");

    // Remove the pinned Download part, then move eth0 Download one cell toward the free side.
    r.vcx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            e.select(pinned, cx);
            assert!(e.remove_selected(cx));
            e.select(rx, cx);
        })
    });
    let cell_of = |r: &mut Rig| {
        r.vcx
            .update(|_, cx| editor.read(cx).working_layout(cx))
            .and_then(|l| l.widgets.into_iter().find(|w| w.id == rx))
            .and_then(|w| w.cell)
            .expect("Download cell")
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
                .known(NET_BOX_CONTAINER)
                .cloned()
        })
        .expect("an overview.net entry");
    assert!(entry.preset.is_none(), "a grid layout");
    let ids: Vec<u32> = entry.widgets.iter().map(|w| w.id).collect();
    assert!(
        ids.contains(&rx) && ids.contains(&total) && ids.contains(&adapters),
        "{ids:?}"
    );
    assert!(!ids.contains(&pinned), "removed: {ids:?}");
    let g = entry.widgets.iter().find(|w| w.id == adapters).unwrap();
    let WidgetContent::Group(g) = &g.content else {
        panic!("a group: {g:?}");
    };
    assert_eq!(g.members, GroupKind::Adapters);
    assert_eq!(g.metric.as_str(), METRIC_NET_RX);

    // The box draws the user's widgets now, each by its own entity.
    let net = r.net();
    assert!(r.vcx.update(|_, cx| net.read(cx).grid().is_some()));
    let container = r.container();
    let placed = r.vcx.update(|_, cx| container.read(cx).placed_ids());
    for id in [rx, total, adapters] {
        assert!(placed.contains(&id), "{id} in {placed:?}");
        assert!(
            r.vcx
                .debug_bounds(leak(widget_id(NET_BOX_CONTAINER, id)))
                .is_some(),
            "{id}"
        );
    }
    assert!(
        r.vcx
            .update(|_, cx| container.read(cx).group(adapters).is_some())
    );
}

/// A converted Network box (old `mode`, `split` and `arrange`), reset in Edit Widgets and
/// applied, reloads as the built-in default with no `overview.net` entry and no old keys
/// (ADR-019 "Saving the default").
#[gpui_kit::test]
fn overview_net_reset_survives_reload(cx: &mut TestAppContext) {
    let mut r = open(cx, 1200., 900.);
    let converted = r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            let mut layout = OverviewLayoutSetting::grid_default();
            let net = layout
                .grid
                .iter_mut()
                .find(|b| b.kind == "net")
                .expect("a Network box");
            net.mode = Some("combined".to_owned());
            net.split = Some(0.3);
            net.arrange = Some("horizontal".to_owned());
            d.settings.performance.overview = Some(layout);
            assert!(convert_old_settings(&mut d.settings.performance));
        });
        box_layout(&TlSettings::settings(cx).performance, "net").expect("net")
    });
    draw(r.vcx);
    assert_eq!(
        converted.preset.as_ref().map(|p| p.id.as_str()),
        Some("combined")
    );
    let net = r.net();
    assert_eq!(r.vcx.update(|_, cx| net.read(cx).mode()), "combined");

    r.page_action(&PerfWidgetEditBox { kind: "net".into() });
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
    assert!(perf.layouts.entry(NET_BOX_CONTAINER).is_none());
    let entry = perf
        .overview
        .as_ref()
        .and_then(|o| o.grid.iter().find(|b| b.kind == "net"))
        .expect("the Network box stays on the page");
    assert_eq!(entry.mode, None);
    assert_eq!(entry.split, None);
    assert_eq!(entry.arrange, None);
    let mut reloaded = perf.clone();
    assert!(!convert_old_settings(&mut reloaded), "nothing converts");
    assert_eq!(
        box_layout(&reloaded, "net").expect("net"),
        default_layout(NET_BOX_CONTAINER)
    );
    assert_eq!(r.vcx.update(|_, cx| net.read(cx).mode()), "stacked");
}
