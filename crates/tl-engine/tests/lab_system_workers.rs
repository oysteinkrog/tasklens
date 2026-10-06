//! Tests for the E12 system workers' sources, on copies of the tl-collect fixtures.
//!
//! Time is injected through each source's `sample_at`, so these are plain `#[test]`s. The
//! demand tests run `source::run` in a `FakeLab`.

#[path = "fake_sources.rs"]
mod fake_sources;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use asupersync::Cx;
use asupersync::channel::mpsc;
use asupersync::time::{TimeSource, VirtualClock};
use fake_sources::{FakeLab, channels, drain, io};
use tl_collect::SysRoot;
use tl_collect::counters::Clock;
use tl_collect::parse::{block, hwmon, netif};
use tl_engine::config::EngineMode;
use tl_engine::source::{self, Source, WorkerCommand};
use tl_engine::update::{
    CpuSample, DeviceKinds, DeviceList, DiskSample, EngineMsg, MemorySample, NetSample,
    SensorSample, SourceUpdate,
};
use tl_engine::workers::cpu::CpuSource;
use tl_engine::workers::disk::DiskSource;
use tl_engine::workers::enumerate::{disk_list, net_list, sensor_list};
use tl_engine::workers::memory::MemorySource;
use tl_engine::workers::net::NetSource;
use tl_engine::workers::sensors::SensorsSource;
use tl_engine::workers::workers_for;
use tl_model::demand::{CpuDemand, Demand};
use tl_model::devices::DeviceIdentity;
use tl_model::health::SourceId;
use tl_model::settings::EngineSettings;
use tl_model::system::{FanControlMode, Metric, SensorKind, SensorReading, ZramTotals};
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

/// Writes `proc/stat` with one CPU and these jiffies.
fn write_stat(dir: &Path, user: u64, system: u64, idle: u64) {
    let line = format!("{user} 0 {system} {idle} 10 0 0 0 0 0");
    let stat = format!(
        "cpu  {line}\ncpu0 {line}\nctxt 100\nbtime 1700000000\nprocesses 50\n\
         procs_running 1\nprocs_blocked 0\n"
    );
    fs::write(dir.join("proc/stat"), stat).expect("write proc/stat");
}

/// Runs the lab for `total` in 10 ms steps. Returns every `Cpu` message with the time in ms
/// it was received, including those already queued.
fn cpu_messages(
    fl: &mut FakeLab,
    rx: &mut mpsc::Receiver<EngineMsg>,
    total: Duration,
) -> Vec<(u64, CpuSample)> {
    let end = fl.now_ms() + total.as_millis() as u64;
    let mut out = Vec::new();
    loop {
        for msg in drain(rx) {
            if let EngineMsg::Source(SourceUpdate::Cpu(sample)) = msg {
                out.push((fl.now_ms(), sample));
            }
        }
        if fl.now_ms() >= end {
            return out;
        }
        fl.advance(Duration::from_millis(10));
    }
}

fn times(messages: &[(u64, CpuSample)]) -> Vec<u64> {
    messages.iter().map(|(t, _)| *t).collect()
}

/// The loop reads demand once per turn, before it sleeps, so the sample already scheduled at a
/// change still runs at the old rate (as in `lab_demand.rs`).
#[test]
fn cpu_rates_follow_demand() {
    let dir = fixture_copy("cpu-math", Some(0));
    let mut fl = FakeLab::new();
    let (updates, mut rx, demand_tx, settings, control) = channels();
    demand_tx
        .send(Demand {
            cpu: CpuDemand::Total,
            cpu_fast: true,
            ..Demand::default()
        })
        .expect("watch has receivers");
    let worker_io = io(&updates, &demand_tx, &settings, &control);
    let src = cpu_source(&dir);
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        let _ = source::run(cx, worker_io, src).await;
    });

    let fast = times(&cpu_messages(
        &mut fl,
        &mut rx,
        Duration::from_millis(1_950),
    ));
    let expected: Vec<u64> = (0..20).map(|k| k * 100).collect();
    assert_eq!(fast, expected);

    // 1 Hz. The 2000 ms sample was already scheduled at 10 Hz; the new rate starts with it.
    let changed = fl.now_ms();
    demand_tx
        .send(demand(CpuDemand::Total))
        .expect("watch has receivers");
    let slow = times(&cpu_messages(
        &mut fl,
        &mut rx,
        Duration::from_millis(3_050),
    ));
    assert_eq!(slow, [2_000, 2_000, 3_000, 4_000, 5_000]);
    assert!(
        slow[1] - changed <= 500,
        "new rate {} ms late",
        slow[1] - changed
    );
    assert!(
        slow[1..].windows(2).all(|w| w[1] - w[0] == 1_000),
        "{slow:?}"
    );

    // Off. The 6000 ms sample was already scheduled; nothing after it for 60 s.
    demand_tx
        .send(demand(CpuDemand::Off))
        .expect("watch has receivers");
    let off = times(&cpu_messages(&mut fl, &mut rx, Duration::from_secs(61)));
    assert_eq!(off, [6_000]);

    handle.abort();
    fl.finish();
}

#[test]
fn cpu_recording_keeps_1hz() {
    let dir = fixture_copy("cpu-math", Some(0));
    let mut fl = FakeLab::new();
    let (updates, mut rx, demand_tx, settings, control) = channels();
    demand_tx
        .send(Demand {
            cpu: CpuDemand::Off,
            recording: true,
            ..Demand::default()
        })
        .expect("watch has receivers");
    let worker_io = io(&updates, &demand_tx, &settings, &control);
    let src = cpu_source(&dir);
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        let _ = source::run(cx, worker_io, src).await;
    });

    let got = times(&cpu_messages(
        &mut fl,
        &mut rx,
        Duration::from_millis(9_990),
    ));
    let expected: Vec<u64> = (0..10).map(|k| k * 1_000).collect();
    assert_eq!(got, expected);

    handle.abort();
    fl.finish();
}

