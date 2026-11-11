//! The Overview GPU box "Load by app" and "VRAM by app" modes (`tasklens-g00wx.4`): the `apps`
//! and `vram_apps` presets show the `gpu_by_app` or `vram_by_app` fixed widget, which draws
//! `AppsChart` of `AppsMetric::Gpu` or `AppsMetric::Vram` for the first GPU and fills the body.
//! Add Widget offers both, a legend click opens Processes filtered to the group, a sample with
//! the same bands notifies nothing around the chart, the chart follows the first GPU, and at
//! 24 h it reads the device metric history.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EntityId, Focusable as _, Global, IntoElement,
    Modifiers, ParentElement as _, Pixels, Render, StyleRefinement, Styled as _, Subscription,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::demand::{DeviceHistoryRequest, DeviceRange};
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{ComponentHeader, DeviceHistory, HistPoint, SeriesSet};
use tl_model::system::{GpuTotals, MemGroupBand};
use tl_model::time::now_boot_ns;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_base::shell::{ProcessJump, ShellApi, ShellState};
use tl_ui_chart::chart::ChartView;
use tl_ui_kit::components::kv_grid;
use tl_ui_overview::overview::gpu_box::{GPU_APPS_ID, GpuBox};
use tl_ui_overview::overview::gpu_flow::apps_of;
use tl_ui_overview::overview::memory_apps::{AppsChart, AppsMetric, LEGEND_ID};
use tl_ui_overview::overview::{OVERVIEW_KEY, OverviewPage};
use tl_ui_overview::widget::container::{WidgetContainer, container_id};
use tl_ui_overview::widget::edit::WidgetEditor;
use tl_ui_overview::widget::fixed_widget::FixedWidgetView;
use tl_ui_overview::widget::group_widget::GroupWidget;
use tl_ui_overview::widget::metric_widget::{MetricWidget, widget_id};
use tl_ui_perf::performance::sidebar::DeviceSidebar;
use tl_ui_perf_core::core::DeviceKey;
use tl_ui_perf_core::core::actions::{PerfOverviewSetBoxMode, PerfWidgetEditBox};
use tl_ui_settings::layout::{ContainerLayout, FixedWidget, GPU_BOX_CONTAINER, WidgetContent};
use tl_ui_settings::settings::{
    AppearanceSettings, OverviewLayoutSetting, OverviewRange, Settings, ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

use super::gpu_page::full_totals;

const S: u64 = 1_000_000_000;
const MIB: f32 = 1_048_576.;
/// The first GPU and the second.
const GPU0: DeviceIdx = DeviceIdx(2);
const GPU1: DeviceIdx = DeviceIdx(6);
const PCI0: &str = "0000:01:00.0";
const PCI1: &str = "0000:02:00.0";
/// Groups `exe:p0` to `exe:p3` of each GPU: GPU `g`'s group `i` is at `20 + 10 * g + i`, its
/// "Other apps" at `+ 4` and its "Other / driver" at `+ 5`.
const GROUPS: u16 = 4;

fn band_dev(gpu: u16, i: u16) -> DeviceIdx {
    DeviceIdx(20 + 10 * gpu + i)
}

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

fn entry(idx: DeviceIdx, identity: DeviceIdentity, key: &str, present: bool) -> DeviceEntry {
    DeviceEntry {
        idx,
        identity,
        generation: 0,
        display_name: Arc::from(key),
        present,
        parent: None,
        persistent_key: Arc::from(key),
        net_kind: None,
        hot_c: None,
    }
}

fn gpu_entry(idx: DeviceIdx, pci: &str, n: u8) -> DeviceEntry {
    entry(
        idx,
        DeviceIdentity::Gpu {
            uuid: [n; 16],
            pci_bus_id: Arc::from(pci),
        },
        &format!("gpu:{pci}"),
        true,
    )
}

/// The CPU package, memory, the GPUs in `gpus` (0 is [`GPU0`], 1 is [`GPU1`]) and each GPU's
/// bands, indexed by position as the catalog reads them; the gaps are inert bands.
fn catalog(version: u64, gpus: &[u16]) -> DeviceCatalog {
    let mut entries = vec![
        entry(DeviceIdx(0), DeviceIdentity::CpuPackage, "cpu", true),
        entry(DeviceIdx(1), DeviceIdentity::Memory, "memory", true),
    ];
    for &g in gpus {
        let (idx, pci) = if g == 0 { (GPU0, PCI0) } else { (GPU1, PCI1) };
        entries.push(gpu_entry(idx, pci, g as u8 + 1));
    }
    for (g, pci) in [(0u16, PCI0), (1, PCI1)] {
        let band = |i: u16, key: &str, name: &str| {
            let key = format!("gpu:{pci}:{key}");
            DeviceEntry {
                display_name: Arc::from(name),
                ..entry(
                    band_dev(g, i),
                    DeviceIdentity::MemGroup {
                        key: Arc::from(key.as_str()),
                    },
                    &format!("memgroup:{key}"),
                    true,
                )
            }
        };
        for i in 0..GROUPS {
            entries.push(band(i, &format!("exe:p{i}"), &format!("p{i}")));
        }
        entries.push(band(GROUPS, "other", "Other apps"));
        entries.push(band(GROUPS + 1, "driver", "Other / driver"));
    }
    entries.sort_by_key(|e| e.idx);
    let mut dense: Vec<DeviceEntry> = Vec::new();
    for e in entries {
        while dense.len() < usize::from(e.idx.0) {
            let n = dense.len() as u16;
            dense.push(entry(
                DeviceIdx(n),
                DeviceIdentity::MemGroup {
                    key: Arc::from(format!("filler:{n}")),
                },
                &format!("filler:{n}"),
                false,
            ));
        }
        dense.push(e);
    }
    DeviceCatalog {
        version,
        entries: dense.into(),
    }
}

fn gpu_totals(idx: DeviceIdx) -> GpuTotals {
    GpuTotals {
        device: idx,
        ..full_totals()
    }
}

struct Rig<'a> {
    engine: Arc<FakeEngine>,
    page: Entity<OverviewPage>,
    vcx: &'a mut VisualTestContext,
    /// The GPUs the catalog holds now.
    gpus: Vec<u16>,
    _host: Entity<Host>,
    _driver: Entity<AnimationDriver>,
}

