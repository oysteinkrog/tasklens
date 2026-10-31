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
use tl_collect::sample::disk::FsStat;
use tl_engine::config::EngineMode;
use tl_engine::source::{self, Source, WorkerCommand};
use tl_engine::update::{
    CpuSample, DeviceDetailsUpdate, DeviceKinds, DeviceList, DiskSample, EngineMsg, MemorySample,
    NetSample, SensorSample, SourceUpdate,
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
use tl_model::series::SampleFlags;
use tl_model::settings::EngineSettings;
use tl_model::snapshot::DiskDetails;
use tl_model::system::{FanControlMode, GpuEngine, Metric, SensorKind, SensorReading, ZramTotals};
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
    // Threads is the loadavg total (tasklens-9o0.36).
    assert_eq!(s.totals.threads, Metric::valid(6667));
}

#[test]
fn cpu_missing_loadavg_is_missing() {
    let dir = fixture_copy("owner-desktop", None);
    fs::remove_file(dir.join("proc/loadavg")).expect("remove loadavg");
    let (_, s) = cpu_sample(cpu_source(&dir), SEC);
    assert_eq!(s.totals.load_avg.validity, Validity::Missing);
    // Missing, never a zero count.
    assert_eq!(s.totals.threads.validity, Validity::Missing);
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

/// `owner-desktop` with the `pressure` fixture's /proc/pressure and /proc/vmstat on top.
fn pressure_fixture(base: &str) -> PathBuf {
    let dir = fixture_copy(base, None);
    copy_tree(&fixtures().join("pressure"), &dir);
    dir
}

/// tasklens-hjn.47: PSI_CPU_SOME is read on slow ticks while CPU or device history is demanded,
/// and not otherwise.
#[test]
fn cpu_pressure_follows_demand() {
    let dir = pressure_fixture("owner-desktop");
    let s = EngineSettings::default();
    let src = cpu_source(&dir);
    src.period(&demand(CpuDemand::Total), &s);
    let (src, sample) = cpu_sample(src, SEC);
    assert_eq!(sample.totals.psi_some_pct, Metric::valid(10.66));
    // CPU_LOAD1 comes from the load average the live stats read.
    assert_eq!(sample.totals.load_avg.value[0], 31.35);

    // The process table alone keeps the worker at 1 Hz but does not read pressure.
    let table = Demand {
        process_table: true,
        ..Demand::default()
    };
    src.period(&table, &s);
    let (src, sample) = cpu_sample(src, 2 * SEC);
    assert_eq!(sample.totals.psi_some_pct.validity, Validity::WarmingUp);

    // Device history alone reads it (ADR-016 "System pressure").
    let history = Demand {
        device_history: true,
        ..Demand::default()
    };
    assert_eq!(src.period(&history, &s), Some(Duration::from_secs(1)));
    let (_, sample) = cpu_sample(src, 3 * SEC);
    assert_eq!(sample.totals.psi_some_pct, Metric::valid(10.66));
}

/// A kernel without PSI has no /proc/pressure: the PSI values are Unsupported.
#[test]
fn cpu_pressure_unsupported_without_psi() {
    let dir = fixture_copy("owner-desktop", None);
    let src = cpu_source(&dir);
    src.period(&demand(CpuDemand::Total), &EngineSettings::default());
    let (_, sample) = cpu_sample(src, SEC);
    assert_eq!(sample.totals.psi_some_pct.validity, Validity::Unsupported);
}

/// tasklens-hjn.47: memory PSI and the swap-in rate are read while memory or device history is
/// demanded, and the swap-in counter flags reach the sample.
#[test]
fn memory_pressure_follows_demand() {
    let dir = pressure_fixture("mem-composition");
    let s = EngineSettings::default();
    let memory = Demand {
        memory: true,
        ..Demand::default()
    };
    let src = MemorySource::new(SysRoot::at(&dir));
    src.period(&memory, &s);
    let (src, sample) = memory_sample(src, SEC);
    let t = &sample.totals;
    assert_eq!(t.psi_some_pct, Metric::valid(6.25));
    assert_eq!(t.psi_full_pct, Metric::valid(2.5));
    assert_eq!(t.swap_in_pps.validity, Validity::WarmingUp);

    // 9_689_207 + 1_000 pages over 2 s.
    let vmstat = "pswpin 9690207\npswpout 12000000\n";
    fs::write(dir.join("proc/vmstat"), vmstat).expect("write vmstat");
    let (src, sample) = memory_sample(src, 3 * SEC);
    assert_eq!(sample.totals.swap_in_pps, Metric::valid(500.0));

    // Recording alone keeps the worker at 1 Hz but does not read pressure.
    let recording = Demand {
        recording: true,
        ..Demand::default()
    };
    src.period(&recording, &s);
    let (src, sample) = memory_sample(src, 4 * SEC);
    assert_eq!(sample.totals.psi_some_pct.validity, Validity::WarmingUp);
    assert_eq!(sample.totals.swap_in_pps.validity, Validity::WarmingUp);

    // Demanded again, the swap-in rate warms up instead of spanning the unread time.
    src.period(&memory, &s);
    let (src, sample) = memory_sample(src, 5 * SEC);
    assert_eq!(sample.totals.psi_some_pct, Metric::valid(6.25));
    assert_eq!(sample.totals.swap_in_pps.validity, Validity::WarmingUp);
    assert_eq!(sample.swap_in_flags, SampleFlags::GAP_BEFORE);

    // Device history alone reads pressure (ADR-016 "System pressure"). A pswpin decrease is a
    // counter reset (ADR-008).
    let history = Demand {
        device_history: true,
        ..Demand::default()
    };
    assert_eq!(src.period(&history, &s), Some(Duration::from_secs(1)));
    fs::write(dir.join("proc/vmstat"), "pswpin 10\npswpout 12000000\n").expect("write vmstat");
    let (_, sample) = memory_sample(src, 6 * SEC);
    assert_eq!(sample.totals.psi_full_pct, Metric::valid(2.5));
    assert_eq!(sample.totals.swap_in_pps.validity, Validity::WarmingUp);
    assert_eq!(sample.swap_in_flags, SampleFlags::RESET);
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

/// A zram device whose `mm_stat` cannot be parsed gives no zram totals, not a valid 0.
#[test]
fn memory_zram_failed_mm_stat_not_valid() {
    let dir = fixture_copy("mem-composition", None);
    fs::write(dir.join("sys/block/zram0/mm_stat"), "1 2\n").expect("write mm_stat");
    let (_, s) = memory_sample(MemorySource::new(SysRoot::at(&dir)), SEC);
    assert_eq!(s.zram.len(), 1);
    assert_eq!(s.totals.zram.validity, Validity::Failed);
    assert_eq!(s.totals.validity, Validity::Valid);
}

/// Two zram devices: one unreadable `mm_stat` makes the totals unavailable, not a partial sum.
#[test]
fn memory_zram_partial_mm_stat_unavailable() {
    let dir = fixture_copy("mem-composition", None);
    copy_tree(&dir.join("sys/block/zram0"), &dir.join("sys/block/zram1"));
    let (src, s) = memory_sample(MemorySource::new(SysRoot::at(&dir)), SEC);
    assert_eq!(s.zram.len(), 2);
    assert_eq!(s.totals.zram.validity, Validity::Valid);
    assert_eq!(
        s.totals.zram.value.orig_data,
        2 * 1_048_576,
        "both reads ok give the sum"
    );

    fs::write(dir.join("sys/block/zram1/mm_stat"), "1 2\n").expect("write mm_stat");
    let (_, s) = memory_sample(src, 2 * SEC);
    assert_eq!(s.zram.len(), 2);
    assert_eq!(s.totals.zram.validity, Validity::Failed);
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
    // The fixture has no `speed` file (E12-014).
    assert_eq!(totals.link_speed_mbps.validity, Validity::Missing);
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

fn fan_ids(list: &DeviceList) -> Vec<DeviceIdentity> {
    list.entries
        .iter()
        .filter(|e| matches!(e.identity, DeviceIdentity::Fan { .. }))
        .map(|e| e.identity.clone())
        .collect()
}

/// tasklens-hjn.45: a plain hwmon tree with fan1..fan3 gives three listed, connected fans.
#[test]
fn sensors_three_fan_tree() {
    let dir = fixture_copy("no-fans", None);
    let chip = hwmon_dir(&dir, 9);
    fs::create_dir_all(&chip).expect("create hwmon9");
    fs::write(chip.join("name"), "nct6775\n").expect("write name");
    for (n, rpm) in [(1, 1500), (2, 900), (3, 2100)] {
        fs::write(chip.join(format!("fan{n}_input")), format!("{rpm}\n")).expect("write fan");
    }
    let (mut src, s) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);
    let want: Vec<DeviceIdentity> = (1..=3).map(|n| fan("nct6775", n)).collect();
    assert_eq!(fan_ids(&sensors_devices(&mut src)), want);
    for (id, rpm) in want.iter().zip([1500., 900., 2100.]) {
        let r = reading(&s, id);
        assert_eq!(r.value, Metric::valid(rpm));
        assert!(r.fan_connected, "{id:?}");
    }
}

/// tasklens-hjn.45: the owner machine's nct6797 (captured 2026-10) lists all seven fans; the
/// five that spin are connected, the two at 0 RPM are not.
#[test]
fn sensors_owner_desktop_seven_fans() {
    let dir = fixture_copy("owner-desktop", None);
    let (mut src, s) = sensors_sample(SensorsSource::new(SysRoot::at(&dir)), SEC);
    let nct: Vec<DeviceIdentity> = fan_ids(&sensors_devices(&mut src))
        .into_iter()
        .filter(|id| chip_of(id) == "nct6797")
        .collect();
    assert_eq!(nct, (1..=7).map(|n| fan("nct6797", n)).collect::<Vec<_>>());
    let connected: Vec<u16> = (1..=7)
        .filter(|&n| reading(&s, &fan("nct6797", n)).fan_connected)
        .collect();
    assert_eq!(connected, [1, 2, 3, 4, 6]);
}

/// Runs the lab for `span` and returns the times (ms) of the device lists and sensor samples.
fn sensors_messages(
    fl: &mut FakeLab,
    rx: &mut mpsc::Receiver<EngineMsg>,
    span: Duration,
) -> (Vec<u64>, Vec<u64>) {
    let end = fl.now_ms() + span.as_millis() as u64;
    let (mut lists, mut samples) = (Vec::new(), Vec::new());
    loop {
        for msg in drain(rx) {
            match msg {
                EngineMsg::Source(SourceUpdate::Devices(list)) => {
                    assert_eq!(list.kinds, DeviceKinds::SENSORS | DeviceKinds::FANS);
                    lists.push(fl.now_ms());
                }
                EngineMsg::Source(SourceUpdate::Sensors(_)) => samples.push(fl.now_ms()),
                _ => {}
            }
        }
        if fl.now_ms() >= end {
            return (lists, samples);
        }
        fl.advance(Duration::from_millis(10));
    }
}

/// tasklens-hjn.45: with no demand the loop lists the chips at start and every 30 s and reads
/// no value. A demand change starts the 1 Hz values at once, not at the next 30 s tick.
#[test]
fn sensors_list_without_demand_then_sample_on_demand() {
    let dir = fixture_copy("fans", None);
    let mut fl = FakeLab::new();
    let (updates, mut rx, demand_tx, settings, control) = channels();
    let worker_io = io(&updates, &demand_tx, &settings, &control);
    let src = SensorsSource::new(SysRoot::at(&dir));
    let handle = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        let _ = source::run(cx, worker_io, src).await;
    });

    let (lists, samples) = sensors_messages(&mut fl, &mut rx, Duration::from_millis(61_000));
    assert_eq!(lists, [0, 30_000, 60_000]);
    assert!(samples.is_empty(), "{samples:?}");

    let asked = fl.now_ms();
    demand_tx
        .send(Demand {
            sensors: true,
            ..Demand::default()
        })
        .expect("watch has receivers");
    let (lists, samples) = sensors_messages(&mut fl, &mut rx, Duration::from_millis(2_500));
    assert!(samples.len() >= 3, "{samples:?}");
    assert!(
        samples[0] - asked <= 100,
        "first value {} ms late",
        samples[0] - asked
    );
    assert!(
        samples.windows(2).all(|w| w[1] - w[0] == 1_000),
        "{samples:?}"
    );
    assert_eq!(
        lists,
        [samples[0]],
        "the first demanded sample sends the list again"
    );

    handle.abort();
    fl.finish();
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

    /// The sample the source's own `sample` takes under the demand `period` last saw. Only
    /// sensors differ: without demand they list the chips and read no value (tasklens-hjn.45).
    fn sample_demanded(self, clock: Clock, _sensors_demanded: bool) -> (Self, SourceUpdate) {
        self.sample_at(clock)
    }
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

    fn sample_demanded(self, clock: Clock, sensors_demanded: bool) -> (Self, SourceUpdate) {
        if sensors_demanded {
            SensorsSource::sample_at(self, clock)
        } else {
            SensorsSource::discover_at(self, clock)
        }
    }
}

/// A real source whose `sample` reads the lab's virtual clock instead of the host clock, so the
/// 5 s enumeration follows virtual time. Everything else is the source's own.
struct OnLabClock<S> {
    inner: S,
    clock: Arc<VirtualClock>,
    /// `Demand.sensors` as last seen by `period`.
    sensors_demanded: std::cell::Cell<bool>,
}

impl<S: SampleAt> Source for OnLabClock<S> {
    fn id(&self) -> SourceId {
        self.inner.id()
    }

