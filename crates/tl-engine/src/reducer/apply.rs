//! The one `match` over `EngineMsg` (ADR-004, ADR-005, ADR-008).
//!
//! Every `SourceUpdate` and `Command` variant has an arm. The P0 arms do the work: health, CPU,
//! memory, disk, network, sensor and GPU samples into series and devices, process sweeps, the
//! table query, baseline resets and control forwarding. The P1 to P3 arms log at `debug` and
//! return until their beads fill them.
//!
//! Privacy: never log record contents, search patterns or hardware serials from here.

use std::sync::Arc;

use asupersync::Cx;
use rustc_hash::{FxHashMap, FxHashSet};
use tl_model::actions::{ActionEvent, ActionState, RejectReason};
use tl_model::command::{Command, DeviceCommand};
use tl_model::demand::{Demand, DetailDemand};
use tl_model::details::{DetailTab, DetailTarget};
use tl_model::devices::{AggregateKind, DeviceIdentity, DeviceKind, NetKind};
use tl_model::gpu_proto::ApiVersions;
use tl_model::health::{SourceId, SourceState};
use tl_model::history::HistoryView;
use tl_model::ids::{DeviceIdx, GroupKey, MetricId, ProcessKey, SeriesId};
use tl_model::series::{Sample, SampleFlags};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, ConnectionsSnapshot, DeviceDetailsSnapshot, DeviceHistory,
    DiskDetails, DiskDrive, EjectState, GraphicsAdapter, InventorySnapshot, NetDetails,
    ProcessSnapshot, ServicesSnapshot, StartupSnapshot, SystemSnapshot, UnitRecord,
    WindowsSnapshot,
};
use tl_model::system::{GpuEngine, GpuTotals, MemGroupBand, MemoryTotals, Metric, SensorKind};
use tl_model::table::{Manager, SysTable, Total, UnitRef};
use tl_model::time::{TimeNs, now_boot_ns};
use tl_model::validity::Validity;

use super::mem_groups::{DEVICE_KEY_PREFIX, MemGroups};
use super::power::{GpuShare, PowerEstimator};
use super::process_series::ProcessSeries;
use super::suspend::Transition;
use super::sys_table::{self, SysTableInput};
use super::table::{self, PseudoInput, SweepTotals, TableInput};
use super::{AdmissionView, HelperCommand, InvariantViolation, Reducer, sys_table_index};
use crate::source::{QueuedCommand, WorkerCommand};
use crate::update::{
    ConnectionsUpdate, CpuSample, DetailsUpdate, DeviceDesc, DeviceDetailsUpdate, DeviceKinds,
    DeviceList, DiskSample, EngineMsg, GpuSample, HelperUpdate, MemorySample, NetSample,
    SensorSample, ServicesUpdate, SourceUpdate, StartupUpdate, source_of,
};
use crate::workers::history::HistoryMsg;
use crate::workers::process_detail::pss_keys;
use crate::workers::processes;

/// Valid CPU samples whose irq share stays 0 before the Interrupts row shows `n/a` (U-21).
const IRQ_ZERO_SAMPLES: u32 = 10;

/// Forwarded commands kept in the control watch (E08-004).
pub const QUEUE_MAX: usize = 64;

/// Nominal sample periods. A spacing below `FAST_LIMIT_NS` counts as the 10 Hz CPU rate,
/// anything else as 1 Hz, so timer jitter never changes a series period.
const FAST_NS: u64 = 100_000_000;
const SLOW_NS: u64 = 1_000_000_000;
const FAST_LIMIT_NS: u64 = 500_000_000;

const DISK_METRICS: [MetricId; 4] = [
    MetricId::DISK_ACTIVE,
    MetricId::DISK_READ,
    MetricId::DISK_WRITE,
    MetricId::DISK_LATENCY,
];
const NET_METRICS: [MetricId; 2] = [MetricId::NET_RX, MetricId::NET_TX];
const GPU_METRICS: [MetricId; 8] = [
    MetricId::GPU_UTIL,
    MetricId::GPU_MEM,
    MetricId::GPU_ENC,
    MetricId::GPU_DEC,
    MetricId::GPU_TEMP,
    MetricId::GPU_POWER,
    MetricId::GPU_CLOCK,
    MetricId::GPU_MEM_CLOCK,
];
/// The engines whose series are not in `GPU_METRICS` (PERF-063). Gpu, Enc and Dec are
/// `GPU_UTIL`, `GPU_ENC` and `GPU_DEC`, pushed from the device totals.
const GPU_ENGINE_SERIES: [GpuEngine; 9] = [
    GpuEngine::MemCtl,
    GpuEngine::Jpeg,
    GpuEngine::Ofa,
    GpuEngine::Compute,
    GpuEngine::Dma,
    GpuEngine::Render,
    GpuEngine::Copy,
    GpuEngine::Video,
    GpuEngine::VideoEnhance,
];

/// Reducer state that only `apply` reads and writes.
#[derive(Default)]
pub struct ApplyState {
    /// Time of the last data sample per source, for the sample spacing.
    last_t: [TimeNs; SourceId::COUNT],
    /// The last period used per source, kept when two samples carry the same time.
    period: [u64; SourceId::COUNT],
    /// The irq share of the last valid CPU sample and the zram devices of the last memory
    /// sample, for the pseudo rows of the next table build (E09-023).
    pub pseudo: PseudoInput,
    /// Valid CPU samples with an irq share of 0 since the engine started, counted until one is
    /// not 0 (U-21).
    irq_zero_samples: u32,
    /// A valid CPU sample had an irq share above 0, so the kernel accounts interrupt time.
    irq_seen: bool,
    /// `pressure_t_ns` of the last CPU sample that the CPU_LOAD1 and PSI_CPU_SOME series took;
    /// `None` before the first.
    cpu_pressure_t: Option<TimeNs>,
    /// The running maximum of `Committed_AS` since engine start (R-166).
    pub peak_committed: u64,
    /// Device details per contributing source (R-139 item 7): sysfs and D-Bus.
    disk_sysfs: FxHashMap<DeviceIdx, DiskDetails>,
    disk_dbus: FxHashMap<DeviceIdx, DiskDetails>,
    net_sysfs: FxHashMap<DeviceIdx, NetDetails>,
    net_dbus: FxHashMap<DeviceIdx, NetDetails>,
    /// `supports_process_util` per GPU UUID from the last `GpuInfo` of each GPU worker
    /// (E09-006). AMD cards share the key `[0; 16]` and are always `false`.
    gpu_caps: FxHashMap<[u8; 16], bool>,
    /// Keys whose PSS the last demand asked for: `process_detail::pss_keys` (E09-006).
    pss_keys: Arc<[ProcessKey]>,
    /// The last health state per source was `Running` or `Late`.
    running: [bool; SourceId::COUNT],
    /// The source has been `Running` before in this session (R-289).
    ran: [bool; SourceId::COUNT],
    /// Per-process series for `Demand.graph_targets` (E09-029).
    process_series: ProcessSeries,
    /// Per-process power estimate (E09-030).
    power: PowerEstimator,
    /// Measured power and UUID per GPU catalog device from the last GPU sample, valid values
    /// only (E09-030). Keyed by device, since AMD cards have no UUID (R-166).
    gpu_power: FxHashMap<DeviceIdx, ([u8; 16], f32)>,
    /// The samples the current system sample batch appended, for `RecordSink::on_system_sample`
    /// (ADR-012). Filled only while `collect` is set; the buffer keeps its capacity.
    batch: Vec<(SeriesId, Sample)>,
    collect: bool,
    /// `Demand.details` at the last tick, to find the tabs that opened since (tasklens-dwy.7).
    details_seen: Arc<[DetailDemand]>,
    /// Memory groups for the memory-by-app chart (D-036, UI-PRF-060).
    mem_groups: MemGroups,
}

impl ApplyState {
    /// Stores the measured power of a GPU for the power estimate; `None` removes it.
    fn set_gpu_power(&mut self, idx: DeviceIdx, identity: &DeviceIdentity, power_w: Option<f32>) {
        let DeviceIdentity::Gpu { uuid, .. } = identity else {
            return;
        };
        match power_w {
            Some(p) => {
                self.gpu_power.insert(idx, (*uuid, p));
            }
            None => {
                self.gpu_power.remove(&idx);
            }
        }
    }

    /// The measured power of GPU `device` from its last sample, if valid.
    pub fn gpu_power_w(&self, device: DeviceIdx) -> Option<f32> {
        self.gpu_power.get(&device).map(|&(_, p)| p)
    }

    /// `supports_process_util` of the GPU with `uuid` from the last `GpuInfo`.
    pub fn gpu_cap(&self, uuid: &[u8; 16]) -> Option<bool> {
        self.gpu_caps.get(uuid).copied()
    }

    /// The series period of a sample from `id` at `t_ns`: the spacing of the last two samples
    /// rounded to 100 ms or 1 s, or `first` for the first sample.
    fn period(&mut self, id: SourceId, t_ns: TimeNs, first: u64) -> u64 {
        let i = id.index();
        let prev = std::mem::replace(&mut self.last_t[i], t_ns);
        let period = if prev == 0 {
            first
        } else if t_ns <= prev {
            match self.period[i] {
                0 => first,
                p => p,
            }
        } else if t_ns - prev < FAST_LIMIT_NS {
            FAST_NS
        } else {
            SLOW_NS
        };
        self.period[i] = period;
        period
    }
}

/// `VALID` for a valid value, else a gap sample.
fn flags(valid: bool) -> SampleFlags {
    if valid {
        SampleFlags::VALID
    } else {
        SampleFlags::empty()
    }
}

/// A value that may be read: `Valid` or `Stale`.
fn readable(v: Validity) -> bool {
    matches!(v, Validity::Valid | Validity::Stale)
}

fn sid(device: DeviceIdx, metric: MetricId) -> SeriesId {
    SeriesId { device, metric }
}

