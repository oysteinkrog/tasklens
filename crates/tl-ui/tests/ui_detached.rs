//! Detached graph window tests (tasklens-xdh.16, PERF-083, UI-PRF-044, R-086). Run with
//! `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, WindowDecorations, WindowKind, div, point, px, size,
};
use smallvec::smallvec;
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_model::time::now_boot_ns;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::ChartState;
use tl_ui::hub::{GlobalHub, SnapshotHub};
use tl_ui::pages::minigraph::GraphColor;
use tl_ui::pages::minigraph::sources::{GraphColumn, MinigraphEntry, MinigraphId, MinigraphSource};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::window::main::APP_ID;
use tl_ui::window::registry::{GlobalRegistry, WindowKey};
use tl_ui::window::titlebar::TitleBarProbe;
use tl_ui::windows::detached_graph::{
    DetachedGraphWindow, detached_window_options, open_detached, window_size,
};

const MS: u64 = 1_000_000;

/// A stand-in for the main window: it owns its own driver.
struct Host {
    _driver: Entity<AnimationDriver>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn catalog() -> DeviceCatalog {
    DeviceCatalog {
        version: 1,
        entries: vec![DeviceEntry {
            idx: DeviceIdx(0),
            identity: DeviceIdentity::CpuPackage,
            generation: 0,
            display_name: Arc::from("cpu"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
        }]
        .into(),
    }
}

fn cpu_entry() -> MinigraphEntry {
    MinigraphEntry {
        id: MinigraphId(1),
        source: MinigraphSource::Cpu,
        column: GraphColumn::Cpu,
        range_s: 60,
        color: GraphColor::Accent,
        slot: 0,
        detached: true,
        window: 1,
    }
}

fn setup(cx: &mut TestAppContext) {
    let engine = FakeEngine::new();
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        let appearance = AppearanceSettings::default();
        tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
        tl_ui::theme::metrics::init_metrics(&appearance, cx);
        tl_ui::i18n::init_strings(cx, "en-US");
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    engine.publish_with(|s| s.devices = Arc::new(catalog()));
    cx.run_until_parked();
    cx.update(|cx| {
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        cx.set_global(GlobalHub(hub));
        GlobalRegistry::init(cx);
    });
}

fn open(
    cx: &mut TestAppContext,
    id: u32,
    cards: Vec<MinigraphEntry>,
) -> (VisualTestContext, Entity<DetachedGraphWindow>) {
    let handle = cx
        .update(|cx| open_detached(id, cards, None, cx))
        .expect("window opens");
    let view = cx
        .update(|cx| DetachedGraphWindow::find(handle.into(), cx))
        .expect("detached view");
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    draw(&mut vcx);
    (vcx, view)
}

fn draw(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui_kit::test]
fn detached_window_opens_with_caption(cx: &mut TestAppContext) {
    setup(cx);
    let (vcx, view) = open(cx, 1, vec![cpu_entry()]);
    assert_eq!(
        vcx.read_entity(&view, |v, _| v.title().to_string()),
        "tasklens Graph 1"
    );
    let held = cx.read(|cx| {
        cx.global::<GlobalRegistry>()
            .0
            .read(cx)
            .get(&WindowKey::DetachedGraph(1))
    });
    assert!(held.is_some(), "the registry holds DetachedGraph(1)");

    let options = cx.update(|cx| detached_window_options(window_size(1, 1.), cx));
    assert!(!options.is_resizable);
    assert!(options.is_movable);
    assert_eq!(options.kind, WindowKind::Floating);
    assert!(options.titlebar.is_none());
    assert_eq!(options.window_decorations, Some(WindowDecorations::Client));
    assert_eq!(options.app_id.as_deref(), Some(APP_ID));
    let bounds = options.window_bounds.expect("bounds").get_bounds();
    assert_eq!(bounds.size, size(px(188.), px(98.)));

    // A window id above 255 does not fit the registry key.
    let before = cx.update(|cx| cx.windows().len());
    assert!(
        cx.update(|cx| open_detached(256, vec![cpu_entry()], None, cx))
            .is_err()
    );
    assert_eq!(cx.update(|cx| cx.windows().len()), before);
}

#[gpui_kit::test]
fn detached_body_drag_moves_window(cx: &mut TestAppContext) {
    setup(cx);
    cx.update(|cx| cx.set_global(TitleBarProbe::default()));
    let (mut vcx, _view) = open(cx, 1, vec![cpu_entry()]);
    // The card centre, then its label row.
    for y in [49., 10.] {
        vcx.simulate_mouse_down(
            point(px(94.), px(y)),
            MouseButton::Left,
            Modifiers::default(),
        );
        vcx.simulate_mouse_up(
            point(px(94.), px(y)),
            MouseButton::Left,
            Modifiers::default(),
        );
    }
    assert_eq!(cx.read(|cx| cx.global::<TitleBarProbe>().moves), 2);
}

#[gpui_kit::test]
fn detached_window_own_driver(cx: &mut TestAppContext) {
    setup(cx);
    let (_host, host_vcx) = cx.add_window_view(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        Host { _driver: driver }
    });
    let host_driver = host_vcx.update(|window, cx| AnimationDriver::global(window, cx));
    let (mut vcx, view) = open(cx, 1, vec![cpu_entry()]);

    let (own, chart) = vcx.read_entity(&view, |v, cx| {
        (v.driver().clone(), v.cards()[0].1.read(cx).chart().clone())
    });
    let newest = now_boot_ns() - 50 * MS;
    let samples: Vec<Sample> = (0..600u64)
        .map(|i| Sample::valid(newest - (599 - i) * 100 * MS, (i % 100) as f32))
        .collect();
    let series = Arc::new(SeriesView {
        id: SeriesId {
            device: DeviceIdx(0),
            metric: MetricId::CPU_TOTAL,
        },
        version: 1,
        period_ns: 100 * MS,
        samples: samples.into(),
    });
    chart.update(&mut vcx, |c, cx| {
        c.set_series_for_test(smallvec![Some(series)], cx);
        c.set_state(ChartState::Live, cx);
    });
    draw(&mut vcx);
    assert!(
        vcx.read_entity(&own, |d, _| d.is_armed()),
        "the window's driver is armed"
    );
    assert_eq!(cx.read_entity(&host_driver, |d, _| d.stats.requested), 0);
    assert_ne!(own, host_driver);
}

/// Detach and attach through the panel (tasklens-xdh.17, R-084, UI-PRF-043).
mod detach {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, ParentElement as _,
        Pixels, Point, Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext,
        Window, div, point, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::settings::{
        AppearanceSettings, DetachedWindowState, MinigraphState, Settings, ThemeSettings, UiState,
    };
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::components::menu::{MenuEntryKind, open_context_menu};
    use tl_ui::components::toast::ToastKind;
    use tl_ui::demand::DemandAggregator;
    use tl_ui::hub::{GlobalHub, SnapshotHub};
    use tl_ui::pages::minigraph::actions::{
        MinigraphAddToWindow, MinigraphAttach, MinigraphCloseDetachedWindow, MinigraphDetach,
    };
    use tl_ui::pages::minigraph::panel::MinigraphPanel;
    use tl_ui::pages::minigraph::sources::{MinigraphId, MinigraphSource};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::registry::{GlobalRegistry, WindowKey};
    use tl_ui::windows::detached_graph::{DetachedGraphWindow, window_size};

