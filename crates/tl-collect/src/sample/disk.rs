//! Disk sampler: per-device active time, read and write rates, average response time and totals
//! since boot from one read of `/proc/diskstats` (ADR-016, ADR-008, PERF-048, PERF-049, R-052).
use rustc_hash::FxHashMap;
use tl_model::series::SampleFlags;
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::SysRoot;
use crate::counters::{Clock, Counter, Delta, Measured, resumed_between};
use crate::parse::diskstats::{DiskStatLine, parse_diskstats};
use crate::sysroot::KeptFile;

/// Bytes per /proc/diskstats sector, whatever the device's real sector size.
const SECTOR_BYTES: u64 = 512;

/// One disk or child as the enumeration saw it. A new `generation` means a new device with the
/// same numbers, so its counters start over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskId {
    pub major: u32,
    pub minor: u32,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DiskDevReading {
    pub major: u32,
    pub minor: u32,
    /// Share of the interval with I/O in flight, 0 to 100.
    pub active_pct: Measured<f32>,
    pub read_bps: Measured<f64>,
    pub write_bps: Measured<f64>,
    /// Average time per completed read or write in the interval (ADR-016).
    pub latency_ms: Measured<f32>,
    /// Raw counters since boot (PERF-049).
    pub total_read_bytes: Measured<u64>,
    pub total_written_bytes: Measured<u64>,
    pub flags: SampleFlags,
}

#[derive(Clone, Debug, Default)]
pub struct DiskReading {
    pub t_ns: TimeNs,
    /// Of the `/proc/diskstats` read.
    pub validity: Validity,
    /// In the order of `set_devices`.
    pub devices: Vec<DiskDevReading>,
}

/// One device with the counters its rates come from.
#[derive(Default)]
struct DevCounters {
    io_ms: Counter,
    sectors_read: Counter,
    sectors_written: Counter,
    /// read_ms + write_ms.
    rw_ms: Counter,
    /// reads + writes.
    ios: Counter,
}

impl DevCounters {
    fn reset(&mut self) {
        self.io_ms.reset();
        self.sectors_read.reset();
        self.sectors_written.reset();
        self.rw_ms.reset();
        self.ios.reset();
    }
}

struct DevState {
    id: DiskId,
    counters: DevCounters,
}

pub struct DiskSampler {
    stats: KeptFile,
    buf: Vec<u8>,
    lines: Vec<DiskStatLine>,
    /// (major, minor) to index in `lines`, rebuilt each tick.
    line_index: FxHashMap<(u32, u32), usize>,
    devices: Vec<DevState>,
    prev_clock: Option<Clock>,
    /// Set by a resume or `reset_baselines`. Cleared by the next successful read.
    gap_pending: bool,
}

impl DiskSampler {
    pub fn new(root: SysRoot) -> Self {
        Self {
            stats: KeptFile::new(root.proc_path("diskstats")),
            buf: Vec::new(),
            lines: Vec::new(),
            line_index: FxHashMap::default(),
            devices: Vec::new(),
            prev_clock: None,
            gap_pending: false,
        }
    }

    /// Called after each enumeration with every disk and child; generation comes from the device
    /// catalog. A device whose (major, minor) is still present keeps its counters; a changed
    /// generation makes its next rates `WarmingUp` with `GAP_BEFORE`.
    pub fn set_devices(&mut self, devices: &[DiskId]) {
        let mut old = std::mem::take(&mut self.devices);
        self.devices = devices
            .iter()
            .map(|id| {
                let counters = old
                    .iter()
                    .position(|s| s.id.major == id.major && s.id.minor == id.minor)
                    .map(|i| old.swap_remove(i).counters)
                    .unwrap_or_default();
                DevState { id: *id, counters }
            })
            .collect();
    }

    pub fn sample(&mut self, out: &mut DiskReading) {
        self.sample_at(Clock::now(), out);
    }

