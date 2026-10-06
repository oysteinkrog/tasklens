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
use rustc_hash::FxHashMap;
use tl_model::actions::{ActionEvent, ActionState, RejectReason};
use tl_model::command::{Command, DeviceCommand};
use tl_model::demand::Demand;
use tl_model::devices::{AggregateKind, DeviceIdentity, DeviceKind, NetKind};
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{ColumnId, DeviceIdx, MetricId, ProcessKey, SeriesId};
use tl_model::series::SampleFlags;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, DeviceDetailsSnapshot, DiskDetails, EjectState, NetDetails,
    SystemSnapshot,
};
use tl_model::system::{Metric, SensorKind};
use tl_model::table::{SysTable, Total};
use tl_model::time::{TimeNs, now_boot_ns};
use tl_model::validity::Validity;

use super::suspend::Transition;
use super::table::{self, SweepTotals, TableInput};
use super::{AdmissionView, HelperCommand, InvariantViolation, Reducer};
use crate::source::{QueuedCommand, WorkerCommand};
use crate::update::{
    CpuSample, DetailsUpdate, DeviceDetailsUpdate, DiskSample, EngineMsg, GpuSample, MemorySample,
    NetSample, SensorSample, SourceUpdate, ZramStat, source_of,
};

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
const GPU_METRICS: [MetricId; 7] = [
    MetricId::GPU_UTIL,
    MetricId::GPU_MEM,
    MetricId::GPU_ENC,
    MetricId::GPU_DEC,
    MetricId::GPU_TEMP,
    MetricId::GPU_POWER,
    MetricId::GPU_CLOCK,
];

/// Reducer state that only `apply` reads and writes.
#[derive(Default)]
pub struct ApplyState {
    /// Time of the last data sample per source, for the sample spacing.
    last_t: [TimeNs; SourceId::COUNT],
    /// The last period used per source, kept when two samples carry the same time.
    period: [u64; SourceId::COUNT],
    /// Δ(irq + softirq) / Δtotal of the last CPU sample, for the Interrupts row (E09-023).
    pub irq_pct: f32,
    /// zram devices of the last memory sample (E09-023).
    pub zram: Vec<ZramStat>,
    /// The running maximum of `Committed_AS` since engine start (R-166).
    pub peak_committed: u64,
    /// Device details per contributing source (R-139 item 7): sysfs and D-Bus.
    disk_sysfs: FxHashMap<DeviceIdx, DiskDetails>,
    disk_dbus: FxHashMap<DeviceIdx, DiskDetails>,
    net_sysfs: FxHashMap<DeviceIdx, NetDetails>,
    net_dbus: FxHashMap<DeviceIdx, NetDetails>,
    /// `supports_process_util` per GPU UUID from the last `GpuInfo` (E09-006).
    gpu_caps: FxHashMap<[u8; 16], bool>,
    /// Keys whose PSS the last demand asked for: `detail_keys` while a PSS or USS column is
    /// shown, else none (E09-006).
    pss_keys: Arc<[ProcessKey]>,
    /// The last health state per source was `Running` or `Late`.
    running: [bool; SourceId::COUNT],
    /// The source has been `Running` before in this session (R-289).
    ran: [bool; SourceId::COUNT],
}

impl ApplyState {
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
/// it. The next list from the worker replaces both.
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
        ],
        DeviceKind::Gpu => &GPU_METRICS,
        DeviceKind::Disk => &DISK_METRICS,
        DeviceKind::Net => &NET_METRICS,
        DeviceKind::Sensor => &[MetricId::SENSOR_TEMP],
        DeviceKind::Fan => &[MetricId::FAN_RPM],
        DeviceKind::Battery => &[MetricId::BATTERY_PCT],
        DeviceKind::Aggregate => &[],
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

