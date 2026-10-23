//! Memory page: charts, the Swap chart rules, the menu checks and the demand (E25, UI-PRF-026;
//! PERF-018, PERF-026, PERF-041).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use tl_model::client::EngineClient;
use tl_model::demand::Demand;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::settings::{AppearanceSettings, NetworkUnit, Settings, ThemeSettings, UiState};
use tl_model::snapshot::SystemSnapshot;
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;
use tl_ui::anim::AnimationDriver;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::chart::scale::{ScaleState, compute};
use tl_ui::chart::{ChartState, ValueFormat, YScale, format_value};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::actions::PerfToggleMemorySwapGraph;
use tl_ui::pages::performance::memory::{COMMIT_ID, MemoryPage, PHYSICAL_ID, SWAP_ID};
use tl_ui::pages::performance::page::DevicePage as _;
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

const GIB: u64 = 1 << 30;
/// The Memory device in the fake catalog.
const MEM: DeviceIdx = DeviceIdx(1);

struct Host(Entity<MemoryPage>);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(1000.)).h(px(2000.)).child(self.0.clone())
    }
}

fn entry(idx: u16, identity: DeviceIdentity) -> DeviceEntry {
    DeviceEntry {
        idx: DeviceIdx(idx),
        identity,
        generation: 0,
        display_name: Arc::from("device"),
        present: true,
        parent: None,
        persistent_key: Arc::from(""),
        net_kind: None,
        hot_c: None,
    }
}

/// Valid memory totals: `total`, `commit_limit` and `swap_total` in bytes.
fn system(total: u64, commit_limit: u64, swap_total: u64) -> SystemSnapshot {
    let mut sys = SystemSnapshot::default();
    sys.header.validity = Validity::Valid;
    sys.header.version = 1;
    let m = &mut sys.memory;
    m.validity = Validity::Valid;
    m.total = total;
    m.used = total / 2;
    m.available = total - m.used;
    m.committed = commit_limit / 2;
    m.commit_limit = commit_limit;
    m.swap_total = swap_total;
    m.swap_free = swap_total;
    sys
}

struct Rig<'a> {
    page: Entity<MemoryPage>,
    demand: Entity<DemandAggregator>,
    engine: Arc<FakeEngine>,
    /// A live chart asks the window's driver for its next frame.
    _driver: Entity<AnimationDriver>,
    vcx: &'a mut VisualTestContext,
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

/// A Memory page over `sys` in a 1000 px wide window, with `settings`, its root focused.
fn open(cx: &mut TestAppContext, settings: Settings, sys: SystemSnapshot) -> Rig<'_> {
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(vec![
                entry(0, DeviceIdentity::CpuPackage),
                entry(1, DeviceIdentity::Memory),
            ]),
        });
        s.system = Arc::new(sys);
    });
    let client: Arc<dyn EngineClient> = engine.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client)));
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
    let key = DeviceKey(Arc::from(DeviceKey::MEMORY));
    let row = DeviceRow {
        key: key.clone(),
        kind: DeviceKind::Memory,
        idx: MEM,
        generation: 0,
        net_class: None,
        fan_connected: None,
    };
    let (h, d) = (hub.clone(), demand.clone());
    let page = cx.new(|cx| MemoryPage::new(key, row, h, d, cx));
    let host = page.clone();
    let (_, vcx) = cx.add_window_view(move |_, _| Host(host));
    vcx.simulate_scale_factor_change(1.0);
    let driver = vcx.update(|window, cx| {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        driver
    });
    let focus = vcx.update(|_, cx| page.read(cx).focus_handle(cx));
    vcx.update(|window, cx| focus.focus(window, cx));
    draw(vcx);
    Rig {
        page,
        demand,
        engine,
        _driver: driver,
        vcx,
    }
}