    pub fn sample_at(&mut self, clock: Clock, out: &mut DiskReading) {
        if let Some(prev) = self.prev_clock
            && resumed_between(prev, clock)
        {
            self.gap_pending = true;
        }
        self.prev_clock = Some(clock);
        out.t_ns = clock.boot_ns;

        let read = self
            .stats
            .read(&mut self.buf)
            .and_then(|_| parse_diskstats(&self.buf, &mut self.lines));
        out.validity = match &read {
            Ok(()) => Validity::Valid,
            Err(err) => err.validity(),
        };
        let gap = read.is_ok() && self.gap_pending;
        if gap {
            self.gap_pending = false;
            for state in &mut self.devices {
                state.counters.reset();
            }
        }

        self.line_index.clear();
        if read.is_ok() {
            for (i, line) in self.lines.iter().enumerate() {
                self.line_index.insert((line.major, line.minor), i);
            }
        }

        out.devices.truncate(self.devices.len());
        out.devices
            .resize_with(self.devices.len(), DiskDevReading::default);
        let t = clock.boot_ns;
        for (state, reading) in self.devices.iter_mut().zip(out.devices.iter_mut()) {
            let id = state.id;
            reading.major = id.major;
            reading.minor = id.minor;
            if read.is_err() {
                fill_invalid(reading, out.validity);
                continue;
            }
            let Some(line) = self
                .line_index
                .get(&(id.major, id.minor))
                .and_then(|&i| self.lines.get(i))
            else {
                fill_invalid(reading, Validity::Missing);
                continue;
            };

            let generation = id.generation;
            let c = &mut state.counters;
            let io = c.io_ms.step(line.io_ms, t, generation);
            let rd = c.sectors_read.step(line.sectors_read, t, generation);
            let wr = c.sectors_written.step(line.sectors_written, t, generation);
            let rw_ms = c
                .rw_ms
                .step(line.read_ms.saturating_add(line.write_ms), t, generation);
            let ios = c
                .ios
                .step(line.reads.saturating_add(line.writes), t, generation);

            reading.total_read_bytes =
                Measured::valid(line.sectors_read.saturating_mul(SECTOR_BYTES));
            reading.total_written_bytes =
                Measured::valid(line.sectors_written.saturating_mul(SECTOR_BYTES));

            match (io, rd, wr, rw_ms, ios) {
                (
                    Delta::Ok { delta: d_io, dt_ns },
                    Delta::Ok { delta: d_rd, .. },
                    Delta::Ok { delta: d_wr, .. },
                    Delta::Ok { delta: d_rw_ms, .. },
                    Delta::Ok { delta: d_ios, .. },
                ) => {
                    let dt_ms = dt_ns as f64 / 1e6;
                    let dt_s = dt_ns as f64 / 1e9;
                    let active = (100.0 * d_io as f64 / dt_ms).clamp(0.0, 100.0);
                    reading.active_pct = Measured::valid(active as f32);
                    reading.read_bps =
                        Measured::valid(d_rd.saturating_mul(SECTOR_BYTES) as f64 / dt_s);
                    reading.write_bps =
                        Measured::valid(d_wr.saturating_mul(SECTOR_BYTES) as f64 / dt_s);
                    let latency = if d_ios == 0 {
                        0.0
                    } else {
                        (d_rw_ms as f64 / d_ios as f64) as f32
                    };
                    reading.latency_ms = Measured::valid(latency);
                    reading.flags = SampleFlags::VALID;
                }
                _ => {
                    let flags = [io, rd, wr, rw_ms, ios].into_iter().fold(
                        SampleFlags::empty(),
                        |acc, d| match d {
                            Delta::WarmingUp { flags } => acc | flags,
                            Delta::Ok { .. } => acc,
                        },
                    );
                    reading.active_pct = Measured::invalid(Validity::WarmingUp);
                    reading.read_bps = Measured::invalid(Validity::WarmingUp);
                    reading.write_bps = Measured::invalid(Validity::WarmingUp);
                    reading.latency_ms = Measured::invalid(Validity::WarmingUp);
                    reading.flags = flags;
                }
            }
            if gap {
                reading.flags = SampleFlags::GAP_BEFORE;
            }
        }
    }

