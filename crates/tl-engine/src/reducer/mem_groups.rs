//! Memory groups for the memory-by-app chart (D-036, UI-PRF-060).
//!
//! On every process sweep the reducer sums RssAnon per memory group (tl-collect
//! `apps/mem_group.rs`) and publishes the top 20 groups, "Other apps" and "Shared and kernel" as
//! `MEM_GROUP` series on `DeviceIdentity::MemGroup` devices. "Shared and kernel" is In use minus
//! the sum over all processes, clamped at 0. It is also published as six parts from
//! /proc/meminfo (`kernel_parts`, UI-PRF-060 "Shared and kernel split", ADR-017). The engine
//! always publishes 20 groups, so the UI's `top_n` setting needs no engine change.
//!
//! The same pass sums `cpu_pct` per memory group for the CPU-by-app chart (D-042, UI-PRF-062)
//! when the sweep has a `/proc/stat` window: no second classify. Kernel threads go to the band
//! "Kernel threads". The CPU of processes that exited in the window, from the `cpu.stat` delta
//! of their cgroup, goes to the group with the most live CPU in that cgroup, else to the group
//! of its oldest live process. Short-lived processes is user + nice + system minus all of that,
//! clamped at 0; when the processes sum to more, the groups, "Other apps" and "Kernel threads"
//! are scaled down by one factor. The CPU top 20 is ranked by CPU, apart from the memory top 20.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use tl_collect::apps::mem_group::{MemGroup, MemGroupInput, MemGroupKind, MemGrouper};
use tl_collect::apps::naming::windows_file_name;
use tl_model::ids::{DeviceIdx, GroupKey, ProcessKey};
use tl_model::process::{ProcFlags, ProcessExtras, ProcessRecord};
use tl_model::system::{MemoryTotals, WineGroupInfo};
use tl_model::validity::{ProcessField, Validity};

use super::table::{GroupClass, group_class};
use crate::update::SweepCpu;

/// Groups published as their own series.
pub const TOP_GROUPS: usize = 20;
/// Key and name of the band that holds every group outside the top 20.
pub const OTHER_KEY: &str = "other";
pub const OTHER_NAME: &str = "Other apps";
/// Key and name of the band for In use minus the sum over all processes.
pub const SHARED_KEY: &str = "shared";
pub const SHARED_NAME: &str = "Shared and kernel";
/// Key and name of the six parts of "Shared and kernel", in stacking order. The parts sum to
/// the "Shared and kernel" band.
pub const KERNEL_PARTS: [(&str, &str); 6] = [
    ("k:shmem", "Shared memory"),
    ("k:zswap", "Compressed swap"),
    ("k:slab", "Kernel slab"),
    ("k:kernel", "Other kernel"),
    ("k:driver", "Driver memory"),
    ("k:filecache", "File cache in use"),
];
/// Prefix of the persistent device key: `memgroup:<group key>`.
pub const DEVICE_KEY_PREFIX: &str = "memgroup:";
/// Key and name of the five system bands of the CPU-by-app chart, in stacking order
/// (UI-PRF-062 "System bands").
pub const CPU_BANDS: [(&str, &str); 5] = [
    ("cpu:kthreads", "Kernel threads"),
    ("cpu:irq", "Hardware interrupts"),
    ("cpu:softirq", "Software interrupts"),
    ("cpu:steal", "Steal"),
    ("cpu:short", "Short-lived processes"),
];
/// Index of "Kernel threads" and "Short-lived processes" in `CPU_BANDS`.
pub const KTHREADS: usize = 0;
pub const SHORT: usize = 4;

/// The RssAnon sum of one memory group in one sweep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupSum {
    pub group: Arc<MemGroup>,
    pub bytes: u64,
    pub processes: u32,
    /// The prefix and Windows image names of a Wine group in the top 20.
    pub wine: Option<Arc<WineGroupInfo>>,
}

/// The CPU of one memory group in one sweep, in percent of all CPUs, with the exited CPU it
/// owns.
#[derive(Clone, Debug, PartialEq)]
pub struct CpuGroupSum {
    pub group: Arc<MemGroup>,
    pub pct: f64,
    /// Live processes of the group, kernel threads left out.
    pub processes: u32,
    pub wine: Option<Arc<WineGroupInfo>>,
}

/// The CPU bands of one sweep with a window, each in percent of all CPUs. They sum to the
/// window's busy share.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuBands {
    /// At most `TOP_GROUPS`, largest first. Equal values are ordered by key.
    pub top: Vec<CpuGroupSum>,
    /// Every group outside `top`.
    pub other_pct: f64,
    pub other_processes: u32,
    /// The five system bands in `CPU_BANDS` order.
    pub system: [f64; 5],
    /// Live kernel threads, the process count of "Kernel threads".
    pub kernel_threads: u32,
    /// The processes and the exited CPU summed to more than user + nice + system, so the groups,
    /// "Other apps" and "Kernel threads" were scaled down and Short-lived processes is 0.
    pub scaled: bool,
}

/// The bands of one sweep.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bands {
    /// At most `TOP_GROUPS`, largest first. Equal sums are ordered by key.
    pub top: Vec<GroupSum>,
    /// Every group outside `top`.
    pub other_bytes: u64,
    pub other_processes: u32,
    /// In use minus the sum over all processes, clamped at 0. `None` when In use is not known.
    /// When the processes sum to more than In use, the group bands are scaled down so the
    /// bands still sum to In use.
    pub shared_bytes: Option<u64>,
    /// The CPU bands; `None` when the sweep has no window.
    pub cpu: Option<CpuBands>,
}

/// The grouping state kept across sweeps, and the band devices the reducer has added.
pub struct MemGroups {
    grouper: MemGrouper,
    out: Vec<Arc<MemGroup>>,
    /// The memory group key of each live process in the last sweep. The process table's
    /// memory group filter reads it, so the UI does not repeat the grouping rules.
    pub keys: FxHashMap<ProcessKey, Arc<str>>,
    /// `getuid()`. Tests set it.
    pub self_uid: u32,
    /// Device and display name per band key, so a sweep in steady state does not touch the
    /// catalog.
    pub devices: FxHashMap<Arc<str>, (DeviceIdx, Arc<str>)>,
    /// The top-group devices that got a sample in the last sweep. One that drops out gets a gap.
    pub shown: Vec<DeviceIdx>,
    /// The same for the CPU top 20 (`CPU_GROUP`), with each group's key. A sweep without a
    /// window gaps these and keeps them, so the next one gaps them again.
    pub cpu_shown: Vec<(Arc<str>, DeviceIdx)>,
    pub other_key: Arc<str>,
    pub other_name: Arc<str>,
    pub shared_key: Arc<str>,
    pub shared_name: Arc<str>,
    /// Key and name of each of the six parts, as `KERNEL_PARTS`.
    pub kernel_keys: [(Arc<str>, Arc<str>); 6],
    /// Key and name of each of the five CPU system bands, as `CPU_BANDS`.
    pub cpu_keys: [(Arc<str>, Arc<str>); 5],
}