#[test]
fn cpu_reset_generation_warms_up() {
    let dir = fixture_copy("cpu-math", Some(0));
    let mut fl = FakeLab::new();
    let (updates, mut rx, demand_tx, settings, control) = channels();
    demand_tx
        .send(demand(CpuDemand::Total))
        .expect("watch has receivers");
    let worker_io = io(&updates, &demand_tx, &settings, &control);
    let src = cpu_source(&dir);
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        let _ = source::run(cx, worker_io, src).await;
    });

    let first = cpu_messages(&mut fl, &mut rx, Duration::ZERO);
    assert_eq!(times(&first), [0]);
    assert_eq!(first[0].1.totals.util_pct.validity, Validity::WarmingUp);
    apply_step(&dir, "cpu-math", 1);
    let second = cpu_messages(&mut fl, &mut rx, Duration::from_secs(1));
    assert_eq!(times(&second), [1_000]);
    assert_eq!(second[0].1.totals.util_pct, Metric::valid(70.0));

    // The counters move again, so only a reset can make the next sample `WarmingUp`.
    write_stat(&dir, 200, 90, 860);
    control
        .send_modify(|c| c.reset_generation += 1)
        .expect("watch has receivers");
    let after_reset = cpu_messages(&mut fl, &mut rx, Duration::from_secs(1));
    assert_eq!(times(&after_reset), [2_000]);
    assert_eq!(
        after_reset[0].1.totals.util_pct.validity,
        Validity::WarmingUp
    );

    write_stat(&dir, 250, 110, 890);
    let next = cpu_messages(&mut fl, &mut rx, Duration::from_secs(1));
    assert_eq!(times(&next), [3_000]);
    assert_eq!(next[0].1.totals.util_pct, Metric::valid(70.0));

    handle.abort();
    fl.finish();
}

/// Reads the live host: run on the owner machine with `--ignored`.
#[test]
#[ignore]
fn cpu_real_host_two_samples() {
    let src = CpuSource::new(SysRoot::host()).expect("host topology reads");
    src.period(&demand(CpuDemand::PerCore), &EngineSettings::default());
    let (src, first) = match src.sample() {
        (src, SourceUpdate::Cpu(sample)) => (src, sample),
        (_, other) => panic!("expected SourceUpdate::Cpu, got {other:?}"),
    };
    assert!(first.topology.is_some());
    std::thread::sleep(Duration::from_secs(1));
    let second = match src.sample() {
        (_, SourceUpdate::Cpu(sample)) => sample,
        (_, other) => panic!("expected SourceUpdate::Cpu, got {other:?}"),
    };
    let util = second.totals.util_pct;
    assert_eq!(util.validity, Validity::Valid);
    assert!((0.0..=100.0).contains(&util.value), "{util:?}");
    let cpus = std::thread::available_parallelism()
        .expect("available parallelism")
        .get();
    assert_eq!(second.per_cpu.len(), cpus);
}

#[test]
fn cpu_listed_once_in_full_mode() {
    let full = workers_for(EngineMode::Full);
    let cpu: Vec<&str> = full
        .iter()
        .filter(|spec| spec.id == SourceId::Cpu)
        .map(|spec| spec.name)
        .collect();
    assert_eq!(cpu, ["cpu"]);
    assert!(
        workers_for(EngineMode::LowMemory)
            .iter()
            .all(|spec| spec.id != SourceId::Cpu)
    );
}

#[test]
fn cpu_live_stats_owner_desktop() {
    let dir = fixture_copy("owner-desktop", None);
    let (_, s) = cpu_sample(cpu_source(&dir), SEC);
    // The fixture holds 50 pid directories.
    assert_eq!(s.totals.processes, Metric::valid(50));
    // file-nr "26143 0 2097152": field 1.
    assert_eq!(s.totals.handles, Metric::valid(26143));
    // loadavg "31.35 27.73 27.82 19/6667 3047020".
    assert_eq!(s.totals.load_avg, Metric::valid([31.35, 27.73, 27.82]));
    // The reducer fills Threads.
    assert_eq!(s.totals.threads.validity, Validity::WarmingUp);
}

#[test]
fn cpu_missing_loadavg_is_missing() {
    let dir = fixture_copy("owner-desktop", None);
    fs::remove_file(dir.join("proc/loadavg")).expect("remove loadavg");
    let (_, s) = cpu_sample(cpu_source(&dir), SEC);
    assert_eq!(s.totals.load_avg.validity, Validity::Missing);
    assert_eq!(s.totals.processes, Metric::valid(50));
    assert_eq!(s.totals.handles, Metric::valid(26143));
}

