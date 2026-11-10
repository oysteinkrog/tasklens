//! tasklens-g00wx.2: the GPU load and VRAM by app charts, `AppsChart` of kind `AppsMetric::Gpu`
//! and `AppsMetric::Vram`, and the remainder-band rule over every kind: the drawn bands plus the
//! remainder band sum to the device total, and no remainder band is drawn when the app bands
//! already reach or pass the total (leader decision, tasklens-g00wx.2).

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{ComponentHeader, SeriesSet};
use tl_model::system::{GpuTotals, MemGroupBand, Metric};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui_base::app_env::{BootSettings, EngineGlobal};
use tl_ui_base::hub::SnapshotHub;
use tl_ui_base::i18n::init_strings;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_overview::overview::memory_apps::{AppsChart, AppsMetric, AppsPlace, BandKind};
use tl_ui_settings::settings::{
    AppearanceSettings, OverviewRange, Settings, ThemeSettings, UiState,
};
use tl_ui_testkit::fake_engine::FakeEngine;

const S: u64 = 1_000_000_000;
const STEPS: u64 = 60;
const GPU: DeviceIdx = DeviceIdx(70);
const OTHER: DeviceIdx = DeviceIdx(80);
const REMAINDER: DeviceIdx = DeviceIdx(81);
const SHARED: DeviceIdx = DeviceIdx(82);
const NOISE: DeviceIdx = DeviceIdx(83);
const PCI: &str = "0000:01:00.0";
const OTHER_PCI: &str = "0000:02:00.0";

/// One kind under test.
struct Kind {
    name: &'static str,
    metric: AppsMetric,
    series: MetricId,
    /// The key of the remainder band as the chart shows it.
    remainder: &'static str,
    total: u64,
    gpu: bool,
}

fn kinds() -> [Kind; 4] {
    [
        Kind {
            name: "memory",
            metric: AppsMetric::Memory,
            series: MetricId::MEM_GROUP,
            remainder: "k:filecache",
            total: 1 << 34,
            gpu: false,
        },
        Kind {
            name: "cpu",
            metric: AppsMetric::Cpu,
            series: MetricId::CPU_GROUP,
            remainder: "cpu:short",
            total: 100,
            gpu: false,
        },
        Kind {
            name: "gpu",
            metric: AppsMetric::Gpu(GPU),
            series: MetricId::GPU_GROUP,
            remainder: "driver",
            total: 100,
            gpu: true,
        },
        Kind {
            name: "vram",
            metric: AppsMetric::Vram(GPU),
            series: MetricId::VRAM_GROUP,
            remainder: "driver",
            total: 1 << 33,
            gpu: true,
        },
    ]
}

fn entry(idx: DeviceIdx, identity: DeviceIdentity, name: &str) -> DeviceEntry {
    DeviceEntry {
        idx,
        identity,
        generation: 0,
        display_name: Arc::from(name),
        present: true,
        parent: None,
        persistent_key: Arc::from(format!("test:{}", idx.0)),
        net_kind: None,
        hot_c: None,
    }
}

fn group_entry(idx: DeviceIdx, key: &str, name: &str) -> DeviceEntry {
    entry(
        idx,
        DeviceIdentity::MemGroup {
            key: Arc::from(key),
        },
        name,
    )
}

/// The values of one flat sweep script, as fractions of the kind's total.
struct Fixture {
    base: u64,
    groups: Vec<f64>,
    other: f64,
    remainder: f64,
}

impl Fixture {
    fn new(groups: &[f64], other: f64, remainder: f64) -> Self {
        Self {
            base: now_boot_ns() - (STEPS - 1) * S,
            groups: groups.to_vec(),
            other,
            remainder,
        }
    }

    fn t(&self, k: u64) -> u64 {
        self.base + k * S
    }

    fn view(&self, kind: &Kind, device: DeviceIdx, fraction: f64) -> Arc<SeriesView> {
        let value = (fraction * kind.total as f64) as f32;
        Arc::new(SeriesView {
            id: SeriesId {
                device,
                metric: kind.series,
            },
            version: 1,
            period_ns: S,
            samples: (0..STEPS)
                .map(|k| Sample::valid(self.t(k), value))
                .collect::<Vec<_>>()
                .into(),
        })
    }

