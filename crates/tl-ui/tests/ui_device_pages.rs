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
use tl_model::demand::Demand;
use tl_model::devices::{DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::ids::DeviceIdx;
use tl_model::settings::{AppearanceSettings, Settings, ThemeSettings, UiState};
use tl_model::snapshot::{
    InvField, InventorySnapshot, MemoryDetails, MemoryModule, SwapDevice, SwapKindModel,
    ZramDetail, ZswapInfo,
};
use tl_model::system::{MemoryTotals, ZramTotals};
use tl_model::validity::Validity;
use tl_ui::app::{BootSettings, EngineGlobal};
use tl_ui::components::kv_grid;
use tl_ui::demand::DemandAggregator;
use tl_ui::hub::SnapshotHub;
use tl_ui::i18n::init_strings;
use tl_ui::pages::performance::actions::PerfSetGraphRange;
use tl_ui::pages::performance::cpu::CpuPage;
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
use tl_ui::pages::performance::{DeviceKey, DeviceKind, DeviceRow, PerformancePage};
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

    select(&mut r, "memory");
    let memory = view_of(&mut r, "memory")
        .downcast::<MemoryPage>()
        .expect("a MemoryPage");
    r.vcx.update(|_, cx| {
        let page = memory.read(cx);
        assert_eq!(page.header().0.as_ref(), "Memory");
        for chart in [page.physical(), page.commit(), page.swap()] {
            assert!(!chart.read(cx).spec.series.is_empty());
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
        fan_connected: None,
    };
    let view = cx.update(|cx| build_page(&k, &row, &hub, &demand, cx));
    assert!(view.downcast::<PlaceholderPage>().is_ok());
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

    /// The Disk page of sda with an action tracker installed.
    fn setup(cx: &mut TestAppContext) -> Rig<'_> {
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
        assert_eq!(labels.len(), 15, "Public IP is hidden until E25-021");
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
    fn open_root(
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
        assert_eq!(r.vcx.update(|_, cx| net.read(cx).focus_order().len()), 2);

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
        assert_eq!(r.vcx.update(|_, cx| net.read(cx).focus_order().len()), 1);
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
