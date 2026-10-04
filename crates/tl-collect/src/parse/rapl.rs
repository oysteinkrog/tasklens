//! CPU package and core power from /sys/class/powercap/intel-rapl:<N>/energy_uj (PERF-038,
//! PERF-106, ADR-016).
//!
//! `energy_uj` is kept open and re-read with `pread` every tick. The AMD driver uses the same
//! `intel-rapl` names, so there is no vendor branch. This module never changes permissions:
//! energy counters can leak information about other programs (CVE-2020-8694).

use tl_model::validity::Validity;

use crate::counters::{Clock, Counter, Measured, Rate};
use crate::sysroot::{KeptFile, read_u64};
use crate::{CollectError, SysRoot};

const PACKAGE_ZONE: &str = "class/powercap/intel-rapl:0";
const CORE_ZONE: &str = "class/powercap/intel-rapl:0:0";

const ENERGY_ERROR: CollectError = CollectError::Parse {
    file: "energy_uj",
    what: "not a number",
};

/// Whether the package zone counter can be read (Additional Setup window, PERF-106).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaplStatus {
    /// No `intel-rapl:0` directory.
    NoZone,
    /// The directory exists but `energy_uj` cannot be read.
    Unreadable,
    Readable,
}

/// Watts. `Unsupported` when the zone is absent, `Restricted` when `energy_uj` is root-only.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RaplReading {
    pub package_w: Measured<f32>,
    pub core_w: Measured<f32>,
}

/// The kept-open energy counters of the package zone and its core subzone.
pub struct Rapl {
    package: Option<Zone>,
    core: Option<Zone>,
    buf: Vec<u8>,
}

struct Zone {
    energy: KeptFile,
    /// 0 when `max_energy_range_uj` is unreadable or 0: a decrease is then a reset.
    max_range_uj: u64,
    counter: Counter,
}

/// Checks root.sys/class/powercap/intel-rapl:0/energy_uj:
/// directory absent -> NoZone; read fails -> Unreadable; read succeeds -> Readable.
pub fn rapl_status(root: &SysRoot) -> RaplStatus {
    let dir = root.sys_path(PACKAGE_ZONE);
    if !dir.is_dir() {
        return RaplStatus::NoZone;
    }
    let mut buf = Vec::new();
    match read_u64(&dir.join("energy_uj"), &mut buf) {
        Ok(_) => RaplStatus::Readable,
        Err(_) => RaplStatus::Unreadable,
    }
}

impl Rapl {
    /// package = intel-rapl:0, core = intel-rapl:0:0. Reads `max_energy_range_uj` once.
    pub fn open(root: &SysRoot) -> Self {
        let mut buf = Vec::new();
        let package = Zone::open(root, PACKAGE_ZONE, &mut buf);
        let core = Zone::open(root, CORE_ZONE, &mut buf);
        Self { package, core, buf }
    }

    pub fn read(&mut self, clock: Clock, generation: u32, out: &mut RaplReading) {
        out.package_w = read_zone(self.package.as_mut(), &mut self.buf, clock, generation);
        out.core_w = read_zone(self.core.as_mut(), &mut self.buf, clock, generation);
    }

    /// Resets both counters. The next read is `WarmingUp`.
    pub fn reset(&mut self) {
        for zone in [self.package.as_mut(), self.core.as_mut()]
            .into_iter()
            .flatten()
        {
            zone.counter.reset();
        }
    }
}

impl Zone {
    fn open(root: &SysRoot, rel: &str, buf: &mut Vec<u8>) -> Option<Self> {
        let dir = root.sys_path(rel);
        if !dir.is_dir() {
            return None;
        }
        let max_range_uj = read_u64(&dir.join("max_energy_range_uj"), buf).unwrap_or(0);
        Some(Self {
            energy: KeptFile::new(dir.join("energy_uj")),
            max_range_uj,
            counter: Counter::default(),
        })
    }

    fn read_energy(&mut self, buf: &mut Vec<u8>) -> Result<u64, CollectError> {
        self.energy.read(buf)?;
        std::str::from_utf8(buf.trim_ascii())
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or(ENERGY_ERROR)
    }
}

fn read_zone(
    zone: Option<&mut Zone>,
    buf: &mut Vec<u8>,
    clock: Clock,
    generation: u32,
) -> Measured<f32> {
    let Some(zone) = zone else {
        return Measured::invalid(Validity::Unsupported);
    };
    let energy = match zone.read_energy(buf) {
        Ok(energy) => energy,
        Err(err) => return Measured::invalid(err.validity()),
    };
    let t = clock.boot_ns;
    let rate = if zone.max_range_uj == 0 {
        zone.counter.update(energy, t, generation)
    } else {
        zone.counter
            .update_wrapping(energy, t, generation, zone.max_range_uj)
    };
    watts(rate)
}

