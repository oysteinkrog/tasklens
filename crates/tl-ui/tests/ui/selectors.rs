//! Debug selectors (tasklens-6swdr.11, ADR-019). Every tl-ui debug selector goes through
//! `tl_ui_base::selector::TlSelector`, so a test can count how many painted elements share one.
//! GPUI keeps one `debug_bounds` entry per selector, so a second element with the same selector
//! hides the first from every test that looks it up. The page sweeps here check the Processes
//! tab and each Performance resource page; the Overview sweep is
//! `overview::page::overview_widget_selectors_unique_on_the_page` in the tl-ui-perf `perf` binary.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use tl_model::client::EngineClient;
use tl_model::command::Tab;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::{DeviceIdx, NodeId};
use tl_model::process::ProcessRecord;
use tl_model::snapshot::{ComponentHeader, ProcessSnapshot};
use tl_model::table::{ProcessTableView, TableRow};
use tl_model::validity::Validity;
use tl_ui::app::{BootSettings, EngineGlobal, StartOptions};
use tl_ui::window::main::{self as shell, MainWindow, main_window_view, open_main_window};
use tl_ui_base::demand::{DemandAggregator, GlobalDemand};
use tl_ui_base::hub::{GlobalHub, SnapshotHub};
use tl_ui_base::i18n::init_strings;
use tl_ui_base::selector::TlSelector as _;
use tl_ui_base::settings_global::TlSettings;
use tl_ui_kit::components::kv_grid;
use tl_ui_perf::performance::{DeviceKey, PerformancePage};
use tl_ui_settings::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_ui_testkit::fake_engine::FakeEngine;

use crate::common::{key, record};

// The selector counts moved to the page host (tasklens-zljr5.41), so the page crates' tests use
// them too. The re-export keeps `crate::selectors::` working for the shell tests.
pub use tl_ui_pagehost::selectors::{assert_none_drawn_twice, drawn_selectors};

/// No file in tl-ui or the other tl-ui crates calls GPUI's `debug_selector` outside the helper
/// module, so the drawn-selector count sees every selector.
#[test]
fn every_selector_goes_through_the_helper() {
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("source file");
                // A call may put spaces or a line break between the name and its `(`.
                for (at, _) in text.match_indices("debug_selector") {
                    let rest = text[at + "debug_selector".len()..].trim_start();
                    if rest.starts_with('(') {
                        let line = text[..at].lines().count().max(1);
                        out.push(format!("{}:{}", path.display(), line));
                    }
                }
            }
        }
    }
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let helper = crates.join("tl-ui-base/src/selector.rs");
    let text = std::fs::read_to_string(&helper).expect("helper module");
    assert!(
        text.contains(".debug_selector("),
        "the helper module sets the selector"
    );
    let mut calls = Vec::new();
    for krate in std::fs::read_dir(&crates).expect("crates dir") {
        let src = krate.expect("dir entry").path().join("src");
        let ui = src
            .parent()
            .and_then(|dir| dir.file_name())
            .is_some_and(|name| name.to_string_lossy().starts_with("tl-ui"));
        if ui && src.is_dir() {
            walk(&src, &mut calls);
        }
    }
    let helper = format!("{}:", helper.display());
    calls.retain(|c| !c.starts_with(&helper));
    assert!(
        calls.is_empty(),
        "call `tl_selector` (tl_ui_base::selector) instead of `debug_selector`: {calls:#?}"
    );
}

/// A view that builds one selector without painting it and paints another one twice.
struct Twice;

impl Render for Twice {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Built but never painted.
        let _dropped = div().tl_selector(|| "unpainted".into());
        div()
            .size_full()
            .child(div().size_4().tl_selector(|| "twice".into()))
            .child(div().size_4().tl_selector(|| "twice".into()))
            .child(div().size_4().tl_selector(|| "once".into()))
            .child(div().size_4().id("x").tl_selector(|| "stateful".into()))
    }
}

/// The count is of painted elements: a selector that is built but not painted does not count,
/// and plain lookups work again after the count.
#[gpui_kit::test]
fn drawn_selectors_count_painted_elements(cx: &mut TestAppContext) {
    let (_, vcx) = cx.add_window_view(|_, _| Twice);
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let drawn = drawn_selectors(vcx);
    let want: BTreeMap<String, usize> = [("once", 1), ("stateful", 1), ("twice", 2)]
        .into_iter()
        .map(|(s, n)| (s.to_owned(), n))
        .collect();
    assert_eq!(drawn, want);
    assert!(vcx.debug_bounds("once").is_some(), "plain selector back");
    // Twice in a row gives the same count: the tagged frame leaves nothing behind.
    assert_eq!(drawn_selectors(vcx), want);
}

/// The check names each selector painted more than once.
#[gpui_kit::test]
#[should_panic(expected = "1 selectors drawn more than once: [(\"twice\", 2)]")]
fn assert_none_drawn_twice_names_the_repeats(cx: &mut TestAppContext) {
    let (_, vcx) = cx.add_window_view(|_, _| Twice);
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert_none_drawn_twice(vcx, "Twice");
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
        hot_c: None,
    }
}

