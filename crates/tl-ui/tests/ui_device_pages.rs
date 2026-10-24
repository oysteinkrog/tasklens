//! Device pages through the real Performance tab: `build_page` makes each page kind once, and
//! each page's demand reaches the engine while it is shown (E25-010, UI-PRF-020; ADR-008,
//! ADR-009).
//! Run with `--features test-hooks`.
#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AnyView, App, AppContext as _, Entity, Focusable as _, SharedString, TestAppContext,
    VisualTestContext, px, size,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, DeviceCommand};
use tl_model::demand::Demand;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::DeviceIdx;
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{
    BatteryDetails, DeviceDetailsSnapshot, DiskDetails, DisplayConnector, DisplayMode,
    GraphicsAdapter, InvField, InventorySnapshot, MemoryDetails, MemoryModule, SwapDevice,
    SwapKindModel, ZramDetail, ZswapInfo,
};
use tl_model::system::{
    FanControlMode, MemoryTotals, Metric, SensorKind, SensorReading, ZramTotals,
};
use tl_model::validity::Validity;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::actions::PerfSetGraphRange;
use tl_ui::pages::performance::aggregate::{
    AggregateDiskPage, AggregateGpuPage, ROW_ADAPTERS, ROW_DISKS, ROW_TYPE as AGG_ROW_TYPE,
};
use tl_ui::pages::performance::cpu::{
    CpuPage, PageSettingsView as CpuPageSettingsView, overlaid_spec, stacked_spec,
};
use tl_ui::pages::performance::disk::{
    DiskPage, ROW_ACTIVE, ROW_CAPACITY, ROW_READ, ROW_RESPONSE, ROW_WRITE,
};
use tl_ui::pages::performance::gpu::{
    GpuPage, ROW_DECODE, ROW_DEDICATED, ROW_ENCODE, ROW_RESERVED, ROW_SHARED, ROW_TEMP_SOURCE,
    ROW_TEMPERATURE, ROW_UTILIZATION,
};
use tl_ui::pages::performance::memory::{
    CompressedBlock, CompressedInputs, MemoryPage, ROW_AVAILABLE, ROW_CACHED, ROW_COMMITTED,
    ROW_COMPRESSED, ROW_FORM_FACTOR, ROW_HUGE_PAGES, ROW_IN_USE, ROW_RESERVED as MEM_ROW_RESERVED,
    ROW_SAVINGS, ROW_SLAB_RECLAIMABLE, ROW_SLAB_UNRECLAIMABLE, ROW_SLOTS, ROW_SPEED,
    ROW_SWAP_AVAILABLE, ROW_SWAP_USED, ROW_TYPE, ROW_ZRAM_ALGORITHM, ROW_ZRAM_COMPRESSED,
    ROW_ZRAM_DEVICE, ROW_ZRAM_MEM_USED, ROW_ZRAM_RATIO, ROW_ZRAM_SIZE, ROW_ZRAM_STORED,
    ROW_ZSWAP_ENABLED, ROW_ZSWAP_POOL, SWAP_TABLE_ID, SwapColumn, ZRAM_GRID_ID, ZSWAP_GRID_ID,
    update_compressed_block, update_memory_details_modules,
};
use tl_ui::pages::performance::net::{
    NetPage, ROW_DISPLAY_UNIT, ROW_DOWNLOAD, ROW_TOTAL_DOWN, ROW_TOTAL_UP, ROW_UPLOAD,
};
use tl_ui::pages::performance::page::{DevicePage, PlaceholderPage, build_page};
use tl_ui::pages::performance::sensors::{BatteryPage, FanPage, local_midnights};
use tl_ui::pages::performance::{
    DeviceKey, DeviceKind, DeviceRow, PerformancePage, PerformanceSettingsView, sidebar_order,
};
use tl_ui::settings_global::TlSettings;
use tl_ui::testing::fake_engine::FakeEngine;
use tl_ui::theme::metrics;

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
        hot_c: None,
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

/// Default settings with the Overview card hidden (D-022), so CPU is the first page.
fn hidden_overview() -> Settings {
    let mut s = Settings::default();
    s.performance.device_overrides.insert(
        tl_ui::pages::performance::overview::OVERVIEW_KEY.to_owned(),
        tl_model::settings::DeviceVisibility::Hide,
    );
    s
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
            settings: hidden_overview(),
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
        .or_else(|| read::<AggregateDiskPage>(view, cx))
        .or_else(|| read::<AggregateGpuPage>(view, cx))
        .or_else(|| read::<MemoryPage>(view, cx))
        .or_else(|| read::<DiskPage>(view, cx))
        .or_else(|| read::<NetPage>(view, cx))
        .or_else(|| read::<GpuPage>(view, cx))
        .or_else(|| read::<FanPage>(view, cx))
        .or_else(|| read::<BatteryPage>(view, cx))
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

fn assert_rows<T: DevicePage>(page: &T, expected: &[&str]) {
    let labels: Vec<_> = page.details().rows.iter().map(|row| row.label).collect();
    for label in expected {
        assert!(
            labels.contains(label),
            "missing Details row {label}: {labels:?}"
        );
    }
}

/// Every P1 device page renders through the real 1280 x 800 Performance tab with its header,
/// charts and Details rows present.
#[gpui_kit::test]
fn device_pages_render_smoke(cx: &mut TestAppContext) {
    let mut r = open(cx);

    select(&mut r, "cpu");
    let cpu = view_of(&mut r, "cpu")
        .downcast::<CpuPage>()
        .expect("a CpuPage");
    r.vcx.update(|_, cx| {
        let page = cpu.read(cx);
        assert!(
            page.overall().read(cx).spec.hover,
            "CPU overall takes hover"
        );
        if let Some(secondary) = page.secondary() {
            assert!(secondary.read(cx).spec.hover, "CPU secondary takes hover");
        }
        // The overlaid and stacked all-cores modes build their charts from these specs.
        let theme = tl_ui::theme::current(cx).clone();
        let view = CpuPageSettingsView {
            range_ns: 60_000_000_000,
            smooth: false,
            rounded: false,
        };
        let cores: Vec<DeviceIdx> = (0..4).map(DeviceIdx).collect();
        assert!(
            overlaid_spec(&cores, &view, &theme).hover,
            "CPU overlaid takes hover"
        );
        assert!(
            stacked_spec(&cores, &view, &theme).hover,
            "CPU stacked takes hover"
        );
    });

    select(&mut r, "memory");
    let memory = view_of(&mut r, "memory")
        .downcast::<MemoryPage>()
        .expect("a MemoryPage");
    r.vcx.update(|_, cx| {
        let page = memory.read(cx);
        assert_eq!(page.header().0.as_ref(), "Memory");
        for chart in [page.physical(), page.commit(), page.swap()] {
            assert!(!chart.read(cx).spec.series.is_empty());
            assert!(chart.read(cx).spec.hover, "memory chart takes hover");
        }
        assert_rows(
            page,
            &[
                ROW_IN_USE,
                ROW_AVAILABLE,
                ROW_COMMITTED,
                ROW_CACHED,
                ROW_SLAB_RECLAIMABLE,
                ROW_SLAB_UNRECLAIMABLE,
                ROW_SWAP_USED,
                ROW_SWAP_AVAILABLE,
                ROW_COMPRESSED,
                ROW_SAVINGS,
                ROW_HUGE_PAGES,
            ],
        );
    });

    select(&mut r, DISKS[0]);
    let disk = view_of(&mut r, DISKS[0])
        .downcast::<DiskPage>()
        .expect("a DiskPage");
    r.vcx.update(|_, cx| {
        let page = disk.read(cx);
        assert_eq!(page.header().0.as_ref(), "Disk 0");
        assert!(!page.active_chart().read(cx).spec.series.is_empty());
        assert!(!page.transfer_chart().read(cx).spec.series.is_empty());
        assert!(page.active_chart().read(cx).spec.hover);
        assert!(page.transfer_chart().read(cx).spec.hover);
        assert_rows(
            page,
            &[ROW_ACTIVE, ROW_RESPONSE, ROW_READ, ROW_WRITE, ROW_CAPACITY],
        );
    });

    select(&mut r, "net:enp38s0");
    let net = view_of(&mut r, "net:enp38s0")
        .downcast::<NetPage>()
        .expect("a NetPage");
    r.vcx.update(|_, cx| {
        let page = net.read(cx);
        assert_eq!(page.header().0.as_ref(), "enp38s0");
        assert_eq!(page.throughput().read(cx).spec.series.len(), 2);
        assert!(page.throughput().read(cx).spec.hover);
        assert_rows(
            page,
            &[
                ROW_DOWNLOAD,
                ROW_UPLOAD,
                ROW_TOTAL_DOWN,
                ROW_TOTAL_UP,
                ROW_DISPLAY_UNIT,
            ],
        );
    });

    select(&mut r, GPU);
    let gpu = view_of(&mut r, GPU)
        .downcast::<GpuPage>()
        .expect("a GpuPage");
    r.vcx.update(|_, cx| {
        let page = gpu.read(cx);
        assert_eq!(page.header().0.as_ref(), "Graphics adapter");
        assert_eq!(page.engines().read(cx).spec.cells.len(), 3);
        assert!(!page.memory().read(cx).spec.series.is_empty());
        assert!(!page.temperature().read(cx).spec.series.is_empty());
        assert!(page.engines().read(cx).spec.hover);
        assert!(page.memory().read(cx).spec.hover);
        assert!(page.temperature().read(cx).spec.hover);
        assert_rows(
            page,
            &[
                ROW_UTILIZATION,
                ROW_DEDICATED,
                ROW_SHARED,
                ROW_RESERVED,
                ROW_ENCODE,
                ROW_DECODE,
                ROW_TEMPERATURE,
                ROW_TEMP_SOURCE,
            ],
        );
    });
}

#[gpui_kit::test]
fn range_change_reaches_device_pages(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, "memory");
    let memory = view_of(&mut r, "memory")
        .downcast::<MemoryPage>()
        .expect("a MemoryPage");

    let performance = r.page.clone();
    r.vcx.update(|window, cx| {
        performance.read(cx).focus_handle(cx).focus(window, cx);
    });
    r.vcx.dispatch_action(PerfSetGraphRange { secs: 30 });
    draw(r.vcx);

    let (setting, ranges) = r.vcx.update(|_, cx| {
        let page = memory.read(cx);
        (
            TlSettings::settings(cx).performance.graph_range_s,
            [
                page.physical().read(cx).spec.range_ns,
                page.commit().read(cx).spec.range_ns,
                page.swap().read(cx).spec.range_ns,
            ],
        )
    });
    assert_eq!(setting, 30);
    assert_eq!(ranges, [30_000_000_000; 3]);
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

const MOUSE: &str = "bat:hidpp_battery_0";

fn valid<T>(value: T) -> InvField<T> {
    InvField {
        value,
        validity: Validity::Valid,
    }
}

fn text(s: &str) -> InvField<Arc<str>> {
    valid(Arc::from(s))
}

fn mouse_record() -> BatteryDetails {
    BatteryDetails {
        kind: Arc::from("Mouse"),
        vendor: text("Logitech"),
        model: text("MX Master 3S"),
        serial: text("SN-1"),
        native_path: Arc::from("hidpp_battery_0"),
        percentage: valid(85.0),
        energy_wh: valid(0.0),
        energy_rate_w: valid(0.0),
        voltage_v: valid(3.9),
        state: Arc::from("discharging"),
        time_to_full_s: valid(0),
        time_to_empty_s: valid(3600),
        charge_cycles: valid(12),
        technology: Arc::from("Lithium ion"),
        capacity_pct: valid(92.0),
        energy_empty_wh: valid(0.0),
        energy_full_wh: valid(0.0),
        energy_full_design_wh: valid(0.0),
        voltage_min_design_v: valid(3.0),
        voltage_max_design_v: valid(4.2),
        power_supply: false,
        charge_threshold: None,
        temperature_c: valid(31.0),
        history: None,
    }
}

/// Adds seven connected fans on `nct6797` (idx 9 to 15) and a mouse battery (idx 16).
fn publish_p2(r: &mut Rig) {
    r.engine.publish_with(|s| {
        let mut entries = catalog();
        let mut sensors = Vec::new();
        for n in 1..=7u16 {
            entries.push(entry(
                9 + (n - 1),
                DeviceIdentity::Fan {
                    chip: Arc::from("nct6797"),
                    index: n,
                },
                None,
            ));
            sensors.push((
                DeviceIdx(9 + (n - 1)),
                SensorReading {
                    device: DeviceIdx(9 + (n - 1)),
                    kind: SensorKind::Fan,
                    value: Metric::valid(1200.0),
                    pwm_pct: Metric::valid(45.0),
                    fan_control: Some(FanControlMode::Automatic),
                    fan_connected: true,
                    fan_max_rpm: Some(2000),
                },
            ));
        }
        entries.push(entry(
            16,
            DeviceIdentity::Battery {
                native_path: Arc::from("hidpp_battery_0"),
            },
            None,
        ));
        s.devices = Arc::new(DeviceCatalog {
            version: 2,
            entries: Arc::from(entries),
        });
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.sensors = Arc::from(sensors);
        let mut d = DeviceDetailsSnapshot::clone(&s.device_details);
        d.header.version += 1;
        d.batteries = Arc::from(vec![(DeviceIdx(16), mouse_record())]);
        s.device_details = Arc::new(d);
    });
    draw(r.vcx);
}

#[gpui_kit::test]
fn build_page_fan_and_battery(cx: &mut TestAppContext) {
    let mut r = open(cx);
    publish_p2(&mut r);
    // Fans count from 0 in sidebar order, so "Fan 1" is the second fan.
    select(&mut r, "fan:nct6797:2");
    select(&mut r, MOUSE);
    let fan = view_of(&mut r, "fan:nct6797:2");
    let fan_page = fan.clone().downcast::<FanPage>().expect("a FanPage");
    r.vcx.update(|_, cx| {
        let page = fan_page.read(cx);
        assert!(page.speed().read(cx).spec.hover);
        assert!(page.temperature().read(cx).spec.hover);
    });
    let header = r.vcx.update(|_, cx| header_of(&fan, cx));
    assert_eq!((header.0.as_ref(), header.1.as_ref()), ("Fan 1", "nct6797"));
    let mouse = view_of(&mut r, MOUSE);
    let mouse_page = mouse
        .clone()
        .downcast::<BatteryPage>()
        .expect("a BatteryPage");
    r.vcx.update(|_, cx| {
        assert!(
            mouse_page.read(cx).chart().read(cx).spec.hover,
            "battery chart takes hover"
        );
    });
    let header = r.vcx.update(|_, cx| header_of(&mouse, cx));
    assert_eq!(
        (header.0.as_ref(), header.1.as_ref()),
        ("Mouse", "Logitech MX Master 3S")
    );
}

#[gpui_kit::test]
fn sensor_pages_demand_switch(cx: &mut TestAppContext) {
    let mut r = open(cx);
    publish_p2(&mut r);
    select(&mut r, "fan:nct6797:2");
    let fan = view_of(&mut r, "fan:nct6797:2")
        .downcast::<FanPage>()
        .expect("a FanPage");
    assert!(r.vcx.update(|_, cx| fan.read(cx).is_active()));
    assert!(last_demand(&r).sensors);

    select(&mut r, MOUSE);
    let mouse = view_of(&mut r, MOUSE)
        .downcast::<BatteryPage>()
        .expect("a BatteryPage");
    let (fan_on, mouse_part) = r
        .vcx
        .update(|_, cx| (fan.read(cx).is_active(), mouse.read(cx).demand_part()));
    assert!(!fan_on, "the Fan page withdrew its part");
    assert!(mouse_part.sensors);
    let d = last_demand(&r);
    assert!(d.sensors && d.any_surface_visible);
}

/// Battery charge history chart (E25, PERF-079, UI-PRF-035).
mod battery_history {
    use super::*;

    fn now_s() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    }

    /// Replaces the battery record with one that has `system` and `history`.
    fn publish_battery(r: &mut Rig, system: bool, history: Option<Arc<[(i64, f32)]>>) {
        r.engine.publish_with(|s| {
            let mut d = DeviceDetailsSnapshot::clone(&s.device_details);
            d.header.version += 1;
            let mut record = mouse_record();
            record.power_supply = system;
            record.history = history;
            d.batteries = Arc::from(vec![(DeviceIdx(16), record)]);
            s.device_details = Arc::new(d);
        });
        draw(r.vcx);
    }

    fn page(r: &mut Rig) -> Entity<BatteryPage> {
        view_of(r, MOUSE)
            .downcast::<BatteryPage>()
            .expect("a BatteryPage")
    }

    fn has_chart(r: &mut Rig) -> bool {
        let p = page(r);
        r.vcx.update(|_, cx| p.read(cx).history_chart().is_some())
    }

    /// `n` points every 30 minutes, newest 10 minutes ago.
    fn recent(n: i64) -> Arc<[(i64, f32)]> {
        let now = now_s();
        (0..n)
            .map(|i| (now - 600 - 1800 * (n - 1 - i), 50.0 + i as f32))
            .collect::<Vec<_>>()
            .into()
    }

    fn fetches(r: &Rig) -> usize {
        r.engine
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::Device(DeviceCommand::BatteryHistory { .. })))
            .count()
    }

    #[gpui_kit::test]
    fn week_chart_hidden_under_2_points(cx: &mut TestAppContext) {
        let mut r = open(cx);
        publish_p2(&mut r);
        publish_battery(&mut r, true, Some(recent(1)));
        select(&mut r, MOUSE);
        assert!(!has_chart(&mut r), "1 point");
        assert!(r.vcx.debug_bounds("perf-battery-history").is_none());
        publish_battery(&mut r, true, None);
        assert!(!has_chart(&mut r), "no history yet");
        publish_battery(&mut r, true, Some(recent(2)));
        assert!(has_chart(&mut r), "2 points");
        assert!(r.vcx.debug_bounds("perf-battery-history").is_some());
        publish_battery(&mut r, true, Some(recent(1)));
        assert!(!has_chart(&mut r), "back to 1 point");
    }

    #[gpui_kit::test]
    fn week_chart_only_for_system_battery(cx: &mut TestAppContext) {
        let mut r = open(cx);
        publish_p2(&mut r);
        publish_battery(&mut r, false, Some(recent(10)));
        select(&mut r, MOUSE);
        assert!(!has_chart(&mut r), "a mouse has no history chart");
        assert_eq!(fetches(&r), 0, "and asks for none");
        publish_battery(&mut r, true, Some(recent(10)));
        assert!(has_chart(&mut r), "a system battery has one");
    }

    #[gpui_kit::test]
    fn history_requested_on_open_and_every_10_minutes(cx: &mut TestAppContext) {
        let mut r = open(cx);
        publish_p2(&mut r);
        publish_battery(&mut r, true, None);
        select(&mut r, MOUSE);
        assert_eq!(fetches(&r), 1, "on open");
        r.vcx.executor().advance_clock(Duration::from_secs(601));
        draw(r.vcx);
        assert_eq!(fetches(&r), 2, "after 10 minutes");
        // The worker reconnects and drops the history: the page asks again.
        publish_battery(&mut r, true, Some(recent(5)));
        assert_eq!(fetches(&r), 2);
        publish_battery(&mut r, true, None);
        assert_eq!(fetches(&r), 3, "history went back to None");
        select(&mut r, "cpu");
        r.vcx.executor().advance_clock(Duration::from_secs(1300));
        draw(r.vcx);
        assert_eq!(fetches(&r), 3, "none while hidden");
    }

    #[gpui_kit::test]
    fn week_chart_gaps_dashed_and_midnight_grid(cx: &mut TestAppContext) {
        let mut r = open(cx);
        publish_p2(&mut r);
        // 24 hours every 30 minutes, with a 2 hour hole.
        let now = now_s();
        let points: Vec<(i64, f32)> = (1..=48)
            .filter(|i| !(21..=23).contains(i))
            .map(|i| (now - 86_400 + 1800 * i, 90.0 - i as f32))
            .collect();
        publish_battery(&mut r, true, Some(points.into()));
        select(&mut r, MOUSE);
        let p = page(&mut r);
        let (chart, fractions) = r.vcx.update(|_, cx| {
            let p = p.read(cx);
            (
                p.history_chart().cloned().expect("a chart"),
                p.history_midnights(),
            )
        });
        let midnights = local_midnights(now).len();
        assert_eq!(fractions.len(), midnights);
        assert!(
            fractions.iter().all(|f| (0.0..1.0).contains(f)),
            "{fractions:?}"
        );
        r.vcx.update(|_, cx| {
            let v = chart.read(cx);
            // One vertical line per midnight and the 3 horizontal ones.
            assert_eq!(v.last_grid_quads.get() as usize, midnights + 3);
            // The gap is a dashed stroke between its neighbours: no dotted baseline, and a
            // solid and a dashed stroke set.
            assert!(v.last_gap_spans.borrow().is_empty());
            assert_eq!(v.cached_fill_stroke_counts(), (0, 2));
        });
        if midnights == 1 {
            assert!(r.vcx.debug_bounds("perf-battery-day-label").is_some());
        }
    }

    /// PERF-023: the history chart takes hover like every large graph. The pointer in the
    /// middle of the 24 hour plot pins a sample and paints the readout, about 12 hours back.
    #[gpui_kit::test]
    fn week_chart_takes_hover(cx: &mut TestAppContext) {
        let mut r = open(cx);
        publish_p2(&mut r);
        publish_battery(&mut r, true, Some(recent(40)));
        select(&mut r, MOUSE);
        let p = page(&mut r);
        let chart = r
            .vcx
            .update(|_, cx| p.read(cx).history_chart().cloned().expect("a chart"));
        assert!(r.vcx.read_entity(&chart, |v, _| v.spec.hover));
        r.vcx.update(|window, _| window.activate_window());
        let b = r
            .vcx
            .debug_bounds("perf-battery-history")
            .expect("history chart bounds");
        r.vcx
            .simulate_mouse_move(b.center(), None, gpui_kit::Modifiers::none());
        draw(r.vcx);
        r.vcx.read_entity(&chart, |v, _| {
            let h = v.hover_state();
            assert!(h.hitbox_inserted, "hitbox");
            assert!(h.pinned_t_ns.is_some(), "pinned");
            assert!(h.last_box.is_some(), "readout painted");
            let offset = h.line_text(0).expect("offset line");
            assert!(
                offset.starts_with("-11 h") || offset.starts_with("-12 h"),
                "{offset}"
            );
        });
    }

    #[gpui_kit::test]
    fn week_chart_rebuilds_on_history_version(cx: &mut TestAppContext) {
        let mut r = open(cx);
        publish_p2(&mut r);
        let first = recent(6);
        publish_battery(&mut r, true, Some(first.clone()));
        select(&mut r, MOUSE);
        let p = page(&mut r);
        let chart = r
            .vcx
            .update(|_, cx| p.read(cx).history_chart().cloned().expect("a chart"));
        let builds = |r: &mut Rig| r.vcx.update(|_, cx| chart.read(cx).build_count.get());
        let base = builds(&mut r);
        assert!(base >= 1);
        // The same record again: nothing rebuilds.
        publish_battery(&mut r, true, Some(first));
        assert_eq!(builds(&mut r), base);
        // A new record: one rebuild.
        publish_battery(&mut r, true, Some(recent(7)));
        assert_eq!(builds(&mut r), base + 1);
    }
}