/// µJ per second to W.
fn watts(rate: Rate) -> Measured<f32> {
    if rate.validity == Validity::Valid {
        Measured::valid((rate.per_sec / 1e6) as f32)
    } else {
        Measured::invalid(rate.validity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    const SEC: u64 = 1_000_000_000;

    fn at(secs: u64) -> Clock {
        Clock {
            boot_ns: secs * SEC,
            mono_ns: secs * SEC,
        }
    }

    fn zone(base: &Path, name: &str, energy: u64, max_range: Option<u64>) {
        let dir = base.join("sys/class/powercap").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("energy_uj"), format!("{energy}\n")).unwrap();
        if let Some(max) = max_range {
            fs::write(dir.join("max_energy_range_uj"), format!("{max}\n")).unwrap();
        }
    }

    /// Rewrites in place so the kept fd sees the new value.
    fn set_energy(base: &Path, name: &str, energy: u64) {
        let path = base.join("sys/class/powercap").join(name).join("energy_uj");
        fs::write(path, format!("{energy}\n")).unwrap();
    }

    #[test]
    fn watts_from_delta() {
        let dir = tempfile::tempdir().unwrap();
        zone(dir.path(), "intel-rapl:0", 1_000_000, Some(262_143_328_850));
        let mut rapl = Rapl::open(&SysRoot::at(dir.path()));
        let mut out = RaplReading::default();

        rapl.read(at(0), 0, &mut out);
        assert_eq!(out.package_w.validity, Validity::WarmingUp);

        set_energy(dir.path(), "intel-rapl:0", 3_000_000);
        rapl.read(at(1), 0, &mut out);
        assert_eq!(out.package_w, Measured::valid(2.0));
    }

    #[test]
    fn wrap_uses_max_range() {
        let dir = tempfile::tempdir().unwrap();
        zone(dir.path(), "intel-rapl:0", 900_000, Some(1_000_000));
        let mut rapl = Rapl::open(&SysRoot::at(dir.path()));
        let mut out = RaplReading::default();

        rapl.read(at(0), 0, &mut out);
        set_energy(dir.path(), "intel-rapl:0", 100_000);
        rapl.read(at(1), 0, &mut out);
        assert_eq!(out.package_w, Measured::valid(0.2));
    }

    #[test]
    fn decrease_without_max_range_is_reset() {
        let dir = tempfile::tempdir().unwrap();
        zone(dir.path(), "intel-rapl:0", 900_000, None);
        let mut rapl = Rapl::open(&SysRoot::at(dir.path()));
        let mut out = RaplReading::default();

        rapl.read(at(0), 0, &mut out);
        set_energy(dir.path(), "intel-rapl:0", 100_000);
        rapl.read(at(1), 0, &mut out);
        assert_eq!(out.package_w.validity, Validity::WarmingUp);
    }

    #[test]
    fn missing_core_zone_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        zone(dir.path(), "intel-rapl:0", 1_000_000, Some(262_143_328_850));
        let mut rapl = Rapl::open(&SysRoot::at(dir.path()));
        let mut out = RaplReading::default();

        rapl.read(at(0), 0, &mut out);
        assert_eq!(out.core_w.validity, Validity::Unsupported);
        rapl.read(at(1), 0, &mut out);
        assert_eq!(out.core_w.validity, Validity::Unsupported);
        assert_eq!(out.package_w, Measured::valid(0.0));
    }

    #[test]
    fn unreadable_is_restricted() {
        if rustix::process::geteuid().is_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        zone(dir.path(), "intel-rapl:0", 1_000_000, Some(262_143_328_850));
        let energy = dir.path().join("sys/class/powercap/intel-rapl:0/energy_uj");
        fs::set_permissions(&energy, fs::Permissions::from_mode(0o000)).unwrap();
        let root = SysRoot::at(dir.path());
        let mut rapl = Rapl::open(&root);
        let mut out = RaplReading::default();

        rapl.read(at(0), 0, &mut out);
        assert_eq!(out.package_w.validity, Validity::Restricted);
        assert_eq!(rapl_status(&root), RaplStatus::Unreadable);
    }

    #[test]
    fn no_zone_status() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sys/class/powercap")).unwrap();
        let root = SysRoot::at(dir.path());
        assert_eq!(rapl_status(&root), RaplStatus::NoZone);

        let mut rapl = Rapl::open(&root);
        let mut out = RaplReading::default();
        rapl.read(at(0), 0, &mut out);
        assert_eq!(out.package_w.validity, Validity::Unsupported);
        assert_eq!(out.core_w.validity, Validity::Unsupported);
    }

    #[test]
    fn readable_status() {
        let dir = tempfile::tempdir().unwrap();
        zone(dir.path(), "intel-rapl:0", 1_000_000, Some(262_143_328_850));
        assert_eq!(
            rapl_status(&SysRoot::at(dir.path())),
            RaplStatus::Readable
        );
    }

    #[test]
    fn reset_warms_up() {
        let dir = tempfile::tempdir().unwrap();
        zone(dir.path(), "intel-rapl:0", 1_000_000, Some(262_143_328_850));
        zone(dir.path(), "intel-rapl:0:0", 500_000, Some(262_143_328_850));
        let mut rapl = Rapl::open(&SysRoot::at(dir.path()));
        let mut out = RaplReading::default();

        rapl.read(at(0), 0, &mut out);
        set_energy(dir.path(), "intel-rapl:0", 2_000_000);
        set_energy(dir.path(), "intel-rapl:0:0", 1_000_000);
        rapl.read(at(1), 0, &mut out);
        assert_eq!(out.package_w, Measured::valid(1.0));
        assert_eq!(out.core_w, Measured::valid(0.5));

        rapl.reset();
        rapl.read(at(2), 0, &mut out);
        assert_eq!(out.package_w.validity, Validity::WarmingUp);
        assert_eq!(out.core_w.validity, Validity::WarmingUp);
    }
}