    fn period(&self, demand: &Demand, settings: &EngineSettings) -> Option<Duration> {
        self.sensors_demanded.set(demand.sensors);
        self.inner.period(demand, settings)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        let t = self.clock.now().as_nanos();
        let demanded = self.sensors_demanded.get();
        let (inner, update) = self.inner.sample_demanded(at(t), demanded);
        let clock = self.clock;
        let sensors_demanded = self.sensors_demanded;
        (
            OnLabClock {
                inner,
                clock,
                sensors_demanded,
            },
            update,
        )
    }

    fn wakes_on_demand(&self) -> bool {
        self.inner.wakes_on_demand()
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
        sensors_demanded: std::cell::Cell::new(false),
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

/// ADR-008: hidden with recording off reads no value over 60 virtual s. Sensors still list their
/// chips at start and every 30 s (tasklens-hjn.45).
#[test]
fn p1_sources_hidden_zero_samples() {
    let msgs = run_p1(Demand::default(), Duration::from_secs(60));
    let others: Vec<_> = msgs.iter().filter(|m| m.1 != P1::SensorList).collect();
    assert!(others.is_empty(), "{msgs:?}");
    assert_eq!(p1_times(&msgs, P1::SensorList), [0, 30_000, 60_000]);
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
    // Recording does not demand sensors: only the list at start (tasklens-hjn.45).
    assert_eq!(p1_times(&msgs, P1::SensorList), [0]);
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

/// `cpu-math` step 0 plus a package zone `intel-rapl:0` without the core subzone.
fn cpu_with_rapl(energy_uj: u64) -> PathBuf {
    let dir = fixture_copy("cpu-math", Some(0));
    let zone = dir.join("sys/class/powercap/intel-rapl:0");
    fs::create_dir_all(&zone).expect("create rapl zone");
    fs::write(zone.join("max_energy_range_uj"), "262143328850\n").expect("write max range");
    set_energy_uj(&dir, energy_uj);
    dir
}

/// Rewrites the package zone's counter in place.
fn set_energy_uj(dir: &Path, energy_uj: u64) {
    let file = dir.join("sys/class/powercap/intel-rapl:0/energy_uj");
    fs::write(file, format!("{energy_uj}\n")).expect("write energy_uj");
}

#[test]
fn cpu_power_watts() {
    let dir = cpu_with_rapl(1_000_000);
    let (src, first) = cpu_sample(cpu_source(&dir), SEC);
    assert_eq!(first.totals.package_power_w.validity, Validity::WarmingUp);

    set_energy_uj(&dir, 6_000_000);
    let (_, s) = cpu_sample(src, 2 * SEC);
    assert_eq!(s.totals.package_power_w, Metric::valid(5.0));
    assert_eq!(s.totals.core_power_w.validity, Validity::Unsupported);
}

#[test]
fn cpu_power_restricted() {
    use std::os::unix::fs::PermissionsExt;

    let dir = cpu_with_rapl(1_000_000);
    let file = dir.join("sys/class/powercap/intel-rapl:0/energy_uj");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).expect("chmod energy_uj");
    if fs::read(&file).is_ok() {
        eprintln!("skipped: running as root, mode 000 does not block reads");
        return;
    }
    let (src, first) = cpu_sample(cpu_source(&dir), SEC);
    assert_eq!(first.totals.package_power_w.validity, Validity::Restricted);
    let (_, s) = cpu_sample(src, 2 * SEC);
    assert_eq!(s.totals.package_power_w.validity, Validity::Restricted);
}

#[test]
fn cpu_power_no_zone() {
    let dir = fixture_copy("cpu-math", Some(0));
    assert!(!dir.join("sys/class/powercap").exists());
    let (src, _) = cpu_sample(cpu_source(&dir), SEC);
    let (_, s) = cpu_sample(src, 2 * SEC);
    assert_eq!(s.totals.package_power_w.validity, Validity::Unsupported);
    assert_eq!(s.totals.core_power_w.validity, Validity::Unsupported);
}

#[test]
fn cpu_power_only_on_slow_ticks() {
    let dir = cpu_with_rapl(1_000_000);
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
    let (src, _) = cpu_sample(src, SEC);
    set_energy_uj(&dir, 6_000_000);
    let (mut src, second) = cpu_sample(src, 2 * SEC);
    assert_eq!(second.totals.package_power_w, Metric::valid(5.0));

    // Fast ticks between slow ticks repeat the last value, even though the counter moves.
    set_energy_uj(&dir, 16_000_000);
    for n in 1..10 {
        let (next, s) = cpu_sample(src, 2 * SEC + n * 100 * MS);
        src = next;
        assert_eq!(
            s.totals.package_power_w, second.totals.package_power_w,
            "sample at +{n}00 ms"
        );
    }

    // The next slow tick, 1 s after the last one, reads 10 J over 1 s.
    let (_, s) = cpu_sample(src, 3 * SEC);
    assert_eq!(s.totals.package_power_w, Metric::valid(10.0));
}

/// E12-009: the process table alone keeps `Cpu` and `Memory` at 1 Hz for the pseudo rows, and
/// `Cpu` carries no per-core data.
#[test]
fn pseudo_rows_keep_cpu_and_memory_at_1hz() {
    let cpu_dir = fixture_copy("cpu-math", Some(0));
    let mem_dir = fixture_copy("mem-composition", None);
    let mut fl = FakeLab::new();
    let (updates, mut rx, demand_tx, settings, control) = channels();
    demand_tx
        .send(Demand {
            process_table: true,
            ..Demand::default()
        })
        .expect("watch has receivers");
    let cpu_io = io(&updates, &demand_tx, &settings, &control);
    let mem_io = io(&updates, &demand_tx, &settings, &control);
    let cpu = cpu_source(&cpu_dir);
    let memory = MemorySource::new(SysRoot::at(&mem_dir));
    let handles = [
        fl.spawn(async move {
            let cx = Cx::current().expect("task has a Cx");
            let _ = source::run(cx, cpu_io, cpu).await;
        }),
        fl.spawn(async move {
            let cx = Cx::current().expect("task has a Cx");
            let _ = source::run(cx, mem_io, memory).await;
        }),
    ];

    // 5 s: samples at 0, 1, 2, 3 and 4 s.
    let end = fl.now_ms() + 4_990;
    let mut cpu_times = Vec::new();
    let mut memory_times = Vec::new();
    loop {
        for msg in drain(&mut rx) {
            match msg {
                EngineMsg::Source(SourceUpdate::Cpu(sample)) => {
                    assert!(
                        sample.per_cpu.is_empty(),
                        "per-core data at {}",
                        fl.now_ms()
                    );
                    cpu_times.push(fl.now_ms());
                }
                EngineMsg::Source(SourceUpdate::Memory(_)) => memory_times.push(fl.now_ms()),
                _ => {}
            }
        }
        if fl.now_ms() >= end {
            break;
        }
        fl.advance(Duration::from_millis(10));
    }
    let every_second: Vec<u64> = (0..5).map(|k| k * 1_000).collect();
    assert_eq!(cpu_times, every_second);
    assert_eq!(memory_times, every_second);

    for handle in &handles {
        handle.abort();
    }
    fl.finish();
}

// E12-011: memory details.

fn memory_demand() -> Demand {
    Demand {
        memory: true,
        ..Demand::default()
    }
}

/// The `DeviceDetails::Memory` update queued by the last sample, if any.
fn memory_details(src: &mut MemorySource) -> Option<Arc<tl_model::snapshot::MemoryDetails>> {
    use tl_engine::update::DeviceDetailsUpdate;
    let extra = src.take_extra()?;
    assert!(src.take_extra().is_none(), "one details update per sample");
    match extra {
        SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Memory(d)) => Some(d),
        other => panic!("expected DeviceDetails::Memory, got {other:?}"),
    }
}

fn owner_desktop_swap0() -> tl_model::snapshot::SwapDevice {
    tl_model::snapshot::SwapDevice {
        name: Arc::from("/dev/zram0"),
        kind: tl_model::snapshot::SwapKindModel::Zram,
        size_bytes: 67_331_158_016,
        used_bytes: 31_138_131_968,
        priority: 100,
    }
}

#[test]
fn memory_details_first_sample() {
    use tl_model::snapshot::{ZramDetail, ZswapInfo};
    let dir = fixture_copy("owner-desktop", None);
    let src = MemorySource::new(SysRoot::at(&dir));
    src.period(&memory_demand(), &EngineSettings::default());
    let (mut src, _) = memory_sample(src, 0);
    let d = memory_details(&mut src).expect("details on the first sample");

    assert_eq!(d.swaps.validity, Validity::Valid);
    assert_eq!(&*d.swaps.value, [owner_desktop_swap0()]);
    assert_eq!(d.zswap.validity, Validity::Valid);
    assert_eq!(
        d.zswap.value,
        Some(ZswapInfo {
            enabled: false,
            compressor: Some(Arc::from("zstd")),
            max_pool_percent: Some(20),
        })
    );
    assert_eq!(
        &*d.zram,
        [ZramDetail {
            name: Arc::from("zram0"),
            disksize: 67_331_162_112,
            algorithm: Some(Arc::from("zstd")),
            stats: Some(ZramTotals {
                orig_data: 29_526_052_864,
                compr_data: 6_871_065_127,
                mem_used: 7_058_923_520,
            }),
        }]
    );
    assert_eq!(src.details_reads(), 1);
}

#[test]
fn memory_details_only_on_change() {
    let dir = fixture_copy("owner-desktop", None);
    let src = MemorySource::new(SysRoot::at(&dir));
    src.period(&memory_demand(), &EngineSettings::default());
    let (mut src, _) = memory_sample(src, 0);
    assert!(memory_details(&mut src).is_some());

    for t in 1..=4 {
        let (next, _) = memory_sample(src, t * SEC);
        src = next;
        assert!(memory_details(&mut src).is_none(), "not due at {t} s");
    }
    assert_eq!(src.details_reads(), 1);

    let (mut src, _) = memory_sample(src, 5 * SEC);
    assert_eq!(src.details_reads(), 2, "due at 5 s");
    assert!(memory_details(&mut src).is_none(), "unchanged at 5 s");
}

#[test]
fn memory_details_swap_added() {
    let dir = fixture_copy("owner-desktop", None);
    let src = MemorySource::new(SysRoot::at(&dir));
    src.period(&memory_demand(), &EngineSettings::default());
    let (mut src, _) = memory_sample(src, 0);
    assert!(memory_details(&mut src).is_some());

    let swaps = dir.join("proc/swaps");
    let mut text = fs::read_to_string(&swaps).expect("read fixture swaps");
    text.push_str("/swapfile                               file\t\t1048576\t\t0\t\t-2\n");
    fs::write(&swaps, text).expect("write fixture swaps");

    let (mut src, _) = memory_sample(src, 5 * SEC);
    let d = memory_details(&mut src).expect("details after the swap change");
    assert_eq!(d.swaps.validity, Validity::Valid);
    assert_eq!(
        &*d.swaps.value,
        [
            owner_desktop_swap0(),
            tl_model::snapshot::SwapDevice {
                name: Arc::from("/swapfile"),
                kind: tl_model::snapshot::SwapKindModel::File,
                size_bytes: 1_073_741_824,
                used_bytes: 0,
                priority: -2,
            },
        ]
    );
}

#[test]
fn memory_details_off_when_recording_only() {
    let dir = fixture_copy("owner-desktop", None);
    let settings = EngineSettings::default();
    let recording = Demand {
        memory: false,
        recording: true,
        ..Demand::default()
    };
    let mut src = MemorySource::new(SysRoot::at(&dir));
    for t in 0..10 {
        assert_eq!(
            src.period(&recording, &settings),
            Some(Duration::from_secs(1))
        );
        let (next, _) = memory_sample(src, t * SEC);
        src = next;
        assert!(memory_details(&mut src).is_none());
    }
    assert_eq!(src.details_reads(), 0);
}

/// `disk-partitions-dm` plus a `proc/diskstats` line for each of its disks and their children.
fn disk_partitions_fixture() -> PathBuf {
    let dir = fixture_copy("disk-partitions-dm", None);
    let mut stats = String::new();
    for (major, minor, name) in [
        (179, 0, "mmcblk0"),
        (259, 0, "nvme0n1"),
        (8, 0, "sda"),
        (8, 1, "sda1"),
        (8, 2, "sda2"),
        (254, 0, "dm-0"),
        (8, 16, "sdb"),
        (11, 0, "sr0"),
        (253, 0, "vda"),
    ] {
        stats.push_str(&format!(
            "{major:4} {minor:7} {name} 1000 10 100000 2000 500 20 200000 1000 0 10000 3000 0 0 0 0 0 0\n"
        ));
    }
    fs::write(dir.join("proc/diskstats"), stats).expect("write fixture diskstats");
    dir
}

fn disk_named(name: &str, major: u32, minor: u32) -> DeviceIdentity {
    DeviceIdentity::Disk {
        major,
        minor,
        name: name.into(),
        wwn_or_serial: None,
    }
}

/// Every `statvfs` call answers 1000 blocks of 4096 bytes, 400 free, 300 available.
fn fake_statvfs_source(dir: &Path) -> DiskSource {
    DiskSource::with_statvfs(
        SysRoot::at(dir),
        Box::new(|_: &str| {
            Ok(FsStat {
                blocks: 1000,
                bfree: 400,
                bavail: 300,
                frsize: 4096,
            })
        }),
    )
}

/// The `Devices` list, then the disk details, from the last enumeration.
fn disk_details(src: &mut DiskSource) -> Vec<(DeviceIdentity, DiskDetails)> {
    disk_devices(src);
    match src.take_extra() {
        Some(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks { from, entries })) => {
            assert_eq!(from, SourceId::Disk);
            entries
        }
        other => panic!("expected DeviceDetails::Disks, got {other:?}"),
    }
}

