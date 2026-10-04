//! CPU clock from /sys/devices/system/cpu/cpufreq/policy<N>/, with /proc/cpuinfo as the
//! fallback (PERF-028, PERF-033, PERF-040, ADR-016).
//!
//! Each policy's chosen clock file is kept open and re-read with `pread` every tick. The
//! cpuinfo fallback is read with a fresh open every tick, because it is not on the ADR-016
//! kept-open list.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use smallvec::SmallVec;
use tl_model::validity::Validity;

use crate::counters::Measured;
use crate::parse::topology::parse_cpu_list;
use crate::sysroot::{KeptFile, read_file, read_trimmed, read_u64};
use crate::{CollectError, SysRoot};

const CPUFREQ_DIR: &str = "devices/system/cpu/cpufreq";

/// Most bytes read from /proc/cpuinfo.
const CPUINFO_LIMIT: usize = 1 << 20;

const FREQ_ERROR: CollectError = CollectError::Parse {
    file: "cpufreq",
    what: "bad frequency",
};

/// Where the clock values come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FreqSource {
    /// `cpuinfo_avg_freq` in every policy.
    AvgFreq,
    /// `scaling_cur_freq` in at least one policy.
    ScalingCur,
    /// `cpu MHz` lines in /proc/cpuinfo.
    CpuinfoMhz,
    /// `BogoMIPS` lines in /proc/cpuinfo. The UI marks the value with "(?)" (PERF-033).
    BogoMips,
    /// Nothing readable.
    #[default]
    None,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FreqReading {
    /// `FreqSource::None` when nothing is readable.
    pub source: FreqSource,
    /// Rounded mean over the policies (or over the cpuinfo entries) that read `Valid`.
    pub avg_mhz: Measured<u32>,
    /// Fastest policy, for the Speed tooltip.
    pub max_mhz: Measured<u32>,
    /// Index is the logical CPU id; the length is the highest id + 1.
    pub per_cpu_mhz: Vec<Measured<u32>>,
}

/// The kept-open clock files, one per cpufreq policy.
pub struct CpuFreq {
    policies: Vec<Policy>,
    /// Highest CPU id in any policy + 1.
    cpu_count: usize,
    cpuinfo_path: PathBuf,
    source: FreqSource,
    buf: Vec<u8>,
}

struct Policy {
    #[allow(dead_code)]
    id: u16,
    cpus: SmallVec<[u16; 4]>,
    file: KeptFile,
    source: FreqSource,
}

impl CpuFreq {
    /// Lists the policy directories and picks one clock file per policy. Without any policy,
    /// /proc/cpuinfo is the source.
    pub fn open(root: &SysRoot) -> Self {
        let dir = root.sys_path(CPUFREQ_DIR);
        let ids = policy_ids(&dir);

        let mut buf = Vec::new();
        let mut policies = Vec::with_capacity(ids.len());
        for id in ids {
            let pdir = dir.join(format!("policy{id}"));
            let cpus = policy_cpus(&pdir, &mut buf);
            let mut avg = KeptFile::new(pdir.join("cpuinfo_avg_freq"));
            let (file, source) = if avg.read(&mut buf).is_ok() {
                (avg, FreqSource::AvgFreq)
            } else {
                (
                    KeptFile::new(pdir.join("scaling_cur_freq")),
                    FreqSource::ScalingCur,
                )
            };
            policies.push(Policy {
                id,
                cpus,
                file,
                source,
            });
        }

        let cpu_count = cpu_count(policies.iter().map(|p| &p.cpus));
        let source = if policies.is_empty() {
            FreqSource::None
        } else if policies.iter().all(|p| p.source == FreqSource::AvgFreq) {
            FreqSource::AvgFreq
        } else {
            FreqSource::ScalingCur
        };
        let mut freq = Self {
            policies,
            cpu_count,
            cpuinfo_path: root.proc_path("cpuinfo"),
            source,
            buf,
        };
        if freq.policies.is_empty() {
            // Sets `source` for the cpuinfo fallback.
            freq.read(&mut FreqReading::default());
        }
        freq
    }

