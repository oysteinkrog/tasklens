//! The `Inventory` worker (ADR-004, ADR-008): fills `InventorySnapshot` for the Information tab.
//!
//! It scans when the tab opens (`Demand.inventory` goes from false to true) and on
//! `RefreshNow(Inventory)` while the tab is visible. It does no I/O otherwise. It runs its own
//! event loop instead of `source::run`, because later beads add async D-Bus reads to a scan.
//!
//! This module reads the file sections: Operating System, Processor, Memory (static rows),
//! Memory modules, BIOS/Firmware and Motherboard (PERF-095 to PERF-099, PERF-101, PERF-102).
//! Memory module serials are never read (D-007). The machine ID and the board serial are shown
//! on screen only and never logged.
//!
//! The System section (PERF-097) adds two system bus reads per scan (hostnamed, and logind when
//! `XDG_VTNR` is missing), the environment, and one run of the desktop shell's `--version` per
//! worker generation. Hostnames and the user name are shown on screen only and never logged.
//!
//! The Disk drives, Graphics and Network adapters sections (PERF-100, PERF-103, PERF-104) use the
//! device page readers: block devices, mountinfo and `statvfs`, the udev database, DRM cards and
//! connectors, `/sys/class/net` and one rtnetlink address dump. Rows carry `device: None`; the
//! reducer merges the catalog device and the API versions (R-166). Disk serials, MACs and IP
//! addresses are shown on screen only and never logged.

use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::sleep;
use asupersync::{Cx, Outcome};
use tl_collect::gpu_drm::intel::{self, IntelDriver};
use tl_collect::gpu_drm::{
    self, AmdSample, DrmCard, VENDOR_AMD, VENDOR_INTEL, VENDOR_NVIDIA, amdgpu, main_temp, read_amd,
};
use tl_collect::inventory::{
    self, CpuFlags, DmiInfo, Field, InventoryPaths, InventoryRaw, KernelInfo, ModulesInfo,
    Virtualization, read_inventory,
};
use tl_collect::net::addrs::{AddrDump, AddrFamily, AddrScope, IfAddr, format_cidr};
use tl_collect::parse::block::{self, BlockDevice, DiskKind};
use tl_collect::parse::cpufreq::read_base_mhz;
use tl_collect::parse::meminfo::{MemInfo, parse_meminfo};
use tl_collect::parse::mountinfo::{MountEntry, parse_mountinfo};
use tl_collect::parse::netif;
use tl_collect::parse::topology::read_topology;
use tl_collect::parse::udev::{UdevProps, block_entry, net_entry, pci_entry, read_udev_props};
use tl_collect::proc::meta::{PasswdUserNames, UserNames};
use tl_collect::sample::disk::{FsStat, disk_usage, host_statvfs};
use tl_collect::sanitize::sanitize_text;
use tl_collect::sysroot::{read_file, read_trimmed, read_u64};
use tl_collect::{CollectError, SysRoot};
use tl_model::devices::NetKind;
use tl_model::health::{SourceId, SourceState};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    BoardInfo, DiskDrive, DisplayMode, FirmwareInfo, FirmwareType, GraphicsAdapter, InvField,
    InventorySnapshot, MemoryInfo, MemoryModule, NetworkAdapter, OsInfo, ProcessorInfo, SystemInfo,
};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::dbus::{self, Bus, DbusConn, DbusError};
use crate::source::{IDLE_RECHECK, WorkerCommand, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{EngineMsg, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;
use crate::workers::net::net_kind;

/// Read limit for `/proc/meminfo`, in bytes.
const MEMINFO_MAX: usize = 64 * 1024;
/// Longest hostname or bus text kept, in bytes.
const TEXT_MAX: usize = 256;
/// Read limit for `/proc/self/mountinfo`, in bytes.
const MOUNTINFO_MAX: usize = 1 << 20;
/// The udev keys read for a disk row (R-140 item 1).
const DISK_UDEV_KEYS: [&str; 2] = ["ID_BUS", "ID_SERIAL_SHORT"];
/// The udev key read for a GPU name.
const GPU_UDEV_KEYS: [&str; 1] = ["ID_MODEL_FROM_DATABASE"];
/// The udev keys read for a network adapter name (R-140 item 1).
const NET_UDEV_KEYS: [&str; 2] = ["ID_VENDOR_FROM_DATABASE", "ID_MODEL_FROM_DATABASE"];
/// Longest wait for the desktop shell's `--version`.
const SHELL_TIMEOUT: Duration = Duration::from_secs(2);
/// Poll period while waiting for it.
const SHELL_POLL: Duration = Duration::from_millis(10);
/// Read limit for its output, in bytes.
const SHELL_OUTPUT_MAX: u64 = 4096;

const HOSTNAME_NAME: &str = "org.freedesktop.hostname1";
const HOSTNAME_PATH: &str = "/org/freedesktop/hostname1";
const LOGIN1_NAME: &str = "org.freedesktop.login1";
const LOGIN1_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_MANAGER: &str = "org.freedesktop.login1.Manager";
const LOGIN1_SESSION: &str = "org.freedesktop.login1.Session";

pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Inventory,
        name: "inventory",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// Each generation runs [`run`] on the host paths, or on the fixture below `cfg.sys_root`. A
/// fixture root never reads host files, the system bus, the environment or a program.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    let host = cfg.sys_root == SysRoot::host();
    let paths = paths_for(&cfg.sys_root);
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| run(cx, io.clone(), paths.clone(), host))
}

/// `InventoryPaths::system()` for the host root, else every path under the fixture base (the
/// parent of `root.proc`).
fn paths_for(root: &SysRoot) -> InventoryPaths {
    if *root == SysRoot::host() {
        return InventoryPaths::system();
    }
    let base = root.proc.parent().unwrap_or(&root.proc);
    InventoryPaths::under(base)
}

/// The `SysRoot` beside `paths`: `/proc` and `/sys` from `paths`, `/run` under `paths.root`.
fn sys_root(paths: &InventoryPaths) -> SysRoot {
    SysRoot {
        proc: paths.proc.clone(),
        sys: paths.sys.clone(),
        run: paths.root.join("run"),
    }
}

async fn run(cx: Cx, io: WorkerIo, paths: InventoryPaths, host: bool) -> Outcome<(), WorkerError> {
    let root = sys_root(&paths);
    let env = Arc::new(if host {
        SystemEnv::from_process()
    } else {
        SystemEnv::default()
    });
    // The shell version, read at most once per generation.
    let shell: Arc<OnceLock<Option<Arc<str>>>> = Arc::new(OnceLock::new());
    let bus_env = host.then(|| Arc::clone(&env));
    let settings = io.settings.clone();
    let scan = move |bus: SystemBus| {
        let raw = read_inventory(&paths);
        let mut snap = from_raw(&raw, &root);
        add_devices(&mut snap, &raw, &root, &settings.borrow_and_clone(), host);
        let version = shell.get_or_init(|| desktop_shell_version(&env)).clone();
        let user = user_name(&paths.etc.join("passwd"));
        snap.system = build_system(&env, &bus, &raw.kernel, user, version);
        snap
    };
    run_loop(cx, io, bus_env, scan).await
}