/// Name and persistent key for a device first seen in a sample, before any `DeviceList` named
/// it. The next list from the worker replaces both. The sensor key here has no device path
/// and no `temp<n>` index (the identity holds only chip and label), so it cannot match the
/// worker's `sensor:<chip>:<path>:temp<n>` form. Nothing stores it.
fn fallback_name_key(id: &DeviceIdentity) -> (String, String) {
    match id {
        DeviceIdentity::CpuPackage => ("CPU".into(), "cpu".into()),
        DeviceIdentity::CpuLogical { id } => (format!("CPU {id}"), format!("cpu:{id}")),
        DeviceIdentity::Memory => ("Memory".into(), "memory".into()),
        DeviceIdentity::Gpu { pci_bus_id, .. } => ("GPU".into(), format!("gpu:{pci_bus_id}")),
        DeviceIdentity::Disk { name, .. } => (name.to_string(), format!("disk:{name}")),
        DeviceIdentity::Net { name, .. } => (name.to_string(), format!("net:{name}")),
        DeviceIdentity::Sensor { chip, label } => {
            (label.to_string(), format!("sensor:{chip}:{label}"))
        }
        DeviceIdentity::Fan { chip, index } => {
            (format!("Fan {index}"), format!("fan:{chip}:{index}"))
        }
        DeviceIdentity::Battery { native_path } => {
            (native_path.to_string(), format!("bat:{native_path}"))
        }
        DeviceIdentity::Aggregate(kind) => match kind {
            AggregateKind::Disks => ("Disk".into(), "aggregate:disk".into()),
            AggregateKind::Nets => ("Network".into(), "aggregate:net".into()),
            AggregateKind::Gpus => ("GPU".into(), "aggregate:gpu".into()),
        },
        DeviceIdentity::MemGroup { key } => (key.to_string(), format!("{DEVICE_KEY_PREFIX}{key}")),
    }
}

/// The series metrics a device of `kind` has, for gap marking.
fn metrics_of(kind: DeviceKind) -> &'static [MetricId] {
    match kind {
        DeviceKind::Cpu => &[
            MetricId::CPU_TOTAL,
            MetricId::CPU_KERNEL,
            MetricId::CPU_FREQ,
            MetricId::CPU_TEMP,
            MetricId::CPU_POWER_PKG,
            MetricId::CPU_POWER_CORE,
            MetricId::CPU_CLOCK_MAX,
            MetricId::CPU_LOAD1,
            MetricId::PSI_CPU_SOME,
        ],
        DeviceKind::CpuLogical => &[
            MetricId::CPU_TOTAL,
            MetricId::CPU_KERNEL,
            MetricId::CPU_FREQ,
        ],
        DeviceKind::Memory => &[
            MetricId::MEM_USED,
            MetricId::MEM_COMMIT,
            MetricId::SWAP_USED,
            MetricId::MEM_CACHED,
            MetricId::MEM_AVAILABLE,
            MetricId::PSI_MEM_SOME,
            MetricId::PSI_MEM_FULL,
            MetricId::SWAP_IN,
        ],
        DeviceKind::Gpu => &GPU_METRICS,
        DeviceKind::Disk => &DISK_METRICS,
        DeviceKind::Net => &NET_METRICS,
        DeviceKind::Sensor => &[MetricId::SENSOR_TEMP],
        DeviceKind::Fan => &[MetricId::FAN_RPM, MetricId::FAN_PWM],
        DeviceKind::Battery => &[MetricId::BATTERY_PCT, MetricId::BATTERY_POWER],
        DeviceKind::Aggregate => &[],
        DeviceKind::MemGroup => &[MetricId::MEM_GROUP],
    }
}

/// Whether `settings` shows interfaces of `kind`. An interface with no kind counts as `Other`.
fn net_shown(kind: Option<NetKind>, settings: &EngineSettings) -> bool {
    match kind.unwrap_or(NetKind::Other) {
        NetKind::Wired => settings.show_network_wired,
        NetKind::Wireless => settings.show_network_wireless,
        NetKind::VpnTunnel => settings.show_network_vpn,
        NetKind::Virtual => settings.show_network_virtual,
        NetKind::Other => settings.show_network_other,
    }
}

/// The `ServicesSnapshot.managers` slot of `manager`: 0 is System, 1 is User.
fn manager_index(manager: Manager) -> usize {
    match manager {
        Manager::System => 0,
        Manager::User => 1,
    }
}

/// A PCI address `domain:bus:device.function` as hex numbers, so the NVML form
/// "00000000:01:00.0" and the sysfs form "0000:01:00.0" compare equal, in any case. `None` for
/// text that is not such an address.
fn normalise_pci(s: &str) -> Option<(u32, u8, u8, u8)> {
    let (domain, rest) = s.trim().split_once(':')?;
    let (bus, rest) = rest.split_once(':')?;
    let (device, function) = rest.split_once('.')?;
    let hex = |part: &str| {
        let digits = !part.is_empty() && part.bytes().all(|b| b.is_ascii_hexdigit());
        digits.then(|| u32::from_str_radix(part, 16).ok())?
    };
    let byte = |part: &str| hex(part).and_then(|v| u8::try_from(v).ok());
    Some((hex(domain)?, byte(bus)?, byte(device)?, byte(function)?))
}

/// Whether two PCI bus ids name the same address. Text that does not parse matches nothing.
fn same_pci(a: &str, b: &str) -> bool {
    normalise_pci(a).is_some_and(|a| Some(a) == normalise_pci(b))
}

/// A probe value:`Valid` when the probe reported it, else `Unsupported` ("Not reported").
fn probe_field(value: Option<&str>) -> Total<Arc<str>> {
    match value {
        Some(v) => Total {
            value: Arc::from(v),
            validity: Validity::Valid,
        },
        None => Total {
            value: Arc::from(""),
            validity: Validity::Unsupported,
        },
    }
}

/// The sysfs record with the D-Bus owned fields of `dbus` on top (R-139 item 7).
fn merge_disk(sysfs: Option<&DiskDetails>, dbus: Option<&DiskDetails>) -> DiskDetails {
    let mut out = sysfs.cloned().unwrap_or_default();
    if let Some(d) = dbus {
        out.model = d.model.clone();
        out.wwn = d.wwn.clone();
        out.serial = d.serial.clone();
        out.rotation_rpm = d.rotation_rpm;
        out.smart = d.smart;
        out.smart_data = d.smart_data.clone();
        out.smart_temp_c = d.smart_temp_c;
        out.ejectable = d.ejectable;
        out.can_power_off = d.can_power_off;
        out.eject = d.eject.clone();
    }
    out
}

/// The sysfs record with the NetworkManager owned fields on top. A NetworkManager `wifi: None`
/// keeps the sysfs `wifi` (R-139 item 7).
fn merge_net(sysfs: Option<&NetDetails>, nm: Option<&NetDetails>) -> Option<NetDetails> {
    match (sysfs, nm) {
        (None, None) => None,
        (Some(s), None) => Some(s.clone()),
        (None, Some(n)) => Some(n.clone()),
        (Some(s), Some(n)) => {
            let mut out = s.clone();
            out.kind = n.kind;
            out.type_label = n.type_label.clone();
            out.connection = n.connection.clone();
            if n.wifi.is_some() {
                out.wifi = n.wifi.clone();
            }
            Some(out)
        }
    }
}

/// Sorted by `DeviceIdx`, one record per device in either map.
fn merged<T, M>(
    a: &FxHashMap<DeviceIdx, T>,
    b: &FxHashMap<DeviceIdx, T>,
    merge: M,
) -> Vec<(DeviceIdx, T)>
where
    M: Fn(Option<&T>, Option<&T>) -> Option<T>,
{
    let mut keys: Vec<DeviceIdx> = a.keys().chain(b.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter()
        .filter_map(|idx| merge(a.get(&idx), b.get(&idx)).map(|v| (idx, v)))
        .collect()
}

/// The GPU worker that owns a device with `uuid`: NVML devices carry their UUID, AMD and Intel
/// cards use `[0; 16]` (R-166).
fn uuid_owner(uuid: &[u8; 16]) -> SourceId {
    if *uuid == [0; 16] {
        SourceId::GpuDrm
    } else {
        SourceId::GpuNvidia
    }
}

/// The GPU worker that owns `identity`, or `None` for a device that is not a GPU.
fn gpu_owner(identity: &DeviceIdentity) -> Option<SourceId> {
    match identity {
        DeviceIdentity::Gpu { uuid, .. } => Some(uuid_owner(uuid)),
        _ => None,
    }
}

/// The GPU worker that sent an update about `ids`. Only the DRM worker sends an empty card set,
/// so an update without a GPU belongs to `GpuDrm`.
fn gpu_update_owner<'a>(ids: impl IntoIterator<Item = &'a DeviceIdentity>) -> SourceId {
    ids.into_iter()
        .find_map(gpu_owner)
        .unwrap_or(SourceId::GpuDrm)
}

impl Reducer {
    /// Applies one message. `Err` only on a broken invariant; the session then restarts.
    pub fn apply(
        &mut self,
        cx: &Cx,
        msg: EngineMsg,
        settings: &EngineSettings,
    ) -> Result<(), InvariantViolation> {
        match msg {
            EngineMsg::Source(up) => self.apply_source(cx, up, settings),
            EngineMsg::Command(cmd) => {
                self.apply_command(cx, cmd, settings);
                Ok(())
            }
            EngineMsg::ActionFinished { id } => {
                if let Some(source) = self.action_gate.as_mut().and_then(|g| g.finished(id)) {
                    self.forward(source, WorkerCommand::RefreshNow);
                }
                Ok(())
            }
            EngineMsg::Tick => {
                self.tick(settings);
                Ok(())
            }
        }
    }