/// One window of `w × h` with the Overview page filling it and the GPUs in `gpus`.
fn open<'a>(cx: &'a mut TestAppContext, gpus: &[u16], w: f32, h: f32) -> Rig<'a> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(catalog(1, gpus));
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.gpus = gpus
            .iter()
            .map(|&g| {
                let idx = if g == 0 { GPU0 } else { GPU1 };
                (idx, gpu_totals(idx))
            })
            .collect();
    });
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
        let mut settings = Settings::default();
        settings.motion.smooth_graphs = false;
        // Leave the Memory, Disk and Network boxes out, so the GPU box has their room. Widget
        // selectors have the container id, so other boxes do not shadow them.
        let mut overview = OverviewLayoutSetting::grid_default();
        overview
            .grid
            .retain(|b| !matches!(b.kind.as_str(), "memory" | "disk" | "net"));
        settings.performance.overview = Some(overview);
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
        gpus: gpus.to_vec(),
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

impl Rig<'_> {
    fn page_action(&mut self, action: &dyn gpui_kit::Action) {
        let page = self.page.clone();
        self.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            focus.dispatch_action(action, window, cx);
        });
        draw(self.vcx);
    }

    fn gpu(&mut self) -> Entity<GpuBox> {
        let page = self.page.clone();
        self.vcx.update(|_, cx| page.read(cx).gpu_box().clone())
    }

    fn container(&mut self) -> Entity<WidgetContainer> {
        let gpu = self.gpu();
        self.vcx.update(|_, cx| gpu.read(cx).container().clone())
    }

    fn editor(&mut self) -> Entity<WidgetEditor> {
        let gpu = self.gpu();
        self.vcx.update(|_, cx| gpu.read(cx).editor().clone())
    }

    fn flow(&mut self) -> ContainerLayout {
        let gpu = self.gpu();
        self.vcx.update(|_, cx| gpu.read(cx).flow().clone())
    }

    fn bounds_of(&mut self, id: String) -> gpui_kit::Bounds<Pixels> {
        let id: &'static str = Box::leak(id.into_boxed_str());
        self.vcx.debug_bounds(id).expect(id)
    }

    fn try_bounds(&mut self, id: String) -> Option<gpui_kit::Bounds<Pixels>> {
        let id: &'static str = Box::leak(id.into_boxed_str());
        self.vcx.debug_bounds(id)
    }

    /// The placement of widget `id` in the container's body, px.
    fn placed(&mut self, id: u32) -> Option<gpui_kit::Bounds<Pixels>> {
        let container = self.container();
        self.vcx
            .update(|_, cx| container.read(cx).placement().bounds(id))
    }

    fn set_mode(&mut self, mode: &str) {
        self.page_action(&PerfOverviewSetBoxMode {
            kind: "gpu".into(),
            mode: mode.into(),
        });
    }

    /// Whether the page asks for the process sweep and the per-process GPU rows.
    fn demand(&mut self) -> (bool, bool) {
        let page = self.page.clone();
        self.vcx.update(|_, cx| {
            let d = page.read(cx).demand_part();
            (d.process_top, d.gpu_process)
        })
    }

    /// The box's GPU load by app and VRAM by app charts.
    fn charts(&mut self) -> (Option<Entity<AppsChart>>, Option<Entity<AppsChart>>) {
        let gpu = self.gpu();
        self.vcx.update(|_, cx| {
            let b = gpu.read(cx);
            (b.gpu_apps(), b.vram_apps())
        })
    }

    /// The entity id of the content fixed widget `id` draws.
    fn content(&mut self, id: u32) -> Option<EntityId> {
        let container = self.container();
        self.vcx.update(|_, cx| {
            let fixed = container.read(cx).fixed(id)?;
            fixed.read(cx).content().map(|v| v.entity_id())
        })
    }

    /// One GPU load and one VRAM sample a second for the last minute on each GPU in the
    /// catalog: group `exe:p<i>` at `gpu[i]` percent and `vram[i]` MiB, "Other apps" at 1% and
    /// 64 MiB, "Other / driver" at 2% and 128 MiB. The catalog stays as it is.
    fn publish_groups(&mut self, gpu: &[f32], vram: &[f32]) {
        let now = now_boot_ns();
        let gpus = self.gpus.clone();
        self.engine.publish_with(|s| {
            let version = s.series.header.version + 1;
            let line = |device: DeviceIdx, metric: MetricId, value: f32| {
                Arc::new(SeriesView {
                    id: SeriesId { device, metric },
                    version,
                    period_ns: S,
                    samples: (0..60u64)
                        .map(|k| Sample::valid(now - (59 - k) * S, value))
                        .collect::<Vec<_>>()
                        .into(),
                })
            };
            let mut views = Vec::new();
            let mut totals = Vec::new();
            for &g in &gpus {
                for (list, metric, scale) in [
                    (gpu, MetricId::GPU_GROUP, 1.),
                    (vram, MetricId::VRAM_GROUP, MIB),
                ] {
                    for (i, v) in list.iter().enumerate() {
                        views.push(line(band_dev(g, i as u16), metric, v * scale));
                    }
                }
                for (i, load, mib) in [(GROUPS, 1., 64.), (GROUPS + 1, 2., 128.)] {
                    views.push(line(band_dev(g, i), MetricId::GPU_GROUP, load));
                    views.push(line(band_dev(g, i), MetricId::VRAM_GROUP, mib * MIB));
                }
                let bands = |n: usize| {
                    Arc::new(
                        (0..n as u16)
                            .map(|i| MemGroupBand {
                                key: Arc::from(format!("exe:p{i}")),
                                name: Arc::from(format!("p{i}")),
                                process_count: 2,
                                device: band_dev(g, i),
                                wine: None,
                            })
                            .collect::<Vec<_>>(),
                    )
                };
                let idx = if g == 0 { GPU0 } else { GPU1 };
                let mut t = gpu_totals(idx);
                t.groups = bands(gpu.len());
                t.vram_groups = bands(vram.len());
                totals.push((idx, t));
            }
            views.sort_by_key(|v| v.id);
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                series: views.into(),
                slow: Arc::from([]),
            });
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.gpus = totals.into();
        });
        draw(self.vcx);
    }

    /// The catalog now holds the GPUs in `gpus`.
    fn set_gpus(&mut self, gpus: &[u16]) {
        self.gpus = gpus.to_vec();
        self.engine.publish_with(|s| {
            s.devices = Arc::new(catalog(s.devices.version + 1, gpus));
        });
        draw(self.vcx);
    }
}

