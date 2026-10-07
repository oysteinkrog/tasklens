//! Minigraph actions (tasklens-xdh.4, UI-PRF-040 to UI-PRF-045, UI-PRC-020).

use std::sync::Arc;

use gpui_kit::{Action, TestAppContext};
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;
use tl_ui::pages::minigraph::GraphColor;
use tl_ui::pages::minigraph::actions::{
    AddAppGraph, AddMinigraph, AddNameGraph, CloseMiniWindow, MinigraphAddToWindow,
    MinigraphAttach, MinigraphAttachAll, MinigraphCloseDetachedWindow, MinigraphDetach,
    MinigraphHide, MinigraphMoveToSlot, MinigraphRemove, MinigraphSetColor, MinigraphSetRange,
    ResetMinigraphLayout, ToggleMiniAlwaysOnTop,
};
use tl_ui::pages::minigraph::sources::{GraphColumn, MinigraphId, MinigraphSource};

fn id() -> MinigraphId {
    MinigraphId(3)
}

fn all() -> Vec<Box<dyn Action>> {
    vec![
        Box::new(ResetMinigraphLayout),
        Box::new(MinigraphAttachAll),
        Box::new(ToggleMiniAlwaysOnTop),
        Box::new(CloseMiniWindow),
        Box::new(MinigraphHide { id: id() }),
        Box::new(MinigraphRemove { id: id() }),
        Box::new(MinigraphSetRange {
            id: id(),
            secs: 300,
        }),
        Box::new(MinigraphSetColor {
            id: id(),
            color: GraphColor::Teal,
        }),
        Box::new(MinigraphMoveToSlot { id: id(), slot: 7 }),
        Box::new(MinigraphDetach { id: id() }),
        Box::new(MinigraphAttach { id: id() }),
        Box::new(MinigraphAddToWindow {
            window: 2,
            source: MinigraphSource::Process {
                key: ProcessKey {
                    start_ticks: 1_234_567,
                    boot_id: BootId(0x0123_4567_89ab_cdef),
                    pid: 4242,
                },
            },
        }),
        Box::new(MinigraphCloseDetachedWindow { window: 2 }),
        Box::new(AddMinigraph {
            column: GraphColumn::RssShared,
        }),
        Box::new(AddNameGraph {
            column: GraphColumn::Cpu,
        }),
        Box::new(AddAppGraph {
            column: GraphColumn::GpuShared,
        }),
    ]
}

#[gpui_kit::test]
fn minigraph_actions_in_namespace(cx: &mut TestAppContext) {
    let all = all();
    assert_eq!(all.len(), 16);
    cx.read(|cx| {
        let names = cx.all_action_names();
        for action in &all {
            let name = action.name();
            assert!(name.starts_with("tasklens::"), "{name}");
            assert!(names.contains(&name), "{name} is registered");
        }
    });
}

/// Every action survives serialising its fields and building it again by name, the path the
/// row menu and tray bridge use.
#[gpui_kit::test]
fn minigraph_action_round_trip(cx: &mut TestAppContext) {
    let range = MinigraphSetRange {
        id: id(),
        secs: 300,
    };
    assert_eq!(
        serde_json::to_value(&range).unwrap(),
        serde_json::json!({ "id": 3, "secs": 300 })
    );
    let add = MinigraphAddToWindow {
        window: 2,
        source: MinigraphSource::Gpu(Some(tl_ui::pages::performance::DeviceKey(Arc::from(
            "gpu:GPU-1",
        )))),
    };
    assert_eq!(
        serde_json::to_value(&add).unwrap(),
        serde_json::json!({ "window": 2, "source": "gpu:gpu:GPU-1" })
    );
    let set_color = MinigraphSetColor {
        id: id(),
        color: GraphColor::Lavender,
    };
    assert_eq!(
        serde_json::to_value(&set_color).unwrap()["color"],
        serde_json::json!("lavender")
    );
    let mut actions = all();
    actions.push(Box::new(range));
    actions.push(Box::new(add));
    cx.read(|cx| {
        for action in &actions {
            let json = action_json(action.as_ref());
            let built = cx
                .build_action(action.name(), json)
                .unwrap_or_else(|e| panic!("build {}: {e:?}", action.name()));
            assert!(built.partial_eq(action.as_ref()), "got {built:?}");
        }
    });
}

/// The JSON fields of a data action, `None` for a unit action.
fn action_json(action: &dyn Action) -> Option<serde_json::Value> {
    let any = action.as_any();
    macro_rules! data {
        ($($ty:ty),*) => {
            $(if let Some(a) = any.downcast_ref::<$ty>() {
                return Some(serde_json::to_value(a).unwrap());
            })*
        };
    }
    data!(
        MinigraphHide,
        MinigraphRemove,
        MinigraphSetRange,
        MinigraphSetColor,
        MinigraphMoveToSlot,
        MinigraphDetach,
        MinigraphAttach,
        MinigraphAddToWindow,
        MinigraphCloseDetachedWindow,
        AddMinigraph,
        AddNameGraph,
        AddAppGraph
    );
    None
}

