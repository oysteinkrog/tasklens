//! Checks for the captured `minimal-vm` fixture (ADR-013, E04-032).
//!
//! The fixture comes from `cargo xtask capture-fixture minimal-vm`, run as the default user in
//! an Arch Linux cloud image under QEMU with KVM: 2 vCPUs, 2 GiB RAM, one virtio-blk disk, one
//! virtio-net NIC on user networking, no zram, no cpufreq driver and no hwmon devices. The
//! samplers must handle the missing sources without panicking or reporting zeros.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{fixture, fixture_dir};
use tl_collect::counters::Clock;
use tl_collect::parse::block::{DiskKind, enumerate_disks};
use tl_collect::parse::cpufreq::{FreqSource, read_base_mhz, read_static};
use tl_collect::sample::cpu::{CpuOpts, CpuReading, CpuSampler};
use tl_collect::sample::disk::{DiskId, DiskReading, DiskSampler};
use tl_collect::sample::memory::{MemOpts, MemoryReading, MemorySampler};
use tl_collect::sample::net::{IfaceId, NetReading, NetSampler};
use tl_collect::sample::sensors::{SensorsReading, SensorsSampler};
use tl_model::validity::Validity;

const NAME: &str = "minimal-vm";

/// Hostname, user name and NIC address of the capture VM. None may appear in the fixture.
const VM_SECRETS: &[&str] = &["tlminivmhost", "52:54:00:ab:cd:ef", "52:54:00:AB:CD:EF"];

/// Two clocks one second apart.
const T0: Clock = Clock {
    boot_ns: 1_000_000_000,
    mono_ns: 1_000_000_000,
};
const T1: Clock = Clock {
    boot_ns: 2_000_000_000,
    mono_ns: 2_000_000_000,
};

/// Every entry under `root`, relative to it. Symlinks are listed but not followed.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in fs::read_dir(root.join(&rel)).expect("read fixture dir") {
            let entry = entry.expect("read fixture dir entry");
            let child = rel.join(entry.file_name());
            if entry.file_type().expect("file type").is_dir() {
                stack.push(child.clone());
            }
            out.push(child);
        }
    }
    out
}

#[test]
fn layout() {
    let root = fixture_dir(NAME);
    for rel in [
        "proc/stat",
        "proc/meminfo",
        "proc/diskstats",
        "proc/net/dev",
    ] {
        assert!(root.join(rel).is_file(), "missing {rel}");
    }
    assert!(
        root.join("sys/block/vda").is_dir(),
        "missing sys/block/vda/"
    );
    for iface in ["eth0", "lo"] {
        assert!(
            root.join("sys/class/net").join(iface).is_dir(),
            "missing sys/class/net/{iface}/"
        );
    }
    let eth0 = fs::read_link(root.join("sys/class/net/eth0")).expect("eth0 is a link");
    assert!(
        eth0.to_string_lossy().contains("/virtio"),
        "eth0 is not virtio: {}",
        eth0.display()
    );

    let hwmon = root.join("sys/class/hwmon");
    if hwmon.exists() {
        let chips: Vec<_> = fs::read_dir(&hwmon)
            .expect("read sys/class/hwmon")
            .map(|e| e.expect("hwmon entry").file_name())
            .filter(|n| n.to_string_lossy().starts_with("hwmon"))
            .collect();
        assert!(chips.is_empty(), "hwmon chips: {chips:?}");
    }
    let zram: Vec<_> = fs::read_dir(root.join("sys/block"))
        .expect("read sys/block")
        .map(|e| e.expect("block entry").file_name())
        .filter(|n| n.to_string_lossy().starts_with("zram"))
        .collect();
    assert!(zram.is_empty(), "zram devices: {zram:?}");
}

#[test]
fn scrubbed() {
    let root = fixture_dir(NAME);
    for rel in walk(&root) {
        let path = root.join(&rel);
        let text = if path.is_symlink() {
            fs::read_link(&path)
                .expect("read fixture link")
                .to_string_lossy()
                .into_owned()
        } else if path.is_file() {
            String::from_utf8_lossy(&fs::read(&path).expect("read fixture file")).into_owned()
        } else {
            continue;
        };
        for secret in VM_SECRETS {
            assert!(!text.contains(secret), "{} holds {secret}", rel.display());
        }
        let lower = text.to_ascii_lowercase();
        assert!(
            !lower.contains("arch:x:") && !lower.contains("/home/arch"),
            "{} holds the user name",
            rel.display()
        );
        if rel.ends_with("address") {
            assert_eq!(text.trim(), "00:00:00:00:00:00", "{}", rel.display());
        }
    }
    let boot_id = fs::read_to_string(root.join("proc/sys/kernel/random/boot_id")).unwrap();
    assert_eq!(boot_id, "00000000-0000-4000-8000-000000000001\n");
}

