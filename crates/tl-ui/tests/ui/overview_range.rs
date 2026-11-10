//! The Overview time range in the main window (UI-PRF-055, D-030): the rig that opens the main
//! window on the Overview, used by overview_timeline and device_pages, and the box chrome check,
//! whose sizes depend on the page area the main window gives. The other range tests moved to the
//! tl-ui-perf `perf` binary on the page host (tasklens-zljr5.41).

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, IntoElement, Modifiers, MouseButton,
    MouseDownEvent, MouseUpEvent, Render, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::DeviceIdx;
use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
use tl_ui::window::main::{
    self as shell, MainWindow, PageSlot, main_window_view, open_main_window, set_test_page,
};
use tl_ui::window::titlebar::TitleBarProbe;
use tl_ui_base::demand::{DemandAggregator, GlobalDemand};
use tl_ui_base::hub::{GlobalHub, SnapshotHub};
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_chart::chart::ChartView;
use tl_ui_kit::components::kv_grid;
use tl_ui_perf::performance::overview::range::{segment_id, trigger_id};
use tl_ui_perf::performance::overview::{
    BOX_ID_PREFIX, OVERVIEW_KEY, OverviewPage, RANGE_HEADER_ID,
};
use tl_ui_perf::performance::status_bar::BAR_ID;
use tl_ui_perf::performance::{DeviceKey, PerformancePage};
use tl_ui_settings::settings::{
    AppearanceSettings, OverviewRange, Settings, ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

fn entry(idx: u16, identity: DeviceIdentity, net_kind: Option<NetKind>) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("dev"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind,
        hot_c: None,
    }
}

/// CPU, memory, a GPU, a disk and a wired interface.
fn catalog() -> Vec<DeviceEntry> {
    vec![
        entry(0, DeviceIdentity::CpuPackage, None),
        entry(1, DeviceIdentity::Memory, None),
        entry(
            2,
            DeviceIdentity::Gpu {
                uuid: [0; 16],
                pci_bus_id: Arc::from("0000:01:00.0"),
            },
            None,
        ),
        entry(
            3,
            DeviceIdentity::Disk {
                major: 8,
                minor: 0,
                name: Arc::from("sda"),
                wwn_or_serial: None,
            },
            None,
        ),
        entry(
            5,
            DeviceIdentity::Net {
                netns_ino: 1,
                ifindex: 2,
                name: Arc::from("eth0"),
            },
            Some(NetKind::Wired),
        ),
    ]
}

struct FakeProcesses {
    focus: FocusHandle,
}

impl gpui_kit::EventEmitter<tl_ui::window::status::StatusChanged> for FakeProcesses {}

impl Render for FakeProcesses {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::{InteractiveElement as _, Styled as _};
        div().track_focus(&self.focus).size_full()
    }
}

impl shell::ShellPage for FakeProcesses {
    fn primary_focus(&self) -> FocusHandle {
        self.focus.clone()
    }
}

pub(crate) struct Rig<'a> {
    pub(crate) page: Entity<PerformancePage>,
    pub(crate) demand: Entity<DemandAggregator>,
    pub(crate) engine: Arc<FakeEngine>,
    pub(crate) vcx: &'a mut VisualTestContext,
}

