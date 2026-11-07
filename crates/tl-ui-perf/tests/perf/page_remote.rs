//! Device pages on a remote host tab (tasklens-jmc36, leader decisions 2026-11-07): a remote GPU
//! page with no saved entry borrows the local page of the same-order GPU and shows it on its own
//! GPU, a saved entry under the remote key wins, Edit Widgets saves to the local page with local
//! keys, and the old `gpu_optional_graphs` conversion does not run on a remote host.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::system::GpuTotals;
use tl_model::validity::Validity;
use tl_ui_base::anim::AnimationDriver;
use tl_ui_base::app_env::{AppEnv, BootSettings, EngineGlobal};
use tl_ui_base::demand::DemandAggregator;
use tl_ui_base::hub::{GlobalHub, SnapshotHub};
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_kit::components::kv_grid;
use tl_ui_overview::widget::edit::saved_layout;
use tl_ui_overview::widget::source::Availability;
use tl_ui_perf::performance::gpu::GpuPage;
use tl_ui_perf::performance::page::DevicePage as _;
use tl_ui_perf::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui_perf_core::core::actions::WidgetSource;
use tl_ui_settings::layout::{
    ContainerLayout, DeviceSelector, FormSet, LAYOUT_VERSION, MetricKey, SourceSelector,
    WidgetCell, WidgetContent, WidgetOptions, WidgetSetting,
};
use tl_ui_settings::layout_pages::page_container;
use tl_ui_settings::settings::{
    AppearanceSettings, GpuOptionalGraph, PerformanceSettings, Settings, ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

use super::gpu_page::{full_totals, totals};

/// Both hosts list two GPUs at these indexes, with other uuids.
const GPU_0: DeviceIdx = DeviceIdx(7);
const GPU_1: DeviceIdx = DeviceIdx(8);
/// The uuid bytes of the local GPUs and of the remote host's.
const LOCAL: [u8; 2] = [0x01, 0x02];
const REMOTE: [u8; 2] = [0x21, 0x22];

fn identity(n: u8) -> DeviceIdentity {
    DeviceIdentity::Gpu {
        uuid: [n; 16],
        pci_bus_id: Arc::from(format!("0000:{n:02x}:00.0")),
    }
}

fn key(n: u8) -> Arc<str> {
    DeviceKey::from_identity(&identity(n)).expect("gpu key").0
}

fn entry(idx: DeviceIdx, n: u8) -> DeviceEntry {
    DeviceEntry {
        idx,
        identity: identity(n),
        generation: 0,
        display_name: Arc::from(format!("GPU {n:02x}")),
        present: true,
        parent: None,
        persistent_key: key(n),
        net_kind: None,
        hot_c: None,
    }
}

fn publish(engine: &FakeEngine, uuids: [u8; 2]) {
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: s.devices.version + 1,
            entries: Arc::from(vec![entry(GPU_0, uuids[0]), entry(GPU_1, uuids[1])]),
        });
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        let b = GpuTotals {
            device: GPU_1,
            ..totals(true)
        };
        sys.gpus = Arc::from(vec![(GPU_0, full_totals()), (GPU_1, b)]);
    });
}

/// A grid layout of the page of GPU `n` with one VRAM widget per key in `keys`, ids from 1.
fn grid(n: u8, keys: &[u8]) -> ContainerLayout {
    let widgets: Vec<WidgetSetting> = keys
        .iter()
        .enumerate()
        .map(|(i, &k)| WidgetSetting {
            id: i as u32 + 1,
            content: WidgetContent::Metric(SourceSelector {
                device: DeviceSelector::Key(key(k)),
                metric: MetricKey::new("vram"),
            }),
            forms: FormSet::VALUE | FormSet::LABEL,
            cell: Some(WidgetCell {
                col: 0,
                row: 2 * i as u16,
                cols: 12,
                rows: 2,
            }),
            options: WidgetOptions::default(),
            device_name: None,
        })
        .collect();
    ContainerLayout {
        container: page_container(&key(n)),
        version: LAYOUT_VERSION,
        preset: None,
        next_id: widgets.len() as u32 + 1,
        widgets,
    }
}