const GIB: u64 = 1 << 30;

fn value_of(model: &tl_ui::pages::performance::details::KeyValueModel, label: &str) -> String {
    model
        .rows
        .iter()
        .find(|r| r.label == label)
        .unwrap_or_else(|| panic!("no row {label}"))
        .value
        .to_string()
}

fn zram_mem(orig: u64, compr: u64) -> MemoryTotals {
    MemoryTotals {
        validity: Validity::Valid,
        total: 64 * GIB,
        zram: tl_model::system::Metric {
            value: ZramTotals {
                orig_data: orig,
                compr_data: compr,
                mem_used: compr + GIB / 10,
            },
            validity: Validity::Valid,
        },
        ..MemoryTotals::default()
    }
}

fn zram_details() -> MemoryDetails {
    MemoryDetails {
        zram: Arc::from([ZramDetail {
            name: Arc::from("zram0"),
            disksize: 8 * GIB,
            algorithm: Some(Arc::from("zstd")),
            stats: None,
        }]),
        ..MemoryDetails::default()
    }
}

fn build(mem: &MemoryTotals, details: &MemoryDetails) -> CompressedBlock {
    let mut block = CompressedBlock::new();
    update_compressed_block(&mut block, &CompressedInputs { mem, details });
    block
}

#[test]
fn compressed_block_rows() {
    let orig = GIB * 340 / 100;
    let block = build(&zram_mem(orig, GIB), &zram_details());
    assert!(!block.is_hidden());
    assert_eq!(block.names, ["zram0"]);
    let z = &block.zram[0];
    assert_eq!(value_of(z, ROW_ZRAM_DEVICE), "/dev/zram0");
    assert_eq!(value_of(z, ROW_ZRAM_SIZE), "8.00 GiB");
    assert_eq!(value_of(z, ROW_ZRAM_STORED), "3.40 GiB");
    assert_eq!(value_of(z, ROW_ZRAM_COMPRESSED), "1.00 GiB");
    assert_eq!(value_of(z, ROW_ZRAM_MEM_USED), "1.10 GiB");
    assert_eq!(value_of(z, ROW_ZRAM_RATIO), "3.4 : 1");
    assert_eq!(value_of(z, ROW_ZRAM_ALGORITHM), "zstd");
    assert_eq!(value_of(&block.zswap, ROW_ZSWAP_ENABLED), "No");
}

#[test]
fn compressed_block_hidden_without_zram_or_zswap() {
    let block = build(&zram_mem(0, 0), &MemoryDetails::default());
    assert!(block.is_hidden());
    let disabled = MemoryDetails {
        zswap: InvField {
            value: Some(ZswapInfo {
                enabled: false,
                compressor: None,
                max_pool_percent: Some(20),
            }),
            validity: Validity::Valid,
        },
        ..MemoryDetails::default()
    };
    assert!(build(&zram_mem(0, 0), &disabled).is_hidden());
}

#[test]
fn zswap_pool_size() {
    let details = MemoryDetails {
        zswap: InvField {
            value: Some(ZswapInfo {
                enabled: true,
                compressor: None,
                max_pool_percent: Some(20),
            }),
            validity: Validity::Valid,
        },
        ..MemoryDetails::default()
    };
    let mut mem = zram_mem(0, 0);
    mem.zswapped = 2 * GIB;
    let block = build(&mem, &details);
    assert!(!block.is_hidden());
    assert_eq!(value_of(&block.zswap, ROW_ZSWAP_ENABLED), "Yes");
    assert_eq!(value_of(&block.zswap, ROW_ZSWAP_POOL), "12.80 GiB");
    assert_eq!(value_of(&block.zswap, ROW_ZRAM_STORED), "2.00 GiB");
    assert!(block.zswap.rows.iter().all(|r| !r.hidden));
}

#[test]
fn zram_ratio_dash_when_zero() {
    let block = build(&zram_mem(GIB, 0), &zram_details());
    assert_eq!(value_of(&block.zram[0], ROW_ZRAM_RATIO), "-");
}

/// The engine sends a non-valid zram total when the device's `mm_stat` read failed. The rows
/// built from it read Unavailable, not 0.
#[test]
fn zram_failed_mm_stat_is_unavailable() {
    let mut mem = zram_mem(0, 0);
    mem.zram = tl_model::system::Metric::with(Validity::Failed);
    let block = build(&mem, &zram_details());
    let z = &block.zram[0];
    for label in [
        ROW_ZRAM_STORED,
        ROW_ZRAM_COMPRESSED,
        ROW_ZRAM_MEM_USED,
        ROW_ZRAM_RATIO,
    ] {
        assert_eq!(value_of(z, label), "Unavailable", "{label}");
    }
    assert_eq!(value_of(z, ROW_ZRAM_DEVICE), "/dev/zram0");
    assert_eq!(value_of(z, ROW_ZRAM_SIZE), "8.00 GiB");
}

/// `MemoryPage::blocks` returns no block while no zram device exists and zswap is off, and one
/// block once a zram device appears.
#[gpui_kit::test]
fn compressed_blocks_follow_visibility(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, "memory");
    let memory = view_of(&mut r, "memory")
        .downcast::<MemoryPage>()
        .expect("a MemoryPage");
    let blocks = |r: &mut Rig| {
        let memory = memory.clone();
        r.vcx
            .update(|window, cx| memory.update(cx, |page, cx| page.blocks(window, cx).len()))
    };

    r.engine.publish_with(|s| {
        let system = Arc::make_mut(&mut s.system);
        system.header.version += 1;
        system.memory = zram_mem(0, 0);
        let details = Arc::make_mut(&mut s.device_details);
        details.header.version += 1;
        details.memory = Arc::new(MemoryDetails::default());
    });
    draw(r.vcx);
    r.vcx
        .update(|_, cx| assert!(memory.read(cx).compressed().is_hidden()));
    assert_eq!(blocks(&mut r), 0, "no block without zram or zswap");

    r.engine.publish_with(|s| {
        let system = Arc::make_mut(&mut s.system);
        system.header.version += 1;
        system.memory = zram_mem(GIB, GIB / 2);
        let details = Arc::make_mut(&mut s.device_details);
        details.header.version += 1;
        details.memory = Arc::new(zram_details());
    });
    draw(r.vcx);
    r.vcx
        .update(|_, cx| assert!(!memory.read(cx).compressed().is_hidden()));
    assert_eq!(blocks(&mut r), 1, "one block with a zram device");
}

/// Every grid of the block is drawn inside its own parent id, so two zram grids and the zswap
/// grid do not share element state with each other or with the Details grid.
#[gpui_kit::test]
fn compressed_grids_have_own_ids(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, "memory");
    let zram = |name: &str| ZramDetail {
        name: Arc::from(name),
        disksize: 8 * GIB,
        algorithm: Some(Arc::from("zstd")),
        stats: None,
    };
    r.engine.publish_with(|s| {
        let system = Arc::make_mut(&mut s.system);
        system.header.version += 1;
        system.memory = zram_mem(GIB, GIB / 2);
        let details = Arc::make_mut(&mut s.device_details);
        details.header.version += 1;
        details.memory = Arc::new(MemoryDetails {
            zram: Arc::from([zram("zram0"), zram("zram1")]),
            zswap: InvField {
                value: Some(ZswapInfo {
                    enabled: true,
                    compressor: None,
                    max_pool_percent: Some(20),
                }),
                validity: Validity::Valid,
            },
            ..MemoryDetails::default()
        });
    });
    draw(r.vcx);
    assert_eq!(ZRAM_GRID_ID, "perf-mem-zram-grid");
    let ids = [
        "perf-mem-zram-grid-0",
        "perf-mem-zram-grid-1",
        ZSWAP_GRID_ID,
    ];
    let bounds: Vec<_> = ids
        .iter()
        .map(|id| {
            r.vcx
                .debug_bounds(id)
                .unwrap_or_else(|| panic!("{id} is drawn"))
        })
        .collect();
    assert!(bounds[0].origin.y < bounds[1].origin.y);
    assert!(bounds[1].origin.y < bounds[2].origin.y);
}

/// Two ratios whose old cache keys matched (4 GiB : 1 GiB and 12 GiB : 1 GiB + 2 bytes) both
/// show their own text.
#[test]
fn zram_ratio_key_has_no_collisions() {
    let mut block = CompressedBlock::new();
    let details = zram_details();
    let mem = zram_mem(4 * GIB, GIB);
    update_compressed_block(
        &mut block,
        &CompressedInputs {
            mem: &mem,
            details: &details,
        },
    );
    assert_eq!(value_of(&block.zram[0], ROW_ZRAM_RATIO), "4.0 : 1");
    let mem = zram_mem(12 * GIB, GIB + 2);
    update_compressed_block(
        &mut block,
        &CompressedInputs {
            mem: &mem,
            details: &details,
        },
    );
    assert_eq!(value_of(&block.zram[0], ROW_ZRAM_RATIO), "12.0 : 1");
}

/// With two zram devices each grid shows its own `mm_stat` values, not Unavailable.
#[test]
fn zram_two_devices_show_own_stats() {
    let dev = |name: &str, orig: u64, compr: u64| ZramDetail {
        name: Arc::from(name),
        disksize: 8 * GIB,
        algorithm: Some(Arc::from("zstd")),
        stats: Some(ZramTotals {
            orig_data: orig,
            compr_data: compr,
            mem_used: compr + GIB / 10,
        }),
    };
    let details = MemoryDetails {
        zram: Arc::from([dev("zram0", 4 * GIB, GIB), dev("zram1", 2 * GIB, GIB)]),
        ..MemoryDetails::default()
    };
    let block = build(&zram_mem(6 * GIB, 2 * GIB), &details);
    assert_eq!(value_of(&block.zram[0], ROW_ZRAM_STORED), "4.00 GiB");
    assert_eq!(value_of(&block.zram[0], ROW_ZRAM_COMPRESSED), "1.00 GiB");
    assert_eq!(value_of(&block.zram[0], ROW_ZRAM_MEM_USED), "1.10 GiB");
    assert_eq!(value_of(&block.zram[0], ROW_ZRAM_RATIO), "4.0 : 1");
    assert_eq!(value_of(&block.zram[1], ROW_ZRAM_STORED), "2.00 GiB");
    assert_eq!(value_of(&block.zram[1], ROW_ZRAM_RATIO), "2.0 : 1");
}

fn swap_device(name: &str, kind: SwapKindModel, priority: i32) -> SwapDevice {
    SwapDevice {
        name: Arc::from(name),
        kind,
        size_bytes: 8 * GIB,
        used_bytes: GIB,
        priority,
    }
}

/// Publishes `swaps` with `validity` as the memory details and draws.
fn publish_swaps(r: &mut Rig, swaps: Vec<SwapDevice>, validity: Validity) {
    r.engine.publish_with(|s| {
        let details = Arc::make_mut(&mut s.device_details);
        details.header.version += 1;
        details.memory = Arc::new(MemoryDetails {
            swaps: InvField {
                value: Arc::from(swaps),
                validity,
            },
            ..MemoryDetails::default()
        });
    });
    draw(r.vcx);
    draw(r.vcx);
}

fn swap_blocks(r: &mut Rig, memory: &Entity<MemoryPage>) -> usize {
    let memory = memory.clone();
    r.vcx
        .update(|window, cx| memory.update(cx, |page, cx| page.blocks(window, cx).len()))
}

/// Two swap entries give one block with five columns at their widths, the heading, and a zram
/// entry whose Type cell reads `zram`.
#[gpui_kit::test]
fn swap_table_columns(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, "memory");
    let memory = view_of(&mut r, "memory")
        .downcast::<MemoryPage>()
        .expect("a MemoryPage");
    publish_swaps(
        &mut r,
        vec![
            swap_device("/dev/nvme0n1p3", SwapKindModel::Partition, -2),
            swap_device("/dev/zram0", SwapKindModel::Zram, 100),
        ],
        Validity::Valid,
    );
    assert_eq!(swap_blocks(&mut r, &memory), 1);
    r.vcx.update(|_, cx| {
        let page = memory.read(cx);
        assert_eq!(page.words().block_swap.as_ref(), "Swap devices");
        let table = page.swap_table().expect("table created").clone();
        table.update(cx, |t, _| {
            let layout = t.columns();
            assert_eq!(&**layout.visible_columns(), &SwapColumn::ALL[..]);
            for (col, w) in [
                (SwapColumn::Type, 80.),
                (SwapColumn::Size, 90.),
                (SwapColumn::Used, 90.),
                (SwapColumn::Priority, 70.),
            ] {
                assert_eq!(layout.width(col).as_f32(), w * layout.ui_scale(), "{col:?}");
            }
            let tsv = t.rows_as_tsv(&[0, 1]);
            let lines: Vec<&str> = tsv.lines().collect();
            assert_eq!(lines.len(), 3, "header and two rows: {tsv}");
            assert_eq!(lines[0], "Name\tType\tSize\tUsed\tPriority");
            assert_eq!(lines[1].split('\t').nth(1), Some("partition"));
            assert_eq!(lines[1].split('\t').nth(4), Some("-2"));
            assert_eq!(lines[2].split('\t').nth(1), Some("zram"));
        });
    });
}

