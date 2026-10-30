//! Memory groups for the memory-by-app chart (D-036, UI-PRF-060).
//!
//! On every process sweep the reducer sums RssAnon per memory group (tl-collect
//! `apps/mem_group.rs`) and publishes the top 20 groups, "Other apps" and "Shared and kernel" as
//! `MEM_GROUP` series on `DeviceIdentity::MemGroup` devices. "Shared and kernel" is In use minus
//! the sum over all processes, clamped at 0. It is also published as six parts from
//! /proc/meminfo (`kernel_parts`, UI-PRF-060 "Shared and kernel split", ADR-017). The engine
//! always publishes 20 groups, so the UI's `top_n` setting needs no engine change.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use tl_collect::apps::mem_group::{MemGroup, MemGroupInput, MemGroupKind, MemGrouper};
use tl_collect::apps::naming::windows_file_name;
use tl_model::ids::{DeviceIdx, GroupKey, ProcessKey};
use tl_model::process::{ProcFlags, ProcessExtras, ProcessRecord};
use tl_model::system::{MemoryTotals, WineGroupInfo};
use tl_model::validity::{ProcessField, Validity};

use super::table::{GroupClass, group_class};

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

/// The RssAnon sum of one memory group in one sweep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupSum {
    pub group: Arc<MemGroup>,
    pub bytes: u64,
    pub processes: u32,
    /// The prefix and Windows image names of a Wine group in the top 20.
    pub wine: Option<Arc<WineGroupInfo>>,
}

/// The bands of one sweep.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    pub other_key: Arc<str>,
    pub other_name: Arc<str>,
    pub shared_key: Arc<str>,
    pub shared_name: Arc<str>,
    /// Key and name of each of the six parts, as `KERNEL_PARTS`.
    pub kernel_keys: [(Arc<str>, Arc<str>); 6],
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
            other_key: Arc::from(OTHER_KEY),
            other_name: Arc::from(OTHER_NAME),
            shared_key: Arc::from(SHARED_KEY),
            shared_name: Arc::from(SHARED_NAME),
            kernel_keys: KERNEL_PARTS.map(|(k, n)| (Arc::from(k), Arc::from(n))),
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
        assert_eq!(&*g.group.key, "wine:/srv/backblaze/prefix:Backblaze");
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
}
