//! Graph summary mode (E24-015) and the page menu, keys and copy on the Performance tab, on the
//! page host. Moved from the tl-ui `ui` binary (tasklens-zljr5.41). Four tests stay there,
//! because they check the main window: the tab switch block, the title bar menus, the main
//! toast queue and the hidden window.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    Entity, Focusable as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext,
    VisualTestContext, px, size,
};
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, MetricId, SeriesId};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::SeriesSet;
use tl_model::system::Metric;
use tl_model::validity::Validity;
use tl_ui_base::app_env::BootSettings;
use tl_ui_base::motion::{M_BASE, M_FAST};
use tl_ui_base::settings_global::TlSettings;
use tl_ui_kit::components::menu::{MenuEntryKind, TlMenuPopup, open_context_menu};
use tl_ui_pagehost::PageHost;
use tl_ui_perf::performance::overview::OVERVIEW_KEY;
use tl_ui_perf::performance::summary::{MARKER_ID, SummaryView, tile_grid};
use tl_ui_perf::performance::{DeviceKey, PerformancePage, SIDEBAR_PANEL_ID};
use tl_ui_settings::settings::{DeviceVisibility, Settings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

const GPU: &str = "gpu:pci:0000:2d:00.0";

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
        hot_c: None,
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
fn catalog() -> Vec<DeviceEntry> {
    let gpu = DeviceIdentity::Gpu {
        uuid: [0; 16],
        pci_bus_id: Arc::from("0000:2d:00.0"),
    };
    vec![
        entry(0, DeviceIdentity::CpuPackage, None),
        entry(1, DeviceIdentity::Memory, None),
        entry(2, gpu, None),
        disk(3, "sda"),
        disk(4, "sdb"),
        disk(5, "sdc"),
        disk(6, "sdd"),
        net(7, "enp38s0", NetKind::Wired),
        net(8, "tailscale0", NetKind::VpnTunnel),
    ]
}

struct Rig<'a> {
    page: Entity<PerformancePage>,
    engine: Arc<FakeEngine>,
    /// The last value handed to the `state.toml` persistence callback.
    persisted_state: Rc<RefCell<Option<UiState>>>,
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

/// The Performance tab on the page host, `width` × `height`, with the sidebar focused.
fn open(cx: &mut TestAppContext, width: f32, height: f32, reduce_motion: bool) -> Rig<'_> {
    open_with(cx, width, height, reduce_motion, Settings::default())
}

/// [`open`] with `settings`. The Overview card is hidden unless `settings` has an entry
/// for it (D-022).
fn open_with(
    cx: &mut TestAppContext,
    width: f32,
    height: f32,
    reduce_motion: bool,
    settings: Settings,
) -> Rig<'_> {
    open_catalog(cx, width, height, reduce_motion, settings, catalog())
}

/// [`open_with`] on a catalog of `devices`.
fn open_catalog(
    cx: &mut TestAppContext,
    width: f32,
    height: f32,
    reduce_motion: bool,
    mut settings: Settings,
    devices: Vec<DeviceEntry>,
) -> Rig<'_> {
    settings
        .performance
        .device_overrides
        .entry(OVERVIEW_KEY.to_owned())
        .or_insert(DeviceVisibility::Hide);
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(devices),
        });
    });
    let host = PageHost::install(cx, Arc::clone(&engine), settings);
    let persisted_state = Rc::new(RefCell::new(None));
    let persisted = Rc::clone(&persisted_state);
    cx.update(|cx| {
        cx.set_reduce_motion(reduce_motion);
        // The page host persists nothing; the state.toml checks need the callback.
        cx.global_mut::<BootSettings>().persist = Some(Box::new(move |_, state| {
            *persisted.borrow_mut() = Some(state.clone());
        }));
        TlSettings::init(cx);
        // The keys as `TasklensUi::init` binds them for this page.
        tl_ui_kit::actions::bind_keys_global(cx);
        tl_ui_kit::components::menu::bind_keys(cx);
        tl_ui_perf::performance::actions::bind_keys_performance(cx);
    });
    let page_host = host.host();
    let (page, vcx) = host.open(cx, size(px(width), px(height)), move |_, cx| {
        PerformancePage::new(&page_host, cx)
    });
    vcx.update(|window, _| window.activate_window());
    let focus = vcx.update(|_, cx| page.read(cx).primary_focus());
    vcx.update(|window, cx| focus.focus(window, cx));
    draw(vcx);
    Rig {
        page,
        engine,
        persisted_state,
        vcx,
    }
}

fn key(s: &str) -> DeviceKey {
    DeviceKey(Arc::from(s))
}

fn summary(r: &mut Rig) -> Option<Entity<SummaryView>> {
    let page = r.page.clone();
    r.vcx.update(|_, cx| page.read(cx).summary().cloned())
}

/// Ctrl+M from the sidebar, then the `m_fast` fade-out when animations are on.
fn enter(r: &mut Rig) -> Entity<SummaryView> {
    r.vcx.simulate_keystrokes("ctrl-m");
    draw(r.vcx);
    if summary(r).is_none() {
        r.vcx.executor().advance_clock(M_FAST);
        draw(r.vcx);
    }
    summary(r).expect("summary mode on")
}

fn pending(r: &mut Rig) -> bool {
    let page = r.page.clone();
    r.vcx.update(|_, cx| page.read(cx).summary_pending())
}

/// Frame callbacks queued per frame, for `n` frames.
fn frames(r: &mut Rig, n: usize) -> Vec<usize> {
    (0..n)
        .map(|_| {
            r.vcx.run_until_parked();
            r.vcx.update(|window, cx| {
                let queued = window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
                queued
            })
        })
        .collect()
}

fn select(r: &mut Rig, k: &str) {
    let page = r.page.clone();
    let k = key(k);
    r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().clone();
        sidebar.update(cx, |s, cx| s.select(k, cx));
    });
    draw(r.vcx);
}

fn selected(r: &mut Rig) -> Option<String> {
    let page = r.page.clone();
    r.vcx
        .update(|_, cx| page.read(cx).selected().map(|k| k.0.to_string()))
}

fn title(r: &mut Rig) -> String {
    let page = r.page.clone();
    r.vcx.update(|_, cx| page.read(cx).window_title.clone())
}

fn click(cx: &mut VisualTestContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
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
}