    fn apply_source(
        &mut self,
        _cx: &Cx,
        up: SourceUpdate,
        settings: &EngineSettings,
    ) -> Result<(), InvariantViolation> {
        let source = source_of(&up);
        self.series.set_source(source);
        match up {
            SourceUpdate::Health {
                id,
                state,
                last_error,
                t_ns,
            } => {
                self.health.apply(id, state, last_error, t_ns);
                self.mark(ComponentMask::HEALTH);
                // A worker restart gaps that source's series; the first Running does not
                // (R-289). `Late` is a live worker with slow jobs, not a restart.
                let i = id.index();
                let running = state == SourceState::Running;
                if running && !self.apply_state.running[i] && self.apply_state.ran[i] {
                    self.series.mark_gap_source(id);
                }
                self.apply_state.running[i] =
                    matches!(state, SourceState::Running | SourceState::Late);
                self.apply_state.ran[i] |= running;
                match state {
                    SourceState::Failed | SourceState::Disabled { .. } => {
                        self.set_component_validity(id, Validity::Failed);
                    }
                    SourceState::Unsupported => {
                        self.set_component_validity(id, Validity::Unsupported);
                    }
                    _ => {}
                }
            }
            SourceUpdate::ResumeDetected { id, t_ns } => {
                if self.suspend.on_clock_resume(id, t_ns) == Transition::Resumed {
                    self.resume();
                }
            }
            SourceUpdate::Sleep { entering, t_ns } => {
                if let Some(sink) = self.metric_sink.as_mut() {
                    sink.on_sleep(entering, t_ns);
                }
                if self.suspend.on_sleep(entering, t_ns) == Transition::Resumed {
                    self.resume();
                }
            }
            SourceUpdate::Cpu(s) => self.record_system(s.t_ns, |r| r.apply_cpu(s))?,
            SourceUpdate::Memory(s) => self.record_system(s.t_ns, |r| r.apply_memory(s))?,
            SourceUpdate::Disks(s) => self.record_system(s.t_ns, |r| r.apply_disks(s))?,
            SourceUpdate::Nets(s) => self.record_system(s.t_ns, |r| r.apply_nets(s, settings))?,
            SourceUpdate::Sensors(s) => self.record_system(s.t_ns, |r| r.apply_sensors(s))?,
            SourceUpdate::Devices(mut list) => {
                if list.kinds.contains(DeviceKinds::GPUS) {
                    self.keep_other_gpus(&mut list);
                }
                let changes = self.devices.apply_list(&list)?;
                let t = now_boot_ns();
                for (idx, change) in changes {
                    if change != super::devices::DeviceChange::Returned {
                        continue;
                    }
                    let Some(kind) = self.devices.entry(idx).map(|e| e.identity.kind()) else {
                        return Err(InvariantViolation("device list gave an unknown index"));
                    };
                    for &metric in metrics_of(kind) {
                        self.series.append_gap(sid(idx, metric), t);
                    }
                    self.mark(ComponentMask::SERIES);
                }
                self.mark(ComponentMask::DEVICES);
            }
            SourceUpdate::Gpu(s) => {
                self.record_system(s.t_ns, |r| r.apply_gpu(s, source, settings))?;
            }
            SourceUpdate::GpuInfo(info) => {
                let owner = gpu_update_owner(info.devices.iter().map(|d| &d.identity));
                self.apply_state
                    .gpu_caps
                    .retain(|uuid, _| uuid_owner(uuid) != owner);
                for gpu in &info.devices {
                    if let DeviceIdentity::Gpu { uuid, .. } = &gpu.identity {
                        self.apply_state
                            .gpu_caps
                            .insert(*uuid, gpu.supports_process_util);
                    }
                    self.devices.ensure(
                        &gpu.identity,
                        &gpu.name,
                        None,
                        &gpu.persistent_key,
                        None,
                        gpu.hot_c,
                    )?;
                }
                self.mark(ComponentMask::DEVICES);
                self.remap_inventory();
            }
            SourceUpdate::GpuApiVersions(ApiVersions { devices }) => {
                self.api_versions = devices;
                let mut next = (*self.components.inventory).clone();
                if self.merge_api_versions(&mut next) {
                    self.components.inventory = Arc::new(next);
                    self.mark(ComponentMask::INVENTORY);
                }
            }
            SourceUpdate::Processes(sweep) => {
                let observed = (sweep.observed_start, sweep.observed_end);
                let sink = self.record_sink.as_deref_mut();
                self.processes
                    .apply_sweep(sweep, now_boot_ns(), settings.interval_ms, sink);
                // The worker's period: 10 s while only device history needs the memory group
                // series (D-036). Then the sweep feeds those series and no table work runs.
                let demand = self.demand_rx.as_ref().map(|rx| rx.borrow_and_clone());
                let history_only = demand
                    .as_ref()
                    .is_some_and(|d| processes::history_only(d, settings));
                let period = demand
                    .as_ref()
                    .and_then(|d| processes::processes_period(d, settings))
                    .map_or(u64::from(settings.interval_ms) * 1_000_000, |p| {
                        u64::try_from(p.as_nanos()).unwrap_or(u64::MAX)
                    });
                if let Some(id) = source {
                    self.sample_ok(id, observed.1, period);
                }
                self.publish_processes(observed, Some(observed.1));
                // Before the table, so its memory group filter uses this sweep's keys.
                let mem_groups =
                    self.record_system(observed.1, |r| r.apply_mem_groups(observed.1, period));
                if history_only {
                    return mem_groups;
                }
                self.rebuild_table(settings);
                mem_groups?;
                let processes = &self.components.processes;
                self.apply_state.process_series.on_sweep(
                    observed.1,
                    period,
                    &processes.records,
                    &processes.extras,
                );
                self.publish_process_series();
            }
            SourceUpdate::DeviceDetails(d) => self.apply_device_details(d)?,
            SourceUpdate::ColorScheme(v) => {
                Arc::make_mut(&mut self.components.system).color_scheme = v;
                self.mark(ComponentMask::SYSTEM);
            }
            SourceUpdate::Details(DetailsUpdate::Pss { entries, .. }) => {
                self.processes.apply_pss(&entries);
                self.republish_processes(settings);
            }
            SourceUpdate::Details(DetailsUpdate::Tab {
                target,
                tab,
                payload,
                validity,
            }) => self.details.apply_tab(target, tab, payload, validity),
            SourceUpdate::Details(DetailsUpdate::Affinity { entries, .. }) => {
                self.details.apply_affinity(entries);
            }
            SourceUpdate::Details(DetailsUpdate::HandleSearch(state)) => {
                self.details.apply_handle_search(state);
            }
            SourceUpdate::Services(s) => self.apply_services(s, settings),
            SourceUpdate::Connections(c) => self.apply_connections(c, settings),
            SourceUpdate::ConnectionsDestroySupport(supported) => {
                let mut next = (*self.components.connections).clone();
                next.destroy_supported = Some(supported);
                self.components.connections = Arc::new(next);
                self.mark(ComponentMask::CONNECTIONS);
            }
            SourceUpdate::Startup(s) => self.apply_startup(s, settings),
            SourceUpdate::Inventory(b) => self.apply_inventory(*b),
            SourceUpdate::Windows(b) => {
                let mut next: WindowsSnapshot = *b;
                next.header.version = 0;
                next.header.validity = Validity::Valid;
                self.components.windows = Arc::new(next);
                self.mark(ComponentMask::WINDOWS);
            }
            SourceUpdate::History(view) => {
                // A new `Arc` each answer; the reducer owns the version (ADR-003).
                let mut next = HistoryView::clone(&view);
                next.header.version = 0;
                next.header.validity = Validity::Valid;
                self.components.history = Arc::new(next);
                self.mark(ComponentMask::HISTORY);
            }
            SourceUpdate::DeviceHistory(history) => {
                // The worker reuses the series `Arc`s; the reducer owns the version (ADR-003).
                let mut next = DeviceHistory::clone(&history);
                next.header.version = 0;
                next.header.validity = Validity::Valid;
                self.components.device_history = Arc::new(next);
                self.mark(ComponentMask::DEVICE_HISTORY);
            }
            // E29-020: `HEALTH` only on a change, so `health` keeps its `Arc` otherwise.
            SourceUpdate::Helper(HelperUpdate { state, start }) => {
                if self.health.set_helper(state, start) {
                    self.mark(ComponentMask::HEALTH);
                }
            }
            SourceUpdate::Setup(state) => {
                if self.health.set_setup(state) {
                    self.mark(ComponentMask::HEALTH);
                }
            }
        }
        Ok(())
    }

    fn apply_command(&mut self, cx: &Cx, cmd: Command, settings: &EngineSettings) {
        match cmd {
            Command::SetTableQuery(q) => {
                self.query = q;
                self.rebuild_table(settings);
            }
            Command::SetSysTableQuery(q) => {
                let table = q.table;
                self.sys_queries[sys_table_index(table)] = Some(q);
                self.rebuild_sys(table, settings);
            }
            Command::Action(req) => {
                if let Some(gate) = self.action_gate.as_mut() {
                    let exited = self.processes.exited_within_linger();
                    let view = AdmissionView {
                        boot_id: self.boot_id,
                        live: self.processes.live_keys(),
                        exited_within_linger: &exited,
                    };
                    gate.submit(cx, req, &view);
                } else {
                    let event = ActionEvent {
                        id: req.id,
                        target: None,
                        state: ActionState::Rejected(RejectReason::Unsupported),
                    };
                    if self.events_tx.try_send(event).is_err() {
                        tracing::debug!(target: "tasklens::engine", "action event dropped: queue full or closed");
                    }
                }
            }
            Command::RefreshNow(id) => self.forward(id, WorkerCommand::RefreshNow),
            Command::ResetBaselines => {
                self.bump_reset_generation();
                self.series.mark_gap_all();
                self.apply_state.process_series.mark_gap_all();
                self.apply_state.power.reset();
            }
            Command::ReprobeSource(id) => self.forward(id, WorkerCommand::Reprobe),
            Command::KWin(c) => self.forward(SourceId::KWin, WorkerCommand::KWin(c)),
            Command::DetailRefresh { target, tab } => self.forward(
                SourceId::ProcessDetail,
                WorkerCommand::DetailRefresh { target, tab },
            ),
            Command::ComputeHashes(key) => {
                self.forward(SourceId::ProcessDetail, WorkerCommand::ComputeHashes(key));
            }
            Command::VerifyPackage(key) => {
                self.forward(SourceId::ProcessDetail, WorkerCommand::VerifyPackage(key));
            }
            Command::StringScan { scan, key, opts } => self.forward(
                SourceId::ProcessDetail,
                WorkerCommand::StringScan { scan, key, opts },
            ),
            Command::CancelStringScan(scan) => {
                self.forward(
                    SourceId::ProcessDetail,
                    WorkerCommand::CancelStringScan(scan),
                );
            }
            Command::HandleSearch {
                search,
                pattern,
                include_maps,
                privileged,
            } => self.forward(
                SourceId::ProcessDetail,
                WorkerCommand::HandleSearch {
                    search,
                    pattern,
                    include_maps,
                    privileged,
                },
            ),
            Command::CancelHandleSearch(search) => self.forward(
                SourceId::ProcessDetail,
                WorkerCommand::CancelHandleSearch(search),
            ),
            Command::HistoryQuery { id, range, what } => {
                self.send_history(HistoryMsg::Query { id, range, what });
            }
            Command::HistoryDelete { keys } => self.send_history(HistoryMsg::Delete { keys }),
            Command::SetupStatus => self.send_helper(HelperCommand::SetupStatus),
            Command::RunSetup { step } => self.send_helper(HelperCommand::RunSetup(step)),
            Command::Device(cmd) => {
                let target = match cmd {
                    DeviceCommand::SmartRead { .. }
                    | DeviceCommand::SmartReadAdmin { .. }
                    | DeviceCommand::Eject { .. } => SourceId::Udisks,
                    DeviceCommand::FetchPublicIp => SourceId::Inventory,
                    DeviceCommand::BatteryHistory { .. } => SourceId::UPower,
                };
                self.forward(target, WorkerCommand::Device(cmd));
            }
            Command::StartHelperSession => self.send_helper(HelperCommand::Start),
            Command::EndHelperSession => self.send_helper(HelperCommand::End),
        }
    }