/// The two by-app modes, their fixed widget and the chart kind each draws.
const MODES: [(&str, FixedWidget, fn(DeviceIdx) -> AppsMetric); 2] = [
    ("apps", FixedWidget::GpuByApp, AppsMetric::Gpu),
    ("vram_apps", FixedWidget::VramByApp, AppsMetric::Vram),
];

/// Each by-app preset shows its fixed widget filling the box body, drawing the chart of the
/// first GPU, and the page asks for the process sweep and the per-process GPU rows. Back to the
/// default mode, the widget goes and so does the demand.
#[gpui_kit::test]
fn overview_gpu_by_app_preset(cx: &mut TestAppContext) {
    let mut r = open(cx, &[0], 1200., 900.);
    r.publish_groups(&[20., 10., 5.], &[512., 256., 128.]);
    assert_eq!(r.demand(), (false, false), "no sweep before By app");
    let gpu = r.gpu();
    for (mode, kind, metric) in MODES {
        r.set_mode(mode);
        r.publish_groups(&[20., 10., 5.], &[512., 256., 128.]);
        assert_eq!(r.demand(), (true, true), "{mode}");
        assert_eq!(r.vcx.update(|_, cx| gpu.read(cx).mode()), mode);
        assert!(r.vcx.update(|_, cx| gpu.read(cx).apps_placed()), "{mode}");
        let flow = r.flow();
        assert_eq!(flow.preset.as_ref().map(|p| p.id.as_str()), Some(mode));
        let id = apps_of(&flow, kind.clone()).expect("the by-app widget");
        assert_eq!(
            r.vcx
                .update(|_, cx| gpu.read(cx).drawn_widgets())
                .as_slice(),
            [id],
            "{mode}"
        );
        let (gpu_chart, vram_chart) = r.charts();
        let chart = if kind == FixedWidget::GpuByApp {
            gpu_chart
        } else {
            vram_chart
        }
        .expect("the chart");
        assert_eq!(r.content(id), Some(chart.entity_id()), "{mode}");
        assert_eq!(r.vcx.update(|_, cx| chart.read(cx).metric()), metric(GPU0));
        assert!(
            r.vcx.update(|_, cx| chart.read(cx).bands().len()) >= 4,
            "{mode}: three groups and Other apps at least"
        );
        let body = r.bounds_of(container_id(GPU_BOX_CONTAINER));
        let part = r.bounds_of(GPU_APPS_ID.to_owned());
        let widget = r.bounds_of(widget_id(GPU_BOX_CONTAINER, id));
        for b in [part, widget] {
            assert!(
                near(b.origin.x, body.origin.x)
                    && near(b.origin.y, body.origin.y)
                    && near(b.size.width, body.size.width)
                    && near(b.size.height, body.size.height),
                "{mode}: fills the body: {b:?} in {body:?}"
            );
        }
        let at = r.placed(id).expect("placed");
        assert!(
            near(at.origin.y, px(0.)) && near(at.size.height, body.size.height),
            "{mode}: placed where it is drawn: {at:?}"
        );
        let legend = r.bounds_of(format!("{LEGEND_ID}-0"));
        assert!(widget.contains(&legend.center()), "{mode}: {legend:?}");
    }
    r.set_mode("engine_charts");
    assert_eq!(r.demand(), (false, false), "no sweep without By app");
    assert!(!r.vcx.update(|_, cx| gpu.read(cx).apps_placed()));
    assert!(r.try_bounds(GPU_APPS_ID.to_owned()).is_none());
}

