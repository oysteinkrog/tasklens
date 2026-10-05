//! Tests for the E12 system workers' sources, on copies of the tl-collect fixtures.
//!
//! Time is injected through each source's `sample_at`, so these are plain `#[test]`s.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tl_collect::SysRoot;
use tl_collect::counters::Clock;
use tl_collect::parse::{block, hwmon, netif};
use tl_engine::source::Source;
use tl_engine::update::{CpuSample, DeviceKinds, SourceUpdate};
use tl_engine::workers::cpu::CpuSource;
use tl_engine::workers::enumerate::{disk_list, net_list, sensor_list};
use tl_model::demand::{CpuDemand, Demand};
use tl_model::devices::DeviceIdentity;
use tl_model::settings::EngineSettings;
use tl_model::system::Metric;
use tl_model::validity::Validity;

const SEC: u64 = 1_000_000_000;
const MS: u64 = 1_000_000;

fn fixtures() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tl-collect/tests/fixtures/"
    ))
}

/// Copies `src` into `dst`, recreating symlinks. Files are rewritten in place, so a kept fd on
/// an existing file sees the new content.
fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create fixture dir");
    for entry in fs::read_dir(src).expect("list fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type().expect("fixture file type");
        if file_type.is_symlink() {
            let target = fs::read_link(entry.path()).expect("read fixture symlink");
            std::os::unix::fs::symlink(target, &to).expect("create fixture symlink");
        } else if file_type.is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            let bytes = fs::read(entry.path()).expect("read fixture file");
            fs::write(&to, bytes).expect("write fixture file");
        }
    }
}

/// A fresh copy of fixture `name` (or of its `step<N>` subdirectory) under the target tmpdir.
fn fixture_copy(name: &str, step: Option<u32>) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dst = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{n}"));
    if dst.exists() {
        fs::remove_dir_all(&dst).expect("remove old fixture copy");
    }
    let mut src = fixtures().join(name);
    if let Some(step) = step {
        src = src.join(format!("step{step}"));
    }
    copy_tree(&src, &dst);
    dst
}

/// Copies every file of `<name>/step<n>/` over `dst`.
fn apply_step(dst: &Path, name: &str, n: u32) {
    copy_tree(&fixtures().join(name).join(format!("step{n}")), dst);
}

fn at(t: u64) -> Clock {
    Clock {
        boot_ns: t,
        mono_ns: t,
    }
}

fn cpu_source(dir: &Path) -> CpuSource {
    CpuSource::new(SysRoot::at(dir)).expect("fixture topology reads")
}

fn cpu_sample(src: CpuSource, t: u64) -> (CpuSource, CpuSample) {
    match src.sample_at(at(t)) {
        (src, SourceUpdate::Cpu(sample)) => (src, sample),
        (_, other) => panic!("expected SourceUpdate::Cpu, got {other:?}"),
    }
}

fn demand(cpu: CpuDemand) -> Demand {
    Demand {
        cpu,
        ..Demand::default()
    }
}

/// `cpu-math` step 0 plus the `cpufreq-mixed` policies: policy0 reads `cpuinfo_avg_freq`.
fn cpu_with_freq() -> PathBuf {
    let dir = fixture_copy("cpu-math", Some(0));
    copy_tree(&fixtures().join("cpufreq-mixed"), &dir);
    dir
}

/// Rewrites policy0's clock in place.
fn set_policy0_khz(dir: &Path, khz: u64) {
    let file = dir.join("sys/devices/system/cpu/cpufreq/policy0/cpuinfo_avg_freq");
    fs::write(file, format!("{khz}\n")).expect("write cpuinfo_avg_freq");
}

#[test]
fn cpu_first_sample_warming_up_with_topology() {
    let dir = fixture_copy("cpu-math", Some(0));
    let (_, s) = cpu_sample(cpu_source(&dir), SEC);
    assert_eq!(s.totals.util_pct.validity, Validity::WarmingUp);
    assert_eq!(s.validity, Validity::WarmingUp);
    let topology = s.topology.expect("first sample carries the topology");
    assert_eq!(topology.online.count(), 1);
    assert!(topology.online.contains(0));
}

