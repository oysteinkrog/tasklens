//! Memory and swap sampler: used memory, commit, swap, the five composition segments, savings
//! and zram rates (ADR-008, PERF-041 to PERF-043, PROC-012, ADR-016).
use std::path::PathBuf;
use std::sync::Arc;

use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::counters::{Clock, Counter, Measured, Rate, resumed_between};
use crate::parse::meminfo::{MemInfo, parse_meminfo};
use crate::parse::swaps::{SwapEntry, parse_swaps};
use crate::parse::zram::{self, ZramDevice, ZramMmStat};
use crate::parse::zswap::{ZswapParams, read_zswap};
use crate::sysroot::{KeptFile, read_file};
use crate::{CollectError, SysRoot};

/// Bytes per block layer sector in `/sys/block/<d>/stat`.
const SECTOR_BYTES: u64 = 512;

/// Read limit for `/proc/swaps`.
const SWAPS_LIMIT: usize = 64 * 1024;

/// What a memory sample reads besides meminfo and zram.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemOpts {
    /// Swap devices and zswap parameters (P2 memory details). Only while details are shown.
    pub details: bool,
}

/// The five memory composition segments in bytes (PERF-042).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Composition {
    pub in_use: u64,
    pub compressed: u64,
    pub modified: u64,
    pub standby: u64,
    pub free: u64,
}

#[derive(Clone, Debug)]
pub struct ZramReading {
    pub name: Arc<str>,
    pub disksize_bytes: u64,
    pub algorithm: Option<Arc<str>>,
    pub mm: Measured<ZramMmStat>,
    /// Bytes per second from sectors × 512.
    pub read_bps: Rate,
    pub write_bps: Rate,
}

