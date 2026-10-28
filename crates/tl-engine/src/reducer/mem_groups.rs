//! Memory groups for the memory-by-app chart (D-036, UI-PRF-060).
//!
//! On every process sweep the reducer sums RssAnon per memory group (tl-collect
//! `apps/mem_group.rs`) and publishes the top 20 groups, "Other apps" and "Shared and kernel" as
//! `MEM_GROUP` series on `DeviceIdentity::MemGroup` devices. "Shared and kernel" is In use minus
//! the sum over all processes, clamped at 0. The engine always publishes 20 groups, so the UI's
//! `top_n` setting needs no engine change.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use tl_collect::apps::mem_group::{MemGroup, MemGroupInput, MemGrouper};
use tl_model::ids::{DeviceIdx, GroupKey};
use tl_model::process::{ProcFlags, ProcessExtras, ProcessRecord};
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
/// Prefix of the persistent device key: `memgroup:<group key>`.
pub const DEVICE_KEY_PREFIX: &str = "memgroup:";

/// The RssAnon sum of one memory group in one sweep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupSum {
    pub group: Arc<MemGroup>,
    pub bytes: u64,
    pub processes: u32,
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
            self_uid,
            devices: FxHashMap::default(),
            shown: Vec::new(),
            other_key: Arc::from(OTHER_KEY),
            other_name: Arc::from(OTHER_NAME),
            shared_key: Arc::from(SHARED_KEY),
            shared_name: Arc::from(SHARED_NAME),
        }
    }

    /// Sums RssAnon per memory group over the live `records`. `extras` is index aligned with
    /// `records`, or empty. `in_use` is `MemTotal - MemAvailable` when known. Exited records
    /// are left out; a record whose RssAnon cannot be read counts as a process with 0 bytes.
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
                }
            })
            .collect();
        self.grouper.classify(&inputs, self.self_uid, &mut self.out);

        let mut sums: Vec<GroupSum> = Vec::new();
        let mut index: FxHashMap<&str, usize> = FxHashMap::default();
        let mut total = 0u64;
        for (&i, group) in live.iter().zip(&self.out) {
            let bytes = rss_anon(&records[i]);
            total = total.saturating_add(bytes);
            let slot = *index.entry(&group.key).or_insert_with(|| {
                sums.push(GroupSum {
                    group: Arc::clone(group),
                    bytes: 0,
                    processes: 0,
                });
                sums.len() - 1
            });
            let sum = &mut sums[slot];
            sum.bytes = sum.bytes.saturating_add(bytes);
            sum.processes += 1;
        }
        drop(index);

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
    use tl_model::process::{GroupKind, ProcState, ProcessMeta};
    use tl_model::system::CpuSet;
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
}