    /// Every sweep of the kind, with a second GPU's group that no kind but its own GPU's may
    /// draw.
    fn publish(&self, kind: &Kind, engine: &FakeEngine) {
        engine.publish_with(|s| {
            // The engine's device key of a band: `gpu:<pci>:<key>` for the GPU kinds.
            let dev_key = |key: &str| {
                if kind.gpu {
                    format!("gpu:{PCI}:{key}")
                } else {
                    key.to_owned()
                }
            };
            let mut entries = Vec::new();
            let mut views = Vec::new();
            let mut published = Vec::new();
            if kind.gpu {
                entries.push(entry(
                    GPU,
                    DeviceIdentity::Gpu {
                        uuid: [0; 16],
                        pci_bus_id: Arc::from(PCI),
                    },
                    "GPU 0",
                ));
            }
            for (i, v) in self.groups.iter().enumerate() {
                let idx = DeviceIdx(10 + i as u16);
                let key = format!("exe:p{i}");
                entries.push(group_entry(idx, &dev_key(&key), &format!("p{i}")));
                views.push(self.view(kind, idx, *v));
                published.push(MemGroupBand {
                    key: Arc::from(key.as_str()),
                    name: Arc::from(format!("p{i}")),
                    process_count: 3,
                    device: idx,
                    wine: None,
                });
            }
            entries.push(group_entry(OTHER, &dev_key("other"), "Other apps"));
            views.push(self.view(kind, OTHER, self.other));
            entries.push(group_entry(
                REMAINDER,
                &dev_key(kind.remainder),
                "Other / driver",
            ));
            views.push(self.view(kind, REMAINDER, self.remainder));
            if kind.metric == AppsMetric::Memory {
                // The aggregate "Shared and kernel" and the clock band: the sum of the parts.
                entries.push(group_entry(SHARED, "shared", "Shared and kernel"));
                views.push(self.view(kind, SHARED, self.remainder));
            }
            // A group of another GPU: never drawn by any of the four charts.
            entries.push(group_entry(NOISE, &format!("gpu:{OTHER_PCI}:exe:zz"), "zz"));
            views.push(self.view(kind, NOISE, 0.5));
            views.sort_by_key(|v| v.id);
            s.devices = Arc::new(DeviceCatalog {
                version: s.devices.version + 1,
                entries: entries.into(),
            });
            s.series = Arc::new(SeriesSet {
                header: ComponentHeader {
                    version: s.series.header.version + 1,
                    ..ComponentHeader::default()
                },
                series: views.into(),
                slow: Arc::from([]),
            });
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            match kind.metric {
                AppsMetric::Memory => {
                    sys.memory.total = kind.total;
                    sys.memory.groups = Arc::new(published);
                }
                AppsMetric::Cpu => sys.cpu.groups = Arc::new(published),
                AppsMetric::Gpu(_) | AppsMetric::Vram(_) => {
                    let mut g = gpu_totals(kind.total);
                    if matches!(kind.metric, AppsMetric::Gpu(_)) {
                        g.groups = Arc::new(published);
                    } else {
                        g.vram_groups = Arc::new(published);
                    }
                    sys.gpus = Arc::from(vec![(GPU, g)]);
                }
            }
        });
    }
}

fn gpu_totals(mem_total: u64) -> GpuTotals {
    let off = || Metric::with(Validity::Unsupported);
    GpuTotals {
        groups: Default::default(),
        vram_groups: Default::default(),
        device: GPU,
        util_pct: Metric::valid(10.0),
        mem_util_pct: Metric::valid(0.0),
        mem_used: Metric::valid(0),
        mem_total: Metric::valid(mem_total),
        shared_used: off(),
        shared_total: off(),
        enc_pct: off(),
        dec_pct: off(),
        mem_reserved: off(),
        enc_sessions: off(),
        enc_dec_shared: false,
        temp_c: off(),
        temp_edge_c: off(),
        temp_junction_c: off(),
        temp_mem_c: off(),
        power_w: off(),
        power_limit_w: off(),
        clock_gfx_mhz: off(),
        clock_mem_mhz: off(),
        fan_pct: off(),
        pcie_rx_bps: off(),
        pcie_tx_bps: off(),
        engines: Default::default(),
        throttling: Default::default(),
        fdinfo_partial: false,
        energy_since_boot: off(),
        clock_video_mhz: off(),
        pstate: off(),
        clock_event_reasons: off(),
        fan_rpm: off(),
        pcie_gen: off(),
        pcie_width: off(),
        info: Arc::default(),
    }
}

struct Host {
    apps: Entity<AppsChart>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(div().w(px(700.)).h(px(400.)).child(self.apps.clone()))
    }
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

struct Rig<'a> {
    apps: Entity<AppsChart>,
    vcx: &'a mut VisualTestContext,
}

impl Rig<'_> {
    fn bands(&mut self) -> Vec<(String, BandKind, String, f64)> {
        self.vcx.read_entity(&self.apps, |a, _| {
            a.bands()
                .iter()
                .map(|b| (b.key.to_string(), b.kind, b.name.to_string(), b.value))
                .collect()
        })
    }

    fn kinds(&mut self) -> Vec<BandKind> {
        self.bands().into_iter().map(|b| b.1).collect()
    }

    fn settings(&mut self) -> Settings {
        self.vcx.update(|_, cx| TlSettings::settings(cx).clone())
    }
}

/// A by-app chart of `kind` in a 700 × 400 Overview body at the 1 min range over `fixture`, with
/// `settings` changed by `edit`.
fn open<'a>(
    cx: &'a mut TestAppContext,
    kind: &Kind,
    fixture: &Fixture,
    edit: impl FnOnce(&mut Settings),
) -> Rig<'a> {
    let engine = FakeEngine::new();
    fixture.publish(kind, &engine);
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
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        let mut settings = Settings::default();
        settings.performance.overview_range = OverviewRange::M1;
        edit(&mut settings);
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let (host, vcx) = cx.add_window_view(move |_, cx| {
        let apps = cx.new(|cx| AppsChart::new_in(hub, kind.metric, AppsPlace::Overview, cx));
        Host { apps }
    });
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1000.), px(700.)));
    draw(vcx);
    let apps = vcx.update(|_, cx| host.read(cx).apps.clone());
    Rig { apps, vcx }
}