    /// Appends a `QueuedCommand` with the next `seq` to the control watch and keeps the newest
    /// `QUEUE_MAX`. `send_modify` clones the current value, so the `progress` `Arc` stays the
    /// same (R-276).
    fn forward(&mut self, target: SourceId, command: WorkerCommand) {
        let kind = command.kind();
        let sent = self.control.send_modify(move |c| {
            let seq = c.commands.last().map_or(0, |q| q.seq) + 1;
            let keep = c.commands.len().min(QUEUE_MAX - 1);
            let mut next = Vec::with_capacity(keep + 1);
            next.extend_from_slice(&c.commands[c.commands.len() - keep..]);
            next.push(QueuedCommand {
                seq,
                target,
                command,
            });
            c.commands = next.into();
        });
        if sent.is_err() {
            tracing::debug!(target: "tasklens::engine", ?target, kind, "control watch closed: command dropped");
        }
    }

    /// `try_send` into the history worker's channel (E30-A2). Without a sink the command is
    /// dropped: history is not wired in this engine.
    fn send_history(&mut self, msg: HistoryMsg) {
        match self.history_cmd.as_mut() {
            Some(sink) => sink.command(msg),
            None => tracing::debug!(target: "tasklens::engine", "no history sink: command dropped"),
        }
    }

    fn bump_reset_generation(&mut self) {
        if self
            .control
            .send_modify(|c| c.reset_generation += 1)
            .is_err()
        {
            tracing::debug!(target: "tasklens::engine", "control watch closed: reset dropped");
        }
    }

    /// `try_send` on the helper channel. No channel or a full queue drops the command (R-148
    /// item 1, R-247).
    fn send_helper(&mut self, cmd: HelperCommand) {
        let Some(tx) = self.helper_tx.as_ref() else {
            tracing::debug!(target: "tasklens::engine", ?cmd, "no helper channel: command dropped");
            return;
        };
        if tx.try_send(cmd).is_err() {
            tracing::debug!(target: "tasklens::engine", ?cmd, "helper queue full or closed: command dropped");
        }
    }

    /// Records fresh data for `id`; marks `HEALTH` when that cleared a stale mark.
    fn sample_ok(&mut self, id: SourceId, t_ns: TimeNs, period_ns: u64) {
        if self.health.sample_ok(id, t_ns, period_ns) {
            self.mark(ComponentMask::HEALTH);
        }
    }

    /// Resume from suspend: a gap in every series, new baselines and device re-enumeration
    /// (ADR-008). The GPU probe gets no command.
    fn resume(&mut self) {
        self.series.mark_gap_all();
        self.apply_state.process_series.mark_gap_all();
        self.apply_state.power.reset();
        self.bump_reset_generation();
        for source in [SourceId::Disk, SourceId::Net, SourceId::Sensors] {
            self.forward(source, WorkerCommand::RefreshNow);
        }
    }

    fn tick(&mut self, settings: &EngineSettings) {
        let now = now_boot_ns();
        if self.health.tick(now) {
            self.mark(ComponentMask::HEALTH);
        }
        self.series.mark_stale(now);
        if !self.series.changed().is_empty() {
            self.mark(ComponentMask::SERIES);
        }
        if self.processes.expire(now, settings.interval_ms) {
            self.republish_processes(settings);
        }
        // Closed Properties windows and Set Affinity dialogs free their data (E09-026).
        if let Some(demand) = self.demand_rx.as_ref().map(|rx| rx.borrow_and_clone()) {
            self.details.retain(&demand);
            self.refresh_opened_tabs(&demand);
        }
        self.refresh_app_details();
    }

    /// Sends the ProcessDetail worker a `DetailOpened` for each process tab that opened since
    /// the last tick. A window that closed and opened again between two worker polls lost its
    /// payloads here, but the worker kept its state and would send nothing for an unchanged tab.
    fn refresh_opened_tabs(&mut self, demand: &Demand) {
        let seen = std::mem::replace(
            &mut self.apply_state.details_seen,
            Arc::clone(&demand.details),
        );
        for d in demand.details.iter() {
            if matches!(d.target, DetailTarget::Process(_)) && !seen.contains(d) {
                self.forward(
                    SourceId::ProcessDetail,
                    WorkerCommand::DetailOpened {
                        target: d.target,
                        tab: d.tab,
                    },
                );
            }
        }
    }

    /// Builds the App General tab of each `DetailTarget::App` in `Demand.details` from the
    /// process snapshot (tasklens-dwy.12).
    fn refresh_app_details(&mut self) {
        let Some(demand) = self.demand_rx.as_ref().map(|rx| rx.borrow_and_clone()) else {
            return;
        };
        let mut groups: Vec<GroupKey> = demand
            .details
            .iter()
            .filter(|d| d.tab == DetailTab::AppGeneral)
            .filter_map(|d| match d.target {
                DetailTarget::App(key) => Some(key),
                DetailTarget::Process(_) => None,
            })
            .collect();
        if groups.is_empty() {
            return;
        }
        groups.sort_unstable();
        groups.dedup();
        super::app_detail::refresh(
            &mut self.details,
            &groups,
            &self.components.processes,
            self.processes.unit_stats(),
            &self.unit_names,
        );
    }

    /// Sets the header validity of the component that `id` alone feeds (R-155 item 4). Sources
    /// that share `system` or `device_details` carry validity per value instead.
    fn set_component_validity(&mut self, id: SourceId, validity: Validity) {
        let c = &mut self.components;
        let bit = match id {
            SourceId::Processes => {
                Arc::make_mut(&mut c.processes).header.validity = validity;
                ComponentMask::PROCESSES
            }
            SourceId::ProcessDetail => {
                Arc::make_mut(&mut c.details).header.validity = validity;
                ComponentMask::DETAILS
            }
            SourceId::Connections => {
                Arc::make_mut(&mut c.connections).header.validity = validity;
                ComponentMask::CONNECTIONS
            }
            SourceId::SystemdSystem | SourceId::SystemdUser => {
                Arc::make_mut(&mut c.services).header.validity = validity;
                ComponentMask::SERVICES
            }
            SourceId::Startup => {
                Arc::make_mut(&mut c.startup).header.validity = validity;
                ComponentMask::STARTUP
            }
            SourceId::Inventory => {
                Arc::make_mut(&mut c.inventory).header.validity = validity;
                ComponentMask::INVENTORY
            }
            SourceId::KWin => {
                Arc::make_mut(&mut c.windows).header.validity = validity;
                ComponentMask::WINDOWS
            }
            SourceId::HistoryRecorder => {
                Arc::make_mut(&mut c.history).header.validity = validity;
                ComponentMask::HISTORY
            }
            SourceId::MetricHistory => {
                Arc::make_mut(&mut c.device_history).header.validity = validity;
                ComponentMask::DEVICE_HISTORY
            }
            _ => return,
        };
        self.mark(bit);
    }

    /// The index of a sampled device and whether it is present. An identity not in the
    /// catalog is added; a new GPU or disk entry maps the inventory rows again.
    fn sample_device(
        &mut self,
        id: &DeviceIdentity,
    ) -> Result<(DeviceIdx, bool), InvariantViolation> {
        if let Some(idx) = self.devices.idx(id) {
            let Some(entry) = self.devices.entry(idx) else {
                return Err(InvariantViolation("catalog index without an entry"));
            };
            return Ok((idx, entry.present));
        }
        let (name, key) = fallback_name_key(id);
        let (idx, _) = self.devices.ensure(id, &name, None, &key, None, None)?;
        self.mark(ComponentMask::DEVICES);
        if matches!(id, DeviceIdentity::Gpu { .. } | DeviceIdentity::Disk { .. }) {
            self.remap_inventory();
        }
        Ok((idx, true))
    }

    fn fixed_device(&self, id: &DeviceIdentity) -> Result<DeviceIdx, InvariantViolation> {
        self.devices
            .idx(id)
            .ok_or(InvariantViolation("fixed device missing from the catalog"))
    }

    /// Applies one system sample batch, then hands the samples it appended to the record sink and
    /// the metric sink in one `on_system_sample` call each. Without a sink nothing is collected.
    fn record_system(
        &mut self,
        t_ns: TimeNs,
        apply: impl FnOnce(&mut Self) -> Result<(), InvariantViolation>,
    ) -> Result<(), InvariantViolation> {
        self.apply_state.batch.clear();
        self.apply_state.collect = self.record_sink.is_some() || self.metric_sink.is_some();
        let result = apply(self);
        self.apply_state.collect = false;
        result?;
        let batch = &self.apply_state.batch;
        if batch.is_empty() {
            return Ok(());
        }
        if let Some(sink) = self.record_sink.as_deref_mut() {
            sink.on_system_sample(t_ns, batch);
        }
        if let Some(sink) = self.metric_sink.as_mut() {
            sink.on_system_sample(t_ns, batch);
        }
        Ok(())
    }

    fn collect(&mut self, id: SeriesId, sample: Sample) {
        if self.apply_state.collect {
            self.apply_state.batch.push((id, sample));
        }
    }

    fn push(
        &mut self,
        device: DeviceIdx,
        metric: MetricId,
        t: TimeNs,
        value: f32,
        valid: bool,
        period: u64,
    ) {
        self.push_flagged(
            device,
            metric,
            t,
            value,
            valid,
            period,
            SampleFlags::empty(),
        );
    }

    /// `push` that also keeps the `RESET` and `GAP_BEFORE` of a counter rate (ADR-008).
    #[allow(clippy::too_many_arguments, reason = "push plus the rate flags")]
    fn push_flagged(
        &mut self,
        device: DeviceIdx,
        metric: MetricId,
        t: TimeNs,
        value: f32,
        valid: bool,
        period: u64,
        rate_flags: SampleFlags,
    ) {
        // Samples between `PrepareForSleep(true)` and the resume are gaps (ADR-008).
        let valid = valid && !self.suspend.drop_rates();
        let id = sid(device, metric);
        let kept = rate_flags & (SampleFlags::RESET | SampleFlags::GAP_BEFORE);
        let sample = self
            .series
            .append(id, t, value, period, flags(valid) | kept);
        self.collect(id, sample);
    }

    fn gaps(&mut self, device: DeviceIdx, metrics: &[MetricId], t: TimeNs) {
        for &metric in metrics {
            let id = sid(device, metric);
            if let Some(sample) = self.series.append_gap(id, t) {
                self.collect(id, sample);
            }
        }
    }

    /// The system component for writing; observed at `t`.
    fn system_mut(&mut self, t: TimeNs) -> &mut SystemSnapshot {
        self.mark(ComponentMask::SYSTEM);
        let system = Arc::make_mut(&mut self.components.system);
        system.header = ComponentHeader {
            version: system.header.version,
            observed_start: t,
            observed_end: t,
            validity: Validity::Valid,
        };
        system
    }