/// No block for an empty list, and none when the list failed to read.
#[gpui_kit::test]
fn swap_table_hidden_when_empty(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, "memory");
    let memory = view_of(&mut r, "memory")
        .downcast::<MemoryPage>()
        .expect("a MemoryPage");
    publish_swaps(&mut r, Vec::new(), Validity::Valid);
    assert_eq!(swap_blocks(&mut r, &memory), 0, "empty list");
    publish_swaps(
        &mut r,
        vec![swap_device("/swapfile", SwapKindModel::File, -2)],
        Validity::Failed,
    );
    assert_eq!(swap_blocks(&mut r, &memory), 0, "failed list");
}

/// Twelve entries show 8 rows of height, and the table becomes a Tab stop.
#[gpui_kit::test]
fn swap_table_caps_at_8_rows(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, "memory");
    let memory = view_of(&mut r, "memory")
        .downcast::<MemoryPage>()
        .expect("a MemoryPage");
    let swaps = |n: usize| {
        (0..n)
            .map(|i| swap_device(&format!("/dev/swap{i}"), SwapKindModel::Partition, i as i32))
            .collect::<Vec<_>>()
    };
    let tab_stop_and_rows = |r: &mut Rig| {
        let memory = memory.clone();
        r.vcx.update(|_, cx| {
            let table = memory.read(cx).swap_table().expect("table").clone();
            table.update(cx, |t, _| {
                let d = t.delegate_mut();
                (
                    d.scrolls(),
                    tl_ui::table::tree_table::TreeTableDelegate::row_count(d),
                )
            })
        })
    };
    publish_swaps(&mut r, swaps(2), Validity::Valid);
    assert_eq!(tab_stop_and_rows(&mut r), (false, 2));
    publish_swaps(&mut r, swaps(12), Validity::Valid);
    assert_eq!(tab_stop_and_rows(&mut r), (true, 12));
    let m = r.vcx.update(|_, cx| *metrics(cx));
    let bounds = r.vcx.debug_bounds(SWAP_TABLE_ID).expect("table box drawn");
    let want = 28. * m.ui_scale + 8. * m.row_height.as_f32();
    assert!(
        (bounds.size.height.as_f32() - want).abs() < 0.5,
        "{} vs {want}",
        bounds.size.height.as_f32()
    );
}

/// The 14 Disk Details rows after the P1 rows (E25-015).
mod disk_p2 {
    use super::*;
    use tl_model::snapshot::DiskDetails;
    use tl_model::system::{DiskTotals, Metric};
    use tl_ui::pages::performance::details::KeyValueModel;
    use tl_ui::pages::performance::disk::{
        DISK_DETAIL_ROWS, DiskDetailInputs, disk_details_model, update_disk_details_p1,
        update_disk_details_p2,
    };

    const GIB: u64 = 1 << 30;

    fn totals() -> DiskTotals {
        DiskTotals {
            device: DeviceIdx(1),
            active_pct: Metric::valid(12.0),
            read_bps: Metric::valid(4.0 * 1048576.0),
            write_bps: Metric::valid(1.5 * 1048576.0),
            response_ms: Metric::valid(0.42),
            read_total: 812 * GIB + 410 * (GIB / 1000),
            written_total: 400 * GIB,
            capacity: 1_000_204_886_016,
            temp_c: Metric::valid(41.0),
        }
    }

    fn nvme_details() -> DiskDetails {
        let text = |s: &str| InvField {
            value: Arc::<str>::from(s),
            validity: Validity::Valid,
        };
        let size = |n: u64| InvField {
            value: n,
            validity: Validity::Valid,
        };
        let flag = |b: bool| InvField {
            value: b,
            validity: Validity::Valid,
        };
        DiskDetails {
            kind: Arc::from("NVMe"),
            model: text("WD_BLACK SN850X 1000GB"),
            interface: text("NVMe"),
            wwn: text("eui.0000000000000001"),
            serial: text("S6XXNX0T"),
            rotation_rpm: Some(7200),
            formatted_bytes: size(920 * GIB),
            used_bytes: size(400 * GIB),
            free_bytes: size(520 * GIB),
            system_disk: flag(true),
            swap: flag(false),
            ..DiskDetails::default()
        }
    }

    fn model(cx: &mut TestAppContext, t: &DiskTotals, d: &DiskDetails) -> KeyValueModel {
        let mut model = disk_details_model();
        let inp = DiskDetailInputs {
            now_boot_ns: 0,
            observed_end_ns: 0,
            totals: Some(t),
            error: None,
        };
        update_disk_details_p1(&mut model, &inp);
        update_disk_details_p2(&mut model, &inp, Some(d));
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
    fn disk_details_order(cx: &mut TestAppContext) {
        let m = model(cx, &totals(), &nvme_details());
        let r = rows(&m);
        let labels: Vec<&str> = r.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels.len(), 19);
        assert_eq!(
            labels,
            [
                "Active time",
                "Average response time",
                "Read speed",
                "Write speed",
                "Total read",
                "Total written",
                "Capacity",
                "Formatted",
                "Used space",
                "Free space",
                "System disk",
                "Swap",
                "Hardware name",
                "Type",
                "Interface",
                "WWN",
                "Serial",
                "Rotation rate",
                "Temperature",
            ]
        );
        assert_eq!(DISK_DETAIL_ROWS.len(), 19);
        assert_eq!(value(&m, "Total read").as_deref(), Some("812.41 GiB"));
        assert_eq!(value(&m, "Formatted").as_deref(), Some("920.00 GiB"));
        assert_eq!(
            value(&m, "Hardware name").as_deref(),
            Some("WD_BLACK SN850X 1000GB")
        );
        assert_eq!(value(&m, "Type").as_deref(), Some("NVMe"));
        assert_eq!(value(&m, "Rotation rate").as_deref(), Some("7200 RPM"));
        assert_eq!(value(&m, "Temperature").as_deref(), Some("41 °C"));
    }

    #[gpui_kit::test]
    fn disk_hidden_rows(cx: &mut TestAppContext) {
        let mut d = nvme_details();
        d.wwn.validity = Validity::Unsupported;
        d.serial.validity = Validity::Unsupported;
        d.rotation_rpm = None;
        let m = model(cx, &totals(), &d);
        let r = rows(&m);
        let labels: Vec<&str> = r.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels.len(), 16);
        let want: Vec<&str> = [
            "Active time",
            "Average response time",
            "Read speed",
            "Write speed",
            "Total read",
            "Total written",
            "Capacity",
            "Formatted",
            "Used space",
            "Free space",
            "System disk",
            "Swap",
            "Hardware name",
            "Type",
            "Interface",
            "Temperature",
        ]
        .to_vec();
        assert_eq!(labels, want);
    }

    #[gpui_kit::test]
    fn disk_system_disk_yes(cx: &mut TestAppContext) {
        let m = model(cx, &totals(), &nvme_details());
        assert_eq!(value(&m, "System disk").as_deref(), Some("Yes"));
        assert_eq!(value(&m, "Swap").as_deref(), Some("No"));
    }

    #[gpui_kit::test]
    fn disk_no_mount_not_reported(cx: &mut TestAppContext) {
        let mut d = nvme_details();
        d.formatted_bytes.validity = Validity::Unsupported;
        d.used_bytes.validity = Validity::Unsupported;
        d.free_bytes.validity = Validity::Unsupported;
        let m = model(cx, &totals(), &d);
        for label in ["Formatted", "Used space", "Free space"] {
            assert_eq!(value(&m, label).as_deref(), Some("Not reported"), "{label}");
        }
    }

    #[gpui_kit::test]
    fn disk_temperature_not_reported(cx: &mut TestAppContext) {
        let mut t = totals();
        t.temp_c = Metric::with(Validity::Unsupported);
        let mut d = nvme_details();
        d.smart_temp_c = None;
        let m = model(cx, &t, &d);
        assert_eq!(value(&m, "Temperature").as_deref(), Some("Not reported"));
    }

    #[gpui_kit::test]
    fn disk_totals_failed_read_shows_fallback(cx: &mut TestAppContext) {
        let mut t = totals();
        t.read_bps = Metric::with(Validity::Failed);
        t.write_bps = Metric::with(Validity::Unsupported);
        let m = model(cx, &t, &nvme_details());
        assert_eq!(value(&m, "Total read").as_deref(), Some("Unavailable"));
        assert_eq!(value(&m, "Total written").as_deref(), Some("Not reported"));
    }

    #[gpui_kit::test]
    fn disk_totals_shown_while_rates_warm_up(cx: &mut TestAppContext) {
        // First sample and counter reset: no rates yet, but the totals are valid.
        let mut t = totals();
        t.read_bps = Metric::with(Validity::WarmingUp);
        t.write_bps = Metric::with(Validity::WarmingUp);
        let m = model(cx, &t, &nvme_details());
        assert_eq!(value(&m, "Total read").as_deref(), Some("812.41 GiB"));
        assert_eq!(value(&m, "Total written").as_deref(), Some("400.00 GiB"));
    }

    #[gpui_kit::test]
    fn disk_totals_missing_read_shows_fallback(cx: &mut TestAppContext) {
        let mut t = totals();
        t.read_bps = Metric::with(Validity::Missing);
        let m = model(cx, &t, &nvme_details());
        assert_eq!(value(&m, "Total read").as_deref(), Some("Collecting…"));
        assert_eq!(value(&m, "Total written").as_deref(), Some("400.00 GiB"));
    }
}

/// The Disk header buttons and page-menu items (E25-017).
mod disk_buttons {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point};
    use tl_model::snapshot::{DiskDetails, SmartKind};
    use tl_ui::components::menu::{MenuEntryKind, TlMenuPopup, open_context_menu};
    use tl_ui::pages::performance::PAGE_PANEL_ID;
    use tl_ui::pages::performance::actions::PerfShowSmart;

    const SDA: &str = "disk:sda";

    fn publish(r: &mut Rig, smart: SmartKind, removable: bool, ejectable: bool) {
        r.engine.publish_with(|s| {
            let details = Arc::make_mut(&mut s.device_details);
            details.header.version += 1;
            details.disks = Arc::from([(
                DeviceIdx(3),
                DiskDetails {
                    smart,
                    removable,
                    ejectable,
                    ..DiskDetails::default()
                },
            )]);
        });
        draw(r.vcx);
    }

    fn disk_page(r: &mut Rig) -> Entity<DiskPage> {
        view_of(r, SDA).downcast::<DiskPage>().expect("a DiskPage")
    }

    fn open_disk(cx: &mut TestAppContext) -> Rig<'_> {
        let mut r = open(cx);
        select(&mut r, SDA);
        r
    }

    #[gpui_kit::test]
    fn disk_buttons_visibility(cx: &mut TestAppContext) {
        let mut r = open_disk(cx);
        publish(&mut r, SmartKind::Nvme, false, false);
        let page = disk_page(&mut r);
        r.vcx.update(|window, cx| {
            let toggles = page.update(cx, |p, cx| p.toggles(window, cx));
            assert_eq!(toggles.len(), 1, "only the SMART button");
            assert!(!page.read(cx).ejectable());
        });
        assert!(r.vcx.debug_bounds("perf-page-toggle-0").is_some());
        assert!(r.vcx.debug_bounds("perf-page-toggle-1").is_none());
    }

    #[gpui_kit::test]
    fn disk_eject_button_live(cx: &mut TestAppContext) {
        let mut r = open_disk(cx);
        publish(&mut r, SmartKind::Nvme, false, false);
        let before = disk_page(&mut r);
        assert!(r.vcx.debug_bounds("perf-page-toggle-1").is_none());
        publish(&mut r, SmartKind::Nvme, true, false);
        let after = disk_page(&mut r);
        assert_eq!(before.entity_id(), after.entity_id(), "page not re-created");
        assert!(
            r.vcx.debug_bounds("perf-page-toggle-1").is_some(),
            "Eject appears"
        );
        publish(&mut r, SmartKind::Nvme, false, false);
        assert!(
            r.vcx.debug_bounds("perf-page-toggle-1").is_none(),
            "Eject goes"
        );
    }

    #[gpui_kit::test]
    fn disk_smart_button_dispatch(cx: &mut TestAppContext) {
        let mut r = open_disk(cx);
        publish(&mut r, SmartKind::Nvme, false, false);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        r.vcx.update(|_, cx| {
            cx.on_action(move |a: &PerfShowSmart, _| sink.borrow_mut().push(a.clone()));
        });
        let at = r
            .vcx
            .debug_bounds("perf-page-toggle-0")
            .expect("the SMART button")
            .center();
        r.vcx.simulate_click(at, Modifiers::none());
        r.vcx.run_until_parked();
        let seen = seen.borrow();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].key, key(SDA));
    }

    fn right_click(cx: &mut VisualTestContext, at: Point<Pixels>) {
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

    fn menu_labels(r: &mut Rig) -> Vec<(MenuEntryKind, String)> {
        let at = r
            .vcx
            .debug_bounds(PAGE_PANEL_ID)
            .expect("the page panel")
            .center();
        right_click(r.vcx, at);
        let menu: Entity<TlMenuPopup> = r
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("a context menu is open");
        r.vcx.update(|window, cx| {
            menu.read(cx)
                .entries_for_test(window)
                .into_iter()
                .map(|e| (e.kind, e.label.map(|l| l.to_string()).unwrap_or_default()))
                .collect()
        })
    }

    #[gpui_kit::test]
    fn disk_menu_items(cx: &mut TestAppContext) {
        let mut r = open_disk(cx);
        publish(&mut r, SmartKind::Nvme, true, false);
        let list = menu_labels(&mut r);
        let names: Vec<&str> = list.iter().map(|e| e.1.as_str()).collect();
        let smart = names.iter().position(|n| *n == "Show SMART data…");
        let eject = names.iter().position(|n| *n == "Eject");
        let (smart, eject) = (smart.expect("SMART item"), eject.expect("Eject item"));
        assert_eq!(eject, smart + 1, "{names:?}");
        let first_sep = list
            .iter()
            .position(|e| e.0 == MenuEntryKind::Separator)
            .expect("a separator");
        assert!(smart > first_sep, "after the common items: {names:?}");
        assert_eq!(list[eject + 1].0, MenuEntryKind::Separator, "{names:?}");
        assert_eq!(names[eject + 2], "View");
    }

    #[gpui_kit::test]
    fn disk_focus_order(cx: &mut TestAppContext) {
        let mut r = open_disk(cx);
        publish(&mut r, SmartKind::Nvme, true, false);
        let page = disk_page(&mut r);
        r.vcx.update(|_, cx| {
            let order = page.read(cx).focus_order();
            assert_eq!(order.len(), 2, "SMART then Eject");
            let p = page.read(cx);
            assert_eq!(order[0], *p.smart_focus(), "SMART first");
            assert_eq!(order[1], *p.eject_focus(), "Eject second");
            assert_ne!(order[0], order[1]);
        });
        publish(&mut r, SmartKind::None, true, false);
        r.vcx.update(|_, cx| {
            let p = page.read(cx);
            let order = p.focus_order();
            assert_eq!(order.len(), 1);
            assert_eq!(order[0], *p.eject_focus(), "only Eject remains");
        });
    }
}

/// The "Disk is in use" window (E25-020, PERF-052, UI-PRF-030).
mod disk_in_use {
    use super::*;

    use gpui_kit::Modifiers;
    use gpui_kit::base::Root;
    use tl_model::actions::{
        ActionEvent, ActionKind, ActionRequest, ActionState, ActionTarget, Signal,
    };
    use tl_model::command::{Command, DeviceCommand};
    use tl_model::ids::{GroupKey, ProcessKey};
    use tl_model::process::{ProcFlags, ProcState, ProcessMeta, ProcessRecord};
    use tl_model::snapshot::{Blocker, DiskDetails, EjectState};
    use tl_model::time::BootId;
    use tl_model::validity::FieldValidity;
    use tl_ui::action_tracker::{ActionTracker, GlobalActionTracker};
    use tl_ui::components::dialog::{DialogButton, TlDialogView};
    use tl_ui::components::toast::ToastKind;
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::pages::performance::actions::PerfEject;
    use tl_ui::windows::disk_in_use::{
        DiskInUseWindow, EjectToasts, RETRY_DELAY, find_disk_in_use,
    };

    const SDA: &str = "disk:sda";
    const IDX: DeviceIdx = DeviceIdx(3);
    const PIDS: [u32; 3] = [101, 102, 103];