#[gpui_kit::test]
fn summary_tiles_all_visible_devices(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., false);
    let page = r.page.clone();
    let cards: Vec<String> = r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().clone();
        sidebar
            .read(cx)
            .visible_keys()
            .map(|k| k.0.to_string())
            .collect()
    });
    assert_eq!(cards.len(), 9, "{cards:?}");
    let view = enter(&mut r);
    let tiles: Vec<String> = r.vcx.read_entity(&view, |v, _| {
        v.tiles.iter().map(|t| t.key.0.to_string()).collect()
    });
    assert_eq!(tiles, cards, "one tile per card, in sidebar order");
    for i in 0..9 {
        let id = format!("perf-summary-tile-{i}");
        assert!(r.vcx.debug_bounds(Box::leak(id.into_boxed_str())).is_some());
    }
    // The tab strip and the sidebar are not drawn under the summary.
    assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_none());
}

#[gpui_kit::test]
fn summary_skips_overview_card(cx: &mut TestAppContext) {
    let mut settings = Settings::default();
    settings
        .performance
        .device_overrides
        .insert(OVERVIEW_KEY.to_owned(), DeviceVisibility::Show);
    let mut r = open_with(cx, 1600., 1000., false, settings);
    let page = r.page.clone();
    let cards: Vec<String> = r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().clone();
        sidebar
            .read(cx)
            .visible_keys()
            .map(|k| k.0.to_string())
            .collect()
    });
    assert_eq!(cards.len(), 10, "{cards:?}");
    assert_eq!(cards[0], OVERVIEW_KEY);
    let view = enter(&mut r);
    let tiles: Vec<String> = r.vcx.read_entity(&view, |v, _| {
        v.tiles.iter().map(|t| t.key.0.to_string()).collect()
    });
    assert_eq!(tiles, cards[1..], "no Overview tile (D-022)");
    // A sidebar notify re-runs the tile sync; the Overview card does not rebuild the tiles.
    let first = r
        .vcx
        .read_entity(&view, |v, _| v.tiles[0].chart.entity_id());
    r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().clone();
        sidebar.update(cx, |_, cx| cx.notify());
    });
    draw(r.vcx);
    let again = r
        .vcx
        .read_entity(&view, |v, _| v.tiles[0].chart.entity_id());
    assert_eq!(first, again);
}

/// 14 visible devices make 14 tiles in 4 columns and 4 rows, `ceil(sqrt(14))` (UI-PRF-016),
/// not 5 columns, through the real sidebar, the tile sync and the first paint.
#[gpui_kit::test]
fn summary_tile_grid_for_14(cx: &mut TestAppContext) {
    let mut devices = catalog();
    devices.extend((9..14).map(|i| disk(i, &format!("sd{i}"))));
    let mut r = open_catalog(cx, 1600., 1000., false, Settings::default(), devices);
    let view = enter(&mut r);
    let n = r.vcx.read_entity(&view, |v, _| v.tiles.len());
    let l = r.vcx.read_entity(&view, |v, _| v.layout());
    assert_eq!((n, l.cols, l.rows), (14, 4, 4));
    let t3 = r.vcx.debug_bounds("perf-summary-tile-3").expect("tile 3");
    let t4 = r.vcx.debug_bounds("perf-summary-tile-4").expect("tile 4");
    assert_eq!(
        t4.left(),
        r.vcx.debug_bounds("perf-summary-tile-0").unwrap().left()
    );
    assert!(t4.top() > t3.top(), "tile 4 starts the second row");
}

/// The drawn grid has the layout's columns at every width. 18 tiles take 5 columns; at a
/// width where a tile is not a whole number of pixels (242.8 px at 1278 px, the 1.0.2 smoke
/// test), a wrapping row dropped the fifth tile to the next row, so the grid drew 4 columns
/// and 5 rows and the last row fell below the window.
#[gpui_kit::test]
fn summary_tiles_fill_every_column(cx: &mut TestAppContext) {
    let mut devices = catalog();
    devices.extend((9..18).map(|i| disk(i, &format!("sd{i}"))));
    let mut r = open_catalog(cx, 1600., 1000., false, Settings::default(), devices);
    let view = enter(&mut r);
    let ids: Vec<&'static str> = (0..18)
        .map(|i| &*format!("perf-summary-tile-{i}").leak())
        .collect();
    for width in 1270..=1290 {
        r.vcx.simulate_resize(size(px(width as f32), px(1000.)));
        draw(r.vcx);
        let (n, l) = r.vcx.read_entity(&view, |v, _| (v.tiles.len(), v.layout()));
        assert_eq!((n, l.cols, l.rows), (18, 5, 4), "at {width} px");
        let tile = |r: &mut Rig, i: usize| r.vcx.debug_bounds(ids[i]).expect("tile bounds");
        for i in 0..n {
            let (row_first, col_first) = (tile(&mut r, i - i % l.cols), tile(&mut r, i % l.cols));
            let t = tile(&mut r, i);
            assert_eq!(
                t.top(),
                row_first.top(),
                "tile {i} in row {} at {width} px",
                i / l.cols
            );
            assert_eq!(
                t.left(),
                col_first.left(),
                "tile {i} in column {} at {width} px",
                i % l.cols
            );
        }
    }
}

#[gpui_kit::test]
fn summary_tile_grid_for_9(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., false);
    let view = enter(&mut r);
    let l = r.vcx.read_entity(&view, |v, _| v.layout());
    assert_eq!((l.cols, l.rows), (3, 3));
    assert!(!l.scrolls);
    let t0 = r.vcx.debug_bounds("perf-summary-tile-0").expect("tile 0");
    let t2 = r.vcx.debug_bounds("perf-summary-tile-2").expect("tile 2");
    let t3 = r.vcx.debug_bounds("perf-summary-tile-3").expect("tile 3");
    assert_eq!(t0.size, t3.size);
    assert_eq!(t2.top(), t0.top(), "three tiles in the first row");
    assert!(t3.top() > t0.bottom(), "tile 3 starts the second row");
    assert_eq!(t3.left(), t0.left());
}

