//! Device sidebar card rendering (E24, UI-PRF-011). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Pixels,
    Render, SharedString, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::DeviceIdx;
use tl_model::settings::{AppearanceSettings, NetworkUnit, ThemeSettings};
use tl_model::snapshot::{DeviceDetailsSnapshot, DiskDetails, Snapshot, SystemSnapshot};
use tl_model::system::{DiskTotals, LinkState, Metric, NetTotals};
use tl_model::table::Total;
use tl_model::time::BootId;
use tl_model::validity::Validity;
use tl_ui::chart::view::ChartView;
use tl_ui::chart::{ChartSpec, ValueFormat};
use tl_ui::hub::SnapshotHub;
use tl_ui::pages::performance::actions::PerfSelectDevice;
use tl_ui::pages::performance::card::{
    CARD_CHART_BG, CardLines, CardPrepared, CardTextInput, CardWords, DeviceCard, card_chart_grid,
    card_chart_labels, card_lines, card_tooltip,
};
use tl_ui::pages::performance::{DeviceKey, DeviceKind};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::current;

const CARD_W: f32 = 244.;
/// 40 characters (crw.57 AC).
const LONG_GPU: &str = "NVIDIA GeForce RTX 4090 Laptop GPU 16 GB";

/// A debug selector built at run time.
fn sel(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// `inner` lies inside `outer`.
fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    inner.left() >= outer.left()
        && inner.right() <= outer.right()
        && inner.top() >= outer.top()
        && inner.bottom() <= outer.bottom()
}

struct Host {
    chart: Entity<ChartView>,
    lines: CardLines,
    selected: bool,
    focused: bool,
    edit_mode: bool,
    width: f32,
    clicked: Rc<RefCell<Vec<DeviceKey>>>,
    focus: FocusHandle,
}

impl Host {
    fn card(&self) -> DeviceCard {
        DeviceCard {
            key: DeviceKey(Arc::from("net:enp38s0")),
            chart: self.chart.clone(),
            lines: self.lines.clone(),
            selected: self.selected,
            focused: self.focused,
            edit_mode: self.edit_mode,
            hidden_in_edit: false,
            width: px(self.width),
            prepared: None,
        }
    }
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicked = Rc::clone(&self.clicked);
        div()
            .size_full()
            .p(px(20.))
            .track_focus(&self.focus)
            .on_action(move |a: &PerfSelectDevice, _, _| clicked.borrow_mut().push(a.key.clone()))
            .child(self.card())
    }
}

fn setup(cx: &mut App) {
    gpui_kit::init(cx);
    tl_ui::theme::store::init(
        cx,
        &ThemeSettings::default(),
        &AppearanceSettings::default(),
        Vec::new(),
    );
    tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
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

fn open(cx: &mut TestAppContext, lines: CardLines) -> (Entity<Host>, &mut VisualTestContext) {
    cx.update(setup);
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine;
    let hub = cx.new(|cx| SnapshotHub::new(client, cx));
    let (host, vcx) = cx.add_window_view(move |window, cx| {
        let spec = ChartSpec {
            grid: card_chart_grid(),
            background: CARD_CHART_BG,
            ..ChartSpec::default()
        };
        let chart = cx
            .new(|cx| ChartView::new(spec, card_chart_labels(ValueFormat::RateNetwork), &hub, cx));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Host {
            chart,
            lines,
            selected: false,
            focused: false,
            edit_mode: false,
            width: CARD_W,
            clicked: Rc::default(),
            focus,
        }
    });
    draw(vcx);
    (host, vcx)
}

fn entry(idx: u16, identity: DeviceIdentity, kind: Option<NetKind>) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from(""),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: kind,
    }
}

fn snap(system: SystemSnapshot, details: DeviceDetailsSnapshot) -> Snapshot {
    let mut s = Snapshot::empty(BootId(1), 1);
    s.system = Arc::new(system);
    s.device_details = Arc::new(details);
    s
}

fn lines_for(kind: DeviceKind, entry: &DeviceEntry, snap: &Snapshot) -> CardLines {
    let input = CardTextInput {
        words: CardWords::en(),
        kind,
        number: 0,
        entry,
        snap,
        network_unit: NetworkUnit::Bits,
        auto_scale: false,
        fan_label: None,
        fan_temp: None,
    };
    card_lines(&input, &mut String::new())
}

/// A wired interface whose operstate is `state`.
fn net_lines(state: LinkState) -> CardLines {
    let net = entry(
        2,
        DeviceIdentity::Net {
            netns_ino: 1,
            ifindex: 2,
            name: Arc::from("enp38s0"),
        },
        Some(NetKind::Wired),
    );
    let system = SystemSnapshot {
        nets: Arc::from(vec![(
            DeviceIdx(2),
            NetTotals {
                device: DeviceIdx(2),
                rx_bps: Metric::valid(150_000.0),
                tx_bps: Metric::valid(37_500.0),
                rx_total: 0,
                tx_total: 0,
                link_speed_mbps: Metric::valid(1000),
                state,
            },
        )]),
        ..SystemSnapshot::default()
    };
    lines_for(
        DeviceKind::Net,
        &net,
        &snap(system, DeviceDetailsSnapshot::default()),
    )
}

#[gpui_kit::test]
fn card_layout_and_click(cx: &mut TestAppContext) {
    let (host, cx) = open(cx, net_lines(LinkState::Up));
    let card = cx.debug_bounds("perf-card").expect("card");
    assert_eq!(card.size.width, px(CARD_W));
    assert_eq!(card.size.height, px(80.));
    let graph = cx.debug_bounds("perf-card-graph").expect("graph");
    assert_eq!((graph.size.width, graph.size.height), (px(116.), px(56.)));
    // 1 px border, then the sp5 inset.
    assert_eq!(graph.left() - card.left(), px(13.));
    assert_eq!(
        graph.center().y,
        card.center().y,
        "graph centred vertically"
    );
    let text = cx
        .debug_bounds("perf-card-text:net:enp38s0")
        .expect("text column");
    assert_eq!(text.left() - graph.right(), px(8.), "sp4 gap");
    let heights: Vec<_> = ["perf-card-heading", "perf-card-line1", "perf-card-line2"]
        .iter()
        .map(|s| cx.debug_bounds(s).expect("line").size.height)
        .collect();
    // "↓ 1.2 Mbps ↑ 0.3 Mbps" does not fit one row: line 2 wraps, and the card keeps 80 px.
    assert_eq!(heights, [px(18.), px(16.), px(32.)]);
    for (line, rows) in [("heading", 1), ("line1", 1), ("line2", 2)] {
        for i in 0..rows {
            let row = cx
                .debug_bounds(sel(format!("perf-card-row:net:enp38s0:{line}:{i}")))
                .expect("row");
            assert!(inside(row, text), "{line} {i}: {row:?} in {text:?}");
        }
        assert!(
            cx.debug_bounds(sel(format!("perf-card-row:net:enp38s0:{line}:{rows}")))
                .is_none()
        );
    }
    // The card draws the value; the graph draws none.
    let chart = cx.read_entity(&host, |h, _| h.chart.clone());
    let painted = cx.read_entity(&chart, |c, _| c.painted_value_text().map(str::to_owned));
    assert_eq!(painted, None);

    let at = card.center();
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    cx.run_until_parked();
    let clicked = cx.read_entity(&host, |h, _| h.clicked.borrow().clone());
    assert_eq!(clicked, [DeviceKey(Arc::from("net:enp38s0"))]);
}

/// crw.57 AC: a 40-character GPU name on a card of the default width (nav_width 252 less 2 ×
/// sp4) wraps to 2 rows. The rest of the name is cut in the middle, and the tooltip has the full
/// name. 18 + 2 × 16 + 16 px of text still fit the least card height.
#[gpui_kit::test]
fn card_long_gpu_name_wraps_then_middle_cut(cx: &mut TestAppContext) {
    assert_eq!(LONG_GPU.chars().count(), 40);
    let gpu = DeviceEntry {
        display_name: Arc::from(LONG_GPU),
        ..entry(
            9,
            DeviceIdentity::Gpu {
                uuid: [0; 16],
                pci_bus_id: Arc::from("0000:2d:00.0"),
            },
            None,
        )
    };
    let lines = lines_for(
        DeviceKind::Gpu,
        &gpu,
        &snap(SystemSnapshot::default(), DeviceDetailsSnapshot::default()),
    );
    assert_eq!(lines.line1, LONG_GPU);
    let (host, cx) = open(cx, lines.clone());
    host.update(cx, |h, cx| {
        h.width = 236.;
        cx.notify();
    });
    draw(cx);
    let prepared = cx.update(|window, cx| {
        let mut card = host.read(cx).card();
        card.prepare(window, cx).clone()
    });
    let line1 = &prepared.layout.line1;
    assert!(line1.cut);
    assert_eq!(line1.rows.len(), 2);
    assert!(LONG_GPU.starts_with(&*line1.rows[0].text), "{line1:?}");
    assert!(line1.rows[1].text.contains('\u{2026}'), "{line1:?}");
    assert!(line1.rows[1].text.ends_with("GPU 16 GB"), "{line1:?}");
    assert_eq!(
        &*card_tooltip(&lines, &prepared.layout),
        [SharedString::from(LONG_GPU)]
    );
    assert_eq!(prepared.height, px(80.));

    let card = cx.debug_bounds("perf-card").expect("card");
    assert_eq!(card.size, gpui_kit::size(px(236.), px(80.)));
    let text = cx
        .debug_bounds("perf-card-text:net:enp38s0")
        .expect("text column");
    for i in 0..2 {
        let row = cx
            .debug_bounds(sel(format!("perf-card-row:net:enp38s0:line1:{i}")))
            .expect("line 1 row");
        assert!(inside(row, text), "row {i}: {row:?} in {text:?}");
    }
}

#[gpui_kit::test]
fn card_disconnected_opacity(cx: &mut TestAppContext) {
    let connected = net_lines(LinkState::Up);
    let down = net_lines(LinkState::Down);
    assert_eq!(down.line2, "Disconnected");
    let (host, cx) = open(cx, down);
    let theme = cx.update(|_, cx| current(cx).clone());
    let card = cx.read_entity(&host, |h, _| h.card());
    assert_eq!(card.opacity(), 0.6);
    assert_eq!(card.line2_color(&theme), theme.text_secondary);
    assert!(cx.debug_bounds("perf-card-line2").is_some());

    let card = DeviceCard {
        lines: connected,
        ..card
    };
    assert_eq!(card.opacity(), 1.0);
    assert_eq!(card.line2_color(&theme), theme.accent);
    // A hidden card dims in edit mode only.
    let hidden = DeviceCard {
        hidden_in_edit: true,
        ..card
    };
    assert_eq!(hidden.opacity(), 1.0);
    assert_eq!(
        DeviceCard {
            edit_mode: true,
            ..hidden
        }
        .opacity(),
        0.6
    );
}

#[gpui_kit::test]
fn card_value_glyph_for_restricted(cx: &mut TestAppContext) {
    let disk = entry(
        3,
        DeviceIdentity::Disk {
            major: 259,
            minor: 0,
            name: Arc::from("nvme0n1"),
            wwn_or_serial: None,
        },
        None,
    );
    let details = DiskDetails {
        kind: Arc::from("NVMe"),
        model: Total {
            value: Arc::from("Samsung SSD 990 PRO 2TB"),
            validity: Validity::Valid,
        },
        ..DiskDetails::default()
    };
    let system = SystemSnapshot {
        disks: Arc::from(vec![(
            DeviceIdx(3),
            DiskTotals {
                device: DeviceIdx(3),
                active_pct: Metric::valid(0.1),
                read_bps: Metric::valid(0.0),
                write_bps: Metric::valid(0.0),
                response_ms: Metric::valid(0.0),
                read_total: 0,
                written_total: 0,
                capacity: 0,
                temp_c: Metric::with(Validity::Restricted),
            },
        )]),
        ..SystemSnapshot::default()
    };
    let s = snap(
        system,
        DeviceDetailsSnapshot {
            disks: Arc::from(vec![(DeviceIdx(3), details)]),
            ..DeviceDetailsSnapshot::default()
        },
    );
    let lines = lines_for(DeviceKind::Disk, &disk, &s);
    assert_eq!(lines.line2, "Samsung SSD 990 PRO 2TB · ");
    assert_eq!(&lines.line2_locks[..], &[lines.line2.len() as u16]);

    let (_, cx) = open(cx, lines);
    let line2 = cx.debug_bounds("perf-card-line2").expect("line 2");
    let lock = cx.debug_bounds("perf-card-lock").expect("lock glyph");
    assert!(
        line2.contains(&lock.center()),
        "the lock sits in line 2: {line2:?} {lock:?}"
    );
    assert_eq!(lock.size.width, px(12.), "i_xs");
}