    /// Reads every policy once (one `pread` each), or /proc/cpuinfo in the fallback case.
    /// Allocates nothing once `out.per_cpu_mhz` has its length.
    pub fn read(&mut self, out: &mut FreqReading) {
        if self.policies.is_empty() {
            self.read_cpuinfo(out);
            return;
        }
        out.source = self.source;
        let unsupported = Measured::invalid(Validity::Unsupported);
        out.per_cpu_mhz.clear();
        out.per_cpu_mhz.resize(self.cpu_count, unsupported);

        let mut sum = 0u64;
        let mut count = 0u64;
        let mut max = 0u32;
        let mut first_error = None;
        for policy in &mut self.policies {
            let value = match policy.file.read(&mut self.buf) {
                Ok(_) => parse_khz(&self.buf).map(khz_to_mhz),
                Err(err) => Err(err),
            };
            let measured = match value {
                Ok(mhz) => {
                    sum += u64::from(mhz);
                    count += 1;
                    max = max.max(mhz);
                    Measured::valid(mhz)
                }
                Err(err) => {
                    let validity = err.validity();
                    first_error.get_or_insert(validity);
                    Measured::invalid(validity)
                }
            };
            for &cpu in &policy.cpus {
                out.per_cpu_mhz[usize::from(cpu)] = measured;
            }
        }

        if count > 0 {
            out.avg_mhz = Measured::valid(rounded_mean(sum, count));
            out.max_mhz = Measured::valid(max);
        } else {
            let validity = first_error.unwrap_or(Validity::Unsupported);
            out.avg_mhz = Measured::invalid(validity);
            out.max_mhz = Measured::invalid(validity);
        }
    }

    pub fn source(&self) -> FreqSource {
        self.source
    }

    fn read_cpuinfo(&mut self, out: &mut FreqReading) {
        out.per_cpu_mhz.clear();
        self.source = FreqSource::None;
        if read_file(&self.cpuinfo_path, &mut self.buf, CPUINFO_LIMIT).is_ok() {
            scan_cpuinfo(&self.buf, b"cpu MHz", &mut out.per_cpu_mhz);
            if !out.per_cpu_mhz.is_empty() {
                self.source = FreqSource::CpuinfoMhz;
            } else {
                scan_cpuinfo(&self.buf, b"BogoMIPS", &mut out.per_cpu_mhz);
                if !out.per_cpu_mhz.is_empty() {
                    self.source = FreqSource::BogoMips;
                }
            }
        }
        out.source = self.source;

        let mut sum = 0u64;
        let mut count = 0u64;
        let mut max = 0u32;
        for m in out.per_cpu_mhz.iter().filter(|m| m.validity.is_valid()) {
            sum += u64::from(m.value);
            count += 1;
            max = max.max(m.value);
        }
        if count > 0 {
            out.avg_mhz = Measured::valid(rounded_mean(sum, count));
            out.max_mhz = Measured::valid(max);
        } else if out.per_cpu_mhz.is_empty() {
            out.avg_mhz = Measured::invalid(Validity::Unsupported);
            out.max_mhz = Measured::invalid(Validity::Unsupported);
        } else {
            out.avg_mhz = Measured::invalid(Validity::Failed);
            out.max_mhz = Measured::invalid(Validity::Failed);
        }
    }
}

/// Base speed in MHz (PERF-033), read once.
///
/// The first file that reads: `cpufreq/policy0/base_frequency` (kHz),
/// `cpu0/acpi_cppc/nominal_freq` (MHz), `cpufreq/policy0/bios_limit` (kHz).
pub fn read_base_mhz(root: &SysRoot) -> Measured<u32> {
    let mut buf = Vec::new();
    let mut read = |rel: &str| read_u64(&root.sys_path(rel), &mut buf).ok();
    if let Some(khz) = read("devices/system/cpu/cpufreq/policy0/base_frequency") {
        return Measured::valid(khz_to_mhz(khz));
    }
    if let Some(mhz) = read("devices/system/cpu/cpu0/acpi_cppc/nominal_freq") {
        return Measured::valid(u32::try_from(mhz).unwrap_or(u32::MAX));
    }
    if let Some(khz) = read("devices/system/cpu/cpufreq/policy0/bios_limit") {
        return Measured::valid(khz_to_mhz(khz));
    }
    Measured::invalid(Validity::Unsupported)
}

/// Static cpufreq facts for the CPU page (PERF-035, PERF-031), read once at start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuFreqStatic {
    /// `scaling_driver` of the policy covering cpu0.
    pub driver: Measured<Option<Arc<str>>>,
    /// `scaling_governor` of the policy covering cpu0.
    pub governor: Measured<Option<Arc<str>>>,
    /// `energy_performance_preference` of the policy covering cpu0.
    pub epp: Measured<Option<Arc<str>>>,
    /// /sys/firmware/acpi/platform_profile. The power-profiles-daemon fallback is the
    /// engine's job (E12).
    pub platform_profile: Measured<Option<Arc<str>>>,
    /// Index is the logical CPU id; from `policy<N>/amd_pstate_prefcore_ranking`.
    pub prefcore_rank: Vec<Option<u32>>,
}