/// The event loop with the blocking scan injected. Returns `Outcome::Ok(())` on cancel and
/// `Err` when a scan job fails or the reducer's queue is closed, so the supervisor restarts it.
#[doc(hidden)]
pub async fn run_with<F>(cx: Cx, io: WorkerIo, scan: F) -> Outcome<(), WorkerError>
where
    F: Fn() -> InventorySnapshot + Clone + Send + Sync + 'static,
{
    run_loop(cx, io, None, move |_| scan()).await
}

/// The event loop. Each scan reads the system bus first when `bus_env` is `Some` (else the bus
/// fields are `Failed`), then runs `scan` with the result as a blocking job.
async fn run_loop<F>(
    cx: Cx,
    io: WorkerIo,
    bus_env: Option<Arc<SystemEnv>>,
    scan: F,
) -> Outcome<(), WorkerError>
where
    F: Fn(SystemBus) -> InventorySnapshot + Clone + Send + Sync + 'static,
{
    let running = SourceUpdate::Health {
        id: SourceId::Inventory,
        state: SourceState::Running,
        last_error: None,
        t_ns: now_boot_ns(),
    };
    if let Some(stop) = send(&cx, &io, running).await {
        return stop;
    }

    // R-276: resume after the last command a generation started. A `RefreshNow` that was
    // running when the last generation died is skipped without a message.
    let progress = Arc::clone(&io.control.borrow_and_clone().progress);
    let mut last_seq = progress.started(SourceId::Inventory);
    if last_seq > progress.done(SourceId::Inventory) {
        progress.set_done(SourceId::Inventory, last_seq);
    }
    let mut was_visible = false;

    loop {
        sleep(cx.now(), IDLE_RECHECK).await;
        if cx.checkpoint().is_err() {
            return Outcome::Ok(());
        }

        let visible = io.demand.borrow_and_clone().inventory;
        let opened = visible && !was_visible;
        was_visible = visible;

        let control = io.control.borrow_and_clone();
        // The newest `RefreshNow` seen while visible, done once this turn's scan is sent. A
        // hidden tab's `RefreshNow` and every other command are done at once.
        let mut pending_done = None;
        for queued in control.commands.iter().filter(|c| c.seq > last_seq) {
            if queued.target != SourceId::Inventory {
                continue;
            }
            progress.set_started(SourceId::Inventory, queued.seq);
            let refresh = visible && queued.command == WorkerCommand::RefreshNow;
            if refresh || pending_done.is_some() {
                pending_done = Some(queued.seq);
            } else {
                progress.set_done(SourceId::Inventory, queued.seq);
            }
        }
        if let Some(newest) = control.commands.last() {
            last_seq = last_seq.max(newest.seq);
        }
        drop(control);
        if !opened && pending_done.is_none() {
            continue;
        }

        let bus = match &bus_env {
            Some(env) => read_system_bus(&cx, env).await,
            None => SystemBus::failed(),
        };
        if cx.checkpoint().is_err() {
            return Outcome::Ok(());
        }
        let job_scan = scan.clone();
        let mut job = match cx.spawn_blocking(move |_| job_scan(bus)) {
            Ok(job) => job,
            Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
        };
        let snap = match job.join(&cx).await {
            Ok(snap) => snap,
            Err(JoinError::Cancelled(reason)) => {
                if cx.checkpoint().is_err() {
                    return Outcome::Ok(());
                }
                return Outcome::Cancelled(reason);
            }
            Err(JoinError::Panicked(payload)) => return Outcome::Panicked(payload),
            Err(JoinError::PolledAfterCompletion) => {
                return Outcome::Err(WorkerError::Other("job polled after completion".into()));
            }
        };
        if let Some(stop) = send(&cx, &io, SourceUpdate::Inventory(Box::new(snap))).await {
            return stop;
        }
        if let Some(seq) = pending_done {
            progress.set_done(SourceId::Inventory, seq);
        }
    }
}

/// Sends one update. `Some` holds the loop's result when it must stop.
async fn send(cx: &Cx, io: &WorkerIo, update: SourceUpdate) -> Option<Outcome<(), WorkerError>> {
    match io.updates.send(cx, EngineMsg::Source(update)).await {
        Ok(()) => None,
        Err(mpsc::SendError::Cancelled(_)) => Some(Outcome::Ok(())),
        Err(_) => Some(Outcome::Err(WorkerError::Closed)),
    }
}

/// Blocking part of one scan with the default settings. A function of the files under `paths`,
/// plus `uname` and the page size. On the host root it also calls `statvfs` and runs one
/// address dump.
pub fn scan_files(paths: &InventoryPaths, root: &SysRoot) -> InventorySnapshot {
    scan_with(paths, root, &EngineSettings::default())
}

/// `scan_files` with `settings` choosing the visible network adapters.
pub fn scan_with(
    paths: &InventoryPaths,
    root: &SysRoot,
    settings: &EngineSettings,
) -> InventorySnapshot {
    let raw = read_inventory(paths);
    let mut snap = from_raw(&raw, root);
    add_devices(&mut snap, &raw, root, settings, *root == SysRoot::host());
    snap
}

/// Builds the snapshot from one `read_inventory` result plus the topology, cpufreq, meminfo and
/// zram files under `root`.
#[doc(hidden)]
pub fn from_raw(raw: &InventoryRaw, root: &SysRoot) -> InventorySnapshot {
    let mut snap = InventorySnapshot::default();
    snap.header.validity = Validity::Valid;
    snap.os = os_info(raw);
    snap.processor = processor_info(raw, root);
    snap.memory = memory_info(root);
    snap.modules = modules(&raw.modules);
    snap.firmware = firmware_info(raw);
    snap.board = board_info(&raw.dmi);
    snap
}

/// Fills the Disk drives, Graphics and Network adapters sections. `host` allows `statvfs` and
/// the address dump; under a fixture root free space reads `Unsupported` and no address is listed.
#[doc(hidden)]
pub fn add_devices(
    snap: &mut InventorySnapshot,
    raw: &InventoryRaw,
    root: &SysRoot,
    settings: &EngineSettings,
    host: bool,
) {
    snap.disks = disk_drives(root, host);
    snap.graphics = graphics(raw, root);
    let mut addrs = Vec::new();
    if host {
        host_addrs(&mut addrs);
    }
    snap.network = network(root, settings, &addrs);
}