    use super::catalog;
    use std::time::Duration;
    use tl_model::command::{Command, KWinCommand};
    use tl_model::snapshot::{ComponentHeader, FrameGeometry, WindowEntry, WindowsSnapshot};
    use tl_model::validity::Validity;
    use tl_ui::pages::minigraph::bridge::OwnWindowBridge;

    /// Hosts the panel cached over a 1000 × 600 box, as the Processes page does.
    struct PanelHost {
        panel: Entity<MinigraphPanel>,
    }

    impl Render for PanelHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().relative().w(px(1000.)).h(px(600.)).child(
                self.panel
                    .clone()
                    .cached(StyleRefinement::default().absolute().inset_0()),
            )
        }
    }

    struct Rig {
        panel: Entity<MinigraphPanel>,
        vcx: VisualTestContext,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        engine: Arc<FakeEngine>,
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn open_panel(
        cx: &mut TestAppContext,
        hub: &Entity<SnapshotHub>,
        demand: &Entity<DemandAggregator>,
    ) -> (Entity<MinigraphPanel>, VisualTestContext) {
        let (hub, demand) = (hub.clone(), demand.clone());
        let (host, vcx) = cx.add_window_view(move |_, cx| PanelHost {
            panel: cx.new(|cx| MinigraphPanel::new(&hub, &demand, cx)),
        });
        let mut vcx = vcx.clone();
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(1000.), px(600.)));
        let panel = vcx.read_entity(&host, |h, _| h.panel.clone());
        draw(&mut vcx);
        (panel, vcx)
    }

    fn rig(cx: &mut TestAppContext, state: UiState) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            let appearance = AppearanceSettings::default();
            tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
            tl_ui::theme::metrics::init_metrics(&appearance, cx);
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            cx.set_global(BootSettings {
                settings: Settings::default(),
                state,
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        engine.publish_with(|s| s.devices = Arc::new(catalog()));
        cx.run_until_parked();
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        cx.update(|cx| {
            cx.set_global(GlobalHub(hub.clone()));
            GlobalRegistry::init(cx);
        });
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        let (panel, vcx) = open_panel(cx, &hub, &demand);
        Rig {
            panel,
            vcx,
            hub,
            demand,
            engine,
        }
    }

    impl Rig {
        fn id_of(&mut self, source: &MinigraphSource) -> MinigraphId {
            let panel = self.panel.clone();
            self.vcx.update(|_, cx| {
                panel
                    .read(cx)
                    .entries()
                    .iter()
                    .find(|e| &e.source == source)
                    .map(|e| e.id)
                    .expect("card present")
            })
        }

        fn dispatch(&mut self, action: impl gpui_kit::Action) {
            self.vcx.update(|window, cx| {
                window.dispatch_action(action.boxed_clone(), cx);
            });
            draw(&mut self.vcx);
        }

        fn dispatch_boxed(&mut self, action: Box<dyn gpui_kit::Action>) {
            self.vcx.update(|window, cx| {
                window.dispatch_action(action, cx);
            });
            draw(&mut self.vcx);
        }

        fn attached_count(&mut self) -> usize {
            let panel = self.panel.clone();
            self.vcx.update(|_, cx| panel.read(cx).attached().count())
        }

        /// `(detached, window, slot)` of the card for `source`.
        fn state_of(&mut self, source: &MinigraphSource) -> (bool, u32, u16) {
            let panel = self.panel.clone();
            self.vcx.update(|_, cx| {
                let p = panel.read(cx);
                let e = p
                    .entries()
                    .iter()
                    .find(|e| &e.source == source)
                    .expect("entry");
                (e.detached, e.window, e.slot)
            })
        }

        fn toasts(&mut self) -> Vec<(ToastKind, String)> {
            let panel = self.panel.clone();
            self.vcx.update(|_, cx| {
                panel
                    .read(cx)
                    .toasts
                    .iter()
                    .map(|(k, t)| (*k, t.to_string()))
                    .collect()
            })
        }
    }

    fn registry_has(cx: &mut TestAppContext, window: u8) -> bool {
        cx.read(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::DetachedGraph(window))
                .is_some()
        })
    }

    /// The title and card sources of detached window `window`.
    fn window_cards(cx: &mut TestAppContext, window: u8) -> (String, Vec<MinigraphSource>) {
        cx.read(|cx| {
            let handle = cx
                .global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::DetachedGraph(window))
                .expect("window open");
            let view = DetachedGraphWindow::find(handle, cx).expect("detached view");
            let v = view.read(cx);
            (
                v.title().to_string(),
                v.cards()
                    .iter()
                    .map(|(_, c)| c.read(cx).entry.source.clone())
                    .collect(),
            )
        })
    }

    fn saved_state(cx: &mut TestAppContext) -> (Vec<(String, bool, u32)>, Vec<u32>) {
        cx.read(|cx| {
            let st = TlSettings::state(cx);
            (
                st.minigraphs
                    .iter()
                    .map(|m| (m.source.clone(), m.detached, m.window))
                    .collect(),
                st.detached_windows.iter().map(|w| w.id).collect(),
            )
        })
    }

    #[gpui_kit::test]
    fn detached_state_round_trip(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        let memory = r.id_of(&MinigraphSource::Memory);
        r.dispatch(MinigraphDetach { id: memory });
        assert_eq!(r.attached_count(), 5);
        assert!(registry_has(cx, 1));
        let (title, cards) = window_cards(cx, 1);
        assert_eq!(title, "tasklens Graph 1");
        assert_eq!(cards, [MinigraphSource::Memory]);
        let (minigraphs, windows) = saved_state(cx);
        assert!(minigraphs.contains(&("memory".to_owned(), true, 1)));
        assert_eq!(windows, [1]);

        // Part 2: the window and the panel go; a new panel reads the saved state.
        cx.update(|cx| {
            let registry = cx.global::<GlobalRegistry>().0.clone();
            registry.update(cx, |r, cx| r.close(&WindowKey::DetachedGraph(1), cx));
        });
        r.vcx.update(|window, _| window.remove_window());
        cx.run_until_parked();
        assert!(!registry_has(cx, 1));
        let (panel, mut vcx) = open_panel(cx, &r.hub, &r.demand);
        draw(&mut vcx);
        assert!(registry_has(cx, 1), "the window reopens");
        assert_eq!(window_cards(cx, 1).1, [MinigraphSource::Memory]);
        let attached = vcx.update(|_, cx| panel.read(cx).attached().count());
        assert_eq!(attached, 5);
    }

    #[gpui_kit::test]
    fn attach_returns_card_and_closes_empty_window(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        let memory = r.id_of(&MinigraphSource::Memory);
        r.dispatch(MinigraphDetach { id: memory });
        assert!(registry_has(cx, 1));
        r.dispatch(MinigraphAttach { id: memory });
        assert_eq!(r.state_of(&MinigraphSource::Memory), (false, 0, 5));
        assert_eq!(r.attached_count(), 6);
        assert!(!registry_has(cx, 1), "the empty window closed");
        let (minigraphs, windows) = saved_state(cx);
        assert!(minigraphs.iter().all(|(_, detached, _)| !detached));
        assert!(windows.is_empty());
    }

    fn slot_center(r: &mut Rig, slot: u16) -> Point<Pixels> {
        let selector: &'static str = Box::leak(format!("MinigraphSlot-{slot}").into_boxed_str());
        r.vcx.debug_bounds(selector).expect("slot drawn").center()
    }

    /// Presses the CPU card, drags it to `to` and releases there.
    fn drag_cpu_to(r: &mut Rig, to: Point<Pixels>) {
        let from = slot_center(r, 0);
        r.vcx
            .simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        draw(&mut r.vcx);
        for i in 1..=4 {
            let t = i as f32 / 4.;
            let p = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            r.vcx
                .simulate_mouse_move(p, MouseButton::Left, Modifiers::none());
            draw(&mut r.vcx);
        }
        r.vcx
            .simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        draw(&mut r.vcx);
    }

    #[gpui_kit::test]
    fn drag_past_edge_detaches(cx: &mut TestAppContext) {
        let mut r = rig(cx, UiState::default());
        // 20 px past the right edge of the 1000 px window: not far enough.
        drag_cpu_to(&mut r, point(px(1020.), px(300.)));
        assert!(!r.state_of(&MinigraphSource::Cpu).0);
        assert!(!registry_has(cx, 1));
        // 30 px past it.
        drag_cpu_to(&mut r, point(px(1030.), px(300.)));
        assert!(r.state_of(&MinigraphSource::Cpu).0);
        assert_eq!(r.state_of(&MinigraphSource::Cpu).1, 1);
        assert!(registry_has(cx, 1));
        assert_eq!(window_cards(cx, 1).1, [MinigraphSource::Cpu]);
    }

    #[gpui_kit::test]
    fn attach_into_full_panel_toasts(cx: &mut TestAppContext) {
        let mut minigraphs: Vec<MinigraphState> = (0..12u16)
            .map(|i| MinigraphState {
                source: format!("name:p{i}"),
                column: "cpu".to_owned(),
                slot: i,
                ..MinigraphState::default()
            })
            .collect();
        minigraphs.push(MinigraphState {
            source: "memory".to_owned(),
            column: "memory".to_owned(),
            detached: true,
            window: 1,
            ..MinigraphState::default()
        });
        let state = UiState {
            minigraphs,
            detached_windows: vec![DetachedWindowState {
                id: 1,
                ..DetachedWindowState::default()
            }],
            ..UiState::default()
        };
        let mut r = rig(cx, state);
        assert!(registry_has(cx, 1), "the saved window opens at startup");
        let memory = r.id_of(&MinigraphSource::Memory);
        r.dispatch(MinigraphAttach { id: memory });
        assert_eq!(
            r.toasts(),
            [(
                ToastKind::Warning,
                "No space for more minigraphs".to_owned()
            )]
        );
        assert!(r.state_of(&MinigraphSource::Memory).0);
        assert!(registry_has(cx, 1));
    }

    /// State with CPU and Memory detached into window 1.
    fn two_card_state() -> UiState {
        let card = |source: &str, column: &str, slot: u16| MinigraphState {
            source: source.to_owned(),
            column: column.to_owned(),
            slot,
            detached: true,
            window: 1,
            ..MinigraphState::default()
        };
        UiState {
            minigraphs: vec![card("cpu", "cpu", 0), card("memory", "memory", 1)],
            detached_windows: vec![DetachedWindowState {
                id: 1,
                ..DetachedWindowState::default()
            }],
            ..UiState::default()
        }
    }

    const GRAPH_1_ID: &str = "{11111111-2222-3333-4444-555555555555}";

    /// Window 1 holds Memory and has the saved position (x, y).
    fn placed_state(x: i32, y: i32) -> UiState {
        let mut state = two_card_state();
        state.detached_windows[0].x = x;
        state.detached_windows[0].y = y;
        state
    }

    fn publish_windows(r: &mut Rig, validity: Validity, x: i32, y: i32) {
        let entry = WindowEntry {
            internal_id: Arc::from(GRAPH_1_ID),
            pid: std::process::id(),
            caption: Arc::from("tasklens Graph 1"),
            resource_class: Arc::from("tasklens"),
            desktop_file_name: Arc::from("tasklens"),
            minimized: false,
            maximized: false,
            keep_above: false,
            frame_geometry: FrameGeometry {
                x,
                y,
                width: 180,
                height: 90,
            },
            window_type: 0,
            stack_index: 0,
        };
        r.engine.publish_with(|s| {
            let version = s.windows.header.version + 1;
            s.windows = Arc::new(WindowsSnapshot {
                header: ComponentHeader {
                    version,
                    validity,
                    ..ComponentHeader::default()
                },
                windows: Arc::from(vec![entry]),
                pick: None,
            });
        });
        r.vcx.run_until_parked();
    }

    fn kwin_commands(r: &Rig) -> Vec<KWinCommand> {
        r.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::KWin(k) => Some(k),
                _ => None,
            })
            .collect()
    }

    fn saved_position(cx: &mut TestAppContext) -> (i32, i32) {
        cx.read(|cx| {
            let st = TlSettings::state(cx);
            let w = st
                .detached_windows
                .iter()
                .find(|w| w.id == 1)
                .expect("entry");
            (w.x, w.y)
        })
    }

    fn bridge_writes(cx: &mut TestAppContext) -> u32 {
        cx.read(|cx| OwnWindowBridge::global(cx).expect("bridge").read(cx).writes)
    }

    #[gpui_kit::test]
    fn detached_bridge_commands_once(cx: &mut TestAppContext) {
        let mut r = rig(cx, placed_state(100, 200));
        assert!(registry_has(cx, 1));
        publish_windows(&mut r, Validity::Valid, 0, 0);
        let first = kwin_commands(&r);
        let pid = std::process::id();
        assert_eq!(first.len(), 2, "{first:?}");
        assert_eq!(
            first[0],
            KWinCommand::KeepAbove {
                pid,
                caption: Some(Arc::from("tasklens Graph 1")),
                on: true,
            }
        );
        assert_eq!(
            first[1],
            KWinCommand::SetFrameGeometry {
                pid,
                caption: Arc::from("tasklens Graph 1"),
                geometry: FrameGeometry {
                    x: 100,
                    y: 200,
                    width: 180,
                    height: 90,
                },
            }
        );
        publish_windows(&mut r, Validity::Valid, 0, 0);
        assert_eq!(kwin_commands(&r).len(), 2, "no new command");
    }

    #[gpui_kit::test]
    fn detached_move_readback_persists(cx: &mut TestAppContext) {
        let mut r = rig(cx, placed_state(100, 200));
        publish_windows(&mut r, Validity::Valid, 100, 200);
        let sent = kwin_commands(&r).len();
        publish_windows(&mut r, Validity::Valid, 300, 400);
        cx.executor().advance_clock(Duration::from_millis(600));
        r.vcx.run_until_parked();
        assert_eq!(saved_position(cx), (300, 400));
        assert_eq!(kwin_commands(&r).len(), sent, "no further command");
    }

    #[gpui_kit::test]
    fn no_bridge_no_commands(cx: &mut TestAppContext) {
        let mut r = rig(cx, placed_state(0, 0));
        publish_windows(&mut r, Validity::Unsupported, 0, 0);
        publish_windows(&mut r, Validity::Unsupported, 300, 400);
        cx.executor().advance_clock(Duration::from_millis(600));
        r.vcx.run_until_parked();
        assert!(kwin_commands(&r).is_empty());
        assert_eq!(saved_position(cx), (0, 0));
    }

    #[gpui_kit::test]
    fn unchanged_geometry_not_persisted(cx: &mut TestAppContext) {
        let mut r = rig(cx, placed_state(100, 200));
        publish_windows(&mut r, Validity::Valid, 100, 200);
        publish_windows(&mut r, Validity::Valid, 100, 200);
        publish_windows(&mut r, Validity::Valid, 100, 200);
        cx.executor().advance_clock(Duration::from_millis(600));
        r.vcx.run_until_parked();
        assert_eq!(bridge_writes(cx), 0);
        assert_eq!(saved_position(cx), (100, 200));
    }

    type MenuRows = Vec<(MenuEntryKind, String)>;

    /// Right-clicks `at` in detached window 1; returns the menu rows and the Add graph labels.
    fn right_click_window(cx: &mut TestAppContext, at: Point<Pixels>) -> (MenuRows, Vec<String>) {
        let handle = cx
            .read(|cx| {
                cx.global::<GlobalRegistry>()
                    .0
                    .read(cx)
                    .get(&WindowKey::DetachedGraph(1))
            })
            .expect("window open");
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        vcx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
        draw(&mut vcx);
        vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx).expect("the window menu is open");
            let rows = menu.read(cx).entries_for_test(window);
            let add: Vec<String> = rows
                .iter()
                .find(|e| e.label.as_deref() == Some("Add graph"))
                .and_then(|e| e.submenu.clone())
                .map(|sub| {
                    sub.read(cx)
                        .entries_for_test(window)
                        .into_iter()
                        .filter_map(|e| e.label.map(|l| l.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let rows = rows
                .into_iter()
                .map(|e| (e.kind, e.label.map(|l| l.to_string()).unwrap_or_default()))
                .collect();
            (rows, add)
        })
    }

    fn labels(rows: &MenuRows) -> Vec<&str> {
        rows.iter().map(|(_, l)| l.as_str()).collect()
    }

    #[gpui_kit::test]
    fn detached_menu_order(cx: &mut TestAppContext) {
        let _r = rig(cx, two_card_state());
        let (rows, add) = right_click_window(cx, point(px(94.), px(49.)));
        assert_eq!(
            labels(&rows),
            [
                "Show Main Window",
                "Close graph",
                "Attach to main window",
                "Time range",
                "Graph color",
                "Add graph",
                "Remove graph",
                "Exit tasklens"
            ]
        );
        for (kind, label) in &rows {
            let submenu = ["Time range", "Graph color", "Add graph"].contains(&label.as_str());
            assert_eq!(*kind == MenuEntryKind::Submenu, submenu, "{label}");
        }
        assert_eq!(add, ["Disk", "Network", "GPU", "VRAM", "GPU temp"]);
    }

    #[gpui_kit::test]
    fn detached_menu_body_click_subset(cx: &mut TestAppContext) {
        let _r = rig(cx, two_card_state());
        let (rows, _) = right_click_window(cx, point(px(94.), px(96.)));
        assert_eq!(
            labels(&rows),
            [
                "Show Main Window",
                "Close graph",
                "Add graph",
                "Exit tasklens"
            ]
        );
    }

    #[gpui_kit::test]
    fn close_graph_reattaches(cx: &mut TestAppContext) {
        let mut r = rig(cx, two_card_state());
        assert!(registry_has(cx, 1));
        r.dispatch(MinigraphCloseDetachedWindow { window: 1 });
        assert!(!r.state_of(&MinigraphSource::Cpu).0);
        assert!(!r.state_of(&MinigraphSource::Memory).0);
        assert!(!registry_has(cx, 1), "the empty window closed");
        assert!(r.toasts().is_empty());
    }

    #[gpui_kit::test]
    fn add_graph_to_window_grows_it(cx: &mut TestAppContext) {
        let mut r = rig(cx, two_card_state());
        r.dispatch(MinigraphAddToWindow {
            window: 1,
            source: MinigraphSource::Disk,
        });
        assert_eq!(
            window_cards(cx, 1).1,
            [
                MinigraphSource::Cpu,
                MinigraphSource::Memory,
                MinigraphSource::Disk
            ]
        );
        let height = cx.read(|cx| TlSettings::state(cx).detached_windows[0].height);
        assert_eq!(height as f32, f32::from(window_size(3, 1.).height));
    }

    #[gpui_kit::test]
    fn remove_graph_from_window(cx: &mut TestAppContext) {
        let mut r = rig(cx, two_card_state());
        let handle = cx
            .read(|cx| {
                cx.global::<GlobalRegistry>()
                    .0
                    .read(cx)
                    .get(&WindowKey::DetachedGraph(1))
            })
            .expect("window open");
        let _ = right_click_window(cx, point(px(94.), px(143.)));
        let mut vcx = VisualTestContext::from_window(handle, cx);
        let action = vcx.update(|window, cx| {
            open_context_menu(window, cx)
                .expect("menu open")
                .read(cx)
                .entries_for_test(window)
                .into_iter()
                .find(|e| e.label.as_deref() == Some("Remove graph"))
                .and_then(|e| e.action)
                .expect("Remove graph action")
        });
        r.dispatch_boxed(action);
        assert_eq!(window_cards(cx, 1).1, [MinigraphSource::Cpu]);
        let memory_on = cx.read(|cx| TlSettings::settings(cx).graphs.minigraph_memory);
        assert!(!memory_on);
        let gone = r.vcx.update(|_, cx| {
            r.panel
                .read(cx)
                .entries()
                .iter()
                .all(|e| e.source != MinigraphSource::Memory)
        });
        assert!(gone);
    }
}