impl CpuFreqStatic {
    /// True when at least two different ranking values exist (PERF-031: stars only then).
    pub fn prefcore_differs(&self) -> bool {
        let mut ranks = self.prefcore_rank.iter().flatten();
        match ranks.next() {
            Some(first) => ranks.any(|rank| rank != first),
            None => false,
        }
    }

    /// Highest ranking value, if any.
    pub fn top_rank(&self) -> Option<u32> {
        self.prefcore_rank.iter().flatten().copied().max()
    }
}

/// Reads the static cpufreq facts once (PERF-035, PERF-031).
///
/// The string fields come from the policy whose `related_cpus` (else `affected_cpus`)
/// contains cpu0. A missing file gives `Unsupported`.
pub fn read_static(root: &SysRoot) -> CpuFreqStatic {
    let dir = root.sys_path(CPUFREQ_DIR);
    let mut buf = Vec::new();
    let policies: Vec<(PathBuf, SmallVec<[u16; 4]>)> = policy_ids(&dir)
        .into_iter()
        .map(|id| {
            let pdir = dir.join(format!("policy{id}"));
            let cpus = policy_cpus(&pdir, &mut buf);
            (pdir, cpus)
        })
        .collect();

    let mut out = CpuFreqStatic {
        driver: Measured::invalid(Validity::Unsupported),
        governor: Measured::invalid(Validity::Unsupported),
        epp: Measured::invalid(Validity::Unsupported),
        platform_profile: read_string(&root.sys_path("firmware/acpi/platform_profile"), &mut buf),
        prefcore_rank: vec![None; cpu_count(policies.iter().map(|(_, cpus)| cpus))],
    };
    if let Some((pdir, _)) = policies.iter().find(|(_, cpus)| cpus.contains(&0)) {
        out.driver = read_string(&pdir.join("scaling_driver"), &mut buf);
        out.governor = read_string(&pdir.join("scaling_governor"), &mut buf);
        out.epp = read_string(&pdir.join("energy_performance_preference"), &mut buf);
    }
    for (pdir, cpus) in &policies {
        let Ok(rank) = read_u64(&pdir.join("amd_pstate_prefcore_ranking"), &mut buf) else {
            continue;
        };
        let Ok(rank) = u32::try_from(rank) else {
            continue;
        };
        for &cpu in cpus {
            out.prefcore_rank[usize::from(cpu)] = Some(rank);
        }
    }
    out
}

/// A small text file, trimmed, as lossy UTF-8. A missing file gives `Unsupported`.
fn read_string(path: &Path, buf: &mut Vec<u8>) -> Measured<Option<Arc<str>>> {
    match read_trimmed(path, buf) {
        Ok(bytes) => Measured::valid(Some(Arc::from(String::from_utf8_lossy(bytes)))),
        Err(CollectError::NotFound) => Measured::invalid(Validity::Unsupported),
        Err(err) => Measured::invalid(err.validity()),
    }
}

/// The ids of the `policy<N>` directories in `dir`, sorted. Empty when `dir` does not read.
fn policy_ids(dir: &Path) -> Vec<u16> {
    let mut ids: Vec<u16> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter_map(|e| policy_id(e.file_name().as_encoded_bytes()))
            .collect(),
        Err(_) => Vec::new(),
    };
    ids.sort_unstable();
    ids
}

/// The CPUs of one policy: `related_cpus`, else `affected_cpus`, else none.
fn policy_cpus(pdir: &Path, buf: &mut Vec<u8>) -> SmallVec<[u16; 4]> {
    let mut list = SmallVec::<[u16; 64]>::new();
    for name in ["related_cpus", "affected_cpus"] {
        let parsed =
            read_trimmed(&pdir.join(name), buf).and_then(|bytes| parse_cpu_list(bytes, &mut list));
        if parsed.is_ok() && !list.is_empty() {
            return list.iter().copied().collect();
        }
    }
    SmallVec::new()
}

/// Highest CPU id in any policy + 1, or 0.
fn cpu_count<'a>(policies: impl Iterator<Item = &'a SmallVec<[u16; 4]>>) -> usize {
    policies
        .flat_map(|cpus| cpus.iter())
        .map(|&cpu| usize::from(cpu) + 1)
        .max()
        .unwrap_or(0)
}