#[test]
fn samplers_handle_missing_sources() {
    let opts = CpuOpts {
        per_cpu: true,
        freq: true,
        temp: true,
        live_stats: true,
        power: true,
    };
    let mut cpu = CpuSampler::new(fixture(NAME)).expect("topology reads");
    let mut cpu_out = CpuReading::default();
    let mut mem = MemorySampler::new(fixture(NAME));
    let mut mem_out = MemoryReading::default();
    let mut disk = DiskSampler::new(fixture(NAME));
    disk.set_devices(&[DiskId {
        major: 254,
        minor: 0,
        generation: 0,
    }]);
    let mut disk_out = DiskReading::default();
    let mut net = NetSampler::new(fixture(NAME));
    let ifaces: Vec<IfaceId> = ["eth0", "lo"]
        .into_iter()
        .map(|name| IfaceId {
            name: Arc::from(name),
            generation: 0,
        })
        .collect();
    net.set_interfaces(&ifaces);
    let mut net_out = NetReading::default();
    let mut sensors = SensorsSampler::new(fixture(NAME));
    let mut sensors_out = SensorsReading::default();

    for clock in [T0, T1] {
        cpu.sample_at(clock, opts, &mut cpu_out);
        mem.sample_at(clock, MemOpts { details: true }, &mut mem_out);
        disk.sample_at(clock, &mut disk_out);
        net.sample_at(clock, &mut net_out);
        sensors.sample_at(clock, &mut sensors_out);
    }

    // No cpufreq policy: the clock falls back to the `cpu MHz` lines of /proc/cpuinfo
    // (PERF-033), and everything that only cpufreq gives is Unsupported.
    assert_eq!(cpu_out.online_cpus, 2);
    assert_eq!(cpu_out.freq.source, FreqSource::CpuinfoMhz);
    assert_eq!(
        read_base_mhz(&fixture(NAME)).validity,
        Validity::Unsupported
    );
    let freq_static = read_static(&fixture(NAME));
    assert_eq!(freq_static.driver.validity, Validity::Unsupported);
    assert_eq!(freq_static.governor.validity, Validity::Unsupported);
    assert_eq!(cpu_out.temp_c.validity, Validity::Unsupported);

    // No zram device: nothing is listed and nothing is counted as compressed.
    assert_eq!(mem_out.validity, Validity::Valid);
    assert!(mem_out.zram.is_empty(), "{:?}", mem_out.zram);
    assert_eq!(mem_out.zram_mem_used_total, 0);

    assert_eq!(disk_out.validity, Validity::Valid);
    assert_eq!(disk_out.devices.len(), 1);
    assert_eq!(disk_out.devices[0].temp_c.validity, Validity::Unsupported);
    assert!(disk_out.devices[0].total_read_bytes.validity.is_valid());

    assert_eq!(net_out.validity, Validity::Valid);
    assert_eq!(net_out.ifaces.len(), 2);
    assert!(
        net_out
            .ifaces
            .iter()
            .all(|i| i.total_rx_bytes.validity.is_valid())
    );

    assert!(sensors.chips().is_empty());
    assert!(sensors_out.temps.is_empty(), "{:?}", sensors_out.temps);
    assert!(sensors_out.fans.is_empty(), "{:?}", sensors_out.fans);
}

#[test]
fn vda_is_virtual() {
    let mut disks = Vec::new();
    enumerate_disks(&fixture(NAME), &mut disks).expect("sys/block lists");
    let names: Vec<&str> = disks.iter().map(|d| &*d.name).collect();
    assert_eq!(names, ["vda"]);
    assert_eq!(disks[0].kind, DiskKind::Virtual);
    assert_eq!((disks[0].major, disks[0].minor), (254, 0));
    assert_eq!(disks[0].children.len(), 3);
}