#[test]
fn cpu_math_second_sample_values() {
    let dir = fixture_copy("cpu-math", Some(0));
    let (src, _) = cpu_sample(cpu_source(&dir), SEC);
    apply_step(&dir, "cpu-math", 1);
    let (_, s) = cpu_sample(src, 2 * SEC);
    assert_eq!(s.totals.util_pct, Metric::valid(70.0));
    assert_eq!(s.totals.kernel_pct, Metric::valid(20.0));
    assert_eq!(s.irq_pct, 0.0);
    assert_eq!(s.validity, Validity::Valid);
    assert!(s.topology.is_none());
}

#[test]
fn cpu_per_core_follows_demand() {
    let dir = fixture_copy("cpu-hotplug", Some(0));
    let settings = EngineSettings::default();
    let src = cpu_source(&dir);
    src.period(&demand(CpuDemand::Total), &settings);
    let (src, s) = cpu_sample(src, SEC);
    assert!(s.per_cpu.is_empty());

    assert_eq!(
        src.period(&demand(CpuDemand::PerCore), &settings),
        Some(Duration::from_secs(1))
    );
    let (src, s) = cpu_sample(src, 2 * SEC);
    assert_eq!(s.per_cpu.len(), 4);
    let ids: Vec<u16> = s.per_cpu.iter().map(|c| c.cpu).collect();
    assert_eq!(ids, [0, 1, 2, 3]);

    // Recording keeps per-core data for history.
    let recording = Demand {
        recording: true,
        ..Demand::default()
    };
    src.period(&recording, &settings);
    let (_, s) = cpu_sample(src, 3 * SEC);
    assert_eq!(s.per_cpu.len(), 4);
}

#[test]
fn cpu_fast_ticks_reuse_slow_fields() {
    let dir = cpu_with_freq();
    let src = cpu_source(&dir);
    src.period(&demand(CpuDemand::PerCore), &EngineSettings::default());
    let (src, first) = cpu_sample(src, SEC);
    // policy0 4200 MHz (cpuinfo_avg_freq), policy2 2800 MHz (scaling_cur_freq).
    assert_eq!(first.freq_mhz, Some(3500));
    assert_eq!(first.totals.freq_fastest_mhz, Metric::valid(4200));
    assert_eq!(first.per_cpu[0].freq_mhz, Some(4200));
    // cpu-math has no CPU temperature chip.
    assert_eq!(first.temp_validity, Validity::Unsupported);
    assert_eq!(first.temp_c, None);

    // A fast tick does not read the clock files: the new value is not seen.
    set_policy0_khz(&dir, 1_000_000);
    let (src, fast) = cpu_sample(src, SEC + 100 * MS);
    assert_eq!(fast.freq_mhz, first.freq_mhz);
    assert_eq!(fast.totals.freq_mhz, first.totals.freq_mhz);
    assert_eq!(fast.per_cpu[0].freq_mhz, Some(4200));
    assert_eq!(fast.temp_c, first.temp_c);
    assert_eq!(fast.temp_validity, first.temp_validity);

    // 900 ms after the slow tick is still fast.
    let (src, fast) = cpu_sample(src, SEC + 900 * MS);
    assert_eq!(fast.freq_mhz, Some(3500));

    // One second after the slow tick reads the clock again.
    let (_, slow) = cpu_sample(src, 2 * SEC);
    assert_eq!(slow.freq_mhz, Some(1900));
    assert_eq!(slow.per_cpu[0].freq_mhz, Some(1000));
}

#[test]
fn cpu_reset_forces_slow_tick() {
    let dir = cpu_with_freq();
    let (src, first) = cpu_sample(cpu_source(&dir), SEC);
    assert_eq!(first.freq_mhz, Some(3500));
    apply_step(&dir, "cpu-math", 1);
    set_policy0_khz(&dir, 1_000_000);

    let mut src = src;
    src.reset_baselines();
    let (_, s) = cpu_sample(src, SEC + 100 * MS);
    assert_eq!(s.totals.util_pct.validity, Validity::WarmingUp);
    assert_eq!(s.freq_mhz, Some(1900));
}

#[test]
fn cpu_hotplug_sends_topology() {
    let dir = fixture_copy("cpu-hotplug", Some(0));
    let (src, first) = cpu_sample(cpu_source(&dir), SEC);
    let topology = first.topology.expect("first sample carries the topology");
    assert!(topology.online.contains(2));

    apply_step(&dir, "cpu-hotplug", 1);
    let (src, s) = cpu_sample(src, 2 * SEC);
    let topology = s.topology.expect("hotplug sends the new topology");
    assert!(!topology.online.contains(2));
    assert_eq!(topology.online.count(), 3);

    // Sent once per change.
    let (_, s) = cpu_sample(src, 3 * SEC);
    assert!(s.topology.is_none());
}