    fn pkey(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 500,
            boot_id: BootId(0),
            pid,
        }
    }

    fn record(pid: u32, name: &str, app: &str) -> ProcessRecord {
        let text = |value: &str| Arc::<str>::from(value);
        ProcessRecord {
            key: pkey(pid),
            ppid: 1,
            uid: 1000,
            meta: Arc::new(ProcessMeta {
                comm: text(name),
                name: text(name),
                exe: Some(text(&format!("/usr/bin/{name}"))),
                cmdline: None,
                user: text("me"),
                cgroup: None,
                app_id: Some(text(app)),
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: text(name),
            }),
            group: GroupKey(u64::from(pid)),
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

    /// Two apps and three processes: 101 and 103 in Files, 102 in Editor.
    fn blockers() -> Arc<[Blocker]> {
        let b = |pid: u32, name: &str, app: &str, path: &str| Blocker {
            key: pkey(pid),
            name: Arc::from(name),
            app: Some(Arc::from(app)),
            paths: Arc::from([Arc::<str>::from(path), Arc::<str>::from("/media/usb/b")]),
        };
        Arc::from([
            b(101, "files", "org.files", "/media/usb/a"),
            b(102, "editor", "org.editor", "/media/usb/c"),
            b(103, "files-helper", "org.files", "/media/usb/d"),
        ])
    }

    fn records() -> Arc<[ProcessRecord]> {
        Arc::from([
            record(101, "Files", "org.files"),
            record(102, "Editor", "org.editor"),
            record(103, "files-helper", "org.files"),
        ])
    }

    /// The Disk page of sda with an action tracker installed. The toast layer is registered as
    /// the app does, so the window's own toast stack takes the action toasts.
    fn setup(cx: &mut TestAppContext) -> Rig<'_> {
        cx.update(tl_ui::components::toast::init);
        let mut r = open(cx);
        select(&mut r, SDA);
        let client: Arc<dyn EngineClient> = r.engine.clone();
        r.vcx.update(|_, cx| {
            let tracker = cx.new(|cx| ActionTracker::new(client, cx));
            cx.set_global(GlobalActionTracker(tracker));
        });
        publish(&mut r, None);
        r
    }

    fn publish(r: &mut Rig, eject: Option<(u64, EjectState)>) {
        r.engine.publish_with(|s| {
            let details = Arc::make_mut(&mut s.device_details);
            details.header.version += 1;
            details.disks = Arc::from([(
                IDX,
                DiskDetails {
                    removable: true,
                    eject,
                    ..DiskDetails::default()
                },
            )]);
            let p = Arc::make_mut(&mut s.processes);
            p.header.version += 1;
            p.records = records();
        });
        draw(r.vcx);
    }

    fn eject_seqs(r: &Rig) -> Vec<u64> {
        r.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Device(DeviceCommand::Eject { seq, disk }) => {
                    assert_eq!(&*disk, "sda");
                    Some(seq)
                }
                _ => None,
            })
            .collect()
    }

    fn sent(r: &Rig) -> Vec<ActionRequest> {
        r.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    /// Dispatches `PerfEject` on the Disk page and returns the sequence it sent.
    fn eject(r: &mut Rig) -> u64 {
        let page = view_of(r, SDA).downcast::<DiskPage>().expect("a DiskPage");
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            focus.dispatch_action(&PerfEject { key: key(SDA) }, window, cx);
        });
        r.vcx.run_until_parked();
        *eject_seqs(r).last().expect("an eject was sent")
    }

    fn blocked(others_may_hold: bool) -> EjectState {
        EjectState::Blocked {
            blockers: blockers(),
            others_may_hold,
        }
    }

    /// The open window, drawn.
    fn window(r: &mut Rig) -> (VisualTestContext, Entity<DiskInUseWindow>) {
        let (handle, view) = r
            .vcx
            .update(|_, cx| find_disk_in_use(IDX, cx))
            .expect("the Disk is in use window is open");
        let mut vcx = VisualTestContext::from_window(handle.into(), &r.vcx.cx);
        redraw(&mut vcx);
        (vcx, view)
    }

    fn redraw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// Ejects and publishes `Blocked` for that sequence; returns the sequence and the window.
    fn open_blocked(
        r: &mut Rig,
        others_may_hold: bool,
    ) -> (u64, VisualTestContext, Entity<DiskInUseWindow>) {
        let seq = eject(r);
        publish(r, Some((seq, blocked(others_may_hold))));
        let (vcx, view) = window(r);
        (seq, vcx, view)
    }

    fn click(vcx: &mut VisualTestContext, selector: &'static str) {
        let at = vcx.debug_bounds(selector).expect(selector).center();
        vcx.simulate_click(at, Modifiers::none());
        vcx.run_until_parked();
    }

    /// The open confirmation dialog, drawn.
    fn dialog(r: &mut Rig) -> Option<(VisualTestContext, Entity<TlDialogView>)> {
        let (any, frame) = r.vcx.update(|_, cx| {
            cx.windows().into_iter().find_map(|w| {
                let handle = w.downcast::<Root>()?;
                let frame = TlSecondaryWindow::<TlDialogView>::find(handle, cx)?;
                Some((w, frame))
            })
        })?;
        let view = r.vcx.update(|_, cx| frame.read(cx).content().clone());
        let mut vcx = VisualTestContext::from_window(any, &r.vcx.cx);
        vcx.update(|window, _| window.activate_window());
        redraw(&mut vcx);
        Some((vcx, view))
    }

    #[gpui_kit::test]
    fn disk_in_use_layout(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, view) = open_blocked(&mut r, false);
        vcx.update(|_, cx| {
            let w = view.read(cx);
            assert_eq!(w.groups().len(), 2);
            assert_eq!(w.groups()[0].name.as_ref(), "Files");
            assert_eq!(w.groups()[0].rows.len(), 2, "101 and 103 share an app");
            assert_eq!(w.groups()[1].name.as_ref(), "Editor");
            assert_eq!(w.row_label().as_ref(), "End Task");
            assert_eq!(w.intro().as_ref(), "These programs are using Disk 0 (sda):");
            assert!(!w.eacces_note());
        });
        assert!(vcx.debug_bounds("DiskInUseGroup-0").is_some());
        assert!(vcx.debug_bounds("DiskInUseGroup-1").is_some());
        assert!(vcx.debug_bounds("DiskInUseGroup-2").is_none());
        for sel in ["DiskInUseEnd-0", "DiskInUseEnd-1", "DiskInUseEnd-2"] {
            assert!(vcx.debug_bounds(sel).is_some(), "{sel}");
        }
        assert!(vcx.debug_bounds("DiskInUseRow-3").is_none());
        assert!(vcx.debug_bounds("DiskInUseNote").is_none());
        for sel in ["DiskInUseClose", "DiskInUseRetry", "DiskInUseEndAll"] {
            assert!(vcx.debug_bounds(sel).is_some(), "{sel}");
        }
    }

    #[gpui_kit::test]
    fn disk_in_use_shift_relabels(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, view) = open_blocked(&mut r, false);
        vcx.simulate_modifiers_change(Modifiers::shift());
        redraw(&mut vcx);
        let label = vcx.update(|_, cx| view.read(cx).row_label().clone());
        assert_eq!(label.as_ref(), "End Process");
        vcx.simulate_modifiers_change(Modifiers::none());
        redraw(&mut vcx);
        let label = vcx.update(|_, cx| view.read(cx).row_label().clone());
        assert_eq!(label.as_ref(), "End Task");
    }

    #[gpui_kit::test]
    fn disk_in_use_single_end_task_no_dialog(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, _) = open_blocked(&mut r, false);
        click(&mut vcx, "DiskInUseEnd-0");
        r.vcx.run_until_parked();
        assert!(dialog(&mut r).is_none(), "no confirmation");
        let sent = sent(&r);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].kind, ActionKind::Signal(Signal::Term));
        assert_eq!(
            sent[0].targets.as_slice(),
            &[ActionTarget::Process(pkey(PIDS[0]))]
        );
    }

    #[gpui_kit::test]
    fn disk_in_use_end_all_confirms(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, _) = open_blocked(&mut r, false);
        click(&mut vcx, "DiskInUseEndAll");
        r.vcx.run_until_parked();
        assert!(sent(&r).is_empty(), "nothing before the confirmation");
        let (mut dvcx, view) = dialog(&mut r).expect("a confirmation");
        let destructive = dvcx.update(|_, cx| view.read(cx).dialog().is_destructive());
        if destructive {
            assert_eq!(
                dvcx.update(|window, cx| view.read(cx).focused_button(window)),
                Some(DialogButton::Cancel)
            );
            dvcx.simulate_keystrokes("tab");
        }
        dvcx.simulate_keystrokes("enter");
        dvcx.run_until_parked();
        r.vcx.run_until_parked();
        let sent = sent(&r);
        assert_eq!(sent.len(), 1, "one request");
        assert_eq!(sent[0].kind, ActionKind::Signal(Signal::Term));
        // List order: the Files group (101, 103), then Editor (102).
        let targets: Vec<_> = [101, 103, 102]
            .iter()
            .map(|p| ActionTarget::Process(pkey(*p)))
            .collect();
        assert_eq!(sent[0].targets.to_vec(), targets);
    }

    /// End All runs through the shared process action entry point (tasklens-pdu.58): tasklens
    /// itself is dropped with the Processes tab's refusal toast, and the rest are ended after the
    /// same "End 2 selected processes?" confirmation.
    #[gpui_kit::test]
    fn disk_in_use_end_all_refuses_self_like_processes_tab(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, _) = open_blocked(&mut r, false);
        r.engine.publish_with(|s| {
            let p = Arc::make_mut(&mut s.processes);
            p.header.version += 1;
            let mut records = records().to_vec();
            records[1].flags = ProcFlags::IS_SELF | ProcFlags::PROTECTED;
            p.records = records.into();
        });
        draw(r.vcx);
        click(&mut vcx, "DiskInUseEndAll");
        r.vcx.run_until_parked();
        redraw(&mut vcx);
        let toasts: Vec<String> = vcx.update(|window, cx| {
            tl_ui::components::toast::ToastQueue::of_window(window, cx)
                .map(|q| {
                    let texts = q.read(cx).visible_texts();
                    texts.iter().map(ToString::to_string).collect()
                })
                .unwrap_or_default()
        });
        assert!(
            toasts
                .iter()
                .any(|t| t == "Refusing to end tasklens itself."),
            "{toasts:?}"
        );
        assert!(sent(&r).is_empty(), "nothing before the confirmation");
        let (mut dvcx, view) = dialog(&mut r).expect("a confirmation");
        let text = dvcx.update(|_, cx| view.read(cx).dialog().text().to_string());
        assert_eq!(text, "End 2 selected processes?");
        if dvcx.update(|_, cx| view.read(cx).dialog().is_destructive()) {
            dvcx.simulate_keystrokes("tab");
        }
        dvcx.simulate_keystrokes("enter");
        dvcx.run_until_parked();
        r.vcx.run_until_parked();
        let sent = sent(&r);
        assert_eq!(sent.len(), 1, "one request");
        assert_eq!(sent[0].kind, ActionKind::Signal(Signal::Term));
        let targets: Vec<_> = [101, 103]
            .iter()
            .map(|p| ActionTarget::Process(pkey(*p)))
            .collect();
        assert_eq!(sent[0].targets.to_vec(), targets);
    }

    #[gpui_kit::test]
    fn disk_in_use_auto_retry(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (seq, mut vcx, _) = open_blocked(&mut r, false);
        click(&mut vcx, "DiskInUseEnd-1");
        let id = sent(&r).first().expect("End Task sent").id;
        r.engine.emit(ActionEvent {
            id,
            target: None,
            state: ActionState::Succeeded,
        });
        r.vcx.run_until_parked();
        assert_eq!(eject_seqs(&r), vec![seq], "no retry before 500 ms");
        r.vcx
            .executor()
            .advance_clock(RETRY_DELAY - Duration::from_millis(1));
        r.vcx.run_until_parked();
        assert_eq!(eject_seqs(&r).len(), 1);
        r.vcx.executor().advance_clock(Duration::from_millis(1));
        r.vcx.run_until_parked();
        let seqs = eject_seqs(&r);
        assert_eq!(seqs.len(), 2, "one retry");
        assert!(seqs[1] > seq);
        r.vcx.executor().advance_clock(Duration::from_secs(2));
        r.vcx.run_until_parked();
        assert_eq!(eject_seqs(&r).len(), 2, "once per action");
    }

    #[gpui_kit::test]
    fn disk_in_use_done_closes_and_toasts(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (seq, _, _) = open_blocked(&mut r, false);
        publish(&mut r, Some((seq, EjectState::Done)));
        r.vcx.run_until_parked();
        assert!(
            r.vcx.update(|_, cx| find_disk_in_use(IDX, cx)).is_none(),
            "the window closed"
        );
        let toasts = r.vcx.update(|_, cx| {
            cx.try_global::<EjectToasts>()
                .map(|t| t.0.clone())
                .unwrap_or_default()
        });
        assert_eq!(
            toasts,
            vec![(ToastKind::Success, SharedString::from("Disk ejected"))]
        );
    }

    #[gpui_kit::test]
    fn disk_in_use_eject_again_activates(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, _, first) = open_blocked(&mut r, false);
        r.vcx.update(|window, _| window.activate_window());
        r.vcx.run_until_parked();
        let seq = eject(&mut r);
        publish(&mut r, Some((seq, blocked(false))));
        r.vcx.run_until_parked();
        let (handle, view) = r
            .vcx
            .update(|_, cx| find_disk_in_use(IDX, cx))
            .expect("the window is still open");
        assert_eq!(view, first, "the same window, not a second one");
        let active = r
            .vcx
            .update(|_, cx| cx.active_window().map(|w| w.window_id()));
        assert_eq!(active, Some(handle.window_id()), "brought to the front");
    }

    fn eject_toasts(r: &mut Rig) -> Vec<(ToastKind, SharedString)> {
        r.vcx.update(|_, cx| {
            cx.try_global::<EjectToasts>()
                .map(|t| t.0.clone())
                .unwrap_or_default()
        })
    }

    #[gpui_kit::test]
    fn disk_in_use_eject_while_running_keeps_watch(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let seq = eject(&mut r);
        publish(&mut r, Some((seq, EjectState::Running)));
        eject(&mut r);
        assert_eq!(eject_seqs(&r), vec![seq], "nothing sent while it runs");
        publish(&mut r, Some((seq, EjectState::Done)));
        r.vcx.run_until_parked();
        assert!(
            r.vcx.update(|_, cx| find_disk_in_use(IDX, cx)).is_none(),
            "no window"
        );
        assert_eq!(
            eject_toasts(&mut r),
            vec![(ToastKind::Success, SharedString::from("Disk ejected"))]
        );
    }

    #[gpui_kit::test]
    fn disk_in_use_retry_while_running_keeps_watch(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (first, mut vcx, _) = open_blocked(&mut r, false);
        click(&mut vcx, "DiskInUseRetry");
        let seqs = eject_seqs(&r);
        assert_eq!(seqs.len(), 2, "Retry after Blocked sends a new eject");
        let second = seqs[1];
        assert!(second > first);
        publish(&mut r, Some((second, EjectState::Running)));
        redraw(&mut vcx);
        click(&mut vcx, "DiskInUseRetry");
        eject(&mut r);
        assert_eq!(
            eject_seqs(&r),
            vec![first, second],
            "nothing sent while it runs"
        );
        publish(&mut r, Some((second, EjectState::Done)));
        r.vcx.run_until_parked();
        assert!(
            r.vcx.update(|_, cx| find_disk_in_use(IDX, cx)).is_none(),
            "the window closed"
        );
        assert_eq!(
            eject_toasts(&mut r),
            vec![(ToastKind::Success, SharedString::from("Disk ejected"))]
        );
    }

    #[gpui_kit::test]
    fn disk_in_use_eacces_note(cx: &mut TestAppContext) {
        let mut r = setup(cx);
        let (_, mut vcx, view) = open_blocked(&mut r, true);
        assert!(vcx.update(|_, cx| view.read(cx).eacces_note()));
        let note = vcx.debug_bounds("DiskInUseNote").expect("the note");
        let footer = vcx.debug_bounds("DiskInUseFooter").expect("the footer");
        let list = vcx.debug_bounds("DiskInUseList").expect("the list");
        assert!(note.top() >= list.bottom(), "below the list");
        assert!(note.bottom() <= footer.top(), "above the footer");
    }
}

mod net_p2 {
    use super::*;
    use tl_model::snapshot::{NetDetails, WifiInfo};
    use tl_model::system::{LinkState, Metric, NetTotals};
    use tl_ui::pages::performance::details::KeyValueModel;
    use tl_ui::pages::performance::net::{
        NetP2Inputs, net_details_model, signal_bar_colors, update_net_details_p2,
    };
    use tl_ui::theme::current;

    fn text(s: &str) -> InvField<Arc<str>> {
        InvField {
            value: Arc::from(s),
            validity: Validity::Valid,
        }
    }

    fn totals(state: LinkState, speed: Metric<u32>) -> NetTotals {
        NetTotals {
            device: DeviceIdx(1),
            rx_bps: Metric::valid(0.0),
            tx_bps: Metric::valid(0.0),
            rx_total: 0,
            tx_total: 0,
            link_speed_mbps: speed,
            state,
        }
    }

    fn ethernet() -> NetDetails {
        NetDetails {
            type_label: Arc::from("Ethernet"),
            connection: None,
            adapter_name: text("Intel I350 Gigabit Network Connection"),
            driver: text("igb"),
            mac: text("aa:bb:cc:dd:ee:ff"),
            ifindex: 2,
            link_speed_mbps: InvField {
                value: 1000,
                validity: Validity::Valid,
            },
            state: LinkState::Up,
            addresses: Arc::from([Arc::<str>::from("192.168.1.10/24")]),
            kind: NetKind::Wired,
            wifi: None,
        }
    }

    fn wifi() -> WifiInfo {
        WifiInfo {
            ssid: Arc::from("Home"),
            signal_dbm: Some(-52),
            signal_pct: 78,
            rx_mbps: Some(866.0),
            tx_mbps: Some(866.0),
            freq_mhz: 5180,
            channel: Some(36),
            security: Arc::from("WPA2"),
        }
    }

    fn model(
        cx: &mut TestAppContext,
        wifi: bool,
        d: Option<&NetDetails>,
        t: &NetTotals,
    ) -> KeyValueModel {
        let mut model = net_details_model(wifi);
        let inp = NetP2Inputs {
            now_boot_ns: 0,
            observed_end_ns: 0,
            details_observed_end_ns: 0,
            details: d,
            totals: Some(t),
            iface: Some("enp38s0"),
        };
        cx.update(|cx| {
            init_strings(cx, "en-US");
            model.refresh_strings(cx);
            model.refresh_labels(cx);
        });
        update_net_details_p2(&mut model, &inp);
        model
    }

    fn rows(model: &KeyValueModel) -> Vec<(String, String)> {
        model
            .to_kv_rows()
            .map(|r| (r.label.to_string(), r.value.text.to_string()))
            .collect()
    }