    fn apply_cpu(&mut self, s: CpuSample) -> Result<(), InvariantViolation> {
        let t = s.t_ns;
        let period = self.apply_state.period(SourceId::Cpu, t, FAST_NS);
        if let Some(topology) = &s.topology {
            if self.devices.set_cpus(&topology.online)? {
                self.mark(ComponentMask::DEVICES);
            }
            self.system_mut(t).cpu_topology = Arc::clone(topology);
        }
        let ok = s.validity == Validity::Valid;
        let pkg = self.fixed_device(&DeviceIdentity::CpuPackage)?;
        let totals = s.totals;
        self.push(
            pkg,
            MetricId::CPU_TOTAL,
            t,
            totals.util_pct.value,
            ok && totals.util_pct.validity.is_valid(),
            period,
        );
        self.push(
            pkg,
            MetricId::CPU_KERNEL,
            t,
            totals.kernel_pct.value,
            ok && totals.kernel_pct.validity.is_valid(),
            period,
        );
        let freq = s.freq_mhz.map_or(0.0, |f| f as f32);
        self.push(
            pkg,
            MetricId::CPU_FREQ,
            t,
            freq,
            ok && s.freq_mhz.is_some(),
            period,
        );
        let temp_ok = ok && s.temp_validity.is_valid() && s.temp_c.is_some();
        self.push(
            pkg,
            MetricId::CPU_TEMP,
            t,
            s.temp_c.unwrap_or(0.0),
            temp_ok,
            period,
        );
        // The secondary chart's series (UI-PRF-025). A power reading that is not Valid, such
        // as Restricted or Unsupported RAPL, is a gap.
        for (metric, m) in [
            (MetricId::CPU_POWER_PKG, totals.package_power_w),
            (MetricId::CPU_POWER_CORE, totals.core_power_w),
        ] {
            self.push(pkg, metric, t, m.value, ok && m.validity.is_valid(), period);
        }
        let fastest = totals.freq_fastest_mhz;
        self.push(
            pkg,
            MetricId::CPU_CLOCK_MAX,
            t,
            fastest.value as f32,
            ok && fastest.validity.is_valid(),
            period,
        );
        // System pressure (ADR-016), at 1 Hz also while the CPU runs at 10 Hz: only a sample
        // with a new read time carries a fresh read, fast ticks repeat the last one. These do
        // not come from /proc/stat, so only their own validity counts.
        if self.apply_state.cpu_pressure_t != Some(s.pressure_t_ns) {
            self.apply_state.cpu_pressure_t = Some(s.pressure_t_ns);
            let load = totals.load_avg;
            self.push(
                pkg,
                MetricId::CPU_LOAD1,
                t,
                load.value[0],
                load.validity.is_valid(),
                SLOW_NS,
            );
            let psi = totals.psi_some_pct;
            self.push(
                pkg,
                MetricId::PSI_CPU_SOME,
                t,
                psi.value,
                psi.validity.is_valid(),
                SLOW_NS,
            );
        }
        for core in &s.per_cpu {
            let (idx, present) =
                self.sample_device(&DeviceIdentity::CpuLogical { id: core.cpu })?;
            if !present {
                self.gaps(idx, metrics_of(DeviceKind::CpuLogical), t);
                continue;
            }
            let core_ok = ok && core.validity.is_valid();
            self.push(idx, MetricId::CPU_TOTAL, t, core.pct, core_ok, period);
            self.push(
                idx,
                MetricId::CPU_KERNEL,
                t,
                core.kernel_pct,
                core_ok,
                period,
            );
            let freq = core.freq_mhz.map_or(0.0, |f| f as f32);
            self.push(
                idx,
                MetricId::CPU_FREQ,
                t,
                freq,
                core_ok && core.freq_mhz.is_some(),
                period,
            );
        }
        if ok {
            let st = &mut self.apply_state;
            st.irq_seen |= s.irq_pct > 0.0;
            if !st.irq_seen {
                st.irq_zero_samples = st.irq_zero_samples.saturating_add(1);
            }
            // U-21: an irq share that stays 0 means the kernel counts interrupt time in tasks.
            st.pseudo.irq_pct =
                (st.irq_seen || st.irq_zero_samples < IRQ_ZERO_SAMPLES).then_some(s.irq_pct);
        }
        self.system_mut(t).cpu = totals;
        self.sample_ok(SourceId::Cpu, t, period);
        self.mark(ComponentMask::SERIES);
        Ok(())
    }

    fn apply_memory(&mut self, s: MemorySample) -> Result<(), InvariantViolation> {
        let t = s.t_ns;
        let period = self.apply_state.period(SourceId::Memory, t, SLOW_NS);
        let (idx, _) = self.sample_device(&DeviceIdentity::Memory)?;
        let m = s.totals;
        let ok = s.validity.is_valid() && m.validity.is_valid();
        let swap_used = m.swap_total.saturating_sub(m.swap_free);
        self.push(idx, MetricId::MEM_USED, t, m.used as f32, ok, period);
        self.push(idx, MetricId::MEM_COMMIT, t, m.committed as f32, ok, period);
        self.push(idx, MetricId::SWAP_USED, t, swap_used as f32, ok, period);
        self.push(idx, MetricId::MEM_CACHED, t, m.cached as f32, ok, period);
        self.push(
            idx,
            MetricId::MEM_AVAILABLE,
            t,
            m.available as f32,
            ok,
            period,
        );
        // System pressure (ADR-016). These do not come from /proc/meminfo, so only their own
        // validity counts. The swap-in rate keeps its counter flags.
        for (metric, v, rate_flags) in [
            (MetricId::PSI_MEM_SOME, m.psi_some_pct, SampleFlags::empty()),
            (MetricId::PSI_MEM_FULL, m.psi_full_pct, SampleFlags::empty()),
            (MetricId::SWAP_IN, m.swap_in_pps, s.swap_in_flags),
        ] {
            self.push_flagged(
                idx,
                metric,
                t,
                v.value,
                v.validity.is_valid(),
                period,
                rate_flags,
            );
        }
        if readable(m.validity) && m.committed > self.apply_state.peak_committed {
            self.apply_state.peak_committed = m.committed;
            Arc::make_mut(&mut self.components.inventory)
                .memory
                .peak_committed_bytes = Total {
                value: m.committed,
                validity: Validity::Valid,
            };
            self.mark(ComponentMask::INVENTORY);
        }
        self.apply_state.pseudo.zram = s.zram;
        let system = self.system_mut(t);
        // The memory groups come from the process sweep, not from this sample.
        let groups = std::mem::take(&mut system.memory.groups);
        system.memory = MemoryTotals { groups, ..m };
        self.sample_ok(SourceId::Memory, t, period);
        self.mark(ComponentMask::SERIES);
        Ok(())
    }

    /// Sums RssAnon per memory group over the published processes and pushes the top 20
    /// groups, "Other apps" and "Shared and kernel" as `MEM_GROUP` series (D-036, UI-PRF-060).
    /// A group that leaves the top 20 gets one gap sample. A sweep without records still
    /// publishes the two fixed bands, with all of In use in "Shared and kernel".
    fn apply_mem_groups(&mut self, t: TimeNs, period: u64) -> Result<(), InvariantViolation> {
        let processes = Arc::clone(&self.components.processes);
        let mem = &self.components.system.memory;
        let in_use = readable(mem.validity).then_some(mem.used);
        let bands =
            self.apply_state
                .mem_groups
                .bands(&processes.records, &processes.extras, in_use);

        let mut published = Vec::with_capacity(bands.top.len() + 2);
        let mut shown = Vec::with_capacity(bands.top.len());
        for g in &bands.top {
            let idx = self.mem_group_device(&g.group.key, &g.group.name)?;
            self.push(idx, MetricId::MEM_GROUP, t, g.bytes as f32, true, period);
            shown.push(idx);
            published.push(MemGroupBand {
                key: Arc::clone(&g.group.key),
                name: Arc::clone(&g.group.name),
                process_count: g.processes,
                device: idx,
            });
        }
        let groups = &self.apply_state.mem_groups;
        let fixed = [
            (
                Arc::clone(&groups.other_key),
                Arc::clone(&groups.other_name),
                Some(bands.other_bytes),
                bands.other_processes,
            ),
            (
                Arc::clone(&groups.shared_key),
                Arc::clone(&groups.shared_name),
                bands.shared_bytes,
                0,
            ),
        ];
        for (key, name, bytes, process_count) in fixed {
            let idx = self.mem_group_device(&key, &name)?;
            let value = bytes.unwrap_or(0) as f32;
            self.push(idx, MetricId::MEM_GROUP, t, value, bytes.is_some(), period);
            published.push(MemGroupBand {
                key,
                name,
                process_count,
                device: idx,
            });
        }

        let left: Vec<DeviceIdx> = std::mem::replace(&mut self.apply_state.mem_groups.shown, shown)
            .into_iter()
            .filter(|idx| !self.apply_state.mem_groups.shown.contains(idx))
            .collect();
        for idx in left {
            self.gaps(idx, &[MetricId::MEM_GROUP], t);
        }
        if *self.components.system.memory.groups != *published {
            Arc::make_mut(&mut self.components.system).memory.groups = published.into();
            self.mark(ComponentMask::SYSTEM);
        }
        self.mark(ComponentMask::SERIES);
        Ok(())
    }

    /// The device of a memory group band. A new key, or a new display name for a known key,
    /// updates the catalog.
    fn mem_group_device(
        &mut self,
        key: &Arc<str>,
        name: &Arc<str>,
    ) -> Result<DeviceIdx, InvariantViolation> {
        if let Some((idx, known)) = self.apply_state.mem_groups.devices.get(key)
            && known == name
        {
            return Ok(*idx);
        }
        let identity = DeviceIdentity::MemGroup {
            key: Arc::clone(key),
        };
        let persistent_key = format!("{DEVICE_KEY_PREFIX}{key}");
        let (idx, _) = self
            .devices
            .ensure(&identity, name, None, &persistent_key, None, None)?;
        self.apply_state
            .mem_groups
            .devices
            .insert(Arc::clone(key), (idx, Arc::clone(name)));
        self.mark(ComponentMask::DEVICES);
        Ok(idx)
    }

