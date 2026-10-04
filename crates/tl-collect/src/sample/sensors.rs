//! Sensors sampler: hwmon temperatures and fans (ADR-016, PERF-068, PERF-076, PERF-077).
//!
//! Every value file is kept open from `rescan` and re-read with `pread` each tick. GPU chips are
//! skipped: their fans and temperatures belong to the GPU page (PERF-068). The sampler reports
//! every fan, including fans that read 0 RPM; hiding "not connected" fans is up to the engine
//! and UI (PERF-076).

use std::sync::Arc;

use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::counters::{Clock, Measured};
use crate::parse::hwmon::{
    FanControl, GPU_HWMON_NAMES, HwmonChip, enumerate, fan_input_path, fan_max_path,
    parse_fan_control, parse_millidegrees, parse_pwm_pct, parse_rpm, pwm_enable_path, pwm_path,
    temp_input_path,
};
use crate::sysroot::{KeptFile, read_u64};
use crate::{CollectError, SysRoot};

/// Needed so `Measured<FanControl>` has a placeholder value. Readers check `validity` first.
impl Default for FanControl {
    fn default() -> Self {
        Self::Automatic
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TempReading {
    pub chip: Arc<str>,
    pub chip_device_path: Option<Arc<str>>,
    pub n: u16,
    /// `temp<n>_label`, else `temp<n>`.
    pub label: Arc<str>,
    pub celsius: Measured<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FanReading {
    pub chip: Arc<str>,
    pub chip_device_path: Option<Arc<str>>,
    pub n: u16,
    pub label: Option<Arc<str>>,
    pub rpm: Measured<u32>,
    /// `fan<n>_max`, read once at `rescan`.
    pub max_rpm: Option<u32>,
    /// `Unsupported` when the chip has no `pwm<n>`.
    pub pwm_pct: Measured<f32>,
    /// `Unsupported` when there is no `pwm<n>_enable`.
    pub control: Measured<FanControl>,
}

#[derive(Clone, Debug, Default)]
pub struct SensorsReading {
    pub t_ns: TimeNs,
    pub temps: Vec<TempReading>,
    pub fans: Vec<FanReading>,
}

/// The kept-open hwmon value files of every non-GPU chip.
pub struct SensorsSampler {
    root: SysRoot,
    chips: Vec<HwmonChip>,
    temps: Vec<TempFile>,
    fans: Vec<FanFiles>,
    scratch: Vec<HwmonChip>,
    buf: Vec<u8>,
}

struct TempFile {
    chip: Arc<str>,
    chip_device_path: Option<Arc<str>>,
    n: u16,
    label: Arc<str>,
    input: KeptFile,
}

struct FanFiles {
    chip: Arc<str>,
    chip_device_path: Option<Arc<str>>,
    n: u16,
    label: Option<Arc<str>>,
    max_rpm: Option<u32>,
    input: KeptFile,
    pwm: Option<KeptFile>,
    enable: Option<KeptFile>,
}

impl SensorsSampler {
    /// Runs `rescan`. Each value file opens on its first read.
    pub fn new(root: SysRoot) -> Self {
        let mut sampler = Self {
            root,
            chips: Vec::new(),
            temps: Vec::new(),
            fans: Vec::new(),
            scratch: Vec::new(),
            buf: Vec::new(),
        };
        sampler.rescan();
        sampler
    }

    /// Enumerates the chips again (device enumeration, every 5 s). When nothing changed, the
    /// open files are kept.
    pub fn rescan(&mut self) {
        if enumerate(&self.root, &mut self.scratch).is_err() {
            self.scratch.clear();
        }
        if self.scratch == self.chips {
            return;
        }
        std::mem::swap(&mut self.chips, &mut self.scratch);
        self.build_files();
    }

    /// Every chip from the last enumeration, GPU chips included.
    pub fn chips(&self) -> &[HwmonChip] {
        &self.chips
    }

    pub fn sample(&mut self, out: &mut SensorsReading) {
        self.sample_at(Clock::now(), out);
    }

    pub fn sample_at(&mut self, clock: Clock, out: &mut SensorsReading) {
        out.t_ns = clock.boot_ns;
        out.temps.clear();
        out.fans.clear();
        let buf = &mut self.buf;
        for temp in &mut self.temps {
            out.temps.push(TempReading {
                chip: temp.chip.clone(),
                chip_device_path: temp.chip_device_path.clone(),
                n: temp.n,
                label: temp.label.clone(),
                celsius: read_value(&mut temp.input, buf, parse_millidegrees),
            });
        }
        for fan in &mut self.fans {
            out.fans.push(FanReading {
                chip: fan.chip.clone(),
                chip_device_path: fan.chip_device_path.clone(),
                n: fan.n,
                label: fan.label.clone(),
                rpm: read_value(&mut fan.input, buf, parse_rpm),
                max_rpm: fan.max_rpm,
                pwm_pct: read_optional(fan.pwm.as_mut(), buf, parse_pwm_pct),
                control: read_optional(fan.enable.as_mut(), buf, parse_fan_control),
            });
        }
    }

    /// One `KeptFile` per value file of every non-GPU chip, in chip then n order.
    fn build_files(&mut self) {
        self.temps.clear();
        self.fans.clear();
        for chip in &self.chips {
            if GPU_HWMON_NAMES.contains(&&*chip.name) {
                continue;
            }
            for temp in &chip.temps {
                let label = match &temp.label {
                    Some(label) => label.clone(),
                    None => format!("temp{}", temp.n).into(),
                };
                self.temps.push(TempFile {
                    chip: chip.name.clone(),
                    chip_device_path: chip.device_path.clone(),
                    n: temp.n,
                    label,
                    input: KeptFile::new(temp_input_path(chip, temp.n)),
                });
            }
            for fan in &chip.fans {
                let max_rpm = if fan.has_max {
                    read_u64(&fan_max_path(chip, fan.n), &mut self.buf)
                        .ok()
                        .and_then(|max| u32::try_from(max).ok())
                } else {
                    None
                };
                let pwm = chip.pwms.iter().find(|p| p.n == fan.n);
                self.fans.push(FanFiles {
                    chip: chip.name.clone(),
                    chip_device_path: chip.device_path.clone(),
                    n: fan.n,
                    label: fan.label.clone(),
                    max_rpm,
                    input: KeptFile::new(fan_input_path(chip, fan.n)),
                    pwm: pwm.map(|p| KeptFile::new(pwm_path(chip, p.n))),
                    enable: pwm
                        .filter(|p| p.has_enable)
                        .map(|p| KeptFile::new(pwm_enable_path(chip, p.n))),
                });
            }
        }
    }
}

/// Reads and parses one value. A failed read or parse gives the error's validity.
fn read_value<T: Default>(
    file: &mut KeptFile,
    buf: &mut Vec<u8>,
    parse: fn(&[u8]) -> Result<T, CollectError>,
) -> Measured<T> {
    match file.read(buf).and_then(|_| parse(buf)) {
        Ok(value) => Measured::valid(value),
        Err(err) => Measured::invalid(err.validity()),
    }
}

/// `Unsupported` when the file does not exist on this chip.
fn read_optional<T: Default>(
    file: Option<&mut KeptFile>,
    buf: &mut Vec<u8>,
    parse: fn(&[u8]) -> Result<T, CollectError>,
) -> Measured<T> {
    match file {
        Some(file) => read_value(file, buf, parse),
        None => Measured::invalid(Validity::Unsupported),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    const CLOCK: Clock = Clock {
        boot_ns: 5_000_000_000,
        mono_ns: 5_000_000_000,
    };

    fn fixture_dir(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn sample(root: SysRoot) -> SensorsReading {
        let mut sampler = SensorsSampler::new(root);
        let mut out = SensorsReading::default();
        sampler.sample_at(CLOCK, &mut out);
        out
    }

    /// Copies a fixture into a tempdir. Symlinks are copied as symlinks.
    fn fixture_copy(name: &str) -> tempfile::TempDir {
        fn copy(from: &Path, to: &Path) {
            fs::create_dir_all(to).unwrap();
            for entry in fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let (src, dst) = (entry.path(), to.join(entry.file_name()));
                let kind = entry.file_type().unwrap();
                if kind.is_symlink() {
                    std::os::unix::fs::symlink(fs::read_link(&src).unwrap(), &dst).unwrap();
                } else if kind.is_dir() {
                    copy(&src, &dst);
                } else {
                    fs::copy(&src, &dst).unwrap();
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        copy(&fixture_dir(name), dir.path());
        dir
    }

    fn summary_temps(out: &SensorsReading) -> Vec<(&str, u16, &str, Measured<f32>)> {
        out.temps
            .iter()
            .map(|t| (&*t.chip, t.n, &*t.label, t.celsius))
            .collect()
    }

    #[test]
    fn fans_fixture_fans() {
        let out = sample(SysRoot::at(&fixture_dir("fans")));
        assert_eq!(out.t_ns, CLOCK.boot_ns);
        assert_eq!(out.fans.len(), 3);

        let fan1 = &out.fans[0];
        assert_eq!((&*fan1.chip, fan1.n), ("nct6797", 1));
        assert_eq!(fan1.rpm, Measured::valid(1200));
        assert_eq!(fan1.pwm_pct, Measured::valid(50.19608));
        assert_eq!(fan1.control, Measured::valid(FanControl::Automatic));
        assert_eq!(fan1.max_rpm, None);
        assert_eq!(fan1.label, None);

        let fan2 = &out.fans[1];
        assert_eq!((&*fan2.chip, fan2.n), ("nct6797", 2));
        assert_eq!(fan2.rpm, Measured::valid(0));
        assert_eq!(fan2.pwm_pct.validity, Validity::Unsupported);
        assert_eq!(fan2.control.validity, Validity::Unsupported);

        let single = &out.fans[2];
        assert_eq!((&*single.chip, single.n), ("single", 1));
        assert_eq!(single.rpm, Measured::valid(900));
    }

    #[test]
    fn fans_fixture_temps() {
        let out = sample(SysRoot::at(&fixture_dir("fans")));
        assert_eq!(
            summary_temps(&out),
            [
                ("nct6797", 1, "temp1", Measured::valid(40.0)),
                ("nct6797", 2, "temp2", Measured::valid(45.0)),
                ("single", 1, "temp1", Measured::valid(35.0)),
            ]
        );
    }

    #[test]
    fn gpu_chip_skipped() {
        let mut sampler = SensorsSampler::new(SysRoot::at(&fixture_dir("fans")));
        assert!(sampler.chips().iter().any(|c| &*c.name == "amdgpu"));
        let mut out = SensorsReading::default();
        sampler.sample_at(CLOCK, &mut out);
        assert!(out.fans.iter().all(|f| &*f.chip != "amdgpu"));
        assert!(out.temps.iter().all(|t| &*t.chip != "amdgpu"));
    }

    #[test]
    fn no_fans_fixture() {
        let out = sample(SysRoot::at(&fixture_dir("no-fans")));
        assert!(out.fans.is_empty());
        assert_eq!(
            summary_temps(&out),
            [("k10temp", 1, "Tctl", Measured::valid(48.25))]
        );
    }

    #[test]
    fn unreadable_temp_restricted() {
        if rustix::process::geteuid().is_root() {
            return;
        }
        let dir = fixture_copy("fans");
        let path = dir.path().join("sys/class/hwmon/hwmon0/temp1_input");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();

        let out = sample(SysRoot::at(dir.path()));
        assert_eq!(out.temps[0].n, 1);
        assert_eq!(out.temps[0].celsius.validity, Validity::Restricted);
        assert!(out.temps[1..].iter().all(|t| t.celsius.validity.is_valid()));
        assert!(out.fans.iter().all(|f| f.rpm.validity.is_valid()));
    }

    #[test]
    fn owner_desktop_snapshot() {
        let out = sample(SysRoot::at(&fixture_dir("owner-desktop")));
        let mut text = String::new();
        for t in &out.temps {
            writeln!(
                text,
                "temp {} {:?} temp{} {:?} {:?}",
                t.chip, t.chip_device_path, t.n, t.label, t.celsius
            )
            .unwrap();
        }
        for f in &out.fans {
            writeln!(
                text,
                "fan {} {:?} fan{} label={:?} rpm={:?} max={:?} pwm={:?} control={:?}",
                f.chip, f.chip_device_path, f.n, f.label, f.rpm, f.max_rpm, f.pwm_pct, f.control
            )
            .unwrap();
        }
        insta::assert_snapshot!(text);
    }

    #[test]
    fn rescan_picks_up_new_chip() {
        let dir = fixture_copy("fans");
        let mut sampler = SensorsSampler::new(SysRoot::at(dir.path()));
        let mut out = SensorsReading::default();
        sampler.sample_at(CLOCK, &mut out);
        assert!(out.temps.iter().all(|t| &*t.chip != "added"));

        let chip = dir.path().join("sys/class/hwmon/hwmon3");
        fs::create_dir(&chip).unwrap();
        fs::write(chip.join("name"), "added\n").unwrap();
        fs::write(chip.join("temp1_input"), "30500\n").unwrap();
        fs::write(chip.join("temp1_label"), "Board\n").unwrap();
        fs::write(chip.join("fan1_input"), "700\n").unwrap();
        fs::write(chip.join("fan1_max"), "2400\n").unwrap();

        sampler.rescan();
        sampler.sample_at(CLOCK, &mut out);
        let temp = out.temps.last().unwrap();
        assert_eq!(
            (&*temp.chip, &*temp.label, temp.celsius),
            ("added", "Board", Measured::valid(30.5))
        );
        let fan = out.fans.last().unwrap();
        assert_eq!((&*fan.chip, fan.rpm), ("added", Measured::valid(700)));
        assert_eq!(fan.max_rpm, Some(2400));
        assert_eq!(out.temps.len(), 4);
        assert_eq!(out.fans.len(), 4);
    }
}