    fn value(model: &KeyValueModel, label: &str) -> String {
        rows(model)
            .into_iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("row {label}"))
    }

    const HEAD: [&str; 8] = [
        "Download speed",
        "Upload speed",
        "Total downloaded",
        "Total uploaded",
        "Adapter name",
        "Interface name",
        "Connection type",
        "Connection status",
    ];
    const WIFI_LABELS: [&str; 6] = [
        "SSID",
        "Signal",
        "RX bitrate",
        "TX bitrate",
        "Frequency",
        "Security",
    ];
    const TAIL: [&str; 7] = [
        "Link speed",
        "IPv4 address",
        "IPv6 address",
        "MAC address",
        "Interface index",
        "Driver",
        "Display unit",
    ];

    #[gpui_kit::test]
    fn net_details_order_ethernet(cx: &mut TestAppContext) {
        let t = totals(LinkState::Up, Metric::valid(1000));
        let d = ethernet();
        let m = model(cx, false, Some(&d), &t);
        let r = rows(&m);
        let labels: Vec<&str> = r.iter().map(|(l, _)| l.as_str()).collect();
        let want: Vec<&str> = HEAD.iter().chain(TAIL.iter()).copied().collect();
        assert_eq!(labels, want);
        assert_eq!(
            labels.len(),
            15,
            "update_net_details_p2 leaves Public IP hidden"
        );
        assert_eq!(
            value(&m, "Adapter name"),
            "Intel I350 Gigabit Network Connection"
        );
        assert_eq!(value(&m, "Interface name"), "enp38s0");
        assert_eq!(value(&m, "Connection type"), "Ethernet");
        assert_eq!(value(&m, "Connection status"), "Connected");
        assert_eq!(value(&m, "Link speed"), "1000 Mbps");
        assert_eq!(value(&m, "IPv4 address"), "192.168.1.10/24");
        assert_eq!(value(&m, "MAC address"), "aa:bb:cc:dd:ee:ff");
        assert_eq!(value(&m, "Interface index"), "2");
        assert_eq!(value(&m, "Driver"), "igb");
    }

    #[gpui_kit::test]
    fn net_details_order_wifi(cx: &mut TestAppContext) {
        let t = totals(LinkState::Up, Metric::valid(866));
        let mut d = ethernet();
        d.type_label = Arc::from("Wi-Fi");
        d.kind = NetKind::Wireless;
        d.wifi = Some(wifi());
        let m = model(cx, true, Some(&d), &t);
        let r = rows(&m);
        let labels: Vec<&str> = r.iter().map(|(l, _)| l.as_str()).collect();
        let want: Vec<&str> = HEAD
            .iter()
            .chain(WIFI_LABELS.iter())
            .chain(TAIL.iter())
            .copied()
            .collect();
        assert_eq!(labels, want);
        assert_eq!(labels.len(), 21);
        assert_eq!(value(&m, "SSID"), "Home");
        assert_eq!(value(&m, "Signal"), "-52 dBm (78%)");
        assert_eq!(value(&m, "RX bitrate"), "866 Mbps");
        assert_eq!(value(&m, "Frequency"), "5180 MHz, channel 36");
        assert_eq!(value(&m, "Security"), "WPA2");
        assert_eq!(value(&m, "Link speed"), "RX 866 Mbps · TX 866 Mbps");
    }

    #[gpui_kit::test]
    fn net_wifi_unavailable_without_data(cx: &mut TestAppContext) {
        let t = totals(LinkState::Up, Metric::valid(866));
        let d = ethernet();
        let m = model(cx, true, Some(&d), &t);
        for label in WIFI_LABELS {
            assert_eq!(value(&m, label), "Unavailable", "{label}");
        }
    }

    #[gpui_kit::test]
    fn signal_icon_bars(cx: &mut TestAppContext) {
        let r = open(cx);
        let theme = r.vcx.update(|_, cx| current(cx).clone());
        let (a, d) = (theme.accent, theme.divider);
        assert_ne!(a, d);
        assert_eq!(signal_bar_colors(78, &theme), [a, a, a, d]);
        assert_eq!(signal_bar_colors(24, &theme), [d, d, d, d]);
        assert_eq!(signal_bar_colors(25, &theme), [a, d, d, d]);
        assert_eq!(signal_bar_colors(100, &theme), [a, a, a, a]);
    }

    #[gpui_kit::test]
    fn net_link_speed_not_reported(cx: &mut TestAppContext) {
        let t = totals(LinkState::Up, Metric::with(Validity::Unsupported));
        let d = ethernet();
        let m = model(cx, false, Some(&d), &t);
        assert_eq!(value(&m, "Link speed"), "Not reported");
    }

    #[gpui_kit::test]
    fn net_ipv6_link_local_tooltip(cx: &mut TestAppContext) {
        let t = totals(LinkState::Up, Metric::valid(1000));
        let mut d = ethernet();
        d.addresses = Arc::from([
            Arc::<str>::from("192.168.1.10/24"),
            Arc::from("2001:db8::1/64"),
            Arc::from("fe80::1/64"),
        ]);
        let m = model(cx, false, Some(&d), &t);
        assert_eq!(value(&m, "IPv6 address"), "2001:db8::1/64");
        let tip = m
            .to_kv_rows()
            .find(|r| r.label.as_ref() == "IPv6 address")
            .and_then(|r| r.value.tooltip.clone());
        assert_eq!(tip.as_deref(), Some("Link-local: fe80::1/64"));
    }

    #[gpui_kit::test]
    fn net_link_speed_validity_words(cx: &mut TestAppContext) {
        let d = ethernet();
        for (v, word) in [
            (Validity::Failed, "Unavailable"),
            (Validity::Restricted, "Restricted"),
            (Validity::WarmingUp, "Collecting…"),
            (Validity::Unsupported, "Not reported"),
        ] {
            let t = totals(LinkState::Up, Metric::with(v));
            let m = model(cx, false, Some(&d), &t);
            assert_eq!(value(&m, "Link speed"), word, "{v:?}");
        }
    }

    #[gpui_kit::test]
    fn net_stale_mac_keeps_value_and_marks_stale(cx: &mut TestAppContext) {
        let t = totals(LinkState::Up, Metric::valid(1000));
        let mut d = ethernet();
        d.mac.validity = Validity::Stale;
        let m = model(cx, false, Some(&d), &t);
        assert_eq!(value(&m, "MAC address"), "aa:bb:cc:dd:ee:ff");
        let row = m
            .rows
            .iter()
            .find(|r| r.label == "perf-net-row-mac")
            .unwrap();
        assert_eq!(row.validity, Validity::Stale);
        assert!(row.tooltip.is_some());
    }

    #[gpui_kit::test]
    fn net_ipv6_link_local_only_keeps_tooltip(cx: &mut TestAppContext) {
        let t = totals(LinkState::Up, Metric::valid(1000));
        let mut d = ethernet();
        d.addresses = Arc::from([Arc::<str>::from("fe80::1/64")]);
        let m = model(cx, false, Some(&d), &t);
        assert_eq!(value(&m, "IPv6 address"), "fe80::1/64");
        let tip = m
            .to_kv_rows()
            .find(|r| r.label.as_ref() == "IPv6 address")
            .and_then(|r| r.value.tooltip.clone());
        assert_eq!(tip.as_deref(), Some("Link-local: fe80::1/64"));
    }

    #[gpui_kit::test]
    fn net_tunnel_unknown_state(cx: &mut TestAppContext) {
        let t = totals(LinkState::Unknown, Metric::with(Validity::Unsupported));
        let mut d = ethernet();
        d.type_label = Arc::from("Tunnel");
        d.kind = NetKind::VpnTunnel;
        let m = {
            let mut model = net_details_model(false);
            let inp = NetP2Inputs {
                now_boot_ns: 0,
                observed_end_ns: 0,
                details_observed_end_ns: 0,
                details: Some(&d),
                totals: Some(&t),
                iface: Some("tailscale0"),
            };
            cx.update(|cx| {
                init_strings(cx, "en-US");
                model.refresh_strings(cx);
            });
            update_net_details_p2(&mut model, &inp);
            model
        };
        assert_eq!(value(&m, "Interface name"), "tailscale0");
        assert_eq!(value(&m, "Connection type"), "Tunnel");
        assert_eq!(value(&m, "Connection status"), "Unknown");
    }

    /// A stale Adapter name, MAC address and Driver keep their value and report the age of
    /// the device details read, not the age of the 1 Hz system sample.
    #[gpui_kit::test]
    fn net_stale_inventory_uses_details_age(cx: &mut TestAppContext) {
        const S: u64 = 1_000_000_000;
        let t = totals(LinkState::Up, Metric::valid(1000));
        let mut d = ethernet();
        d.adapter_name.validity = Validity::Stale;
        d.mac.validity = Validity::Stale;
        d.driver.validity = Validity::Stale;
        let mut model = net_details_model(false);
        let inp = NetP2Inputs {
            now_boot_ns: 100 * S,
            observed_end_ns: 100 * S,
            details_observed_end_ns: 10 * S,
            details: Some(&d),
            totals: Some(&t),
            iface: Some("enp38s0"),
        };
        cx.update(|cx| {
            init_strings(cx, "en-US");
            model.refresh_strings(cx);
        });
        update_net_details_p2(&mut model, &inp);
        assert_eq!(
            value(&model, "Adapter name"),
            "Intel I350 Gigabit Network Connection"
        );
        for label in ["Adapter name", "MAC address", "Driver"] {
            let row = model
                .to_kv_rows()
                .find(|r| r.label.as_ref() == label)
                .unwrap();
            assert_eq!(row.value.validity, Validity::Stale, "{label}");
            let tip = row.value.tooltip.clone();
            assert_eq!(tip.as_deref(), Some("Last updated 1m 30s ago"), "{label}");
        }
    }

    /// Before the first device details read, a stale Link speed still reports the age of the
    /// system sample.
    #[gpui_kit::test]
    fn net_stale_link_speed_without_details(cx: &mut TestAppContext) {
        const S: u64 = 1_000_000_000;
        let t = totals(
            LinkState::Up,
            Metric {
                value: 1000,
                validity: Validity::Stale,
            },
        );
        let mut model = net_details_model(false);
        let inp = NetP2Inputs {
            now_boot_ns: 100 * S,
            observed_end_ns: 70 * S,
            details_observed_end_ns: 0,
            details: None,
            totals: Some(&t),
            iface: Some("enp38s0"),
        };
        cx.update(|cx| {
            init_strings(cx, "en-US");
            model.refresh_strings(cx);
        });
        update_net_details_p2(&mut model, &inp);
        assert_eq!(value(&model, "Link speed"), "1000 Mbps");
        let tip = model
            .to_kv_rows()
            .find(|r| r.label.as_ref() == "Link speed")
            .and_then(|r| r.value.tooltip.clone());
        assert_eq!(tip.as_deref(), Some("Last updated 30s ago"));
    }
}

fn inv_ok<T>(value: T) -> InvField<T> {
    InvField {
        value,
        validity: Validity::Valid,
    }
}

fn dimm(configured: InvField<u32>) -> MemoryModule {
    MemoryModule {
        capacity_bytes: inv_ok(16 * GIB),
        speed_mts: inv_ok(3200),
        configured_speed_mts: configured,
        kind: inv_ok(Arc::from("DDR4")),
        form_factor: inv_ok(Arc::from("DIMM")),
        ..MemoryModule::default()
    }
}

fn inventory(modules: Vec<MemoryModule>, slots: InvField<u32>) -> InventorySnapshot {
    let mut inv = InventorySnapshot::default();
    inv.header.validity = Validity::Valid;
    inv.modules = Arc::from(modules);
    inv.slots_total = slots;
    inv
}

#[test]
fn memory_module_rows() {
    let mut model = tl_ui::pages::performance::memory::memory_details_model();
    let inv = inventory(vec![dimm(inv_ok(3200)), dimm(inv_ok(3200))], inv_ok(4));
    // 32 GiB minus 716.80 MiB.
    let mem_total = 32 * GIB - 751_619_277;
    assert!(update_memory_details_modules(&mut model, &inv, mem_total));
    assert_eq!(value_of(&model, ROW_SPEED), "3200 MT/s");
    assert_eq!(value_of(&model, ROW_SLOTS), "2 of 4");
    assert_eq!(value_of(&model, ROW_FORM_FACTOR), "DIMM");
    assert_eq!(value_of(&model, ROW_TYPE), "DDR4");
    assert_eq!(value_of(&model, MEM_ROW_RESERVED), "716.80 MiB");
    let row = model
        .rows
        .iter()
        .find(|r| r.label == MEM_ROW_RESERVED)
        .expect("row");
    assert!(!row.hidden);
    assert_eq!(
        row.tooltip.as_deref(),
        Some("Sum of module sizes minus MemTotal")
    );
    assert!(!update_memory_details_modules(&mut model, &inv, mem_total));
}

#[test]
fn memory_module_rows_not_reported() {
    let mut model = tl_ui::pages::performance::memory::memory_details_model();
    let inv = inventory(Vec::new(), InvField::default());
    update_memory_details_modules(&mut model, &inv, 32 * GIB);
    for label in [ROW_SPEED, ROW_SLOTS, ROW_FORM_FACTOR, ROW_TYPE] {
        assert_eq!(value_of(&model, label), "Not reported", "{label}");
    }
    let hidden = |m: &tl_ui::pages::performance::details::KeyValueModel| {
        m.rows
            .iter()
            .find(|r| r.label == MEM_ROW_RESERVED)
            .expect("row")
            .hidden
    };
    assert!(hidden(&model));

    // A missing configured speed reads Not reported even when the rated speed is set.
    let mut model = tl_ui::pages::performance::memory::memory_details_model();
    let unsupported = InvField {
        value: 0,
        validity: Validity::Unsupported,
    };
    let inv = inventory(vec![dimm(unsupported)], inv_ok(2));
    update_memory_details_modules(&mut model, &inv, 32 * GIB);
    assert_eq!(value_of(&model, ROW_SPEED), "Not reported");
    assert_eq!(value_of(&model, ROW_SLOTS), "1 of 2");
}

#[gpui_kit::test]
fn memory_page_demands_inventory(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, "memory");
    assert!(last_demand(&r).inventory);
}

// ---- "Memory modules" block (E25-015) ----

use tl_ui::pages::performance::memory::{
    ModulesBlock, ROW_MOD_CAPACITY, ROW_MOD_CONFIGURED, ROW_MOD_DETAILS, ROW_MOD_FORM_FACTOR,
    ROW_MOD_MANUFACTURER, ROW_MOD_PART, ROW_MOD_RANK, ROW_MOD_SLOT, ROW_MOD_SPEED, ROW_MOD_TYPE,
    ROW_MOD_VOLTAGE, update_modules_block,
};

fn full_dimm(speed: u32, configured: u32) -> MemoryModule {
    MemoryModule {
        slot: inv_ok(Arc::from("P0 CHANNEL A (DIMM 0)")),
        capacity_bytes: inv_ok(16 * GIB),
        speed_mts: inv_ok(speed),
        configured_speed_mts: inv_ok(configured),
        kind: inv_ok(Arc::from("DDR4")),
        form_factor: inv_ok(Arc::from("DIMM")),
        manufacturer: inv_ok(Arc::from("Kingston")),
        part_number: inv_ok(Arc::from("KF3200C16D4/16GX")),
        rank: inv_ok(2),
        configured_voltage_mv: inv_ok(1200),
    }
}

fn module_block(modules: Vec<MemoryModule>, slots: InvField<u32>) -> ModulesBlock {
    let mut block = ModulesBlock::new();
    update_modules_block(&mut block, &inventory(modules, slots));
    block
}

fn publish_modules(r: &mut Rig, modules: Vec<MemoryModule>, slots: InvField<u32>) {
    r.engine.publish_with(|s| {
        let inv = Arc::make_mut(&mut s.inventory);
        inv.header.version += 1;
        inv.header.validity = Validity::Valid;
        inv.modules = Arc::from(modules);
        inv.slots_total = slots;
    });
    draw(r.vcx);
    draw(r.vcx);
}

fn memory_page(r: &mut Rig) -> Entity<MemoryPage> {
    select(r, "memory");
    view_of(r, "memory")
        .downcast::<MemoryPage>()
        .expect("a MemoryPage")
}

fn module_blocks(r: &mut Rig, memory: &Entity<MemoryPage>) -> usize {
    let memory = memory.clone();
    r.vcx
        .update(|window, cx| memory.update(cx, |page, cx| page.blocks(window, cx).len()))
}

#[gpui_kit::test]
fn modules_block_layout(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let memory = memory_page(&mut r);
    publish_modules(
        &mut r,
        vec![full_dimm(3200, 3200), full_dimm(3200, 3200)],
        inv_ok(4),
    );
    assert_eq!(module_blocks(&mut r, &memory), 1, "the modules block");
    r.vcx.update(|_, cx| {
        let block = memory.read(cx).modules();
        assert_eq!(block.slots_line.as_ref(), "Slots used: 2 of 4");
        assert_eq!(block.expanded, [true, true]);
        assert_eq!(block.modules[0].0.as_ref(), "Memory module 1");
        assert_eq!(block.modules[1].0.as_ref(), "Memory module 2");
        let grid = &block.modules[0].1;
        for (label, want) in [
            (ROW_MOD_SLOT, "P0 CHANNEL A (DIMM 0)"),
            (ROW_MOD_CAPACITY, "16.00 GiB"),
            (ROW_MOD_SPEED, "3200 MT/s"),
            (ROW_MOD_TYPE, "DDR4"),
            (ROW_MOD_FORM_FACTOR, "DIMM"),
            (ROW_MOD_MANUFACTURER, "Kingston"),
            (ROW_MOD_PART, "KF3200C16D4/16GX"),
            (ROW_MOD_RANK, "2"),
            (ROW_MOD_VOLTAGE, "1.20 V"),
        ] {
            assert_eq!(value_of(grid, label), want, "{label}");
        }
    });
    for id in [
        "perf-mem-module-chevron-0",
        "perf-mem-module-chevron-1",
        "perf-mem-module-grid-0",
        "perf-mem-module-grid-1",
    ] {
        assert!(r.vcx.debug_bounds(id).is_some(), "{id} is drawn");
    }
}

#[gpui_kit::test]
fn modules_collapsed_when_more_than_4(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let memory = memory_page(&mut r);
    publish_modules(&mut r, vec![full_dimm(3200, 3200); 6], inv_ok(8));
    r.vcx.update(|_, cx| {
        let block = memory.read(cx).modules();
        assert_eq!(block.slots_line.as_ref(), "Slots used: 6 of 8");
        assert_eq!(block.expanded, [false; 6]);
    });
    assert!(r.vcx.debug_bounds("perf-mem-module-grid-0").is_none());

    let at = r
        .vcx
        .debug_bounds("perf-mem-module-chevron-0")
        .expect("the first chevron")
        .center();
    r.vcx.simulate_click(at, gpui_kit::Modifiers::none());
    draw(r.vcx);
    r.vcx.update(|_, cx| {
        let block = memory.read(cx).modules();
        assert_eq!(block.expanded, [true, false, false, false, false, false]);
    });
    assert!(r.vcx.debug_bounds("perf-mem-module-grid-0").is_some());
    assert!(r.vcx.debug_bounds("perf-mem-module-grid-1").is_none());
}

#[test]
fn module_configured_speed_only_when_differs() {
    let block = module_block(vec![full_dimm(3200, 2933)], inv_ok(2));
    let grid = &block.modules[0].1;
    assert_eq!(value_of(grid, ROW_MOD_SPEED), "3200 MT/s");
    assert_eq!(value_of(grid, ROW_MOD_CONFIGURED), "2933 MT/s");
    let hidden = |g: &tl_ui::pages::performance::details::KeyValueModel| {
        g.rows
            .iter()
            .find(|r| r.label == ROW_MOD_CONFIGURED)
            .expect("row")
            .hidden
    };
    assert!(!hidden(grid));

    let block = module_block(vec![full_dimm(3200, 3200)], inv_ok(2));
    assert!(hidden(&block.modules[0].1), "equal speeds hide the row");

    // A change back and forth shows and hides the row again.
    let mut block = block;
    update_modules_block(
        &mut block,
        &inventory(vec![full_dimm(3200, 2933)], inv_ok(2)),
    );
    assert!(!hidden(&block.modules[0].1));
}