/// 60 one-second samples of `metric` on the Memory device ending now, the last one `newest`
/// and the others `older`.
fn series(metric: MetricId, older: f32, newest: f32) -> Arc<SeriesView> {
    const PERIOD: u64 = 1_000_000_000;
    let now = now_boot_ns();
    Arc::new(SeriesView {
        id: SeriesId {
            device: MEM,
            metric,
        },
        version: 1,
        period_ns: PERIOD,
        samples: (0..60u64)
            .map(|k| {
                Sample::valid(
                    now - (59 - k) * PERIOD,
                    if k == 59 { newest } else { older },
                )
            })
            .collect::<Vec<_>>()
            .into(),
    })
}

fn with_swap(on: bool) -> Settings {
    let mut s = Settings::default();
    s.performance.memory_swap_graph = on;
    s
}

#[gpui_kit::test]
fn memory_swap_graph_hidden_without_swap(cx: &mut TestAppContext) {
    let r = open(cx, with_swap(true), system(64 * GIB, 80 * GIB, 0));
    assert!(r.vcx.debug_bounds(PHYSICAL_ID).is_some());
    assert!(r.vcx.debug_bounds(COMMIT_ID).is_some());
    assert!(r.vcx.debug_bounds(SWAP_ID).is_none(), "no swap, no chart");
    let physical = r.vcx.debug_bounds(PHYSICAL_ID).expect("physical");
    let commit = r.vcx.debug_bounds(COMMIT_ID).expect("commit");
    assert_eq!(physical.size.height, px(150.));
    assert_eq!(commit.top() - physical.bottom(), px(8.), "sp4 apart");
}

#[gpui_kit::test]
fn memory_swap_toggle_follows_setting(cx: &mut TestAppContext) {
    let r = open(cx, with_swap(false), system(64 * GIB, 80 * GIB, 32 * GIB));
    assert!(r.vcx.debug_bounds(SWAP_ID).is_none(), "setting off");

    r.vcx.dispatch_action(PerfToggleMemorySwapGraph);
    draw(r.vcx);
    r.vcx.executor().advance_clock(Duration::from_millis(250));
    draw(r.vcx);
    let swap = r.vcx.debug_bounds(SWAP_ID).expect("swap chart shown");
    assert_eq!(swap.size.height, px(150.));
    let page = r.page.clone();
    let (setting, shown) = r.vcx.update(|_, cx| {
        (
            TlSettings::settings(cx).performance.memory_swap_graph,
            page.read(cx).show_swap(),
        )
    });
    assert!(setting && shown);
}

#[gpui_kit::test]
fn memory_menu_checks_follow_settings(cx: &mut TestAppContext) {
    use tl_ui::pages::performance::actions::PerfToggleMemoryComposition;
    let r = open(
        cx,
        Settings::default(),
        system(64 * GIB, 80 * GIB, 32 * GIB),
    );
    let page = r.page.clone();
    let before = r.vcx.update(|_, cx| page.read(cx).show_composition());
    r.vcx.dispatch_action(PerfToggleMemoryComposition);
    draw(r.vcx);
    let (after, setting) = r.vcx.update(|_, cx| {
        (
            page.read(cx).show_composition(),
            TlSettings::settings(cx).performance.memory_composition,
        )
    });
    assert_eq!((after, setting), (!before, !before));
    let words = r.vcx.update(|_, cx| {
        let w = page.read(cx).words();
        (w.menu_composition.to_string(), w.menu_swap.to_string())
    });
    assert_eq!(
        words,
        (
            "Show memory composition".to_owned(),
            "Show swap usage".to_owned()
        )
    );
}