fn details_of<'a>(entries: &'a [(DeviceIdentity, DiskDetails)], name: &str) -> &'a DiskDetails {
    entries
        .iter()
        .find(|(id, _)| matches!(id, DeviceIdentity::Disk { name: n, .. } if &**n == name))
        .map(|(_, d)| d)
        .unwrap_or_else(|| panic!("no details for {name}"))
}

#[test]
fn disk_temperatures() {
    let dir = disk_partitions_fixture();
    let (_, s) = disk_sample(DiskSource::new(SysRoot::at(&dir)), SEC);
    let temp = |id: DeviceIdentity| {
        s.disks
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, t)| t.temp_c)
            .unwrap_or_else(|| panic!("no sample for {id:?}"))
    };
    assert_eq!(temp(nvme0n1()), Metric::valid(41.85));
    assert_eq!(temp(disk_named("sda", 8, 0)), Metric::valid(35.0));
    assert_eq!(
        temp(disk_named("vda", 253, 0)).validity,
        Validity::Unsupported
    );
    // Children never get a temperature input.
    assert_eq!(
        temp(disk_named("sda1", 8, 1)).validity,
        Validity::Unsupported
    );
}

/// Disk temperatures may be unreadable without root: that is `Restricted`, never 0.
#[test]
fn disk_temperature_unreadable_restricted() {
    let dir = disk_partitions_fixture();
    let input = block::disk_temp_input(&SysRoot::at(&dir), "sda").expect("sda has a temp input");
    fs::set_permissions(&input, std::os::unix::fs::PermissionsExt::from_mode(0o000))
        .expect("chmod sda temp input");
    if fs::read(&input).is_ok() {
        eprintln!("running as root: the unreadable case cannot be checked");
        return;
    }
    let (_, s) = disk_sample(DiskSource::new(SysRoot::at(&dir)), SEC);
    let (_, sda) = s
        .disks
        .iter()
        .find(|(i, _)| *i == disk_named("sda", 8, 0))
        .expect("sda sample");
    assert_eq!(sda.temp_c.validity, Validity::Restricted);
}

#[test]
fn disk_usage_details() {
    let dir = disk_partitions_fixture();
    let (mut src, _) = disk_sample(fake_statvfs_source(&dir), SEC);
    let entries = disk_details(&mut src);

    let sda = details_of(&entries, "sda");
    // `/` on dm-0 and `/boot` on sda1, each 1000 blocks of 4096 bytes. The second sda1 mount
    // is counted once.
    assert_eq!(sda.formatted_bytes, valid(8_192_000));
    assert_eq!(sda.used_bytes, valid(4_915_200));
    assert_eq!(sda.free_bytes, valid(2_457_600));
    assert_eq!(sda.system_disk, valid(true));
    assert_eq!(sda.swap, valid(false));

    let nvme = details_of(&entries, "nvme0n1");
    assert_eq!(&*nvme.kind, "NVMe");
    assert_eq!(nvme.formatted_bytes.validity, Validity::Unsupported);
    assert_eq!(nvme.used_bytes.validity, Validity::Unsupported);
    assert_eq!(nvme.free_bytes.validity, Validity::Unsupported);
    assert_eq!(nvme.system_disk, valid(false));
    assert_eq!(&*details_of(&entries, "vda").kind, "Virtual");
    // Whole disks only.
    assert!(
        entries
            .iter()
            .all(|(id, _)| !matches!(id, DeviceIdentity::Disk { name, .. } if &**name == "sda1"))
    );
}

#[test]
fn disk_usage_statvfs_error() {
    let dir = disk_partitions_fixture();
    let src = DiskSource::with_statvfs(
        SysRoot::at(&dir),
        Box::new(|_: &str| Err(tl_collect::CollectError::PermissionDenied)),
    );
    let (mut src, _) = disk_sample(src, SEC);
    let entries = disk_details(&mut src);
    let sda = details_of(&entries, "sda");
    assert_eq!(sda.formatted_bytes.validity, Validity::Restricted);
    assert_eq!(sda.used_bytes.validity, Validity::Restricted);
    assert_eq!(sda.free_bytes.validity, Validity::Restricted);
    assert_eq!(sda.system_disk, valid(true));
}

#[test]
fn disk_usage_no_mount_table_failed() {
    let dir = disk_partitions_fixture();
    fs::remove_file(dir.join("proc/self/mountinfo")).expect("remove fixture mountinfo");
    let (mut src, _) = disk_sample(fake_statvfs_source(&dir), SEC);
    let entries = disk_details(&mut src);
    let sda = details_of(&entries, "sda");
    assert_eq!(sda.system_disk.validity, Validity::Failed);
    assert_eq!(sda.formatted_bytes.validity, Validity::Missing);
    // No `proc/swaps` in the fixture: a kernel without swap, so no disk holds swap.
    assert_eq!(sda.swap, valid(false));
}

#[test]
fn disk_swap_flag() {
    let dir = disk_partitions_fixture();
    fs::write(
        dir.join("proc/swaps"),
        "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n\
         /dev/sda2                               partition\t8388604\t\t0\t\t-2\n",
    )
    .expect("write fixture swaps");
    let (mut src, _) = disk_sample(fake_statvfs_source(&dir), SEC);
    let entries = disk_details(&mut src);
    assert_eq!(details_of(&entries, "sda").swap, valid(true));
    assert_eq!(details_of(&entries, "nvme0n1").swap, valid(false));
}

#[test]
fn disk_update_order() {
    let dir = disk_partitions_fixture();
    let mut fl = FakeLab::new();
    let (updates, mut rx, demand_tx, settings, control) = channels();
    demand_tx
        .send(Demand {
            disks: true,
            ..Demand::default()
        })
        .expect("watch has receivers");
    let handle = spawn_on_lab(
        &mut fl,
        io(&updates, &demand_tx, &settings, &control),
        fake_statvfs_source(&dir),
    );

    let mut kinds: Vec<&str> = Vec::new();
    for _ in 0..50 {
        for msg in drain(&mut rx) {
            let EngineMsg::Source(update) = msg else {
                continue;
            };
            kinds.push(match update {
                SourceUpdate::Devices(_) => "devices",
                SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Disks { .. }) => "details",
                SourceUpdate::Disks(_) => "sample",
                _ => "other",
            });
        }
        if kinds.len() >= 3 {
            break;
        }
        fl.advance(Duration::from_millis(10));
    }
    handle.abort();
    fl.finish();
    assert_eq!(kinds[..3], ["devices", "details", "sample"], "{kinds:?}");
}

/// Reads the live host: run on the owner machine with `--ignored`. Prints counts only, never
/// mount points.
#[test]
#[ignore]
fn disk_usage_real_host() {
    let (mut src, _) = disk_sample(DiskSource::new(SysRoot::host()), SEC);
    let entries = disk_details(&mut src);
    let system: Vec<&DiskDetails> = entries
        .iter()
        .map(|(_, d)| d)
        .filter(|d| d.system_disk == valid(true))
        .collect();
    eprintln!("disks {} system disks {}", entries.len(), system.len());
    assert!(!system.is_empty());
    assert!(
        system
            .iter()
            .any(|d| d.formatted_bytes.validity == Validity::Valid)
    );
}

fn valid<T>(value: T) -> tl_model::snapshot::InvField<T> {
    tl_model::snapshot::InvField {
        value,
        validity: Validity::Valid,
    }
}

// E12-010: CPU static info ------------------------------------------------------------------

use std::sync::{Mutex, PoisonError};

use asupersync::runtime::RuntimeBuilder;
use tl_collect::parse::topology::read_topology;
use tl_engine::dbus::CONNECT_TIMEOUT;
use tl_engine::workers::cpu::{CpuBusFacts, build_static, build_static_in, read_bus_facts};
use tl_model::snapshot::{CpuStaticInfo, InvField, RaplState, VirtState};

/// Held by the tests that connect to the system bus, since one of them changes
/// `DBUS_SYSTEM_BUS_ADDRESS` for the whole test process.
static SYSTEM_BUS_ENV: Mutex<()> = Mutex::new(());

fn bare_metal_facts(ppd_profile: InvField<Arc<str>>) -> CpuBusFacts {
    CpuBusFacts {
        running_in_vm: valid(Arc::from("")),
        ppd_profile,
    }
}

fn unsupported<T: Default>() -> InvField<T> {
    InvField {
        value: T::default(),
        validity: Validity::Unsupported,
    }
}

/// The `CpuStaticInfo` waiting in `take_extra`, if any.
fn take_static(src: &mut CpuSource) -> Option<Arc<CpuStaticInfo>> {
    match src.take_extra() {
        Some(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Cpu(info))) => Some(info),
        None => None,
        Some(other) => panic!("expected DeviceDetails::Cpu, got {other:?}"),
    }
}

/// `build_static` on `dir` with the topology read from it.
fn static_at(dir: &Path, facts: &CpuBusFacts) -> CpuStaticInfo {
    let root = SysRoot::at(dir);
    let topo = read_topology(&root).expect("fixture topology reads");
    build_static(&root, &topo, facts)
}

#[test]
fn cpu_static_owner_desktop() {
    let dir = fixture_copy("owner-desktop", None);
    let topo = read_topology(&SysRoot::at(&dir)).expect("fixture topology reads");
    let mut src = cpu_source(&dir).with_facts(bare_metal_facts(unsupported()));
    assert!(
        take_static(&mut src).is_none(),
        "nothing before the first sample"
    );
    let (mut src, _) = cpu_sample(src, SEC);
    let info = take_static(&mut src).expect("the first sample sends the static info");
    assert_eq!(info.driver, valid(Arc::from("amd-pstate-epp")));
    assert_eq!(info.governor, valid(Arc::from("powersave")));
    assert_eq!(info.epp, valid(Arc::from("balance_performance")));
    assert_eq!(info.cores, topo.cores);
    assert_eq!(info.logical, topo.logical_count);
    assert!(info.caches.l3.is_some());
    assert_eq!(info.running_in_vm, valid(Arc::from("")));
}

#[test]
fn cpu_static_once_per_topology() {
    let dir = fixture_copy("owner-desktop", None);
    let src = cpu_source(&dir).with_facts(bare_metal_facts(unsupported()));
    let (mut src, _) = cpu_sample(src, SEC);
    assert!(take_static(&mut src).is_some());
    let (mut src, _) = cpu_sample(src, 2 * SEC);
    assert!(take_static(&mut src).is_none(), "no hotplug, no resend");
}

#[test]
fn cpu_static_hotplug_resend() {
    let dir = fixture_copy("cpu-hotplug", Some(0));
    let (mut src, _) = cpu_sample(cpu_source(&dir), SEC);
    let before = take_static(&mut src).expect("first sample sends");

    apply_step(&dir, "cpu-hotplug", 1);
    let (mut src, _) = cpu_sample(src, 2 * SEC);
    let after = take_static(&mut src).expect("hotplug resends");
    assert_eq!(after.logical, before.logical - 1);

    let (mut src, _) = cpu_sample(src, 3 * SEC);
    assert!(take_static(&mut src).is_none());
}

#[test]
fn cpu_static_ppd_fallback() {
    let dir = fixture_copy("owner-desktop", None);
    let _ = fs::remove_file(dir.join("sys/firmware/acpi/platform_profile"));
    let info = static_at(&dir, &bare_metal_facts(valid(Arc::from("balanced"))));
    assert_eq!(info.platform_profile, valid(Arc::from("balanced")));
}

#[test]
fn cpu_static_virtualization() {
    let dir = fixture_copy("owner-desktop", None);
    fs::write(
        dir.join("proc/cpuinfo"),
        "processor\t: 0\nmicrocode\t: 0xa201030\nflags\t\t: fpu svm\n\n",
    )
    .expect("write cpuinfo");
    let dev = dir.join("dev");
    fs::create_dir_all(&dev).expect("create dev");
    let root = SysRoot::at(&dir);
    let topo = read_topology(&root).expect("fixture topology reads");
    let info = build_static_in(&root, &topo, &bare_metal_facts(unsupported()), &dev);
    assert_eq!(info.virtualization, valid(VirtState::SupportedNoKvm));
    assert_eq!(info.microcode, valid(Arc::from("0xa201030")));
    assert_eq!(info.hypervisor, None);
}

#[test]
fn cpu_static_no_rapl_zone() {
    let dir = fixture_copy("owner-desktop", None);
    fs::remove_dir_all(dir.join("sys/class/powercap")).expect("remove powercap");
    let info = static_at(&dir, &bare_metal_facts(unsupported()));
    assert_eq!(info.rapl, Some(RaplState::NoZone));
}

#[test]
fn cpu_bus_facts_no_bus_failed() {
    let _env = SYSTEM_BUS_ENV
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let socket = Path::new(env!("CARGO_TARGET_TMPDIR")).join("hjn13-no-system-bus");
    let _ = fs::remove_file(&socket);
    let old = std::env::var_os("DBUS_SYSTEM_BUS_ADDRESS");
    // SAFETY: `SYSTEM_BUS_ENV` is held, and no other test in this binary reads the
    // environment.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var(
            "DBUS_SYSTEM_BUS_ADDRESS",
            format!("unix:path={}", socket.display()),
        );
    }

    let mut fl = FakeLab::new();
    let start = fl.now_ms();
    let mut handle = fl.spawn(async {
        let cx = Cx::current().expect("task has a Cx");
        read_bus_facts(&cx).await
    });
    let limit = start + CONNECT_TIMEOUT.as_millis() as u64;
    while !handle.is_finished() && fl.now_ms() < limit {
        fl.advance(Duration::from_millis(100));
    }
    let facts = handle.try_join();

    // SAFETY: as above.
    #[allow(unsafe_code)]
    unsafe {
        match old {
            Some(old) => std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", old),
            None => std::env::remove_var("DBUS_SYSTEM_BUS_ADDRESS"),
        }
    }

    let facts = facts
        .expect("task joins")
        .expect("read_bus_facts returns within CONNECT_TIMEOUT");
    assert!(fl.now_ms() - start <= CONNECT_TIMEOUT.as_millis() as u64);
    assert_eq!(facts.running_in_vm.validity, Validity::Failed);
    assert_eq!(facts.ppd_profile.validity, Validity::Failed);
    fl.finish();
}