#[test]
fn enum_disk_list_children_and_parents() {
    let dir = fixture_copy("disk-partitions-dm", None);
    let root = SysRoot::at(&dir);
    let mut disks = Vec::new();
    block::enumerate_disks(&root, &mut disks).expect("fixture disks enumerate");
    let list = disk_list(7 * SEC, &disks, &root.run);
    assert_eq!(list.kinds, DeviceKinds::DISKS);
    assert_eq!(list.t_ns, 7 * SEC);

    let names: Vec<&str> = list.entries.iter().map(|e| &*e.display_name).collect();
    assert_eq!(
        names,
        [
            "mmcblk0", "nvme0n1", "sda", "sda1", "sda2", "dm-0", "sdb", "sr0", "vda"
        ]
    );
    let sda = DeviceIdentity::Disk {
        major: 8,
        minor: 0,
        name: "sda".into(),
        wwn_or_serial: None,
    };
    for entry in &list.entries {
        let is_child = matches!(&*entry.display_name, "sda1" | "sda2" | "dm-0");
        let want = is_child.then(|| sda.clone());
        assert_eq!(entry.parent, want, "{}", entry.display_name);
        assert!(entry.net_kind.is_none());
        // The fixture's udev entries carry no WWN, serial or path: the kernel name is the key.
        assert_eq!(
            &*entry.persistent_key,
            format!("disk:{}", entry.display_name)
        );
    }
}

#[test]
fn enum_net_list_no_loopback() {
    let dir = fixture_copy("netif-kinds", None);
    let root = SysRoot::at(&dir);
    let mut ifaces = Vec::new();
    netif::enumerate(&root, &mut ifaces).expect("fixture interfaces enumerate");
    let list = net_list(SEC, &ifaces, &root.sys);
    assert_eq!(list.kinds, DeviceKinds::NETS);
    assert!(list.entries.iter().all(|e| &*e.display_name != "lo"));
    assert_eq!(list.entries.len(), ifaces.len());

    let tailscale = list
        .entries
        .iter()
        .find(|e| &*e.display_name == "tailscale0")
        .expect("tailscale0 listed");
    assert_eq!(
        tailscale.identity,
        DeviceIdentity::Net {
            netns_ino: 4026531840,
            ifindex: 8,
            name: "tailscale0".into(),
        }
    );
    // No `address` file: the name is the key.
    assert_eq!(&*tailscale.persistent_key, "net:tailscale0");
    let eth0 = list
        .entries
        .iter()
        .find(|e| &*e.display_name == "eth0")
        .expect("eth0 listed");
    assert_eq!(&*eth0.persistent_key, "net:aa:bb:cc:dd:ee:01");
}

#[test]
fn enum_sensor_list_skips_gpu_chips() {
    let dir = fixture_copy("fans", None);
    let root = SysRoot::at(&dir);
    let mut chips = Vec::new();
    hwmon::enumerate(&root, &mut chips).expect("fixture chips enumerate");
    let list = sensor_list(SEC, &chips);
    assert_eq!(list.kinds, DeviceKinds::SENSORS | DeviceKinds::FANS);

    let mut nct_fans = Vec::new();
    let mut nct_temps = Vec::new();
    for entry in &list.entries {
        match &entry.identity {
            DeviceIdentity::Fan { chip, index } => {
                assert_ne!(&**chip, "amdgpu");
                if &**chip == "nct6797" {
                    nct_fans.push(*index);
                }
            }
            DeviceIdentity::Sensor { chip, label } => {
                assert_ne!(&**chip, "amdgpu");
                if &**chip == "nct6797" {
                    nct_temps.push(label.to_string());
                }
            }
            other => panic!("unexpected identity {other:?}"),
        }
    }
    assert_eq!(nct_fans, [1, 2]);
    assert_eq!(nct_temps, ["temp1", "temp2"]);
    let fan1 = list
        .entries
        .iter()
        .find(|e| {
            e.identity
                == DeviceIdentity::Fan {
                    chip: "nct6797".into(),
                    index: 1,
                }
        })
        .expect("nct6797 fan1 listed");
    assert_eq!(&*fan1.display_name, "nct6797 fan1");
    assert!(fan1.persistent_key.starts_with("fan:nct6797:"));
    assert!(fan1.persistent_key.ends_with(":1"));
}