/// PERF-100: one row per disk from `enumerate_disks`. No disk list gives no rows.
fn disk_drives(root: &SysRoot, host: bool) -> Arc<[DiskDrive]> {
    let mut disks = Vec::new();
    if block::enumerate_disks(root, &mut disks).is_err() {
        return Arc::from([]);
    }
    let mut buf = Vec::new();
    let mut mounts = Vec::new();
    let mounts_read = read_file(&root.proc_path("self/mountinfo"), &mut buf, MOUNTINFO_MAX)
        .and_then(|_| parse_mountinfo(&buf, &mut mounts));
    if mounts_read.is_err() {
        mounts.clear();
    }
    // The E04-024 rule: `statvfs` only on the disks' own devices, and only on the host.
    let mut statvfs = |path: &str| -> Result<FsStat, CollectError> {
        if host {
            host_statvfs(path)
        } else {
            Err(CollectError::NotFound)
        }
    };
    let usage = disk_usage(&disks, &mounts, &[], &mut statvfs, &mut |_: &str| None);
    let mut props = UdevProps::default();
    disks
        .iter()
        .zip(usage)
        .map(|(disk, u)| {
            let path = root.run_path(&format!(
                "udev/data/{}",
                block_entry(disk.major, disk.minor)
            ));
            let read = read_udev_props(&path, &DISK_UDEV_KEYS, &mut props);
            let (interface, serial) = match read {
                Ok(()) => (
                    opt(interface_name(props.get("ID_BUS"), &disk.name).map(Arc::from)),
                    udev_text(props.get("ID_SERIAL_SHORT")),
                ),
                Err(e) => (with(e.validity()), with(e.validity())),
            };
            DiskDrive {
                device: None,
                name: Some(Arc::clone(&disk.name)),
                model: opt(disk.model.clone()),
                kind: valid(Arc::from(kind_name(disk.kind))),
                interface,
                capacity_bytes: valid(disk.size_bytes),
                free_bytes: InvField {
                    value: u.free_bytes.value,
                    validity: u.free_bytes.validity,
                },
                volumes: volumes(disk, &mounts),
                serial,
            }
        })
        .collect()
}

/// The mount points of the disk, its partitions and its dm children, in mountinfo order.
fn volumes(disk: &BlockDevice, mounts: &[MountEntry]) -> Arc<[Arc<str>]> {
    let owns = |major: u32, minor: u32| {
        major != 0
            && ((disk.major, disk.minor) == (major, minor)
                || disk
                    .children
                    .iter()
                    .any(|c| (c.major, c.minor) == (major, minor)))
    };
    mounts
        .iter()
        .filter(|m| owns(m.major, m.minor))
        .map(|m| Arc::clone(&m.mount_point))
        .collect()
}

/// The PERF-049 interface from udev `ID_BUS`, else from the disk name (the E12-013 mapping).
fn interface_name(bus: Option<&str>, name: &str) -> Option<&'static str> {
    match bus.map(str::trim) {
        Some("ata") => Some("SATA"),
        Some("usb") => Some("USB"),
        Some("scsi") => Some("SCSI"),
        Some("virtio") => Some("virtio"),
        Some(b) if b.starts_with("nvme") => Some("NVMe"),
        _ if name.starts_with("nvme") => Some("NVMe"),
        _ if name.starts_with("vd") => Some("virtio"),
        _ => None,
    }
}

/// The PERF-047 kind word.
fn kind_name(kind: DiskKind) -> &'static str {
    match kind {
        DiskKind::Nvme => "NVMe",
        DiskKind::Ssd => "SSD",
        DiskKind::Hdd => "HDD",
        DiskKind::Emmc => "eMMC",
        DiskKind::Sd => "SD",
        DiskKind::Usb => "USB",
        DiskKind::Optical => "Optical",
        DiskKind::Virtual => "Virtual",
    }
}

/// Sanitized udev text. Missing or empty is `Unsupported`.
fn udev_text(value: Option<&str>) -> InvField<Arc<str>> {
    bus_text(value.unwrap_or(""))
}

/// PERF-103: one adapter per DRM card, all vendors. No card list gives no rows.
fn graphics(raw: &InventoryRaw, root: &SysRoot) -> Arc<[GraphicsAdapter]> {
    let mut props = UdevProps::default();
    let mut buf = Vec::new();
    gpu_drm::discover_cards(&root.sys)
        .iter()
        .map(|card| {
            let path = root.run_path(&format!("udev/data/{}", pci_entry(&card.pci_slot)));
            let name = match read_udev_props(&path, &GPU_UDEV_KEYS, &mut props) {
                Ok(()) => udev_text(props.get("ID_MODEL_FROM_DATABASE")),
                Err(e) => with(e.validity()),
            };
            let version = card.driver.as_deref().and_then(|driver| {
                let file = root.sys.join(format!("module/{driver}/version"));
                let text = read_trimmed(&file, &mut buf).ok()?;
                let clean = bus_text(&String::from_utf8_lossy(text));
                (clean.validity == Validity::Valid).then_some(clean.value)
            });
            let kernel_module = match (&card.driver, &version) {
                (Some(driver), Some(v)) => valid(Arc::from(format!("{driver} {v}"))),
                (Some(driver), None) => valid(Arc::clone(driver)),
                (None, _) => with(Validity::Unsupported),
            };
            let mut amd = AmdSample::default();
            if card.vendor == VENDOR_AMD {
                read_amd(card, &root.sys, &mut amd);
            }
            let (dedicated_bytes, shared_bytes) = match card.vendor {
                VENDOR_AMD => (opt(amd.vram_total), opt(amd.gtt_total)),
                // NVIDIA dedicated memory comes from `GpuInfo` in the reducer (PERF-064).
                _ => (with(Validity::WarmingUp), with(Validity::Unsupported)),
            };
            GraphicsAdapter {
                device: None,
                pci_slot: Some(Arc::from(card.pci_slot.to_ascii_lowercase())),
                name,
                driver_version: opt(version),
                kernel_module,
                vulkan_version: with(Validity::WarmingUp),
                opengl_version: with(Validity::WarmingUp),
                temperature_source: valid(Arc::from(temp_source(card, &amd))),
                dedicated_bytes,
                shared_bytes,
                displays: displays(raw, card.card),
            }
        })
        .collect()
}

/// The PERF-069 temperature source text.
fn temp_source(card: &DrmCard, amd: &AmdSample) -> &'static str {
    let text = match card.vendor {
        VENDOR_AMD => main_temp(amd).map(|(t, _)| amdgpu::temp_source_text(t)),
        VENDOR_NVIDIA => Some("NVML (GPU core)"),
        VENDOR_INTEL => IntelDriver::from_card(card).map(intel::temp_source_text),
        _ => None,
    };
    text.unwrap_or("Not reported")
}