impl Default for MemGroups {
    fn default() -> Self {
        Self::new(rustix::process::getuid().as_raw())
    }
}

impl MemGroups {
    pub fn new(self_uid: u32) -> Self {
        MemGroups {
            grouper: MemGrouper::new(),
            out: Vec::new(),
            keys: FxHashMap::default(),
            self_uid,
            devices: FxHashMap::default(),
            shown: Vec::new(),
            cpu_shown: Vec::new(),
            other_key: Arc::from(OTHER_KEY),
            other_name: Arc::from(OTHER_NAME),
            shared_key: Arc::from(SHARED_KEY),
            shared_name: Arc::from(SHARED_NAME),
            kernel_keys: KERNEL_PARTS.map(|(k, n)| (Arc::from(k), Arc::from(n))),
            cpu_keys: CPU_BANDS.map(|(k, n)| (Arc::from(k), Arc::from(n))),
        }
    }

    /// Sums RssAnon per memory group over the live `records`. `extras` is index aligned with
    /// `records`, or empty. `in_use` is `MemTotal - MemAvailable` when known. Exited records
    /// are left out; a record whose RssAnon cannot be read counts as a process with 0 bytes.
    /// Also refills `keys` with the group key of each live record.
    pub fn bands(
        &mut self,
        records: &[ProcessRecord],
        extras: &[ProcessExtras],
        in_use: Option<u64>,
    ) -> Bands {
        self.bands_with_cpu(records, extras, in_use, None)
    }

    /// `bands`, plus the CPU bands when `cpu` holds the sweep's window (see the module doc).
    pub fn bands_with_cpu(
        &mut self,
        records: &[ProcessRecord],
        extras: &[ProcessExtras],
        in_use: Option<u64>,
        cpu: Option<&SweepCpu>,
    ) -> Bands {
        let live: Vec<usize> = (0..records.len())
            .filter(|&i| !records[i].flags.contains(ProcFlags::EXITED))
            .collect();

        // One app id and name per app group: those of its oldest record with an app id, as
        // the process table names its app rows. Records are sorted by key, oldest first.
        let mut apps: FxHashMap<GroupKey, (&str, &str)> = FxHashMap::default();
        for &i in &live {
            let r = &records[i];
            if group_class(r) == GroupClass::Apps
                && let Some(app_id) = r.meta.app_id.as_deref()
            {
                apps.entry(r.group).or_insert((app_id, &r.meta.name));
            }
        }

        let aligned = extras.len() == records.len();
        let inputs: Vec<MemGroupInput<'_>> = live
            .iter()
            .map(|&i| {
                let r = &records[i];
                let app = apps.get(&r.group).copied();
                MemGroupInput {
                    pid: r.key.pid,
                    start_ticks: r.key.start_ticks,
                    ruid: r.uid,
                    kernel_thread: r.flags.contains(ProcFlags::KERNEL_THREAD),
                    comm: &r.meta.comm,
                    exe: r.meta.exe.as_deref(),
                    app_id: app.map(|a| a.0),
                    app_name: app.map(|a| a.1),
                    unit: aligned
                        .then(|| extras[i].unit.as_ref())
                        .flatten()
                        .map(|u| &*u.name),
                    wine: aligned.then(|| extras[i].wine.as_deref()).flatten(),
                }
            })
            .collect();
        self.grouper.classify(&inputs, self.self_uid, &mut self.out);

        let mut sums: Vec<GroupSum> = Vec::new();
        let mut index: FxHashMap<&str, usize> = FxHashMap::default();
        // The Windows image names per Wine group, by index in `sums`.
        let mut images: FxHashMap<usize, Vec<&str>> = FxHashMap::default();
        let mut total = 0u64;
        // The CPU of each group, by index in `sums`, and the slot of each live record.
        let mut cpu_acc = CpuAcc::default();
        self.keys.clear();
        for (&i, group) in live.iter().zip(&self.out) {
            self.keys.insert(records[i].key, Arc::clone(&group.key));
            let bytes = rss_anon(&records[i]);
            total = total.saturating_add(bytes);
            let slot = *index.entry(&group.key).or_insert_with(|| {
                sums.push(GroupSum {
                    group: Arc::clone(group),
                    bytes: 0,
                    processes: 0,
                    wine: None,
                });
                sums.len() - 1
            });
            if cpu.is_some() {
                cpu_acc.add(&records[i], slot);
            }
            let sum = &mut sums[slot];
            sum.bytes = sum.bytes.saturating_add(bytes);
            sum.processes += 1;
            if group.kind == MemGroupKind::Wine {
                let image = aligned
                    .then(|| extras[i].wine.as_ref()?.image.as_deref())
                    .flatten()
                    .map_or(&*records[i].meta.comm, windows_file_name);
                let names = images.entry(slot).or_default();
                if !names.contains(&image) {
                    names.push(image);
                }
            }
        }
        drop(index);
        for (slot, mut names) in images {
            names.sort_unstable();
            let sum = &mut sums[slot];
            sum.wine = Some(Arc::new(WineGroupInfo {
                prefix: sum.group.wine_prefix.clone(),
                images: names.into_iter().map(Arc::from).collect(),
            }));
        }

        let cpu = cpu.map(|c| cpu_acc.finish(c, records, &live, &sums));

        sums.sort_unstable_by(|a, b| {
            b.bytes
                .cmp(&a.bytes)
                .then_with(|| a.group.key.cmp(&b.group.key))
        });
        let rest = sums.split_off(sums.len().min(TOP_GROUPS));
        let mut bands = Bands {
            top: sums,
            other_bytes: rest.iter().fold(0u64, |a, g| a.saturating_add(g.bytes)),
            other_processes: rest.iter().map(|g| g.processes).sum(),
            shared_bytes: in_use.map(|used| used.saturating_sub(total)),
            cpu,
        };
        if let Some(used) = in_use
            && total > used
        {
            bands.fit(total, used);
        }
        bands
    }
}

impl Bands {
    /// Scales the group bands so they sum to `used` when the processes sum to more (`total`).
    /// The stack then still tops out at In use. The order stays largest first: the rounding
    /// remainder, less than one byte per band, goes to the largest band.
    fn fit(&mut self, total: u64, used: u64) {
        let scale = |bytes: u64| (u128::from(bytes) * u128::from(used) / u128::from(total)) as u64;
        let mut sum = 0u64;
        for g in &mut self.top {
            g.bytes = scale(g.bytes);
            sum += g.bytes;
        }
        self.other_bytes = scale(self.other_bytes);
        sum += self.other_bytes;
        let rest = used - sum;
        match self.top.first_mut() {
            Some(g) => g.bytes += rest,
            None => self.other_bytes += rest,
        }
    }
}