#[test]
fn cpu_live_stats_only_on_slow_ticks() {
    let dir = fixture_copy("owner-desktop", None);
    let src = cpu_source(&dir);
    let fast = Demand {
        cpu: CpuDemand::Total,
        cpu_fast: true,
        ..Demand::default()
    };
    assert_eq!(
        src.period(&fast, &EngineSettings::default()),
        Some(Duration::from_millis(100))
    );
    let (mut src, first) = cpu_sample(src, SEC);

    // New file values and one more pid directory. Only a read of the live stats sees them.
    fs::write(dir.join("proc/loadavg"), "1.00 2.00 3.00 1/100 42\n").expect("write loadavg");
    fs::write(dir.join("proc/sys/fs/file-nr"), "999\t0\t2097152\n").expect("write file-nr");
    fs::create_dir(dir.join("proc/99999")).expect("add pid dir");

    // Samples 2 to 10, 100 ms apart, repeat the first sample's values.
    for n in 1..10 {
        let (next, s) = cpu_sample(src, SEC + n * 100 * MS);
        src = next;
        assert_eq!(
            s.totals.processes,
            first.totals.processes,
            "sample {}",
            n + 1
        );
        assert_eq!(s.totals.handles, first.totals.handles, "sample {}", n + 1);
        assert_eq!(s.totals.load_avg, first.totals.load_avg, "sample {}", n + 1);
    }

    // The eleventh sample, 1 s after the first, reads them again.
    let (_, s) = cpu_sample(src, 2 * SEC);
    assert_eq!(s.totals.processes, Metric::valid(51));
    assert_eq!(s.totals.handles, Metric::valid(999));
    assert_eq!(s.totals.load_avg, Metric::valid([1.0, 2.0, 3.0]));
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

fn memory_sample(src: MemorySource, t: u64) -> (MemorySource, MemorySample) {
    match src.sample_at(at(t)) {
        (src, SourceUpdate::Memory(sample)) => (src, sample),
        (_, other) => panic!("expected SourceUpdate::Memory, got {other:?}"),
    }
}

/// `mem-composition` without its zram device. Returns the removed device's fixture path.
fn memory_without_zram() -> (PathBuf, PathBuf) {
    let dir = fixture_copy("mem-composition", None);
    fs::remove_dir_all(dir.join("sys/block/zram0")).expect("remove fixture zram0");
    (dir, fixtures().join("mem-composition/sys/block/zram0"))
}

#[test]
fn memory_composition_fixture_totals() {
    let dir = fixture_copy("mem-composition", None);
    let (_, s) = memory_sample(MemorySource::new(SysRoot::at(&dir)), SEC);
    let t = &s.totals;
    assert_eq!(s.validity, Validity::Valid);
    assert_eq!(t.validity, Validity::Valid);
    assert_eq!(t.used, 8_589_934_592);
    assert_eq!(t.committed, 21_474_836_480);
    assert_eq!(t.commit_limit, 17_179_869_184);
    assert_eq!(t.swap_total - t.swap_free, 1_073_741_824);
    assert_eq!(
        t.zram,
        Metric::valid(ZramTotals {
            orig_data: 1_048_576,
            compr_data: 262_144,
            mem_used: 300_000,
        })
    );
}

#[test]
fn memory_zram_stats() {
    let dir = fixture_copy("mem-composition", None);
    let (_, s) = memory_sample(MemorySource::new(SysRoot::at(&dir)), SEC);
    assert_eq!(s.zram.len(), 1);
    let z = &s.zram[0];
    assert_eq!(&*z.name, "zram0");
    assert_eq!(z.disksize, 8_589_934_592);
    assert_eq!(z.orig_bytes, 1_048_576);
    assert_eq!(z.compr_bytes, 262_144);
    assert_eq!(z.mem_used_total, 300_000);
    // First sample: the rates warm up and read as 0.
    assert_eq!(z.read_bps, 0.0);
    assert_eq!(z.write_bps, 0.0);
}

#[test]
fn memory_no_zram_unsupported() {
    let (dir, _) = memory_without_zram();
    let (_, s) = memory_sample(MemorySource::new(SysRoot::at(&dir)), SEC);
    assert_eq!(s.totals.zram.validity, Validity::Unsupported);
    assert!(s.zram.is_empty());
    assert_eq!(s.totals.validity, Validity::Valid);
}

#[test]
fn memory_zram_rescan_every_5s() {
    let (dir, zram0) = memory_without_zram();
    let (src, s) = memory_sample(MemorySource::new(SysRoot::at(&dir)), 0);
    assert!(s.zram.is_empty());

    copy_tree(&zram0, &dir.join("sys/block/zram0"));
    let (src, s) = memory_sample(src, SEC);
    assert!(s.zram.is_empty(), "no rescan before 5 s");
    assert_eq!(s.totals.zram.validity, Validity::Unsupported);

    let (_, s) = memory_sample(src, 5 * SEC);
    let names: Vec<&str> = s.zram.iter().map(|z| &*z.name).collect();
    assert_eq!(names, ["zram0"]);
    assert_eq!(s.totals.zram.validity, Validity::Valid);
}

#[test]
fn memory_listed_once_in_full_mode() {
    let ids: Vec<SourceId> = workers_for(EngineMode::Full)
        .iter()
        .map(|spec| spec.id)
        .collect();
    assert_eq!(ids.iter().filter(|id| **id == SourceId::Memory).count(), 1);
}

#[test]
fn memory_demand_off_then_on() {
    let dir = fixture_copy("mem-composition", None);
    let mut fl = FakeLab::new();
    let (updates, mut rx, demand, settings, control) = channels();
    let worker_io = io(&updates, &demand, &settings, &control);
    let src = MemorySource::new(SysRoot::at(&dir));
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        let _ = source::run(cx, worker_io, src).await;
    });

    // Receive times in ms of the `Memory` messages, read after every 10 ms step.
    let mut times = Vec::new();
    let mut run = |fl: &mut FakeLab, total: Duration, times: &mut Vec<u64>| {
        let end = fl.now_ms() + total.as_millis() as u64;
        while fl.now_ms() < end {
            fl.advance(Duration::from_millis(10));
            for msg in drain(&mut rx) {
                if matches!(msg, EngineMsg::Source(SourceUpdate::Memory(_))) {
                    times.push(fl.now_ms());
                }
            }
        }
    };

    run(&mut fl, Duration::from_secs(60), &mut times);
    assert!(times.is_empty(), "sampled while off: {times:?}");

    demand
        .send(Demand {
            memory: true,
            ..Demand::default()
        })
        .expect("watch has receivers");
    run(&mut fl, Duration::from_secs(3), &mut times);
    assert_eq!(times.len(), 3, "{times:?}");
    assert!(times.windows(2).all(|w| w[1] - w[0] == 1_000), "{times:?}");

    handle.abort();
    fl.finish();
}

/// `disk-two-steps` step 0 plus `sys/block/nvme0n1` with `dev` 259:0.
fn disk_fixture() -> PathBuf {
    let dir = fixture_copy("disk-two-steps", Some(0));
    add_nvme0n1(&dir);
    dir
}

fn add_nvme0n1(dir: &Path) {
    let nvme = dir.join("sys/block/nvme0n1");
    fs::create_dir_all(&nvme).expect("create fixture nvme0n1");
    fs::write(nvme.join("dev"), "259:0\n").expect("write nvme0n1 dev");
    fs::write(nvme.join("size"), "1000215216\n").expect("write nvme0n1 size");
}

fn disk_sample(src: DiskSource, t: u64) -> (DiskSource, DiskSample) {
    match src.sample_at(at(t)) {
        (src, SourceUpdate::Disks(sample)) => (src, sample),
        (_, other) => panic!("expected SourceUpdate::Disks, got {other:?}"),
    }
}

fn disk_devices(src: &mut DiskSource) -> DeviceList {
    match src.take_extra() {
        Some(SourceUpdate::Devices(list)) => list,
        other => panic!("expected SourceUpdate::Devices, got {other:?}"),
    }
}