/// The connected, not disabled connectors of `card` with their preferred mode (PERF-072). Sysfs
/// has no refresh rate or primary flag.
fn displays(raw: &InventoryRaw, card: u32) -> Arc<[DisplayMode]> {
    raw.connectors
        .iter()
        .filter(|c| c.card == card && c.connected && c.enabled != Some(false))
        .filter_map(|c| {
            let &(width, height) = c.modes.first()?;
            Some(DisplayMode {
                width,
                height,
                refresh_mhz: 0,
                primary: false,
            })
        })
        .collect()
}

/// One rtnetlink address dump into `out`. A failed open or dump leaves it empty.
fn host_addrs(out: &mut Vec<IfAddr>) {
    let dumped = AddrDump::open().and_then(|mut dump| dump.dump(out));
    if let Err(err) = dumped {
        tracing::debug!(%err, "inventory: rtnetlink address dump failed");
        out.clear();
    }
}

/// PERF-104: one adapter per interface whose kind `settings` shows. No interface list gives no
/// rows.
fn network(root: &SysRoot, settings: &EngineSettings, addrs: &[IfAddr]) -> Arc<[NetworkAdapter]> {
    let mut ifaces = Vec::new();
    if netif::enumerate(root, &mut ifaces).is_err() {
        return Arc::from([]);
    }
    let mut buf = Vec::new();
    let mut props = UdevProps::default();
    ifaces
        .iter()
        .filter_map(|iface| {
            let (kind, label) = net_kind(iface.kind);
            if !kind_shown(kind, settings) {
                return None;
            }
            let details = netif::read_details(root, &iface.name, &mut buf);
            let path = root.run_path(&format!("udev/data/{}", net_entry(iface.ifindex)));
            let adapter_name = match read_udev_props(&path, &NET_UDEV_KEYS, &mut props) {
                Ok(()) => adapter_name(&props),
                Err(e) => with(e.validity()),
            };
            let mac = match details.mac.validity {
                Validity::Valid => opt(details.mac.value),
                validity => with(validity),
            };
            Some(NetworkAdapter {
                device: None,
                connection: valid(Arc::from(format!("{} ({label})", iface.name))),
                adapter_name,
                driver: opt(details.driver),
                link_speed_mbps: InvField {
                    value: details.speed_mbps.value,
                    validity: details.speed_mbps.validity,
                },
                ip_addresses: adapter_addrs(addrs, iface.ifindex),
                mac,
            })
        })
        .collect()
}

/// The `EngineSettings.show_network_*` flag of `kind`.
fn kind_shown(kind: NetKind, s: &EngineSettings) -> bool {
    match kind {
        NetKind::Wired => s.show_network_wired,
        NetKind::Wireless => s.show_network_wireless,
        NetKind::VpnTunnel => s.show_network_vpn,
        NetKind::Virtual => s.show_network_virtual,
        NetKind::Other => s.show_network_other,
    }
}

/// "<vendor> <model>" from the udev hwdb, either alone when the other is missing (E12-014).
/// Neither gives `Unsupported`.
fn adapter_name(props: &UdevProps) -> InvField<Arc<str>> {
    let part = |key| props.get(key).map(str::trim).filter(|v| !v.is_empty());
    match (
        part("ID_VENDOR_FROM_DATABASE"),
        part("ID_MODEL_FROM_DATABASE"),
    ) {
        (Some(vendor), Some(model)) => bus_text(&format!("{vendor} {model}")),
        (Some(one), None) | (None, Some(one)) => bus_text(one),
        (None, None) => with(Validity::Unsupported),
    }
}

/// The CIDR text of every IPv4 address of `ifindex` (scope Global or Site first), then its
/// global IPv6 addresses (scope Global or Site), each group in dump order.
fn adapter_addrs(all: &[IfAddr], ifindex: u32) -> Arc<[Arc<str>]> {
    let group = |a: &IfAddr| match (a.family, a.scope) {
        (AddrFamily::V4, AddrScope::Global | AddrScope::Site) => Some(0),
        (AddrFamily::V4, _) => Some(1),
        (AddrFamily::V6, AddrScope::Global | AddrScope::Site) => Some(2),
        (AddrFamily::V6, _) => None,
    };
    (0..3)
        .flat_map(|g| {
            all.iter()
                .filter(move |a| a.ifindex == ifindex && group(a) == Some(g))
        })
        .map(|a| Arc::from(format_cidr(a)))
        .collect()
}

fn valid<T>(value: T) -> InvField<T> {
    InvField {
        value,
        validity: Validity::Valid,
    }
}

fn with<T: Default>(validity: Validity) -> InvField<T> {
    InvField {
        value: T::default(),
        validity,
    }
}

/// `Some` is `Valid`, `None` is `Unsupported` ("Not reported").
fn opt<T: Default>(value: Option<T>) -> InvField<T> {
    value.map_or_else(|| with(Validity::Unsupported), valid)
}

/// An empty `uname` string is `Unsupported`.
fn text(value: &Arc<str>) -> InvField<Arc<str>> {
    opt((!value.is_empty()).then(|| Arc::clone(value)))
}

fn dmi_field(field: &Field) -> InvField<Arc<str>> {
    match field {
        Field::Value(s) => valid(Arc::clone(s)),
        Field::Restricted => with(Validity::Restricted),
        Field::Missing => with(Validity::Unsupported),
    }
}

/// PERF-095. Without any os-release, the name, version and build are `Failed`.
fn os_info(raw: &InventoryRaw) -> OsInfo {
    let (name, logo_icon, version, build) = match &raw.os {
        Some(os) => (
            opt(os.pretty_name.clone()),
            os.logo.clone(),
            valid(
                os.version_id
                    .clone()
                    .unwrap_or_else(|| Arc::from("Rolling")),
            ),
            opt(os.build_id.clone()),
        ),
        None => (
            with(Validity::Failed),
            None,
            with(Validity::Failed),
            with(Validity::Failed),
        ),
    };
    OsInfo {
        name,
        logo_icon,
        version,
        build,
        kernel: text(&raw.kernel.release),
        kernel_version: text(&raw.kernel.version),
        architecture: text(&raw.kernel.machine),
        install_date_s: opt(raw.install_date.map(|(s, _)| s)),
        package_manager: opt(raw.package_manager.clone()),
        machine_id: opt(raw.machine_id.clone()),
    }
}