/// The CPU sums of one sweep while `MemGroups::bands_with_cpu` walks the live records.
#[derive(Default)]
struct CpuAcc {
    /// Live process CPU per group slot, kernel threads left out.
    pct: Vec<f64>,
    /// Live processes per group slot, kernel threads left out.
    processes: Vec<u32>,
    /// The group slot of each live record, in `live` order.
    slots: Vec<usize>,
    kthreads_pct: f64,
    kthreads: u32,
}

/// One cgroup with a `cpu.stat` delta, while its exited CPU is found an owner.
struct CgroupAcc {
    delta: f64,
    /// Σ cpu_pct of the cgroup's live processes.
    live: f64,
    /// Live CPU per group slot in this cgroup, kernel threads left out.
    groups: Vec<(usize, f64)>,
    /// The group slot of the cgroup's oldest live process that is not a kernel thread.
    oldest: Option<usize>,
}

impl CpuAcc {
    fn add(&mut self, r: &ProcessRecord, slot: usize) {
        if self.pct.len() <= slot {
            self.pct.resize(slot + 1, 0.0);
            self.processes.resize(slot + 1, 0);
        }
        self.slots.push(slot);
        let pct = cpu_pct(r);
        if r.flags.contains(ProcFlags::KERNEL_THREAD) {
            self.kthreads_pct += pct;
            self.kthreads += 1;
        } else {
            self.pct[slot] += pct;
            self.processes[slot] += 1;
        }
    }

    /// The CPU bands over `c`'s window. `live` and `sums` are those of `bands_with_cpu`, before
    /// `sums` is sorted.
    fn finish(
        mut self,
        c: &SweepCpu,
        records: &[ProcessRecord],
        live: &[usize],
        sums: &[GroupSum],
    ) -> CpuBands {
        self.pct.resize(sums.len(), 0.0);
        self.processes.resize(sums.len(), 0);
        self.attribute_exited(c, records, live);

        let w = &c.window;
        let user_nice_system = w.user_nice_system_pct.max(0.0);
        let procs = self.pct.iter().sum::<f64>() + self.kthreads_pct;
        let (factor, short, scaled) = if procs > user_nice_system {
            (user_nice_system / procs, 0.0, true)
        } else {
            (1.0, user_nice_system - procs, false)
        };

        let mut groups: Vec<CpuGroupSum> = sums
            .iter()
            .enumerate()
            .filter(|&(slot, _)| self.processes[slot] > 0)
            .map(|(slot, g)| CpuGroupSum {
                group: Arc::clone(&g.group),
                pct: self.pct[slot] * factor,
                processes: self.processes[slot],
                wine: g.wine.clone(),
            })
            .collect();
        groups.sort_unstable_by(|a, b| {
            b.pct
                .total_cmp(&a.pct)
                .then_with(|| a.group.key.cmp(&b.group.key))
        });
        let rest = groups.split_off(groups.len().min(TOP_GROUPS));
        let mut system = [0.0; 5];
        system[KTHREADS] = self.kthreads_pct * factor;
        system[1] = w.irq_pct.max(0.0);
        system[2] = w.softirq_pct.max(0.0);
        system[3] = w.steal_pct.max(0.0);
        system[SHORT] = short;
        CpuBands {
            top: groups,
            other_pct: rest.iter().map(|g| g.pct).sum(),
            other_processes: rest.iter().map(|g| g.processes).sum(),
            system,
            kernel_threads: self.kthreads,
            scaled,
        }
    }

    /// Per cgroup with a delta: `max(0, delta - Σ live cpu_pct)` goes to the group with the most
    /// live CPU in the cgroup, else to the group of its oldest live process. A kernel thread is
    /// never an owner; a cgroup with no owner keeps its exited CPU in Short-lived processes.
    fn attribute_exited(&mut self, c: &SweepCpu, records: &[ProcessRecord], live: &[usize]) {
        if c.cgroups.is_empty() {
            return;
        }
        let mut cgroups: FxHashMap<&str, CgroupAcc> = FxHashMap::default();
        // Records are sorted by key, start time first, so the first record of a cgroup is its
        // oldest.
        for (&i, &slot) in live.iter().zip(&self.slots) {
            let r = &records[i];
            let Some(path) = r.meta.cgroup.as_deref() else {
                continue;
            };
            let Some(&delta) = c.cgroups.get(path) else {
                continue;
            };
            let acc = cgroups.entry(path).or_insert_with(|| CgroupAcc {
                delta,
                live: 0.0,
                groups: Vec::new(),
                oldest: None,
            });
            let pct = cpu_pct(r);
            acc.live += pct;
            if r.flags.contains(ProcFlags::KERNEL_THREAD) {
                continue;
            }
            acc.oldest.get_or_insert(slot);
            match acc.groups.iter_mut().find(|(s, _)| *s == slot) {
                Some((_, sum)) => *sum += pct,
                None => acc.groups.push((slot, pct)),
            }
        }
        for acc in cgroups.into_values() {
            let exited = acc.delta - acc.live;
            if exited.is_nan() || exited <= 0.0 {
                continue;
            }
            // The most live CPU; on a tie the first group seen, which holds the older process.
            let busiest = acc
                .groups
                .iter()
                .filter(|(_, pct)| *pct > 0.0)
                .fold(None::<(usize, f64)>, |best, &(slot, pct)| match best {
                    Some((_, b)) if b >= pct => best,
                    _ => Some((slot, pct)),
                })
                .map(|(slot, _)| slot);
            if let Some(owner) = busiest.or(acc.oldest) {
                self.pct[owner] += exited;
            }
        }
    }
}

/// `cpu_pct` of a record, 0 when it has no rate.
fn cpu_pct(r: &ProcessRecord) -> f64 {
    match r.validity.get(ProcessField::Cpu) {
        Validity::Valid | Validity::Stale if r.cpu_pct.is_finite() && r.cpu_pct > 0.0 => {
            f64::from(r.cpu_pct)
        }
        _ => 0.0,
    }
}