fn names(list: &DeviceList) -> Vec<&str> {
    list.entries.iter().map(|e| &*e.display_name).collect()
}

fn nvme0n1() -> DeviceIdentity {
    DeviceIdentity::Disk {
        major: 259,
        minor: 0,
        name: "nvme0n1".into(),
        wwn_or_serial: None,
    }
}

#[test]
fn disk_first_sample_and_device_list() {
    let dir = disk_fixture();
    let (mut src, s) = disk_sample(DiskSource::new(SysRoot::at(&dir)), SEC);
    let list = disk_devices(&mut src);
    assert_eq!(list.kinds, DeviceKinds::DISKS);
    assert_eq!(names(&list), ["nvme0n1"]);

    assert_eq!(s.t_ns, SEC);
    assert_eq!(s.disks.len(), 1);
    let (identity, totals) = &s.disks[0];
    assert_eq!(*identity, nvme0n1());
    assert_eq!(totals.active_pct.validity, Validity::WarmingUp);
    assert_eq!(totals.read_bps.validity, Validity::WarmingUp);
    // 100000 sectors of 512 bytes.
    assert_eq!(totals.read_total, 51_200_000);
    assert_eq!(totals.written_total, 102_400_000);
    assert_eq!(totals.temp_c.validity, Validity::Unsupported);
}

#[test]
fn disk_rates_step1() {
    let dir = disk_fixture();
    let (mut src, _) = disk_sample(DiskSource::new(SysRoot::at(&dir)), SEC);
    disk_devices(&mut src);

    apply_step(&dir, "disk-two-steps", 1);
    let (mut src, s) = disk_sample(src, 2 * SEC);
    let (identity, totals) = &s.disks[0];
    assert_eq!(*identity, nvme0n1());
    assert_eq!(totals.active_pct, Metric::valid(50.0));
    assert_eq!(totals.read_bps, Metric::valid(1_048_576.0));
    assert_eq!(totals.write_bps, Metric::valid(2_097_152.0));
    assert_eq!(totals.response_ms, Metric::valid(3.0));
    assert_eq!(totals.capacity, 1_000_215_216 * 512);
    assert!(src.take_extra().is_none(), "no enumeration due");
}

#[test]
fn disk_unplug_and_replug_generation() {
    let dir = disk_fixture();
    let (mut src, _) = disk_sample(DiskSource::new(SysRoot::at(&dir)), SEC);
    disk_devices(&mut src);
    let gens: Vec<u32> = src.disk_ids().map(|id| id.generation).collect();
    assert_eq!(gens, [0]);

    fs::remove_dir_all(dir.join("sys/block/nvme0n1")).expect("remove fixture nvme0n1");
    let (mut src, s) = disk_sample(src, 6 * SEC);
    let list = disk_devices(&mut src);
    assert!(names(&list).is_empty(), "{:?}", names(&list));
    assert!(s.disks.is_empty());

    add_nvme0n1(&dir);
    let (mut src, s) = disk_sample(src, 11 * SEC);
    assert_eq!(names(&disk_devices(&mut src)), ["nvme0n1"]);
    let gens: Vec<u32> = src.disk_ids().map(|id| id.generation).collect();
    assert_eq!(gens, [1]);
    let (identity, totals) = &s.disks[0];
    assert_eq!(*identity, nvme0n1());
    assert_eq!(totals.active_pct.validity, Validity::WarmingUp);
    assert_eq!(totals.read_bps.validity, Validity::WarmingUp);
}

#[test]
fn disk_reset_forces_enumeration() {
    let dir = disk_fixture();
    let (mut src, _) = disk_sample(DiskSource::new(SysRoot::at(&dir)), SEC);
    disk_devices(&mut src);
    apply_step(&dir, "disk-two-steps", 1);

    src.reset_baselines();
    let (mut src, s) = disk_sample(src, 2 * SEC);
    assert_eq!(names(&disk_devices(&mut src)), ["nvme0n1"]);
    assert_eq!(s.disks[0].1.active_pct.validity, Validity::WarmingUp);
    assert_eq!(s.disks[0].1.read_bps.validity, Validity::WarmingUp);
}

#[test]
fn disk_listed_once_in_full_mode() {
    let ids: Vec<SourceId> = workers_for(EngineMode::Full)
        .iter()
        .map(|spec| spec.id)
        .collect();
    assert_eq!(ids.iter().filter(|id| **id == SourceId::Disk).count(), 1);
}

/// `net-two-steps` step 0 plus `proc/self/ns/net` linking to `net:[4026531840]`.
fn net_fixture() -> PathBuf {
    let dir = fixture_copy("net-two-steps", Some(0));
    let ns = dir.join("proc/self/ns");
    fs::create_dir_all(&ns).expect("create fixture ns dir");
    std::os::unix::fs::symlink("net:[4026531840]", ns.join("net")).expect("create netns link");
    dir
}

fn net_sample(src: NetSource, t: u64) -> (NetSource, NetSample) {
    match src.sample_at(at(t)) {
        (src, SourceUpdate::Nets(sample)) => (src, sample),
        (_, other) => panic!("expected SourceUpdate::Nets, got {other:?}"),
    }
}

fn net_devices(src: &mut NetSource) -> DeviceList {
    match src.take_extra() {
        Some(SourceUpdate::Devices(list)) => list,
        other => panic!("expected SourceUpdate::Devices, got {other:?}"),
    }
}

fn net_iface(ifindex: u32, name: &str) -> DeviceIdentity {
    DeviceIdentity::Net {
        netns_ino: 4026531840,
        ifindex,
        name: name.into(),
    }
}

#[test]
fn net_first_sample_and_device_list() {
    let dir = net_fixture();
    let (mut src, s) = net_sample(NetSource::new(SysRoot::at(&dir)), SEC);
    let list = net_devices(&mut src);
    assert_eq!(list.kinds, DeviceKinds::NETS);
    assert_eq!(names(&list), ["eth0"]);

    assert_eq!(s.t_ns, SEC);
    assert_eq!(s.nets.len(), 1);
    let (identity, totals) = &s.nets[0];
    let ifindex = match identity {
        DeviceIdentity::Net { ifindex, .. } => *ifindex,
        other => panic!("expected a Net identity, got {other:?}"),
    };
    assert_eq!(*identity, net_iface(ifindex, "eth0"));
    assert_eq!(totals.rx_bps.validity, Validity::WarmingUp);
    assert_eq!(totals.tx_bps.validity, Validity::WarmingUp);
    assert_eq!(totals.rx_total, 1_000_000);
    assert_eq!(totals.tx_total, 500_000);
    assert_eq!(totals.link_speed_mbps.validity, Validity::WarmingUp);
}