#[gpui_kit::test]
fn card_selected_and_focus_outlines(cx: &mut TestAppContext) {
    let (host, cx) = open(cx, net_lines(LinkState::Up));
    assert!(cx.debug_bounds("perf-card-selected").is_none());
    assert!(cx.debug_bounds("perf-card-focus-ring").is_none());

    let set = |cx: &mut VisualTestContext, selected: bool, focused: bool, edit: bool| {
        host.update(cx, |h, cx| {
            h.selected = selected;
            h.focused = focused;
            h.edit_mode = edit;
            cx.notify();
        });
        draw(cx);
    };

    // Selected: one 2 px outline inside the card bounds, no focus ring.
    set(cx, true, false, false);
    let card = cx.debug_bounds("perf-card").expect("card");
    let border = cx
        .debug_bounds("perf-card-selected")
        .expect("selected border");
    assert_eq!(border, card, "drawn inside the card bounds");
    assert!(cx.debug_bounds("perf-card-focus-ring").is_none());

    // Selected and focused: the 1 px ring sits 2 px outside the card edge.
    set(cx, true, true, false);
    assert!(cx.debug_bounds("perf-card-selected").is_some());
    let ring = cx.debug_bounds("perf-card-focus-ring").expect("focus ring");
    assert_eq!(ring.left(), card.left() - px(3.));
    assert_eq!(ring.right(), card.right() + px(3.));
    assert_eq!(ring.top(), card.top() - px(3.));
    assert_eq!(ring.bottom(), card.bottom() + px(3.));

    // Edit mode draws no selected border.
    set(cx, true, true, true);
    assert!(cx.debug_bounds("perf-card-selected").is_none());
    assert!(cx.debug_bounds("perf-card-focus-ring").is_none());
}