/// Each tile's legend names its series in the UI-PRF-003 words and styles: Memory "Used"
/// solid and "Committed" dashed, Network "Download" solid and "Upload" dashed, CPU "Total"
/// and "Kernel" only with the kernel line, and none on the one-series tiles. Checked against
/// the 1.0.0 smoke-test shot of summary mode (o55.43 h).
#[gpui_kit::test]
fn summary_tile_legends_match_their_series(cx: &mut TestAppContext) {
    use tl_ui_chart::chart::LegendStyle::{Dashed, Solid};
    use tl_ui_perf::performance::DeviceKind;
    let mut r = open(cx, 1600., 1000., false);
    let view = enter(&mut r);
    let tiles: Vec<_> = r.vcx.update(|_, cx| {
        view.read(cx)
            .tiles
            .iter()
            .map(|t| {
                let c = t.chart.read(cx);
                let legend: Vec<_> = c
                    .labels
                    .legend
                    .iter()
                    .map(|(s, style)| (s.to_string(), *style))
                    .collect();
                let dashed: Vec<bool> = c.spec.series.iter().map(|s| s.dashed).collect();
                (t.kind, legend, dashed)
            })
            .collect()
    });
    let words = |l: &[(String, tl_ui_chart::chart::LegendStyle)]| -> Vec<_> {
        l.iter().map(|(s, style)| (s.clone(), *style)).collect()
    };
    for (kind, legend, dashed) in &tiles {
        let pairs = |a: &str, sa, b: &str, sb| vec![(a.to_string(), sa), (b.to_string(), sb)];
        match kind {
            DeviceKind::Memory => {
                assert_eq!(words(legend), pairs("Used", Solid, "Committed", Dashed));
            }
            DeviceKind::Net => {
                assert_eq!(words(legend), pairs("Download", Solid, "Upload", Dashed));
            }
            DeviceKind::Cpu if dashed.len() > 1 => {
                assert_eq!(words(legend), pairs("Total", Solid, "Kernel", Solid));
            }
            _ => assert!(legend.is_empty(), "{kind:?}: {legend:?}"),
        }
        if !legend.is_empty() {
            assert_eq!(legend.len(), dashed.len(), "{kind:?}: one item per series");
            for ((_, style), dashed) in legend.iter().zip(dashed) {
                assert_eq!(
                    *style == Dashed,
                    *dashed,
                    "{kind:?}: swatch matches the line"
                );
            }
        }
    }
    let kinds: Vec<_> = tiles.iter().map(|t| t.0).collect();
    assert!(kinds.contains(&DeviceKind::Memory) && kinds.contains(&DeviceKind::Net));
}

#[gpui_kit::test]
fn summary_esc_returns_to_selected_device(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., false);
    select(&mut r, "memory");
    assert_eq!(title(&mut r), "");
    let view = enter(&mut r);
    assert_eq!(title(&mut r), "tasklens · Graph Summary");
    let marked = r.vcx.read_entity(&view, |v, _| v.selected_on_entry.clone());
    assert_eq!(marked, Some(key("memory")));
    let marker = r.vcx.debug_bounds(MARKER_ID).expect("accent marker");
    let tile = r
        .vcx
        .debug_bounds("perf-summary-tile-1")
        .expect("memory tile");
    assert!(
        tile.contains(&marker.origin),
        "the marker is on the memory tile"
    );

    r.vcx.simulate_keystrokes("escape");
    draw(r.vcx);
    assert!(summary(&mut r).is_none());
    assert_eq!(selected(&mut r).as_deref(), Some("memory"));
    assert_eq!(title(&mut r), "tasklens");
    assert!(r.vcx.debug_bounds("perf-summary-tile-0").is_none());
    assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_some());
}

#[gpui_kit::test]
fn summary_click_selects_and_exits(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., false);
    let view = enter(&mut r);
    let i = r
        .vcx
        .read_entity(&view, |v, _| {
            v.tiles.iter().position(|t| t.key.0.as_ref() == GPU)
        })
        .expect("a GPU tile");
    let id = format!("perf-summary-tile-{i}");
    let tile = r
        .vcx
        .debug_bounds(Box::leak(id.into_boxed_str()))
        .expect("GPU tile drawn");
    click(r.vcx, tile.center());
    draw(r.vcx);
    assert!(summary(&mut r).is_none());
    assert_eq!(selected(&mut r).as_deref(), Some(GPU));
    assert_eq!(title(&mut r), "tasklens");
}

#[test]
fn summary_small_window_shrinks_cols_and_scrolls() {
    let l = tile_grid(9, px(500.), px(300.));
    assert_eq!(l.cols, 2, "two 200 px columns fit, three do not");
    assert_eq!(l.rows, 5);
    assert!(l.scrolls);
    assert!(l.tile_w >= px(200.));
    assert_eq!(l.tile_h, px(120.), "the minimum tile height");

    let big = tile_grid(9, px(1600.), px(1000.));
    assert_eq!((big.cols, big.rows), (3, 3));
    assert!(!big.scrolls);
}

#[gpui_kit::test]
fn summary_fade_follows_reduce_motion(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    let view = enter(&mut r);
    assert!(
        !r.vcx.read_entity(&view, |v, _| v.fades()),
        "reduce motion: instant"
    );
}

/// The sidebar and page fade out over `m_fast` before the tiles show, and the frames stop
/// once both fades end.
#[gpui_kit::test]
fn summary_fades_page_out_then_tiles_in(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., false);
    r.vcx.simulate_keystrokes("ctrl-m");
    draw(r.vcx);
    assert!(pending(&mut r), "fading out");
    assert!(summary(&mut r).is_none());
    assert!(
        r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_some(),
        "still drawn"
    );
    assert!(frames(&mut r, 1)[0] > 0, "the fade requests frames");

    r.vcx.executor().advance_clock(M_FAST);
    draw(r.vcx);
    assert!(!pending(&mut r));
    let view = summary(&mut r).expect("summary on after m_fast");
    assert!(r.vcx.read_entity(&view, |v, _| v.fades()));
    assert!(r.vcx.debug_bounds(SIDEBAR_PANEL_ID).is_none());

    // GPUI times `with_animation` by the wall clock, not the test clock.
    std::thread::sleep(M_BASE + M_FAST);
    let seen = frames(&mut r, 8);
    assert_eq!(
        &seen[5..],
        &[0, 0, 0],
        "no frames after the fades: {seen:?}"
    );
}

/// Ctrl+M again during the fade-out stays on the page.
#[gpui_kit::test]
fn summary_toggle_during_fade_cancels(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., false);
    r.vcx.simulate_keystrokes("ctrl-m");
    draw(r.vcx);
    assert!(pending(&mut r));
    r.vcx.simulate_keystrokes("ctrl-m");
    r.vcx.executor().advance_clock(M_FAST * 2);
    draw(r.vcx);
    assert!(!pending(&mut r));
    assert!(summary(&mut r).is_none());
    assert_eq!(title(&mut r), "");
}

#[gpui_kit::test]
fn summary_opens_at_once_with_animations_off(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    r.vcx.simulate_keystrokes("ctrl-m");
    draw(r.vcx);
    assert!(!pending(&mut r));
    assert!(summary(&mut r).is_some(), "no fade-out wait");
}

#[gpui_kit::test]
fn summary_fades_in_with_motion(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., false);
    let view = enter(&mut r);
    assert!(r.vcx.read_entity(&view, |v, _| v.fades()));
}