/// Reads the live system bus: run on the owner machine with `--ignored`.
#[test]
#[ignore]
fn cpu_bus_facts_real_host() {
    let _env = SYSTEM_BUS_ENV
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let rt = RuntimeBuilder::new()
        .worker_threads(1)
        .blocking_threads(1, 2)
        .build()
        .expect("runtime builds");
    let (tx, rx) = std::sync::mpsc::channel();
    rt.handle()
        .try_spawn_with_cx(move |cx| async move {
            let _ = tx.send(read_bus_facts(&cx).await);
        })
        .expect("task spawns");
    let facts = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("read_bus_facts finished");
    eprintln!(
        "running_in_vm {:?} ppd_profile {:?}",
        facts.running_in_vm, facts.ppd_profile
    );
    assert_eq!(facts.running_in_vm, valid(Arc::from("")));
}

/// Writes the E12-013 udev files for `sda` and `sda1` into the fixture copy.
fn write_sda_udev(dir: &Path) {
    let data = dir.join("run/udev/data");
    fs::write(
        data.join("b8:1"),
        "E:ID_FS_TYPE=vfat\nE:ID_FS_LABEL=EFI\n\
         E:ID_PART_ENTRY_TYPE=c12a7328-f81f-11d2-ba4b-00a0c93ec93b\nE:ID_PART_ENTRY_FLAGS=0x1\n",
    )
    .expect("write udev b8:1");
    fs::write(
        data.join("b8:0"),
        "E:ID_BUS=ata\nE:ID_WWN=0x50014ee2b5f1c0de\nE:ID_SERIAL_SHORT=WD-TEST01\n\
         E:ID_PART_TABLE_TYPE=gpt\nE:ID_MODEL=WDC_WD10EZEX-08W\n",
    )
    .expect("write udev b8:0");
}

#[test]
fn disk_udev_rows() {
    let dir = disk_partitions_fixture();
    write_sda_udev(&dir);
    let (mut src, _) = disk_sample(fake_statvfs_source(&dir), SEC);
    let entries = disk_details(&mut src);
    let sda = details_of(&entries, "sda");
    assert_eq!(sda.interface, valid(Arc::from("SATA")));
    assert_eq!(sda.wwn, valid(Arc::from("0x50014ee2b5f1c0de")));
    assert_eq!(sda.serial, valid(Arc::from("WD-TEST01")));
    assert_eq!(sda.model, valid(Arc::from("WDC WD10EZEX-08W")));
    // The fixture's sdb file has `ID_BUS=usb` and a model with an underscore.
    let sdb = details_of(&entries, "sdb");
    assert_eq!(sdb.interface, valid(Arc::from("USB")));
    assert_eq!(sdb.model, valid(Arc::from("Flash Disk")));
    assert_eq!(sdb.serial.validity, Validity::Unsupported);
}

#[test]
fn disk_partitions_present() {
    let dir = disk_partitions_fixture();
    write_sda_udev(&dir);
    let (mut src, _) = disk_sample(fake_statvfs_source(&dir), SEC);
    let entries = disk_details(&mut src);
    let table = details_of(&entries, "sda")
        .partitions
        .clone()
        .expect("sda has a partition table");
    assert_eq!(&*table.kind, "GPT");
    let parts: Vec<&str> = table
        .rows
        .iter()
        .filter(|r| !r.unallocated)
        .map(|r| &*r.name)
        .collect();
    assert_eq!(parts, ["sda1", "sda2"]);
}

#[test]
fn disk_no_udev_data() {
    let dir = disk_partitions_fixture();
    fs::remove_file(dir.join("run/udev/data/b8:0")).expect("remove udev b8:0");
    let (mut src, _) = disk_sample(fake_statvfs_source(&dir), SEC);
    let entries = disk_details(&mut src);
    for name in ["sda", "nvme0n1"] {
        let d = details_of(&entries, name);
        assert_eq!(d.interface.validity, Validity::Unsupported, "{name}");
        assert_eq!(d.wwn.validity, Validity::Unsupported, "{name}");
        assert_eq!(d.serial.validity, Validity::Unsupported, "{name}");
    }
    assert_eq!(
        details_of(&entries, "sda").model,
        valid(Arc::from("WDC WD10EZEX-08W"))
    );
    assert_eq!(
        details_of(&entries, "nvme0n1").model.validity,
        Validity::Valid
    );
}

use tl_collect::parse::netif::IfaceKind;
use tl_engine::workers::net::net_kind;
use tl_model::devices::NetKind;
use tl_model::snapshot::NetDetails;
use tl_model::system::LinkState;

/// `netif-kinds` with `proc/net/dev` lines for every interface but `lo`.
fn netif_kinds_fixture() -> PathBuf {
    let dir = fixture_copy("netif-kinds", None);
    let mut dev = String::from(
        "Inter-|   Receive                                                |  Transmit\n \
         face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
    );
    for name in [
        "br0",
        "docker0",
        "dummy0",
        "eth0",
        "tailscale0",
        "usb0",
        "veth12ab",
        "virbr0",
        "wlan0",
        "wwan0",
    ] {
        dev.push_str(&format!(
            "{name:>6}: 1000 10 0 0 0 0 0 0 500 5 0 0 0 0 0 0\n"
        ));
    }
    fs::create_dir_all(dir.join("proc/net")).expect("create fixture proc/net");
    fs::write(dir.join("proc/net/dev"), dev).expect("write fixture proc/net/dev");
    dir
}

fn net_details(src: &mut NetSource) -> Vec<(DeviceIdentity, NetDetails)> {
    match src.take_extra() {
        Some(SourceUpdate::DeviceDetails(DeviceDetailsUpdate::Nets { from, entries })) => {
            assert_eq!(from, SourceId::Net);
            entries
        }
        other => panic!("expected net DeviceDetails, got {other:?}"),
    }
}

fn net_details_of<'a>(entries: &'a [(DeviceIdentity, NetDetails)], name: &str) -> &'a NetDetails {
    entries
        .iter()
        .find(|(id, _)| matches!(id, DeviceIdentity::Net { name: n, .. } if &**n == name))
        .map(|(_, d)| d)
        .unwrap_or_else(|| panic!("no net details for {name}"))
}

#[test]
fn net_link_speed_and_state() {
    let dir = netif_kinds_fixture();
    let (_, s) = net_sample(NetSource::new(SysRoot::at(&dir)), SEC);
    let totals = |name: &str| {
        s.nets
            .iter()
            .find(|(id, _)| matches!(id, DeviceIdentity::Net { name: n, .. } if &**n == name))
            .map(|(_, t)| *t)
            .unwrap_or_else(|| panic!("no totals for {name}"))
    };
    let eth0 = totals("eth0");
    assert_eq!(eth0.link_speed_mbps, Metric::valid(1000));
    assert_eq!(eth0.state, LinkState::Up);
    let wlan0 = totals("wlan0");
    assert_eq!(wlan0.link_speed_mbps.validity, Validity::Unsupported);
    assert_eq!(wlan0.state, LinkState::Dormant);
}

#[test]
fn net_details_kinds_and_mac() {
    let dir = netif_kinds_fixture();
    let (mut src, _) = net_sample(NetSource::new(SysRoot::at(&dir)), SEC);
    let list = net_devices(&mut src);
    let entries = net_details(&mut src);

    let eth0 = net_details_of(&entries, "eth0");
    assert_eq!(eth0.kind, NetKind::Wired);
    assert_eq!(&*eth0.type_label, "Ethernet");
    assert_eq!(eth0.mac, valid(Arc::from("aa:bb:cc:dd:ee:01")));
    assert_eq!(eth0.driver, valid(Arc::from("e1000e")));
    assert_eq!(eth0.ifindex, 2);
    assert_eq!(eth0.link_speed_mbps, valid(1000));
    assert_eq!(eth0.state, LinkState::Up);
    assert!(eth0.addresses.is_empty());
    assert!(eth0.wifi.is_none());
    let desc = list
        .entries
        .iter()
        .find(|e| &*e.display_name == "eth0")
        .expect("eth0 in the NETS list");
    assert_eq!(desc.net_kind, Some(NetKind::Wired));
    assert!(list.entries.iter().all(|e| e.net_kind.is_some()));

    let br0 = net_details_of(&entries, "br0");
    assert_eq!(br0.kind, NetKind::Virtual);
    assert_eq!(&*br0.type_label, "Bridge");
    assert_eq!(br0.mac.validity, Validity::Missing);
    assert_eq!(br0.driver.validity, Validity::Unsupported);
    assert_eq!(
        net_details_of(&entries, "tailscale0").kind,
        NetKind::VpnTunnel
    );
    assert_eq!(
        net_details_of(&entries, "eth0").adapter_name.validity,
        Validity::Unsupported,
        "no udev data in the fixture"
    );
}

#[test]
fn net_adapter_name_from_udev() {
    let dir = netif_kinds_fixture();
    let data = dir.join("run/udev/data");
    fs::create_dir_all(&data).expect("create fixture udev data");
    fs::write(
        data.join("n2"),
        "E:ID_VENDOR_FROM_DATABASE=Intel Corporation\n\
         E:ID_MODEL_FROM_DATABASE=I350 Gigabit Network Connection\n",
    )
    .expect("write udev n2");
    fs::write(data.join("n3"), "E:ID_MODEL_FROM_DATABASE=Wi-Fi 6 AX201\n").expect("write udev n3");
    let (mut src, _) = net_sample(NetSource::new(SysRoot::at(&dir)), SEC);
    net_devices(&mut src);
    let entries = net_details(&mut src);
    assert_eq!(
        net_details_of(&entries, "eth0").adapter_name,
        valid(Arc::from(
            "Intel Corporation I350 Gigabit Network Connection"
        ))
    );
    assert_eq!(
        net_details_of(&entries, "wlan0").adapter_name,
        valid(Arc::from("Wi-Fi 6 AX201"))
    );
    assert_eq!(
        net_details_of(&entries, "br0").adapter_name.validity,
        Validity::Unsupported
    );
}

#[test]
fn net_kind_table() {
    assert_eq!(
        net_kind(IfaceKind::Cellular),
        (NetKind::Wireless, "Cellular")
    );
    assert_eq!(net_kind(IfaceKind::Bridge), (NetKind::Virtual, "Bridge"));
    assert_eq!(
        net_kind(IfaceKind::UsbNetwork),
        (NetKind::Wired, "USB network")
    );
    assert_eq!(net_kind(IfaceKind::Tunnel), (NetKind::VpnTunnel, "Tunnel"));
    assert_eq!(net_kind(IfaceKind::Other), (NetKind::Other, "Other"));
}

#[test]
fn net_details_only_on_change() {
    let dir = netif_kinds_fixture();
    let (mut src, _) = net_sample(NetSource::new(SysRoot::at(&dir)), SEC);
    net_devices(&mut src);
    net_details(&mut src);

    let (mut src, _) = net_sample(src, 6 * SEC);
    net_devices(&mut src);
    assert!(src.take_extra().is_none(), "no change, no details");

    fs::write(dir.join("sys/class/net/eth0/speed"), "2500\n").expect("write eth0 speed");
    let (mut src, s) = net_sample(src, 11 * SEC);
    net_devices(&mut src);
    let entries = net_details(&mut src);
    assert_eq!(
        net_details_of(&entries, "eth0").link_speed_mbps,
        valid(2500)
    );
    let eth0 = s
        .nets
        .iter()
        .find(|(id, _)| matches!(id, DeviceIdentity::Net { name, .. } if &**name == "eth0"))
        .expect("eth0 totals");
    assert_eq!(eth0.1.link_speed_mbps, Metric::valid(2500));
}

use tl_collect::net::addrs::{AddrFamily, AddrScope, IfAddr};

fn if_addr(ifindex: u32, addr: &str, prefix_len: u8, scope: AddrScope) -> IfAddr {
    let addr: std::net::IpAddr = addr.parse().expect("test address parses");
    IfAddr {
        ifindex,
        family: if addr.is_ipv4() {
            AddrFamily::V4
        } else {
            AddrFamily::V6
        },
        addr,
        prefix_len,
        scope,
    }
}

#[test]
fn net_addresses_into_details() {
    let dir = netif_kinds_fixture();
    // Dump order puts link-local first; the details list it last.
    let addrs = vec![
        if_addr(2, "fe80::1", 64, AddrScope::Link),
        if_addr(2, "2001:db8::5", 64, AddrScope::Global),
        if_addr(2, "192.168.1.10", 24, AddrScope::Global),
        if_addr(99, "10.9.9.9", 8, AddrScope::Global),
    ];
    let src = NetSource::with_addrs(SysRoot::at(&dir), addrs);

    // Network not demanded: no dump, no addresses.
    let (mut src, _) = net_sample(src, SEC);
    net_devices(&mut src);
    let entries = net_details(&mut src);
    assert!(net_details_of(&entries, "eth0").addresses.is_empty());

    let nets = Demand {
        nets: true,
        ..Demand::default()
    };
    assert!(src.period(&nets, &EngineSettings::default()).is_some());
    let (mut src, _) = net_sample(src, 6 * SEC);
    net_devices(&mut src);
    let entries = net_details(&mut src);
    let eth0: Vec<&str> = net_details_of(&entries, "eth0")
        .addresses
        .iter()
        .map(|a| &**a)
        .collect();
    assert_eq!(eth0, ["192.168.1.10/24", "2001:db8::5/64", "fe80::1/64"]);
    assert!(net_details_of(&entries, "wlan0").addresses.is_empty());
}