fn top_n_3(s: &mut Settings) {
    s.performance.memory_by_app.top_n = 3;
    s.performance.cpu_by_app.top_n = 3;
    s.performance.gpu_by_app.top_n = 3;
    s.performance.vram_by_app.top_n = 3;
}

/// The stack top at every step against `want` (a fraction of the total), within 1e-6 of it.
fn assert_top(r: &mut Rig<'_>, kind: &Kind, fixture: &Fixture, want: f64, what: &str) {
    for k in 0..STEPS {
        let t = fixture.t(k);
        let top = r.vcx.read_entity(&r.apps, |a, _| a.stack_top(t));
        let want = want * kind.total as f64;
        assert!(
            (top - want).abs() <= 1e-6 * kind.total as f64,
            "{} {what} step {k}: stack {top}, want {want}",
            kind.name
        );
    }
}

/// For every kind: the drawn bands plus the remainder band sum to the device total, and no
/// remainder band is drawn when the app bands already reach or pass the total. The remainder
/// band is `k:filecache`, `cpu:short`, `driver`, `driver`. A group of another GPU is never drawn.
#[gpui_kit::test]
fn remainder_band_rule_over_every_kind(cx: &mut TestAppContext) {
    // Apps and "Other apps" take 0.625 of the total, the remainder the other 0.375.
    let cases: [(&str, Fixture, bool, f64); 3] = [
        (
            "under",
            Fixture::new(&[0.25, 0.125, 0.0625, 0.0625, 0.03125], 0.09375, 0.375),
            true,
            1.0,
        ),
        (
            "exact",
            Fixture::new(&[0.25, 0.25, 0.125, 0.0625, 0.0625], 0.25, 0.0),
            false,
            1.0,
        ),
        (
            "over",
            Fixture::new(&[0.5, 0.25, 0.125, 0.0625, 0.0625], 0.25, 0.0),
            false,
            1.25,
        ),
    ];
    for kind in kinds() {
        for (what, fixture, drawn, top) in &cases {
            let mut r = open(cx, &kind, fixture, top_n_3);
            let bands = r.bands();
            let keys: Vec<&str> = bands.iter().map(|b| b.0.as_str()).collect();
            assert_eq!(
                keys.contains(&kind.remainder),
                *drawn,
                "{} {what}: {keys:?}",
                kind.name
            );
            assert!(
                !keys.iter().any(|k| k.contains("zz")),
                "{} {what}: {keys:?}",
                kind.name
            );
            assert_top(&mut r, &kind, fixture, *top, what);
            // The legend values add up to the same.
            let sum: f64 = r.bands().iter().map(|b| b.3).sum();
            let want = top * kind.total as f64;
            assert!(
                (sum - want).abs() <= 1e-6 * kind.total as f64,
                "{} {what}: legend {sum}, want {want}",
                kind.name
            );
            // A theme-free rebuild trigger: more groups shown folds fewer into "Other apps".
            r.vcx.update(|_, cx| kind.metric.set_top_n(5, cx));
            draw(r.vcx);
            assert_top(&mut r, &kind, fixture, *top, what);
            let again: Vec<String> = r.bands().into_iter().map(|b| b.0).collect();
            assert_eq!(
                again.iter().any(|k| k == kind.remainder),
                *drawn,
                "{} {what} after top N 5",
                kind.name
            );
        }
    }
}

/// The GPU kinds keep only their own GPU's bands, with the plain group key, name the remainder
/// band from the catalog, and save their colour picks in their own section.
#[gpui_kit::test]
fn gpu_kinds_read_their_own_bands_and_save_picks(cx: &mut TestAppContext) {
    for kind in kinds().into_iter().filter(|k| k.gpu) {
        let fixture = Fixture::new(&[0.25, 0.125, 0.0625], 0.125, 0.25);
        let mut r = open(cx, &kind, &fixture, top_n_3);
        let bands = r.bands();
        let keys: Vec<&str> = bands.iter().map(|b| b.0.as_str()).collect();
        assert_eq!(
            keys,
            ["exe:p0", "exe:p1", "exe:p2", "other", "driver"],
            "{}",
            kind.name
        );
        assert_eq!(bands[4].1, BandKind::Kernel(0), "{}", kind.name);
        assert_eq!(bands[4].2, "Other / driver", "{}", kind.name);
        let s = r.settings().performance;
        let (own, others) = if matches!(kind.metric, AppsMetric::Gpu(_)) {
            (&s.gpu_by_app.colors, &s.vram_by_app.colors)
        } else {
            (&s.vram_by_app.colors, &s.gpu_by_app.colors)
        };
        assert_eq!(own.len(), 3, "{}", kind.name);
        assert!(others.is_empty(), "{}", kind.name);
        assert!(s.cpu_by_app.colors.is_empty() && s.memory_by_app.colors.is_empty());
    }
}
