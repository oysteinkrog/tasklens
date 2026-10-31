//! ADR-018 Budgets: bytes on the wire for a host with 1500 processes at a 1 s refresh.
//!
//! - A session on the Processes tab sends at most 64 KiB/s. Measured: 44,342 B/s (43.3 KiB/s),
//!   2026-10-31.
//! - A hidden host sends at most 2 KiB/s.
//!
//! Bytes are counted as the session sends them (leader decision on tasklens-q4x76.15, Q2 A):
//! each 1 s frame from `DeltaEncoder::next_frame` with its envelope from `FrameWriter`, and the
//! client's `Ack` frame after it, over 60 s from the session's start, so the complete first frame
//! counts. The client side applies every frame, so the stream is one a client can follow.
//!
//! The host is a seeded synthetic one, `Host` below (leader decision, Q1 C): no recording of a
//! real machine, so no real process names or paths. Its shape and churn come from one
//! measurement on the office desktop (`measured`).
//!
//! A Processes tab session gets every part its demand gates on (`ClientSession::snapshot` in
//! tl-engine): besides the process view and the series, the services (`process_table`), and
//! with a visible window the inventory, windows, devices, health and device details. Their
//! sizes and how often each changes come from the real engine (`real`).

#![allow(clippy::unwrap_used)]
// The measured numbers are part of the result; `--nocapture` shows them.
#![allow(clippy::print_stdout)]

use std::collections::VecDeque;
use std::sync::Arc;

use tl_model::actions::CgroupRef;
use tl_model::devices::{AggregateKind, DeviceCatalog, DeviceEntry, DeviceIdentity, NetKind};
use tl_model::health::HealthSnapshot;
use tl_model::ids::{DeviceIdx, GroupKey, MetricId, NodeId, ProcessKey, SeriesId};
use tl_model::process::{
    AppCgroupSample, Coverage, GroupKind, GroupRow, GroupRowKind, ProcFlags, ProcState,
    ProcessExtras, ProcessExtrasP2, ProcessMeta, ProcessRecord,
};
use tl_model::remote_proto::{ClientMsg, Part, ServerMsg, SnapshotFrame, encode_message};
use tl_model::series::{Sample, SeriesView};
use tl_model::snapshot::{
    BoardInfo, ComponentHeader, CpuStaticInfo, DeviceDetailsSnapshot, DiskDetails, DiskDrive,
    DisplayConnector, DisplayMode, FirmwareInfo, FirmwareType, FrameGeometry, GraphicsAdapter,
    InventorySnapshot, MemoryDetails, MemoryInfo, MemoryModule, NetDetails, NetworkAdapter, OsInfo,
    PartitionRow, PartitionTable, ProcessSnapshot, ProcessorInfo, SeriesSet, ServiceStatus,
    ServicesSnapshot, Snapshot, StartupType, SwapDevice, SwapKindModel, SystemInfo, SystemSnapshot,
    UnitRecord, WindowEntry, WindowsSnapshot, ZramDetail,
};
use tl_model::system::{
    CoreInfo, CoreKind, CpuSet, CpuTopology, CpuTotals, DiskTotals, LinkState, MemGroupBand,
    MemoryTotals, Metric, NetTotals, SensorKind, SensorReading,
};
use tl_model::table::{Manager, ProcessTableView, TableRow, TableTotals, Total, UnitRef, UnitType};
use tl_model::time::{BootId, TimeNs};
use tl_model::validity::{FieldValidity, Validity};
use tl_remote::delta::{DeltaEncoder, apply};
use tl_remote::envelope::FrameWriter;

const SEC: u64 = 1_000_000_000;
const BOOT: BootId = BootId(0x5eed);
/// Seconds measured: 60 frames at 1 s, the first complete.
const SECONDS: u64 = 60;
const PROCESSES_TAB_MAX: f64 = 64.0 * 1024.0;
const HIDDEN_HOST_MAX: f64 = 2.0 * 1024.0;

/// One measurement on the office desktop (oystein-office-primary, 32 logical CPUs), 2026-10-31,
/// at a load average of 6 to 7 from other agents' builds, so the churn is on the high side.
///
/// Churn: `python3 -I churn.py 60`, which reads every /proc/<pid>/stat once per second for 61 s
/// and counts, per second, the rows whose CPU % changes (the tick delta differs from the
/// previous second's), the rows whose RSS changes, the starts and the exits. Means over 60 s at
/// 1153 processes: 62.75 CPU, 18.83 RSS, 67.98 either, 3.95 starts, 3.83 exits.
///
/// Shape: `python3 -I groups.py`, which sorts the processes into the grouped table's collapsed
/// rows by /proc/<pid>/stat flags and /proc/<pid>/cgroup: 522 kernel threads, 20 app cgroups
/// holding 138 processes, 69 other units with more than one process holding 430, and 61 loose
/// processes.
///
/// Both were one-off Python scripts in the lane's scratch directory, not kept in the repo; the
/// bead comment on tasklens-q4x76.15 has the churn script. The numbers below are those, scaled
/// from 1153 processes to 1500 (× 1.3) and rounded.
mod measured {
    pub const PROCESSES: usize = 1500;
    pub const KERNEL_THREADS: usize = 680;
    pub const APP_GROUPS: usize = 26;
    pub const APP_PROCESSES: usize = 180;
    pub const UNIT_GROUPS: usize = 90;
    pub const UNIT_PROCESSES: usize = 560;
    pub const LOOSE: usize = 80;
    /// Rows whose CPU % changes each second.
    pub const CPU_CHANGES: usize = 82;
    /// Rows whose memory changes each second.
    pub const MEMORY_CHANGES: usize = 24;
    pub const STARTS: usize = 5;
    pub const EXITS: usize = 5;
}
use measured as m;

/// One run of `wire_bytes_of_the_real_engine_on_this_host` (tl-server/tests/budgets.rs) on the
/// office desktop, 2026-10-31, a Processes tab session at 1166 processes after 610 s, load
/// average 6.2 at the end. Per part: its uncompressed size in the complete frame, and how many
/// of the 59 delta frames carried it whole. The fixture builds each part at about that size and
/// changes it as often. These parts do not grow with the process count, so they are not scaled.
/// The fixture's sizes: services 201,301 B, devices 8,202 B, windows 2,440 B, device details
/// 2,046 B, inventory 2,146 B and health 94 B (the empty one, with its version bumped).
mod real {
    /// Systemd units of both managers: 198,057 B, about 219 B a unit. In 5 of 59 delta frames:
    /// a signal batch from systemd rebuilds the part.
    pub const UNITS: usize = 904;
    pub const SERVICES_PERIOD_S: u64 = 12;
    /// KWin windows: 2,360 B. In every delta frame: the worker publishes a new `Arc` each
    /// second.
    pub const WINDOWS: usize = 22;
    /// Device catalog entries: 8,066 B. In 23 of 59 delta frames.
    pub const DEVICES: usize = 136;
    /// Device details: 2,330 B. In 19 of 59 delta frames, every third second.
    pub const DEVICE_DETAILS_PERIOD_S: u64 = 3;
    /// Inventory: 2,571 B, only in the complete frame. Health: 144 B, in every delta frame.
    pub const INVENTORY_DISKS: usize = 3;
    pub const INVENTORY_NETS: usize = 7;
}

/// Device series the background demand samples at 1 s (ADR-018 Background demand, ADR-017): the
/// real engine on the office desktop had 292 after 10 minutes, 2026-10-31
/// (`wire_bytes_of_the_real_engine_on_this_host` in tl-server/tests/budgets.rs). Each has a fast
/// ring (62 samples at 1 s) and a 1 Hz ring (602 samples), and both are full: the engine has run
/// for over 10 minutes.
const DEVICE_SERIES: usize = 292;
const FAST_LEN: usize = 62;
/// Without a process view the sweep runs only for history, every 10 s.
const HISTORY_SWEEP_S: u64 = 10;
const SLOW_LEN: usize = 602;

/// SplitMix64: small, seeded, the same stream on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// A float in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// A lowercase word of 4 to 10 letters.
    fn word(&mut self) -> String {
        let len = 4 + self.below(7);
        (0..len)
            .map(|_| (b'a' + self.below(26) as u8) as char)
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Kernel,
    App(usize),
    Unit(usize),
    Loose,
}