/// The six parts of "Shared and kernel" in bytes, in `KERNEL_PARTS` order, from one meminfo
/// read. They sum to `shared` exactly:
///
/// 1. Shmem.
/// 2. Zswap.
/// 3. SUnreclaim.
/// 4. Page tables, secondary page tables, kernel stacks, per-CPU and vmalloc.
/// 5. Driver memory: MemTotal minus everything meminfo accounts for.
/// 6. File cache in use, the remainder.
///
/// When parts 1 to 5 add up to more than `shared`, part 6 is 0 and the excess comes off part 5,
/// then 4, 3, 2 and 1. A field the kernel does not print counts as 0.
pub fn kernel_parts(shared: u64, m: &MemoryTotals) -> [u64; 6] {
    let k = &*m.kernel;
    let p4 = u128::from(k.page_tables)
        + u128::from(k.sec_page_tables)
        + u128::from(k.kernel_stack)
        + u128::from(k.percpu)
        + u128::from(k.vmalloc_used);
    let accounted = u128::from(m.free)
        + u128::from(k.buffers)
        + u128::from(m.cached)
        + u128::from(k.anon_pages)
        + u128::from(m.slab_reclaimable)
        + u128::from(m.slab_unreclaimable)
        + p4
        + u128::from(m.zswap);
    let mut parts = [
        u128::from(k.shmem),
        u128::from(m.zswap),
        u128::from(m.slab_unreclaimable),
        p4,
        u128::from(m.total).saturating_sub(accounted),
        0,
    ];
    let shared = u128::from(shared);
    let sum: u128 = parts.iter().sum();
    if sum <= shared {
        parts[5] = shared - sum;
    } else {
        let mut excess = sum - shared;
        for part in parts[..5].iter_mut().rev() {
            let cut = excess.min(*part);
            *part -= cut;
            excess -= cut;
        }
    }
    // Each part is at most `shared`, which came from a u64.
    parts.map(|p| p as u64)
}