/// Add Widget in the GPU box lists GPU load by app, then VRAM by app, while the box has none
/// of them; added and applied they save with their cells, draw the box's charts and ask for
/// the per-process GPU rows; removed and applied the demand stops and both are offered again.
#[gpui_kit::test]
fn overview_gpu_by_app_edit_add_remove_apply(cx: &mut TestAppContext) {
    let mut r = open(cx, &[0], 1200., 900.);
    r.publish_groups(&[20., 10., 5.], &[512., 256., 128.]);
    r.page_action(&PerfWidgetEditBox { kind: "gpu".into() });
    let editor = r.editor();
    let offered = |r: &mut Rig| {
        r.vcx.update(|_, cx| {
            editor
                .read(cx)
                .fixed_adds(cx)
                .iter()
                .map(|f| (f.name.to_string(), f.label.to_string()))
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        offered(&mut r),
        [
            ("gpu_by_app".to_owned(), "GPU load by app".to_owned()),
            ("vram_by_app".to_owned(), "VRAM by app".to_owned()),
        ]
    );
    let mut added = Vec::new();
    for name in ["gpu_by_app", "vram_by_app"] {
        added.push(
            r.vcx
                .update(|_, cx| editor.update(cx, |e, cx| e.add_fixed(name, cx)))
                .expect(name),
        );
        draw(r.vcx);
    }
    assert!(offered(&mut r).is_empty(), "each is offered once");
    assert_eq!(r.demand(), (true, true), "the widgets in Edit Widgets ask");
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    let saved = r
        .vcx
        .update(|_, cx| {
            TlSettings::settings(cx)
                .performance
                .layouts
                .known(GPU_BOX_CONTAINER)
                .cloned()
        })
        .expect("an overview.gpu entry");
    assert!(saved.preset.is_none(), "a grid layout");
    for (id, kind) in added
        .iter()
        .zip([FixedWidget::GpuByApp, FixedWidget::VramByApp])
    {
        let w = saved.widgets.iter().find(|w| w.id == *id).expect("saved");
        assert_eq!(w.content, WidgetContent::Fixed(kind));
        assert!(w.cell.is_some(), "it has its cells");
    }
    let (gpu_chart, vram_chart) = r.charts();
    assert_eq!(
        r.content(added[0]),
        gpu_chart.map(|c| c.entity_id()),
        "GPU load by app draws the box's chart"
    );
    assert_eq!(
        r.content(added[1]),
        vram_chart.map(|c| c.entity_id()),
        "VRAM by app draws the box's chart"
    );
    assert!(r.content(added[0]).is_some() && r.content(added[1]).is_some());
    assert_eq!(r.demand(), (true, true), "the saved widgets ask");

    // Remove both and apply.
    r.page_action(&PerfWidgetEditBox { kind: "gpu".into() });
    for id in &added {
        r.vcx.update(|_, cx| {
            editor.update(cx, |e, cx| {
                e.select(*id, cx);
                assert!(e.remove_selected(cx));
            })
        });
    }
    draw(r.vcx);
    assert_eq!(offered(&mut r).len(), 2, "offered again");
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    assert_eq!(r.demand(), (false, false), "no demand once removed");
}

/// The Processes jumps the recording shell got, in call order.
#[derive(Default)]
struct Jumps(Vec<ProcessJump>);

impl Global for Jumps {}

/// Installs a `ShellApi` that records each Processes jump in [`Jumps`] and drops every other
/// call, and marks the rig's window as the main view.
fn record_jumps(r: &mut Rig) {
    let host = r._host.downgrade();
    r.vcx.update(|_, cx| {
        cx.set_global(Jumps::default());
        cx.set_global(ShellApi {
            toast: |_, _, _, _, _| {},
            main_toasts: |_| None,
            select_tab: |_, _, _| {},
            page: |_, _| None,
            filter_text: |_, _| None,
            set_group_chip: |_, _| {},
            note_restricted: |_, _| false,
            public_ip_fetch: |_, _, _| {},
            process_jump: |_, cx, jump| cx.default_global::<Jumps>().0.push(jump),
            disk_eject: |_, _| {},
        });
        cx.default_global::<ShellState>().main_view = Some(host.into());
    });
}

/// A click on a group's legend row in each by-app mode opens the Processes tab filtered to that
/// group; "Other apps" and "Other / driver" open nothing.
#[gpui_kit::test]
fn overview_gpu_by_app_legend_click(cx: &mut TestAppContext) {
    let mut r = open(cx, &[0], 1200., 900.);
    record_jumps(&mut r);
    for (mode, kind, _) in MODES {
        r.vcx.update(|_, cx| cx.global_mut::<Jumps>().0.clear());
        r.set_mode(mode);
        r.publish_groups(&[20., 10., 5.], &[512., 256., 128.]);
        let id = apps_of(&r.flow(), kind).expect("the widget");
        let (gpu_chart, vram_chart) = r.charts();
        let chart = gpu_chart
            .filter(|_| mode == "apps")
            .or(vram_chart)
            .expect("the chart");
        let widget = r.bounds_of(widget_id(GPU_BOX_CONTAINER, id));
        let rows = r.vcx.update(|_, cx| chart.read(cx).bands().len());
        for row in 0..rows {
            let Some(at) = r.try_bounds(format!("{LEGEND_ID}-{row}")) else {
                continue;
            };
            assert!(
                widget.contains(&at.center()),
                "{mode}: {at:?} in {widget:?}"
            );
            r.vcx.simulate_click(at.center(), Modifiers::none());
            draw(r.vcx);
        }
        let jumps: Vec<(String, String)> = r.vcx.update(|_, cx| {
            cx.global::<Jumps>()
                .0
                .iter()
                .map(|j| match j {
                    ProcessJump::MemGroup { key, name } => (key.to_string(), name.to_string()),
                    other => panic!("not a group jump: {other:?}"),
                })
                .collect()
        });
        let mut keys: Vec<&str> = jumps.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["exe:p0", "exe:p1", "exe:p2"], "{mode}: {jumps:?}");
        assert!(
            jumps.contains(&("exe:p2".to_owned(), "p2".to_owned())),
            "{mode}"
        );
    }
}

/// Counts the widget, chart and by-app entities made, and the watched ones dropped.
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
        let (a, b, c, d, e) = (count(), count(), count(), count(), count());
        let subs = r.vcx.update(|_, cx| {
            vec![
                cx.observe_new(move |_: &mut MetricWidget, _, _| a()),
                cx.observe_new(move |_: &mut GroupWidget, _, _| b()),
                cx.observe_new(move |_: &mut FixedWidgetView, _, _| c()),
                cx.observe_new(move |_: &mut ChartView, _, _| d()),
                cx.observe_new(move |_: &mut AppsChart, _, _| e()),
            ]
        });
        Self {
            made,
            dropped: Rc::new(Cell::new(0)),
            subs,
        }
    }

    fn watch<T: 'static>(&mut self, entity: &Entity<T>, cx: &mut App) {
        let dropped = Rc::clone(&self.dropped);
        self.subs.push(cx.observe_release(entity, move |_, _| {
            dropped.set(dropped.get() + 1);
        }));
    }

    fn counts(&self) -> (u32, u32) {
        (self.made.get(), self.dropped.get())
    }
}