/// CPU, memory, a GPU, two disks and a wired interface.
fn catalog() -> Vec<DeviceEntry> {
    let disk = |idx: u16, name: &str| DeviceIdentity::Disk {
        major: 8,
        minor: idx.into(),
        name: Arc::from(name),
        wwn_or_serial: None,
    };
    vec![
        entry(0, DeviceIdentity::CpuPackage, None),
        entry(1, DeviceIdentity::Memory, None),
        entry(
            2,
            DeviceIdentity::Gpu {
                uuid: [0; 16],
                pci_bus_id: Arc::from("0000:2d:00.0"),
            },
            None,
        ),
        entry(3, disk(3, "sda"), None),
        entry(4, disk(4, "sdb"), None),
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

/// The resource pages the sweep opens, by sidebar key.
const PAGES: [&str; 6] = [
    "cpu",
    "memory",
    "gpu:pci:0000:2d:00.0",
    "disk:sda",
    "disk:sdb",
    "net:eth0",
];

struct Rig<'a> {
    main: Entity<MainWindow>,
    engine: Arc<FakeEngine>,
    vcx: &'a mut VisualTestContext,
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

/// The real main window at 1200 × 900, with the real Processes and Performance pages.
fn open(cx: &mut TestAppContext) -> Rig<'_> {
    let mut settings = Settings::default();
    settings.motion.tab_switch_ms = 0;
    let engine = FakeEngine::new();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 1,
            entries: Arc::from(catalog()),
        });
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
        tl_ui_table::table::actions::bind_keys_table(cx);
        cx.set_global(EngineGlobal(Arc::clone(&client)));
        cx.set_global(BootSettings {
            settings,
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        tl_ui_base::action_tracker::install(Arc::clone(&client), Some(hub.clone()), cx);
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        cx.set_global(GlobalHub(hub));
        cx.set_global(GlobalDemand(demand));
        tl_ui_kit::components::toast::init(cx);
        tl_ui_kit::actions::bind_keys_global(cx);
        tl_ui_kit::components::menu::bind_keys(cx);
        tl_ui_perf::performance::actions::bind_keys_performance(cx);
        cx.set_global(StartOptions::default());
        tl_ui::window::menubar::init(cx);
        shell::init(cx);
    });
    let handle = cx.update(open_main_window).expect("the main window opens");
    let vcx = VisualTestContext::from_window(handle.into(), cx).into_mut();
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1200.), px(900.)));
    vcx.update(|window, _| window.activate_window());
    draw(vcx);
    let main = vcx.update(|_, cx| main_window_view(cx).expect("main window open"));
    Rig { main, engine, vcx }
}

fn select_tab(r: &mut Rig, tab: Tab) {
    r.vcx
        .dispatch_action(tl_ui_kit::actions::SelectTab(tab as usize));
    draw(r.vcx);
}

/// Six sleeping processes, one row each.
fn publish_processes(engine: &FakeEngine) {
    let records: Vec<ProcessRecord> = (10..16).map(record).collect();
    let rows: Vec<TableRow> = (10..16u32)
        .enumerate()
        .map(|(i, pid)| TableRow {
            node: NodeId::Process(key(pid)),
            depth: 0,
            expanded: false,
            has_children: false,
            record: Some(i as u32),
            group: None,
            descendants: 0,
            subtree: None,
        })
        .collect();
    engine.publish_with(|s| {
        s.table = Arc::new(ProcessTableView {
            query_generation: 1,
            header: ComponentHeader {
                version: s.table.header.version + 1,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: rows.into(),
            ..ProcessTableView::default()
        });
        s.processes = Arc::new(ProcessSnapshot {
            records: records.into(),
            ..ProcessSnapshot::default()
        });
    });
}

/// The Processes tab with its minigraph cards and six process rows draws no selector twice.
#[gpui_kit::test]
fn processes_tab_draws_no_selector_twice(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select_tab(&mut r, Tab::Processes);
    publish_processes(&r.engine);
    draw(r.vcx);
    let drawn = assert_none_drawn_twice(r.vcx, "Processes");
    assert!(drawn.contains_key("ProcessesPage"), "the page is drawn");
    // Count roots only: a card is `MinigraphCard-<id>` (its parts add `-value`, `-label`, ...),
    // and a row's name cell is `TlNameCell-<row>-<col>` (its parts are `TlNameCell-primary-...`).
    let cards = drawn
        .keys()
        .filter_map(|k| k.strip_prefix("MinigraphCard-"))
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        .count();
    assert!(cards >= 6, "the minigraph cards are drawn: {drawn:?}");
    let rows: std::collections::BTreeSet<usize> = drawn
        .keys()
        .filter_map(|k| k.strip_prefix("TlNameCell-"))
        .filter_map(|rest| rest.split('-').next()?.parse().ok())
        .collect();
    assert!(rows.len() >= 6, "the six rows are drawn: {drawn:?}");
}

/// Each Performance resource page draws no selector twice.
#[gpui_kit::test]
fn performance_pages_draw_no_selector_twice(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select_tab(&mut r, Tab::Performance);
    let page = r
        .vcx
        .read_entity(&r.main, |m, _| {
            m.page_entity::<PerformancePage>(Tab::Performance)
        })
        .expect("PerformancePage built");
    for k in PAGES {
        let key = DeviceKey(Arc::from(k));
        r.vcx.update(|_, cx| {
            let sidebar = page.read(cx).sidebar().clone();
            sidebar.update(cx, |s, cx| s.select(key.clone(), cx));
        });
        draw(r.vcx);
        let selected = r
            .vcx
            .update(|_, cx| page.read(cx).selected().map(|k| k.0.to_string()));
        assert_eq!(selected.as_deref(), Some(k), "{k} selected");
        assert_none_drawn_twice(r.vcx, k);
    }
}