#[test]
fn net_rates_step1() {
    let dir = net_fixture();
    let (mut src, _) = net_sample(NetSource::new(SysRoot::at(&dir)), SEC);
    net_devices(&mut src);

    apply_step(&dir, "net-two-steps", 1);
    let (mut src, s) = net_sample(src, 2 * SEC);
    let (_, totals) = &s.nets[0];
    assert_eq!(totals.rx_bps, Metric::valid(2_500_000.0));
    assert_eq!(totals.tx_bps, Metric::valid(1_000_000.0));
    assert_eq!(totals.rx_total, 3_500_000);
    assert!(src.take_extra().is_none(), "no enumeration due");
}

#[test]
fn net_list_no_loopback() {
    let dir = fixture_copy("netif-kinds", None);
    let (mut src, _) = net_sample(NetSource::new(SysRoot::at(&dir)), SEC);
    let list = net_devices(&mut src);
    assert_eq!(list.kinds, DeviceKinds::NETS);
    let names = names(&list);
    assert!(!names.contains(&"lo"), "{names:?}");
    assert!(names.contains(&"docker0"), "{names:?}");
    assert!(names.contains(&"tailscale0"), "{names:?}");
}

#[test]
fn net_rename_is_new_identity() {
    let dir = net_fixture();
    let (mut src, s) = net_sample(NetSource::new(SysRoot::at(&dir)), SEC);
    net_devices(&mut src);
    let old = s.nets[0].0.clone();
    let ifindex = match &old {
        DeviceIdentity::Net { ifindex, .. } => *ifindex,
        other => panic!("expected a Net identity, got {other:?}"),
    };

    let class = dir.join("sys/class/net");
    fs::rename(class.join("eth0"), class.join("eth1")).expect("rename fixture eth0");
    let (mut src, s) = net_sample(src, 6 * SEC);
    let list = net_devices(&mut src);
    let identities: Vec<&DeviceIdentity> = list.entries.iter().map(|e| &e.identity).collect();
    let new = net_iface(ifindex, "eth1");
    assert!(!identities.contains(&&old), "{identities:?}");
    assert!(identities.contains(&&new), "{identities:?}");
    assert_eq!(s.nets.len(), 1);
    assert_eq!(s.nets[0].0, new);
    let ids: Vec<(&str, u32)> = src
        .iface_ids()
        .map(|id| (&*id.name, id.generation))
        .collect();
    assert_eq!(ids, [("eth1", 0)]);
}

#[test]
fn net_listed_once_in_full_mode() {
    let ids: Vec<SourceId> = workers_for(EngineMode::Full)
        .iter()
        .map(|spec| spec.id)
        .collect();
    assert_eq!(ids.iter().filter(|id| **id == SourceId::Net).count(), 1);
}

fn sensors_sample(src: SensorsSource, t: u64) -> (SensorsSource, SensorSample) {
    match src.sample_at(at(t)) {
        (src, SourceUpdate::Sensors(sample)) => (src, sample),
        (_, other) => panic!("expected SourceUpdate::Sensors, got {other:?}"),
    }
}

fn sensors_devices(src: &mut SensorsSource) -> DeviceList {
    match src.take_extra() {
        Some(SourceUpdate::Devices(list)) => list,
        other => panic!("expected a Devices list, got {other:?}"),
    }
}

fn hwmon_dir(dir: &Path, n: u32) -> PathBuf {
    dir.join(format!("sys/class/hwmon/hwmon{n}"))
}

fn fan(chip: &str, index: u16) -> DeviceIdentity {
    DeviceIdentity::Fan {
        chip: chip.into(),
        index,
    }
}

fn temp(chip: &str, label: &str) -> DeviceIdentity {
    DeviceIdentity::Sensor {
        chip: chip.into(),
        label: label.into(),
    }
}

fn reading<'a>(s: &'a SensorSample, id: &DeviceIdentity) -> &'a SensorReading {
    &s.readings
        .iter()
        .find(|(identity, _)| identity == id)
        .unwrap_or_else(|| panic!("{id:?} missing from the sample"))
        .1
}

fn chip_of(id: &DeviceIdentity) -> &str {
    match id {
        DeviceIdentity::Sensor { chip, .. } | DeviceIdentity::Fan { chip, .. } => chip,
        other => panic!("unexpected identity {other:?}"),
    }
}

#[test]
fn sensors_fans_fixture() {
    let dir = fixture_copy("fans", None);
    let (mut src, s) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);
    let list = sensors_devices(&mut src);
    assert_eq!(list.kinds, DeviceKinds::SENSORS | DeviceKinds::FANS);
    assert!(
        list.entries
            .iter()
            .all(|e| chip_of(&e.identity) != "amdgpu")
    );
    assert!(s.readings.iter().all(|(id, _)| chip_of(id) != "amdgpu"));

    let fan1 = reading(&s, &fan("nct6797", 1));
    assert_eq!(fan1.kind, SensorKind::Fan);
    assert_eq!(fan1.value, Metric::valid(1200.0));
    assert_eq!(fan1.pwm_pct, Metric::valid(50.19608));
    assert_eq!(fan1.fan_control, Some(FanControlMode::Automatic));
    assert!(fan1.fan_connected);
    // Every reading's identity is in the device list (same identity rule as enumerate).
    for (id, _) in &s.readings {
        assert!(list.entries.iter().any(|e| &e.identity == id), "{id:?}");
    }
}