/// Render contract: in the `apps` preset repeated samples with the same bands make and drop no
/// entity and never notify the page, the box, its container or the widget; a sample in which a
/// group enters the top N notifies only the chart, once.
#[gpui_kit::test]
fn overview_gpu_by_app_render_contract(cx: &mut TestAppContext) {
    let mut r = open(cx, &[0], 1200., 900.);
    r.set_mode("apps");
    r.publish_groups(&[20., 10., 5.], &[]);
    r.publish_groups(&[20., 10., 5.], &[]);
    let id = apps_of(&r.flow(), FixedWidget::GpuByApp).expect("the widget");
    let chart = r.charts().0.expect("the chart");
    let (page, gpu, container) = (r.page.clone(), r.gpu(), r.container());
    let fixed = r
        .vcx
        .update(|_, cx| container.read(cx).fixed(id).cloned())
        .expect("the widget");
    let bands = |r: &mut Rig| r.vcx.update(|_, cx| chart.read(cx).bands().len());
    let shown = bands(&mut r);
    let notified = Rc::new(Cell::new(0u32));
    let _watch = {
        let notified = Rc::clone(&notified);
        r.vcx
            .update(|_, cx| cx.observe(&chart, move |_, _| notified.set(notified.get() + 1)))
    };
    let counts = |r: &mut Rig| {
        r.vcx.update(|_, cx| {
            (
                page.read(cx).notify_count.get(),
                gpu.read(cx).notify_count.get(),
                container.read(cx).notify_count.get(),
                fixed.read(cx).notify_count.get(),
            )
        })
    };
    let mut life = Lifecycle::new(&mut r);
    let inner = r.vcx.update(|_, cx| chart.read(cx).chart().clone());
    r.vcx.update(|_, cx| {
        life.watch(&chart, cx);
        life.watch(&inner, cx);
        life.watch(&fixed, cx);
    });
    let before = counts(&mut r);
    for _ in 0..3 {
        r.publish_groups(&[20., 10., 5.], &[]);
    }
    assert_eq!(
        counts(&mut r),
        before,
        "the page, box, container and widget never notify"
    );
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    assert_eq!(bands(&mut r), shown, "the same bands");

    // p3 gets GPU time: it enters the top N.
    notified.set(0);
    r.publish_groups(&[20., 10., 5., 4.], &[]);
    assert_eq!(bands(&mut r), shown + 1, "p3 has its own band");
    assert_eq!(notified.get(), 1, "the chart notifies once");
    assert_eq!(
        counts(&mut r),
        before,
        "the page, box, container and widget never notify"
    );
    assert_eq!(life.counts(), (0, 0), "no entity made or dropped");
    let same = r
        .vcx
        .update(|_, cx| container.read(cx).fixed(id).map(|f| f.entity_id()));
    assert_eq!(same, Some(fixed.entity_id()), "the same widget entity");
}

