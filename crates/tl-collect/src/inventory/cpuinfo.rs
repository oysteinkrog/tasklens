//! /proc/cpuinfo for the Information tab's Processor section and the CPU page's Virtualization
//! and Microcode rows (PERF-096, PERF-035, PERF-037, ADR-016 "Information").
//!
//! Cores, logical processors, base and max speed come from the topology and cpufreq readers.
//! This module only reads the identity fields, the first `cpu MHz` and the flags we show.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::sanitize::sanitize_text;

/// Most bytes kept from one string value.
const VALUE_MAX: usize = 256;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuInfo {
    /// `model name`.
    pub model_name: Option<Arc<str>>,
    /// `vendor_id`, for example "AuthenticAMD".
    pub vendor_id: Option<Arc<str>>,
    /// `cpu family`.
    pub family: Option<u32>,
    /// `model`.
    pub model: Option<u32>,
    /// `stepping`.
    pub stepping: Option<u32>,
    /// `microcode`, for example "0xa201016".
    pub microcode: Option<Arc<str>>,
    /// `cpu MHz` of the first block, the speed fallback (PERF-033).
    pub mhz: Option<f32>,
    /// No `cpu MHz` line anywhere, but a `bogomips` line is present.
    pub bogomips_only: bool,
    /// Number of `processor` lines.
    pub logical_count: u32,
    pub flags: CpuFlags,
}

bitflags::bitflags! {
    /// The `flags` entries the UI uses.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct CpuFlags: u16 {
        const SVM = 1;
        const VMX = 2;
        const HYPERVISOR = 4;
        const SME = 8;
        const USER_SHSTK = 16;
        const AVX512F = 32;
    }
}

/// Parses /proc/cpuinfo into `out`, which is reset first. Never panics.
///
/// Fields come from the first block (up to the first blank line), except `processor`, which is
/// counted over the whole file. A number that does not parse, or a string that is empty after
/// sanitizing, leaves its field `None`.
pub fn parse_cpuinfo(bytes: &[u8], out: &mut CpuInfo) {
    *out = CpuInfo::default();
    let mut first_block = true;
    let mut any_mhz = false;
    let mut any_bogomips = false;
    let mut scratch = String::new();
    for line in bytes.split(|&b| b == b'\n') {
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            if line.trim_ascii().is_empty() {
                first_block = false;
            }
            continue;
        };
        let key = line[..colon].trim_ascii();
        let value = line[colon + 1..].trim_ascii();
        match key {
            b"processor" => out.logical_count = out.logical_count.saturating_add(1),
            b"cpu MHz" => {
                any_mhz = true;
                if first_block && out.mhz.is_none() {
                    out.mhz = parse_num::<f32>(value).filter(|v| v.is_finite());
                }
            }
            _ if key.eq_ignore_ascii_case(b"bogomips") => any_bogomips = true,
            _ if !first_block => {}
            b"model name" => out.model_name = text(value, &mut scratch),
            b"vendor_id" => out.vendor_id = text(value, &mut scratch),
            b"cpu family" => out.family = parse_num(value),
            b"model" => out.model = parse_num(value),
            b"stepping" => out.stepping = parse_num(value),
            b"microcode" => out.microcode = text(value, &mut scratch),
            b"flags" => out.flags = parse_flags(value),
            _ => {}
        }
    }
    out.bogomips_only = !any_mhz && any_bogomips;
}

/// Writes "Family 25 Model 97 Stepping 2" into `out`, which is cleared first. Returns false and
/// leaves `out` empty when any of the three is missing.
pub fn identifier(info: &CpuInfo, out: &mut String) -> bool {
    out.clear();
    let (Some(family), Some(model), Some(stepping)) = (info.family, info.model, info.stepping)
    else {
        return false;
    };
    let _ = write!(out, "Family {family} Model {model} Stepping {stepping}");
    true
}

fn parse_num<T: std::str::FromStr>(value: &[u8]) -> Option<T> {
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn text(value: &[u8], scratch: &mut String) -> Option<Arc<str>> {
    sanitize_text(value, VALUE_MAX, scratch);
    let trimmed = scratch.trim();
    (!trimmed.is_empty()).then(|| Arc::from(trimmed))
}

fn parse_flags(value: &[u8]) -> CpuFlags {
    let mut flags = CpuFlags::empty();
    for name in value.split(|&b| b == b' ') {
        flags |= match name {
            b"svm" => CpuFlags::SVM,
            b"vmx" => CpuFlags::VMX,
            b"hypervisor" => CpuFlags::HYPERVISOR,
            b"sme" => CpuFlags::SME,
            b"user_shstk" => CpuFlags::USER_SHSTK,
            b"avx512f" => CpuFlags::AVX512F,
            _ => CpuFlags::empty(),
        };
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    const DMI_DESKTOP: &[u8] = include_bytes!("../../tests/fixtures/dmi-desktop/proc/cpuinfo");
    const BOGOMIPS_ONLY: &[u8] = include_bytes!("../../tests/fixtures/bogomips-only/proc/cpuinfo");

    fn parse(bytes: &[u8]) -> CpuInfo {
        let mut info = CpuInfo::default();
        parse_cpuinfo(bytes, &mut info);
        info
    }

    #[test]
    fn amd_two_blocks() {
        let info = parse(DMI_DESKTOP);
        assert_eq!(info.vendor_id.as_deref(), Some("AuthenticAMD"));
        assert_eq!(
            info.model_name.as_deref(),
            Some("AMD Ryzen 9 7950X 16-Core Processor")
        );
        assert_eq!(info.family, Some(25));
        assert_eq!(info.model, Some(97));
        assert_eq!(info.stepping, Some(2));
        assert_eq!(info.microcode.as_deref(), Some("0xa601206"));
        assert_eq!(info.mhz, Some(4500.0));
        assert!(!info.bogomips_only);
        assert_eq!(info.logical_count, 2);
        assert_eq!(info.flags, CpuFlags::SVM | CpuFlags::AVX512F);
    }

    #[test]
    fn identifier_text() {
        let mut out = String::from("stale");
        assert!(identifier(&parse(DMI_DESKTOP), &mut out));
        assert_eq!(out, "Family 25 Model 97 Stepping 2");

        let mut missing = parse(DMI_DESKTOP);
        missing.stepping = None;
        assert!(!identifier(&missing, &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn bogomips_only() {
        let info = parse(BOGOMIPS_ONLY);
        assert_eq!(info.mhz, None);
        assert!(info.bogomips_only);
        assert_eq!(info.logical_count, 2);
    }

    #[test]
    fn bad_number_none() {
        let info = parse(b"processor\t: 0\ncpu family\t: abc\nmodel\t\t: 97\n");
        assert_eq!(info.family, None);
        assert_eq!(info.model, Some(97));
    }

    #[test]
    fn reuse_resets_and_sanitizes() {
        let mut info = parse(DMI_DESKTOP);
        parse_cpuinfo(
            "model name\t: A\x1b[31mB\u{202e}C  \n".as_bytes(),
            &mut info,
        );
        assert_eq!(info.model_name.as_deref(), Some("A\u{FFFD}[31mBC"));
        assert_eq!(info.vendor_id, None);
        assert_eq!(info.logical_count, 0);
        assert_eq!(info.flags, CpuFlags::empty());
    }
}