struct Host(Entity<GpuPage>);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.0
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

struct Rig<'a> {
    page: Entity<GpuPage>,
    copies: Arc<AtomicU32>,
    vcx: &'a mut VisualTestContext,
    _driver: Entity<AnimationDriver>,
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

/// A window with the remote host's page of its GPU `slot` (0 or 1), built with `perf` as the
/// loaded settings. The local host is the global hub, with the local GPUs.
fn open(cx: &mut TestAppContext, perf: PerformanceSettings, slot: usize) -> Rig<'_> {
    let local = FakeEngine::new();
    publish(&local, LOCAL);
    let remote = FakeEngine::new();
    publish(&remote, REMOTE);
    let local_client: Arc<dyn EngineClient> = local.clone();
    let remote_client: Arc<dyn EngineClient> = remote.clone();
    let copies = Arc::new(AtomicU32::new(0));
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
        cx.set_global(EngineGlobal(Arc::clone(&local_client)));
        let mut settings = Settings {
            performance: perf,
            ..Settings::default()
        };
        settings.motion.smooth_graphs = false;
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        let count = Arc::clone(&copies);
        cx.set_global(AppEnv {
            keep_pre_layouts_copy: Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }),
            ..AppEnv::test()
        });
    });
    let local_hub = cx.new(|cx| SnapshotHub::new(local_client, cx));
    cx.update(|cx| cx.set_global(GlobalHub(local_hub)));
    let hub = cx.new(|cx| SnapshotHub::new_remote(Arc::clone(&remote_client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(remote_client, &hub, cx));
    let (idx, n) = ([GPU_0, GPU_1][slot], REMOTE[slot]);
    let row = DeviceRow {
        key: DeviceKey(key(n)),
        kind: DeviceKind::Gpu,
        idx,
        generation: 0,
        net_class: None,
        fan_connected: None,
    };
    let (host, vcx) = cx.add_window_view(move |_, cx| {
        let page = cx.new(|cx| GpuPage::new(DeviceKey(key(n)), row, hub, demand, cx));
        Host(page)
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1200.), px(900.)));
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    let page = vcx.read_entity(&host, |h, _| h.0.clone());
    vcx.update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    draw(vcx);
    Rig {
        page,
        copies,
        vcx,
        _driver: driver,
    }
}

/// A widget's saved key, availability and device.
type Shown = (Arc<str>, Availability, Option<DeviceIdx>);

impl Rig<'_> {
    /// The container's layout and, per metric widget, its saved key, availability and device.
    fn shown(&mut self) -> (ContainerLayout, Vec<Shown>) {
        let page = self.page.clone();
        self.vcx.update(|_, cx| {
            let c = page.read(cx).container().read(cx);
            let layout = c.layout().clone();
            let widgets = layout
                .widgets
                .iter()
                .filter_map(|w| {
                    let Some(DeviceSelector::Key(k)) = w.content.device() else {
                        return None;
                    };
                    let s = c.source(w.id)?.read(cx);
                    Some((Arc::clone(k), s.availability(), s.device()))
                })
                .collect();
            (layout, widgets)
        })
    }

    fn saved(&mut self, n: u8) -> Option<ContainerLayout> {
        self.vcx
            .update(|_, cx| saved_layout(&page_container(&key(n)), cx))
    }
}

fn perf_with(layouts: &[ContainerLayout]) -> PerformanceSettings {
    let mut perf = PerformanceSettings::default();
    for l in layouts {
        perf.layouts.set(l.clone());
    }
    perf
}