pub(crate) fn draw(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

/// The main window, 1600 × 1000, on the Performance tab with the Overview selected and every
/// card shown. Animations are off.
pub(crate) fn open(cx: &mut TestAppContext) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(catalog()),
        });
    });
    let fake = Arc::clone(&engine);
    let client: Arc<dyn EngineClient> = engine;
    let demand = cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_reduce_motion(true);
        let theme_settings = ThemeSettings::default();
        tl_ui_base::theme::store::init(
            cx,
            &theme_settings,
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui_base::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        cx.set_global(BootSettings {
            settings: Settings {
                theme: theme_settings,
                ..Settings::default()
            },
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        cx.set_global(GlobalHub(hub));
        cx.set_global(GlobalDemand(demand.clone()));
        tl_ui_kit::components::toast::init(cx);
        tl_ui_kit::actions::bind_keys_global(cx);
        tl_ui_perf::performance::actions::bind_keys_performance(cx);
        cx.set_global(StartOptions::default());
        set_test_page(cx, Tab::Processes, |_, cx: &mut Context<MainWindow>| {
            let page = cx.new(|cx| FakeProcesses {
                focus: cx.focus_handle().tab_stop(true),
            });
            PageSlot::new(page, cx)
        });
        cx.set_global(TitleBarProbe {
            client: Some(true),
            ..TitleBarProbe::default()
        });
        tl_ui::window::menubar::init(cx);
        shell::init(cx);
        demand
    });
    let handle = cx.update(open_main_window).expect("the main window opens");
    let vcx = VisualTestContext::from_window(handle.into(), cx).into_mut();
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1600.), px(1000.)));
    vcx.update(|window, _| window.activate_window());
    draw(vcx);
    let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
    vcx.dispatch_action(tl_ui_kit::actions::SelectTab(Tab::Performance as usize));
    draw(vcx);
    let page = vcx
        .read_entity(&main, |m, _| {
            m.page_entity::<PerformancePage>(Tab::Performance)
        })
        .expect("PerformancePage built");
    let r = Rig {
        page,
        demand,
        engine: fake,
        vcx,
    };
    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().clone();
        sidebar.update(cx, |s, cx| s.select(DeviceKey(Arc::from(OVERVIEW_KEY)), cx));
    });
    draw(r.vcx);
    r
}

pub(crate) fn overview(r: &mut Rig) -> Entity<OverviewPage> {
    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        page.read(cx)
            .page(&DeviceKey(Arc::from(OVERVIEW_KEY)))
            .and_then(|v| v.clone().downcast::<OverviewPage>().ok())
            .expect("an OverviewPage")
    })
}

/// One chart of each device box.
pub(crate) fn box_charts(r: &mut Rig) -> Vec<Entity<ChartView>> {
    let page = overview(r);
    r.vcx.update(|_, cx| {
        let page = page.read(cx);
        vec![
            page.cpu_box().read(cx).total_chart().clone(),
            page.mem_box().read(cx).sparks()[0].clone(),
            page.gpu_box().read(cx).sections()[0].chart.clone(),
            page.disk_box().read(cx).rows()[0].read_chart.clone(),
            page.net_box().read(cx).down_chart().clone(),
        ]
    })
}