/// The Performance mini window (tasklens-xdh.21, PERF-093, UI-PRF-045).
mod mini {
    use std::sync::Arc;
    use std::time::Duration;

    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Entity, TestAppContext, VisualTestContext, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::demand::{CpuDemand, Demand};
    use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
    use tl_model::ids::DeviceIdx;
    use tl_model::settings::{
        AppearanceSettings, DeviceVisibility, Settings, ThemeSettings, UiState,
    };
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::demand::{DEMAND_COALESCE, DemandAggregator, GlobalDemand};
    use tl_ui::hub::{GlobalHub, SnapshotHub};
    use tl_ui::i18n::tr;
    use tl_ui::pages::performance::overview::OVERVIEW_KEY;
    use tl_ui::pages::performance::sidebar::DeviceSidebar;
    use tl_ui::pages::performance::{DeviceKey, DeviceKind};
    use tl_ui::settings_global::TlSettings;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::registry::{GlobalRegistry, OpenOutcome, WindowKey};
    use tl_ui::windows::mini::{MINI_WINDOW_TITLE, MiniWindow, mini_height, open_mini};

    struct Empty;

    impl gpui_kit::Render for Empty {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            gpui_kit::div()
        }
    }

    fn entry(idx: u16, identity: DeviceIdentity, net_kind: Option<NetKind>) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("device"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind,
        }
    }

    fn disk(idx: u16, name: &str) -> DeviceEntry {
        let identity = DeviceIdentity::Disk {
            major: 8,
            minor: idx.into(),
            name: Arc::from(name),
            wwn_or_serial: None,
        };
        entry(idx, identity, None)
    }

    fn net(idx: u16, name: &str, kind: NetKind) -> DeviceEntry {
        let identity = DeviceIdentity::Net {
            netns_ino: 1,
            ifindex: idx.into(),
            name: Arc::from(name),
        };
        entry(idx, identity, Some(kind))
    }

    /// Nine visible cards: CPU, Memory, a GPU, four disks, a wired interface and a VPN tunnel.
    /// `extra_disks` more disks follow.
    fn catalog(version: u64, extra_disks: u16) -> DeviceCatalog {
        let gpu = DeviceIdentity::Gpu {
            uuid: [0; 16],
            pci_bus_id: Arc::from("0000:2d:00.0"),
        };
        let mut entries = vec![
            entry(0, DeviceIdentity::CpuPackage, None),
            entry(1, DeviceIdentity::Memory, None),
            entry(2, gpu, None),
            disk(3, "sda"),
            disk(4, "sdb"),
            disk(5, "sdc"),
            disk(6, "sdd"),
            net(7, "enp38s0", NetKind::Wired),
            net(8, "tailscale0", NetKind::VpnTunnel),
        ];
        for i in 0..extra_disks {
            entries.push(disk(9 + i, &format!("nvme{i}n1")));
        }
        DeviceCatalog {
            version,
            entries: entries.into(),
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
    }

    fn setup(cx: &mut TestAppContext) -> Rig {
        let mut settings = Settings::default();
        settings
            .performance
            .device_overrides
            .insert(OVERVIEW_KEY.to_owned(), DeviceVisibility::Hide);
        let engine = FakeEngine::new();
        engine.publish_with(|s| s.devices = Arc::new(catalog(1, 0)));
        let client: Arc<dyn EngineClient> = engine.clone();
        let (hub, demand) = cx.update(|cx| {
            gpui_kit::init(cx);
            let appearance = AppearanceSettings::default();
            tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
            tl_ui::theme::metrics::init_metrics(&appearance, cx);
            tl_ui::i18n::init_strings(cx, "en-US");
            cx.set_global(EngineGlobal(Arc::clone(&client)));
            cx.set_global(BootSettings {
                settings,
                state: UiState::default(),
                persist: None,
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
            let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
            let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
            cx.set_global(GlobalHub(hub.clone()));
            cx.set_global(GlobalDemand(demand.clone()));
            GlobalRegistry::init(cx);
            (hub, demand)
        });
        cx.run_until_parked();
        Rig {
            engine,
            hub,
            demand,
        }
    }

    fn mini_handle(cx: &mut TestAppContext) -> Option<AnyWindowHandle> {
        cx.read(|cx| {
            cx.global::<GlobalRegistry>()
                .0
                .read(cx)
                .get(&WindowKey::Mini)
        })
    }

    fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<MiniWindow>) {
        assert_eq!(cx.update(open_mini), OpenOutcome::Opened);
        let handle = mini_handle(cx).expect("the registry holds Mini");
        let view = cx
            .update(|cx| MiniWindow::find(handle, cx))
            .expect("mini view");
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        (vcx, view)
    }

    fn draw(vcx: &mut VisualTestContext) {
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
    }

    fn keys(vcx: &mut VisualTestContext, view: &Entity<MiniWindow>) -> Vec<DeviceKey> {
        vcx.read_entity(view, |v, _| v.keys().cloned().collect())
    }

    /// The window height the cards ask for, at ui_scale 1.
    fn wanted_height(vcx: &mut VisualTestContext, view: &Entity<MiniWindow>) -> f32 {
        vcx.update(|window, cx| {
            let heights: Vec<f32> = view
                .read(cx)
                .cards()
                .iter()
                .map(|c| f32::from(c.height.get()))
                .collect();
            let work = window
                .display(cx)
                .map(|d| f32::from(d.visible_bounds().size.height))
                .expect("a display");
            mini_height(heights, work, 1.)
        })
    }

    #[gpui_kit::test]
    fn mini_window_lists_visible_cards(cx: &mut TestAppContext) {
        let rig = setup(cx);
        let (mut vcx, view) = open(cx);

        // The same cards, in the same order, as a sidebar built from the same snapshot.
        let expected: Vec<DeviceKey> = vcx.update(|_, cx| {
            let sidebar = cx.new(|cx| DeviceSidebar::new(rig.hub.clone(), rig.demand.clone(), cx));
            let keys = sidebar.read(cx).visible_keys().cloned().collect();
            sidebar.update(cx, |s, cx| s.set_active(false, cx));
            keys
        });
        assert_eq!(expected.len(), 9, "{expected:?}");
        assert_eq!(keys(&mut vcx, &view), expected);
        let shown = (0..9)
            .filter(|i| {
                let id = format!("perf-card-text:{}", expected[*i].0);
                vcx.debug_bounds(Box::leak(id.into_boxed_str())).is_some()
            })
            .count();
        assert_eq!(shown, 9, "every card is drawn");
        assert!(
            vcx.debug_bounds("perf-card-selected").is_none(),
            "no card is selected"
        );

        // The caption string, the size and the private sidebar.
        assert_eq!(
            vcx.update(|_, cx| tr(cx, "mini-window-title")),
            MINI_WINDOW_TITLE
        );
        let bounds = vcx.update(|window, _| window.bounds());
        assert_eq!(bounds.size.width, gpui_kit::px(260.));
        assert_eq!(
            f32::from(bounds.size.height),
            wanted_height(&mut vcx, &view)
        );
        // 9 × 80 + 10 × 8 = 800 when no card text wraps; a card that wraps at 244 px adds its
        // extra height.
        let heights: Vec<f32> = vcx.read_entity(&view, |v, _| {
            v.cards()
                .iter()
                .map(|c| f32::from(c.height.get()))
                .collect()
        });
        assert!(heights.iter().all(|h| *h >= 80.), "{heights:?}");
        let extra: f32 = heights.iter().map(|h| h - 80.).sum();
        assert_eq!(f32::from(bounds.size.height), 800. + extra);
        let sidebar_active = vcx.read_entity(&view, |v, cx| v.sidebar().read(cx).is_active());
        assert!(!sidebar_active, "the private sidebar sends no demand");
        assert!(vcx.read_entity(&view, |v, _| v.always_on_top()));

        // Six more disks: 15 cards pass the cap (90% of the 1080 px test display) and scroll.
        rig.engine
            .publish_with(|s| s.devices = Arc::new(catalog(2, 6)));
        draw(&mut vcx);
        assert_eq!(keys(&mut vcx, &view).len(), 15);
        let height = vcx.update(|window, _| window.bounds().size.height);
        assert_eq!(f32::from(height), 972.);
        vcx.simulate_resize(size(gpui_kit::px(260.), height));
        draw(&mut vcx);
        let max = vcx.read_entity(&view, |v, _| v.scroll_handle().max_offset());
        assert!(max.y > gpui_kit::px(0.), "the list scrolls: {max:?}");
    }

    #[gpui_kit::test]
    fn mini_window_single_instance(cx: &mut TestAppContext) {
        let _rig = setup(cx);
        let (_vcx, _view) = open(cx);
        let before = cx.update(|cx| cx.windows().len());
        assert_eq!(cx.update(open_mini), OpenOutcome::Focused);
        assert_eq!(cx.update(|cx| cx.windows().len()), before);
    }

    #[gpui_kit::test]
    fn mini_window_follows_sidebar_visibility(cx: &mut TestAppContext) {
        let _rig = setup(cx);
        let (mut vcx, view) = open(cx);
        let disks =
            |keys: &[DeviceKey]| keys.iter().filter(|k| k.kind() == DeviceKind::Disk).count();
        assert_eq!(disks(&keys(&mut vcx, &view)), 4);
        assert!(vcx.read_entity(&view, |v, _| v.demand_part().is_some_and(|p| p.disks)));

        vcx.update(|_, cx| TlSettings::update(cx, |d| d.settings.performance.show_disks = false));
        draw(&mut vcx);
        let keys = keys(&mut vcx, &view);
        assert_eq!(disks(&keys), 0, "{keys:?}");
        assert_eq!(keys.len(), 5);
        assert!(vcx.read_entity(&view, |v, _| v.demand_part().is_some_and(|p| !p.disks)));
    }

    /// The private sidebar never saves its selection, so its fallback after a device goes
    /// does not replace the device the main window selected.
    #[gpui_kit::test]
    fn mini_window_keeps_saved_selection(cx: &mut TestAppContext) {
        let rig = setup(cx);
        let (mut vcx, view) = open(cx);
        let before = keys(&mut vcx, &view);
        rig.engine
            .publish_with(|s| s.devices = Arc::new(catalog(2, 1)));
        draw(&mut vcx);
        let added = keys(&mut vcx, &view)
            .into_iter()
            .find(|k| !before.contains(k))
            .expect("the new disk has a card");
        let saved = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| TlSettings::state(cx).performance.selected_device.clone())
        };
        let start = saved(&mut vcx);

        // The private sidebar selects the new disk; the main window then selects another.
        let sidebar = vcx.read_entity(&view, |v, _| v.sidebar().clone());
        vcx.update(|_, cx| sidebar.update(cx, |s, cx| s.select(added.clone(), cx)));
        assert_eq!(saved(&mut vcx), start, "the private selection is not saved");
        let main_pick = before
            .iter()
            .find(|k| k.kind() == DeviceKind::Disk)
            .expect("a disk")
            .clone();
        vcx.update(|_, cx| {
            let key = main_pick.0.to_string();
            TlSettings::update(cx, |d| d.state.performance.selected_device = key);
        });

        // The new disk goes: the private sidebar moves its selection, the saved one stays.
        rig.engine
            .publish_with(|s| s.devices = Arc::new(catalog(3, 0)));
        draw(&mut vcx);
        let moved = vcx.read_entity(&sidebar, |s, _| s.selected().cloned());
        assert!(
            moved.is_some_and(|k| k != added),
            "the private sidebar fell back"
        );
        assert_eq!(*saved(&mut vcx), *main_pick.0);
    }

    #[gpui_kit::test]
    fn mini_window_esc_closes(cx: &mut TestAppContext) {
        let _rig = setup(cx);
        let (mut vcx, _view) = open(cx);
        let handle = mini_handle(cx).expect("open");
        vcx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(mini_handle(cx).is_none(), "the registry dropped Mini");
        assert!(
            !cx.update(|cx| cx
                .windows()
                .iter()
                .any(|w| w.window_id() == handle.window_id())),
            "the window closed"
        );
    }

    #[gpui_kit::test]
    fn mini_window_demand_while_main_hidden(cx: &mut TestAppContext) {
        let rig = setup(cx);
        // A stand-in main window with a Processes part.
        let main: AnyWindowHandle = cx.add_window(|_, _| Empty).into();
        rig.demand.update(cx, |d, cx| {
            d.set_main_window(main);
            let part = Demand {
                any_surface_visible: true,
                process_table: true,
                ..Demand::default()
            };
            d.set(gpui_kit::EntityId::from(1u64), Some(main), part, cx);
        });
        let (_vcx, _view) = open(cx);

        // Hidden: a change waits more than 1000 ms with no main window render.
        rig.engine
            .publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(1500));
        cx.run_until_parked();
        assert!(rig.hub.read_with(cx, |h, _| h.is_hidden()));
        cx.executor().advance_clock(DEMAND_COALESCE);
        cx.run_until_parked();

        let merged = rig.demand.read_with(cx, |d, _| d.merged());
        assert!(merged.any_surface_visible);
        assert!(!merged.process_table, "the main window's part is left out");
        assert_eq!(merged.cpu, CpuDemand::Total);
        assert!(
            merged.memory && merged.disks && merged.nets && merged.gpus,
            "{merged:?}"
        );
        let sent = rig
            .engine
            .demands()
            .last()
            .cloned()
            .expect("a demand was sent");
        assert!(
            sent.memory && sent.disks && sent.nets && sent.gpus,
            "{sent:?}"
        );
    }
}
