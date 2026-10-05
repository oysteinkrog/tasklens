//! /proc/cpuinfo for the Information tab's Processor section and the CPU page's Virtualization
//! and Microcode rows (PERF-096, PERF-035, PERF-037, ADR-016 "Information").
//!
//! Cores, logical processors, base and max speed come from the topology and cpufreq readers.
//! This module only reads the identity fields, the first `cpu MHz` and the flags we show.
//!
//! `read_security` reads the Virtualization, Hypervisor and Security features sources and the
//! CPU vulnerability summary (PERF-037, PERF-101, U-27, R-056).

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use smallvec::SmallVec;

use crate::CollectError;
use crate::sanitize::sanitize_text;
use crate::sysroot::{SMALL_FILE_LIMIT, read_file, read_trimmed};

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Virtualization {
    EnabledKvm,
    SupportedNoKvm,
    NotSupported,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lockdown {
    None,
    Integrity,
    Confidentiality,
}

/// Files in `devices/system/cpu/vulnerabilities/`, counted by the start of their text.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct VulnCounts {
    pub mitigated: u16,
    pub vulnerable: u16,
    pub not_affected: u16,
    pub unknown: u16,
}

/// A source the Security features tooltip names when it could not be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SecuritySource {
    SecureBoot,
    KernelLockdown,
    MemoryEncryption,
    IommuSysfs,
    Vulnerabilities,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SecurityInfo {
    pub virtualization: Virtualization,
    /// `sys/hypervisor/type`, or "unknown" when only the flag is set. `None` when no flag.
    pub hypervisor: Option<Arc<str>>,
    pub secure_boot: Option<bool>,
    pub lockdown: Option<Lockdown>,
    pub memory_encryption: Option<bool>,
    pub user_shadow_stack: bool,
    pub iommu: Option<bool>,
    pub vulns: Option<VulnCounts>,
    /// Sources that gave EACCES or EPERM.
    pub unreadable: SmallVec<[SecuritySource; 4]>,
}

/// Roots for `read_security`: `/sys`, `/proc` and `/dev` on the host, or fixture folders.
pub struct SecurityPaths<'a> {
    pub sys: &'a Path,
    pub proc: &'a Path,
    pub dev: &'a Path,
}

/// The EFI global variable GUID (PERF-037).
pub const EFI_GLOBAL_GUID: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";

/// Reads the CPU page's security sources. Never fails: a source that gives EACCES or EPERM
/// leaves its field `None` (or counts as unknown) and is pushed to `unreadable`.
pub fn read_security(cpu: &CpuInfo, p: &SecurityPaths<'_>) -> SecurityInfo {
    let flags = cpu.flags;
    let mut unreadable = SmallVec::new();
    let mut buf = Vec::new();
    let mut scratch = String::new();

    let virtualization = if !flags.intersects(CpuFlags::SVM | CpuFlags::VMX) {
        Virtualization::NotSupported
    } else if p.dev.join("kvm").exists() {
        Virtualization::EnabledKvm
    } else {
        Virtualization::SupportedNoKvm
    };

    let hypervisor = flags.contains(CpuFlags::HYPERVISOR).then(|| {
        read_trimmed(&p.sys.join("hypervisor/type"), &mut buf)
            .ok()
            .and_then(|value| text(value, &mut scratch))
            .unwrap_or_else(|| Arc::from("unknown"))
    });

    let efivar = format!("firmware/efi/efivars/SecureBoot-{EFI_GLOBAL_GUID}");
    let secure_boot = match read_file(&p.sys.join(efivar), &mut buf, SMALL_FILE_LIMIT) {
        Ok(_) => match buf.get(4) {
            Some(1) => Some(true),
            Some(0) => Some(false),
            _ => None,
        },
        Err(err) => denied(err, SecuritySource::SecureBoot, &mut unreadable),
    };

    let lockdown = match read_trimmed(&p.sys.join("kernel/security/lockdown"), &mut buf) {
        Ok(value) => parse_lockdown(value),
        Err(err) => denied(err, SecuritySource::KernelLockdown, &mut unreadable),
    };

    let memory_encryption = if flags.contains(CpuFlags::SME) {
        match read_file(&p.proc.join("cmdline"), &mut buf, SMALL_FILE_LIMIT) {
            Ok(_) => Some(
                buf.split(|b| b.is_ascii_whitespace())
                    .any(|word| word == b"mem_encrypt=on"),
            ),
            Err(err) => {
                denied::<()>(err, SecuritySource::MemoryEncryption, &mut unreadable);
                Some(false)
            }
        }
    } else {
        None
    };

    let iommu = match std::fs::read_dir(p.sys.join("class/iommu")) {
        Ok(mut entries) => Some(entries.next().is_some()),
        Err(err) => match CollectError::from(err) {
            CollectError::NotFound => Some(false),
            err => denied(err, SecuritySource::IommuSysfs, &mut unreadable),
        },
    };

    let vulns = read_vulns(
        &p.sys.join("devices/system/cpu/vulnerabilities"),
        &mut buf,
        &mut unreadable,
    );

    SecurityInfo {
        virtualization,
        hypervisor,
        secure_boot,
        lockdown,
        memory_encryption,
        user_shadow_stack: flags.contains(CpuFlags::USER_SHSTK),
        iommu,
        vulns,
        unreadable,
    }
}

