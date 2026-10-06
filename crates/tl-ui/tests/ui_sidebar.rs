//! Device sidebar card rendering (E24, UI-PRF-011). Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext,
    Window, div, px,
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
    CARD_CHART_BG, CardLines, CardTextInput, DeviceCard, card_chart_grid, card_chart_labels,
    card_lines,
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
    let (host, vcx) = cx.add_window_view(move |_, cx| {
        let spec = ChartSpec {
            grid: card_chart_grid(),
            background: CARD_CHART_BG,
            ..ChartSpec::default()
        };
        let chart = cx
            .new(|cx| ChartView::new(spec, card_chart_labels(ValueFormat::RateNetwork), &hub, cx));
        Host {
            chart,
            lines,
            selected: false,
            focused: false,
            edit_mode: false,
            clicked: Rc::default(),
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
    assert!(line2.contains(&lock.center()), "the lock sits in line 2");
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
