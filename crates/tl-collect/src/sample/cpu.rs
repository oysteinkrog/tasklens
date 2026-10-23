//! CPU sampler: total, kernel, interrupt and per-core percentages, clocks and temperature from
//! /proc/stat, cpufreq and the CPU hwmon input (ADR-008, ADR-016, PERF-028, PERF-029, PERF-033,
//! PERF-039, PERF-040, PROC-012), and the 1 Hz live stats: load average, open files, process
//! count and uptime (PERF-033, PERF-034), and RAPL power (PERF-038).
//!
//! `sample_at` allocates nothing after the first sample: the two `StatData` buffers are swapped
//! and the output vectors keep their capacity. Only a CPU hotplug re-reads the topology.

use std::os::unix::ffi::OsStrExt;

use rustix::time::{ClockId, clock_gettime};
use tl_model::series::SampleFlags;
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::counters::{Clock, Measured, ratio_pct, resumed_between};
use crate::parse::cpufreq::{CpuFreq, FreqReading, FreqSource};
use crate::parse::filenr::parse_file_nr;
use crate::parse::hwmon::{self, parse_millidegrees};
use crate::parse::loadavg::{LoadAvg, parse_loadavg};
use crate::parse::rapl::{Rapl, RaplReading};
use crate::parse::stat::{CpuTimes, StatData, parse_stat};
use crate::parse::topology::{TopologyInfo, read_topology};
use crate::sysroot::{KeptFile, read_file};
use crate::{CollectError, SysRoot};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuOpts {
    pub per_cpu: bool,
    pub freq: bool,
    pub temp: bool,
    /// Load average, open files, process count and uptime. 1 Hz ticks only: it scans /proc.
    pub live_stats: bool,
    /// RAPL package and core power. Two `pread`s per sample.
    pub power: bool,
}