#[gpui_kit::test]
fn commit_axis_text_above_100(cx: &mut TestAppContext) {
    let limit = 80 * GIB;
    let r = open(cx, Settings::default(), system(64 * GIB, limit, 0));
    let chart = r.vcx.update(|_, cx| r.page.read(cx).commit().clone());
    let peak = limit as f32 * 1.12;
    chart.update(r.vcx, |v, cx| {
        v.set_series_for_test(
            [
                Some(series(MetricId::MEM_COMMIT, peak, limit as f32 * 0.69)),
                None,
            ]
            .into(),
            cx,
        );
        v.set_state(ChartState::Live, cx);
    });
    draw(r.vcx);
    let (y, labels, value) = r
        .vcx
        .read_entity(&chart, |v, _| (v.spec.y, v.labels.clone(), v.value_now()));
    assert_eq!(y, YScale::Commit(limit as f32));
    assert!(labels.show_axis_max);
    assert_eq!(labels.value_fmt, ValueFormat::PercentOf { total: limit });
    assert_eq!(labels.heading.as_deref(), Some("Commit"));

    // The axis text the chart computes for this spec and the peak in view.
    let mut axis = String::new();
    let now = now_boot_ns();
    compute(
        &y,
        0.0,
        peak,
        now,
        &mut ScaleState::default(),
        labels.value_fmt,
        NetworkUnit::Bits,
        &mut axis,
    );
    assert_eq!(axis, "112%");

    let (value, validity) = value.expect("newest value");
    assert_eq!(validity, Validity::Valid);
    let mut text = String::new();
    format_value(labels.value_fmt, value, NetworkUnit::Bits, &mut text);
    assert_eq!(text, "69.0%");
}

#[gpui_kit::test]
fn memory_physical_value_and_scale(cx: &mut TestAppContext) {
    let total = (63.16 * GIB as f64) as u64;
    let used = 34.68 * GIB as f32;
    let r = open(cx, Settings::default(), system(total, 80 * GIB, 0));
    let chart = r.vcx.update(|_, cx| r.page.read(cx).physical().clone());
    chart.update(r.vcx, |v, cx| {
        v.set_series_for_test(
            [Some(series(MetricId::MEM_USED, used, used)), None].into(),
            cx,
        );
        v.set_state(ChartState::Live, cx);
    });
    draw(r.vcx);
    let (y, labels, value, ids) = r.vcx.read_entity(&chart, |v, _| {
        (
            v.spec.y,
            v.labels.clone(),
            v.value_now(),
            v.series_ids().map(|s| s.metric).collect::<Vec<_>>(),
        )
    });
    assert_eq!(y, YScale::FixedTotal(total as f32));
    assert_eq!(
        ids,
        [
            MetricId::MEM_USED,
            MetricId::MEM_COMMIT,
            MetricId::MEM_DIRTY
        ]
    );
    assert_eq!(labels.heading.as_deref(), Some("Physical memory"));
    let legend: Vec<&str> = labels.legend.iter().map(|(t, _)| t.as_ref()).collect();
    assert_eq!(legend, ["Used", "Committed"], "no legend entry for dirty");
    let (value, _) = value.expect("newest value");
    let mut text = String::new();
    format_value(labels.value_fmt, value, NetworkUnit::Bits, &mut text);
    assert_eq!(text, "34.68 GiB");

    let page = r.page.clone();
    let header = r.vcx.update(|_, cx| page.read(cx).header());
    assert_eq!(header.0.as_ref(), "Memory");
    assert!(header.1.is_empty());
}

#[gpui_kit::test]
fn memory_demand_only_when_active(cx: &mut TestAppContext) {
    let r = open(cx, Settings::default(), system(64 * GIB, 80 * GIB, 0));
    let (page, demand) = (r.page.clone(), r.demand.clone());
    let merged = |vcx: &mut VisualTestContext| vcx.update(|_, cx| demand.read(cx).merged());
    assert_eq!(merged(r.vcx), Demand::default(), "never active");

    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(true, cx)));
    let d = merged(r.vcx);
    assert!(d.any_surface_visible);
    assert!(d.memory);
    assert!(!d.disks && !d.nets && !d.gpus && !d.cpu_fast);

    r.vcx
        .update(|_, cx| page.update(cx, |p, cx| p.set_active(false, cx)));
    assert_eq!(merged(r.vcx), Demand::default());
}

