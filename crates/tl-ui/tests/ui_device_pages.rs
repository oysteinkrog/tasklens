//! Device pages through the real Performance tab: `build_page` makes each page kind once, and
//! each page's demand reaches the engine while it is shown (E25-010, UI-PRF-020; ADR-008,
//! ADR-009).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AnyView, App, AppContext as _, Entity, SharedString, TestAppContext, VisualTestContext, px,
    size,
};
use tl_model::client::EngineClient;
use tl_model::demand::Demand;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::DeviceIdx;
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::cpu::CpuPage;
use tl_ui::pages::performance::disk::DiskPage;
use tl_ui::pages::performance::gpu::GpuPage;
use tl_ui::pages::performance::memory::MemoryPage;
use tl_ui::pages::performance::net::NetPage;
use tl_ui::pages::performance::page::{DevicePage, PlaceholderPage, build_page};
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow, PerformancePage};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;

const GPU: &str = "gpu:pci:0000:2d:00.0";
const DISKS: [&str; 4] = ["disk:sda", "disk:sdb", "disk:sdc", "disk:sdd"];

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
    entry(
        idx,
        DeviceIdentity::Disk {
            major: 8,
            minor: idx.into(),
            name: Arc::from(name),
            wwn_or_serial: None,
        },
        None,
    )
}

fn net(idx: u16, name: &str, kind: NetKind) -> DeviceEntry {
    entry(
        idx,
        DeviceIdentity::Net {
            netns_ino: 1,
            ifindex: idx.into(),
            name: Arc::from(name),
        },
        Some(kind),
    )
}

/// CPU, Memory, a GPU, four disks, a wired interface and a VPN tunnel.
fn catalog() -> Vec<DeviceEntry> {
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

fn install(cx: &mut TestAppContext) -> Arc<FakeEngine> {
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
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        tl_ui::theme::metrics::init_metrics(&AppearanceSettings::default(), cx);
        init_strings(cx, "en-US");
        kv_grid::init(cx);
        cx.set_global(EngineGlobal(client));
        cx.set_global(BootSettings {
            settings: Settings::default(),
            state: UiState::default(),
            persist: None,
            startup_toasts: Vec::new(),
        });
        TlSettings::init(cx);
    });
    engine
}

/// The Performance tab as the window root, 1280 × 800, CPU selected.
fn open(cx: &mut TestAppContext) -> Rig<'_> {
    let engine = install(cx);
    let client: Arc<dyn EngineClient> = engine.clone();
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let (page, vcx) = cx.add_window_view(move |_, cx| PerformancePage::new(hub, demand, cx));
    vcx.simulate_scale_factor_change(1.0);
    vcx.simulate_resize(size(px(1280.), px(800.)));
    draw(vcx);
    Rig { page, engine, vcx }
}

fn key(s: &str) -> DeviceKey {
    DeviceKey(Arc::from(s))
}

/// Selects the card of `k` and lets the demand flush reach the engine.
fn select(r: &mut Rig, k: &str) {
    let page = r.page.clone();
    let k = key(k);
    r.vcx.update(|_, cx| {
        let sidebar = page.read(cx).sidebar().clone();
        sidebar.update(cx, |s, cx| s.select(k, cx));
    });
    draw(r.vcx);
    r.vcx.executor().advance_clock(Duration::from_millis(200));
    draw(r.vcx);
}

fn view_of(r: &mut Rig, k: &str) -> AnyView {
    let page = r.page.clone();
    let k = key(k);
    r.vcx
        .update(|_, cx| page.read(cx).page(&k).expect("page built").clone())
}

/// The header of the device page behind `view`, through the `DevicePage` trait.
fn header_of(view: &AnyView, cx: &App) -> (SharedString, SharedString) {
    fn read<T: DevicePage>(view: &AnyView, cx: &App) -> Option<(SharedString, SharedString)> {
        view.clone()
            .downcast::<T>()
            .ok()
            .map(|e| e.read(cx).header())
    }
    read::<CpuPage>(view, cx)
        .or_else(|| read::<MemoryPage>(view, cx))
        .or_else(|| read::<DiskPage>(view, cx))
        .or_else(|| read::<NetPage>(view, cx))
        .or_else(|| read::<GpuPage>(view, cx))
        .or_else(|| read::<PlaceholderPage>(view, cx))
        .expect("a known page type")
}

fn last_demand(r: &Rig) -> Demand {
    r.engine
        .demands()
        .last()
        .cloned()
        .expect("a demand was sent")
}