#[test]
fn module_voltage_and_missing_fields() {
    let mut dimm = full_dimm(3200, 3200);
    dimm.configured_voltage_mv = inv_ok(1100);
    dimm.rank = InvField::default();
    let block = module_block(vec![dimm], InvField::default());
    assert_eq!(block.slots_line.as_ref(), "Slots used: 1", "no slot total");
    let grid = &block.modules[0].1;
    assert_eq!(value_of(grid, ROW_MOD_VOLTAGE), "1.10 V");
    assert_eq!(value_of(grid, ROW_MOD_RANK), "Not reported");

    let mut dimm = full_dimm(3200, 3200);
    dimm.configured_voltage_mv = inv_ok(0);
    let block = module_block(vec![dimm], inv_ok(2));
    assert_eq!(
        value_of(&block.modules[0].1, ROW_MOD_VOLTAGE),
        "Not reported"
    );
}

#[test]
fn modules_not_reported() {
    let block = module_block(Vec::new(), InvField::default());
    assert!(block.not_reported);
    assert!(block.modules.is_empty());
    assert!(block.slots_line.is_empty());
    assert_eq!(
        value_of(block.details_model(), ROW_MOD_DETAILS),
        "Not reported"
    );

    // Failed inventory reads the same; warming up hides the block.
    let mut block = ModulesBlock::new();
    let mut inv = inventory(vec![full_dimm(3200, 3200)], inv_ok(2));
    inv.header.validity = Validity::Failed;
    update_modules_block(&mut block, &inv);
    assert!(block.not_reported && !block.is_hidden());
    inv.header.validity = Validity::WarmingUp;
    update_modules_block(&mut block, &inv);
    assert!(block.is_hidden());
}

#[gpui_kit::test]
fn no_serial_in_copy_text(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let memory = memory_page(&mut r);
    publish_modules(&mut r, vec![full_dimm(3200, 3200)], inv_ok(2));
    r.vcx.update(|_, cx| {
        let page = memory.read(cx);
        let mut text = String::new();
        page.copy_text(&mut text);
        page.modules().copy_into(&mut text);
        assert!(text.contains("Memory module 1"), "{text}");
        assert!(!text.to_lowercase().contains("serial"), "{text}");
    });
}

mod net_settings {
    use super::*;

    use gpui_kit::base::Root;
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point};
    use tl_model::actions::{
        ActionError, ActionEvent, ActionKind, ActionRequest, ActionState, LaunchSpec, LaunchWhat,
    };
    use tl_model::command::Command;
    use tl_ui::action_tracker::{ActionTracker, GlobalActionTracker};
    use tl_ui::components::menu::{MenuEntryKind, TlMenuPopup, open_context_menu};
    use tl_ui::components::toast::{self, ToastQueue};
    use tl_ui::pages::performance::PAGE_PANEL_ID;
    use tl_ui::pages::performance::actions::PerfOpenNetworkSettings;
    use tl_ui::pages::performance::net::{
        NetSettingsDesktop, SETTINGS_LINK_ID, SettingsDesktop, settings_desktop,
    };

    const NET: &str = "net:enp38s0";

    /// The Performance tab inside a kit `Root` with a toast queue and an action tracker, the
    /// Network settings desktop set to `desktop`.
    pub(super) fn open_root(
        cx: &mut TestAppContext,
        desktop: Option<SettingsDesktop>,
    ) -> (
        Arc<FakeEngine>,
        VisualTestContext,
        Entity<PerformancePage>,
        Entity<ToastQueue>,
    ) {
        let engine = install(cx);
        let client: Arc<dyn EngineClient> = engine.clone();
        cx.update(|cx| {
            toast::init(cx);
            cx.set_global(NetSettingsDesktop(desktop));
        });
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        let demand = cx.new(|cx| DemandAggregator::new(Arc::clone(&client), &hub, cx));
        let page = cx.new(|cx| PerformancePage::new(hub, demand, cx));
        let root_page = page.clone();
        let window = cx.add_window(move |window, cx| Root::new(root_page, window, cx));
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| window.activate_window());
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(1280.), px(800.)));
        let queue = vcx.update(|window, cx| cx.new(|cx| ToastQueue::new(window, cx, px(64.))));
        vcx.update(|_, cx| {
            let tracker = cx.new(|cx| ActionTracker::new(client, cx));
            cx.set_global(GlobalActionTracker(tracker));
        });
        draw(&mut vcx);
        (engine, vcx, page, queue)
    }

    fn net_page(r: &mut Rig) -> Entity<NetPage> {
        view_of(r, NET).downcast::<NetPage>().expect("a NetPage")
    }

    fn right_click(cx: &mut VisualTestContext, at: Point<Pixels>) {
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

    fn menu_labels(r: &mut Rig) -> Vec<String> {
        let at = r
            .vcx
            .debug_bounds(PAGE_PANEL_ID)
            .expect("the page panel")
            .center();
        right_click(r.vcx, at);
        let menu: Entity<TlMenuPopup> = r
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("a context menu is open");
        r.vcx.update(|window, cx| {
            menu.read(cx)
                .entries_for_test(window)
                .into_iter()
                .filter(|e| e.kind != MenuEntryKind::Separator)
                .map(|e| e.label.map(|l| l.to_string()).unwrap_or_default())
                .collect()
        })
    }

    fn launches(engine: &FakeEngine) -> Vec<ActionRequest> {
        engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Action(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    fn open_settings(r: &mut Rig) {
        let page = net_page(r);
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            focus.dispatch_action(&PerfOpenNetworkSettings, window, cx);
        });
        r.vcx.run_until_parked();
    }

    #[test]
    fn desktop_from_xdg_list() {
        assert_eq!(settings_desktop(Some("KDE")), Some(SettingsDesktop::Kde));
        assert_eq!(
            settings_desktop(Some("ubuntu:GNOME")),
            Some(SettingsDesktop::Gnome)
        );
        assert_eq!(
            settings_desktop(Some("GNOME:KDE")),
            Some(SettingsDesktop::Kde)
        );
        assert_eq!(settings_desktop(Some("sway")), None);
        assert_eq!(settings_desktop(Some("kde")), None);
        assert_eq!(settings_desktop(None), None);
    }

    #[gpui_kit::test]
    fn network_settings_link_visibility(cx: &mut TestAppContext) {
        let (engine, mut vcx, page, _queue) = open_root(cx, Some(SettingsDesktop::Kde));
        let mut r = Rig {
            page,
            engine,
            vcx: &mut vcx,
        };
        select(&mut r, NET);
        assert!(r.vcx.debug_bounds(SETTINGS_LINK_ID).is_some(), "link row");
        assert!(menu_labels(&mut r).contains(&"Network settings".to_owned()));
        let net = net_page(&mut r);
        assert_eq!(r.vcx.update(|_, cx| net.read(cx).focus_order().len()), 3);

        let (engine, mut vcx, page, _queue) = open_root(cx, None);
        let mut r = Rig {
            page,
            engine,
            vcx: &mut vcx,
        };
        select(&mut r, NET);
        assert!(
            r.vcx.debug_bounds(SETTINGS_LINK_ID).is_none(),
            "no link row"
        );
        assert!(!menu_labels(&mut r).contains(&"Network settings".to_owned()));
        let net = net_page(&mut r);
        assert_eq!(r.vcx.update(|_, cx| net.read(cx).focus_order().len()), 2);
    }

    #[gpui_kit::test]
    fn network_settings_launch_kde(cx: &mut TestAppContext) {
        let (engine, mut vcx, page, _queue) = open_root(cx, Some(SettingsDesktop::Kde));
        let mut r = Rig {
            page,
            engine,
            vcx: &mut vcx,
        };
        select(&mut r, NET);
        open_settings(&mut r);
        let sent = launches(&r.engine);
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].kind,
            ActionKind::Launch(LaunchSpec {
                what: LaunchWhat::Command("kcmshell6 kcm_networkmanagement".into()),
                as_admin: false,
                cwd: None,
            })
        );
        assert!(sent[0].targets.is_empty());
    }

    #[gpui_kit::test]
    fn network_settings_launch_gnome(cx: &mut TestAppContext) {
        let (engine, mut vcx, page, _queue) = open_root(cx, Some(SettingsDesktop::Gnome));
        let mut r = Rig {
            page,
            engine,
            vcx: &mut vcx,
        };
        select(&mut r, NET);
        open_settings(&mut r);
        let sent = launches(&r.engine);
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].kind,
            ActionKind::Launch(LaunchSpec {
                what: LaunchWhat::DbusActivate {
                    bus_name: "org.gnome.Settings".into(),
                    object_path: "/org/gnome/Settings".into(),
                    action: "launch-panel".into(),
                    parameter: Some("network".into()),
                },
                as_admin: false,
                cwd: None,
            })
        );
    }

    #[gpui_kit::test]
    fn network_settings_failure_toast(cx: &mut TestAppContext) {
        let (engine, mut vcx, page, _queue) = open_root(cx, Some(SettingsDesktop::Kde));
        let mut r = Rig {
            page,
            engine,
            vcx: &mut vcx,
        };
        select(&mut r, NET);
        open_settings(&mut r);
        let sent = launches(&r.engine);
        r.engine.emit(ActionEvent {
            id: sent[0].id,
            target: None,
            state: ActionState::Failed(ActionError::PermissionDenied),
        });
        draw(r.vcx);
        let texts: Vec<String> = r.vcx.update(|window, cx| {
            ToastQueue::of_window(window, cx)
                .expect("toast queue")
                .read(cx)
                .visible_texts()
                .iter()
                .map(|t| t.to_string())
                .collect()
        });
        assert_eq!(texts.len(), 1, "{texts:?}");
        assert!(
            texts[0].starts_with("Could not open network settings."),
            "{texts:?}"
        );
    }
}

// ---- Aggregate pages (E25-035) ----

fn gpu_entry(idx: u16, name: &str, bus: &str) -> DeviceEntry {
    let mut e = entry(
        idx,
        DeviceIdentity::Gpu {
            uuid: [0; 16],
            pci_bus_id: Arc::from(bus),
        },
        None,
    );
    e.display_name = Arc::from(name);
    e
}

fn aggregate_entry(idx: u16, kind: AggregateKind) -> DeviceEntry {
    entry(idx, DeviceIdentity::Aggregate(kind), None)
}

fn aggregate_row(kind: DeviceKind, k: &DeviceKey, idx: u16) -> DeviceRow {
    DeviceRow {
        key: k.clone(),
        kind,
        idx: DeviceIdx(idx),
        generation: 0,
        net_class: None,
        fan_connected: None,
    }
}

/// A hub and demand over a catalog of `entries` and the disk kinds in `kinds` (idx, kind).
fn aggregate_hub(
    cx: &mut TestAppContext,
    entries: Vec<DeviceEntry>,
    kinds: &[(u16, &str)],
) -> (Entity<SnapshotHub>, Entity<DemandAggregator>) {
    let engine = install(cx);
    let kinds: Vec<_> = kinds
        .iter()
        .map(|(i, k)| {
            (
                DeviceIdx(*i),
                DiskDetails {
                    kind: Arc::from(*k),
                    ..DiskDetails::default()
                },
            )
        })
        .collect();
    engine.publish_with(|s| {
        s.devices = Arc::new(DeviceCatalog {
            version: 2,
            entries: Arc::from(entries),
        });
        let details = Arc::make_mut(&mut s.device_details);
        details.header.version += 1;
        details.disks = Arc::from(kinds);
    });
    let client: Arc<dyn EngineClient> = engine;
    let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
    let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
    (hub, demand)
}

#[gpui_kit::test]
fn aggregate_type_multiple(cx: &mut TestAppContext) {
    let entries = vec![
        disk(0, "nvme0n1"),
        disk(1, "nvme1n1"),
        disk(2, "sda"),
        aggregate_entry(3, AggregateKind::Disks),
    ];
    let (hub, demand) = aggregate_hub(cx, entries, &[(0, "NVMe"), (1, "NVMe"), (2, "SSD")]);
    let k = key("aggregate:disk");
    let row = aggregate_row(DeviceKind::AggregateDisk, &k, 3);
    let view = cx.update(|cx| build_page(&k, &row, &hub, &demand, cx));
    let page = view
        .downcast::<AggregateDiskPage>()
        .expect("an AggregateDiskPage");
    cx.update(|cx| {
        let page = page.read(cx);
        assert_eq!(
            page.header(),
            (
                SharedString::from("Aggregate storage"),
                SharedString::from("3 disks")
            )
        );
        assert_eq!(value_of(page.details(), AGG_ROW_TYPE), "Multiple");
        assert_eq!(value_of(page.details(), ROW_DISKS), "nvme0n1, nvme1n1, sda");
    });
}

#[gpui_kit::test]
fn aggregate_exists_only_with_two_devices(cx: &mut TestAppContext) {
    let entries = vec![
        gpu_entry(0, "NVIDIA GeForce RTX 4090", "0000:2d:00.0"),
        aggregate_entry(1, AggregateKind::Gpus),
    ];
    let catalog = DeviceCatalog {
        version: 2,
        entries: Arc::from(entries.clone()),
    };
    let (hub, demand) = aggregate_hub(cx, entries, &[]);
    let k = key("aggregate:gpu");
    let row = aggregate_row(DeviceKind::AggregateGpu, &k, 1);
    let view = cx.update(|cx| build_page(&k, &row, &hub, &demand, cx));
    assert!(view.downcast::<PlaceholderPage>().is_ok());

    let (order, overrides) = (Vec::new(), std::collections::HashMap::new());
    let settings = PerformanceSettingsView {
        order: &order,
        overrides: &overrides,
        show_disks: true,
        show_gpus: true,
        show_fans: true,
        show_disconnected_fans: true,
        show_batteries: true,
        show_network_wired: true,
        show_network_wireless: true,
        show_network_vpn: true,
        show_network_virtual: true,
        show_network_other: true,
    };
    let mut rows = Vec::new();
    sidebar_order(&catalog, &[], &settings, &mut rows);
    assert!(rows.iter().all(|r| &*r.key.0 != "aggregate:gpu"));
}

#[gpui_kit::test]
fn aggregate_gpu_adapters_row(cx: &mut TestAppContext) {
    let entries = vec![
        gpu_entry(0, "AMD Radeon RX 7900 XTX", "0000:03:00.0"),
        gpu_entry(1, "NVIDIA GeForce RTX 4090", "0000:2d:00.0"),
        aggregate_entry(2, AggregateKind::Gpus),
    ];
    let (hub, demand) = aggregate_hub(cx, entries, &[]);
    let k = key("aggregate:gpu");
    let row = aggregate_row(DeviceKind::AggregateGpu, &k, 2);
    let view = cx.update(|cx| build_page(&k, &row, &hub, &demand, cx));
    let page = view
        .downcast::<AggregateGpuPage>()
        .expect("an AggregateGpuPage");
    cx.update(|cx| {
        let page = page.read(cx);
        assert_eq!(
            value_of(page.details(), ROW_ADAPTERS),
            "AMD Radeon RX 7900 XTX, NVIDIA GeForce RTX 4090"
        );
        assert_eq!(value_of(page.details(), AGG_ROW_TYPE), "Multiple");
        assert_eq!(page.header().1, "2 adapters");
    });
}

fn publish_displays(r: &mut Rig, displays: Vec<DisplayMode>) {
    r.engine.publish_with(|s| {
        let inv = Arc::make_mut(&mut s.inventory);
        inv.header.version += 1;
        inv.graphics = Arc::from(
            [GraphicsAdapter {
                device: Some(DeviceIdx(2)),
                displays: Arc::from(displays),
                ..GraphicsAdapter::default()
            }]
            .as_slice(),
        );
    });
    draw(r.vcx);
    draw(r.vcx);
}

/// No enabled display gives one block with the heading and `No display connected`; one display
/// gives one formatted row.
#[gpui_kit::test]
fn gpu_displays_empty_text(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, GPU);
    let gpu = view_of(&mut r, GPU)
        .downcast::<GpuPage>()
        .expect("a GpuPage");
    publish_displays(&mut r, Vec::new());
    r.vcx.update(|window, cx| {
        gpu.update(cx, |page, cx| {
            assert!(page.display_rows().is_empty());
            assert_eq!(page.words().displays.as_ref(), "Displays");
            assert_eq!(page.words().no_display.as_ref(), "No display connected");
            assert_eq!(page.blocks(window, cx).len(), 1);
        });
    });
    publish_displays(
        &mut r,
        vec![DisplayMode {
            width: 1920,
            height: 1200,
            refresh_mhz: 59_950,
            primary: true,
            monitor_name: Some(Arc::from("DELL U2415")),
            connector: DisplayConnector::DisplayPort,
            max_bpc: Some(10),
            rotation: Some(2),
            scale: Some(1.1),
        }],
    );
    r.vcx.update(|_, cx| {
        let rows: Vec<&str> = gpu.read(cx).display_rows().iter().map(|s| &**s).collect();
        assert_eq!(
            rows,
            ["DELL U2415 (primary) · DisplayPort · 1920x1200 @ 59.95 Hz · Max bit depth 10 · Portrait (left) · 110%"]
        );
    });
}

#[gpui_kit::test]
fn gpu_page_demands_inventory(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, GPU);
    assert!(last_demand(&r).inventory);
}

// ---- Copy text of the device pages (E25-030) ----

/// The text `copy_text` writes for `page`.
fn copied<T: DevicePage>(r: &mut Rig, page: &Entity<T>) -> String {
    r.vcx.update(|_, cx| {
        let mut out = String::new();
        page.read(cx).copy_text(&mut out);
        out
    })
}

fn module_with_slot(slot: &str) -> MemoryModule {
    MemoryModule {
        slot: inv_ok(Arc::from(slot)),
        ..full_dimm(3200, 3200)
    }
}