#[gpui_kit::test]
fn add_minigraph_from_json(cx: &mut TestAppContext) {
    cx.read(|cx| {
        let built = cx
            .build_action(
                AddMinigraph {
                    column: GraphColumn::Vram,
                }
                .name(),
                Some(serde_json::json!({ "column": "vram" })),
            )
            .expect("build AddMinigraph");
        assert!(built.partial_eq(&AddMinigraph {
            column: GraphColumn::Vram,
        }));
        let bad = cx.build_action(
            "tasklens::MinigraphAddToWindow",
            Some(serde_json::json!({ "window": 1, "source": "bogus:1" })),
        );
        assert!(bad.is_err(), "an unknown source does not build");
    });
}

/// `MinigraphCard` for the seven system sources (tasklens-xdh.5, PERF-080, UI-PRF-041).
#[cfg(feature = "test-hooks")]
mod card {
    use std::sync::Arc;

    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
        Styled as _, TestAppContext, VisualTestContext, Window, div, px,
    };
    use tl_model::client::EngineClient;
    use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
    use tl_model::series::{Sample, SeriesView};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_model::snapshot::{ComponentHeader, SeriesSet};
    use tl_model::time::now_boot_ns;
    use tl_ui::chart::YScale;
    use tl_ui::hub::SnapshotHub;
    use tl_ui::pages::minigraph::GraphColor;
    use tl_ui::pages::minigraph::card::{MinigraphCard, card_shadow};
    use tl_ui::pages::minigraph::sources::{
        GraphColumn, MinigraphEntry, MinigraphId, MinigraphSource,
    };
    use tl_ui::pages::performance::DeviceKey;
    use tl_ui::testing::fake_engine::FakeEngine;

    const CPU: DeviceIdx = DeviceIdx(0);
    const MEMORY: DeviceIdx = DeviceIdx(1);
    const DISKS: DeviceIdx = DeviceIdx(2);
    const NETS: DeviceIdx = DeviceIdx(3);
    const GPU_A: DeviceIdx = DeviceIdx(4);
    const GPU_B: DeviceIdx = DeviceIdx(5);

    fn entry(idx: DeviceIdx, identity: DeviceIdentity) -> DeviceEntry {
        DeviceEntry {
            idx,
            identity,
            generation: 0,
            display_name: Arc::from("dev"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
        }
    }

    fn gpu(bus: &str) -> DeviceIdentity {
        DeviceIdentity::Gpu {
            uuid: [0; 16],
            pci_bus_id: Arc::from(bus),
        }
    }

    fn catalog() -> DeviceCatalog {
        DeviceCatalog {
            version: 1,
            entries: vec![
                entry(CPU, DeviceIdentity::CpuPackage),
                entry(MEMORY, DeviceIdentity::Memory),
                entry(DISKS, DeviceIdentity::Aggregate(AggregateKind::Disks)),
                entry(NETS, DeviceIdentity::Aggregate(AggregateKind::Nets)),
                entry(GPU_A, gpu("0000:01:00.0")),
                entry(GPU_B, gpu("0000:02:00.0")),
            ]
            .into(),
        }
    }

    fn mini(source: MinigraphSource) -> MinigraphEntry {
        MinigraphEntry {
            id: MinigraphId(1),
            source,
            column: GraphColumn::Cpu,
            range_s: 60,
            color: GraphColor::Accent,
            slot: 0,
            detached: false,
            window: 0,
        }
    }

    /// 60 one-second samples of `value` ending now, at `version`.
    fn series(device: DeviceIdx, metric: MetricId, value: f32, version: u64) -> Arc<SeriesView> {
        const PERIOD: u64 = 1_000_000_000;
        let newest = now_boot_ns();
        Arc::new(SeriesView {
            id: SeriesId { device, metric },
            version,
            period_ns: PERIOD,
            samples: (0..60u64)
                .map(|k| Sample::valid(newest - (59 - k) * PERIOD, value))
                .collect::<Vec<_>>()
                .into(),
        })
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        hub: Entity<SnapshotHub>,
        series_version: u64,
    }

    fn rig(cx: &mut TestAppContext, shadow_pct: u8) -> Rig {
        let appearance = AppearanceSettings {
            shadow_intensity_pct: shadow_pct,
            ..AppearanceSettings::default()
        };
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(cx, &ThemeSettings::default(), &appearance, Vec::new());
            tl_ui::theme::metrics::init_metrics(&appearance, cx);
            tl_ui::i18n::init_strings(cx, "en-US");
        });
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(client, cx));
        let mut r = Rig {
            engine,
            hub,
            series_version: 0,
        };
        r.publish(cx, Vec::new());
        r
    }

    impl Rig {
        /// Publishes the catalog and `views` (sorted here) as the series set.
        fn publish(&mut self, cx: &mut TestAppContext, mut views: Vec<Arc<SeriesView>>) {
            views.sort_by_key(|v| v.id);
            self.series_version += 1;
            let version = self.series_version;
            self.engine.publish_with(|s| {
                s.devices = Arc::new(catalog());
                s.series = Arc::new(SeriesSet {
                    header: ComponentHeader {
                        version,
                        ..ComponentHeader::default()
                    },
                    series: views.into(),
                    slow: Arc::from([]),
                });
            });
            cx.run_until_parked();
        }

        fn card(
            &self,
            cx: &mut TestAppContext,
            source: MinigraphSource,
            order: Vec<DeviceKey>,
        ) -> (Entity<MinigraphCard>, VisualTestContext) {
            let hub = self.hub.clone();
            let (host, vcx) = cx.add_window_view(move |_, cx| Host {
                card: cx.new(|cx| MinigraphCard::new(mini(source), &catalog(), &order, &hub, cx)),
            });
            let mut vcx = vcx.clone();
            let card = vcx.read_entity(&host, |h, _| h.card.clone());
            draw(&mut vcx);
            (card, vcx)
        }
    }

    /// Embeds the card cached, as the panel does: the window's root re-renders on every draw.
    struct Host {
        card: Entity<MinigraphCard>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size(px(400.)).child(
                self.card
                    .clone()
                    .cached(StyleRefinement::default().w(px(180.)).h(px(90.))),
            )
        }
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui_kit::test]
    fn card_labels_system(cx: &mut TestAppContext) {
        let r = rig(cx, 100);
        let cases = [
            (
                MinigraphSource::Cpu,
                "CPU",
                YScale::Fixed {
                    min: 0.0,
                    max: 100.0,
                },
            ),
            (MinigraphSource::Memory, "Memory", YScale::FixedTotal(0.0)),
            (
                MinigraphSource::Disk,
                "Disk",
                YScale::Fixed {
                    min: 0.0,
                    max: 100.0,
                },
            ),
            (
                MinigraphSource::Network,
                "Network",
                YScale::AutoRate { iec: false },
            ),
            (
                MinigraphSource::Gpu(None),
                "GPU",
                YScale::Fixed {
                    min: 0.0,
                    max: 100.0,
                },
            ),
            (MinigraphSource::Vram(None), "VRAM", YScale::FixedTotal(0.0)),
            (
                MinigraphSource::GpuTemp(None),
                "GPU temp",
                YScale::StickyTemp,
            ),
        ];
        for (source, label, y) in cases {
            let (card, mut vcx) = r.card(cx, source.clone(), Vec::new());
            let (got, scale) = vcx.update(|_, cx| {
                let c = card.read(cx);
                (
                    c.label().to_string(),
                    c.chart().read(cx).spec.y.scale_bits(),
                )
            });
            assert_eq!(got, label, "{source:?}");
            assert_eq!(scale, y.scale_bits(), "{source:?}");
        }
    }

    #[gpui_kit::test]
    fn card_gpu_none_is_first_in_sidebar_order(cx: &mut TestAppContext) {
        let r = rig(cx, 100);
        let b = DeviceKey(Arc::from("gpu:pci:0000:02:00.0"));
        let a = DeviceKey(Arc::from("gpu:pci:0000:01:00.0"));
        let (card, mut vcx) = r.card(cx, MinigraphSource::Gpu(None), vec![b, a]);
        let ids = vcx.update(|_, cx| card.read(cx).series_ids(cx));
        assert_eq!(
            ids.as_slice(),
            [SeriesId {
                device: GPU_B,
                metric: MetricId::GPU_UTIL
            }]
        );
    }

    #[gpui_kit::test]
    fn card_cpu_tooltip_two_lines(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100);
        let (card, mut vcx) = r.card(cx, MinigraphSource::Cpu, Vec::new());
        r.publish(
            cx,
            vec![
                series(CPU, MetricId::CPU_TOTAL, 88.6, 1),
                series(CPU, MetricId::CPU_KERNEL, 4.1, 1),
            ],
        );
        draw(&mut vcx);
        let lines = vcx.update(|_, cx| card.read(cx).tooltip_lines().cloned());
        let lines = lines.expect("a CPU card has a tooltip");
        assert_eq!(lines[0].as_ref(), "\u{2500}\u{2500} Total 88.6%");
        assert_eq!(lines[1].as_ref(), "\u{2500}\u{2500} Kernel 4.1%");
        let value = vcx.update(|_, cx| card.read(cx).value().to_string());
        assert_eq!(value, "88.6%");
    }

    /// The card adds no render of its own for an equal value. A snapshot that changes none of
    /// its series renders neither the card nor its chart. A new sample with the same value
    /// renders the chart, and gpui re-renders the card as the chart's ancestor (crw.34), but the
    /// card does not notify and its value text is unchanged.
    #[gpui_kit::test]
    fn card_no_rerender_on_equal_value(cx: &mut TestAppContext) {
        let mut r = rig(cx, 100);
        let (card, mut vcx) = r.card(cx, MinigraphSource::Cpu, Vec::new());
        let cpu = |v| {
            vec![
                series(CPU, MetricId::CPU_TOTAL, 50.0, v),
                series(CPU, MetricId::CPU_KERNEL, 5.0, v),
            ]
        };
        r.publish(cx, cpu(1));
        draw(&mut vcx);
        let counts = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| {
                let c = card.read(cx);
                (
                    c.render_count.get(),
                    c.notify_count.get(),
                    c.chart().read(cx).render_count.get(),
                )
            })
        };
        let before = counts(&mut vcx);

        // Another source's series changed: nothing of this card renders.
        let mut other = cpu(1);
        other.push(series(MEMORY, MetricId::MEM_USED, 1.0, 2));
        r.publish(cx, other);
        draw(&mut vcx);
        assert_eq!(counts(&mut vcx), before);

        // Same newest values in new series versions.
        r.publish(cx, cpu(3));
        draw(&mut vcx);
        let after = counts(&mut vcx);
        assert_eq!(
            after.1, before.1,
            "the card does not notify for an equal value"
        );
        assert_eq!(after.2, before.2 + 1, "the chart draws the new sample");
        let value = vcx.update(|_, cx| card.read(cx).value().to_string());
        assert_eq!(value, "50.0%");
    }

    #[gpui_kit::test]
    fn card_shadow_follows_intensity(cx: &mut TestAppContext) {
        let _r = rig(cx, 50);
        let shadow = cx.update(|cx| card_shadow(tl_ui::theme::current(cx), 1.0));
        let shadow = shadow.expect("a shadow at 50 %");
        assert!((shadow.color.a - 0.175).abs() < 1e-6, "{}", shadow.color.a);
        assert_eq!(shadow.offset.y, gpui_kit::px(4.));
        assert_eq!(shadow.blur_radius, gpui_kit::px(12.));
    }
}