/// PERF-096.
fn processor_info(raw: &InventoryRaw, root: &SysRoot) -> ProcessorInfo {
    let cpu = &raw.cpu;
    let (cores, logical) = match read_topology(root) {
        Ok(t) => (valid(u32::from(t.cores)), valid(u32::from(t.logical_count))),
        Err(_) => (with(Validity::Failed), with(Validity::Failed)),
    };
    let base = read_base_mhz(root);
    ProcessorInfo {
        name: opt(cpu.model_name.clone()),
        vendor: opt(cpu.vendor_id.clone()),
        architecture: text(&raw.kernel.machine),
        family: opt(cpu.family),
        model: opt(cpu.model),
        stepping: opt(cpu.stepping),
        cores,
        logical,
        base_mhz: InvField {
            value: base.value,
            validity: base.validity,
        },
        max_mhz: opt(max_mhz(&root.sys)),
        microcode: opt(cpu.microcode.clone()),
        virtualization: valid(Arc::from(virtualization_text(raw.security.virtualization))),
        avx512: valid(cpu.flags.contains(CpuFlags::AVX512F)),
        bits: opt((!raw.kernel.machine.is_empty()).then(|| machine_bits(&raw.kernel.machine))),
    }
}

/// The PERF-037 text.
fn virtualization_text(v: Virtualization) -> &'static str {
    match v {
        Virtualization::EnabledKvm => "Enabled (KVM available)",
        Virtualization::SupportedNoKvm => "Supported, KVM module not loaded",
        Virtualization::NotSupported => "Not supported",
    }
}

/// 64 for the 64-bit `uname -m` names, else 32 (R-170 item 7).
#[doc(hidden)]
pub fn machine_bits(machine: &str) -> u8 {
    match machine {
        "x86_64" | "aarch64" | "riscv64" | "ppc64" | "ppc64le" | "s390x" | "loongarch64"
        | "mips64" => 64,
        _ => 32,
    }
}

/// The largest `cpufreq/policy*/cpuinfo_max_freq`, kHz rounded to MHz. `None` when none reads.
fn max_mhz(sys: &Path) -> Option<u32> {
    let dir = sys.join("devices/system/cpu/cpufreq");
    let mut buf = Vec::new();
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| {
            let entry = e.ok()?;
            let name = entry.file_name();
            if !name.to_str()?.starts_with("policy") {
                return None;
            }
            read_u64(&entry.path().join("cpuinfo_max_freq"), &mut buf).ok()
        })
        .max()
        .map(|khz| u32::try_from(khz.saturating_add(500) / 1000).unwrap_or(u32::MAX))
}

/// PERF-098 static rows. A meminfo that does not read or parse gives `Failed` for its rows.
fn memory_info(root: &SysRoot) -> MemoryInfo {
    let mut buf = Vec::new();
    let mut mem = MemInfo::default();
    let parsed = read_file(&root.proc_path("meminfo"), &mut buf, MEMINFO_MAX).is_ok()
        && parse_meminfo(&buf, &mut mem).is_ok();
    let (total_bytes, huge_page_size, swap_total_bytes) = if parsed {
        (
            valid(mem.mem_total),
            opt(mem.hugepagesize),
            valid(mem.swap_total),
        )
    } else {
        (
            with(Validity::Failed),
            with(Validity::Failed),
            with(Validity::Failed),
        )
    };
    MemoryInfo {
        total_bytes,
        page_size: valid(rustix::param::page_size() as u64),
        huge_page_size,
        swap_total_bytes,
        zram_bytes: valid(zram_bytes(&root.sys_path("block"))),
        peak_committed_bytes: with(Validity::WarmingUp),
    }
}

/// The sum of `disksize` over `<block>/zram*`. 0 when there is none.
fn zram_bytes(block: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(block) else {
        return 0;
    };
    let mut buf = Vec::new();
    entries
        .filter_map(|e| {
            let entry = e.ok()?;
            if !entry.file_name().to_str()?.starts_with("zram") {
                return None;
            }
            let path: PathBuf = entry.path().join("disksize");
            read_u64(&path, &mut buf).ok()
        })
        .fold(0u64, u64::saturating_add)
}

/// PERF-099. No serial is read or stored (D-007).
fn modules(info: &ModulesInfo) -> Arc<[MemoryModule]> {
    let ModulesInfo::Modules { modules, .. } = info else {
        return Arc::from([]);
    };
    modules.iter().map(module).collect()
}

fn module(m: &inventory::MemoryModule) -> MemoryModule {
    let slot = match (&m.bank_locator, &m.locator) {
        (Some(bank), Some(loc)) => Some(Arc::from(format!("{bank} ({loc})"))),
        (Some(one), None) | (None, Some(one)) => Some(Arc::clone(one)),
        (None, None) => None,
    };
    MemoryModule {
        slot: opt(slot),
        capacity_bytes: valid(m.size_bytes),
        speed_mts: opt(m.speed_mts),
        kind: opt(m.mem_type.clone()),
        form_factor: opt(m.form_factor.clone()),
        manufacturer: opt(m.manufacturer.clone()),
        part_number: opt(m.part_number.clone()),
    }
}

/// The environment the System section reads (PERF-097). Empty variables count as missing.
#[derive(Clone, Debug, Default)]
pub struct SystemEnv {
    pub xdg_current_desktop: Option<String>,
    pub xdg_session_type: Option<String>,
    pub xdg_vtnr: Option<String>,
    pub xdg_session_id: Option<String>,
    /// `$PATH` split on ':', absolute entries only.
    pub path_dirs: Vec<PathBuf>,
}

impl SystemEnv {
    pub fn from_process() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let path_dirs = std::env::var_os("PATH")
            .map(|p| {
                std::env::split_paths(&p)
                    .filter(|d| d.is_absolute())
                    .collect()
            })
            .unwrap_or_default();
        SystemEnv {
            xdg_current_desktop: var("XDG_CURRENT_DESKTOP"),
            xdg_session_type: var("XDG_SESSION_TYPE"),
            xdg_vtnr: var("XDG_VTNR"),
            xdg_session_id: var("XDG_SESSION_ID"),
            path_dirs,
        }
    }
}

/// The System section's system bus values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemBus {
    /// hostnamed `PrettyHostname`. Empty is `Unsupported`.
    pub pretty: InvField<Arc<str>>,
    /// hostnamed `StaticHostname`. Empty is `Unsupported`.
    pub static_name: InvField<Arc<str>>,
    /// logind's session `VTNr`. `None` when not read or 0.
    pub vtnr: Option<u32>,
}

impl SystemBus {
    /// Both hostnames `Failed`, no VT.
    pub fn failed() -> Self {
        SystemBus {
            pretty: with(Validity::Failed),
            static_name: with(Validity::Failed),
            vtnr: None,
        }
    }
}