/// The summary view never notifies on a tick: a snapshot that changes no tile series renders
/// nothing. A CPU sample notifies the CPU chart only. GPUI then marks every ancestor view
/// dirty, so the summary view and the other tile charts re-run `render` too, but the other
/// charts build no new geometry (ADR-009).
#[gpui_kit::test]
fn summary_sample_tick_does_not_rerender_view(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    let view = enter(&mut r);
    draw(r.vcx);
    let (cpu, memory) = r.vcx.read_entity(&view, |v, _| {
        (v.tiles[0].chart.clone(), v.tiles[1].chart.clone())
    });
    assert_eq!(
        r.vcx.read_entity(&view, |v, _| v.tiles[1].key.clone()),
        key("memory")
    );
    let counts = |r: &mut Rig| {
        let (view, cpu, memory) = (view.clone(), cpu.clone(), memory.clone());
        r.vcx.update(|_, cx| {
            (
                view.read(cx).render_count.get(),
                cpu.read(cx).render_count.get(),
                memory.read(cx).build_count.get(),
            )
        })
    };

    let before = counts(&mut r);
    for _ in 0..3 {
        r.engine
            .publish_with(|s| Arc::make_mut(&mut s.processes).header.version += 1);
        draw(r.vcx);
    }
    assert_eq!(
        counts(&mut r),
        before,
        "no tile series changed: nothing renders"
    );

    for version in 1..=3u64 {
        r.engine.publish_with(|s| {
            let id = SeriesId {
                device: DeviceIdx(0),
                metric: MetricId::CPU_TOTAL,
            };
            let samples: Vec<Sample> = (0..version)
                .map(|i| Sample::valid(i * 1_000_000_000, 10.0 * i as f32))
                .collect();
            let mut set = SeriesSet::default();
            set.header.version = version;
            set.series = Arc::from([Arc::new(SeriesView {
                id,
                version,
                period_ns: 1_000_000_000,
                samples: Arc::from(samples),
            })]);
            s.series = Arc::new(set);
        });
        draw(r.vcx);
    }
    let after = counts(&mut r);
    assert!(
        after.1 > before.1,
        "the CPU chart re-rendered: {before:?} {after:?}"
    );
    assert_eq!(after.2, before.2, "the memory chart built no geometry");
}