#[gpui_kit::test]
fn copy_text_memory_format(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let memory = memory_page(&mut r);
    publish_swaps(
        &mut r,
        vec![swap_device("/dev/nvme0n1p3", SwapKindModel::Partition, -2)],
        Validity::Valid,
    );
    publish_modules(
        &mut r,
        vec![
            module_with_slot("P0 CHANNEL A (DIMM 0)"),
            module_with_slot("P0 CHANNEL B (DIMM 0)"),
        ],
        inv_ok(4),
    );
    let text = copied(&mut r, &memory);
    assert!(text.starts_with("Memory\n    "), "{text}");
    assert!(
        text.contains(
            "\nSwap devices\n    Name: /dev/nvme0n1p3\n    Type: partition\n    Size: 8.00 GiB\n    Used: 1.00 GiB\n    Priority: -2\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("\nMemory modules\nMemory module 1\n    Slot: P0 CHANNEL A (DIMM 0)\n"),
        "{text}"
    );
    assert!(
        text.contains("\nMemory module 2\n    Slot: P0 CHANNEL B"),
        "{text}"
    );
    let modules = &text[text.find("\nMemory modules\n").expect(&text)..];
    assert!(!modules.contains("Slots used"), "{text}");
    assert!(text.ends_with('\n') && !text.ends_with("\n\n"), "{text}");
}

#[gpui_kit::test]
fn copy_text_memory_compressed_block_comes_first(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let memory = memory_page(&mut r);
    publish_swaps(
        &mut r,
        vec![swap_device("/dev/zram0", SwapKindModel::Zram, 100)],
        Validity::Valid,
    );
    r.engine.publish_with(|s| {
        let details = Arc::make_mut(&mut s.device_details);
        details.header.version += 1;
        details.memory = Arc::new(MemoryDetails {
            swaps: InvField {
                value: Arc::from(vec![swap_device("/dev/zram0", SwapKindModel::Zram, 100)]),
                validity: Validity::Valid,
            },
            ..zram_details()
        });
        let sys = Arc::make_mut(&mut s.system);
        sys.header.version += 1;
        sys.memory = zram_mem(100 * GIB, 25 * GIB);
    });
    draw(r.vcx);
    draw(r.vcx);
    let text = copied(&mut r, &memory);
    let compressed = text.find("\nCompressed memory\n");
    let swap = text.find("\nSwap devices\n");
    assert!(compressed.is_some() && swap.is_some(), "{text}");
    assert!(compressed < swap, "{text}");
    assert!(text.contains("\nzswap\n"), "{text}");
}

#[gpui_kit::test]
fn no_serial_in_copy_text_whole_page(cx: &mut TestAppContext) {
    let mut r = open(cx);
    let memory = memory_page(&mut r);
    publish_modules(&mut r, vec![full_dimm(3200, 3200)], inv_ok(2));
    let text = copied(&mut r, &memory);
    assert!(text.contains("Memory module 1"), "{text}");
    assert!(!text.to_lowercase().contains("serial"), "{text}");
}

#[gpui_kit::test]
fn copy_text_disk_partitions(cx: &mut TestAppContext) {
    use tl_model::snapshot::{PartitionRow, PartitionTable};
    let part = |name: &str, start: u64| PartitionRow {
        name: Arc::from(name),
        start_bytes: start,
        size_bytes: 8 * GIB,
        holder: None,
        mount: None,
        extra_mounts: Arc::from([]),
        label: None,
        fs_type: Some(Arc::from("ext4")),
        usage_pct: valid(42.0),
        flags: Arc::from([]),
        type_name: Arc::from("Linux filesystem"),
        read_only: false,
        unallocated: false,
    };
    let gap = PartitionRow {
        name: Arc::from(""),
        fs_type: None,
        usage_pct: InvField {
            value: 0.,
            validity: Validity::Failed,
        },
        type_name: Arc::from(""),
        unallocated: true,
        ..part("", 24 * GIB)
    };
    let mut r = open(cx);
    select(&mut r, "disk:sda");
    r.engine.publish_with(|s| {
        let details = Arc::make_mut(&mut s.device_details);
        details.header.version += 1;
        details.disks = Arc::from([(
            DeviceIdx(3),
            DiskDetails {
                partitions: Some(Arc::new(PartitionTable {
                    kind: Arc::from("GPT"),
                    rows: Arc::from(vec![part("sda1", 0), part("sda2", 8 * GIB), gap.clone()]),
                })),
                ..DiskDetails::default()
            },
        )]);
    });
    draw(r.vcx);
    draw(r.vcx);
    let disk = view_of(&mut r, "disk:sda")
        .downcast::<DiskPage>()
        .expect("a DiskPage");
    let text = copied(&mut r, &disk);
    assert!(text.starts_with("Disk "), "{text}");
    let at = text.find("\n2 partitions - GPT\n").expect(&text);
    let paragraphs: Vec<&str> = text[at + 1..].trim_end().split("\n\n").collect();
    assert_eq!(paragraphs.len(), 3, "{text}");
    assert!(paragraphs[0].starts_with("2 partitions - GPT\n    Partition: sda1\n"));
    assert!(paragraphs[1].starts_with("    Partition: sda2\n"));
    assert!(paragraphs[2].starts_with("    Partition: -\n"));
    assert!(paragraphs[2].contains("    Type: Unallocated"), "{text}");
    assert!(!text.to_lowercase().contains("smart"), "{text}");
}

#[gpui_kit::test]
fn copy_text_gpu_displays(cx: &mut TestAppContext) {
    let mode = |name: &str, primary: bool| DisplayMode {
        width: 1920,
        height: 1200,
        refresh_mhz: 59_950,
        primary,
        monitor_name: Some(Arc::from(name)),
        connector: DisplayConnector::DisplayPort,
        max_bpc: None,
        rotation: None,
        scale: None,
    };
    let mut r = open(cx);
    select(&mut r, GPU);
    let gpu = view_of(&mut r, GPU)
        .downcast::<GpuPage>()
        .expect("a GpuPage");
    publish_displays(&mut r, Vec::new());
    let text = copied(&mut r, &gpu);
    assert!(
        text.ends_with("\nDisplays\n    No display connected\n"),
        "{text}"
    );
    publish_displays(&mut r, vec![mode("DELL A", true), mode("DELL B", false)]);
    let (text, rows) = r.vcx.update(|_, cx| {
        let page = gpu.read(cx);
        let mut out = String::new();
        page.copy_text(&mut out);
        (out, page.display_rows().to_vec())
    });
    assert_eq!(rows.len(), 2);
    assert!(
        text.ends_with(&format!("\nDisplays\n    {}\n    {}\n", rows[0], rows[1])),
        "{text}"
    );
}

#[gpui_kit::test]
fn copy_text_net_rows(cx: &mut TestAppContext) {
    let mut r = open(cx);
    select(&mut r, "net:enp38s0");
    let net = view_of(&mut r, "net:enp38s0")
        .downcast::<NetPage>()
        .expect("a NetPage");
    let text = copied(&mut r, &net);
    assert!(text.starts_with("enp38s0\nUnknown adapter\n"), "{text}");
    assert!(text.contains("\n    Display unit: Mbps\n"), "{text}");
    assert!(text.contains("\n    Public IP: Not fetched\n"), "{text}");
    assert!(text.ends_with('\n'), "{text}");
}

#[gpui_kit::test]
fn copy_text_fan_and_battery(cx: &mut TestAppContext) {
    let mut r = open(cx);
    publish_p2(&mut r);
    select(&mut r, "fan:nct6797:2");
    let fan = view_of(&mut r, "fan:nct6797:2")
        .downcast::<FanPage>()
        .expect("a FanPage");
    let text = copied(&mut r, &fan);
    assert!(text.starts_with("Fan 1\nnct6797\n    "), "{text}");
    assert!(text.ends_with('\n'), "{text}");
}

mod copy_toast {
    use super::*;
    use tl_ui::components::toast::ToastQueue;
    use tl_ui::pages::performance::actions::PerfCopyDeviceInfo;

    /// Ctrl+C on a page copies its text and pushes the toast once.
    #[gpui_kit::test]
    fn copy_dispatches_toast_once_per_page(cx: &mut TestAppContext) {
        let (_engine, mut vcx, page, _queue) = net_settings::open_root(cx, None);
        for k in ["memory", "disk:sda", "net:enp38s0", GPU] {
            let sidebar = page.read_with(&vcx, |p, _| p.sidebar().clone());
            vcx.update(|_, cx| sidebar.update(cx, |s, cx| s.select(key(k), cx)));
            draw(&mut vcx);
            let focus = page.read_with(&vcx, |p, _| p.primary_focus());
            vcx.update(|window, cx| focus.focus(window, cx));
            vcx.dispatch_action(PerfCopyDeviceInfo);
            vcx.run_until_parked();
            let clip = vcx.read_from_clipboard().and_then(|i| i.text());
            assert!(clip.is_some_and(|t| t.contains("\n    ")), "{k}");
            let texts: Vec<String> = vcx.update(|window, cx| {
                ToastQueue::of_window(window, cx)
                    .expect("toast queue")
                    .read(cx)
                    .visible_texts()
                    .iter()
                    .map(|t| t.to_string())
                    .collect()
            });
            let n = texts
                .iter()
                .filter(|t| t.as_str() == "Copied metrics to clipboard")
                .count();
            assert_eq!(n, 1, "{k}: {texts:?}");
        }
    }
}

/// The Fan page menu: "Rename fan…" and "Temperature sensor >" with `[fans]` persistence
/// (E25-032, PERF-077, UI-PRF-034).
mod fan_menu {
    use super::*;

    use gpui_kit::base::Root;
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point};
    use tl_ui::components::dialog::TlDialogView;
    use tl_ui::components::menu::{MenuEntryInfo, MenuEntryKind, TlMenuPopup, open_context_menu};
    use tl_ui::components::window_frame::TlSecondaryWindow;
    use tl_ui::pages::performance::PAGE_PANEL_ID;
    use tl_ui::pages::performance::actions::{PerfRenameFan, PerfSetFanTempSensor};

    const FAN: &str = "fan:nct6797:2";
    const FAN_IDX: u16 = 10;

    fn sensor_key(n: u16) -> String {
        format!("nct6797:/sys/devices/platform/nct6797.2592/hwmon/hwmon3:temp{n}")
    }

    fn temp(idx: u16, c: f32) -> (DeviceIdx, SensorReading) {
        (
            DeviceIdx(idx),
            SensorReading {
                device: DeviceIdx(idx),
                kind: SensorKind::Temperature,
                value: Metric::valid(c),
                pwm_pct: Metric::with(Validity::Unsupported),
                fan_control: None,
                fan_connected: false,
                fan_max_rpm: None,
            },
        )
    }

    /// Seven fans on `nct6797` and twelve hwmon temperature sensors (idx 20 to 31, 48 °C up).
    fn publish_fan_and_sensors(r: &mut Rig) {
        r.engine.publish_with(|s| {
            let mut entries = catalog();
            let mut sensors = Vec::new();
            for n in 1..=7u16 {
                entries.push(entry(
                    9 + (n - 1),
                    DeviceIdentity::Fan {
                        chip: Arc::from("nct6797"),
                        index: n,
                    },
                    None,
                ));
                sensors.push((
                    DeviceIdx(9 + (n - 1)),
                    SensorReading {
                        device: DeviceIdx(9 + (n - 1)),
                        kind: SensorKind::Fan,
                        value: Metric::valid(1200.0),
                        pwm_pct: Metric::valid(45.0),
                        fan_control: Some(FanControlMode::Automatic),
                        fan_connected: true,
                        fan_max_rpm: Some(2000),
                    },
                ));
            }
            for n in 1..=12u16 {
                let mut e = entry(
                    19 + n,
                    DeviceIdentity::Sensor {
                        chip: Arc::from("nct6797"),
                        label: Arc::from(format!("temp{n}")),
                    },
                    None,
                );
                e.persistent_key = Arc::from(format!("sensor:{}", sensor_key(n)));
                entries.push(e);
                sensors.push(temp(19 + n, 47.6 + f32::from(n)));
            }
            s.devices = Arc::new(DeviceCatalog {
                version: 2,
                entries: Arc::from(entries),
            });
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            sys.sensors = Arc::from(sensors);
        });
        draw(r.vcx);
    }

    /// Sets the temperature of each sensor in `values` (device idx, °C).
    fn set_readings(r: &mut Rig, values: &[(u16, f32)]) {
        r.engine.publish_with(|s| {
            let sys = Arc::make_mut(&mut s.system);
            sys.header.version += 1;
            let mut sensors = sys.sensors.to_vec();
            for (idx, r) in &mut sensors {
                if let Some((_, c)) = values.iter().find(|(i, _)| DeviceIdx(*i) == *idx) {
                    r.value = Metric::valid(*c);
                }
            }
            sys.sensors = Arc::from(sensors);
        });
        draw(r.vcx);
    }

    /// The labels of the "Temperature sensor" submenu.
    fn sensor_labels(r: &mut Rig) -> Vec<String> {
        sensor_entries(r)
            .iter()
            .filter(|e| e.kind != MenuEntryKind::Separator)
            .filter_map(|e| e.label.as_deref().map(str::to_owned))
            .collect()
    }

    fn fan_page(r: &mut Rig) -> Entity<FanPage> {
        view_of(r, FAN).downcast::<FanPage>().expect("a FanPage")
    }

    fn right_click(cx: &mut VisualTestContext, at: Point<Pixels>) {
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

    /// The entries of the "Temperature sensor" submenu.
    fn sensor_entries(r: &mut Rig) -> Vec<MenuEntryInfo> {
        let at = r
            .vcx
            .debug_bounds(PAGE_PANEL_ID)
            .expect("the page panel")
            .center();
        right_click(r.vcx, at);
        let menu: Entity<TlMenuPopup> = r
            .vcx
            .update(|window, cx| open_context_menu(window, cx))
            .expect("a context menu is open");
        r.vcx.update(|window, cx| {
            let entries = menu.read(cx).entries_for_test(window);
            let rename = entries
                .iter()
                .any(|e| e.label.as_deref() == Some("Rename fan…"));
            assert!(rename, "Rename fan… item");
            let sub = entries
                .iter()
                .find(|e| e.label.as_deref() == Some("Temperature sensor"))
                .and_then(|e| e.submenu.clone())
                .expect("the sensor submenu");
            sub.read(cx).entries_for_test(window)
        })
    }

    fn dispatch<A: gpui_kit::Action>(r: &mut Rig, action: A) {
        let page = fan_page(r);
        r.vcx.update(|window, cx| {
            let focus = page.read(cx).focus_handle(cx);
            focus.dispatch_action(&action, window, cx);
        });
        draw(r.vcx);
    }

    fn header(r: &mut Rig) -> String {
        let view = view_of(r, FAN);
        r.vcx.update(|_, cx| header_of(&view, cx).0.to_string())
    }

    fn label_of(r: &mut Rig) -> Option<String> {
        r.vcx.update(|_, cx| {
            TlSettings::settings(cx)
                .fans
                .get(FAN)
                .and_then(|f| f.label.clone())
        })
    }

    fn temp_of(r: &mut Rig) -> Option<String> {
        r.vcx.update(|_, cx| {
            TlSettings::settings(cx)
                .fans
                .get(FAN)
                .and_then(|f| f.temp.clone())
        })
    }

    /// The open Rename dialog, drawn.
    fn dialog(r: &mut Rig) -> (VisualTestContext, Entity<TlDialogView>) {
        let (any, frame) = r
            .vcx
            .update(|_, cx| {
                cx.windows().into_iter().find_map(|w| {
                    let handle = w.downcast::<Root>()?;
                    let frame = TlSecondaryWindow::<TlDialogView>::find(handle, cx)?;
                    Some((w, frame))
                })
            })
            .expect("the Rename dialog is open");
        let view = r.vcx.update(|_, cx| frame.read(cx).content().clone());
        let mut vcx = VisualTestContext::from_window(any, &r.vcx.cx);
        vcx.update(|window, _| window.activate_window());
        draw(&mut vcx);
        (vcx, view)
    }

    macro_rules! rig {
        ($cx:ident, $r:ident) => {
            let (engine, mut vcx, page, _queue) = net_settings::open_root($cx, None);
            let mut $r = Rig {
                page,
                engine,
                vcx: &mut vcx,
            };
            publish_fan_and_sensors(&mut $r);
            select(&mut $r, FAN);
        };
        // The sensors in `$dead` read their dead value before the Fan page is built.
        ($cx:ident, $r:ident, dead: $dead:expr) => {
            let (engine, mut vcx, page, _queue) = net_settings::open_root($cx, None);
            let mut $r = Rig {
                page,
                engine,
                vcx: &mut vcx,
            };
            publish_fan_and_sensors(&mut $r);
            set_readings(&mut $r, $dead);
            select(&mut $r, FAN);
        };
    }

    fn rename(r: &mut Rig, text: &str) {
        dispatch(r, PerfRenameFan { key: key(FAN) });
        let (mut dvcx, _view) = dialog(r);
        if text.is_empty() {
            dvcx.simulate_keystrokes("backspace");
        } else {
            dvcx.simulate_input(text);
        }
        draw(&mut dvcx);
        dvcx.simulate_keystrokes("enter");
        dvcx.run_until_parked();
        draw(r.vcx);
    }

    #[gpui_kit::test]
    fn fan_rename_writes_label(cx: &mut TestAppContext) {
        rig!(cx, r);
        assert_eq!(header(&mut r), "Fan 1");
        rename(&mut r, "Front intake");
        assert_eq!(label_of(&mut r).as_deref(), Some("Front intake"));
        assert_eq!(header(&mut r), "Front intake");
    }

    #[gpui_kit::test]
    fn fan_rename_empty_restores_default(cx: &mut TestAppContext) {
        rig!(cx, r);
        rename(&mut r, "Front intake");
        assert_eq!(header(&mut r), "Front intake");
        rename(&mut r, "");
        assert_eq!(label_of(&mut r), None);
        r.vcx.update(|_, cx| {
            assert!(
                !TlSettings::settings(cx).fans.contains_key(FAN),
                "the entry is removed"
            );
        });
        assert_eq!(header(&mut r), "Fan 1");
    }

    #[gpui_kit::test]
    fn fan_sensor_menu_items(cx: &mut TestAppContext) {
        rig!(cx, r);
        let page = fan_page(&mut r);
        let chosen = sensor_key(3);
        dispatch(
            &mut r,
            PerfSetFanTempSensor {
                key: key(FAN),
                sensor: Some(Arc::from(chosen.as_str())),
            },
        );
        assert_eq!(
            r.vcx
                .update(|_, cx| page.read(cx).sensor().map(str::to_owned)),
            Some(chosen)
        );
        let items = sensor_entries(&mut r);
        let radios: Vec<_> = items
            .iter()
            .filter(|e| e.kind != MenuEntryKind::Separator)
            .collect();
        assert_eq!(radios.len(), 13, "12 sensors and None");
        assert_eq!(radios[0].label.as_deref(), Some("nct6797 temp1"));
        assert_eq!(radios[0].value.as_deref(), Some("49 °C"));
        assert_eq!(radios[11].label.as_deref(), Some("nct6797 temp12"));
        assert_eq!(radios[12].label.as_deref(), Some("None"));
        let checked: Vec<_> = radios.iter().map(|e| e.checked).collect();
        let mut expected = vec![false; 13];
        expected[2] = true;
        assert_eq!(checked, expected, "only temp3 is checked");
    }

    /// Sensors that read -128 °C or exactly 0 °C with no history are not listed.
    #[gpui_kit::test]
    fn fan_sensor_menu_hides_dead_readings(cx: &mut TestAppContext) {
        rig!(cx, r, dead: &[(20, -128.0), (21, 0.0)]);
        let labels = sensor_labels(&mut r);
        assert_eq!(labels.len(), 11, "10 sensors and None: {labels:?}");
        assert_eq!(labels[0], "nct6797 temp3");
        assert!(
            !labels
                .iter()
                .any(|l| l == "nct6797 temp1" || l == "nct6797 temp2")
        );
    }

    /// A dead reading stays listed when it is the paired sensor or has a real value in its history.
    #[gpui_kit::test]
    fn fan_sensor_menu_keeps_paired_and_history_sensors(cx: &mut TestAppContext) {
        use tl_model::ids::{MetricId, SeriesId};
        use tl_model::series::{Sample, SeriesView};
        use tl_model::snapshot::SeriesSet;

        rig!(cx, r, dead: &[(20, -128.0), (21, 0.0), (22, -128.0), (23, 0.0)]);
        dispatch(
            &mut r,
            PerfSetFanTempSensor {
                key: key(FAN),
                sensor: Some(Arc::from(sensor_key(1).as_str())),
            },
        );
        let view = |metric| {
            Arc::new(SeriesView {
                id: SeriesId {
                    device: DeviceIdx(0),
                    metric,
                },
                version: 1,
                period_ns: 1_000_000_000,
                samples: Arc::from([Sample::valid(1_000_000_000, 45.0)]),
            })
        };
        r.engine.publish_with(|s| {
            // temp2 has a real sample in the fast tier, temp3 only in the 1 Hz tier.
            let mut fast = s.series.series.to_vec();
            let mut v = view(MetricId::SENSOR_TEMP);
            Arc::make_mut(&mut v).id.device = DeviceIdx(21);
            fast.push(v);
            fast.sort_by_key(|v| v.id);
            let mut slow = s.series.slow.to_vec();
            let mut v = view(MetricId::SENSOR_TEMP.with_tier_1hz());
            Arc::make_mut(&mut v).id.device = DeviceIdx(22);
            slow.push(v);
            slow.sort_by_key(|v| v.id);
            s.series = Arc::new(SeriesSet {
                header: s.series.header,
                series: Arc::from(fast),
                slow: Arc::from(slow),
            });
        });
        draw(r.vcx);
        let labels = sensor_labels(&mut r);
        assert_eq!(labels.len(), 12, "11 sensors and None: {labels:?}");
        assert_eq!(labels[0], "nct6797 temp1", "the paired sensor stays");
        assert_eq!(
            labels[1], "nct6797 temp2",
            "a sensor with fast-tier history stays"
        );
        assert_eq!(
            labels[2], "nct6797 temp3",
            "a sensor with 1 Hz history stays"
        );
        assert!(
            !labels.iter().any(|l| l == "nct6797 temp4"),
            "temp4 is dead with no history"
        );
    }

    /// A sensor that read a real value earlier in the session stays listed after it turns dead,
    /// even when no sample of that value is left in either series tier.
    #[gpui_kit::test]
    fn fan_sensor_menu_keeps_sensor_live_earlier_in_session(cx: &mut TestAppContext) {
        rig!(cx, r);
        set_readings(&mut r, &[(20, -128.0), (21, 0.0)]);
        let labels = sensor_labels(&mut r);
        assert_eq!(labels.len(), 13, "12 sensors and None: {labels:?}");
        assert_eq!(labels[0], "nct6797 temp1");
        assert_eq!(labels[1], "nct6797 temp2");
    }

    #[gpui_kit::test]
    fn fan_sensor_choice_rebinds_chart(cx: &mut TestAppContext) {
        rig!(cx, r);
        let page = fan_page(&mut r);
        let sensor = sensor_key(3);
        dispatch(
            &mut r,
            PerfSetFanTempSensor {
                key: key(FAN),
                sensor: Some(Arc::from(sensor.as_str())),
            },
        );
        assert_eq!(temp_of(&mut r).as_deref(), Some(sensor.as_str()));
        r.vcx.update(|_, cx| {
            let p = page.read(cx);
            assert_eq!(p.sensor(), Some(sensor.as_str()));
            let t = p.temperature().read(cx);
            assert_eq!(t.spec.series.len(), 1);
            assert_eq!(t.spec.series[0].id.device, DeviceIdx(22));
        });
        assert_eq!(FAN_IDX, 10);
    }

    #[gpui_kit::test]
    fn fan_sensor_none(cx: &mut TestAppContext) {
        rig!(cx, r);
        let page = fan_page(&mut r);
        let sensor = sensor_key(3);
        dispatch(
            &mut r,
            PerfSetFanTempSensor {
                key: key(FAN),
                sensor: Some(Arc::from(sensor.as_str())),
            },
        );
        dispatch(
            &mut r,
            PerfSetFanTempSensor {
                key: key(FAN),
                sensor: None,
            },
        );
        assert_eq!(temp_of(&mut r), None);
        r.vcx.update(|_, cx| {
            let p = page.read(cx);
            assert_eq!(
                p.sensor(),
                None,
                "seven fans on one chip: no default pairing"
            );
            assert_eq!(
                p.choose_sensor_text().map(|t| t.as_ref()),
                Some("Choose a sensor")
            );
        });
    }
}

/// The SMART window's smartctl fallback button (E25, PERF-051, UI-PRF-030, R-088).
mod smartctl_fallback {
    use super::*;

    use gpui_kit::Modifiers;
    use tl_model::snapshot::{SmartData, SmartKind};
    use tl_model::table::Total;
    use tl_ui::components::window_frame::{TestClientFrame, frame_bottom};
    use tl_ui::hub::GlobalHub;
    use tl_ui::i18n::tr;
    use tl_ui::pages::performance::actions::PerfShowSmart;
    use tl_ui::windows::smart::{SmartWindow, find_smart};

    const SDA: &str = "disk:sda";
    const IDX: DeviceIdx = DeviceIdx(3);

    /// The page rig plus what the SMART window needs: the global hub and its action handlers.
    fn open_smart_rig(cx: &mut TestAppContext) -> Rig<'_> {
        let engine = install(cx);
        let client: Arc<dyn EngineClient> = engine.clone();
        let hub = cx.new(|cx| SnapshotHub::new(Arc::clone(&client), cx));
        cx.update(|cx| {
            cx.set_global(GlobalHub(hub.clone()));
            tl_ui::windows::smart::init(cx);
        });
        let demand = cx.new(|cx| DemandAggregator::new(client, &hub, cx));
        let (page, vcx) = cx.add_window_view(move |_, cx| PerformancePage::new(hub, demand, cx));
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(1280.), px(800.)));
        draw(vcx);
        let mut r = Rig { page, engine, vcx };
        select(&mut r, SDA);
        r
    }

    fn empty() -> SmartData {
        SmartData {
            updated_s: 0,
            health: Total {
                value: Arc::from(""),
                validity: Validity::Unsupported,
            },
            power_on_s: Total {
                value: 0,
                validity: Validity::Unsupported,
            },
            selftest: Total {
                value: Arc::from(""),
                validity: Validity::Unsupported,
            },
            ata: Arc::from([]),
            nvme: Arc::from([]),
        }
    }

    fn publish(r: &mut Rig, smart: SmartKind, data: Option<(Validity, SmartData)>) {
        r.engine.publish_with(move |s| {
            let details = Arc::make_mut(&mut s.device_details);
            details.header.version += 1;
            details.disks = Arc::from([(
                IDX,
                DiskDetails {
                    smart,
                    smart_data: data.map(|(validity, d)| Total {
                        value: Arc::new(d),
                        validity,
                    }),
                    ..DiskDetails::default()
                },
            )]);
        });
        draw(r.vcx);
    }

    fn smart_available(r: &mut Rig) -> bool {
        let page = view_of(r, SDA).downcast::<DiskPage>().expect("a DiskPage");
        r.vcx.update(|_, cx| page.read(cx).smart_available())
    }

    fn open_window(r: &mut Rig) -> (VisualTestContext, Entity<SmartWindow>) {
        r.vcx.update(|window, cx| {
            window.dispatch_action(Box::new(PerfShowSmart { key: key(SDA) }), cx)
        });
        r.vcx.run_until_parked();
        let (handle, view) = r
            .vcx
            .update(|_, cx| find_smart(IDX, cx))
            .expect("the SMART window is open");
        let mut vcx = VisualTestContext::from_window(handle.into(), &r.vcx.cx);
        redraw(&mut vcx);
        (vcx, view)
    }

    fn redraw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn click(vcx: &mut VisualTestContext, selector: &'static str) {
        let at = vcx.debug_bounds(selector).expect(selector).center();
        vcx.simulate_click(at, Modifiers::none());
        redraw(vcx);
    }

    fn smart_commands(r: &Rig) -> Vec<DeviceCommand> {
        r.engine
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Device(
                    d @ (DeviceCommand::SmartRead { .. } | DeviceCommand::SmartReadAdmin { .. }),
                ) => Some(d),
                _ => None,
            })
            .collect()
    }

    #[gpui_kit::test]
    fn smartctl_button_shown_without_udisks_smart(cx: &mut TestAppContext) {
        let mut r = open_smart_rig(cx);
        publish(&mut r, SmartKind::NeedsFallback, None);
        assert!(smart_available(&mut r), "the page button shows");
        let (mut vcx, view) = open_window(&mut r);
        assert!(vcx.debug_bounds("SmartReadSmartctl").is_some());
        assert!(
            vcx.debug_bounds("SmartRefresh").is_none(),
            "replaces Refresh"
        );
        assert!(vcx.debug_bounds("SmartTable").is_none(), "no table yet");
        assert!(vcx.debug_bounds("SmartFailure").is_none());
        let (fallback, data) = vcx.update(|_, cx| {
            let w = view.read(cx);
            (w.fallback(), w.data().is_some())
        });
        assert!(fallback && !data);
        // UDisks2 cannot read this disk, so opening sends nothing.
        assert!(smart_commands(&r).is_empty());
        let label = vcx.update(|_, cx| tr(cx, "perf-smart-smartctl"));
        assert_eq!(label.as_ref(), "Read with smartctl (admin)");
    }

    #[gpui_kit::test]
    fn smartctl_sends_command_once(cx: &mut TestAppContext) {
        let mut r = open_smart_rig(cx);
        publish(&mut r, SmartKind::NeedsFallback, None);
        let (mut vcx, view) = open_window(&mut r);
        click(&mut vcx, "SmartReadSmartctl");
        assert!(vcx.update(|_, cx| view.read(cx).reading_admin()), "spins");
        // Disabled while the read runs.
        click(&mut vcx, "SmartReadSmartctl");
        // Security: only the kernel name goes out; the engine builds the command line.
        let sent = smart_commands(&r);
        assert_eq!(sent.len(), 1, "{sent:?}");
        match &sent[0] {
            DeviceCommand::SmartReadAdmin { disk } => {
                assert_eq!(&**disk, "sda");
                assert!(!disk.contains('/'), "no path: {disk}");
            }
            other => panic!("expected SmartReadAdmin, got {other:?}"),
        }
        // The engine's WarmingUp step is not the answer.
        publish(
            &mut r,
            SmartKind::NeedsFallback,
            Some((Validity::WarmingUp, empty())),
        );
        redraw(&mut vcx);
        assert!(vcx.update(|_, cx| view.read(cx).reading_admin()));
        let mut read = empty();
        read.updated_s = 1_000;
        read.nvme = Arc::from([(Arc::<str>::from("Percent used"), Arc::<str>::from("3%"))]);
        publish(
            &mut r,
            SmartKind::NeedsFallback,
            Some((Validity::Valid, read)),
        );
        redraw(&mut vcx);
        assert!(!vcx.update(|_, cx| view.read(cx).reading_admin()), "stops");
        assert!(vcx.debug_bounds("SmartNvmeRow-0").is_some());
        assert_eq!(smart_commands(&r).len(), 1);
    }

    #[gpui_kit::test]
    fn smartctl_auth_cancelled_text(cx: &mut TestAppContext) {
        let mut r = open_smart_rig(cx);
        publish(&mut r, SmartKind::NeedsFallback, None);
        let (mut vcx, view) = open_window(&mut r);
        click(&mut vcx, "SmartReadSmartctl");
        let mut failed = empty();
        failed.health = Total {
            value: Arc::from("Authentication cancelled"),
            validity: Validity::Failed,
        };
        publish(
            &mut r,
            SmartKind::NeedsFallback,
            Some((Validity::Failed, failed)),
        );
        redraw(&mut vcx);
        assert!(vcx.debug_bounds("SmartFailure").is_some());
        let (reason, spinning) = vcx.update(|_, cx| {
            let w = view.read(cx);
            (w.error().cloned(), w.reading_admin())
        });
        assert_eq!(reason.as_deref(), Some("Authentication cancelled"));
        assert!(!spinning);
        let title = vcx.update(|_, cx| tr(cx, "perf-smart-failed"));
        assert_eq!(title.as_ref(), "Failed to get SMART data");
        // The button stays for another try.
        assert!(vcx.debug_bounds("SmartReadSmartctl").is_some());
    }

    #[gpui_kit::test]
    fn smartctl_missing_hides_buttons(cx: &mut TestAppContext) {
        let mut r = open_smart_rig(cx);
        // Without smartctl the engine reports no SMART at all, never NeedsFallback.
        publish(&mut r, SmartKind::None, None);
        assert!(!smart_available(&mut r));
        assert!(
            r.vcx.debug_bounds("perf-page-toggle-0").is_none(),
            "no Show SMART data button"
        );
        let (mut vcx, _view) = open_window(&mut r);
        assert!(vcx.debug_bounds("SmartReadSmartctl").is_none());
    }

    /// The header pairs as drawn: box, button and the four pairs, after the paint that measures
    /// the box and the one that draws its columns.
    fn header_layout(
        vcx: &mut VisualTestContext,
        button: &'static str,
    ) -> (
        gpui_kit::Bounds<gpui_kit::Pixels>,
        gpui_kit::Bounds<gpui_kit::Pixels>,
        Vec<gpui_kit::Bounds<gpui_kit::Pixels>>,
    ) {
        redraw(vcx);
        let header = vcx.debug_bounds("SmartHeaderBox").expect("header box");
        let button = vcx.debug_bounds(button).expect(button);
        let rows = ["TlKv-row-0", "TlKv-row-1", "TlKv-row-2", "TlKv-row-3"]
            .into_iter()
            .map(|s| vcx.debug_bounds(s).expect(s))
            .collect();
        (header, button, rows)
    }

    fn assert_clear(
        header: gpui_kit::Bounds<gpui_kit::Pixels>,
        button: gpui_kit::Bounds<gpui_kit::Pixels>,
        rows: &[gpui_kit::Bounds<gpui_kit::Pixels>],
    ) {
        for (i, row) in rows.iter().enumerate() {
            assert!(
                row.right() <= header.right() && row.right() < button.left(),
                "pair {i} {row:?} runs under the button {button:?} (box {header:?})"
            );
            for (j, other) in rows.iter().enumerate().skip(i + 1) {
                assert!(!row.intersects(other), "pairs {i} and {j} overlap");
            }
        }
    }

    /// Shot s1: at the default 760 px the four header pairs ran under "Read with smartctl
    /// (admin)". The grid drops to 2 columns when 4 do not fit beside the button, also inside
    /// the client frame (UI-PRF-030).
    fn smartctl_header(cx: &mut TestAppContext, frame: bool) {
        if frame {
            cx.update(|cx| cx.set_global(TestClientFrame));
        }
        let mut r = open_smart_rig(cx);
        publish(&mut r, SmartKind::NeedsFallback, None);
        let (mut vcx, _view) = open_window(&mut r);
        // The window opens with room for a client frame; without the frame the content would get
        // that room too. Size it so the content is the default 760 x 520 either way.
        let inset = vcx.update(|window, cx| frame_bottom(window, cx));
        vcx.simulate_resize(size(px(760.) + inset * 2., px(520.) + inset * 2.));
        redraw(&mut vcx);
        let width = vcx.update(|window, _| window.viewport_size().width);
        assert_eq!(width - inset * 2., px(760.), "default content width");
        let (header, button, rows) = header_layout(&mut vcx, "SmartReadSmartctl");
        assert_clear(header, button, &rows);
        assert!(rows[2].top() > rows[0].top(), "2 columns: {rows:?}");
    }

    #[gpui_kit::test]
    fn smartctl_header_pairs_clear_the_button(cx: &mut TestAppContext) {
        smartctl_header(cx, false);
    }

    #[gpui_kit::test]
    fn smartctl_header_pairs_clear_the_button_in_the_frame(cx: &mut TestAppContext) {
        smartctl_header(cx, true);
    }

    /// With the short Refresh button the default size keeps the spec's 4 columns on one line.
    #[gpui_kit::test]
    fn refresh_header_has_four_columns(cx: &mut TestAppContext) {
        let mut r = open_smart_rig(cx);
        publish(&mut r, SmartKind::Nvme, None);
        let (mut vcx, _view) = open_window(&mut r);
        let (header, button, rows) = header_layout(&mut vcx, "SmartRefresh");
        assert_clear(header, button, &rows);
        assert!(
            rows.iter().all(|row| row.top() == rows[0].top()),
            "{rows:?}"
        );
    }
}