#[gpui_kit::test]
fn memory_chart_hide_instant_under_reduce_motion(cx: &mut TestAppContext) {
    let mut settings = with_swap(true);
    settings.motion.animations = false;
    let r = open(cx, settings, system(64 * GIB, 80 * GIB, 32 * GIB));
    assert!(r.vcx.update(|_, cx| cx.reduce_motion()));
    assert!(r.vcx.debug_bounds(SWAP_ID).is_some());

    r.vcx.dispatch_action(PerfToggleMemorySwapGraph);
    let page = r.page.clone();
    assert!(
        !r.vcx.update(|_, cx| page.read(cx).swap_animating()),
        "no animation"
    );
    // One frame, no clock advance: the chart is gone.
    r.vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(r.vcx.debug_bounds(SWAP_ID).is_none());
}

#[gpui_kit::test]
fn memory_totals_change_rebuilds_specs(cx: &mut TestAppContext) {
    let r = open(cx, with_swap(true), system(64 * GIB, 80 * GIB, 0));
    assert!(r.vcx.debug_bounds(SWAP_ID).is_none());
    let page = r.page.clone();
    // A new SwapTotal arrives with the next system snapshot.
    let (physical, swap) = r.vcx.update(|_, cx| {
        let p = page.read(cx);
        (p.physical().clone(), p.swap().clone())
    });
    let builds = r.vcx.read_entity(&physical, |v, _| v.spec.clone());
    let mut sys = system(64 * GIB, 80 * GIB, 16 * GIB);
    sys.header.version = 2;
    r.engine.publish_with(move |s| s.system = Arc::new(sys));
    draw(r.vcx);
    r.vcx.executor().advance_clock(Duration::from_millis(250));
    draw(r.vcx);
    assert!(r.vcx.debug_bounds(SWAP_ID).is_some(), "swap appeared");
    let y = r.vcx.read_entity(&swap, |v, _| v.spec.y);
    assert_eq!(y, YScale::FixedTotal((16 * GIB) as f32));
    let same = r.vcx.read_entity(&physical, |v, _| v.spec.clone());
    assert_eq!(
        same, builds,
        "an unchanged MemTotal keeps the Physical spec"
    );
}

// Memory composition bar (E25, UI-PRF-027; PERF-042).

mod composition {
    use gpui_kit::{Modifiers, StyleRefinement, point};
    use tl_ui::pages::performance::memory_bar::{
        BAR_ID, Composition, CompositionBarView, SegmentKind,
    };

    use super::*;

    const MIB: u64 = 1 << 20;
    /// The bar is 1024 px wide, so 1 MiB of a 1024 MiB total is exactly 1 px.
    const W: f32 = 1024.;

    struct BarHost(Entity<CompositionBarView>);

