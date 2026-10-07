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

    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
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
            let (card, vcx) = cx.add_window_view(move |_, cx| {
                MinigraphCard::new(mini(source), &catalog(), &order, &hub, cx)
            });
            let mut vcx = vcx.clone();
            draw(&mut vcx);
            (card, vcx)
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