/// UI-PRF-058: the page header shows its device's main value, the sidebar card's value, at
/// the right before the toggles, and draws it again on each new sample.
#[gpui_kit::test]
fn page_header_value_follows_the_card(cx: &mut TestAppContext) {
    let r = open(cx, 1600., 1000., true);
    let publish = |r: &Rig, version: u64, value: f32| {
        r.engine.publish_with(|s| {
            let mut set = SeriesSet::default();
            set.header.version = version;
            set.series = Arc::from([Arc::new(SeriesView {
                id: SeriesId {
                    device: DeviceIdx(0),
                    metric: MetricId::CPU_TOTAL,
                },
                version,
                period_ns: 1_000_000_000,
                samples: Arc::from([Sample::valid(1_000_000_000, value)]),
            })]);
            s.series = Arc::new(set);
        });
    };
    publish(&r, 1, 8.0);
    draw(r.vcx);
    let header = r.vcx.debug_bounds("perf-page-header").expect("header");
    let model = r.vcx.debug_bounds("perf-page-model").expect("model");
    let toggles = r.vcx.debug_bounds("perf-page-toggles").expect("toggles");
    let narrow = r.vcx.debug_bounds("perf-page-value").expect("header value");
    assert!(narrow.left() >= model.right(), "{narrow:?} after {model:?}");
    assert_eq!(
        toggles.left() - narrow.right(),
        px(8.),
        "sp4 before the toggles"
    );
    assert!(narrow.top() >= header.top() && narrow.bottom() <= header.bottom());

    // "100.0%" is wider than "8.0%": the value follows the new sample.
    publish(&r, 2, 100.0);
    draw(r.vcx);
    let wide = r.vcx.debug_bounds("perf-page-value").expect("header value");
    assert!(
        wide.size.width > narrow.size.width,
        "{wide:?} after {narrow:?}"
    );
    assert_eq!(
        wide.right(),
        narrow.right(),
        "right-aligned before the toggles"
    );
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

/// One built menu entry: kind, label, tooltip, checked, submenu.
type Entry = (
    MenuEntryKind,
    String,
    Option<String>,
    bool,
    Option<Entity<TlMenuPopup>>,
);

fn entries_of(r: &mut Rig, menu: &Entity<TlMenuPopup>) -> Vec<Entry> {
    r.vcx.update(|window, cx| {
        menu.read(cx)
            .entries_for_test(window)
            .into_iter()
            .map(|e| {
                (
                    e.kind,
                    e.label.map(|l| l.to_string()).unwrap_or_default(),
                    e.tooltip.map(|t| t.to_string()),
                    e.checked,
                    e.submenu,
                )
            })
            .collect()
    })
}

/// The open context menu's entries.
fn open_menu(r: &mut Rig) -> Vec<Entry> {
    let menu = r
        .vcx
        .update(|window, cx| open_context_menu(window, cx))
        .expect("a context menu is open");
    entries_of(r, &menu)
}

fn labels(list: &[Entry]) -> Vec<(MenuEntryKind, &str)> {
    list.iter().map(|e| (e.0, e.1.as_str())).collect()
}

/// The Time range submenu of `list`.
fn ranges(r: &mut Rig, list: &[Entry]) -> Vec<Entry> {
    let sub = list
        .iter()
        .find(|e| e.1 == "Time range")
        .and_then(|e| e.4.clone())
        .expect("a Time range submenu");
    entries_of(r, &sub)
}

/// A point inside the page panel's top-left padding, away from every Details value, so the
/// page menu opens without "Copy value" whatever the values are.
fn page_panel(r: &mut Rig) -> gpui_kit::Point<gpui_kit::Pixels> {
    let b = r
        .vcx
        .debug_bounds(tl_ui_perf::performance::PAGE_PANEL_ID)
        .expect("the page panel");
    b.origin + gpui_kit::point(gpui_kit::px(6.), gpui_kit::px(6.))
}

#[gpui_kit::test]
fn page_menu_common_order(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    let at = page_panel(&mut r);
    right_click(r.vcx, at);
    let list = open_menu(&mut r);
    assert_eq!(
        labels(&list),
        [
            (MenuEntryKind::Item, "Copy"),
            (MenuEntryKind::Item, "Graph Summary View"),
            (MenuEntryKind::Submenu, "Time range"),
            (MenuEntryKind::Separator, ""),
            (MenuEntryKind::Submenu, "Graph"),
            (MenuEntryKind::Submenu, "Secondary graph"),
            (MenuEntryKind::Item, "Kernel times"),
            (MenuEntryKind::Separator, ""),
            (MenuEntryKind::Submenu, "View"),
            (MenuEntryKind::Disabled, "Add tray icon graph"),
        ]
    );
    assert!(!list[1].3, "summary mode is off");
    let ranges = ranges(&mut r, &list);
    let checked: Vec<&str> = ranges
        .iter()
        .filter(|e| e.3)
        .map(|e| e.1.as_str())
        .collect();
    assert_eq!(checked, ["1 minute"], "range 60 s");
    assert_eq!(
        selected(&mut r).as_deref(),
        Some("cpu"),
        "selection unchanged"
    );
}

#[gpui_kit::test]
fn page_menu_long_ranges_enabled(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    let at = page_panel(&mut r);
    right_click(r.vcx, at);
    let list = open_menu(&mut r);
    let ranges = ranges(&mut r, &list);
    let shape: Vec<(MenuEntryKind, &str, Option<String>)> = ranges
        .iter()
        .map(|e| (e.0, e.1.as_str(), e.2.clone()))
        .collect();
    assert_eq!(
        shape,
        [
            (MenuEntryKind::Item, "30 seconds", None),
            (MenuEntryKind::Item, "1 minute", None),
            (MenuEntryKind::Item, "5 minutes", None),
            (MenuEntryKind::Item, "10 minutes", None),
        ]
    );
}

/// The View submenu of `list`.
fn view_items(r: &mut Rig, list: &[Entry]) -> Vec<Entry> {
    let sub = list
        .iter()
        .find(|e| e.1 == "View")
        .and_then(|e| e.4.clone())
        .expect("a View submenu");
    entries_of(r, &sub)
}

/// The rig has CPU, Memory, a GPU, disks and network cards, and no fan or battery.
#[gpui_kit::test]
fn view_submenu_disables_empty_types(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    let at = page_panel(&mut r);
    right_click(r.vcx, at);
    let list = open_menu(&mut r);
    let items = view_items(&mut r, &list);
    assert_eq!(
        labels(&items),
        [
            (MenuEntryKind::Item, "Processor"),
            (MenuEntryKind::Item, "Memory"),
            (MenuEntryKind::Item, "Disks"),
            (MenuEntryKind::Item, "Graphics adapters"),
            (MenuEntryKind::Item, "Network adapters"),
            (MenuEntryKind::Disabled, "Fans"),
            (MenuEntryKind::Disabled, "Batteries"),
        ]
    );
}

fn menu_is_open(r: &mut Rig) -> bool {
    r.vcx
        .update(|window, cx| open_context_menu(window, cx))
        .is_some()
}

/// jqk.37: after Esc closes the page menu, F2 selects the second card at once, with no
/// click in between.
#[gpui_kit::test]
fn page_menu_esc_keeps_keys(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
    let at = page_panel(&mut r);
    right_click(r.vcx, at);
    draw(r.vcx);
    assert!(menu_is_open(&mut r));

    r.vcx.simulate_keystrokes("escape");
    draw(r.vcx);
    assert!(!menu_is_open(&mut r), "Esc closed the menu");

    r.vcx.simulate_keystrokes("f2");
    draw(r.vcx);
    assert_ne!(
        selected(&mut r).as_deref(),
        Some("cpu"),
        "F2 ran after the menu closed"
    );
}

/// jqk.37: after a picked item closes the page menu, F2 selects the second card and Ctrl+M
/// turns summary mode on, with no click in between.
#[gpui_kit::test]
fn page_menu_pick_keeps_keys(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
    let at = page_panel(&mut r);
    right_click(r.vcx, at);
    draw(r.vcx);
    assert!(menu_is_open(&mut r));

    // "Copy", the first item.
    r.vcx.simulate_keystrokes("down enter");
    draw(r.vcx);
    assert!(!menu_is_open(&mut r), "the picked item closed the menu");

    r.vcx.simulate_keystrokes("f2");
    draw(r.vcx);
    assert_ne!(
        selected(&mut r).as_deref(),
        Some("cpu"),
        "F2 ran after the menu closed"
    );
    enter(&mut r);
}

/// jqk.37: with the keys on the device page, F2 shows another page and the keys follow
/// it, so Ctrl+M still turns summary mode on.
#[gpui_kit::test]
fn keys_follow_the_shown_page(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    assert_eq!(selected(&mut r).as_deref(), Some("cpu"));
    let at = page_panel(&mut r);
    click(r.vcx, at);
    draw(r.vcx);

    r.vcx.simulate_keystrokes("f2");
    draw(r.vcx);
    assert_ne!(selected(&mut r).as_deref(), Some("cpu"), "F2 ran");
    enter(&mut r);
}

#[gpui_kit::test]
fn jump_to_type_selects_first_visible(cx: &mut TestAppContext) {
    use tl_ui_kit::components::menu::close_context_menu;
    use tl_ui_perf::performance::DeviceKind;
    use tl_ui_perf::performance::actions::PerfJumpToType;

    let mut r = open(cx, 1600., 1000., true);
    let page = r.page.clone();
    let first_net = r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().read(cx);
        sidebar
            .visible_keys()
            .find(|k| {
                sidebar
                    .card(k)
                    .is_some_and(|c| c.row.kind == DeviceKind::Net)
            })
            .map(|k| k.0.to_string())
    });
    assert!(first_net.is_some(), "the rig has a network card");
    r.vcx.update(close_context_menu);
    r.vcx.dispatch_action(PerfJumpToType {
        kind: DeviceKind::Net,
    });
    draw(r.vcx);
    assert_eq!(selected(&mut r), first_net);

    // No fan: nothing changes.
    r.vcx.dispatch_action(PerfJumpToType {
        kind: DeviceKind::Fan,
    });
    draw(r.vcx);
    assert_eq!(selected(&mut r), first_net);
}