/// Always `None`. Pushes `source` when the error is EACCES or EPERM.
fn denied<T>(
    err: CollectError,
    source: SecuritySource,
    unreadable: &mut SmallVec<[SecuritySource; 4]>,
) -> Option<T> {
    if matches!(err, CollectError::PermissionDenied) && !unreadable.contains(&source) {
        unreadable.push(source);
    }
    None
}

/// The bracketed word of `none [integrity] confidentiality`.
fn parse_lockdown(value: &[u8]) -> Option<Lockdown> {
    let start = value.iter().position(|&b| b == b'[')? + 1;
    let len = value[start..].iter().position(|&b| b == b']')?;
    match &value[start..start + len] {
        b"none" => Some(Lockdown::None),
        b"integrity" => Some(Lockdown::Integrity),
        b"confidentiality" => Some(Lockdown::Confidentiality),
        _ => None,
    }
}

fn read_vulns(
    dir: &Path,
    buf: &mut Vec<u8>,
    unreadable: &mut SmallVec<[SecuritySource; 4]>,
) -> Option<VulnCounts> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => return denied(err.into(), SecuritySource::Vulnerabilities, unreadable),
    };
    let mut counts = VulnCounts::default();
    for entry in entries.flatten() {
        let slot = match read_file(&entry.path(), buf, SMALL_FILE_LIMIT) {
            Ok(_) if buf.starts_with(b"Not affected") => &mut counts.not_affected,
            Ok(_) if buf.starts_with(b"Mitigation") => &mut counts.mitigated,
            Ok(_) if buf.starts_with(b"Vulnerable") => &mut counts.vulnerable,
            Ok(_) => &mut counts.unknown,
            Err(err) => {
                denied::<()>(err, SecuritySource::Vulnerabilities, unreadable);
                &mut counts.unknown
            }
        };
        *slot = slot.saturating_add(1);
    }
    Some(counts)
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

    fn fixture(rel: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/dmi-desktop")
            .join(rel)
    }

    fn read_fixture(cpu: &CpuInfo) -> SecurityInfo {
        let (sys, proc, dev) = (fixture("sys"), fixture("proc"), fixture("dev"));
        read_security(
            cpu,
            &SecurityPaths {
                sys: &sys,
                proc: &proc,
                dev: &dev,
            },
        )
    }

    fn with_flags(flags: CpuFlags) -> CpuInfo {
        CpuInfo {
            flags,
            ..CpuInfo::default()
        }
    }

    #[test]
    fn virtualization_states() {
        let cpu = parse(DMI_DESKTOP);
        assert_eq!(
            read_fixture(&cpu).virtualization,
            Virtualization::EnabledKvm
        );

        let (sys, proc, missing) = (fixture("sys"), fixture("proc"), fixture("no-dev"));
        let paths = SecurityPaths {
            sys: &sys,
            proc: &proc,
            dev: &missing,
        };
        assert_eq!(
            read_security(&cpu, &paths).virtualization,
            Virtualization::SupportedNoKvm
        );
        assert_eq!(
            read_fixture(&with_flags(CpuFlags::VMX)).virtualization,
            Virtualization::EnabledKvm
        );
        assert_eq!(
            read_fixture(&CpuInfo::default()).virtualization,
            Virtualization::NotSupported
        );
    }

    #[test]
    fn hypervisor_and_flags() {
        let plain = read_fixture(&CpuInfo::default());
        assert_eq!(plain.hypervisor, None);
        assert_eq!(plain.memory_encryption, None);
        assert!(!plain.user_shadow_stack);

        let info = read_fixture(&with_flags(
            CpuFlags::HYPERVISOR | CpuFlags::SME | CpuFlags::USER_SHSTK,
        ));
        assert_eq!(info.hypervisor.as_deref(), Some("xen"));
        assert_eq!(info.memory_encryption, Some(true));
        assert!(info.user_shadow_stack);
        assert!(info.unreadable.is_empty());

        let (empty, dev) = (fixture("no-sys"), fixture("dev"));
        let paths = SecurityPaths {
            sys: &empty,
            proc: &empty,
            dev: &dev,
        };
        let bare = read_security(&with_flags(CpuFlags::HYPERVISOR | CpuFlags::SME), &paths);
        assert_eq!(bare.hypervisor.as_deref(), Some("unknown"));
        assert_eq!(bare.memory_encryption, Some(false));
        assert_eq!(bare.secure_boot, None);
        assert_eq!(bare.lockdown, None);
        assert_eq!(bare.iommu, Some(false));
        assert_eq!(bare.vulns, None);
        assert!(bare.unreadable.is_empty());
    }

    #[test]
    fn secure_boot_byte4() {
        assert_eq!(read_fixture(&CpuInfo::default()).secure_boot, Some(true));
    }

    #[test]
    fn lockdown_bracket() {
        assert_eq!(
            read_fixture(&CpuInfo::default()).lockdown,
            Some(Lockdown::Integrity)
        );
        assert_eq!(parse_lockdown(b"[none] integrity"), Some(Lockdown::None));
        assert_eq!(
            parse_lockdown(b"none integrity [confidentiality]"),
            Some(Lockdown::Confidentiality)
        );
        assert_eq!(parse_lockdown(b"none integrity"), None);
    }

    #[test]
    fn vuln_counts() {
        assert_eq!(
            read_fixture(&CpuInfo::default()).vulns,
            Some(VulnCounts {
                mitigated: 1,
                vulnerable: 1,
                not_affected: 1,
                unknown: 1,
            })
        );
    }

    #[test]
    fn iommu_entries() {
        assert_eq!(read_fixture(&CpuInfo::default()).iommu, Some(true));
    }

    #[test]
    fn denied_sources_named() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let sys = root.path();
        let lockdown = sys.join("kernel/security/lockdown");
        let vulns = sys.join("devices/system/cpu/vulnerabilities");
        std::fs::create_dir_all(lockdown.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&vulns).unwrap();
        std::fs::write(&lockdown, "[none]").unwrap();
        std::fs::write(vulns.join("meltdown"), "Not affected").unwrap();
        std::fs::write(vulns.join("spectre_v1"), "Mitigation: X").unwrap();
        std::fs::write(vulns.join("spectre_v2"), "Mitigation: Y").unwrap();
        for file in [
            &lockdown,
            &vulns.join("spectre_v1"),
            &vulns.join("spectre_v2"),
        ] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        if std::fs::read(&lockdown).is_ok() {
            // Root ignores file modes, so there is nothing to test.
            return;
        }
        let paths = SecurityPaths {
            sys,
            proc: sys,
            dev: sys,
        };
        let info = read_security(&CpuInfo::default(), &paths);
        assert_eq!(info.lockdown, None);
        assert_eq!(
            info.vulns,
            Some(VulnCounts {
                not_affected: 1,
                unknown: 2,
                ..VulnCounts::default()
            })
        );
        assert_eq!(
            info.unreadable.as_slice(),
            [
                SecuritySource::KernelLockdown,
                SecuritySource::Vulnerabilities
            ]
        );
    }
}