pub(crate) fn click(cx: &mut VisualTestContext, id: &'static str) {
    let at = cx.debug_bounds(id).expect(id).center();
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
    draw(cx);
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Picks `range` from the drop-down `prefix`: opens the menu unless it is open, then clicks the
/// item. A disabled item takes the click and the menu stays open.
pub(crate) fn pick(cx: &mut VisualTestContext, prefix: &str, range: OverviewRange) {
    let item = leak(segment_id(prefix, range));
    if cx.debug_bounds(item).is_none() {
        click(cx, leak(trigger_id(prefix)));
    }
    click(cx, item);
}

pub(crate) fn set_range(r: &mut Rig, range: OverviewRange) {
    r.vcx.update(|_, cx| {
        TlSettings::update(cx, |d| d.settings.performance.overview_range = range);
    });
    draw(r.vcx);
}

/// A GPU that reports the six engines of an NVIDIA card (3D / Compute, memory controller, video
/// encode and decode, JPEG and optical flow), with 4 of 16 GiB VRAM used.
fn gpu_totals(device: DeviceIdx) -> tl_model::system::GpuTotals {
    use tl_model::system::{GpuEngine, GpuEngines, Metric};
    const GIB: u64 = 1 << 30;
    let mut engines = GpuEngines::none();
    for (engine, pct) in [
        (GpuEngine::Gpu, 25.),
        (GpuEngine::MemCtl, 8.),
        (GpuEngine::Enc, 10.),
        (GpuEngine::Dec, 5.),
        (GpuEngine::Jpeg, 0.),
        (GpuEngine::Ofa, 0.),
    ] {
        engines.set(engine, Metric::valid(pct));
    }
    tl_model::system::GpuTotals {
        groups: Default::default(),
        vram_groups: Default::default(),
        device,
        util_pct: Metric::valid(25.),
        mem_util_pct: Metric::default(),
        mem_used: Metric::valid(4 * GIB),
        mem_total: Metric::valid(16 * GIB),
        shared_used: Metric::default(),
        shared_total: Metric::default(),
        mem_reserved: Metric::default(),
        enc_pct: Metric::valid(10.),
        dec_pct: Metric::valid(5.),
        enc_sessions: Metric::default(),
        enc_dec_shared: false,
        temp_c: Metric::valid(62.),
        temp_edge_c: Metric::default(),
        temp_junction_c: Metric::default(),
        temp_mem_c: Metric::default(),
        power_w: Metric::valid(120.),
        power_limit_w: Metric::default(),
        clock_gfx_mhz: Metric::default(),
        clock_mem_mhz: Metric::default(),
        fan_pct: Metric::default(),
        pcie_rx_bps: Metric::default(),
        pcie_tx_bps: Metric::default(),
        engines,
        throttling: Default::default(),
        fdinfo_partial: false,
        energy_since_boot: Metric::default(),
        clock_video_mhz: Metric::default(),
        pstate: Metric::default(),
        clock_event_reasons: Metric::default(),
        fan_rpm: Metric::default(),
        pcie_gen: Metric::default(),
        pcie_width: Metric::default(),
        info: Arc::default(),
    }
}

/// The height `plots` cover from top to bottom, with plots side by side counted once, and the
/// number of rows they sit in.
fn plot_cover(plots: &[gpui_kit::Bounds<gpui_kit::Pixels>]) -> (f32, usize) {
    let mut spans: Vec<(f32, f32)> = plots
        .iter()
        .map(|p| (f32::from(p.top()), f32::from(p.bottom())))
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut covered, mut rows) = (0., 0);
    let mut open: Option<(f32, f32)> = None;
    for (top, bottom) in spans {
        match open {
            Some((t, b)) if top < b => open = Some((t, b.max(bottom))),
            _ => {
                if let Some((t, b)) = open {
                    covered += b - t;
                }
                rows += 1;
                open = Some((top, bottom));
            }
        }
    }
    if let Some((t, b)) = open {
        covered += b - t;
    }
    (covered, rows)
}

