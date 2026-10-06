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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use asupersync::channel::mpsc;
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time::sleep;
use asupersync::{Cx, Outcome};
use tl_collect::SysRoot;
use tl_collect::inventory::{
    self, CpuFlags, DmiInfo, Field, InventoryPaths, InventoryRaw, ModulesInfo, Virtualization,
    read_inventory,
};
use tl_collect::parse::cpufreq::read_base_mhz;
use tl_collect::parse::meminfo::{MemInfo, parse_meminfo};
use tl_collect::parse::topology::read_topology;
use tl_collect::sysroot::{read_file, read_u64};
use tl_model::health::{SourceId, SourceState};
use tl_model::snapshot::{
    BoardInfo, FirmwareInfo, FirmwareType, InvField, InventorySnapshot, MemoryInfo, MemoryModule,
    OsInfo, ProcessorInfo,
};
use tl_model::time::now_boot_ns;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{IDLE_RECHECK, WorkerCommand, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{EngineMsg, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;

/// Read limit for `/proc/meminfo`, in bytes.
const MEMINFO_MAX: usize = 64 * 1024;

pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Inventory,
        name: "inventory",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// Each generation runs [`run`] on the host paths, or on the fixture below `cfg.sys_root`. A
/// fixture root never reads host files.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    let paths = paths_for(&cfg.sys_root);
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        run(cx, io.clone(), paths.clone())
    })
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

async fn run(cx: Cx, io: WorkerIo, paths: InventoryPaths) -> Outcome<(), WorkerError> {
    let root = sys_root(&paths);
    run_with(cx, io, move || scan_files(&paths, &root)).await
}

/// The event loop with the blocking scan injected. Returns `Outcome::Ok(())` on cancel and
/// `Err` when a scan job fails or the reducer's queue is closed, so the supervisor restarts it.
#[doc(hidden)]
pub async fn run_with<F>(cx: Cx, io: WorkerIo, scan: F) -> Outcome<(), WorkerError>
where
    F: Fn() -> InventorySnapshot + Clone + Send + Sync + 'static,
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

        let job_scan = scan.clone();
        let mut job = match cx.spawn_blocking(move |_| job_scan()) {
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

/// Blocking part of one scan. Pure function of the files under `paths`, plus `uname` and the
/// page size.
pub fn scan_files(paths: &InventoryPaths, root: &SysRoot) -> InventorySnapshot {
    from_raw(&read_inventory(paths), root)
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
            valid(os.version_id.clone().unwrap_or_else(|| Arc::from("Rolling"))),
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
            assert!(self.out.lock().expect("slot lock").is_none(), "worker ended");
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
        for (machine, bits) in [("x86_64", 64), ("aarch64", 64), ("i686", 32), ("armv7l", 32)] {
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
        assert_eq!(snap.memory.peak_committed_bytes.validity, Validity::WarmingUp);
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
