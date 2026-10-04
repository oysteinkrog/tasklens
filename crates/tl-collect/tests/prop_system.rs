//! Property tests for the system side of `tl-collect` (ADR-013): kept file reads, counter rules
//! and the suspend check, plus self-checks of the shared fixture helpers.

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use common::{apply_step, fixture, fixture_bytes, fixture_copy, fixture_step, mutated};
use proptest::prelude::*;
use tl_collect::counters::{Clock, Counter, resumed_between};
use tl_collect::sysroot::KeptFile;
use tl_model::validity::Validity;

proptest! {
    #[test]
    fn prop_kept_file_returns_exact_bytes(
        bytes in mutated(fixture_bytes("owner-desktop", "proc/stat"))
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stat");
        fs::write(&path, &bytes).unwrap();
        let mut kept = KeptFile::new(path);
        let mut buf = Vec::new();
        prop_assert_eq!(kept.read(&mut buf).unwrap(), bytes.len());
        prop_assert_eq!(&buf, &bytes);
    }

    #[test]
    fn prop_counter_sequences(
        steps in prop::collection::vec((any::<u64>(), any::<u64>(), any::<u32>()), 0..=64)
    ) {
        let mut counter = Counter::default();
        for (value, t_ns, generation) in steps {
            let rate = counter.update(value, t_ns, generation);
            if rate.validity == Validity::Valid {
                prop_assert!(rate.per_sec.is_finite());
                prop_assert!(rate.per_sec >= 0.0);
            }
        }
    }

    #[test]
    fn prop_resumed_between_no_panic(
        a in (any::<u64>(), any::<u64>()),
        b in (any::<u64>(), any::<u64>()),
    ) {
        let prev = Clock { boot_ns: a.0, mono_ns: a.1 };
        let cur = Clock { boot_ns: b.0, mono_ns: b.1 };
        resumed_between(prev, cur);
        resumed_between(cur, prev);
    }
}

#[test]
fn owner_desktop_proc_stat_exists() {
    assert!(fixture("owner-desktop").proc_path("stat").exists());
}

#[test]
fn fixture_step_paths() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-step");
    let root = fixture_step("two-step", 1);
    assert_eq!(root.proc, base.join("step1/proc"));
    assert_eq!(root.sys, base.join("step1/sys"));
    assert_eq!(root.run, base.join("step1/run"));
    assert_eq!(root.proc_path("stat"), base.join("step1/proc/stat"));
}

#[test]
fn apply_step_keeps_inodes() {
    // A two-step fixture in a tempdir; the helpers take an absolute fixture name as is.
    let src = tempfile::tempdir().unwrap();
    let write = |rel: &str, content: &str| {
        let path = src.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    };
    write("step0/proc/stat", "cpu 1 2 3\n");
    write("step0/sys/class/x/value", "10\n");
    write("step1/proc/stat", "cpu 4 5 6 7 8\n");
    write("step1/sys/class/x/value", "2\n");
    std::os::unix::fs::symlink("../proc/stat", src.path().join("step0/sys/link")).unwrap();
    std::os::unix::fs::symlink("../proc/stat", src.path().join("step1/sys/link")).unwrap();
    let name = src.path().to_str().unwrap();

    let copy = fixture_copy(name, Some(0));
    let files: Vec<PathBuf> = ["proc/stat", "sys/class/x/value"]
        .iter()
        .map(|rel| copy.path().join(rel))
        .collect();
    let link = copy.path().join("sys/link");
    assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("../proc/stat"));
    assert_eq!(fs::read(&files[0]).unwrap(), b"cpu 1 2 3\n");
    let inodes: Vec<u64> = files
        .iter()
        .map(|p| fs::metadata(p).unwrap().ino())
        .collect();

    // A kept fd opened on step 0 sees the step 1 content after apply_step.
    let mut kept = KeptFile::new(files[0].clone());
    let mut buf = Vec::new();
    kept.read(&mut buf).unwrap();
    assert_eq!(buf, b"cpu 1 2 3\n");

    apply_step(copy.path(), name, 1);

    assert_eq!(fs::read(&files[0]).unwrap(), b"cpu 4 5 6 7 8\n");
    assert_eq!(fs::read(&files[1]).unwrap(), b"2\n");
    for (path, ino) in files.iter().zip(&inodes) {
        assert_eq!(fs::metadata(path).unwrap().ino(), *ino);
    }
    kept.read(&mut buf).unwrap();
    assert_eq!(buf, b"cpu 4 5 6 7 8\n");
}

// Parser cases: added by E04-A3.