// ---- Inventory (E12-016) ----

use std::os::unix::fs::PermissionsExt;

use tl_collect::inventory::InventoryPaths;
use tl_engine::workers::inventory::scan_files;
use tl_model::snapshot::{FirmwareType, InventorySnapshot};

fn inventory_scan(dir: &Path) -> InventorySnapshot {
    scan_files(&InventoryPaths::under(dir), &SysRoot::at(dir))
}

fn inv_text(value: &str) -> tl_model::snapshot::InvField<Arc<str>> {
    tl_model::snapshot::InvField {
        value: Arc::from(value),
        validity: Validity::Valid,
    }
}

#[test]
fn inventory_files_sections() {
    let dir = fixture_copy("dmi-desktop", None);
    let snap = inventory_scan(&dir);
    assert_eq!(snap.header.validity, Validity::Valid);
    assert_eq!(
        snap.firmware.bios_vendor,
        inv_text("American Megatrends International, LLC.")
    );
    assert_eq!(snap.firmware.firmware_type.value, Some(FirmwareType::Uefi));
    assert_eq!(snap.firmware.firmware_type.validity, Validity::Valid);
    assert_eq!(snap.board.product, inv_text("ROG STRIX X570-E GAMING"));
    assert_eq!(snap.board.chassis_type, inv_text("3"));
    // No `board_serial` file in the fixture.
    assert_eq!(snap.board.serial.validity, Validity::Unsupported);
    assert_eq!(snap.os.name, inv_text("CachyOS Linux"));
    assert_eq!(snap.os.logo_icon.as_deref(), Some("cachyos"));
    assert_eq!(snap.modules.len(), 2);
    assert_eq!(snap.modules[0].slot, inv_text("P0 CHANNEL A (DIMM 0)"));
    assert_eq!(snap.modules[0].kind, inv_text("DDR4"));
    assert_eq!(snap.modules[0].capacity_bytes.value, 17_179_869_184);
    assert_eq!(
        snap.memory.peak_committed_bytes.validity,
        Validity::WarmingUp
    );
    assert!(!format!("{snap:?}").contains("SERIAL-MUST-NOT-APPEAR"));
}

#[test]
fn inventory_board_serial_restricted() {
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: root can read a mode 000 file");
        return;
    }
    let dir = fixture_copy("dmi-desktop", None);
    let serial = dir.join("sys/class/dmi/id/board_serial");
    fs::write(&serial, "BOARD-SERIAL\n").expect("write board_serial");
    fs::set_permissions(&serial, fs::Permissions::from_mode(0o000)).expect("chmod board_serial");
    let snap = inventory_scan(&dir);
    assert_eq!(snap.board.serial.validity, Validity::Restricted);
    assert!(!format!("{snap:?}").contains("BOARD-SERIAL"));
}

#[test]
fn inventory_os_release_fallbacks() {
    let dir = fixture_copy("dmi-desktop", None);
    // The fixture has no VERSION_ID.
    let snap = inventory_scan(&dir);
    assert_eq!(snap.os.version, inv_text("Rolling"));
    assert_eq!(snap.os.build, inv_text("rolling"));

    fs::write(
        dir.join("etc/os-release"),
        "PRETTY_NAME=\"Fedora Linux 42\"\nVERSION_ID=42\n",
    )
    .expect("write os-release");
    let snap = inventory_scan(&dir);
    assert_eq!(snap.os.version, inv_text("42"));
    assert_eq!(snap.os.build.validity, Validity::Unsupported);

    fs::remove_file(dir.join("etc/os-release")).expect("remove os-release");
    let snap = inventory_scan(&dir);
    assert_eq!(snap.os.name.validity, Validity::Failed);
    assert_eq!(snap.os.version.validity, Validity::Failed);
    assert_eq!(snap.os.build.validity, Validity::Failed);
}

#[test]
fn inventory_chassis_codes() {
    let dir = fixture_copy("dmi-desktop", None);
    let chassis = dir.join("sys/class/dmi/id/chassis_type");
    for (code, name) in [("3", "3"), ("10", "10"), ("36", "36")] {
        fs::write(&chassis, format!("{code}\n")).expect("write chassis_type");
        assert_eq!(inventory_scan(&dir).board.chassis_type, inv_text(name));
    }
    fs::write(&chassis, "99\n").expect("write chassis_type");
    assert_eq!(inventory_scan(&dir).board.chassis_type, inv_text("99"));
    fs::remove_file(&chassis).expect("remove chassis_type");
    assert_eq!(
        inventory_scan(&dir).board.chassis_type.validity,
        Validity::Unsupported
    );
}

/// D-007: memory module serials are never read or stored.
#[test]
fn inventory_no_module_serials() {
    let dir = fixture_copy("dmi-desktop", None);
    let udev = fs::read_to_string(dir.join("run/udev/data/+dmi:id")).expect("read udev dmi");
    assert!(udev.contains("SERIAL_NUMBER=SERIAL-MUST-NOT-APPEAR"));
    let snap = inventory_scan(&dir);
    assert!(!snap.modules.is_empty());
    let text = format!("{:?}", snap.modules);
    assert!(!text.contains("SERIAL-MUST-NOT-APPEAR"), "{text}");
}

// ---- Inventory System section (E12-017) ----

use tl_collect::inventory::KernelInfo;
use tl_engine::workers::inventory::{SystemBus, SystemEnv, build_system, parse_shell_version};

fn kernel(nis_domain: Option<&str>) -> KernelInfo {
    KernelInfo {
        release: Arc::from("6.17.0"),
        version: Arc::from("#1 SMP"),
        machine: Arc::from("x86_64"),
        nis_domain: nis_domain.map(Arc::from),
    }
}

fn kde_env() -> SystemEnv {
    SystemEnv {
        xdg_current_desktop: Some("KDE".into()),
        xdg_session_type: Some("wayland".into()),
        xdg_vtnr: Some("2".into()),
        ..SystemEnv::default()
    }
}

fn bus(static_name: &str) -> SystemBus {
    SystemBus {
        pretty: inv_text("Oystein's desktop"),
        static_name: inv_text(static_name),
        vtnr: None,
        class: None,
    }
}

#[test]
fn inventory_system_section() {
    let sys = build_system(
        &kde_env(),
        &bus("hawk.example.lan"),
        &kernel(None),
        inv_text("oystein"),
        Some(Arc::from("KDE Plasma 6.4.5")),
    );
    assert_eq!(sys.desktop, inv_text("KDE Plasma 6.4.5"));
    assert_eq!(sys.windowing_system, inv_text("Wayland"));
    assert_eq!(sys.session_type, inv_text("wayland"));
    assert_eq!(sys.virtual_terminal, inv_text("2"));
    assert_eq!(sys.domain.as_deref(), Some("example.lan"));
    assert_eq!(sys.pretty_hostname, inv_text("Oystein's desktop"));
    assert_eq!(sys.static_hostname, inv_text("hawk.example.lan"));
    assert_eq!(sys.user_name, inv_text("oystein"));

    // Without a shell version the desktop is the variable as is; the VT can come from the bus.
    let env = SystemEnv {
        xdg_current_desktop: Some("ubuntu:GNOME".into()),
        xdg_session_type: Some("x11".into()),
        ..SystemEnv::default()
    };
    let mut from_bus = bus("hawk");
    from_bus.vtnr = Some(3);
    let sys = build_system(&env, &from_bus, &kernel(None), inv_text("1000"), None);
    assert_eq!(sys.desktop, inv_text("ubuntu:GNOME"));
    assert_eq!(sys.windowing_system, inv_text("X11"));
    assert_eq!(sys.virtual_terminal, inv_text("3"));

    let env = SystemEnv {
        xdg_session_type: Some("tty".into()),
        ..SystemEnv::default()
    };
    let sys = build_system(&env, &bus("hawk"), &kernel(None), inv_text("1000"), None);
    assert_eq!(sys.desktop.validity, Validity::Unsupported);
    assert_eq!(sys.windowing_system, inv_text("None (text console)"));
    assert_eq!(sys.virtual_terminal.validity, Validity::Unsupported);

    let sys = build_system(
        &SystemEnv::default(),
        &bus("hawk"),
        &kernel(None),
        inv_text("1000"),
        None,
    );
    assert_eq!(sys.windowing_system.validity, Validity::Unsupported);
    assert_eq!(sys.session_type.validity, Validity::Unsupported);
}

#[test]
fn inventory_session_class() {
    let build = |env: &SystemEnv, bus: &SystemBus| {
        build_system(env, bus, &kernel(None), inv_text("oystein"), None).session_class
    };
    let with_class = |c: &str| SystemEnv {
        xdg_session_class: Some(c.into()),
        ..kde_env()
    };
    let bus_class = |c: &str| SystemBus {
        class: Some(Arc::from(c)),
        ..bus("hawk")
    };
    assert_eq!(build(&with_class("user"), &bus("hawk")), inv_text("user"));
    assert_eq!(
        build(&kde_env(), &bus_class("greeter")),
        inv_text("greeter")
    );
    assert_eq!(
        build(&with_class("user"), &bus_class("greeter")),
        inv_text("user")
    );
    assert_eq!(
        build(&kde_env(), &bus("hawk")).validity,
        Validity::Unsupported
    );
}

#[test]
fn inventory_domain_rules() {
    let user = || inv_text("oystein");
    let env = kde_env();
    // No dot and no NIS domain hides the row.
    let sys = build_system(&env, &bus("hawk"), &kernel(None), user(), None);
    assert_eq!(sys.domain, None);
    // A NIS domain wins over the hostname.
    let sys = build_system(
        &env,
        &bus("hawk.example.lan"),
        &kernel(Some("nis.example")),
        user(),
        None,
    );
    assert_eq!(sys.domain.as_deref(), Some("nis.example"));
    // E06-017 maps "(none)" to `None`, so the hostname rule applies.
    let sys = build_system(&env, &bus("hawk.lan"), &kernel(None), user(), None);
    assert_eq!(sys.domain.as_deref(), Some("lan"));
    // A trailing dot gives no domain.
    let sys = build_system(&env, &bus("hawk."), &kernel(None), user(), None);
    assert_eq!(sys.domain, None);
}

#[test]
fn inventory_bus_failure_falls_back() {
    let sys = build_system(
        &kde_env(),
        &SystemBus::failed(),
        &kernel(None),
        inv_text("oystein"),
        None,
    );
    let uts = rustix::system::uname();
    let nodename = uts.nodename().to_string_lossy();
    assert_eq!(sys.static_hostname, inv_text(&nodename));
    assert_eq!(sys.pretty_hostname.validity, Validity::Failed);
}

#[test]
fn inventory_shell_version_parse() {
    assert_eq!(
        parse_shell_version(b"plasmashell 6.4.5\n").as_deref(),
        Some("KDE Plasma 6.4.5")
    );
    assert_eq!(
        parse_shell_version(b"GNOME Shell 47.1\n").as_deref(),
        Some("GNOME 47.1")
    );
    assert_eq!(parse_shell_version(b"weird"), None);
    assert_eq!(parse_shell_version(b"plasmashell \n"), None);
}

#[test]
fn inventory_boot_time_close_to_uptime() {
    let sys = build_system(
        &SystemEnv::default(),
        &bus("hawk"),
        &kernel(None),
        inv_text("oystein"),
        None,
    );
    assert_eq!(sys.boot_time_s.validity, Validity::Valid);
    let uptime = fs::read_to_string("/proc/uptime").expect("read /proc/uptime");
    let uptime_s: f64 = uptime
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .expect("parse /proc/uptime");
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after 1970")
        .as_secs_f64();
    let expected = now_s - uptime_s;
    let diff = (sys.boot_time_s.value as f64 - expected).abs();
    assert!(diff <= 2.0, "boot time differs by {diff} s");
}

// ---- Inventory devices (E12-018) ----

use tl_collect::gpu_drm::{self, AmdSample, amdgpu};
use tl_model::snapshot::DisplayConnector;