fn sys_table_index(table: SysTable) -> usize {
    match table {
        SysTable::Services => 0,
        SysTable::Startup => 1,
        SysTable::Connections => 2,
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
                if self.suspend.on_sleep(entering, t_ns) == Transition::Resumed {
                    self.resume();
                }
            }
            SourceUpdate::Cpu(s) => self.apply_cpu(s)?,
            SourceUpdate::Memory(s) => self.apply_memory(s)?,
            SourceUpdate::Disks(s) => self.apply_disks(s)?,
            SourceUpdate::Nets(s) => self.apply_nets(s, settings)?,
            SourceUpdate::Sensors(s) => self.apply_sensors(s)?,
            SourceUpdate::Devices(list) => {
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
            SourceUpdate::Gpu(s) => self.apply_gpu(s, source, settings)?,
            SourceUpdate::GpuInfo(info) => {
                self.apply_state.gpu_caps.clear();
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
                    )?;
                }
                self.mark(ComponentMask::DEVICES);
            }
            SourceUpdate::GpuApiVersions(_) => {
                tracing::debug!(target: "tasklens::engine", "gpu api versions: E09-027");
            }
            SourceUpdate::Processes(sweep) => {
                let observed = (sweep.observed_start, sweep.observed_end);
                let sink = self.record_sink.as_deref_mut();
                self.processes
                    .apply_sweep(sweep, now_boot_ns(), settings.interval_ms, sink);
                if let Some(id) = source {
                    let period = u64::from(settings.interval_ms) * 1_000_000;
                    self.sample_ok(id, observed.1, period);
                }
                self.publish_processes(observed);
                self.rebuild_table(settings);
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
            SourceUpdate::Details(_) => {
                tracing::debug!(target: "tasklens::engine", "details update: E09-026");
            }
            SourceUpdate::Connections(_)
            | SourceUpdate::ConnectionsDestroySupport(_)
            | SourceUpdate::Services(_)
            | SourceUpdate::Startup(_)
            | SourceUpdate::Inventory(_)
            | SourceUpdate::Windows(_) => {
                tracing::debug!(target: "tasklens::engine", ?source, "system update: E09-027");
            }
            SourceUpdate::History(_) => {
                tracing::debug!(target: "tasklens::engine", "history view: E09-028");
            }
            SourceUpdate::Helper(_) | SourceUpdate::Setup(_) => {
                tracing::debug!(target: "tasklens::engine", "helper or setup update: E29-020");
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
                // E09-027 rebuilds the table from the stored query.
                let i = sys_table_index(q.table);
                self.sys_queries[i] = Some(q);
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
            Command::HistoryQuery { id, range, what } => self.forward(
                SourceId::HistoryRecorder,
                WorkerCommand::HistoryQuery { id, range, what },
            ),
            Command::HistoryDelete { keys } => self.forward(
                SourceId::HistoryRecorder,
                WorkerCommand::HistoryDelete { keys },
            ),
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
            _ => return,
        };
        self.mark(bit);
    }

    /// The index of a sampled device and whether it is present. An identity not in the
    /// catalog is added.
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
        let (idx, _) = self.devices.ensure(id, &name, None, &key, None)?;
        self.mark(ComponentMask::DEVICES);
        Ok((idx, true))
    }

    fn fixed_device(&self, id: &DeviceIdentity) -> Result<DeviceIdx, InvariantViolation> {
        self.devices
            .idx(id)
            .ok_or(InvariantViolation("fixed device missing from the catalog"))
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
        // Samples between `PrepareForSleep(true)` and the resume are gaps (ADR-008).
        let valid = valid && !self.suspend.drop_rates();
        self.series
            .append(sid(device, metric), t, value, period, flags(valid));
    }

    fn gaps(&mut self, device: DeviceIdx, metrics: &[MetricId], t: TimeNs) {
        for &metric in metrics {
            self.series.append_gap(sid(device, metric), t);
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
        self.apply_state.irq_pct = s.irq_pct;
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
        self.apply_state.zram = s.zram;
        self.system_mut(t).memory = m;
        self.sample_ok(SourceId::Memory, t, period);
        self.mark(ComponentMask::SERIES);
        Ok(())
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
            if present {
                self.push(
                    idx,
                    metric,
                    t,
                    r.value.value,
                    r.value.validity.is_valid(),
                    period,
                );
            } else {
                self.series.append_gap(sid(idx, metric), t);
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
        let mut rows = Vec::with_capacity(s.devices.len());
        let (mut util, mut mem) = (0.0f32, 0u64);
        let (mut util_ok, mut mem_ok) = (false, false);
        for (identity, mut g) in s.devices {
            let (idx, present) = self.sample_device(&identity)?;
            g.device = idx;
            if !present {
                self.gaps(idx, &GPU_METRICS, t);
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
            ];
            for (metric, value, validity) in values {
                self.push(idx, metric, t, value, validity.is_valid(), period);
            }
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
        if let Some(processes) = &s.processes {
            self.processes
                .apply_gpu_processes(processes, &self.apply_state.gpu_caps);
            self.republish_processes(settings);
        }
        Ok(())
    }

    fn apply_device_details(&mut self, d: DeviceDetailsUpdate) -> Result<(), InvariantViolation> {
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
                    rows.push((idx, battery));
                }
                rows.sort_unstable_by_key(|(idx, _)| *idx);
                next.batteries = rows.into();
                self.mark(ComponentMask::SERIES);
            }
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

    /// Publishes the process cache into `components.processes` when it changed.
    fn publish_processes(&mut self, observed: (TimeNs, TimeNs)) {
        let coverage = self.processes.coverage();
        if let Some(next) = self.processes.publish(coverage, observed) {
            self.components.processes = next;
            self.mark(ComponentMask::PROCESSES);
        }
    }

    /// Applies the demand of this batch: PSS of keys no longer demanded becomes `WarmingUp`
    /// and leaves coverage. A hidden PSS and USS column demands no keys.
    pub fn apply_demand(&mut self, demand: &Demand, settings: &EngineSettings) {
        let shown = demand.process_columns.contains(ColumnId::Pss)
            || demand.process_columns.contains(ColumnId::Uss);
        let keys: Arc<[ProcessKey]> = if shown {
            Arc::clone(&demand.detail_keys)
        } else {
            Arc::from([])
        };
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
        self.publish_processes((header.observed_start, header.observed_end));
        self.rebuild_table(settings);
    }

    /// Rebuilds the process table and its header totals from the current state. Always marks
    /// `TABLE`, so `query_generation` is echoed even when the rows are equal.
    fn rebuild_table(&mut self, settings: &EngineSettings) {
        let processes = Arc::clone(&self.components.processes);
        let system = Arc::clone(&self.components.system);
        let devices = Arc::clone(&self.components.devices);
        let unit_names = FxHashMap::default();
        let input = TableInput {
            records: &processes.records,
            extras: &processes.extras,
            app_stats: &processes.apps,
            unit_stats: self.processes.unit_stats(),
            system: &system,
            devices: &devices,
            settings,
            unit_names: &unit_names,
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
    }
}