/// Byte limit for the one-shot read of /proc/sys/fs/file-nr.
const LIVE_FILE_LIMIT: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveStats {
    pub load: Measured<LoadAvg>,
    /// "Handles" in the UI: allocated file handles, system-wide (field 1 of file-nr).
    pub open_files: Measured<u64>,
    /// Entries of `root.proc` whose names are all ASCII digits.
    pub process_count: Measured<u32>,
    /// `Clock::boot_ns`. CLOCK_BOOTTIME counts time spent in suspend.
    pub uptime_ns: u64,
    /// CLOCK_REALTIME seconds minus `boot_ns / 1e9`.
    pub boot_time_unix_s: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CpuCoreReading {
    pub total_pct: Measured<f32>,
    pub kernel_pct: Measured<f32>,
    pub flags: SampleFlags,
}

#[derive(Clone, Debug, Default)]
pub struct CpuReading {
    /// CLOCK_BOOTTIME read just before reading /proc/stat.
    pub t_ns: TimeNs,
    /// GAP_BEFORE after resume or `reset_baselines`; RESET on a counter reset of the `cpu` line.
    pub flags: SampleFlags,
    /// 100 × Δbusy / Δtotal of the `cpu` line.
    pub total_pct: Measured<f32>,
    /// 100 × Δ(system + irq + softirq) / Δtotal.
    pub kernel_pct: Measured<f32>,
    /// 100 × Δ(irq + softirq) / Δtotal (PROC-012 "Interrupts" row).
    pub interrupts_pct: Measured<f32>,
    /// 100 × Δiowait / Δtotal (CPU details; counted as idle).
    pub iowait_pct: Measured<f32>,
    /// Index is the logical CPU id, length is the highest present id + 1. Empty when
    /// `!opts.per_cpu`.
    pub per_cpu: Vec<CpuCoreReading>,
    /// Default when `!opts.freq`.
    pub freq: FreqReading,
    /// Default when `!opts.temp`.
    pub temp_c: Measured<f32>,
    /// The set of `cpuN` lines changed since the previous sample.
    pub topology_changed: bool,
    pub online_cpus: u16,
    pub procs_running: u32,
    pub procs_blocked: u32,
    pub processes_forked: u64,
    pub btime: u64,
    /// Default when `!opts.live_stats`.
    pub live: LiveStats,
    /// Default when `!opts.power`.
    pub power: RaplReading,
}

pub struct CpuSampler {
    root: SysRoot,
    stat: KeptFile,
    /// Kept open from P2 (ADR-016 "System pressure"): it also feeds series CPU_LOAD1.
    loadavg: KeptFile,
    buf: Vec<u8>,
    /// The spare buffer each sample parses into.
    cur: StatData,
    /// The last successful parse. Its `per_cpu` ids are the CPU id set of the previous sample.
    prev: StatData,
    /// `prev` holds a parsed sample.
    have_prev: bool,
    /// `prev` is a usable rate baseline. False at start and after `reset_baselines`.
    baseline: bool,
    /// `reset_baselines` was called, so the next sample gets GAP_BEFORE.
    gap_pending: bool,
    prev_clock: Option<Clock>,
    freq: CpuFreq,
    /// The CPU temperature input, `None` without a CPU temperature chip.
    temp: Option<KeptFile>,
    topology: TopologyInfo,
    rapl: Rapl,
    /// Counter generation of the RAPL counters. Bumped on resume and `reset_baselines` (ADR-008).
    generation: u32,
}

/// Counter deltas of one `cpu` line between two samples.
struct Deltas {
    busy: u64,
    total: u64,
    kernel: u64,
    interrupts: u64,
    iowait: u64,
}

impl CpuSampler {
    /// Reads the topology, opens cpufreq and picks the temperature input. Fails only when
    /// `read_topology` fails.
    pub fn new(root: SysRoot) -> Result<Self, CollectError> {
        let topology = read_topology(&root)?;
        let freq = CpuFreq::open(&root);
        let rapl = Rapl::open(&root);
        let mut chips = Vec::new();
        let temp = match hwmon::enumerate(&root, &mut chips) {
            Ok(()) => hwmon::cpu_temp_input(&chips).and_then(|(i, n)| {
                chips
                    .get(i)
                    .map(|chip| KeptFile::new(hwmon::temp_input_path(chip, n)))
            }),
            Err(_) => None,
        };
        Ok(Self {
            stat: KeptFile::new(root.proc_path("stat")),
            loadavg: KeptFile::new(root.proc_path("loadavg")),
            root,
            buf: Vec::new(),
            cur: StatData::default(),
            prev: StatData::default(),
            have_prev: false,
            baseline: false,
            gap_pending: false,
            prev_clock: None,
            freq,
            temp,
            topology,
            rapl,
            generation: 0,
        })
    }

    pub fn sample(&mut self, opts: CpuOpts, out: &mut CpuReading) {
        self.sample_at(Clock::now(), opts, out);
    }

    pub fn sample_at(&mut self, clock: Clock, opts: CpuOpts, out: &mut CpuReading) {
        out.t_ns = clock.boot_ns;
        out.flags = SampleFlags::empty();
        out.topology_changed = false;
        out.live = if opts.live_stats {
            let realtime = clock_gettime(ClockId::Realtime);
            self.live_stats(clock, realtime.tv_sec)
        } else {
            LiveStats::default()
        };

        let resumed = self
            .prev_clock
            .is_some_and(|prev| resumed_between(prev, clock));
        if resumed {
            self.generation = self.generation.wrapping_add(1);
            self.rapl.reset();
        }
        if opts.power {
            self.rapl.read(clock, self.generation, &mut out.power);
        } else {
            out.power = RaplReading::default();
        }

        let parsed = self
            .stat
            .read(&mut self.buf)
            .and_then(|_| parse_stat(&self.buf, &mut self.cur));
        if let Err(err) = parsed {
            // The baseline in `prev` stays as it was.
            self.fill_invalid(err.validity(), opts, out);
            return;
        }

        if self.have_prev && !same_cpu_ids(&self.prev, &self.cur) {
            // Keep the old topology if the re-read fails.
            if let Ok(topology) = read_topology(&self.root) {
                self.topology = topology;
            }
            out.topology_changed = true;
        }

        let len = self.per_cpu_len();
        if resumed || !self.baseline {
            let flags = if resumed || self.gap_pending {
                SampleFlags::GAP_BEFORE
            } else {
                SampleFlags::empty()
            };
            out.flags = flags;
            let warming = Measured::invalid(Validity::WarmingUp);
            out.total_pct = warming;
            out.kernel_pct = warming;
            out.interrupts_pct = warming;
            out.iowait_pct = warming;
            if opts.per_cpu {
                reset_cores(&mut out.per_cpu, len);
                for (id, _) in &self.cur.per_cpu {
                    if let Some(core) = out.per_cpu.get_mut(usize::from(*id)) {
                        *core = warming_core(flags);
                    }
                }
            }
        } else {
            self.fill_percentages(opts, len, out);
        }
        if !opts.per_cpu {
            out.per_cpu.clear();
        }

        self.baseline = true;
        self.gap_pending = false;
        self.have_prev = true;
        std::mem::swap(&mut self.cur, &mut self.prev);
        self.prev_clock = Some(clock);

        let stat = &self.prev;
        out.procs_running = stat.procs_running;
        out.procs_blocked = stat.procs_blocked;
        out.processes_forked = stat.processes;
        out.btime = stat.btime;
        out.online_cpus = self.topology.logical_count;

        if opts.freq {
            self.freq.read(&mut out.freq);
        } else {
            out.freq.source = FreqSource::None;
            out.freq.avg_mhz = Measured::default();
            out.freq.max_mhz = Measured::default();
            out.freq.per_cpu_mhz.clear();
        }

        out.temp_c = if !opts.temp {
            Measured::default()
        } else if let Some(file) = &mut self.temp {
            match file
                .read(&mut self.buf)
                .and_then(|_| parse_millidegrees(&self.buf))
            {
                Ok(celsius) => Measured::valid(celsius),
                Err(err) => Measured::invalid(err.validity()),
            }
        } else {
            Measured::invalid(Validity::Unsupported)
        };
    }

    /// A `pread` of the kept loadavg, a one-shot read of file-nr and one pass over
    /// `root.proc`. Each field gets its own validity.
    fn live_stats(&mut self, clock: Clock, realtime_s: i64) -> LiveStats {
        let load = match self
            .loadavg
            .read(&mut self.buf)
            .and_then(|_| parse_loadavg(&self.buf))
        {
            Ok(load) => Measured::valid(load),
            Err(err) => Measured::invalid(err.validity()),
        };
        let open_files = match read_file(
            &self.root.proc_path("sys/fs/file-nr"),
            &mut self.buf,
            LIVE_FILE_LIMIT,
        )
        .and_then(|_| parse_file_nr(&self.buf))
        {
            Ok(n) => Measured::valid(n),
            Err(err) => Measured::invalid(err.validity()),
        };
        let process_count = match count_pid_dirs(&self.root) {
            Ok(n) => Measured::valid(n),
            Err(err) => Measured::invalid(err.validity()),
        };
        let boot_s = i64::try_from(clock.boot_ns / 1_000_000_000).unwrap_or(i64::MAX);
        LiveStats {
            load,
            open_files,
            process_count,
            uptime_ns: clock.boot_ns,
            boot_time_unix_s: realtime_s.saturating_sub(boot_s),
        }
    }

    /// ADR-008 ResetBaselines: the next sample is `WarmingUp` with GAP_BEFORE.
    pub fn reset_baselines(&mut self) {
        self.baseline = false;
        self.gap_pending = true;
        self.generation = self.generation.wrapping_add(1);
        self.rapl.reset();
    }

    pub fn topology(&self) -> &TopologyInfo {
        &self.topology
    }

    /// Highest CPU id in the topology or in the current parse, + 1.
    fn per_cpu_len(&self) -> usize {
        let present = self
            .topology
            .logical
            .iter()
            .map(|cpu| usize::from(cpu.id) + 1)
            .max()
            .unwrap_or(0);
        let parsed = self
            .cur
            .per_cpu
            .iter()
            .map(|(id, _)| usize::from(*id) + 1)
            .max()
            .unwrap_or(0);
        present.max(parsed)
    }

    /// Read or parse error: every percentage gets `validity`.
    fn fill_invalid(&self, validity: Validity, opts: CpuOpts, out: &mut CpuReading) {
        let invalid = Measured::invalid(validity);
        out.total_pct = invalid;
        out.kernel_pct = invalid;
        out.interrupts_pct = invalid;
        out.iowait_pct = invalid;
        if opts.per_cpu {
            let len = self
                .topology
                .logical
                .iter()
                .map(|cpu| usize::from(cpu.id) + 1)
                .max()
                .unwrap_or(0);
            out.per_cpu.clear();
            out.per_cpu.resize(
                len,
                CpuCoreReading {
                    total_pct: invalid,
                    kernel_pct: invalid,
                    flags: SampleFlags::empty(),
                },
            );
        } else {
            out.per_cpu.clear();
        }
    }

    /// Step 7: percentages from `prev` to `cur`.
    fn fill_percentages(&self, opts: CpuOpts, len: usize, out: &mut CpuReading) {
        match deltas(&self.prev.total, &self.cur.total) {
            Ok(d) => {
                out.total_pct = pct(d.busy, d.total);
                out.kernel_pct = pct(d.kernel, d.total);
                out.interrupts_pct = pct(d.interrupts, d.total);
                out.iowait_pct = pct(d.iowait, d.total);
            }
            Err(flags) => {
                out.flags |= flags;
                let warming = Measured::invalid(Validity::WarmingUp);
                out.total_pct = warming;
                out.kernel_pct = warming;
                out.interrupts_pct = warming;
                out.iowait_pct = warming;
            }
        }
        if !opts.per_cpu {
            return;
        }
        reset_cores(&mut out.per_cpu, len);
        for (id, cur) in &self.cur.per_cpu {
            let Some(core) = out.per_cpu.get_mut(usize::from(*id)) else {
                continue;
            };
            let prev = self
                .prev
                .per_cpu
                .binary_search_by_key(id, |(prev_id, _)| *prev_id)
                .ok()
                .and_then(|i| self.prev.per_cpu.get(i));
            *core = match prev {
                // The CPU just appeared.
                None => warming_core(SampleFlags::GAP_BEFORE),
                Some((_, prev)) => match deltas(prev, cur) {
                    Ok(d) => CpuCoreReading {
                        total_pct: pct(d.busy, d.total),
                        kernel_pct: pct(d.kernel, d.total),
                        flags: SampleFlags::empty(),
                    },
                    Err(flags) => warming_core(flags),
                },
            };
        }
    }
}

/// `Err(RESET)` when total or busy went backwards, `Err(empty)` when Δtotal is 0.
fn deltas(prev: &CpuTimes, cur: &CpuTimes) -> Result<Deltas, SampleFlags> {
    let (total, busy) = (cur.total(), cur.busy());
    if total < prev.total() || busy < prev.busy() {
        return Err(SampleFlags::RESET);
    }
    let total = total - prev.total();
    if total == 0 {
        return Err(SampleFlags::empty());
    }
    Ok(Deltas {
        busy: busy - prev.busy(),
        total,
        kernel: cur.kernel().saturating_sub(prev.kernel()),
        interrupts: cur.interrupts().saturating_sub(prev.interrupts()),
        // iowait can go backwards on Linux; that reads as 0.
        iowait: cur.iowait.saturating_sub(prev.iowait),
    })
}

fn pct(num: u64, den: u64) -> Measured<f32> {
    ratio_pct(num, den).map_or(Measured::invalid(Validity::WarmingUp), Measured::valid)
}

fn warming_core(flags: SampleFlags) -> CpuCoreReading {
    CpuCoreReading {
        total_pct: Measured::invalid(Validity::WarmingUp),
        kernel_pct: Measured::invalid(Validity::WarmingUp),
        flags,
    }
}

/// `len` entries, all `Missing`. CPUs without a `cpuN` line (offline) stay `Missing`.
fn reset_cores(cores: &mut Vec<CpuCoreReading>, len: usize) {
    let missing = Measured::invalid(Validity::Missing);
    cores.clear();
    cores.resize(
        len,
        CpuCoreReading {
            total_pct: missing,
            kernel_pct: missing,
            flags: SampleFlags::empty(),
        },
    );
}

/// Entries of `root.proc` whose names are non-empty and all ASCII digits.
fn count_pid_dirs(root: &SysRoot) -> Result<u32, CollectError> {
    let mut count = 0u32;
    for entry in std::fs::read_dir(&root.proc)? {
        let name = entry?.file_name();
        let name = name.as_bytes();
        if !name.is_empty() && name.iter().all(u8::is_ascii_digit) {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

fn same_cpu_ids(a: &StatData, b: &StatData) -> bool {
    a.per_cpu
        .iter()
        .map(|(id, _)| *id)
        .eq(b.per_cpu.iter().map(|(id, _)| *id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    const SEC: u64 = 1_000_000_000;
    const MS: u64 = 1_000_000;

    const ALL: CpuOpts = CpuOpts {
        per_cpu: true,
        freq: true,
        temp: true,
        live_stats: false,
        power: false,
    };
    const LIVE: CpuOpts = CpuOpts {
        per_cpu: false,
        freq: false,
        temp: false,
        live_stats: true,
        power: false,
    };
    const PER_CPU: CpuOpts = CpuOpts {
        per_cpu: true,
        freq: false,
        temp: false,
        live_stats: false,
        power: false,
    };

    fn fixture_dir(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn at(t: u64) -> Clock {
        Clock {
            boot_ns: t,
            mono_ns: t,
        }
    }

    fn copy_tree(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let to = dst.join(entry.file_name());
            let file_type = entry.file_type().unwrap();
            if file_type.is_symlink() {
                std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), &to).unwrap();
            } else if file_type.is_dir() {
                copy_tree(&entry.path(), &to);
            } else {
                // Truncate and rewrite, so a kept fd on an existing file sees the new content.
                fs::write(&to, fs::read(entry.path()).unwrap()).unwrap();
            }
        }
    }

    /// A tempdir holding `<name>/step<n>`.
    fn step_copy(name: &str, n: u32) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        apply_step(dir.path(), name, n);
        dir
    }

    /// Rewrites the files of `dir` with those of `<name>/step<n>`, in place.
    fn apply_step(dir: &Path, name: &str, n: u32) {
        copy_tree(&fixture_dir(name).join(format!("step{n}")), dir);
    }

    /// The `counter-reset` fixture has only /proc/stat, so give it a two-CPU topology.
    fn write_two_cpu_topology(dir: &Path) {
        let cpu = dir.join("sys/devices/system/cpu");
        fs::create_dir_all(&cpu).unwrap();
        fs::write(cpu.join("online"), "0-1\n").unwrap();
        fs::write(cpu.join("present"), "0-1\n").unwrap();
        for id in 0..2 {
            let topo = cpu.join(format!("cpu{id}/topology"));
            fs::create_dir_all(&topo).unwrap();
            fs::write(topo.join("core_id"), format!("{id}\n")).unwrap();
            fs::write(topo.join("physical_package_id"), "0\n").unwrap();
            fs::write(topo.join("thread_siblings_list"), format!("{id}\n")).unwrap();
        }
    }

    fn sampler(dir: &Path) -> CpuSampler {
        CpuSampler::new(SysRoot::at(dir)).unwrap()
    }

    fn assert_all_warming(r: &CpuReading) {
        for m in [r.total_pct, r.kernel_pct, r.interrupts_pct, r.iowait_pct] {
            assert_eq!(m.validity, Validity::WarmingUp);
        }
        for core in &r.per_cpu {
            assert_eq!(core.total_pct.validity, Validity::WarmingUp);
            assert_eq!(core.kernel_pct.validity, Validity::WarmingUp);
        }
    }

    #[test]
    fn first_sample_warming_up() {
        let dir = step_copy("cpu-math", 0);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), PER_CPU, &mut r);
        assert_all_warming(&r);
        assert_eq!(r.per_cpu.len(), 1);
        assert_eq!(r.flags, SampleFlags::empty());
        assert_eq!(r.t_ns, SEC);
    }

    #[test]
    fn cpu_math_two_samples() {
        let dir = step_copy("cpu-math", 0);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), PER_CPU, &mut r);
        apply_step(dir.path(), "cpu-math", 1);
        s.sample_at(at(2 * SEC), PER_CPU, &mut r);
        assert_eq!(r.total_pct, Measured::valid(70.0));
        assert_eq!(r.kernel_pct, Measured::valid(20.0));
        assert_eq!(r.interrupts_pct, Measured::valid(0.0));
        assert_eq!(r.iowait_pct, Measured::valid(0.0));
        assert_eq!(r.per_cpu[0].total_pct, Measured::valid(70.0));
        assert_eq!(r.per_cpu[0].kernel_pct, Measured::valid(20.0));
        assert_eq!(r.flags, SampleFlags::empty());
        assert_eq!(r.procs_running, 2);
        assert_eq!(r.processes_forked, 52);
        assert_eq!(r.btime, 1_700_000_000);
        assert_eq!(r.online_cpus, 1);
        assert!(!r.topology_changed);
    }

    #[test]
    fn counter_reset_flags() {
        let dir = step_copy("counter-reset", 0);
        write_two_cpu_topology(dir.path());
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), PER_CPU, &mut r);
        apply_step(dir.path(), "counter-reset", 1);
        s.sample_at(at(2 * SEC), PER_CPU, &mut r);
        assert_eq!(r.total_pct.validity, Validity::WarmingUp);
        assert!(r.flags.contains(SampleFlags::RESET));
        assert_eq!(r.per_cpu[0].total_pct.validity, Validity::WarmingUp);
        assert_eq!(r.per_cpu[0].flags, SampleFlags::RESET);
        assert_eq!(r.per_cpu[1].total_pct.validity, Validity::Valid);
        assert_eq!(r.per_cpu[1].flags, SampleFlags::empty());
    }

    #[test]
    fn hotplug_missing_and_topology_changed() {
        let dir = step_copy("cpu-hotplug", 0);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), PER_CPU, &mut r);
        assert_eq!(r.online_cpus, 4);
        assert!(!r.topology_changed);
        apply_step(dir.path(), "cpu-hotplug", 1);
        s.sample_at(at(2 * SEC), PER_CPU, &mut r);
        assert!(r.topology_changed);
        assert_eq!(r.per_cpu.len(), 4);
        assert_eq!(r.per_cpu[2].total_pct.validity, Validity::Missing);
        for id in [0, 1, 3] {
            assert_eq!(r.per_cpu[id].total_pct.validity, Validity::Valid);
        }
        assert_eq!(r.online_cpus, 3);
        assert_eq!(s.topology().logical_count, 3);
    }

    #[test]
    fn resume_gap_then_valid() {
        let dir = step_copy("cpu-math", 0);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), PER_CPU, &mut r);
        apply_step(dir.path(), "cpu-math", 1);
        // 5 s of boottime, 10 ms of monotonic time: the machine slept.
        let resumed = Clock {
            boot_ns: 6 * SEC,
            mono_ns: SEC + 10 * MS,
        };
        s.sample_at(resumed, PER_CPU, &mut r);
        assert_all_warming(&r);
        assert_eq!(r.flags, SampleFlags::GAP_BEFORE);
        assert_eq!(r.per_cpu[0].flags, SampleFlags::GAP_BEFORE);

        fs::write(
            dir.path().join("proc/stat"),
            "cpu  200 0 90 860 10 0 0 0 0 0\ncpu0 200 0 90 860 10 0 0 0 0 0\n",
        )
        .unwrap();
        let next = Clock {
            boot_ns: 7 * SEC,
            mono_ns: 2 * SEC + 10 * MS,
        };
        s.sample_at(next, PER_CPU, &mut r);
        assert_eq!(r.flags, SampleFlags::empty());
        assert_eq!(r.total_pct, Measured::valid(70.0));
        assert_eq!(r.per_cpu[0].total_pct, Measured::valid(70.0));
    }

    #[test]
    fn reset_baselines_gap() {
        let dir = step_copy("cpu-math", 0);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), PER_CPU, &mut r);
        apply_step(dir.path(), "cpu-math", 1);
        s.sample_at(at(2 * SEC), PER_CPU, &mut r);
        assert_eq!(r.total_pct.validity, Validity::Valid);

        s.reset_baselines();
        s.sample_at(at(3 * SEC), PER_CPU, &mut r);
        assert_all_warming(&r);
        assert_eq!(r.flags, SampleFlags::GAP_BEFORE);
    }

    #[test]
    fn per_cpu_toggle_keeps_baseline() {
        let dir = step_copy("cpu-math", 0);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), CpuOpts::default(), &mut r);
        assert!(r.per_cpu.is_empty());
        apply_step(dir.path(), "cpu-math", 1);
        s.sample_at(at(2 * SEC), PER_CPU, &mut r);
        assert_eq!(r.per_cpu.len(), 1);
        assert_eq!(r.per_cpu[0].total_pct, Measured::valid(70.0));
    }

    #[test]
    fn owner_desktop_temp_is_tctl() {
        let mut s = CpuSampler::new(SysRoot::at(&fixture_dir("owner-desktop"))).unwrap();
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), ALL, &mut r);
        let tctl = fs::read_to_string(
            fixture_dir("owner-desktop").join("sys/class/hwmon/hwmon2/temp1_input"),
        )
        .unwrap();
        let expected = tctl.trim().parse::<f32>().unwrap() / 1000.0;
        assert_eq!(r.temp_c, Measured::valid(expected));
        assert_eq!(r.temp_c, Measured::valid(56.75));
    }

    #[test]
    fn no_temp_chip_unsupported() {
        let dir = step_copy("cpu-math", 0);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), ALL, &mut r);
        assert_eq!(r.temp_c.validity, Validity::Unsupported);
    }

    #[test]
    fn unreadable_stat_restricted() {
        if rustix::process::geteuid().is_root() {
            return;
        }
        let dir = step_copy("cpu-math", 0);
        let stat = dir.path().join("proc/stat");
        fs::set_permissions(&stat, fs::Permissions::from_mode(0o000)).unwrap();
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), PER_CPU, &mut r);
        assert_eq!(r.total_pct.validity, Validity::Restricted);
        assert_eq!(r.kernel_pct.validity, Validity::Restricted);
        assert_eq!(r.per_cpu[0].total_pct.validity, Validity::Restricted);
    }

    #[test]
    fn owner_desktop_snapshot() {
        let mut s = CpuSampler::new(SysRoot::at(&fixture_dir("owner-desktop"))).unwrap();
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), ALL, &mut r);
        insta::with_settings!({ snapshot_path => "../../tests/snapshots" }, {
            insta::assert_debug_snapshot!((s.topology(), &r));
        });
    }
    fn now_unix_s() -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        i64::try_from(now.as_secs()).unwrap()
    }

    #[test]
    fn live_stats_owner_desktop() {
        let mut s = CpuSampler::new(SysRoot::at(&fixture_dir("owner-desktop"))).unwrap();
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), LIVE, &mut r);
        let proc_dir = fixture_dir("owner-desktop").join("proc");
        let load = parse_loadavg(&fs::read(proc_dir.join("loadavg")).unwrap()).unwrap();
        let files = parse_file_nr(&fs::read(proc_dir.join("sys/fs/file-nr")).unwrap()).unwrap();
        assert_eq!(r.live.process_count, Measured::valid(50));
        assert_eq!(r.live.load, Measured::valid(load));
        assert_eq!(r.live.load.value.total, 6667);
        assert_eq!(r.live.open_files, Measured::valid(files));
        assert_eq!(r.live.open_files, Measured::valid(26143));
    }

    #[test]
    fn live_stats_off_keeps_default() {
        let mut s = CpuSampler::new(SysRoot::at(&fixture_dir("owner-desktop"))).unwrap();
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), PER_CPU, &mut r);
        assert_eq!(r.live, LiveStats::default());
    }

    #[test]
    fn uptime_and_boot_time() {
        let mut s = CpuSampler::new(SysRoot::at(&fixture_dir("owner-desktop"))).unwrap();
        let mut r = CpuReading::default();
        s.sample_at(at(90_000 * SEC), LIVE, &mut r);
        assert_eq!(r.live.uptime_ns, 90_000_000_000_000);
        let expected = now_unix_s() - 90_000;
        assert!(
            (r.live.boot_time_unix_s - expected).abs() <= 5,
            "{} vs {expected}",
            r.live.boot_time_unix_s
        );
    }

    const POWER: CpuOpts = CpuOpts {
        per_cpu: false,
        freq: false,
        temp: false,
        live_stats: false,
        power: true,
    };

    /// `cpu-math` step 0 plus a package zone holding `energy` µJ.
    fn power_copy(energy: u64) -> tempfile::TempDir {
        let dir = step_copy("cpu-math", 0);
        let zone = dir.path().join("sys/class/powercap/intel-rapl:0");
        fs::create_dir_all(&zone).unwrap();
        fs::write(zone.join("max_energy_range_uj"), "262143328850\n").unwrap();
        set_energy(dir.path(), energy);
        dir
    }

    /// Rewrites in place so the kept fd sees the new value.
    fn set_energy(dir: &Path, energy: u64) {
        let path = dir.join("sys/class/powercap/intel-rapl:0/energy_uj");
        fs::write(path, format!("{energy}\n")).unwrap();
    }

    #[test]
    fn power_watts() {
        let dir = power_copy(1_000_000);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), POWER, &mut r);
        assert_eq!(r.power.package_w.validity, Validity::WarmingUp);
        apply_step(dir.path(), "cpu-math", 1);
        set_energy(dir.path(), 6_000_000);
        s.sample_at(at(2 * SEC), POWER, &mut r);
        assert_eq!(r.power.package_w, Measured::valid(5.0));
        assert_eq!(r.power.core_w.validity, Validity::Unsupported);
    }

    #[test]
    fn power_reset_on_reset_baselines() {
        let dir = power_copy(1_000_000);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), POWER, &mut r);
        set_energy(dir.path(), 6_000_000);
        s.sample_at(at(2 * SEC), POWER, &mut r);
        assert_eq!(r.power.package_w, Measured::valid(5.0));

        s.reset_baselines();
        set_energy(dir.path(), 11_000_000);
        s.sample_at(at(3 * SEC), POWER, &mut r);
        assert_eq!(r.power.package_w.validity, Validity::WarmingUp);
    }

    #[test]
    fn power_reset_on_resume() {
        let dir = power_copy(1_000_000);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), POWER, &mut r);
        set_energy(dir.path(), 6_000_000);
        // 5 s of boottime, 10 ms of monotonic time: the machine slept.
        let resumed = Clock {
            boot_ns: 6 * SEC,
            mono_ns: SEC + 10 * MS,
        };
        s.sample_at(resumed, POWER, &mut r);
        assert_eq!(r.power.package_w.validity, Validity::WarmingUp);
    }

    #[test]
    fn power_off_default() {
        let dir = power_copy(1_000_000);
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), PER_CPU, &mut r);
        set_energy(dir.path(), 6_000_000);
        s.sample_at(at(2 * SEC), PER_CPU, &mut r);
        assert_eq!(r.power, RaplReading::default());
    }

    #[test]
    fn missing_loadavg_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        copy_tree(&fixture_dir("owner-desktop"), dir.path());
        fs::remove_file(dir.path().join("proc/loadavg")).unwrap();
        let mut s = sampler(dir.path());
        let mut r = CpuReading::default();
        s.sample_at(at(SEC), LIVE, &mut r);
        assert_eq!(r.live.load.validity, Validity::Missing);
        assert_eq!(r.live.open_files, Measured::valid(26143));
        assert_eq!(r.live.process_count, Measured::valid(50));
    }
}