#[test]
fn sensors_temps_fixture() {
    let dir = fixture_copy("fans", None);
    let (_, s) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);
    for (id, celsius) in [
        (temp("nct6797", "temp1"), 40.0),
        (temp("nct6797", "temp2"), 45.0),
        (temp("single", "temp1"), 35.0),
    ] {
        let r = reading(&s, &id);
        assert_eq!(r.kind, SensorKind::Temperature, "{id:?}");
        assert_eq!(r.value, Metric::valid(celsius), "{id:?}");
        assert_eq!(r.pwm_pct.validity, Validity::Unsupported, "{id:?}");
        assert_eq!(r.fan_control, None, "{id:?}");
        assert!(!r.fan_connected, "{id:?}");
    }
}

#[test]
fn sensors_unreadable_restricted() {
    use std::os::unix::fs::PermissionsExt;

    let dir = fixture_copy("fans", None);
    let file = hwmon_dir(&dir, 0).join("temp2_input");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).expect("chmod temp2_input");
    if fs::read(&file).is_ok() {
        eprintln!("skipped: running as root, mode 000 does not block reads");
        return;
    }
    let (_, s) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);
    assert_eq!(
        reading(&s, &temp("nct6797", "temp2")).value.validity,
        Validity::Restricted
    );
    assert_eq!(
        reading(&s, &temp("nct6797", "temp1")).value,
        Metric::valid(40.0)
    );
    assert_eq!(
        reading(&s, &fan("nct6797", 1)).value.validity,
        Validity::Valid
    );
}

#[test]
fn sensors_rescan_every_5s() {
    let dir = fixture_copy("fans", None);
    let (mut src, _) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), 0);
    sensors_devices(&mut src);

    let extra = hwmon_dir(&dir, 3);
    copy_tree(&hwmon_dir(&dir, 2), &extra);
    fs::write(extra.join("name"), "extra\n").expect("write new chip name");
    let new_temp = temp("extra", "temp1");

    let (mut src, s) = sensors_sample(src, SEC);
    assert!(src.take_extra().is_none(), "no rescan before 5 s");
    assert!(s.readings.iter().all(|(id, _)| chip_of(id) != "extra"));

    let (mut src, s) = sensors_sample(src, 5 * SEC);
    let list = sensors_devices(&mut src);
    assert!(list.entries.iter().any(|e| e.identity == new_temp));
    assert!(list.entries.iter().any(|e| e.identity == fan("extra", 1)));
    assert_eq!(reading(&s, &new_temp).value, Metric::valid(35.0));
    assert_eq!(reading(&s, &fan("extra", 1)).value, Metric::valid(900.0));
}

#[test]
fn sensors_fan_connected_sticky() {
    let dir = fixture_copy("fans", None);
    let input = hwmon_dir(&dir, 2).join("fan1_input");
    let single = fan("single", 1);
    fs::write(&input, "0\n").expect("write fan1_input");
    let (src, s) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);
    assert!(!reading(&s, &single).fan_connected);

    fs::write(&input, "900\n").expect("write fan1_input");
    let (src, s) = sensors_sample(src, 2 * SEC);
    assert!(reading(&s, &single).fan_connected);

    fs::write(&input, "0\n").expect("write fan1_input");
    let (_, s) = sensors_sample(src, 3 * SEC);
    let r = reading(&s, &single);
    assert_eq!(r.value, Metric::valid(0.0));
    assert!(r.fan_connected);
}

#[test]
fn sensors_fan_label_connected() {
    let dir = fixture_copy("fans", None);
    let (_, s) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);
    assert!(!reading(&s, &fan("nct6797", 2)).fan_connected);

    let dir = fixture_copy("fans", None);
    fs::write(hwmon_dir(&dir, 0).join("fan2_label"), "SYS_FAN\n").expect("write fan2_label");
    let (_, s) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);
    let r = reading(&s, &fan("nct6797", 2));
    assert_eq!(r.value, Metric::valid(0.0));
    assert!(r.fan_connected);
}

#[test]
fn sensors_listed_once_in_full_mode() {
    let ids: Vec<SourceId> = workers_for(EngineMode::Full)
        .iter()
        .map(|spec| spec.id)
        .collect();
    assert_eq!(ids.iter().filter(|id| **id == SourceId::Sensors).count(), 1);
}

#[test]
fn sensors_fan_control_mode() {
    let dir = fixture_copy("fans", None);
    let (_, s) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);
    assert_eq!(
        reading(&s, &fan("nct6797", 1)).fan_control,
        Some(FanControlMode::Automatic)
    );
    assert_eq!(reading(&s, &fan("nct6797", 2)).fan_control, None);
    for (id, r) in &s.readings {
        if r.kind == SensorKind::Temperature {
            assert_eq!(r.fan_control, None, "{id:?}");
        }
    }
}

/// Writes only the fixture copy under the target tmpdir, never a real hwmon file.
#[test]
fn sensors_fan_control_changes() {
    let dir = fixture_copy("fans", None);
    let enable = hwmon_dir(&dir, 0).join("pwm1_enable");
    let fan1 = fan("nct6797", 1);
    let (src, _) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);

    fs::write(&enable, "1\n").expect("write fixture pwm1_enable");
    let (src, s) = sensors_sample(src, 2 * SEC);
    assert_eq!(reading(&s, &fan1).fan_control, Some(FanControlMode::Manual));

    fs::write(&enable, "0\n").expect("write fixture pwm1_enable");
    let (_, s) = sensors_sample(src, 3 * SEC);
    assert_eq!(
        reading(&s, &fan1).fan_control,
        Some(FanControlMode::FullSpeed)
    );
}

/// The injected-clock entry point the four P1 sources share.
trait SampleAt: Source + Sized {
    fn sample_at(self, clock: Clock) -> (Self, SourceUpdate);
}

impl SampleAt for MemorySource {
    fn sample_at(self, clock: Clock) -> (Self, SourceUpdate) {
        MemorySource::sample_at(self, clock)
    }
}

impl SampleAt for DiskSource {
    fn sample_at(self, clock: Clock) -> (Self, SourceUpdate) {
        DiskSource::sample_at(self, clock)
    }
}

impl SampleAt for NetSource {
    fn sample_at(self, clock: Clock) -> (Self, SourceUpdate) {
        NetSource::sample_at(self, clock)
    }
}

impl SampleAt for SensorsSource {
    fn sample_at(self, clock: Clock) -> (Self, SourceUpdate) {
        SensorsSource::sample_at(self, clock)
    }
}