/// crw.2: the P1 sidebar actions work together in one app and persist only durable state.
#[gpui_kit::test]
fn sidebar_p1_flow(cx: &mut TestAppContext) {
    use tl_ui_perf::performance::actions::{
        PerfOpenCardMenu, PerfSetGraphRange, PerfToggleSidebar,
    };

    let mut r = open(cx, 1600., 1000., true);
    select(&mut r, "disk:sdb");

    // Open the selected card's menu, then run its Hide This Page action.
    r.vcx.dispatch_action(PerfOpenCardMenu);
    draw(r.vcx);
    r.vcx.update(|window, cx| {
        let menu = open_context_menu(window, cx).expect("the card menu is open");
        let hide = menu
            .read(cx)
            .entries_for_test(window)
            .into_iter()
            .find(|entry| entry.label.as_deref() == Some("Hide This Page"))
            .and_then(|entry| entry.action)
            .expect("Hide This Page has an action");
        assert!(
            hide.partial_eq(&tl_ui_perf::performance::actions::PerfSetPageVisible {
                key: key("disk:sdb"),
                visible: false,
            })
        );
    });
    r.vcx.simulate_keystrokes("down down enter");
    draw(r.vcx);
    assert_eq!(
        selected(&mut r).as_deref(),
        Some("disk:sdc"),
        "selection moves to the next card"
    );

    r.vcx.dispatch_action(PerfSetGraphRange { secs: 30 });
    draw(r.vcx);
    let page = r.page.clone();
    let ranges = r.vcx.update(|_, cx| {
        page.read(cx)
            .sidebar()
            .read(cx)
            .cards()
            .iter()
            .map(|card| card.chart.read(cx).spec.range_ns)
            .collect::<Vec<_>>()
    });
    assert!(!ranges.is_empty());
    assert!(
        ranges.iter().all(|&range| range == 30_000_000_000),
        "every sidebar chart has the 30 s range: {ranges:?}"
    );

    r.vcx.dispatch_action(PerfToggleSidebar);
    draw(r.vcx);
    let page = r.page.clone();
    let sidebar_visible = r.vcx.update(|_, cx| page.read(cx).sidebar_visible());
    assert!(!sidebar_visible, "the sidebar is hidden");

    let state = r
        .persisted_state
        .borrow()
        .clone()
        .expect("state.toml persistence ran");
    assert_eq!(state.performance.selected_device, "disk:sdc");
    assert_eq!(
        state.performance,
        tl_ui_settings::settings::PerformanceState {
            nav_width: 252,
            selected_device: "disk:sdc".to_owned(),
        },
        "state.toml has no transient sidebar visibility value"
    );
}

#[gpui_kit::test]
fn long_range_switches_tier(cx: &mut TestAppContext) {
    use tl_ui_perf::performance::actions::PerfSetGraphRange;

    let r = open(cx, 1600., 1000., true);
    let id = SeriesId {
        device: DeviceIdx(0),
        metric: MetricId::CPU_TOTAL,
    };
    let view = |id: SeriesId, period_ns: u64| {
        Arc::new(SeriesView {
            id,
            version: 1,
            period_ns,
            samples: Arc::from([Sample::valid(period_ns, 10.0)]),
        })
    };
    r.engine.publish_with(|s| {
        let mut set = SeriesSet::default();
        set.header.version = 1;
        set.series = Arc::from([view(id, 100_000_000)]);
        set.slow = Arc::from([view(
            SeriesId {
                metric: id.metric.with_tier_1hz(),
                ..id
            },
            1_000_000_000,
        )]);
        s.series = Arc::new(set);
    });
    draw(r.vcx);
    let page = r.page.clone();
    let period = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            let sidebar = page.read(cx).sidebar().read(cx);
            let card = sidebar
                .cards()
                .iter()
                .find(|c| c.row.key.0.as_ref() == "cpu")
                .expect("CPU card");
            let chart = card.chart.read(cx);
            (
                TlSettings::settings(cx).performance.graph_range_s,
                chart.resolved()[0].as_ref().map(|v| v.period_ns),
            )
        })
    };
    assert_eq!(period(r.vcx), (60, Some(100_000_000)), "60 s reads 10 Hz");

    r.vcx.dispatch_action(PerfSetGraphRange { secs: 600 });
    draw(r.vcx);
    assert_eq!(
        period(r.vcx),
        (600, Some(1_000_000_000)),
        "10 min reads 1 Hz"
    );

    r.vcx.dispatch_action(PerfSetGraphRange { secs: 300 });
    draw(r.vcx);
    assert_eq!(period(r.vcx).0, 300);
}

/// `TlMenu` drops a doubled separator, so a page without its own items has one separator
/// between the common items and View, never two.
#[gpui_kit::test]
fn page_menu_single_separator_without_page_items(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    select(&mut r, "disk:sda");
    let at = page_panel(&mut r);
    right_click(r.vcx, at);
    let list = open_menu(&mut r);
    assert_eq!(
        labels(&list),
        [
            (MenuEntryKind::Item, "Copy"),
            (MenuEntryKind::Item, "Graph Summary View"),
            (MenuEntryKind::Submenu, "Time range"),
            (MenuEntryKind::Separator, ""),
            (MenuEntryKind::Submenu, "View"),
            (MenuEntryKind::Disabled, "Add tray icon graph"),
        ]
    );
}

#[gpui_kit::test]
fn set_graph_range_updates_every_chart(cx: &mut TestAppContext) {
    use tl_ui_perf::performance::actions::PerfSetGraphRange;

    let r = open(cx, 1600., 1000., true);
    r.vcx.dispatch_action(PerfSetGraphRange { secs: 30 });
    draw(r.vcx);
    let page = r.page.clone();
    let (range_s, card_ranges) = r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().clone();
        let ranges: Vec<u64> = sidebar
            .read(cx)
            .cards()
            .iter()
            .map(|c| c.chart.read(cx).spec.range_ns)
            .collect();
        (TlSettings::settings(cx).performance.graph_range_s, ranges)
    });
    assert_eq!(range_s, 30);
    assert_eq!(card_ranges.len(), 9);
    assert!(
        card_ranges.iter().all(|&ns| ns == 30_000_000_000),
        "{card_ranges:?}"
    );
}

/// An entry without its submenu handle, and the entries of its submenu.
type Deep = (
    MenuEntryKind,
    String,
    Option<String>,
    bool,
    Vec<(MenuEntryKind, String, Option<String>, bool)>,
);

fn deep(r: &mut Rig, list: Vec<Entry>) -> Vec<Deep> {
    list.into_iter()
        .map(|(kind, label, tip, checked, sub)| {
            let sub = sub
                .map(|sub| entries_of(r, &sub))
                .unwrap_or_default()
                .into_iter()
                .map(|e| (e.0, e.1, e.2, e.3))
                .collect();
            (kind, label, tip, checked, sub)
        })
        .collect()
}

