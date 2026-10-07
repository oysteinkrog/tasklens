//! Device sidebar card rendering (E24, UI-PRF-011). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px,
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
    CARD_CHART_BG, CardLines, CardTextInput, CardWords, DeviceCard, card_chart_grid,
    card_chart_labels, card_lines,
};
use tl_ui::pages::performance::{DeviceKey, DeviceKind};
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::current;

const CARD_W: f32 = 244.;

struct Host {
    chart: Entity<ChartView>,
    lines: CardLines,
    selected: bool,
    focused: bool,
    edit_mode: bool,
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
            width: px(CARD_W),
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
    let text = cx.debug_bounds("perf-card-text").expect("text column");
    assert_eq!(text.left() - graph.right(), px(8.), "sp4 gap");
    let heights: Vec<_> = ["perf-card-heading", "perf-card-line1", "perf-card-line2"]
        .iter()
        .map(|s| cx.debug_bounds(s).expect("line").size.height)
        .collect();
    assert_eq!(heights, [px(18.), px(16.), px(16.)]);

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
    use gpui_kit::{Focusable as _, StyleRefinement};
    use tl_model::demand::{CpuDemand, Demand};
    use tl_model::devices::DeviceCatalog;
    use tl_model::ids::{MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{Settings, UiState};
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
    use tl_ui::pages::performance::sidebar::{CardAnim, DeviceSidebar, SidebarEvent};
    use tl_ui::settings_global::TlSettings;

    use super::*;

    const GPU: &str = "gpu:pci:0000:2d:00.0";
    const CPU_MODEL: &str = "AMD Ryzen 7 7800X3D 8-Core Processor";

    struct SidebarHost(Entity<DeviceSidebar>);

    impl Render for SidebarHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(260.)).h(px(900.)).child(
                self.0
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
        }
    }

    struct Rig<'a> {
        sidebar: Entity<DeviceSidebar>,
        demand: Entity<DemandAggregator>,
        engine: Arc<FakeEngine>,
        events: Rc<RefCell<Vec<SidebarEvent>>>,
        /// The window's toast queue, when the rig has a kit `Root`.
        toasts: Option<Entity<ToastQueue>>,
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
    fn open_in(
        cx: &mut TestAppContext,
        devices: Vec<DeviceEntry>,
        settings: Settings,
        state: UiState,
        rooted: bool,
    ) -> Rig<'_> {
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
        let d = demand.clone();
        let sidebar = cx.new(|cx| DeviceSidebar::new(hub, d, cx));
        let events: Rc<RefCell<Vec<SidebarEvent>>> = Rc::default();
        let seen = Rc::clone(&events);
        cx.update(|cx| {
            cx.subscribe(&sidebar, move |_, ev: &SidebarEvent, _| {
                seen.borrow_mut().push(ev.clone())
            })
            .detach();
        });
        let host = sidebar.clone();
        let (vcx, toasts) = if rooted {
            cx.update(tl_ui::components::toast::init);
            let window = cx.add_window(move |window, cx| {
                let host = cx.new(|_| SidebarHost(host));
                gpui_kit::base::Root::new(host, window, cx)
            });
            let vcx = VisualTestContext::from_window(window.into(), cx).into_mut();
            let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(40.))));
            (vcx, Some(queue))
        } else {
            let (_, vcx) = cx.add_window_view(move |_, _| SidebarHost(host));
            (vcx, None)
        };
        vcx.simulate_scale_factor_change(1.0);
        let focus = vcx.update(|_, cx| sidebar.read(cx).focus_handle(cx));
        vcx.update(|window, cx| focus.focus(window, cx));
        draw(vcx);
        Rig {
            sidebar,
            demand,
            engine,
            events,
            toasts,
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

    fn renders(r: &mut Rig) -> u32 {
        let sidebar = r.sidebar.clone();
        r.vcx.update(|_, cx| sidebar.read(cx).render_count.get())
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
        assert_eq!((cards, rows), (9, 9));
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
        let before = renders(&mut r);
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
        assert_eq!(renders(&mut r), before + 1);
        assert_eq!(
            tr_calls(),
            calls,
            "the sidebar translates nothing per frame"
        );
    }

    #[gpui_kit::test]
    fn unrelated_snapshot_does_not_rerender(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        let before = renders(&mut r);
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
        assert_eq!(renders(&mut r), before);
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
        PerfAddTrayGraph, PerfEnterEditMode, PerfSetPageVisible, bind_keys_performance,
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

    fn overrides(r: &mut Rig) -> Vec<(String, DeviceVisibility)> {
        r.vcx.update(|_, cx| {
            TlSettings::settings(cx)
                .performance
                .device_overrides
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect()
        })
    }

    #[gpui_kit::test]
    fn card_menu_items_default(cx: &mut TestAppContext) {
        let mut r = open(cx, nine(), Settings::default(), UiState::default());
        let sidebar = r.sidebar.clone();
        let list = r.vcx.update(|window, cx| {
            let focus = sidebar.read(cx).focus_handle(cx);
            let menu = sidebar
                .read(cx)
                .card_menu_for(&key("cpu"), focus, window, cx);
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
            let menu = sidebar
                .read(cx)
                .card_menu_for(&key("cpu"), focus, window, cx);
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
        let first = r.vcx.debug_bounds("perf-card").expect("first card");
        // The second card (memory) starts one card and the sp4 gap below.
        let at = first.center() + gpui_kit::point(px(0.), first.size.height + px(8.));
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
}