/// tasklens-o55.54, leader rule of 2026-10-30: with the default layout and
/// `overview_row_unit_px` 40 at 1920 × 1200, each box's chrome is at most 40 px. Chrome is the
/// box height less the height its plots cover as painted (each with its 1 px outline), less
/// 16 px for each allowed label line: one per row of plots (a disk's name and rate, a GPU engine's name and %, the Network
/// Download and Upload lines) and one for the GPU VRAM row. A sparkline under 32 px tall sits
/// in a label row (VRAM, a core row) and is part of that row. The CPU chart has no label line.
/// Memory and the timeline have no plots and are left out. The first box starts at most 8 px
/// under the status bar.
#[gpui_kit::test]
fn overview_box_chrome_at_most_40(cx: &mut TestAppContext) {
    use gpui_kit::{Bounds, EntityId, Pixels};
    use tl_ui_chart::chart::element::take_plots;
    use tl_ui_perf::performance::overview::gpu_box::vram_id;
    use tl_ui_perf::performance::overview::layout::BoxKind;

    let mut r = open(cx);
    // Three disks and a GPU with six engines in two rows, as on the owner's machine. Catalog entries sit
    // at their index.
    r.engine.publish_with(|s| {
        let disk = |idx: u16, minor: u32, name: &str| {
            entry(
                idx,
                DeviceIdentity::Disk {
                    major: 8,
                    minor,
                    name: Arc::from(name),
                    wwn_or_serial: None,
                },
                None,
            )
        };
        let mut entries = catalog();
        entries.retain(|e| !matches!(e.identity, DeviceIdentity::Net { .. }));
        entries.push(disk(4, 16, "sdb"));
        entries.extend(catalog().into_iter().filter(|e| e.idx == DeviceIdx(5)));
        entries.push(disk(6, 32, "sdc"));
        s.devices = Arc::new(DeviceCatalog {
            version: 2,
            entries: Arc::from(entries),
        });
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.gpus = Arc::from([(DeviceIdx(2), gpu_totals(DeviceIdx(2)))]);
        // An NVIDIA adapter, so the Overview shows all six engines.
        let inv = Arc::make_mut(&mut s.inventory);
        inv.header.version += 1;
        inv.graphics = Arc::from(
            [tl_model::snapshot::GraphicsAdapter {
                device: Some(DeviceIdx(2)),
                kernel_module: tl_model::snapshot::InvField {
                    value: Arc::from("nvidia"),
                    validity: tl_model::validity::Validity::Valid,
                },
                ..Default::default()
            }]
            .as_slice(),
        );
    });
    let unit = r
        .vcx
        .update(|_, cx| TlSettings::settings(cx).performance.overview_row_unit_px);
    assert_eq!(unit, 40);
    let page = overview(&mut r);
    // A 1 px wider window first, so every chart paints again at its place in the frames read.
    r.vcx.simulate_resize(size(px(1921.), px(1200.)));
    draw(r.vcx);
    let _ = take_plots();
    r.vcx.simulate_resize(size(px(1920.), px(1200.)));
    draw(r.vcx);
    let mut last: Vec<(EntityId, Bounds<Pixels>)> = Vec::new();
    for (id, plot) in take_plots() {
        match last.iter_mut().find(|e| e.0 == id) {
            Some(e) => e.1 = plot,
            None => last.push((id, plot)),
        }
    }
    let bar = r
        .vcx
        .debug_bounds(BAR_ID)
        .expect("status bar on the Overview");
    assert!(
        r.vcx.debug_bounds(RANGE_HEADER_ID).is_none(),
        "no title row"
    );
    let kinds: Vec<_> = r.vcx.update(|_, cx| {
        page.read(cx)
            .placement()
            .boxes
            .iter()
            .map(|b| b.kind)
            .collect()
    });
    assert_eq!(kinds.len(), 6, "five device boxes and the timeline");
    let vram_sel: &'static str = Box::leak(vram_id(0).into_boxed_str());
    let mut first = px(f32::MAX);
    let mut report = String::new();
    let mut over = false;
    for kind in kinds {
        let frame_id: &'static str =
            Box::leak(format!("{BOX_ID_PREFIX}{}", kind.key()).into_boxed_str());
        let frame = r.vcx.debug_bounds(frame_id).expect(frame_id);
        first = first.min(frame.top());
        if matches!(kind, BoxKind::Memory | BoxKind::Timeline) {
            continue;
        }
        let plots: Vec<_> = last
            .iter()
            .map(|e| e.1)
            .filter(|p| {
                p.size.height >= px(32.)
                    && p.top() >= frame.top()
                    && p.bottom() <= frame.bottom()
                    && p.left() >= frame.left()
                    && p.right() <= frame.right()
            })
            .collect();
        assert!(!plots.is_empty(), "{kind:?}: plots painted in {frame:?}");
        let (covered, rows) = plot_cover(&plots);
        if kind == BoxKind::Gpu {
            assert_eq!(rows, 2, "Gpu: six engines in two rows: {plots:?}");
        }
        let lines = match kind {
            BoxKind::Cpu => 0,
            BoxKind::Gpu => rows + usize::from(r.vcx.debug_bounds(vram_sel).is_some()),
            _ => rows,
        };
        let h = f32::from(frame.size.height);
        let chrome = h - covered - 16. * lines as f32;
        over |= chrome > 40.;
        report.push_str(&format!(
            "{kind:?}: box {h} px, {} plots cover {covered} px in {rows} rows, \
             {lines} label lines: chrome {chrome} px\n",
            plots.len()
        ));
    }
    assert!(!over, "chrome over 40 px:\n{report}");
    let gap = first - bar.bottom();
    assert!(
        gap >= px(0.) && gap <= px(8.),
        "first box {gap:?} below the status bar"
    );
}