/// A real source whose `sample` reads the lab's virtual clock instead of the host clock, so the
/// 5 s enumeration follows virtual time. Everything else is the source's own.
struct OnLabClock<S> {
    inner: S,
    clock: Arc<VirtualClock>,
}

impl<S: SampleAt> Source for OnLabClock<S> {
    fn id(&self) -> SourceId {
        self.inner.id()
    }

    fn period(&self, demand: &Demand, settings: &EngineSettings) -> Option<Duration> {
        self.inner.period(demand, settings)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        let t = self.clock.now().as_nanos();
        let (inner, update) = self.inner.sample_at(at(t));
        let clock = self.clock;
        (OnLabClock { inner, clock }, update)
    }

    fn reset_baselines(&mut self) {
        self.inner.reset_baselines();
    }

    fn command(&mut self, cmd: &WorkerCommand) -> bool {
        self.inner.command(cmd)
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.inner.take_extra()
    }
}

/// Starts `src` under `source::run` in `fl`, on the lab clock.
fn spawn_on_lab<S: SampleAt>(
    fl: &mut FakeLab,
    worker_io: tl_engine::source::WorkerIo,
    src: S,
) -> asupersync::runtime::TaskHandle<()> {
    let src = OnLabClock {
        inner: src,
        clock: Arc::clone(&fl.clock),
    };
    fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        let _ = source::run(cx, worker_io, src).await;
    })
}

/// The message kinds the P1 tests count. Device lists are named after the source that sends
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum P1 {
    Memory,
    Disks,
    Nets,
    Sensors,
    DiskList,
    NetList,
    SensorList,
    OtherList,
}

impl P1 {
    /// The sample a list is sent before.
    fn sample_of(self) -> Option<P1> {
        match self {
            P1::DiskList => Some(P1::Disks),
            P1::NetList => Some(P1::Nets),
            P1::SensorList => Some(P1::Sensors),
            _ => None,
        }
    }
}

/// Receive time in ms, kind and list time in ns (lists only).
type P1Msg = (u64, P1, Option<u64>);

fn p1_kind(msg: &EngineMsg) -> Option<(P1, Option<u64>)> {
    let EngineMsg::Source(update) = msg else {
        return None;
    };
    match update {
        SourceUpdate::Memory(_) => Some((P1::Memory, None)),
        SourceUpdate::Disks(_) => Some((P1::Disks, None)),
        SourceUpdate::Nets(_) => Some((P1::Nets, None)),
        SourceUpdate::Sensors(_) => Some((P1::Sensors, None)),
        SourceUpdate::Devices(list) => {
            let kind = if list.kinds == DeviceKinds::DISKS {
                P1::DiskList
            } else if list.kinds == DeviceKinds::NETS {
                P1::NetList
            } else if list.kinds == DeviceKinds::SENSORS | DeviceKinds::FANS {
                P1::SensorList
            } else {
                P1::OtherList
            };
            Some((kind, Some(list.t_ns)))
        }
        _ => None,
    }
}

/// Runs `Memory`, `Disk`, `Net` and `Sensors` on one `owner-desktop` copy under `demand` for
/// `total` in 10 ms steps, then checks the lab's oracles. Returns the P1 messages in arrival
/// order, including those sent at t = 0.
fn run_p1(demand: Demand, total: Duration) -> Vec<P1Msg> {
    let dir = fixture_copy("owner-desktop", None);
    let mut fl = FakeLab::new();
    let (updates, mut rx, demand_tx, settings, control) = channels();
    demand_tx.send(demand).expect("watch has receivers");
    let new_io = || io(&updates, &demand_tx, &settings, &control);
    let handles = [
        spawn_on_lab(&mut fl, new_io(), MemorySource::new(SysRoot::at(&dir))),
        spawn_on_lab(&mut fl, new_io(), DiskSource::new(SysRoot::at(&dir))),
        spawn_on_lab(&mut fl, new_io(), NetSource::new(SysRoot::at(&dir))),
        spawn_on_lab(&mut fl, new_io(), SensorsSource::new(SysRoot::at(&dir))),
    ];

    let end = fl.now_ms() + total.as_millis() as u64;
    let mut out = Vec::new();
    loop {
        for msg in drain(&mut rx) {
            if let Some((kind, list_ns)) = p1_kind(&msg) {
                out.push((fl.now_ms(), kind, list_ns));
            }
        }
        if fl.now_ms() >= end {
            break;
        }
        fl.advance(Duration::from_millis(10));
    }

    for handle in &handles {
        handle.abort();
    }
    fl.finish();
    out
}

fn p1_times(msgs: &[P1Msg], kind: P1) -> Vec<u64> {
    msgs.iter().filter(|m| m.1 == kind).map(|m| m.0).collect()
}

/// ADR-008: hidden with recording off produces zero `sample()` calls over 60 virtual s.
#[test]
fn p1_sources_hidden_zero_samples() {
    let msgs = run_p1(Demand::default(), Duration::from_secs(60));
    assert!(msgs.is_empty(), "{msgs:?}");
}

#[test]
fn p1_sources_recording_policy() {
    let recording = Demand {
        recording: true,
        ..Demand::default()
    };
    let msgs = run_p1(recording, Duration::from_millis(9_990));
    let every_second: Vec<u64> = (0..10).map(|k| k * 1_000).collect();
    for kind in [P1::Memory, P1::Disks, P1::Nets] {
        assert_eq!(p1_times(&msgs, kind), every_second, "{kind:?}");
    }
    assert!(p1_times(&msgs, P1::Sensors).is_empty());
    assert!(p1_times(&msgs, P1::SensorList).is_empty());
}