    fn apply_disks(&mut self, s: DiskSample) -> Result<(), InvariantViolation> {
        let t = s.t_ns;
        let period = self.apply_state.period(SourceId::Disk, t, SLOW_NS);
        let mut rows = Vec::with_capacity(s.disks.len());
        let (mut active, mut read, mut write) = (0.0f32, 0.0f64, 0.0f64);
        let (mut active_ok, mut read_ok, mut write_ok) = (false, false, false);
        for (identity, mut d) in s.disks {
            let (idx, present) = self.sample_device(&identity)?;
            d.device = idx;
            if !present {
                self.gaps(idx, &DISK_METRICS, t);
                let missing = Validity::Missing;
                d.active_pct = Metric::with(missing);
                d.read_bps = Metric::with(missing);
                d.write_bps = Metric::with(missing);
                d.response_ms = Metric::with(missing);
                rows.push((idx, d));
                continue;
            }
            self.push(
                idx,
                MetricId::DISK_ACTIVE,
                t,
                d.active_pct.value,
                d.active_pct.validity.is_valid(),
                period,
            );
            self.push(
                idx,
                MetricId::DISK_READ,
                t,
                d.read_bps.value as f32,
                d.read_bps.validity.is_valid(),
                period,
            );
            self.push(
                idx,
                MetricId::DISK_WRITE,
                t,
                d.write_bps.value as f32,
                d.write_bps.validity.is_valid(),
                period,
            );
            self.push(
                idx,
                MetricId::DISK_LATENCY,
                t,
                d.response_ms.value,
                d.response_ms.validity.is_valid(),
                period,
            );
            let physical = self.devices.entry(idx).is_some_and(|e| e.parent.is_none());
            if physical {
                if d.active_pct.validity.is_valid() {
                    active = active.max(d.active_pct.value);
                    active_ok = true;
                }
                if d.read_bps.validity.is_valid() {
                    read += d.read_bps.value;
                    read_ok = true;
                }
                if d.write_bps.validity.is_valid() {
                    write += d.write_bps.value;
                    write_ok = true;
                }
            }
            rows.push((idx, d));
        }
        let agg = self.fixed_device(&DeviceIdentity::Aggregate(AggregateKind::Disks))?;
        self.push(agg, MetricId::DISK_ACTIVE, t, active, active_ok, period);
        self.push(agg, MetricId::DISK_READ, t, read as f32, read_ok, period);
        self.push(agg, MetricId::DISK_WRITE, t, write as f32, write_ok, period);
        rows.sort_unstable_by_key(|(idx, _)| *idx);
        self.system_mut(t).disks = rows.into();
        self.sample_ok(SourceId::Disk, t, period);
        self.mark(ComponentMask::SERIES);
        Ok(())
    }

    fn apply_nets(
        &mut self,
        s: NetSample,
        settings: &EngineSettings,
    ) -> Result<(), InvariantViolation> {
        let t = s.t_ns;
        let period = self.apply_state.period(SourceId::Net, t, SLOW_NS);
        let mut rows = Vec::with_capacity(s.nets.len());
        let (mut rx, mut tx) = (0.0f64, 0.0f64);
        let (mut rx_ok, mut tx_ok) = (false, false);
        for (identity, mut n) in s.nets {
            let (idx, present) = self.sample_device(&identity)?;
            n.device = idx;
            if !present {
                self.gaps(idx, &NET_METRICS, t);
                n.rx_bps = Metric::with(Validity::Missing);
                n.tx_bps = Metric::with(Validity::Missing);
                rows.push((idx, n));
                continue;
            }
            self.push(
                idx,
                MetricId::NET_RX,
                t,
                n.rx_bps.value as f32,
                n.rx_bps.validity.is_valid(),
                period,
            );
            self.push(
                idx,
                MetricId::NET_TX,
                t,
                n.tx_bps.value as f32,
                n.tx_bps.validity.is_valid(),
                period,
            );
            let kind = self.devices.entry(idx).and_then(|e| e.net_kind);
            if net_shown(kind, settings) {
                if n.rx_bps.validity.is_valid() {
                    rx += n.rx_bps.value;
                    rx_ok = true;
                }
                if n.tx_bps.validity.is_valid() {
                    tx += n.tx_bps.value;
                    tx_ok = true;
                }
            }
            rows.push((idx, n));
        }
        let agg = self.fixed_device(&DeviceIdentity::Aggregate(AggregateKind::Nets))?;
        self.push(agg, MetricId::NET_RX, t, rx as f32, rx_ok, period);
        self.push(agg, MetricId::NET_TX, t, tx as f32, tx_ok, period);
        rows.sort_unstable_by_key(|(idx, _)| *idx);
        self.system_mut(t).nets = rows.into();
        self.sample_ok(SourceId::Net, t, period);
        self.mark(ComponentMask::SERIES);
        Ok(())
    }

    fn apply_sensors(&mut self, s: SensorSample) -> Result<(), InvariantViolation> {
        let t = s.t_ns;
        let period = self.apply_state.period(SourceId::Sensors, t, SLOW_NS);
        let mut rows = Vec::with_capacity(s.readings.len());
        for (identity, mut r) in s.readings {
            let (idx, present) = self.sample_device(&identity)?;
            r.device = idx;
            let metric = match r.kind {
                SensorKind::Temperature => MetricId::SENSOR_TEMP,
                SensorKind::Fan => MetricId::FAN_RPM,
            };
            let fan = r.kind == SensorKind::Fan;
            if present {
                self.push(
                    idx,
                    metric,
                    t,
                    r.value.value,
                    r.value.validity.is_valid(),
                    period,
                );
                if fan {
                    self.push(
                        idx,
                        MetricId::FAN_PWM,
                        t,
                        r.pwm_pct.value,
                        r.pwm_pct.validity.is_valid(),
                        period,
                    );
                }
            } else {
                if fan {
                    self.gaps(idx, &[metric, MetricId::FAN_PWM], t);
                } else {
                    self.gaps(idx, &[metric], t);
                }
                r.value = Metric::with(Validity::Missing);
            }
            rows.push((idx, r));
        }
        rows.sort_unstable_by_key(|(idx, _)| *idx);
        self.system_mut(t).sensors = rows.into();
        self.sample_ok(SourceId::Sensors, t, period);
        self.mark(ComponentMask::SERIES);
        Ok(())
    }

    fn apply_gpu(
        &mut self,
        s: GpuSample,
        source: Option<SourceId>,
        settings: &EngineSettings,
    ) -> Result<(), InvariantViolation> {
        let t = s.t_ns;
        let period = match source {
            Some(id) => self.apply_state.period(id, t, SLOW_NS),
            None => SLOW_NS,
        };
        let owner = source.unwrap_or_else(|| gpu_update_owner(s.devices.iter().map(|(id, _)| id)));
        let mut rows = self.other_gpu_rows(owner);
        let (mut util, mut mem) = (0.0f32, 0u64);
        let (mut util_ok, mut mem_ok) = (false, false);
        for (idx, g) in &rows {
            if !self.devices.entry(*idx).is_some_and(|e| e.present) {
                continue;
            }
            if g.util_pct.validity.is_valid() {
                util = util.max(g.util_pct.value);
                util_ok = true;
            }
            if g.mem_used.validity.is_valid() {
                mem += g.mem_used.value;
                mem_ok = true;
            }
        }
        rows.reserve(s.devices.len());
        for (identity, mut g) in s.devices {
            let (idx, present) = self.sample_device(&identity)?;
            g.device = idx;
            if !present {
                self.gaps(idx, &GPU_METRICS, t);
                self.gaps(idx, &GPU_ENGINE_SERIES.map(GpuEngine::metric), t);
                self.apply_state.set_gpu_power(idx, &identity, None);
                rows.push((idx, g));
                continue;
            }
            let values = [
                (MetricId::GPU_UTIL, g.util_pct.value, g.util_pct.validity),
                (
                    MetricId::GPU_MEM,
                    g.mem_used.value as f32,
                    g.mem_used.validity,
                ),
                (MetricId::GPU_ENC, g.enc_pct.value, g.enc_pct.validity),
                (MetricId::GPU_DEC, g.dec_pct.value, g.dec_pct.validity),
                (MetricId::GPU_TEMP, g.temp_c.value, g.temp_c.validity),
                (MetricId::GPU_POWER, g.power_w.value, g.power_w.validity),
                (
                    MetricId::GPU_CLOCK,
                    g.clock_gfx_mhz.value as f32,
                    g.clock_gfx_mhz.validity,
                ),
                (
                    MetricId::GPU_MEM_CLOCK,
                    g.clock_mem_mhz.value as f32,
                    g.clock_mem_mhz.validity,
                ),
            ];
            for (metric, value, validity) in values {
                self.push(idx, metric, t, value, validity.is_valid(), period);
            }
            // One series per reported engine; an engine the device does not report has none.
            for engine in GPU_ENGINE_SERIES {
                let m = g.engines.get(engine);
                if g.engines.is_reported(engine) {
                    self.push(
                        idx,
                        engine.metric(),
                        t,
                        m.value,
                        m.validity.is_valid(),
                        period,
                    );
                }
            }
            let power = g.power_w.validity.is_valid().then_some(g.power_w.value);
            self.apply_state.set_gpu_power(idx, &identity, power);
            if g.util_pct.validity.is_valid() {
                util = util.max(g.util_pct.value);
                util_ok = true;
            }
            if g.mem_used.validity.is_valid() {
                mem += g.mem_used.value;
                mem_ok = true;
            }
            rows.push((idx, g));
        }
        let agg = self.fixed_device(&DeviceIdentity::Aggregate(AggregateKind::Gpus))?;
        self.push(agg, MetricId::GPU_UTIL, t, util, util_ok, period);
        self.push(agg, MetricId::GPU_MEM, t, mem as f32, mem_ok, period);
        rows.sort_unstable_by_key(|(idx, _)| *idx);
        self.system_mut(t).gpus = rows.into();
        if let Some(id) = source {
            self.sample_ok(id, t, period);
        }
        self.mark(ComponentMask::SERIES);
        if s.processes.is_some() || s.drm_processes.is_some() {
            let columns = table::gpu_columns(&self.components.system, &self.components.devices);
            self.processes
                .set_gpu_slots(columns.into_iter().map(|(id, _)| id));
            let caps = &self.apply_state.gpu_caps;
            if let Some(processes) = &s.processes {
                self.processes.apply_gpu_processes(processes, caps);
            }
            if let Some(rows) = &s.drm_processes {
                self.processes.apply_drm_vram(rows, caps);
            }
            self.republish_processes(settings);
        }
        Ok(())
    }

    /// The current GPU rows whose device belongs to another worker than `owner`, so a sample
    /// from one vendor keeps the other vendor's rows.
    fn other_gpu_rows(&self, owner: SourceId) -> Vec<(DeviceIdx, GpuTotals)> {
        self.components
            .system
            .gpus
            .iter()
            .filter(|(idx, _)| {
                self.devices
                    .entry(*idx)
                    .and_then(|e| gpu_owner(&e.identity))
                    .is_some_and(|o| o != owner)
            })
            .cloned()
            .collect()
    }