    impl Render for BarHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(W))
                .child(self.0.clone().cached(StyleRefinement::default().w_full()))
        }
    }

    /// A bar alone in a window, at scale 1.
    fn bar(
        cx: &mut TestAppContext,
        c: Option<Composition>,
    ) -> (Entity<CompositionBarView>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            tl_ui::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
            tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
            init_strings(cx, "en-US");
        });
        let bar = cx.new(|cx| CompositionBarView::new(c, cx));
        let host = bar.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| BarHost(host));
        vcx.simulate_scale_factor_change(1.0);
        draw(vcx);
        (bar, vcx)
    }

    /// In use 600 px, Modified 60 px, Standby 72 px, Free 292 px.
    fn widths() -> Composition {
        Composition {
            in_use: 600 * MIB,
            compressed: 0,
            modified: 60 * MIB,
            standby: 72 * MIB,
            free: 292 * MIB,
            total: 1024 * MIB,
        }
    }

    #[gpui_kit::test]
    fn composition_label_min_72px(cx: &mut TestAppContext) {
        let (bar, vcx) = bar(cx, Some(widths()));
        vcx.read_entity(&bar, |b, _| {
            let w = |k| {
                b.state
                    .segments
                    .iter()
                    .find(|s| s.kind == k)
                    .map(|s| s.width())
            };
            assert_eq!(w(SegmentKind::Modified), Some(60.));
            assert_eq!(w(SegmentKind::Standby), Some(72.));
            assert_eq!(
                b.painted_labels.borrow().as_slice(),
                [SegmentKind::InUse, SegmentKind::Standby, SegmentKind::Free],
                "no label on the 60 px segment"
            );
            assert_eq!(b.painted_segments.get(), 4);
            assert_eq!(b.caption().as_ref(), "Memory composition");
        });
    }

    #[gpui_kit::test]
    fn composition_warming_up_outline_only(cx: &mut TestAppContext) {
        let (bar, vcx) = bar(cx, None);
        vcx.read_entity(&bar, |b, _| {
            assert_eq!(b.painted_segments.get(), 0, "outline only");
            assert!(b.painted_labels.borrow().is_empty());
            assert_eq!(b.caption().as_ref(), "Collecting…");
        });
    }

    #[gpui_kit::test]
    fn composition_hitboxes_only_when_inside(cx: &mut TestAppContext) {
        let (bar, vcx) = bar(cx, Some(widths()));
        let bounds = vcx.debug_bounds(BAR_ID).expect("bar box");
        // Outside the bar: no hitboxes.
        let below = bounds.bottom() + px(20.);
        vcx.simulate_mouse_move(point(px(10.), below), None, Modifiers::none());
        draw(vcx);
        assert_eq!(vcx.read_entity(&bar, |b, _| b.hitbox_count.get()), 0);
        // Over In use: one hitbox per segment, In use hovered with its tooltip.
        let y = bounds.top() + bounds.size.height / 2.;
        vcx.simulate_mouse_move(point(bounds.left() + px(100.), y), None, Modifiers::none());
        draw(vcx);
        vcx.read_entity(&bar, |b, _| {
            assert_eq!(b.hitbox_count.get(), 4);
            assert_eq!(b.state.hover, Some(SegmentKind::InUse));
            assert_eq!(
                b.hover_tooltip().as_deref(),
                Some("In use: 600.00 MiB. Memory used by running programs and the kernel.")
            );
        });
        // Out again: none.
        vcx.simulate_mouse_move(point(px(10.), below), None, Modifiers::none());
        draw(vcx);
        vcx.read_entity(&bar, |b, _| {
            assert_eq!(b.hitbox_count.get(), 0);
            assert_eq!(b.state.hover, None);
        });
    }

    #[gpui_kit::test]
    fn composition_hatch_built_once(cx: &mut TestAppContext) {
        let c = Composition {
            compressed: 64 * MIB,
            in_use: 536 * MIB,
            ..widths()
        };
        let (bar, vcx) = bar(cx, Some(c));
        // The window's first frame can come before the scale factor is set, so count from here.
        let builds = vcx.read_entity(&bar, |b, _| b.hatch_builds.get());
        assert!(builds >= 1);
        // Repaints with the same edges reuse the path.
        for _ in 0..3 {
            vcx.update(|_, cx| bar.update(cx, |_, cx| cx.notify()));
            draw(vcx);
        }
        assert_eq!(vcx.read_entity(&bar, |b, _| b.hatch_builds.get()), builds);
        // A same composition sent again does not repaint the bar.
        let renders = vcx.read_entity(&bar, |b, _| b.render_count.get());
        vcx.update(|_, cx| bar.update(cx, |b, cx| b.set_composition(Some(c), cx)));
        draw(vcx);
        assert_eq!(vcx.read_entity(&bar, |b, _| b.render_count.get()), renders);
    }

    #[gpui_kit::test]
    fn memory_page_shows_composition_bar(cx: &mut TestAppContext) {
        let mut sys = system(64 * GIB, 80 * GIB, 0);
        sys.memory.free = 20 * GIB;
        let r = open(cx, Settings::default(), sys);
        let bar = r
            .vcx
            .update(|_, cx| r.page.read(cx).bar().cloned())
            .expect("bar shown by default");
        r.vcx.read_entity(&bar, |b, _| {
            let c = b.state.composition.expect("valid memory");
            assert_eq!(c.in_use, 32 * GIB);
            assert_eq!(c.free, 20 * GIB);
            assert_eq!(c.standby, 12 * GIB);
        });
        assert!(r.vcx.debug_bounds(BAR_ID).is_some(), "bar drawn");
    }

    /// The box around the bar holds the caption and the bar, so what follows it (the Details
    /// heading) cannot be drawn over the bar.
    #[gpui_kit::test]
    fn composition_box_encloses_caption_and_bar(cx: &mut TestAppContext) {
        let mut sys = system(64 * GIB, 80 * GIB, 0);
        sys.memory.free = 20 * GIB;
        let r = open(cx, Settings::default(), sys);
        let wrap = r
            .vcx
            .debug_bounds(tl_ui::pages::performance::memory::BAR_WRAP_ID)
            .expect("bar box");
        let bar = r.vcx.debug_bounds(BAR_ID).expect("bar");
        assert!(
            bar.bottom() <= wrap.bottom(),
            "bar {bar:?} inside box {wrap:?}"
        );
        assert!(
            wrap.size.height > bar.size.height,
            "box also holds the caption"
        );
    }
}

