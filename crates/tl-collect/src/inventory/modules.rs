//! Memory module (DIMM) details from the udev properties that systemd's `dmi_memory_id` writes
//! for the `dmi:id` device (PERF-046, PERF-099). No root and no `dmidecode` are needed.
//!
//! Serial numbers are never read, stored, shown or copied (D-007, R-030): the parser matches
//! field names only and skips every other key before it touches the value.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::sanitize::sanitize_text;

/// The udev database entry for the `dmi:id` device.
pub const UDEV_DMI_DATA: &str = "/run/udev/data/+dmi:id";
/// Read limit for the udev data file, in bytes.
const UDEV_DMI_MAX: u64 = 64 * 1024;
/// Most bytes kept from one string value.
const VALUE_MAX: usize = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryModule {
    /// `n` in `MEMORY_DEVICE_<n>_*`, SMBIOS order.
    pub index: u32,
    /// `SIZE`, in bytes.
    pub size_bytes: u64,
    /// `LOCATOR`, for example "DIMM 0".
    pub locator: Option<Arc<str>>,
    /// `BANK_LOCATOR`, for example "P0 CHANNEL A".
    pub bank_locator: Option<Arc<str>>,
    /// `SPEED_MTS`.
    pub speed_mts: Option<u32>,
    /// `CONFIGURED_SPEED_MTS`.
    pub configured_speed_mts: Option<u32>,
    /// `TYPE` as reported (DDR4, DDR5, LPDDR5, ...).
    pub mem_type: Option<Arc<str>>,
    /// `FORM_FACTOR`.
    pub form_factor: Option<Arc<str>>,
    /// `MANUFACTURER`.
    pub manufacturer: Option<Arc<str>>,
    /// `PART_NUMBER`, trimmed.
    pub part_number: Option<Arc<str>>,
    /// `RANK`.
    pub rank: Option<u32>,
    /// `CONFIGURED_VOLTAGE`, in millivolts. `None` when the key is missing or has no exact
    /// value (see `voltage_mv`).
    pub configured_voltage_mv: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModulesInfo {
    /// `slots_total` is `MEMORY_ARRAY_NUM_DEVICES`. Empty slots are left out of `modules`.
    Modules {
        slots_total: Option<u32>,
        modules: Vec<MemoryModule>,
    },
    /// No `MEMORY_DEVICE_` keys at all. The UI shows "Module details: Not reported".
    NotReported,
}

/// One device's fields while the file is parsed.
#[derive(Default)]
struct Partial {
    size: Option<u64>,
    /// `SIZE` was present but did not parse; the module is dropped.
    bad_size: bool,
    absent: bool,
    locator: Option<Arc<str>>,
    bank_locator: Option<Arc<str>>,
    speed_mts: Option<u32>,
    configured_speed_mts: Option<u32>,
    mem_type: Option<Arc<str>>,
    form_factor: Option<Arc<str>>,
    manufacturer: Option<Arc<str>>,
    part_number: Option<Arc<str>>,
    rank: Option<u32>,
    configured_voltage_mv: Option<u32>,
}

/// Parses a udev database entry. Only `E:KEY=value` lines are read.
pub fn parse_udev_dmi(bytes: &[u8]) -> ModulesInfo {
    let mut slots_total = None;
    let mut devices: BTreeMap<u32, Partial> = BTreeMap::new();
    let mut seen_device = false;
    for line in bytes.split(|&b| b == b'\n') {
        let Some(entry) = line.strip_prefix(b"E:") else {
            continue;
        };
        let Some(eq) = entry.iter().position(|&b| b == b'=') else {
            continue;
        };
        let (key, value) = (&entry[..eq], &entry[eq + 1..]);
        if key == b"MEMORY_ARRAY_NUM_DEVICES" {
            slots_total = number(value);
            continue;
        }
        let Some(rest) = key.strip_prefix(b"MEMORY_DEVICE_") else {
            continue;
        };
        seen_device = true;
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        let (Some(index), Some(field)) = (
            number::<u32>(&rest[..digits]),
            rest[digits..].strip_prefix(b"_"),
        ) else {
            continue;
        };
        // Match on the field name first, so skipped keys (SERIAL_NUMBER and the rest) never get
        // a device entry and their value is never read.
        let apply: fn(&mut Partial, &[u8]) = match field {
            b"SIZE" => |d, v| match number(v) {
                Some(n) => d.size = Some(n),
                None => d.bad_size = true,
            },
            b"PRESENT" => |d, v| d.absent |= v.trim_ascii() == b"0",
            b"LOCATOR" => |d, v| d.locator = text(v),
            b"BANK_LOCATOR" => |d, v| d.bank_locator = text(v),
            b"SPEED_MTS" => |d, v| d.speed_mts = number(v),
            b"CONFIGURED_SPEED_MTS" => |d, v| d.configured_speed_mts = number(v),
            b"TYPE" => |d, v| d.mem_type = text(v),
            b"FORM_FACTOR" => |d, v| d.form_factor = text(v),
            b"MANUFACTURER" => |d, v| d.manufacturer = text(v),
            b"PART_NUMBER" => |d, v| d.part_number = text(v),
            b"RANK" => |d, v| d.rank = number(v),
            b"CONFIGURED_VOLTAGE" => |d, v| d.configured_voltage_mv = voltage_mv(v),
            _ => continue,
        };
        apply(devices.entry(index).or_default(), value);
    }
    if !seen_device {
        return ModulesInfo::NotReported;
    }
    let modules = devices
        .into_iter()
        .filter_map(|(index, d)| {
            let size_bytes = d.size.filter(|&n| n > 0 && !d.bad_size && !d.absent)?;
            Some(MemoryModule {
                index,
                size_bytes,
                locator: d.locator,
                bank_locator: d.bank_locator,
                speed_mts: d.speed_mts,
                configured_speed_mts: d.configured_speed_mts,
                mem_type: d.mem_type,
                form_factor: d.form_factor,
                manufacturer: d.manufacturer,
                part_number: d.part_number,
                rank: d.rank,
                configured_voltage_mv: d.configured_voltage_mv,
            })
        })
        .collect();
    ModulesInfo::Modules {
        slots_total,
        modules,
    }
}

/// Reads and parses `udev_dmi` (normally [`UDEV_DMI_DATA`]). A missing or unreadable file gives
/// `NotReported`.
pub fn read_modules(udev_dmi: &Path) -> ModulesInfo {
    let mut buf = Vec::new();
    match File::open(udev_dmi) {
        Ok(f) if (&f).take(UDEV_DMI_MAX).read_to_end(&mut buf).is_ok() => parse_udev_dmi(&buf),
        _ => ModulesInfo::NotReported,
    }
}

/// Trims and sanitizes a value; empty values become `None`.
fn text(raw: &[u8]) -> Option<Arc<str>> {
    let mut out = String::new();
    sanitize_text(raw.trim_ascii(), VALUE_MAX, &mut out);
    let trimmed = out.trim();
    (!trimmed.is_empty()).then(|| Arc::from(trimmed))
}

/// Parses a trimmed ASCII decimal number.
fn number<T: std::str::FromStr>(raw: &[u8]) -> Option<T> {
    std::str::from_utf8(raw.trim_ascii()).ok()?.parse().ok()
}

/// `CONFIGURED_VOLTAGE` as systemd's `dmi_memory_id` writes it: volts with `%g`, except that a
/// whole multiple of 100 mV is written with `%.1g`. So 1.35 V reads "1.35" and 0.9 V reads
/// "0.9", but 1.0 V to 1.4 V all read "1". A whole number has no exact value and gives `None`,
/// as do 0 and text that is not a number (the page shows "Not reported", PERF-013).
fn voltage_mv(raw: &[u8]) -> Option<u32> {
    let raw = raw.trim_ascii();
    let volts = number::<f64>(raw)?;
    let exact = raw.contains(&b'.') && volts > 0. && volts < 10.;
    exact.then(|| (volts * 1000.).round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/dmi-desktop/run/udev/data/+dmi:id")
    }

    const FOUR_SLOTS: &[u8] = b"I:1234567
E:MEMORY_ARRAY_LOCATION=System Board Or Motherboard
E:MEMORY_ARRAY_NUM_DEVICES=4
E:MEMORY_DEVICE_0_PRESENT=1
E:MEMORY_DEVICE_0_SIZE=17179869184
E:MEMORY_DEVICE_0_FORM_FACTOR=DIMM
E:MEMORY_DEVICE_0_LOCATOR=DIMM 0
E:MEMORY_DEVICE_0_BANK_LOCATOR=P0 CHANNEL A
E:MEMORY_DEVICE_0_TYPE=DDR4
E:MEMORY_DEVICE_0_SPEED_MTS=3200
E:MEMORY_DEVICE_0_CONFIGURED_SPEED_MTS=3200
E:MEMORY_DEVICE_0_MANUFACTURER=Kingston
E:MEMORY_DEVICE_0_PART_NUMBER=KHX3200C18D4/16G
E:MEMORY_DEVICE_0_RANK=2
E:MEMORY_DEVICE_0_CONFIGURED_VOLTAGE=1.35
E:MEMORY_DEVICE_1_PRESENT=0
E:MEMORY_DEVICE_1_LOCATOR=DIMM 1
E:MEMORY_DEVICE_2_PRESENT=1
E:MEMORY_DEVICE_2_SIZE=17179869184
E:MEMORY_DEVICE_2_LOCATOR=DIMM 0
E:MEMORY_DEVICE_2_BANK_LOCATOR=P0 CHANNEL B
E:MEMORY_DEVICE_2_TYPE=DDR4
E:MEMORY_DEVICE_2_MANUFACTURER=Kingston
E:MEMORY_DEVICE_2_PART_NUMBER=KHX3200C18D4/16G
E:MEMORY_DEVICE_2_CONFIGURED_VOLTAGE=1.35
E:MEMORY_DEVICE_3_PRESENT=0
E:MEMORY_DEVICE_3_LOCATOR=DIMM 1
G:systemd
";

    #[test]
    fn two_of_four_populated() {
        for info in [parse_udev_dmi(FOUR_SLOTS), read_modules(&fixture())] {
            let ModulesInfo::Modules {
                slots_total,
                modules,
            } = info
            else {
                panic!("expected modules");
            };
            assert_eq!(slots_total, Some(4));
            assert_eq!(modules.iter().map(|m| m.index).collect::<Vec<_>>(), [0, 2]);
            let m = &modules[0];
            assert_eq!(m.size_bytes, 17_179_869_184);
            assert_eq!(m.configured_voltage_mv, Some(1350));
            assert_eq!(m.speed_mts, Some(3200));
            assert_eq!(m.mem_type.as_deref(), Some("DDR4"));
            assert_eq!(m.manufacturer.as_deref(), Some("Kingston"));
            assert_eq!(m.part_number.as_deref(), Some("KHX3200C18D4/16G"));
            assert_eq!(m.locator.as_deref(), Some("DIMM 0"));
            assert_eq!(m.bank_locator.as_deref(), Some("P0 CHANNEL A"));
            assert_eq!(modules[1].configured_voltage_mv, Some(1350));
        }
    }

    #[test]
    fn serial_never_stored() {
        let mut data = FOUR_SLOTS.to_vec();
        data.extend_from_slice(b"E:MEMORY_DEVICE_0_SERIAL_NUMBER=SERIAL-MUST-NOT-APPEAR\n");
        let parsed = format!("{:?}", parse_udev_dmi(&data));
        assert!(!parsed.contains("SERIAL-MUST-NOT-APPEAR"));
        // The committed fixture carries a serial line too.
        let read = format!("{:?}", read_modules(&fixture()));
        assert!(!read.contains("SERIAL-MUST-NOT-APPEAR"));
        // A serial key alone does not create a module.
        let only = parse_udev_dmi(b"E:MEMORY_DEVICE_5_SERIAL_NUMBER=SERIAL-MUST-NOT-APPEAR\n");
        assert_eq!(
            only,
            ModulesInfo::Modules {
                slots_total: None,
                modules: Vec::new()
            }
        );
    }

    #[test]
    fn no_keys_not_reported() {
        let data = b"I:1234567\nE:MEMORY_ARRAY_NUM_DEVICES=4\nE:ID_VENDOR=x\nG:systemd\n";
        assert_eq!(parse_udev_dmi(data), ModulesInfo::NotReported);
        assert_eq!(parse_udev_dmi(b""), ModulesInfo::NotReported);
    }

    #[test]
    fn missing_file_not_reported() {
        assert_eq!(
            read_modules(Path::new("/nonexistent/+dmi:id")),
            ModulesInfo::NotReported
        );
    }

    #[test]
    fn bad_size_dropped() {
        let data = b"E:MEMORY_DEVICE_0_SIZE=abc\nE:MEMORY_DEVICE_0_LOCATOR=DIMM 0\n\
E:MEMORY_DEVICE_1_SIZE=8589934592\nE:MEMORY_DEVICE_1_SPEED_MTS=fast\n\
E:MEMORY_DEVICE_2_SIZE=0\n";
        let ModulesInfo::Modules { modules, .. } = parse_udev_dmi(data) else {
            panic!("expected modules");
        };
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].index, 1);
        assert_eq!(modules[0].speed_mts, None);
    }

    /// systemd writes volts; a whole number is the lossy `%.1g` form (1.2 V reads "1").
    #[test]
    fn configured_voltage_in_volts() {
        let cases: [(&[u8], Option<u32>); 9] = [
            (b"1.35", Some(1350)),
            (b"1.1\n", Some(1100)),
            (b"0.9", Some(900)),
            (b"1.25", Some(1250)),
            (b"1", None),
            (b"1350", None),
            (b"abc", None),
            (b"-1.2", None),
            (b"0", None),
        ];
        for (raw, want) in cases {
            assert_eq!(voltage_mv(raw), want, "{:?}", String::from_utf8_lossy(raw));
        }

        // Owner machine (2026-10-17): DDR4 reported as "1" has no voltage, never 1 mV (0.00 V).
        let data = b"E:MEMORY_DEVICE_0_SIZE=17179869184\nE:MEMORY_DEVICE_0_CONFIGURED_VOLTAGE=1\n";
        let ModulesInfo::Modules { modules, .. } = parse_udev_dmi(data) else {
            panic!("expected modules");
        };
        assert_eq!(modules[0].configured_voltage_mv, None);
    }
}
