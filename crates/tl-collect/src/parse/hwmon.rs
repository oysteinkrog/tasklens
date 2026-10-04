//! Parser for /sys/class/hwmon/hwmon<N>/: chip enumeration and the value files (ADR-016,
//! PERF-039, PERF-076, PERF-077, PERF-108).
//!
//! `enumerate` runs at start and every 5 s during device enumeration (ADR-008), not per tick.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::sysroot::read_trimmed;
use crate::{CollectError, SysRoot};

const VALUE_ERROR: CollectError = CollectError::Parse {
    file: "hwmon",
    what: "value",
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempInput {
    pub n: u16,
    pub label: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FanInput {
    pub n: u16,
    pub label: Option<Arc<str>>,
    /// `fan<n>_max` exists.
    pub has_max: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PwmInput {
    pub n: u16,
    /// `pwm<n>_enable` exists.
    pub has_enable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwmonChip {
    /// `<root.sys>/class/hwmon/hwmon<index>`
    pub dir: PathBuf,
    pub index: u32,
    /// Contents of `name`, trimmed.
    pub name: Arc<str>,
    /// Canonical path of `hwmon<N>/device` relative to `root.sys`, for example
    /// `devices/platform/nct6775.656`. `None` when `device` is absent.
    pub device_path: Option<Arc<str>>,
    /// Sorted by n.
    pub temps: Vec<TempInput>,
    /// Sorted by n.
    pub fans: Vec<FanInput>,
    /// Sorted by n.
    pub pwms: Vec<PwmInput>,
}

/// Chips that carry the CPU temperature, in order of preference (PERF-039).
pub const CPU_TEMP_CHIPS: [&str; 4] = ["k10temp", "zenpower", "coretemp", "cpu_thermal"];

/// GPU chips. Their fans and temperatures belong to the GPU page (PERF-068).
pub const GPU_HWMON_NAMES: [&str; 4] = ["amdgpu", "nouveau", "i915", "xe"];

/// The mode in `pwm<n>_enable` (kernel hwmon ABI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanControl {
    FullSpeed,
    Manual,
    Automatic,
}

/// Clears `out` and fills it with every chip under `root.sys/class/hwmon`, sorted by N.
///
/// A chip without a readable `name` file, or whose directory cannot be listed, is skipped.
pub fn enumerate(root: &SysRoot, out: &mut Vec<HwmonChip>) -> Result<(), CollectError> {
    out.clear();
    let class = root.sys_path("class/hwmon");
    let mut found: Vec<(u32, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&class)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(index) = file_name
            .to_str()
            .and_then(|s| s.strip_prefix("hwmon"))
            .and_then(parse_digits::<u32>)
        else {
            continue;
        };
        found.push((index, entry.path()));
    }
    found.sort_unstable_by_key(|(index, _)| *index);

    // Canonical form of root.sys, so the device path prefix can be removed.
    let sys_canonical = fs::canonicalize(&root.sys).ok();
    let mut buf = Vec::new();
    for (index, dir) in found {
        let Ok(name) = read_trimmed(&dir.join("name"), &mut buf) else {
            continue;
        };
        let name: Arc<str> = String::from_utf8_lossy(name).into();
        let Some(mut chip) = scan_chip(dir, index, name, &mut buf) else {
            continue;
        };
        chip.device_path = device_path(&chip.dir, sys_canonical.as_deref());
        out.push(chip);
    }
    Ok(())
}

/// Lists the inputs of one chip. `None` when the directory cannot be listed.
fn scan_chip(dir: PathBuf, index: u32, name: Arc<str>, buf: &mut Vec<u8>) -> Option<HwmonChip> {
    let mut chip = HwmonChip {
        dir,
        index,
        name,
        device_path: None,
        temps: Vec::new(),
        fans: Vec::new(),
        pwms: Vec::new(),
    };
    for entry in fs::read_dir(&chip.dir).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if let Some(n) = input_number(file_name, "temp", "_input") {
            let label = read_label(&chip.dir.join(format!("temp{n}_label")), buf);
            chip.temps.push(TempInput { n, label });
        } else if let Some(n) = input_number(file_name, "fan", "_input") {
            let label = read_label(&chip.dir.join(format!("fan{n}_label")), buf);
            let has_max = chip.dir.join(format!("fan{n}_max")).exists();
            chip.fans.push(FanInput { n, label, has_max });
        } else if let Some(n) = file_name.strip_prefix("pwm").and_then(parse_digits::<u16>) {
            let has_enable = chip.dir.join(format!("pwm{n}_enable")).exists();
            chip.pwms.push(PwmInput { n, has_enable });
        }
    }
    chip.temps.sort_unstable_by_key(|t| t.n);
    chip.fans.sort_unstable_by_key(|f| f.n);
    chip.pwms.sort_unstable_by_key(|p| p.n);
    Some(chip)
}

/// `<prefix><digits><suffix>` gives the number.
fn input_number(file_name: &str, prefix: &str, suffix: &str) -> Option<u16> {
    file_name
        .strip_prefix(prefix)?
        .strip_suffix(suffix)
        .and_then(parse_digits)
}

/// A non-empty string of ASCII digits only.
fn parse_digits<T: std::str::FromStr>(s: &str) -> Option<T> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Trimmed, lossy UTF-8. Absent or unreadable gives `None`.
fn read_label(path: &Path, buf: &mut Vec<u8>) -> Option<Arc<str>> {
    let label = read_trimmed(path, buf).ok()?;
    Some(String::from_utf8_lossy(label).into())
}

fn device_path(dir: &Path, sys_canonical: Option<&Path>) -> Option<Arc<str>> {
    let device = fs::canonicalize(dir.join("device")).ok()?;
    let rel = device.strip_prefix(sys_canonical?).ok()?;
    Some(rel.to_string_lossy().into())
}

/// A fan input exists on any chip (the Additional Setup row, PERF-108).
pub fn any_fan(chips: &[HwmonChip]) -> bool {
    chips.iter().any(|chip| !chip.fans.is_empty())
}

/// (index into chips, temp n) for the CPU temperature, or None.
///
/// The first chip in `CPU_TEMP_CHIPS` order that has a temperature input. Within it: the temp
/// labelled `Tctl` or `Package id 0`, else `temp1`, else the lowest n.
pub fn cpu_temp_input(chips: &[HwmonChip]) -> Option<(usize, u16)> {
    CPU_TEMP_CHIPS.iter().find_map(|wanted| {
        let (i, chip) = chips
            .iter()
            .enumerate()
            .find(|(_, chip)| &*chip.name == *wanted && !chip.temps.is_empty())?;
        let by_label = chip.temps.iter().find(|t| {
            t.label
                .as_deref()
                .is_some_and(|l| l == "Tctl" || l == "Package id 0")
        });
        let temp = by_label
            .or_else(|| chip.temps.iter().find(|t| t.n == 1))
            .or_else(|| chip.temps.first())?;
        Some((i, temp.n))
    })
}

/// PERF-077 default pairing: Some(fan_n) only when the chip has exactly one fan and temp<fan_n> exists.
pub fn default_temp_pair(chip: &HwmonChip, fan_n: u16) -> Option<u16> {
    let [fan] = chip.fans.as_slice() else {
        return None;
    };
    (fan.n == fan_n && chip.temps.iter().any(|t| t.n == fan_n)).then_some(fan_n)
}

/// `dir/temp<n>_input`
pub fn temp_input_path(chip: &HwmonChip, n: u16) -> PathBuf {
    chip.dir.join(format!("temp{n}_input"))
}

/// `dir/fan<n>_input`
pub fn fan_input_path(chip: &HwmonChip, n: u16) -> PathBuf {
    chip.dir.join(format!("fan{n}_input"))
}

/// `dir/fan<n>_max`
pub fn fan_max_path(chip: &HwmonChip, n: u16) -> PathBuf {
    chip.dir.join(format!("fan{n}_max"))
}

/// `dir/pwm<n>`
pub fn pwm_path(chip: &HwmonChip, n: u16) -> PathBuf {
    chip.dir.join(format!("pwm{n}"))
}

/// `dir/pwm<n>_enable`
pub fn pwm_enable_path(chip: &HwmonChip, n: u16) -> PathBuf {
    chip.dir.join(format!("pwm{n}_enable"))
}

/// `"-5000\n"` gives -5.0 (°C).
pub fn parse_millidegrees(buf: &[u8]) -> Result<f32, CollectError> {
    let milli: i64 = parse_value(buf)?;
    Ok((milli as f64 / 1000.0) as f32)
}

pub fn parse_rpm(buf: &[u8]) -> Result<u32, CollectError> {
    parse_value(buf)
}

/// 0..=255 gives value * 100 / 255. Above 255 is an error.
pub fn parse_pwm_pct(buf: &[u8]) -> Result<f32, CollectError> {
    let value: u32 = parse_value(buf)?;
    if value > 255 {
        return Err(VALUE_ERROR);
    }
    Ok(value as f32 * 100.0 / 255.0)
}

/// `pwm<n>_enable`: 0 full speed (no control), 1 manual, 2 and above automatic.
pub fn parse_fan_control(buf: &[u8]) -> Result<FanControl, CollectError> {
    Ok(match parse_value::<u64>(buf)? {
        0 => FanControl::FullSpeed,
        1 => FanControl::Manual,
        _ => FanControl::Automatic,
    })
}

fn parse_value<T: std::str::FromStr>(buf: &[u8]) -> Result<T, CollectError> {
    std::str::from_utf8(buf.trim_ascii())
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(VALUE_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_value_parsers_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=64)) {
            let _ = parse_millidegrees(&bytes);
            let _ = parse_rpm(&bytes);
            let _ = parse_fan_control(&bytes);
            if let Ok(pct) = parse_pwm_pct(&bytes) {
                prop_assert!((0.0..=100.0).contains(&pct));
            }
        }

        #[test]
        fn prop_numbers_round_trip(milli in any::<i32>(), rpm in any::<u32>(), pwm in 0u32..=255) {
            let parsed = parse_millidegrees(format!("{milli}\n").as_bytes()).unwrap();
            prop_assert_eq!(parsed, (milli as f64 / 1000.0) as f32);
            prop_assert_eq!(parse_rpm(format!(" {rpm}\n").as_bytes()).unwrap(), rpm);
            prop_assert!(parse_pwm_pct(pwm.to_string().as_bytes()).is_ok());
        }
    }

    #[test]
    fn input_names() {
        assert_eq!(input_number("temp12_input", "temp", "_input"), Some(12));
        assert_eq!(input_number("temp_input", "temp", "_input"), None);
        assert_eq!(input_number("temp1_label", "temp", "_input"), None);
        assert_eq!(input_number("temp+1_input", "temp", "_input"), None);
        assert_eq!(parse_digits::<u16>("7"), Some(7));
        assert_eq!(parse_digits::<u16>("1_enable"), None);
        assert_eq!(parse_digits::<u16>(""), None);
    }

    #[test]
    fn bad_values() {
        for bad in [&b""[..], b" \n", b"abc", b"12x", b"1.5"] {
            assert!(matches!(
                parse_millidegrees(bad),
                Err(CollectError::Parse { file: "hwmon", what: "value" })
            ));
            assert!(parse_rpm(bad).is_err());
            assert!(parse_pwm_pct(bad).is_err());
            assert!(parse_fan_control(bad).is_err());
        }
        assert!(parse_rpm(b"-1").is_err());
        assert!(parse_fan_control(b"-1").is_err());
    }
}