// Memory Details rows (E25, UI-PRF-028; PERF-043).

mod details {
    use tl_model::system::{Metric, ZramTotals};
    use tl_ui::pages::performance::details::KeyValueModel;
    use tl_ui::pages::performance::memory::{
        MemoryDetailInputs, memory_details_model, update_memory_details,
    };

    use super::*;

    const MIB: u64 = 1 << 20;
    const KIB: u64 = 1 << 10;

    fn gib(x: f64) -> u64 {
        (x * GIB as f64).round() as u64
    }

    /// The memory record of [`system`] with every row field set.
    fn record() -> tl_model::system::MemoryTotals {
        let mut m = system(64 * GIB, gib(67.20), 32 * GIB).memory;
        m.committed = gib(46.20);
        m.cached = gib(12.40);
        m.free = 10 * GIB;
        m.slab_reclaimable = gib(1.10);
        m.slab_unreclaimable = 420 * MIB;
        m.swap_free = gib(30.80);
        m.zswap = 512 * MIB;
        m.zswapped = 2 * GIB;
        m.zram = Metric::valid(ZramTotals {
            orig_data: GIB,
            compr_data: 256 * MIB,
            mem_used: 512 * MIB,
        });
        m.huge_pages_total = 2048;
        m.huge_page_size = 2 * MIB;
        m
    }

    /// The model updated from `m`, with translated labels.
    fn model(cx: &mut TestAppContext, m: &tl_model::system::MemoryTotals) -> KeyValueModel {
        let mut model = memory_details_model();
        let inp = MemoryDetailInputs {
            mem: m,
            now_boot_ns: 0,
            observed_end_ns: 0,
            error: None,
        };
        update_memory_details(&mut model, &inp);
        cx.update(|cx| {
            init_strings(cx, "en-US");
            model.refresh_strings(cx);
            model.refresh_labels(cx);
        });
        model
    }

    fn rows(model: &KeyValueModel) -> Vec<(String, String)> {
        model
            .to_kv_rows()
            .map(|r| (r.label.to_string(), r.value.text.to_string()))
            .collect()
    }