/// The panel (tasklens-xdh.6, E26-004, UI-PRF-040).
mod panel {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use gpui_kit::{
        App, AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton,
        ParentElement as _, Render, StyleRefinement, Styled as _, TestAppContext,
        VisualTestContext, Window, div, px, size,
    };
    use tl_model::client::EngineClient;
    use tl_model::demand::CpuDemand;
    use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity};
    use tl_model::ids::DeviceIdx;
    use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
    use tl_ui::app::{BootSettings, EngineGlobal};
    use tl_ui::components::menu::{MenuEntryKind, TlMenuPopup, open_context_menu};
    use tl_ui::components::toast::ToastKind;
    use tl_ui::demand::DemandAggregator;
    use tl_ui::hub::SnapshotHub;
    use tl_ui::pages::minigraph::actions::{
        MinigraphHide, MinigraphSetRange, ResetMinigraphLayout,
    };
    use tl_ui::pages::minigraph::panel::{MinigraphPanel, PanelFull, toast_full};
    use tl_ui::pages::minigraph::sources::{
        MinigraphEntry, MinigraphId, MinigraphSource, default_entries,
    };
    use tl_ui::settings_global::{SettingKey, SettingValue, TlSettings};
    use tl_ui::testing::fake_engine::FakeEngine;

    fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
        DeviceEntry {
            idx: DeviceIdx(idx),
            identity,
            generation: 0,
            display_name: Arc::from("dev"),
            present: true,
            parent: None,
            persistent_key: Arc::from(""),
            net_kind: None,
        }
    }

    fn catalog() -> DeviceCatalog {
        DeviceCatalog {
            version: 1,
            entries: vec![
                entry(0, DeviceIdentity::CpuPackage),
                entry(1, DeviceIdentity::Memory),
                entry(2, DeviceIdentity::Aggregate(AggregateKind::Disks)),
                entry(3, DeviceIdentity::Aggregate(AggregateKind::Nets)),
                entry(
                    4,
                    DeviceIdentity::Gpu {
                        uuid: [0; 16],
                        pci_bus_id: Arc::from("0000:01:00.0"),
                    },
                ),
            ]
            .into(),
        }
    }

    struct Rig {
        engine: Arc<FakeEngine>,
        panel: Entity<MinigraphPanel>,
        vcx: VisualTestContext,
        saved: Arc<Mutex<Vec<UiState>>>,
    }

    /// Hosts the panel cached over a 1000 × 600 box, as the Processes page does.
    struct Host {
        panel: Entity<MinigraphPanel>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().relative().w(px(1000.)).h(px(600.)).child(
                self.panel
                    .clone()
                    .cached(StyleRefinement::default().absolute().inset_0()),
            )
        }
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let saved: Arc<Mutex<Vec<UiState>>> = Arc::default();
        let sink = Arc::clone(&saved);
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
                persist: Some(Box::new(move |_, state| {
                    sink.lock().expect("lock").push(state.clone());
                })),
                startup_toasts: Vec::new(),
            });
            TlSettings::init(cx);
        });
        engine.publish_with(|s| s.devices = Arc::new(catalog()));
        cx.run_until_parked();
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        let (host, vcx) = cx.add_window_view(move |_, cx| Host {
            panel: cx.new(|cx| MinigraphPanel::new(&hub, &demand, cx)),
        });
        let mut vcx = vcx.clone();
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(1000.), px(600.)));
        let panel = vcx.read_entity(&host, |h, _| h.panel.clone());
        draw(&mut vcx);
        Rig {
            engine,
            panel,
            vcx,
            saved,
        }
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    impl Rig {
        fn attached(&mut self) -> Vec<(MinigraphSource, u16)> {
            let panel = self.panel.clone();
            self.vcx.update(|_, cx| {
                let mut v: Vec<_> = panel
                    .read(cx)
                    .attached()
                    .map(|e| (e.source.clone(), e.slot))
                    .collect();
                v.sort_by_key(|(_, slot)| *slot);
                v
            })
        }

        fn id_of(&mut self, source: &MinigraphSource) -> MinigraphId {
            let panel = self.panel.clone();
            self.vcx.update(|_, cx| {
                panel
                    .read(cx)
                    .attached()
                    .find(|e| &e.source == source)
                    .map(|e| e.id)
                    .expect("card present")
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

        fn graphs(&mut self) -> tl_model::settings::GraphsSettings {
            self.vcx
                .update(|_, cx| TlSettings::settings(cx).graphs.clone())
        }

        fn set(&mut self, key: SettingKey, on: bool) {
            self.vcx
                .update(|_, cx| TlSettings::set(cx, key, SettingValue::Bool(on)));
            draw(&mut self.vcx);
        }
    }

    fn defaults() -> Vec<(MinigraphSource, u16)> {
        default_entries()
            .into_iter()
            .map(|e: MinigraphEntry| (e.source, e.slot))
            .collect()
    }

    #[gpui_kit::test]
    fn panel_slots_fill_bottom_right_first(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        assert_eq!(r.attached(), defaults());
        let cards = r.vcx.update(|_, cx| r.panel.read(cx).cards().len());
        assert_eq!(cards, 6);
        // Slot 0 (CPU) is at the bottom right, 16 px in; slot 1 (Memory) is left of it, 8 px
        // apart; slot 4 (GPU) is row 1, column 0.
        let bounds = |vcx: &mut VisualTestContext, slot: u16| {
            vcx.debug_bounds(Box::leak(format!("MinigraphSlot-{slot}").into_boxed_str()))
                .expect("slot drawn")
        };
        let cpu = bounds(&mut r.vcx, 0);
        assert_eq!(cpu.bottom_right(), gpui_kit::point(px(984.), px(584.)));
        assert_eq!(cpu.size, size(px(180.), px(90.)));
        let memory = bounds(&mut r.vcx, 1);
        assert_eq!(memory.bottom_right(), gpui_kit::point(px(796.), px(584.)));
        let gpu = bounds(&mut r.vcx, 4);
        assert_eq!(gpu.bottom_right(), gpui_kit::point(px(984.), px(486.)));
        assert!(r.vcx.debug_bounds("MinigraphSlot-6").is_none());
    }

    #[gpui_kit::test]
    fn panel_full_toast(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let panel = r.panel.clone();
        for _ in 0..6 {
            r.vcx.update(|_, cx| {
                panel
                    .update(cx, |p, cx| p.add_system(MinigraphSource::Cpu, cx))
                    .expect("a free slot");
            });
        }
        let full = r.vcx.update(|_, cx| {
            panel.update(cx, |p, cx| p.add_system(MinigraphSource::GpuTemp(None), cx))
        });
        assert_eq!(full, Err(PanelFull));
        r.vcx.update(|_, cx| toast_full(&panel, cx));
        let want = (
            ToastKind::Warning,
            "No space for more minigraphs".to_owned(),
        );
        assert_eq!(r.toasts(), std::slice::from_ref(&want));

        // View > Graphs > GPU temp on a full grid toasts the same.
        r.set(SettingKey::GraphsMinigraphGpuTemperature, true);
        assert_eq!(r.toasts(), [want.clone(), want]);
        assert_eq!(r.attached().len(), 12);
    }

    #[gpui_kit::test]
    fn panel_reset_restores_defaults(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let panel = r.panel.clone();
        let memory = r.id_of(&MinigraphSource::Memory);
        r.vcx.dispatch_action(MinigraphHide { id: memory });
        draw(&mut r.vcx);
        let cpu = r.id_of(&MinigraphSource::Cpu);
        r.vcx.update(|_, cx| {
            panel.update(cx, |p, cx| {
                p.remove(cpu, cx);
                p.add_system(MinigraphSource::Name(Arc::from("x")), cx)
                    .expect("slot 0");
                p.add_system(MinigraphSource::Cpu, cx).expect("slot 1");
            });
        });
        draw(&mut r.vcx);
        assert!(!r.attached().contains(&(MinigraphSource::Memory, 1)));
        assert!(r.attached().contains(&(MinigraphSource::Cpu, 1)));

        r.vcx.dispatch_action(ResetMinigraphLayout);
        draw(&mut r.vcx);
        assert_eq!(r.attached(), defaults());
        assert!(r.graphs().minigraph_memory, "Memory is on again");
        assert_eq!(
            r.toasts(),
            [(ToastKind::Success, "Minigraph layout reset".to_owned())]
        );
        let cards = r.vcx.update(|_, cx| panel.read(cx).cards().len());
        assert_eq!(cards, 6);
    }

    #[gpui_kit::test]
    fn panel_follows_graph_settings(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        r.set(SettingKey::GraphsMinigraphDisk, false);
        assert!(
            r.attached()
                .iter()
                .all(|(s, _)| *s != MinigraphSource::Disk)
        );
        assert!(r.vcx.debug_bounds("MinigraphSlot-2").is_none());
        r.set(SettingKey::GraphsMinigraphDisk, true);
        assert!(
            r.attached().contains(&(MinigraphSource::Disk, 2)),
            "the next free slot"
        );
        r.set(SettingKey::GraphsMinigraphGpuTemperature, true);
        assert!(r.attached().contains(&(MinigraphSource::GpuTemp(None), 6)));
        assert!(r.vcx.debug_bounds("MinigraphSlot-6").is_some());
    }

    #[gpui_kit::test]
    fn panel_hide_system_card_writes_setting(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let cpu = r.id_of(&MinigraphSource::Cpu);
        r.vcx.dispatch_action(MinigraphHide { id: cpu });
        draw(&mut r.vcx);
        assert!(!r.graphs().minigraph_cpu);
        assert!(r.attached().iter().all(|(s, _)| *s != MinigraphSource::Cpu));
    }

    #[gpui_kit::test]
    fn panel_demand_sum(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let panel = r.panel.clone();
        let d = r.vcx.update(|_, cx| panel.read(cx).demand_part(cx));
        assert_eq!(d.cpu, CpuDemand::Total);
        assert!(d.any_surface_visible && d.memory && d.disks && d.nets && d.gpus);
        assert!(!d.cpu_fast);

        // Sent only while the Processes tab is shown.
        r.vcx.executor().advance_clock(Duration::from_millis(200));
        draw(&mut r.vcx);
        assert!(r.engine.demands().iter().all(|d| !d.memory));
        r.vcx
            .update(|_, cx| panel.update(cx, |p, cx| p.set_active(true, cx)));
        r.vcx.executor().advance_clock(Duration::from_millis(200));
        draw(&mut r.vcx);
        let sent = r.engine.demands().last().cloned().expect("demand sent");
        assert!(sent.memory && sent.gpus && sent.cpu == CpuDemand::Total);
        assert!(!sent.cpu_fast);
    }

    #[gpui_kit::test]
    fn panel_persists_state(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let panel = r.panel.clone();
        r.vcx.update(|_, cx| {
            panel
                .update(cx, |p, cx| p.add_system(MinigraphSource::GpuTemp(None), cx))
                .expect("slot 6");
        });
        r.vcx.executor().advance_clock(Duration::from_millis(600));
        draw(&mut r.vcx);
        let saved = r
            .saved
            .lock()
            .expect("lock")
            .last()
            .cloned()
            .expect("persisted");
        let card = saved
            .minigraphs
            .iter()
            .find(|m| m.source == "gpu_temp")
            .expect("the new card in [[minigraphs]]");
        assert_eq!(card.slot, 6);
        assert_eq!(saved.minigraphs.len(), 7);
    }

    type Entry = (MenuEntryKind, Option<String>, Option<String>, bool);

    fn entries(window: &Window, menu: &Entity<TlMenuPopup>, cx: &App) -> Vec<Entry> {
        menu.read(cx)
            .entries_for_test(window)
            .into_iter()
            .map(|e| {
                (
                    e.kind,
                    e.label.map(|l| l.to_string()),
                    e.tooltip.map(|t| t.to_string()),
                    e.checked,
                )
            })
            .collect()
    }

    /// Right-clicks the card in `slot` and returns the open menu and its Time range submenu.
    fn right_click(r: &mut Rig, slot: u16) -> (Vec<Entry>, Vec<Entry>) {
        let selector: &'static str = Box::leak(format!("MinigraphSlot-{slot}").into_boxed_str());
        let card = r.vcx.debug_bounds(selector).expect("card drawn");
        r.vcx
            .simulate_mouse_down(card.center(), MouseButton::Right, Modifiers::none());
        draw(&mut r.vcx);
        r.vcx.update(|window, cx| {
            let menu = open_context_menu(window, cx).expect("the card menu is open");
            let range = menu
                .read(cx)
                .entries_for_test(window)
                .into_iter()
                .find_map(|e| e.submenu)
                .expect("the Time range submenu");
            (entries(window, &menu, cx), entries(window, &range, cx))
        })
    }

    fn item(label: &str) -> Entry {
        (MenuEntryKind::Item, Some(label.to_owned()), None, false)
    }

    #[gpui_kit::test]
    fn card_menu_order_system(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let (menu, range) = right_click(&mut r, 0);
        assert_eq!(
            menu,
            [
                item("Hide graph"),
                (
                    MenuEntryKind::Submenu,
                    Some("Time range".to_owned()),
                    None,
                    false
                ),
                (MenuEntryKind::Separator, None, None, false),
                item("Reset minigraph layout"),
            ]
        );
        let checked: Vec<_> = range
            .iter()
            .filter(|e| e.3)
            .map(|e| e.1.clone().unwrap_or_default())
            .collect();
        assert_eq!(checked, ["1 minute"]);
    }

    #[gpui_kit::test]
    fn card_menu_long_ranges_disabled_before_p2(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let (_, range) = right_click(&mut r, 0);
        let later = Some("Available in a later version".to_owned());
        assert_eq!(
            range,
            [
                (
                    MenuEntryKind::Item,
                    Some("30 seconds".to_owned()),
                    None,
                    false
                ),
                (MenuEntryKind::Item, Some("1 minute".to_owned()), None, true),
                (
                    MenuEntryKind::Disabled,
                    Some("5 minutes".to_owned()),
                    later.clone(),
                    false
                ),
                (
                    MenuEntryKind::Disabled,
                    Some("10 minutes".to_owned()),
                    later,
                    false
                ),
            ]
        );

        // A process card has Remove graph in place of Hide graph.
        let panel = r.panel.clone();
        let key = tl_model::ids::ProcessKey {
            start_ticks: 1,
            boot_id: tl_model::time::BootId(1),
            pid: 42,
        };
        r.vcx.update(|_, cx| {
            panel
                .update(cx, |p, cx| {
                    p.add_system(MinigraphSource::Process { key }, cx)
                })
                .expect("slot 6");
        });
        draw(&mut r.vcx);
        let (menu, _) = right_click(&mut r, 6);
        let labels: Vec<_> = menu.iter().filter_map(|e| e.1.clone()).collect();
        assert_eq!(
            labels,
            ["Remove graph", "Time range", "Reset minigraph layout"]
        );
    }

    #[gpui_kit::test]
    fn card_set_range_persists(cx: &mut TestAppContext) {
        let mut r = rig(cx);
        let cpu = r.id_of(&MinigraphSource::Cpu);
        r.vcx
            .dispatch_action(MinigraphSetRange { id: cpu, secs: 30 });
        draw(&mut r.vcx);
        let panel = r.panel.clone();
        let (range_ns, state) = r.vcx.update(|_, cx| {
            let card = panel
                .read(cx)
                .cards()
                .iter()
                .find(|(id, _)| *id == cpu)
                .map(|(_, c)| c.clone())
                .expect("CPU card");
            let range_ns = card.read(cx).chart().read(cx).spec.range_ns;
            (range_ns, TlSettings::state(cx).minigraphs.clone())
        });
        assert_eq!(range_ns, 30_000_000_000);
        let entry = state.iter().find(|m| m.source == "cpu").expect("cpu entry");
        assert_eq!(entry.range_s, 30);
        let saved = r
            .saved
            .lock()
            .expect("lock")
            .last()
            .cloned()
            .expect("persisted");
        assert_eq!(saved.minigraphs, state);
    }
}

/// The panel mounted in the Processes page (tasklens-xdh.8, UI-PRF-040).
mod mounted {
    use std::sync::Arc;

    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, point, px, size};
    use tl_model::client::EngineClient;
    use tl_model::command::Tab;
    use tl_model::ids::{NodeId, ProcessKey};
    use tl_model::process::{GroupKind, ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::settings::{Settings, StartTab, UiState};
    use tl_model::snapshot::{ComponentHeader, ProcessSnapshot};
    use tl_model::table::{ProcessTableView, TableRow};
    use tl_model::time::BootId;
    use tl_model::validity::{FieldValidity, Validity};
    use tl_ui::app::{AppEnv, StartOptions, TasklensUi, UiInit};
    use tl_ui::pages::minigraph::actions::ResetMinigraphLayout;
    use tl_ui::pages::minigraph::panel::MinigraphPanel;
    use tl_ui::pages::minigraph::sources::{MinigraphSource, default_entries};
    use tl_ui::pages::processes::ProcessesPage;
    use tl_ui::testing::fake_engine::FakeEngine;
    use tl_ui::window::main::{main_window_view, open_main_window};

    fn record(pid: u32) -> ProcessRecord {
        let a = |s: &str| Arc::<str>::from(s);
        ProcessRecord {
            key: ProcessKey {
                start_ticks: 100,
                boot_id: BootId(0),
                pid,
            },
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: a("demo"),
                name: a("demo"),
                exe: None,
                cmdline: None,
                user: a("u"),
                cgroup: None,
                app_id: None,
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: a("demo"),
            }),
            group: GroupKind::System.key(),
            state: ProcState::Sleeping,
            flags: ProcFlags::empty(),
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: 0,
            rss_bytes: 0,
            pss_bytes: 0,
            uss_bytes: 0,
            swap_bytes: 0,
            io_read_bps: 0.0,
            io_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            gpu_util_pct: 0.0,
            gpu_mem_bytes: 0,
            euid: None,
            net_shared: false,
            cancelled_write_bps: 0.0,
            validity: FieldValidity::all(Validity::Valid),
        }
    }

    /// `TasklensUi::init` on `start_tab` with 80 processes (in a flat table view on Processes), then the main window at 1280 × 800.
    fn open(cx: &mut TestAppContext, start_tab: StartTab) -> VisualTestContext {
        let engine = FakeEngine::new();
        let client: Arc<dyn EngineClient> = engine.clone();
        let (_tx, rx) = async_channel::bounded(4);
        let mut settings = Settings::default();
        settings.general.start_tab = start_tab;
        let init = UiInit {
            settings,
            state: UiState::default(),
            themes_json: None,
            persist: Box::new(|_, _| {}),
            persist_themes: Box::new(|_| {}),
            app_requests: rx,
            startup_toasts: Vec::new(),
            env: AppEnv::test(),
        };
        cx.update(|cx| {
            TasklensUi::init(cx, client, init);
            cx.set_global(StartOptions::default());
            cx.set_reduce_motion(true);
        });
        let records: Vec<ProcessRecord> = (100..180).map(record).collect();
        // A flat view of every record for the first table query.
        let rows: Vec<TableRow> = records
            .iter()
            .zip(0u32..)
            .map(|(r, i)| TableRow {
                node: NodeId::Process(r.key),
                depth: 0,
                expanded: false,
                has_children: false,
                record: Some(i),
                group: None,
                descendants: 0,
                subtree: None,
            })
            .collect();
        engine.publish_with(|s| {
            s.processes = Arc::new(ProcessSnapshot {
                header: ComponentHeader {
                    version: 1,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                records: records.into(),
                ..ProcessSnapshot::default()
            });
        });
        cx.run_until_parked();
        let handle = cx.update(open_main_window).expect("the main window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(1280.), px(800.)));
        draw(&mut vcx);
        // The view comes after the page's first query, as from the engine. Without a page
        // there is no query to answer.
        if start_tab != StartTab::Processes {
            return vcx;
        }
        engine.publish_with(|s| {
            s.table = Arc::new(ProcessTableView {
                header: ComponentHeader {
                    version: 1,
                    validity: Validity::Valid,
                    ..ComponentHeader::default()
                },
                query_generation: 1,
                rows: rows.into(),
                ..ProcessTableView::default()
            });
        });
        draw(&mut vcx);
        vcx
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.run_until_parked();
    }

    fn page(vcx: &mut VisualTestContext) -> gpui_kit::Entity<ProcessesPage> {
        vcx.update(|_, cx| {
            main_window_view(cx)
                .expect("main window")
                .read(cx)
                .page_entity::<ProcessesPage>(Tab::Processes)
                .expect("the Processes page is built")
        })
    }

    #[gpui_kit::test]
    fn panel_mounted_bottom_right(cx: &mut TestAppContext) {
        let mut vcx = open(cx, StartTab::Processes);
        let table = vcx.debug_bounds("TlTreeTable").expect("table drawn");
        let cpu = vcx.debug_bounds("MinigraphSlot-0").expect("CPU card drawn");
        assert_eq!(table.right() - cpu.right(), px(16.));
        assert_eq!(table.bottom() - cpu.bottom(), px(16.));
        let cards = vcx.update(|_, cx| {
            MinigraphPanel::global(cx)
                .expect("panel")
                .read(cx)
                .cards()
                .len()
        });
        assert_eq!(cards, 6);
        assert!(vcx.debug_bounds("MinigraphSlot-5").is_some());
    }

    #[gpui_kit::test]
    fn panel_absent_on_other_tabs(cx: &mut TestAppContext) {
        let mut vcx = open(cx, StartTab::Performance);
        assert!(vcx.debug_bounds("MinigraphSlot-0").is_none());
        assert!(
            vcx.update(|_, cx| MinigraphPanel::global(cx)).is_none(),
            "never built before the Processes tab"
        );

        // Visited, then left: the panel exists but draws nothing.
        let main = vcx.update(|_, cx| main_window_view(cx).expect("main window"));
        vcx.update(|window, cx| main.update(cx, |m, cx| m.select_tab(Tab::Processes, window, cx)));
        draw(&mut vcx);
        assert!(vcx.debug_bounds("MinigraphSlot-0").is_some());
        vcx.update(|window, cx| {
            main.update(cx, |m, cx| m.select_tab(Tab::Performance, window, cx))
        });
        draw(&mut vcx);
        assert!(vcx.debug_bounds("MinigraphSlot-0").is_none());
    }

    #[gpui_kit::test]
    fn gap_click_reaches_table(cx: &mut TestAppContext) {
        let mut vcx = open(cx, StartTab::Processes);
        let page = page(&mut vcx);
        let selected = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| page.read(cx).table().read(cx).selection.len())
        };
        assert_eq!(selected(&mut vcx), 0);
        let cpu = vcx.debug_bounds("MinigraphSlot-0").expect("CPU card");
        let memory = vcx.debug_bounds("MinigraphSlot-1").expect("Memory card");
        assert_eq!(cpu.left() - memory.right(), px(8.));
        let gap = point(cpu.left() - px(4.), cpu.center().y);
        vcx.simulate_click(gap, Modifiers::none());
        draw(&mut vcx);
        assert_eq!(selected(&mut vcx), 1, "the row under the gap is selected");
    }

    #[gpui_kit::test]
    fn reset_reaches_panel_from_window(cx: &mut TestAppContext) {
        let mut vcx = open(cx, StartTab::Processes);
        let panel = vcx.update(|_, cx| MinigraphPanel::global(cx).expect("panel"));
        vcx.update(|_, cx| {
            panel.update(cx, |p, cx| {
                let memory = p
                    .attached()
                    .find(|e| e.source == MinigraphSource::Memory)
                    .map(|e| e.id)
                    .expect("Memory card");
                p.hide(memory, cx);
            });
        });
        draw(&mut vcx);
        vcx.dispatch_action(ResetMinigraphLayout);
        draw(&mut vcx);
        let attached = vcx.update(|_, cx| {
            let mut v: Vec<_> = panel
                .read(cx)
                .attached()
                .map(|e| (e.source.clone(), e.slot))
                .collect();
            v.sort_by_key(|(_, slot)| *slot);
            v
        });
        let defaults: Vec<_> = default_entries()
            .into_iter()
            .map(|e| (e.source, e.slot))
            .collect();
        assert_eq!(attached, defaults);
    }
}