impl Default for ZramReading {
    fn default() -> Self {
        Self {
            name: Arc::from(""),
            disksize_bytes: 0,
            algorithm: None,
            mm: Measured::default(),
            read_bps: warming_up(),
            write_bps: warming_up(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MemoryReading {
    pub t_ns: TimeNs,
    /// Of the meminfo-derived fields.
    pub validity: Validity,
    pub info: MemInfo,
    pub used_bytes: u64,
    /// `Unsupported` when CommitLimit is 0. Not clamped, so it can exceed 100.
    pub commit_pct: Measured<f32>,
    pub swap_used_bytes: u64,
    pub composition: Composition,
    pub zram: Vec<ZramReading>,
    /// Σ mm.mem_used_total over Valid devices.
    pub zram_mem_used_total: u64,
    pub savings_bytes: i64,
    /// From `/proc/swaps`. Updated only by detailed samples.
    pub swaps: Measured<Vec<SwapEntry>>,
    /// `None` when zswap is not built. Updated only by detailed samples.
    pub zswap: Measured<Option<ZswapParams>>,
}

/// One zram device with its read and write sector counters.
struct ZramState {
    dev: ZramDevice,
    read: Counter,
    write: Counter,
}

pub struct MemorySampler {
    root: SysRoot,
    meminfo: KeptFile,
    zram: Vec<ZramState>,
    buf: Vec<u8>,
    swaps_path: PathBuf,
    /// Read in `new` and `rescan_zram`.
    zswap: Measured<Option<ZswapParams>>,
    prev_clock: Option<Clock>,
    /// Counter generation. Bumped on resume and `reset_baselines` (ADR-008).
    generation: u32,
}

impl MemorySampler {
    pub fn new(root: SysRoot) -> Self {
        let meminfo = KeptFile::new(root.proc_path("meminfo"));
        let swaps_path = root.proc_path("swaps");
        let mut sampler = Self {
            root,
            meminfo,
            zram: Vec::new(),
            buf: Vec::new(),
            swaps_path,
            zswap: Measured::default(),
            prev_clock: None,
            generation: 0,
        };
        sampler.rescan_zram();
        sampler
    }

    pub fn sample(&mut self, opts: MemOpts, out: &mut MemoryReading) {
        self.sample_at(Clock::now(), opts, out);
    }

    pub fn sample_at(&mut self, clock: Clock, opts: MemOpts, out: &mut MemoryReading) {
        if let Some(prev) = self.prev_clock
            && resumed_between(prev, clock)
        {
            self.generation = self.generation.wrapping_add(1);
        }
        self.prev_clock = Some(clock);
        out.t_ns = clock.boot_ns;

        let meminfo = self
            .meminfo
            .read(&mut self.buf)
            .and_then(|_| parse_meminfo(&self.buf, &mut out.info));
        out.validity = match &meminfo {
            Ok(()) => Validity::Valid,
            Err(err) => {
                out.info = MemInfo::default();
                err.validity()
            }
        };

        self.sample_zram(clock, out);
        if opts.details {
            self.sample_details(out);
        }

        let info = out.info;
        let zswap = info.zswap.unwrap_or(0);
        let zswapped = info.zswapped.unwrap_or(0);
        let (orig, compr) = out
            .zram
            .iter()
            .filter(|z| z.mm.validity == Validity::Valid)
            .fold((0u64, 0u64), |(o, c), z| {
                (
                    o.saturating_add(z.mm.value.orig_data_size),
                    c.saturating_add(z.mm.value.compr_data_size),
                )
            });
        out.savings_bytes =
            to_i64(orig.saturating_add(zswapped)) - to_i64(compr.saturating_add(zswap));

        if meminfo.is_err() {
            out.used_bytes = 0;
            out.commit_pct = Measured::invalid(out.validity);
            out.swap_used_bytes = 0;
            out.composition = Composition::default();
            return;
        }

        // PERF-041: MemFree stands in when MemAvailable is larger than MemTotal.
        let avail = if info.mem_available > info.mem_total {
            info.mem_free
        } else {
            info.mem_available
        };
        let used = info.mem_total.saturating_sub(avail);
        out.used_bytes = used;
        out.commit_pct = if info.commit_limit == 0 {
            Measured::invalid(Validity::Unsupported)
        } else {
            Measured::valid((100.0 * info.committed_as as f64 / info.commit_limit as f64) as f32)
        };
        out.swap_used_bytes = info.swap_total.saturating_sub(info.swap_free);

        let compressed = out.zram_mem_used_total.saturating_add(zswap).min(used);
        let in_use = used - compressed;
        let modified = info.dirty.saturating_add(info.writeback);
        let free = info.mem_free;
        let standby = info
            .mem_total
            .saturating_sub(in_use)
            .saturating_sub(compressed)
            .saturating_sub(modified)
            .saturating_sub(free);
        out.composition = Composition {
            in_use,
            compressed,
            modified,
            standby,
            free,
        };
    }

    /// Re-runs zram device enumeration (every 5 s) and rereads the zswap parameters. A device
    /// that is still present keeps its rate baselines. When enumeration fails, the device list
    /// is empty.
    pub fn rescan_zram(&mut self) {
        self.zswap = match read_zswap(&self.root) {
            Ok(params) => Measured::valid(params),
            Err(err) => Measured::invalid(err.validity()),
        };
        let mut found = Vec::new();
        if zram::enumerate(&self.root, &mut found).is_err() {
            found.clear();
        }
        let mut old = std::mem::take(&mut self.zram);
        self.zram = found
            .into_iter()
            .map(|dev| {
                let (read, write) = old
                    .iter()
                    .position(|s| s.dev.name == dev.name)
                    .map(|i| {
                        let s = old.swap_remove(i);
                        (s.read, s.write)
                    })
                    .unwrap_or_default();
                ZramState { dev, read, write }
            })
            .collect();
    }

    /// The next zram rates are `WarmingUp` with `GAP_BEFORE`.
    pub fn reset_baselines(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// One-shot read of `/proc/swaps` into the reused vector, and the zswap parameters.
    fn sample_details(&mut self, out: &mut MemoryReading) {
        let swaps = read_file(&self.swaps_path, &mut self.buf, SWAPS_LIMIT)
            .and_then(|_| parse_swaps(&self.buf, &mut out.swaps.value));
        out.swaps.validity = match swaps {
            Ok(()) => Validity::Valid,
            Err(err) => {
                out.swaps.value.clear();
                err.validity()
            }
        };
        out.zswap.clone_from(&self.zswap);
    }

    fn sample_zram(&mut self, clock: Clock, out: &mut MemoryReading) {
        out.zram.truncate(self.zram.len());
        out.zram.resize_with(self.zram.len(), ZramReading::default);
        out.zram_mem_used_total = 0;
        for (state, reading) in self.zram.iter_mut().zip(out.zram.iter_mut()) {
            if !Arc::ptr_eq(&reading.name, &state.dev.name) {
                reading.name = Arc::clone(&state.dev.name);
                reading.algorithm = state.dev.algorithm.clone();
            }
            reading.disksize_bytes = state.dev.disksize_bytes;

            reading.mm = match state.dev.read_mm_stat(&mut self.buf, &mut reading.mm.value) {
                Ok(()) => Measured::valid(reading.mm.value),
                Err(err) => Measured::invalid(err.validity()),
            };
            if reading.mm.validity == Validity::Valid {
                out.zram_mem_used_total = out
                    .zram_mem_used_total
                    .saturating_add(reading.mm.value.mem_used_total);
            }

            match state.dev.read_sectors(&mut self.buf) {
                Ok((read, written)) => {
                    let t = clock.boot_ns;
                    reading.read_bps =
                        state
                            .read
                            .update(read.saturating_mul(SECTOR_BYTES), t, self.generation);
                    reading.write_bps = state.write.update(
                        written.saturating_mul(SECTOR_BYTES),
                        t,
                        self.generation,
                    );
                }
                Err(err) => {
                    state.read.reset();
                    state.write.reset();
                    reading.read_bps = failed_rate(&err);
                    reading.write_bps = failed_rate(&err);
                }
            }
        }
    }
}

fn warming_up() -> Rate {
    Rate {
        per_sec: 0.0,
        validity: Validity::WarmingUp,
        flags: Default::default(),
    }
}

fn failed_rate(err: &CollectError) -> Rate {
    Rate {
        per_sec: 0.0,
        validity: err.validity(),
        flags: Default::default(),
    }
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tl_model::series::SampleFlags;

    const SEC: u64 = 1_000_000_000;
    const GIB: u64 = 1 << 30;
    const OFF: MemOpts = MemOpts { details: false };
    const DETAILS: MemOpts = MemOpts { details: true };

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

    fn sample(name: &str) -> MemoryReading {
        let mut sampler = MemorySampler::new(SysRoot::at(&fixture_dir(name)));
        let mut out = MemoryReading::default();
        sampler.sample_at(at(SEC), OFF, &mut out);
        out
    }

    /// A writable copy of `mem-composition` in a temp dir.
    fn temp_copy() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = fixture_dir("mem-composition");
        let zram0 = dir.path().join("sys/block/zram0");
        fs::create_dir_all(dir.path().join("proc")).expect("mkdir proc");
        fs::create_dir_all(&zram0).expect("mkdir zram0");
        fs::copy(src.join("proc/meminfo"), dir.path().join("proc/meminfo")).expect("copy");
        for f in ["disksize", "mm_stat", "comp_algorithm", "stat"] {
            fs::copy(src.join("sys/block/zram0").join(f), zram0.join(f)).expect("copy");
        }
        dir
    }

    fn write_stat(dir: &Path, read_sectors: u64, write_sectors: u64) {
        let line = format!("100 0 {read_sectors} 5 200 0 {write_sectors} 7 0 10 12 0 0 0 0 0 0\n");
        fs::write(dir.join("sys/block/zram0/stat"), line).expect("write stat");
    }

    #[test]
    fn used_commit_swap() {
        let r = sample("mem-composition");
        assert_eq!(r.validity, Validity::Valid);
        assert_eq!(r.used_bytes, 8 * GIB);
        assert_eq!(r.commit_pct, Measured::valid(125.0));
        assert_eq!(r.swap_used_bytes, GIB);
        assert_eq!(r.t_ns, SEC);
    }

    #[test]
    fn composition_segments() {
        let c = sample("mem-composition").composition;
        let in_use = 8_589_934_592 - 300_000;
        assert_eq!(c.compressed, 300_000);
        assert_eq!(c.in_use, in_use);
        assert_eq!(c.modified, 104_857_600);
        assert_eq!(c.free, 2_147_483_648);
        assert_eq!(
            c.standby,
            17_179_869_184 - in_use - 300_000 - 104_857_600 - 2_147_483_648
        );
    }

    #[test]
    fn savings() {
        let r = sample("mem-composition");
        assert_eq!(r.zram_mem_used_total, 300_000);
        assert_eq!(r.savings_bytes, 1_048_576 - 262_144);
        assert_eq!(r.savings_bytes, 786_432);
    }

    #[test]
    fn available_gt_total_uses_free() {
        let r = sample("mem-available-gt-total");
        assert!(r.info.mem_available > r.info.mem_total);
        assert_eq!(r.used_bytes, r.info.mem_total - r.info.mem_free);
    }

    #[test]
    fn zram_rates() {
        let dir = temp_copy();
        write_stat(dir.path(), 800, 1600);
        let mut sampler = MemorySampler::new(SysRoot::at(dir.path()));
        let mut out = MemoryReading::default();
        sampler.sample_at(at(SEC), OFF, &mut out);
        assert_eq!(out.zram.len(), 1);
        assert_eq!(out.zram[0].read_bps.validity, Validity::WarmingUp);
        assert_eq!(out.zram[0].write_bps.validity, Validity::WarmingUp);

        write_stat(dir.path(), 800 + 2048, 1600 + 4096);
        sampler.sample_at(at(2 * SEC), OFF, &mut out);
        let z = &out.zram[0];
        assert_eq!(&*z.name, "zram0");
        assert_eq!(z.algorithm.as_deref(), Some("zstd"));
        assert_eq!(z.read_bps.validity, Validity::Valid);
        assert_eq!(z.read_bps.per_sec, 1_048_576.0);
        assert_eq!(z.write_bps.per_sec, 2_097_152.0);

        // A rescan keeps the baselines of a device that is still there.
        sampler.rescan_zram();
        write_stat(dir.path(), 800 + 4096, 1600 + 8192);
        sampler.sample_at(at(3 * SEC), OFF, &mut out);
        assert_eq!(out.zram[0].read_bps.per_sec, 1_048_576.0);
    }

    #[test]
    fn zram_rates_reset_on_resume() {
        let dir = temp_copy();
        let mut sampler = MemorySampler::new(SysRoot::at(dir.path()));
        let mut out = MemoryReading::default();
        sampler.sample_at(at(SEC), OFF, &mut out);
        write_stat(dir.path(), 900, 1700);
        // Boot time moved 10 s while monotonic time moved 1 s: the machine slept.
        sampler.sample_at(
            Clock {
                boot_ns: 11 * SEC,
                mono_ns: 2 * SEC,
            },
            OFF,
            &mut out,
        );
        for rate in [out.zram[0].read_bps, out.zram[0].write_bps] {
            assert_eq!(rate.validity, Validity::WarmingUp);
            assert_eq!(rate.flags, SampleFlags::GAP_BEFORE);
        }

        // reset_baselines does the same.
        write_stat(dir.path(), 1000, 1800);
        sampler.sample_at(
            Clock {
                boot_ns: 12 * SEC,
                mono_ns: 3 * SEC,
            },
            OFF,
            &mut out,
        );
        assert_eq!(out.zram[0].read_bps.validity, Validity::Valid);
        sampler.reset_baselines();
        sampler.sample_at(
            Clock {
                boot_ns: 13 * SEC,
                mono_ns: 4 * SEC,
            },
            OFF,
            &mut out,
        );
        assert_eq!(out.zram[0].read_bps.validity, Validity::WarmingUp);
        assert_eq!(out.zram[0].read_bps.flags, SampleFlags::GAP_BEFORE);
    }

    #[test]
    fn commit_limit_zero() {
        let dir = temp_copy();
        let path = dir.path().join("proc/meminfo");
        let text = fs::read_to_string(&path).expect("read meminfo");
        let text = text.replace("CommitLimit:    16777216 kB", "CommitLimit:           0 kB");
        fs::write(&path, text).expect("write meminfo");
        let mut sampler = MemorySampler::new(SysRoot::at(dir.path()));
        let mut out = MemoryReading::default();
        sampler.sample_at(at(SEC), OFF, &mut out);
        assert_eq!(out.validity, Validity::Valid);
        assert_eq!(out.commit_pct.validity, Validity::Unsupported);
    }

    #[test]
    fn meminfo_errors_leave_fields_zero() {
        let dir = temp_copy();
        let path = dir.path().join("proc/meminfo");
        fs::write(&path, "MemTotal: 1 kB\n").expect("write meminfo");
        let mut sampler = MemorySampler::new(SysRoot::at(dir.path()));
        let mut out = MemoryReading::default();
        sampler.sample_at(at(SEC), OFF, &mut out);
        assert_eq!(out.validity, Validity::Failed);
        assert_eq!(out.used_bytes, 0);
        assert_eq!(out.composition, Composition::default());
        assert_eq!(out.commit_pct.validity, Validity::Failed);

        // A new sampler, since the kept fd still reads the removed file.
        fs::remove_file(&path).expect("remove meminfo");
        let mut sampler = MemorySampler::new(SysRoot::at(dir.path()));
        sampler.sample_at(at(2 * SEC), OFF, &mut out);
        assert_eq!(out.validity, Validity::Missing);

        // A zram device error marks only that device.
        fs::remove_file(dir.path().join("sys/block/zram0/mm_stat")).expect("remove mm_stat");
        let mut sampler = MemorySampler::new(SysRoot::at(dir.path()));
        sampler.sample_at(at(3 * SEC), OFF, &mut out);
        assert_eq!(out.zram[0].mm.validity, Validity::Missing);
        assert_eq!(out.zram_mem_used_total, 0);
    }

    #[test]
    fn no_allocation_after_first_sample() {
        let mut sampler = MemorySampler::new(SysRoot::at(&fixture_dir("mem-composition")));
        let mut out = MemoryReading::default();
        sampler.sample_at(at(SEC), OFF, &mut out);
        let (buf_cap, zram_cap) = (sampler.buf.capacity(), out.zram.capacity());
        sampler.sample_at(at(2 * SEC), OFF, &mut out);
        assert_eq!(sampler.buf.capacity(), buf_cap);
        assert_eq!(out.zram.capacity(), zram_cap);
    }

    #[test]
    fn owner_desktop_snapshot() {
        let r = sample("owner-desktop");
        assert_eq!(r.validity, Validity::Valid);
        insta::assert_debug_snapshot!(r);
    }

    #[test]
    fn details_owner_desktop_snapshot() {
        let mut sampler = MemorySampler::new(SysRoot::at(&fixture_dir("owner-desktop")));
        let mut out = MemoryReading::default();
        sampler.sample_at(at(SEC), DETAILS, &mut out);
        assert_eq!(out.swaps.validity, Validity::Valid);
        assert_eq!(out.zswap.validity, Validity::Valid);
        insta::assert_debug_snapshot!((&out.swaps, &out.zswap));
    }

    #[test]
    fn details_off_keeps_defaults() {
        let mut sampler = MemorySampler::new(SysRoot::at(&fixture_dir("owner-desktop")));
        let mut out = MemoryReading::default();
        sampler.sample_at(at(SEC), OFF, &mut out);
        assert_eq!(out.swaps.validity, Validity::WarmingUp);
        assert!(out.swaps.value.is_empty());
        assert_eq!(out.zswap, Measured::invalid(Validity::WarmingUp));

        // After a detailed sample, a plain sample keeps the detailed values.
        sampler.sample_at(at(2 * SEC), DETAILS, &mut out);
        let (swaps, zswap) = (out.swaps.clone(), out.zswap.clone());
        sampler.sample_at(at(3 * SEC), OFF, &mut out);
        assert_eq!(out.swaps, swaps);
        assert_eq!(out.zswap, zswap);

        // A fixture without /proc/swaps or zswap.
        let mut sampler = MemorySampler::new(SysRoot::at(&fixture_dir("mem-composition")));
        let mut out = MemoryReading::default();
        sampler.sample_at(at(SEC), DETAILS, &mut out);
        assert_eq!(out.swaps.validity, Validity::Missing);
        assert_eq!(out.zswap, Measured::valid(None));
    }
}