    fn value(model: &KeyValueModel, label: &str) -> Option<String> {
        rows(model)
            .into_iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| v)
    }

    #[gpui_kit::test]
    fn memory_details_order(cx: &mut TestAppContext) {
        let model = model(cx, &record());
        let labels: Vec<String> = rows(&model).into_iter().map(|(l, _)| l).collect();
        assert_eq!(
            labels,
            [
                "In use",
                "Available",
                "Committed",
                "Cached",
                "Kernel slab (reclaimable)",
                "Kernel slab (unreclaimable)",
                "Swap used",
                "Swap available",
                "Compressed",
                "Savings",
                "Huge pages",
            ]
        );
        assert_eq!(model.rows.len(), 16);
        let values: Vec<String> = rows(&model).into_iter().map(|(_, v)| v).collect();
        assert_eq!(
            values,
            [
                "32.00 GiB",
                "32.00 GiB",
                "46.20 GiB / 67.20 GiB",
                "12.40 GiB",
                "1.10 GiB",
                "420.00 MiB",
                "1.20 GiB",
                "30.80 GiB",
                "1.00 GiB",
                // (1 GiB + 2 GiB) − (256 MiB + 512 MiB)
                "2.25 GiB",
                "2\u{2009}048 × 2 MiB",
            ]
        );
        let tip = |label: &str| {
            model
                .to_kv_rows()
                .find(|r| r.label.as_ref() == label)
                .and_then(|r| r.value.tooltip.map(|t| t.to_string()))
        };
        assert_eq!(
            tip("Kernel slab (reclaimable)").as_deref(),
            Some("SReclaimable")
        );
        assert_eq!(
            tip("Kernel slab (unreclaimable)").as_deref(),
            Some("SUnreclaim")
        );
    }

    #[gpui_kit::test]
    fn memory_hidden_rows_rule(cx: &mut TestAppContext) {
        let mut m = record();
        m.zram = Metric::with(Validity::Unsupported);
        m.zswap = 512 * KIB;
        let small = model(cx, &m);
        assert_eq!(value(&small, "Compressed"), None);
        assert_eq!(value(&small, "Savings"), None);
        m.zswap = MIB;
        let shown = model(cx, &m);
        assert_eq!(value(&shown, "Compressed").as_deref(), Some("1.00 MiB"));
        assert!(value(&shown, "Savings").is_some());
    }

    #[gpui_kit::test]
    fn memory_huge_pages_format(cx: &mut TestAppContext) {
        let mut m = record();
        m.huge_pages_total = 0;
        assert_eq!(value(&model(cx, &m), "Huge pages"), None);
        m.huge_pages_total = 2048;
        assert_eq!(
            value(&model(cx, &m), "Huge pages").as_deref(),
            Some("2\u{2009}048 × 2 MiB")
        );
    }

    #[gpui_kit::test]
    fn memory_committed_composite(cx: &mut TestAppContext) {
        let model = model(cx, &record());
        assert_eq!(
            value(&model, "Committed").as_deref(),
            Some("46.20 GiB / 67.20 GiB")
        );
    }

    #[gpui_kit::test]
    fn memory_details_collecting_and_failed(cx: &mut TestAppContext) {
        let mut m = record();
        m.validity = Validity::WarmingUp;
        let warming = model(cx, &m);
        assert!(rows(&warming).iter().all(|(_, v)| v == "Collecting…"));

        m.validity = Validity::Failed;
        let mut failed = memory_details_model();
        let inp = MemoryDetailInputs {
            mem: &m,
            now_boot_ns: 0,
            observed_end_ns: 0,
            error: Some("read /proc/meminfo: EACCES"),
        };
        update_memory_details(&mut failed, &inp);
        let row = failed.rows.iter().find(|r| !r.hidden).expect("a row");
        assert_eq!(row.value.as_ref(), "Unavailable");
        assert_eq!(
            row.tooltip.as_deref(),
            Some("Could not read: read /proc/meminfo: EACCES")
        );
    }

    #[gpui_kit::test]
    fn memory_page_details_from_snapshot(cx: &mut TestAppContext) {
        let mut sys = system(64 * GIB, 80 * GIB, 0);
        sys.memory.cached = gib(12.40);
        let r = open(cx, Settings::default(), sys);
        let page = r.page.clone();
        let cached = r.vcx.update(|_, cx| {
            let d = page.read(cx).details();
            d.refresh_labels(cx);
            d.to_kv_rows()
                .find(|row| row.label.as_ref() == "Cached")
                .map(|row| row.value.text.to_string())
        });
        assert_eq!(cached.as_deref(), Some("12.40 GiB"));
    }
}