/// Clears `out`, then pushes one entry per line whose key (the text before `:`, trimmed)
/// equals `key` ignoring ASCII case. The value is a decimal such as `3400.000`, rounded to the
/// nearest integer; a value that does not parse gives a `Failed` entry.
fn scan_cpuinfo(text: &[u8], key: &[u8], out: &mut Vec<Measured<u32>>) {
    out.clear();
    for line in text.split(|&b| b == b'\n') {
        let Some(colon) = memchr::memchr(b':', line) else {
            continue;
        };
        if !line[..colon].trim_ascii().eq_ignore_ascii_case(key) {
            continue;
        }
        out.push(match parse_decimal(line[colon + 1..].trim_ascii()) {
            Some(value) => Measured::valid(value),
            None => Measured::invalid(Validity::Failed),
        });
    }
}

/// A non-negative decimal rounded to the nearest u32, or None.
fn parse_decimal(s: &[u8]) -> Option<u32> {
    let v: f64 = std::str::from_utf8(s).ok()?.parse().ok()?;
    if !v.is_finite() || v < 0.0 || v >= f64::from(u32::MAX) {
        return None;
    }
    // In range, so the cast is exact after rounding.
    Some(v.round() as u32)
}

/// "policy<N>" to N.
fn policy_id(name: &[u8]) -> Option<u16> {
    let digits = name.strip_prefix(b"policy")?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

fn parse_khz(buf: &[u8]) -> Result<u64, CollectError> {
    std::str::from_utf8(buf.trim_ascii())
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(FREQ_ERROR)
}

fn khz_to_mhz(khz: u64) -> u32 {
    u32::try_from(khz.saturating_add(500) / 1000).unwrap_or(u32::MAX)
}

fn rounded_mean(sum: u64, count: u64) -> u32 {
    u32::try_from((sum + count / 2) / count).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const OWNER_DESKTOP: &[u8] = include_bytes!("../../tests/fixtures/owner-desktop/proc/cpuinfo");

    #[test]
    fn scan_owner_desktop() {
        let mut out = Vec::new();
        scan_cpuinfo(OWNER_DESKTOP, b"cpu MHz", &mut out);
        assert!(!out.is_empty());
        assert!(out.iter().all(|m| m.validity.is_valid()));
        scan_cpuinfo(OWNER_DESKTOP, b"bogomips", &mut out);
        assert!(!out.is_empty());
    }

    #[test]
    fn decimal_rounding() {
        assert_eq!(parse_decimal(b"3400.000"), Some(3400));
        assert_eq!(parse_decimal(b"3875.561"), Some(3876));
        assert_eq!(parse_decimal(b"50.00"), Some(50));
        assert_eq!(parse_decimal(b"-1"), None);
        assert_eq!(parse_decimal(b"x"), None);
        assert_eq!(parse_decimal(b"NaN"), None);
        assert_eq!(khz_to_mhz(4_199_500), 4200);
        assert_eq!(khz_to_mhz(4_199_499), 4199);
        assert_eq!(khz_to_mhz(u64::MAX), u32::MAX);
        assert_eq!(policy_id(b"policy12"), Some(12));
        assert_eq!(policy_id(b"policy"), None);
        assert_eq!(policy_id(b"policy1a"), None);
    }

    #[test]
    fn prefcore_helpers() {
        let info = |ranks: Vec<Option<u32>>| CpuFreqStatic {
            prefcore_rank: ranks,
            ..CpuFreqStatic::default()
        };
        assert!(!info(vec![]).prefcore_differs());
        assert!(!info(vec![None, Some(5), None, Some(5)]).prefcore_differs());
        assert!(info(vec![Some(5), None, Some(7)]).prefcore_differs());
        assert_eq!(info(vec![Some(5), None, Some(7)]).top_rank(), Some(7));
        assert_eq!(info(vec![None, None]).top_rank(), None);
    }

    proptest! {
        #[test]
        fn prop_cpuinfo_scan_never_panics(
            arbitrary in prop::collection::vec(any::<u8>(), 0..=8192),
            cut in 0..=OWNER_DESKTOP.len(),
        ) {
            let mut out = Vec::new();
            for text in [&arbitrary[..], &OWNER_DESKTOP[..cut]] {
                for key in [&b"cpu MHz"[..], &b"BogoMIPS"[..]] {
                    scan_cpuinfo(text, key, &mut out);
                    prop_assert!(out.len() <= text.len());
                }
            }
        }
    }
}