/// Right-click on a tile opens the full page menu of its device (UI-PRF-016): the same
/// entries, marks, tooltips and submenus as the page menu, with only the Graph Summary View
/// mark on. The page's own items run on the page.
#[gpui_kit::test]
fn summary_tile_right_click_page_menu(cx: &mut TestAppContext) {
    use tl_ui_kit::components::menu::close_context_menu;
    use tl_ui_perf::performance::actions::PerfSetGraphRange;

    let mut r = open(cx, 1600., 1000., true);
    let at = page_panel(&mut r);
    right_click(r.vcx, at);
    let list = open_menu(&mut r);
    let mut page_menu = deep(&mut r, list);
    r.vcx.update(close_context_menu);
    let view = enter(&mut r);
    let tile = r.vcx.debug_bounds("perf-summary-tile-0").expect("CPU tile");
    right_click(r.vcx, tile.center());
    draw(r.vcx);
    assert!(
        summary(&mut r).is_some(),
        "right-click does not leave summary mode"
    );
    assert!(r.vcx.update(|_, cx| view.read(cx).has_menu_page()));
    let list = open_menu(&mut r);
    let tile_menu = deep(&mut r, list);
    assert_eq!(page_menu[1].1, "Graph Summary View");
    assert!(!page_menu[1].3 && tile_menu[1].3, "checked in summary mode");
    page_menu[1].3 = true;
    assert_eq!(tile_menu, page_menu);

    // "Kernel times" from the tile menu runs on the CPU page.
    let kernel = |r: &mut Rig| {
        r.vcx
            .update(|_, cx| TlSettings::settings(cx).performance.cpu_kernel)
    };
    let before = kernel(&mut r);
    let pos = tile_menu
        .iter()
        .filter(|e| matches!(e.0, MenuEntryKind::Item | MenuEntryKind::Submenu))
        .position(|e| e.1 == "Kernel times")
        .expect("Kernel times is enabled");
    let keys = vec!["down"; pos + 1].join(" ");
    r.vcx.simulate_keystrokes(&keys);
    r.vcx.simulate_keystrokes("enter");
    draw(r.vcx);
    assert_eq!(kernel(&mut r), !before, "the page toggled kernel times");
    assert!(summary(&mut r).is_some());
    assert!(
        r.vcx
            .update(|window, cx| open_context_menu(window, cx).is_none()),
        "the menu closed"
    );
    assert!(
        !r.vcx.update(|_, cx| view.read(cx).has_menu_page()),
        "the page is drawn only while the menu is open"
    );

    // The common items still run on the summary view.
    let focus = r.vcx.update(|_, cx| view.read(cx).focus_handle(cx));
    r.vcx.update(|window, cx| focus.focus(window, cx));
    r.vcx.dispatch_action(PerfSetGraphRange { secs: 30 });
    draw(r.vcx);
    let ranges: Vec<u64> = r.vcx.update(|_, cx| {
        view.read(cx)
            .tiles
            .iter()
            .map(|t| t.chart.read(cx).spec.range_ns)
            .collect()
    });
    assert!(ranges.iter().all(|&ns| ns == 30_000_000_000), "{ranges:?}");
    assert!(summary(&mut r).is_some());
}

/// Publishes a CPU speed of 4631 MHz and returns the bounds of the CPU page's Speed value.
fn speed_value(r: &mut Rig) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    use tl_ui_perf::performance::cpu::CpuPage;
    use tl_ui_perf::performance::cpu_details::ROW_SPEED;
    use tl_ui_perf::performance::page::DevicePage as _;

    r.engine.publish_with(|s| {
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.cpu.freq_mhz = tl_model::system::Metric::valid(4631);
    });
    draw(r.vcx);
    let page = r.page.clone();
    let n = r.vcx.update(|_, cx| {
        let cpu = page
            .read(cx)
            .page(&key("cpu"))
            .expect("CPU page built")
            .clone()
            .downcast::<CpuPage>()
            .expect("a CpuPage");
        cpu.read(cx)
            .details()
            .rows
            .iter()
            .filter(|row| !row.hidden)
            .position(|row| row.label == ROW_SPEED)
            .expect("a Speed row")
    });
    let selector: &'static str = format!("TlKv-value-{n}").leak();
    r.vcx.debug_bounds(selector).expect("Speed value drawn")
}

fn clipboard(r: &mut Rig) -> Option<String> {
    r.vcx.read_from_clipboard().and_then(|item| item.text())
}

/// Details values are selectable text (U-46): a drag over the Speed value and Ctrl+C copy
/// the value alone.
#[gpui_kit::test]
fn value_drag_select_ctrl_c_copies_value(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    let value = speed_value(&mut r);
    let (from, to) = (
        gpui_kit::point(value.left() + px(1.), value.center().y),
        gpui_kit::point(value.right() - px(1.), value.center().y),
    );
    r.vcx
        .simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    r.vcx
        .simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    r.vcx
        .simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    draw(r.vcx);
    r.vcx.simulate_keystrokes("ctrl-c");
    r.vcx.run_until_parked();
    assert_eq!(clipboard(&mut r).as_deref(), Some("4631 MHz"));
}

/// Without a selection, Ctrl+C on the page goes on to the page's Copy (E24-016), which
/// copies the page text.
#[gpui_kit::test]
fn copy_without_selection_goes_to_page_copy(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    let value = speed_value(&mut r);
    // A click focuses the grid but selects nothing.
    r.vcx.simulate_click(value.center(), Modifiers::none());
    draw(r.vcx);
    r.vcx.simulate_keystrokes("ctrl-c");
    r.vcx.run_until_parked();
    let text = clipboard(&mut r).expect("the page copied");
    assert!(text.starts_with("Processor\n"), "the page copy ran: {text}");
}

/// A right-click on a value opens the page menu with "Copy value" first; it copies the
/// value.
#[gpui_kit::test]
fn copy_value_context_item(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    let value = speed_value(&mut r);
    r.vcx
        .update(|_, cx| tl_ui_kit::components::menu::bind_keys(cx));
    right_click(r.vcx, value.center());
    let list = open_menu(&mut r);
    assert_eq!(
        labels(&list)[..2],
        [
            (MenuEntryKind::Item, "Copy value"),
            (MenuEntryKind::Item, "Copy"),
        ]
    );
    r.vcx.simulate_keystrokes("down enter");
    r.vcx.run_until_parked();
    assert_eq!(clipboard(&mut r).as_deref(), Some("4631 MHz"));

    // A right-click off the values has no "Copy value".
    let at = page_panel(&mut r);
    right_click(r.vcx, at);
    let list = open_menu(&mut r);
    assert_eq!(labels(&list)[0], (MenuEntryKind::Item, "Copy"));
}