/// The `DeviceSidebar` entity: cards, selection, keys, change-only text and demand (E24-004,
/// UI-PRF-011, UI-PRF-012).
mod sidebar {
    use gpui_kit::Focusable as _;
    use tl_model::demand::{CpuDemand, Demand};
    use tl_model::devices::DeviceCatalog;
    use tl_model::ids::{MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{Settings, UiState};
    use tl_model::snapshot::{ComponentHeader, SeriesSet};
    use tl_model::time::now_boot_ns;
    use tl_ui::anim::AnimationDriver;
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::chart::ChartState;
    use tl_ui::chart::labels::format_value;
    use tl_ui::components::toast::ToastQueue;
    use tl_ui::demand::DemandAggregator;
    use tl_ui::i18n::{init_strings, set_test_catalogue, tr_calls};
    use tl_ui::pages::performance::actions::{
        PerfSelectNextDevice, PerfSelectPrevDevice, PerfSelectVisibleDevice,
    };
    use tl_ui::pages::performance::overview::OVERVIEW_KEY as OVERVIEW;
    use tl_ui::pages::performance::sidebar::{CardAnim, DeviceSidebar, SidebarEvent};
    use tl_ui::settings_global::TlSettings;

    use super::*;

    const GPU: &str = "gpu:pci:0000:2d:00.0";
    const CPU_MODEL: &str = "AMD Ryzen 7 7800X3D 8-Core Processor";

    struct SidebarHost {
        sidebar: Entity<DeviceSidebar>,
        global_escapes: Rc<RefCell<u32>>,
        /// The sidebar width, 260 unless a test sets it.
        width: Rc<Cell<f32>>,
    }

    impl Render for SidebarHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let global_escapes = Rc::clone(&self.global_escapes);
            div()
                .key_context(tl_ui::actions::GLOBAL_CONTEXT)
                .on_action(move |_: &tl_ui::actions::EscapeChain, _, _| {
                    *global_escapes.borrow_mut() += 1
                })
                .w(px(self.width.get()))
                .h(px(900.))
                .child(self.sidebar.clone())
        }
    }

    struct Rig<'a> {
        sidebar: Entity<DeviceSidebar>,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        engine: Arc<FakeEngine>,
        events: Rc<RefCell<Vec<SidebarEvent>>>,
        /// The window's toast queue, when the rig has a kit `Root`.
        toasts: Option<Entity<ToastQueue>>,
        global_escapes: Rc<RefCell<u32>>,
        host_w: Rc<Cell<f32>>,
        vcx: &'a mut VisualTestContext,
    }

    fn disk(idx: u16, name: &str) -> DeviceEntry {
        entry(
            idx,
            DeviceIdentity::Disk {
                major: 8,
                minor: idx.into(),
                name: Arc::from(name),
                wwn_or_serial: None,
            },
            None,
        )
    }

    fn net(idx: u16, name: &str) -> DeviceEntry {
        entry(
            idx,
            DeviceIdentity::Net {
                netns_ino: 1,
                ifindex: idx.into(),
                name: Arc::from(name),
            },
            Some(NetKind::Wired),
        )
    }

    /// CPU, Memory, a GPU, four disks and two wired interfaces, in a scrambled catalog order.
    fn nine() -> Vec<DeviceEntry> {
        vec![
            net(7, "eth0"),
            disk(3, "sda"),
            entry(0, DeviceIdentity::CpuPackage, None),
            disk(4, "sdb"),
            entry(
                2,
                DeviceIdentity::Gpu {
                    uuid: [0; 16],
                    pci_bus_id: Arc::from("0000:2d:00.0"),
                },
                None,
            ),
            entry(1, DeviceIdentity::Memory, None),
            disk(5, "sdc"),
            net(8, "eth1"),
            disk(6, "sdd"),
        ]
    }

    /// A system snapshot with valid memory totals; `used` in GiB.
    fn system(used_gib: u64) -> SystemSnapshot {
        let mut sys = SystemSnapshot::default();
        sys.header.validity = Validity::Valid;
        sys.header.version = used_gib;
        sys.memory.validity = Validity::Valid;
        sys.memory.total = 64 << 30;
        sys.memory.used = used_gib << 30;
        sys.memory.available = sys.memory.total - sys.memory.used;
        sys.memory.committed = 20 << 30;
        sys.memory.commit_limit = 80 << 30;
        sys
    }

    fn open(
        cx: &mut TestAppContext,
        devices: Vec<DeviceEntry>,
        settings: Settings,
        state: UiState,
    ) -> Rig<'_> {
        open_in(cx, devices, settings, state, false)
    }

    /// `rooted`: the sidebar sits in a kit `Root` with a toast queue, as in the main window.
    /// The Overview card is hidden unless `settings` has an entry for it (D-022), so the
    /// device tests keep their card lists.
    fn open_in(
        cx: &mut TestAppContext,
        devices: Vec<DeviceEntry>,
        mut settings: Settings,
        state: UiState,
        rooted: bool,
    ) -> Rig<'_> {
        settings
            .performance
            .device_overrides
            .entry(OVERVIEW.to_owned())
            .or_insert(DeviceVisibility::Hide);
        let engine = FakeEngine::new();
        engine.publish_with(|s| {
            s.devices = Arc::new(DeviceCatalog {
                version: 1,
                entries: Arc::from(devices),
            });
            s.system = Arc::new(system(10));
            let name = &mut Arc::make_mut(&mut s.inventory).processor.name;
            name.value = Arc::from(CPU_MODEL);
            name.validity = Validity::Valid;
        });
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            setup(cx);
            init_strings(cx, "en-US");
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            cx.set_global(BootSettings {
                settings,
                state,
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        let (h, d) = (hub.clone(), demand.clone());
        let sidebar = cx.new(|cx| DeviceSidebar::new(h, d, cx));
        let events: Rc<RefCell<Vec<SidebarEvent>>> = Rc::default();
        let seen = Rc::clone(&events);
        cx.update(|cx| {
            cx.subscribe(&sidebar, move |_, ev: &SidebarEvent, _| {
                seen.borrow_mut().push(ev.clone())
            })
            .detach();
        });
        let host = sidebar.clone();
        let global_escapes = Rc::new(RefCell::new(0));
        let host_escapes = Rc::clone(&global_escapes);
        let host_w = Rc::new(Cell::new(260.));
        let width = Rc::clone(&host_w);
        let (vcx, toasts) = if rooted {
            cx.update(tl_ui::components::toast::init);
            let window = cx.add_window(move |window, cx| {
                let host = cx.new(|_| SidebarHost {
                    sidebar: host,
                    global_escapes: host_escapes,
                    width,
                });
                gpui_kit::base::Root::new(host, window, cx)
            });
            let vcx = VisualTestContext::from_window(window.into(), cx).into_mut();
            let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(40.))));
            (vcx, Some(queue))
        } else {
            let (_, vcx) = cx.add_window_view(move |_, _| SidebarHost {
                sidebar: host,
                global_escapes: host_escapes,
                width,
            });
            (vcx, None)
        };
        vcx.simulate_scale_factor_change(1.0);
        let focus = vcx.update(|_, cx| sidebar.read(cx).focus_handle(cx));
        vcx.update(|window, cx| focus.focus(window, cx));
        draw(vcx);
        Rig {
            sidebar,
            hub,
            demand,
            engine,
            events,
            toasts,
            global_escapes,
            host_w,
            vcx,
        }
    }

    fn keys(r: &mut Rig) -> Vec<String> {
        let sidebar = r.sidebar.clone();
        r.vcx.update(|_, cx| {
            sidebar
                .read(cx)
                .visible_keys()
                .map(|k| k.0.to_string())
                .collect()
        })
    }

    fn selected(r: &mut Rig) -> Option<String> {
        let sidebar = r.sidebar.clone();
        r.vcx
            .update(|_, cx| sidebar.read(cx).selected().map(|k| k.0.to_string()))
    }

    fn lines(r: &mut Rig, key: &str) -> CardLines {
        let sidebar = r.sidebar.clone();
        let key = DeviceKey(Arc::from(key));
        r.vcx.update(|_, cx| {
            sidebar
                .read(cx)
                .heading_for(&key)
                .expect("card exists")
                .clone()
        })
    }

    fn card_counts(r: &mut Rig) -> Vec<(String, u32, u32)> {
        let sidebar = r.sidebar.clone();
        r.vcx.update(|_, cx| {
            sidebar
                .read(cx)
                .cards()
                .iter()
                .filter(|card| !card.is_removing())
                .map(|card| {
                    let view = card.view.read(cx);
                    (
                        card.row.key.0.to_string(),
                        view.render_count.get(),
                        view.paint_count.get(),
                    )
                })
                .collect()
        })
    }

    fn publish_cpu_series(r: &mut Rig) {
        publish_cpu_series_at(r, now_boot_ns());
    }

    /// CPU total samples of 20% and 30%, 100 ms apart, the newest at `newest`.
    fn publish_cpu_series_at(r: &mut Rig, newest: u64) {
        publish_cpu_value_at(r, newest, 30.);
    }

    /// CPU total samples of 20% and `last`, 100 ms apart, the newest at `newest`.
    fn publish_cpu_value_at(r: &mut Rig, newest: u64, last: f32) {
        r.engine.publish_with(|s| {
            let version = s.series.header.version + 1;
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: Arc::from([Arc::new(SeriesView {
                    id: SeriesId {
                        device: DeviceIdx(0),
                        metric: MetricId::CPU_TOTAL,
                    },
                    version,
                    period_ns: 100_000_000,
                    samples: Arc::from([
                        Sample::valid(newest - 100_000_000, 20.),
                        Sample::valid(newest, last),
                    ]),
                })]),
                slow: Arc::from([]),
            });
        });
        draw(r.vcx);
    }

    fn cpu_series_card_deltas(
        cx: &mut TestAppContext,
        mut settings: Settings,
    ) -> Vec<(String, u32, u32)> {
        settings.motion.animations = false;
        settings.motion.smooth_graphs = false;
        let mut r = open(cx, nine(), settings, UiState::default());
        draw(r.vcx);
        let before = card_counts(&mut r);

        publish_cpu_series(&mut r);

        let after = card_counts(&mut r);
        before
            .into_iter()
            .zip(after)
            .map(
                |((key, renders, paints), (after_key, after_renders, after_paints))| {
                    assert_eq!(after_key, key);
                    (key, after_renders - renders, after_paints - paints)
                },
            )
            .collect()
    }

    fn no_disks() -> Settings {
        let mut s = Settings::default();
        s.performance.show_disks = false;
        s
    }

    fn saved(key: &str) -> UiState {
        let mut state = UiState::default();
        state.performance.selected_device = key.to_owned();
        state
    }

    #[gpui_kit::test]
    fn sidebar_nine_cards_in_type_order(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        assert_eq!(
            keys(&mut r),
            [
                "cpu", "memory", GPU, "disk:sda", "disk:sdb", "disk:sdc", "disk:sdd", "net:eth0",
                "net:eth1"
            ]
        );
        let sidebar = r.sidebar.clone();
        let (cards, rows) = r.vcx.update(|_, cx| {
            (
                sidebar.read(cx).cards().len(),
                sidebar.read(cx).rows().len(),
            )
        });
        // The hidden Overview card still has its row.
        assert_eq!((cards, rows), (9, 10));
        assert_eq!(lines(&mut r, "disk:sdc").heading, "Disk 2");
        // 260 px less sp4 padding on each side.
        let card = r.vcx.debug_bounds("perf-card").expect("card");
        assert_eq!(card.size.width, px(244.));
        assert_eq!(card.left(), px(8.));
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
    }

    #[gpui_kit::test]
    fn saved_selection_or_first_visible(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), saved("disk:sdb"));
        assert_eq!(selected(&mut r).as_deref(), Some("disk:sdb"));
        assert!(r.events.borrow().is_empty(), "construction emits nothing");
    }

    #[gpui_kit::test]
    fn saved_selection_hidden_selects_first(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), no_disks(), saved("disk:sdb"));
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
    }

    #[gpui_kit::test]
    fn f3_selects_third_visible_card_skipping_hidden(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), no_disks(), UiState::default());
        assert_eq!(keys(&mut r), ["cpu", "memory", GPU, "net:eth0", "net:eth1"]);
        r.vcx.dispatch_action(PerfSelectVisibleDevice { index: 2 });
        r.vcx.run_until_parked();
        assert_eq!(selected(&mut r).as_deref(), Some(GPU));
        assert_eq!(
            *r.events.borrow(),
            [SidebarEvent::Selected(DeviceKey(Arc::from(GPU)))]
        );
        // Past the last visible card: nothing.
        r.vcx.dispatch_action(PerfSelectVisibleDevice { index: 7 });
        r.vcx.run_until_parked();
        assert_eq!(selected(&mut r).as_deref(), Some(GPU));
    }

    #[gpui_kit::test]
    fn prev_at_first_is_noop(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
        r.vcx.dispatch_action(PerfSelectPrevDevice);
        r.vcx.run_until_parked();
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
        assert!(r.events.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn only_changed_card_reformats(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        let (cpu, memory) = (lines(&mut r, "cpu"), lines(&mut r, "memory"));
        let calls = tr_calls();

        r.engine.publish_with(|s| s.system = Arc::new(system(20)));
        draw(r.vcx);

        let (cpu2, memory2) = (lines(&mut r, "cpu"), lines(&mut r, "memory"));
        assert_ne!(memory2.line1, memory.line1, "memory re-formats");
        for (a, b) in [
            (&cpu.heading, &cpu2.heading),
            (&cpu.line1, &cpu2.line1),
            (&cpu.line2, &cpu2.line2),
        ] {
            assert_eq!(a, b);
            // Strings up to 23 bytes are stored inline and have no allocation to keep.
            if a.len() > 23 {
                assert_eq!(a.as_ptr(), b.as_ptr(), "CPU text keeps its strings");
            }
        }
        assert_eq!(cpu2.line1, CPU_MODEL);
        assert_eq!(
            tr_calls(),
            calls,
            "the sidebar translates nothing per frame"
        );
    }

    #[gpui_kit::test]
    fn unrelated_snapshot_keeps_card_leaves(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        let keys = keys(&mut r);
        let before_lines: Vec<_> = keys.iter().map(|key| lines(&mut r, key)).collect();
        let charts: Vec<_> = keys.iter().map(|key| chart_of(&mut r, key)).collect();
        let chart_renders = |r: &mut Rig| -> Vec<u32> {
            charts
                .iter()
                .map(|chart| r.vcx.read_entity(chart, |v, _| v.render_count.get()))
                .collect()
        };
        let before_renders = chart_renders(&mut r);
        let calls = tr_calls();
        r.engine
            .publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
        draw(r.vcx);
        // A tick whose values format to the same text redraws nothing either.
        r.engine.publish_with(|s| {
            let mut sys = system(10);
            sys.header.version = 99;
            s.system = Arc::new(sys);
        });
        draw(r.vcx);
        let after_lines: Vec<_> = keys.iter().map(|key| lines(&mut r, key)).collect();
        assert_eq!(after_lines, before_lines, "card text stays unchanged");
        assert_eq!(chart_renders(&mut r), before_renders, "charts stay cached");
        assert_eq!(tr_calls(), calls, "no card text is translated again");
    }

    #[gpui_kit::test]
    fn cpu_series_renders_and_paints_only_cpu_card_when_overview_hidden(cx: &mut TestAppContext) {
        let deltas = cpu_series_card_deltas(cx, Settings::default());
        for (key, renders, paints) in deltas {
            let expected = u32::from(key == "cpu");
            assert_eq!(renders, expected, "{key} render delta");
            assert_eq!(paints, expected, "{key} paint delta");
        }
    }

    #[gpui_kit::test]
    fn cpu_series_renders_and_paints_cpu_and_overview_cards(cx: &mut TestAppContext) {
        let deltas = cpu_series_card_deltas(cx, with_overview());
        for (key, renders, paints) in deltas {
            let expected = u32::from(key == "cpu" || key == OVERVIEW);
            assert_eq!(renders, expected, "{key} render delta");
            assert_eq!(paints, expected, "{key} paint delta");
        }
    }

    /// What the card of `key` measured on its last render.
    fn prepared(r: &mut Rig, key: &str) -> CardPrepared {
        let sidebar = r.sidebar.clone();
        let key = DeviceKey(Arc::from(key));
        r.vcx.update(|_, cx| {
            let view = sidebar.read(cx).card(&key).expect("card").view.clone();
            view.read(cx).prepared.clone().expect("card rendered")
        })
    }

    fn text_column(r: &mut Rig, key: &str) -> Bounds<Pixels> {
        r.vcx
            .debug_bounds(sel(format!("perf-card-text:{key}")))
            .unwrap_or_else(|| panic!("{key} text column"))
    }

    /// crw.57 AC: at 1200 × 900 with the default nav_width (252), no card text is clipped. Every
    /// row of every card, a 40-character GPU name included, lies inside its card, and each card
    /// is laid out at its measured height, so no card overlaps the next.
    #[gpui_kit::test]
    fn sidebar_cards_unclipped_at_1200x900(cx: &mut TestAppContext) {
        let mut devices = nine();
        for e in &mut devices {
            if matches!(e.identity, DeviceIdentity::Gpu { .. }) {
                e.display_name = Arc::from(LONG_GPU);
            }
        }
        let mut r = open(cx, devices, Settings::default(), UiState::default());
        r.vcx.simulate_resize(gpui_kit::size(px(1200.), px(900.)));
        r.host_w.set(252.);
        r.vcx.update(|window, _| window.refresh());
        // New width, then the cards' new heights.
        draw(r.vcx);
        draw(r.vcx);

        let card = r.vcx.debug_bounds("perf-card").expect("card");
        assert_eq!(card.size.width, px(236.), "252 less sp4 on each side");
        let keys = keys(&mut r);
        assert_eq!(keys.len(), 9);
        let mut above: Option<Bounds<Pixels>> = None;
        for key in &keys {
            let col = text_column(&mut r, key);
            let height = prepared(&mut r, key).height;
            assert!(height >= px(80.), "{key}");
            // The text column fills the card inside its 1 px border.
            assert_eq!(
                col.size.height + px(2.),
                height,
                "{key} laid out at its height"
            );
            let mut drawn = 0;
            for line in ["heading", "value", "line1", "line2"] {
                for i in 0..2 {
                    let id = sel(format!("perf-card-row:{key}:{line}:{i}"));
                    let Some(row) = r.vcx.debug_bounds(id) else {
                        continue;
                    };
                    drawn += 1;
                    assert!(inside(row, col), "{key} {line} row {i}: {row:?} in {col:?}");
                }
            }
            assert!(drawn >= 3, "{key}: a heading and two lines, got {drawn}");
            if let Some(above) = above {
                assert!(col.top() > above.bottom(), "{key} overlaps the card above");
            }
            above = Some(col);
        }
        // The GPU name wraps, then is cut in the middle.
        let line1 = prepared(&mut r, GPU).layout.line1;
        assert!(line1.cut && line1.rows.len() == 2, "{line1:?}");
        assert!(line1.rows[1].text.ends_with("GB"), "{line1:?}");
    }

    /// Every value row of every card lies inside its text column.
    fn assert_values_inside(r: &mut Rig, what: &str) {
        for key in keys(r) {
            let col = text_column(r, &key);
            for i in 0..2 {
                let id = sel(format!("perf-card-row:{key}:value:{i}"));
                if let Some(row) = r.vcx.debug_bounds(id) {
                    assert!(
                        inside(row, col),
                        "{what} {key} value {i}: {row:?} in {col:?}"
                    );
                }
            }
        }
    }

    /// At the least nav_width (180) and on the narrower edit-mode cards the graph gives way, so
    /// the widest percent ("100.0%") and every other value fit the text column.
    #[gpui_kit::test]
    fn values_fit_narrow_cards(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings.motion.animations = false;
        let mut r = open(cx, nine(), settings, UiState::default());
        r.vcx.simulate_resize(gpui_kit::size(px(1200.), px(900.)));
        r.host_w.set(180.);
        r.vcx.update(|window, _| window.refresh());
        draw(r.vcx);
        publish_cpu_value_at(&mut r, now_boot_ns(), 100.);
        draw(r.vcx);

        let value = prepared(&mut r, "cpu").value.expect("value");
        assert_eq!(&*value.text, "100.0%");
        let graph_w = prepared(&mut r, "cpu").graph_w;
        assert!(
            graph_w > px(0.) && graph_w < px(116.),
            "the graph narrows: {graph_w:?}"
        );
        assert_values_inside(&mut r, "nav_width 180");

        r.host_w.set(252.);
        r.vcx.update(|window, _| window.refresh());
        draw(r.vcx);
        assert_eq!(
            prepared(&mut r, "cpu").graph_w,
            px(116.),
            "full graph at 252"
        );
        enter_edit(&mut r);
        draw(r.vcx);
        assert_values_inside(&mut r, "edit mode");
    }

    /// The widest IEC rate before the unit steps up, "1023.9 KiB/s", fits a network card at the
    /// least nav_width (180). The text column once kept room only for "999.9 KiB/s". The edit-mode
    /// card there has an 82 px text column even with no graph, so the value takes 2 rows.
    #[gpui_kit::test]
    fn widest_network_byte_rate_fits_at_nav_width_180(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings.motion.animations = false;
        settings.units.network = NetworkUnit::Bytes;
        let mut r = open(cx, nine(), settings, UiState::default());
        r.vcx.simulate_resize(gpui_kit::size(px(1200.), px(900.)));
        r.host_w.set(180.);
        r.vcx.update(|window, _| window.refresh());
        draw(r.vcx);
        let newest = now_boot_ns();
        r.engine.publish_with(|s| {
            let version = s.series.header.version + 1;
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: Arc::from([Arc::new(SeriesView {
                    id: SeriesId {
                        device: DeviceIdx(7),
                        metric: MetricId::NET_RX,
                    },
                    version,
                    period_ns: 100_000_000,
                    samples: Arc::from([
                        Sample::valid(newest - 100_000_000, 1024.),
                        // 1023.9 × 1024 bytes per second.
                        Sample::valid(newest, 1_048_473.6),
                    ]),
                })]),
                slow: Arc::from([]),
            });
        });
        draw(r.vcx);
        draw(r.vcx);

        let value = prepared(&mut r, "net:eth0").value.expect("value");
        assert_eq!(&*value.text, "1023.9 KiB/s");
        assert_values_inside(&mut r, "nav_width 180, 1023.9 KiB/s");

        enter_edit(&mut r);
        draw(r.vcx);
        let col = text_column(&mut r, "net:eth0");
        assert_eq!(
            col.size.width,
            px(82.),
            "edit card: 180 - 2 × 8 - 52 - 2 × 8 - 2 - 12"
        );
        for i in 0..2 {
            let id = sel(format!("perf-card-row:net:eth0:value:{i}"));
            assert!(r.vcx.debug_bounds(id).is_some(), "edit value row {i}");
        }
        assert_values_inside(&mut r, "edit mode at nav_width 180, 1023.9 KiB/s");
    }

    /// A card with no room for its graph keeps the chart mounted, so a new sample still draws
    /// the cached card with the new value. At nav_width 180 and 200% UI scale the CPU card hides
    /// its graph. The chart once left the tree there, and the value stopped at its last number.
    #[gpui_kit::test]
    fn hidden_graph_card_value_follows_new_samples(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings.motion.animations = false;
        settings.motion.smooth_graphs = false;
        let mut r = open(cx, nine(), settings, UiState::default());
        r.vcx.update(|window, cx| {
            let appearance = AppearanceSettings {
                ui_scale_pct: 200,
                ..AppearanceSettings::default()
            };
            assert!(tl_ui::theme::metrics::recompute(&appearance, cx));
            window.refresh();
        });
        r.host_w.set(180.);
        r.vcx.update(|window, _| window.refresh());
        draw(r.vcx);
        publish_cpu_value_at(&mut r, now_boot_ns(), 30.);
        draw(r.vcx);
        let before = prepared(&mut r, "cpu");
        assert_eq!(before.graph_w, px(0.), "no graph: {before:?}");
        assert_eq!(&*before.value.expect("value").text, "30.0%");
        let cpu_renders = |r: &mut Rig| {
            card_counts(r)
                .into_iter()
                .find(|(key, ..)| key == "cpu")
                .map(|(_, renders, _)| renders)
                .expect("cpu card")
        };
        let renders = cpu_renders(&mut r);

        publish_cpu_value_at(&mut r, now_boot_ns(), 55.);

        assert_eq!(
            cpu_renders(&mut r),
            renders + 1,
            "the new sample draws the card"
        );
        let after = prepared(&mut r, "cpu");
        assert_eq!(after.graph_w, px(0.));
        assert_eq!(&*after.value.expect("value").text, "55.0%");
        assert_values_inside(&mut r, "nav_width 180 at 200%");
    }

    /// A line that gains a row grows its card on the next draw, and the cards below move down.
    #[gpui_kit::test]
    fn height_change_relays_out_sidebar(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        draw(r.vcx);
        let before = prepared(&mut r, "cpu");
        assert_eq!(before.layout.line2.rows.len(), 1, "{:?}", before.layout);
        assert_eq!(before.height, px(80.));
        let (cpu, memory) = (text_column(&mut r, "cpu"), text_column(&mut r, "memory"));

        r.engine.publish_with(|s| {
            let mut sys = system(10);
            sys.header.version = 50;
            sys.cpu.freq_mhz = Metric::valid(4631);
            sys.cpu.temp_c = Metric::valid(52.0);
            s.system = Arc::new(sys);
        });
        draw(r.vcx);
        draw(r.vcx);

        let after = prepared(&mut r, "cpu");
        assert_eq!(lines(&mut r, "cpu").line2, "4631 MHz · 52 °C");
        assert_eq!(after.layout.line2.rows.len(), 2, "{:?}", after.layout);
        let grown = after.height - before.height;
        assert!(grown > px(0.), "{after:?}");
        let (cpu2, memory2) = (text_column(&mut r, "cpu"), text_column(&mut r, "memory"));
        assert_eq!(cpu2.size.height - cpu.size.height, grown);
        assert_eq!(
            memory2.top() - memory.top(),
            grown,
            "the memory card moves down"
        );
    }

    /// The value sits in the heading row, read from the card's own series. The graph draws none.
    #[gpui_kit::test]
    fn card_value_in_heading_row(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        // A live chart asks the window's driver for its next frame.
        let _driver = r.vcx.update(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            driver
        });
        let warming = prepared(&mut r, "cpu").value.expect("value");
        assert_eq!(
            (&*warming.text, warming.validity),
            ("…", Validity::WarmingUp)
        );

        publish_cpu_series(&mut r);
        let p = prepared(&mut r, "cpu");
        let value = p.value.expect("value");
        assert_eq!((&*value.text, value.validity), ("30.0%", Validity::Valid));
        assert!(p.layout.value_inline);
        let chart = chart_of(&mut r, "cpu");
        let painted = r
            .vcx
            .read_entity(&chart, |c, _| c.painted_value_text().map(str::to_owned));
        assert_eq!(painted, None, "the graph draws no value");
        let heading = r
            .vcx
            .debug_bounds("perf-card-row:cpu:heading:0")
            .expect("heading row");
        let value = r
            .vcx
            .debug_bounds("perf-card-row:cpu:value:0")
            .expect("value");
        assert!(
            heading.contains(&value.center()),
            "{value:?} in {heading:?}"
        );
    }

    /// The value turns Stale when the samples stop: a Valid value draws its card again at the
    /// stale deadline without a new snapshot, not before and not after. The newest sample is 1 s
    /// old at the first draw and the stale floor is 1.5 s for a 100 ms period, so the deadline
    /// is 0.5 s away.
    #[gpui_kit::test]
    fn value_goes_stale_without_samples(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings.motion.animations = false;
        settings.motion.smooth_graphs = false;
        let mut r = open(cx, nine(), settings, UiState::default());
        let _driver = r.vcx.update(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            driver
        });
        publish_cpu_series_at(&mut r, now_boot_ns() - 1_000_000_000);
        let value = prepared(&mut r, "cpu").value.expect("value");
        assert_eq!(value.validity, Validity::Valid);
        let cpu_renders = |r: &mut Rig| {
            card_counts(r)
                .into_iter()
                .find(|(key, ..)| key == "cpu")
                .map(|(_, renders, _)| renders)
                .expect("cpu card")
        };
        let renders = cpu_renders(&mut r);

        // The sample age follows the real boot clock; the timer follows the test clock.
        let advance = |r: &mut Rig, ms: u64| {
            r.vcx
                .executor()
                .advance_clock(std::time::Duration::from_millis(ms));
            draw(r.vcx);
        };
        advance(&mut r, 400);
        assert_eq!(cpu_renders(&mut r), renders, "no draw before the deadline");
        std::thread::sleep(std::time::Duration::from_millis(700));
        advance(&mut r, 200);
        assert_eq!(cpu_renders(&mut r), renders + 1, "one draw at the deadline");
        let value = prepared(&mut r, "cpu").value.expect("value");
        assert_eq!(value.validity, Validity::Stale);
        assert_eq!(&*value.text, "30.0%", "a Stale value keeps its number");
        let theme = r.vcx.update(|_, cx| current(cx).clone());
        assert_eq!(value.color(&theme), theme.text_secondary);

        advance(&mut r, 5000);
        assert_eq!(
            cpu_renders(&mut r),
            renders + 1,
            "a Stale value sets no timer"
        );
    }

    /// An edit card is not a cached card view, so the sidebar draws it again at the stale
    /// deadline when the samples stop, as `value_goes_stale_without_samples` does for a normal
    /// card. Same timing: the deadline is 0.5 s after the first draw.
    #[gpui_kit::test]
    fn edit_card_value_goes_stale_without_samples(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings.motion.animations = false;
        settings.motion.smooth_graphs = false;
        let mut r = open(cx, nine(), settings, UiState::default());
        let _driver = r.vcx.update(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            driver
        });
        enter_edit(&mut r);
        publish_cpu_series_at(&mut r, now_boot_ns() - 1_000_000_000);
        draw(r.vcx);
        let sidebar = r.sidebar.clone();
        let armed = |r: &mut Rig| {
            r.vcx
                .update(|_, cx| sidebar.read(cx).edit_stale_at_for_test())
        };
        let renders = |r: &mut Rig| r.vcx.update(|_, cx| sidebar.read(cx).render_count.get());
        assert!(
            armed(&mut r).is_some(),
            "a Valid edit card value arms the timer"
        );
        let before = renders(&mut r);

        let advance = |r: &mut Rig, ms: u64| {
            r.vcx
                .executor()
                .advance_clock(std::time::Duration::from_millis(ms));
            draw(r.vcx);
        };
        advance(&mut r, 400);
        assert_eq!(renders(&mut r), before, "no draw before the deadline");
        std::thread::sleep(std::time::Duration::from_millis(700));
        advance(&mut r, 200);
        assert_eq!(renders(&mut r), before + 1, "one draw at the deadline");
        assert_eq!(
            armed(&mut r),
            None,
            "the value is Stale, so no timer is armed"
        );

        advance(&mut r, 5000);
        assert_eq!(renders(&mut r), before + 1, "a Stale value sets no timer");
    }

    #[gpui_kit::test]
    fn demand_follows_visible_kinds(cx: &mut TestAppContext) {
        let devices = vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
            disk(3, "sda"),
        ];
        let r = open(cx, devices, Settings::default(), UiState::default());
        r.vcx
            .executor()
            .advance_clock(std::time::Duration::from_millis(200));
        r.vcx.run_until_parked();
        let demand = r.demand.clone();
        let merged = r.vcx.update(|_, cx| demand.read(cx).merged());
        assert_eq!(
            merged,
            Demand {
                any_surface_visible: true,
                cpu: CpuDemand::Total,
                memory: true,
                disks: true,
                ..Demand::default()
            }
        );
        assert!(!merged.nets && !merged.gpus && !merged.sensors && !merged.cpu_fast);
    }

    /// Advances past the demand coalesce delay and returns the merge and every `set_demand`.
    fn settle_demand(r: &mut Rig) -> (Demand, Vec<Demand>) {
        r.vcx
            .executor()
            .advance_clock(std::time::Duration::from_millis(200));
        r.vcx.run_until_parked();
        let demand = r.demand.clone();
        let merged = r.vcx.update(|_, cx| demand.read(cx).merged());
        (merged, r.engine.demands())
    }

    fn set_active(r: &mut Rig, active: bool) {
        let sidebar = r.sidebar.clone();
        r.vcx
            .update(|_, cx| sidebar.update(cx, |s, cx| s.set_active(active, cx)));
    }

    #[gpui_kit::test]
    fn demand_withdrawn_while_another_tab_is_shown(cx: &mut TestAppContext) {
        let devices = vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
            disk(3, "sda"),
        ];
        let mut r = open(cx, devices, Settings::default(), UiState::default());
        let (merged, sent) = settle_demand(&mut r);
        assert!(merged.memory && merged.disks);

        // Staying on the tab: no new demand.
        set_active(&mut r, true);
        draw(r.vcx);
        set_active(&mut r, true);
        let (_, after) = settle_demand(&mut r);
        assert_eq!(after.len(), sent.len());

        // Another tab: the part goes and the engine hears it once.
        set_active(&mut r, false);
        let (merged, away) = settle_demand(&mut r);
        assert!(!merged.any_surface_visible && !merged.memory && !merged.disks);
        assert_eq!(away.len(), sent.len() + 1);
        assert_eq!(away.last(), Some(&merged));

        // A snapshot or a render while away sends nothing.
        r.engine.publish_with(|s| s.system = Arc::new(system(12)));
        draw(r.vcx);
        let (_, still) = settle_demand(&mut r);
        assert_eq!(still.len(), away.len());

        // Back on the tab: the part returns.
        set_active(&mut r, true);
        let (merged, back) = settle_demand(&mut r);
        assert!(merged.any_surface_visible && merged.memory && merged.disks);
        assert_eq!(back.len(), away.len() + 1);
        assert_eq!(back.last(), sent.last());
    }

    #[gpui_kit::test]
    fn select_persists_and_emits(cx: &mut TestAppContext) {
        let r = open(cx, nine(), Settings::default(), UiState::default());
        let sidebar = r.sidebar.clone();
        let key = DeviceKey(Arc::from("net:eth1"));
        let k = key.clone();
        r.vcx
            .update(|_, cx| sidebar.update(cx, |s, cx| s.select(k, cx)));
        r.vcx.run_until_parked();
        let saved = r
            .vcx
            .update(|_, cx| TlSettings::state(cx).performance.selected_device.clone());
        assert_eq!(saved, "net:eth1");
        assert_eq!(*r.events.borrow(), [SidebarEvent::Selected(key)]);
        draw(r.vcx);
        assert!(r.vcx.debug_bounds("perf-card-selected").is_some());
        assert!(
            r.vcx.debug_bounds("perf-card-focus-ring").is_some(),
            "the list has focus"
        );
    }

    #[gpui_kit::test]
    fn removed_selection_moves_to_next_below(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), saved("disk:sdb"));
        r.engine.publish_with(|s| {
            let entries: Vec<DeviceEntry> = nine()
                .into_iter()
                .filter(|e| e.idx != DeviceIdx(4))
                .collect();
            s.devices = Arc::new(DeviceCatalog {
                version: 2,
                entries: Arc::from(entries),
            });
        });
        draw(r.vcx);
        assert_eq!(selected(&mut r).as_deref(), Some("disk:sdc"));
        assert_eq!(lines(&mut r, "disk:sdc").heading, "Disk 1", "renumbered");
        settle(&mut r);
        assert_eq!(
            *r.events.borrow(),
            [
                SidebarEvent::Selected(DeviceKey(Arc::from("disk:sdc"))),
                SidebarEvent::Removed(DeviceKey(Arc::from("disk:sdb"))),
            ],
            "the card goes when its collapse ends"
        );
    }

    /// Runs the hotplug animations to their end.
    fn settle(r: &mut Rig) {
        r.vcx
            .executor()
            .advance_clock(std::time::Duration::from_millis(250));
        draw(r.vcx);
    }

    /// Every card in the list with its animation, collapsing ones included.
    fn anims(r: &mut Rig) -> Vec<(String, CardAnim)> {
        let sidebar = r.sidebar.clone();
        r.vcx.update(|_, cx| {
            sidebar
                .read(cx)
                .cards()
                .iter()
                .map(|c| (c.row.key.0.to_string(), c.anim))
                .collect()
        })
    }

    fn is_inserting(a: &CardAnim) -> bool {
        matches!(a, CardAnim::Inserting { .. })
    }

    fn is_removing(a: &CardAnim) -> bool {
        matches!(a, CardAnim::Removing { .. })
    }

    /// Installs a catalog of `entries` at `version`.
    fn plug(r: &mut Rig, version: u64, entries: Vec<DeviceEntry>) {
        r.engine.publish_with(|s| {
            s.devices = Arc::new(DeviceCatalog {
                version,
                entries: Arc::from(entries),
            });
        });
        draw(r.vcx);
    }

    fn cpu_and_memory() -> Vec<DeviceEntry> {
        vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
        ]
    }

    fn with_order(order: &[&str]) -> Settings {
        let mut s = Settings::default();
        s.performance.device_order = order.iter().map(|k| (*k).to_owned()).collect();
        s
    }

    #[gpui_kit::test]
    fn device_appears_in_saved_slot(cx: &mut TestAppContext) {
        let settings = with_order(&["cpu", "disk:sdz", "memory"]);
        let mut r = open(cx, cpu_and_memory(), settings, UiState::default());
        assert!(
            anims(&mut r).iter().all(|(_, a)| *a == CardAnim::Steady),
            "the first cards do not animate"
        );
        let mut entries = cpu_and_memory();
        entries.push(disk(9, "sdz"));
        plug(&mut r, 2, entries);
        assert_eq!(keys(&mut r), ["cpu", "disk:sdz", "memory"]);
        let a = anims(&mut r);
        assert_eq!(a[1].0, "disk:sdz");
        assert!(is_inserting(&a[1].1), "{a:?}");
        assert_eq!(a[0].1, CardAnim::Steady);

        settle(&mut r);
        assert_eq!(anims(&mut r)[1].1, CardAnim::Steady);
    }

    #[gpui_kit::test]
    fn selected_device_removed_selects_next(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), saved("disk:sdb"));
        let without: Vec<DeviceEntry> = nine()
            .into_iter()
            .filter(|e| e.idx != DeviceIdx(4))
            .collect();
        plug(&mut r, 2, without);
        // The selection moves at once; the card still collapses in its place.
        assert_eq!(selected(&mut r).as_deref(), Some("disk:sdc"));
        let a = anims(&mut r);
        let at = a
            .iter()
            .position(|(k, _)| k == "disk:sdb")
            .expect("collapsing");
        assert!(is_removing(&a[at].1));
        assert_eq!(a[at + 1].0, "disk:sdc", "in its old place");
        assert!(!keys(&mut r).contains(&"disk:sdb".to_owned()));
        let sdb = DeviceKey(Arc::from("disk:sdb"));
        assert!(
            !r.events
                .borrow()
                .contains(&SidebarEvent::Removed(sdb.clone()))
        );

        settle(&mut r);
        assert!(anims(&mut r).iter().all(|(k, _)| k != "disk:sdb"), "gone");
        assert!(
            r.events.borrow().contains(&SidebarEvent::Removed(sdb)),
            "the page goes with the card"
        );
    }

    #[gpui_kit::test]
    fn reduced_motion_skips_animation(cx: &mut TestAppContext) {
        let mut settings = with_order(&["cpu", "disk:sdz", "memory"]);
        settings.motion.animations = false;
        let mut r = open(cx, cpu_and_memory(), settings, UiState::default());
        let mut entries = cpu_and_memory();
        entries.push(disk(9, "sdz"));
        plug(&mut r, 2, entries);
        let a = anims(&mut r);
        assert_eq!(a[1], ("disk:sdz".to_owned(), CardAnim::Steady));

        // Removal is at once too.
        plug(&mut r, 3, cpu_and_memory());
        assert_eq!(anims(&mut r).len(), 2);
        assert!(
            r.events
                .borrow()
                .contains(&SidebarEvent::Removed(DeviceKey(Arc::from("disk:sdz"))))
        );
    }

    #[gpui_kit::test]
    fn removing_card_skipped_by_keys(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), saved("cpu"));
        let without: Vec<DeviceEntry> = nine()
            .into_iter()
            .filter(|e| e.idx != DeviceIdx(4))
            .collect();
        plug(&mut r, 2, without);
        assert!(
            anims(&mut r)
                .iter()
                .any(|(k, a)| k == "disk:sdb" && is_removing(a))
        );
        // cpu, memory, gpu, sda, sdc: the collapsing sdb does not count.
        r.vcx.dispatch_action(PerfSelectVisibleDevice { index: 4 });
        draw(r.vcx);
        assert_eq!(selected(&mut r).as_deref(), Some("disk:sdc"));
        // Down from sda goes past sdb.
        r.vcx.dispatch_action(PerfSelectVisibleDevice { index: 3 });
        r.vcx.dispatch_action(PerfSelectNextDevice);
        draw(r.vcx);
        assert_eq!(selected(&mut r).as_deref(), Some("disk:sdc"));
    }

    #[gpui_kit::test]
    fn replug_creates_new_chart(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), saved("cpu"));
        let old = chart_of(&mut r, "disk:sda").entity_id();
        let without: Vec<DeviceEntry> = nine()
            .into_iter()
            .filter(|e| e.idx != DeviceIdx(3))
            .collect();
        plug(&mut r, 2, without);
        settle(&mut r);
        let back: Vec<DeviceEntry> = nine()
            .into_iter()
            .map(|mut e| {
                if e.idx == DeviceIdx(3) {
                    e.generation = 1;
                }
                e
            })
            .collect();
        plug(&mut r, 3, back);
        let new = chart_of(&mut r, "disk:sda").entity_id();
        assert_ne!(new, old, "a new card with a new chart");
        assert!(keys(&mut r).contains(&"disk:sda".to_owned()));
    }

    fn chart_of(r: &mut Rig, key: &str) -> Entity<ChartView> {
        let sidebar = r.sidebar.clone();
        let key = DeviceKey(Arc::from(key));
        r.vcx.update(|_, cx| {
            sidebar
                .read(cx)
                .card(&key)
                .expect("card exists")
                .chart
                .clone()
        })
    }

    /// 60 one-second MEM_USED samples of `bytes` ending now.
    fn mem_used(bytes: f32) -> Arc<SeriesView> {
        const PERIOD: u64 = 1_000_000_000;
        let newest = now_boot_ns();
        Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(1),
                metric: MetricId::MEM_USED,
            },
            version: 1,
            period_ns: PERIOD,
            samples: (0..60u64)
                .map(|k| Sample::valid(newest - (59 - k) * PERIOD, bytes))
                .collect::<Vec<_>>()
                .into(),
        })
    }

    #[gpui_kit::test]
    fn memory_card_value_is_percent_of_total(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        // A live chart asks the window's driver for its next frame.
        let _driver = r.vcx.update(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            driver
        });
        r.engine.publish_with(|s| {
            let mut sys = system(8);
            sys.memory.total = 16 << 30;
            sys.memory.available = 8 << 30;
            s.system = Arc::new(sys);
        });
        draw(r.vcx);
        let chart = chart_of(&mut r, "memory");
        chart.update(r.vcx, |v, cx| {
            v.set_series_for_test([Some(mem_used(8.0 * (1u64 << 30) as f32)), None].into(), cx);
            v.set_state(ChartState::Live, cx);
        });
        draw(r.vcx);
        let (fmt, value) = r
            .vcx
            .read_entity(&chart, |v, _| (v.labels.value_fmt, v.value_now()));
        assert_eq!(fmt, ValueFormat::PercentOf { total: 16 << 30 });
        let (value, validity) = value.expect("newest value");
        assert_eq!(validity, Validity::Valid);
        let mut text = String::new();
        format_value(fmt, value, NetworkUnit::Bits, &mut text);
        assert_eq!(text, "50.0%", "8 GiB of 16 GiB");

        // A new format alone repaints the value but keeps the geometry.
        let (builds, renders) = r
            .vcx
            .read_entity(&chart, |v, _| (v.build_count.get(), v.render_count.get()));
        assert!(builds >= 1, "the chart has geometry");
        chart.update(r.vcx, |v, cx| {
            v.set_labels(
                card_chart_labels(ValueFormat::PercentOf { total: 32 << 30 }),
                cx,
            )
        });
        draw(r.vcx);
        let (builds2, renders2) = r
            .vcx
            .read_entity(&chart, |v, _| (v.build_count.get(), v.render_count.get()));
        assert!(renders2 > renders, "the chart repainted");
        assert_eq!(builds2, builds, "a label format change builds no geometry");

        // A new total reaches the format.
        r.engine.publish_with(|s| {
            let mut sys = system(8);
            sys.header.version = 99;
            sys.memory.total = 24 << 30;
            sys.memory.available = 16 << 30;
            s.system = Arc::new(sys);
        });
        draw(r.vcx);
        let fmt = r.vcx.read_entity(&chart, |v, _| v.labels.value_fmt);
        assert_eq!(fmt, ValueFormat::PercentOf { total: 24 << 30 });
    }

    /// A 1 Gbps link at full speed, 1.25e8 B/s, reaches the top of a link speed scale.
    #[gpui_kit::test]
    fn net_card_full_link_speed_reaches_top(cx: &mut TestAppContext) {
        use tl_ui::chart::YScale;
        let mut settings = Settings::default();
        settings.performance.network_dynamic_scale = false;
        let mut r = open(cx, nine(), settings, UiState::default());
        let _driver = r.vcx.update(|window, cx| {
            let driver = cx.new(AnimationDriver::new);
            AnimationDriver::register(&driver, window, cx);
            driver
        });
        r.engine.publish_with(|s| {
            let mut sys = system(10);
            sys.header.version = 77;
            sys.nets = Arc::from(
                [(
                    DeviceIdx(7),
                    NetTotals {
                        device: DeviceIdx(7),
                        rx_bps: Metric::valid(1.25e8),
                        tx_bps: Metric::valid(0.),
                        rx_total: 0,
                        tx_total: 0,
                        link_speed_mbps: Metric::valid(1000),
                        state: LinkState::Up,
                    },
                )]
                .as_slice(),
            );
            s.system = Arc::new(sys);
        });
        draw(r.vcx);
        let chart = chart_of(&mut r, "net:eth0");
        let y = r.vcx.read_entity(&chart, |v, _| v.spec.y);
        assert_eq!(y, YScale::LinkSpeed(1e9));
        let newest = now_boot_ns();
        let rx = Arc::new(SeriesView {
            id: SeriesId {
                device: DeviceIdx(7),
                metric: MetricId::NET_RX,
            },
            version: 1,
            period_ns: 1_000_000_000,
            samples: (0..60u64)
                .map(|k| Sample::valid(newest - (59 - k) * 1_000_000_000, 1.25e8))
                .collect::<Vec<_>>()
                .into(),
        });
        chart.update(r.vcx, |v, cx| {
            v.set_series_for_test([Some(rx), None].into(), cx);
            v.set_state(ChartState::Live, cx);
        });
        draw(r.vcx);
        let top = r
            .vcx
            .read_entity(&chart, |v, _| v.stroke_top_y())
            .expect("strokes");
        assert!(top.abs() <= 1.0, "the line sits at {top}, not the plot top");
    }

    #[gpui_kit::test]
    fn marker_catalogue_changes_card_words(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        assert_eq!(lines(&mut r, "memory").heading, "Memory");
        r.vcx.update(|_, cx| {
            set_test_catalogue(
                cx,
                "perf-card-heading-memory = [Memory]\n\
                 perf-card-heading-disk = [Disk {$n}]\n\
                 perf-card-commit = [Commit {$pct}]\n\
                 perf-net-type-ethernet = [Ethernet]\n",
            )
        });
        draw(r.vcx);
        let memory = lines(&mut r, "memory");
        assert_eq!(memory.heading, "[Memory]");
        assert_eq!(memory.line2, "[Commit 25.0%]");
        assert_eq!(lines(&mut r, "disk:sdc").heading, "[Disk 2]");
        assert_eq!(lines(&mut r, "net:eth0").line1, "[Ethernet]");
        assert_eq!(
            lines(&mut r, "cpu").heading,
            "CPU",
            "a missing key falls back"
        );
    }

    #[gpui_kit::test]
    fn sixty_frames_make_no_tr_calls(cx: &mut TestAppContext) {
        let r = open(cx, nine(), Settings::default(), UiState::default());
        let calls = tr_calls();
        for i in 0..60 {
            r.engine
                .publish_with(|s| s.system = Arc::new(system(10 + i % 7)));
            draw(r.vcx);
        }
        assert_eq!(tr_calls(), calls, "60 frames: 0 tr calls");
    }

    // ---- Card menu and page visibility (E24-011 part 1, UI-PRF-014, PERF-002) ----

    use gpui_kit::{Action, SharedString};
    use tl_model::settings::DeviceVisibility;
    use tl_ui::components::menu::{MenuEntryKind, TlMenu, open_context_menu};
    use tl_ui::pages::performance::actions::{
        PerfAddTrayGraph, PerfDisableAllPages, PerfEnableAllPages, PerfEnterEditMode,
        PerfMoveDevice, PerfResetPagesToDefault, PerfSetPageVisible, bind_keys_performance,
    };
    use tl_ui::pages::performance::menu::card_menu;

    fn key(s: &str) -> DeviceKey {
        DeviceKey(Arc::from(s))
    }

    type Entry = (
        MenuEntryKind,
        Option<String>,
        Option<String>,
        Option<Box<dyn Action>>,
    );

    fn entries(
        window: &Window,
        menu: &Entity<tl_ui::components::menu::TlMenuPopup>,
        cx: &App,
    ) -> Vec<Entry> {
        menu.read(cx)
            .entries_for_test(window)
            .into_iter()
            .map(|e| {
                (
                    e.kind,
                    e.label.map(|l| l.to_string()),
                    e.tooltip.map(|t| t.to_string()),
                    e.action,
                )
            })
            .collect()
    }

    /// `card_menu` built for `key` with the given state.
    fn built(
        r: &mut Rig,
        k: &str,
        count: usize,
        hidden: bool,
        edit_mode: bool,
        tray: Option<bool>,
    ) -> Vec<Entry> {
        let focus = r.vcx.update(|_, cx| r.sidebar.read(cx).focus_handle(cx));
        let k = key(k);
        r.vcx.update(|window, cx| {
            let menu = TlMenu::build(window, cx, focus, move |menu, _, cx| {
                card_menu(menu, &k, count, hidden, edit_mode, tray, cx)
            });
            entries(window, &menu, cx)
        })
    }

    fn labels(list: &[Entry]) -> Vec<(MenuEntryKind, &str)> {
        list.iter()
            .map(|(kind, label, _, _)| (*kind, label.as_deref().unwrap_or("")))
            .collect()
    }

    fn set_visible(k: &str, visible: bool) -> PerfSetPageVisible {
        PerfSetPageVisible {
            key: key(k),
            visible,
        }
    }

    /// The device overrides, without the Overview entry `open_in` adds.
    fn overrides(r: &mut Rig) -> Vec<(String, DeviceVisibility)> {
        r.vcx.update(|_, cx| {
            TlSettings::settings(cx)
                .performance
                .device_overrides
                .iter()
                .filter(|(k, _)| *k != OVERVIEW)
                .map(|(k, v)| (k.clone(), *v))
                .collect()
        })
    }

    fn overview_override(r: &mut Rig) -> Option<DeviceVisibility> {
        r.vcx.update(|_, cx| {
            TlSettings::settings(cx)
                .performance
                .device_overrides
                .get(OVERVIEW)
                .copied()
        })
    }

    /// Settings with the Overview card on by its type default.
    fn with_overview() -> Settings {
        let mut s = Settings::default();
        s.performance
            .device_overrides
            .insert(OVERVIEW.to_owned(), DeviceVisibility::Show);
        s
    }

    fn edit_fixture() -> Vec<DeviceEntry> {
        vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
            disk(2, "sda"),
            disk(3, "sdb"),
            net(4, "eth0"),
            net(5, "eth1"),
        ]
    }

    fn reorder_fixture() -> Vec<DeviceEntry> {
        vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
            disk(2, "sda"),
            disk(3, "sdb"),
            disk(4, "sdc"),
            net(5, "eth0"),
        ]
    }

    fn saved_order(r: &mut Rig) -> Vec<String> {
        r.vcx
            .update(|_, cx| TlSettings::settings(cx).performance.device_order.clone())
    }

    fn enter_edit(r: &mut Rig) {
        r.vcx.dispatch_action(PerfEnterEditMode);
        draw(r.vcx);
        let sidebar = r.sidebar.clone();
        assert!(r.vcx.update(|_, cx| sidebar.read(cx).edit_mode()));
    }

    #[gpui_kit::test]
    fn edit_mode_groups_and_empty_states(cx: &mut TestAppContext) {
        let mut r = open(cx, edit_fixture(), Settings::default(), UiState::default());
        enter_edit(&mut r);
        let mut previous = px(0.);
        let selectors = [
            "perf-edit-group-0",
            "perf-edit-group-1",
            "perf-edit-group-2",
            "perf-edit-group-3",
            "perf-edit-group-4",
            "perf-edit-group-5",
            "perf-edit-group-6",
        ];
        for (group, selector) in selectors.into_iter().enumerate() {
            let bounds = r.vcx.debug_bounds(selector).expect("group header");
            assert!(bounds.top() >= previous, "group {group} is ordered");
            previous = bounds.top();
        }
        assert!(r.vcx.debug_bounds("perf-edit-group-7").is_none());
        assert!(r.vcx.debug_bounds("perf-edit-empty-3").is_some());
        assert!(r.vcx.debug_bounds("perf-edit-empty-5").is_some());
        assert!(r.vcx.debug_bounds("perf-edit-empty-6").is_some());
    }

    #[gpui_kit::test]
    fn edit_mode_hidden_card_switch_off(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings
            .performance
            .device_overrides
            .insert("disk:sdb".into(), DeviceVisibility::Hide);
        let mut r = open(cx, edit_fixture(), settings, UiState::default());
        enter_edit(&mut r);
        let sidebar = r.sidebar.clone();
        let rows = r.vcx.update(|_, cx| sidebar.read(cx).edit_rows_for_test());
        let disk = rows
            .iter()
            .find(|(key, _, _)| &*key.0 == "disk:sdb")
            .expect("hidden disk remains in edit mode");
        assert!(!disk.2, "the switch is rendered unchecked");
        let opacity = r.vcx.update(|_, cx| {
            sidebar
                .read(cx)
                .edit_card_opacity_for_test(&key("disk:sdb"))
                .expect("edit card")
        });
        assert_eq!(opacity, 0.6);
        assert!(r.vcx.debug_bounds("perf-edit-row-disk:sdb").is_some());
        let card = r.vcx.debug_bounds("perf-card").expect("edit card");
        // The narrow edit card wraps its text, so it may grow past the least height.
        assert!(card.size.height >= px(80.), "{card:?}");
    }

    #[gpui_kit::test]
    fn disable_all_keeps_selected_and_toasts_once(cx: &mut TestAppContext) {
        let mut r = open_in(
            cx,
            edit_fixture(),
            Settings::default(),
            saved("memory"),
            true,
        );
        enter_edit(&mut r);
        r.vcx.dispatch_action(PerfDisableAllPages);
        draw(r.vcx);
        assert_eq!(selected(&mut r).as_deref(), Some("memory"));
        assert_eq!(
            overrides(&mut r),
            [
                ("cpu".into(), DeviceVisibility::Hide),
                ("disk:sda".into(), DeviceVisibility::Hide),
                ("disk:sdb".into(), DeviceVisibility::Hide),
                ("net:eth0".into(), DeviceVisibility::Hide),
                ("net:eth1".into(), DeviceVisibility::Hide),
            ]
        );
        let queue = r.toasts.clone().expect("toast queue");
        let shown = r.vcx.update(|_, cx| queue.read(cx).visible_texts());
        assert_eq!(shown, ["At least one performance page must stay visible"]);
    }

    #[gpui_kit::test]
    fn enable_all_writes_show_for_off_defaults(cx: &mut TestAppContext) {
        let mut devices = edit_fixture();
        devices.push(entry(
            6,
            DeviceIdentity::Net {
                netns_ino: 1,
                ifindex: 6,
                name: Arc::from("veth0"),
            },
            Some(NetKind::Virtual),
        ));
        let mut settings = Settings::default();
        settings.performance.show_network_virtual = false;
        settings
            .performance
            .device_overrides
            .insert("disk:sda".into(), DeviceVisibility::Hide);
        let mut r = open(cx, devices, settings, UiState::default());
        enter_edit(&mut r);
        r.vcx.dispatch_action(PerfEnableAllPages);
        draw(r.vcx);
        let values = overrides(&mut r);
        assert!(
            values.contains(&("net:veth0".into(), DeviceVisibility::Show)),
            "an off-by-default virtual interface gets Show"
        );
        assert!(
            values
                .iter()
                .all(|(_, value)| *value != DeviceVisibility::Hide),
            "all Hide overrides are gone"
        );
    }

    #[gpui_kit::test]
    fn reset_clears_order_and_overrides(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings.performance.device_order = vec!["disk:sdb".into(), "cpu".into()];
        settings
            .performance
            .device_overrides
            .insert("disk:sda".into(), DeviceVisibility::Hide);
        let mut r = open(cx, edit_fixture(), settings, UiState::default());
        enter_edit(&mut r);
        r.vcx.dispatch_action(PerfResetPagesToDefault);
        draw(r.vcx);
        let (order, overrides) = r.vcx.update(|_, cx| {
            let settings = &TlSettings::settings(cx).performance;
            (
                settings.device_order.clone(),
                settings.device_overrides.clone(),
            )
        });
        assert!(order.is_empty());
        assert!(overrides.is_empty());
    }

    #[gpui_kit::test]
    fn move_device_rewrites_order_and_numbering(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            reorder_fixture(),
            Settings::default(),
            UiState::default(),
        );
        enter_edit(&mut r);
        r.vcx.dispatch_action(PerfMoveDevice {
            key: key("disk:sdc"),
            before: Some(key("disk:sda")),
        });
        draw(r.vcx);

        assert_eq!(
            saved_order(&mut r),
            [
                "overview", "cpu", "memory", "disk:sdc", "disk:sda", "disk:sdb", "net:eth0"
            ]
        );
        assert_eq!(lines(&mut r, "disk:sdc").heading, "Disk 0");
        assert_eq!(lines(&mut r, "disk:sda").heading, "Disk 1");
    }

    #[gpui_kit::test]
    fn move_lone_group_device_to_end_keeps_position(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            reorder_fixture(),
            Settings::default(),
            UiState::default(),
        );
        enter_edit(&mut r);
        r.vcx.dispatch_action(PerfMoveDevice {
            key: key("memory"),
            before: None,
        });
        draw(r.vcx);

        assert_eq!(
            saved_order(&mut r),
            [
                "overview", "cpu", "memory", "disk:sda", "disk:sdb", "disk:sdc", "net:eth0"
            ]
        );
    }

    #[gpui_kit::test]
    fn reorder_within_group_only(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            reorder_fixture(),
            Settings::default(),
            UiState::default(),
        );
        enter_edit(&mut r);
        let before = keys(&mut r);
        r.vcx.dispatch_action(PerfMoveDevice {
            key: key("disk:sdc"),
            before: Some(key("net:eth0")),
        });
        draw(r.vcx);

        assert_eq!(keys(&mut r), before);
        assert!(
            saved_order(&mut r).is_empty(),
            "cross-group drop is ignored"
        );
    }

    #[gpui_kit::test]
    fn drag_cross_group_and_outside_release(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            reorder_fixture(),
            Settings::default(),
            UiState::default(),
        );
        r.vcx.update(|_, cx| cx.set_reduce_motion(true));
        enter_edit(&mut r);
        draw(r.vcx);
        let before = keys(&mut r);
        let row = |r: &mut Rig, name: &str| {
            let id: &'static str = Box::leak(format!("perf-edit-row-{name}").into_boxed_str());
            r.vcx.debug_bounds(id).expect("row")
        };
        let sdc = row(&mut r, "disk:sdc");
        let sda = row(&mut r, "disk:sda");
        let handle = point(sdc.left() + px(12.), sdc.center().y);
        r.vcx
            .simulate_mouse_down(handle, MouseButton::Left, Modifiers::none());
        r.vcx.simulate_mouse_move(
            point(handle.x + px(8.), handle.y),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(r.vcx);
        r.vcx.simulate_mouse_move(
            point(sda.center().x, sda.top() + px(2.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(r.vcx);
        r.vcx.simulate_mouse_move(
            point(sda.center().x, sda.top() + px(3.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(r.vcx);
        assert!(
            r.vcx.debug_bounds("perf-drop-indicator").is_some(),
            "indicator over a disk row"
        );

        let empty = r.vcx.debug_bounds("perf-edit-empty-5").expect("fans hint");
        r.vcx.simulate_mouse_move(
            point(empty.left() + px(2.), empty.center().y),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(r.vcx);
        assert!(
            r.vcx.debug_bounds("perf-drop-indicator").is_none(),
            "no indicator over the empty hint"
        );

        r.vcx.simulate_mouse_move(
            point(sda.center().x, sda.top() + px(3.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(r.vcx);
        assert!(
            r.vcx.debug_bounds("perf-drop-indicator").is_some(),
            "indicator returns over a disk row"
        );
        let eth = row(&mut r, "net:eth0");
        r.vcx
            .simulate_mouse_move(eth.center(), MouseButton::Left, Modifiers::none());
        draw(r.vcx);
        assert!(
            r.vcx.debug_bounds("perf-drop-indicator").is_none(),
            "no indicator over another group"
        );

        r.vcx.simulate_mouse_up(
            point(empty.left() + px(2.), empty.center().y),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(r.vcx);
        assert!(r.vcx.debug_bounds("perf-drop-indicator").is_none());
        assert_eq!(keys(&mut r), before);
        assert!(saved_order(&mut r).is_empty(), "nothing saved");
        let sidebar = r.sidebar.clone();
        assert!(
            r.vcx.update(|_, cx| !sidebar.read(cx).dragging_for_test()),
            "drag state cleared after the release"
        );
    }

    #[gpui_kit::test]
    fn drag_gaps_follow_the_indicator(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            reorder_fixture(),
            Settings::default(),
            UiState::default(),
        );
        r.vcx.update(|_, cx| cx.set_reduce_motion(true));
        enter_edit(&mut r);
        draw(r.vcx);
        let row = |r: &mut Rig, name: &str| {
            let id: &'static str = Box::leak(format!("perf-edit-row-{name}").into_boxed_str());
            r.vcx.debug_bounds(id).expect("row")
        };
        let indicator = |r: &mut Rig| r.vcx.debug_bounds("perf-drop-indicator").is_some();
        let move_to = |r: &mut Rig, at| {
            r.vcx
                .simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
            draw(r.vcx);
        };
        let start_drag = |r: &mut Rig, name: &str| {
            let card = row(r, name);
            let handle = point(card.left() + px(12.), card.center().y);
            r.vcx
                .simulate_mouse_down(handle, MouseButton::Left, Modifiers::none());
            move_to(r, point(handle.x + px(8.), handle.y));
        };

        // A disk row, then another group's padding: the indicator goes away.
        start_drag(&mut r, "disk:sdc");
        let sda = row(&mut r, "disk:sda");
        let eth = row(&mut r, "net:eth0");
        move_to(&mut r, point(sda.center().x, sda.top() + px(2.)));
        move_to(&mut r, point(sda.center().x, sda.top() + px(3.)));
        assert!(indicator(&mut r), "indicator over a disk row");
        let net_side = point(eth.left() - px(4.), eth.center().y);
        move_to(&mut r, net_side);
        assert!(!indicator(&mut r), "no indicator beside the network row");
        move_to(&mut r, point(sda.center().x, sda.top() + px(3.)));
        assert!(indicator(&mut r), "indicator returns over a disk row");
        let net_gap = point(eth.center().x, eth.top() - px(2.));
        move_to(&mut r, net_gap);
        assert!(!indicator(&mut r), "no indicator above the network row");
        r.vcx
            .simulate_mouse_up(net_gap, MouseButton::Left, Modifiers::none());
        draw(r.vcx);
        assert!(saved_order(&mut r).is_empty(), "nothing saved");

        // A release in the gap between two disk rows drops at the indicator there.
        start_drag(&mut r, "disk:sdc");
        let sda = row(&mut r, "disk:sda");
        let sdb = row(&mut r, "disk:sdb");
        move_to(&mut r, point(sda.center().x, sda.bottom() - px(3.)));
        move_to(&mut r, point(sda.center().x, sda.bottom() - px(2.)));
        assert!(indicator(&mut r), "indicator between sda and sdb");
        let gap = point(sda.center().x, (sda.bottom() + sdb.top()) / 2.);
        assert!(gap.y > sda.bottom() && gap.y < sdb.top(), "rows have a gap");
        move_to(&mut r, gap);
        assert!(indicator(&mut r), "the gap keeps the indicator");
        r.vcx
            .simulate_mouse_up(gap, MouseButton::Left, Modifiers::none());
        draw(r.vcx);
        assert_eq!(
            saved_order(&mut r),
            [
                "overview", "cpu", "memory", "disk:sda", "disk:sdc", "disk:sdb", "net:eth0"
            ]
        );
        let sidebar = r.sidebar.clone();
        assert!(r.vcx.update(|_, cx| !sidebar.read(cx).dragging_for_test()));
    }

    #[gpui_kit::test]
    fn alt_up_down_moves_focused_row(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            reorder_fixture(),
            Settings::default(),
            saved("disk:sdb"),
        );
        r.vcx.update(|_, cx| bind_keys_performance(cx));
        enter_edit(&mut r);

        r.vcx.simulate_keystrokes("alt-up");
        draw(r.vcx);
        let disks = keys(&mut r)
            .into_iter()
            .filter(|key| key.starts_with("disk:"))
            .collect::<Vec<_>>();
        assert_eq!(disks, ["disk:sdb", "disk:sda", "disk:sdc"]);

        r.vcx.simulate_keystrokes("alt-up");
        draw(r.vcx);
        let after_end = saved_order(&mut r);
        r.vcx.simulate_keystrokes("alt-down");
        draw(r.vcx);
        assert_ne!(saved_order(&mut r), after_end, "Alt+Down moves it back");
        assert_eq!(
            keys(&mut r)
                .into_iter()
                .filter(|key| key.starts_with("disk:"))
                .collect::<Vec<_>>(),
            ["disk:sda", "disk:sdb", "disk:sdc"]
        );
    }

    #[gpui_kit::test]
    fn alt_up_down_follows_arrow_selection(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            reorder_fixture(),
            Settings::default(),
            saved("disk:sda"),
        );
        r.vcx.update(|_, cx| bind_keys_performance(cx));
        enter_edit(&mut r);

        r.vcx.simulate_keystrokes("down");
        draw(r.vcx);
        r.vcx.simulate_keystrokes("alt-up");
        draw(r.vcx);
        let disks = keys(&mut r)
            .into_iter()
            .filter(|key| key.starts_with("disk:"))
            .collect::<Vec<_>>();
        assert_eq!(disks, ["disk:sdb", "disk:sda", "disk:sdc"]);
    }

    #[gpui_kit::test]
    fn alt_up_moves_clicked_hidden_row(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings
            .performance
            .device_overrides
            .insert("disk:sdb".into(), DeviceVisibility::Hide);
        let mut r = open(cx, reorder_fixture(), settings, saved("disk:sda"));
        r.vcx.update(|_, cx| bind_keys_performance(cx));
        r.vcx.update(|_, cx| cx.set_reduce_motion(true));
        enter_edit(&mut r);

        let row = r
            .vcx
            .debug_bounds("perf-edit-row-disk:sdb")
            .expect("hidden row in edit mode");
        r.vcx.simulate_click(row.center(), Modifiers::none());
        draw(r.vcx);
        r.vcx.simulate_keystrokes("alt-up");
        draw(r.vcx);
        let disks = saved_order(&mut r)
            .into_iter()
            .filter(|key| key.starts_with("disk:"))
            .collect::<Vec<_>>();
        assert_eq!(disks, ["disk:sdb", "disk:sda", "disk:sdc"]);
    }

    #[gpui_kit::test]
    fn drop_indicator_geometry(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            reorder_fixture(),
            Settings::default(),
            UiState::default(),
        );
        enter_edit(&mut r);
        let sidebar = r.sidebar.clone();
        r.vcx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.set_drag_for_test(key("disk:sdb"), Some(1));
                cx.notify();
            });
        });
        draw(r.vcx);

        let line = r
            .vcx
            .debug_bounds("perf-drop-indicator")
            .expect("drop indicator");
        let sidebar = r.vcx.debug_bounds("perf-sidebar").expect("sidebar");
        assert_eq!(line.size.height, px(3.));
        assert_eq!(line.left(), sidebar.left());
        assert_eq!(line.right(), sidebar.right());
    }

    #[gpui_kit::test]
    fn saved_order_survives_replug(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            reorder_fixture(),
            Settings::default(),
            UiState::default(),
        );
        enter_edit(&mut r);
        r.vcx.dispatch_action(PerfMoveDevice {
            key: key("disk:sdc"),
            before: Some(key("disk:sda")),
        });
        draw(r.vcx);

        let unplugged = reorder_fixture()
            .into_iter()
            .filter(|entry| entry.idx != DeviceIdx(4))
            .collect();
        plug(&mut r, 2, unplugged);
        settle(&mut r);
        plug(&mut r, 3, reorder_fixture());
        let disks = keys(&mut r)
            .into_iter()
            .filter(|key| key.starts_with("disk:"))
            .collect::<Vec<_>>();
        assert_eq!(disks, ["disk:sdc", "disk:sda", "disk:sdb"]);
    }

    #[gpui_kit::test]
    fn edit_mode_esc_exits_before_global_chain(cx: &mut TestAppContext) {
        let mut r = open(cx, edit_fixture(), Settings::default(), UiState::default());
        r.vcx.update(|_, cx| {
            tl_ui::actions::bind_keys_global(cx);
            bind_keys_performance(cx);
        });
        enter_edit(&mut r);
        r.vcx.simulate_keystrokes("escape");
        draw(r.vcx);
        let sidebar = r.sidebar.clone();
        assert!(!r.vcx.update(|_, cx| sidebar.read(cx).edit_mode()));
        assert_eq!(*r.global_escapes.borrow(), 0, "global Esc did not run");
    }

    #[gpui_kit::test]
    fn edit_mode_header_instant_under_reduced_motion(cx: &mut TestAppContext) {
        let r = open(cx, edit_fixture(), Settings::default(), UiState::default());
        r.vcx.update(|_, cx| cx.set_reduce_motion(true));
        r.vcx.dispatch_action(PerfEnterEditMode);
        draw(r.vcx);
        let header = r.vcx.debug_bounds("perf-edit-header").expect("edit header");
        assert_eq!(header.size.height, px(64.));
    }

    #[gpui_kit::test]
    fn card_menu_items_default(cx: &mut TestAppContext) {
        let r = open(cx, nine(), Settings::default(), UiState::default());
        let sidebar = r.sidebar.clone();
        let list = r.vcx.update(|window, cx| {
            let focus = sidebar.read(cx).focus_handle(cx);
            let menu = sidebar.update(cx, |s, cx| s.card_menu_for(&key("cpu"), focus, window, cx));
            entries(window, &menu, cx)
        });
        // The trailing separator before the P3 tray item is dropped by `TlMenu::build`.
        assert_eq!(
            labels(&list),
            [
                (MenuEntryKind::Item, "Select Pages"),
                (MenuEntryKind::Item, "Hide This Page"),
            ]
        );
        assert!(list[0].3.as_ref().unwrap().partial_eq(&PerfEnterEditMode));
        assert!(
            list[1]
                .3
                .as_ref()
                .unwrap()
                .partial_eq(&set_visible("cpu", false))
        );
    }

    #[gpui_kit::test]
    fn card_menu_show_only_in_edit_mode_hidden(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        let show = |list: &[Entry]| {
            list.iter()
                .find(|e| e.1.as_deref() == Some("Show This Page"))
                .map(|e| e.3.as_ref().unwrap().boxed_clone())
        };
        let edit_hidden = built(&mut r, "disk:sda", 9, true, true, None);
        let action = show(&edit_hidden).expect("Show This Page in edit mode");
        assert!(action.partial_eq(&set_visible("disk:sda", true)));
        assert!(show(&built(&mut r, "disk:sda", 9, true, false, None)).is_none());
        assert!(show(&built(&mut r, "disk:sda", 9, false, true, None)).is_none());

        // P3 passes the tray state: a separator, then the tray item.
        let tray = built(&mut r, "cpu", 9, false, false, Some(true));
        assert_eq!(
            labels(&tray)[2..],
            [
                (MenuEntryKind::Separator, ""),
                (MenuEntryKind::Item, "Add tray icon graph"),
            ]
        );
        let add = PerfAddTrayGraph { source: key("cpu") };
        assert!(tray[3].3.as_ref().unwrap().partial_eq(&add));
        let no_tray = built(&mut r, "cpu", 9, false, false, Some(false));
        assert_eq!(no_tray[3].0, MenuEntryKind::Disabled);
        assert_eq!(no_tray[3].2.as_deref(), Some("No system tray available"));
    }

    #[gpui_kit::test]
    fn card_menu_hide_disabled_for_last(cx: &mut TestAppContext) {
        let mut r = open(
            cx,
            vec![entry(0, DeviceIdentity::CpuPackage, None)],
            Settings::default(),
            UiState::default(),
        );
        assert_eq!(keys(&mut r), ["cpu"]);
        let sidebar = r.sidebar.clone();
        let list = r.vcx.update(|window, cx| {
            let focus = sidebar.read(cx).focus_handle(cx);
            let menu = sidebar.update(cx, |s, cx| s.card_menu_for(&key("cpu"), focus, window, cx));
            entries(window, &menu, cx)
        });
        assert_eq!(list[1].0, MenuEntryKind::Disabled);
        assert_eq!(list[1].1.as_deref(), Some("Hide This Page"));
        assert_eq!(
            list[1].2.as_deref(),
            Some("At least one performance page must stay visible")
        );
    }

    #[gpui_kit::test]
    fn show_override_written_only_when_default_off(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        r.vcx.dispatch_action(set_visible("disk:sda", false));
        draw(r.vcx);
        assert_eq!(
            overrides(&mut r),
            [("disk:sda".to_owned(), DeviceVisibility::Hide)]
        );
        assert!(!keys(&mut r).contains(&"disk:sda".to_owned()));
        r.vcx.dispatch_action(set_visible("disk:sda", true));
        draw(r.vcx);
        assert_eq!(overrides(&mut r), [], "the type default shows disks");
        assert!(keys(&mut r).contains(&"disk:sda".to_owned()));
    }

    #[gpui_kit::test]
    fn show_override_written_when_type_hidden(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), no_disks(), UiState::default());
        r.vcx.dispatch_action(set_visible("disk:sdb", true));
        draw(r.vcx);
        assert_eq!(
            overrides(&mut r),
            [("disk:sdb".to_owned(), DeviceVisibility::Show)]
        );
        assert!(keys(&mut r).contains(&"disk:sdb".to_owned()));
    }

    #[gpui_kit::test]
    fn last_visible_refused_with_toast(cx: &mut TestAppContext) {
        let mut r = open_in(
            cx,
            vec![entry(0, DeviceIdentity::CpuPackage, None)],
            Settings::default(),
            UiState::default(),
            true,
        );
        r.vcx.dispatch_action(set_visible("cpu", false));
        draw(r.vcx);
        assert_eq!(overrides(&mut r), [], "the override is unchanged");
        assert_eq!(keys(&mut r), ["cpu"]);
        let queue = r.toasts.clone().expect("a toast queue");
        let shown: Vec<SharedString> = r.vcx.update(|_, cx| queue.read(cx).visible_texts());
        assert_eq!(shown, ["At least one performance page must stay visible"]);
    }

    #[gpui_kit::test]
    fn overview_card_first_by_default(cx: &mut TestAppContext) {
        // `with_overview` only stops `open_in` from hiding the card; Show is the type default.
        let mut r = open(cx, nine(), with_overview(), UiState::default());
        let shown = keys(&mut r);
        assert_eq!(shown[..2], ["overview", "cpu"]);
        assert_eq!(shown.len(), 10);
        let l = lines(&mut r, OVERVIEW);
        assert_eq!(
            (&*l.heading, &*l.line1, &*l.line2),
            ("Overview", "CPU, memory, GPU, disk, network", "")
        );
        r.vcx.dispatch_action(PerfSelectVisibleDevice { index: 1 });
        r.vcx.run_until_parked();
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"), "F2 selects CPU");
        r.vcx.dispatch_action(PerfSelectVisibleDevice { index: 0 });
        r.vcx.run_until_parked();
        assert_eq!(
            selected(&mut r).as_deref(),
            Some(OVERVIEW),
            "F1 selects Overview"
        );
    }

    #[gpui_kit::test]
    fn overview_hidden_by_card_menu(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), with_overview(), saved("cpu"));
        draw(r.vcx);
        // The Overview card is the first card; the list starts at sp4 (8 px).
        let card = r.vcx.debug_bounds("perf-card").expect("a card");
        let at = gpui_kit::point(card.center().x, px(8.) + card.size.height / 2.);
        right_click(r.vcx, at);
        draw(r.vcx);
        let hide = open_menu_hide(&mut r).expect("the Overview card's menu is open");
        assert!(hide.partial_eq(&set_visible(OVERVIEW, false)));
        // Close the menu and focus the list again, so the action reaches the sidebar.
        r.vcx.simulate_keystrokes("escape");
        let sidebar = r.sidebar.clone();
        r.vcx.update(|window, cx| {
            let focus = sidebar.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        draw(r.vcx);
        r.vcx.dispatch_action(set_visible(OVERVIEW, false));
        draw(r.vcx);
        assert_eq!(overview_override(&mut r), Some(DeviceVisibility::Hide));
        assert_eq!(keys(&mut r)[0], "cpu");
        r.vcx.dispatch_action(set_visible(OVERVIEW, true));
        draw(r.vcx);
        assert_eq!(overview_override(&mut r), None, "the type default shows it");
        assert_eq!(keys(&mut r)[0], OVERVIEW);
    }

    #[gpui_kit::test]
    fn overview_selection_persists(cx: &mut TestAppContext) {
        let r = open(cx, nine(), with_overview(), saved("cpu"));
        let sidebar = r.sidebar.clone();
        r.vcx.update(|_, cx| {
            sidebar.update(cx, |s, cx| s.select(DeviceKey(Arc::from(OVERVIEW)), cx))
        });
        r.vcx.run_until_parked();
        let saved = r
            .vcx
            .update(|_, cx| TlSettings::state(cx).performance.selected_device.clone());
        assert_eq!(saved, OVERVIEW);
        // A new sidebar reads the saved state, as when the tab opens again.
        let (hub, demand) = (r.hub.clone(), r.demand.clone());
        let again = r
            .vcx
            .update(|_, cx| cx.new(|cx| DeviceSidebar::new(hub, demand, cx)));
        let first = r
            .vcx
            .update(|_, cx| again.read(cx).selected().map(|k| k.0.to_string()));
        assert_eq!(first.as_deref(), Some(OVERVIEW));
    }

    #[gpui_kit::test]
    fn hide_selected_selects_next(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), saved("memory"));
        r.vcx.dispatch_action(set_visible("memory", false));
        draw(r.vcx);
        assert_eq!(selected(&mut r).as_deref(), Some(GPU));
    }

    fn right_click(cx: &mut VisualTestContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        cx.run_until_parked();
    }

    /// The Hide This Page action of the open context menu.
    fn open_menu_hide(r: &mut Rig) -> Option<Box<dyn Action>> {
        r.vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx)?;
            entries(window, &menu, cx)
                .into_iter()
                .find(|e| e.1.as_deref() == Some("Hide This Page"))
                .and_then(|e| e.3)
        })
    }

    #[gpui_kit::test]
    fn right_click_keeps_selection(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
        // `debug_bounds` keeps the last card drawn; every card has the same size. The list
        // starts at sp4 (8 px), and the second card (memory) is one card and a gap lower.
        let card = r.vcx.debug_bounds("perf-card").expect("a card");
        let h = card.size.height;
        let at = gpui_kit::point(card.center().x, px(8.) + h + px(8.) + h / 2.);
        right_click(r.vcx, at);
        draw(r.vcx);
        assert_eq!(
            selected(&mut r).as_deref(),
            Some("cpu"),
            "selection unchanged"
        );
        let hide = open_menu_hide(&mut r).expect("the memory card's menu is open");
        assert!(hide.partial_eq(&set_visible("memory", false)));
        assert!(r.events.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn shift_f10_opens_menu_for_selected(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), saved("disk:sdb"));
        r.vcx.update(|_, cx| bind_keys_performance(cx));
        r.vcx.simulate_keystrokes("shift-f10");
        draw(r.vcx);
        let hide = open_menu_hide(&mut r).expect("a menu is open");
        assert!(hide.partial_eq(&set_visible("disk:sdb", false)));
    }

    /// crw.2: Home and End select the first and the last visible card.
    #[gpui_kit::test]
    fn home_end_select_first_last(cx: &mut TestAppContext) {
        use tl_ui::pages::performance::actions::{PerfSelectFirstDevice, PerfSelectLastDevice};

        let mut r = open(cx, nine(), Settings::default(), saved("disk:sdb"));
        assert_eq!(keys(&mut r)[4], "disk:sdb", "the fifth of nine cards");
        assert_eq!(selected(&mut r).as_deref(), Some("disk:sdb"));
        r.vcx.dispatch_action(PerfSelectFirstDevice);
        r.vcx.run_until_parked();
        assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
        r.vcx.dispatch_action(PerfSelectLastDevice);
        r.vcx.run_until_parked();
        assert_eq!(selected(&mut r).as_deref(), Some("net:eth1"));
        assert_eq!(
            *r.events.borrow(),
            [
                SidebarEvent::Selected(DeviceKey(Arc::from("cpu"))),
                SidebarEvent::Selected(DeviceKey(Arc::from("net:eth1"))),
            ]
        );
    }

    /// crw.2: `show_disks` off in the settings removes the four disk cards through their
    /// collapse, and the selected disk hands the selection to the next visible card. On again,
    /// they come back through the insert animation.
    #[gpui_kit::test]
    fn type_switch_hides_cards_and_moves_selection(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), saved("disk:sdb"));
        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.performance.show_disks = false);
        });
        draw(r.vcx);
        let disks = |a: &[(String, CardAnim)]| -> Vec<bool> {
            a.iter()
                .filter(|(k, _)| k.starts_with("disk:"))
                .map(|(_, anim)| is_removing(anim))
                .collect()
        };
        assert_eq!(disks(&anims(&mut r)), [true; 4], "the disks collapse");
        assert_eq!(selected(&mut r).as_deref(), Some("net:eth0"));
        settle(&mut r);
        assert_eq!(keys(&mut r), ["cpu", "memory", GPU, "net:eth0", "net:eth1"]);
        assert!(disks(&anims(&mut r)).is_empty(), "the disk cards are gone");

        r.vcx.update(|_, cx| {
            TlSettings::update(cx, |d| d.settings.performance.show_disks = true);
        });
        draw(r.vcx);
        let inserting: Vec<String> = anims(&mut r)
            .into_iter()
            .filter(|(_, a)| is_inserting(a))
            .map(|(k, _)| k)
            .collect();
        assert_eq!(inserting, ["disk:sda", "disk:sdb", "disk:sdc", "disk:sdd"]);
        assert_eq!(selected(&mut r).as_deref(), Some("net:eth0"), "kept");
    }

    /// crw.2, UI-PRF-011: a network card's tooltip is line 1, then the NetworkManager
    /// connection name on its own line. With no name it is line 1 alone.
    #[gpui_kit::test]
    fn network_card_tooltip_connection_name(cx: &mut TestAppContext) {
        use tl_model::snapshot::NetDetails;

        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        let details = |idx: u16, connection: Option<&str>| {
            (
                DeviceIdx(idx),
                NetDetails {
                    type_label: Arc::from("Ethernet"),
                    connection: connection.map(Arc::from),
                    adapter_name: Total {
                        value: Arc::from("Intel I350"),
                        validity: Validity::Valid,
                    },
                    driver: Total::default(),
                    mac: Total::default(),
                    ifindex: u32::from(idx),
                    link_speed_mbps: Total::default(),
                    state: LinkState::Up,
                    addresses: Arc::from([]),
                    kind: NetKind::Wired,
                    wifi: None,
                },
            )
        };
        r.engine.publish_with(|s| {
            let d = Arc::make_mut(&mut s.device_details);
            d.header.version += 1;
            d.nets = Arc::from(vec![
                details(7, Some("Wired connection 1")),
                details(8, None),
            ]);
        });
        draw(r.vcx);
        let eth0 = lines(&mut r, "net:eth0");
        assert_eq!(eth0.line1, "Ethernet");
        let tip = eth0.tooltip.expect("a tooltip");
        assert_eq!(
            tip.lines().collect::<Vec<_>>(),
            ["Intel I350", "Wired connection 1"]
        );
        let eth1 = lines(&mut r, "net:eth1");
        assert_eq!(
            eth1.tooltip.as_deref(),
            Some("Intel I350"),
            "adapter name only"
        );

        let before = lines(&mut r, "net:eth0");
        r.engine.publish_with(|s| {
            let mut sys = system(10);
            sys.header.version += 1;
            s.system = Arc::new(sys);
        });
        draw(r.vcx);
        assert_eq!(
            lines(&mut r, "net:eth0"),
            before,
            "an unchanged tooltip keeps the same card text on the next snapshot tick"
        );
    }

    fn fan(idx: u16) -> DeviceEntry {
        entry(
            idx,
            DeviceIdentity::Fan {
                chip: Arc::from("nct6797"),
                index: idx,
            },
            None,
        )
    }

    /// Publishes one Fan reading for `idx` and bumps the SYSTEM version.
    fn spin(r: &mut Rig, idx: u16, rpm: f32) {
        use tl_model::system::{SensorKind, SensorReading};
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.sensors = Arc::from(vec![(
                DeviceIdx(idx),
                SensorReading {
                    device: DeviceIdx(idx),
                    kind: SensorKind::Fan,
                    value: Metric::valid(rpm),
                    pwm_pct: Metric::with(Validity::Unsupported),
                    fan_control: None,
                    fan_connected: true,
                },
            )]);
        });
        draw(r.vcx);
    }

    fn with_fan() -> Vec<DeviceEntry> {
        let mut entries = cpu_and_memory();
        entries.push(fan(5));
        entries
    }

    #[gpui_kit::test]
    fn fan_appears_when_it_spins(cx: &mut TestAppContext) {
        let mut r = open(cx, with_fan(), Settings::default(), UiState::default());
        assert!(!keys(&mut r).iter().any(|k| k.starts_with("fan:")));
        spin(&mut r, 5, 800.);
        let a = anims(&mut r);
        let at = a
            .iter()
            .position(|(k, _)| k == "fan:nct6797:5")
            .expect("fan card");
        assert!(is_inserting(&a[at].1), "{a:?}");

        settle(&mut r);
        assert_eq!(anims(&mut r)[at].1, CardAnim::Steady);
    }

    #[gpui_kit::test]
    fn fan_flip_keeps_card_when_shown(cx: &mut TestAppContext) {
        let mut settings = Settings::default();
        settings.performance.show_disconnected_fans = true;
        let mut r = open(cx, with_fan(), settings, UiState::default());
        assert!(keys(&mut r).contains(&"fan:nct6797:5".to_owned()));
        spin(&mut r, 5, 800.);
        settle(&mut r);
        let a = anims(&mut r);
        let fan = a.iter().find(|(k, _)| k == "fan:nct6797:5").expect("card");
        assert_eq!(fan.1, CardAnim::Steady);
        assert!(
            !r.events
                .borrow()
                .iter()
                .any(|e| matches!(e, SidebarEvent::Removed(_)))
        );
    }
}