#[test]
fn p1_sources_visible_rates_and_enumeration() {
    let visible = Demand {
        memory: true,
        disks: true,
        nets: true,
        sensors: true,
        ..Demand::default()
    };
    let msgs = run_p1(visible, Duration::from_millis(10_990));
    let every_second: Vec<u64> = (0..11).map(|k| k * 1_000).collect();
    for kind in [P1::Memory, P1::Disks, P1::Nets, P1::Sensors] {
        assert_eq!(p1_times(&msgs, kind), every_second, "{kind:?}");
    }
    assert!(p1_times(&msgs, P1::OtherList).is_empty());

    for list in [P1::DiskList, P1::NetList, P1::SensorList] {
        assert_eq!(p1_times(&msgs, list), [0, 5_000, 10_000], "{list:?}");
        let sample = list.sample_of().expect("a list kind");
        for (i, &(t, kind, list_ns)) in msgs.iter().enumerate() {
            if kind != list {
                continue;
            }
            assert_eq!(list_ns, Some(t * MS), "{list:?} t_ns");
            // The same tick's sample comes after the list, not before it.
            let same_tick = msgs
                .iter()
                .position(|m| m.0 == t && m.1 == sample)
                .unwrap_or_else(|| panic!("no {sample:?} at {t} ms"));
            assert!(i < same_tick, "{list:?} at {t} ms after its sample");
        }
    }
}

/// `fixture/sys/block` disks and their partitions, as the kernel names them.
fn fixture_disk_names(dir: &Path) -> Vec<String> {
    let block_dir = dir.join("sys/block");
    let mut names = Vec::new();
    for entry in fs::read_dir(&block_dir).expect("list fixture sys/block") {
        let name = entry
            .expect("fixture sys/block entry")
            .file_name()
            .into_string()
            .expect("UTF-8 disk name");
        if block::EXCLUDED_PREFIXES.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        for child in fs::read_dir(block_dir.join(&name)).expect("list fixture disk dir") {
            let child = child
                .expect("fixture disk dir entry")
                .file_name()
                .into_string()
                .expect("UTF-8 child name");
            if child.starts_with(&name) {
                names.push(child);
            }
        }
        names.push(name);
    }
    names.sort();
    names
}

#[test]
fn p1_devices_lists_are_complete() {
    let dir = fixture_copy("owner-desktop", None);

    let (mut src, _) = disk_sample(DiskSource::new(SysRoot::at(&dir)), SEC);
    let list = disk_devices(&mut src);
    assert_eq!(list.kinds, DeviceKinds::DISKS);
    let mut listed: Vec<String> = names(&list).iter().map(|n| n.to_string()).collect();
    listed.sort();
    let expected = fixture_disk_names(&dir);
    assert!(expected.contains(&"nvme0n1p4".to_string()), "{expected:?}");
    assert_eq!(listed, expected);
    for entry in &list.entries {
        let is_child = !matches!(&*entry.display_name, "nvme0n1" | "nvme1n1" | "sda");
        assert_eq!(entry.parent.is_some(), is_child, "{}", entry.display_name);
    }

    let (mut src, _) = net_sample(NetSource::new(SysRoot::at(&dir)), SEC);
    let list = net_devices(&mut src);
    assert_eq!(list.kinds, DeviceKinds::NETS);
    let listed = names(&list);
    assert!(!listed.contains(&"lo"), "{listed:?}");
    assert!(listed.contains(&"enp38s0"), "{listed:?}");
    assert!(listed.contains(&"wlan0"), "{listed:?}");

    let (mut src, _) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);
    let list = sensors_devices(&mut src);
    assert_eq!(list.kinds, DeviceKinds::SENSORS | DeviceKinds::FANS);
    let chips: Vec<&str> = list.entries.iter().map(|e| chip_of(&e.identity)).collect();
    assert!(chips.contains(&"k10temp"), "{chips:?}");
    assert!(chips.contains(&"nct6797"), "{chips:?}");
    assert!(
        chips.iter().all(|c| !hwmon::GPU_HWMON_NAMES.contains(c)),
        "{chips:?}"
    );
}

#[test]
fn p1_sources_listed_once_in_full_mode() {
    let full: Vec<SourceId> = workers_for(EngineMode::Full)
        .iter()
        .map(|spec| spec.id)
        .collect();
    let low: Vec<SourceId> = workers_for(EngineMode::LowMemory)
        .iter()
        .map(|spec| spec.id)
        .collect();
    for id in [
        SourceId::Cpu,
        SourceId::Memory,
        SourceId::Disk,
        SourceId::Net,
        SourceId::Sensors,
        SourceId::Logind,
    ] {
        assert_eq!(full.iter().filter(|f| **f == id).count(), 1, "{id:?}");
        assert!(!low.contains(&id), "{id:?}");
    }
}

/// Reads the live host: run on the owner machine with `--ignored`. Prints counts only.
#[test]
#[ignore]
fn p1_sources_real_host() {
    fn twice<S: Source>(src: S) -> SourceUpdate {
        let (src, _) = src.sample();
        std::thread::sleep(Duration::from_secs(1));
        src.sample().1
    }

    let SourceUpdate::Memory(memory) = twice(MemorySource::new(SysRoot::host())) else {
        panic!("expected SourceUpdate::Memory");
    };
    assert!(memory.totals.total > 0);

    let SourceUpdate::Disks(disks) = twice(DiskSource::new(SysRoot::host())) else {
        panic!("expected SourceUpdate::Disks");
    };
    assert!(disks.disks.iter().any(|(_, d)| d.read_total > 0));

    let SourceUpdate::Nets(nets) = twice(NetSource::new(SysRoot::host())) else {
        panic!("expected SourceUpdate::Nets");
    };
    let not_lo = nets
        .nets
        .iter()
        .filter(|(id, _)| !matches!(id, DeviceIdentity::Net { name, .. } if &**name == "lo"))
        .count();
    assert!(not_lo > 0);

    let SourceUpdate::Sensors(sensors) = twice(SensorsSource::new(SysRoot::host())) else {
        panic!("expected SourceUpdate::Sensors");
    };
    let valid_temps = sensors
        .readings
        .iter()
        .filter(|(_, r)| r.kind == SensorKind::Temperature && r.value.validity == Validity::Valid)
        .count();
    let has_k10temp = fs::read_dir("/sys/class/hwmon")
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| fs::read_to_string(e.path().join("name")).is_ok_and(|n| n.trim() == "k10temp"));
    if has_k10temp {
        assert!(valid_temps > 0);
    }
    eprintln!(
        "disks {} interfaces {not_lo} readings {} valid temperatures {valid_temps}",
        disks.disks.len(),
        sensors.readings.len()
    );
}