    /// Adds the present GPUs of the other GPU worker to a GPU device list, so the list only
    /// adds and removes the GPUs of the worker that sent it.
    fn keep_other_gpus(&self, list: &mut DeviceList) {
        let owner = gpu_update_owner(list.entries.iter().map(|d| &d.identity));
        let others: Vec<DeviceDesc> = (0..=u16::MAX)
            .map_while(|i| self.devices.entry(DeviceIdx(i)))
            .filter(|e| e.present && gpu_owner(&e.identity).is_some_and(|o| o != owner))
            .filter(|e| !list.entries.iter().any(|d| d.identity == e.identity))
            .map(|e| DeviceDesc {
                identity: e.identity.clone(),
                display_name: Arc::clone(&e.display_name),
                parent: e
                    .parent
                    .and_then(|p| self.devices.entry(p))
                    .map(|p| p.identity.clone()),
                persistent_key: Arc::clone(&e.persistent_key),
                net_kind: e.net_kind,
                hot_c: e.hot_c,
            })
            .collect();
        list.entries.extend(others);
    }

    fn apply_device_details(&mut self, d: DeviceDetailsUpdate) -> Result<(), InvariantViolation> {
        // An unchanged battery state publishes nothing.
        if let DeviceDetailsUpdate::OnBattery(on) = d
            && self.components.device_details.on_battery == on
        {
            return Ok(());
        }
        let mut next: DeviceDetailsSnapshot = (*self.components.device_details).clone();
        match d {
            DeviceDetailsUpdate::Cpu(cpu) => next.cpu = cpu,
            DeviceDetailsUpdate::Memory(memory) => next.memory = memory,
            DeviceDetailsUpdate::Disks { from, entries } => {
                let mut map = FxHashMap::default();
                for (identity, details) in entries {
                    let (idx, _) = self.sample_device(&identity)?;
                    map.insert(idx, details);
                }
                match from {
                    SourceId::Disk => self.apply_state.disk_sysfs = map,
                    SourceId::Udisks => self.apply_state.disk_dbus = map,
                    _ => {
                        tracing::debug!(target: "tasklens::engine", ?from, "disk details from an unknown source: dropped");
                        return Ok(());
                    }
                }
                let mut disks = merged(
                    &self.apply_state.disk_sysfs,
                    &self.apply_state.disk_dbus,
                    |a, b| Some(merge_disk(a, b)),
                );
                for (_, disk) in &mut disks {
                    self.fill_blocker_apps(disk);
                }
                next.disks = disks.into();
            }
            DeviceDetailsUpdate::Nets { from, entries } => {
                let mut map = FxHashMap::default();
                for (identity, details) in entries {
                    let (idx, _) = self.sample_device(&identity)?;
                    map.insert(idx, details);
                }
                match from {
                    SourceId::Net => self.apply_state.net_sysfs = map,
                    SourceId::NetworkManager => self.apply_state.net_dbus = map,
                    _ => {
                        tracing::debug!(target: "tasklens::engine", ?from, "net details from an unknown source: dropped");
                        return Ok(());
                    }
                }
                next.nets = merged(
                    &self.apply_state.net_sysfs,
                    &self.apply_state.net_dbus,
                    merge_net,
                )
                .into();
            }
            DeviceDetailsUpdate::Batteries(list) => {
                let t = now_boot_ns();
                let period = self.apply_state.period(SourceId::UPower, t, SLOW_NS);
                let mut rows = Vec::with_capacity(list.len());
                for (identity, battery) in list {
                    let (idx, _) = self.sample_device(&identity)?;
                    let pct = battery.percentage;
                    self.push(
                        idx,
                        MetricId::BATTERY_PCT,
                        t,
                        pct.value,
                        pct.validity.is_valid(),
                        period,
                    );
                    if battery.power_supply {
                        let rate = battery.energy_rate_w;
                        let power = match &*battery.state {
                            "Charging" => rate.value,
                            "Discharging" => -rate.value,
                            _ => 0.0,
                        };
                        self.push(
                            idx,
                            MetricId::BATTERY_POWER,
                            t,
                            power,
                            rate.validity.is_valid(),
                            period,
                        );
                    }
                    rows.push((idx, battery));
                }
                rows.sort_unstable_by_key(|(idx, _)| *idx);
                next.batteries = rows.into();
                self.mark(ComponentMask::SERIES);
            }
            DeviceDetailsUpdate::OnBattery(on) => next.on_battery = on,
        }
        self.components.device_details = Arc::new(next);
        self.mark(ComponentMask::DEVICE_DETAILS);
        Ok(())
    }

    /// Fills `Blocker.app` of an eject from the process grouping (E12-022).
    fn fill_blocker_apps(&self, disk: &mut DiskDetails) {
        let Some((_, EjectState::Blocked { blockers, .. })) = &mut disk.eject else {
            return;
        };
        let processes = &self.components.processes;
        let filled: Vec<_> = blockers
            .iter()
            .map(|b| {
                let mut b = b.clone();
                if b.app.is_none() {
                    b.app = processes.get(b.key).and_then(|r| r.meta.app_id.clone());
                }
                b
            })
            .collect();
        *blockers = filled.into();
    }

    /// Publishes the process cache into `components.processes` when it changed. `sweep_t` is
    /// the time of a new sweep, which gets a new power estimate; a republish without one writes
    /// the estimates of the last sweep again (E09-030).
    fn publish_processes(&mut self, observed: (TimeNs, TimeNs), sweep_t: Option<TimeNs>) {
        let coverage = self.processes.coverage();
        if let Some(mut next) = self.processes.publish(coverage, observed) {
            self.estimate_power(&mut next, sweep_t);
            self.components.processes = next;
            self.mark(ComponentMask::PROCESSES);
        }
    }

    /// Writes `power_w` and `power_trend_w` of every row of `next` (E09-030, PROC-034). Only a
    /// GPU with measured power and per-process utilisation adds a GPU term.
    fn estimate_power(&mut self, next: &mut Arc<ProcessSnapshot>, sweep_t: Option<TimeNs>) {
        let snap = Arc::make_mut(next);
        let mut p2 = snap.extras_p2.to_vec();
        match sweep_t {
            Some(t) => {
                let empty = FxHashMap::default();
                let by_dev = self.processes.gpu_by_dev();
                let caps = &self.apply_state.gpu_caps;
                let gpus: Vec<GpuShare> = self
                    .apply_state
                    .gpu_power
                    .values()
                    .filter(|(uuid, _)| caps.get(uuid).copied().unwrap_or(false))
                    .map(|(uuid, p)| GpuShare {
                        power_w: Some(*p),
                        util_by_key: by_dev.get(uuid).unwrap_or(&empty),
                    })
                    .collect();
                self.apply_state.power.on_sweep(
                    t,
                    &snap.records,
                    &snap.extras,
                    &self.components.system.cpu,
                    &gpus,
                    &mut p2,
                );
            }
            None => self.apply_state.power.fill(&snap.records, &mut p2),
        }
        snap.extras_p2 = p2.into();
    }

    /// Sets `process_series` and marks `PROCESS_SERIES` when the series changed (E09-029). The
    /// header carries the observed times and validity of the last sweep.
    fn publish_process_series(&mut self) {
        let header = ComponentHeader {
            version: 0,
            ..self.components.processes.header
        };
        if let Some(next) = self.apply_state.process_series.publish(header) {
            self.components.process_series = next;
            self.mark(ComponentMask::PROCESS_SERIES);
        }
    }

    /// Applies the demand of this batch: PSS of keys no longer demanded becomes `WarmingUp`
    /// and leaves coverage. The demanded keys are the worker's `pss_keys`.
    pub fn apply_demand(&mut self, demand: &Demand, settings: &EngineSettings) {
        let keys = pss_keys(demand);
        self.retain_unit_details(&demand.unit_details);
        self.apply_state
            .process_series
            .set_targets(&demand.graph_targets);
        self.publish_process_series();
        if *keys == *self.apply_state.pss_keys {
            return;
        }
        self.processes.clear_pss_outside(&keys);
        self.apply_state.pss_keys = keys;
        self.republish_processes(settings);
    }

    /// Publishes a change between sweeps with the last sweep's observed times, then rebuilds
    /// the table.
    fn republish_processes(&mut self, settings: &EngineSettings) {
        let header = self.components.processes.header;
        self.publish_processes((header.observed_start, header.observed_end), None);
        self.rebuild_table(settings);
    }

    /// Stores the non-empty `Description` and `user` of each unit for the unit row name,
    /// Description and User (E09-025). An empty value keeps the stored one. Returns true when a
    /// value of a unit that a process is in changed, so the table needs a rebuild.
    fn apply_unit_names(&mut self, units: &[UnitRecord]) -> bool {
        let mut changed: FxHashSet<&UnitRef> = FxHashSet::default();
        for u in units {
            for (map, value) in [
                (&mut self.unit_names, &u.description),
                (&mut self.unit_users, &u.user),
            ] {
                if value.is_empty() || map.get(&u.unit).is_some_and(|v| v == value) {
                    continue;
                }
                map.insert(u.unit.clone(), Arc::clone(value));
                changed.insert(&u.unit);
            }
        }
        !changed.is_empty()
            && self
                .components
                .processes
                .extras
                .iter()
                .any(|x| x.unit.as_ref().is_some_and(|u| changed.contains(u)))
    }

    /// Rebuilds the process table and its header totals from the current state. Always marks
    /// `TABLE`, so `query_generation` is echoed even when the rows are equal.
    fn rebuild_table(&mut self, settings: &EngineSettings) {
        let processes = Arc::clone(&self.components.processes);
        let system = Arc::clone(&self.components.system);
        let devices = Arc::clone(&self.components.devices);
        let input = TableInput {
            records: &processes.records,
            extras: &processes.extras,
            app_stats: &processes.apps,
            unit_stats: self.processes.unit_stats(),
            system: &system,
            devices: &devices,
            settings,
            unit_names: &self.unit_names,
            unit_users: &self.unit_users,
            pseudo: Some(&self.apply_state.pseudo),
            mem_groups: &self.apply_state.mem_groups.keys,
        };
        let mut view = table::build(&input, &self.query);
        view.totals = table::totals(
            &system,
            &devices,
            &SweepTotals {
                system_threads: self.processes.system_threads(),
                file_nr: self.processes.file_nr(),
            },
        );
        view.header = ComponentHeader {
            version: 0,
            ..processes.header
        };
        self.components.table = Arc::new(view);
        self.mark(ComponentMask::TABLE);
        self.refresh_app_details();
    }