/// Reads the hostnames from hostnamed and, only when `XDG_VTNR` is missing and
/// `XDG_SESSION_ID` is set, the session's VT from logind. Each read waits at most
/// `READ_TIMEOUT`. A connect or hostname read failure makes both hostnames `Failed`. Never fails
/// the worker. The connection is dropped before this returns.
pub async fn read_system_bus(cx: &Cx, env: &SystemEnv) -> SystemBus {
    let Ok(conn) = dbus::connect(cx, Bus::System).await else {
        return SystemBus::failed();
    };
    let mut bus = match read_hostnames(cx, &conn).await {
        Ok((pretty, static_name)) => SystemBus {
            pretty: bus_text(&pretty),
            static_name: bus_text(&static_name),
            vtnr: None,
        },
        Err(_) => SystemBus::failed(),
    };
    if env.xdg_vtnr.is_none()
        && let Some(id) = &env.xdg_session_id
    {
        bus.vtnr = read_vtnr(cx, &conn, id).await.ok().filter(|&vt| vt != 0);
    }
    drop(conn);
    bus
}

async fn read_hostnames(cx: &Cx, conn: &DbusConn) -> Result<(String, String), DbusError> {
    let pretty = read_property::<String>(
        cx,
        conn,
        HOSTNAME_NAME,
        HOSTNAME_PATH,
        HOSTNAME_NAME,
        "PrettyHostname",
    )
    .await?;
    let static_name = read_property::<String>(
        cx,
        conn,
        HOSTNAME_NAME,
        HOSTNAME_PATH,
        HOSTNAME_NAME,
        "StaticHostname",
    )
    .await?;
    Ok((pretty, static_name))
}

/// `Manager.GetSession(id)`, then that session's `VTNr`. Both only read state.
async fn read_vtnr(cx: &Cx, conn: &DbusConn, id: &str) -> Result<u32, DbusError> {
    let session: zbus::zvariant::OwnedObjectPath = conn
        .read(cx, async {
            let proxy = uncached_proxy(conn, LOGIN1_NAME, LOGIN1_PATH, LOGIN1_MANAGER).await?;
            proxy.call("GetSession", &(id,)).await
        })
        .await?;
    read_property::<u32>(
        cx,
        conn,
        LOGIN1_NAME,
        session.as_str(),
        LOGIN1_SESSION,
        "VTNr",
    )
    .await
}

/// One `Properties.Get` through an uncached proxy, in one `read`.
async fn read_property<T>(
    cx: &Cx,
    conn: &DbusConn,
    name: &str,
    path: &str,
    interface: &str,
    property: &str,
) -> Result<T, DbusError>
where
    T: TryFrom<zbus::zvariant::OwnedValue>,
    T::Error: Into<zbus::Error>,
{
    conn.read(cx, async {
        let proxy = uncached_proxy(conn, name, path, interface).await?;
        proxy.get_property::<T>(property).await
    })
    .await
}

async fn uncached_proxy<'a>(
    conn: &'a DbusConn,
    name: &'a str,
    path: &'a str,
    interface: &'a str,
) -> zbus::Result<zbus::Proxy<'a>> {
    zbus::proxy::Builder::new(conn.raw())
        .destination(name)?
        .path(path)?
        .interface(interface)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

/// Sanitized bus or kernel text. Empty is `Unsupported`.
fn bus_text(raw: &str) -> InvField<Arc<str>> {
    let mut clean = String::new();
    sanitize_text(raw.as_bytes(), TEXT_MAX, &mut clean);
    opt((!clean.is_empty()).then(|| Arc::from(clean)))
}

/// The current user's name from `passwd`, or the uid as text when it is not there.
fn user_name(passwd: &Path) -> InvField<Arc<str>> {
    let uid = rustix::process::getuid().as_raw();
    valid(PasswdUserNames::load(passwd).name(uid))
}

/// PERF-097, from the bus values, the environment, `kernel`, `user` and the shell version.
pub fn build_system(
    env: &SystemEnv,
    bus: &SystemBus,
    kernel: &KernelInfo,
    user: InvField<Arc<str>>,
    shell_version: Option<Arc<str>>,
) -> SystemInfo {
    let static_hostname = if bus.static_name.validity == Validity::Failed {
        bus_text(&String::from_utf8_lossy(
            rustix::system::uname().nodename().to_bytes(),
        ))
    } else {
        bus.static_name.clone()
    };
    let domain = kernel.nis_domain.clone().or_else(|| {
        if static_hostname.validity != Validity::Valid {
            return None;
        }
        let (_, rest) = static_hostname.value.split_once('.')?;
        (!rest.is_empty()).then(|| Arc::from(rest))
    });
    let env_text = |v: &Option<String>| v.as_deref().map(Arc::<str>::from);
    let desktop = opt(shell_version.or_else(|| env_text(&env.xdg_current_desktop)));
    let windowing_system = opt(env.xdg_session_type.as_deref().map(|t| match t {
        "wayland" => Arc::from("Wayland"),
        "x11" => Arc::from("X11"),
        "tty" => Arc::from("None (text console)"),
        other => Arc::from(other),
    }));
    let virtual_terminal =
        opt(env_text(&env.xdg_vtnr).or_else(|| bus.vtnr.map(|vt| Arc::from(vt.to_string()))));
    SystemInfo {
        pretty_hostname: bus.pretty.clone(),
        static_hostname,
        user_name: user,
        domain,
        boot_time_s: valid(boot_time_s()),
        desktop,
        windowing_system,
        session_type: opt(env_text(&env.xdg_session_type)),
        virtual_terminal,
    }
}

/// Unix seconds at boot: `CLOCK_REALTIME` minus `CLOCK_BOOTTIME`.
fn boot_time_s() -> i64 {
    use rustix::time::{ClockId, clock_gettime};
    let ns = |id| {
        let t = clock_gettime(id);
        i128::from(t.tv_sec) * 1_000_000_000 + i128::from(t.tv_nsec)
    };
    let boot = ns(ClockId::Realtime) - ns(ClockId::Boottime);
    i64::try_from(boot.div_euclid(1_000_000_000)).unwrap_or(0)
}

/// "KDE Plasma 6.4.5" or "GNOME 47.1" from `plasmashell --version` or `gnome-shell --version`,
/// chosen by `XDG_CURRENT_DESKTOP`. Blocks for up to 2 s. `None` for other desktops, when the
/// program is not in `env.path_dirs`, or on a timeout or unknown output.
pub fn desktop_shell_version(env: &SystemEnv) -> Option<Arc<str>> {
    let desktops = env.xdg_current_desktop.as_deref()?;
    let has = |name: &str| desktops.split(':').any(|d| d == name);
    let program = if has("KDE") {
        "plasmashell"
    } else if has("GNOME") {
        "gnome-shell"
    } else {
        return None;
    };
    let exe = env
        .path_dirs
        .iter()
        .map(|d| d.join(program))
        .find(|p| p.is_file())?;
    parse_shell_version(&run_version(&exe)?)
}