/// The chart is for the first GPU: when the first GPU goes, the box makes the chart again for
/// the GPU that is now first and the widget draws the new chart; with no GPU it draws nothing.
#[gpui_kit::test]
fn overview_gpu_by_app_first_gpu_changes(cx: &mut TestAppContext) {
    let mut r = open(cx, &[0, 1], 1200., 900.);
    r.set_mode("vram_apps");
    r.publish_groups(&[20., 10., 5.], &[512., 256., 128.]);
    let id = apps_of(&r.flow(), FixedWidget::VramByApp).expect("the widget");
    let first = r.charts().1.expect("the chart");
    assert_eq!(
        r.vcx.update(|_, cx| first.read(cx).metric()),
        AppsMetric::Vram(GPU0)
    );
    assert_eq!(r.content(id), Some(first.entity_id()));

    // GPU 0 goes: GPU 1 is first.
    r.set_gpus(&[1]);
    r.publish_groups(&[20., 10., 5.], &[512., 256., 128.]);
    let second = r.charts().1.expect("a new chart");
    assert_ne!(second.entity_id(), first.entity_id(), "made again");
    assert_eq!(
        r.vcx.update(|_, cx| second.read(cx).metric()),
        AppsMetric::Vram(GPU1)
    );
    assert_eq!(
        r.content(id),
        Some(second.entity_id()),
        "the widget draws it"
    );
    assert!(
        r.vcx.update(|_, cx| second.read(cx).bands().len()) >= 4,
        "GPU 1's bands"
    );

    // No GPU: no chart, nothing drawn.
    r.set_gpus(&[]);
    assert!(r.charts().1.is_none(), "no chart without a GPU");
    assert!(
        r.try_bounds(widget_id(GPU_BOX_CONTAINER, id)).is_none(),
        "nothing drawn"
    );
}