#[test]
fn inventory_disk_drives() {
    let dir = fixture_copy("disk-partitions-dm", None);
    copy_tree(&fixtures().join("dmi-desktop"), &dir);
    let snap = inventory_scan(&dir);
    let names: Vec<&str> = snap
        .disks
        .iter()
        .map(|d| d.name.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(names, ["mmcblk0", "nvme0n1", "sda", "sdb", "sr0", "vda"]);
    let kinds: Vec<&str> = snap.disks.iter().map(|d| &*d.kind.value).collect();
    assert_eq!(kinds, ["SD", "NVMe", "HDD", "USB", "Optical", "Virtual"]);
    let sizes: Vec<u64> = snap.disks.iter().map(|d| d.capacity_bytes.value).collect();
    assert_eq!(
        sizes,
        [
            62_521_344 * 512,
            1_000_215_216 * 512,
            1_953_525_168 * 512,
            62_521_344 * 512,
            0,
            41_943_040 * 512,
        ]
    );
    assert!(snap.disks.iter().all(|d| d.device.is_none()));
    let sda = &snap.disks[2];
    assert_eq!(sda.kind, inv_text("HDD"));
    assert_eq!(sda.model, inv_text("WDC WD10EZEX-08W"));
    let volumes: Vec<&str> = sda.volumes.iter().map(|v| &**v).collect();
    assert_eq!(volumes, ["/", "/boot", "/mnt/boot copy"]);
    // A fixture root never calls `statvfs`: its answer is `NotFound`.
    assert_eq!(sda.free_bytes.validity, Validity::Missing);
    assert_eq!(sda.serial.validity, Validity::Unsupported);
    let nvme = &snap.disks[1];
    assert!(nvme.volumes.is_empty());
    assert_eq!(nvme.free_bytes.validity, Validity::Unsupported);
    assert_eq!(nvme.interface, inv_text("NVMe"));
    let sdb = &snap.disks[3];
    assert_eq!(sdb.interface, inv_text("USB"));
    assert_eq!(sdb.model, inv_text("Flash Disk"));
    assert_eq!(snap.disks[5].interface, inv_text("virtio"));
}

#[test]
fn inventory_graphics_amd() {
    let dir = fixture_copy("drm-amd", None);
    let udev = dir.join("run/udev/data");
    fs::create_dir_all(&udev).expect("create udev dir");
    fs::write(
        udev.join("+pci:0000:03:00.0"),
        "E:ID_MODEL_FROM_DATABASE=Navi 31 [Radeon RX 7900 XT]\n",
    )
    .expect("write udev file");
    let snap = inventory_scan(&dir);
    assert_eq!(snap.graphics.len(), 1);
    let gpu = &snap.graphics[0];
    assert_eq!(gpu.name, inv_text("Radeon RX 7900 XT"));
    assert_eq!(gpu.device, None);
    assert_eq!(gpu.pci_slot.as_deref(), Some("0000:03:00.0"));
    let sys = dir.join("sys");
    let card = &gpu_drm::discover_cards(&sys)[0];
    let mut amd = AmdSample::default();
    gpu_drm::read_amd(card, &sys, &mut amd);
    let (temp, _) = gpu_drm::main_temp(&amd).expect("fixture has a main temperature");
    assert_eq!(
        gpu.temperature_source,
        inv_text(amdgpu::temp_source_text(temp))
    );
    assert_eq!(gpu.dedicated_bytes.value, 17_095_983_104);
    assert_eq!(gpu.dedicated_bytes.validity, Validity::Valid);
    // No gtt total in the fixture.
    assert_eq!(gpu.shared_bytes.validity, Validity::Unsupported);
    assert_eq!(gpu.vulkan_version.validity, Validity::WarmingUp);
    assert_eq!(gpu.opengl_version.validity, Validity::WarmingUp);
    // No `module/amdgpu/version`: the module is the driver alone.
    assert_eq!(gpu.kernel_module, inv_text("amdgpu"));
    assert_eq!(gpu.driver_version.validity, Validity::Unsupported);
    assert!(gpu.displays.is_empty());
}

#[test]
fn inventory_displays_connected_only() {
    let dir = fixture_copy("dmi-desktop", None);
    let device = dir.join("sys/class/drm/card1/device");
    fs::create_dir_all(&device).expect("create card dir");
    fs::write(device.join("vendor"), "0x1002\n").expect("write vendor");
    fs::write(device.join("device"), "0x744c\n").expect("write device");
    fs::write(device.join("uevent"), "PCI_SLOT_NAME=0000:03:00.0\n").expect("write uevent");
    let snap = inventory_scan(&dir);
    assert_eq!(snap.graphics.len(), 1);
    let gpu = &snap.graphics[0];
    assert_eq!(gpu.displays.len(), 1);
    let mode = &gpu.displays[0];
    assert_eq!((mode.width, mode.height), (3840, 2160));
    assert_eq!(mode.refresh_mhz, 0);
    assert!(!mode.primary);
    assert_eq!(mode.max_bpc, None);
    assert_eq!(mode.rotation, None);
    assert_eq!(mode.scale, None);
    assert_ne!(mode.connector, DisplayConnector::Unknown);
    assert!(mode.monitor_name.is_some());
    // No udev file for the card.
    assert_eq!(gpu.name.validity, Validity::Unsupported);
    assert_eq!(gpu.temperature_source, inv_text("Not reported"));
}

#[test]
fn inventory_network_adapters_visibility() {
    // The engine lists every kind with its kind; the window filters (tasklens-q4x76.22).
    let dir = fixture_copy("netif-kinds", None);
    let snap = inventory_scan(&dir);
    let names: Vec<&str> = snap
        .network
        .iter()
        .map(|a| a.connection.value.split(' ').next().unwrap_or(""))
        .collect();
    assert!(!names.contains(&"lo"), "lo listed: {names:?}");
    for listed in ["docker0", "veth12ab", "virbr0", "br0", "wlan0", "eth0"] {
        assert!(names.contains(&listed), "{listed} missing: {names:?}");
    }
    let kind_of = |name: &str| {
        snap.network
            .iter()
            .find(|a| a.connection.value.split(' ').next() == Some(name))
            .and_then(|a| a.kind)
    };
    assert_eq!(kind_of("eth0"), Some(NetKind::Wired));
    assert_eq!(kind_of("wlan0"), Some(NetKind::Wireless));
    assert_eq!(kind_of("docker0"), Some(NetKind::Virtual));
    assert_eq!(kind_of("veth12ab"), Some(NetKind::Virtual));
    let eth0 = snap
        .network
        .iter()
        .find(|a| a.connection.value.starts_with("eth0 "))
        .expect("eth0 listed");
    assert_eq!(eth0.connection, inv_text("eth0 (Ethernet)"));
    assert_eq!(eth0.link_speed_mbps.value, 1000);
    assert_eq!(eth0.link_speed_mbps.validity, Validity::Valid);
    assert_eq!(eth0.adapter_name.validity, Validity::Unsupported);
    // A fixture root runs no address dump.
    assert!(eth0.ip_addresses.is_empty());
    assert!(snap.network.iter().all(|a| a.device.is_none()));
    assert!(
        snap.network
            .iter()
            .any(|a| a.connection == inv_text("docker0 (Virtual)"))
    );
}

// ---- GpuDrm for AMD cards (E12-026) ----

use tl_engine::update::{GpuInfoUpdate, GpuSample};
use tl_engine::workers::gpu_drm::GpuDrmSource;

impl SampleAt for GpuDrmSource {
    fn sample_at(self, clock: Clock) -> (Self, SourceUpdate) {
        GpuDrmSource::sample_at(self, clock)
    }
}

/// Every update `take_extra` holds, in order.
fn gpu_extras(src: &mut GpuDrmSource) -> Vec<SourceUpdate> {
    std::iter::from_fn(|| src.take_extra()).collect()
}

/// The `Devices` list and the `GpuInfo` of one enumeration.
fn gpu_lists(extras: &[SourceUpdate]) -> (&DeviceList, &GpuInfoUpdate) {
    match extras {
        [SourceUpdate::Devices(list), SourceUpdate::GpuInfo(info)] => (list, info),
        other => panic!("expected Devices then GpuInfo, got {other:?}"),
    }
}

fn gpu_sample(src: GpuDrmSource, t: u64) -> (GpuDrmSource, GpuSample) {
    match src.sample_at(at(t)) {
        (src, SourceUpdate::Gpu(sample)) => (src, sample),
        (_, other) => panic!("expected SourceUpdate::Gpu, got {other:?}"),
    }
}

#[test]
fn gpu_drm_fixture_sample() {
    let dir = fixture_copy("drm-amd", None);
    let (mut src, s) = gpu_sample(GpuDrmSource::new(SysRoot::at(&dir)), SEC);
    let extras = gpu_extras(&mut src);
    let (list, info) = gpu_lists(&extras);
    assert_eq!(list.kinds, DeviceKinds::GPUS);
    assert_eq!(list.entries.len(), 1);
    let identity = DeviceIdentity::Gpu {
        uuid: [0; 16],
        pci_bus_id: Arc::from("0000:03:00.0"),
    };
    assert_eq!(list.entries[0].identity, identity);
    assert_eq!(&*list.entries[0].persistent_key, "gpu:0000:03:00.0");
    // No udev database in the fixture.
    assert_eq!(&*list.entries[0].display_name, "AMD GPU 744c");
    assert_eq!(info.devices.len(), 1);
    let gpu = &info.devices[0];
    assert_eq!(gpu.mem_total, 17_095_983_104);
    assert_eq!(gpu.power_limit_mw, Some(300_000));
    assert_eq!(&*gpu.driver, "amdgpu");
    assert!(!gpu.supports_process_util);

    assert_eq!(s.t_ns, SEC);
    assert!(s.processes.is_none());
    assert_eq!(s.devices.len(), 1);
    let (id, t) = &s.devices[0];
    assert_eq!(*id, identity);
    assert_eq!(t.util_pct, Metric::valid(37.0));
    assert_eq!(t.clock_gfx_mhz, Metric::valid(1905));
    assert_eq!(t.temp_c, Metric::valid(61.0));
    assert_eq!(t.power_w.validity, Validity::Valid);
    assert!((t.power_w.value - 31.9).abs() < 1e-3, "{:?}", t.power_w);
    assert_eq!(t.power_limit_w, Metric::valid(300.0));
    assert_eq!(t.mem_total, Metric::valid(17_095_983_104));
    // No gtt or fan files, and the first fdinfo scan has no baseline.
    assert_eq!(t.shared_total.validity, Validity::Unsupported);
    assert_eq!(t.fan_pct.validity, Validity::Unsupported);
    assert_eq!(t.enc_pct.validity, Validity::WarmingUp);
    // No mem_busy_percent file, and JPEG was never named in fdinfo.
    let engines: Vec<GpuEngine> = t.engines.reported().map(|(e, _)| e).collect();
    assert_eq!(
        engines,
        [
            GpuEngine::Gpu,
            GpuEngine::Enc,
            GpuEngine::Dec,
            GpuEngine::Compute,
            GpuEngine::Dma
        ]
    );
    assert_eq!(t.engines.get(GpuEngine::Gpu), Metric::valid(37.0));
    assert_eq!(
        t.engines.get(GpuEngine::Compute).validity,
        Validity::WarmingUp
    );
    assert!(!t.fdinfo_partial);

    // The set did not change: no new lists, and fdinfo is warm.
    let (mut src, s) = gpu_sample(src, 2 * SEC);
    assert!(gpu_extras(&mut src).is_empty());
    let t = &s.devices[0].1;
    assert_eq!(t.enc_pct, Metric::valid(0.0));
    assert_eq!(t.engines.get(GpuEngine::Compute), Metric::valid(0.0));
    assert_eq!(t.engines.get(GpuEngine::Dma), Metric::valid(0.0));
}

#[test]
fn gpu_drm_skips_nvidia() {
    // card0 is i915 at 0000:00:02.0, card1 is NVIDIA at 0000:2d:00.0.
    let dir = fixture_copy("drm-basic", None);
    let (mut src, s) = gpu_sample(GpuDrmSource::new(SysRoot::at(&dir)), SEC);
    let extras = gpu_extras(&mut src);
    let (list, _) = gpu_lists(&extras);
    let keys: Vec<&str> = list.entries.iter().map(|e| &*e.persistent_key).collect();
    assert_eq!(keys, ["gpu:0000:00:02.0"]);
    assert_eq!(s.devices.len(), 1);
    // The fixture's fdinfo client on 0000:00:02.0 holds 8 MiB of system memory.
    assert_eq!(s.devices[0].1.shared_used, Metric::valid(8 << 20));

    // With the Intel card gone, nothing but NVIDIA is left: no `Gpu` sample at all.
    fs::remove_file(dir.join("sys/class/drm/card0/device/vendor")).expect("remove vendor");
    for k in 2..=7 {
        let (next, update) = src.sample_at(at(k * SEC));
        src = next;
        if k < 6 {
            continue;
        }
        assert!(
            !matches!(update, SourceUpdate::Gpu(_)),
            "Gpu sample without an AMD or Intel card: {update:?}"
        );
    }
}

// ---- GpuDrm for Intel cards (E12-027) ----

/// The fixture's Intel cards: card0 is i915 at 0000:00:02.0 (an iGPU), card1 is xe at
/// 0000:03:00.0 (a dGPU with `tile0`).
const INTEL_I915: &str = "0000:00:02.0";
const INTEL_XE: &str = "0000:03:00.0";

fn gpu_by_slot<'a>(s: &'a GpuSample, slot: &str) -> &'a tl_model::system::GpuTotals {
    s.devices
        .iter()
        .find(
            |(id, _)| matches!(id, DeviceIdentity::Gpu { pci_bus_id, .. } if &**pci_bus_id == slot),
        )
        .map(|(_, t)| t)
        .unwrap_or_else(|| panic!("no GPU at {slot}: {s:?}"))
}