/// `"plasmashell 6.4.5"` gives `"KDE Plasma 6.4.5"`, `"GNOME Shell 47.1"` gives
/// `"GNOME 47.1"`, anything else `None`.
#[doc(hidden)]
pub fn parse_shell_version(output: &[u8]) -> Option<Arc<str>> {
    let line = std::str::from_utf8(output).ok()?.lines().next()?.trim();
    let (label, rest) = if let Some(rest) = line.strip_prefix("plasmashell ") {
        ("KDE Plasma", rest)
    } else {
        ("GNOME", line.strip_prefix("GNOME Shell ")?)
    };
    let version = rest.split_whitespace().next()?;
    let ok = version
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b".-_+~".contains(&b));
    ok.then(|| Arc::from(format!("{label} {version}")))
}

/// Runs `exe --version` with stdin and stderr null, polling every 10 ms for at most 2 s, then
/// kills it. `None` on a spawn error, a timeout or a failed read.
fn run_version(exe: &Path) -> Option<Vec<u8>> {
    let mut child = spawn_version(exe)?;
    let deadline = Instant::now() + SHELL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(SHELL_POLL),
            // Timed out or failed. Do not read stdout here: a grandchild may still hold it open.
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = Vec::new();
    child
        .stdout
        .take()?
        .take(SHELL_OUTPUT_MAX)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

/// Spawns `exe --version`, retrying briefly on `ETXTBSY`.
fn spawn_version(exe: &Path) -> Option<Child> {
    for _ in 0..3 {
        let spawned = Command::new(exe)
            .arg("--version")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(child) => return Some(child),
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy => std::thread::sleep(SHELL_POLL),
            Err(_) => return None,
        }
    }
    None
}

/// PERF-101.
fn firmware_info(raw: &InventoryRaw) -> FirmwareInfo {
    let dmi = &raw.dmi;
    FirmwareInfo {
        firmware_type: valid(Some(match dmi.firmware {
            inventory::FirmwareType::Uefi => FirmwareType::Uefi,
            inventory::FirmwareType::LegacyBios => FirmwareType::LegacyBios,
        })),
        bios_vendor: dmi_field(&dmi.bios_vendor),
        bios_version: dmi_field(&dmi.bios_version),
        bios_date: dmi_field(&dmi.bios_date),
        bios_release: dmi_field(&dmi.bios_release),
        secure_boot: opt(raw.security.secure_boot),
    }
}

/// PERF-102. `board_serial` is normally `Restricted`. Never log it.
fn board_info(dmi: &DmiInfo) -> BoardInfo {
    BoardInfo {
        manufacturer: dmi_field(&dmi.board_vendor),
        product: dmi_field(&dmi.board_name),
        version: dmi_field(&dmi.board_version),
        system_vendor: dmi_field(&dmi.sys_vendor),
        system_product: dmi_field(&dmi.product_name),
        chassis_type: opt(dmi.chassis_type.map(chassis_name)),
        serial: dmi_field(&dmi.board_serial),
    }
}