/// Publishes the fake CPU values of the copy tests: a model name, every Details value
/// Valid, and the caches.
fn publish_cpu(r: &mut Rig, temp: Validity) {
    use tl_model::snapshot::{
        CacheInfo, CacheLevelInfo, CpuStaticInfo, InvField, SystemSnapshot, VirtState,
    };

    fn inv<T>(value: T) -> InvField<T> {
        InvField {
            value,
            validity: Validity::Valid,
        }
    }
    fn level(instances: u16, instance_kib: u64) -> Option<CacheLevelInfo> {
        Some(CacheLevelInfo {
            instances,
            instance_kib,
            total_kib: u64::from(instances) * instance_kib,
        })
    }
    r.engine.publish_with(|s| {
        let sys: &mut SystemSnapshot = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.header.validity = Validity::Valid;
        let c = &mut sys.cpu;
        c.util_pct = Metric::valid(18.6);
        c.kernel_pct = Metric::valid(2.3);
        c.freq_mhz = Metric::valid(4631);
        c.base_mhz = Metric::valid(4200);
        c.temp_c = if temp == Validity::Valid {
            Metric::valid(52.0)
        } else {
            Metric::with(temp)
        };
        c.processes = Metric::valid(295);
        c.threads = Metric::valid(5596);
        c.handles = Metric::valid(253_248);
        c.load_avg = Metric::valid([1.24, 0.98, 0.87]);
        c.sockets = Metric::valid(1);
        let dd = Arc::make_mut(&mut s.device_details);
        dd.header.version += 1;
        dd.cpu = Arc::new(CpuStaticInfo {
            cores: 8,
            logical: 16,
            caches: CacheInfo {
                l1d: level(8, 32),
                l1i: level(8, 32),
                l2: level(8, 1024),
                l3: level(1, 98_304),
                ..CacheInfo::default()
            },
            driver: inv(Arc::from("amd-pstate-epp")),
            governor: inv(Arc::from("powersave")),
            epp: inv(Arc::from("balance_performance")),
            platform_profile: inv(Arc::from("balanced")),
            virtualization: inv(VirtState::EnabledKvm),
            running_in_vm: inv(Arc::from("")),
            microcode: inv(Arc::from("0xa601206")),
            rapl: None,
            ..CpuStaticInfo::default()
        });
        let invt = Arc::make_mut(&mut s.inventory);
        invt.header.version += 1;
        invt.processor.name = inv(Arc::from("AMD Ryzen 7 7800X3D 8-Core Processor"));
    });
    draw(r.vcx);
}

/// The Up time value the CPU page shows (it comes from the real boot clock).
fn up_time(r: &mut Rig) -> String {
    use tl_ui_perf::performance::cpu::CpuPage;
    use tl_ui_perf::performance::cpu_details::ROW_UP_TIME;
    use tl_ui_perf::performance::page::DevicePage as _;

    let page = r.page.clone();
    r.vcx.update(|_, cx| {
        let cpu = page
            .read(cx)
            .page(&key("cpu"))
            .expect("CPU page built")
            .clone()
            .downcast::<CpuPage>()
            .expect("a CpuPage");
        cpu.read(cx)
            .details()
            .rows
            .iter()
            .find(|row| row.label == ROW_UP_TIME)
            .expect("an Up time row")
            .value
            .to_string()
    })
}

#[gpui_kit::test]
fn copy_text_cpu_format(cx: &mut TestAppContext) {
    use tl_ui_perf::performance::actions::PerfCopyDeviceInfo;

    let mut r = open(cx, 1600., 1000., true);
    publish_cpu(&mut r, Validity::Valid);
    let up = up_time(&mut r);
    r.vcx.dispatch_action(PerfCopyDeviceInfo);
    r.vcx.run_until_parked();
    let expected = format!(
        "Processor\nAMD Ryzen 7 7800X3D 8-Core Processor\n\
         \x20   Utilization: 18.6%\n\
         \x20   Kernel time: 2.3%\n\
         \x20   Speed: 4631 MHz\n\
         \x20   Processes: 295\n\
         \x20   Threads: 5\u{2009}596\n\
         \x20   Handles: 253\u{2009}248\n\
         \x20   Up time: {up}\n\
         \x20   Base speed: 4200 MHz\n\
         \x20   Sockets: 1\n\
         \x20   Temperature: 52 \u{b0}C\n\
         \x20   Load average: 1.24 0.98 0.87\n\
         \x20   Cores: 8\n\
         \x20   Logical processors: 16\n\
         \x20   Virtualization: Enabled (KVM available)\n\
         \x20   Running in VM: No\n\
         \x20   Security features: None\n\
         \x20   Hypervisor: None\n\
         \x20   L1 cache: 512 KiB\n\
         \x20   L2 cache: 8 MiB\n\
         \x20   L3 cache: 96 MiB\n\
         \x20   Cpufreq driver: amd-pstate-epp\n\
         \x20   Governor: powersave\n\
         \x20   Power preference: balance_performance\n\
         \x20   Platform profile: balanced\n\
         \x20   Microcode: 0xa601206\n"
    );
    assert_eq!(clipboard(&mut r).as_deref(), Some(expected.as_str()));
}

#[gpui_kit::test]
fn copy_text_fallback_words(cx: &mut TestAppContext) {
    use tl_ui_perf::performance::actions::PerfCopyDeviceInfo;

    let mut r = open(cx, 1600., 1000., true);
    publish_cpu(&mut r, Validity::Restricted);
    r.vcx.dispatch_action(PerfCopyDeviceInfo);
    r.vcx.run_until_parked();
    let text = clipboard(&mut r).expect("copied");
    assert!(
        text.lines().any(|l| l == "    Temperature: Restricted"),
        "{text}"
    );
}

/// A right-click on a value opens only the page menu: the grid's own menu (Copy value,
/// Copy row, Copy section) stays shut, and the page menu has the focus.
#[gpui_kit::test]
fn right_click_value_opens_one_menu(cx: &mut TestAppContext) {
    let mut r = open(cx, 1600., 1000., true);
    let value = speed_value(&mut r);
    r.vcx
        .update(|_, cx| tl_ui_kit::components::menu::bind_keys(cx));
    right_click(r.vcx, value.center());
    draw(r.vcx);
    let menu = r
        .vcx
        .update(|window, cx| open_context_menu(window, cx))
        .expect("the page menu is open");
    let focused = r
        .vcx
        .update(|window, cx| menu.read(cx).focus_handle(cx).contains_focused(window, cx));
    assert!(
        focused,
        "the page menu has the focus, no other menu took it"
    );
    // The second item of the grid's menu is "Copy row"; the page menu's is "Copy", which
    // copies the page.
    r.vcx.simulate_keystrokes("down down enter");
    r.vcx.run_until_parked();
    let text = clipboard(&mut r).expect("the page Copy ran");
    assert!(
        text.starts_with("Processor\n"),
        "no grid menu item ran: {text}"
    );
}