    /// The next rates are `WarmingUp` and every device has `flags = GAP_BEFORE`.
    pub fn reset_baselines(&mut self) {
        self.gap_pending = true;
    }
}

/// Every field of `reading` gets `validity`, with `flags` empty.
fn fill_invalid(reading: &mut DiskDevReading, validity: Validity) {
    reading.active_pct = Measured::invalid(validity);
    reading.read_bps = Measured::invalid(validity);
    reading.write_bps = Measured::invalid(validity);
    reading.latency_ms = Measured::invalid(validity);
    reading.total_read_bytes = Measured::invalid(validity);
    reading.total_written_bytes = Measured::invalid(validity);
    reading.flags = SampleFlags::empty();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    const SEC: u64 = 1_000_000_000;

    fn step_file(step: u32) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/disk-two-steps")
            .join(format!("step{step}/proc/diskstats"))
    }

    /// Writes the diskstats of `step` over the file in `to`. Writing in place keeps the inode, so
    /// the sampler's kept descriptor sees the new contents.
    fn load_step(to: &Path, step: u32) {
        fs::create_dir_all(to.join("proc")).unwrap();
        fs::write(
            to.join("proc/diskstats"),
            fs::read(step_file(step)).unwrap(),
        )
        .unwrap();
    }

    fn at(secs: u64) -> Clock {
        Clock {
            boot_ns: secs * SEC,
            mono_ns: secs * SEC,
        }
    }

    fn nvme(generation: u32) -> DiskId {
        DiskId {
            major: 259,
            minor: 0,
            generation,
        }
    }

    fn sampler(dir: &Path, devices: &[DiskId]) -> DiskSampler {
        let mut s = DiskSampler::new(SysRoot::at(dir));
        s.set_devices(devices);
        s
    }

    fn assert_rates(r: &DiskDevReading, validity: Validity) {
        assert_eq!(r.active_pct.validity, validity);
        assert_eq!(r.read_bps.validity, validity);
        assert_eq!(r.write_bps.validity, validity);
        assert_eq!(r.latency_ms.validity, validity);
    }

    #[test]
    fn first_sample_warming_up() {
        let dir = tempfile::tempdir().unwrap();
        load_step(dir.path(), 0);
        let mut s = sampler(dir.path(), &[nvme(0)]);
        let mut out = DiskReading::default();
        s.sample_at(at(0), &mut out);
        assert_eq!(out.validity, Validity::Valid);
        let r = &out.devices[0];
        assert_eq!((r.major, r.minor), (259, 0));
        assert_rates(r, Validity::WarmingUp);
        assert_eq!(r.total_read_bytes, Measured::valid(100_000 * 512));
        assert_eq!(r.total_written_bytes, Measured::valid(200_000 * 512));
    }

    #[test]
    fn rates_and_latency() {
        let dir = tempfile::tempdir().unwrap();
        load_step(dir.path(), 0);
        let mut s = sampler(dir.path(), &[nvme(0)]);
        let mut out = DiskReading::default();
        s.sample_at(at(0), &mut out);
        load_step(dir.path(), 1);
        s.sample_at(at(1), &mut out);
        let r = &out.devices[0];
        assert_eq!(out.t_ns, SEC);
        assert_eq!(r.active_pct, Measured::valid(50.0));
        assert_eq!(r.read_bps, Measured::valid(1_048_576.0));
        assert_eq!(r.write_bps, Measured::valid(2_097_152.0));
        assert_eq!(r.latency_ms, Measured::valid(3.0));
        assert_eq!(r.total_read_bytes, Measured::valid(102_048 * 512));
        assert_eq!(r.flags, SampleFlags::VALID);
    }

    #[test]
    fn counter_reset() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = sampler(dir.path(), &[nvme(0)]);
        let mut out = DiskReading::default();
        for step in 0..=2 {
            load_step(dir.path(), step);
            s.sample_at(at(u64::from(step)), &mut out);
        }
        let r = &out.devices[0];
        assert_rates(r, Validity::WarmingUp);
        assert!(r.flags.contains(SampleFlags::RESET));
        assert_eq!(r.total_read_bytes, Measured::valid(50_000 * 512));
    }

    #[test]
    fn generation_change_gap() {
        let dir = tempfile::tempdir().unwrap();
        load_step(dir.path(), 0);
        let mut s = sampler(dir.path(), &[nvme(0)]);
        let mut out = DiskReading::default();
        s.sample_at(at(0), &mut out);
        s.set_devices(&[nvme(1)]);
        load_step(dir.path(), 1);
        s.sample_at(at(1), &mut out);
        let r = &out.devices[0];
        assert_rates(r, Validity::WarmingUp);
        assert!(r.flags.contains(SampleFlags::GAP_BEFORE));
    }

    #[test]
    fn missing_device() {
        let dir = tempfile::tempdir().unwrap();
        load_step(dir.path(), 0);
        let absent = DiskId {
            major: 8,
            minor: 99,
            generation: 0,
        };
        let mut s = sampler(dir.path(), &[absent, nvme(0)]);
        let mut out = DiskReading::default();
        s.sample_at(at(0), &mut out);
        let r = &out.devices[0];
        assert_eq!((r.major, r.minor), (8, 99));
        assert_rates(r, Validity::Missing);
        assert_eq!(r.total_read_bytes.validity, Validity::Missing);
        assert_eq!(r.total_written_bytes.validity, Validity::Missing);
        assert_eq!((out.devices[1].major, out.devices[1].minor), (259, 0));
        assert_eq!(
            out.devices[1].total_read_bytes,
            Measured::valid(100_000 * 512)
        );
    }

    #[test]
    fn resume_gap() {
        let dir = tempfile::tempdir().unwrap();
        load_step(dir.path(), 0);
        let mut s = sampler(dir.path(), &[nvme(0)]);
        let mut out = DiskReading::default();
        s.sample_at(at(0), &mut out);
        load_step(dir.path(), 1);
        // Boot time moved 10 s, monotonic time 1 s: the machine slept.
        s.sample_at(
            Clock {
                boot_ns: 10 * SEC,
                mono_ns: SEC,
            },
            &mut out,
        );
        let r = &out.devices[0];
        assert_rates(r, Validity::WarmingUp);
        assert_eq!(r.flags, SampleFlags::GAP_BEFORE);
    }

    #[test]
    fn reset_baselines_gap() {
        let dir = tempfile::tempdir().unwrap();
        load_step(dir.path(), 0);
        let mut s = sampler(dir.path(), &[nvme(0)]);
        let mut out = DiskReading::default();
        s.sample_at(at(0), &mut out);
        s.reset_baselines();
        load_step(dir.path(), 1);
        s.sample_at(at(1), &mut out);
        assert_rates(&out.devices[0], Validity::WarmingUp);
        assert_eq!(out.devices[0].flags, SampleFlags::GAP_BEFORE);
    }

    #[test]
    fn idle_disk_zero() {
        let dir = tempfile::tempdir().unwrap();
        load_step(dir.path(), 0);
        let mut s = sampler(dir.path(), &[nvme(0)]);
        let mut out = DiskReading::default();
        s.sample_at(at(0), &mut out);
        load_step(dir.path(), 1);
        s.sample_at(at(1), &mut out);
        s.sample_at(at(2), &mut out);
        let r = &out.devices[0];
        assert_eq!(r.active_pct, Measured::valid(0.0));
        assert_eq!(r.latency_ms, Measured::valid(0.0));
        assert_eq!(r.read_bps, Measured::valid(0.0));
        assert_eq!(r.flags, SampleFlags::VALID);
    }

    #[test]
    fn read_failure_sets_validity() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = sampler(dir.path(), &[nvme(0)]);
        let mut out = DiskReading::default();
        s.sample_at(at(0), &mut out);
        assert_ne!(out.validity, Validity::Valid);
        let r = &out.devices[0];
        assert_rates(r, out.validity);
        assert_eq!(r.total_read_bytes.validity, out.validity);
        assert_eq!(r.total_written_bytes.validity, out.validity);
    }
}