/// Remote GPU 1's page has no entry: it shows local GPU 1's page layout. The local GPU 1 key is
/// pinned to remote GPU 1, and the local GPU 0 key maps to remote GPU 0 by order.
#[gpui_kit::test]
fn remote_gpu_page_borrows_local_layout(cx: &mut TestAppContext) {
    let layout = grid(LOCAL[1], &[LOCAL[1], LOCAL[0]]);
    let mut r = open(cx, perf_with(std::slice::from_ref(&layout)), 1);
    let (shown, widgets) = r.shown();
    assert_eq!(shown, layout, "the local page's layout, unchanged");
    assert_eq!(
        widgets,
        vec![
            (key(LOCAL[1]), Availability::Available, Some(GPU_1)),
            (key(LOCAL[0]), Availability::Available, Some(GPU_0)),
        ]
    );
    assert_eq!(
        r.saved(REMOTE[1]),
        None,
        "nothing is written for the remote key"
    );
    assert_eq!(r.saved(LOCAL[1]), Some(layout), "the local entry stays");
}

/// A saved entry under the remote key wins over the borrowed one.
#[gpui_kit::test]
fn remote_page_saved_entry_wins(cx: &mut TestAppContext) {
    let local = grid(LOCAL[1], &[LOCAL[1]]);
    let own = grid(REMOTE[1], &[REMOTE[1], REMOTE[0]]);
    let mut r = open(cx, perf_with(&[local, own.clone()]), 1);
    let (shown, widgets) = r.shown();
    assert_eq!(shown, own);
    assert_eq!(
        widgets,
        vec![
            (key(REMOTE[1]), Availability::Available, Some(GPU_1)),
            (key(REMOTE[0]), Availability::Available, Some(GPU_0)),
        ]
    );
}

/// Edit Widgets on a borrowed remote page saves to the local page, and a widget added on the
/// page's own GPU is written with the local key the page borrowed.
#[gpui_kit::test]
fn remote_page_edit_saves_local_container(cx: &mut TestAppContext) {
    let layout = grid(LOCAL[1], &[LOCAL[1]]);
    let mut r = open(cx, perf_with(&[layout]), 1);
    let editor = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| page.read(cx).editor().clone())
    };
    r.vcx
        .update(|window, cx| editor.update(cx, |e, cx| e.start(window, cx)));
    draw(r.vcx);
    let source = WidgetSource {
        device: key(REMOTE[1]),
        metric: Arc::from("temp"),
    };
    let added = r
        .vcx
        .update(|_, cx| editor.update(cx, |e, cx| e.add(&source, cx)))
        .expect("added");
    assert!(r.vcx.update(|_, cx| editor.update(cx, |e, cx| e.apply(cx))));
    draw(r.vcx);
    assert_eq!(r.saved(REMOTE[1]), None, "no entry under the remote key");
    let saved = r.saved(LOCAL[1]).expect("the local page's entry");
    let new = saved
        .widgets
        .iter()
        .find(|w| w.id == added)
        .expect("the added widget");
    assert_eq!(
        new.content.device(),
        Some(&DeviceSelector::Key(key(LOCAL[1])))
    );
    let (_, widgets) = r.shown();
    assert!(
        widgets
            .iter()
            .all(|(_, a, d)| *a != Availability::Gone && *d == Some(GPU_1)),
        "every widget shows the page's own GPU: {widgets:?}"
    );
}

/// The old `gpu_optional_graphs` key is not converted when a remote GPU page opens: no entry
/// under the remote key and no backup copy.
#[gpui_kit::test]
fn remote_gpu_page_skips_conversion(cx: &mut TestAppContext) {
    let perf = PerformanceSettings {
        gpu_optional_graphs: vec![GpuOptionalGraph::Power],
        ..PerformanceSettings::default()
    };
    let mut r = open(cx, perf, 0);
    assert_eq!(r.saved(REMOTE[0]), None);
    assert_eq!(r.copies.load(Ordering::SeqCst), 0);
}
