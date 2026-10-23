//! System pressure samplers (ADR-016 "System pressure", UI-PRF-054): the PSI avg10 values of
//! /proc/pressure/cpu and /proc/pressure/memory, and the swap-in rate from the /proc/vmstat
//! `pswpin` delta. The `Cpu` and `Memory` workers call them once per second while their
//! pressure series are wanted. The 1-minute load average comes from the CPU sampler's live
//! stats.
//!
//! The files are kept open and re-read with `pread`. A kernel without PSI has no
//! /proc/pressure, so a missing file gives `Unsupported` for the PSI values.

use tl_model::validity::Validity;

use crate::counters::{Clock, Counter, Measured, Rate, resumed_between};
use crate::parse::psi::{PsiAvg10, parse_psi};
use crate::parse::vmstat::parse_pswpin;
use crate::sysroot::KeptFile;
use crate::{CollectError, SysRoot};

/// Reads /proc/pressure/cpu.
pub struct CpuPressureSampler {
    psi: KeptFile,
    buf: Vec<u8>,
}

impl CpuPressureSampler {
    pub fn new(root: &SysRoot) -> Self {
        Self {
            psi: KeptFile::new(root.proc_path("pressure/cpu")),
            buf: Vec::new(),
        }
    }

    /// The `some` avg10 in percent.
    pub fn sample(&mut self) -> Measured<f32> {
        match read_psi(&mut self.psi, &mut self.buf) {
            Ok(psi) => Measured::valid(psi.some),
            Err(validity) => Measured::invalid(validity),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MemPressureReading {
    /// /proc/pressure/memory `some` avg10 in percent.
    pub psi_some_pct: Measured<f32>,
    /// /proc/pressure/memory `full` avg10 in percent.
    pub psi_full_pct: Measured<f32>,
    /// Pages per second from the `pswpin` delta. `Unsupported` without a `pswpin` line.
    pub swap_in_pps: Rate,
}

impl Default for MemPressureReading {
    fn default() -> Self {
        Self {
            psi_some_pct: Measured::default(),
            psi_full_pct: Measured::default(),
            swap_in_pps: rate_with(Validity::WarmingUp),
        }
    }
}

/// Reads /proc/pressure/memory and `pswpin` from /proc/vmstat.
pub struct MemPressureSampler {
    psi: KeptFile,
    vmstat: KeptFile,
    buf: Vec<u8>,
    swap_in: Counter,
    prev_clock: Option<Clock>,
    /// Counter generation. Bumped on resume and `reset_baselines` (ADR-008).
    generation: u32,
}

impl MemPressureSampler {
    pub fn new(root: &SysRoot) -> Self {
        Self {
            psi: KeptFile::new(root.proc_path("pressure/memory")),
            vmstat: KeptFile::new(root.proc_path("vmstat")),
            buf: Vec::new(),
            swap_in: Counter::default(),
            prev_clock: None,
            generation: 0,
        }
    }

    pub fn sample(&mut self) -> MemPressureReading {
        self.sample_at(Clock::now())
    }

    pub fn sample_at(&mut self, clock: Clock) -> MemPressureReading {
        if let Some(prev) = self.prev_clock
            && resumed_between(prev, clock)
        {
            self.generation = self.generation.wrapping_add(1);
        }
        self.prev_clock = Some(clock);

        let (psi_some_pct, psi_full_pct) = match read_psi(&mut self.psi, &mut self.buf) {
            Ok(PsiAvg10 { some, full }) => (
                Measured::valid(some),
                full.map_or(Measured::invalid(Validity::Unsupported), Measured::valid),
            ),
            Err(validity) => (Measured::invalid(validity), Measured::invalid(validity)),
        };

        let pswpin = self
            .vmstat
            .read(&mut self.buf)
            .and_then(|_| parse_pswpin(&self.buf));
        let swap_in_pps = match pswpin {
            Ok(Some(pages)) => self.swap_in.update(pages, clock.boot_ns, self.generation),
            Ok(None) => {
                self.swap_in.reset();
                rate_with(Validity::Unsupported)
            }
            Err(err) => {
                self.swap_in.reset();
                rate_with(err.validity())
            }
        };

        MemPressureReading {
            psi_some_pct,
            psi_full_pct,
            swap_in_pps,
        }
    }

    /// The next swap-in rate is `WarmingUp` with `GAP_BEFORE`.
    pub fn reset_baselines(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

/// A missing file means the kernel has no PSI: `Unsupported`.
fn read_psi(file: &mut KeptFile, buf: &mut Vec<u8>) -> Result<PsiAvg10, Validity> {
    file.read(buf)
        .and_then(|_| parse_psi(buf))
        .map_err(|err| match err {
            CollectError::NotFound => Validity::Unsupported,
            err => err.validity(),
        })
}

fn rate_with(validity: Validity) -> Rate {
    Rate {
        per_sec: 0.0,
        validity,
        flags: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tl_model::series::SampleFlags;

    const SEC: u64 = 1_000_000_000;

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

    /// A writable copy of the `pressure` fixture in a temp dir.
    fn temp_copy() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = fixture_dir("pressure/proc");
        let proc = dir.path().join("proc");
        fs::create_dir_all(proc.join("pressure")).expect("mkdir pressure");
        for f in ["pressure/cpu", "pressure/memory", "vmstat"] {
            fs::copy(src.join(f), proc.join(f)).expect("copy");
        }
        dir
    }

    fn write_pswpin(dir: &Path, pages: u64) {
        let text = format!("nr_free_pages 100\npswpin {pages}\npswpout 7\n");
        fs::write(dir.join("proc/vmstat"), text).expect("write vmstat");
    }

    #[test]
    fn cpu_fixture() {
        let mut s = CpuPressureSampler::new(&SysRoot::at(&fixture_dir("pressure")));
        assert_eq!(s.sample(), Measured::valid(10.66));
    }

    #[test]
    fn memory_fixture() {
        let mut s = MemPressureSampler::new(&SysRoot::at(&fixture_dir("pressure")));
        let r = s.sample_at(at(SEC));
        assert_eq!(r.psi_some_pct, Measured::valid(6.25));
        assert_eq!(r.psi_full_pct, Measured::valid(2.5));
        assert_eq!(r.swap_in_pps.validity, Validity::WarmingUp);
    }

    #[test]
    fn swap_in_rate() {
        let dir = temp_copy();
        write_pswpin(dir.path(), 1000);
        let mut s = MemPressureSampler::new(&SysRoot::at(dir.path()));
        assert_eq!(
            s.sample_at(at(SEC)).swap_in_pps.validity,
            Validity::WarmingUp
        );

        write_pswpin(dir.path(), 1000 + 1200);
        let rate = s.sample_at(at(3 * SEC)).swap_in_pps;
        assert_eq!(rate.validity, Validity::Valid);
        assert_eq!(rate.per_sec, 600.0);

        // A decrease is a counter reset (ADR-008).
        write_pswpin(dir.path(), 10);
        let rate = s.sample_at(at(4 * SEC)).swap_in_pps;
        assert_eq!(rate.validity, Validity::WarmingUp);
        assert_eq!(rate.flags, SampleFlags::RESET);
        write_pswpin(dir.path(), 60);
        assert_eq!(s.sample_at(at(5 * SEC)).swap_in_pps.per_sec, 50.0);
    }

    #[test]
    fn swap_in_gap_on_resume_and_reset() {
        let dir = temp_copy();
        write_pswpin(dir.path(), 1000);
        let mut s = MemPressureSampler::new(&SysRoot::at(dir.path()));
        s.sample_at(at(SEC));
        write_pswpin(dir.path(), 2000);
        // Boot time moved 10 s while monotonic time moved 1 s: the machine slept.
        let rate = s
            .sample_at(Clock {
                boot_ns: 11 * SEC,
                mono_ns: 2 * SEC,
            })
            .swap_in_pps;
        assert_eq!(rate.validity, Validity::WarmingUp);
        assert_eq!(rate.flags, SampleFlags::GAP_BEFORE);

        s.reset_baselines();
        write_pswpin(dir.path(), 3000);
        let rate = s
            .sample_at(Clock {
                boot_ns: 12 * SEC,
                mono_ns: 3 * SEC,
            })
            .swap_in_pps;
        assert_eq!(rate.validity, Validity::WarmingUp);
        assert_eq!(rate.flags, SampleFlags::GAP_BEFORE);
    }

    #[test]
    fn no_psi_is_unsupported() {
        let dir = temp_copy();
        fs::remove_dir_all(dir.path().join("proc/pressure")).expect("remove pressure");
        let root = SysRoot::at(dir.path());
        let mut cpu = CpuPressureSampler::new(&root);
        assert_eq!(cpu.sample().validity, Validity::Unsupported);
        let mut mem = MemPressureSampler::new(&root);
        let r = mem.sample_at(at(SEC));
        assert_eq!(r.psi_some_pct.validity, Validity::Unsupported);
        assert_eq!(r.psi_full_pct.validity, Validity::Unsupported);
        // The swap-in rate does not depend on PSI.
        assert_eq!(r.swap_in_pps.validity, Validity::WarmingUp);
    }

    #[test]
    fn bad_files() {
        let dir = temp_copy();
        let proc = dir.path().join("proc");
        fs::write(proc.join("pressure/memory"), "garbage\n").expect("write psi");
        fs::write(proc.join("vmstat"), "pgfault 1\n").expect("write vmstat");
        let mut s = MemPressureSampler::new(&SysRoot::at(dir.path()));
        let r = s.sample_at(at(SEC));
        assert_eq!(r.psi_some_pct.validity, Validity::Failed);
        assert_eq!(r.psi_full_pct.validity, Validity::Failed);
        assert_eq!(r.swap_in_pps.validity, Validity::Unsupported);

        // A memory file without a `full` line keeps `some`.
        fs::write(
            proc.join("pressure/memory"),
            "some avg10=3.00 avg60=0.00 avg300=0.00 total=1\n",
        )
        .expect("write psi");
        let r = s.sample_at(at(2 * SEC));
        assert_eq!(r.psi_some_pct, Measured::valid(3.0));
        assert_eq!(r.psi_full_pct.validity, Validity::Unsupported);

        // A missing vmstat is Missing, not Unsupported.
        fs::remove_file(proc.join("vmstat")).expect("remove vmstat");
        let mut s = MemPressureSampler::new(&SysRoot::at(dir.path()));
        assert_eq!(s.sample_at(at(SEC)).swap_in_pps.validity, Validity::Missing);
    }

    #[test]
    fn owner_desktop_swap_in() {
        // The captured fixture has /proc/vmstat but no /proc/pressure.
        let mut s = MemPressureSampler::new(&SysRoot::at(&fixture_dir("owner-desktop")));
        let r = s.sample_at(at(SEC));
        assert_eq!(r.psi_some_pct.validity, Validity::Unsupported);
        assert_eq!(r.swap_in_pps.validity, Validity::WarmingUp);
        let r = s.sample_at(at(2 * SEC));
        assert_eq!(r.swap_in_pps.validity, Validity::Valid);
        assert_eq!(r.swap_in_pps.per_sec, 0.0);
    }

    #[test]
    fn no_allocation_after_first_sample() {
        let mut s = MemPressureSampler::new(&SysRoot::at(&fixture_dir("pressure")));
        s.sample_at(at(SEC));
        let cap = s.buf.capacity();
        s.sample_at(at(2 * SEC));
        assert_eq!(s.buf.capacity(), cap);
    }
}