struct Proc {
    kind: Kind,
    record: ProcessRecord,
    extras: ProcessExtras,
}

struct Ring {
    id: SeriesId,
    version: u64,
    fast: VecDeque<Sample>,
    slow: VecDeque<Sample>,
    fast_view: Arc<SeriesView>,
    slow_view: Arc<SeriesView>,
}

fn view(id: SeriesId, version: u64, samples: &VecDeque<Sample>) -> Arc<SeriesView> {
    Arc::new(SeriesView {
        id,
        version,
        period_ns: SEC,
        samples: samples.iter().copied().collect(),
    })
}

fn header(version: u64, t_ns: TimeNs) -> ComponentHeader {
    ComponentHeader {
        version,
        observed_start: t_ns.saturating_sub(SEC),
        observed_end: t_ns,
        validity: Validity::Valid,
    }
}

fn valid<T: Copy + Default>(v: T) -> Metric<T> {
    Metric::valid(v)
}

/// A known inventory value.
fn known<T>(value: T) -> Total<T> {
    Total {
        value,
        validity: Validity::Valid,
    }
}

fn text(s: impl AsRef<str>) -> Arc<str> {
    Arc::from(s.as_ref())
}

/// Partition `n` of disk `d`, for its device details.
fn partition(
    d: u16,
    n: u16,
    start_bytes: u64,
    size_bytes: u64,
    mount: &str,
    fs_type: &str,
    type_name: &str,
) -> PartitionRow {
    PartitionRow {
        name: text(format!("nvme{d}n1p{n}")),
        start_bytes,
        size_bytes,
        holder: None,
        mount: Some(text(mount)),
        extra_mounts: Arc::from([]),
        label: Some(text(format!("part{d}{n}"))),
        fs_type: Some(text(fs_type)),
        usage_pct: known(40.0),
        flags: Arc::from([]),
        type_name: text(type_name),
        read_only: false,
        unallocated: false,
    }
}

/// A seeded host whose processes and devices change once per second at the measured rates.
struct Host {
    rng: Rng,
    /// Sorted by key; new processes start later, so they go at the end.
    procs: Vec<Proc>,
    apps: Vec<(GroupKey, Arc<str>, Arc<str>)>,
    units: Vec<UnitRef>,
    rings: Vec<Ring>,
    next_pid: u32,
    /// Monotonic time, about 20 hours after boot.
    now_ns: TimeNs,
    tick: u64,
    topology: Arc<CpuTopology>,
    bands: Arc<Vec<MemGroupBand>>,
    /// Every unit of both managers; the first `m::UNIT_GROUPS` are `units`.
    unit_records: Vec<UnitRecord>,
    windows: Vec<WindowEntry>,
    catalog: Vec<DeviceEntry>,
    disk_details: Vec<(DeviceIdx, DiskDetails)>,
    /// Every part this test never changes keeps one `Arc`, as the engine reuses it.
    base: Snapshot,
}