use smallvec::SmallVec;
use tl_collect::parse::block::{enumerate_disks, parse_dev};
use tl_collect::parse::diskstats::parse_diskstats;
use tl_collect::parse::filenr::parse_file_nr;
use tl_collect::parse::hwmon::{parse_fan_control, parse_millidegrees, parse_pwm_pct, parse_rpm};
use tl_collect::parse::loadavg::parse_loadavg;
use tl_collect::parse::meminfo::{MemInfo, parse_meminfo};
use tl_collect::parse::netdev::parse_net_dev;
use tl_collect::parse::netif;
use tl_collect::parse::stat::{StatData, parse_stat};
use tl_collect::parse::topology::parse_cpu_list;
use tl_collect::parse::zram::{
    ZramMmStat, parse_block_stat_sectors, parse_comp_algorithm, parse_mm_stat,
};
use tl_collect::sample::cpu::{CpuOpts, CpuReading, CpuSampler};
use tl_collect::sample::disk::{DiskId, DiskReading, DiskSampler};
use tl_collect::sample::memory::{MemOpts, MemoryReading, MemorySampler};
use tl_collect::sample::net::{IfaceId, NetReading, NetSampler};
use tl_collect::sample::sensors::{SensorsReading, SensorsSampler};

const OWNER: &str = "owner-desktop";
const ZRAM: &str = "zram-device";

proptest! {
    #[test]
    fn prop_parse_stat(
        a in common::arbitrary_bytes(),
        m in mutated(fixture_bytes(OWNER, "proc/stat")),
    ) {
        let mut out = StatData::default();
        let _ = parse_stat(&a, &mut out);
        let _ = parse_stat(&m, &mut out);
    }

    #[test]
    fn prop_parse_meminfo(
        a in common::arbitrary_bytes(),
        m in mutated(fixture_bytes(OWNER, "proc/meminfo")),
    ) {
        let mut out = MemInfo::default();
        let _ = parse_meminfo(&a, &mut out);
        let _ = parse_meminfo(&m, &mut out);
    }

    #[test]
    fn prop_parse_diskstats(
        a in common::arbitrary_bytes(),
        m in mutated(fixture_bytes(OWNER, "proc/diskstats")),
    ) {
        let mut out = Vec::new();
        let _ = parse_diskstats(&a, &mut out);
        let _ = parse_diskstats(&m, &mut out);
    }

    #[test]
    fn prop_parse_net_dev(
        a in common::arbitrary_bytes(),
        m in mutated(fixture_bytes(OWNER, "proc/net/dev")),
    ) {
        let mut out = Vec::new();
        let _ = parse_net_dev(&a, &mut out);
        let _ = parse_net_dev(&m, &mut out);
    }

    #[test]
    fn prop_parse_loadavg(
        a in common::arbitrary_bytes(),
        m in mutated(fixture_bytes(OWNER, "proc/loadavg")),
    ) {
        let _ = parse_loadavg(&a);
        let _ = parse_loadavg(&m);
    }

    #[test]
    fn prop_parse_file_nr(
        a in common::arbitrary_bytes(),
        m in mutated(fixture_bytes(OWNER, "proc/sys/fs/file-nr")),
    ) {
        let _ = parse_file_nr(&a);
        let _ = parse_file_nr(&m);
    }

    #[test]
    fn prop_parse_mm_stat(
        a in common::arbitrary_bytes(),
        m in mutated(fixture_bytes(ZRAM, "sys/block/zram0/mm_stat")),
    ) {
        let mut out = ZramMmStat::default();
        let _ = parse_mm_stat(&a, &mut out);
        let _ = parse_mm_stat(&m, &mut out);
    }

    #[test]
    fn prop_parse_block_stat_sectors(
        a in common::arbitrary_bytes(),
        m in mutated(fixture_bytes(ZRAM, "sys/block/zram0/stat")),
    ) {
        let _ = parse_block_stat_sectors(&a);
        let _ = parse_block_stat_sectors(&m);
    }

    #[test]
    fn prop_parse_comp_algorithm(
        a in common::arbitrary_bytes(),
        m in mutated(fixture_bytes(ZRAM, "sys/block/zram0/comp_algorithm")),
    ) {
        let _ = parse_comp_algorithm(&a);
        let _ = parse_comp_algorithm(&m);
    }

    #[test]
    fn prop_parse_cpu_list(
        a in common::arbitrary_bytes(),
        m in mutated(fixture_bytes(OWNER, "sys/devices/system/cpu/online")),
    ) {
        let mut out = SmallVec::<[u16; 64]>::new();
        let _ = parse_cpu_list(&a, &mut out);
        let _ = parse_cpu_list(&m, &mut out);
    }

    #[test]
    fn prop_parse_dev(a in common::arbitrary_bytes(), m in mutated(b"259:0\n".to_vec())) {
        let _ = parse_dev(&a);
        let _ = parse_dev(&m);
    }

    #[test]
    fn prop_hwmon_values(a in common::arbitrary_bytes()) {
        let _ = parse_millidegrees(&a);
        let _ = parse_rpm(&a);
        let _ = parse_pwm_pct(&a);
        let _ = parse_fan_control(&a);
    }
}