/// The SMBIOS chassis type name. Codes outside the table read "Type <n>".
fn chassis_name(code: u8) -> Arc<str> {
    let name = match code {
        1 => "Other",
        2 => "Unknown",
        3 => "Desktop",
        4 => "Low Profile Desktop",
        5 => "Pizza Box",
        6 => "Mini Tower",
        7 => "Tower",
        8 => "Portable",
        9 => "Laptop",
        10 => "Notebook",
        11 => "Hand Held",
        12 => "Docking Station",
        13 => "All in One",
        14 => "Sub Notebook",
        15 => "Space-saving",
        16 => "Lunch Box",
        17 => "Main Server Chassis",
        18 => "Expansion Chassis",
        19 => "SubChassis",
        20 => "Bus Expansion Chassis",
        21 => "Peripheral Chassis",
        22 => "RAID Chassis",
        23 => "Rack Mount Chassis",
        24 => "Sealed-case PC",
        25 => "Multi-system chassis",
        26 => "Compact PCI",
        27 => "Advanced TCA",
        28 => "Blade",
        29 => "Blade Enclosure",
        30 => "Tablet",
        31 => "Convertible",
        32 => "Detachable",
        33 => "IoT Gateway",
        34 => "Embedded PC",
        35 => "Mini PC",
        36 => "Stick PC",
        n => return Arc::from(format!("Type {n}")),
    };
    Arc::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use asupersync::channel::watch;
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use tl_collect::inventory::{CpuInfo, KernelInfo, SecurityInfo};
    use tl_model::demand::Demand;
    use tl_model::settings::EngineSettings;

    use crate::source::{QueuedCommand, WorkerControl};

    type Ended = Arc<Mutex<Option<Outcome<(), WorkerError>>>>;

    struct Rig {
        lab: LabRuntime,
        rx: mpsc::Receiver<EngineMsg>,
        demand: watch::Sender<Demand>,
        control: watch::Sender<WorkerControl>,
        scans: Arc<AtomicU32>,
        out: Ended,
        handle: asupersync::runtime::TaskHandle<()>,
    }

    impl Rig {
        fn start(inventory: bool) -> Rig {
            let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(10_000_000));
            let (updates, rx) = mpsc::channel::<EngineMsg>(256);
            let (demand, demand_rx) = watch::channel(Demand {
                inventory,
                ..Demand::default()
            });
            let (_settings_tx, settings) = watch::channel(EngineSettings::default());
            let (control, control_rx) = watch::channel(WorkerControl::default());
            let io = WorkerIo::new(
                updates,
                demand_rx,
                settings,
                control_rx,
                crate::dbus::SharedHandles::default(),
            );
            let scans = Arc::new(AtomicU32::new(0));
            let counter = Arc::clone(&scans);
            let scan = move || {
                counter.fetch_add(1, Ordering::SeqCst);
                InventorySnapshot::default()
            };
            let out = Ended::default();
            let sink = Arc::clone(&out);
            let root = lab.state.create_root_region(Budget::INFINITE);
            let (task, handle) = lab
                .state
                .create_task(root, Budget::INFINITE, async move {
                    let cx = Cx::current().expect("task has a Cx");
                    let outcome = run_with(cx, io, scan).await;
                    *sink.lock().expect("slot lock") = Some(outcome);
                })
                .expect("task is created");
            lab.scheduler.lock().schedule(task, 0);
            lab.run_until_idle();
            Rig {
                lab,
                rx,
                demand,
                control,
                scans,
                out,
                handle,
            }
        }

        fn advance(&mut self, d: Duration) {
            self.lab
                .advance_time(u64::try_from(d.as_nanos()).expect("fits in u64"));
            // `run_until_idle` stops when no task is ready; one step fires the due timers.
            self.lab.step_for_test();
            self.lab.run_until_idle();
        }

        /// Advances `secs` virtual seconds in `IDLE_RECHECK` steps.
        fn run_for(&mut self, secs: u32) {
            for _ in 0..secs * 2 {
                self.advance(IDLE_RECHECK);
            }
        }

        /// The number of `Inventory` messages since the last call.
        fn inventory_msgs(&mut self) -> usize {
            let mut n = 0;
            while let Ok(msg) = self.rx.try_recv() {
                if matches!(msg, EngineMsg::Source(SourceUpdate::Inventory(_))) {
                    n += 1;
                }
            }
            n
        }

        fn set_inventory(&mut self, on: bool) {
            self.demand
                .send(Demand {
                    inventory: on,
                    ..Demand::default()
                })
                .expect("demand sends");
        }

        fn refresh(&mut self, seq: u64) {
            self.control
                .send(WorkerControl {
                    commands: Arc::from([QueuedCommand {
                        seq,
                        target: SourceId::Inventory,
                        command: WorkerCommand::RefreshNow,
                    }]),
                    ..WorkerControl::default()
                })
                .expect("control sends");
        }

        fn scans(&self) -> u32 {
            self.scans.load(Ordering::SeqCst)
        }

        /// Cancels the worker and checks that it ends with `Ok` and leaves the lab clean.
        fn finish(mut self) {
            assert!(
                self.out.lock().expect("slot lock").is_none(),
                "worker ended"
            );
            self.handle.abort();
            self.lab.run_until_idle();
            for _ in 0..5 {
                if self.out.lock().expect("slot lock").is_some() {
                    break;
                }
                self.advance(IDLE_RECHECK);
            }
            let outcome = self.out.lock().expect("slot lock").take();
            assert!(matches!(outcome, Some(Outcome::Ok(()))), "{outcome:?}");
            let report = self.lab.run_until_quiescent_with_report();
            assert!(report.lab_test_passed(), "{report:?}");
        }
    }

    #[test]
    fn no_scan_when_hidden() {
        let mut rig = Rig::start(false);
        rig.run_for(60);
        assert_eq!(rig.scans(), 0);
        assert_eq!(rig.inventory_msgs(), 0);
        rig.finish();
    }

    #[test]
    fn scan_on_open_once() {
        let mut rig = Rig::start(false);
        rig.run_for(2);
        rig.set_inventory(true);
        rig.advance(IDLE_RECHECK);
        assert_eq!(rig.scans(), 1);
        assert_eq!(rig.inventory_msgs(), 1);
        rig.run_for(60);
        assert_eq!(rig.scans(), 1);
        assert_eq!(rig.inventory_msgs(), 0);
        // Closing and opening the tab again scans once more.
        rig.set_inventory(false);
        rig.run_for(1);
        rig.set_inventory(true);
        rig.run_for(1);
        assert_eq!(rig.scans(), 2);
        assert_eq!(rig.inventory_msgs(), 1);
        rig.finish();
    }

    #[test]
    fn refresh_while_visible() {
        let mut rig = Rig::start(true);
        rig.run_for(1);
        assert_eq!(rig.scans(), 1);
        rig.inventory_msgs();
        rig.refresh(1);
        rig.advance(IDLE_RECHECK);
        assert_eq!(rig.scans(), 2);
        assert_eq!(rig.inventory_msgs(), 1);
        // The same command is not served twice.
        rig.run_for(10);
        assert_eq!(rig.scans(), 2);
        rig.finish();
    }

    #[test]
    fn refresh_ignored_when_hidden() {
        let mut rig = Rig::start(false);
        rig.run_for(1);
        rig.refresh(1);
        rig.run_for(10);
        assert_eq!(rig.scans(), 0);
        assert_eq!(rig.inventory_msgs(), 0);
        rig.finish();
    }

    fn raw(machine: &str) -> InventoryRaw {
        InventoryRaw {
            cpu: CpuInfo::default(),
            security: SecurityInfo {
                virtualization: Virtualization::NotSupported,
                hypervisor: None,
                secure_boot: None,
                lockdown: None,
                memory_encryption: None,
                user_shadow_stack: false,
                iommu: None,
                vulns: None,
                unreadable: Default::default(),
            },
            dmi: inventory::read_dmi(Path::new("/nonexistent-tasklens-sys")),
            os: None,
            kernel: KernelInfo {
                release: Arc::from("6.17.0"),
                version: Arc::from("#1 SMP"),
                machine: Arc::from(machine),
                nis_domain: None,
            },
            machine_id: None,
            install_date: None,
            package_manager: None,
            modules: ModulesInfo::NotReported,
            connectors: Vec::new(),
        }
    }

    /// R-170 item 7.
    #[test]
    fn processor_bits_from_machine() {
        let root = SysRoot::at(Path::new("/nonexistent-tasklens-root"));
        for (machine, bits) in [
            ("x86_64", 64),
            ("aarch64", 64),
            ("i686", 32),
            ("armv7l", 32),
        ] {
            let snap = from_raw(&raw(machine), &root);
            assert_eq!(snap.processor.bits, valid(bits), "{machine}");
        }
        let snap = from_raw(&raw(""), &root);
        assert_eq!(snap.processor.bits.validity, Validity::Unsupported);
    }

    #[test]
    fn missing_files_degrade_per_field() {
        let root = SysRoot::at(Path::new("/nonexistent-tasklens-root"));
        let snap = from_raw(&raw("x86_64"), &root);
        assert_eq!(snap.header.validity, Validity::Valid);
        assert_eq!(snap.os.name.validity, Validity::Failed);
        assert_eq!(snap.os.version.validity, Validity::Failed);
        assert_eq!(snap.os.build.validity, Validity::Failed);
        assert_eq!(snap.processor.name.validity, Validity::Unsupported);
        assert_eq!(snap.processor.cores.validity, Validity::Failed);
        assert_eq!(snap.processor.max_mhz.validity, Validity::Unsupported);
        assert_eq!(&*snap.processor.virtualization.value, "Not supported");
        assert_eq!(snap.memory.total_bytes.validity, Validity::Failed);
        assert_eq!(snap.memory.zram_bytes, valid(0));
        assert_eq!(
            snap.memory.peak_committed_bytes.validity,
            Validity::WarmingUp
        );
        assert!(snap.modules.is_empty());
        assert_eq!(snap.firmware.secure_boot.validity, Validity::Unsupported);
        assert_eq!(snap.board.chassis_type.validity, Validity::Unsupported);
    }

    #[test]
    fn spec_is_full_mode_only() {
        let s = spec();
        assert_eq!(s.id, SourceId::Inventory);
        assert_eq!(s.name, "inventory");
        assert_eq!(s.modes, &[EngineMode::Full]);
    }
}