    /// Merges one manager's update into `services`: the other manager's units and details stay,
    /// this manager's are replaced (E09-027). Then rebuilds the Services rows and counts.
    fn apply_services(&mut self, u: ServicesUpdate, settings: &EngineSettings) {
        if self.apply_unit_names(&u.units) {
            self.rebuild_table(settings);
        }
        let old = &self.components.services;
        let mut units: Vec<UnitRecord> = old
            .units
            .iter()
            .filter(|r| r.unit.manager != u.manager)
            .cloned()
            .collect();
        units.extend(u.units);
        // Stable: System units first, each manager in the worker's order.
        units.sort_by_key(|r| manager_index(r.unit.manager));
        let mut details: std::collections::HashMap<_, _> = old
            .details
            .iter()
            .filter(|(unit, _)| unit.manager != u.manager)
            .map(|(unit, d)| (unit.clone(), Arc::clone(d)))
            .collect();
        details.extend(u.details);
        let mut managers = old.managers;
        managers[manager_index(u.manager)] = u.state;
        let t = now_boot_ns();
        self.components.services = Arc::new(ServicesSnapshot {
            header: ComponentHeader {
                version: 0,
                observed_start: t,
                observed_end: t,
                validity: u.validity,
            },
            rows: Arc::clone(&old.rows),
            query_generation: old.query_generation,
            units: units.into(),
            counts: old.counts,
            managers,
            manager_counts: old.manager_counts,
            details: Arc::new(details),
            backend: u.backend,
        });
        if let Some(wanted) = self
            .demand_rx
            .as_ref()
            .map(|rx| rx.borrow().unit_details.clone())
        {
            self.retain_unit_details(&wanted);
        }
        self.rebuild_sys(SysTable::Services, settings);
    }

    /// Drops the unit details not in `wanted` (`Demand.unit_details`), so a closed Service
    /// Properties window frees its payload. The other entries keep their `Arc`.
    fn retain_unit_details(&mut self, wanted: &[UnitRef]) {
        let details = &self.components.services.details;
        if details.keys().all(|unit| wanted.contains(unit)) {
            return;
        }
        let kept = details
            .iter()
            .filter(|(unit, _)| wanted.contains(unit))
            .map(|(unit, d)| (unit.clone(), Arc::clone(d)))
            .collect();
        Arc::make_mut(&mut self.components.services).details = Arc::new(kept);
        self.mark(ComponentMask::SERVICES);
    }

    /// Replaces `connections` with the update's records, then rebuilds the rows and the counts
    /// of the filtered table. `destroy_supported` stays (E09-027). The attribution goes to the process records.
    fn apply_connections(&mut self, mut u: ConnectionsUpdate, settings: &EngineSettings) {
        // Attributed TCP rates fill the process Network columns (PROC-032).
        self.processes
            .apply_net_rates(std::mem::take(&mut u.attribution));
        self.republish_processes(settings);
        let old = &self.components.connections;
        self.components.connections = Arc::new(ConnectionsSnapshot {
            header: ComponentHeader {
                version: 0,
                observed_start: u.t_ns,
                observed_end: u.t_ns,
                validity: u.validity,
            },
            rows: Arc::clone(&old.rows),
            query_generation: old.query_generation,
            counts: old.counts,
            records: u.records.into(),
            destroy_supported: old.destroy_supported,
        });
        self.rebuild_sys(SysTable::Connections, settings);
    }

    fn apply_startup(&mut self, u: StartupUpdate, settings: &EngineSettings) {
        let old = &self.components.startup;
        let t = now_boot_ns();
        self.components.startup = Arc::new(StartupSnapshot {
            header: ComponentHeader {
                version: 0,
                observed_start: t,
                observed_end: t,
                validity: u.validity,
            },
            rows: Arc::clone(&old.rows),
            query_generation: old.query_generation,
            counts: old.counts,
            entries: u.entries.into(),
        });
        self.rebuild_sys(SysTable::Startup, settings);
    }

    /// Replaces `inventory` with the worker's box: new version and observed time, the box's
    /// validity. The reducer-owned peak commit and the API versions are merged in.
    fn apply_inventory(&mut self, mut next: InventorySnapshot) {
        let t = now_boot_ns();
        next.header = ComponentHeader {
            version: 0,
            observed_start: t,
            observed_end: t,
            validity: next.header.validity,
        };
        if self.apply_state.peak_committed > 0 {
            next.memory.peak_committed_bytes = Total {
                value: self.apply_state.peak_committed,
                validity: Validity::Valid,
            };
        }
        self.map_inventory_devices(&mut next);
        self.merge_api_versions(&mut next);
        self.components.inventory = Arc::new(next);
        self.mark(ComponentMask::INVENTORY);
    }

    /// Sets `device` of each graphics adapter to the GPU catalog entry whose PCI bus id names
    /// the same address as its `pci_slot`, and of each disk drive to the disk entry with its
    /// `name` (R-166). A row with no match keeps its `device`. Returns true when a row changed.
    fn map_inventory_devices(&self, inventory: &mut InventorySnapshot) -> bool {
        let find = |matches: &dyn Fn(&DeviceIdentity) -> bool| {
            (0..=u16::MAX)
                .map_while(|i| self.devices.entry(DeviceIdx(i)))
                .find(|e| matches(&e.identity))
                .map(|e| e.idx)
        };
        let mut changed = false;
        if inventory.graphics.iter().any(|a| a.pci_slot.is_some()) {
            let mut graphics: Vec<GraphicsAdapter> = inventory.graphics.to_vec();
            for adapter in &mut graphics {
                let Some(slot) = adapter.pci_slot.as_deref() else {
                    continue;
                };
                let found = find(&|id| {
                    matches!(id, DeviceIdentity::Gpu { pci_bus_id, .. }
                        if same_pci(pci_bus_id, slot))
                });
                if found.is_some() && found != adapter.device {
                    adapter.device = found;
                    changed = true;
                }
            }
            inventory.graphics = graphics.into();
        }
        if inventory.disks.iter().any(|d| d.name.is_some()) {
            let mut disks: Vec<DiskDrive> = inventory.disks.to_vec();
            for disk in &mut disks {
                let Some(want) = disk.name.as_deref() else {
                    continue;
                };
                let found =
                    find(&|id| matches!(id, DeviceIdentity::Disk { name, .. } if &**name == want));
                if found.is_some() && found != disk.device {
                    disk.device = found;
                    changed = true;
                }
            }
            inventory.disks = disks.into();
        }
        changed
    }

    /// Maps the stored inventory rows to catalog devices again and merges the API versions,
    /// for a GPU or disk entry that reached the catalog after the inventory (R-166).
    fn remap_inventory(&mut self) {
        let mut next = (*self.components.inventory).clone();
        let mapped = self.map_inventory_devices(&mut next);
        let merged = self.merge_api_versions(&mut next);
        if mapped || merged {
            self.components.inventory = Arc::new(next);
            self.mark(ComponentMask::INVENTORY);
        }
    }

    /// Sets the Vulkan and OpenGL versions of each graphics adapter from the stored
    /// `GpuApiVersions`, matched by the PCI bus ID of the adapter's catalog device. Returns true
    /// when an adapter matched.
    fn merge_api_versions(&self, inventory: &mut InventorySnapshot) -> bool {
        if self.api_versions.is_empty() {
            return false;
        }
        let mut graphics: Vec<GraphicsAdapter> = inventory.graphics.to_vec();
        let mut matched = false;
        for adapter in &mut graphics {
            let Some(DeviceIdentity::Gpu { pci_bus_id, .. }) = adapter
                .device
                .and_then(|idx| self.devices.entry(idx))
                .map(|e| &e.identity)
            else {
                continue;
            };
            let Some(v) = self
                .api_versions
                .iter()
                .find(|v| same_pci(&v.pci_bus_id, pci_bus_id))
            else {
                continue;
            };
            adapter.vulkan_version = probe_field(v.vulkan_api.as_deref());
            adapter.opengl_version = probe_field(v.opengl.as_deref());
            matched = true;
        }
        if matched {
            inventory.graphics = graphics.into();
        }
        matched
    }

    /// Rebuilds the rows of one system table from its component and stored query, and echoes
    /// the query generation and stores the table's counts. Always marks the
    /// component, so the generation is echoed even when the rows are equal (E09-027).
    fn rebuild_sys(&mut self, table: SysTable, settings: &EngineSettings) {
        let query = self.sys_query(table);
        let c = &self.components;
        let input = SysTableInput {
            services: &c.services,
            startup: &c.startup,
            connections: &c.connections,
            processes: &c.processes,
            settings,
        };
        match table {
            SysTable::Services => {
                let (out, counts, manager_counts) = sys_table::build_services(&input, &query);
                let next = Arc::make_mut(&mut self.components.services);
                next.rows = out.rows;
                next.query_generation = out.query_generation;
                next.counts = counts;
                next.manager_counts = manager_counts;
                self.mark(ComponentMask::SERVICES);
            }
            SysTable::Startup => {
                let (out, counts) = sys_table::build_startup(&input, &query);
                let next = Arc::make_mut(&mut self.components.startup);
                next.rows = out.rows;
                next.query_generation = out.query_generation;
                next.counts = counts;
                self.mark(ComponentMask::STARTUP);
            }
            SysTable::Connections => {
                let (out, counts) = sys_table::build_connections(&input, &query);
                let next = Arc::make_mut(&mut self.components.connections);
                next.rows = out.rows;
                next.query_generation = out.query_generation;
                next.counts = counts;
                self.mark(ComponentMask::CONNECTIONS);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{normalise_pci, same_pci};

    #[test]
    fn normalise_pci_domain_width_and_case() {
        assert_eq!(normalise_pci("0000:01:00.0"), Some((0, 1, 0, 0)));
        assert!(same_pci("00000000:01:00.0", "0000:01:00.0"));
        assert!(same_pci("0000:0A:1f.3", "00000000:0a:1F.3"));
        assert!(!same_pci("0000:01:00.0", "0000:02:00.0"));
        assert!(!same_pci("0000:01:00.0", "0000:01:00.1"));
    }

    #[test]
    fn normalise_pci_garbage_matches_nothing() {
        for bad in [
            "",
            "garbage",
            "0000:01:00",
            "0000:01.00.0",
            "0000:1g:00.0",
            "0000:100:00.0",
        ] {
            assert_eq!(normalise_pci(bad), None, "{bad}");
        }
        assert!(!same_pci("garbage", "garbage"));
        assert!(!same_pci("", ""));
    }
}