#[test]
fn gpu_drm_intel_i915_fixture() {
    let dir = fixture_copy("drm-intel", None);
    let (mut src, s) = gpu_sample(GpuDrmSource::new(SysRoot::at(&dir)), SEC);
    let extras = gpu_extras(&mut src);
    let (list, info) = gpu_lists(&extras);
    let keys: Vec<&str> = list.entries.iter().map(|e| &*e.persistent_key).collect();
    assert_eq!(
        keys,
        [format!("gpu:{INTEL_I915}"), format!("gpu:{INTEL_XE}")]
    );
    // No udev database in the fixture.
    assert_eq!(&*list.entries[0].display_name, "Intel GPU 46a6");
    assert_eq!(&*info.devices[0].driver, "i915");
    assert_eq!(info.devices[0].power_limit_mw, Some(28_000));

    let t = gpu_by_slot(&s, INTEL_I915);
    assert_eq!(t.clock_gfx_mhz, Metric::valid(1300));
    assert_eq!(t.power_w.validity, Validity::WarmingUp);
    assert_eq!(t.temp_c.validity, Validity::Unsupported);
    assert_eq!(t.util_pct.validity, Validity::WarmingUp);
    assert_eq!(t.power_limit_w, Metric::valid(28.0));
    // An iGPU has no dedicated memory.
    assert_eq!(t.mem_total.validity, Validity::Unsupported);

    // 12 J in 1 s.
    fs::write(
        dir.join("sys/class/drm/card0/device/hwmon/hwmon4/energy1_input"),
        format!("{}\n", 123_456_789 + 12_000_000),
    )
    .expect("write energy");
    let (_, s) = gpu_sample(src, 2 * SEC);
    let t = gpu_by_slot(&s, INTEL_I915);
    assert_eq!(t.power_w.validity, Validity::Valid);
    assert!((t.power_w.value - 12.0).abs() < 1e-4, "{:?}", t.power_w);
    // Warm, and no fdinfo client on the card.
    assert_eq!(t.util_pct, Metric::valid(0.0));
    let engines: Vec<(GpuEngine, Metric<f32>)> = t.engines.reported().collect();
    assert_eq!(
        engines,
        [
            (GpuEngine::Render, Metric::valid(0.0)),
            (GpuEngine::Copy, Metric::valid(0.0)),
            (GpuEngine::Video, Metric::valid(0.0)),
        ]
    );
}

#[test]
fn gpu_drm_intel_xe_fixture() {
    let dir = fixture_copy("drm-intel", None);
    let (mut src, s) = gpu_sample(GpuDrmSource::new(SysRoot::at(&dir)), SEC);
    let extras = gpu_extras(&mut src);
    let (list, info) = gpu_lists(&extras);
    assert_eq!(&*list.entries[1].display_name, "Intel GPU e20b");
    assert!(
        info.devices[1].driver.starts_with("xe"),
        "{:?}",
        info.devices[1]
    );
    assert_eq!(info.devices[1].power_limit_mw, Some(190_000));

    let t = gpu_by_slot(&s, INTEL_XE);
    assert_eq!(t.clock_gfx_mhz, Metric::valid(900));
    assert_eq!(t.clock_mem_mhz.validity, Validity::Unsupported);
    // A dGPU shows its temperature.
    assert_eq!(t.temp_c, Metric::valid(48.0));
    assert_eq!(t.power_w.validity, Validity::WarmingUp);
    assert_eq!(t.power_limit_w, Metric::valid(190.0));
}

#[test]
fn gpu_drm_amd_details_fields() {
    let dir = fixture_copy("drm-amd", None);
    let device = dir.join("sys/class/drm/card0/device");
    let write_link = |cur: &str, width: &str| {
        for (file, text) in [
            ("current_link_speed", cur),
            ("current_link_width", width),
            ("max_link_speed", "16.0 GT/s PCIe"),
            ("max_link_width", "16"),
        ] {
            fs::write(device.join(file), format!("{text}\n")).expect("write link");
        }
    };
    write_link("8.0 GT/s PCIe", "8");
    fs::write(device.join("hwmon/hwmon3/fan1_input"), "1100\n").expect("write fan");

    let (src, s) = gpu_sample(GpuDrmSource::new(SysRoot::at(&dir)), SEC);
    let t = &s.devices[0].1;
    assert_eq!(t.pcie_gen, Metric::valid(3));
    assert_eq!(t.pcie_width, Metric::valid(8));
    assert_eq!(t.fan_rpm, Metric::valid(1100));
    assert_eq!(t.pstate.validity, Validity::Unsupported);
    assert_eq!(t.clock_video_mhz.validity, Validity::Unsupported);
    assert_eq!(t.energy_since_boot.validity, Validity::Unsupported);
    let info = Arc::clone(&t.info);
    assert_eq!(info.pcie_max_gen, Some(4));
    assert_eq!(info.pcie_max_width, Some(16));
    // The last pp_dpm_sclk line; the fixture has no pp_dpm_mclk.
    assert_eq!(info.clock_gfx_max_mhz, Some(2500));
    assert_eq!(info.clock_mem_max_mhz, None);
    assert_eq!(info.cuda_version, None);
    assert_eq!(info.power_max_limit_w, None);
    assert_eq!(info.slowdown_temp_c, None);

    // The link drops while the card idles. The static values stay the same `Arc`.
    write_link("2.5 GT/s PCIe", "1");
    let (_, s) = gpu_sample(src, 2 * SEC);
    let t = &s.devices[0].1;
    assert_eq!(t.pcie_gen, Metric::valid(1));
    assert_eq!(t.pcie_width, Metric::valid(1));
    assert!(Arc::ptr_eq(&t.info, &info));
}

/// tasklens-hjn.43: hwmon `power1_cap_min` gives the lowest power limit; a card without the
/// file has none.
#[test]
fn gpu_drm_amd_power_min_limit() {
    let dir = fixture_copy("drm-amd", None);
    let (_, s) = gpu_sample(GpuDrmSource::new(SysRoot::at(&dir)), SEC);
    assert_eq!(s.devices[0].1.info.power_min_limit_w, None);

    let dir = fixture_copy("drm-amd", None);
    // 100 W in microwatts.
    fs::write(
        dir.join("sys/class/drm/card0/device/hwmon/hwmon3/power1_cap_min"),
        "100000000\n",
    )
    .expect("write power1_cap_min");
    let (_, s) = gpu_sample(GpuDrmSource::new(SysRoot::at(&dir)), SEC);
    let t = &s.devices[0].1;
    assert_eq!(t.info.power_min_limit_w, Some(100.0));
    // amdgpu has no clock event reasons.
    assert_eq!(t.clock_event_reasons.validity, Validity::Unsupported);
}

#[test]
fn gpu_drm_intel_details_fields() {
    let dir = fixture_copy("drm-intel", None);
    let (_, s) = gpu_sample(GpuDrmSource::new(SysRoot::at(&dir)), SEC);
    let t = gpu_by_slot(&s, INTEL_I915);
    // hwmon energy1_input is 123456789 µJ.
    assert_eq!(t.energy_since_boot, Metric::valid(123.456_789));
    assert_eq!(t.info.clock_gfx_max_mhz, Some(2200));
    assert_eq!(t.info.clock_mem_max_mhz, None);
    assert_eq!(t.info.power_min_limit_w, None);
    assert_eq!(t.clock_event_reasons.validity, Validity::Unsupported);
    // No link files in the fixture.
    assert_eq!(t.pcie_gen.validity, Validity::Unsupported);
    assert_eq!(t.info.pcie_max_gen, None);
    assert_eq!(t.fan_rpm.validity, Validity::Unsupported);
}

/// Writes a second AMD card, `card1` at `0000:0b:00.0`, into the fixture copy.
fn add_amd_card1(dir: &Path) {
    let device = dir.join("sys/class/drm/card1/device");
    fs::create_dir_all(&device).expect("create card dir");
    fs::write(device.join("vendor"), "0x1002\n").expect("write vendor");
    fs::write(device.join("device"), "0x73bf\n").expect("write device");
    fs::write(
        device.join("uevent"),
        "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:0b:00.0\n",
    )
    .expect("write uevent");
    fs::write(device.join("gpu_busy_percent"), "12\n").expect("write busy");
    std::os::unix::fs::symlink("../../bus/pci/drivers/amdgpu", device.join("driver"))
        .expect("create driver link");
}

#[test]
fn gpu_drm_card_added() {
    let dir = fixture_copy("drm-amd", None);
    let (mut src, _) = gpu_sample(GpuDrmSource::new(SysRoot::at(&dir)), SEC);
    assert_eq!(gpu_extras(&mut src).len(), 2);

    add_amd_card1(&dir);
    // Before the next 5 s rescan the new card is not seen.
    let (mut src, s) = gpu_sample(src, 2 * SEC);
    assert!(gpu_extras(&mut src).is_empty());
    assert_eq!(s.devices.len(), 1);

    let (mut src, s) = gpu_sample(src, 6 * SEC);
    let extras = gpu_extras(&mut src);
    let (list, info) = gpu_lists(&extras);
    let slots: Vec<&str> = list.entries.iter().map(|e| &*e.persistent_key).collect();
    assert_eq!(slots, ["gpu:0000:03:00.0", "gpu:0000:0b:00.0"]);
    assert_eq!(&*list.entries[1].display_name, "AMD GPU 73bf");
    assert_eq!(info.devices.len(), 2);
    // No VRAM file on card1.
    assert_eq!(info.devices[1].mem_total, 0);
    assert_eq!(s.devices.len(), 2);
    assert_eq!(s.devices[1].1.util_pct, Metric::valid(12.0));

    let (mut src, s) = gpu_sample(src, 7 * SEC);
    assert!(gpu_extras(&mut src).is_empty());
    assert_eq!(s.devices.len(), 2);
}

#[test]
fn gpu_drm_demand_and_hidden() {
    let dir = fixture_copy("drm-amd", None);
    let mut fl = FakeLab::new();
    let (updates, mut rx, demand, settings, control) = channels();
    demand
        .send(Demand {
            gpus: true,
            ..Demand::default()
        })
        .expect("watch has receivers");
    let worker_io = io(&updates, &demand, &settings, &control);
    let handle = spawn_on_lab(&mut fl, worker_io, GpuDrmSource::new(SysRoot::at(&dir)));

    // Receive times in ms of the `Gpu` messages, read after every 10 ms step.
    let mut times = Vec::new();
    let mut run = |fl: &mut FakeLab, total: Duration, times: &mut Vec<u64>| {
        let end = fl.now_ms() + total.as_millis() as u64;
        while fl.now_ms() < end {
            fl.advance(Duration::from_millis(10));
            for msg in drain(&mut rx) {
                if matches!(msg, EngineMsg::Source(SourceUpdate::Gpu(_))) {
                    times.push(fl.now_ms());
                }
            }
        }
    };

    run(&mut fl, Duration::from_millis(9_990), &mut times);
    // The t = 0 sample is read after the first 10 ms step.
    let expected: Vec<u64> = std::iter::once(10)
        .chain((1..10).map(|k| k * 1_000))
        .collect();
    assert_eq!(times, expected);

    demand.send(Demand::default()).expect("watch has receivers");
    times.clear();
    run(&mut fl, Duration::from_secs(60), &mut times);
    // `source::run` reads demand at each deadline, so the tick already due at 10 s still
    // samples. After it, nothing for the rest of the 60 s.
    assert_eq!(times, [10_000], "sampled while hidden");

    handle.abort();
    fl.finish();
}

/// Reads the live host: run on a machine with an AMD card with `--ignored`.
#[test]
#[ignore]
fn gpu_drm_real_host() {
    let (mut src, s) = gpu_sample(
        GpuDrmSource::new(SysRoot::host()),
        tl_model::time::now_boot_ns(),
    );
    let extras = gpu_extras(&mut src);
    let (_, info) = gpu_lists(&extras);
    assert!(!info.devices.is_empty());
    let (_, t) = s.devices.first().expect("an AMD card");
    assert_eq!(t.util_pct.validity, Validity::Valid);
    assert_eq!(t.mem_total.validity, Validity::Valid);
    assert!(t.mem_total.value > 8 << 30, "VRAM {}", t.mem_total.value);
}

// ---- The P2 workers together (E12-A3) ----
//
// The `Inventory` and `GpuDrm` workers run here through their public seams. The `Udisks`,
// `UPower` and `NetworkManager` fakes (`Connector`) are private to their modules, so their idle
// and demand cases are the in-crate tests `dbus_udisks::no_connect_when_not_demanded`,
// `dbus_upower::idle_until_demand` and `dbus_nm::nm_hidden_no_connect`.

use tl_engine::workers::inventory;
use tl_model::snapshot::PublicIpState;

/// The five P2 sources (ADR-004).
const P2_SOURCES: [SourceId; 5] = [
    SourceId::Inventory,
    SourceId::Udisks,
    SourceId::UPower,
    SourceId::NetworkManager,
    SourceId::GpuDrm,
];

/// The P0 and P1 sources that run in full mode.
const P0_P1_SOURCES: [SourceId; 8] = [
    SourceId::Cpu,
    SourceId::Memory,
    SourceId::Disk,
    SourceId::Net,
    SourceId::Sensors,
    SourceId::Processes,
    SourceId::GpuNvidia,
    SourceId::Logind,
];

#[test]
fn p2_workers_registered() {
    let full: Vec<SourceId> = workers_for(EngineMode::Full)
        .iter()
        .map(|spec| spec.id)
        .collect();
    let low: Vec<SourceId> = workers_for(EngineMode::LowMemory)
        .iter()
        .map(|spec| spec.id)
        .collect();
    for id in P2_SOURCES.iter().chain(&P0_P1_SOURCES) {
        assert_eq!(full.iter().filter(|f| *f == id).count(), 1, "{id:?}");
    }
    for id in P2_SOURCES {
        assert!(!low.contains(&id), "{id:?}");
    }
}

/// A source that counts its `sample` calls. For `GpuDrm` each call is one fdinfo scan.
struct Counting<S> {
    inner: S,
    samples: Arc<AtomicU32>,
}

impl<S: SampleAt> SampleAt for Counting<S> {
    fn sample_at(self, clock: Clock) -> (Self, SourceUpdate) {
        self.samples.fetch_add(1, Ordering::SeqCst);
        let (inner, update) = self.inner.sample_at(clock);
        let samples = self.samples;
        (Counting { inner, samples }, update)
    }
}

impl<S: SampleAt> Source for Counting<S> {
    fn id(&self) -> SourceId {
        self.inner.id()
    }