/// RssAnon of a record, 0 when it was not read.
fn rss_anon(r: &ProcessRecord) -> u64 {
    match r.validity.get(ProcessField::RssAnon) {
        Validity::Valid | Validity::Stale => r.rss_anon_bytes,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use tl_collect::parse::stat::CpuTimes;
    use tl_collect::proc::sweep::CpuWindow;
    use tl_model::actions::SchedPolicy;
    use tl_model::ids::ProcessKey;
    use tl_model::process::{GroupKind, ProcState, ProcessMeta, WineProcess};
    use tl_model::system::{CpuSet, KernelMemory};
    use tl_model::table::{Manager, UnitRef};
    use tl_model::time::BootId;
    use tl_model::validity::FieldValidity;

    use super::*;

    const UID: u32 = 1000;
    const MIB: u64 = 1 << 20;

    struct Proc {
        pid: u32,
        uid: u32,
        comm: &'static str,
        exe: Option<&'static str>,
        app: Option<(&'static str, &'static str)>,
        rss: u64,
        flags: ProcFlags,
    }

    fn proc(pid: u32, exe: &'static str, rss: u64) -> Proc {
        Proc {
            pid,
            uid: UID,
            comm: exe.rsplit('/').next().unwrap_or(exe),
            exe: Some(exe),
            app: None,
            rss,
            flags: ProcFlags::empty(),
        }
    }

    fn record(p: &Proc) -> ProcessRecord {
        let comm: Arc<str> = Arc::from(p.comm);
        let group = match p.app {
            Some((id, _)) => GroupKind::App {
                app_id: Arc::from(id),
            },
            None => GroupKind::Background,
        };
        let mut validity = FieldValidity::default();
        validity.set(ProcessField::RssAnon, Validity::Valid);
        ProcessRecord {
            key: ProcessKey {
                start_ticks: u64::from(p.pid) * 10,
                boot_id: BootId(1),
                pid: p.pid,
            },
            ppid: 1,
            uid: p.uid,
            meta: Arc::new(ProcessMeta {
                comm: comm.clone(),
                name: Arc::from(p.app.map_or(&*comm, |a| a.1)),
                exe: p.exe.map(Arc::from),
                cmdline: None,
                user: Arc::from("alice"),
                cgroup: None,
                app_id: p.app.map(|a| Arc::from(a.0)),
                icon: None,
                description: None,
                desktop_path: None,
                search_lower: comm,
            }),
            group: group.key(),
            state: ProcState::Sleeping,
            flags: p.flags,
            nice: 0,
            threads: 1,
            cpu_pct: 0.0,
            rss_anon_bytes: p.rss,
            rss_bytes: p.rss,
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
            validity,
        }
    }

    fn extras(p: &Proc) -> ProcessExtras {
        let exe = p.exe.unwrap_or("");
        ProcessExtras {
            cpu_kernel_pct: 0.0,
            cpu_user_pct: 0.0,
            cpu_relative_pct: 0.0,
            utime_ticks: 0,
            stime_ticks: 0,
            start_boot_ns: 0,
            rss_peak_bytes: 0,
            rss_shared_bytes: 0,
            commit_bytes: 0,
            vsize_bytes: 0,
            vsize_peak_bytes: 0,
            page_faults: 0,
            page_faults_delta: 0,
            major_faults: 0,
            policy: SchedPolicy::Other,
            rt_prio: 0,
            affinity: CpuSet::empty(),
            handles: None,
            exe_name: Arc::from(p.comm),
            exe_path: Arc::from(exe),
            exe_deleted: false,
            unit: p.app.map(|(id, _)| UnitRef {
                manager: Manager::User,
                name: Arc::from(format!("app-{id}-1.scope")),
            }),
            state_letter: b'S',
            frozen: false,
            disk_read_bps: 0.0,
            disk_write_raw_bps: 0.0,
            flag_since_ns: 0,
            pgrp: 0,
            sid: 0,
            last_cpu: 0,
            wine: None,
        }
    }

    fn build(procs: &[Proc]) -> (Vec<ProcessRecord>, Vec<ProcessExtras>) {
        let mut records: Vec<ProcessRecord> = procs.iter().map(record).collect();
        let mut extras: Vec<ProcessExtras> = procs.iter().map(extras).collect();
        let mut order: Vec<usize> = (0..records.len()).collect();
        order.sort_by_key(|&i| records[i].key);
        records = order.iter().map(|&i| records[i].clone()).collect();
        extras = order.iter().map(|&i| extras[i].clone()).collect();
        (records, extras)
    }

    fn band_sum(b: &Bands) -> u64 {
        b.top.iter().map(|g| g.bytes).sum::<u64>() + b.other_bytes + b.shared_bytes.unwrap_or(0)
    }

    /// A small deterministic generator, so the test needs no extra crate.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, n: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % n
        }
    }

    const EXES: [&str; 30] = [
        "/usr/bin/rustc",
        "/usr/bin/node",
        "/usr/bin/bash",
        "/usr/bin/fish",
        "/usr/bin/python3",
        "/usr/bin/cc1",
        "/usr/bin/ld",
        "/usr/bin/git",
        "/usr/bin/ssh",
        "/usr/bin/vim",
        "/usr/bin/kwin_wayland",
        "/usr/bin/plasmashell",
        "/usr/bin/pipewire",
        "/usr/bin/wireplumber",
        "/usr/bin/dbus-broker",
        "/usr/lib/firefox/firefox",
        "/usr/bin/konsole",
        "/usr/bin/dolphin",
        "/usr/bin/kded6",
        "/usr/bin/baloo_file",
        "/usr/bin/cargo",
        "/usr/bin/clippy-driver",
        "/usr/bin/rust-analyzer",
        "/usr/bin/heavy-build",
        "/usr/bin/sleep",
        "/usr/bin/top",
        "/usr/bin/htop",
        "/usr/bin/btop",
        "/home/u/.local/share/claude/versions/2.1.309",
        "/opt/google/chrome/chrome",
    ];

    #[test]
    fn memory_groups_sum_to_in_use() {
        let mut groups = MemGroups::new(UID);
        let mut rng = Lcg(7);
        for sample in 0..200 {
            let n = 1 + rng.next(300) as u32;
            let procs: Vec<Proc> = (0..n)
                .map(|i| {
                    let mut p = proc(
                        i + 1,
                        EXES[rng.next(EXES.len() as u64) as usize],
                        rng.next(512) * MIB,
                    );
                    match rng.next(10) {
                        0 => p.uid = 0,
                        1 => p.exe = None,
                        2 => p.app = Some(("google-chrome", "Google Chrome")),
                        3 => p.flags = ProcFlags::EXITED,
                        _ => {}
                    }
                    p
                })
                .collect();
            let (records, extras) = build(&procs);
            let live_sum: u64 = procs
                .iter()
                .filter(|p| !p.flags.contains(ProcFlags::EXITED))
                .map(|p| p.rss)
                .sum();
            // Mostly In use above the process sum, sometimes below it.
            let in_use = if rng.next(5) == 0 {
                live_sum / 2
            } else {
                live_sum + rng.next(8192) * MIB
            };
            let b = groups.bands(&records, &extras, Some(in_use));

            assert!(b.top.len() <= TOP_GROUPS, "sample {sample}");
            assert!(
                b.top.windows(2).all(|w| w[0].bytes >= w[1].bytes),
                "sample {sample}: largest first"
            );
            let shared = b.shared_bytes.expect("In use is known");
            assert_eq!(band_sum(&b), in_use, "sample {sample}");
            // The six parts of Shared and kernel add up to the band, so the groups, Other apps
            // and the six add up to In use.
            let m = random_totals(&mut rng);
            let parts: u64 = kernel_parts(b.shared_bytes.expect("In use is known"), &m)
                .iter()
                .sum();
            let top_other = b.top.iter().map(|g| g.bytes).sum::<u64>() + b.other_bytes;
            assert_eq!(top_other + parts, in_use, "sample {sample}: six parts");
            if live_sum <= in_use {
                assert_eq!(shared, in_use - live_sum, "sample {sample}");
            } else {
                // Shared and kernel is clamped at 0, never negative.
                assert_eq!(shared, 0, "sample {sample}");
            }
            let live = procs
                .iter()
                .filter(|p| !p.flags.contains(ProcFlags::EXITED))
                .count() as u32;
            let counted: u32 = b.top.iter().map(|g| g.processes).sum::<u32>() + b.other_processes;
            assert_eq!(counted, live, "sample {sample}: every live process once");
        }
    }

    /// `MemoryTotals` from a meminfo fixture, mapped as the memory worker does.
    fn totals(meminfo: &[u8]) -> MemoryTotals {
        use tl_collect::parse::meminfo::{MemInfo, parse_meminfo};
        let mut info = MemInfo::default();
        parse_meminfo(meminfo, &mut info).expect("fixture parses");
        MemoryTotals {
            total: info.mem_total,
            available: info.mem_available,
            used: info.mem_total.saturating_sub(info.mem_available),
            free: info.mem_free,
            cached: info.cached,
            slab_reclaimable: info.sreclaimable,
            slab_unreclaimable: info.sunreclaim,
            zswap: info.zswap.unwrap_or(0),
            kernel: Arc::new(KernelMemory {
                shmem: info.shmem,
                buffers: info.buffers,
                anon_pages: info.anon_pages.unwrap_or(0),
                page_tables: info.page_tables.unwrap_or(0),
                sec_page_tables: info.sec_page_tables.unwrap_or(0),
                kernel_stack: info.kernel_stack.unwrap_or(0),
                percpu: info.percpu.unwrap_or(0),
                vmalloc_used: info.vmalloc_used.unwrap_or(0),
            }),
            ..MemoryTotals::default()
        }
    }

    fn random_totals(rng: &mut Lcg) -> MemoryTotals {
        let mut mib = |n: u64| rng.next(n) * MIB;
        MemoryTotals {
            total: 1024 * MIB + mib(65536),
            free: mib(4096),
            cached: mib(8192),
            slab_reclaimable: mib(1024),
            slab_unreclaimable: mib(1024),
            zswap: mib(512),
            kernel: Arc::new(KernelMemory {
                shmem: mib(2048),
                buffers: mib(256),
                anon_pages: mib(16384),
                page_tables: mib(512),
                sec_page_tables: mib(64),
                kernel_stack: mib(128),
                percpu: mib(64),
                vmalloc_used: mib(1024),
            }),
            ..MemoryTotals::default()
        }
    }

    fn check_parts(label: &str, shared: u64, m: &MemoryTotals) {
        let parts = kernel_parts(shared, m);
        let sum: u128 = parts.iter().map(|&p| u128::from(p)).sum();
        assert_eq!(sum, u128::from(shared), "{label}: parts sum to shared");
        if shared == 0 {
            assert_eq!(parts, [0; 6], "{label}");
        }
    }

    #[test]
    fn kernel_parts_sum_to_shared() {
        let fixtures: [(&str, &[u8]); 4] = [
            (
                "owner-desktop",
                include_bytes!("../../../tl-collect/tests/fixtures/owner-desktop/proc/meminfo"),
            ),
            (
                "minimal-vm",
                include_bytes!("../../../tl-collect/tests/fixtures/minimal-vm/proc/meminfo"),
            ),
            (
                "meminfo-minimal",
                include_bytes!("../../../tl-collect/tests/fixtures/meminfo-minimal/proc/meminfo"),
            ),
            (
                "mem-composition",
                include_bytes!("../../../tl-collect/tests/fixtures/mem-composition/proc/meminfo"),
            ),
        ];
        for (name, buf) in fixtures {
            let m = totals(buf);
            for shared in [0, MIB, m.used / 2, m.used] {
                check_parts(&format!("{name} shared {shared}"), shared, &m);
            }
        }
        let mut rng = Lcg(11);
        for i in 0..500 {
            let m = random_totals(&mut rng);
            // Often below the sum of parts 1 to 5, so the shrink order runs.
            let shared = rng.next(20_000) * MIB;
            check_parts(&format!("random {i}"), shared, &m);
        }
    }

    #[test]
    fn kernel_parts_shrink_order() {
        let m = MemoryTotals {
            total: 100 * MIB,
            zswap: 5 * MIB,
            slab_unreclaimable: 10 * MIB,
            kernel: Arc::new(KernelMemory {
                shmem: 20 * MIB,
                page_tables: 30 * MIB,
                ..KernelMemory::default()
            }),
            ..MemoryTotals::default()
        };
        // Part 5 is 100 - (10 + 30 + 5) = 55. Parts 1 to 5 sum to 120.
        assert_eq!(
            kernel_parts(200 * MIB, &m).map(|p| p / MIB),
            [20, 5, 10, 30, 55, 80]
        );
        assert_eq!(
            kernel_parts(100 * MIB, &m).map(|p| p / MIB),
            [20, 5, 10, 30, 35, 0]
        );
        // Driver memory goes first, then other kernel.
        assert_eq!(
            kernel_parts(50 * MIB, &m).map(|p| p / MIB),
            [20, 5, 10, 15, 0, 0]
        );
        assert_eq!(
            kernel_parts(10 * MIB, &m).map(|p| p / MIB),
            [10, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn top_20_then_other() {
        let mut groups = MemGroups::new(UID);
        // 25 programs; program i has i + 1 processes of 1 MiB.
        let mut procs = Vec::new();
        let mut pid = 1;
        for (i, exe) in EXES[..25].iter().enumerate() {
            for _ in 0..=i {
                procs.push(proc(pid, exe, MIB));
                pid += 1;
            }
        }
        let (records, extras) = build(&procs);
        let b = groups.bands(&records, &extras, None);
        assert_eq!(b.top.len(), 20);
        assert_eq!(&*b.top[0].group.key, "exe:sleep");
        assert_eq!(b.top[0].bytes, 25 * MIB);
        assert_eq!(b.top[0].processes, 25);
        assert_eq!(b.top[19].bytes, 6 * MIB);
        // The five smallest programs hold 1 to 5 processes.
        assert_eq!(b.other_processes, 15);
        assert_eq!(b.other_bytes, 15 * MIB);
        assert_eq!(b.shared_bytes, None);
    }

    #[test]
    fn app_and_program_groups() {
        let mut groups = MemGroups::new(UID);
        let chrome = Some(("google-chrome", "Google Chrome"));
        let procs = [
            Proc {
                app: chrome,
                ..proc(10, "/opt/google/chrome/chrome", 100 * MIB)
            },
            Proc {
                app: chrome,
                ..proc(11, "/opt/google/chrome/chrome_crashpad_handler", 5 * MIB)
            },
            // A program started in the app's scope from elsewhere groups by program.
            Proc {
                app: chrome,
                ..proc(12, "/usr/bin/bash", 3 * MIB)
            },
            proc(20, "/home/u/.local/share/claude/versions/2.1.309", 40 * MIB),
            proc(21, "/home/u/.local/share/claude/versions/2.1.310", 30 * MIB),
            Proc {
                uid: 0,
                ..proc(30, "/usr/lib/systemd/systemd", 7 * MIB)
            },
            Proc {
                exe: None,
                ..proc(40, "/usr/bin/hidden", 2 * MIB)
            },
        ];
        let (records, extras) = build(&procs);
        let b = groups.bands(&records, &extras, Some(500 * MIB));
        let got: Vec<(&str, &str, u64, u32)> = b
            .top
            .iter()
            .map(|g| (&*g.group.key, &*g.group.name, g.bytes / MIB, g.processes))
            .collect();
        assert_eq!(
            got,
            [
                ("app:google-chrome", "Google Chrome", 105, 2),
                ("exe:claude", "Claude Code", 70, 2),
                ("system", "System", 7, 1),
                ("exe:bash", "bash", 3, 1),
                ("comm:hidden", "hidden", 2, 1),
            ]
        );
        assert_eq!(b.shared_bytes, Some(500 * MIB - 187 * MIB));
    }

    #[test]
    fn wine_group_lists_images_and_prefix() {
        let mut groups = MemGroups::new(UID);
        let preloader = "/usr/lib/wine/x86_64-unix/wine-preloader";
        let images = [
            "C:\\Program Files\\Backblaze\\bztransmit.exe",
            "C:\\Program Files\\Backblaze\\bzbui.exe",
            "C:\\windows\\system32\\services.exe",
            "C:\\windows\\system32\\conhost.exe",
            "C:\\windows\\system32\\conhost.exe",
        ];
        let procs: Vec<Proc> = (0..images.len() as u32)
            .map(|i| Proc {
                comm: "x.exe",
                ..proc(100 + i, preloader, (10 + u64::from(i)) * MIB)
            })
            .collect();
        let (records, mut extras) = build(&procs);
        for (e, image) in extras.iter_mut().zip(images) {
            e.wine = Some(Arc::new(WineProcess {
                prefix: Some(Arc::from("/srv/backblaze/prefix")),
                image: Some(Arc::from(image)),
            }));
        }
        let b = groups.bands(&records, &extras, Some(500 * MIB));
        assert_eq!(b.top.len(), 1);
        let g = &b.top[0];
        assert_eq!(&*g.group.key, "wine:/srv/backblaze/prefix:backblaze");
        assert_eq!(&*g.group.name, "Backblaze");
        assert_eq!(g.processes, 5);
        let wine = g.wine.as_deref().expect("Wine info");
        assert_eq!(wine.prefix.as_deref(), Some("/srv/backblaze/prefix"));
        let names: Vec<&str> = wine.images.iter().map(|n| &**n).collect();
        assert_eq!(
            names,
            ["bzbui.exe", "bztransmit.exe", "conhost.exe", "services.exe"]
        );

        // Other groups carry no Wine info.
        let (records, extras) = build(&[proc(1, "/usr/bin/bash", MIB)]);
        let b = groups.bands(&records, &extras, None);
        assert_eq!(b.top[0].wine, None);
    }

    #[test]
    fn mem_group_keys_per_process() {
        let mut groups = MemGroups::new(UID);
        let chrome = Some(("google-chrome", "Google Chrome"));
        let procs = [
            Proc {
                app: chrome,
                ..proc(10, "/opt/google/chrome/chrome", 100 * MIB)
            },
            proc(20, "/home/u/.local/share/claude/versions/2.1.309", 40 * MIB),
            proc(21, "/home/u/.local/share/claude/versions/2.1.310", 30 * MIB),
            Proc {
                flags: ProcFlags::EXITED,
                ..proc(22, "/home/u/.local/share/claude/versions/2.1.310", 30 * MIB)
            },
        ];
        let (records, extras) = build(&procs);
        groups.bands(&records, &extras, None);
        let key = |pid: u32| {
            let r = records.iter().find(|r| r.key.pid == pid).expect("record");
            groups.keys.get(&r.key).map(|k| &**k)
        };
        assert_eq!(key(10), Some("app:google-chrome"));
        assert_eq!(key(20), Some("exe:claude"));
        assert_eq!(key(21), Some("exe:claude"));
        assert_eq!(key(22), None, "an exited record has no key");
        assert_eq!(groups.keys.len(), 3);

        // The next sweep replaces the keys.
        let (records, extras) = build(&procs[..1]);
        groups.bands(&records, &extras, None);
        assert_eq!(groups.keys.len(), 1);
    }

    /// Sets the CPU rate and the cgroup of the record with `pid`.
    fn set_cpu(records: &mut [ProcessRecord], pid: u32, pct: f32, cgroup: Option<&str>) {
        let r = records
            .iter_mut()
            .find(|r| r.key.pid == pid)
            .expect("record");
        r.cpu_pct = pct;
        r.validity.set(ProcessField::Cpu, Validity::Valid);
        let mut meta = ProcessMeta::clone(&r.meta);
        meta.cgroup = cgroup.map(Arc::from);
        r.meta = Arc::new(meta);
    }

    /// The window of one `/proc/stat` step with these tick deltas.
    fn window(user: u64, irq: u64, softirq: u64, steal: u64, idle: u64) -> CpuWindow {
        let new = CpuTimes {
            user,
            idle,
            irq,
            softirq,
            steal,
            ..CpuTimes::default()
        };
        CpuWindow::between(&CpuTimes::default(), &new).expect("a window")
    }

    fn sweep_cpu(window: CpuWindow, cgroups: &[(&str, f64)]) -> SweepCpu {
        SweepCpu {
            window,
            cgroups: cgroups.iter().map(|&(k, v)| (Arc::from(k), v)).collect(),
        }
    }

    fn cpu_of<'a>(c: &'a CpuBands, key: &str) -> Option<&'a CpuGroupSum> {
        c.top.iter().find(|g| &*g.group.key == key)
    }

    fn cpu_sum(c: &CpuBands) -> f64 {
        c.top.iter().map(|g| g.pct).sum::<f64>() + c.other_pct + c.system.iter().sum::<f64>()
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn cpu_exited_children_go_to_parent_group() {
        const SCOPE: &str = "user.slice/user-1000.slice/app-konsole-1.scope";
        // user + nice + system 60%, no interrupts.
        let w = window(600, 0, 0, 0, 400);
        let mut groups = MemGroups::new(UID);
        // bash is the oldest process in the scope; cargo runs the build.
        let procs = [
            proc(5, "/usr/bin/bash", MIB),
            proc(10, "/usr/bin/cargo", MIB),
            proc(20, "/usr/bin/fish", MIB),
        ];
        let (mut records, extras) = build(&procs);
        set_cpu(&mut records, 5, 0.0, Some(SCOPE));
        set_cpu(&mut records, 10, 5.0, Some(SCOPE));
        set_cpu(&mut records, 20, 1.0, Some("user.slice/other.scope"));
        // The rustc children ran and exited between the sweeps: the scope used 40%.
        let c = sweep_cpu(w, &[(SCOPE, 40.0)]);
        let b = groups.bands_with_cpu(&records, &extras, None, Some(&c));
        let cpu = b.cpu.expect("a window");
        assert!(near(cpu_of(&cpu, "exe:cargo").expect("cargo").pct, 40.0));
        assert!(near(cpu_of(&cpu, "exe:bash").expect("bash").pct, 0.0));
        assert!(
            near(cpu.system[SHORT], 60.0 - 41.0),
            "35% is not short-lived"
        );
        assert!(!cpu.scaled);

        // No live CPU in the scope: the 40% goes to the group of the oldest live process.
        set_cpu(&mut records, 10, 0.0, Some(SCOPE));
        let b = groups.bands_with_cpu(&records, &extras, None, Some(&c));
        let cpu = b.cpu.expect("a window");
        assert!(near(cpu_of(&cpu, "exe:bash").expect("bash").pct, 40.0));
        assert!(near(cpu_of(&cpu, "exe:cargo").expect("cargo").pct, 0.0));
        assert!(near(cpu.system[SHORT], 60.0 - 41.0));

        // The scope has no live process: its CPU stays in Short-lived processes.
        set_cpu(&mut records, 5, 0.0, Some("user.slice/a.scope"));
        set_cpu(&mut records, 10, 5.0, Some("user.slice/a.scope"));
        let b = groups.bands_with_cpu(&records, &extras, None, Some(&c));
        let cpu = b.cpu.expect("a window");
        assert!(near(cpu_of(&cpu, "exe:cargo").expect("cargo").pct, 5.0));
        assert!(near(cpu.system[SHORT], 60.0 - 6.0));
        // Nor does a cgroup without a delta give exited CPU.
        let b = groups.bands_with_cpu(&records, &extras, None, Some(&sweep_cpu(w, &[])));
        assert!(near(b.cpu.expect("a window").system[SHORT], 60.0 - 6.0));
    }

    #[test]
    fn cpu_kernel_threads_band() {
        let w = window(500, 20, 30, 10, 440);
        let mut groups = MemGroups::new(UID);
        let procs = [
            Proc {
                uid: 0,
                exe: None,
                flags: ProcFlags::KERNEL_THREAD,
                ..proc(2, "kworker/0:1", 0)
            },
            Proc {
                uid: 0,
                ..proc(1, "/usr/lib/systemd/systemd", 7 * MIB)
            },
        ];
        let (mut records, extras) = build(&procs);
        set_cpu(&mut records, 2, 3.0, None);
        set_cpu(&mut records, 1, 1.0, None);
        let b = groups.bands_with_cpu(&records, &extras, None, Some(&sweep_cpu(w, &[])));
        // The memory chart does not change: the kernel thread counts in system with 0 bytes.
        assert_eq!(b.top.len(), 1);
        assert_eq!(&*b.top[0].group.key, "system");
        assert_eq!(b.top[0].processes, 2);
        let cpu = b.cpu.expect("a window");
        assert!(near(cpu.system[KTHREADS], 3.0));
        assert_eq!(cpu.kernel_threads, 1);
        let system = cpu_of(&cpu, "system").expect("system group");
        assert!(near(system.pct, 1.0), "kernel thread CPU is not in system");
        assert_eq!(system.processes, 1);
        assert!(near(cpu.system[1], 2.0));
        assert!(near(cpu.system[2], 3.0));
        assert!(near(cpu.system[3], 1.0));
        assert!(near(cpu.system[SHORT], 50.0 - 4.0));
        assert!(near(cpu_sum(&cpu), w.busy_pct));

        // A cgroup that holds only a kernel thread has no owner for its exited CPU.
        set_cpu(&mut records, 2, 3.0, Some("kthreadd.scope"));
        let c = sweep_cpu(w, &[("kthreadd.scope", 10.0)]);
        let cpu = groups
            .bands_with_cpu(&records, &extras, None, Some(&c))
            .cpu
            .expect("a window");
        assert!(near(cpu.system[KTHREADS], 3.0));
        assert!(near(cpu.system[SHORT], 50.0 - 4.0));
    }

    /// One simulated process in `cpu_groups_sum_to_busy`.
    struct SimProc {
        pid: u32,
        exe: &'static str,
        cgroup: Option<usize>,
        kernel: bool,
        /// Sweeps seen; the first has no rate.
        age: u32,
    }

    #[test]
    fn cpu_groups_sum_to_busy() {
        const CGROUPS: [&str; 8] = [
            "user.slice/a.scope",
            "user.slice/b.scope",
            "user.slice/c.scope",
            "user.slice/d.service",
            "system.slice/e.service",
            "system.slice/f.service",
            "user.slice/g.scope",
            "user.slice/h.scope",
        ];
        let mut rng = Lcg(42);
        let mut groups = MemGroups::new(UID);
        let mut sim: Vec<SimProc> = Vec::new();
        let mut next_pid = 1u32;
        let mut scaled_seen = 0;
        for sweep in 0..500 {
            // Exits, then starts.
            sim.retain(|_| rng.next(10) != 0);
            for _ in 0..rng.next(20) {
                let kernel = rng.next(8) == 0;
                sim.push(SimProc {
                    pid: next_pid,
                    exe: EXES[rng.next(EXES.len() as u64) as usize],
                    cgroup: match rng.next(10) {
                        0 => None,
                        _ if kernel && rng.next(2) == 0 => None,
                        _ => Some(rng.next(CGROUPS.len() as u64) as usize),
                    },
                    kernel,
                    age: 0,
                });
                next_pid += 1;
            }
            let procs: Vec<Proc> = sim
                .iter()
                .map(|p| Proc {
                    uid: if p.kernel { 0 } else { UID },
                    exe: (!p.kernel).then_some(p.exe),
                    flags: if p.kernel {
                        ProcFlags::KERNEL_THREAD
                    } else {
                        ProcFlags::empty()
                    },
                    ..proc(p.pid, p.exe, rng.next(64) * MIB)
                })
                .collect();
            let (mut records, extras) = build(&procs);
            // Random rates; a process in its first sweep has none.
            let mut live_cpu = [0.0f64; CGROUPS.len()];
            let mut owners = [false; CGROUPS.len()];
            let mut procs_total = 0.0f64;
            for p in &mut sim {
                let pct = if p.age == 0 {
                    0.0
                } else {
                    rng.next(400) as f32 / 100.0
                };
                p.age += 1;
                let cgroup = p.cgroup.map(|c| CGROUPS[c]);
                set_cpu(&mut records, p.pid, pct, cgroup);
                if p.age == 1 {
                    let r = records
                        .iter_mut()
                        .find(|r| r.key.pid == p.pid)
                        .expect("record");
                    r.validity.set(ProcessField::Cpu, Validity::Missing);
                }
                procs_total += f64::from(pct);
                if let Some(c) = p.cgroup {
                    live_cpu[c] += f64::from(pct);
                    owners[c] |= !p.kernel;
                }
            }
            // Cgroup deltas: the live CPU plus exited CPU, sometimes less than the live CPU,
            // sometimes unreadable, sometimes for a cgroup with no live process.
            let mut cgroups = Vec::new();
            let mut exited_total = 0.0f64;
            for (c, path) in CGROUPS.iter().enumerate() {
                if rng.next(6) == 0 {
                    continue;
                }
                let delta = (live_cpu[c] + rng.next(3000) as f64 / 100.0 - 5.0).max(0.0);
                cgroups.push((*path, delta));
                if owners[c] {
                    exited_total += (delta - live_cpu[c]).max(0.0);
                }
            }
            let total_ticks = 10_000;
            let user = rng.next(8_000);
            let irq = rng.next(300);
            let softirq = rng.next(300);
            let steal = rng.next(200);
            let idle = total_ticks - user - irq - softirq - steal;
            let w = window(user, irq, softirq, steal, idle);
            let busy = 100.0 * (user + irq + softirq + steal) as f64 / total_ticks as f64;

            let b = groups.bands_with_cpu(&records, &extras, None, Some(&sweep_cpu(w, &cgroups)));
            let cpu = b.cpu.expect("a window");
            let label = format!("sweep {sweep}");
            assert!(
                (cpu_sum(&cpu) - busy).abs() < 1e-3,
                "{label}: {} != {busy}",
                cpu_sum(&cpu)
            );
            // The published f32 values sum the same.
            let f32_sum: f64 = cpu
                .top
                .iter()
                .map(|g| g.pct)
                .chain([cpu.other_pct])
                .chain(cpu.system)
                .map(|v| f64::from(v as f32))
                .sum();
            assert!((f32_sum - busy).abs() < 1e-3, "{label}: f32 sum");
            assert!(cpu.top.iter().all(|g| g.pct >= 0.0), "{label}");
            assert!(cpu.other_pct >= 0.0, "{label}");
            assert!(cpu.system.iter().all(|&v| v >= 0.0), "{label}");
            assert!(cpu.top.len() <= TOP_GROUPS, "{label}");
            assert!(
                cpu.top.windows(2).all(|w| w[0].pct >= w[1].pct),
                "{label}: largest first"
            );
            let user_pct = 100.0 * user as f64 / total_ticks as f64;
            let attributed = procs_total + exited_total;
            if attributed > user_pct + 1e-6 {
                assert!(cpu.scaled, "{label}: scaled");
                assert_eq!(cpu.system[SHORT], 0.0, "{label}: short-lived is 0");
                scaled_seen += 1;
            } else if attributed < user_pct - 1e-6 {
                assert!(!cpu.scaled, "{label}");
                assert!(
                    (cpu.system[SHORT] - (user_pct - attributed)).abs() < 1e-6,
                    "{label}: short-lived"
                );
            }
            let kernel = sim.iter().filter(|p| p.kernel).count() as u32;
            let counted: u32 =
                cpu.top.iter().map(|g| g.processes).sum::<u32>() + cpu.other_processes;
            assert_eq!(counted + cpu.kernel_threads, sim.len() as u32, "{label}");
            assert_eq!(cpu.kernel_threads, kernel, "{label}");
        }
        assert!(scaled_seen > 10, "the scaled case ran {scaled_seen} times");
    }
}