impl Host {
    fn new(seed: u64) -> Host {
        let mut rng = Rng(seed);
        let now_ns = 72_000 * SEC;
        let apps = (0..m::APP_GROUPS)
            .map(|i| {
                let name = rng.word();
                (
                    GroupKey(0xA000 + i as u64),
                    Arc::from(format!("org.example.{name}")),
                    Arc::from(format!("app-{name}@{i}.scope")),
                )
            })
            .collect();
        let units = (0..m::UNIT_GROUPS)
            .map(|i| UnitRef {
                manager: if i % 3 == 0 {
                    Manager::User
                } else {
                    Manager::System
                },
                name: Arc::from(format!("{}-{i}.service", rng.word())),
            })
            .collect();
        let rings = (0..DEVICE_SERIES)
            .map(|i| {
                let id = SeriesId {
                    device: DeviceIdx((i / 8) as u16),
                    metric: MetricId(1 + (i % 8) as u16),
                };
                let fast: VecDeque<Sample> = (0..FAST_LEN as u64)
                    .map(|k| {
                        Sample::valid(now_ns - (FAST_LEN as u64 - k) * SEC, rng.unit() * 100.0)
                    })
                    .collect();
                let slow: VecDeque<Sample> = (0..SLOW_LEN as u64)
                    .map(|k| {
                        Sample::valid(now_ns - (SLOW_LEN as u64 - k) * SEC, rng.unit() * 100.0)
                    })
                    .collect();
                let slow_id = SeriesId {
                    device: id.device,
                    metric: id.metric.with_tier_1hz(),
                };
                Ring {
                    id,
                    version: 1000,
                    fast_view: view(id, 1000, &fast),
                    slow_view: view(slow_id, 1000, &slow),
                    fast,
                    slow,
                }
            })
            .collect();
        let topology = Arc::new(CpuTopology {
            online: CpuSet([u32::MAX as u64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            cores: (0..16u16)
                .map(|c| CoreInfo {
                    id: c,
                    logical: [c, c + 16].into_iter().collect(),
                    l3_group: c / 8,
                    kind: CoreKind::Standard,
                })
                .collect(),
        });
        let bands = Arc::new(
            (0..22u16)
                .map(|i| MemGroupBand {
                    key: Arc::from(format!("exe:{}", rng.word())),
                    name: Arc::from(rng.word()),
                    process_count: 1 + i as u32,
                    device: DeviceIdx(100 + i),
                    wine: None,
                })
                .collect(),
        );
        let mut host = Host {
            rng,
            procs: Vec::with_capacity(m::PROCESSES + 64),
            apps,
            units,
            rings,
            next_pid: 2,
            now_ns,
            tick: 0,
            topology,
            bands,
            unit_records: Vec::new(),
            windows: Vec::new(),
            catalog: Vec::new(),
            disk_details: Vec::new(),
            base: Snapshot::empty(BOOT, 1),
        };
        let mut kinds = Vec::with_capacity(m::PROCESSES);
        kinds.extend((0..m::KERNEL_THREADS).map(|_| Kind::Kernel));
        kinds.extend((0..m::APP_PROCESSES).map(|i| Kind::App(i % m::APP_GROUPS)));
        kinds.extend((0..m::UNIT_PROCESSES).map(|i| Kind::Unit(i % m::UNIT_GROUPS)));
        kinds.extend((0..m::LOOSE).map(|_| Kind::Loose));
        // Kernel threads start first; the rest in a seeded order.
        for i in (m::KERNEL_THREADS + 1..kinds.len()).rev() {
            let j = m::KERNEL_THREADS + host.rng.below(i - m::KERNEL_THREADS + 1);
            kinds.swap(i, j);
        }
        for kind in kinds {
            host.start(kind);
        }
        host.unit_records = host.make_units();
        host.windows = host.make_windows();
        host.catalog = host.make_catalog();
        host.disk_details = host.make_disk_details();
        host.base.inventory = host.inventory();
        host.base.services = host.services(1);
        host.base.devices = host.devices(1);
        host.base.device_details = host.device_details(1);
        host
    }

    fn start(&mut self, kind: Kind) {
        let pid = self.next_pid;
        self.next_pid += 1 + self.rng.below(3) as u32;
        let rng = &mut self.rng;
        let comm = match kind {
            Kind::Kernel => format!("kworker/{}:{}-{}", rng.below(32), rng.below(4), rng.word()),
            _ => rng.word(),
        };
        let (exe, cmdline, user, cgroup, group, app_id) = match kind {
            Kind::Kernel => (None, None, "root", None, GroupKey(3), None),
            Kind::App(a) => {
                let (key, app_id, scope) = &self.apps[a];
                let exe = format!("/usr/lib/{}/{comm}", rng.word());
                let cmdline = format!(
                    "{exe} --type={} --field-trial-handle={} --lang=en-US --{}={}",
                    rng.word(),
                    rng.next(),
                    rng.word(),
                    rng.word()
                );
                (
                    Some(exe),
                    Some(cmdline),
                    "user",
                    Some(format!(
                        "user.slice/user-1000.slice/user@1000.service/app.slice/{scope}"
                    )),
                    *key,
                    Some(app_id.clone()),
                )
            }
            Kind::Unit(u) => {
                let unit = &self.units[u];
                let exe = format!("/usr/lib/{}/{comm}", rng.word());
                let cmdline = format!("{exe} --{} {}", rng.word(), rng.word());
                let (user, cgroup) = match unit.manager {
                    Manager::User => (
                        "user",
                        format!("user.slice/user-1000.slice/user@1000.service/{}", unit.name),
                    ),
                    Manager::System => ("root", format!("system.slice/{}", unit.name)),
                };
                (
                    Some(exe),
                    Some(cmdline),
                    user,
                    Some(cgroup),
                    GroupKey(2),
                    None,
                )
            }
            Kind::Loose => {
                let exe = format!("/usr/bin/{comm}");
                let cmdline = format!("{comm} -{} {}", rng.word(), rng.word());
                (
                    Some(exe),
                    Some(cmdline),
                    "user",
                    Some("user.slice/user-1000.slice/session-2.scope".to_owned()),
                    GroupKey(2),
                    None,
                )
            }
        };
        let kernel = kind == Kind::Kernel;
        let meta = Arc::new(ProcessMeta {
            name: Arc::from(comm.as_str()),
            search_lower: Arc::from(comm.as_str()),
            comm: Arc::from(comm.as_str()),
            exe: exe.as_deref().map(Arc::from),
            cmdline: cmdline.as_deref().map(Arc::from),
            user: Arc::from(user),
            cgroup: cgroup.as_deref().map(Arc::from),
            app_id,
            icon: None,
            description: None,
            desktop_path: None,
        });
        let rss = if kernel {
            0
        } else {
            (1 + rng.below(200_000) as u64) * 4096
        };
        let key = ProcessKey {
            start_ticks: self.now_ns / 10_000_000 + self.tick,
            boot_id: BOOT,
            pid,
        };
        self.tick += 1;
        let record = ProcessRecord {
            key,
            ppid: if kernel {
                2
            } else {
                1 + rng.below(pid as usize) as u32
            },
            uid: if user == "root" { 0 } else { 1000 },
            meta,
            group,
            state: ProcState::Sleeping,
            flags: if kernel {
                ProcFlags::KERNEL_THREAD
            } else {
                ProcFlags::empty()
            },
            nice: 0,
            threads: if kernel { 1 } else { 1 + rng.below(40) as u32 },
            cpu_pct: 0.0,
            rss_anon_bytes: rss / 2,
            rss_bytes: rss,
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
        };
        let extras = ProcessExtras {
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: rng.below(100_000) as u64,
            stime_ticks: rng.below(10_000) as u64,
            start_boot_ns: key.start_ticks * 10_000_000,
            rss_peak_bytes: rss,
            rss_shared_bytes: rss / 4,
            commit_bytes: rss * 2,
            vsize_bytes: rss * 8,
            vsize_peak_bytes: rss * 8,
            page_faults: rng.below(1_000_000) as u64,
            page_faults_delta: 0,
            major_faults: 0,
            policy: tl_model::actions::SchedPolicy::Other,
            rt_prio: 0,
            affinity: CpuSet([u32::MAX as u64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            handles: if kernel {
                None
            } else {
                Some(rng.below(500) as u32)
            },
            exe_name: record.meta.comm.clone(),
            exe_path: record.meta.exe.clone().unwrap_or_else(|| Arc::from("")),
            exe_deleted: false,
            unit: match kind {
                Kind::Unit(u) => Some(self.units[u].clone()),
                _ => None,
            },
            state_letter: if kernel { b'I' } else { b'S' },
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
            pgrp: pid,
            sid: pid,
            last_cpu: rng.below(32) as u32,
            wine: None,
        };
        self.procs.push(Proc {
            kind,
            record,
            extras,
        });
    }

    /// `n` distinct rows, drawn at random from those `pick` accepts. A rejected or repeated row
    /// is drawn again, so a second changes exactly `n` rows.
    fn draw(&mut self, n: usize, pick: fn(&Proc) -> bool) -> Vec<usize> {
        let mut rows = Vec::with_capacity(n);
        while rows.len() < n {
            let i = self.rng.below(self.procs.len());
            if pick(&self.procs[i]) && !rows.contains(&i) {
                rows.push(i);
            }
        }
        rows
    }

    /// One second of churn at the measured rates. Every drawn row changes. Memory changes and
    /// exits are drawn from user-space rows only: kernel threads have no RSS, and starts make
    /// only user-space processes, so exits do too and the host stays at `m::PROCESSES`.
    fn step_processes(&mut self) {
        for i in self.draw(m::CPU_CHANGES, |_| true) {
            let mut pct = if self.rng.unit() < 0.7 {
                0.1 + self.rng.unit() * 25.0
            } else {
                0.0
            };
            if pct == self.procs[i].record.cpu_pct {
                pct = if pct == 0.0 { 0.1 } else { 0.0 };
            }
            let p = &mut self.procs[i];
            p.record.cpu_pct = pct;
            p.extras.cpu_user_pct = pct * 0.8;
            p.extras.cpu_kernel_pct = pct * 0.2;
            p.extras.cpu_relative_pct = pct / 32.0;
            p.extras.utime_ticks += (pct * 0.8) as u64;
            p.extras.stime_ticks += (pct * 0.2) as u64;
        }
        for i in self.draw(m::MEMORY_CHANGES, |p| p.kind != Kind::Kernel) {
            let delta = (1 + self.rng.below(512) as u64) * 4096;
            let grow = self.rng.unit() < 0.5;
            let p = &mut self.procs[i];
            let rss = p.record.rss_bytes;
            p.record.rss_bytes = if grow || rss < delta + 4096 {
                rss + delta
            } else {
                rss - delta
            };
            p.record.rss_anon_bytes = p.record.rss_bytes / 2;
            p.extras.rss_peak_bytes = p.extras.rss_peak_bytes.max(p.record.rss_bytes);
        }
        let mut exits = self.draw(m::EXITS, |p| p.kind != Kind::Kernel);
        exits.sort_unstable();
        for i in exits.into_iter().rev() {
            self.procs.remove(i);
        }
        for _ in 0..m::STARTS {
            // A start joins a group in proportion to the user-space processes.
            let pick = self
                .rng
                .below(m::APP_PROCESSES + m::UNIT_PROCESSES + m::LOOSE);
            let kind = if pick < m::APP_PROCESSES {
                Kind::App(self.rng.below(m::APP_GROUPS))
            } else if pick < m::APP_PROCESSES + m::UNIT_PROCESSES {
                Kind::Unit(self.rng.below(m::UNIT_GROUPS))
            } else {
                Kind::Loose
            };
            self.start(kind);
        }
    }

    /// One new sample on every device series.
    fn step_series(&mut self) {
        for r in &mut self.rings {
            let s = Sample::valid(self.now_ns, self.rng.unit() * 100.0);
            r.fast.pop_front();
            r.fast.push_back(s);
            r.slow.pop_front();
            r.slow.push_back(s);
            r.version += 1;
            r.fast_view = view(r.id, r.version, &r.fast);
            r.slow_view = view(r.slow_view.id, r.version, &r.slow);
        }
    }

    fn system(&mut self, version: u64) -> Arc<SystemSnapshot> {
        let rng = &mut self.rng;
        let util = rng.unit() * 40.0;
        let cpu = CpuTotals {
            util_pct: valid(util),
            kernel_pct: valid(util * 0.2),
            iowait_pct: valid(rng.unit()),
            freq_mhz: valid(3000 + rng.below(1500) as u32),
            freq_fastest_mhz: valid(5000 + rng.below(700) as u32),
            per_cpu_mhz: (0..32)
                .map(|_| valid(2000 + rng.below(3700) as u32))
                .collect(),
            base_mhz: valid(4500),
            temp_c: valid(40.0 + rng.unit() * 30.0),
            processes: valid(self.procs.len() as u32),
            threads: valid(5000 + rng.below(500) as u32),
            handles: valid(30_000 + rng.below(1000) as u64),
            load_avg: valid([6.5, 9.1, 11.3]),
            psi_some_pct: valid(rng.unit() * 5.0),
            ..CpuTotals::default()
        };
        let total = 64u64 << 30;
        let used = (20u64 << 30) + rng.below(1 << 30) as u64;
        let memory = MemoryTotals {
            validity: Validity::Valid,
            total,
            available: total - used,
            used,
            free: total - used - (8 << 30),
            cached: 8 << 30,
            dirty: rng.below(1 << 24) as u64,
            committed: used + (4 << 30),
            commit_limit: total,
            slab_reclaimable: 1 << 30,
            slab_unreclaimable: 1 << 29,
            swap_total: 16 << 30,
            swap_free: 15 << 30,
            psi_some_pct: valid(rng.unit()),
            psi_full_pct: valid(rng.unit() * 0.5),
            groups: self.bands.clone(),
            ..MemoryTotals::default()
        };
        let disks = (0..3u16)
            .map(|d| {
                (
                    DeviceIdx(d),
                    DiskTotals {
                        device: DeviceIdx(d),
                        active_pct: valid(rng.unit() * 10.0),
                        read_bps: valid(rng.unit() as f64 * 1e7),
                        write_bps: valid(rng.unit() as f64 * 1e7),
                        response_ms: valid(rng.unit()),
                        read_total: rng.next() >> 20,
                        written_total: rng.next() >> 20,
                        capacity: 2 << 40,
                        temp_c: valid(40.0),
                    },
                )
            })
            .collect();
        let nets = (0..3u16)
            .map(|d| {
                (
                    DeviceIdx(10 + d),
                    NetTotals {
                        device: DeviceIdx(10 + d),
                        rx_bps: valid(rng.unit() as f64 * 1e6),
                        tx_bps: valid(rng.unit() as f64 * 1e6),
                        rx_total: rng.next() >> 20,
                        tx_total: rng.next() >> 20,
                        link_speed_mbps: valid(1000),
                        state: LinkState::Up,
                    },
                )
            })
            .collect();
        let sensors = (0..12u16)
            .map(|d| {
                (
                    DeviceIdx(20 + d),
                    SensorReading {
                        device: DeviceIdx(20 + d),
                        kind: if d < 9 {
                            SensorKind::Temperature
                        } else {
                            SensorKind::Fan
                        },
                        value: valid(30.0 + rng.unit() * 40.0),
                        pwm_pct: Metric::with(Validity::Unsupported),
                        fan_control: None,
                        fan_connected: d >= 9,
                        fan_max_rpm: None,
                    },
                )
            })
            .collect();
        Arc::new(SystemSnapshot {
            header: header(version, self.now_ns),
            cpu,
            memory,
            disks,
            nets,
            gpus: Arc::from([]),
            sensors,
            cpu_topology: self.topology.clone(),
            boot_time_s: 1_761_800_000,
            color_scheme: 1,
        })
    }

    fn series(&self, version: u64) -> Arc<SeriesSet> {
        Arc::new(SeriesSet {
            header: header(version, self.now_ns),
            series: self.rings.iter().map(|r| r.fast_view.clone()).collect(),
            slow: self.rings.iter().map(|r| r.slow_view.clone()).collect(),
        })
    }

    fn processes(&self, version: u64) -> Arc<ProcessSnapshot> {
        let apps = self
            .apps
            .iter()
            .enumerate()
            .map(|(a, (key, _, scope))| {
                let members = self.procs.iter().filter(|p| p.kind == Kind::App(a));
                let (pids, anon) =
                    members.fold((0, 0), |(n, b), p| (n + 1, b + p.record.rss_anon_bytes));
                AppCgroupSample {
                    group: *key,
                    anon_bytes: anon,
                    swap_bytes: 0,
                    io_read_bps: 0.0,
                    io_write_bps: 0.0,
                    rios: 0,
                    wios: 0,
                    pids,
                    frozen: false,
                    cgroup: CgroupRef {
                        rel_path: Arc::from(format!("user.slice/app.slice/{scope}")),
                        cgroup_id: 0x1000 + a as u64,
                    },
                    validity: FieldValidity::all(Validity::Valid),
                }
            })
            .collect();
        Arc::new(ProcessSnapshot {
            header: header(version, self.now_ns),
            records: self.procs.iter().map(|p| p.record.clone()).collect(),
            coverage: Coverage::default(),
            extras: self.procs.iter().map(|p| p.extras.clone()).collect(),
            apps,
            extras_p2: self
                .procs
                .iter()
                .map(|_| ProcessExtrasP2::default())
                .collect(),
            low_memory: None,
        })
    }

    /// The default `TableQuery`: grouped, sorted by PID ascending, nothing expanded. The rows are
    /// the three collapsed section headers (Apps, Background processes, System processes), and
    /// `groups` holds a row per app, per unit with more than one process and one for kernel
    /// threads, as the engine builds them (the real engine on the office desktop: 3 rows, 81
    /// groups).
    fn table(&self, version: u64) -> Arc<ProcessTableView> {
        let n_groups = m::APP_GROUPS + m::UNIT_GROUPS + 1;
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); n_groups];
        for (i, p) in self.procs.iter().enumerate() {
            match p.kind {
                Kind::App(a) => members[a].push(i),
                Kind::Unit(u) => members[m::APP_GROUPS + u].push(i),
                Kind::Kernel => members[n_groups - 1].push(i),
                Kind::Loose => {}
            }
        }
        let mut groups = Vec::new();
        for (g, list) in members.iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            let (node, key, kind, class, name) = if g < m::APP_GROUPS {
                let (key, app_id, _) = &self.apps[g];
                (
                    NodeId::Group(*key),
                    *key,
                    GroupRowKind::App,
                    GroupKind::App {
                        app_id: app_id.clone(),
                    },
                    app_id.clone(),
                )
            } else if g < n_groups - 1 {
                let unit = &self.units[g - m::APP_GROUPS];
                (
                    NodeId::Unit(unit.clone()),
                    GroupKey(0xB000 + g as u64),
                    GroupRowKind::Unit,
                    GroupKind::System,
                    unit.name.clone(),
                )
            } else {
                (
                    NodeId::KernelThreads,
                    GroupKey(3),
                    GroupRowKind::KernelThreads,
                    GroupKind::System,
                    Arc::from("Kernel threads"),
                )
            };
            let sum =
                |f: &dyn Fn(&Proc) -> f64| list.iter().map(|&i| f(&self.procs[i])).sum::<f64>();
            let cpu = sum(&|p| p.record.cpu_pct as f64) as f32;
            let rss = sum(&|p| p.record.rss_bytes as f64) as u64;
            let anon = sum(&|p| p.record.rss_anon_bytes as f64) as u64;
            let threads = sum(&|p| p.record.threads as f64) as u32;
            let utime = sum(&|p| p.extras.utime_ticks as f64) as u64;
            let stime = sum(&|p| p.extras.stime_ticks as f64) as u64;
            groups.push(GroupRow {
                node,
                key,
                kind,
                class,
                name,
                icon: None,
                members: list.len() as u32,
                pids: list.len() as u32,
                note: None,
                memory_bytes: anon,
                disk_bps: 0.0,
                net_bps: 0.0,
                gpu_pct: 0.0,
                cpu_pct: cpu,
                rss_anon_bytes: anon,
                rss_bytes: rss,
                pss_bytes: 0,
                uss_bytes: 0,
                swap_bytes: 0,
                io_read_bps: 0.0,
                io_write_bps: 0.0,
                net_rx_bps: 0.0,
                net_tx_bps: 0.0,
                gpu_util_pct: 0.0,
                gpu_mem_bytes: 0,
                threads,
                cpu_kernel_pct: cpu * 0.2,
                cpu_user_pct: cpu * 0.8,
                cpu_relative_pct: cpu / 32.0,
                utime_ticks: utime,
                stime_ticks: stime,
                commit_bytes: rss * 2,
                page_faults: 0,
                page_faults_delta: 0,
                major_faults: 0,
                handles: Some(threads * 10),
                desktop_path: None,
                description: None,
                user: None,
                frozen: false,
                validity: FieldValidity::all(Validity::Valid),
            });
        }
        // Apps; Background: this user's processes outside an app; System: the rest.
        let mut counts = [0u32; 3];
        for p in &self.procs {
            let section = match p.kind {
                Kind::App(_) => 0,
                Kind::Loose => 1,
                Kind::Unit(u) if self.units[u].manager == Manager::User => 1,
                Kind::Unit(_) | Kind::Kernel => 2,
            };
            counts[section] += 1;
        }
        let rows: Vec<TableRow> = counts
            .iter()
            .enumerate()
            .map(|(c, &count)| TableRow {
                node: NodeId::Group(GroupKey(1 + c as u64)),
                depth: 0,
                expanded: false,
                has_children: count > 0,
                record: None,
                group: None,
                descendants: count,
                subtree: None,
            })
            .collect();
        let cpu: f32 = self.procs.iter().map(|p| p.record.cpu_pct).sum::<f32>() / 32.0;
        Arc::new(ProcessTableView {
            header: header(version, self.now_ns),
            query_generation: 1,
            rows: rows.into(),
            groups: groups.into(),
            subtree: Arc::from([]),
            totals: TableTotals {
                cpu_pct: Total {
                    value: cpu,
                    validity: Validity::Valid,
                },
                ..TableTotals::default()
            },
            total: self.procs.len() as u32,
            shown: self.procs.len() as u32,
            restricted: 0,
        })
    }

    /// The units of both managers, System first, as the engine sorts them. The first
    /// `m::UNIT_GROUPS` are the units the processes run in: running services with a main PID.
    /// The rest are a mix of unit types, a third of them active.
    fn make_units(&mut self) -> Vec<UnitRecord> {
        const KINDS: [(UnitType, &str); 9] = [
            (UnitType::Service, "service"),
            (UnitType::Service, "service"),
            (UnitType::Service, "service"),
            (UnitType::Socket, "socket"),
            (UnitType::Timer, "timer"),
            (UnitType::Mount, "mount"),
            (UnitType::Target, "target"),
            (UnitType::Slice, "slice"),
            (UnitType::Path, "path"),
        ];
        let mut out = Vec::with_capacity(real::UNITS);
        for i in 0..real::UNITS {
            let (unit, kind, running, main_pid) = if i < m::UNIT_GROUPS {
                let pid = self
                    .procs
                    .iter()
                    .find(|p| p.kind == Kind::Unit(i))
                    .map_or(0, |p| p.record.key.pid);
                (self.units[i].clone(), UnitType::Service, true, pid)
            } else {
                let (kind, suffix) = KINDS[self.rng.below(KINDS.len())];
                let manager = if self.rng.below(4) == 0 {
                    Manager::User
                } else {
                    Manager::System
                };
                let name = text(format!(
                    "{}-{}-{}.{suffix}",
                    self.rng.word(),
                    self.rng.word(),
                    self.rng.word()
                ));
                (UnitRef { manager, name }, kind, self.rng.below(3) == 0, 0)
            };
            out.push(self.unit_record(unit, kind, running, main_pid));
        }
        out.sort_by_key(|r| r.unit.manager == Manager::User);
        out
    }

    fn unit_record(
        &mut self,
        unit: UnitRef,
        kind: UnitType,
        running: bool,
        main_pid: u32,
    ) -> UnitRecord {
        let rng = &mut self.rng;
        let (dir, slice) = match unit.manager {
            Manager::System => ("/usr/lib/systemd/system", "system.slice"),
            Manager::User => (
                "/usr/lib/systemd/user",
                "user.slice/user-1000.slice/user@1000.service/app.slice",
            ),
        };
        let (status, sub_state) = match (kind, running) {
            (UnitType::Service, true) => (ServiceStatus::Running, "running"),
            (UnitType::Socket, true) => (ServiceStatus::Active, "listening"),
            (UnitType::Timer, true) => (ServiceStatus::Active, "waiting"),
            (UnitType::Mount, true) => (ServiceStatus::Active, "mounted"),
            (_, true) => (ServiceStatus::Active, "active"),
            (_, false) => (ServiceStatus::Stopped, "dead"),
        };
        let service = kind == UnitType::Service;
        let unit_file = format!("{dir}/{}", unit.name);
        let path = if service {
            format!("/usr/lib/{}/{}", rng.word(), rng.word())
        } else {
            unit_file.clone()
        };
        let control_group = (running && matches!(kind, UnitType::Service | UnitType::Slice))
            .then(|| text(format!("/{slice}/{}", unit.name)));
        let socket = service && rng.below(5) == 0;
        UnitRecord {
            description: text(format!(
                "{} {} {} {}",
                rng.word(),
                rng.word(),
                rng.word(),
                rng.word()
            )),
            status,
            sub_state: text(sub_state),
            load_state: text("loaded"),
            startup: [
                StartupType::Enabled,
                StartupType::Static,
                StartupType::Disabled,
                StartupType::Indirect,
            ][rng.below(4)],
            main_pid,
            user: text(match unit.manager {
                Manager::System => "root",
                Manager::User => "user",
            }),
            path: text(path),
            unit_file: Some(text(unit_file)),
            active_since_us: running.then(|| 1_761_800_000_000_000 + rng.next() % 72_000_000_000),
            triggered_by: if socket {
                Arc::from([text(unit.name.replace(".service", ".socket"))])
            } else {
                Arc::from([])
            },
            has_cgroup_procs: control_group.is_some(),
            control_group,
            can_reload: service && rng.below(4) == 0,
            cgroup_pids: Arc::from([]),
            cpu_pct: Total::default(),
            memory_bytes: Total::default(),
            disk_bps: Total::default(),
            tasks: Total::default(),
            kind,
            unit,
        }
    }

    /// A unit with no processes in the table starts or stops, as a signal batch from systemd
    /// reports it.
    fn step_services(&mut self) {
        let i = loop {
            let i = self.rng.below(self.unit_records.len());
            if self.unit_records[i].main_pid == 0 {
                break i;
            }
        };
        let r = &self.unit_records[i];
        let (unit, kind, running) = (r.unit.clone(), r.kind, r.status == ServiceStatus::Stopped);
        self.unit_records[i] = self.unit_record(unit, kind, running, 0);
    }

    /// The services part of a session with no Services page query: no rows and no counts of
    /// its own, every unit record.
    fn services(&self, version: u64) -> Arc<ServicesSnapshot> {
        Arc::new(ServicesSnapshot {
            header: header(version, self.now_ns),
            units: self.unit_records.clone().into(),
            ..ServicesSnapshot::default()
        })
    }

    /// Windows of the app processes.
    fn make_windows(&mut self) -> Vec<WindowEntry> {
        let pids: Vec<u32> = self
            .procs
            .iter()
            .filter(|p| matches!(p.kind, Kind::App(_)))
            .map(|p| p.record.key.pid)
            .take(real::WINDOWS)
            .collect();
        let rng = &mut self.rng;
        let mut out: Vec<WindowEntry> = pids
            .into_iter()
            .enumerate()
            .map(|(i, pid)| {
                let class = rng.word();
                WindowEntry {
                    internal_id: text(format!(
                        "{{{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}}}",
                        rng.next() as u32,
                        rng.below(0x1_0000),
                        rng.below(0x1000),
                        rng.below(0x1_0000),
                        rng.next() >> 16
                    )),
                    pid,
                    caption: text(format!("{} {} - {}", rng.word(), rng.word(), class)),
                    desktop_file_name: text(format!("org.example.{class}")),
                    resource_class: text(class),
                    minimized: rng.below(4) == 0,
                    maximized: rng.below(3) == 0,
                    keep_above: false,
                    frame_geometry: FrameGeometry {
                        x: rng.below(2560) as i32,
                        y: rng.below(1440) as i32,
                        width: 400 + rng.below(2000) as i32,
                        height: 300 + rng.below(1100) as i32,
                    },
                    window_type: 0,
                    stack_index: i as u32,
                }
            })
            .collect();
        out.sort_by_key(|w| w.pid);
        out
    }

    fn windows(&self, version: u64) -> Arc<WindowsSnapshot> {
        Arc::new(WindowsSnapshot {
            header: header(version, self.now_ns),
            windows: self.windows.clone().into(),
            pick: None,
        })
    }

    /// The catalog of a desktop: the CPU package and its 32 logical CPUs, memory, one GPU, 3
    /// disks, 7 network interfaces, 40 temperature sensors, 6 fans, the 3 aggregates and 42
    /// memory groups.
    fn make_catalog(&mut self) -> Vec<DeviceEntry> {
        let mut ids = vec![(DeviceIdentity::CpuPackage, text("CPU"), text("cpu"))];
        for id in 0..32u16 {
            ids.push((
                DeviceIdentity::CpuLogical { id },
                text(format!("CPU {id}")),
                text(format!("cpu:{id}")),
            ));
        }
        ids.push((DeviceIdentity::Memory, text("Memory"), text("memory")));
        ids.push((
            DeviceIdentity::Gpu {
                uuid: [7; 16],
                pci_bus_id: text("0000:03:00.0"),
            },
            text("GPU 0"),
            text("gpu:pci:0000:03:00.0"),
        ));
        for d in 0..3u32 {
            let name = text(format!("nvme{d}n1"));
            ids.push((
                DeviceIdentity::Disk {
                    major: 259,
                    minor: d,
                    name: name.clone(),
                    wwn_or_serial: Some(text(format!("eui.{:016x}", self.rng.next()))),
                },
                text(format!("Disk {d} ({name})")),
                text(format!("disk:{name}")),
            ));
        }
        for n in 0..7u32 {
            let name = text(format!("{}{n}", self.rng.word()));
            ids.push((
                DeviceIdentity::Net {
                    netns_ino: 4_026_531_840,
                    ifindex: 2 + n,
                    name: name.clone(),
                },
                name.clone(),
                text(format!("net:{name}")),
            ));
        }
        for s in 0..40u16 {
            let chip = text(["k10temp", "nvme", "amdgpu", "it8689"][usize::from(s % 4)]);
            let label = text(format!("temp{}", s / 4 + 1));
            ids.push((
                DeviceIdentity::Sensor {
                    chip: chip.clone(),
                    label: label.clone(),
                },
                text(format!("{chip} {label}")),
                text(format!("sensor:hwmon:{chip}-pci-00c3:{label}_input")),
            ));
        }
        for index in 0..6u16 {
            ids.push((
                DeviceIdentity::Fan {
                    chip: text("it8689"),
                    index,
                },
                text(format!("Fan {}", index + 1)),
                text(format!("fan:it8689:{index}")),
            ));
        }
        for (kind, name) in [
            (AggregateKind::Disks, "Disks"),
            (AggregateKind::Nets, "Network"),
            (AggregateKind::Gpus, "GPUs"),
        ] {
            ids.push((
                DeviceIdentity::Aggregate(kind),
                text(name),
                text(format!("aggregate:{name}")),
            ));
        }
        for _ in 0..42 {
            let word = self.rng.word();
            let exe = format!("exe:/usr/lib/{}/{word}", self.rng.word());
            ids.push((
                DeviceIdentity::MemGroup { key: text(&exe) },
                text(&word),
                text(format!("memgroup:{exe}")),
            ));
        }
        assert_eq!(ids.len(), real::DEVICES);
        ids.into_iter()
            .enumerate()
            .map(
                |(i, (identity, display_name, persistent_key))| DeviceEntry {
                    idx: DeviceIdx(i as u16),
                    net_kind: matches!(identity, DeviceIdentity::Net { .. })
                        .then_some(NetKind::Wired),
                    parent: matches!(identity, DeviceIdentity::CpuLogical { .. })
                        .then_some(DeviceIdx(0)),
                    hot_c: matches!(identity, DeviceIdentity::Sensor { .. }).then_some(95.0),
                    identity,
                    generation: 0,
                    display_name,
                    present: true,
                    persistent_key,
                },
            )
            .collect()
    }

    /// A memory group comes or goes.
    fn step_devices(&mut self) {
        let groups = real::DEVICES - 42;
        let i = groups + self.rng.below(42);
        let e = &mut self.catalog[i];
        e.present = !e.present;
        if e.present {
            e.generation += 1;
        }
    }

    fn devices(&self, version: u64) -> Arc<DeviceCatalog> {
        Arc::new(DeviceCatalog {
            version,
            entries: self.catalog.clone().into(),
        })
    }

    fn make_disk_details(&mut self) -> Vec<(DeviceIdx, DiskDetails)> {
        (0..3u16)
            .map(|d| {
                let used = (300 + self.rng.below(900) as u64) << 30;
                (
                    DeviceIdx(36 + d),
                    DiskDetails {
                        kind: text("SSD"),
                        model: known(text(format!("Example NVMe SSD {}", self.rng.word()))),
                        wwn: known(text(format!("eui.{:016x}", self.rng.next()))),
                        serial: known(text(format!("{:020X}", self.rng.next()))),
                        interface: known(text("NVMe")),
                        scheduler: known(text("none")),
                        smart_temp_c: Some(40.0),
                        formatted_bytes: known(2 << 40),
                        used_bytes: known(used),
                        free_bytes: known((2 << 40) - used),
                        system_disk: known(d == 0),
                        swap: known(false),
                        partitions: Some(Arc::new(PartitionTable {
                            kind: text("GPT"),
                            rows: Arc::from([
                                partition(d, 1, 1 << 20, 1 << 30, "/boot", "vfat", "EFI system"),
                                partition(
                                    d,
                                    2,
                                    (1 << 30) + (1 << 20),
                                    (2 << 40) - (2 << 30),
                                    if d == 0 { "/" } else { "/home" },
                                    "btrfs",
                                    "Linux filesystem",
                                ),
                            ]),
                        })),
                        ..DiskDetails::default()
                    },
                )
            })
            .collect()
    }

    /// Free space on the system disk moves.
    fn step_disks(&mut self) {
        let delta = (1 + self.rng.below(64) as u64) << 20;
        let disk = &mut self.disk_details[0].1;
        disk.used_bytes.value += delta;
        disk.free_bytes.value -= delta;
    }

    fn device_details(&mut self, version: u64) -> Arc<DeviceDetailsSnapshot> {
        let rng = &mut self.rng;
        let nets = (0..7u16)
            .map(|n| {
                (
                    DeviceIdx(39 + n),
                    NetDetails {
                        type_label: text("Ethernet"),
                        connection: Some(text(format!("Wired connection {}", n + 1))),
                        adapter_name: known(text("Example 2.5GbE Controller")),
                        driver: known(text("igc")),
                        mac: known(text(format!(
                            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                            rng.below(256),
                            rng.below(256),
                            rng.below(256),
                            rng.below(256),
                            rng.below(256),
                            rng.below(256)
                        ))),
                        ifindex: 2 + u32::from(n),
                        link_speed_mbps: known(2500),
                        state: LinkState::Up,
                        addresses: Arc::from([
                            text(format!("10.0.{n}.{}/24", 2 + rng.below(250))),
                            text(format!(
                                "2001:db8:{n:x}::{:x}:{:x}/64",
                                rng.next() as u16,
                                rng.next() as u16
                            )),
                            text(format!(
                                "fe80::{:x}:{:x}/64",
                                rng.next() as u16,
                                rng.next() as u16
                            )),
                        ]),
                        kind: NetKind::Wired,
                        wifi: None,
                    },
                )
            })
            .collect();
        Arc::new(DeviceDetailsSnapshot {
            header: header(version, self.now_ns),
            cpu: Arc::new(CpuStaticInfo {
                cores: 16,
                logical: 32,
                driver: known(text("amd-pstate-epp")),
                governor: known(text("powersave")),
                epp: known(text("balance_performance")),
                platform_profile: known(text("balanced")),
                prefcore_rank: (0..32u32).map(|c| Some(166 + c % 16 * 5)).collect(),
                prefcore_differs: true,
                microcode: known(text("0x0b404023")),
                ..CpuStaticInfo::default()
            }),
            memory: Arc::new(MemoryDetails {
                zram: Arc::from([ZramDetail {
                    name: text("zram0"),
                    disksize: 16 << 30,
                    algorithm: Some(text("zstd")),
                    stats: None,
                }]),
                zswap: known(None),
                swaps: known(Arc::from([SwapDevice {
                    name: text("/dev/zram0"),
                    kind: SwapKindModel::Zram,
                    size_bytes: 16 << 30,
                    used_bytes: 1 << 30,
                    priority: 100,
                }])),
            }),
            disks: self.disk_details.clone().into(),
            nets,
            batteries: Arc::from([]),
            on_battery: None,
        })
    }

    /// The Information tab values: one scan at the start, never changed.
    fn inventory(&mut self) -> Arc<InventorySnapshot> {
        let rng = &mut self.rng;
        Arc::new(InventorySnapshot {
            header: header(1, self.now_ns),
            os: OsInfo {
                name: known(text("Example Linux")),
                logo_icon: Some(text("example-logo")),
                version: known(text("rolling")),
                build: known(text("rolling")),
                kernel: known(text("Linux")),
                kernel_version: known(text("7.3.0-4-generic #1 SMP PREEMPT_DYNAMIC")),
                architecture: known(text("x86_64")),
                install_date_s: known(1_700_000_000),
                package_manager: known(text("pacman (1532 packages)")),
                machine_id: known(text(format!("{:016x}{:016x}", rng.next(), rng.next()))),
            },
            processor: ProcessorInfo {
                name: known(text("Example 16-Core Processor")),
                vendor: known(text("AuthenticAMD")),
                architecture: known(text("x86_64")),
                family: known(26),
                model: known(68),
                stepping: known(0),
                cores: known(16),
                logical: known(32),
                base_mhz: known(4300),
                max_mhz: known(5750),
                core_max_mhz: known(
                    (0..32)
                        .map(|_| Some(5000 + rng.below(750) as u32))
                        .collect(),
                ),
                microcode: known(text("0x0b404023")),
                virtualization: known(text("AMD-V")),
                avx512: known(true),
                bits: known(64),
            },
            system: SystemInfo {
                pretty_hostname: known(text("desktop")),
                static_hostname: known(text("desktop")),
                user_name: known(text("user")),
                domain: None,
                boot_time_s: known(1_761_800_000),
                desktop: known(text("KDE")),
                windowing_system: known(text("Wayland")),
                session_type: known(text("wayland")),
                session_class: known(text("user")),
                virtual_terminal: known(text("tty2")),
            },
            memory: MemoryInfo {
                total_bytes: known(64 << 30),
                page_size: known(4096),
                huge_page_size: known(2 << 20),
                swap_total_bytes: known(16 << 30),
                zram_bytes: known(16 << 30),
                peak_committed_bytes: known(40 << 30),
            },
            modules: (0..4)
                .map(|s| MemoryModule {
                    slot: known(text(format!("DIMM {s}"))),
                    capacity_bytes: known(16 << 30),
                    speed_mts: known(6000),
                    kind: known(text("DDR5")),
                    form_factor: known(text("DIMM")),
                    manufacturer: known(text("Example")),
                    part_number: known(text(format!("EX{:08X}", rng.next() as u32))),
                    configured_speed_mts: known(6000),
                    rank: known(1),
                    configured_voltage_mv: known(1350),
                })
                .collect(),
            slots_total: known(4),
            disks: self
                .disk_details
                .iter()
                .enumerate()
                .take(real::INVENTORY_DISKS)
                .map(|(d, (idx, disk))| DiskDrive {
                    device: Some(*idx),
                    name: Some(text(format!("nvme{d}n1"))),
                    model: disk.model.clone(),
                    kind: known(text("SSD")),
                    interface: known(text("NVMe")),
                    capacity_bytes: known(2 << 40),
                    free_bytes: disk.free_bytes,
                    volumes: Arc::from([text("/"), text("/home"), text("/boot")]),
                    serial: disk.serial.clone(),
                })
                .collect(),
            firmware: FirmwareInfo {
                firmware_type: known(Some(FirmwareType::Uefi)),
                bios_vendor: known(text("Example Megatrends")),
                bios_version: known(text("3001")),
                bios_date: known(text("2025-08-14")),
                bios_release: known(text("5.35")),
                secure_boot: known(false),
            },
            board: BoardInfo {
                manufacturer: known(text("Example Computer Inc.")),
                product: known(text("EXAMPLE X870E BOARD")),
                version: known(text("Rev 1.xx")),
                system_vendor: known(text("Example Computer Inc.")),
                system_product: known(text("System Product Name")),
                chassis_type: known(text("Desktop")),
                serial: known(text(format!("{:016X}", rng.next()))),
            },
            graphics: Arc::from([GraphicsAdapter {
                device: Some(DeviceIdx(35)),
                pci_slot: Some(text("0000:03:00.0")),
                name: known(text("Example GPU 16 GB")),
                driver_version: known(text("580.95.05")),
                kernel_module: known(text("nvidia")),
                vulkan_version: known(text("1.4.312")),
                opengl_version: known(text("4.6.0")),
                temperature_source: known(text("nvidia-smi")),
                dedicated_bytes: known(16 << 30),
                shared_bytes: known(32 << 30),
                displays: (0..2)
                    .map(|d| DisplayMode {
                        width: 2560,
                        height: 1440,
                        refresh_mhz: 144_000,
                        primary: d == 0,
                        monitor_name: Some(text(format!("Example 27 {d}"))),
                        connector: DisplayConnector::DisplayPort,
                        max_bpc: Some(10),
                        rotation: Some(0),
                        scale: Some(1.0),
                    })
                    .collect(),
            }]),
            network: (0..real::INVENTORY_NETS as u16)
                .map(|n| NetworkAdapter {
                    device: Some(DeviceIdx(39 + n)),
                    connection: known(text(format!("Wired connection {}", n + 1))),
                    adapter_name: known(text("Example 2.5GbE Controller")),
                    driver: known(text("igc")),
                    link_speed_mbps: known(2500),
                    ip_addresses: Arc::from([
                        text(format!("10.0.{n}.{}", 2 + rng.below(250))),
                        text(format!("2001:db8:{n:x}::{:x}", rng.next() as u16)),
                        text(format!(
                            "fe80::{:x}:{:x}",
                            rng.next() as u16,
                            rng.next() as u16
                        )),
                    ]),
                    mac: known(text(format!("{:012x}", rng.next() >> 16))),
                })
                .collect(),
            public_ip: Default::default(),
        })
    }

    /// The snapshot of second `second`. Devices change every second. With `processes_tab` the
    /// process sweep runs every second; without it only the history sweep runs, every 10 s
    /// (`HISTORY_PERIOD` in tl-engine's processes worker, for the memory group series), and
    /// carries 10 s of churn. Each sweep rebuilds the session's table too, as the real engine
    /// does.
    fn snapshot(&mut self, second: u64, processes_tab: bool) -> Snapshot {
        let sweep = second == 0 || processes_tab || second.is_multiple_of(HISTORY_SWEEP_S);
        if second > 0 {
            self.now_ns += SEC;
            self.step_series();
            if processes_tab {
                self.step_processes();
            } else if sweep {
                for _ in 0..HISTORY_SWEEP_S {
                    self.step_processes();
                }
            }
        }
        let version = second + 1;
        // The parts a visible window gets besides the process view, as often as the real
        // engine changes them (`real`): services in 5 of 59 frames, devices in 24, device
        // details in 19, windows and health in every one.
        if second % real::SERVICES_PERIOD_S == real::SERVICES_PERIOD_S / 2 {
            self.step_services();
            self.base.services = self.services(version);
        }
        if matches!(second % 5, 1 | 3) {
            self.step_devices();
            self.base.devices = self.devices(version);
        }
        if second > 0 && second.is_multiple_of(real::DEVICE_DETAILS_PERIOD_S) {
            self.step_disks();
            self.base.device_details = self.device_details(version);
        }
        let mut s = self.base.clone();
        s.sequence = version;
        s.published_ns = self.now_ns;
        s.system = self.system(version);
        s.series = self.series(version);
        s.windows = self.windows(version);
        s.health = Arc::new(HealthSnapshot {
            version,
            ..(*self.base.health).clone()
        });
        if sweep {
            s.processes = self.processes(version);
            s.table = self.table(version);
            // Unchanged parts keep their Arc until the next sweep.
            self.base.processes = s.processes.clone();
            self.base.table = s.table.clone();
        }
        s
    }
}

/// What one session sends over `SECONDS`, and what each part of the delta frames weighs.
struct Wire {
    /// Snapshot frames, with their envelope.
    server: usize,
    /// The client's `Ack` frames.
    acks: usize,
    complete: usize,
    /// Uncompressed postcard bytes per part over the delta frames.
    parts: Vec<(&'static str, usize)>,
    /// Delta frames that carry each part, in the order of `parts`.
    carried: Vec<usize>,
    /// Uncompressed postcard bytes of each part in the complete frame.
    whole: Vec<(&'static str, usize)>,
}

impl Wire {
    fn per_second(&self) -> f64 {
        (self.server + self.acks) as f64 / SECONDS as f64
    }

    fn print(&self, name: &str) {
        println!(
            "{name}: {:.0} B/s ({:.2} KiB/s); {} B snapshot frames, {} B of them the complete frame, {} B acks",
            self.per_second(),
            self.per_second() / 1024.0,
            self.server,
            self.complete,
            self.acks
        );
        let deltas = SECONDS as usize - 1;
        for ((part, n), carried) in self.parts.iter().zip(&self.carried) {
            let whole = self
                .whole
                .iter()
                .find(|(name, _)| name == part)
                .map_or(0, |(_, w)| *w);
            if *n > 0 || whole > 0 {
                println!(
                    "  {part}: {whole} B in the complete frame; in {carried} of {deltas} delta frames, {} B per delta frame, uncompressed",
                    n / deltas
                );
            }
        }
    }
}

/// The uncompressed bytes of each part of `frame`: the frame with only that part, less the
/// frame with none.
fn part_sizes(frame: &SnapshotFrame) -> Vec<(&'static str, usize)> {
    let len = |f: &SnapshotFrame| {
        let mut bytes = Vec::new();
        encode_message(f, &mut bytes).unwrap();
        bytes.len()
    };
    let empty = SnapshotFrame::unchanged(
        frame.frame,
        frame.base,
        frame.epoch,
        frame.boot_id,
        frame.published_ns,
    );
    let none = len(&empty);
    let mut out = Vec::new();
    macro_rules! parts {
        ($($f:ident),*) => {$(
            let mut one = empty.clone();
            one.$f = frame.$f.clone();
            out.push((stringify!($f), len(&one) - none));
        )*};
    }
    parts!(
        system,
        series,
        processes,
        table,
        connections,
        services,
        startup,
        inventory,
        devices,
        health,
        windows,
        details,
        history,
        process_series,
        device_details,
        device_history
    );
    out
}

/// What the engine publishes to a session whose host is not shown in any window: the system
/// part, which holds the title bar value, and the session's empty parts, the same `Arc` on every
/// publish (`ClientSession::snapshot` in tl-engine, leader decision on tasklens-q4x76.15; tested
/// by `a_session_gets_only_the_parts_its_demand_asks_for` in tl-engine's lab_client_session.rs).
fn hidden(shared: &Snapshot, empty: &Snapshot) -> Snapshot {
    Snapshot {
        sequence: shared.sequence,
        epoch: shared.epoch,
        boot_id: shared.boot_id,
        published_ns: shared.published_ns,
        system: shared.system.clone(),
        ..empty.clone()
    }
}

/// One session for `SECONDS`, on the Processes tab while `shown(second)` and hidden otherwise:
/// a frame each second from the newest snapshot, the client applies it and acknowledges it
/// before the next.
fn run_session(shown: impl Fn(u64) -> bool) -> Wire {
    run_session_with(shown, |_, _| {})
}

/// `run_session`, with `check(second, frame)` called on each frame.
fn run_session_with(
    shown: impl Fn(u64) -> bool,
    mut check: impl FnMut(u64, &SnapshotFrame),
) -> Wire {
    let mut host = Host::new(0x7a51_0e25);
    let empty = Snapshot::empty(BOOT, 0);
    let mut encoder = DeltaEncoder::default();
    let mut writer = FrameWriter::new().unwrap();
    let mut wire = Wire {
        server: 0,
        acks: 0,
        complete: 0,
        parts: Vec::new(),
        carried: Vec::new(),
        whole: Vec::new(),
    };
    let mut installed = Snapshot::empty(BOOT, 0);
    let mut installed_frame = 0;
    let mut shared = host.base.clone();
    for second in 0..SECONDS {
        let processes_tab = shown(second);
        shared = host.snapshot(second, processes_tab);
        let newest = if processes_tab {
            shared.clone()
        } else {
            hidden(&shared, &empty)
        };
        let built = encoder.next_frame(&newest, second * SEC).unwrap().unwrap();
        assert!(!built.trimmed);
        let frame = built.frame;
        assert_eq!(
            frame.base == 0,
            second == 0,
            "only the first frame is complete"
        );

        let mut bytes = Vec::new();
        writer
            .encode_server(&ServerMsg::Snapshot(Box::new(frame.clone())), &mut bytes)
            .unwrap();
        wire.server += bytes.len();
        if frame.base == 0 {
            wire.complete = bytes.len();
            wire.whole = part_sizes(&frame);
        } else {
            let sizes = part_sizes(&frame);
            if wire.parts.is_empty() {
                wire.carried = sizes.iter().map(|_| 0).collect();
                wire.parts = sizes.iter().map(|(name, _)| (*name, 0)).collect();
            }
            for ((sum, carried), (_, n)) in wire.parts.iter_mut().zip(&mut wire.carried).zip(sizes)
            {
                sum.1 += n;
                *carried += usize::from(n > 0);
            }
        }

        check(second, &frame);
        installed = apply(&installed, installed_frame, &frame, second + 1).unwrap();
        installed_frame = frame.frame;
        assert!(*installed.processes == *newest.processes);
        assert!(*installed.table == *newest.table);
        assert!(*installed.series == *newest.series);

        let mut ack = Vec::new();
        writer
            .encode_client(&ClientMsg::Ack { frame: frame.frame }, &mut ack)
            .unwrap();
        wire.acks += ack.len();
        encoder.on_ack(frame.frame).unwrap();
    }
    // Starts and exits balance, so the host keeps its size.
    assert_eq!(shared.processes.records.len(), m::PROCESSES);
    wire
}

#[test]
fn processes_tab_session_with_1500_processes_sends_at_most_64_kib_per_second() {
    let wire = run_session(|_| true);
    wire.print("Processes tab session");
    assert!(
        wire.per_second() <= PROCESSES_TAB_MAX,
        "{:.0} B/s is over the budget of {PROCESSES_TAB_MAX} B/s",
        wire.per_second()
    );
}

#[test]
fn hidden_host_sends_at_most_2_kib_per_second() {
    let wire = run_session(|_| false);
    wire.print("hidden host");
    assert!(
        wire.per_second() <= HIDDEN_HOST_MAX,
        "{:.0} B/s is over the budget of {HIDDEN_HOST_MAX} B/s",
        wire.per_second()
    );
}

/// Leader decision on tasklens-q4x76.15: when a session that was hidden shows its Processes
/// surface again, its next frame carries the table whole, a full resync, and the client's table
/// is the server's. `run_session_with` checks after every frame that the client's processes,
/// table and series equal what the session was sent.
#[test]
fn hiding_then_showing_the_processes_tab_resyncs_the_whole_table() {
    const HIDE: u64 = 10;
    const SHOW: u64 = 30;
    let wire = run_session_with(
        |second| !(HIDE..SHOW).contains(&second),
        |second, frame| match second {
            HIDE => assert!(frame.table.is_some(), "hiding sends the empty table"),
            s if s > HIDE && s < SHOW => {
                assert!(frame.table.is_none() && frame.processes.is_none());
                assert!(frame.series.is_none() && frame.services.is_none());
            }
            SHOW => {
                let Some(Part::Whole(table)) = &frame.table else {
                    panic!("the first frame after showing carries the table whole")
                };
                assert!(!table.rows.is_empty());
                assert!(frame.processes.is_some() && frame.series.is_some());
            }
            _ => {}
        },
    );
    wire.print("Processes tab, hidden from 10 s to 30 s");
}