    fn period(&self, demand: &Demand, settings: &EngineSettings) -> Option<Duration> {
        self.inner.period(demand, settings)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        let clock = at(tl_model::time::now_boot_ns());
        self.sample_at(clock)
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

/// The message kinds the P2 tests count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum P2 {
    Inventory,
    Gpu,
    Other(SourceId),
}

fn p2_kind(msg: &EngineMsg) -> Option<P2> {
    let EngineMsg::Source(update) = msg else {
        return None;
    };
    match update {
        SourceUpdate::Inventory(_) => Some(P2::Inventory),
        SourceUpdate::Gpu(_) => Some(P2::Gpu),
        SourceUpdate::Health { id, .. } => Some(P2::Other(*id)),
        SourceUpdate::Devices(_) | SourceUpdate::GpuInfo(_) => Some(P2::Other(SourceId::GpuDrm)),
        _ => None,
    }
}

/// What one P2 run saw: the messages, and the inventory scans, public IP fetches and GpuDrm
/// samples the workers made.
struct P2Run {
    msgs: Vec<P2>,
    scans: u32,
    fetches: u32,
    gpu_samples: u32,
}

/// Runs `Inventory` (on the `dmi-desktop` fixture) and `GpuDrm` (on `drm-amd`) under `demand` for
/// `total` in 10 ms steps, then checks the lab's oracles.
fn run_p2(demand: Demand, total: Duration) -> P2Run {
    let dmi = fixture_copy("dmi-desktop", None);
    let drm = fixture_copy("drm-amd", None);
    let mut fl = FakeLab::new();
    let (updates, mut rx, demand_tx, settings, control) = channels();
    demand_tx.send(demand).expect("watch has receivers");

    let scans = Arc::new(AtomicU32::new(0));
    let fetches = Arc::new(AtomicU32::new(0));
    let gpu_samples = Arc::new(AtomicU32::new(0));
    let scan = {
        let scans = Arc::clone(&scans);
        move || {
            scans.fetch_add(1, Ordering::SeqCst);
            inventory_scan(&dmi)
        }
    };
    let fetch_ip = {
        let fetches = Arc::clone(&fetches);
        move || {
            fetches.fetch_add(1, Ordering::SeqCst);
            PublicIpState::NotFetched
        }
    };
    let inv_io = io(&updates, &demand_tx, &settings, &control);
    let inv = fl.spawn(async move {
        let cx = Cx::current().expect("task has a Cx");
        let _ = inventory::run_with(cx, inv_io, scan, fetch_ip).await;
    });
    let gpu = Counting {
        inner: GpuDrmSource::new(SysRoot::at(&drm)),
        samples: Arc::clone(&gpu_samples),
    };
    let gpu = spawn_on_lab(&mut fl, io(&updates, &demand_tx, &settings, &control), gpu);

    let end = fl.now_ms() + total.as_millis() as u64;
    let mut msgs = Vec::new();
    loop {
        msgs.extend(drain(&mut rx).iter().filter_map(p2_kind));
        if fl.now_ms() >= end {
            break;
        }
        fl.advance(Duration::from_millis(10));
    }
    msgs.extend(drain(&mut rx).iter().filter_map(p2_kind));

    inv.abort();
    gpu.abort();
    fl.finish();
    P2Run {
        msgs,
        scans: scans.load(Ordering::SeqCst),
        fetches: fetches.load(Ordering::SeqCst),
        gpu_samples: gpu_samples.load(Ordering::SeqCst),
    }
}

/// ADR-008 for the P2 workers: hidden with no commands, 60 virtual s of zero work.
#[test]
fn p2_workers_idle_when_hidden() {
    let run = run_p2(Demand::default(), Duration::from_secs(60));
    // The Inventory worker sends one `Health(Running)` when its generation starts, hidden or
    // not. Nothing else is sent.
    assert_eq!(run.msgs, [P2::Other(SourceId::Inventory)]);
    assert_eq!(run.scans, 0, "inventory scans");
    assert_eq!(run.fetches, 0, "public IP fetches");
    assert_eq!(run.gpu_samples, 0, "GpuDrm fdinfo scans");
}

#[test]
fn p2_workers_demand_wakeup() {
    let demand = Demand {
        disks: true,
        nets: true,
        gpus: true,
        inventory: true,
        any_surface_visible: true,
        ..Demand::default()
    };
    let run = run_p2(demand, Duration::from_millis(9_990));
    let count = |kind: P2| run.msgs.iter().filter(|m| **m == kind).count();
    assert_eq!(count(P2::Inventory), 1, "{:?}", run.msgs);
    assert_eq!(run.scans, 1);
    assert_eq!(count(P2::Gpu), 10, "{:?}", run.msgs);
    assert_eq!(run.gpu_samples, 10);
}

// ---- KScreen display reader (hjn.40) ----

use std::time::SystemTime;

use tl_collect::inventory::read_inventory;
use tl_engine::workers::inventory::{add_devices, from_raw};
use tl_engine::workers::kscreen::{self, KScreenOutput};
use tl_model::snapshot::DisplayMode;

const KSCREEN_JSON: &[u8] = include_bytes!("fixtures/kscreen/doctor-j.json");

/// `dmi-desktop` with an AMD card1: DP-2 connected and enabled (3840x2160), HDMI-A-1 not.
fn kscreen_fixture() -> PathBuf {
    let dir = fixture_copy("dmi-desktop", None);
    let device = dir.join("sys/class/drm/card1/device");
    fs::create_dir_all(&device).expect("create card dir");
    fs::write(device.join("vendor"), "0x1002\n").expect("write vendor");
    fs::write(device.join("device"), "0x744c\n").expect("write device");
    fs::write(device.join("uevent"), "PCI_SLOT_NAME=0000:03:00.0\n").expect("write uevent");
    dir
}

/// The display rows of the fixture's one GPU, with `outputs` as the KScreen read.
fn displays_with(dir: &Path, outputs: Option<&[KScreenOutput]>) -> Vec<DisplayMode> {
    let root = SysRoot::at(dir);
    let raw = read_inventory(&InventoryPaths::under(dir));
    let mut snap = from_raw(&raw, &root);
    add_devices(&mut snap, &raw, &root, false, outputs);
    assert_eq!(snap.graphics.len(), 1);
    snap.graphics[0].displays.to_vec()
}

#[test]
fn kscreen_fills_display_rows() {
    let dir = kscreen_fixture();
    let sysfs = displays_with(&dir, None);
    let outputs = kscreen::parse(KSCREEN_JSON).expect("fixture parses");
    let rows = displays_with(&dir, Some(&outputs));
    assert_eq!(rows.len(), 1);
    let d = &rows[0];
    assert_eq!((d.width, d.height, d.refresh_mhz), (3840, 2160, 143_963));
    assert!(d.primary);
    assert_eq!(d.rotation, Some(1));
    assert_eq!(d.scale, Some(1.5));
    // Not in the KScreen JSON: KMS only (P4).
    assert_eq!(d.max_bpc, None);
    // From sysfs and EDID.
    assert_eq!(d.monitor_name, sysfs[0].monitor_name);
    assert_eq!(d.connector, sysfs[0].connector);
}

#[test]
fn kscreen_enabled_state_wins() {
    let dir = kscreen_fixture();
    let mut outputs = kscreen::parse(KSCREEN_JSON).expect("fixture parses");
    outputs[0].enabled = false;
    assert!(displays_with(&dir, Some(&outputs)).is_empty());
    // Sysfs says disabled, KScreen enabled.
    fs::write(dir.join("sys/class/drm/card1-DP-2/enabled"), "disabled\n").expect("write");
    assert!(displays_with(&dir, None).is_empty());
    outputs[0].enabled = true;
    assert_eq!(displays_with(&dir, Some(&outputs)).len(), 1);
}

#[test]
fn kscreen_missing_leaves_sysfs_values() {
    let dir = kscreen_fixture();
    let sysfs = displays_with(&dir, None);
    assert_eq!(sysfs, inventory_scan(&dir).graphics[0].displays.to_vec());
    // KScreen ran but does not know the connector.
    assert_eq!(displays_with(&dir, Some(&[])), sysfs);
    let d = &sysfs[0];
    assert_eq!((d.width, d.height, d.refresh_mhz), (3840, 2160, 0));
    assert!(!d.primary);
    assert_eq!((d.rotation, d.scale), (None, None));
}

/// Sets the modification time of `path` to `secs` after the epoch.
fn set_mtime(path: &Path, secs: u64) {
    let file = fs::File::options()
        .write(true)
        .open(path)
        .expect("open config");
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .expect("set mtime");
}

/// The GPU page's demand part: GPU samples and the inventory.
fn gpu_page() -> Demand {
    Demand {
        inventory: true,
        gpus: true,
        any_surface_visible: true,
        ..Demand::default()
    }
}

/// An inventory loop watching `watch` that counts its scans.
struct WatchRig {
    fl: FakeLab,
    demand_tx: asupersync::channel::watch::Sender<Demand>,
    rx: mpsc::Receiver<EngineMsg>,
    scans: Arc<AtomicU32>,
    task: asupersync::runtime::TaskHandle<()>,
}

impl WatchRig {
    fn start(dir: &Path, watch: PathBuf, first: Demand) -> Self {
        let mut fl = FakeLab::new();
        let (updates, rx, demand_tx, settings, control) = channels();
        demand_tx.send(first).expect("watch has receivers");
        let scans = Arc::new(AtomicU32::new(0));
        let scan = {
            let scans = Arc::clone(&scans);
            let dir = dir.to_path_buf();
            move || {
                scans.fetch_add(1, Ordering::SeqCst);
                inventory_scan(&dir)
            }
        };
        let inv_io = io(&updates, &demand_tx, &settings, &control);
        let task = fl.spawn(async move {
            let cx = Cx::current().expect("task has a Cx");
            let fetch_ip = || PublicIpState::NotFetched;
            let _ = inventory::run_with_watch(cx, inv_io, watch, scan, fetch_ip).await;
        });
        Self {
            fl,
            demand_tx,
            rx,
            scans,
            task,
        }
    }

    /// Sends `demand`, runs `secs` seconds and returns the scan count.
    fn show(&mut self, demand: Demand, secs: u64) -> u32 {
        self.demand_tx.send(demand).expect("watch has receivers");
        self.run(secs)
    }

    fn run(&mut self, secs: u64) -> u32 {
        self.fl
            .run_for(Duration::from_secs(secs), Duration::from_millis(10));
        self.scans.load(Ordering::SeqCst)
    }

    /// Checks that every scan sent one inventory message, and stops the loop.
    fn finish(mut self) {
        let kinds: Vec<_> = drain(&mut self.rx).iter().filter_map(p2_kind).collect();
        let inventory_msgs = kinds.iter().filter(|k| **k == P2::Inventory).count();
        let scans = self.scans.load(Ordering::SeqCst);
        assert_eq!(inventory_msgs, scans as usize, "{kinds:?}");
        self.task.abort();
        self.fl.finish();
    }
}

/// PERF-072: a KScreen config change rescans while a display surface is open, and is not
/// checked while it is hidden.
#[test]
fn kscreen_config_change_rescans() {
    let dir = fixture_copy("dmi-desktop", None);
    let config = dir.join("kwinoutputconfig.json");
    fs::write(&config, "{}").expect("write config");
    set_mtime(&config, 1_000);

    let mut rig = WatchRig::start(&dir, config.clone(), gpu_page());
    assert_eq!(rig.run(2), 1, "open");
    set_mtime(&config, 2_000);
    assert_eq!(rig.run(1), 2, "config change");
    assert_eq!(rig.run(2), 2, "no change");

    assert_eq!(rig.show(Demand::default(), 1), 2, "close");
    set_mtime(&config, 3_000);
    assert_eq!(rig.run(2), 2, "hidden");

    // The open scan covers the change made while hidden.
    assert_eq!(rig.show(gpu_page(), 2), 3, "reopen");
    rig.finish();
}

/// PERF-072: the GPU page opening runs KScreen even when the CPU page or the Memory page kept
/// the inventory visible, and the config is only watched while a display surface is open.
#[test]
fn gpu_page_open_scans_while_inventory_visible() {
    let dir = fixture_copy("dmi-desktop", None);
    let config = dir.join("kwinoutputconfig.json");
    fs::write(&config, "{}").expect("write config");
    set_mtime(&config, 1_000);
    let cpu_page = Demand {
        any_surface_visible: true,
        cpu: CpuDemand::Total,
        ..Demand::default()
    };
    let memory_page = Demand {
        any_surface_visible: true,
        memory: true,
        inventory: true,
        ..Demand::default()
    };
    let with_cpu = |d: Demand| Demand {
        cpu: CpuDemand::Total,
        ..d
    };

    let mut rig = WatchRig::start(&dir, config.clone(), cpu_page.clone());
    assert_eq!(rig.run(2), 1, "CPU page open");
    // A sidebar keeps CPU demand on while the GPU page opens.
    assert_eq!(
        rig.show(with_cpu(gpu_page()), 2),
        2,
        "GPU page open over CPU"
    );
    assert_eq!(rig.show(cpu_page.clone(), 2), 2, "GPU page closed");
    assert_eq!(
        rig.show(with_cpu(gpu_page()), 2),
        3,
        "GPU page reopen over CPU"
    );

    assert_eq!(rig.show(memory_page.clone(), 2), 3, "Memory page");
    // The Memory page alone does not watch the display config.
    set_mtime(&config, 2_000);
    assert_eq!(rig.run(2), 3, "config change, Memory page");
    assert_eq!(rig.show(gpu_page(), 2), 4, "GPU page open over Memory");
    set_mtime(&config, 3_000);
    assert_eq!(rig.run(1), 5, "config change, GPU page");
    rig.finish();
}