/// Every disk and child of the owner-desktop fixture, generation 1.
fn owner_disk_ids() -> Vec<DiskId> {
    let mut disks = Vec::new();
    enumerate_disks(&fixture(OWNER), &mut disks).unwrap();
    let mut ids = Vec::new();
    for disk in &disks {
        ids.push(DiskId {
            major: disk.major,
            minor: disk.minor,
            generation: 1,
        });
        for child in &disk.children {
            ids.push(DiskId {
                major: child.major,
                minor: child.minor,
                generation: 1,
            });
        }
    }
    ids
}

/// Every interface of the owner-desktop fixture, generation 1.
fn owner_iface_ids() -> Vec<IfaceId> {
    let mut ifaces = Vec::new();
    netif::enumerate(&fixture(OWNER), &mut ifaces).unwrap();
    ifaces
        .iter()
        .map(|iface| IfaceId {
            name: iface.name.clone(),
            generation: 1,
        })
        .collect()
}

const ALL_CPU: CpuOpts = CpuOpts {
    per_cpu: true,
    freq: true,
    temp: true,
    live_stats: true,
};

const ALL_MEM: MemOpts = MemOpts { details: true };

proptest! {
    // Each case copies the fixture to a tempdir, so keep the case count low.
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn prop_samplers_on_mutated_files(
        stat in mutated(fixture_bytes(OWNER, "proc/stat")),
        meminfo in mutated(fixture_bytes(OWNER, "proc/meminfo")),
        diskstats in mutated(fixture_bytes(OWNER, "proc/diskstats")),
        net_dev in mutated(fixture_bytes(OWNER, "proc/net/dev")),
    ) {
        let copy = fixture_copy(OWNER, None);
        let dir = copy.path();
        fs::write(dir.join("proc/stat"), &stat).unwrap();
        fs::write(dir.join("proc/meminfo"), &meminfo).unwrap();
        fs::write(dir.join("proc/diskstats"), &diskstats).unwrap();
        fs::write(dir.join("proc/net/dev"), &net_dev).unwrap();
        let root = tl_collect::SysRoot::at(dir);

        // A mutated proc/stat may make construction fail; that is a result, not a panic.
        if let Ok(mut cpu) = CpuSampler::new(root.clone()) {
            cpu.sample(ALL_CPU, &mut CpuReading::default());
        }
        MemorySampler::new(root.clone()).sample(ALL_MEM, &mut MemoryReading::default());
        let mut disk = DiskSampler::new(root.clone());
        disk.set_devices(&owner_disk_ids());
        disk.sample(&mut DiskReading::default());
        let mut net = NetSampler::new(root);
        net.set_interfaces(&owner_iface_ids());
        net.sample(&mut NetReading::default());
    }
}

#[test]
fn samplers_smoke_owner_desktop() {
    let root = fixture(OWNER);

    let mut cpu = CpuSampler::new(root.clone()).unwrap();
    let mut cpu_out = CpuReading::default();
    cpu.sample(ALL_CPU, &mut cpu_out);
    cpu.sample(ALL_CPU, &mut cpu_out);
    assert_eq!(cpu_out.live.load.validity, Validity::Valid);
    assert_eq!(cpu_out.live.open_files.validity, Validity::Valid);

    let mut mem = MemorySampler::new(root.clone());
    let mut mem_out = MemoryReading::default();
    mem.sample(ALL_MEM, &mut mem_out);
    mem.sample(ALL_MEM, &mut mem_out);
    assert_eq!(mem_out.validity, Validity::Valid);
    assert!(mem_out.used_bytes > 0);

    let mut disk = DiskSampler::new(root.clone());
    disk.set_devices(&owner_disk_ids());
    let mut disk_out = DiskReading::default();
    disk.sample(&mut disk_out);
    disk.sample(&mut disk_out);
    assert_eq!(disk_out.validity, Validity::Valid);
    assert!(
        disk_out
            .devices
            .iter()
            .any(|d| d.total_read_bytes.validity == Validity::Valid && d.total_read_bytes.value > 0)
    );

    let mut net = NetSampler::new(root.clone());
    net.set_interfaces(&owner_iface_ids());
    let mut net_out = NetReading::default();
    net.sample(&mut net_out);
    net.sample(&mut net_out);
    assert_eq!(net_out.validity, Validity::Valid);
    assert!(
        net_out
            .ifaces
            .iter()
            .any(|i| i.total_rx_bytes.validity == Validity::Valid && i.total_rx_bytes.value > 0)
    );

    let mut sensors = SensorsSampler::new(root);
    let mut sensors_out = SensorsReading::default();
    sensors.sample(&mut sensors_out);
    sensors.sample(&mut sensors_out);
    assert!(
        sensors_out
            .temps
            .iter()
            .any(|t| t.celsius.validity == Validity::Valid)
    );
}