/// At 24 h each by-app chart is a box chart over 24 hours, and its ranking and legend values
/// read `GPU_GROUP` and `VRAM_GROUP` from the device metric history, not the live series.
#[gpui_kit::test]
fn overview_gpu_by_app_history_range(cx: &mut TestAppContext) {
    let mut r = open(cx, &[0], 1200., 900.);
    r.publish_groups(&[20., 10., 5.], &[512., 256., 128.]);
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| {
            d.settings.performance.overview_range = OverviewRange::H24;
        });
    });
    draw(r.vcx);
    for (mode, _, metric) in MODES {
        r.set_mode(mode);
        r.publish_groups(&[20., 10., 5.], &[512., 256., 128.]);
        let (gpu_chart, vram_chart) = r.charts();
        let chart = if mode == "apps" {
            gpu_chart
        } else {
            vram_chart
        }
        .expect("the chart");
        let range = r
            .vcx
            .update(|_, cx| chart.read(cx).chart().read(cx).spec.range_ns);
        assert_eq!(range, 24 * 3600 * S, "{mode}");

        // The history has the order the other way round: p2 holds the most, p0 the least.
        let id = metric(GPU0).metric_id();
        let scale = if mode == "apps" { 1. } else { MIB };
        let now = i64::try_from(jiff::Timestamp::now().as_nanosecond()).expect("fits");
        let step = 60 * S;
        let points = |mean: f32| -> Arc<[HistPoint]> {
            (0..30i64)
                .map(|k| HistPoint {
                    t_realtime_ns: now - (30 - k) * step as i64,
                    mean,
                    min: mean,
                    max: mean,
                })
                .collect()
        };
        let sid = |device| SeriesId { device, metric: id };
        let mut series: Vec<(SeriesId, Arc<[HistPoint]>)> = (0..3u16)
            .map(|i| (sid(band_dev(0, i)), points(f32::from(10 + i) * scale)))
            .collect();
        series.push((sid(band_dev(0, GROUPS)), points(3. * scale)));
        series.push((sid(band_dev(0, GROUPS + 1)), points(5. * scale)));
        series.sort_by_key(|(id, _)| *id);
        r.engine.publish_with(|s| {
            let version = s.device_history.header.version + 1;
            s.device_history = Arc::new(DeviceHistory {
                header: ComponentHeader {
                    version,
                    ..ComponentHeader::default()
                },
                request: Some(DeviceHistoryRequest {
                    range: DeviceRange::H24,
                    end_realtime_ns: None,
                }),
                realtime_offset_ns: now - now_boot_ns() as i64,
                since_realtime_ns: Some(now - 30 * step as i64),
                step_ns: step,
                series: series.into(),
                boots: Arc::from([]),
                timeline: None,
            });
        });
        draw(r.vcx);
        let values: Vec<(String, f64)> = r.vcx.read_entity(&chart, |a, _| {
            a.bands()
                .iter()
                .map(|b| (b.key.to_string(), b.value))
                .collect()
        });
        let value = |key: &str| values.iter().find(|v| v.0 == key).map(|v| v.1);
        assert_eq!(
            value("exe:p2"),
            Some(f64::from(12. * scale)),
            "{mode}: {values:?}"
        );
        assert_eq!(
            value("exe:p0"),
            Some(f64::from(10. * scale)),
            "{mode}: {values:?}"
        );
        assert_eq!(
            value("other"),
            Some(f64::from(3. * scale)),
            "{mode}: {values:?}"
        );
    }
}