#[gpui_kit::test]
fn build_page_all_p1_kinds(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let keys: Vec<String> = {
        let page = r.page.clone();
        r.vcx.update(|_, cx| {
            let sidebar = page.read(cx).sidebar().clone();
            sidebar
                .read(cx)
                .visible_keys()
                .map(|k| k.0.to_string())
                .collect()
        })
    };
    assert_eq!(keys.len(), 9, "{keys:?}");
    for k in &keys {
        select(&mut r, k);
    }
    // A second visit builds nothing.
    select(&mut r, DISKS[0]);
    let page = r.page.clone();
    assert_eq!(r.vcx.update(|_, cx| page.read(cx).page_count()), 9);

    let expect = [
        ("cpu", "Processor"),
        ("memory", "Memory"),
        (DISKS[0], "Disk 0"),
        (DISKS[1], "Disk 1"),
        (DISKS[2], "Disk 2"),
        (DISKS[3], "Disk 3"),
        ("net:enp38s0", "enp38s0"),
        ("net:tailscale0", "tailscale0"),
        (GPU, "Graphics adapter"),
    ];
    for (k, word) in expect {
        let view = view_of(&mut r, k);
        let header = r.vcx.update(|_, cx| header_of(&view, cx));
        assert_eq!(header.0.as_ref(), word, "{k}");
    }
    let memory = view_of(&mut r, "memory");
    assert!(memory.downcast::<MemoryPage>().is_ok());
    let net = view_of(&mut r, "net:enp38s0");
    assert!(net.downcast::<NetPage>().is_ok());
    let disk = view_of(&mut r, DISKS[0]);
    assert!(disk.downcast::<DiskPage>().is_ok());
    assert!(view_of(&mut r, GPU).downcast::<GpuPage>().is_ok());
}

#[gpui_kit::test]
fn page_switch_moves_demand(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, DISKS[0]);
    let disk = view_of(&mut r, DISKS[0])
        .downcast::<DiskPage>()
        .expect("a DiskPage");
    assert!(r.vcx.update(|_, cx| disk.read(cx).is_active()));

    select(&mut r, "memory");
    let memory = view_of(&mut r, "memory")
        .downcast::<MemoryPage>()
        .expect("a MemoryPage");
    let (disk_on, memory_on) = r
        .vcx
        .update(|_, cx| (disk.read(cx).is_active(), memory.read(cx).is_active()));
    assert!(!disk_on, "the Disk page withdrew its part");
    assert!(memory_on);
    let d = last_demand(&r);
    // The sidebar keeps disks on for its disk cards; the Memory page adds memory.
    assert!(d.any_surface_visible && d.memory && d.disks);
    assert!(!d.cpu_fast, "the CPU page is not shown");
}

/// Each page, opened through the tab, sends its demand part and the engine receives it.
#[gpui_kit::test]
fn pages_demand_reaches_engine(cx: &mut TestAppContext) {
    let mut r = open(cx);
    r.vcx.executor().advance_clock(Duration::from_millis(200));
    draw(r.vcx);
    assert!(last_demand(&r).cpu_fast, "the CPU page is shown first");

    for (k, check) in [
        ("memory", (|d: &Demand| d.memory) as fn(&Demand) -> bool),
        (DISKS[1], |d: &Demand| d.disks),
        ("net:enp38s0", |d: &Demand| d.nets),
        (GPU, |d: &Demand| d.gpus),
    ] {
        select(&mut r, k);
        let d = last_demand(&r);
        assert!(d.any_surface_visible, "{k}");
        assert!(check(&d), "{k}: {d:?}");
        assert!(!d.cpu_fast, "{k}: the CPU page left");
    }
    select(&mut r, "cpu");
    assert!(last_demand(&r).cpu_fast, "back on the CPU page");
}

#[gpui_kit::test]
fn build_page_placeholder_for_fan_before_p2(cx: &mut TestAppContext) {
    let engine = install(cx);
    let client: Arc<dyn EngineClient> = engine;
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    let k = key("fan:nct6799:2");
    let row = DeviceRow {
        key: k.clone(),
        kind: DeviceKind::Fan,
        idx: DeviceIdx(9),
        generation: 0,
        net_class: None,
    };
    let view = cx.update(|cx| build_page(&k, &row, &hub, &demand, cx));
    assert!(view.downcast::<PlaceholderPage>().is_ok());
}
